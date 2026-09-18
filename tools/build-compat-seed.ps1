$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$agent = Join-Path $root "build\compat\aer-guest-agent"
$output = Join-Path $root "build\compat\aeros-cidata.img"

if (-not (Test-Path -LiteralPath $agent)) {
    & (Join-Path $PSScriptRoot "build-compat-agent.ps1") | Out-Null
}

$agentBase64 = [Convert]::ToBase64String([IO.File]::ReadAllBytes($agent))
$service = @"
[Unit]
After=dev-virtio\x2dports-org.aeros.compat.device
Requires=dev-virtio\x2dports-org.aeros.compat.device

[Service]
Type=simple
ExecStart=/usr/local/libexec/aer-guest-agent
Restart=always
RestartSec=1
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
DevicePolicy=closed
DeviceAllow=/dev/virtio-ports/org.aeros.compat rw

[Install]
WantedBy=multi-user.target
"@
$serviceBase64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($service))
$cloudHeader = [char]35 + "cloud-config"
$userData = @"
$cloudHeader
disable_root: true
ssh_pwauth: false
write_files:
  - path: /usr/local/libexec/aer-guest-agent
    owner: root:root
    permissions: '0755'
    encoding: b64
    content: $agentBase64
  - path: /etc/systemd/system/aer-guest-agent.service
    owner: root:root
    permissions: '0644'
    encoding: b64
    content: $serviceBase64
runcmd:
  - [systemctl, daemon-reload]
  - [systemctl, enable, --now, aer-guest-agent.service]
  - [systemctl, disable, --now, ssh.service]
  - [systemctl, mask, apt-daily.service, apt-daily-upgrade.service]
"@
$metadata = "instance-id: aeros-compat-v1`nlocal-hostname: aeros-compat`n"

function Set-U16 {
    param([byte[]]$Buffer, [int]$Offset, [uint16]$Value)
    $Buffer[$Offset] = $Value -band 0xff
    $Buffer[$Offset + 1] = ($Value -shr 8) -band 0xff
}

function Set-U32 {
    param([byte[]]$Buffer, [int]$Offset, [uint32]$Value)
    for ($index = 0; $index -lt 4; $index++) {
        $Buffer[$Offset + $index] = ($Value -shr ($index * 8)) -band 0xff
    }
}

function Set-Fat12 {
    param([byte[]]$Fat, [int]$Cluster, [int]$Value)
    $offset = [int][Math]::Floor($Cluster * 3 / 2)
    if (($Cluster -band 1) -eq 0) {
        $Fat[$offset] = $Value -band 0xff
        $Fat[$offset + 1] = ($Fat[$offset + 1] -band 0xf0) -bor (($Value -shr 8) -band 0x0f)
    } else {
        $Fat[$offset] = ($Fat[$offset] -band 0x0f) -bor (($Value -shl 4) -band 0xf0)
        $Fat[$offset + 1] = ($Value -shr 4) -band 0xff
    }
}

function Get-ShortChecksum {
    param([byte[]]$Name)
    $sum = 0
    foreach ($value in $Name) {
        $sum = ((($sum -band 1) * 128) + ($sum -shr 1) + $value) -band 0xff
    }
    [byte]$sum
}

function Set-LfnEntry {
    param([byte[]]$Image, [int]$Offset, [string]$Name, [byte[]]$ShortName)
    $positions = @(1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30)
    $Image[$Offset] = 0x41
    $Image[$Offset + 11] = 0x0f
    $Image[$Offset + 12] = 0
    $Image[$Offset + 13] = Get-ShortChecksum $ShortName
    Set-U16 $Image ($Offset + 26) 0
    $characters = @([char[]]$Name | ForEach-Object { [int]$_ }) + 0
    while ($characters.Count -lt 13) {
        $characters += 0xffff
    }
    for ($index = 0; $index -lt 13; $index++) {
        Set-U16 $Image ($Offset + $positions[$index]) $characters[$index]
    }
}

