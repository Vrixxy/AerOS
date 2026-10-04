# AerOS against Linux

One static program (`userspace/bench`, built for musl) runs unchanged on both systems and times a few operations with the monotonic clock. On AerOS it runs as a process during the boot-test (`AEROS_BENCH` lines). On Linux it is `/init` of a tiny initramfs on the Debian 6.12 kernel the project ships (`tools/bench-linux.ps1`, `tools/make-initramfs.py`). Both run in QEMU with the same CPU model and the same acceleration setting (`accel=whpx:tcg`) on the same host, Linux with one CPU, AerOS with the boot-test's two (its user tasks only run on the first).

Results from one run each on 2026-10-04 (nanoseconds per operation; lower is better):

| Operation | AerOS | Linux | AerOS / Linux |
| --- | ---: | ---: | ---: |
| xorshift step (CPU calibration) | 3 | 2 | ~2 |
| `getpid` system call | 37,066 | 150 | 247 |
| `clock_gettime` | 112,134 | 59 (no system call) | n/a |
| copy 64 KiB | 7,264 | 3,133 | 2.3 |
| `mmap` 8 pages, touch, `munmap` | 77,470 | 25,196 | 3.1 |
| create, write 4 KiB, read back, unlink (tmpfs) | 6,410,115 | 19,580 | 327 |
| `fork`, child exit, `wait4` | 1,435,805 | 84,788 | 17 |
| pipe round trip between two processes | 1,432,906 | 5,787 | 248 |

## What this says

- The CPU-bound and memory-bound numbers are within a factor of two or three, which is about what two different code generators and page-table layouts explain.
- Everything that enters the kernel is slow, by two orders of magnitude: a bare system call costs about 37 µs on AerOS against 150 ns on Linux. Nothing here has been profiled, so the cause is not known. Work done on every call (the syscall trace ring, polling the network stack while a socket exists, tracing and seccomp checks, locks, swapping the saved process state) is where to look first. The file and pipe numbers are mostly this overhead multiplied by several calls.
- AerOS's `clock_gettime` is a real system call; Linux answers it from the vDSO.

## Caveats

One run each, no warm-up control, a virtualised clock, and a Linux guest with a different CPU count. The ratios are good to an order of magnitude, not to a percent. The benchmark file size is 4 KiB because AerOS's tmpfs files cannot be larger. To repeat: `tools\test.ps1` (AerOS numbers are in `build\boot-test.log`) and `tools\bench-linux.ps1`.
