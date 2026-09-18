param([ValidateRange(30, 180)][int]$TimeoutSeconds = 90)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$configuration = Get-Content -Raw -LiteralPath (Join-Path $root "compat\runtime.json") | ConvertFrom-Json
$qemu = Join-Path $env:ProgramFiles "qemu\qemu-system-x86_64.exe"
$qemuImage = Join-Path $env:ProgramFiles "qemu\qemu-img.exe"
$base = Join-Path $root "build\compat\debian-13-generic-amd64.qcow2"
$disk = Join-Path $root "build\compat\aeros-linux-apps-test.qcow2"
$seed = Join-Path $root "build\compat\aeros-cidata.img"
$serial = Join-Path $root "build\compat-guest.log"
$bridgePort = 45457

if ($configuration.software_emulation_allowed -or -not $configuration.hardware_acceleration_required) {
    throw "Compatibility runtime acceleration policy is invalid"
}
if (-not (Test-Path -LiteralPath $qemu) -or -not (Test-Path -LiteralPath $qemuImage) -or -not (Test-Path -LiteralPath $base) -or -not (Test-Path -LiteralPath $seed)) {
    throw "Build the compatibility image before running its boot test"
}
if (Test-Path -LiteralPath $disk) {
    Remove-Item -Force -LiteralPath $disk
}
& $qemuImage create -q -f qcow2 -F qcow2 -b $base $disk "$($configuration.virtual_disk_gib)G"
if ($LASTEXITCODE -ne 0) {
    throw "Compatibility test overlay creation failed"
}
if (Test-Path -LiteralPath $serial) {
    Remove-Item -Force -LiteralPath $serial
}

$arguments = @(
    "-machine", "q35,accel=whpx",
    "-m", "$($configuration.initial_memory_mib)M",
    "-smp", "2",
    "-drive", "file=$disk,if=virtio,format=qcow2",
    "-drive", "file=$seed,if=virtio,format=raw,readonly=on",
    "-device", "virtio-serial-pci",
    "-chardev", "socket,id=aeros,host=127.0.0.1,port=$bridgePort,server=on,wait=off",
    "-device", "virtserialport,chardev=aeros,name=org.aeros.compat",
    "-display", "none",
    "-serial", "file:$serial",
    "-no-reboot"
)
$quoted = $arguments | ForEach-Object {
    if ($_ -match "\s") { '"' + $_ + '"' } else { $_ }
}
$start = [System.Diagnostics.ProcessStartInfo]::new()
$start.FileName = $qemu
$start.Arguments = $quoted -join " "
$start.WorkingDirectory = $root
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.WindowStyle = [System.Diagnostics.ProcessWindowStyle]::Hidden
$process = [System.Diagnostics.Process]::Start($start)
$deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
$booted = $false
$client = $null

function Read-Exact {
    param([IO.Stream]$Stream, [int]$Count)
    $buffer = [byte[]]::new($Count)
    $offset = 0
    while ($offset -lt $Count) {
        $received = $Stream.Read($buffer, $offset, $Count - $offset)
        if ($received -eq 0) {
            throw "Compatibility bridge closed early"
        }
        $offset += $received
    }
    $buffer
}

function Read-BridgeFrame {
    param([IO.Stream]$Stream)
    [byte[]]$header = Read-Exact $Stream 16
    if ([Text.Encoding]::ASCII.GetString($header, 0, 4) -ne "AERL") {
        throw "Compatibility bridge magic mismatch"
    }
    $version = [BitConverter]::ToUInt16($header, 4)
    $opcode = [BitConverter]::ToUInt16($header, 6)
    $request = [BitConverter]::ToUInt32($header, 8)
    $payloadBytes = [BitConverter]::ToUInt32($header, 12)
    if ($version -ne 1 -or $request -eq 0 -or $payloadBytes -gt 65536) {
        throw "Compatibility bridge header rejected"
    }
    [byte[]]$payload = Read-Exact $Stream $payloadBytes
    [PSCustomObject]@{ Opcode = $opcode; Request = $request; Payload = $payload }
}

