#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="$ROOT/erp-gui/android/app/src/main/jniLibs"

rustup target add aarch64-linux-android x86_64-linux-android

if ! command -v cargo-ndk >/dev/null 2>&1; then
  cargo install cargo-ndk --locked
fi

rm -rf "$OUT/arm64-v8a" "$OUT/x86_64"
mkdir -p "$OUT/arm64-v8a" "$OUT/x86_64"

(
  cd "$ROOT/erp"
  cargo ndk -t arm64-v8a -t x86_64 build --release --bin erp
)

cp "$ROOT/erp/target/aarch64-linux-android/release/erp" "$OUT/arm64-v8a/liberp_exec.so"
cp "$ROOT/erp/target/x86_64-linux-android/release/erp" "$OUT/x86_64/liberp_exec.so"

ls -lh "$OUT/arm64-v8a/liberp_exec.so" "$OUT/x86_64/liberp_exec.so"
