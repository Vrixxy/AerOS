param(
    [int]$Count = 10,
    [int]$IntervalMs = 350,
    [string]$Prefix = "build\burst"
)

# Dumps the live QEMU screen $Count times, $IntervalMs apart (PPM files),
# for animation checks. Convert with tools\ppm2png.py.
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$client = [System.Net.Sockets.TcpClient]::new("127.0.0.1", 45510)
$stream = $client.GetStream()
$writer = [System.IO.StreamWriter]::new($stream)
$writer.NewLine = "`n"; $writer.AutoFlush = $true
Start-Sleep -Milliseconds 200
for ($i = 0; $i -lt $Count; $i++) {
    $path = Join-Path $root ("{0}{1:00}.ppm" -f $Prefix, $i)
    $writer.WriteLine("screendump $path")
    Start-Sleep -Milliseconds $IntervalMs
}
$writer.Close(); $client.Close()
