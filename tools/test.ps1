param(
    [ValidateRange(1, 8)]
    [int]$CpuCount = 2,
    [int]$MemoryMiB = 512
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$esp = & (Join-Path $PSScriptRoot "build.ps1") -BootTest
$qemu = Join-Path $env:ProgramFiles "qemu\qemu-system-x86_64.exe"
$firmware = Join-Path $env:ProgramFiles "qemu\share\edk2-x86_64-code.fd"
$variablesTemplate = Join-Path $env:ProgramFiles "qemu\share\edk2-i386-vars.fd"
$variables = Join-Path $root "build\edk2-test-vars.fd"
$serial = Join-Path $root "build\boot-test.log"

Copy-Item -Force -LiteralPath $variablesTemplate -Destination $variables
if (Test-Path -LiteralPath $serial) {
    Remove-Item -Force -LiteralPath $serial
}
# The AEROS_FAT_WRITE, AEROS_VFS_PERSIST and AEROS_VFS_PERSIST_DIR self-tests
# each persist a file or directory to the ESP; remove any leftover copies
# from a previous run so the VFS node/byte-count assertions below stay
# deterministic regardless of prior test history (these only become visible
# to a *later* boot's /data enumeration, since they're written after that
# boot's own VFS mount). The audit log (kernel/src/audit.rs) persists to
# /data too and otherwise grows across every run, same reason.
foreach ($name in @("aerosfs.txt", "persist.txt", "audit.log", "CRASHT.BIN")) {
    $artifact = Join-Path $root "build\esp\$name"
    if (Test-Path -LiteralPath $artifact) {
        Remove-Item -Force -LiteralPath $artifact
    }
}
$testDirArtifact = Join-Path $root "build\esp\testdir"
if (Test-Path -LiteralPath $testDirArtifact) {
    Remove-Item -Force -Recurse -LiteralPath $testDirArtifact
}
# AerOS Shield's quarantine now lives on /data (so it survives a restart)
# instead of /tmp; a leftover from an earlier live/manual run would likewise
# throw off the VFS node count.
$quarantineArtifact = Join-Path $root "build\esp\quar"
if (Test-Path -LiteralPath $quarantineArtifact) {
    Remove-Item -Force -Recurse -LiteralPath $quarantineArtifact
}

# TLS: a throwaway certificate authority per run (no private keys live in
# the repository). The two roots go onto the virtio test disk for the guest to
# read (not the boot volume: files there show up in /tmp and change unrelated
# counts); two `openssl s_server` processes on the host answer on 18443 (RSA chain,
# ChaCha20-Poly1305) and 18444 (ECDSA chain, AES-128-GCM).
$tlsDir = Join-Path $root "build\tls"
Get-CimInstance Win32_Process -Filter "Name='openssl.exe'" |
    Where-Object { $_.CommandLine -match "s_server.* -accept 1844[34]\b" } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
& python (Join-Path $PSScriptRoot "make-tls-test-pki.py") $tlsDir | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "Could not make the TLS test certificates (python and openssl must be on PATH)"
}
$tlsServers = @()
foreach ($server in @(
    @{ Prefix = "rsa"; Port = 18443; Suite = "TLS_CHACHA20_POLY1305_SHA256" },
    @{ Prefix = "ec"; Port = 18444; Suite = "TLS_AES_128_GCM_SHA256" })) {
    $tlsServers += Start-Process -FilePath "openssl" -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput (Join-Path $tlsDir "$($server.Prefix).out.log") `
        -RedirectStandardError (Join-Path $tlsDir "$($server.Prefix).err.log") `
        -ArgumentList @("s_server", "-accept", "$($server.Port)", "-tls1_3", "-www",
            "-cert", (Join-Path $tlsDir "$($server.Prefix).leaf.pem"),
            "-cert_chain", (Join-Path $tlsDir "$($server.Prefix).chain.pem"),
            "-key", (Join-Path $tlsDir "$($server.Prefix).key.pem"),
            "-ciphersuites", $server.Suite, "-groups", "X25519")
}

# Whatever makes this script stop, the helper servers must not outlive it: they
# would keep the caller's output pipe open and the run would never return.
trap {
    foreach ($leftover in $tlsServers) {
        if (-not $leftover.HasExited) { Stop-Process -Id $leftover.Id -Force }
    }
    throw $_
}

# A 4 MiB scratch NVMe namespace: sector 0 carries the signature the driver
# checks, sector 2 is overwritten by its write/read-back probe.
$nvmeImage = Join-Path $root "build\nvme-test.img"
$nvmeBytes = New-Object byte[] (4MB)
$signature = [System.Text.Encoding]::ASCII.GetBytes("AEROS-NVME-TEST")
[Array]::Copy($signature, $nvmeBytes, $signature.Length)
[System.IO.File]::WriteAllBytes($nvmeImage, $nvmeBytes)
# A 4 MiB USB mass-storage disk behind a hub: signature in sector 0, an
# 8-sector write/read-back probe at sector 8.
$usbImage = Join-Path $root "build\usb-test.img"
$usbBytes = New-Object byte[] (4MB)
$usbSignature = [System.Text.Encoding]::ASCII.GetBytes("AEROS-USB-TEST")
[Array]::Copy($usbSignature, $usbBytes, $usbSignature.Length)
[System.IO.File]::WriteAllBytes($usbImage, $usbBytes)
# A 4 MiB SD card image: signature in sector 0, write/read-back at sector 8.
$sdImage = Join-Path $root "build\sd-test.img"
$sdBytes = New-Object byte[] (4MB)
$sdSignature = [System.Text.Encoding]::ASCII.GetBytes("AEROS-SD-TEST!")
[Array]::Copy($sdSignature, $sdBytes, $sdSignature.Length)
[System.IO.File]::WriteAllBytes($sdImage, $sdBytes)
# A blank 16 MiB disk with the launcher's marker: the kernel formats it as
# the FAT home volume (/home) on first sight.
$homeImage = Join-Path $root "build\home-test.img"
$homeBytes = New-Object byte[] (16MB)
$homeMarker = [System.Text.Encoding]::ASCII.GetBytes("AEROS-DATA-BLANK")
[Array]::Copy($homeMarker, $homeBytes, $homeMarker.Length)
[System.IO.File]::WriteAllBytes($homeImage, $homeBytes)
$audioCapture = Join-Path $root "build\audio-test.wav"
Remove-Item -Force -ErrorAction SilentlyContinue -LiteralPath $audioCapture
$virtioImage = Join-Path $root "build\virtio-blk-test.img"
$virtioBytes = New-Object byte[] (8MB)
$virtioSignature = [System.Text.Encoding]::ASCII.GetBytes("AEROS-VIRTIO-BLK")
[Array]::Copy($virtioSignature, $virtioBytes, $virtioSignature.Length)
# A swap area from sector 4096: the first page carries the swap signature.
$swapSignature = [System.Text.Encoding]::ASCII.GetBytes("SWAPSPACE2")
[Array]::Copy($swapSignature, 0, $virtioBytes, 4096 * 512 + 4086, $swapSignature.Length)
# The TLS test roots: a 32-bit length then the DER, four sectors each.
foreach ($rootDisk in @(@{ File = "TLSRSA.DER"; Sector = 100 }, @{ File = "TLSEC.DER"; Sector = 108 })) {
    $der = [System.IO.File]::ReadAllBytes((Join-Path $tlsDir $rootDisk.File))
    $derLength = [BitConverter]::GetBytes([uint32]$der.Length)
    [Array]::Copy($derLength, 0, $virtioBytes, $rootDisk.Sector * 512, 4)
    [Array]::Copy($der, 0, $virtioBytes, $rootDisk.Sector * 512 + 4, $der.Length)
}
[System.IO.File]::WriteAllBytes($virtioImage, $virtioBytes)
$hdaCapture = Join-Path $root "build\hda-test.wav"
Remove-Item -Force -ErrorAction SilentlyContinue -LiteralPath $hdaCapture
$arguments = @(
    "-machine", "q35,accel=whpx:tcg",
    "-m", "${MemoryMiB}M",
    "-smp", "$CpuCount",
    "-cpu", "qemu64,+nx,+smep,+smap,+xsave,+xsaveopt,+rdrand,+rdtscp,+pdpe1gb",
    "-drive", "if=pflash,format=raw,unit=0,readonly=on,file=$firmware",
    "-drive", "if=pflash,format=raw,unit=1,file=$variables",
    "-drive", "format=raw,file=fat:rw:$esp",
    "-drive", "file=$homeImage,format=raw,if=ide,index=1",
    "-device", "amd-iommu,dma-remap=on",
    "-device", "edu",
    "-device", "virtio-vga",
    "-drive", "file=$nvmeImage,if=none,id=nvm0,format=raw",
    "-device", "nvme,drive=nvm0,serial=aeros0",
    "-audiodev", "wav,id=snd0,path=$audioCapture",
    "-device", "AC97,audiodev=snd0",
    "-audiodev", "wav,id=snd1,path=$hdaCapture",
    "-device", "intel-hda",
    "-device", "hda-duplex,audiodev=snd1",
    "-device", "qemu-xhci,id=xhci",
    "-device", "usb-kbd,bus=xhci.0",
    "-device", "usb-mouse,bus=xhci.0",
    "-device", "usb-hub,bus=xhci.0,port=3",
    "-drive", "file=$usbImage,if=none,id=usbd0,format=raw",
    "-device", "usb-storage,bus=xhci.0,port=3.1,drive=usbd0",
    "-device", "usb-tablet,bus=xhci.0,port=3.2",
    "-drive", "file=$sdImage,if=none,id=sd0,format=raw",
    "-device", "sdhci-pci",
    "-device", "sd-card,drive=sd0",
    "-nic", "user,model=e1000e,hostfwd=tcp::17654-:17654,hostfwd=tcp::17655-:17655",
    "-netdev", "user,id=rtlnet,net=10.10.0.0/24",
    "-device", "rtl8139,netdev=rtlnet",
    "-netdev", "user,id=vnet0,net=10.9.0.0/24",
    "-device", "virtio-net-pci,netdev=vnet0,disable-modern=on,disable-legacy=off",
    "-drive", "file=$virtioImage,if=none,id=vblk0,format=raw",
    "-device", "virtio-blk-pci,drive=vblk0,disable-modern=on,disable-legacy=off",
    # Modern-transport-only (no legacy fallback exists for this device type),
    # unlike virtio-net/virtio-blk above - exercises virtio_modern.rs/
    # virtio_input.rs, the real touchscreen driver.
    "-device", "virtio-multitouch-pci,disable-modern=off,disable-legacy=on",
    "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
    "-display", "none",
    "-serial", "file:$serial",
    "-monitor", "tcp:127.0.0.1:17656,server,nowait",
    "-no-reboot",
    "-no-shutdown"
)
$quoted = $arguments | ForEach-Object {
    if ($_ -match '\s') { '"' + $_ + '"' } else { $_ }
}
$start = [System.Diagnostics.ProcessStartInfo]::new()
$start.FileName = $qemu
$start.Arguments = $quoted -join " "
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$process = [System.Diagnostics.Process]::Start($start)

