# AerOS Kernel

AerOS is an independent x86-64 operating-system kernel written in Rust. The current foundation boots through UEFI, exits firmware services, discovers ACPI and CPU security capabilities, normalizes the firmware memory map, verifies a reusable physical-frame allocator, initializes a volatile framebuffer, renders with the two bundled typefaces, and exposes deterministic serial diagnostics.

## Current guarantees

- Dependency-free `no_std` kernel binary
- UEFI system-table validation and bounded boot-services handoff
- GOP framebuffer support for RGB, BGR, and bitmask formats
- ACPI 1.0 and 2.0 root-table discovery with signature and checksum validation
- Checked XSDT and MADT traversal with processor, local APIC, I/O APIC, and interrupt-override discovery
- Kernel-owned GDT, ring-0 and ring-3 segments, TSS, and isolated double-fault stack
- Complete 256-vector IDT with exception error-frame normalization and a boot-time breakpoint test
- Remapped 8259 interrupt fallback and interrupt-driven PIT clock with a boot-time IRQ test
- Local APIC enablement, PIT-referenced timer calibration, periodic IRQ self-test, and APIC-driven preemption
- Firmware-reserved real-mode trampoline with INIT/SIPI startup for up to eight processors
- Per-CPU GDT, TSS, ring-0, double-fault, and bootstrap stacks with AP identity validation
- Cache-line-isolated AP work counters, fixed-IPI acknowledgement, interrupt-driven AP work queues, and AP idle
- ACPI interrupt-override routing through the I/O APIC with the legacy PIC fully masked
- ACPI HPET discovery, MMIO counter initialization, monotonic nanosecond conversion, and Linux `clock_gettime`
- RDSEED/RDRAND-seeded ChaCha20 kernel generator, randomized user layout, and Linux `getrandom`
- Kernel-owned PML4 root with inherited boot mappings and a private high-half page-table subtree
- NX activation, supervisor write protection, and a post-CR3-switch high-half mapping probe
- One MiB high-half kernel heap with alignment-aware allocation, checked metadata, release, and coalescing
- Ring-3 code and guarded-stack mappings with U/S permission propagation, W^X leaf policy, SMEP, and SMAP
- DPL3 interrupt syscall gate with ABI discovery, return-to-user, and controlled process exit to kernel context
- Native x86-64 `SYSCALL`/`SYSRET` entry with dedicated kernel stack and Linux register calling convention
- Per-CPU 32 KiB syscall stacks, GS-local CPU identity, and a balanced `SWAPGS` entry protocol
- Linux-numbered `open`, `openat`, `read`, `write`, `close`, `poll`, `getpid`, `exit`, and `exit_group` compatibility calls
- Linux `uname`, identity, thread-ID, robust-list, and `arch_prctl` FS/GS thread-local-storage compatibility
- Linux `fstat`, `newfstatat`, `lseek`, `readlinkat`, `prlimit64`, `rseq`, and basic futex compatibility
- Linux `chdir`, canonical `getcwd`, and bounded relative `AT_FDCWD` resolution for open, metadata, status, and access operations
- Linux `mkdir`, `mkdirat`, `rename`, `renameat`, `renameat2`, `unlink`, `unlinkat`, and `rmdir` backed by AerOS VFS mutation with verified cleanup and inode reuse
- Linux `stat`, `ftruncate`, `fsync`, `fdatasync`, `chmod`, `fchmod`, and `fchmodat` with bounded file growth, zero-filled extension, and discarded-byte scrubbing
- Linux `dup`, `dup2`, `dup3`, and `fcntl` descriptor duplication with shared file positions, descriptor-local close-on-exec state, collision replacement, and last-reference release
- Linux `brk`, anonymous and eagerly populated file-private `mmap`, `mprotect`, and `munmap` over zero-filled per-process frames with W^X and TLB invalidation
- SMAP-aware user copies validated against active-process bounds and every page-table permission level
- User-origin exception containment with signal-style exit status and kernel survival after invalid user instructions
- Address-space teardown that removes PML4 branches, scrubs process data, and atomically returns every owned frame
- Read-only initramfs VFS with bounded inodes, canonical traversal, Unix modes, cursor-based reads, and generation-safe descriptors
- Writable bounded tmpfs with file creation, directories, removal, atomic compatible-target rename replacement, permission changes, and safe node and storage reuse
- Strict x86-64 PIE ELF validation and loading with bounded program headers, overflow rejection, W^X enforcement, BSS initialization, and guarded user stacks
- `/bin/init` discovery through the VFS and execution from allocator-backed ring-3 pages
- Separately compiled `no_std` and ordinary Rust `std` static-PIE applications built for the Linux-musl ABI and executed directly as AerOS processes
- Linux `_start` stack with aligned `argc`/`argv`/`envp` and PHDR, entry, identity, platform, executable, and random auxiliary vectors
- Bounded PCI enumeration with multifunction discovery, BAR capture, capability-loop protection, and class summaries
- Native AHCI discovery, controller ownership, command-engine setup, ATA IDENTIFY, and polling DMA sector reads
- Native NVMe driver: controller identify, I/O queue setup, and verified sector read/write
- Native xHCI USB stack: device enumeration, HID keyboard, mouse, and tablet input, USB hubs, and USB mass storage
- Native virtio-blk and virtio-net drivers (legacy transport), plus a modern virtio 1.0+ PCI transport that backs a virtio-input multi-touch driver feeding the same absolute-pointer path as the USB tablet
- Realtek RTL8139 NIC and SD/MMC host-controller (SDHCI) drivers, discovered and exercised alongside AHCI, NVMe, e1000/e1000e, and the virtio devices above
- Reusable synchronized block reads with bounded MBR and CRC-checked GPT partition discovery
- Native FAT16/FAT32 engine: formatting, mounting, subdirectories, VFAT long names, timestamps, rename/move, truncate, delete, and multi-gigabyte files, over any of the disk backends above
- A `/home` data volume (auto-formatted on first sight) and automatic `/media` mounting for inserted USB/SD FAT volumes, backing real Notes, Trash, and Downloads storage
- Native Intel e1000/e1000e DMA rings with MAC/link discovery and verified ARP transmit/receive
- Reusable Ethernet queues with checked IPv4 and ICMP construction, parsing, checksums, and echo exchange
- DHCP lease acquisition, UDP sockets, checked DNS queries, and live shell-driven ICMP and DNS probes
- Native bounded TCP client with SYN/SYN-ACK validation, sequence tracking, transport checksums, acknowledgement handling, HTTP/1.1 requests, status parsing, and clean connection close
- Real time zones (46 zones with US/EU/AU/NZ DST rules) and an SNTP client that syncs against pool.ntp.org/time.cloudflare.com; the dock clock and Settings show local time
- Native PNG (all colour types/depths, interlacing), baseline and progressive JPEG, and TrueType decoders, feeding the wallpaper, Store icons, and non-ASCII text rendering, plus a PrintScreen screenshot capture saved to `/home/Pictures`
- AerOS Shield: a layered on-demand and on-execute malware scanner (SHA-256 hashes, byte signatures including multi-part patterns, and structural heuristics) with quarantine/restore, an optional realtime mode, and an `av` shell command
- Heap-backed 64 KiB kernel-task stacks, lifecycle states, round-robin context switching, task exit, and safe reaping
- Scheduler-owned aligned XSAVE/FXSAVE images with clean task initialization, switch-time preservation, lifecycle reclamation, and adversarial SIMD isolation testing
- Local-APIC-driven timer preemption proven with two CPU-bound tasks that never call the cooperative yield path
- CPU capability discovery for NX, SYSCALL, 1 GiB pages, x2APIC, SMEP, SMAP, and invariant TSC
- Per-CPU x87, SSE, XSAVE, and AVX enablement with an arithmetic validation probe
- Coalesced memory-map ingestion with overflow checks
- Page-aligned physical allocation, recycling, contiguous allocation, and a boot-time invariant test
- COM1 diagnostics with bounded transmit waits
- Plus Jakarta Sans interface rendering
- Roboto Mono diagnostic rendering
- IRQ-driven PS/2 keyboard input routed through I/O APIC vector 52
- Interactive framebuffer terminal with quoting, escaping, history navigation, and command completion
- `aersh` registry containing 87 built-in commands with one-command `ear` privilege elevation, real salted-and-hashed account passwords (`passwd`), loading and running arbitrary ELF binaries (`run`), the AerOS Shield scanner (`av`), and a `notify` command that posts a desktop notification
- Native AerUI geometry, fixed-point scaling, button interaction state, clipping, and bounded software backdrop blur
- Responsive AerOS desktop with the prototype's 752×458 logical layout, embedded mountain wallpaper, a 702×71 seven-control dock, a live time/date capsule that opens Quick Settings when clicked, a 702×420 app switcher, Quick Settings, Settings, Browser, a Store app, a windowed Linux-guest session, and a shell launcher — icon art is a mix of hand-drawn vector glyphs (status tray, Quick Settings) and designed bitmap icons (the dock and app-grid tiles)
- Full setup flow from the Figma mockup: Language, a searchable Keyboard-language list (drawn from all 12 real keyboard layouts and applied live), Username, Password, Confirm, and a "Welcome to AerOS" screen with the traced leaf mark, before falling into the two-stage lock/sign-in screen on a shared glass card; advanced with Enter, skippable with Escape, and the account — a real salted PBKDF2-HMAC-SHA256 password hash — is only written to disk once the Welcome screen has shown
- Lockout screen: after repeated wrong passwords, the wallpaper blurs behind a breathing dark disc with a live "disabled for N seconds" countdown; a wrong password also pops a red cross on the sign-in field
- System-wide power menu (`P` or the power-button hotkey): Terminal, Task manager, and Lock pills plus a power button with a Restart / Shutdown / Sleep flyout, each backed by a real black start-up-style screen with the leaf mark and a progress pill
- Control center redesign: hand-drawn vector Wi-Fi, Bluetooth, hotspot, and battery icons that animate between on/off tints, plus brightness and volume sliders that drive the real screen brightness and the audio mixer
- Left/right screen-edge brightness and volume HUD pills (F5/F6 and the volume keys) that fade, bounce, and redraw their icon based on level
- Desktop notification center: a toast for the newest notification; `N`, or tapping the toast, opens a stacked history of the last three, and AerOS Shield events and screenshots post through the same path
- System-wide `/` search: a glass search field with live-filtered results for every app, opened and dismissed with spring motion
- Dock icons spring up under the pointer, and hovering a dock icon whose app is already open shows a small live preview of its window above the dock
- Native PS/2 mouse with a shadow-buffer software cursor (no framebuffer reads), a diffing presenter that pushes only changed pixels, and a cached wallpaper compositor so redraws land as finished frames
- Super key opens the app switcher; focused dock and app entries surface a name pill
- Browser address bar with keyboard text entry, host/path parsing, an explicit HTTPS-unsupported notice, and distinct DNS-failure, connection-failure, and HTTP-error states over the native DNS/TCP/HTTP path
- Off-screen desktop composition with a bounded 1920×1080 back buffer and completed-frame presentation, preventing partially drawn glass surfaces from becoming visible
- Linux executable routing that keeps supported static PIE binaries native and sends dynamic ELF executables and scripts to the Debian 13 compatibility runtime
- AerOS-owned AMD-V: the kernel enables SVM and runs three verified guests at boot (each gated by the harness) — a 16-bit unreal-mode smoke that round-trips a marker through 20 MB-high nested-page-table RAM, a 64-bit long-mode guest with its own in-guest page tables, and a Linux 64-bit boot-protocol handoff that parses a bzImage, builds a `boot_params` zero page (setup header, e820 map, command line), loads the protected-mode kernel, and enters `startup_64` with `RSI` pointing at `boot_params`
- Boots a real unmodified Debian 13 Linux kernel (6.12) to userspace `/init` on the AerOS hypervisor (`cargo build --features linux-guest`, driven by `tools/boot-linux.ps1`): the kernel and 37 MB initramfs are read from disk over a new multi-sector AHCI DMA path and FAT file reader, loaded into up to 320 MB of contiguous guest RAM behind a full 4 GB nested page table, and run against an emulated 16550 UART (with live host-serial passthrough), i8259 PIC, i8253 PIT, CMOS, and a kvm-clock paravirtual clock that lets the kernel skip PIT calibration; a persistent `#VMEXIT` dispatcher services IO/CPUID/MSR/PAUSE/NPF, injects timer interrupts through the VMCB virtual-interrupt fields, and stops cleanly on guest reset
- A separate windowed Linux-guest desktop session (its own framebuffer bridge, PS/2-style input, and virtio-blk/virtio-net), and a Store app backed by a 24-app Flathub catalog with fetched icons, for installing real Linux applications into that guest
- `tools/extract-debian-kernel.ps1` pulls `vmlinuz` and `initrd.img` out of the Debian compatibility qcow2 into the ESP so the hypervisor can load them
- Self-installing: on a machine with a blank second SATA disk, AerOS writes a protective MBR + GPT with an EFI System Partition, formats it FAT32, lays down its own `\EFI\BOOT\BOOTX64.EFI` and a marker file, and firmware then boots the disk directly — verified end to end by `tools/test-install.ps1` (install to a blank virtual disk, then boot from that disk alone). Only a fully-zeroed target disk is touched, to protect existing data.
- Multi-disk AHCI with multi-sector read/write DMA, a bounce-buffer sector path, cache flush, and a boot-time write probe on non-boot disks
- Hardware virtualization discovery with a no-software-emulation performance policy and default-deny guest access to host files and devices
- Reproducible Debian compatibility-image builder with official SHA-512 verification, an immutable base image, and a writable qcow2 overlay
- Static Rust-musl Debian guest agent with bounded virtio-serial framing, absolute-path validation, explicit permission masks, unprivileged UID/GID 65534 launches, synchronous child reaping, and exit reporting
- Hardware-accelerated compatibility acceptance test that provisions a clean Debian guest, withholds networking, launches `/usr/bin/true`, and requires a verified exit status without software emulation
- Shared glass button component with resting, hovered, pressed, focused, selected, and disabled states plus keyboard activation
- Real motion throughout the desktop, driven by a genuine periodic compositor tick rather than instant state snaps: a press flash on dock icon activation, a fade-from-black on every screen transition (setup steps, lock, sign-in, arriving at the desktop), spring and pop-in animation on the setup, power, control center, notification, search, and context-menu surfaces, and the app switcher and quick settings panels sliding up into place when opened, still drawn as real frosted glass while they slide
- A boot chime and a notification sound, decoded to raw PCM at build time and mixed live on top of whatever the AC'97/HDA audio driver is already playing
- Real threads (`clone`, `clone3`, futexes with priority inheritance and robust lists, thread-local storage) and `ptrace` (signal, syscall, exec, breakpoint, single-step and fault stops, register and memory access)
- Block layer with a write-through page cache and an elevator I/O scheduler, a mount table with bind mounts, and swap on a partition formatted as swap
- Slab allocator, SMP job scheduler with per-CPU queues and stealing, and Linux capability sets enforced for eight privileged operations
- TCP with retransmission, congestion control and SACK; IPv6 with SLAAC, sockets, TCP, UDP and AAAA lookups; the full socket-option surface
- AMD-Vi IOMMU: devices reach only the memory drivers set aside for DMA, and everything else is blocked and logged
- ACPI AML interpreter running the firmware's DSDT and SSDTs (`\_S5_`, resources, PCI interrupt routing) with the `acpi` shell command
- ACPI events: the system control interrupt (power button, general-purpose events, embedded controller and its `_Qxx` events) and `battery` and `thermal` readings through the interpreter
- Compiler stack protector with a per-boot random cookie; signed boot-image updates with crash-safe install and rollback; software measured boot with a sealed image digest
- Installer that works on blank disks and on existing GPT disks with free space, never touching the boot disk
- `docs/SECURITY_AUDIT.md` (self-audit), `docs/BENCHMARKS.md` (first comparison with Linux) and `CHANGELOG.md` (everything above in detail)
- Headless QEMU boot test with a machine-readable ready signal

