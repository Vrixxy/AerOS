# Downloads each catalog app's icon from Flathub and converts it to a raw
# 64x64 RGBA bitmap (straight alpha), packed in catalog order into
# assets\store-icons.bin. Apps whose icon can't be fetched stay fully
# transparent, and the store draws its coloured letter tile instead.
param([switch]$Force)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
$root = Split-Path -Parent $PSScriptRoot
$store = Get-Content -Raw -LiteralPath (Join-Path $root "kernel\src\store.rs")
$ids = [regex]::Matches($store, 'flatpak:\s*"([^"]+)"') | ForEach-Object { $_.Groups[1].Value }
$cache = Join-Path $root "build\store-icons"
$output = Join-Path $root "assets\store-icons.bin"
$size = 64
New-Item -ItemType Directory -Force -Path $cache | Out-Null

if ((Test-Path -LiteralPath $output) -and -not $Force) {
    $expected = $ids.Count * $size * $size * 4
    if ((Get-Item -LiteralPath $output).Length -eq $expected) {
        Write-Output "store icons up to date ($($ids.Count) apps)"
        return
    }
}

$bytes = New-Object byte[] ($ids.Count * $size * $size * 4)
$missing = @()
for ($index = 0; $index -lt $ids.Count; $index++) {
    $id = $ids[$index]
    $png = Join-Path $cache "$id.png"
    if (-not (Test-Path -LiteralPath $png)) {
        $url = "https://dl.flathub.org/repo/appstream/x86_64/icons/128x128/$id.png"
        try {
            Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $png -TimeoutSec 30
        } catch {
            $missing += $id
            continue
        }
    }
    $source = [System.Drawing.Image]::FromFile($png)
    $bitmap = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.Clear([System.Drawing.Color]::Transparent)
    $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $graphics.DrawImage($source, 0, 0, $size, $size)
    $graphics.Dispose()
    $source.Dispose()
    $base = $index * $size * $size * 4
    for ($y = 0; $y -lt $size; $y++) {
        for ($x = 0; $x -lt $size; $x++) {
            $pixel = $bitmap.GetPixel($x, $y)
            $offset = $base + ($y * $size + $x) * 4
            $bytes[$offset] = $pixel.R
            $bytes[$offset + 1] = $pixel.G
            $bytes[$offset + 2] = $pixel.B
            $bytes[$offset + 3] = $pixel.A
        }
    }
    $bitmap.Dispose()
}
[System.IO.File]::WriteAllBytes($output, $bytes)
Write-Output "wrote $output ($($ids.Count - $missing.Count)/$($ids.Count) icons)"
if ($missing.Count -gt 0) {
    Write-Warning ("no icon for: " + ($missing -join ", "))
}