# Real end-to-end proof of the TCP server self-test (kernel/src/net.rs):
# connect in from the host over the hostfwd rule above, send bytes, and
# check the kernel echoed them back - the guest's own serial log is checked
# separately below. Bounded per-attempt so "not listening yet" fails fast
# instead of hanging on the OS's own connect timeout.
function Test-AerosTcpEcho {
    param([int]$Port, [int]$TimeoutMs)
    $client = [System.Net.Sockets.TcpClient]::new()
    try {
        $result = $client.BeginConnect("127.0.0.1", $Port, $null, $null)
        if (-not $result.AsyncWaitHandle.WaitOne($TimeoutMs)) {
            return $null
        }
        $client.EndConnect($result)
        $stream = $client.GetStream()
        $stream.ReadTimeout = 3000
        $message = [System.Text.Encoding]::ASCII.GetBytes("AEROS-TCP-SERVER-TEST")
        $stream.Write($message, 0, $message.Length)
        $buffer = New-Object byte[] 64
        $read = $stream.Read($buffer, 0, $buffer.Length)
        if ($read -eq 0) {
            # The connection closed (or raced) before any echo arrived - not
            # a definitive failure, just try again like a failed connect.
            return $null
        }
        return [System.Text.Encoding]::ASCII.GetString($buffer, 0, $read)
    } catch {
        return $null
    } finally {
        $client.Close()
    }
}
# Pushes a patterned buffer through the guest's socket-API echo server and
# checks every byte comes back in order: multi-segment send, window updates,
# and an orderly close, all over the real card.
function Test-AerosTcpBulk {
    param([int]$Port, [int]$Bytes, [int]$TimeoutMs)
    $client = [System.Net.Sockets.TcpClient]::new()
    try {
        $result = $client.BeginConnect("127.0.0.1", $Port, $null, $null)
        if (-not $result.AsyncWaitHandle.WaitOne($TimeoutMs)) {
            return $null
        }
        $client.EndConnect($result)
        $stream = $client.GetStream()
        $stream.ReadTimeout = 15000
        $data = New-Object byte[] $Bytes
        for ($index = 0; $index -lt $Bytes; $index++) {
            $data[$index] = (($index * 31) + 7) -band 255
        }
        $sender = $stream.WriteAsync($data, 0, $Bytes)
        $received = New-Object byte[] $Bytes
        $total = 0
        while ($total -lt $Bytes) {
            $read = $stream.Read($received, $total, $Bytes - $total)
            if ($read -eq 0) { break }
            $total += $read
        }
        $sender.Wait(15000) | Out-Null
        $client.Client.Shutdown([System.Net.Sockets.SocketShutdown]::Send)
        $extra = $stream.Read((New-Object byte[] 16), 0, 16)
        $same = ($total -eq $Bytes)
        if ($same) {
            for ($index = 0; $index -lt $Bytes; $index++) {
                if ($received[$index] -ne $data[$index]) { $same = $false; break }
            }
        }
        return ($same -and $extra -eq 0)
    } catch {
        return $null
    } finally {
        $client.Close()
    }
}
# A one-shot HTTP server on the host's loopback for the guest's HTTP client to
# download from through the card (the guest reaches the host as 10.0.2.2).
$httpScript = {
    param($address, $port)
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Parse($address), $port)
    $listener.Start()
    try {
        $deadline = [DateTime]::UtcNow.AddSeconds(420)
        while (-not $listener.Pending() -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 50 }
        if (-not $listener.Pending()) { return "no connection" }
        $client = $listener.AcceptTcpClient()
        $stream = $client.GetStream()
        $request = New-Object System.Text.StringBuilder
        $buffer = New-Object byte[] 512
        while (-not $request.ToString().Contains("`r`n`r`n")) {
            $read = $stream.Read($buffer, 0, $buffer.Length)
            if ($read -eq 0) { break }
            [void]$request.Append([System.Text.Encoding]::ASCII.GetString($buffer, 0, $read))
        }
        $body = New-Object byte[] 6000
        for ($index = 0; $index -lt 6000; $index++) { $body[$index] = 97 + ($index % 26) }
        $head = [System.Text.Encoding]::ASCII.GetBytes("HTTP/1.1 200 OK`r`nContent-Type: text/plain`r`nContent-Length: 6000`r`nConnection: close`r`n`r`n")
        $stream.Write($head, 0, $head.Length)
        $stream.Write($body, 0, $body.Length)
        $stream.Flush()
        $client.Close()
        return ($request.ToString() -split "`r`n")[0]
    } finally {
        $listener.Stop()
    }
}
$httpServer = Start-Job -ArgumentList "127.0.0.1", 18080 -ScriptBlock $httpScript
$httpServer6 = Start-Job -ArgumentList "::1", 18081 -ScriptBlock $httpScript
$tcpServerEcho = $null
# The guest's listener is single-shot and latches onto the first SYN it sees.
# Connections opened while it was still booting leave slirp retrying a SYN for
# a host socket that is already gone, so only start once the guest says it is
# listening.
$listenDeadline = [DateTime]::UtcNow.AddSeconds(180)
while ([DateTime]::UtcNow -lt $listenDeadline -and -not $process.HasExited) {
    if ((Test-Path -LiteralPath $serial) -and
        ([string](Get-Content -Raw -LiteralPath $serial)) -match "AEROS_TCP_LISTENING") { break }
    Start-Sleep -Milliseconds 100
}
$tcpServerDeadline = [DateTime]::UtcNow.AddSeconds(10)
while ([DateTime]::UtcNow -lt $tcpServerDeadline -and $null -eq $tcpServerEcho) {
    $tcpServerEcho = Test-AerosTcpEcho -Port 17654 -TimeoutMs 1000
    if ($null -eq $tcpServerEcho) { Start-Sleep -Milliseconds 300 }
}

$netListenDeadline = [DateTime]::UtcNow.AddSeconds(60)
while ([DateTime]::UtcNow -lt $netListenDeadline -and -not $process.HasExited) {
    if ((Test-Path -LiteralPath $serial) -and
        ([string](Get-Content -Raw -LiteralPath $serial)) -match "AEROS_TCP_NET_LISTENING") { break }
    Start-Sleep -Milliseconds 100
}
$tcpBulk = $null
$bulkDeadline = [DateTime]::UtcNow.AddSeconds(20)
while ([DateTime]::UtcNow -lt $bulkDeadline -and $null -eq $tcpBulk -and -not $process.HasExited) {
    $tcpBulk = Test-AerosTcpBulk -Port 17655 -Bytes 30000 -TimeoutMs 1000
    if ($null -eq $tcpBulk) { Start-Sleep -Milliseconds 300 }
}

