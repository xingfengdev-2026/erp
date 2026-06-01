use std::{
    ffi::{CStr, CString, c_char},
    ptr,
};

use serde::Deserialize;

#[unsafe(no_mangle)]
pub extern "C" fn erp_ffi_version() -> *mut c_char {
    into_c_string(env!("CARGO_PKG_VERSION"))
}

/// Validates an `erp` TOML configuration string.
///
/// # Safety
///
/// `config` must be a valid, non-null pointer to a NUL-terminated UTF-8 string.
/// The returned pointer is either null for success or an owned error string that
/// must be released with `erp_ffi_free_string`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn erp_ffi_validate_config(config: *const c_char) -> *mut c_char {
    if config.is_null() {
        return into_c_string("config pointer is null");
    }

    let config = match unsafe { CStr::from_ptr(config) }.to_str() {
        Ok(value) => value,
        Err(error) => return into_c_string(format!("config is not valid UTF-8: {error}")),
    };

    match validate_config(config) {
        Ok(()) => ptr::null_mut(),
        Err(error) => into_c_string(error),
    }
}

/// Frees strings returned by `erp_ffi_version` and `erp_ffi_validate_config`.
///
/// # Safety
///
/// `value` must be null or a pointer returned by an `erp_ffi_*` function in this
/// library. Passing any other pointer is undefined behavior.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn erp_ffi_free_string(value: *mut c_char) {
    if !value.is_null() {
        let _ = unsafe { CString::from_raw(value) };
    }
}

fn into_c_string(value: impl AsRef<str>) -> *mut c_char {
    CString::new(value.as_ref())
        .unwrap_or_else(|_| CString::new("string contains interior nul byte").unwrap())
        .into_raw()
}

fn validate_config(config: &str) -> Result<(), String> {
    let config: Config = toml::from_str(config).map_err(|error| error.to_string())?;
    if config.token.trim().is_empty() {
        return Err("token must not be empty".to_string());
    }
    if config.transport == Transport::Tls {
        return Err("tls transport is deprecated; use raw or aes-gcm".to_string());
    }

    match config.role {
        Role::Server => validate_server(config.server.as_ref()),
        Role::Client => validate_client(config.client.as_ref()),
    }
}

fn validate_server(server: Option<&ServerConfig>) -> Result<(), String> {
    let server = server.ok_or_else(|| "server config is missing".to_string())?;
    if server.bind_addr.trim().is_empty() {
        return Err("server.bind_addr must not be empty".to_string());
    }
    if server.control_port == 0 {
        return Err("server.control_port must be greater than zero".to_string());
    }
    Ok(())
}

fn validate_client(client: Option<&ClientConfig>) -> Result<(), String> {
    let client = client.ok_or_else(|| "client config is missing".to_string())?;
    if client.server_addr.trim().is_empty() {
        return Err("client.server_addr must not be empty".to_string());
    }
    if client.client_id.trim().is_empty() {
        return Err("client.client_id must not be empty".to_string());
    }
    if client.mappings.is_empty() {
        return Err("client.mappings must include at least one mapping".to_string());
    }

    for mapping in &client.mappings {
        if mapping.name.trim().is_empty() {
            return Err("mapping.name must not be empty".to_string());
        }
        if mapping.local_addr.trim().is_empty() {
            return Err(format!(
                "mapping {} local_addr must not be empty",
                mapping.name
            ));
        }
        if mapping.remote_port == 0 {
            return Err(format!(
                "mapping {} remote_port must be greater than zero",
                mapping.name
            ));
        }
        if mapping.protocol == Protocol::Udp && mapping.udp_mode.is_none() {
            return Err(format!(
                "mapping {} udp_mode is required for UDP",
                mapping.name
            ));
        }
    }

    Ok(())
}

#[derive(Debug, Deserialize)]
struct Config {
    role: Role,
    token: String,
    transport: Transport,
    server: Option<ServerConfig>,
    client: Option<ClientConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Role {
    Server,
    Client,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum Transport {
    Raw,
    Aes256Gcm,
    Aes128Gcm,
    Tls,
}

#[derive(Debug, Deserialize)]
struct ServerConfig {
    bind_addr: String,
    control_port: u16,
}

#[derive(Debug, Deserialize)]
struct ClientConfig {
    server_addr: String,
    client_id: String,
    #[serde(default)]
    mappings: Vec<MappingConfig>,
}

#[derive(Debug, Deserialize)]
struct MappingConfig {
    name: String,
    protocol: Protocol,
    local_addr: String,
    remote_port: u16,
    #[serde(default)]
    udp_mode: Option<UdpMode>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Protocol {
    Tcp,
    Udp,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum UdpMode {
    OverTcp,
    Direct,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_client_config() {
        let config = r#"
role = "client"
token = "secret"
transport = "raw"

[client]
server_addr = "127.0.0.1:7000"
client_id = "android"

[[client.mappings]]
name = "socks5"
protocol = "tcp"
local_addr = "127.0.0.1:1080"
remote_port = 18080
"#;

        assert!(validate_config(config).is_ok());
    }

    #[test]
    fn rejects_udp_without_mode() {
        let config = r#"
role = "client"
token = "secret"
transport = "raw"

[client]
server_addr = "127.0.0.1:7000"
client_id = "android"

[[client.mappings]]
name = "dns"
protocol = "udp"
local_addr = "127.0.0.1:53"
remote_port = 1053
"#;

        assert_eq!(
            validate_config(config).unwrap_err(),
            "mapping dns udp_mode is required for UDP"
        );
    }
}