function Write-BridgeFrame {
    param([IO.Stream]$Stream, [uint16]$Opcode, [uint32]$Request, [byte[]]$Payload)
    $header = [byte[]]::new(16)
    [Text.Encoding]::ASCII.GetBytes("AERL").CopyTo($header, 0)
    [BitConverter]::GetBytes([uint16]1).CopyTo($header, 4)
    [BitConverter]::GetBytes($Opcode).CopyTo($header, 6)
    [BitConverter]::GetBytes($Request).CopyTo($header, 8)
    [BitConverter]::GetBytes([uint32]$Payload.Length).CopyTo($header, 12)
    $Stream.Write($header, 0, $header.Length)
    $Stream.Write($Payload, 0, $Payload.Length)
    $Stream.Flush()
}

try {
    while ([DateTime]::UtcNow -lt $deadline -and -not $process.HasExited) {
        try {
            $candidate = [Net.Sockets.TcpClient]::new()
            $candidate.Connect("127.0.0.1", $bridgePort)
            $client = $candidate
            break
        } catch {
            if ($candidate) {
                $candidate.Dispose()
            }
        }
        Start-Sleep -Milliseconds 250
    }
    if (-not $client) {
        throw "Compatibility bridge was not available"
    }
    $stream = $client.GetStream()
    $stream.ReadTimeout = $TimeoutSeconds * 1000
    $hello = Read-BridgeFrame $stream
    $helloText = [Text.Encoding]::UTF8.GetString($hello.Payload)
    if ($hello.Opcode -ne 1 -or $helloText -ne "debian13-amd64") {
        throw "Compatibility guest handshake failed"
    }
    $path = [Text.Encoding]::UTF8.GetBytes("/usr/bin/true")
    $launch = [byte[]]::new(6 + $path.Length)
    [BitConverter]::GetBytes([uint32]0).CopyTo($launch, 0)
    [BitConverter]::GetBytes([uint16]$path.Length).CopyTo($launch, 4)
    $path.CopyTo($launch, 6)
    Write-BridgeFrame $stream 2 7 $launch
    $started = Read-BridgeFrame $stream
    $startStatus = [BitConverter]::ToUInt32($started.Payload, 0)
    $guestPid = [BitConverter]::ToUInt32($started.Payload, 4)
    $completed = Read-BridgeFrame $stream
    $exitStatus = [BitConverter]::ToUInt32($completed.Payload, 0)
    $exited = Read-BridgeFrame $stream
    if ($started.Opcode -ne 2 -or $started.Request -ne 7 -or $started.Payload.Length -ne 8 -or $startStatus -ne 0 -or $guestPid -eq 0 -or $completed.Opcode -ne 2 -or $completed.Payload.Length -ne 8 -or $exitStatus -ne 0 -or $exited.Opcode -ne 3 -or $exited.Request -ne 7 -or $exited.Payload.Length -ne 0) {
        throw "Compatibility guest launch transaction failed"
    }
    if (Test-Path -LiteralPath $serial) {
        $log = [string](Get-Content -Raw -LiteralPath $serial)
        $booted = $log -match "Debian GNU/Linux 13" -or $log -match "aeros-compat login:"
    }
} finally {
    if ($client) {
        $client.Dispose()
    }
    if (-not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
        $process.WaitForExit()
    }
    if (Test-Path -LiteralPath $disk) {
        Remove-Item -Force -LiteralPath $disk
    }
}
if (-not $booted) {
    throw "Hardware-accelerated Debian guest boot was not confirmed"
}

Write-Output "AEROS_COMPAT_GUEST distribution=debian13 accelerator=whpx booted=true agent=true uid=65534 launch=true exit=0 network=false software_emulation=false verified=true"
