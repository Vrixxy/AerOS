$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$projectRoot = Join-Path $root "compat\agent"
$binary = Join-Path $root "compat\agent\target\x86_64-unknown-linux-musl\release\aer-guest-agent"
$destination = Join-Path $root "build\compat\aer-guest-agent"

Push-Location $projectRoot
try {
    cargo build --release --target x86_64-unknown-linux-musl
    if ($LASTEXITCODE -ne 0) {
        throw "AerOS guest agent build failed"
    }
} finally {
    Pop-Location
}
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destination) | Out-Null
Copy-Item -Force -LiteralPath $binary -Destination $destination
Write-Output $destination
