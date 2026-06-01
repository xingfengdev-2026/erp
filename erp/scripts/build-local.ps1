param(
    [string]$LinuxTarget = "x86_64-unknown-linux-gnu.2.17",
    [string]$WindowsTarget = "x86_64-pc-windows-msvc",
    [string]$DistDir = "dist"
)

$ErrorActionPreference = "Stop"

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)]
        [scriptblock]$Command,
        [Parameter(Mandatory = $true)]
        [string]$Description
    )

    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$Description failed with exit code $LASTEXITCODE"
    }
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo is required"
}

if (-not (Get-Command zig -ErrorAction SilentlyContinue)) {
    $zigPackageDir = python -c "import pathlib, ziglang; print(pathlib.Path(ziglang.__file__).parent)" 2>$null
    if ($LASTEXITCODE -eq 0 -and (Test-Path (Join-Path $zigPackageDir "zig.exe"))) {
        $env:PATH = "$zigPackageDir;$env:PATH"
    }
}

if (-not (Get-Command zig -ErrorAction SilentlyContinue)) {
    throw "zig is required for Linux zigbuild; install Zig or the Python ziglang package"
}

Invoke-Checked { cargo zigbuild --help | Out-Null } "cargo zigbuild check"

New-Item -ItemType Directory -Force -Path $DistDir | Out-Null

Invoke-Checked { cargo build --release --target $WindowsTarget --bin erp } "Windows build"
Copy-Item -Force "target\$WindowsTarget\release\erp.exe" "$DistDir\erp-$WindowsTarget.exe"

Invoke-Checked { cargo zigbuild --release --target $LinuxTarget --bin erp } "Linux zigbuild"

$linuxCandidates = @(
    "target\$LinuxTarget\release\erp",
    "target\x86_64-unknown-linux-gnu\release\erp"
)

$linuxBinary = $linuxCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $linuxBinary) {
    throw "Linux binary not found after cargo zigbuild"
}

Copy-Item -Force $linuxBinary "$DistDir\erp-$LinuxTarget"

Get-ChildItem $DistDir | Select-Object Name, Length, LastWriteTime
