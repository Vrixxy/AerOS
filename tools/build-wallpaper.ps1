$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$source = Join-Path $root "assets\wallpapers\aeros-mountains.jpeg"
$destination = Join-Path $root "assets\wallpapers\aeros-mountains.rgb565"

Add-Type -AssemblyName System.Drawing
$bitmap = [System.Drawing.Bitmap]::new($source)
$stream = [System.IO.File]::Open($destination, [System.IO.FileMode]::Create)
$writer = [System.IO.BinaryWriter]::new($stream)
try {
    for ($y = 0; $y -lt $bitmap.Height; $y++) {
        for ($x = 0; $x -lt $bitmap.Width; $x++) {
            $color = $bitmap.GetPixel($x, $y)
            $red = [uint16][Math]::Floor($color.R * 31 / 255)
            $green = [uint16][Math]::Floor($color.G * 63 / 255)
            $blue = [uint16][Math]::Floor($color.B * 31 / 255)
            $value = [uint16]($red * 2048 + $green * 32 + $blue)
            $writer.Write($value)
        }
    }
} finally {
    $writer.Dispose()
    $bitmap.Dispose()
}

Write-Output $destination