## Build

```powershell
powershell -ExecutionPolicy Bypass -File C:\Users\hkvla\AerOS\tools\build.ps1
```

The build compiles the bundled userspace programs (init, the std smoke test, the fork probe, and the thread, ptrace, swap and benchmark test programs) for `x86_64-unknown-linux-musl` before embedding them in the initramfs.

To download and verify the Debian 13 compatibility image and create its writable overlay:

```powershell
powershell -ExecutionPolicy Bypass -File C:\Users\hkvla\AerOS\tools\build-compat-image.ps1
```

To require a hardware-accelerated Debian boot with no software-emulation fallback:

```powershell
powershell -ExecutionPolicy Bypass -File C:\Users\hkvla\AerOS\tools\test-compat-guest.ps1
```

## Run

```powershell
powershell -ExecutionPolicy Bypass -File C:\Users\hkvla\AerOS\tools\run.ps1
```

The launcher and harnesses boot QEMU with `accel=whpx:tcg`; they fall back to software emulation automatically when the Windows Hypervisor Platform is unavailable, which is markedly slower.

## Install AerOS to a disk

Boot AerOS on a UEFI + SATA/AHCI machine (or VM) that has a **blank second disk**. It writes a GPT + EFI System Partition, formats FAT32, installs its own bootloader-less `BOOTX64.EFI`, and the firmware boots that disk from then on. Verify the whole flow in QEMU:

```powershell
powershell -ExecutionPolicy Bypass -File C:\Users\hkvla\AerOS\tools\test-install.ps1
```

The installer writes to a blank disk, or to the largest free gap of an existing GPT disk, and keeps the partitions already there; a disk with data and no usable GPT, a damaged table, or the boot disk is refused. NVMe disks are not yet supported as install targets.

## Boot Linux on the AerOS hypervisor

```powershell
powershell -ExecutionPolicy Bypass -File C:\Users\hkvla\AerOS\tools\extract-debian-kernel.ps1
powershell -ExecutionPolicy Bypass -File C:\Users\hkvla\AerOS\tools\boot-linux.ps1
```

The first script pulls `vmlinuz` and `initrd.img` out of the Debian compatibility image into the ESP. The second builds the kernel with `--features linux-guest` and boots it; AerOS then loads and runs the Debian kernel under its own AMD-V hypervisor, streaming the guest serial log and asserting it reaches userspace `/init`. A recorded transcript is in `docs/linux-guest-boot.txt`.

## Test

```powershell
powershell -ExecutionPolicy Bypass -File C:\Users\hkvla\AerOS\tools\test.ps1
```

The boot harness accepts `-CpuCount 2` through `-CpuCount 8`; the maximum topology is part of the release audit.

