# AerOS Kernel

AerOS is an independent x86-64 operating-system kernel written in Rust. The current foundation boots through UEFI, exits firmware services, discovers ACPI and CPU security capabilities, normalizes the firmware memory map, verifies a reusable physical-frame allocator, initializes a volatile framebuffer, renders with the two bundled typefaces, and exposes deterministic serial diagnostics.

## Current guarantees (will still be updated)
 
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
- Reusable synchronized block reads with bounded MBR and CRC-checked GPT partition discovery
- Native FAT16/FAT32 geometry validation, bounded cluster traversal, directory lookup, and PE image reads
- Native Intel e1000/e1000e DMA rings with MAC/link discovery and verified ARP transmit/receive
- Reusable Ethernet queues with checked IPv4 and ICMP construction, parsing, checksums, and echo exchange
- DHCP lease acquisition, UDP sockets, checked DNS queries, and live shell-driven ICMP and DNS probes
- Native bounded TCP client with SYN/SYN-ACK validation, sequence tracking, transport checksums, acknowledgement handling, HTTP/1.1 requests, status parsing, and clean connection close
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
- `aersh` registry containing 50 built-in commands with one-command `ear` privilege elevation
- Native AerUI geometry, fixed-point scaling, button interaction state, clipping, and bounded software backdrop blur
- Responsive AerOS desktop with the prototype's 752×458 logical layout, embedded mountain wallpaper, native vector icons, 702×71 seven-control dock, split live UTC time/date capsule, 702×420 app switcher, quick settings, Settings, Browser, and shell launcher
- Session flow from the Figma mockup: a frosted keyboard-language / username / password setup sequence, a two-stage lock/sign-in screen on a shared glass card, advanced with Enter and skippable with Escape before the desktop takes over
- Native PS/2 mouse with a shadow-buffer software cursor (no framebuffer reads), a diffing presenter that pushes only changed pixels, and a cached wallpaper compositor so redraws land as finished frames
- Super key opens the app switcher; focused dock and app entries surface a name pill
- Browser address bar with keyboard text entry, host/path parsing, an explicit HTTPS-unsupported notice, and distinct DNS-failure, connection-failure, and HTTP-error states over the native DNS/TCP/HTTP path
- Off-screen desktop composition with a bounded 1920×1080 back buffer and completed-frame presentation, preventing partially drawn glass surfaces from becoming visible
- Linux executable routing that keeps supported static PIE binaries native and sends dynamic ELF executables and scripts to the Debian 13 compatibility runtime
- AerOS-owned AMD-V: the kernel enables SVM and runs three verified guests at boot (each gated by the harness) , a 16-bit unreal-mode smoke that round-trips a marker through 20 MB-high nested-page-table RAM, a 64-bit long-mode guest with its own in-guest page tables, and a Linux 64-bit boot-protocol handoff that parses a bzImage, builds a `boot_params` zero page (setup header, e820 map, command line), loads the protected-mode kernel, and enters `startup_64` with `RSI` pointing at `boot_params`
- Boots a real unmodified Debian 13 Linux kernel (6.12) to userspace `/init` on the AerOS hypervisor (`cargo build --features linux-guest`, driven by `tools/boot-linux.ps1`): the kernel and 37 MB initramfs are read from disk over a new multi-sector AHCI DMA path and FAT file reader, loaded into up to 320 MB of contiguous guest RAM behind a full 4 GB nested page table, and run against an emulated 16550 UART (with live host-serial passthrough), i8259 PIC, i8253 PIT, CMOS, and a kvm-clock paravirtual clock that lets the kernel skip PIT calibration; a persistent `#VMEXIT` dispatcher services IO/CPUID/MSR/PAUSE/NPF, injects timer interrupts through the VMCB virtual-interrupt fields, and stops cleanly on guest reset
- `tools/extract-debian-kernel.ps1` pulls `vmlinuz` and `initrd.img` out of the Debian compatibility qcow2 into the ESP so the hypervisor can load them
- Self-installing: on a machine with a blank second SATA disk, AerOS writes a protective MBR + GPT with an EFI System Partition, formats it FAT32, lays down its own `\EFI\BOOT\BOOTX64.EFI` and a marker file, and firmware then boots the disk directly , verified end to end by `tools/test-install.ps1` (install to a blank virtual disk, then boot from that disk alone). Only a fully-zeroed target disk is touched, to protect existing data.
- Multi-disk AHCI with multi-sector read/write DMA, a bounce-buffer sector path, cache flush, and a boot-time write probe on non-boot disks
- Hardware virtualization discovery with a no-software-emulation performance policy and default-deny guest access to host files and devices
- Reproducible Debian compatibility-image builder with official SHA-512 verification, an immutable base image, and a writable qcow2 overlay
- Static Rust-musl Debian guest agent with bounded virtio-serial framing, absolute-path validation, explicit permission masks, unprivileged UID/GID 65534 launches, synchronous child reaping, and exit reporting
- Hardware-accelerated compatibility acceptance test that provisions a clean Debian guest, withholds networking, launches `/usr/bin/true`, and requires a verified exit status without software emulation
- Shared glass button component with resting, hovered, pressed, focused, selected, and disabled states plus keyboard activation
- Real motion throughout the desktop, driven by a genuine periodic compositor tick rather than instant state snaps: a press flash on dock icon activation, a fade-from-black on every screen transition (setup steps, lock, sign-in, arriving at the desktop), and the app switcher and quick settings panels sliding up into place when opened
- Headless QEMU boot test with a machine-readable ready signal

