param(
    [string]$Accel = "whpx:tcg",
    [int]$CpuCount = 2,
    [switch]$NoBuild,
    [int]$WaitSeconds = 40
)

# Sleeps to RAM right after the processors start and wakes the machine from
# the monitor: a fast loop for the suspend code (the full test does the same
# at the end of a five-minute boot).
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$esp = Join-Path $root "build\esp-s3"
$boot = Join-Path $esp "EFI\BOOT"
$binary = Join-Path $root "target\x86_64-unknown-uefi\release\aeros-kernel.efi"
$qemu = Join-Path $env:ProgramFiles "qemu\qemu-system-x86_64.exe"
$firmware = Join-Path $env:ProgramFiles "qemu\share\edk2-x86_64-code.fd"
$variablesTemplate = Join-Path $env:ProgramFiles "qemu\share\edk2-i386-vars.fd"
$variables = Join-Path $root "build\edk2-s3-vars.fd"
$serial = Join-Path $root "build\s3-quick.log"
$debugcon = Join-Path $root "build\s3-debugcon.log"

if (-not $NoBuild) {
    Push-Location $root
    try {
        cargo build --release --target x86_64-unknown-uefi --features s3-early
        if ($LASTEXITCODE -ne 0) { throw "build failed" }
    } finally {
        Pop-Location
    }
}
New-Item -ItemType Directory -Force -Path $boot | Out-Null
Copy-Item -Force -LiteralPath $binary -Destination (Join-Path $boot "BOOTX64.EFI")
Copy-Item -Force -LiteralPath $variablesTemplate -Destination $variables
Remove-Item -Force -ErrorAction SilentlyContinue $serial, $debugcon

$arguments = @(
    "-machine", "q35,accel=$Accel",
    "-m", "512M",
    "-smp", "$CpuCount",
    "-cpu", "qemu64,+nx,+smep,+smap,+xsave,+xsaveopt,+rdrand,+rdtscp,+pdpe1gb",
    "-drive", "if=pflash,format=raw,unit=0,readonly=on,file=$firmware",
    "-drive", "if=pflash,format=raw,unit=1,file=$variables",
    "-drive", "format=raw,file=fat:rw:$esp",
    "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
    "-debugcon", "file:$debugcon",
    "-global", "isa-debugcon.iobase=0x402",
    "-display", "none",
    "-serial", "file:$serial",
    "-monitor", "tcp:127.0.0.1:17657,server,nowait",
    "-no-shutdown"
)
$start = [System.Diagnostics.ProcessStartInfo]::new()
$start.FileName = $qemu
$start.Arguments = ($arguments | ForEach-Object { if ($_ -match '\s') { '"' + $_ + '"' } else { $_ } }) -join " "
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.RedirectStandardError = $true
$process = [System.Diagnostics.Process]::Start($start)
$errors = $process.StandardError.ReadToEndAsync()

function Ask([string]$Text) {
    $client = [System.Net.Sockets.TcpClient]::new()
    try {
        $client.Connect("127.0.0.1", 17657)
        $stream = $client.GetStream()
        $stream.ReadTimeout = 2000
        Start-Sleep -Milliseconds 300
        $buffer = New-Object byte[] 65536
        $reply = ""
        try { $n = $stream.Read($buffer, 0, $buffer.Length); $reply += [Text.Encoding]::ASCII.GetString($buffer, 0, $n) } catch {}
        $bytes = [Text.Encoding]::ASCII.GetBytes("$Text`r`n")
        $stream.Write($bytes, 0, $bytes.Length)
        Start-Sleep -Milliseconds 800
        try { $n = $stream.Read($buffer, 0, $buffer.Length); $reply += [Text.Encoding]::ASCII.GetString($buffer, 0, $n) } catch {}
        return ($reply -replace "\x1b\[[0-9;]*[A-Za-z]", "" -replace "[^\x20-\x7e\r\n]", "")
    } catch {
        return "ERR $_"
    } finally {
        $client.Close()
    }
}

$deadline = [DateTime]::UtcNow.AddSeconds($WaitSeconds)
$woken = $false
$text = ""
while ([DateTime]::UtcNow -lt $deadline -and -not $process.HasExited) {
    if (Test-Path $serial) {
        $text = [string](Get-Content -Raw $serial)
        if (-not $woken -and $text -match "AEROS_S3_SLEEP") {
            Start-Sleep -Seconds 2
            Write-Output "--- before wake: $((Ask 'info status') -split "`n" | Select-String 'VM status')"
            $null = Ask "system_wakeup"
            $woken = $true
            Start-Sleep -Seconds 2
            Write-Output "--- after wake: $((Ask 'info status') -split "`n" | Select-String 'VM status')"
        }
        if ($text -match "AEROS_S3 supported") { break }
    }
    Start-Sleep -Milliseconds 300
}
if (-not $process.HasExited) {
    if ($woken -and $text -notmatch "AEROS_S3 supported") {
        Write-Output (Ask "info registers")
    }
    Stop-Process -Id $process.Id -Force
    $process.WaitForExit()
}
Write-Output "=== serial"
if (Test-Path $serial) { Get-Content $serial | Select-String "AEROS_S3|AEROS_SMP |AEROS_BOOT" }
Write-Output "=== debugcon"
if (Test-Path $debugcon) { Get-Content $debugcon | Select-Object -Last 40 }
Write-Output "=== qemu"
$errors.Result
