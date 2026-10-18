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

The same measurement on the release build (no test suite, QEMU with a boot disk, a data disk, virtio-gpu, a USB tablet and virtio-rng, 2 CPUs). The first table is from 3 October; the second is from 14 October, after the full-screen buffers were allocated at boot for the real screen instead of being sized for 1080p.

| Memory | 3 Oct | 14 Oct |
|---|---|---|
| 96 and 112 MiB | firmware cannot load the image ("Out of Resources") | firmware cannot load the image |
| 128 MiB | cannot load | boots to the lock screen (`AEROS_READY`) |
| 144, 160 MiB | cannot load | boots to the lock screen |
| 176, 192 MiB | boots to the lock screen | boots to the lock screen |

So the practical floor for the desktop alone is now about 128 MiB at 1280x800. Anything on top of that (the browser, the Linux guest and its disk, large files in `/tmp`) needs more; the guest in particular wants its own RAM on top of the host's. A bigger screen needs more: the four screen buffers are width x height x 4 bytes each (16 MiB at 1280x800, 33 MiB at 1920x1080, 133 MiB at 3840x2160); when they cannot be allocated the desktop draws straight into the framebuffer, which works but is slow.

## Why the memory floor is high

- The kernel image is about 24 MiB on disk and about 93 MiB once loaded (about 70 MiB of zero-initialised statics), so the firmware needs more than that free before the kernel runs at all.
- The full-screen buffers used to be about 33 MiB of `.bss` sized for a 1920x1080 screen whatever the real resolution. They are now allocated at boot for the real screen (`desktop::allocate_screen_buffers`: the composed desktop, its shadow copy, the wallpaper and the panel cache, four buffers of width x height x 4 bytes each, so 16 MiB at 1280x800). A screen larger than 1080p now gets the same off-screen drawing instead of drawing straight into the framebuffer. What stays static is the frost-blur scratch (6 MiB), the window, dock and other region caches, the 4 MiB home file buffer and the 4 MiB boot stack.
- The kernel heap is 16 MiB.
- Decoding the 4148x2228 test JPEG (the boot-test checks the picture decoders on it) is the largest single allocation at run time; the desktop wallpaper itself is pre-converted (1237x751) and is not decoded.

## Practical minimums

- To reach the desktop on a release build: about 128 MiB at 1280x800 (the table above).
- To start the boot-test build's kernel: 192 MiB.
- To run everything the boot-test exercises, including the full-size wallpaper decode: 384 MiB. 512 MiB is recommended.
- CPU: one core is enough for the desktop; two or more are needed for the SMP checks.

Not measured: real hardware and the Linux guest (its root filesystem and kernel add to the memory needed). The release-build row was measured with only the boot disk and a display adapter attached, so treat it as a lower bound for the bare desktop, not a recommendation.
