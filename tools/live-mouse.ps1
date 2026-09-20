param(
    [Parameter(Mandatory = $true)][int]$X,
    [Parameter(Mandatory = $true)][int]$Y,
    [switch]$Click,
    [switch]$Down,
    [switch]$Up,
    [ValidateSet("", "up", "down")][string]$Wheel = ""
)

# Moves the (absolute) pointer of the live QEMU session, optionally clicking.
# X/Y are screen pixels of the 1280x800 display; needs the QMP socket that
# tools\live-linux.ps1 opens on 127.0.0.1:45511.
$ErrorActionPreference = "Stop"
$client = [System.Net.Sockets.TcpClient]::new("127.0.0.1", 45511)
$stream = $client.GetStream()
$reader = [System.IO.StreamReader]::new($stream)
$writer = [System.IO.StreamWriter]::new($stream)
$writer.NewLine = "`n"; $writer.AutoFlush = $true
$null = $reader.ReadLine()
function Send($json) { $writer.WriteLine($json); $null = $reader.ReadLine() }
Send '{"execute":"qmp_capabilities"}'
$ax = [int]($X * 32767 / 1279); $ay = [int]($Y * 32767 / 799)
Send ('{"execute":"input-send-event","arguments":{"events":[{"type":"abs","data":{"axis":"x","value":' + $ax + '}},{"type":"abs","data":{"axis":"y","value":' + $ay + '}}]}}')
function Button($down) {
    Send ('{"execute":"input-send-event","arguments":{"events":[{"type":"btn","data":{"button":"left","down":' + $down.ToString().ToLower() + '}}]}}')
}
if ($Wheel -ne "") {
    $name = "wheel-$Wheel"
    for ($i = 0; $i -lt 3; $i++) {
        Send ('{"execute":"input-send-event","arguments":{"events":[{"type":"btn","data":{"button":"' + $name + '","down":true}}]}}')
        Send ('{"execute":"input-send-event","arguments":{"events":[{"type":"btn","data":{"button":"' + $name + '","down":false}}]}}')
        Start-Sleep -Milliseconds 150
    }
}
if ($Click -or $Down) { Start-Sleep -Milliseconds 120; Button $true }
if ($Click -or $Up) { Start-Sleep -Milliseconds 120; Button $false }
$client.Close()