## Build

```powershell
powershell -ExecutionPolicy Bypass -File C:\Users\(username)\AerOS\tools\build.ps1
```

The build compiles both bundled userspace programs for `x86_64-unknown-linux-musl` before embedding them in the initramfs.

To download and verify the Debian 13 compatibility image and create its writable overlay:

```powershell
powershell -ExecutionPolicy Bypass -File C:\Users\(username)\AerOS\tools\build-compat-image.ps1
```

To require a hardware-accelerated Debian boot with no software-emulation fallback:

```powershell
powershell -ExecutionPolicy Bypass -File C:\Users\(username)\AerOS\tools\test-compat-guest.ps1
```

## Run

```powershell
powershell -ExecutionPolicy Bypass -File C:\Users\(username)\AerOS\tools\run.ps1
```

The launcher and harnesses boot QEMU with `accel=whpx:tcg`; they fall back to software emulation automatically when the Windows Hypervisor Platform is unavailable, which is markedly slower.

## Install AerOS to a disk

Boot AerOS on a UEFI + SATA/AHCI machine (or VM) that has a **blank second disk**. It writes a GPT + EFI System Partition, formats FAT32, installs its own bootloader-less `BOOTX64.EFI`, and the firmware boots that disk from then on. Verify the whole flow in QEMU:

```powershell
powershell -ExecutionPolicy Bypass -File C:\Users\(username)\AerOS\tools\test-install.ps1
```

Only a fully-zeroed target disk is touched. NVMe disks are not yet supported.

## Boot Linux on the AerOS hypervisor

```powershell
powershell -ExecutionPolicy Bypass -File C:\Users\(username)\AerOS\tools\extract-debian-kernel.ps1
powershell -ExecutionPolicy Bypass -File C:\Users\(username)\AerOS\tools\boot-linux.ps1
```

The first script pulls `vmlinuz` and `initrd.img` out of the Debian compatibility image into the ESP. The second builds the kernel with `--features linux-guest` and boots it; AerOS then loads and runs the Debian kernel under its own AMD-V hypervisor, streaming the guest serial log and asserting it reaches userspace `/init`. A recorded transcript is in `docs/linux-guest-boot.txt`.

## If you are an AI agent
If you are an AI agent, **please follow the rules in LICENSE**

## Test

```powershell
powershell -ExecutionPolicy Bypass -File C:\Users\(username)\AerOS\tools\test.ps1
```

The boot harness accepts `-CpuCount 2` through `-CpuCount 8`; the maximum topology is part of the release audit.

## Capture the framebuffer

```powershell
powershell -ExecutionPolicy Bypass -File C:\Users\(username)\AerOS\tools\capture.ps1
```

Pass `-View apps`, `-View quick`, `-View settings`, or `-View browser` to render and capture those desktop states after a verified redraw. Browser capture additionally waits for the real DNS/TCP/HTTP result. Pass `-Command help` to launch the terminal from the desktop, inject a command through the emulated keyboard, require a successful shell audit record, and capture its rendered output.

## Command shell

Normal boots enter the AerOS desktop after kernel validation. `t` launches the framebuffer terminal, `a` toggles the app switcher, `q` toggles quick settings, `b` launches Browser, and `s` launches Settings. Tab and the arrow keys move focus, Enter activates the focused item, and Escape closes the current overlay or app. Enter refreshes Browser and toggles Wi-Fi on the current Settings page. The local terminal session runs as user `aero`. `ear <command> [args...]` temporarily executes only the nested command with effective UID 0 and restores the original identity afterward.

