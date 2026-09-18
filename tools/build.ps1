param([switch]$BootTest)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$fontBuilder = Join-Path $PSScriptRoot "build-fonts.ps1"
$userspaceBuilder = Join-Path $PSScriptRoot "build-userspace.ps1"
$esp = Join-Path $root "build\esp"
$boot = Join-Path $esp "EFI\BOOT"
$binary = Join-Path $root "target\x86_64-unknown-uefi\release\aeros-kernel.efi"

Push-Location $root
try {
    & $fontBuilder
    & $userspaceBuilder
    if ($BootTest) {
        cargo build --release --target x86_64-unknown-uefi --features boot-test
    } else {
        cargo build --release --target x86_64-unknown-uefi
    }
    New-Item -ItemType Directory -Force -Path $boot | Out-Null
    Copy-Item -Force -LiteralPath $binary -Destination (Join-Path $boot "BOOTX64.EFI")
} finally {
    Pop-Location
}

Write-Output $esp
