param([switch]$Refresh)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$configurationPath = Join-Path $root "compat\runtime.json"
$configuration = Get-Content -Raw -LiteralPath $configurationPath | ConvertFrom-Json
$destination = Join-Path $root "build\compat"
$base = Join-Path $destination $configuration.image
$partial = "$base.partial"
$checksums = Join-Path $destination "SHA512SUMS"
$overlay = Join-Path $destination "aeros-linux-apps.qcow2"
$manifest = Join-Path $destination "manifest.json"
$qemuImage = Join-Path $env:ProgramFiles "qemu\qemu-img.exe"
$curl = Join-Path $env:SystemRoot "System32\curl.exe"
$agent = & (Join-Path $PSScriptRoot "build-compat-agent.ps1")
$seed = & (Join-Path $PSScriptRoot "build-compat-seed.ps1")

if (-not $configuration.hardware_acceleration_required -or $configuration.software_emulation_allowed) {
    throw "Compatibility runtime must require hardware acceleration"
}
if (-not (Test-Path -LiteralPath $qemuImage)) {
    throw "qemu-img was not found"
}
if (-not (Test-Path -LiteralPath $curl)) {
    throw "curl was not found"
}

New-Item -ItemType Directory -Force -Path $destination | Out-Null
if ($Refresh) {
    foreach ($path in @($base, $partial, $checksums, $overlay)) {
        if (Test-Path -LiteralPath $path) {
            Remove-Item -Force -LiteralPath $path
        }
    }
}

Invoke-WebRequest -Uri $configuration.checksums -OutFile $checksums -UseBasicParsing
$escapedName = [Regex]::Escape($configuration.source_image)
$checksumLine = Get-Content -LiteralPath $checksums | Where-Object { $_ -match "^([0-9a-fA-F]{128})\s+\*?$escapedName$" } | Select-Object -First 1
if (-not $checksumLine) {
    throw "Official checksum for $($configuration.source_image) was not found"
}
$expected = ([Regex]::Match($checksumLine, "^[0-9a-fA-F]{128}")).Value.ToLowerInvariant()
if ($expected -ne $configuration.sha512.ToLowerInvariant()) {
    throw "Pinned Debian compatibility image checksum changed"
}

$validBase = $false
if (Test-Path -LiteralPath $base) {
    $actual = (Get-FileHash -Algorithm SHA512 -LiteralPath $base).Hash.ToLowerInvariant()
    $validBase = $actual -eq $expected
}
if (-not $validBase) {
    & $curl --fail --location --retry 3 --retry-delay 2 --continue-at - --output $partial $configuration.source
    if ($LASTEXITCODE -ne 0) {
        throw "Debian compatibility image download failed"
    }
    $actual = (Get-FileHash -Algorithm SHA512 -LiteralPath $partial).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        Remove-Item -Force -LiteralPath $partial
        throw "Debian compatibility image checksum mismatch"
    }
    Move-Item -LiteralPath $partial -Destination $base
}

$imageInfo = & $qemuImage info --output=json $base | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or $imageInfo.format -ne "qcow2") {
    throw "Debian compatibility image is not a valid qcow2 image"
}
if (-not (Test-Path -LiteralPath $overlay)) {
    & $qemuImage create -q -f qcow2 -F qcow2 -b $base $overlay "$($configuration.virtual_disk_gib)G"
    if ($LASTEXITCODE -ne 0) {
        throw "Compatibility overlay creation failed"
    }
}
$overlayInfo = & $qemuImage info --output=json $overlay | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or $overlayInfo.format -ne "qcow2") {
    throw "Compatibility overlay validation failed"
}
$agentSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $agent).Hash.ToLowerInvariant()
$manifestData = [PSCustomObject]@{
    Schema = 1
    Distribution = $configuration.distribution
    Release = $configuration.release
    Build = $configuration.build
    Architecture = $configuration.architecture
    BaseImage = $configuration.image
    BaseSha512 = $expected
    GuestAgent = "aer-guest-agent"
    GuestAgentSha256 = $agentSha256
    HardwareAccelerationRequired = $configuration.hardware_acceleration_required
    SoftwareEmulationAllowed = $configuration.software_emulation_allowed
}
$manifestData | ConvertTo-Json | Set-Content -Encoding UTF8 -LiteralPath $manifest

[PSCustomObject]@{
    Distribution = "$($configuration.distribution) $($configuration.release)"
    Architecture = $configuration.architecture
    BaseImage = $base
    BaseBytes = [long]$imageInfo.'actual-size'
    Overlay = $overlay
    GuestAgent = $agent
    ProvisioningSeed = $seed
    Manifest = $manifest
    VirtualDiskBytes = [long]$overlayInfo.'virtual-size'
    Sha512 = $expected
    HardwareAccelerationRequired = $configuration.hardware_acceleration_required
    SoftwareEmulationAllowed = $configuration.software_emulation_allowed
}
