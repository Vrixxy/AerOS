# Converts the hand-designed dock/app-grid icon artwork in
# assets\icons\src\*.png into one raw 64x64 straight-alpha RGBA bitmap per
# icon, packed in a fixed order into assets\desktop-icons.bin (read by
# kernel\src\desktop.rs's draw_icon). Each source image is fit (not
# stretched) into the 64x64 square, centered, transparent padding on the
# short axis, so odd aspect ratios don't distort.
param([switch]$Force)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
$root = Split-Path -Parent $PSScriptRoot
$srcDir = Join-Path $root "assets/icons/src"
$output = Join-Path $root "assets/desktop-icons.bin"
$size = 64

# Order must match kernel/src/desktop.rs's icon(index) lookup.
$names = @("apps", "terminal", "files-dock", "browser", "notes", "trash", "files-grid")

if ((Test-Path -LiteralPath $output) -and -not $Force) {
    $expected = $names.Count * $size * $size * 4
    if ((Get-Item -LiteralPath $output).Length -eq $expected) {
        Write-Output "desktop icons up to date ($($names.Count) icons)"
        return
    }
}

$bytes = New-Object byte[] ($names.Count * $size * $size * 4)
for ($index = 0; $index -lt $names.Count; $index++) {
    $png = Join-Path $srcDir "$($names[$index]).png"
    $source = [System.Drawing.Image]::FromFile($png)
    $scale = [Math]::Min($size / $source.Width, $size / $source.Height)
    $drawWidth = [Math]::Round($source.Width * $scale)
    $drawHeight = [Math]::Round($source.Height * $scale)
    $offsetX = [Math]::Floor(($size - $drawWidth) / 2)
    $offsetY = [Math]::Floor(($size - $drawHeight) / 2)
    $bitmap = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.Clear([System.Drawing.Color]::Transparent)
    $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $graphics.DrawImage($source, $offsetX, $offsetY, $drawWidth, $drawHeight)
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
Write-Output "wrote $output ($($names.Count) icons)"
