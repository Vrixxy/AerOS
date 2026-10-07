# AerOS self-audit

This is the project's own review of its security mechanisms, written from the code and the boot-test. It has not been checked by anyone outside the project. `SECURITY.md` holds the reporting process.

## What is protected from what

| Boundary | Mechanism | Where |
| --- | --- | --- |
| User code against the kernel | Ring 3 with SMEP, SMAP and NX where the CPU has them; every user pointer goes through `user::copy_*` / `range_accessible`, which check the task's own range and the page tables | `arch/user.rs`, `arch/paging.rs` |
| One process against another | Separate page tables, per-process ASLR slot, copy-on-write fork with frame reference counts | `arch/paging.rs`, `scheduler.rs` |
| A process against its own privileges | Linux capability sets that only shrink (`capset`); enforced for CHOWN, KILL, NET_BIND_SERVICE, SYS_PTRACE, SYS_ADMIN, SYS_BOOT, SYS_NICE, SYS_TIME | `capability.rs`, `syscall.rs` |
| A process against the syscall surface | seccomp strict and filter mode (one filter per process, inherited by fork) | `seccomp.rs`, `syscall.rs` |
| A user against another user | Account database with PBKDF2 hashing, lockout, lock screen with idle timeout, `ear` elevation with re-authentication | `auth.rs`, `shell.rs` |
| Devices against memory | AMD-Vi IOMMU: devices reach only the DMA memory drivers registered and the buffer lent for one transfer; everything else is blocked and logged | `iommu.rs` |
| The network against the host | Packet filter with protocol and port rules, TCP engine with bounded buffers | `firewall.rs`, `tcp.rs`, `tcpnet.rs` |
| Installed software | Ed25519-signed packages, hash-checked writes, rollback | `pkg.rs`, `ed25519.rs` |
| Programs on disk | Scan on exec (`antivirus.rs`), quarantine | `antivirus.rs` |
| History | Persistent audit log of logins, elevation, seccomp kills and similar events | `audit.rs` |
| A guest against the host | AMD-V with nested paging; the guest sees only its own RAM and the devices the bridge gives it | `svm.rs` |

## Debugging interface

`ptrace` lets a process read and change the registers and memory of a task it traces. A task can be attached only by its parent or by a holder of `CAP_SYS_PTRACE`; all processes start with every capability, so inside AerOS every process can trace its children and, being root, any other process. Dropping `CAP_SYS_PTRACE` with `capset` removes that. Pokes into shared pages (program text after a fork, copy-on-write data) first give the tracee a private copy, so a debugger cannot alter the parent or a sibling through a shared frame.

## What the boot-test shows

Every run exercises ASLR, SMEP/SMAP set-up, seccomp strict and filter modes, capability enforcement, the audit log, the firewall, the exec antivirus gate, the IOMMU (a real DMA-capable test device is blocked outside its mapping and after the mapping is withdrawn), and deterministic fuzzing of every parser that takes untrusted bytes (ELF, images, antivirus, both FAT drivers on corrupted disks, seccomp programs, package headers, `/proc` paths, TCP frames and segments, the shell) plus two syscall fuzzers. This shows the mechanisms work. It does not show there are no bugs.

## Problems found and fixed during development

- Stale exit status: a freed task slot kept its exit status, so the next task to take the slot looked finished at once (`wait4` returned the previous child's status). Slots are now reset when a task is created.
- `wait4(-1)` followed only the first child.
- FAT ordering bugs found by power-loss injection (old chain freed before the new entry; duplicated directory entry on replace).
- A futex priority-inheritance hand-over returned `EDEADLK` to a waiter that had just been given the lock.
- Fork snapshots lost the thread pointer (FS base) for new threads.
- Event-log overflow in the IOMMU stopped fault reporting until the status bit was cleared.

## Known weaknesses

- No kernel address-space layout randomisation: the kernel is loaded by the firmware at a fixed-per-boot address inside the firmware's identity map, and all physical memory is identity-mapped. A kernel read primitive reveals everything.
- The kernel's stack protector (`-Zstack-protector=strong`, a random cookie per boot) catches overwritten return addresses in functions with buffers, but it is a mitigation, not a guarantee, and there is `unsafe` code throughout the drivers. It depends on an unstable compiler flag.
- Swap writes user pages to the swap partition unencrypted and does not wipe slots when they are freed.
- Nothing verifies the kernel image the firmware loads: updates are signature-checked and a changed image is reported at the next boot (software measurement), but there is no UEFI Secure Boot signing and no TPM driver, so an attacker who can write the boot volume can replace both the image and its sealed digest.
- `/home` is not encrypted.
- Every process starts as root with every capability; the capability model can only be used to give privileges away, not to grant them from a lower-privilege start.
- TLS 1.3 exists as a client (`https` shell command, `tlsnet.rs`) but nothing else uses it yet: the desktop browser and the package manager still speak plain HTTP, the latter protected by its own signatures. The client has no revocation checking and refuses certificates with name constraints rather than enforcing them; the root table is a snapshot of the Mozilla list and does not update by itself; AES-GCM uses a table-lookup S-box (cache-timing exposure), so ChaCha20-Poly1305 is offered first; the big-number code used for signature checks is variable-time, which is fine for public keys and signatures but must never be reused for secrets. There is no server side.
- IOMMU coverage is AMD-V only, one shared domain for all devices (a compromised device can reach another driver's DMA buffers), no interrupt remapping, and it has been run only in QEMU.
- ACPI AML from firmware is executed in the kernel without sandboxing; a malicious table could read mapped physical memory and write device registers (reads and writes outside mapped memory are refused).
- The Intel VT-x backend (`vmx.rs`) has never run on an Intel processor, so its isolation of a guest is unverified; it is compiled into boot-test builds only. The AMD-V path is the one the desktop uses and the one the boot-test exercises.
- Timing side channels: the AES-free crypto in the tree (PBKDF2/SHA, Ed25519) is not audited for constant-time behaviour.
- The antivirus is signature based and only covers what is in its list.
- Hardware has been tried only under QEMU; the device matrix is empty.

## How to report

See `SECURITY.md`.
