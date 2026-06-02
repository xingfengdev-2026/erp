param(
    [Parameter(Mandatory = $true)]
    [string]$Remote,
    [string]$RemotePath = "~/erp",
    [string]$LocalBinary = "dist/erp-x86_64-unknown-linux-gnu.2.17"
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path $LocalBinary)) {
    throw "Local binary not found: $LocalBinary"
}

$UploadPath = "$RemotePath.upload"
scp $LocalBinary "${Remote}:$UploadPath"
ssh $Remote "chmod +x $UploadPath && mv -f $UploadPath $RemotePath && $RemotePath --version"
