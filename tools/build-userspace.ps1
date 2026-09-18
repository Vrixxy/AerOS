$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$projects = @(
    @("init", "aeros-init"),
    @("std-smoke", "aeros-std-smoke")
)
$destinationRoot = Join-Path $root "assets\userspace"

New-Item -ItemType Directory -Force -Path $destinationRoot | Out-Null
foreach ($entry in $projects) {
    $project = Join-Path $root "userspace\$($entry[0])"
    $name = $entry[1]
    Push-Location $project
    try {
        cargo build --release --target x86_64-unknown-linux-musl
        if ($LASTEXITCODE -ne 0) {
            throw "AerOS userspace build failed"
        }
    } finally {
        Pop-Location
    }
    $binary = Join-Path $project "target\x86_64-unknown-linux-musl\release\$name"
    Copy-Item -Force -LiteralPath $binary -Destination (Join-Path $destinationRoot $name)
}

Write-Output $destinationRoot
