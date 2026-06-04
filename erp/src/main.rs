use aes_gcm::{
    Aes128Gcm, Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand, ValueEnum};
use dialoguer::{Confirm, Input, Password, Select, theme::ColorfulTheme};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use socket2::{Domain, Protocol as SocketProtocol, Socket, Type};
use std::{
    collections::{HashMap, HashSet},
    env, io,
    net::{SocketAddr, ToSocketAddrs},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{Mutex, Semaphore, mpsc, oneshot},
    time::{Duration, sleep, timeout},
};
use tracing::{debug, info, warn};

type HmacSha256 = Hmac<Sha256>;
type BoxedStream = Box<dyn TunnelStream>;
const DIRECT_AUTH_WINDOW_SECS: u64 = 300;
const AES_NONCE_LEN: usize = 12;
const DEFAULT_RAW_AES_INFO: &[u8] = b"erp-session-raw-aes-256-gcm-v2";
#[cfg(unix)]
const DEFAULT_NOFILE_LIMIT: u64 = 1_048_576;
const DEFAULT_CONTROL_CHANNEL_CAPACITY: usize = 262_144;
const DEFAULT_LISTEN_BACKLOG: i32 = 65_535;
const DEFAULT_MAX_PENDING_CONNECTIONS: usize = 262_144;
const DEFAULT_MAX_AUTH_HANDSHAKES: usize = 65_536;
const DEFAULT_AUTH_TIMEOUT: Duration = Duration::from_secs(3);
const DEFAULT_DATA_SESSION_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const ACCEPT_FD_BACKOFF: Duration = Duration::from_millis(100);

trait TunnelStream: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T> TunnelStream for T where T: AsyncRead + AsyncWrite + Send + Unpin + 'static {}