function Send-AerosPowerButton {
    $client = [System.Net.Sockets.TcpClient]::new()
    try {
        $client.Connect("127.0.0.1", 17656)
        $stream = $client.GetStream()
        $command = [System.Text.Encoding]::ASCII.GetBytes("system_powerdown`r`n")
        $stream.Write($command, 0, $command.Length)
        $stream.Flush()
        Start-Sleep -Milliseconds 300
        return $true
    } catch {
        return $false
    } finally {
        $client.Close()
    }
}

$deadline = [DateTime]::UtcNow.AddSeconds(240)
$output = ""
$powerButtonSent = $false

while ([DateTime]::UtcNow -lt $deadline) {
    if (Test-Path -LiteralPath $serial) {
        $output = [string](Get-Content -Raw -LiteralPath $serial)
        if (-not $powerButtonSent -and $output -match "AEROS_POWERBTN_WAIT") {
            $powerButtonSent = Send-AerosPowerButton
        }
        if ($output -match "AEROS_READY") {
            break
        }
    }
    if ($process.HasExited) {
        break
    }
    Start-Sleep -Milliseconds 100
}

if (-not $process.HasExited) {
    Stop-Process -Id $process.Id -Force
    $process.WaitForExit()
}
if ($output -notmatch "AEROS_READY") {
    throw "AerOS did not reach the ready state (QEMU exited: $($process.HasExited), exit code: $(if ($process.HasExited) { $process.ExitCode } else { 'still running' }))`n$output"
}
if ($output -notmatch "allocator_test=true") {
    throw "AerOS allocator self-test failed`n$output"
}
if ($output -notmatch "plus_jakarta=true roboto_mono=true") {
    throw "AerOS font validation failed`n$output"
}
if ($output -notmatch "acpi_valid=true") {
    throw "AerOS ACPI validation failed`n$output"
}
if ($output -notmatch "AEROS_ACPI .* madt_valid=true .* processors=[1-9].* ioapics=[1-9]") {
    throw "AerOS APIC topology validation failed`n$output"
}
if ($output -notmatch "AEROS_ACPI_EVENTS sci=[1-9][0-9]* acpi_mode=true routed=true power_button=true gpe_handlers=[0-9]+ gpes_enabled=[0-9]+") {
    throw "AerOS ACPI event setup failed`n$output"
}
if ($output -notmatch "AEROS_POWERBTN pressed=true interrupts=[1-9][0-9]* line_shut_off=false") {
    throw "AerOS did not see the power button press (SCI delivery)`n$output"
}
if ($output -notmatch "AEROS_ARCH gdt=true idt=true") {
    throw "AerOS descriptor-table validation failed`n$output"
}
if ($output -notmatch "AEROS_TIMER source=pit frequency_hz=100 ticks=[4-9]") {
    throw "AerOS interrupt timer validation failed`n$output"
}
if ($output -notmatch "AEROS_APIC enabled=true .* id=[0-9]+ version=0x[1-9a-f][0-9a-f]* max_lvt=[4-9][0-9]* counts_per_100hz=[1-9][0-9]* test_ticks=[4-9] verified=true") {
    throw "AerOS Local APIC validation failed`n$output"
}
if ($output -notmatch "AEROS_IOAPIC present=true address=0x[1-9a-f][0-9a-f]* id=[0-9]+ version=0x[1-9a-f][0-9a-f]* redirections=[1-9][0-9]* gsi_base=0 timer_gsi=2 vector=50 active_low=false level=false ticks=[4-9] pic_masked=true verified=true") {
    throw "AerOS I/O APIC routing failed`n$output"
}
$applicationCount = $CpuCount - 1
$expectedWork = 100000 * $applicationCount
$smpPattern = "AEROS_SMP discovered=$CpuCount applications=$applicationCount online=$CpuCount bsp_id=0 last_ap_id=$applicationCount trampoline=0x[1-9a-f][0-9a-f]* stage=7 gdt_stage=6 work=$expectedWork idle=$applicationCount ipi_acks=$applicationCount queue_dispatches=$applicationCount queue_completions=$applicationCount work_queue=true isolated_stacks=true verified=true"
if ($output -notmatch $smpPattern) {
    throw "AerOS multiprocessor startup failed`n$output"
}
if ($output -notmatch "AEROS_HPET present=true base=0x[1-9a-f][0-9a-f]* period_fs=[1-9][0-9]* counter64=true timers=[1-9][0-9]* .* elapsed_ns=[1-9][0-9]* verified=true") {
    throw "AerOS HPET validation failed`n$output"
}
if ($output -notmatch "AEROS_RTC year=20[2-9][0-9] month=([1-9]|1[0-2]) day=([1-9]|[12][0-9]|3[01]) hour=([0-9]|1[0-9]|2[0-3]) minute=([0-9]|[1-5][0-9]) second=([0-9]|[1-5][0-9]) unix_seconds=[1-9][0-9]{9,} verified=true") {
    throw "AerOS RTC wall-clock validation failed`n$output"
}
if ($output -notmatch "AEROS_ENTROPY .* hardware_words=([8-9]|[1-9][0-9]+) sample_a_nonzero=true sample_b_nonzero=true distinct=true chacha20=true aslr=true process_aslr=true verified=true") {
    throw "AerOS entropy and ASLR validation failed`n$output"
}
if ($output -notmatch "AEROS_COMPAT guest=debian13-amd64 vmx=(true|false) svm=(true|false) npt=(true|false) nested=(true|false) hardware_acceleration=(true|false) software_emulation=false static=native dynamic=guest script=guest malformed=reject readonly_base=true per_app_overlay=true host_files_default_deny=true devices_default_deny=true bridge_protocol=true verified=true") {
    throw "AerOS Linux compatibility routing validation failed`n$output"
}
if ($output -notmatch "AEROS_SVM svm_supported=true npt_supported=true svm_enabled=true locked_off=false guest_ram_bytes=33554432 .* halted=true console_len=10 console_ok=true high_marker=0xcafef00d guest_marker=0xae052d05 verified=true") {
    throw "AerOS hardware-virtualization guest run failed`n$output"
}
if ($output -notmatch "AEROS_VM64 guest_ram_bytes=33554432 .* halted=true console_len=10 console_ok=true high_marker=0xcafef00d guest_marker=0xae052d05 verified=true") {
    throw "AerOS long-mode guest run failed`n$output"
}
if ($output -notmatch "AEROS_VM_LINUX source=embedded-probe bytes=2048 pm_offset=1024 loaded=true .* halted=true console_len=6 console_ok=true boot_flag=0xaa55 marker=0xae052d05 verified=true") {
    throw "AerOS Linux boot-protocol handoff failed`n$output"
}
# VT-x: QEMU here cannot offer VMX, so on this machine the line is a skip. On an
# Intel host that does offer it the smoke guests must all pass.
if ($output -notmatch "AEROS_VMX supported=false verified=skipped" -and
    $output -notmatch "AEROS_VMX supported=true locked_off=false enabled=true ept=true .* real_mode_ok=true long_mode_ok=true linux_probe_ok=true exits=[0-9]+ instruction_error=0 last_exit=0x[0-9a-f]+ qualification=0x[0-9a-f]+ verified=true") {
    throw "AerOS VT-x backend failed`n$output"
}
if ($output -notmatch "AEROS_PAGING .* nx=true wp=true verified=true") {
    throw "AerOS virtual-memory validation failed`n$output"
}
if ($output -notmatch "AEROS_DEMAND_PAGING ready=true pool_pages=4096 reserved=32 committed=2 faults=3 verified=true") {
    throw "AerOS demand paging validation failed`n$output"
}
if ($output -notmatch "AEROS_SYSCALL_ENTRY supported=true sce=true target=true flags_masked=true cpu_mask=0x$([Convert]::ToString((1 -shl $CpuCount) - 1, 16)) per_cpu=true swapgs=true verified=true") {
    throw "AerOS SYSCALL entry validation failed`n$output"
}
if ($output -notmatch "AEROS_FPU fxsave=true sse=true xsave=true avx=true xcr0=0x7 state_bytes=[5-9][0-9]{2,} simd=true per_cpu=true verified=true") {
    throw "AerOS floating-point and SIMD validation failed`n$output"
}
if ($output -notmatch "AEROS_HEAP .* active=0 verified=true") {
    throw "AerOS heap validation failed`n$output"
}
if ($output -notmatch "AEROS_USER .* exit=42 smep=true smap=true mapped=true verified=true") {
    throw "AerOS ring-3 transition failed`n$output"
}
if ($output -notmatch "AEROS_VFS nodes=[0-9]+ directories=11 files=[0-9]+ bytes=[0-9]+ handles=0 mutable_files=0 mutable_bytes=0 readonly_root=true verified=true") {
    throw "AerOS VFS validation failed`n$output"
}
if ($output -notmatch "AEROS_ELF type=pie machine=x86_64 segments=3 .* wx=false verified=true") {
    throw "AerOS ELF validation failed`n$output"
}
if ($output -notmatch "AEROS_PROCESS path=/bin/init .* pages=3 executable=1 writable=1 stack_pages=2 exit=73 verified=true") {
    throw "AerOS init process failed`n$output"
}
if ($output -notmatch "AEROS_PROCESS_STACK abi=linux pointer=0x[1-9a-f][0-9a-f]* argc=1 aux_entries=13 bytes=[1-9][0-9]* aligned=true random=true phdr=0x[1-9a-f][0-9a-f]* verified=true") {
    throw "AerOS Linux process stack validation failed`n$output"
}
if ($output -notmatch "AEROS_COMPILED_USERSPACE path=/bin/aeros-init format=rust-static-pie segments=3 file_bytes=2208 pages=3 executable=1 writable=1 stack_aligned=true exit=74 verified=true") {
    throw "AerOS compiled userspace validation failed`n$output"
}
if ($output -notmatch "AEROS_STD_USERSPACE path=/bin/aeros-std-smoke runtime=rust-std-musl segments=4 file_bytes=380992 pages=97 executable=74 writable=5 stack_aligned=true exit=75 verified=true") {
    throw "AerOS standard Rust userspace validation failed`n$output"
}
if ($output -notmatch "AEROS_ISOLATION path=/bin/fault-probe vector=6 error=0x0 address=0x0 faults=1 exit=132 kernel_survived=true verified=true") {
    throw "AerOS user fault isolation failed`n$output"
}
if ($output -notmatch "AEROS_REAP mappings=5 released_pages=[1-9][0-9]* free_before=([1-9][0-9]*) free_after=\1 address_spaces_removed=true scrubbed=true verified=true") {
    throw "AerOS process address-space reaping failed`n$output"
}
if ($output -notmatch "AEROS_PROCESSES spawned=4 reaped=4 highest_pid=4 ready=0 running=0 zombies=0 generations=true init_exit=73 compiled_exit=74 std_exit=75 fault_exit=132 verified=true") {
    throw "AerOS process table validation failed`n$output"
}
if ($output -notmatch "AEROS_TMPFS path=/tmp nodes=[0-9]+ directories=11 files=[0-9]+ mutable_files=1 mutable_bytes=6 handles=0 verified=true") {
    throw "AerOS writable tmpfs validation failed`n$output"
}
if ($output -notmatch "AEROS_SYSCALL .* calls=161 bootstrap=2 linux=159 exits=4 unknown=0 opens=13 reads=12 writes=10 closes=17 io_bytes=264 clocks=1 random_calls=1 random_bytes=32 compat_calls=73 memory_calls=15 mmaps=5 file_mmaps=1 mprotects=2 munmaps=4 metadata=9 seeks=2 paths=12 resources=1 rseq=1 futex=1 fd_calls=10 dup_calls=4 last_fd=3 signals=12 runtime=8 directories=1 sockets=4 datagrams=2 network_bytes=[3-9][0-9]+ vectored=2 positional=1 access=1 statx=1 wall_clock=2 sleeps=2 chdir=1 relative_paths=5 polls=1 creates=2 renames=2 removes=5 syncs=2 truncates=1 chmods=1 verified=true") {
    throw "AerOS syscall gate failed`n$output"
}
if ($output -notmatch "AEROS_PCI devices=[1-9].* verified=true") {
    throw "AerOS PCI enumeration failed`n$output"
}
if ($output -notmatch "AEROS_NVME present=true .* identify=true io_queue=true read=true write_probe=true sectors=[1-9][0-9]* sector_bytes=[5-9][0-9][0-9] .* verified=true") {
    throw "AerOS NVMe driver validation failed`n$output"
}
if ($output -notmatch "AEROS_AC97 present=true .* codec_ready=true buffers_played=[3-9]|AEROS_AC97 present=true .* codec_ready=true buffers_played=[1-9][0-9]+ verified=true") {
    throw "AerOS AC97 audio driver validation failed`n$output"
}
if ($output -notmatch "AEROS_XHCI present=true .* devices=5 keyboards=1 mice=1 tablets=1 hubs=1 disks=1 disk_sectors=8192 disk_read=true disk_write=true verified=true") {
    throw "AerOS xHCI USB driver validation failed`n$output"
}
if ($output -notmatch "AEROS_VIRTIO_BLK present=true .* read=true write_probe=true verified=true") {
    throw "AerOS virtio-blk driver validation failed`n$output"
}
if ($output -notmatch "AEROS_VIRTIO_NET present=true .* arp_reply=true verified=true") {
    throw "AerOS virtio-net driver validation failed`n$output"
}
if ($output -notmatch "AEROS_VIRTIO_INPUT present=true queue_ready=true abs_x_span=[1-9][0-9]* abs_y_span=[1-9][0-9]* verified=true") {
    throw "AerOS virtio-input (touchscreen) driver validation failed`n$output"
}
if ($output -notmatch "AEROS_VIRTIO_GPU present=true queue_ready=true display_width=[1-9][0-9]* display_height=[1-9][0-9]* resource_created=true backing_attached=true transfer_ok=true flush_ok=true verified=true") {
    throw "AerOS virtio-gpu driver validation failed`n$output"
}
if ($output -notmatch "AEROS_RTL8139 present=true .* link=true tx=true arp_reply=true gateway=10\.10\.0\.2 verified=true") {
    throw "AerOS RTL8139 driver validation failed`n$output"
}
if ($output -notmatch "AEROS_SDHCI present=true .* card=true sdhc=(true|false) sectors=8192 read=true write_probe=true verified=true") {
    throw "AerOS SD host controller validation failed`n$output"
}
if ($output -notmatch "AEROS_FATFS formatted=true fat32=false directories=true long_names=true big_file=true rename_move=true truncate=true delete_frees=true remount=true verified=true") {
    throw "AerOS FAT filesystem validation failed`n$output"
}
if ($output -notmatch "AEROS_MEDIA_VFS mounted=true listed=true data=true write=true verified=true") {
    throw "AerOS removable media (/media) validation failed`n$output"
}
if ($output -notmatch "AEROS_SYSMON cpus=$CpuCount memory_mib=[0-9]+ used_mib=[0-9]+ heap_kib=[0-9]+ processes=[0-9]+ verified=true") {
    throw "AerOS system monitor validation failed`n$output"
}
if ($output -notmatch "AEROS_TIMEZONE zones=[0-9]+ verified=true") {
    throw "AerOS time zone validation failed`n$output"
}

