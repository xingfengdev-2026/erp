# erp v1 Implementation Plan

## Summary

Build `erp` as a single Rust CLI binary for easy reverse proxying. It will support `server` and `client` modes, multiple TCP/UDP mappings, multiple clients, TOML configuration, selectable `tls` or `raw` transport, interactive setup, and GitHub Actions builds/releases.

## Key Changes

- Use Rust with `tokio`, `clap`, `serde`, `toml`, `bincode`, `rustls`/`tokio-rustls`, `hmac`, `sha2`, and `tracing`.
- Support `transport = "tls"` for encrypted client-server tunnels.
- Support `transport = "raw"` for unencrypted tunnels on trusted networks or during testing.
- Require token authentication in both transport modes.
- Authenticate with challenge-response: server sends a nonce, client returns an HMAC-SHA256 proof derived from the token, `client_id`, nonce, and timestamp.
- Never send the token directly, log it, or include it in error messages.
- Recommend `tls` in interactive setup while allowing explicit `raw`.
- Encode tunnel protocol frames as length-prefixed binary messages.

## CLI Behavior

- `erp server --config server.toml` starts server mode.
- `erp client --config client.toml` starts client mode.
- Running only `erp` prompts the user to choose `server` or `client`.
- If `--config` is omitted, scan the executable directory for `*.toml`.
- If configs exist, prompt the user to select one or create a new config.
- If no config exists, ask setup questions interactively and generate a local TOML config.

## Runtime Behavior

- Server listens on a configured control address and port.
- Server accepts multiple authenticated clients.
- Clients register multiple mappings with `name`, `protocol`, `local_addr`, `remote_port`, and optional `udp_mode`.
- Duplicate `remote_port` mappings are rejected and the existing mapping remains active.
- UDP supports both direct mode and UDP-over-TCP mode.

## Test Plan

- Unit tests for config parsing, config discovery, interactive role selection, transport selection, challenge-response authentication, and protocol frame parsing.
- Integration tests for TCP forwarding, UDP direct forwarding, UDP-over-TCP forwarding, TLS mode, raw mode, duplicate remote port rejection, invalid token rejection, and multiple clients.
- CI runs `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test`.

## GitHub & Release Plan

- Initialize Git and create or push GitHub repo `erp`.
- Add CI workflow for format, lint, and test.
- Add release workflow triggered by tags like `v0.1.0`.
- Publish x64 binaries for Windows, Linux, and macOS.

## Assumptions

- `tls` is the recommended default transport; `raw` is explicitly supported.
- TOML is the only config format in v1.
- Shared-token dynamic registration is acceptable for v1.
- Missing `server` or `client` always triggers an interactive role prompt.
