$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$esp = & (Join-Path $PSScriptRoot "build.ps1")
$qemu = Join-Path $env:ProgramFiles "qemu\qemu-system-x86_64.exe"
$firmware = Join-Path $env:ProgramFiles "qemu\share\edk2-x86_64-code.fd"
$variablesTemplate = Join-Path $env:ProgramFiles "qemu\share\edk2-i386-vars.fd"
$variables = Join-Path $root "build\edk2-vars-live.fd"

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
    -device virtio-vga `
    -display none `
    -serial "file:$root\build\live.log" `
    -monitor "tcp:127.0.0.1:45510,server,nowait" `
    -no-reboot
