param(
    [ValidateSet("gtk", "sdl", "vnc", "none")][string]$Display = "gtk",
    [switch]$Gl,
    [int]$Cpus = 2,
    [switch]$Monitor
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$esp = & (Join-Path $PSScriptRoot "build.ps1")
$qemu = Join-Path $env:ProgramFiles "qemu\qemu-system-x86_64.exe"
$firmware = Join-Path $env:ProgramFiles "qemu\share\edk2-x86_64-code.fd"
$variablesTemplate = Join-Path $env:ProgramFiles "qemu\share\edk2-i386-vars.fd"
$variables = Join-Path $root "build\edk2-vars.fd"

if (-not (Test-Path -LiteralPath $qemu)) {
    throw "QEMU was not found"
}

Copy-Item -Force -LiteralPath $variablesTemplate -Destination $variables
$disk = "format=raw,file=fat:rw:$esp"
$codeFlash = "if=pflash,format=raw,unit=0,readonly=on,file=$firmware"
$variableFlash = "if=pflash,format=raw,unit=1,file=$variables"

$glMode = if ($Gl) { "on" } else { "off" }
$displayArg = switch ($Display) {
    "vnc" { "vnc=127.0.0.1:0" }
    "none" { "none" }
    default { "$Display,gl=$glMode" }
}

$serial = Join-Path $root "build\run.log"
if ($Display -eq "vnc") {
    Write-Host "VNC server on 127.0.0.1:5900 - connect a VNC viewer there." -ForegroundColor Cyan
}
Write-Host "AerOS booting. Serial log: $serial  (watch with: Get-Content '$serial' -Wait -Tail 40)" -ForegroundColor Cyan

$monitorArgs = if ($Monitor) { @("-monitor", "tcp:127.0.0.1:45511,server,nowait") } else { @() }

& $qemu `
    -machine q35,accel=whpx:tcg `
    -m 512M `
    -smp $Cpus `
    -cpu "qemu64,+nx,+smep,+smap,+xsave,+xsaveopt,+rdrand,+rdtscp,+pdpe1gb" `
    -drive $codeFlash `
    -drive $variableFlash `
    -drive $disk `
    -device virtio-vga `
    -display $displayArg `
    -serial "file:$serial" `
    @monitorArgs `
    -no-reboot
