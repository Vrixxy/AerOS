# Settings survive a restart: sets up a throwaway account on a copy of the
# boot drive, turns the volume down with the keyboard, waits for the desktop
# to save it, then starts the same drive again and checks that the desktop
# put the volume back.
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$esp = [string]@(& (Join-Path $PSScriptRoot "build.ps1"))[-1]
$esp = & (Join-Path $PSScriptRoot "esp-drive.ps1") -Esp $esp -Name esp-settings -Fresh
$qemu = Join-Path $env:ProgramFiles "qemu\qemu-system-x86_64.exe"
$firmware = Join-Path $env:ProgramFiles "qemu\share\edk2-x86_64-code.fd"
$variablesTemplate = Join-Path $env:ProgramFiles "qemu\share\edk2-i386-vars.fd"
$variables = Join-Path $root "build\edk2-settings-vars.fd"
$serial = Join-Path $root "build\settings-test.log"
$monitorPort = 45458
$settingsFile = Join-Path $esp "settings.dat"
# The copy starts without saved settings, whatever the build drive holds.
if (Test-Path -LiteralPath $settingsFile) { Remove-Item -Force -LiteralPath $settingsFile }

$script:monitor = $null
$script:process = $null

function Start-Machine {
    Copy-Item -Force -LiteralPath $variablesTemplate -Destination $variables
    if (Test-Path -LiteralPath $serial) { Remove-Item -Force -LiteralPath $serial }
    $arguments = @(
        "-machine", "q35,accel=whpx:tcg", "-m", "1024M", "-smp", "2",
        "-cpu", "qemu64,+nx,+smep,+smap,+xsave,+xsaveopt,+rdrand,+rdtscp,+pdpe1gb",
        "-drive", "if=pflash,format=raw,unit=0,readonly=on,file=$firmware",
        "-drive", "if=pflash,format=raw,unit=1,file=$variables",
        "-drive", "format=raw,file=fat:rw:$esp",
        "-device", "virtio-vga", "-display", "none",
        "-serial", "file:$serial",
        "-monitor", "tcp:127.0.0.1:$monitorPort,server,nowait",
        "-no-reboot"
    )
    $start = [System.Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $qemu
    $start.Arguments = ($arguments | ForEach-Object { if ($_ -match '\s') { '"' + $_ + '"' } else { $_ } }) -join " "
    $start.WorkingDirectory = $root
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $script:process = [System.Diagnostics.Process]::Start($start)
    $script:monitor = $null
}

function Stop-Machine {
    if ($null -ne $script:monitor) {
        try { Send-Monitor "quit" 0 } catch { }
        try { $script:monitor.Client.Close() } catch { }
        $script:monitor = $null
    }
    if (-not $script:process.WaitForExit(5000)) {
        Stop-Process -Id $script:process.Id -Force
        $script:process.WaitForExit(3000) | Out-Null
    }
}

function Send-Monitor {
    param([string]$Line, [int]$SettleMs = 120)
    if ($null -eq $script:monitor) {
        $client = [System.Net.Sockets.TcpClient]::new("127.0.0.1", $monitorPort)
        $writer = [System.IO.StreamWriter]::new($client.GetStream())
        $writer.NewLine = "`r`n"
        $script:monitor = @{ Client = $client; Writer = $writer }
        Start-Sleep -Milliseconds 300
    }
    $script:monitor.Writer.WriteLine($Line)
    $script:monitor.Writer.Flush()
    Start-Sleep -Milliseconds (60 + $SettleMs)
}

function Send-Text {
    param([string]$Text)
    foreach ($character in $Text.ToCharArray()) {
        $key = switch -CaseSensitive ([string]$character) {
            "-" { "minus" }
            { $_ -cmatch '^[A-Z]$' } { "shift-" + $_.ToLower() }
            default { $_ }
        }
        Send-Monitor "sendkey $key" 90
    }
}

function Wait-Serial {
    param([string]$Pattern, [int]$Seconds)
    $deadline = [DateTime]::UtcNow.AddSeconds($Seconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (Test-Path -LiteralPath $serial) {
            $text = [string](Get-Content -Raw -LiteralPath $serial)
            if ($text -match $Pattern) { return $Matches }
        }
        if ($script:process.HasExited) { break }
        Start-Sleep -Milliseconds 100
    }
    return $null
}

function Fail($message) {
    Stop-Machine
    throw $message
}

Write-Host "== first start: set up an account and turn the volume down ==" -ForegroundColor Cyan
Start-Machine
if (-not (Wait-Serial "AEROS_DESKTOP_RUNTIME active=true" 90)) { Fail "AerOS did not start" }
Start-Sleep -Seconds 3
$password = "Settings-2026"
$steps = @(
    @("", "keyboard"), @("", "username"), @("settings", "password"),
    @($password, "confirm"), @($password, "welcome"), @("", "lock"),
    @("", "login"), @($password, "desktop")
)
foreach ($step in $steps) {
    Send-Text $step[0]
    Send-Monitor "sendkey ret" 0
    if (-not (Wait-Serial "AEROS_DESKTOP_REDRAW .* screen=$($step[1])" 15)) {
        Fail "setup did not reach the $($step[1]) screen"
    }
}
# The first save is the session's starting values; then three steps down.
if (-not (Wait-Serial "AEROS_PREFS saved=true volume=42 " 15)) { Fail "the desktop did not save its settings" }
Start-Sleep -Milliseconds 500
foreach ($press in 1..3) { Send-Monitor "sendkey volumedown" 250 }
if (-not (Wait-Serial "AEROS_PREFS saved=true volume=24 " 15)) { Fail "the lowered volume was not saved" }
Start-Sleep -Seconds 1
Stop-Machine
if (-not (Test-Path -LiteralPath $settingsFile)) { throw "no settings file on the drive" }
$length = (Get-Item -LiteralPath $settingsFile).Length
if ($length -ne 16) { throw "settings file is $length bytes, expected 16" }

Write-Host "== second start: the volume is put back ==" -ForegroundColor Cyan
Start-Machine
$restored = Wait-Serial "AEROS_PREFS restored=true volume=(\d+) muted=(\w+) brightness=(\d+)" 90
Stop-Machine
if (-not $restored) { throw "the second start did not restore the settings" }
if ($restored[1] -ne "24" -or $restored[2] -ne "false" -or $restored[3] -ne "100") {
    throw "restored volume=$($restored[1]) muted=$($restored[2]) brightness=$($restored[3]), expected 24 false 100"
}
Write-Host ""
Write-Host "AerOS settings test passed: volume 24 saved, and restored at the next start." -ForegroundColor Green
