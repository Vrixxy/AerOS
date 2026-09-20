param(
    [ValidateRange(2, 8)]
    [int]$CpuCount = 2
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
# boot's own VFS mount).
foreach ($name in @("aerosfs.txt", "persist.txt")) {
    $artifact = Join-Path $root "build\esp\$name"
    if (Test-Path -LiteralPath $artifact) {
        Remove-Item -Force -LiteralPath $artifact
    }
}
$testDirArtifact = Join-Path $root "build\esp\testdir"
if (Test-Path -LiteralPath $testDirArtifact) {
    Remove-Item -Force -Recurse -LiteralPath $testDirArtifact
}

$arguments = @(
    "-machine", "q35,accel=whpx:tcg",
    "-m", "512M",
    "-smp", "$CpuCount",
    "-cpu", "qemu64,+nx,+smep,+smap,+xsave,+xsaveopt,+rdrand,+rdtscp,+pdpe1gb",
    "-drive", "if=pflash,format=raw,unit=0,readonly=on,file=$firmware",
    "-drive", "if=pflash,format=raw,unit=1,file=$variables",
    "-drive", "format=raw,file=fat:rw:$esp",
    "-device", "virtio-vga",
    "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
    "-display", "none",
    "-serial", "file:$serial",
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
$deadline = [DateTime]::UtcNow.AddSeconds(25)
$output = ""

while ([DateTime]::UtcNow -lt $deadline) {
    if (Test-Path -LiteralPath $serial) {
        $output = [string](Get-Content -Raw -LiteralPath $serial)
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
    throw "AerOS did not reach the ready state`n$output"
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
if ($output -notmatch "AEROS_ENTROPY .* hardware_words=([8-9]|[1-9][0-9]+) sample_a_nonzero=true sample_b_nonzero=true distinct=true chacha20=true aslr=true verified=true") {
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
if ($output -notmatch "AEROS_VFS nodes=10 directories=5 files=5 bytes=395765 handles=0 mutable_files=0 mutable_bytes=0 readonly_root=true verified=true") {
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
if ($output -notmatch "AEROS_TMPFS path=/tmp nodes=11 directories=5 files=6 mutable_files=1 mutable_bytes=6 handles=0 verified=true") {
    throw "AerOS writable tmpfs validation failed`n$output"
}
if ($output -notmatch "AEROS_SYSCALL .* calls=161 bootstrap=2 linux=159 exits=4 unknown=0 opens=13 reads=12 writes=10 closes=17 io_bytes=264 clocks=1 random_calls=1 random_bytes=32 compat_calls=73 memory_calls=15 mmaps=5 file_mmaps=1 mprotects=2 munmaps=4 metadata=9 seeks=2 paths=12 resources=1 rseq=1 futex=1 fd_calls=10 dup_calls=4 last_fd=3 signals=12 runtime=8 directories=1 sockets=4 datagrams=2 network_bytes=[3-9][0-9]+ vectored=2 positional=1 access=1 statx=1 wall_clock=2 sleeps=2 chdir=1 relative_paths=5 polls=1 creates=2 renames=2 removes=5 syncs=2 truncates=1 chmods=1 verified=true") {
    throw "AerOS syscall gate failed`n$output"
}
if ($output -notmatch "AEROS_PCI devices=[1-9].* verified=true") {
    throw "AerOS PCI enumeration failed`n$output"
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
if ($output -notmatch "AEROS_SYSINFO verified=true") {
    throw "AerOS sysinfo validation failed`n$output"
}
if ($output -notmatch "AEROS_NETWORK driver=e1000e present=true .* mac=([0-9a-f]{2}:){5}[0-9a-f]{2} link=true speed_mbps=(100|1000) duplex=true tx=true rx=true rx_bytes=[1-9][0-9]* arp_gateway=true verified=true") {
    throw "AerOS e1000e network validation failed`n$output"
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
if ($output -notmatch "AEROS_SCHEDULER tasks=1 ready=0 running=1 exited=0 switches=([8-9]|[1-9][0-9]+) stack_bytes=0 fpu_tasks=1 fpu_bytes=[5-9][0-9][0-9]+ fpu_switches=([8-9]|[1-9][0-9]+) fpu_isolation=true highest_id=0 verified=true") {
    throw "AerOS scheduler validation failed`n$output"
}
if ($output -notmatch "AEROS_PREEMPT source=lapic ticks=([8-9]|[1-9][0-9]+) work_a=[1-9][0-9]* work_b=[1-9][0-9]* fpu_isolation=true verified=true") {
    throw "AerOS preemption validation failed`n$output"
}
if ($output -notmatch "AEROS_CONCURRENT_USER exit_a=77 exit_b=88 switches=[1-9][0-9]* reaped=2 verified=true") {
    throw "AerOS concurrent user-task scheduling validation failed`n$output"
}
if ($output -notmatch "AEROS_FORK parent_exit=11 child_exit=22 parent_stack=0xaaaaaaaa child_stack=0xbbbbbbbb reaped=2 verified=true") {
    throw "AerOS fork validation failed`n$output"
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
if ($output -notmatch "AEROS_COMMANDS count=51 shell=aersh elevation=ear unique=true parser=true privilege=true filesystem=true verified=true") {
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

Write-Output $output
Write-Output "AerOS boot test passed"
