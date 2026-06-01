# erp

This repository contains two related projects:

- `erp/`: the Rust CLI reverse proxy.
- `erp-gui/`: the Flutter GUI for managing client profiles, local SOCKS5 access, QR export, and `erp://` import links.

## Development

Run Rust checks from `erp/`:

```powershell
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

Run Flutter checks from `erp-gui/`:

```powershell
flutter pub get
flutter analyze
flutter test
flutter build apk --debug
```

Release builds are handled by root GitHub Actions. Rust Linux releases use the musl target to avoid newer glibc runtime requirements on older servers.
