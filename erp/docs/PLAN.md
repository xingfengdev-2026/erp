# erp v1 Implementation Plan

## Summary

Build `erp` as a single Rust CLI binary for easy reverse proxying. It supports `server` and `client` modes, multiple TCP/UDP mappings, multiple clients, TOML configuration, selectable `raw` or AES-GCM transport, interactive setup, local compatibility builds, and GitHub Actions builds/releases.

## Key Changes

- Use Rust with `tokio`, `clap`, `serde`, `toml`, `bincode`, `aes-gcm`, `hkdf`, `hmac`, `sha2`, and `tracing`.
- Support `transport = "raw"` for unencrypted tunnels on trusted networks or during testing.
- Support `transport = "aes-256-gcm"` and `transport = "aes-128-gcm"` for encrypted post-auth frames and data.
- Treat `transport = "tls"` as a deprecated config value that is parsed but rejected with a clear error.
- Require token authentication in every transport mode.
- Authenticate with challenge-response: server sends a nonce, client returns an HMAC-SHA256 proof derived from the token, `client_id`, server nonce, client nonce, and timestamp.
- Never send the token directly, log it, or include it in error messages.
- Use HKDF-SHA256 to derive AES session keys after authentication.
- Default interactive setup to `raw`, then offer `aes-256-gcm` and `aes-128-gcm`.
- Encode tunnel protocol frames as length-prefixed binary messages.

## CLI Behavior

- `erp server --config server.toml` starts server mode.
- `erp client --config client.toml` starts client mode.
- Running only `erp` prompts the user to choose `client` or `server`.
- If `--config` is omitted, scan the executable directory for `*.toml`.
- If configs exist, prompt the user to select one or create a new config.
- If no config exists, ask setup questions interactively and generate a local TOML config.
- Client setup asks mapping name, protocol, local address, remote port, and UDP mode when relevant.

## Runtime Behavior

- Server listens on a configured control address and port.
- Server accepts multiple authenticated clients.
- Clients register multiple mappings with `name`, `protocol`, `local_addr`, `remote_port`, and optional `udp_mode`.
- Duplicate `remote_port` mappings are rejected and the existing mapping remains active.
- When a client disconnects, its remote ports, listener tasks, pending data sessions, and direct UDP routes are cleaned up.
- UDP supports both direct mode and UDP-over-TCP mode.

## Test Plan

- Unit tests for config parsing, transport selection, challenge-response authentication, AES frame encryption, direct UDP encryption, and protocol frame parsing.
- Integration tests for raw TCP forwarding, AES TCP forwarding, remote port reuse after client disconnect, UDP direct forwarding, UDP-over-TCP forwarding, invalid token rejection, and multiple clients.
- CI runs `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test`.

## GitHub & Release Plan

- Initialize Git and create or push GitHub repo `erp`.
- Add CI workflow for format, lint, and test.
- Add release workflow triggered by tags like `v0.1.0`.
- Publish x64 binaries for Windows, Linux musl, and macOS.
- Use local `scripts/build-local.ps1` for Windows and Linux glibc 2.17 artifacts.
- Use `scripts/deploy-test-remote.ps1` to upload and verify the Linux artifact on `user@example.com`.

## Assumptions

- `raw` is the default transport; `aes-256-gcm` is the recommended encrypted transport.
- TOML is the only config format in v1.
- Shared-token dynamic registration is acceptable for v1.
- Missing `server` or `client` always triggers an interactive role prompt.
- Linux compatibility target for local deployment is glibc 2.17.
