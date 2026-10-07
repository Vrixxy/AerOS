# AerOS against Linux

One static program (`userspace/bench`, built for musl) runs unchanged on both systems and times a few operations with the monotonic clock. On AerOS it runs as a process during the boot-test (`AEROS_BENCH` lines). On Linux it is `/init` of a tiny initramfs on the Debian 6.12 kernel the project ships (`tools/bench-linux.ps1`, `tools/make-initramfs.py`). Both run in QEMU with the same CPU model and the same acceleration.

Nanoseconds per operation, lower is better. The Linux column is one run on 2026-10-04; the AerOS columns are one run each on 2026-10-04 (before) and 2026-10-07 (after the three fixes below).

| Operation | AerOS before | AerOS after | Linux | After / Linux |
| --- | ---: | ---: | ---: | ---: |
| xorshift step (CPU calibration) | 3 | 3 | 2 | ~2 |
| `getpid` system call | 37,066 | 344 | 150 | 2.3 |
| `clock_gettime` | 112,134 | 894 | 59 (no system call) | n/a |
| copy 64 KiB | 7,264 | 7,365 | 3,133 | 2.4 |
| `mmap` 8 pages, touch, `munmap` | 77,470 | 6,026 | 25,196 | 0.24 |
| create, write 4 KiB, read back, unlink (tmpfs) | 6,410,115 | 252,311 | 19,580 | 13 |
| `fork`, child exit, `wait4` | 1,435,805 | 31,719 | 84,788 | 0.37 |
| pipe round trip between two processes | 1,432,906 | 6,126 | 5,787 | 1.06 |

## What was wrong, and what fixed it

The first run was two orders of magnitude off on everything that enters the kernel. Profiling it (`kernel/src/perf.rs` prints `AEROS_PERF` lines in the boot-test) found three unrelated causes:

1. **The clock read the HPET.** `time::monotonic_nanoseconds()` read the HPET main counter over memory-mapped I/O on every call, and every system call reads the clock (the trace ring stamps each call). Under a hypervisor each such read traps to the device model and costs tens of microseconds; on real hardware an HPET read is still about a microsecond. The clock now uses the time stamp counter, calibrated against the HPET twice at boot (it is used only if the two rates agree within 2 per cent) and refined every ten seconds from the whole baseline so it cannot drift from the HPET; a clock read is about 10 ns. `getpid` went from 37,066 ns to 344 ns, `clock_gettime` from 112,134 ns (three HPET reads) to 894 ns. The boot-test requires `AEROS_CLOCK source=tsc`.
2. **The antivirus scan.** Every file opened for reading and every file closed after writing is scanned (real-time protection), and the scanner slid each of its 87 built-in and up to 192 loaded signature needles over every byte: 1.7 ms for a 4 KiB file. Needles are now indexed by their first byte and the data is scanned once, so a needle is compared only where its first byte occurs: 40 µs for the same file (18 µs of it is the SHA-256 the scanner also computes). The create-write-read-unlink row fell from 6.4 ms to 0.25 ms. The filesystem itself was never slow: an open, read and close without the scan costs 460 ns.
3. **The idle task took turns.** The boot context waits with `sti; hlt`. When one process blocked (an empty pipe, a `wait4`, an exit), round-robin order often handed the CPU to that idle task, which then slept until the next timer tick (about 0.8 ms) while another process was ready to run. That was the identical 821 µs in the pipe and `fork` rows. A task that gives up the CPU on its own now yields work-first: the boot task is chosen only when nothing else is ready. The timer tick keeps plain round-robin, so the boot task is still scheduled regularly. Pipe round trip: 1,432,906 ns to 6,126 ns. `fork`: 1,435,805 ns to 31,719 ns.

## What it says now

- A bare system call costs about 2.3 times Linux's, pipes are on par, and `fork` and `mmap` are faster than Linux in this test (Linux's `fork` copies a larger process structure and its `mmap` goes through more machinery; AerOS's smaller address-space bookkeeping is the cheaper design for a program this small).
- File operations are still 13 times slower, almost all of it the real-time scan on every open and close. Skipping the scan for a file that has not changed since it was last found clean needs a trustworthy modification stamp on every filesystem, which tmpfs does not have, so it is not done.
- AerOS's `clock_gettime` is still a real system call; Linux answers it from the vDSO.

## Caveats

One run each, no warm-up control, a virtualised clock, and a Linux guest with a different CPU count. The ratios are good to an order of magnitude, not to a percent. The benchmark file size is 4 KiB because AerOS's tmpfs files cannot be larger. The fork and mmap rows being faster than Linux's says more about what each system does per call than about quality. To repeat: `tools\test.ps1` (AerOS numbers are in `build\boot-test.log`) and `tools\bench-linux.ps1`.