$sectorBytes = 512
$totalSectors = 2880
$fatSectors = 9
$rootEntries = 224
$rootSectors = 14
$dataSector = 1 + 2 * $fatSectors + $rootSectors
$image = [byte[]]::new($sectorBytes * $totalSectors)
$image[0] = 0xeb
$image[1] = 0x3c
$image[2] = 0x90
[Array]::Copy([Text.Encoding]::ASCII.GetBytes("AEROS   "), 0, $image, 3, 8)
Set-U16 $image 11 $sectorBytes
$image[13] = 1
Set-U16 $image 14 1
$image[16] = 2
Set-U16 $image 17 $rootEntries
Set-U16 $image 19 $totalSectors
$image[21] = 0xf0
Set-U16 $image 22 $fatSectors
Set-U16 $image 24 18
Set-U16 $image 26 2
Set-U32 $image 28 0
Set-U32 $image 32 0
$image[36] = 0
$image[38] = 0x29
Set-U32 $image 39 2749367571
[Array]::Copy([Text.Encoding]::ASCII.GetBytes("CIDATA     "), 0, $image, 43, 11)
[Array]::Copy([Text.Encoding]::ASCII.GetBytes("FAT12   "), 0, $image, 54, 8)
$image[510] = 0x55
$image[511] = 0xaa

$fat = [byte[]]::new($fatSectors * $sectorBytes)
$fat[0] = 0xf0
$fat[1] = 0xff
$fat[2] = 0xff
$files = @(
    [PSCustomObject]@{ Name = "user-data"; Short = "USER-D~1   "; Data = [Text.Encoding]::UTF8.GetBytes($userData) },
    [PSCustomObject]@{ Name = "meta-data"; Short = "META-D~1   "; Data = [Text.Encoding]::UTF8.GetBytes($metadata) }
)
$cluster = 2
$rootOffset = (1 + 2 * $fatSectors) * $sectorBytes
[Array]::Copy([Text.Encoding]::ASCII.GetBytes("CIDATA     "), 0, $image, $rootOffset, 11)
$image[$rootOffset + 11] = 0x08
$directoryOffset = $rootOffset + 32

foreach ($file in $files) {
    $shortName = [Text.Encoding]::ASCII.GetBytes($file.Short)
    if ($shortName.Length -ne 11) {
        throw "Invalid FAT short name"
    }
    $clusters = [Math]::Max(1, [Math]::Ceiling($file.Data.Length / $sectorBytes))
    $firstCluster = $cluster
    for ($index = 0; $index -lt $clusters; $index++) {
        $next = if ($index -eq $clusters - 1) { 0xfff } else { $cluster + 1 }
        Set-Fat12 $fat $cluster $next
        $sourceOffset = $index * $sectorBytes
        $copyBytes = [Math]::Min($sectorBytes, $file.Data.Length - $sourceOffset)
        if ($copyBytes -gt 0) {
            $destinationOffset = ($dataSector + $cluster - 2) * $sectorBytes
            [Array]::Copy($file.Data, $sourceOffset, $image, $destinationOffset, $copyBytes)
        }
        $cluster++
    }
    Set-LfnEntry $image $directoryOffset $file.Name $shortName
    $directoryOffset += 32
    [Array]::Copy($shortName, 0, $image, $directoryOffset, 11)
    $image[$directoryOffset + 11] = 0x20
    Set-U16 $image ($directoryOffset + 26) $firstCluster
    Set-U32 $image ($directoryOffset + 28) $file.Data.Length
    $directoryOffset += 32
}
if ($cluster -ge $totalSectors - $dataSector + 2) {
    throw "Compatibility seed exceeds FAT12 capacity"
}
[Array]::Copy($fat, 0, $image, $sectorBytes, $fat.Length)
[Array]::Copy($fat, 0, $image, (1 + $fatSectors) * $sectorBytes, $fat.Length)
[IO.File]::WriteAllBytes($output, $image)
Write-Output $output
