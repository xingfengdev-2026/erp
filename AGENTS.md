# Repository Guidelines

## Project Structure & Module Organization

This monorepo has two deliverables:

- `erp/` contains the Rust CLI, examples, scripts, tests, and docs.
- `erp-gui/` contains the Flutter GUI: app code in `erp-gui/lib/`, tests in `erp-gui/test/`, and Android packaging in `erp-gui/android/`.
- `.github/workflows/` builds and releases CLI and GUI artifacts.
- Generated folders (`erp/target/`, `erp/dist/`, `erp-gui/build/`, `.dart_tool/`, Android `jniLibs/`) stay untracked.

The implementation plan is in `erp/docs/PLAN.md`.

## Build, Test, and Development Commands

From `erp/`:

- `cargo run -- server --config examples/server.raw.toml`: start server mode.
- `cargo run -- client --config examples/client.raw.toml`: start client mode.
- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`: run format, lint, and test checks.
- `.\scripts\build-local.ps1`: build local Windows and Linux artifacts.
- `scp dist/release-v*/erp-x86_64-unknown-linux-musl root@phi.nam2.uk:/root/erp`: deploy the release Linux binary.

From `erp-gui/`:

- `flutter pub get`, `flutter analyze`, `flutter test`: install dependencies and run checks.
- `.\tool\build_android_runtime.ps1`: build bundled Android `erp` runtimes for `arm64-v8a` and `x86_64`.
- `flutter build apk --release --split-per-abi --target-platform android-arm64,android-x64`: build signed split APKs. If Gradle cache locks remain on Windows, add `--android-project-cache-dir $env:TEMP\erp-gradle-project-cache`.
- `flutter build windows --release`: build the Windows GUI.

## Coding Style & Naming Conventions

Use `rustfmt` for Rust and `dart format` for Flutter. Rust uses `snake_case` for functions/modules, `PascalCase` for types, and lowercase underscore TOML fields such as `control_port`. Dart files use `snake_case.dart`; classes and widgets use `PascalCase`.

## Testing Guidelines

Add Rust tests for forwarding, authentication, encryption, cleanup, and config parsing. Keep unit tests near the code and end-to-end cases in `erp/tests/`. Add Flutter tests for profile editing, sharing links, QR/export, and SOCKS5 UI state.

For remote validation, use `root@phi.nam2.uk`: binary `/root/erp`, server config `/root/server.toml`. Start high-load servers with `ulimit -n 1048576` plus `ERP_NOFILE=1048576`, `ERP_LISTEN_BACKLOG=65535`, `ERP_MAX_AUTH_HANDSHAKES`, `ERP_MAX_PENDING`, and `ERP_CONTROL_CHANNEL_CAPACITY` tuned for the test. Before load testing, verify `/proc/<pid>/limits` shows `Max open files` near `1048576`. For Android, install `erp-gui-arm64-v8a.apk`, confirm `lib/arm64-v8a/liberp_exec.so`, start the app, and check logcat.

## Release & Signing Notes

Linux releases must use `x86_64-unknown-linux-musl` to avoid GLIBC failures. Android signing uses `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`, and `ANDROID_KEY_PASSWORD`; never commit keystores. Expected APK signer SHA-256 digest: `9192405bbebf8c233555afd77fc7a65d6b6ed04510051ea0e24f2e79a93e14db`. Older random-signed APKs need one uninstall before fixed-key upgrades.

## Commit & Pull Request Guidelines

Use concise Conventional Commit-style messages, for example `feat: add gui profile import`. Pull requests should include a summary, test results, linked issues, and screenshots for GUI changes.

## Security & Configuration Notes

All transport modes require token authentication. Never log tokens or send them directly over the network. Treat `raw` mode as trusted-network/debug mode; prefer `aes-256-gcm` when traffic should not be visible on the wire.
