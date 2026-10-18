# Hardware support

What the kernel drives, how each driver finds its device, and how much of it has been exercised. "Verified" means the boot test (`tools\test.ps1`, QEMU q35) does real I/O through the driver and checks the result. Nothing here has been run on physical hardware yet; the right-hand column is the honest limit of that.

## Platform

| Area | Support | Verified |
| --- | --- | --- |
| Firmware | 64-bit UEFI, GOP framebuffer | QEMU + OVMF |
| CPU | x86-64 with SYSCALL, NX, SSE/FXSAVE; SMEP/SMAP/x2APIC/XSAVE used when present | QEMU (WHPX, TCG) on a Ryzen host |
| SMP | application processors started by INIT-SIPI | 2 and 4 CPUs |
| Interrupts | local APIC (xAPIC/x2APIC), IOAPIC, legacy PIC masked | yes |
| Timers | TSC (invariant, calibrated), HPET, local APIC timer, PIT | yes |
| ACPI | tables, AML interpreter, GPE/SCI, power button, battery, lid, thermal, EC, S5 and S3 | QEMU only |
| Virtualisation | AMD-V (verified) and Intel VT-x (written, never run on Intel) | AMD only |
| IOMMU | AMD-Vi | QEMU `amd-iommu` |

## Storage

| Device | Found by | Verified |
| --- | --- | --- |
| SATA, AHCI | PCI class 01:06:01 | read, write, power-loss tests on RAM disks |
| NVMe | PCI class 01:08:02 | read, write |
| virtio-blk | 1af4:1001 (legacy transport) | read, write |
| USB mass storage | xHCI, bulk-only transport, SCSI | read, write |
| SD host controller | PCI class 08:05 (SDHCI) | read, write |
| Software volumes | linear, stripe, mirror over any of the above (`raid`) | memory members and the block layer |

Filesystems: FAT12/16/32 (boot volume and `/media`), AerFS (copy-on-write, checksummed), tmpfs, union root over the boot image. Partition tables: MBR and GPT.

## Network

| Device | Found by | Verified |
| --- | --- | --- |
| Intel e1000 / 82540EM, 82545EM, 82574L (e1000e) | 8086:100e, 8086:100f, 8086:10d3 | ARP, TCP, UDP, TLS through slirp |
| Realtek RTL8139 | 10ec:8139 | ARP exchange |
| Realtek RTL8168/8111 family | 10ec:8168, 8161, 8169, 8167 | ARP exchange |
| virtio-net | 1af4:1000 (legacy) | ARP exchange |

Stack: IPv4, IPv6 with SLAAC, TCP (retransmit, congestion control, SACK), UDP, DHCP, DNS, NTP, TLS 1.3 client. Not present: Wi-Fi (WPA2 handshake and 802.11 framing are written and host-tested, there is no radio driver), Bluetooth (HCI codec host-tested, no controller driver).

## Input

| Device | Verified |
| --- | --- |
| PS/2 keyboard and mouse (i8042), VMware absolute mouse | yes |
| USB HID keyboard, mouse, tablet (xHCI, hubs included) | yes |
| virtio-input multitouch / tablet | yes |
| I2C-HID touchpads, precision touchpads, touchscreens | not supported (the usual touchpad of a recent laptop) |

## Display and audio

| Device | Verified |
| --- | --- |
| UEFI GOP framebuffer (any adapter the firmware drives) | yes |
| virtio-gpu 2D scanout, hardware cursor, virgl 3D glass | QEMU only; windowed GL crashes this QEMU build, headless works |
| Intel HDA (class 04:03) | stream position advances |
| AC'97 (class 04:01) | buffers consumed |
| Native GPU drivers (Intel, AMD, NVIDIA) | none: no mode setting, the firmware framebuffer is used |

## Security hardware

TPM 2.0: the TIS/CRB protocol and the measured-boot logic are host-tested (`tools/proto-host`), and there is a driver (`tpm_device.rs`), but QEMU on Windows has no TPM to attach, so the driver has not talked to one. Secure Boot: the kernel image is not signed for the UEFI db; updates and packages are signed (Ed25519).

## Sleep and power

S3 suspend with every driver restarted on wake (`docs/SLEEP.md`); S5 shutdown; CPU frequency listing and switching through ACPI `_PSS`/`_PCT`; microcode loading. S0ix, hibernation and wake sources other than the power button are not done.

## What has not been tried

Real machines: every row above is QEMU. The first things likely to differ on hardware are the Realtek and Intel NIC variants beyond those listed, xHCI quirks (port power, split transactions behind hubs), AHCI link bring-up timing, and ACPI tables written for other operating systems. `lspci` and `dmesg` give the information needed to extend a driver's ID list.
