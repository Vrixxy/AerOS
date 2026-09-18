param([Parameter(Mandatory = $true, ValueFromRemainingArguments = $true)][string[]]$Command)

$ErrorActionPreference = "Stop"
$text = ($Command -join " ")
$client = [System.Net.Sockets.TcpClient]::new("127.0.0.1", 45510)
$stream = $client.GetStream()
$reader = [System.IO.StreamReader]::new($stream)
$writer = [System.IO.StreamWriter]::new($stream)
$writer.NewLine = "`n"
$writer.AutoFlush = $true

Start-Sleep -Milliseconds 150
while ($stream.DataAvailable) { $null = $reader.ReadLine() }

$writer.WriteLine($text)
Start-Sleep -Milliseconds 250
while ($stream.DataAvailable) { Write-Output $reader.ReadLine() }

$writer.Close(); $reader.Close(); $client.Close()
