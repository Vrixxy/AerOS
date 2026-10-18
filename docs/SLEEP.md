# Suspend to RAM (ACPI S3)

`suspend --yes` (privileged) puts the machine into S3 and returns after it has woken. `kernel/src/sleep.rs` has the code; the boot-test gate `AEROS_S3` runs it under QEMU with the harness waking the machine.

## Going down

1. The firmware must name an S3 type (`\_S3_` in the DSDT), the FADT must point at a FACS, and the page the other processors start from must lie below 1 MiB with the page-table root below 4 GiB. Otherwise `suspend` says the firmware offers no S3.
2. A program running on another processor refuses the sleep ("programs are running"). Idle processors are parked: their loop sees a flag, they leave the scheduler (`scheduler::cpu_parked`), clear their online bit and halt with interrupts off.
3. On the boot processor, with interrupts off: control registers, the ring-0 stack in the TSS, the IOAPIC redirection table, the local APIC base register and timer rate, the memory-type registers (real hardware only), the clock and the ACPI tables are saved. A `setjmp`-style context (callee-saved registers, stack pointer, return address) is saved in `save_context`.
4. The FACS real-mode waking vector is pointed at the trampoline page (the same page, and the same real-mode to long-mode code, the application processors start from; it now also turns on SSE before it calls Rust). `SLP_TYP` is written to PM1a/PM1b control, then again with `SLP_EN`.

## Coming back

The firmware runs its resume path and jumps to the waking vector in real mode. The trampoline reaches long mode on the kernel page tables and calls `resume_entry` on its own stack. That:

- restores CR4 and CR0, the GDT, TSS (with the saved ring-0 stack), IDT, FPU/SSE state, `syscall` MSRs, SMEP/SMAP and the CPU number register;
- reloads CR3, re-initialises the serial port, the local APIC (base register, spurious vector) and the IOAPIC table (checked by reading it back), and masks the legacy PIC;
- restarts the HPET counter (it counts from zero again) and carries the monotonic clock on from where it stopped, re-reads the battery-backed clock for the wall time, and sets the ACPI event state up again (SCI route, power button, GPEs);
- jumps back into `suspend` through the saved context.

`suspend` then restarts the timer if it was running and starts the parked processors again with INIT-SIPI (`smp::restart_all`), each taking the boot processor's memory-type registers.

## Drivers

After the processors are back, `suspend` calls `sleep::resume_drivers()`, which starts every driver again over the memory it already owns (no allocation): the hardware is reset and its rings and registers are programmed from scratch.

| Driver | What `resume` does |
| --- | --- |
| AHCI | host bus adapter reset, command list and received-FIS memory cleared, ports configured, link and ready wait |
| NVMe | controller disabled and enabled with fresh admin queues, I/O queue pair created again |
| virtio block / net / input | device reset, queue memory cleared and handed back (`Legacy::restore_queue`, `Modern::restore_queue`), receive buffers posted again |
| virtio-gpu | queues restored, display resource and scanout created again, cursor image uploaded again; the 3D context is not recreated (glass falls back to the CPU) |
| xHCI | controller reset, its memory cleared and laid out again, every root port and hub enumerated afresh (keyboards, pointers and disks come back) |
| e1000, RTL8139, RTL8168 | chip reset, station address, rings emptied and programmed |
| Intel HDA, AC'97 | controller and codec reset, buffer list rewritten, the output path routed again |
| SD host controller | controller reset and the whole card initialisation sequence |
| PS/2 | controller reprogrammed, the mouse put back in reporting (and VMware absolute) mode |

A device that is not present counts as fine. The boot-test gate `AEROS_DRIVER_RESUME_TEST` runs the same call on a running system and then does real I/O on every present device: a disk sector read back unchanged on AHCI, NVMe, virtio-blk, SD and USB, an ARP exchange on each Ethernet driver, a tune on each audio controller, and the same USB input devices enumerated again. That proves the reset-and-reprogram path; what no emulator can prove is the hardware really having lost its state.

## What it does not do

The PCI standard header of every function (BARs, bus numbers, expansion ROM, interrupt line, command) is saved before the sleep and written back after it wherever it differs (`pci::save_config`, `pci::restore_config`, decoding off while the address registers change); capability state such as MSI and power management is left to the firmware and the drivers, none of which use MSI. Wake sources other than the power button are not set up. The 3D context is gone after a sleep.

## Testing

The driver restart is exercised by the full boot test (`AEROS_DRIVER_RESUME_TEST`). `tools/s3-quick.ps1` builds the kernel with the `s3-early` feature, which sleeps right after the processors start, wakes the machine from the QEMU monitor (`system_wakeup`) and prints the `AEROS_S3` line: the local timer, the IOAPIC (a PIT interrupt through it), the clock, and a job run on the restarted second processor all have to work. `tools/test.ps1` runs it as its last step.

It runs on QEMU's emulated CPU (`-accel tcg`). Under WHPX, the accelerator the rest of the boot test uses, the firmware's own reset code faults ("WHPX: Unexpected VP exit code 4", an unrecoverable exception in SEC) before it reaches the kernel's waking vector, so a sleep there never wakes; do not run `suspend` in a WHPX guest.

Unproven because no emulator reproduces it: the memory-type register restore and the processor start-up after a real power loss.