#[derive(Parser)]
#[command(name = "erp", version, about = "easy-to-use reverse proxy")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Clone)]
enum Command {
    Server {
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
    Client {
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Role {
    Server,
    Client,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Transport {
    Raw,
    #[serde(rename = "aes-128-gcm")]
    Aes128Gcm,
    #[serde(rename = "aes-256-gcm")]
    Aes256Gcm,
    #[serde(alias = "tls")]
    Tls,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
enum Protocol {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum UdpMode {
    Direct,
    OverTcp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Config {
    role: Role,
    token: String,
    transport: Transport,
    server: Option<ServerConfig>,
    client: Option<ClientConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServerConfig {
    bind_addr: String,
    control_port: u16,
    #[serde(default)]
    tls_cert_path: Option<PathBuf>,
    #[serde(default)]
    tls_key_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClientConfig {
    server_addr: String,
    client_id: String,
    #[serde(default)]
    tls_server_name: Option<String>,
    #[serde(default)]
    tls_ca_cert_path: Option<PathBuf>,
    mappings: Vec<MappingConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MappingConfig {
    name: String,
    protocol: Protocol,
    local_addr: String,
    remote_port: u16,
    #[serde(default)]
    udp_mode: Option<UdpMode>,
}

#[derive(Debug, Serialize, Deserialize)]
enum Frame {
    AuthChallenge {
        nonce: Vec<u8>,
        key_rounds: u8,
    },
    AuthProof {
        client_id: String,
        timestamp: u64,
        client_nonce: Vec<u8>,
        proof: Vec<u8>,
    },
    AuthOk,
    AuthFailed {
        message: String,
    },
    Register {
        mappings: Vec<MappingConfig>,
    },
    RegisterOk,
    RegisterFailed {
        message: String,
    },
    OpenTcp {
        conn_id: u64,
        remote_port: u16,
    },
    OpenUdp {
        conn_id: u64,
        remote_port: u16,
        payload: Vec<u8>,
        peer: SocketAddr,
    },
    UdpResponse {
        remote_port: u16,
        payload: Vec<u8>,
        peer: SocketAddr,
    },
    DataStart {
        conn_id: u64,
    },
    Data {
        payload: Vec<u8>,
    },
    DataEnd,
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum UdpDatagram {
    Hello {
        key_rounds: u8,
        client_id: String,
        remote_port: u16,
        timestamp: u64,
        nonce: Vec<u8>,
        proof: Vec<u8>,
    },
    Packet {
        key_rounds: u8,
        remote_port: u16,
        peer: SocketAddr,
        payload: Vec<u8>,
        timestamp: u64,
        nonce: Vec<u8>,
        proof: Vec<u8>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
enum DirectUdpFrame {
    Encrypted {
        key_rounds: u8,
        nonce: Vec<u8>,
        ciphertext: Vec<u8>,
    },
}

struct DirectProofRef<'a> {
    timestamp: u64,
    nonce: &'a [u8],
    proof: &'a [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    ClientToServer,
    ServerToClient,
}

#[derive(Clone)]
enum SessionCrypto {
    Aes128(Box<Aes128Gcm>),
    Aes256(Box<Aes256Gcm>),
}

struct FrameReader<R> {
    reader: R,
    crypto: SessionCrypto,
    direction: Direction,
    counter: u64,
}

struct FrameWriter<W> {
    writer: W,
    crypto: SessionCrypto,
    direction: Direction,
    counter: u64,
}

impl<R> FrameReader<R>
where
    R: AsyncRead + Unpin,
{
    fn new(reader: R, crypto: SessionCrypto, direction: Direction, counter: u64) -> Self {
        Self {
            reader,
            crypto,
            direction,
            counter,
        }
    }

    async fn read_frame(&mut self) -> Result<Frame> {
        let payload = read_payload(&mut self.reader).await?;
        let payload = decrypt_payload(&self.crypto, self.direction, self.counter, &payload)?;
        self.counter += 1;
        Ok(bincode::deserialize(&payload)?)
    }
}

impl<W> FrameWriter<W>
where
    W: AsyncWrite + Unpin,
{
    fn new(writer: W, crypto: SessionCrypto, direction: Direction, counter: u64) -> Self {
        Self {
            writer,
            crypto,
            direction,
            counter,
        }
    }

    async fn write_frame(&mut self, frame: &Frame) -> Result<()> {
        let payload = bincode::serialize(frame)?;
        let payload = encrypt_payload(&self.crypto, self.direction, self.counter, &payload)?;
        self.counter += 1;
        write_payload(&mut self.writer, &payload).await
    }
}

async fn read_session_frame<R>(
    reader: &mut R,
    crypto: &SessionCrypto,
    direction: Direction,
    counter: &mut u64,
) -> Result<Frame>
where
    R: AsyncRead + Unpin,
{
    let payload = read_payload(reader).await?;
    let payload = decrypt_payload(crypto, direction, *counter, &payload)?;
    *counter += 1;
    Ok(bincode::deserialize(&payload)?)
}

async fn write_session_frame<W>(
    writer: &mut W,
    crypto: &SessionCrypto,
    direction: Direction,
    counter: &mut u64,
    frame: &Frame,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let payload = bincode::serialize(frame)?;
    let payload = encrypt_payload(crypto, direction, *counter, &payload)?;
    *counter += 1;
    write_payload(writer, &payload).await
}

struct DataSession {
    stream: BoxedStream,
    crypto: SessionCrypto,
    read_counter: u64,
    write_counter: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    configure_process_limits();

    let cli = Cli::parse();
    let command = match cli.command {
        Some(command) => command,
        None => prompt_role()?,
    };

    let config_path = match &command {
        Command::Server { config } | Command::Client { config } => {
            resolve_config_path(*role_of(&command), config.clone())?
        }
    };
    let config = load_config(&config_path)?;

    match (command, config.role) {
        (Command::Server { .. }, Role::Server) => run_server(config).await,
        (Command::Client { .. }, Role::Client) => run_client(config).await,
        (_, role) => bail!(
            "config role is {:?}, but selected command does not match",
            role
        ),
    }
}

fn role_of(command: &Command) -> &Role {
    match command {
        Command::Server { .. } => &Role::Server,
        Command::Client { .. } => &Role::Client,
    }
}

fn prompt_role() -> Result<Command> {
    let choice = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select erp role")
        .items(&["client", "server"])
        .default(0)
        .interact()?;
    Ok(if choice == 0 {
        Command::Client { config: None }
    } else {
        Command::Server { config: None }
    })
}

fn resolve_config_path(role: Role, explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path);
    }

    let dir = env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or(env::current_dir()?);
    let configs = std::fs::read_dir(&dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect::<Vec<_>>();

    if configs.is_empty() {
        let path = dir.join(match role {
            Role::Server => "server.toml",
            Role::Client => "client.toml",
        });
        create_config_interactively(role, &path)?;
        return Ok(path);
    }

    let mut labels = configs
        .iter()
        .map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        })
        .collect::<Vec<_>>();
    labels.push("Create new config".to_string());
    let choice = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select config")
        .items(&labels)
        .default(0)
        .interact()?;
    if choice < configs.len() {
        Ok(configs[choice].clone())
    } else {
        let filename: String = Input::with_theme(&ColorfulTheme::default())
            .with_prompt("Config filename")
            .default(match role {
                Role::Server => "server.toml".to_string(),
                Role::Client => "client.toml".to_string(),
            })
            .interact_text()?;
        let path = dir.join(filename);
        create_config_interactively(role, &path)?;
        Ok(path)
    }
}

fn create_config_interactively(role: Role, path: &Path) -> Result<()> {
    let token = Password::with_theme(&ColorfulTheme::default())
        .with_prompt("Shared token")
        .interact()?;
    let transport_index = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Transport")
        .items(&["raw", "aes-256-gcm", "aes-128-gcm"])
        .default(0)
        .interact()?;
    let transport = match transport_index {
        0 => Transport::Raw,
        1 => Transport::Aes256Gcm,
        _ => Transport::Aes128Gcm,
    };

    let config = match role {
        Role::Server => {
            let bind_addr: String = Input::with_theme(&ColorfulTheme::default())
                .with_prompt("Server bind address")
                .default("0.0.0.0".into())
                .interact_text()?;
            let control_port: u16 = Input::with_theme(&ColorfulTheme::default())
                .with_prompt("Control port")
                .default(7000)
                .interact_text()?;
            Config {
                role,
                token,
                transport,
                server: Some(ServerConfig {
                    bind_addr,
                    control_port,
                    tls_cert_path: None,
                    tls_key_path: None,
                }),
                client: None,
            }
        }
        Role::Client => {
            let server_addr: String = Input::with_theme(&ColorfulTheme::default())
                .with_prompt("Server address host:port")
                .default("127.0.0.1:7000".into())
                .interact_text()?;
            let client_id: String = Input::with_theme(&ColorfulTheme::default())
                .with_prompt("Client id")
                .default("default".into())
                .interact_text()?;
            let mut mappings = Vec::new();
            loop {
                mappings.push(prompt_mapping_interactively(&client_id, mappings.len())?);
                if !Confirm::with_theme(&ColorfulTheme::default())
                    .with_prompt("Add another mapping")
                    .default(false)
                    .interact()?
                {
                    break;
                }
            }
            Config {
                role,
                token,
                transport,
                server: None,
                client: Some(ClientConfig {
                    server_addr,
                    client_id,
                    tls_server_name: None,
                    tls_ca_cert_path: None,
                    mappings,
                }),
            }
        }
    };

    let content = toml::to_string_pretty(&config)?;
    std::fs::write(path, content)?;
    Ok(())
}

fn prompt_mapping_interactively(client_id: &str, index: usize) -> Result<MappingConfig> {
    let default_name = if index == 0 {
        client_id.to_string()
    } else {
        format!("{client_id}-{index}")
    };
    let name: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("Mapping name")
        .default(default_name)
        .interact_text()?;
    let protocol_index = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Mapping protocol")
        .items(&["tcp", "udp"])
        .default(0)
        .interact()?;
    let protocol = if protocol_index == 0 {
        Protocol::Tcp
    } else {
        Protocol::Udp
    };
    let local_addr: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("Local service address")
        .default("127.0.0.1:8080".into())
        .interact_text()?;
    let remote_port: u16 = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("Server remote port")
        .default(18080)
        .interact_text()?;
    let udp_mode = if protocol == Protocol::Udp {
        let mode_index = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("UDP mode")
            .items(&["udp over tcp", "direct udp"])
            .default(0)
            .interact()?;
        Some(if mode_index == 0 {
            UdpMode::OverTcp
        } else {
            UdpMode::Direct
        })
    } else {
        None
    };
    Ok(MappingConfig {
        name,
        protocol,
        local_addr,
        remote_port,
        udp_mode,
    })
}

fn load_config(path: &Path) -> Result<Config> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    toml::from_str(&content).with_context(|| format!("failed to parse config {}", path.display()))
}

fn build_server_tunnel(transport: Transport, _server: &ServerConfig) -> Result<ServerTunnel> {
    match transport {
        Transport::Raw | Transport::Aes128Gcm | Transport::Aes256Gcm => {
            Ok(ServerTunnel { transport })
        }
        Transport::Tls => {
            bail!("transport = \"tls\" is deprecated; use \"aes-256-gcm\" or \"raw\"")
        }
    }
}

fn build_client_tunnel(transport: Transport, _client: &ClientConfig) -> Result<ClientTunnel> {
    match transport {
        Transport::Raw | Transport::Aes128Gcm | Transport::Aes256Gcm => {
            Ok(ClientTunnel { transport })
        }
        Transport::Tls => {
            bail!("transport = \"tls\" is deprecated; use \"aes-256-gcm\" or \"raw\"")
        }
    }
}

fn configure_process_limits() {
    #[cfg(unix)]
    configure_unix_nofile_limit();
}

#[cfg(unix)]
fn configure_unix_nofile_limit() {
    let requested = env::var("ERP_NOFILE")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_NOFILE_LIMIT);

    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    let rc = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) };
    if rc != 0 {
        warn!(
            "failed to read RLIMIT_NOFILE: {}",
            io::Error::last_os_error()
        );
        return;
    }

    let hard = limit.rlim_max;
    let target = if hard == libc::RLIM_INFINITY {
        requested as libc::rlim_t
    } else {
        (requested as libc::rlim_t).min(hard)
    };

    if target > limit.rlim_cur {
        let next = libc::rlimit {
            rlim_cur: target,
            rlim_max: limit.rlim_max,
        };
        let rc = unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &next) };
        if rc != 0 {
            warn!(
                "failed to raise RLIMIT_NOFILE from {} to {}: {}",
                limit.rlim_cur,
                target,
                io::Error::last_os_error()
            );
        } else {
            limit.rlim_cur = target;
        }
    }

    if limit.rlim_cur < requested as libc::rlim_t {
        warn!(
            "RLIMIT_NOFILE is {}; requested {}. Raise the service/user hard limit for very high concurrency.",
            limit.rlim_cur, requested
        );
    } else {
        info!("RLIMIT_NOFILE soft limit is {}", limit.rlim_cur);
    }
}

fn is_fd_exhaustion(err: &io::Error) -> bool {
    #[cfg(unix)]
    {
        matches!(err.raw_os_error(), Some(code) if code == libc::EMFILE || code == libc::ENFILE)
    }
    #[cfg(not(unix))]
    {
        let _ = err;
        false
    }
}

fn listen_backlog() -> i32 {
    static VALUE: OnceLock<i32> = OnceLock::new();
    *VALUE.get_or_init(|| {
        env::var("ERP_LISTEN_BACKLOG")
            .ok()
            .and_then(|value| value.parse::<i32>().ok())
            .map(|value| value.clamp(1, i32::MAX))
            .unwrap_or(DEFAULT_LISTEN_BACKLOG)
    })
}

fn max_pending_connections() -> usize {
    static VALUE: OnceLock<usize> = OnceLock::new();
    *VALUE.get_or_init(|| {
        env::var("ERP_MAX_PENDING")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_MAX_PENDING_CONNECTIONS)
    })
}

fn max_auth_handshakes() -> usize {
    static VALUE: OnceLock<usize> = OnceLock::new();
    *VALUE.get_or_init(|| {
        env::var("ERP_MAX_AUTH_HANDSHAKES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_MAX_AUTH_HANDSHAKES)
    })
}

fn control_channel_capacity() -> usize {
    static VALUE: OnceLock<usize> = OnceLock::new();
    *VALUE.get_or_init(|| {
        env::var("ERP_CONTROL_CHANNEL_CAPACITY")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_CONTROL_CHANNEL_CAPACITY)
    })
}

fn auth_timeout() -> Duration {
    duration_from_env("ERP_AUTH_TIMEOUT_SECS", DEFAULT_AUTH_TIMEOUT)
}

fn data_session_timeout() -> Duration {
    duration_from_env(
        "ERP_DATA_SESSION_TIMEOUT_SECS",
        DEFAULT_DATA_SESSION_TIMEOUT,
    )
}

fn connect_timeout() -> Duration {
    duration_from_env("ERP_CONNECT_TIMEOUT_SECS", DEFAULT_CONNECT_TIMEOUT)
}

fn duration_from_env(name: &'static str, default: Duration) -> Duration {
    static AUTH: OnceLock<Duration> = OnceLock::new();
    static DATA: OnceLock<Duration> = OnceLock::new();
    static CONNECT: OnceLock<Duration> = OnceLock::new();
    let slot = match name {
        "ERP_AUTH_TIMEOUT_SECS" => &AUTH,
        "ERP_DATA_SESSION_TIMEOUT_SECS" => &DATA,
        "ERP_CONNECT_TIMEOUT_SECS" => &CONNECT,
        _ => return default,
    };
    *slot.get_or_init(|| {
        env::var(name)
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .map(Duration::from_secs)
            .unwrap_or(default)
    })
}

fn bind_tcp_listener<A>(addr: A) -> Result<TcpListener>
where
    A: ToSocketAddrs,
{
    let addr = addr
        .to_socket_addrs()?
        .next()
        .context("listen address did not resolve")?;
    let domain = if addr.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    let socket = Socket::new(domain, Type::STREAM, Some(SocketProtocol::TCP))?;
    socket.set_reuse_address(true)?;
    socket.bind(&addr.into())?;
    socket.listen(listen_backlog())?;
    socket.set_nonblocking(true)?;
    Ok(TcpListener::from_std(socket.into())?)
}

fn tune_tcp_stream(stream: &TcpStream) {
    if let Err(err) = stream.set_nodelay(true) {
        debug!("failed to enable TCP_NODELAY: {err}");
    }
}

async fn accept_tunnel(stream: TcpStream, tunnel: &ServerTunnel) -> Result<BoxedStream> {
    let _ = tunnel;
    Ok(Box::new(stream))
}

async fn connect_tunnel(addr: &str, tunnel: &ClientTunnel) -> Result<BoxedStream> {
    let _ = tunnel;
    let stream = TcpStream::connect(addr).await?;
    tune_tcp_stream(&stream);
    Ok(Box::new(stream))
}

async fn run_server(config: Config) -> Result<()> {
    let server = config.server.clone().context("missing server config")?;
    let tunnel = build_server_tunnel(config.transport, &server)?;
    let control_addr = format!("{}:{}", server.bind_addr, server.control_port);
    let listener = bind_tcp_listener(control_addr.as_str())?;
    let state = Arc::new(ServerState::default());
    let direct_socket = Arc::new(UdpSocket::bind(&control_addr).await?);
    *state.direct_socket.lock().await = Some(direct_socket.clone());
    spawn_direct_udp_server(
        direct_socket,
        state.clone(),
        config.token.clone(),
        config.transport,
    );
    let auth_slots = Arc::new(Semaphore::new(max_auth_handshakes()));
    info!("server listening on {}", control_addr);

    loop {
        let (stream, addr) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(err) if is_fd_exhaustion(&err) => {
                warn!("control accept hit file descriptor limit: {err}");
                sleep(ACCEPT_FD_BACKOFF).await;
                continue;
            }
            Err(err) => return Err(err.into()),
        };
        tune_tcp_stream(&stream);
        let Ok(auth_slot) = auth_slots.clone().try_acquire_owned() else {
            debug!("dropping control connection from {addr}: auth handshake limit reached");
            continue;
        };
        let state = state.clone();
        let token = config.token.clone();
        let tunnel = tunnel.clone();
        tokio::spawn(async move {
            let _auth_slot = auth_slot;
            let result = async {
                let stream = accept_tunnel(stream, &tunnel).await?;
                handle_server_conn(stream, state, token, tunnel.transport).await
            }
            .await;
            if let Err(err) = result {
                debug!("server connection from {} ended: {:#}", addr, err);
            }
        });
    }
}

#[derive(Clone)]
struct ServerTunnel {
    transport: Transport,
}

#[derive(Clone)]
struct ClientTunnel {
    transport: Transport,
}

#[derive(Default)]
struct ServerState {
    mappings: Mutex<HashSet<(Protocol, u16)>>,
    pending: Mutex<HashMap<u64, PendingData>>,
    udp_sockets: Mutex<HashMap<u16, Arc<UdpSocket>>>,
    direct_routes: Mutex<HashMap<u16, SocketAddr>>,
    direct_socket: Mutex<Option<Arc<UdpSocket>>>,
}

struct PendingData {
    remote_port: u16,
    sender: oneshot::Sender<DataSession>,
}

async fn handle_server_conn(
    mut stream: BoxedStream,
    state: Arc<ServerState>,
    token: String,
    transport: Transport,
) -> Result<()> {
    let crypto = timeout(
        auth_timeout(),
        authenticate_server_side(&mut stream, &token, transport),
    )
    .await
    .map_err(|_| anyhow!("auth timed out"))??;
    let mut recv_counter = 0;
    let frame = timeout(
        auth_timeout(),
        read_session_frame(
            &mut stream,
            &crypto,
            Direction::ClientToServer,
            &mut recv_counter,
        ),
    )
    .await
    .map_err(|_| anyhow!("initial frame timed out"))??;
    match frame {
        Frame::Register { mappings } => {
            handle_registration(
                stream,
                state,
                token,
                transport,
                mappings,
                crypto,
                recv_counter,
            )
            .await
        }
        Frame::DataStart { conn_id } => {
            let sender = state.pending.lock().await.remove(&conn_id);
            if let Some(pending) = sender {
                let _ = pending.sender.send(DataSession {
                    stream,
                    crypto,
                    read_counter: recv_counter,
                    write_counter: 0,
                });
                Ok(())
            } else {
                bail!("unknown data connection {}", conn_id)
            }
        }
        other => bail!("unexpected first frame: {:?}", other),
    }
}

async fn handle_registration(
    stream: BoxedStream,
    state: Arc<ServerState>,
    token: String,
    transport: Transport,
    mappings: Vec<MappingConfig>,
    crypto: SessionCrypto,
    read_counter: u64,
) -> Result<()> {
    let (reader_half, writer_half) = tokio::io::split(stream);
    let mut reader = FrameReader::new(
        reader_half,
        crypto.clone(),
        Direction::ClientToServer,
        read_counter,
    );
    let mut writer = FrameWriter::new(writer_half, crypto.clone(), Direction::ServerToClient, 0);
    let (tx, mut rx) = mpsc::channel::<Frame>(control_channel_capacity());
    let mut listener_tasks = Vec::new();
    let owned_mappings = mappings
        .iter()
        .map(|mapping| (mapping.protocol, mapping.remote_port))
        .collect::<Vec<_>>();

    {
        let mut registered = state.mappings.lock().await;
        for mapping in &mappings {
            let key = (mapping.protocol, mapping.remote_port);
            if registered.contains(&key) {
                writer
                    .write_frame(&Frame::RegisterFailed {
                        message: format!(
                            "remote {} port {} is already registered",
                            protocol_name(mapping.protocol),
                            mapping.remote_port
                        ),
                    })
                    .await?;
                bail!(
                    "duplicate remote {} port {}",
                    protocol_name(mapping.protocol),
                    mapping.remote_port
                );
            }
        }
        for mapping in &mappings {
            registered.insert((mapping.protocol, mapping.remote_port));
        }
    }

    writer.write_frame(&Frame::RegisterOk).await?;
    for mapping in mappings {
        match mapping.protocol {
            Protocol::Tcp => listener_tasks
                .push(spawn_tcp_listener(mapping.remote_port, state.clone(), tx.clone()).await?),
            Protocol::Udp => listener_tasks.push(
                spawn_udp_listener(mapping, state.clone(), tx.clone(), token.clone(), transport)
                    .await?,
            ),
        }
    }

    let mut writer_task = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if writer.write_frame(&frame).await.is_err() {
                break;
            }
        }
    });
    let reader_state = state.clone();
    let mut reader_task = tokio::spawn(async move {
        loop {
            match reader.read_frame().await {
                Ok(Frame::UdpResponse {
                    remote_port,
                    payload,
                    peer,
                }) => {
                    let socket = reader_state
                        .udp_sockets
                        .lock()
                        .await
                        .get(&remote_port)
                        .cloned();
                    if let Some(socket) = socket {
                        let _ = socket.send_to(&payload, peer).await;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });

    tokio::select! {
        _ = &mut writer_task => {
            reader_task.abort();
        }
        _ = &mut reader_task => {
            writer_task.abort();
        }
    }

    for task in listener_tasks {
        task.abort();
    }
    cleanup_client_ports(&state, &owned_mappings).await;
    Ok(())
}

async fn cleanup_client_ports(state: &ServerState, mappings: &[(Protocol, u16)]) {
    let owned_mappings = mappings.iter().copied().collect::<HashSet<_>>();
    let owned_tcp_ports = mappings
        .iter()
        .filter_map(|(protocol, port)| (*protocol == Protocol::Tcp).then_some(*port))
        .collect::<HashSet<_>>();
    let owned_udp_ports = mappings
        .iter()
        .filter_map(|(protocol, port)| (*protocol == Protocol::Udp).then_some(*port))
        .collect::<HashSet<_>>();
    state
        .mappings
        .lock()
        .await
        .retain(|mapping| !owned_mappings.contains(mapping));
    state
        .udp_sockets
        .lock()
        .await
        .retain(|port, _| !owned_udp_ports.contains(port));
    state
        .direct_routes
        .lock()
        .await
        .retain(|port, _| !owned_udp_ports.contains(port));
    state
        .pending
        .lock()
        .await
        .retain(|_, pending| !owned_tcp_ports.contains(&pending.remote_port));
}

fn protocol_name(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Tcp => "tcp",
        Protocol::Udp => "udp",
    }
}

async fn spawn_tcp_listener(
    remote_port: u16,
    state: Arc<ServerState>,
    tx: mpsc::Sender<Frame>,
) -> Result<tokio::task::JoinHandle<()>> {
    let listener = bind_tcp_listener(SocketAddr::from(([0, 0, 0, 0], remote_port)))?;
    info!("tcp remote port {} listening", remote_port);
    let task = tokio::spawn(async move {
        loop {
            let inbound = match listener.accept().await {
                Ok((inbound, _)) => inbound,
                Err(err) if is_fd_exhaustion(&err) => {
                    warn!("tcp remote port {remote_port} accept hit file descriptor limit: {err}");
                    sleep(ACCEPT_FD_BACKOFF).await;
                    continue;
                }
                Err(err) => {
                    warn!("tcp remote port {remote_port} accept failed: {err}");
                    sleep(Duration::from_millis(10)).await;
                    continue;
                }
            };
            tune_tcp_stream(&inbound);
            let state = state.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let conn_id = next_id();
                let (sender, receiver) = oneshot::channel();
                {
                    let mut pending = state.pending.lock().await;
                    if pending.len() >= max_pending_connections() {
                        debug!("dropping tcp {remote_port}: pending data limit reached");
                        return;
                    }
                    pending.insert(
                        conn_id,
                        PendingData {
                            remote_port,
                            sender,
                        },
                    );
                }
                if tx
                    .try_send(Frame::OpenTcp {
                        conn_id,
                        remote_port,
                    })
                    .is_err()
                {
                    let _ = state.pending.lock().await.remove(&conn_id);
                    debug!("dropping tcp {remote_port}: control channel is full or closed");
                    return;
                }
                match timeout(data_session_timeout(), receiver).await {
                    Ok(Ok(outbound)) => {
                        let _ = relay_tcp_stream(
                            inbound,
                            outbound,
                            Direction::ClientToServer,
                            Direction::ServerToClient,
                        )
                        .await;
                    }
                    _ => {
                        let _ = state.pending.lock().await.remove(&conn_id);
                    }
                }
            });
        }
    });
    Ok(task)
}

