$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$image = Join-Path $root "build\compat\debian-13-generic-amd64.qcow2"
$esp = Join-Path $root "build\esp"
$seven = Join-Path $env:ProgramFiles "7-Zip\7z.exe"
$work = Join-Path $root "build\linux-extract"

if (-not (Test-Path -LiteralPath $seven)) { throw "7-Zip is required at $seven" }
if (-not (Test-Path -LiteralPath $image)) { throw "Debian image missing: run tools\build-compat-image.ps1 first ($image)" }

New-Item -ItemType Directory -Force -Path $work | Out-Null
New-Item -ItemType Directory -Force -Path $esp | Out-Null

& $seven x $image "-o$work" -y | Out-Null
$rootPart = Join-Path $work "0.img"
if (-not (Test-Path -LiteralPath $rootPart)) { throw "root partition (0.img) not found in $work" }

$list = & $seven l $rootPart "boot" | Select-String -Pattern "boot[\\/](vmlinuz|initrd\.img)-\S+"
$vmlinuz = ($list | Where-Object { $_ -match "vmlinuz-" } | Select-Object -First 1) -replace '.*\s(boot[\\/]\S+)$', '$1'
$initrd = ($list | Where-Object { $_ -match "initrd\.img-" } | Select-Object -First 1) -replace '.*\s(boot[\\/]\S+)$', '$1'
if (-not $vmlinuz -or -not $initrd) { throw "could not locate vmlinuz/initrd inside the image" }

& $seven e $rootPart $vmlinuz $initrd "-o$work" -y | Out-Null
Copy-Item -Force -LiteralPath (Join-Path $work (Split-Path -Leaf $vmlinuz)) -Destination (Join-Path $esp "VMLINUZ")
Copy-Item -Force -LiteralPath (Join-Path $work (Split-Path -Leaf $initrd)) -Destination (Join-Path $esp "INITRD")

Write-Output ("VMLINUZ  {0:N0} bytes" -f (Get-Item (Join-Path $esp "VMLINUZ")).Length)
Write-Output ("INITRD   {0:N0} bytes" -f (Get-Item (Join-Path $esp "INITRD")).Length)
