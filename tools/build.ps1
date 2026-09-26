param([switch]$BootTest)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$fontBuilder = Join-Path $PSScriptRoot "build-fonts.ps1"
$userspaceBuilder = Join-Path $PSScriptRoot "build-userspace.ps1"
$esp = Join-Path $root "build\esp"
$boot = Join-Path $esp "EFI\BOOT"
$binary = Join-Path $root "target\x86_64-unknown-uefi\release\aeros-kernel.efi"

# The saved desktop account lives on the ESP (/data). Boot tests must not see
# or delete it, so tests park it in build\account.dat.saved; normal builds
# (run/live) put it back.
$accountLive = Join-Path $esp "account.dat"
$accountSaved = Join-Path $root "build\account.dat.saved"
if ($BootTest) {
    if (Test-Path -LiteralPath $accountLive) {
        Move-Item -Force -LiteralPath $accountLive -Destination $accountSaved
    }
} elseif ((Test-Path -LiteralPath $accountSaved) -and -not (Test-Path -LiteralPath $accountLive)) {
    New-Item -ItemType Directory -Force -Path $esp | Out-Null
    Move-Item -Force -LiteralPath $accountSaved -Destination $accountLive
}

$settingsLive = Join-Path $esp "settings.dat"
$settingsSaved = Join-Path $root "build\settings.dat.saved"
if ($BootTest) {
    if (Test-Path -LiteralPath $settingsLive) {
        Move-Item -Force -LiteralPath $settingsLive -Destination $settingsSaved
    }
} elseif ((Test-Path -LiteralPath $settingsSaved) -and -not (Test-Path -LiteralPath $settingsLive)) {
    Move-Item -Force -LiteralPath $settingsSaved -Destination $settingsLive
}

Push-Location $root
try {
    & $fontBuilder
    & $userspaceBuilder
    & (Join-Path $PSScriptRoot "build-store-icons.ps1")
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