async fn spawn_udp_listener(
    mapping: MappingConfig,
    state: Arc<ServerState>,
    tx: mpsc::Sender<Frame>,
    token: String,
    transport: Transport,
) -> Result<tokio::task::JoinHandle<()>> {
    let remote_port = mapping.remote_port;
    let udp_mode = mapping.udp_mode.unwrap_or(UdpMode::OverTcp);
    let socket = Arc::new(UdpSocket::bind(("0.0.0.0", remote_port)).await?);
    state
        .udp_sockets
        .lock()
        .await
        .insert(remote_port, socket.clone());
    info!("udp remote port {} listening", remote_port);
    let task = tokio::spawn(async move {
        let mut buf = vec![0u8; 65535];
        loop {
            let Ok((n, peer)) = socket.recv_from(&mut buf).await else {
                continue;
            };
            let payload = buf[..n].to_vec();
            match udp_mode {
                UdpMode::OverTcp => {
                    let conn_id = next_id();
                    let _ = tx.try_send(Frame::OpenUdp {
                        conn_id,
                        remote_port,
                        payload,
                        peer,
                    });
                }
                UdpMode::Direct => {
                    let route = state.direct_routes.lock().await.get(&remote_port).cloned();
                    let direct_socket = state.direct_socket.lock().await.clone();
                    if let (Some(route), Some(direct_socket)) = (route, direct_socket)
                        && let Ok(packet) =
                            make_direct_packet(&token, transport, remote_port, peer, payload)
                    {
                        let _ = direct_socket.send_to(&packet, route).await;
                    }
                }
            }
        }
    });
    Ok(task)
}