Other harnesses: `tools\test-install.ps1` and `tools\test-install-existing.ps1` (installer on blank and populated disks), `tools\bench-linux.ps1` (the Linux side of the benchmark comparison), and `cargo test` in `tools\aml-host` (the AML interpreter against a DSDT captured from QEMU). `tools\make-pkg-vectors.py` regenerates the signed package and update test vectors. A boot-test failure that mentions a build error means the harness ran the previous image: check the build output first.

## Capture the framebuffer

```powershell
powershell -ExecutionPolicy Bypass -File C:\Users\hkvla\AerOS\tools\capture.ps1
```

Pass `-View desktop`, `-View apps`, `-View quick`, `-View settings`, `-View browser`, `-View setup`, `-View lock`, or `-View login` to render and capture those states after a verified redraw. Browser capture additionally waits for the real DNS/TCP/HTTP result. Pass `-Command help` to launch the terminal from the desktop, inject a command through the emulated keyboard, require a successful shell audit record, and capture its rendered output.

## Command shell

Normal boots enter the AerOS desktop after kernel validation. `t` launches the framebuffer terminal, `a` toggles the app switcher, `q` toggles quick settings, `b` launches Browser, `s` launches Settings, `p` opens the power menu, `n` toggles the notification center, and `/` opens search. Tab and the arrow keys move focus, Enter activates the focused item, and Escape closes the current overlay or app. Enter refreshes Browser and toggles Wi-Fi on the current Settings page. The local terminal session runs as user `aero`. `ear <command> [args...]` temporarily executes only the nested command with effective UID 0 and restores the original identity afterward.

