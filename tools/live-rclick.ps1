param(
    [Parameter(Mandatory = $true)][int]$X,
    [Parameter(Mandatory = $true)][int]$Y
)

# Moves the live QEMU pointer and right-clicks (dev helper; see live-mouse.ps1).
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
Start-Sleep -Milliseconds 200
Send '{"execute":"input-send-event","arguments":{"events":[{"type":"btn","data":{"button":"right","down":true}}]}}'
Start-Sleep -Milliseconds 120
Send '{"execute":"input-send-event","arguments":{"events":[{"type":"btn","data":{"button":"right","down":false}}]}}'
$writer.Close(); $reader.Close(); $client.Close()