fn spawn_direct_udp_server(
    socket: Arc<UdpSocket>,
    state: Arc<ServerState>,
    token: String,
    transport: Transport,
) {
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65535];
        loop {
            let Ok((n, addr)) = socket.recv_from(&mut buf).await else {
                continue;
            };
            match decode_direct_frame(&token, transport, &buf[..n]) {
                Ok(UdpDatagram::Hello {
                    key_rounds,
                    client_id,
                    remote_port,
                    timestamp,
                    nonce,
                    proof,
                }) => {
                    if verify_direct_hello(
                        &token,
                        key_rounds,
                        &client_id,
                        remote_port,
                        DirectProofRef {
                            timestamp,
                            nonce: &nonce,
                            proof: &proof,
                        },
                    )
                    .is_ok()
                    {
                        state.direct_routes.lock().await.insert(remote_port, addr);
                    }
                }
                Ok(UdpDatagram::Packet {
                    key_rounds,
                    remote_port,
                    peer,
                    payload,
                    timestamp,
                    nonce,
                    proof,
                }) => {
                    if verify_direct_packet(
                        &token,
                        key_rounds,
                        remote_port,
                        peer,
                        &payload,
                        DirectProofRef {
                            timestamp,
                            nonce: &nonce,
                            proof: &proof,
                        },
                    )
                    .is_err()
                    {
                        continue;
                    }
                    let public_socket = state.udp_sockets.lock().await.get(&remote_port).cloned();
                    if let Some(public_socket) = public_socket {
                        let _ = public_socket.send_to(&payload, peer).await;
                    }
                }
                Err(_) => {}
            }
        }
    });
}

async fn spawn_direct_udp_client(
    client: ClientConfig,
    token: String,
    transport: Transport,
) -> Result<()> {
    let socket = Arc::new(UdpSocket::bind("0.0.0.0:0").await?);
    let direct_mappings = client
        .mappings
        .iter()
        .filter(|mapping| mapping.udp_mode == Some(UdpMode::Direct))
        .cloned()
        .collect::<Vec<_>>();

    for mapping in &direct_mappings {
        let hello = make_direct_hello(&token, transport, &client.client_id, mapping.remote_port)?;
        socket.send_to(&hello, &client.server_addr).await?;
    }

    tokio::spawn(async move {
        let mut buf = vec![0u8; 65535];
        loop {
            let Ok((n, _)) = socket.recv_from(&mut buf).await else {
                continue;
            };
            let Ok(UdpDatagram::Packet {
                key_rounds,
                remote_port,
                peer,
                payload,
                timestamp,
                nonce,
                proof,
            }) = decode_direct_frame(&token, transport, &buf[..n])
            else {
                continue;
            };
            if verify_direct_packet(
                &token,
                key_rounds,
                remote_port,
                peer,
                &payload,
                DirectProofRef {
                    timestamp,
                    nonce: &nonce,
                    proof: &proof,
                },
            )
            .is_err()
            {
                continue;
            }

            match forward_udp_once(client.clone(), remote_port, payload).await {
                Ok(Some(payload)) => {
                    if let Ok(packet) =
                        make_direct_packet(&token, transport, remote_port, peer, payload)
                    {
                        let _ = socket.send_to(&packet, &client.server_addr).await;
                    }
                }
                Ok(None) => {}
                Err(err) => warn!("direct udp forwarding failed: {:#}", err),
            }
        }
    });

    Ok(())
}

