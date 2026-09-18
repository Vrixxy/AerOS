param([int]$MemoryMiB = 2048, [int]$TimeoutSeconds = 240)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$esp = Join-Path $root "build\esp"
$boot = Join-Path $esp "EFI\BOOT"
$binary = Join-Path $root "target\x86_64-unknown-uefi\release\aeros-kernel.efi"
$qemu = Join-Path $env:ProgramFiles "qemu\qemu-system-x86_64.exe"
$firmware = Join-Path $env:ProgramFiles "qemu\share\edk2-x86_64-code.fd"
$variablesTemplate = Join-Path $env:ProgramFiles "qemu\share\edk2-i386-vars.fd"
$variables = Join-Path $root "build\edk2-vars-linux.fd"
$serial = Join-Path $root "build\linux-guest.log"

$vmlinuz = Join-Path $esp "VMLINUZ"
$initrd = Join-Path $esp "INITRD"
if (-not (Test-Path -LiteralPath $vmlinuz)) {
    throw "build\esp\VMLINUZ is missing. Extract a Debian bzImage there (see tools\extract-debian-kernel.ps1)."
}

Push-Location $root
try {
    & (Join-Path $PSScriptRoot "build-fonts.ps1")
    & (Join-Path $PSScriptRoot "build-userspace.ps1")
    & cargo build --release --target x86_64-unknown-uefi --features linux-guest
    if ($LASTEXITCODE -ne 0) { throw "kernel build failed (exit $LASTEXITCODE)" }
    if (-not (Test-Path -LiteralPath $binary)) { throw "kernel binary not produced" }
    New-Item -ItemType Directory -Force -Path $boot | Out-Null
    Copy-Item -Force -LiteralPath $binary -Destination (Join-Path $boot "BOOTX64.EFI")
} finally {
    Pop-Location
}

Get-Process qemu-system-x86_64 -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500

Copy-Item -Force -LiteralPath $variablesTemplate -Destination $variables
if (Test-Path -LiteralPath $serial) {
    try { Remove-Item -Force -LiteralPath $serial -ErrorAction Stop } catch { $serial = Join-Path $root ("build\linux-guest-{0}.log" -f (Get-Date -Format HHmmss)) }
}

$arguments = @(
    "-machine", "q35,accel=whpx:tcg",
    "-m", "${MemoryMiB}M",
    "-smp", "2",
    "-cpu", "qemu64,+nx,+smep,+smap,+xsave,+xsaveopt,+rdrand,+rdtscp,+pdpe1gb",
    "-drive", "if=pflash,format=raw,unit=0,readonly=on,file=$firmware",
    "-drive", "if=pflash,format=raw,unit=1,file=$variables",
    "-drive", "format=raw,file=fat:rw:$esp",
    "-device", "virtio-vga",
    "-serial", "file:$serial",
    "-no-reboot"
)
$quoted = $arguments | ForEach-Object { if ($_ -match "\s") { '"' + $_ + '"' } else { $_ } }
$start = [System.Diagnostics.ProcessStartInfo]::new()
$start.FileName = $qemu
$start.Arguments = $quoted -join " "
$start.WorkingDirectory = $root
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$process = [System.Diagnostics.Process]::Start($start)
if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
    try { $process.Kill($true) } catch { }
}

if (-not (Test-Path -LiteralPath $serial)) {
    throw "no serial log produced at $serial"
}
$log = Get-Content -Raw -LiteralPath $serial
Write-Output $log

$checks = @(
    'Linux version 6\.\d+',
    'kvm-clock: Using msrs',
    'Calibrating delay loop \(skipped\)',
    'Run /init as init process',
    'AEROS_VM_LINUX_KERNEL stage=done'
)
$missing = $checks | Where-Object { $log -notmatch $_ }
if ($missing) {
    throw "AerOS Linux guest boot incomplete; missing: $($missing -join ', ')"
}
Write-Output ""
Write-Output "AerOS Linux guest boot OK: Debian kernel reached userspace /init"
