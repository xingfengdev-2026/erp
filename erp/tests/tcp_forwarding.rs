use std::{
    net::TcpListener as StdTcpListener,
    path::{Path, PathBuf},
    process::Stdio,
};

use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    process::{Child, Command},
    time::{Duration, sleep},
};

fn free_port() -> u16 {
    let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

async fn wait_for_tcp(addr: &str) {
    for _ in 0..60 {
        if TcpStream::connect(addr).await.is_ok() {
            return;
        }
        sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {addr}");
}

async fn start_echo(addr: String) {
    let listener = TcpListener::bind(addr).await.unwrap();
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            continue;
        };
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            loop {
                let Ok(n) = stream.read(&mut buf).await else {
                    break;
                };
                if n == 0 {
                    break;
                }
                if stream.write_all(&buf[..n]).await.is_err() {
                    break;
                }
            }
        });
    }
}

fn spawn_erp(args: &[&str]) -> Child {
    Command::new(erp_binary())
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn erp_binary() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_erp") {
        return PathBuf::from(path);
    }

    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.file_name().and_then(|name| name.to_str()) == Some("deps") {
        path.pop();
    }
    path.push(if cfg!(windows) { "erp.exe" } else { "erp" });
    path
}

fn write_configs(
    dir: &TempDir,
    transport: &str,
    control_port: u16,
    local_port: u16,
    remote_port: u16,
) -> (String, String) {
    let server_config = dir.path().join("server.toml");
    let client_config = dir.path().join("client.toml");
    std::fs::write(
        &server_config,
        format!(
            r#"
role = "server"
token = "secret"
transport = "{transport}"

[server]
bind_addr = "127.0.0.1"
control_port = {control_port}
"#
        ),
    )
    .unwrap();
    std::fs::write(
        &client_config,
        format!(
            r#"
role = "client"
token = "secret"
transport = "{transport}"

[client]
server_addr = "127.0.0.1:{control_port}"
client_id = "it"

[[client.mappings]]
name = "echo"
protocol = "tcp"
local_addr = "127.0.0.1:{local_port}"
remote_port = {remote_port}
"#
        ),
    )
    .unwrap();
    (path_string(&server_config), path_string(&client_config))
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

async fn assert_tcp_forwarding(transport: &str) {
    let control_port = free_port();
    let local_port = free_port();
    let remote_port = free_port();
    let dir = TempDir::new().unwrap();
    let (server_config, client_config) =
        write_configs(&dir, transport, control_port, local_port, remote_port);

    tokio::spawn(start_echo(format!("127.0.0.1:{local_port}")));
    wait_for_tcp(&format!("127.0.0.1:{local_port}")).await;

    let mut server = spawn_erp(&["server", "--config", &server_config]);
    wait_for_tcp(&format!("127.0.0.1:{control_port}")).await;

    let mut client = spawn_erp(&["client", "--config", &client_config]);
    wait_for_tcp(&format!("127.0.0.1:{remote_port}")).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{remote_port}"))
        .await
        .unwrap();
    stream.write_all(b"hello").await.unwrap();
    let mut buf = [0u8; 5];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"hello");

    let _ = client.kill().await;
    let _ = server.kill().await;
}

#[tokio::test]
async fn raw_tcp_forwarding_round_trips_data() {
    assert_tcp_forwarding("raw").await;
}

#[tokio::test]
async fn aes_tcp_forwarding_round_trips_data() {
    assert_tcp_forwarding("aes-256-gcm").await;
}

#[tokio::test]
async fn remote_port_is_reusable_after_client_disconnect() {
    let control_port = free_port();
    let local_port = free_port();
    let remote_port = free_port();
    let dir = TempDir::new().unwrap();
    let (server_config, client_config) =
        write_configs(&dir, "raw", control_port, local_port, remote_port);

    tokio::spawn(start_echo(format!("127.0.0.1:{local_port}")));
    wait_for_tcp(&format!("127.0.0.1:{local_port}")).await;

    let mut server = spawn_erp(&["server", "--config", &server_config]);
    wait_for_tcp(&format!("127.0.0.1:{control_port}")).await;

    let mut first_client = spawn_erp(&["client", "--config", &client_config]);
    wait_for_tcp(&format!("127.0.0.1:{remote_port}")).await;
    let _ = first_client.kill().await;
    sleep(Duration::from_millis(100)).await;

    let mut second_client = spawn_erp(&["client", "--config", &client_config]);
    wait_for_tcp(&format!("127.0.0.1:{remote_port}")).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{remote_port}"))
        .await
        .unwrap();
    stream.write_all(b"again").await.unwrap();
    let mut buf = [0u8; 5];
    tokio::time::timeout(Duration::from_secs(3), stream.read_exact(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf, b"again");

    let _ = second_client.kill().await;
    let _ = server.kill().await;
}
