param([Parameter(Mandatory = $true)][string]$Esp, [string]$Name = "esp-run")

# QEMU turns a directory into a FAT volume of at most 516 MB. With the Linux
# guest's 420 MB ROOTFS next to the kernel image, build\esp no longer fits, so
# the drive is a copy without ROOTFS (the guest takes its root from
# build\rootfs-disk.img, the disk labelled aeros-root). What the last session
# wrote to that copy (the account, settings, /data files) is brought back to
# build\esp first, so it survives the next build.
$ErrorActionPreference = "Stop"
$limit = 500MB
$total = (Get-ChildItem -Recurse -File -Force -LiteralPath $Esp | Measure-Object -Property Length -Sum).Sum
if ($total -le $limit) {
    return $Esp
}

$root = Split-Path -Parent $PSScriptRoot
$stage = Join-Path $root "build\$Name"
if (Test-Path -LiteralPath $stage) {
    Get-ChildItem -Recurse -File -Force -LiteralPath $stage | ForEach-Object {
        $target = Join-Path $Esp $_.FullName.Substring($stage.Length + 1)
        $stale = -not (Test-Path -LiteralPath $target)
        if (-not $stale) {
            $stale = $_.LastWriteTimeUtc -gt (Get-Item -LiteralPath $target).LastWriteTimeUtc
        }
        if ($stale) {
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $target) | Out-Null
            Copy-Item -Force -LiteralPath $_.FullName -Destination $target
        }
    }
    Remove-Item -Recurse -Force -LiteralPath $stage
}
robocopy $Esp $stage /E /XF ROOTFS /NFL /NDL /NJH /NJS /NP | Out-Null
if ($LASTEXITCODE -ge 8) {
    throw "could not copy $Esp to $stage (robocopy $LASTEXITCODE)"
}
$global:LASTEXITCODE = 0
Write-Host "build\esp is $([int]($total / 1MB)) MB, too big for QEMU's FAT volume: booting from build\$Name (no ROOTFS)" -ForegroundColor Yellow
return $stage
