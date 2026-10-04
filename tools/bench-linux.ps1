param(
    [int]$Runs = 3,
    [int]$MemoryMiB = 512,
    [int]$TimeoutSeconds = 240
)

# Runs the benchmark program as /init of a tiny initramfs on the Debian
# kernel the project ships, in the same QEMU configuration as the boot-test.
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$qemu = Join-Path $env:ProgramFiles "qemu\qemu-system-x86_64.exe"
$kernel = Join-Path $root "build\esp\VMLINUZ"
$program = Join-Path $root "assets\userspace\aeros-bench"
$initramfs = Join-Path $root "build\bench-linux.cpio"
python (Join-Path $PSScriptRoot "make-initramfs.py") $program $initramfs
if ($LASTEXITCODE -ne 0) { throw "initramfs build failed" }
for ($run = 1; $run -le $Runs; $run++) {
    $log = Join-Path $root "build\bench-linux-$run.log"
    if (Test-Path -LiteralPath $log) { Remove-Item -Force -LiteralPath $log }
    $arguments = @(
        "-machine", "q35,accel=whpx:tcg",
        "-smp", "1",
        "-m", "$MemoryMiB",
        "-cpu", "qemu64,+nx,+smep,+smap,+xsave,+xsaveopt,+rdrand,+rdtscp,+pdpe1gb",
        "-kernel", $kernel,
        "-initrd", $initramfs,
        "-append", "console=ttyS0 quiet panic=-1 rdinit=/init",
        "-display", "none",
        "-serial", "file:$log",
        "-no-reboot"
    )
    $start = [System.Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $qemu
    $start.Arguments = ($arguments | ForEach-Object { if ($_ -match "\s") { '"' + $_ + '"' } else { $_ } }) -join " "
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $process = [System.Diagnostics.Process]::Start($start)
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $done = $false
    while ([DateTime]::UtcNow -lt $deadline -and -not $process.HasExited) {
        Start-Sleep -Milliseconds 500
        if ((Test-Path -LiteralPath $log) -and ((Get-Content -Raw -LiteralPath $log) -match "AEROS_BENCH_DONE")) {
            $done = $true
            break
        }
    }
    if (-not $process.HasExited) { $process.Kill() }
    $process.WaitForExit()
    if (-not $done -and (Test-Path -LiteralPath $log) -and ((Get-Content -Raw -LiteralPath $log) -match "AEROS_BENCH_DONE")) { $done = $true }
    Write-Output "run ${run}: $(if ($done) { 'complete' } else { 'did not finish' })"
}