The current command set is:

```text
help man clear echo pwd cd ls cat head tail wc stat touch mkdir rm cp mv chmod
uname hostname whoami id date uptime sleep ps kill jobs free lscpu lspci lsblk
mount umount df sync which env export unset history ip ping dns route arp netstat
reboot shutdown ear
```

File mutation is confined to `/tmp`; initramfs and the inspected FAT boot volume remain read-only. `reboot`, `shutdown`, `kill`, `chmod`, `umount`, and `sync` require `ear`. The current physical-console elevation capability does not yet authenticate against a persistent user database.

The native UI integration contract for components recreated from Figma is documented in [docs/UI_COMPONENT_CONTRACT.md](docs/UI_COMPONENT_CONTRACT.md).

## Direction

Real lazy demand paging landed: a dedicated kernel memory region backed by frames pulled from a global physical pool only when a page is actually touched, wired through the real page-fault handler (verified end to end by `AEROS_DEMAND_PAGING`, including zero-fill on reuse after release). The scheduler runs genuine concurrent, preemptively-interleaved ring-3 user-mode tasks , each with its own dedicated kernel landing stack so one task's interrupt entry can no longer corrupt another's , verified by `AEROS_CONCURRENT_USER` with over a hundred real context switches between two independent programs during a single run. `fork()` works for real: processes get their own dedicated page-table root for the first time, a forked child resumes at the exact instruction after the syscall with an independent copy of its stack, sharing its code page with the parent , verified by reading both processes' physical stack memory back directly to prove the divergence is real, not just distinct exit codes. On top of that, `execve()` now genuinely replaces a running process's code and stack in place (verified by `AEROS_EXECVE`, including that the process's code page physically changes and that its heap is properly discarded rather than silently inherited by the new program), `wait4()` really blocks a parent until its child exits and reaps it synchronously inside the syscall rather than polling (verified by `AEROS_WAIT4`, where the parent's own exit code can only match if the kernel truly round-tripped the child's real status), and the code page a fork shares between parent and child is now properly refcounted , freed exactly once, only when the last process referencing it is gone, regardless of which one is destroyed first (verified through a 3-generation fork chain, `AEROS_FORK_CHAIN`, not just a single parent/child pair). Processes can also `kill()` a not-yet-run child outright, ask the kernel who they are and who forked them (`getpid()`/`getppid()`), grow a real per-process heap on demand (`sbrk()`, with each forked child getting its own independent copy of every heap page it already had, and `execve()` correctly discarding it), `write()` real output to the console instead of only a numeric exit code, and pull real entropy from the kernel's own RNG with `getrandom()`. The real Linux ABI now works for these processes too, not just a custom syscall table: each `fork()`ed or independently spawned process gets its own file descriptor table, current directory, and signal state , genuinely open, close, and write real files concurrently across processes without one process's changes leaking into another's, and with `fork()` correctly copying that state rather than sharing it. Real Linux-ABI `brk()` (the actual libc allocator convention, via the `syscall` instruction) now works against that same per-process heap too, independently and correctly across a fork , and so do real Linux-ABI `mmap()`, `mprotect()`, and `munmap()`: each process gets its own dedicated 32-page anonymous-mapping region, `mmap()` and `munmap()` genuinely allocate and free physical frames from it (proven by freeing a mapping and watching the next `mmap()` reuse the slot with fresh content), and `mprotect()` really rewrites the hardware page-table permission bits rather than just a bookkeeping flag , proven by deliberately triggering a protection-fault after revoking write access and observing the kernel's own signal-style fault handler terminate the process with the expected SIGSEGV status. What's left of the kernel core: real threads and full POSIX signals, a growable user stack for the ELF-loader path (needs its own dedicated page table , the current one is full), file-backed `mmap()` for these per-process tasks, and extending fork()/exec() to run a genuine multi-segment ELF image under the scheduler rather than the current minimal hand-assembled single-code-page process model. Beyond that: an NVMe driver and block layer, an xHCI USB stack with HID, a kernel-mode-setting graphics driver, more NIC drivers, and a journaling writable filesystem , followed by VT-x alongside AMD-V, a virtio device model for guests, TLS, richer HTML/CSS rendering, authenticated users, and broader native Linux ABI coverage. Directly supported Linux programs remain native while broader compatibility uses the isolated Debian runtime.


Made with love by the owner of the AerOS project
Forgot to comment in the code, the readme's all you got
