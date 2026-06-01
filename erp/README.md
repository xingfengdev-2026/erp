# erp

`erp` is an easy-to-use reverse proxy written in Rust. It exposes ports on a public server and forwards TCP or UDP traffic to services running behind an `erp client`.

## Quick Start

Start a server:

```powershell
cargo run -- server --config examples/server.raw.toml
```

Start a client:

```powershell
cargo run -- client --config examples/client.raw.toml
```

Running `erp` without `server` or `client` opens an interactive role prompt. If no config path is provided, `erp` looks for `*.toml` beside the executable and asks whether to use an existing config or create a new one.

## Configuration

Configs are TOML files with a top-level `role`, `token`, and `transport`.

- `transport = "raw"` uses an unencrypted tunnel.
- `transport = "aes-256-gcm"` encrypts post-auth protocol and data frames.
- `transport = "aes-128-gcm"` is also supported.
- All modes require token authentication.
- The token is never sent directly; authentication uses a server nonce and HMAC-SHA256 proof.
- `transport = "tls"` is deprecated and rejected at runtime.

Client mappings support:

- `protocol = "tcp"` for TCP forwarding.
- `protocol = "udp"` with `udp_mode = "over_tcp"` or `udp_mode = "direct"`.

See `examples/` for raw and AES configs.

## Development

```powershell
cargo fmt --check
cargo clippy -- -D warnings
cargo test
cargo build --release
.\scripts\build-local.ps1
```

GitHub Actions run format, lint, and tests on pushes and pull requests. Tags like `v0.1.0` trigger release builds for Windows, Linux musl, and macOS x64.

Use `scripts/build-local.ps1` on Windows to build local Windows and Linux artifacts. The Linux artifact uses `cargo zigbuild --target x86_64-unknown-linux-gnu.2.17` for compatibility with older servers such as Ubuntu 22. Use `scripts/deploy-test-remote.ps1` to upload that Linux binary to `user@example.com` and run `erp --version`.
