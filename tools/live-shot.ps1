param([string]$Out = "build\live.png")

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$ppm = Join-Path $root "build\live.ppm"
$png = if ([System.IO.Path]::IsPathRooted($Out)) { $Out } else { Join-Path $root $Out }
Remove-Item -Force -LiteralPath $ppm -ErrorAction SilentlyContinue

$client = [System.Net.Sockets.TcpClient]::new("127.0.0.1", 45510)
$stream = $client.GetStream()
$writer = [System.IO.StreamWriter]::new($stream)
$writer.NewLine = "`n"; $writer.AutoFlush = $true
Start-Sleep -Milliseconds 200
$writer.WriteLine("screendump $ppm")

$deadline = [DateTime]::UtcNow.AddSeconds(10)
while (-not (Test-Path -LiteralPath $ppm) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 100 }
Start-Sleep -Milliseconds 600
$writer.Close(); $client.Close()
if (-not (Test-Path -LiteralPath $ppm)) { throw "screendump produced no file - is live-qemu.ps1 running?" }

$bytes = [System.IO.File]::ReadAllBytes($ppm)
$pos = 0
function Read-Token {
    param([byte[]]$b, [ref]$p)
    while ($b[$p.Value] -eq 32 -or $b[$p.Value] -eq 10 -or $b[$p.Value] -eq 13 -or $b[$p.Value] -eq 9) { $p.Value++ }
    $s = ""
    while (-not ($b[$p.Value] -eq 32 -or $b[$p.Value] -eq 10 -or $b[$p.Value] -eq 13 -or $b[$p.Value] -eq 9)) {
        $s += [char]$b[$p.Value]; $p.Value++
    }
    $s
}
$magic = Read-Token $bytes ([ref]$pos)
$w = [int](Read-Token $bytes ([ref]$pos))
$h = [int](Read-Token $bytes ([ref]$pos))
$null = Read-Token $bytes ([ref]$pos)
$pos++
if ($magic -ne "P6") { throw "unexpected ppm magic $magic" }

Add-Type -AssemblyName System.Drawing
$bmp = [System.Drawing.Bitmap]::new($w, $h, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
$rect = [System.Drawing.Rectangle]::new(0, 0, $w, $h)
$data = $bmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::WriteOnly, [System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
$stride = $data.Stride
$buf = [byte[]]::new($stride * $h)
for ($y = 0; $y -lt $h; $y++) {
    $src = $pos + $y * $w * 3
    $dst = $y * $stride
    for ($x = 0; $x -lt $w; $x++) {
        $buf[$dst + $x * 3 + 2] = $bytes[$src + $x * 3]
        $buf[$dst + $x * 3 + 1] = $bytes[$src + $x * 3 + 1]
        $buf[$dst + $x * 3]     = $bytes[$src + $x * 3 + 2]
    }
}
[System.Runtime.InteropServices.Marshal]::Copy($buf, 0, $data.Scan0, $buf.Length)
$bmp.UnlockBits($data)
$bmp.Save($png, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output $png
