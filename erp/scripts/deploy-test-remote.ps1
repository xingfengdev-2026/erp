param(
    [Parameter(Mandatory = $true)]
    [string]$Remote,
    [string]$RemotePath = "/tmp/erp",
    [string]$LocalBinary = "dist/erp-x86_64-unknown-linux-gnu.2.17"
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path $LocalBinary)) {
    throw "Local binary not found: $LocalBinary"
}

scp $LocalBinary "${Remote}:$RemotePath"
ssh $Remote "chmod +x $RemotePath && $RemotePath --version"
