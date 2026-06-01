# Repository Guidelines

## Project Structure & Module Organization

This is a monorepo with two projects:

- `erp/` contains the Rust CLI reverse proxy, examples, scripts, tests, and Rust-specific docs.
- `erp-gui/` contains the Flutter GUI. App code lives in `erp-gui/lib/`, widget tests in `erp-gui/test/`, and Android packaging in `erp-gui/android/`.
- Root `.github/workflows/` contains CI and release automation for both projects.
- Generated folders such as `erp/target/`, `erp/dist/`, `erp-gui/build/`, and `erp-gui/.dart_tool/` must stay untracked.

The Rust implementation plan remains in `erp/docs/PLAN.md`.

## Build, Test, and Development Commands

Run Rust commands from `erp/`:

- `cargo run -- server --config examples/server.raw.toml`: start server mode.
- `cargo run -- client --config examples/client.raw.toml`: start client mode.
- `cargo fmt --check`: verify Rust formatting.
- `cargo clippy -- -D warnings`: run Rust lint checks.
- `cargo test`: run Rust unit and integration tests.
- `.\scripts\build-local.ps1`: build local Windows and Linux compatibility artifacts.

Run Flutter commands from `erp-gui/`:

- `flutter pub get`: install Dart dependencies.
- `flutter analyze`: run static analysis.
- `flutter test`: run widget/unit tests.
- `flutter build apk --debug`: build an Android debug APK.

## Coding Style & Naming Conventions

Use `rustfmt` for Rust and `dart format` for Flutter. Rust uses `snake_case` for functions and modules, `PascalCase` for types, and lowercase underscore TOML fields such as `control_port` and `udp_mode`. Dart files use `snake_case.dart`; classes and widgets use `PascalCase`.

## Testing Guidelines

Add Rust tests for forwarding, authentication, encryption, cleanup, and config parsing. Keep unit tests near the code under test and end-to-end cases in `erp/tests/`. Add Flutter tests for profile editing, sharing links, QR/export flows, and SOCKS5 UI state.

## Commit & Pull Request Guidelines

Use concise Conventional Commit-style messages, for example `feat: add gui profile import` or `fix: release linux musl build`. Pull requests should include a summary, test results, linked issues when relevant, and screenshots for GUI changes.

## Security & Configuration Notes

All transport modes require token authentication. Never log tokens or send them directly over the network. Treat `raw` mode as trusted-network/debug mode; prefer `aes-256-gcm` when traffic should not be visible on the wire.
