param([string]$RootImage = "build\rootfs-disk-webkit.img", [int]$DataMiB = 2400, [switch]$SkipKernel)

# Builds build\AerOS-Setup.exe: the Windows installer (installer\) with the
# AerOS files appended as a zip archive and a 16-byte trailer ("AEROSPKG" +
# the archive's offset). The kernel is the Linux-guest build; ROOTFS is the
# read-only ext4 root at the start of $RootImage followed by a fresh, empty
# ext4 data area of $DataMiB MiB (store installs live there and survive
# reboots); the whole file stays under FAT32's 4 GiB limit.
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$targetDir = Join-Path $root "target\linux-guest"
$kernel = Join-Path $targetDir "x86_64-unknown-uefi\release\aeros-kernel.efi"
$esp = Join-Path $root "build\esp"
$out = Join-Path $root "build\AerOS-Setup.exe"

Push-Location $root
try {
    if (-not $SkipKernel) {
        & (Join-Path $PSScriptRoot "build-fonts.ps1") | Out-Null
        & (Join-Path $PSScriptRoot "build-userspace.ps1") | Out-Null
        & cargo build --release --target x86_64-unknown-uefi --target-dir $targetDir --features linux-guest,linux-smp
        if ($LASTEXITCODE -ne 0) { throw "kernel build failed (exit $LASTEXITCODE)" }
    }
    & dotnet build installer\AerOSSetup.csproj -c Release -nologo -v q
    if ($LASTEXITCODE -ne 0) { throw "installer build failed (exit $LASTEXITCODE)" }
} finally {
    Pop-Location
}
$program = Join-Path $root "build\installer\bin\Release\net48\AerOS-Setup.exe"

# The root ends where the image's data area begins (tools\build-debian-rootfs.sh
# writes that offset into the image as /etc/aeros-data-offset; the ext4
# superblock says the same: blocks * block size).
$image = Join-Path $root $RootImage
$superblock = New-Object byte[] 1024
$stream = [IO.File]::OpenRead($image)
try {
    [void]$stream.Seek(1024, 'Begin')
    [void]$stream.Read($superblock, 0, 1024)
} finally { $stream.Dispose() }
if ([BitConverter]::ToUInt16($superblock, 0x38) -ne 0xEF53) { throw "$RootImage doesn't start with an ext4 root" }
$rootBytes = [int64][BitConverter]::ToUInt32($superblock, 4) * (1024 -shl [BitConverter]::ToUInt32($superblock, 0x18))
if ($rootBytes + [int64]$DataMiB * 1MB -ge 4GB) { throw "ROOTFS would not fit FAT32's 4 GiB file limit" }
$dataImage = Join-Path $root "build\installer\data-area.img"
$wslData = "/mnt/" + $dataImage.Substring(0, 1).ToLower() + ($dataImage.Substring(2) -replace '\\', '/')
& wsl -d Debian -u root -- sh -c "rm -f '$wslData' && truncate -s ${DataMiB}M '$wslData' && mkfs.ext4 -q -F -m 0 -O ^has_journal -E lazy_itable_init=1 -L aeros-data '$wslData'"
if ($LASTEXITCODE -ne 0) { throw "mkfs.ext4 for the data area failed" }

Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archivePath = Join-Path $root "build\installer\payload.zip"
if (Test-Path $archivePath) { Remove-Item -Force $archivePath }
$archive = [IO.Compression.ZipFile]::Open($archivePath, 'Create')
# One archive entry made of the given (file, length) parts in order; a
# length of -1 takes the whole file.
function Add-Entry([string]$Name, [object[]]$Parts) {
    $entry = $archive.CreateEntry($Name, [IO.Compression.CompressionLevel]::Optimal)
    $output = $entry.Open()
    try {
        foreach ($part in $Parts) {
            $input = [IO.File]::OpenRead($part[0])
            try {
                $left = if ($part[1] -ge 0) { [int64]$part[1] } else { $input.Length }
                $buffer = New-Object byte[] (1MB)
                while ($left -gt 0) {
                    $read = $input.Read($buffer, 0, [int][Math]::Min([int64]$buffer.Length, $left))
                    if ($read -le 0) { throw "$($part[0]) ended early" }
                    $output.Write($buffer, 0, $read)
                    $left -= $read
                }
            } finally { $input.Dispose() }
        }
    } finally { $output.Dispose() }
}
# Firmware for the MediaTek MT7921 Wi-Fi card (redistributable, from linux-firmware).
$firmware = Join-Path $root "build\firmware\mediatek"
$firmwareFiles = @{ "MT7961P.BIN" = "WIFI_MT7961_patch_mcu_1_2_hdr.bin"; "MT7961R.BIN" = "WIFI_RAM_CODE_MT7961_1.bin" }
New-Item -ItemType Directory -Force $firmware | Out-Null
foreach ($source in $firmwareFiles.Values) {
    $path = Join-Path $firmware $source
    if (-not (Test-Path $path)) {
        Invoke-WebRequest -UseBasicParsing -OutFile $path "https://git.kernel.org/pub/scm/linux/kernel/git/firmware/linux-firmware.git/plain/mediatek/$source"
    }
}
try {
    Add-Entry "EFI/BOOT/BOOTX64.EFI" @(, @($kernel, -1))
    foreach ($name in $firmwareFiles.Keys) {
        Add-Entry $name @(, @((Join-Path $firmware $firmwareFiles[$name]), -1))
    }
    Add-Entry "VMLINUZ" @(, @((Join-Path $esp "VMLINUZ"), -1))
    Add-Entry "INITRD" @(, @((Join-Path $esp "INITRD"), -1))
    Add-Entry "ROOTFS" @(@($image, $rootBytes), @($dataImage, -1))
    # INSTALL.TXT marks an installed system (AerOS then leaves its boot
    # partition's root alone apart from HOME, which Setup creates).
    $marker = Join-Path $root "build\installer\INSTALL.TXT"
    Set-Content -LiteralPath $marker -Value "AerOS, installed next to Windows by AerOS Setup." -Encoding ascii
    Add-Entry "INSTALL.TXT" @(, @($marker, -1))
} finally { $archive.Dispose() }

$exe = [IO.File]::Create($out)
try {
    foreach ($part in @($program, $archivePath)) {
        $input = [IO.File]::OpenRead($part)
        try { $input.CopyTo($exe) } finally { $input.Dispose() }
    }
    $trailer = [Text.Encoding]::ASCII.GetBytes("AEROSPKG") + [BitConverter]::GetBytes([int64](Get-Item $program).Length)
    $exe.Write($trailer, 0, 16)
} finally { $exe.Dispose() }
Remove-Item -Force $archivePath, $dataImage
Write-Output ("{0} ({1:N0} MB; ROOTFS {2:N0} MB)" -f $out, ((Get-Item $out).Length / 1MB), ($rootBytes / 1MB))
