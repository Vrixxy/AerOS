$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$fontRoot = Join-Path $root "assets\fonts"

Add-Type -AssemblyName System.Drawing

function Set-Bytes {
    param([byte[]]$Target, [int]$Offset, [byte[]]$Value)
    [System.Buffer]::BlockCopy($Value, 0, $Target, $Offset, $Value.Length)
}

function Build-Font {
    param([string]$InputName, [string]$OutputName)

    $inputPath = Join-Path $fontRoot $InputName
    $outputPath = Join-Path $fontRoot $OutputName
    $privateFonts = [System.Drawing.Text.PrivateFontCollection]::new()
    $privateFonts.AddFontFile($inputPath)
    $family = $privateFonts.Families[0]
    $font = [System.Drawing.Font]::new($family, 64.0, [System.Drawing.FontStyle]::Regular, [System.Drawing.GraphicsUnit]::Pixel)
    $format = [System.Drawing.StringFormat]::GenericTypographic
    $format.FormatFlags = $format.FormatFlags -bor [System.Drawing.StringFormatFlags]::MeasureTrailingSpaces
    $width = 64
    $height = 80
    $first = 32
    $last = 126
    $count = $last - $first + 1
    $glyphPixels = $width * $height
    $bitmapOffset = 32
    $advanceOffset = $bitmapOffset + $count * $glyphPixels
    $totalSize = $advanceOffset + $count * 2
    $payload = [byte[]]::new($totalSize)
    Set-Bytes $payload 0 ([System.Text.Encoding]::ASCII.GetBytes("AERFNT01"))
    Set-Bytes $payload 8 ([System.BitConverter]::GetBytes([uint16]$first))
    Set-Bytes $payload 10 ([System.BitConverter]::GetBytes([uint16]$last))
    Set-Bytes $payload 12 ([System.BitConverter]::GetBytes([uint16]$width))
    Set-Bytes $payload 14 ([System.BitConverter]::GetBytes([uint16]$height))
    Set-Bytes $payload 16 ([System.BitConverter]::GetBytes([uint32]$bitmapOffset))
    Set-Bytes $payload 20 ([System.BitConverter]::GetBytes([uint32]$advanceOffset))
    Set-Bytes $payload 24 ([System.BitConverter]::GetBytes([uint32]$totalSize))

    for ($codepoint = $first; $codepoint -le $last; $codepoint++) {
        $index = $codepoint - $first
        $glyph = [string][char]$codepoint
        $bitmap = [System.Drawing.Bitmap]::new($width, $height, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        $graphics.Clear([System.Drawing.Color]::Transparent)
        $graphics.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
        $graphics.DrawString($glyph, $font, [System.Drawing.Brushes]::White, [System.Drawing.PointF]::new(2.0, 1.0), $format)
        $graphics.Flush()
        for ($y = 0; $y -lt $height; $y++) {
            for ($x = 0; $x -lt $width; $x++) {
                $payload[$bitmapOffset + $index * $glyphPixels + $y * $width + $x] = $bitmap.GetPixel($x, $y).A
            }
        }
        $measured = $graphics.MeasureString($glyph, $font, 4096, $format).Width
        $advance = [uint16][Math]::Max(1, [Math]::Min(65535, [Math]::Ceiling($measured)))
        Set-Bytes $payload ($advanceOffset + $index * 2) ([System.BitConverter]::GetBytes($advance))
        $graphics.Dispose()
        $bitmap.Dispose()
    }

    $font.Dispose()
    $privateFonts.Dispose()
    [System.IO.File]::WriteAllBytes($outputPath, $payload)
    Write-Output $outputPath
}

Build-Font "PlusJakartaSans-Regular.ttf" "PlusJakartaSans-Regular.aerfont"
Build-Font "RobotoMono-Variable.ttf" "RobotoMono-Regular.aerfont"

