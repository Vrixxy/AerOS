param([int]$MemoryMiB = 2048, [switch]$Single, [switch]$Smp, [ValidateSet("none", "gtk", "sdl")][string]$Display = "none", [string]$RootDisk = "build\rootfs-disk.img")

# Two virtual CPUs (feature linux-smp) are the default; -Single builds the
# one-CPU guest. (-Smp is accepted and does nothing extra.)
$Smp = -not $Single

# Live desktop session with the Linux guest available: same as
# live-qemu.ps1, but the kernel is built with --features linux-guest and QEMU
# gets enough RAM for the guest. Open the Linux window with the `l` key (or
# Apps > Linux); it needs VMLINUZ, INITRD and ROOTFS in build\esp.
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$esp = @(& (Join-Path $PSScriptRoot "build.ps1"))[-1]
# Its own target directory: sharing target\ with build.ps1's default-feature
# build made every run rebuild the whole kernel twice.
$targetDir = Join-Path $root "target\linux-guest"
$binary = Join-Path $targetDir "x86_64-unknown-uefi\release\aeros-kernel.efi"
Push-Location $root
try {
    # -Smp: two virtual CPUs (feature linux-smp).
    & cargo build --release --target x86_64-unknown-uefi --target-dir $targetDir --features $(if ($Smp) { 'linux-guest,linux-smp' } else { 'linux-guest' })
    if ($LASTEXITCODE -ne 0) { throw "kernel build failed (exit $LASTEXITCODE)" }
} finally {
    Pop-Location
}
Copy-Item -Force -LiteralPath $binary -Destination (Join-Path $esp "EFI\BOOT\BOOTX64.EFI")

# The previous session's QEMU holds the staged drive open; stop it first.
Get-Process qemu-system-x86_64 -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 400
$drive = & (Join-Path $PSScriptRoot "esp-drive.ps1") -Esp $esp -Name esp-live-linux

$qemu = Join-Path $env:ProgramFiles "qemu\qemu-system-x86_64.exe"
$firmware = Join-Path $env:ProgramFiles "qemu\share\edk2-x86_64-code.fd"
$variablesTemplate = Join-Path $env:ProgramFiles "qemu\share\edk2-i386-vars.fd"
$variables = Join-Path $root "build\edk2-vars-live.fd"

$rootDisk = Join-Path $root $RootDisk
$extraDisk = @()
if (Test-Path -LiteralPath $rootDisk) {
    # A dedicated root disk (tools\build-debian-rootfs.sh with FULL=1) - the
    # hypervisor uses any second disk labelled aeros-root instead of ROOTFS.
    $extraDisk = @("-drive", "file=$rootDisk,format=raw,if=ide,index=1")
}

Copy-Item -Force -LiteralPath $variablesTemplate -Destination $variables

& $qemu `
    -machine q35,accel=whpx:tcg `
    -m "${MemoryMiB}M" `
    -smp 2 `
    -cpu "qemu64,+nx,+smep,+smap,+xsave,+xsaveopt,+rdrand,+rdtscp,+pdpe1gb" `
    -drive "if=pflash,format=raw,unit=0,readonly=on,file=$firmware" `
    -drive "if=pflash,format=raw,unit=1,file=$variables" `
    -drive "format=raw,file=fat:rw:$drive" `
    -device virtio-vga `
    -display $Display `
    -serial "file:$root\build\live.log" `
    -monitor "tcp:127.0.0.1:45510,server,nowait" `
    -qmp "tcp:127.0.0.1:45511,server,nowait" `
    @extraDisk `
    -no-reboot
