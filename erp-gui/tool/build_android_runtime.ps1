$ErrorActionPreference = "Stop"

$Root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$Out = Join-Path $Root "erp-gui\android\app\src\main\jniLibs"

rustup target add aarch64-linux-android x86_64-linux-android

if (-not (Get-Command cargo-ndk -ErrorAction SilentlyContinue)) {
    cargo install cargo-ndk --locked
}

Remove-Item -Recurse -Force -ErrorAction SilentlyContinue (Join-Path $Out "arm64-v8a")
Remove-Item -Recurse -Force -ErrorAction SilentlyContinue (Join-Path $Out "x86_64")
New-Item -ItemType Directory -Force -Path (Join-Path $Out "arm64-v8a") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $Out "x86_64") | Out-Null

Push-Location (Join-Path $Root "erp")
try {
    cargo ndk -t arm64-v8a -t x86_64 build --release --bin erp
} finally {
    Pop-Location
}

Copy-Item -Force (Join-Path $Root "erp\target\aarch64-linux-android\release\erp") (Join-Path $Out "arm64-v8a\liberp_exec.so")
Copy-Item -Force (Join-Path $Root "erp\target\x86_64-linux-android\release\erp") (Join-Path $Out "x86_64\liberp_exec.so")

Get-ChildItem -Recurse $Out -Filter liberp_exec.so | Select-Object FullName, Length, LastWriteTime
