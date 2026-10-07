# System requirements (measured)

Measured with `tools\test.ps1 -MemoryMiB <n> -CpuCount <n>` on QEMU q35 with the `boot-test` kernel (3 October 2026). These are the limits of that build and test suite, not of a stripped-down release build.

| Resource | Result |
|---|---|
| 64, 96, 128 MiB | Firmware cannot load the kernel image ("Out of Resources"). |
| 144, 160, 176 MiB | Same. |
| 192 MiB | Kernel loads and starts. |
| 192 and 256 MiB | Boots, but the boot-test stops at the 4148x2228 JPEG decode (`OutOfMemory`). |
| 320 MiB | Same JPEG failure. |
| 384 MiB | Full boot-test passes. |
| 512 MiB | Full boot-test passes (default); about 270 MiB in use at the end. |
| 1 CPU | Boots to the desktop (`AEROS_READY`); only the multiprocessor-startup check fails, since it expects application processors. |
| 2-4 CPUs | Full boot-test passes. |

## The normal release build

The same measurement on the release build (no test suite, QEMU with only the boot disk and a display adapter, 2 CPUs):

| Memory | Result |
|---|---|
| 128 and 160 MiB | Firmware cannot load the image ("Out of Resources"). |
| 176 MiB | Boots to the lock screen (`AEROS_READY`, desktop redraws). |
| 192, 256, 320 MiB | Boots to the lock screen. |

So the practical floor for the desktop alone is about 176 MiB. Anything on top of that (the browser, the Linux guest and its disk, large files in `/tmp`) needs more; the guest in particular wants its own RAM on top of the host's.

## Why the memory floor is high

- The kernel image is about 24 MiB on disk and about 93 MiB once loaded (about 70 MiB of zero-initialised statics), so the firmware needs more than that free before the kernel runs at all.
- About 61 MiB of that is `.bss` sized for a 1920x1080 screen whatever the real resolution: the desktop, shadow, wallpaper, panel, window and setup caches (about 7.9 MiB each for the first four, 6.9 and 4.6 MiB for the others), the frost-blur scratch (6 MiB), the 4 MiB home file buffer and the 4 MiB boot stack. Allocating the screen buffers from the real mode at boot would lower the floor; it has not been done.
- The kernel heap is 16 MiB.
- Decoding the 4148x2228 test JPEG (the boot-test checks the picture decoders on it) is the largest single allocation at run time; the desktop wallpaper itself is pre-converted (1237x751) and is not decoded.

## Practical minimums

- To reach the desktop on a release build: about 176 MiB (the table above).
- To start the boot-test build's kernel: 192 MiB.
- To run everything the boot-test exercises, including the full-size wallpaper decode: 384 MiB. 512 MiB is recommended.
- CPU: one core is enough for the desktop; two or more are needed for the SMP checks.

Not measured: real hardware and the Linux guest (its root filesystem and kernel add to the memory needed). The release-build row was measured with only the boot disk and a display adapter attached, so treat it as a lower bound for the bare desktop, not a recommendation.