# Picture decoders: the kernel decodes the bundled wallpaper files; the host's
# own decoders (System.Drawing) produce the reference values.
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @"
using System;
using System.Drawing;
using System.Drawing.Imaging;
public static class ImageReference {
    public static uint Crc32(byte[] data) {
        uint[] table = new uint[256];
        for (uint n = 0; n < 256; n++) {
            uint c = n;
            for (int k = 0; k < 8; k++) c = (c & 1) != 0 ? 0xEDB88320u ^ (c >> 1) : c >> 1;
            table[n] = c;
        }
        uint crc = 0xFFFFFFFFu;
        foreach (byte b in data) crc = table[(crc ^ b) & 0xFF] ^ (crc >> 8);
        return ~crc;
    }
    static byte[] Rgba(string path, out int width, out int height) {
        using (Bitmap bmp = new Bitmap(path)) {
            width = bmp.Width; height = bmp.Height;
            BitmapData data = bmp.LockBits(new Rectangle(0, 0, width, height), ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
            byte[] raw = new byte[Math.Abs(data.Stride) * height];
            System.Runtime.InteropServices.Marshal.Copy(data.Scan0, raw, 0, raw.Length);
            bmp.UnlockBits(data);
            byte[] rgba = new byte[width * height * 4];
            for (int y = 0; y < height; y++) {
                for (int x = 0; x < width; x++) {
                    int s = y * data.Stride + x * 4, d = (y * width + x) * 4;
                    rgba[d] = raw[s + 2]; rgba[d + 1] = raw[s + 1]; rgba[d + 2] = raw[s]; rgba[d + 3] = raw[s + 3];
                }
            }
            return rgba;
        }
    }
    public static string PngCrc(string path) {
        int w, h; byte[] rgba = Rgba(path, out w, out h);
        return Crc32(rgba).ToString("x8");
    }
    public static string JpegGrid(string path, int columns, int rows) {
        int w, h; byte[] rgba = Rgba(path, out w, out h);
        System.Text.StringBuilder text = new System.Text.StringBuilder();
        for (int row = 0; row < rows; row++) {
            for (int column = 0; column < columns; column++) {
                int x0 = column * w / columns, x1 = (column + 1) * w / columns;
                int y0 = row * h / rows, y1 = (row + 1) * h / rows;
                long sum = 0, count = 0;
                for (int y = y0; y < y1; y += 7) {
                    for (int x = x0; x < x1; x += 7) {
                        int a = (y * w + x) * 4;
                        sum += (rgba[a] * 299L + rgba[a + 1] * 587L + rgba[a + 2] * 114L) / 1000;
                        count++;
                    }
                }
                text.Append(((byte)(sum / Math.Max(1, count))).ToString("x2"));
            }
        }
        return text.ToString();
    }
}
"@
$wallpapers = Join-Path $root "assets\wallpapers"
$pngReference = [ImageReference]::PngCrc((Join-Path $wallpapers "aeros-mountains.png"))
if ($output -notmatch "AEROS_IMAGE_PNG width=774 height=468 crc=0x$pngReference roundtrip=true encoded_bytes=[0-9]+ verified=true") {
    throw "AerOS PNG decoder disagrees with the host decoder (expected crc $pngReference)`n$output"
}
if ($output -notmatch "AEROS_IMAGE_JPEG width=4148 height=2228 decode_ms=[0-9]+ grid=([0-9a-f]{1152}) verified=true") {
    throw "AerOS JPEG decoder did not produce a picture`n$output"
}
$kernelGrid = $Matches[1]
$hostGrid = [ImageReference]::JpegGrid((Join-Path $wallpapers "aeros-mountains.jpeg"), 32, 18)
$worst = 0; $total = 0
for ($i = 0; $i -lt 576; $i++) {
    $difference = [Math]::Abs([Convert]::ToInt32($kernelGrid.Substring($i * 2, 2), 16) - [Convert]::ToInt32($hostGrid.Substring($i * 2, 2), 16))
    $worst = [Math]::Max($worst, $difference); $total += $difference
}
Write-Host ("JPEG vs host decoder: worst cell difference {0}, mean {1:N2}" -f $worst, ($total / 576))
if ($worst -gt 4 -or ($total / 576) -gt 1.5) {
    throw "AerOS JPEG decoder differs from the host decoder (worst $worst, mean $($total / 576))"
}
if ($output -notmatch "AEROS_IMAGE_PNG_CORPUS cases=[0-9]+ passed=[0-9]+ verified=true") {
    throw "AerOS PNG corpus (colour types/depths/interlacing) failed`n$output"
}
if ($output -notmatch "AEROS_IMAGE_PROGRESSIVE identical=true") {
    throw "AerOS progressive JPEG decode differs from the baseline decode of the same data`n$output"
}
if ($output -notmatch "AEROS_LOGO opaque=[0-9]+ clear=[0-9]+ verified=true") {
    throw "AerOS logo mask failed`n$output"
}
if ($output -notmatch "AEROS_SCREENSHOT saved=true file_bytes=[0-9]+ width=1280 height=800 verified=true") {
    throw "AerOS screenshot save/reload failed`n$output"
}
if (([regex]::Matches($output, "AEROS_TRUETYPE font=(ui|mono) min_overlap_permille=[0-9]+ last_shift=-?[0-9]+,-?[0-9]+ non_ascii=true scripts=true verified=true")).Count -ne 2) {
    throw "AerOS TrueType rasterizer validation failed`n$output"
}
if ($output -notmatch "AEROS_TEXT_UNICODE layout=true") {
    throw "AerOS Unicode text layout failed`n$output"
}
if ($output -notmatch "AEROS_KEYMAP layouts=12 mismatches=0 dead_keys=true caps=true ascii=true verified=true") {
    throw "AerOS keyboard layouts failed`n$output"
}
if ($output -notmatch "AEROS_IMAGE_BMP verified=true") {
    throw "AerOS BMP decoder validation failed`n$output"
}
if ($output -notmatch "AEROS_NTP result=(ok|unavailable) drift_seconds=-?[0-9]+") {
    throw "AerOS NTP client validation failed`n$output"
}
if ($output -notmatch "AEROS_HOME present=true formatted=true fat32=false clusters=[0-9]+ free_clusters=[0-9]+ verified=true") {
    throw "AerOS home volume mount failed`n$output"
}
if ($output -notmatch "AEROS_HOME_VFS tree=true file_io=true listing=true rename_paths=true cross_mount=true read_only=true remount=true verified=true") {
    throw "AerOS home volume VFS validation failed`n$output"
}
if ($output -notmatch "AEROS_HDA present=true .* path_found=true bytes_played=[1-9][0-9]* verified=true") {
    throw "AerOS Intel HDA audio driver validation failed`n$output"
}
if ($output -notmatch "AEROS_AHCI present=true .* implemented=[1-9].* active=[1-9].* sata=[1-9].* disks=[1-9].* identify=true read=true write_probe=true sectors=[1-9][0-9]* sector_bytes=[5-9][0-9][0-9] .* verified=true") {
    throw "AerOS AHCI DMA validation failed`n$output"
}
if ($output -notmatch "AEROS_PARTITIONS mbr=true .* count=[1-9].* fat=[1-9].* first_lba=[1-9][0-9]* .* verified=true") {
    throw "AerOS partition discovery failed`n$output"
}
if ($output -notmatch "AEROS_FAT mounted=true bits=(16|32) .* efi=true boot=true bootx64=true bootx64_bytes=[1-9][0-9]* pe=true verified=true") {
    throw "AerOS FAT filesystem validation failed`n$output"
}
if ($output -notmatch "AEROS_FAT_WRITE supported=(true|false) created=(true|false) bytes=[1-9][0-9]* roundtrip=(true|false) verified=true") {
    throw "AerOS persistent FAT write validation failed`n$output"
}
if ($output -notmatch "AEROS_VFS_PERSIST created=true written=true readback=true on_disk=true verified=true") {
    throw "AerOS VFS persistent /data mount validation failed`n$output"
}
if ($output -notmatch "AEROS_VFS_PERSIST_DIR rediscovered=(true|false) present=true on_disk=true verified=true") {
    throw "AerOS VFS persistent /data directory validation failed`n$output"
}
if ($output -notmatch "AEROS_PWRITE created=true verified=true") {
    throw "AerOS pwrite64 validation failed`n$output"
}
if ($output -notmatch "AEROS_PIPE verified=true") {
    throw "AerOS pipe validation failed`n$output"
}
if ($output -notmatch "AEROS_TCP_ENGINE clean=true lossy=true burst_loss=true slow_reader=true refused=true unreachable=true hostile=true sack_receiver=true sack_recovery=true retransmits=[1-9][0-9]* fast_retransmits=[1-9][0-9]* peak_cwnd=[0-9]+ verified=true") {
    throw "AerOS TCP engine validation failed`n$output"
}
if ($output -notmatch "AEROS_SYSCALL_FUZZ exit=0 reaped=1 elapsed_ms=[0-9]+ verified=true") {
    throw "AerOS in-process syscall fuzzing failed`n$output"
}
if ($output -notmatch "AEROS_PKG verified=true") {
    throw "AerOS package manager validation failed`n$output"
}
if ($output -notmatch "AEROS_UDP table=true sockets=true verified=true") {
    throw "AerOS UDP port table / sockets validation failed`n$output"
}
if ($output -notmatch "AEROS_PROC_MAPS verified=true") {
    throw "AerOS address-space region listing failed`n$output"
}
if ($output -notmatch "AEROS_SHELL_FUZZ lines=2500 elapsed_ms=[0-9]+ verified=true") {
    throw "AerOS shell command fuzzing failed`n$output"
}
if ($output -notmatch "AEROS_SMP_SCHED cpus=$CpuCount placement=true stealing=true priority_order=true affinity=true refused=true balanced=true work_stolen=true steals=[0-9]+ verified=true") {
    throw "AerOS SMP job scheduler self-test failed`n$output"
}
if ($output -notmatch "AEROS_SLAB verified=true") {
    throw "AerOS slab allocator self-test failed`n$output"
}
if ($output -notmatch "AEROS_BLOCK .* verified=true") {
    throw "AerOS block layer self-test failed`n$output"
}
if ($output -notmatch "AEROS_INSTALLER verified=true") {
    throw "AerOS installer self-test failed`n$output"
}
if ($output -notmatch "AEROS_PROCFS verified=true") {
    throw "AerOS /proc validation failed`n$output"
}
if ($output -notmatch "AEROS_TCP_SOCKET verified=true") {
    throw "AerOS TCP socket API validation failed`n$output"
}
if ($output -notmatch "AEROS_SYSCALL_MISC verified=true") {
    throw "AerOS flock/close_range/fallocate/preadv validation failed`n$output"
}
if ($output -notmatch "AEROS_SYSINFO verified=true") {
    throw "AerOS sysinfo validation failed`n$output"
}
if ($output -notmatch "AEROS_NETWORK driver=e1000e present=true .* mac=([0-9a-f]{2}:){5}[0-9a-f]{2} link=true speed_mbps=(100|1000) duplex=true tx=true rx=true rx_bytes=[1-9][0-9]* arp_gateway=true verified=true") {
    throw "AerOS e1000e network validation failed`n$output"
}
if ($output -notmatch "AEROS_TCP_SERVER syn_received=true handshake_completed=true echoed_bytes=[1-9][0-9]* closed_cleanly=(true|false) verified=true") {
    throw "AerOS TCP server self-test failed`n$output"
}
if ($tcpServerEcho -ne "AEROS-TCP-SERVER-TEST") {
    throw "AerOS TCP server did not echo real bytes back to a host-side client (got: '$tcpServerEcho')`n$output"
}
if ($output -notmatch "AEROS_HTTP_CLIENT connected=true status=200 bytes=[0-9]+ body_ok=true verified=true") {
    throw "AerOS HTTP client over the TCP engine failed`n$output"
}
foreach ($tlsServer in $tlsServers) {
    if (-not $tlsServer.HasExited) { Stop-Process -Id $tlsServer.Id -Force }
}
# The kernel copied itself to a random 2 MiB-aligned place and relocated there:
# the whole rest of this run is the proof that it still works from there.
if ($output -notmatch "AEROS_KASLR relocated=true origin=0x[0-9a-f]+ base=0x[0-9a-f]+ aligned=true slots=[1-9][0-9]* relocations=[1-9][0-9]{3,} entropy=rdrand reason=none pointers_fixed=true verified=true") {
    throw "AerOS kernel image randomisation failed`n$output"
}
# The clock must be the calibrated counter, not the HPET: the HPET is read over
# memory-mapped I/O and costs tens of microseconds per read under a hypervisor.
if ($output -notmatch "AEROS_CLOCK source=tsc hz=[0-9]{8,}") {
    throw "AerOS is not using the calibrated counter as its clock`n$output"
}
if ($output -notmatch "AEROS_TLS_KAT hashes=true key_schedule=true x25519=true aead=true signatures=4 rejects_bad=true verified=true") {
    throw "AerOS TLS known-answer self-test failed`n$output"
}
if ($output -notmatch "AEROS_TLS_NET roots=true rsa_chacha=true ec_aes=true wrong_host_refused=true untrusted_refused=true verified=true") {
    throw "AerOS TLS 1.3 handshake against OpenSSL failed`n$output"
}
$httpRequestLine = if (Wait-Job $httpServer -Timeout 5) { Receive-Job $httpServer } else { "" }
Remove-Job $httpServer -Force
$httpRequestLine6 = if (Wait-Job $httpServer6 -Timeout 5) { Receive-Job $httpServer6 } else { "" }
Remove-Job $httpServer6 -Force
if ($httpRequestLine -ne "GET /big HTTP/1.1") {
    throw "AerOS HTTP client sent an unexpected request line to the host server ('$httpRequestLine')`n$output"
}
if ($output -notmatch "AEROS_IPV6_OFFLINE addresses_and_frames=true sockets=true verified=true") {
    throw "AerOS IPv6 offline self-test failed`n$output"
}
if ($output -notmatch "AEROS_SOCKOPT verified=true") {
    throw "AerOS socket options self-test failed`n$output"
}
if ($output -notmatch "AEROS_DNS6 found=true address=\[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1\]") {
    throw "AerOS AAAA lookup through the host resolver failed`n$output"
}
if ($output -notmatch "AEROS_HTTP6 connected=true status=200 bytes=[0-9]+ body_ok=true verified=true") {
    throw "AerOS HTTP client over IPv6 failed`n$output"
}
if ($httpRequestLine6 -ne "GET /big6 HTTP/1.1") {
    throw "AerOS IPv6 HTTP client sent an unexpected request line to the host server ('$httpRequestLine6')`n$output"
}
if ($output -notmatch "AEROS_TCP_NET accepted=true echoed_bytes=30000 closed=true verified=true") {
    throw "AerOS socket-layer TCP echo over the network failed`n$output"
}
if ($tcpBulk -ne $true) {
    throw "AerOS socket-layer TCP server did not echo 30000 bytes back intact to a host-side client (got: '$tcpBulk')`n$output"
}
if ($output -notmatch "AEROS_FIREWALL empty_passes=true port_match_blocks=true port_mismatch_passes=true protocol_mismatch_passes=true any_port_blocks_all=true cleared_passes_again=true dropped_counted=true verified=true") {
    throw "AerOS firewall self-test failed`n$output"
}
if ($output -notmatch "AEROS_DHCP discover=true request=true ack=true address=10\.0\.2\.15 gateway=10\.0\.2\.2 dns=10\.0\.2\.3 lease_seconds=[1-9][0-9]* verified=true") {
    throw "AerOS DHCP lease validation failed`n$output"
}
if ($output -notmatch "AEROS_IPV4 local=10.0.2.15 gateway=10.0.2.2 tx=true rx=true reply_bytes=[1-9][0-9]* ip_checksum=true icmp_checksum=true echo_reply=true verified=true") {
    throw "AerOS IPv4 and ICMP validation failed`n$output"
}
if ($output -notmatch "AEROS_UDP_DNS server=10.0.2.3 tx=true rx=true udp_checksum=true response=true answers=[1-9][0-9]* address=([0-9]{1,3}\.){3}[0-9]{1,3} verified=true") {
    throw "AerOS UDP and DNS validation failed`n$output"
}
# Router discovery, neighbour discovery, a link-local echo, SLAAC from the
# advertised prefix and an echo to the router's global address all have to
# work. (This used to be lenient because no IPv6 frame ever arrived; the
# e1000 receive filter was dropping multicast, which router advertisements
# use.)
if ($output -notmatch "AEROS_IPV6 .* router_advertised=true .* neighbor_resolved=true echo_tx=true echo_rx=true prefix_found=true global=\[fe, c0.* global_echo_rx=true verified=true") {
    throw "AerOS IPv6 (SLAAC and global echo) validation failed`n$output"
}
if ($output -notmatch "AEROS_SCHEDULER tasks=1 ready=0 running=1 exited=0 switches=([8-9]|[1-9][0-9]+) stack_bytes=0 fpu_tasks=1 fpu_bytes=[5-9][0-9][0-9]+ fpu_switches=([8-9]|[1-9][0-9]+) fpu_isolation=true highest_id=0 verified=true") {
    throw "AerOS scheduler validation failed`n$output"
}
if ($output -notmatch "AEROS_PREEMPT source=lapic ticks=([8-9]|[1-9][0-9]+) work_a=[1-9][0-9]* work_b=[1-9][0-9]* fpu_isolation=true verified=true") {
    throw "AerOS preemption validation failed`n$output"
}
if ($output -notmatch "AEROS_CONCURRENT_USER exit_a=77 exit_b=88 switches=[1-9][0-9]* reaped=2 verified=true") {
    throw "AerOS concurrent user-task scheduling validation failed`n$output"
}
if ($output -notmatch "AEROS_TASK_EXHAUSTION spawned=[1-9][0-9]* exhausted_cleanly=true reaped=[1-9][0-9]* recovered=true verified=true") {
    throw "AerOS task-table exhaustion validation failed`n$output"
}
if ($output -notmatch "AEROS_FORK parent_exit=11 child_exit=22 parent_stack=0xaaaaaaaa child_stack=0xbbbbbbbb reaped=2 verified=true") {
    throw "AerOS fork validation failed`n$output"
}
if ($output -notmatch "AEROS_STACK_GROWTH grown_exit=33 grown_pages=37 overflow_exit=[1-9][0-9]* verified=true") {
    throw "AerOS growable-stack validation failed`n$output"
}
if ($output -notmatch "AEROS_EPOLL verified=true") {
    throw "AerOS epoll validation failed`n$output"
}
if ($output -notmatch "AEROS_UNIX_SOCKETPAIR verified=true") {
    throw "AerOS unix socketpair validation failed`n$output"
}
if ($output -notmatch "AEROS_UNIX_DOMAIN_SOCKET verified=true") {
    throw "AerOS unix domain socket (bind/listen/connect/accept) validation failed`n$output"
}
if ($output -notmatch "AEROS_RLIMIT_NOFILE verified=true") {
    throw "AerOS RLIMIT_NOFILE enforcement validation failed`n$output"
}
if ($output -notmatch "AEROS_CAPABILITY exit=22 reaped=1 pure=true verified=true") {
    throw "AerOS capability sets validation failed`n$output"
}
if ($output -notmatch "AEROS_SECCOMP_FILTER exit=21 reaped=1 bpf=true verified=true") {
    throw "AerOS seccomp filter (BPF) validation failed`n$output"
}
if ($output -notmatch "AEROS_ED25519 vectors=3 elapsed_ms=[0-9]+ verified=true") {
    throw "AerOS Ed25519 / SHA-512 validation failed`n$output"
}
if ($output -notmatch "AEROS_STATFS verified=true") {
    throw "AerOS statfs helper validation failed`n$output"
}
if ($output -notmatch "AEROS_SELECT verified=true") {
    throw "AerOS select/pselect6 core validation failed`n$output"
}
if ($output -notmatch "AEROS_SOCKET_API verified=true") {
    throw "AerOS socket API helper validation failed`n$output"
}
if ($output -notmatch "AEROS_FAT_CRASH cases=[1-9][0-9]* elapsed_ms=[0-9]+ dangling=0 cross_linked=0 short=0 torn=0 lost=0 verified=true") {
    throw "AerOS FAT power-loss consistency test failed`n$output"
}
if ($output -notmatch "AEROS_AERFS_CRASH cases=[0-9]+ mount_failures=0 fsck_failures=0 torn=0 verified=true") {
    throw "AerFS crash-consistency test failed`n$output"
}
if ($output -notmatch "AEROS_AERFS_VFS written=true read_back=true listing=true renamed=true truncated=true fsck_clean=true persists=true verified=true") {
    throw "AerFS through the VFS failed`n$output"
}
if ($output -notmatch "AEROS_FATFS_CRASH cases=[1-9][0-9]* structural=0 torn=0 leaks_repaired=[0-9]+ repair_failures=0 verified=true") {
    throw "AerOS /home filesystem power-loss consistency test failed`n$output"
}
if ($output -notmatch "AEROS_MOUNTS bound=[0-9]+ same_file=true listing=true rename=true protected=true refused=true unbind=true persists=true verified=true") {
    throw "AerOS mount table self-test failed`n$output"
}
if ($output -notmatch "AEROS_HOME_FSCK files=[0-9]+ directories=[0-9]+ orphans=0 damaged=false") {
    throw "AerOS home volume mount-time fsck did not report a clean volume`n$output"
}
if ($output -notmatch "AEROS_FUZZ iterations=[1-9][0-9]* elapsed_ms=[0-9]+ verified=true") {
    throw "AerOS parser fuzzing did not complete`n$output"
}
if ($output -notmatch "AEROS_OOM pressure=normal verified=true") {
    throw "AerOS memory pressure / OOM policy validation failed`n$output"
}
if ($output -notmatch "AEROS_SERVICES verified=true") {
    throw "AerOS service table validation failed`n$output"
}
if ($output -notmatch "AEROS_KERNEL_LOG verified=true") {
    throw "AerOS kernel log ring validation failed`n$output"
}
if ($output -notmatch "AEROS_SYMLINK_SYSCALL verified=true") {
    throw "AerOS symlink syscall plumbing validation failed`n$output"
}
if ($output -notmatch "AEROS_PROCESS_GROUP verified=true") {
    throw "AerOS process group (setpgid/getpgid/setsid/getsid) validation failed`n$output"
}
if ($output -notmatch "AEROS_FORK_CHAIN exit_a=1 exit_b=2 exit_c=3 reaped=3 verified=true") {
    throw "AerOS fork-chain validation failed`n$output"
}
if ($output -notmatch "AEROS_EXECVE parent_exit=11 child_exit=33 parent_stack=0xaaaaaaaa child_stack=0xcccccccc reaped=2 verified=true") {
    throw "AerOS execve validation failed`n$output"
}
if ($output -notmatch "AEROS_WAIT4 parent_exit=44 reaped=1 verified=true") {
    throw "AerOS wait4 validation failed`n$output"
}
if ($output -notmatch "AEROS_KILL parent_exit=137 reaped=1 verified=true") {
    throw "AerOS kill validation failed`n$output"
}
if ($output -notmatch "AEROS_GETPID parent_pid=1 child_getppid=1 reaped=2 verified=true") {
    throw "AerOS getpid/getppid validation failed`n$output"
}
if ($output -notmatch "AEROS_HEAP_SBRK parent_exit=11 child_exit=22 parent_heap=0xaaaaaaaa child_heap=0xbbbbbbbb reaped=2 verified=true") {
    throw "AerOS heap/sbrk validation failed`n$output"
}
if ($output -notmatch "AEROS_SYS_WRITE_OK") {
    throw "AerOS user-mode write() did not reach the console`n$output"
}
if ($output -notmatch "AEROS_WRITE_SYSCALL exit=1 verified=true") {
    throw "AerOS write() syscall validation failed`n$output"
}
if ($output -notmatch "AEROS_GETRANDOM_SYSCALL exit=1 distinct=true verified=true") {
    throw "AerOS getrandom() syscall validation failed`n$output"
}
if ($output -notmatch "AEROS_LINUX_FD_ISOLATION exit_a=0xfffffff7 exit_b=1 verified=true") {
    throw "AerOS Linux-ABI per-task fd isolation validation failed`n$output"
}
if ($output -notmatch "AEROS_LINUX_FORK_FD parent_exit=1 child_exit=0xfffffff7 reaped=2 verified=true") {
    throw "AerOS Linux-ABI fork() fd duplication validation failed`n$output"
}
if ($output -notmatch "AEROS_LINUX_BRK_FORK parent_exit=11 child_exit=22 parent_value=0xaaaaaaaa child_value=0xbbbbbbbb reaped=2 verified=true") {
    throw "AerOS Linux-ABI brk() fork isolation validation failed`n$output"
}
if ($output -notmatch "AEROS_LINUX_MMAP_FORK parent_exit=51 child_exit=139 parent_marker_second=0xaaaa0002 parent_marker_reused=0xaaaa0003 child_marker=0xbbbb0001 reaped=2 verified=true") {
    throw "AerOS Linux-ABI mmap/mprotect/munmap fork isolation validation failed`n$output"
}
if ($output -notmatch "AEROS_LINUX_FD_REFCOUNT parent_exit=200 reaped=1 verified=true") {
    throw "AerOS fork() fd refcount validation failed`n$output"
}
if ($output -notmatch "AEROS_LINUX_FILE_MMAP exit=55 bytes=\[65, 66, 67, 68\] reaped=1 verified=true") {
    throw "AerOS file-backed mmap validation failed`n$output"
}
if ($output -notmatch "AEROS_SCHEDULED_REAL_ELF exit=74 reaped=1 verified=true") {
    throw "AerOS scheduled real ELF validation failed`n$output"
}
if ($output -notmatch "AEROS_SCHEDULED_STD_ELF exit=75 reaped=1 verified=true") {
    throw "AerOS scheduled std ELF validation failed`n$output"
}
if ($output -notmatch "AEROS_EXEC_PATH exit=74 reaped=1 verified=true") {
    throw "AerOS path-based exec validation failed`n$output"
}
if ($output -notmatch "AEROS_THREADS exit=76 reaped=[0-9]+ verified=true") {
    throw "AerOS threads test failed`n$output"
}
if ($output -notmatch "AEROS_MEASURE entries=2 image_measured=true status=no-reference register=[0-9a-f]{64}") {
    throw "AerOS boot measurement failed`n$output"
}
if ($output -notmatch "AEROS_MEASURE_TEST entries=2 image_measured=true register_chain=true file_hash=true seal_matches=true tamper_detected=true no_reference=true verified=true") {
    throw "AerOS measured-boot self-test failed`n$output"
}
if ($output -notmatch "AEROS_UPDATE applied=true previous_kept=true bad_signature_rejected=true tampered_rejected=true untrusted_rejected=true rollback=true crash_cases=[0-9]+ torn=0 verified=true") {
    throw "AerOS signed update test failed`n$output"
}
if ($output -notmatch "AEROS_STACK_PROTECTOR cookie_random=true instrumented=true intact_passes=true smash_detected=true verified=true") {
    throw "AerOS stack protector check failed`n$output"
}
if ($output -notmatch "AEROS_AML loaded=true tables=1 nodes=[0-9]+ devices=[0-9]+ methods=[0-9]+ load_errors=0 apic_mode=true s5_interpreted=true") {
    throw "AerOS AML namespace load failed`n$output"
}
if ($output -notmatch "AEROS_AML_QUERIES s5=Some\(\([0-9]+, [0-9]+\)\) matches_scan=true pci_root=true crs_ok=true prt_entries=[0-9]+ prt_links=true com1_ok=true link_crs_ok=true sta_ok=[0-9]+ sta_errors=0 verified=true") {
    throw "AerOS AML queries failed`n$output"
}
if ($output -notmatch "AEROS_IOMMU present=true base=0x[0-9a-f]+ devices=[0-9]+ unity=[0-9]+ enabled=true mapped_pages=[0-9]+ edu=true mapped=true blocked_write=true blocked_read=true revoked=true faults=[0-9]+ other_events=[0-9]+ last_fault=0x[0-9a-f]+/0x[0-9a-f]+ verified=true") {
    throw "AerOS IOMMU test failed`n$output"
}
if ($output -notmatch "AEROS_BENCH_RESULT exit=0 reaped=[0-9]+ verified=true") {
    throw "AerOS benchmark program failed`n$output"
}
foreach ($name in @("cpu_xorshift", "getpid", "clock_gettime", "memcpy_64k", "mmap_touch_munmap_8_pages", "tmpfs_create_write_read_unlink_4k", "fork_exit_wait", "pipe_round_trip")) {
    if ($output -notmatch "AEROS_BENCH $name ops=[0-9]+ total_ns=[0-9]+ ns_per_op=[0-9]+") {
        throw "AerOS benchmark $name did not report`n$output"
    }
}
if ($output -notmatch "AEROS_SWAP slots=[0-9]+ exit=88 reaped=[0-9]+ evicted=[0-9]+ written=[0-9]+ read=[0-9]+ in_use=0 failures=0 verified=true") {
    throw "AerOS swap test failed`n$output"
}
if ($output -notmatch "AEROS_PTRACE exit=77 reaped=[0-9]+ verified=true") {
    throw "AerOS ptrace test failed`n$output"
}
if ($output -notmatch "AEROS_BIG_MMAP exit=55 reaped=1 verified=true") {
    throw "AerOS large mmap validation failed`n$output"
}
if ($output -notmatch "AEROS_LINUX_FORK_WAIT exit=11 reaped=1 verified=true") {
    throw "AEROS_LINUX_FORK_WAIT validation failed`n$output"
}
if ($output -notmatch "AEROS_LINUX_EXECVE exit=74 reaped=1 verified=true") {
    throw "AEROS_LINUX_EXECVE validation failed`n$output"
}
if ($output -notmatch "AEROS_LINUX_KILL exit=11 reaped=1 verified=true") {
    throw "AEROS_LINUX_KILL validation failed`n$output"
}
if ($output -notmatch "AEROS_SECCOMP_STRICT exit=137 reaped=1 verified=true") {
    throw "AerOS seccomp strict-mode validation failed`n$output"
}
if ($output -notmatch "AEROS_LINUX_EXECVE_ARGS exit=121 argc=2 arg1=true reaped=1 verified=true") {
    throw "AEROS_LINUX_EXECVE_ARGS validation failed`n$output"
}
if ($output -notmatch "AEROS_COW faults=[0-9]+ copies=[0-9]+ verified=true") {
    throw "AEROS_COW validation failed`n$output"
}
if ($output -notmatch "AEROS_COW_FORK exit=11 faults_delta=[1-9][0-9]* copies_delta=[1-9][0-9]* verified=true") {
    throw "AEROS_COW_FORK validation failed`n$output"
}
if ($output -notmatch "AEROS_COW_KERNEL exit=11 faults_delta=[1-9][0-9]* verified=true") {
    throw "AEROS_COW_KERNEL validation failed`n$output"
}
if ($output -notmatch "AEROS_LINUX_SIGNAL exit=11 reaped=1 verified=true") {
    throw "AEROS_LINUX_SIGNAL validation failed`n$output"
}
if ($output -notmatch "AEROS_LINUX_SIGNAL_MASK exit=11 reaped=1 verified=true") {
    throw "AEROS_LINUX_SIGNAL_MASK validation failed`n$output"
}
if ($output -notmatch "AEROS_LINUX_SIGNAL_CHILD exit=11 reaped=1 verified=true") {
    throw "AEROS_LINUX_SIGNAL_CHILD validation failed`n$output"
}
if ($output -notmatch "AEROS_LINUX_SIGNAL_ASYNC exit=11 reaped=1 async=[1-9][0-9]* verified=true") {
    throw "AEROS_LINUX_SIGNAL_ASYNC validation failed`n$output"
}
if ($output -notmatch "AEROS_LINUX_ITIMER exit=11 reaped=1 async=[1-9][0-9]* verified=true") {
    throw "AEROS_LINUX_ITIMER validation failed`n$output"
}
if ($output -notmatch "AEROS_LINUX_NANOSLEEP exit=11 reaped=1 verified=true") {
    throw "AEROS_LINUX_NANOSLEEP validation failed`n$output"
}
if ($output -notmatch "AEROS_LINUX_PIPE_FORK exit=11 reaped=1 verified=true") {
    throw "AEROS_LINUX_PIPE_FORK validation failed`n$output"
}
if ($output -notmatch "AEROS_LINUX_WNOHANG exit=11 reaped=1 verified=true") {
    throw "AEROS_LINUX_WNOHANG validation failed`n$output"
}
if ($output -notmatch "AEROS_SCHEDULED_REAL_ELF_FORK parent_exit=11 marker=0xaaaa0001 reaped=1 verified=true") {
    throw "AerOS scheduled real ELF fork validation failed`n$output"
}
if ($output -notmatch "AEROS_AV signatures=[1-9][0-9]* self_test=true quarantine_flow=true quarantine_encrypted=true realtime=true home_realtime=true shred=true scanned_files=[0-9]+ threats=0 unreadable=[0-9]+ verified=true") {
    throw "AerOS Shield antivirus validation failed`n$output"
}
if ($output -notmatch "AEROS_AUDIT verified=true") {
    throw "AerOS audit log validation failed`n$output"
}
if ($output -notmatch "AEROS_COMMANDS count=87 shell=aersh elevation=ear unique=true parser=true privilege=true filesystem=true reauth=true redirection=true startup=true symlinks=true background_jobs=true firewall_command=true dmesg_command=true service_command=true text_tools=true priority_command=true crashes_command=true bench_command=true sigcheck_command=true pipelines=true strace_command=true fsck_command=true verified=true") {
    throw "AerOS command registry validation failed`n$output"
}
if ($output -notmatch "AEROS_UI_CORE geometry=true scaling=true interaction=true frost=true max_frost_pixels=1048576 verified=true") {
    throw "AerOS native UI core validation failed`n$output"
}
if ($output -notmatch "AEROS_UI_RENDER pixels=1536 captured=true changed=true corner_clipped=true verified=true") {
    throw "AerOS native UI renderer validation failed`n$output"
}
if ($output -notmatch "AEROS_DESKTOP wallpaper=true dock=true app_switcher=true quick_settings=true window=true button=true input=true verified=true") {
    throw "AerOS desktop compositor validation failed`n$output"
}
if ($output -notmatch "AEROS_WINDOW_ANIMATION mirror=true monotonic=true centered=true verified=true") {
    throw "AerOS window open/close animation validation failed`n$output"
}
if ($output -notmatch "AEROS_TEXT_WRAP verified=true") {
    throw "AerOS browser text-processing validation failed`n$output"
}
if ($output -notmatch "AEROS_KEYBOARD controller=true routed=true vector=52 queued=0 dropped=0 verified=true") {
    throw "AerOS PS/2 keyboard validation failed`n$output"
}

# The AC97 self-test plays a tune; QEMU's wav backend must have recorded
# real (non-silent) samples of it.
if (-not (Test-Path -LiteralPath $audioCapture)) {
    throw "AerOS AC97 produced no audio capture"
}
$stream = [System.IO.File]::Open($audioCapture, "Open", "Read", "ReadWrite")
try {
    $audioBytes = New-Object byte[] $stream.Length
    [void]$stream.Read($audioBytes, 0, $audioBytes.Length)
} finally {
    $stream.Dispose()
}
$loud = 0
for ($index = 44; $index -lt $audioBytes.Length; $index++) {
    if ($audioBytes[$index] -ne 0) { $loud++ }
}
if ($loud -lt 2000) {
    throw "AerOS AC97 audio capture is silent ($loud non-zero bytes of $($audioBytes.Length))"
}
$hdaStream = [System.IO.File]::Open($hdaCapture, "Open", "Read", "ReadWrite")
try {
    $hdaBytes = New-Object byte[] $hdaStream.Length
    [void]$hdaStream.Read($hdaBytes, 0, $hdaBytes.Length)
} finally {
    $hdaStream.Dispose()
}
$hdaLoud = 0
for ($index = 44; $index -lt $hdaBytes.Length; $index++) {
    if ($hdaBytes[$index] -ne 0) { $hdaLoud++ }
}
if ($hdaLoud -lt 2000) {
    throw "AerOS HDA audio capture is silent ($hdaLoud non-zero bytes of $($hdaBytes.Length))"
}
Write-Output $output
Write-Output "AerOS boot test passed"
