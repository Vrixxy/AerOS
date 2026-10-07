# Intel VT-x backend

`kernel/src/vmx.rs` runs AerOS's smoke guests on Intel VT-x with EPT, next to the AMD-V path in `svm.rs`. This page says what it covers, how it was checked, and how to run it for real.

## Status: written, not yet run on an Intel processor

The self-test is compiled only into boot-test builds (`tools/test.ps1`), so an ordinary boot on an Intel PC never touches VMX until this has been validated.

The machine the project is built on has an AMD CPU, and QEMU cannot expose VMX to a guest there (TCG has no VMX emulation, WHPX does not pass it through). So:

- **Checked on the host** (`tools/vmx-host`, `cargo test`): every VMCS field encoding against the layout rules in the Intel SDM (access type, width class and field class are all encoded in the number, and the test parses the source to check each constant), control-bit adjustment against the capability MSRs, segment access-rights conversion, the EPT tables (built into a map and walked by an independent software walker for every page, plus the unmapped cases), I/O exit decoding.
- **Checked by the compiler and clippy**: the instruction wrappers, the VM-entry and exit assembly, the VMCS programming.
- **Checked on AMD in the boot-test**: the shared guest programs (`svm.rs` now exports them) still pass on AMD-V, and `AEROS_VMX` reports `supported=false verified=skipped`.
- **Not checked**: that a real processor accepts the VMCS and runs the guests. The first run on Intel hardware is the real test.

## What it does

- Feature detection (CPUID.1:ECX.VMX), `IA32_FEATURE_CONTROL`, CR0/CR4 fixed bits, `CR4.VMXE`, `VMXON`.
- A VMCS per guest: pin-based (external-interrupt exiting), primary (HLT, unconditional I/O exiting, secondary controls), secondary (EPT, unrestricted guest where offered, RDTSCP), exit and entry controls (64-bit host, load EFER). Every RDMSR and WRMSR exits (no MSR bitmap), so the guest cannot touch real MSRs.
- EPT with 2 MiB pages mapping 32 MiB of guest RAM.
- The exits it handles: CPUID (VMX hidden from the guest), HLT and VMCALL (end of run), I/O to COM1, RDMSR/WRMSR (emulated as zero and ignored), external interrupts (the host takes them with a short `sti`), EPT violations (reported).
- Three guests, the same programs as AMD-V: a real-mode one (needs unrestricted guest), a 64-bit one, and the Linux boot-protocol probe image.

## What it does not do

- The Linux desktop window, the guest devices (serial, virtio, framebuffer bridge, input) and the two-CPU mode in `svm.rs` are still AMD-V only. Porting them means putting the ~3,200 lines that read and write VMCB fields behind a backend interface; the exit handling they contain is the same, the register access is not.
- No VPID, no MSR or I/O bitmaps, no preemption timer, no NMI exiting (an NMI during a guest would be delivered to the guest), no nested virtualisation.

## Running it

Any of these gives the guest a VMX-capable CPU:

- An Intel PC booting the AerOS USB image (`tools/build.ps1`).
- QEMU with KVM on an Intel Linux host with nested virtualisation on: `-enable-kvm -cpu host` (or `-cpu qemu64,+vmx` with KVM) and the same arguments `tools/test.ps1` uses.
- VMware or VirtualBox with "Virtualize Intel VT-x/EPT" turned on.

Then look for the `AEROS_VMX` line in the serial log. `tools/test.ps1` accepts either the skip line or a line with `real_mode_ok=true long_mode_ok=true linux_probe_ok=true instruction_error=0 verified=true`.

## If it fails

Fields in the `AEROS_VMX` line and what they point at:

- `supported=true enabled=false`: `VMXON` failed or `IA32_FEATURE_CONTROL` is locked off in firmware (`locked_off=true`).
- `ept=false`: the processor lacks 2 MiB EPT pages in write-back memory; nothing runs.
- `instruction_error=N` (the VM-instruction error number, SDM volume 3 section 30.4): 7 is an invalid control field (check the adjust step), 8 an invalid host-state field (selectors, TR, EFER), 4 or 5 a launch-state mistake, 1 or 2 a VMCLEAR/VMPTRLD pointer problem.
- A guest that never reaches `verified`: look at the line before it. `AEROS_VMX_LAUNCH` without a following console line and `exits=0` means the VM entry failed; an exit reason with bit 31 set is a failed entry (33 is invalid guest state, 34 an MSR-load failure). Invalid guest state is the likeliest first problem; the guest-state checks are in SDM volume 3 section 27.3.1 and `Vm::guest_state` is written to satisfy them one by one.
- `AEROS_VMX_EPT guest_physical=...`: the guest touched memory outside its 32 MiB.

The pure parts live in `kernel/src/vmx_logic.rs` so they can be fixed and re-tested on any machine with `cd tools/vmx-host && cargo test`.
