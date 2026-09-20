param([int]$MemoryMiB = 2048, [int]$TimeoutSeconds = 240, [switch]$Single, [switch]$Smp)

# Two virtual CPUs (feature linux-smp) are the default; -Single builds the
# one-CPU guest. (-Smp is accepted and does nothing extra.)
$Smp = -not $Single

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
    & cargo build --release --target x86_64-unknown-uefi --features $(if ($Smp) { 'linux-headless,linux-smp' } else { 'linux-headless' })
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
$rootDisk = Join-Path $root "build\rootfs-disk.img"
if (Test-Path -LiteralPath $rootDisk) { $arguments += @("-drive", "file=$rootDisk,format=raw,if=ide,index=1") }
$quoted = $arguments | ForEach-Object { if ($_ -match "\s") { '"' + $_ + '"' } else { $_ } }
$start = [System.Diagnostics.ProcessStartInfo]::new()
$start.FileName = $qemu
$start.Arguments = $quoted -join " "
$start.WorkingDirectory = $root
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$process = [System.Diagnostics.Process]::Start($start)
# Poll the serial log and stop as soon as every success marker is present;
# otherwise give up at the timeout. (The desktop keeps QEMU alive after the
# guest run, so it never exits on its own.)
# The full Debian image (tools\build-debian-rootfs.sh) also starts an X
# session with a window manager, a terminal and a browser; the small busybox
# image (tools\build-rootfs.sh) doesn't.
$rootfs = Join-Path $esp "ROOTFS"
$graphical = (Test-Path -LiteralPath $rootfs) -and ((Get-Item -LiteralPath $rootfs).Length -gt 200MB)
$markers = @('Welcome to Linux inside AerOS', 'fb0: VESA VGA frame buffer device', 'AT Translated Set 2 keyboard', 'AEROS_LINUX_NET_OK', 'AEROS_LINUX_PING_OK')
if ($Smp) { $markers += @('AEROS_LINUX_CPUS 2', 'NMI backtrace for cpu 1') }
if ($graphical) { $markers += @('AEROS_LINUX_X_OK', 'AEROS_LINUX_APP_OK aeros-agent', 'AEROS_LINUX_APP_OK netsurf-gtk', 'AEROS_LINUX_MMAP_OK') }
$deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
while ([DateTime]::UtcNow -lt $deadline -and -not $process.HasExited) {
    Start-Sleep -Seconds 3
    if (Test-Path -LiteralPath $serial) {
        $text = try { $fs = [IO.File]::Open($serial, 'Open', 'Read', 'ReadWrite'); try { (New-Object IO.StreamReader($fs)).ReadToEnd() } finally { $fs.Dispose() } } catch { "" }
        if (-not ($markers | Where-Object { $text -notmatch [regex]::Escape($_) })) { break }
    }
}
if (-not $process.HasExited) {
    # Process.Kill(bool) is .NET Core only; Windows PowerShell 5.1 needs the
    # plain overload (the swallowed exception used to leave QEMU running).
    try { $process.Kill() } catch { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue }
    $process.WaitForExit(5000) | Out-Null
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
    'EXT4-fs \(vda\): mounted filesystem',
    'AEROS_LINUX_INIT_OK pid=1',
    'Welcome to Linux inside AerOS',
    'fb0: VESA VGA frame buffer device',
    'AT Translated Set 2 keyboard',
    'AEROS_LINUX_NET_OK',
    'AEROS_LINUX_PING_OK'
)
if ($graphical) { $checks += @('AEROS_LINUX_INPUT kbd=/dev/input/event\d+ mouse=/dev/input/event\d+', 'AEROS_LINUX_X_OK', 'AEROS_LINUX_APP_OK openbox', 'AEROS_LINUX_APP_OK xterm', 'AEROS_LINUX_APP_OK aeros-agent', 'AEROS_LINUX_APP_OK netsurf-gtk', 'AEROS_LINUX_MMAP_OK', 'mode is 2048x1152x32') }
if ($Smp) { $checks += @('smpboot: Total of 2 processors activated', 'AEROS_LINUX_CPUS 2', 'NMI backtrace for cpu 1') }
$missing = $checks | Where-Object { $log -notmatch $_ }
if ($missing) {
    throw "AerOS Linux guest boot incomplete; missing: $($missing -join ', ')"
}
Write-Output ""
Write-Output "AerOS Linux guest boot OK: Debian kernel mounted the virtio-blk root and its /sbin/init reached a shell"