async fn run_client(config: Config) -> Result<()> {
    let client = config.client.clone().context("missing client config")?;
    let tunnel = build_client_tunnel(config.transport, &client)?;
    let mut stream = connect_tunnel(&client.server_addr, &tunnel).await?;
    let crypto = authenticate_client_side(
        &mut stream,
        &config.token,
        &client.client_id,
        tunnel.transport,
    )
    .await?;
    let mut send_counter = 0;
    let mut recv_counter = 0;
    write_session_frame(
        &mut stream,
        &crypto,
        Direction::ClientToServer,
        &mut send_counter,
        &Frame::Register {
            mappings: client.mappings.clone(),
        },
    )
    .await?;
    match read_session_frame(
        &mut stream,
        &crypto,
        Direction::ServerToClient,
        &mut recv_counter,
    )
    .await?
    {
        Frame::RegisterOk => {
            info!("client registered");
            println!("client registered");
        }
        Frame::RegisterFailed { message } => bail!("registration failed: {}", message),
        other => bail!("unexpected register response: {:?}", other),
    }
    if client
        .mappings
        .iter()
        .any(|mapping| mapping.udp_mode == Some(UdpMode::Direct))
    {
        spawn_direct_udp_client(client.clone(), config.token.clone(), config.transport).await?;
    }

    let (reader, writer) = tokio::io::split(stream);
    let mut reader = FrameReader::new(
        reader,
        crypto.clone(),
        Direction::ServerToClient,
        recv_counter,
    );
    let mut writer = FrameWriter::new(writer, crypto, Direction::ClientToServer, send_counter);
    let (tx, mut rx) = mpsc::channel::<Frame>(control_channel_capacity());
    tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if writer.write_frame(&frame).await.is_err() {
                break;
            }
        }
    });

    while let Ok(frame) = reader.read_frame().await {
        match frame {
            Frame::OpenTcp {
                conn_id,
                remote_port,
            } => {
                let client = client.clone();
                let token = config.token.clone();
                let tunnel = tunnel.clone();
                tokio::spawn(async move {
                    if let Err(err) =
                        open_tcp_data(client, token, tunnel, conn_id, remote_port).await
                    {
                        warn!("tcp data connection failed: {:#}", err);
                    }
                });
            }
            Frame::OpenUdp {
                remote_port,
                payload,
                peer,
                ..
            } => {
                let client = client.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    match forward_udp_once(client, remote_port, payload).await {
                        Ok(Some(payload)) => {
                            let _ = tx
                                .send(Frame::UdpResponse {
                                    remote_port,
                                    payload,
                                    peer,
                                })
                                .await;
                        }
                        Ok(None) => {}
                        Err(err) => warn!("udp packet from {} failed: {:#}", peer, err),
                    }
                });
            }
            Frame::Error { message } => warn!("server error: {}", message),
            _ => {}
        }
    }
    Ok(())
}

async fn open_tcp_data(
    client: ClientConfig,
    token: String,
    tunnel: ClientTunnel,
    conn_id: u64,
    remote_port: u16,
) -> Result<()> {
    let mapping = client
        .mappings
        .iter()
        .find(|mapping| mapping.remote_port == remote_port && mapping.protocol == Protocol::Tcp)
        .context("missing tcp mapping")?;
    let mut server_stream = timeout(
        connect_timeout(),
        connect_tunnel(&client.server_addr, &tunnel),
    )
    .await
    .map_err(|_| anyhow!("server data connection timed out"))??;
    let crypto = authenticate_client_side(
        &mut server_stream,
        &token,
        &client.client_id,
        tunnel.transport,
    )
    .await?;
    let mut send_counter = 0;
    write_session_frame(
        &mut server_stream,
        &crypto,
        Direction::ClientToServer,
        &mut send_counter,
        &Frame::DataStart { conn_id },
    )
    .await?;
    let local = timeout(connect_timeout(), TcpStream::connect(&mapping.local_addr))
        .await
        .map_err(|_| anyhow!("local tcp connection timed out"))??;
    tune_tcp_stream(&local);
    relay_tcp_stream(
        local,
        DataSession {
            stream: server_stream,
            crypto,
            read_counter: 0,
            write_counter: send_counter,
        },
        Direction::ServerToClient,
        Direction::ClientToServer,
    )
    .await?;
    Ok(())
}

async fn relay_tcp_stream(
    plain: TcpStream,
    session: DataSession,
    tunnel_read_direction: Direction,
    tunnel_write_direction: Direction,
) -> Result<()> {
    let (mut plain_reader, mut plain_writer) = plain.into_split();
    let (tunnel_reader, tunnel_writer) = tokio::io::split(session.stream);
    let mut frame_reader = FrameReader::new(
        tunnel_reader,
        session.crypto.clone(),
        tunnel_read_direction,
        session.read_counter,
    );
    let mut frame_writer = FrameWriter::new(
        tunnel_writer,
        session.crypto,
        tunnel_write_direction,
        session.write_counter,
    );

    let plain_to_tunnel = async {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let n = plain_reader.read(&mut buf).await?;
            if n == 0 {
                frame_writer.write_frame(&Frame::DataEnd).await?;
                break;
            }
            frame_writer
                .write_frame(&Frame::Data {
                    payload: buf[..n].to_vec(),
                })
                .await?;
        }
        Result::<()>::Ok(())
    };

    let tunnel_to_plain = async {
        loop {
            match frame_reader.read_frame().await? {
                Frame::Data { payload } => plain_writer.write_all(&payload).await?,
                Frame::DataEnd => break,
                other => bail!("unexpected data frame: {:?}", other),
            }
        }
        let _ = plain_writer.shutdown().await;
        Result::<()>::Ok(())
    };

    tokio::select! {
        result = plain_to_tunnel => result,
        result = tunnel_to_plain => result,
    }
}

async fn forward_udp_once(
    client: ClientConfig,
    remote_port: u16,
    payload: Vec<u8>,
) -> Result<Option<Vec<u8>>> {
    let mapping = client
        .mappings
        .iter()
        .find(|mapping| mapping.remote_port == remote_port && mapping.protocol == Protocol::Udp)
        .context("missing udp mapping")?;
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    socket.send_to(&payload, &mapping.local_addr).await?;
    let mut buf = vec![0u8; 65535];
    match timeout(Duration::from_secs(5), socket.recv_from(&mut buf)).await {
        Ok(Ok((n, _))) => Ok(Some(buf[..n].to_vec())),
        Ok(Err(err)) => Err(err.into()),
        Err(_) => Ok(None),
    }
}

async fn authenticate_server_side<S>(
    stream: &mut S,
    token: &str,
    transport: Transport,
) -> Result<SessionCrypto>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let server_nonce = random_nonce_32();
    let key_rounds = random_key_rounds();
    write_frame(
        stream,
        &Frame::AuthChallenge {
            nonce: server_nonce.clone(),
            key_rounds,
        },
    )
    .await?;
    let handshake_crypto = derive_handshake_crypto(token, &server_nonce, key_rounds)?;
    let mut auth_recv_counter = 0;
    let Frame::AuthProof {
        client_id,
        timestamp,
        client_nonce,
        proof,
    } = read_session_frame(
        stream,
        &handshake_crypto,
        Direction::ClientToServer,
        &mut auth_recv_counter,
    )
    .await?
    else {
        bail!("expected auth proof");
    };
    verify_timestamp(timestamp)?;
    let expected = auth_proof(
        token,
        key_rounds,
        &client_id,
        &server_nonce,
        &client_nonce,
        timestamp,
    )?;
    if expected.ct_eq(&proof).into() {
        let mut auth_send_counter = 0;
        write_session_frame(
            stream,
            &handshake_crypto,
            Direction::ServerToClient,
            &mut auth_send_counter,
            &Frame::AuthOk,
        )
        .await?;
        derive_session_crypto(transport, token, key_rounds, &server_nonce, &client_nonce)
    } else {
        let mut auth_send_counter = 0;
        write_session_frame(
            stream,
            &handshake_crypto,
            Direction::ServerToClient,
            &mut auth_send_counter,
            &Frame::AuthFailed {
                message: "invalid token proof".into(),
            },
        )
        .await?;
        bail!("invalid token proof")
    }
}

