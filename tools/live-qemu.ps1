$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$esp = & (Join-Path $PSScriptRoot "build.ps1")
$qemu = Join-Path $env:ProgramFiles "qemu\qemu-system-x86_64.exe"
$firmware = Join-Path $env:ProgramFiles "qemu\share\edk2-x86_64-code.fd"
$variablesTemplate = Join-Path $env:ProgramFiles "qemu\share\edk2-i386-vars.fd"
$variables = Join-Path $root "build\edk2-vars-live.fd"

$homeImage = Join-Path $root "build\home.img"
if (-not (Test-Path -LiteralPath $homeImage)) {
    $stream = [System.IO.File]::Create($homeImage)
    $stream.SetLength(64MB)
    $marker = [System.Text.Encoding]::ASCII.GetBytes("AEROS-DATA-BLANK")
    $stream.Write($marker, 0, $marker.Length)
    $stream.Close()
}
Get-Process qemu-system-x86_64 -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 400
Copy-Item -Force -LiteralPath $variablesTemplate -Destination $variables

& $qemu `
    -machine q35,accel=whpx:tcg `
    -m 512M `
    -smp 2 `
    -cpu "qemu64,+nx,+smep,+smap,+xsave,+xsaveopt,+rdrand,+rdtscp,+pdpe1gb" `
    -drive "if=pflash,format=raw,unit=0,readonly=on,file=$firmware" `
    -drive "if=pflash,format=raw,unit=1,file=$variables" `
    -drive "format=raw,file=fat:rw:$esp" `
    -drive "file=$homeImage,format=raw,if=ide,index=1" `
    -device virtio-vga `
    -audiodev "wav,id=snd0,path=$root\build\live-audio.wav" `
    -device AC97,audiodev=snd0 `
    -device qemu-xhci,id=xhci `
    -device usb-kbd,bus=xhci.0 `
    -device usb-mouse,bus=xhci.0 `
    -device usb-hub,bus=xhci.0,port=4 `
    -display none `
    -serial "file:$root\build\live.log" `
    -monitor "tcp:127.0.0.1:45510,server,nowait" `
    -qmp "tcp:127.0.0.1:45511,server,nowait" `
    -no-reboot
