$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$esp = [string]@(& (Join-Path $PSScriptRoot "build.ps1"))[-1]
if (-not (Test-Path -LiteralPath $esp -PathType Container)) { throw "build.ps1 did not return the ESP directory (got: $esp)" }
$qemu = Join-Path $env:ProgramFiles "qemu\qemu-system-x86_64.exe"
$firmware = Join-Path $env:ProgramFiles "qemu\share\edk2-x86_64-code.fd"
$variablesTemplate = Join-Path $env:ProgramFiles "qemu\share\edk2-i386-vars.fd"
$maker = Join-Path $PSScriptRoot "make-gpt-disk.py"

$target = Join-Path $root "build\aeros-existing.img"
$vars = Join-Path $root "build\edk2-existing-vars.fd"
$bootVars = Join-Path $root "build\edk2-existing-boot-vars.fd"
$log = Join-Path $root "build\install-existing.log"
$bootLog = Join-Path $root "build\install-existing-boot.log"
$config = Join-Path $esp "INSTALL.CFG"

function Invoke-Qemu($arguments, $timeoutSeconds) {
    $quoted = $arguments | ForEach-Object { if ($_ -match '\s') { '"' + $_ + '"' } else { $_ } }
    $psi = [System.Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $qemu
    $psi.Arguments = $quoted -join " "
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $proc = [System.Diagnostics.Process]::Start($psi)
    if (-not $proc.WaitForExit($timeoutSeconds * 1000)) {
        try { $proc.Kill() } catch { & taskkill /F /T /PID $proc.Id 2>$null | Out-Null }
        $proc.WaitForExit(3000) | Out-Null
    }
}

function Start-Install($label) {
    Copy-Item -Force -LiteralPath $variablesTemplate -Destination $vars
    if (Test-Path -LiteralPath $log) { Remove-Item -Force -LiteralPath $log }
    Invoke-Qemu @(
        "-machine", "q35,accel=whpx:tcg",
        "-m", "512M", "-smp", "2",
        "-cpu", "qemu64,+nx,+smep,+smap,+xsave,+xsaveopt,+rdrand,+rdtscp,+pdpe1gb",
        "-drive", "if=pflash,format=raw,unit=0,readonly=on,file=$firmware",
        "-drive", "if=pflash,format=raw,unit=1,file=$vars",
        "-drive", "format=raw,file=fat:rw:$esp",
        "-drive", "format=raw,file=$target",
        "-display", "none",
        "-serial", "file:$log",
        "-no-reboot"
    ) 45
    if (-not (Test-Path -LiteralPath $log)) { throw "no serial log produced ($label)" }
    return Get-Content -Raw -LiteralPath $log
}

Write-Host "== a disk with a partition table and no permission to install ==" -ForegroundColor Cyan
if (Test-Path -LiteralPath $config) { Remove-Item -Force -LiteralPath $config -ErrorAction SilentlyContinue }
$partitionHash = (python $maker make $target 640 96).Trim()
$before = (Get-FileHash -Algorithm SHA256 -LiteralPath $target).Hash
$output = Start-Install "without permission"
Write-Output ($output -split "`r?`n" | Select-String -Pattern "AEROS_INSTALL ")
if ($output -notmatch "AEROS_INSTALL attempted=true .* existing_table=false preserved=0 refusal=Some\(NotAllowed\) .* gpt=false formatted=false kernel_written=false marker_written=false readback_ok=false verified=false") {
    throw "AerOS installer did not refuse a disk with a table`n$output"
}
$after = (Get-FileHash -Algorithm SHA256 -LiteralPath $target).Hash
if ($before -ne $after) { throw "the disk changed although the installer refused" }

Write-Host "== the same disk with INSTALL.CFG asking for the free space ==" -ForegroundColor Cyan
Set-Content -LiteralPath $config -Value "free-space" -Encoding ascii
$output = Start-Install "with permission"
Write-Output ($output -split "`r?`n" | Select-String -Pattern "AEROS_INSTALL ")
if ($output -notmatch "AEROS_INSTALL attempted=true .* existing_table=true preserved=1 refusal=None .* gpt=true formatted=true kernel_written=true marker_written=true readback_ok=true verified=true") {
    throw "AerOS installer did not install into the free space`n$output"
}
$check = python $maker verify $target $partitionHash
if ($LASTEXITCODE -ne 0) { throw "disk check failed: $check" }
Write-Host $check

Write-Host "== boot from the installed disk only ==" -ForegroundColor Cyan
Remove-Item -Force -LiteralPath $config
Copy-Item -Force -LiteralPath $variablesTemplate -Destination $bootVars
if (Test-Path -LiteralPath $bootLog) { Remove-Item -Force -LiteralPath $bootLog }
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
if (-not (Test-Path -LiteralPath $bootLog)) { throw "no boot log produced" }
$bootOutput = Get-Content -Raw -LiteralPath $bootLog
if ($bootOutput -notmatch "AEROS_BOOT version=") { throw "installed AerOS did not boot`n$bootOutput" }
if ($bootOutput -notmatch "AEROS_PARTITIONS .* gpt=true .* count=2 .* verified=true") {
    throw "installed disk table not recognized with two partitions`n$bootOutput"
}
if ($bootOutput -notmatch "AEROS_FAT mounted=true bits=32 .* efi=true boot=true bootx64=true .* pe=true verified=true") {
    throw "installed FAT32 ESP not usable`n$bootOutput"
}
Write-Host ""
Write-Host "AerOS existing-disk install test passed: refused without permission, installed beside an existing partition without touching it, booted from the result." -ForegroundColor Green
