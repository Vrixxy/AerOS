# Security policy

AerOS is an experimental, from-scratch kernel. It has not had an external security audit and should not be used to protect real data.

## Reporting a vulnerability

Please report privately, not in a public issue. Use GitHub's "Report a vulnerability" button on the repository's Security tab (private vulnerability reporting). Include:

- What you found and which component it is in (kernel, shell, filesystem, network stack, hypervisor, desktop).
- Steps or a boot-test patch that reproduces it, and the QEMU command line if relevant.
- The impact you expect: crash, privilege escalation, memory disclosure, guest escape, and so on.

You can expect an acknowledgement within 7 days. Fixes are developed privately, and the report is credited in `CHANGELOG.md` unless you ask otherwise. Please allow 90 days before public disclosure.

## In scope

- Kernel memory safety and privilege boundaries: syscalls, usercopy, paging, ASLR, `ear` elevation.
- Authentication and lockout (`auth.rs`), the audit log, the antivirus exec gate.
- The network stack (TCP engine, UDP port table, sockets), firewall, and the virtio, NVMe, xHCI and AHCI drivers.
- The package manager (`pkg`): signature checking, path handling and hash verification of installed files.
- The AMD-V hypervisor and its Linux guest bridge.

## Out of scope

- Denial of service from unbounded local resource use by an already-privileged user.
- Issues that need physical access to a machine with no disk encryption (`/home` is not encrypted yet).
- Missing features listed under "Known gaps" in `CHANGELOG.md`, such as TLS, kernel address-space randomisation or secure boot.

## Current self-audit

Boot validation also runs deterministic fuzzing of the parsers that take untrusted bytes (ELF, images, the antivirus scanner, both FAT drivers on corrupted disks, seccomp programs, package headers, `/proc` paths, TCP frames and segments) and two syscall fuzzers, one from kernel context and one from a user process with valid pointers. The TCP engine is tested over a simulated wire that drops, duplicates and reorders segments. Anything that panics or hangs fails the boot.


Every boot-test run exercises ASLR, seccomp strict mode, the exec antivirus gate, the audit log and the firewall. That checks that the mechanisms work. It is not a substitute for an independent review.