async fn authenticate_client_side<S>(
    stream: &mut S,
    token: &str,
    client_id: &str,
    transport: Transport,
) -> Result<SessionCrypto>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let Frame::AuthChallenge {
        nonce: server_nonce,
        key_rounds,
    } = read_frame(stream).await?
    else {
        bail!("expected auth challenge");
    };
    let handshake_crypto = derive_handshake_crypto(token, &server_nonce, key_rounds)?;
    let client_nonce = random_nonce_32();
    let timestamp = now_secs()?;
    let proof = auth_proof(
        token,
        key_rounds,
        client_id,
        &server_nonce,
        &client_nonce,
        timestamp,
    )?;
    let mut auth_send_counter = 0;
    write_session_frame(
        stream,
        &handshake_crypto,
        Direction::ClientToServer,
        &mut auth_send_counter,
        &Frame::AuthProof {
            client_id: client_id.into(),
            timestamp,
            client_nonce: client_nonce.clone(),
            proof,
        },
    )
    .await?;
    let mut auth_recv_counter = 0;
    match read_session_frame(
        stream,
        &handshake_crypto,
        Direction::ServerToClient,
        &mut auth_recv_counter,
    )
    .await?
    {
        Frame::AuthOk => {
            derive_session_crypto(transport, token, key_rounds, &server_nonce, &client_nonce)
        }
        Frame::AuthFailed { message } => bail!("auth failed: {}", message),
        other => bail!("unexpected auth response: {:?}", other),
    }
}

fn auth_proof(
    token: &str,
    key_rounds: u8,
    client_id: &str,
    server_nonce: &[u8],
    client_nonce: &[u8],
    timestamp: u64,
) -> Result<Vec<u8>> {
    let key = token_round_key(token, key_rounds);
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(&key).map_err(|_| anyhow!("invalid hmac key"))?;
    mac.update(b"erp-auth-v3");
    mac.update(&[key_rounds]);
    mac.update(client_id.as_bytes());
    mac.update(&timestamp.to_be_bytes());
    mac.update(server_nonce);
    mac.update(client_nonce);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn derive_handshake_crypto(
    token: &str,
    server_nonce: &[u8],
    key_rounds: u8,
) -> Result<SessionCrypto> {
    let mut key = [0u8; 32];
    let token_key = token_round_key(token, key_rounds);
    Hkdf::<Sha256>::new(Some(server_nonce), &token_key)
        .expand(b"erp-auth-handshake-aes-256-gcm-v1", &mut key)
        .map_err(|_| anyhow!("failed to derive handshake key"))?;
    Ok(SessionCrypto::Aes256(Box::new(
        Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow!("invalid aes key"))?,
    )))
}

fn derive_session_crypto(
    transport: Transport,
    token: &str,
    key_rounds: u8,
    server_nonce: &[u8],
    client_nonce: &[u8],
) -> Result<SessionCrypto> {
    match transport {
        Transport::Raw => {
            let mut key = [0u8; 32];
            expand_session_key(
                token,
                key_rounds,
                server_nonce,
                client_nonce,
                DEFAULT_RAW_AES_INFO,
                &mut key,
            )?;
            Ok(SessionCrypto::Aes256(Box::new(
                Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow!("invalid aes key"))?,
            )))
        }
        Transport::Aes128Gcm => {
            let mut key = [0u8; 16];
            expand_session_key(
                token,
                key_rounds,
                server_nonce,
                client_nonce,
                b"erp-session-aes-128-gcm-v1",
                &mut key,
            )?;
            Ok(SessionCrypto::Aes128(Box::new(
                Aes128Gcm::new_from_slice(&key).map_err(|_| anyhow!("invalid aes key"))?,
            )))
        }
        Transport::Aes256Gcm => {
            let mut key = [0u8; 32];
            expand_session_key(
                token,
                key_rounds,
                server_nonce,
                client_nonce,
                b"erp-session-aes-256-gcm-v1",
                &mut key,
            )?;
            Ok(SessionCrypto::Aes256(Box::new(
                Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow!("invalid aes key"))?,
            )))
        }
        Transport::Tls => {
            bail!("transport = \"tls\" is deprecated; use \"aes-256-gcm\" or \"raw\"")
        }
    }
}

fn expand_session_key(
    token: &str,
    key_rounds: u8,
    server_nonce: &[u8],
    client_nonce: &[u8],
    info: &[u8],
    out: &mut [u8],
) -> Result<()> {
    let mut salt = Vec::with_capacity(server_nonce.len() + client_nonce.len());
    salt.extend_from_slice(server_nonce);
    salt.extend_from_slice(client_nonce);
    let token_key = token_round_key(token, key_rounds);
    Hkdf::<Sha256>::new(Some(&salt), &token_key)
        .expand(info, out)
        .map_err(|_| anyhow!("failed to derive session key"))
}

fn token_round_key(token: &str, key_rounds: u8) -> [u8; 32] {
    let mut material = token.as_bytes().to_vec();
    for _ in 0..key_rounds.max(1) {
        material = Sha256::digest(&material).to_vec();
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&material[..32]);
    key
}

fn encrypt_payload(
    crypto: &SessionCrypto,
    direction: Direction,
    counter: u64,
    payload: &[u8],
) -> Result<Vec<u8>> {
    match crypto {
        SessionCrypto::Aes128(cipher) => cipher
            .encrypt(Nonce::from_slice(&nonce_for(direction, counter)), payload)
            .map_err(|_| anyhow!("failed to encrypt frame")),
        SessionCrypto::Aes256(cipher) => cipher
            .encrypt(Nonce::from_slice(&nonce_for(direction, counter)), payload)
            .map_err(|_| anyhow!("failed to encrypt frame")),
    }
}

fn decrypt_payload(
    crypto: &SessionCrypto,
    direction: Direction,
    counter: u64,
    payload: &[u8],
) -> Result<Vec<u8>> {
    match crypto {
        SessionCrypto::Aes128(cipher) => cipher
            .decrypt(Nonce::from_slice(&nonce_for(direction, counter)), payload)
            .map_err(|_| anyhow!("failed to decrypt frame")),
        SessionCrypto::Aes256(cipher) => cipher
            .decrypt(Nonce::from_slice(&nonce_for(direction, counter)), payload)
            .map_err(|_| anyhow!("failed to decrypt frame")),
    }
}

fn nonce_for(direction: Direction, counter: u64) -> [u8; AES_NONCE_LEN] {
    let mut nonce = [0u8; AES_NONCE_LEN];
    nonce[0] = match direction {
        Direction::ClientToServer => 1,
        Direction::ServerToClient => 2,
    };
    nonce[4..].copy_from_slice(&counter.to_be_bytes());
    nonce
}

fn make_direct_hello(
    token: &str,
    transport: Transport,
    client_id: &str,
    remote_port: u16,
) -> Result<Vec<u8>> {
    let key_rounds = random_key_rounds();
    let timestamp = now_secs()?;
    let nonce = random_nonce();
    let proof = direct_hello_proof(token, key_rounds, client_id, remote_port, timestamp, &nonce)?;
    encode_direct_frame(
        token,
        transport,
        key_rounds,
        &UdpDatagram::Hello {
            key_rounds,
            client_id: client_id.to_string(),
            remote_port,
            timestamp,
            nonce,
            proof,
        },
    )
}

fn make_direct_packet(
    token: &str,
    transport: Transport,
    remote_port: u16,
    peer: SocketAddr,
    payload: Vec<u8>,
) -> Result<Vec<u8>> {
    let key_rounds = random_key_rounds();
    let timestamp = now_secs()?;
    let nonce = random_nonce();
    let proof = direct_packet_proof(
        token,
        key_rounds,
        remote_port,
        peer,
        &payload,
        timestamp,
        &nonce,
    )?;
    encode_direct_frame(
        token,
        transport,
        key_rounds,
        &UdpDatagram::Packet {
            key_rounds,
            remote_port,
            peer,
            payload,
            timestamp,
            nonce,
            proof,
        },
    )
}

