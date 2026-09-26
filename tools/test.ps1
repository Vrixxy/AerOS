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
$virtioBytes = New-Object byte[] (1MB)
$virtioSignature = [System.Text.Encoding]::ASCII.GetBytes("AEROS-VIRTIO-BLK")
[Array]::Copy($virtioSignature, $virtioBytes, $virtioSignature.Length)
[System.IO.File]::WriteAllBytes($virtioImage, $virtioBytes)
$hdaCapture = Join-Path $root "build\hda-test.wav"
Remove-Item -Force -ErrorAction SilentlyContinue -LiteralPath $hdaCapture
$arguments = @(
    "-machine", "q35,accel=whpx:tcg",
    "-m", "512M",
    "-smp", "$CpuCount",
    "-cpu", "qemu64,+nx,+smep,+smap,+xsave,+xsaveopt,+rdrand,+rdtscp,+pdpe1gb",
    "-drive", "if=pflash,format=raw,unit=0,readonly=on,file=$firmware",
    "-drive", "if=pflash,format=raw,unit=1,file=$variables",
    "-drive", "format=raw,file=fat:rw:$esp",
    "-drive", "file=$homeImage,format=raw,if=ide,index=1",
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
    "-nic", "user,model=e1000e",
    "-netdev", "user,id=rtlnet,net=10.10.0.0/24",
    "-device", "rtl8139,netdev=rtlnet",
    "-netdev", "user,id=vnet0,net=10.9.0.0/24",
    "-device", "virtio-net-pci,netdev=vnet0,disable-modern=on,disable-legacy=off",
    "-drive", "file=$virtioImage,if=none,id=vblk0,format=raw",
    "-device", "virtio-blk-pci,drive=vblk0,disable-modern=on,disable-legacy=off",
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
$deadline = [DateTime]::UtcNow.AddSeconds(90)
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
if ($output -notmatch "AEROS_VFS nodes=[0-9]+ directories=7 files=[0-9]+ bytes=[0-9]+ handles=0 mutable_files=0 mutable_bytes=0 readonly_root=true verified=true") {
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
if ($output -notmatch "AEROS_TMPFS path=/tmp nodes=[0-9]+ directories=7 files=[0-9]+ mutable_files=1 mutable_bytes=6 handles=0 verified=true") {
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
if ($output -notmatch "AEROS_AV signatures=[1-9][0-9]* self_test=true quarantine_flow=true realtime=true scanned_files=[0-9]+ threats=0 unreadable=[0-9]+ verified=true") {
    throw "AerOS Shield antivirus validation failed`n$output"
}
if ($output -notmatch "AEROS_COMMANDS count=53 shell=aersh elevation=ear unique=true parser=true privilege=true filesystem=true verified=true") {
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
