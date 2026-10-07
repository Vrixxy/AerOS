param(
    [ValidateRange(2, 40)]
    [int]$Boots = 8
)

# Boots the kernel image in build\esp several times and reports where it put
# itself each time (the AEROS_KASLR line). It does not wait for the desktop.
# Run tools\test.ps1 or tools\build.ps1 first so build\esp holds an image.

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$esp = Join-Path $root "build\esp"
$qemu = Join-Path $env:ProgramFiles "qemu\qemu-system-x86_64.exe"
$firmware = Join-Path $env:ProgramFiles "qemu\share\edk2-x86_64-code.fd"
$template = Join-Path $env:ProgramFiles "qemu\share\edk2-i386-vars.fd"
$bases = @()

for ($index = 1; $index -le $Boots; $index++) {
    $variables = Join-Path $root "build\kaslr-vars-$index.fd"
    $serial = Join-Path $root "build\kaslr-$index.log"
    Copy-Item -Force -LiteralPath $template -Destination $variables
    if (Test-Path -LiteralPath $serial) { Remove-Item -Force -LiteralPath $serial }
    $arguments = @(
        "-machine", "q35,accel=whpx:tcg", "-m", "512M", "-smp", "2",
        "-cpu", "qemu64,+nx,+smep,+smap,+xsave,+xsaveopt,+rdrand,+rdtscp,+pdpe1gb",
        "-drive", "if=pflash,format=raw,unit=0,readonly=on,file=$firmware",
        "-drive", "if=pflash,format=raw,unit=1,file=$variables",
        "-drive", "format=raw,file=fat:rw:$esp",
        "-display", "none", "-serial", "file:$serial", "-no-reboot") |
        ForEach-Object { if ($_ -match "\s") { '"' + $_ + '"' } else { $_ } }
    $process = Start-Process -FilePath $qemu -PassThru -WindowStyle Hidden -ArgumentList ($arguments -join " ")
    $line = $null
    $deadline = [DateTime]::UtcNow.AddSeconds(150)
    while ([DateTime]::UtcNow -lt $deadline -and $null -eq $line) {
        Start-Sleep -Milliseconds 500
        if (Test-Path -LiteralPath $serial) {
            $line = Select-String -Path $serial -Pattern "^AEROS_KASLR " -ErrorAction SilentlyContinue |
                Select-Object -First 1
        }
    }
    if (-not $process.HasExited) { Stop-Process -Id $process.Id -Force }
    Remove-Item -Force -LiteralPath $variables -ErrorAction SilentlyContinue
    if ($null -eq $line) { throw "boot ${index}: no AEROS_KASLR line within 150 s" }
    $text = $line.Line
    if ($text -notmatch "relocated=true .* base=(0x[0-9a-f]+) .* verified=true") {
        throw "boot ${index}: kernel did not relocate correctly: $text"
    }
    $bases += $Matches[1]
    Write-Output "boot ${index}: $($Matches[1])"
}

$distinct = ($bases | Sort-Object -Unique).Count
Write-Output "$distinct distinct base addresses in $Boots boots"
if ($distinct -lt [Math]::Ceiling($Boots / 2)) {
    throw "the base address barely varies ($distinct distinct in $Boots boots)"
}