fn encode_direct_frame(
    token: &str,
    transport: Transport,
    key_rounds: u8,
    datagram: &UdpDatagram,
) -> Result<Vec<u8>> {
    match transport {
        Transport::Raw | Transport::Aes128Gcm | Transport::Aes256Gcm => {
            let plaintext = bincode::serialize(datagram)?;
            let nonce = random_nonce_12();
            let ciphertext =
                encrypt_direct_payload(token, transport, key_rounds, &nonce, &plaintext)?;
            Ok(bincode::serialize(&DirectUdpFrame::Encrypted {
                key_rounds,
                nonce,
                ciphertext,
            })?)
        }
        Transport::Tls => {
            bail!("transport = \"tls\" is deprecated; use \"aes-256-gcm\" or \"raw\"")
        }
    }
}

fn decode_direct_frame(token: &str, transport: Transport, data: &[u8]) -> Result<UdpDatagram> {
    match bincode::deserialize::<DirectUdpFrame>(data)? {
        DirectUdpFrame::Encrypted {
            key_rounds,
            nonce,
            ciphertext,
        } if transport != Transport::Tls => {
            let plaintext =
                decrypt_direct_payload(token, transport, key_rounds, &nonce, &ciphertext)?;
            Ok(bincode::deserialize(&plaintext)?)
        }
        _ => bail!("direct udp transport mismatch"),
    }
}

fn encrypt_direct_payload(
    token: &str,
    transport: Transport,
    key_rounds: u8,
    nonce: &[u8],
    payload: &[u8],
) -> Result<Vec<u8>> {
    if nonce.len() != AES_NONCE_LEN {
        bail!("invalid direct udp nonce length");
    }
    match direct_crypto(token, transport, key_rounds)? {
        SessionCrypto::Aes128(cipher) => cipher
            .encrypt(Nonce::from_slice(nonce), payload)
            .map_err(|_| anyhow!("failed to encrypt direct udp packet")),
        SessionCrypto::Aes256(cipher) => cipher
            .encrypt(Nonce::from_slice(nonce), payload)
            .map_err(|_| anyhow!("failed to encrypt direct udp packet")),
    }
}

fn decrypt_direct_payload(
    token: &str,
    transport: Transport,
    key_rounds: u8,
    nonce: &[u8],
    payload: &[u8],
) -> Result<Vec<u8>> {
    if nonce.len() != AES_NONCE_LEN {
        bail!("invalid direct udp nonce length");
    }
    match direct_crypto(token, transport, key_rounds)? {
        SessionCrypto::Aes128(cipher) => cipher
            .decrypt(Nonce::from_slice(nonce), payload)
            .map_err(|_| anyhow!("failed to decrypt direct udp packet")),
        SessionCrypto::Aes256(cipher) => cipher
            .decrypt(Nonce::from_slice(nonce), payload)
            .map_err(|_| anyhow!("failed to decrypt direct udp packet")),
    }
}

fn direct_crypto(token: &str, transport: Transport, key_rounds: u8) -> Result<SessionCrypto> {
    match transport {
        Transport::Raw => {
            let mut key = [0u8; 32];
            let token_key = token_round_key(token, key_rounds);
            Hkdf::<Sha256>::new(None, &token_key)
                .expand(b"erp-direct-udp-raw-aes-256-gcm-v2", &mut key)
                .map_err(|_| anyhow!("failed to derive direct udp key"))?;
            Ok(SessionCrypto::Aes256(Box::new(
                Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow!("invalid aes key"))?,
            )))
        }
        Transport::Aes128Gcm => {
            let mut key = [0u8; 16];
            let token_key = token_round_key(token, key_rounds);
            Hkdf::<Sha256>::new(None, &token_key)
                .expand(b"erp-direct-udp-aes-128-gcm-v1", &mut key)
                .map_err(|_| anyhow!("failed to derive direct udp key"))?;
            Ok(SessionCrypto::Aes128(Box::new(
                Aes128Gcm::new_from_slice(&key).map_err(|_| anyhow!("invalid aes key"))?,
            )))
        }
        Transport::Aes256Gcm => {
            let mut key = [0u8; 32];
            let token_key = token_round_key(token, key_rounds);
            Hkdf::<Sha256>::new(None, &token_key)
                .expand(b"erp-direct-udp-aes-256-gcm-v1", &mut key)
                .map_err(|_| anyhow!("failed to derive direct udp key"))?;
            Ok(SessionCrypto::Aes256(Box::new(
                Aes256Gcm::new_from_slice(&key).map_err(|_| anyhow!("invalid aes key"))?,
            )))
        }
        Transport::Tls => {
            bail!("transport = \"tls\" is deprecated; use \"aes-256-gcm\" or \"raw\"")
        }
    }
}

fn verify_direct_hello(
    token: &str,
    key_rounds: u8,
    client_id: &str,
    remote_port: u16,
    auth: DirectProofRef<'_>,
) -> Result<()> {
    verify_timestamp(auth.timestamp)?;
    let expected = direct_hello_proof(
        token,
        key_rounds,
        client_id,
        remote_port,
        auth.timestamp,
        auth.nonce,
    )?;
    if expected.ct_eq(auth.proof).into() {
        Ok(())
    } else {
        bail!("invalid direct udp hello proof")
    }
}

fn verify_direct_packet(
    token: &str,
    key_rounds: u8,
    remote_port: u16,
    peer: SocketAddr,
    payload: &[u8],
    auth: DirectProofRef<'_>,
) -> Result<()> {
    verify_timestamp(auth.timestamp)?;
    let expected = direct_packet_proof(
        token,
        key_rounds,
        remote_port,
        peer,
        payload,
        auth.timestamp,
        auth.nonce,
    )?;
    if expected.ct_eq(auth.proof).into() {
        Ok(())
    } else {
        bail!("invalid direct udp packet proof")
    }
}