The current command set is:

```text
notify help man clear echo pwd cd ls cat head tail wc stat touch mkdir rm ln
readlink cp mv chmod uname hostname whoami id passwd date uptime sleep ps
kill jobs wait fw sigcheck bench measure update swap acpi battery thermal install aerfs fsck strace
crashes renice grep find du uniq tee sort basename dirname seq true false svc
pkg dmesg free lscpu lspci lsblk mount umount df sync which env export unset
history ip ping dns https route arp netstat reboot shutdown ear run av
```

File mutation is confined to `/tmp`, `/home`, and mounted `/media` volumes; the initramfs and the inspected FAT boot volume remain read-only. `reboot`, `shutdown`, `kill`, `chmod`, `umount`, and `sync` require `ear`. `ear`'s one-shot elevation is a local-console trust model — it does not check the account password or any persistent user database; that PBKDF2-hashed password only guards the desktop lock/sign-in screen described above.

The native UI integration contract for components recreated from Figma is documented in [docs/UI_COMPONENT_CONTRACT.md](docs/UI_COMPONENT_CONTRACT.md).

## Direction

Real lazy demand paging landed: a dedicated kernel memory region backed by frames pulled from a global physical pool only when a page is actually touched, wired through the real page-fault handler (verified end to end by `AEROS_DEMAND_PAGING`, including zero-fill on reuse after release). The scheduler runs genuine concurrent, preemptively-interleaved ring-3 user-mode tasks — each with its own dedicated kernel landing stack so one task's interrupt entry can no longer corrupt another's — verified by `AEROS_CONCURRENT_USER` with over a hundred real context switches between two independent programs during a single run. `fork()` works for real: processes get their own dedicated page-table root for the first time, a forked child resumes at the exact instruction after the syscall with an independent copy of its stack, sharing its code page with the parent — verified by reading both processes' physical stack memory back directly to prove the divergence is real, not just distinct exit codes. On top of that, `execve()` now genuinely replaces a running process's code and stack in place (verified by `AEROS_EXECVE`, including that the process's code page physically changes and that its heap is properly discarded rather than silently inherited by the new program), `wait4()` really blocks a parent until its child exits and reaps it synchronously inside the syscall rather than polling (verified by `AEROS_WAIT4`, where the parent's own exit code can only match if the kernel truly round-tripped the child's real status), and the code page a fork shares between parent and child is now properly refcounted — freed exactly once, only when the last process referencing it is gone, regardless of which one is destroyed first (verified through a 3-generation fork chain, `AEROS_FORK_CHAIN`, not just a single parent/child pair). Processes can also `kill()` a not-yet-run child outright, ask the kernel who they are and who forked them (`getpid()`/`getppid()`), grow a real per-process heap on demand (`sbrk()`, with each forked child getting its own independent copy of every heap page it already had, and `execve()` correctly discarding it), `write()` real output to the console instead of only a numeric exit code, and pull real entropy from the kernel's own RNG with `getrandom()`. The real Linux ABI now works for these processes too, not just a custom syscall table: each `fork()`ed or independently spawned process gets its own file descriptor table, current directory, and signal state — genuinely open, close, and write real files concurrently across processes without one process's changes leaking into another's, and with `fork()` correctly copying that state rather than sharing it. Real Linux-ABI `brk()` (the actual libc allocator convention, via the `syscall` instruction) now works against that same per-process heap too, independently and correctly across a fork — and so do real Linux-ABI `mmap()`, `mprotect()`, and `munmap()`: each process gets its own dedicated 32-page anonymous-mapping region, `mmap()` and `munmap()` genuinely allocate and free physical frames from it (proven by freeing a mapping and watching the next `mmap()` reuse the slot with fresh content), and `mprotect()` really rewrites the hardware page-table permission bits rather than just a bookkeeping flag — proven by deliberately triggering a protection-fault after revoking write access and observing the kernel's own signal-style fault handler terminate the process with the expected SIGSEGV status. Since then the kernel has gained: real threads (`clone`/`clone3`, futexes with priority inheritance and robust lists), `ptrace`, a block layer with page cache and I/O scheduler, a mount table, swap, a slab allocator, an AMD-Vi IOMMU, an ACPI AML interpreter, a stack protector, TCP SACK, IPv6 sockets and DNS, a TLS 1.3 client that checks server certificates against 136 built-in roots (`https`), an Intel VT-x backend (untested on Intel hardware), kernel image randomisation at boot, AerFS (a copy-on-write filesystem with checksummed blocks that survives a power cut at any write; `aerfs format|mount`), signed boot-image updates with rollback, software measured boot, and an installer that handles existing GPT disks. `CHANGELOG.md` has the details, `docs/SECURITY_AUDIT.md` the self-audit and `docs/BENCHMARKS.md` a comparison with Linux (after profiling found three causes, a bare system call costs about 2.3 times Linux and pipes are on par). Still open: a full kernel address-space randomisation (the image is randomised at boot, the direct map is not), VT-x on real Intel hardware (the backend is written but has not run there), Wi-Fi and Bluetooth, a writable root and AerFS as the home volume, ACPI sleep states, and testing on real hardware (the power button and general-purpose events work, a laptop battery through the embedded controller is tested only on the host). Directly supported Linux programs remain native while broader compatibility uses the isolated Debian runtime.



## This readme is AI polished but is 100% accurate and verified
