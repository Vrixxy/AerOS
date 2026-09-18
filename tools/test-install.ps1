$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$esp = & (Join-Path $PSScriptRoot "build.ps1")
$qemu = Join-Path $env:ProgramFiles "qemu\qemu-system-x86_64.exe"
$firmware = Join-Path $env:ProgramFiles "qemu\share\edk2-x86_64-code.fd"
$variablesTemplate = Join-Path $env:ProgramFiles "qemu\share\edk2-i386-vars.fd"

$target = Join-Path $root "build\aeros-target.img"
$installVars = Join-Path $root "build\edk2-install-vars.fd"
$bootVars = Join-Path $root "build\edk2-installed-vars.fd"
$installLog = Join-Path $root "build\install.log"
$bootLog = Join-Path $root "build\installed-boot.log"

function New-BlankImage($path, $bytes) {
    if (Test-Path -LiteralPath $path) { Remove-Item -Force -LiteralPath $path }
    $fs = [System.IO.File]::Create($path)
    try { $fs.SetLength($bytes) } finally { $fs.Dispose() }
}

function Invoke-Qemu($arguments, $timeoutSeconds) {
    $quoted = $arguments | ForEach-Object { if ($_ -match '\s') { '"' + $_ + '"' } else { $_ } }
    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $qemu
    $psi.Arguments = $quoted -join " "
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $proc = [System.Diagnostics.Process]::Start($psi)
    if (-not $proc.WaitForExit($timeoutSeconds * 1000)) {
        try { $proc.Kill($true) } catch { }
        $proc.WaitForExit(3000) | Out-Null
    }
}

Write-Host "== stage 1: install AerOS onto a blank disk ==" -ForegroundColor Cyan
New-BlankImage $target (512MB)
Copy-Item -Force -LiteralPath $variablesTemplate -Destination $installVars
foreach ($p in @($installLog, $bootLog)) { if (Test-Path -LiteralPath $p) { Remove-Item -Force -LiteralPath $p } }

Invoke-Qemu @(
    "-machine", "q35,accel=whpx:tcg",
    "-m", "512M", "-smp", "2",
    "-cpu", "qemu64,+nx,+smep,+smap,+xsave,+xsaveopt,+rdrand,+rdtscp,+pdpe1gb",
    "-drive", "if=pflash,format=raw,unit=0,readonly=on,file=$firmware",
    "-drive", "if=pflash,format=raw,unit=1,file=$installVars",
    "-drive", "format=raw,file=fat:rw:$esp",
    "-drive", "format=raw,file=$target",
    "-display", "none",
    "-serial", "file:$installLog",
    "-no-reboot"
) 90

if (-not (Test-Path -LiteralPath $installLog)) { throw "no install serial log produced" }
$installOutput = Get-Content -Raw -LiteralPath $installLog
Write-Output ($installOutput -split "`r?`n" | Select-String -Pattern "AEROS_AHCI |AEROS_INSTALL ")

if ($installOutput -notmatch "AEROS_AHCI present=true .* disks=2 .* verified=true") {
    throw "AerOS did not see the second disk`n$installOutput"
}
if ($installOutput -notmatch "AEROS_INSTALL attempted=true target_disk=[1-9] .* gpt=true formatted=true kernel_written=true marker_written=true readback_ok=true verified=true") {
    throw "AerOS installer did not complete`n$installOutput"
}

Write-Host "== stage 2: boot from the installed disk only ==" -ForegroundColor Cyan
Copy-Item -Force -LiteralPath $variablesTemplate -Destination $bootVars

Invoke-Qemu @(
    "-machine", "q35,accel=whpx:tcg",
    "-m", "512M", "-smp", "2",
    "-cpu", "qemu64,+nx,+smep,+smap,+xsave,+xsaveopt,+rdrand,+rdtscp,+pdpe1gb",
    "-drive", "if=pflash,format=raw,unit=0,readonly=on,file=$firmware",
    "-drive", "if=pflash,format=raw,unit=1,file=$bootVars",
    "-drive", "format=raw,file=$target",
    "-display", "none",
    "-serial", "file:$bootLog",
    "-no-reboot"
) 60

if (-not (Test-Path -LiteralPath $bootLog)) { throw "no installed-boot serial log produced" }
$bootOutput = Get-Content -Raw -LiteralPath $bootLog
Write-Output ($bootOutput -split "`r?`n" | Select-String -Pattern "AEROS_BOOT |AEROS_PARTITIONS |AEROS_FAT |AEROS_READY")

if ($bootOutput -notmatch "AEROS_BOOT version=") {
    throw "installed AerOS did not boot`n$bootOutput"
}
if ($bootOutput -notmatch "AEROS_PARTITIONS .* gpt=true .* verified=true") {
    throw "installed disk GPT not recognized`n$bootOutput"
}
if ($bootOutput -notmatch "AEROS_FAT mounted=true bits=32 .* efi=true boot=true bootx64=true .* pe=true verified=true") {
    throw "installed FAT32 ESP not usable`n$bootOutput"
}
if ($bootOutput -notmatch "AEROS_READY") {
    throw "installed AerOS did not reach AEROS_READY`n$bootOutput"
}

Write-Host ""
Write-Host "AerOS install test passed: installed to a blank GPT disk and booted from it." -ForegroundColor Green
