# erp-gui

Flutter GUI for managing `erp` client profiles.

Current capabilities:

- Create, edit, persist, and delete client profiles.
- Configure TCP/UDP mappings, transport mode, token, client id, and server address.
- Start a local Android/desktop SOCKS5 listener for profile mappings.
- Export encrypted QR codes and `erp://import` links.
- Import shared profiles with a passphrase.
- Copy generated client TOML for use with the Rust CLI.

Desktop builds can start the Rust CLI directly when `erp` is on `PATH` or `ERP_BIN` points to a binary. Android currently includes the GUI, local SOCKS5 server, and sharing flows; the native Rust runtime bridge still needs to be packaged as an Android library before the Start erp button can launch tunnels on-device.

## Development

```powershell
flutter pub get
flutter analyze
flutter test
flutter build apk --debug
```
