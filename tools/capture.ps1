param(
    [string]$Command = "",
    [ValidateSet("desktop", "apps", "quick", "browser", "settings", "setup", "lock", "login")]
    [string]$View = "desktop"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$esp = & (Join-Path $PSScriptRoot "build.ps1")
$qemu = Join-Path $env:ProgramFiles "qemu\qemu-system-x86_64.exe"
$firmware = Join-Path $env:ProgramFiles "qemu\share\edk2-x86_64-code.fd"
$variablesTemplate = Join-Path $env:ProgramFiles "qemu\share\edk2-i386-vars.fd"
$variables = Join-Path $root "build\edk2-capture-vars.fd"
$serial = Join-Path $root "build\capture.log"
$ppm = Join-Path $root "build\aeros-boot.ppm"
$png = Join-Path $root "build\aeros-boot.png"
$monitorPort = 45456

Copy-Item -Force -LiteralPath $variablesTemplate -Destination $variables
foreach ($path in @($serial, $ppm, $png)) {
    if (Test-Path -LiteralPath $path) {
        Remove-Item -Force -LiteralPath $path
    }
}

$arguments = @(
    "-machine", "q35,accel=whpx:tcg",
    "-m", "512M",
    "-smp", "2",
    "-cpu", "qemu64,+nx,+smep,+smap,+xsave,+xsaveopt,+rdrand,+rdtscp,+pdpe1gb",
    "-drive", "if=pflash,format=raw,unit=0,readonly=on,file=$firmware",
    "-drive", "if=pflash,format=raw,unit=1,file=$variables",
    "-drive", "format=raw,file=fat:rw:$esp",
    "-device", "virtio-vga",
    "-display", "none",
    "-serial", "file:$serial",
    "-monitor", "tcp:127.0.0.1:$monitorPort,server,nowait",
    "-no-reboot"
)
$quoted = $arguments | ForEach-Object {
    if ($_ -match '\s') { '"' + $_ + '"' } else { $_ }
}
$start = [System.Diagnostics.ProcessStartInfo]::new()
$start.FileName = $qemu
$start.Arguments = $quoted -join " "
$start.WorkingDirectory = $root
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$process = [System.Diagnostics.Process]::Start($start)
$deadline = [DateTime]::UtcNow.AddSeconds(25)
$ready = $false

while ([DateTime]::UtcNow -lt $deadline) {
    if (Test-Path -LiteralPath $serial) {
        $serialText = [string](Get-Content -Raw -LiteralPath $serial)
        if ($serialText -match "AEROS_READY") {
            $ready = $true
            break
        }
    }
    if ($process.HasExited) {
        break
    }
    Start-Sleep -Milliseconds 100
}

if (-not $ready) {
    if (-not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
    }
    throw "AerOS was not ready for capture"
}

$runtimeDeadline = [DateTime]::UtcNow.AddSeconds(8)
$runtimeReady = $false
while ([DateTime]::UtcNow -lt $runtimeDeadline) {
    $serialText = [string](Get-Content -Raw -LiteralPath $serial)
    if ($serialText -match "AEROS_DESKTOP_RUNTIME active=true") {
        $runtimeReady = $true
        break
    }
    if ($process.HasExited) {
        break
    }
    Start-Sleep -Milliseconds 50
}
if (-not $runtimeReady) {
    if (-not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
    }
    throw "AerOS desktop runtime was not ready for capture"
}

$monitorHost = "127.0.0.1"

function Send-Monitor {
    param([string]$Line, [int]$SettleMs = 120)
    $c = [System.Net.Sockets.TcpClient]::new($monitorHost, $monitorPort)
    try {
        $s = $c.GetStream()
        $w = [System.IO.StreamWriter]::new($s)
        $w.NewLine = "`n"
        Start-Sleep -Milliseconds 60
        $w.WriteLine($Line)
        $w.Flush()
        Start-Sleep -Milliseconds 60
    }
    finally {
        $c.Close()
    }
    if ($SettleMs -gt 0) {
        Start-Sleep -Milliseconds $SettleMs
    }
}
if ($View -eq "setup") {
    Start-Sleep -Milliseconds 250
}
elseif ($View -in @("lock", "login", "desktop")) {
    Start-Sleep -Milliseconds 250
    $waypoints = switch ($View) {
        "login" { @("username", "password", "lock", "login") }
        "lock" { @("username", "password", "lock") }
        default { @("desktop") }
    }
    foreach ($waypoint in $waypoints) {
        $key = if ($waypoint -eq "desktop") { "esc" } else { "ret" }
        Send-Monitor "sendkey $key" 0
        $stepDeadline = [DateTime]::UtcNow.AddSeconds(12)
        $stepReady = $false
        while ([DateTime]::UtcNow -lt $stepDeadline) {
            $serialText = [string](Get-Content -Raw -LiteralPath $serial)
            if ($serialText -match "AEROS_DESKTOP_REDRAW .* screen=$waypoint") {
                $stepReady = $true
                break
            }
            Start-Sleep -Milliseconds 50
        }
        if (-not $stepReady) {
            Send-Monitor "quit" 0
            throw "AerOS session did not reach the $waypoint screen"
        }
    }
}
elseif ($View -ne "desktop") {
    Start-Sleep -Milliseconds 250
    $viewKey = switch ($View) {
        "apps" { "a" }
        "quick" { "q" }
        "browser" { "b" }
        "settings" { "s" }
    }
    Send-Monitor "sendkey $viewKey"
    $viewDeadline = [DateTime]::UtcNow.AddSeconds(15)
    $viewReady = $false
    while ([DateTime]::UtcNow -lt $viewDeadline) {
        $serialText = [string](Get-Content -Raw -LiteralPath $serial)
        $viewPattern = if ($View -in @("apps", "quick")) {
            "AEROS_DESKTOP_REDRAW overlay=$View window=false verified=true"
        } else {
            "AEROS_DESKTOP_REDRAW overlay=desktop window=true verified=true app=$View"
        }
        if ($serialText -match $viewPattern) {
            $viewReady = $true
            break
        }
        Start-Sleep -Milliseconds 50
    }
    if (-not $viewReady) {
        Send-Monitor "quit" 0
        throw "AerOS $View view did not finish rendering"
    }
    if ($View -eq "browser") {
        $browserDeadline = [DateTime]::UtcNow.AddSeconds(25)
        $browserReady = $false
        while ([DateTime]::UtcNow -lt $browserDeadline) {
            $serialText = [string](Get-Content -Raw -LiteralPath $serial)
            if ($serialText -match "AEROS_BROWSER host=\S+ path=\S+ dns=(true|false) tcp=(true|false) http_status=[0-9]+ bytes=[0-9]+ phase=\S+ verified=true") {
                $browserReady = $true
                break
            }
            Start-Sleep -Milliseconds 50
        }
        if (-not $browserReady) {
            Send-Monitor "quit" 0
            throw "AerOS browser network state did not finish"
        }
    }
}
if ($Command.Length -gt 0) {
    Start-Sleep -Milliseconds 250
    Send-Monitor "sendkey t" 500
    foreach ($character in $Command.ToCharArray()) {
        $key = switch ($character) {
            " " { "spc" }
            "-" { "minus" }
            "/" { "slash" }
            "." { "dot" }
            default { [string]$character }
        }
        Send-Monitor "sendkey $key" 90
    }
    Send-Monitor "sendkey ret" 120
    $commandDeadline = [DateTime]::UtcNow.AddSeconds(5)
    $executed = $false
    while ([DateTime]::UtcNow -lt $commandDeadline) {
        $serialText = [string](Get-Content -Raw -LiteralPath $serial)
        if ($serialText -match "AEROS_SHELL_EXEC sequence=1 status=0 .* verified=true") {
            $executed = $true
            break
        }
        Start-Sleep -Milliseconds 50
    }
    if (-not $executed) {
        Send-Monitor "quit" 0
        throw "AerOS did not execute the injected shell command"
    }
}
Start-Sleep -Milliseconds 250
Send-Monitor "screendump build/aeros-boot.ppm" 0
$captureDeadline = [DateTime]::UtcNow.AddSeconds(5)
while (-not (Test-Path -LiteralPath $ppm) -and [DateTime]::UtcNow -lt $captureDeadline) {
    Start-Sleep -Milliseconds 100
}
Send-Monitor "quit" 0
$process.WaitForExit(5000) | Out-Null
if (-not $process.HasExited) {
    Stop-Process -Id $process.Id -Force
}

if (-not (Test-Path -LiteralPath $ppm)) {
    throw "QEMU did not produce a framebuffer capture"
}

function Read-PpmToken {
    param([byte[]]$Bytes, [ref]$Position)
    while ($Position.Value -lt $Bytes.Length) {
        $value = $Bytes[$Position.Value]
        if ($value -eq 35) {
            while ($Position.Value -lt $Bytes.Length -and $Bytes[$Position.Value] -ne 10) {
                $Position.Value++
            }
        } elseif ($value -eq 9 -or $value -eq 10 -or $value -eq 13 -or $value -eq 32) {
            $Position.Value++
        } else {
            break
        }
    }
    $startPosition = $Position.Value
    while ($Position.Value -lt $Bytes.Length) {
        $value = $Bytes[$Position.Value]
        if ($value -eq 9 -or $value -eq 10 -or $value -eq 13 -or $value -eq 32) {
            break
        }
        $Position.Value++
    }
    [System.Text.Encoding]::ASCII.GetString($Bytes, $startPosition, $Position.Value - $startPosition)
}

$bytes = [System.IO.File]::ReadAllBytes($ppm)
$position = 0
$magic = Read-PpmToken $bytes ([ref]$position)
$captureWidth = [int](Read-PpmToken $bytes ([ref]$position))
$captureHeight = [int](Read-PpmToken $bytes ([ref]$position))
$maximum = [int](Read-PpmToken $bytes ([ref]$position))
if ($position -lt $bytes.Length -and $bytes[$position] -eq 13) {
    $position++
    if ($position -lt $bytes.Length -and $bytes[$position] -eq 10) {
        $position++
    }
} elseif ($position -lt $bytes.Length -and $bytes[$position] -in @(9, 10, 32)) {
    $position++
}
if ($magic -ne "P6" -or $maximum -ne 255) {
    throw "Unsupported framebuffer capture format"
}

Add-Type -AssemblyName System.Drawing
$bitmap = [System.Drawing.Bitmap]::new($captureWidth, $captureHeight)
for ($y = 0; $y -lt $captureHeight; $y++) {
    for ($x = 0; $x -lt $captureWidth; $x++) {
        $red = $bytes[$position]
        $green = $bytes[$position + 1]
        $blue = $bytes[$position + 2]
        $position += 3
        $bitmap.SetPixel($x, $y, [System.Drawing.Color]::FromArgb($red, $green, $blue))
    }
}
$bitmap.Save($png, [System.Drawing.Imaging.ImageFormat]::Png)
$bitmap.Dispose()
Write-Output $png
