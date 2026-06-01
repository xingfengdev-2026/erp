# erp

This repository contains two related projects:

- `erp/`: the Rust CLI reverse proxy.
- `erp-gui/`: the Flutter GUI for managing client profiles, local SOCKS5 access, QR export, `erp://` import links, and bundled Android `erp` runtime.

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
.\tool\build_android_runtime.ps1
flutter build apk --debug --split-per-abi --target-platform android-arm64,android-x64
flutter build windows --release
```

Release builds are handled by root GitHub Actions. Rust Linux releases use the musl target to avoid newer glibc runtime requirements on older servers. Android GUI releases are split into `arm64-v8a` and `x86_64` APKs with the matching `erp` CLI bundled inside each APK.
