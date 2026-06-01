use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand, ValueEnum};
use dialoguer::{Input, Password, Select, theme::ColorfulTheme};
use hmac::{Hmac, Mac};
use rand::{RngCore, rngs::OsRng};
use rustls::{
    ClientConfig as RustlsClientConfig, RootCertStore, ServerConfig as RustlsServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer, ServerName},
};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::{
    collections::{HashMap, HashSet},
    env,
    fs::File,
    io::BufReader,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, copy_bidirectional},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{Mutex, mpsc, oneshot},
    time::{Duration, timeout},
};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tracing::{debug, info, warn};

type HmacSha256 = Hmac<Sha256>;
type BoxedStream = Box<dyn TunnelStream>;
const DIRECT_AUTH_WINDOW_SECS: u64 = 300;

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
    Tls,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
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
    },
    AuthProof {
        client_id: String,
        timestamp: u64,
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
    Error {
        message: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
enum UdpDatagram {
    Hello {
        client_id: String,
        remote_port: u16,
        timestamp: u64,
        nonce: Vec<u8>,
        proof: Vec<u8>,
    },
    Packet {
        remote_port: u16,
        peer: SocketAddr,
        payload: Vec<u8>,
        timestamp: u64,
        nonce: Vec<u8>,
        proof: Vec<u8>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

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
        .items(&["server", "client"])
        .default(0)
        .interact()?;
    Ok(if choice == 0 {
        Command::Server { config: None }
    } else {
        Command::Client { config: None }
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
        .items(&["tls", "raw"])
        .default(0)
        .interact()?;
    let transport = if transport_index == 0 {
        Transport::Tls
    } else {
        Transport::Raw
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
            let (tls_cert_path, tls_key_path) = if transport == Transport::Tls {
                let cert: String = Input::with_theme(&ColorfulTheme::default())
                    .with_prompt("TLS certificate PEM path")
                    .interact_text()?;
                let key: String = Input::with_theme(&ColorfulTheme::default())
                    .with_prompt("TLS private key PEM path")
                    .interact_text()?;
                (Some(PathBuf::from(cert)), Some(PathBuf::from(key)))
            } else {
                (None, None)
            };
            Config {
                role,
                token,
                transport,
                server: Some(ServerConfig {
                    bind_addr,
                    control_port,
                    tls_cert_path,
                    tls_key_path,
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
            let (tls_server_name, tls_ca_cert_path) = if transport == Transport::Tls {
                let server_name: String = Input::with_theme(&ColorfulTheme::default())
                    .with_prompt("TLS server name")
                    .default(default_tls_server_name(&server_addr))
                    .interact_text()?;
                let ca_cert: String = Input::with_theme(&ColorfulTheme::default())
                    .with_prompt("TLS CA certificate PEM path")
                    .interact_text()?;
                (Some(server_name), Some(PathBuf::from(ca_cert)))
            } else {
                (None, None)
            };
            let local_addr: String = Input::with_theme(&ColorfulTheme::default())
                .with_prompt("Local service address")
                .default("127.0.0.1:8080".into())
                .interact_text()?;
            let remote_port: u16 = Input::with_theme(&ColorfulTheme::default())
                .with_prompt("Server remote port")
                .default(18080)
                .interact_text()?;
            Config {
                role,
                token,
                transport,
                server: None,
                client: Some(ClientConfig {
                    server_addr,
                    client_id,
                    tls_server_name,
                    tls_ca_cert_path,
                    mappings: vec![MappingConfig {
                        name: "default".into(),
                        protocol: Protocol::Tcp,
                        local_addr,
                        remote_port,
                        udp_mode: None,
                    }],
                }),
            }
        }
    };

    let content = toml::to_string_pretty(&config)?;
    std::fs::write(path, content)?;
    Ok(())
}

fn load_config(path: &Path) -> Result<Config> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    toml::from_str(&content).with_context(|| format!("failed to parse config {}", path.display()))
}

fn build_server_tunnel(transport: Transport, server: &ServerConfig) -> Result<ServerTunnel> {
    match transport {
        Transport::Raw => Ok(ServerTunnel::Raw),
        Transport::Tls => {
            let cert_path = server
                .tls_cert_path
                .as_ref()
                .context("tls_cert_path is required when transport = \"tls\"")?;
            let key_path = server
                .tls_key_path
                .as_ref()
                .context("tls_key_path is required when transport = \"tls\"")?;
            let certs = load_certs(cert_path)?;
            let key = load_private_key(key_path)?;
            let config = RustlsServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(certs, key)?;
            Ok(ServerTunnel::Tls(TlsAcceptor::from(Arc::new(config))))
        }
    }
}

fn build_client_tunnel(transport: Transport, client: &ClientConfig) -> Result<ClientTunnel> {
    match transport {
        Transport::Raw => Ok(ClientTunnel::Raw),
        Transport::Tls => {
            let ca_path = client
                .tls_ca_cert_path
                .as_ref()
                .context("tls_ca_cert_path is required when transport = \"tls\"")?;
            let mut roots = RootCertStore::empty();
            for cert in load_certs(ca_path)? {
                roots.add(cert)?;
            }
            let config = RustlsClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();
            Ok(ClientTunnel::Tls {
                connector: TlsConnector::from(Arc::new(config)),
                server_name: client
                    .tls_server_name
                    .clone()
                    .unwrap_or_else(|| default_tls_server_name(&client.server_addr)),
            })
        }
    }
}

async fn accept_tunnel(stream: TcpStream, tunnel: &ServerTunnel) -> Result<BoxedStream> {
    match tunnel {
        ServerTunnel::Raw => Ok(Box::new(stream)),
        ServerTunnel::Tls(acceptor) => Ok(Box::new(acceptor.accept(stream).await?)),
    }
}

async fn connect_tunnel(addr: &str, tunnel: &ClientTunnel) -> Result<BoxedStream> {
    let stream = TcpStream::connect(addr).await?;
    match tunnel {
        ClientTunnel::Raw => Ok(Box::new(stream)),
        ClientTunnel::Tls {
            connector,
            server_name,
        } => {
            let server_name = ServerName::try_from(server_name.clone())
                .map_err(|_| anyhow!("invalid tls_server_name {}", server_name))?;
            Ok(Box::new(connector.connect(server_name, stream).await?))
        }
    }
}

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let mut reader = BufReader::new(
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?,
    );
    let certs = rustls_pemfile::certs(&mut reader).collect::<std::io::Result<Vec<_>>>()?;
    if certs.is_empty() {
        bail!("no certificates found in {}", path.display());
    }
    Ok(certs)
}

fn load_private_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    let mut reader = BufReader::new(
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?,
    );
    rustls_pemfile::private_key(&mut reader)?
        .with_context(|| format!("no private key found in {}", path.display()))
}

fn default_tls_server_name(addr: &str) -> String {
    addr.rsplit_once(':')
        .map(|(host, _)| host.trim_matches(|c| c == '[' || c == ']').to_string())
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

async fn run_server(config: Config) -> Result<()> {
    let server = config.server.clone().context("missing server config")?;
    let tunnel = build_server_tunnel(config.transport, &server)?;
    let control_addr = format!("{}:{}", server.bind_addr, server.control_port);
    let listener = TcpListener::bind(&control_addr).await?;
    let state = Arc::new(ServerState::default());
    let direct_socket = Arc::new(UdpSocket::bind(&control_addr).await?);
    *state.direct_socket.lock().await = Some(direct_socket.clone());
    spawn_direct_udp_server(direct_socket, state.clone(), config.token.clone());
    info!("server listening on {}", control_addr);

    loop {
        let (stream, addr) = listener.accept().await?;
        let state = state.clone();
        let token = config.token.clone();
        let tunnel = tunnel.clone();
        tokio::spawn(async move {
            let result = async {
                let stream = accept_tunnel(stream, &tunnel).await?;
                handle_server_conn(stream, state, token).await
            }
            .await;
            if let Err(err) = result {
                debug!("server connection from {} ended: {:#}", addr, err);
            }
        });
    }
}

#[derive(Clone)]
enum ServerTunnel {
    Raw,
    Tls(TlsAcceptor),
}

#[derive(Clone)]
enum ClientTunnel {
    Raw,
    Tls {
        connector: TlsConnector,
        server_name: String,
    },
}

#[derive(Default)]
struct ServerState {
    mappings: Mutex<HashSet<u16>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<BoxedStream>>>,
    udp_sockets: Mutex<HashMap<u16, Arc<UdpSocket>>>,
    direct_routes: Mutex<HashMap<u16, SocketAddr>>,
    direct_socket: Mutex<Option<Arc<UdpSocket>>>,
}

async fn handle_server_conn(
    mut stream: BoxedStream,
    state: Arc<ServerState>,
    token: String,
) -> Result<()> {
    authenticate_server_side(&mut stream, &token).await?;
    let frame = read_frame(&mut stream).await?;
    match frame {
        Frame::Register { mappings } => handle_registration(stream, state, token, mappings).await,
        Frame::DataStart { conn_id } => {
            let sender = state.pending.lock().await.remove(&conn_id);
            if let Some(sender) = sender {
                let _ = sender.send(stream);
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
    mappings: Vec<MappingConfig>,
) -> Result<()> {
    let (mut reader, mut writer) = tokio::io::split(stream);
    let (tx, mut rx) = mpsc::channel::<Frame>(128);

    {
        let mut registered = state.mappings.lock().await;
        for mapping in &mappings {
            if registered.contains(&mapping.remote_port) {
                write_frame(
                    &mut writer,
                    &Frame::RegisterFailed {
                        message: format!(
                            "remote port {} is already registered",
                            mapping.remote_port
                        ),
                    },
                )
                .await?;
                bail!("duplicate remote port {}", mapping.remote_port);
            }
        }
        for mapping in &mappings {
            registered.insert(mapping.remote_port);
        }
    }

    write_frame(&mut writer, &Frame::RegisterOk).await?;
    for mapping in mappings {
        match mapping.protocol {
            Protocol::Tcp => {
                spawn_tcp_listener(mapping.remote_port, state.clone(), tx.clone()).await?
            }
            Protocol::Udp => {
                spawn_udp_listener(mapping, state.clone(), tx.clone(), token.clone()).await?
            }
        }
    }

    let writer_task = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if write_frame(&mut writer, &frame).await.is_err() {
                break;
            }
        }
    });
    let reader_task = tokio::spawn(async move {
        loop {
            match read_frame(&mut reader).await {
                Ok(Frame::UdpResponse {
                    remote_port,
                    payload,
                    peer,
                }) => {
                    let socket = state.udp_sockets.lock().await.get(&remote_port).cloned();
                    if let Some(socket) = socket {
                        let _ = socket.send_to(&payload, peer).await;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });

    let _ = tokio::join!(writer_task, reader_task);
    Ok(())
}

async fn spawn_tcp_listener(
    remote_port: u16,
    state: Arc<ServerState>,
    tx: mpsc::Sender<Frame>,
) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", remote_port)).await?;
    info!("tcp remote port {} listening", remote_port);
    tokio::spawn(async move {
        loop {
            let Ok((mut inbound, _)) = listener.accept().await else {
                continue;
            };
            let state = state.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let conn_id = next_id();
                let (sender, receiver) = oneshot::channel();
                state.pending.lock().await.insert(conn_id, sender);
                if tx
                    .send(Frame::OpenTcp {
                        conn_id,
                        remote_port,
                    })
                    .await
                    .is_err()
                {
                    let _ = state.pending.lock().await.remove(&conn_id);
                    return;
                }
                match timeout(Duration::from_secs(10), receiver).await {
                    Ok(Ok(mut outbound)) => {
                        let _ = copy_bidirectional(&mut inbound, &mut outbound).await;
                    }
                    _ => {
                        let _ = state.pending.lock().await.remove(&conn_id);
                    }
                }
            });
        }
    });
    Ok(())
}

async fn spawn_udp_listener(
    mapping: MappingConfig,
    state: Arc<ServerState>,
    tx: mpsc::Sender<Frame>,
    token: String,
) -> Result<()> {
    let remote_port = mapping.remote_port;
    let udp_mode = mapping.udp_mode.unwrap_or(UdpMode::OverTcp);
    let socket = Arc::new(UdpSocket::bind(("0.0.0.0", remote_port)).await?);
    state
        .udp_sockets
        .lock()
        .await
        .insert(remote_port, socket.clone());
    info!("udp remote port {} listening", remote_port);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65535];
        loop {
            let Ok((n, peer)) = socket.recv_from(&mut buf).await else {
                continue;
            };
            let payload = buf[..n].to_vec();
            match udp_mode {
                UdpMode::OverTcp => {
                    let conn_id = next_id();
                    let _ = tx
                        .send(Frame::OpenUdp {
                            conn_id,
                            remote_port,
                            payload,
                            peer,
                        })
                        .await;
                }
                UdpMode::Direct => {
                    let route = state.direct_routes.lock().await.get(&remote_port).cloned();
                    let direct_socket = state.direct_socket.lock().await.clone();
                    if let (Some(route), Some(direct_socket)) = (route, direct_socket)
                        && let Ok(packet) = make_direct_packet(&token, remote_port, peer, payload)
                    {
                        let _ = direct_socket.send_to(&packet, route).await;
                    }
                }
            }
        }
    });
    Ok(())
}

fn spawn_direct_udp_server(socket: Arc<UdpSocket>, state: Arc<ServerState>, token: String) {
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65535];
        loop {
            let Ok((n, addr)) = socket.recv_from(&mut buf).await else {
                continue;
            };
            match bincode::deserialize::<UdpDatagram>(&buf[..n]) {
                Ok(UdpDatagram::Hello {
                    client_id,
                    remote_port,
                    timestamp,
                    nonce,
                    proof,
                }) => {
                    if verify_direct_hello(
                        &token,
                        &client_id,
                        remote_port,
                        timestamp,
                        &nonce,
                        &proof,
                    )
                    .is_ok()
                    {
                        state.direct_routes.lock().await.insert(remote_port, addr);
                    }
                }
                Ok(UdpDatagram::Packet {
                    remote_port,
                    peer,
                    payload,
                    timestamp,
                    nonce,
                    proof,
                }) => {
                    if verify_direct_packet(
                        &token,
                        remote_port,
                        peer,
                        &payload,
                        timestamp,
                        &nonce,
                        &proof,
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

async fn spawn_direct_udp_client(client: ClientConfig, token: String) -> Result<()> {
    let socket = Arc::new(UdpSocket::bind("0.0.0.0:0").await?);
    let direct_mappings = client
        .mappings
        .iter()
        .filter(|mapping| mapping.udp_mode == Some(UdpMode::Direct))
        .cloned()
        .collect::<Vec<_>>();

    for mapping in &direct_mappings {
        let hello = make_direct_hello(&token, &client.client_id, mapping.remote_port)?;
        socket.send_to(&hello, &client.server_addr).await?;
    }

    tokio::spawn(async move {
        let mut buf = vec![0u8; 65535];
        loop {
            let Ok((n, _)) = socket.recv_from(&mut buf).await else {
                continue;
            };
            let Ok(UdpDatagram::Packet {
                remote_port,
                peer,
                payload,
                timestamp,
                nonce,
                proof,
            }) = bincode::deserialize::<UdpDatagram>(&buf[..n])
            else {
                continue;
            };
            if verify_direct_packet(
                &token,
                remote_port,
                peer,
                &payload,
                timestamp,
                &nonce,
                &proof,
            )
            .is_err()
            {
                continue;
            }

            match forward_udp_once(client.clone(), remote_port, payload).await {
                Ok(Some(payload)) => {
                    if let Ok(packet) = make_direct_packet(&token, remote_port, peer, payload) {
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
    authenticate_client_side(&mut stream, &config.token, &client.client_id).await?;
    write_frame(
        &mut stream,
        &Frame::Register {
            mappings: client.mappings.clone(),
        },
    )
    .await?;
    match read_frame(&mut stream).await? {
        Frame::RegisterOk => info!("client registered"),
        Frame::RegisterFailed { message } => bail!("registration failed: {}", message),
        other => bail!("unexpected register response: {:?}", other),
    }
    if client
        .mappings
        .iter()
        .any(|mapping| mapping.udp_mode == Some(UdpMode::Direct))
    {
        spawn_direct_udp_client(client.clone(), config.token.clone()).await?;
    }

    let (mut reader, mut writer) = tokio::io::split(stream);
    let (tx, mut rx) = mpsc::channel::<Frame>(128);
    tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if write_frame(&mut writer, &frame).await.is_err() {
                break;
            }
        }
    });

    while let Ok(frame) = read_frame(&mut reader).await {
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
    let mut server_stream = connect_tunnel(&client.server_addr, &tunnel).await?;
    authenticate_client_side(&mut server_stream, &token, &client.client_id).await?;
    write_frame(&mut server_stream, &Frame::DataStart { conn_id }).await?;
    let mut local = TcpStream::connect(&mapping.local_addr).await?;
    copy_bidirectional(&mut server_stream, &mut local).await?;
    Ok(())
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

async fn authenticate_server_side<S>(stream: &mut S, token: &str) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut nonce = vec![0u8; 32];
    OsRng.fill_bytes(&mut nonce);
    write_frame(
        stream,
        &Frame::AuthChallenge {
            nonce: nonce.clone(),
        },
    )
    .await?;
    let Frame::AuthProof {
        client_id,
        timestamp,
        proof,
    } = read_frame(stream).await?
    else {
        bail!("expected auth proof");
    };
    let expected = auth_proof(token, &client_id, &nonce, timestamp)?;
    if expected.ct_eq(&proof).into() {
        write_frame(stream, &Frame::AuthOk).await?;
        Ok(())
    } else {
        write_frame(
            stream,
            &Frame::AuthFailed {
                message: "invalid token proof".into(),
            },
        )
        .await?;
        bail!("invalid token proof")
    }
}

async fn authenticate_client_side<S>(stream: &mut S, token: &str, client_id: &str) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let Frame::AuthChallenge { nonce } = read_frame(stream).await? else {
        bail!("expected auth challenge");
    };
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let proof = auth_proof(token, client_id, &nonce, timestamp)?;
    write_frame(
        stream,
        &Frame::AuthProof {
            client_id: client_id.into(),
            timestamp,
            proof,
        },
    )
    .await?;
    match read_frame(stream).await? {
        Frame::AuthOk => Ok(()),
        Frame::AuthFailed { message } => bail!("auth failed: {}", message),
        other => bail!("unexpected auth response: {:?}", other),
    }
}

fn auth_proof(token: &str, client_id: &str, nonce: &[u8], timestamp: u64) -> Result<Vec<u8>> {
    let mut mac =
        HmacSha256::new_from_slice(token.as_bytes()).map_err(|_| anyhow!("invalid hmac key"))?;
    mac.update(client_id.as_bytes());
    mac.update(&timestamp.to_be_bytes());
    mac.update(nonce);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn make_direct_hello(token: &str, client_id: &str, remote_port: u16) -> Result<Vec<u8>> {
    let timestamp = now_secs()?;
    let nonce = random_nonce();
    let proof = direct_hello_proof(token, client_id, remote_port, timestamp, &nonce)?;
    Ok(bincode::serialize(&UdpDatagram::Hello {
        client_id: client_id.to_string(),
        remote_port,
        timestamp,
        nonce,
        proof,
    })?)
}

fn make_direct_packet(
    token: &str,
    remote_port: u16,
    peer: SocketAddr,
    payload: Vec<u8>,
) -> Result<Vec<u8>> {
    let timestamp = now_secs()?;
    let nonce = random_nonce();
    let proof = direct_packet_proof(token, remote_port, peer, &payload, timestamp, &nonce)?;
    Ok(bincode::serialize(&UdpDatagram::Packet {
        remote_port,
        peer,
        payload,
        timestamp,
        nonce,
        proof,
    })?)
}

fn verify_direct_hello(
    token: &str,
    client_id: &str,
    remote_port: u16,
    timestamp: u64,
    nonce: &[u8],
    proof: &[u8],
) -> Result<()> {
    verify_timestamp(timestamp)?;
    let expected = direct_hello_proof(token, client_id, remote_port, timestamp, nonce)?;
    if expected.ct_eq(proof).into() {
        Ok(())
    } else {
        bail!("invalid direct udp hello proof")
    }
}

fn verify_direct_packet(
    token: &str,
    remote_port: u16,
    peer: SocketAddr,
    payload: &[u8],
    timestamp: u64,
    nonce: &[u8],
    proof: &[u8],
) -> Result<()> {
    verify_timestamp(timestamp)?;
    let expected = direct_packet_proof(token, remote_port, peer, payload, timestamp, nonce)?;
    if expected.ct_eq(proof).into() {
        Ok(())
    } else {
        bail!("invalid direct udp packet proof")
    }
}

fn direct_hello_proof(
    token: &str,
    client_id: &str,
    remote_port: u16,
    timestamp: u64,
    nonce: &[u8],
) -> Result<Vec<u8>> {
    let mut mac =
        HmacSha256::new_from_slice(token.as_bytes()).map_err(|_| anyhow!("invalid hmac key"))?;
    mac.update(b"erp-direct-hello-v1");
    mac.update(client_id.as_bytes());
    mac.update(&remote_port.to_be_bytes());
    mac.update(&timestamp.to_be_bytes());
    mac.update(nonce);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn direct_packet_proof(
    token: &str,
    remote_port: u16,
    peer: SocketAddr,
    payload: &[u8],
    timestamp: u64,
    nonce: &[u8],
) -> Result<Vec<u8>> {
    let mut mac =
        HmacSha256::new_from_slice(token.as_bytes()).map_err(|_| anyhow!("invalid hmac key"))?;
    mac.update(b"erp-direct-packet-v1");
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

async fn read_frame<R>(reader: &mut R) -> Result<Frame>
where
    R: AsyncRead + Unpin,
{
    let len = reader.read_u32().await? as usize;
    if len > 16 * 1024 * 1024 {
        bail!("frame too large");
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    Ok(bincode::deserialize(&buf)?)
}

async fn write_frame<W>(writer: &mut W, frame: &Frame) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let data = bincode::serialize(frame)?;
    writer.write_u32(data.len() as u32).await?;
    writer.write_all(&data).await?;
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
    fn parses_client_config_with_tls_fields() {
        let config: Config = toml::from_str(
            r#"
role = "client"
token = "secret"
transport = "tls"

[client]
server_addr = "example.com:7000"
client_id = "office"
tls_server_name = "example.com"
tls_ca_cert_path = "ca.pem"

[[client.mappings]]
name = "web"
protocol = "tcp"
local_addr = "127.0.0.1:8080"
remote_port = 18080
"#,
        )
        .unwrap();

        assert_eq!(config.role, Role::Client);
        assert_eq!(config.transport, Transport::Tls);
        let client = config.client.unwrap();
        assert_eq!(client.tls_server_name.as_deref(), Some("example.com"));
        assert_eq!(client.mappings[0].remote_port, 18080);
    }

    #[test]
    fn auth_proof_depends_on_token() {
        let nonce = b"server-nonce";
        let good = auth_proof("secret", "client-a", nonce, 42).unwrap();
        let same = auth_proof("secret", "client-a", nonce, 42).unwrap();
        let bad = auth_proof("other", "client-a", nonce, 42).unwrap();

        assert_eq!(good, same);
        assert_ne!(good, bad);
    }

    #[test]
    fn direct_udp_hello_and_packet_verify() {
        let hello = make_direct_hello("secret", "client-a", 18080).unwrap();
        let UdpDatagram::Hello {
            client_id,
            remote_port,
            timestamp,
            nonce,
            proof,
        } = bincode::deserialize::<UdpDatagram>(&hello).unwrap()
        else {
            panic!("expected hello datagram");
        };
        verify_direct_hello("secret", &client_id, remote_port, timestamp, &nonce, &proof).unwrap();

        let peer: SocketAddr = "127.0.0.1:50000".parse().unwrap();
        let packet = make_direct_packet("secret", 18080, peer, b"ping".to_vec()).unwrap();
        let UdpDatagram::Packet {
            remote_port,
            peer,
            payload,
            timestamp,
            nonce,
            proof,
        } = bincode::deserialize::<UdpDatagram>(&packet).unwrap()
        else {
            panic!("expected packet datagram");
        };
        verify_direct_packet(
            "secret",
            remote_port,
            peer,
            &payload,
            timestamp,
            &nonce,
            &proof,
        )
        .unwrap();
    }

    #[test]
    fn default_tls_name_uses_host_part() {
        assert_eq!(default_tls_server_name("example.com:7000"), "example.com");
        assert_eq!(default_tls_server_name("[::1]:7000"), "::1");
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
}