fn direct_hello_proof(
    token: &str,
    key_rounds: u8,
    client_id: &str,
    remote_port: u16,
    timestamp: u64,
    nonce: &[u8],
) -> Result<Vec<u8>> {
    let key = token_round_key(token, key_rounds);
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(&key).map_err(|_| anyhow!("invalid hmac key"))?;
    mac.update(b"erp-direct-hello-v2");
    mac.update(&[key_rounds]);
    mac.update(client_id.as_bytes());
    mac.update(&remote_port.to_be_bytes());
    mac.update(&timestamp.to_be_bytes());
    mac.update(nonce);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn direct_packet_proof(
    token: &str,
    key_rounds: u8,
    remote_port: u16,
    peer: SocketAddr,
    payload: &[u8],
    timestamp: u64,
    nonce: &[u8],
) -> Result<Vec<u8>> {
    let key = token_round_key(token, key_rounds);
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(&key).map_err(|_| anyhow!("invalid hmac key"))?;
    mac.update(b"erp-direct-packet-v2");
    mac.update(&[key_rounds]);
    mac.update(&remote_port.to_be_bytes());
    mac.update(peer.to_string().as_bytes());
    mac.update(&timestamp.to_be_bytes());
    mac.update(nonce);
    mac.update(payload);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn verify_timestamp(timestamp: u64) -> Result<()> {
    let now = now_secs()?;
    if now.abs_diff(timestamp) > DIRECT_AUTH_WINDOW_SECS {
        bail!("timestamp outside accepted window")
    }
    Ok(())
}

fn now_secs() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn random_nonce() -> Vec<u8> {
    let mut nonce = vec![0u8; 16];
    OsRng.fill_bytes(&mut nonce);
    nonce
}

fn random_nonce_12() -> Vec<u8> {
    let mut nonce = vec![0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    nonce
}

fn random_nonce_32() -> Vec<u8> {
    let mut nonce = vec![0u8; 32];
    OsRng.fill_bytes(&mut nonce);
    nonce
}

fn random_key_rounds() -> u8 {
    let mut byte = [0u8; 1];
    OsRng.fill_bytes(&mut byte);
    byte[0].max(1)
}

async fn read_frame<R>(reader: &mut R) -> Result<Frame>
where
    R: AsyncRead + Unpin,
{
    let buf = read_payload(reader).await?;
    Ok(bincode::deserialize(&buf)?)
}

async fn write_frame<W>(writer: &mut W, frame: &Frame) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let data = bincode::serialize(frame)?;
    write_payload(writer, &data).await
}

async fn read_payload<R>(reader: &mut R) -> Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let len = reader.read_u32().await? as usize;
    if len > 16 * 1024 * 1024 {
        bail!("frame too large");
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    Ok(buf)
}

async fn write_payload<W>(writer: &mut W, data: &[u8]) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    writer.write_u32(data.len() as u32).await?;
    writer.write_all(data).await?;
    writer.flush().await?;
    Ok(())
}

fn next_id() -> u64 {
    let mut bytes = [0u8; 8];
    OsRng.fill_bytes(&mut bytes);
    u64::from_be_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[test]
    fn parses_client_config_with_aes_transport() {
        let config: Config = toml::from_str(
            r#"
role = "client"
token = "secret"
transport = "aes-256-gcm"

[client]
server_addr = "example.com:7000"
client_id = "office"

[[client.mappings]]
name = "web"
protocol = "tcp"
local_addr = "127.0.0.1:8080"
remote_port = 18080
"#,
        )
        .unwrap();

        assert_eq!(config.role, Role::Client);
        assert_eq!(config.transport, Transport::Aes256Gcm);
        let client = config.client.unwrap();
        assert_eq!(client.mappings[0].remote_port, 18080);
    }

    #[test]
    fn deprecated_tls_config_is_rejected() {
        let config: Config = toml::from_str(
            r#"
role = "client"
token = "secret"
transport = "tls"

[client]
server_addr = "example.com:7000"
client_id = "office"

[[client.mappings]]
name = "web"
protocol = "tcp"
local_addr = "127.0.0.1:8080"
remote_port = 18080
"#,
        )
        .unwrap();

        let err = match build_client_tunnel(config.transport, config.client.as_ref().unwrap()) {
            Ok(_) => panic!("tls config should be rejected"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("deprecated"));
    }

    #[test]
    fn auth_proof_depends_on_token() {
        let server_nonce = b"server-nonce";
        let client_nonce = b"client-nonce";
        let good = auth_proof("secret", 7, "client-a", server_nonce, client_nonce, 42).unwrap();
        let same = auth_proof("secret", 7, "client-a", server_nonce, client_nonce, 42).unwrap();
        let bad = auth_proof("other", 7, "client-a", server_nonce, client_nonce, 42).unwrap();
        let bad_rounds =
            auth_proof("secret", 8, "client-a", server_nonce, client_nonce, 42).unwrap();

        assert_eq!(good, same);
        assert_ne!(good, bad);
        assert_ne!(good, bad_rounds);
    }

    #[test]
    fn direct_udp_hello_and_packet_verify() {
        let hello = make_direct_hello("secret", Transport::Raw, "client-a", 18080).unwrap();
        let UdpDatagram::Hello {
            key_rounds,
            client_id,
            remote_port,
            timestamp,
            nonce,
            proof,
        } = decode_direct_frame("secret", Transport::Raw, &hello).unwrap()
        else {
            panic!("expected hello datagram");
        };
        verify_direct_hello(
            "secret",
            key_rounds,
            &client_id,
            remote_port,
            DirectProofRef {
                timestamp,
                nonce: &nonce,
                proof: &proof,
            },
        )
        .unwrap();

        let peer: SocketAddr = "127.0.0.1:50000".parse().unwrap();
        let packet =
            make_direct_packet("secret", Transport::Raw, 18080, peer, b"ping".to_vec()).unwrap();
        assert!(!packet.windows(4).any(|window| window == b"ping"));
        let UdpDatagram::Packet {
            key_rounds,
            remote_port,
            peer,
            payload,
            timestamp,
            nonce,
            proof,
        } = decode_direct_frame("secret", Transport::Raw, &packet).unwrap()
        else {
            panic!("expected packet datagram");
        };
        verify_direct_packet(
            "secret",
            key_rounds,
            remote_port,
            peer,
            &payload,
            DirectProofRef {
                timestamp,
                nonce: &nonce,
                proof: &proof,
            },
        )
        .unwrap();
    }

    #[test]
    fn aes_direct_udp_packet_hides_payload() {
        let peer: SocketAddr = "127.0.0.1:50000".parse().unwrap();
        let packet = make_direct_packet(
            "secret",
            Transport::Aes256Gcm,
            18080,
            peer,
            b"ping".to_vec(),
        )
        .unwrap();
        assert!(!packet.windows(4).any(|window| window == b"ping"));
        let UdpDatagram::Packet { payload, .. } =
            decode_direct_frame("secret", Transport::Aes256Gcm, &packet).unwrap()
        else {
            panic!("expected packet datagram");
        };
        assert_eq!(payload, b"ping");
    }

    #[tokio::test]
    async fn frame_round_trips_over_binary_stream() {
        let (mut a, mut b) = duplex(1024);
        let writer = tokio::spawn(async move {
            write_frame(
                &mut a,
                &Frame::AuthFailed {
                    message: "nope".to_string(),
                },
            )
            .await
            .unwrap();
        });

        let frame = read_frame(&mut b).await.unwrap();
        writer.await.unwrap();
        match frame {
            Frame::AuthFailed { message } => assert_eq!(message, "nope"),
            _ => panic!("unexpected frame"),
        }
    }

    #[tokio::test]
    async fn aes_session_frame_hides_error_text() {
        let crypto =
            derive_session_crypto(Transport::Aes256Gcm, "secret", 7, b"server", b"client").unwrap();
        let (mut a, mut b) = duplex(2048);
        let writer_crypto = crypto.clone();
        let writer = tokio::spawn(async move {
            let mut counter = 0;
            write_session_frame(
                &mut a,
                &writer_crypto,
                Direction::ServerToClient,
                &mut counter,
                &Frame::RegisterFailed {
                    message: "remote port 8080 is already registered".to_string(),
                },
            )
            .await
            .unwrap();
        });

        let raw = read_payload(&mut b).await.unwrap();
        writer.await.unwrap();
        assert!(!raw.windows(11).any(|window| window == b"remote port"));
        let decrypted = decrypt_payload(&crypto, Direction::ServerToClient, 0, &raw).unwrap();
        let frame: Frame = bincode::deserialize(&decrypted).unwrap();
        match frame {
            Frame::RegisterFailed { message } => assert!(message.contains("8080")),
            _ => panic!("unexpected frame"),
        }
    }

    #[tokio::test]
    async fn raw_session_frame_hides_registration_details() {
        let crypto =
            derive_session_crypto(Transport::Raw, "secret", 9, b"server", b"client").unwrap();
        let mappings = vec![
            MappingConfig {
                name: "socks5-tcp".to_string(),
                protocol: Protocol::Tcp,
                local_addr: "127.0.0.1:1080".to_string(),
                remote_port: 18080,
                udp_mode: None,
            },
            MappingConfig {
                name: "socks5-udp".to_string(),
                protocol: Protocol::Udp,
                local_addr: "127.0.0.1:1080".to_string(),
                remote_port: 18080,
                udp_mode: Some(UdpMode::OverTcp),
            },
        ];
        let (mut a, mut b) = duplex(4096);
        let writer_crypto = crypto.clone();
        let writer = tokio::spawn(async move {
            let mut counter = 0;
            write_session_frame(
                &mut a,
                &writer_crypto,
                Direction::ClientToServer,
                &mut counter,
                &Frame::Register { mappings },
            )
            .await
            .unwrap();
        });

        let raw = read_payload(&mut b).await.unwrap();
        writer.await.unwrap();
        assert!(!raw.windows(10).any(|window| window == b"socks5-tcp"));
        assert!(!raw.windows(10).any(|window| window == b"socks5-udp"));
        assert!(!raw.windows(14).any(|window| window == b"127.0.0.1:1080"));
        let decrypted = decrypt_payload(&crypto, Direction::ClientToServer, 0, &raw).unwrap();
        let frame: Frame = bincode::deserialize(&decrypted).unwrap();
        match frame {
            Frame::Register { mappings } => assert_eq!(mappings.len(), 2),
            _ => panic!("unexpected frame"),
        }
    }
}
