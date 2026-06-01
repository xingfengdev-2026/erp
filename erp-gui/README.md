# erp-gui

Flutter GUI for managing `erp` client profiles.

Current capabilities:

- Create, edit, persist, and delete client profiles.
- Configure SOCKS5 reverse proxy profiles or TCP/UDP forwarding nodes.
- Select configs from a v2rayNG-style list with row-level share, edit, and delete actions.
- Start a local Android/desktop SOCKS5 listener for SOCKS5 profiles.
- Export plain QR codes or encrypted `erp://import` links.
- Import plain or encrypted shared profiles.

Desktop builds start the Rust CLI directly when `erp` is on `PATH` or `ERP_BIN` points to a binary. Android APKs bundle the Rust CLI as `liberp_exec.so` for `arm64-v8a` and `x86_64`, then start the matching runtime from the app native library directory.

## Development

```powershell
flutter pub get
flutter analyze
flutter test
.\tool\build_android_runtime.ps1
flutter build apk --debug --split-per-abi --target-platform android-arm64,android-x64
flutter build windows --release
```
