//! The boot disk, whichever controller it sits behind: SATA (AHCI) first,
//! then NVMe, then virtio-blk, then a USB mass-storage disk, then an SD card. The FAT layer only talks to this module.

use crate::{ahci, nvme, sdhci, virtio_blk, xhci};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Backend {
    Ahci,
    Nvme,
    Virtio,
    Usb,
    Sd,
    None,
}

fn backend() -> Backend {
    if ahci::disk_count() > 0 {
        Backend::Ahci
    } else if nvme::sectors() > 0 && nvme::sector_bytes() == 512 {
        Backend::Nvme
    } else if virtio_blk::sectors() > 0 {
        Backend::Virtio
    } else if xhci::storage_sectors() > 0 {
        Backend::Usb
    } else if sdhci::sectors() > 0 {
        Backend::Sd
    } else {
        Backend::None
    }
}

/// Size of the boot disk in 512-byte sectors (0 = no disk).
pub fn boot_sectors() -> u64 {
    match backend() {
        Backend::Ahci => ahci::disk_sectors(ahci::boot_disk()),
        Backend::Nvme => nvme::sectors(),
        Backend::Virtio => virtio_blk::sectors(),
        Backend::Usb => xhci::storage_sectors(),
        Backend::Sd => sdhci::sectors(),
        Backend::None => 0,
    }
}

pub fn read_sector(lba: u64, destination: &mut [u8; 512]) -> bool {
    match backend() {
        Backend::Ahci => ahci::read_sector(lba, destination),
        Backend::Nvme => nvme::read(lba, 1, destination),
        Backend::Virtio => virtio_blk::read(lba, 1, destination),
        Backend::Usb => xhci::storage_read(lba, 1, destination),
        Backend::Sd => sdhci::read(lba, 1, destination),
        Backend::None => false,
    }
}

pub fn write_boot_sector(lba: u64, source: &[u8; 512]) -> bool {
    match backend() {
        Backend::Ahci => ahci::write_disk_sector(ahci::boot_disk(), lba, source),
        Backend::Nvme => nvme::write(lba, 1, source),
        Backend::Virtio => virtio_blk::write(lba, 1, source),
        Backend::Usb => xhci::storage_write(lba, 1, source),
        Backend::Sd => sdhci::write(lba, 1, source),
        Backend::None => false,
    }
}

/// Reads `sectors` sectors starting at `lba` into physical memory at
/// `destination` (the kernel identity-maps RAM).
pub fn read_into(lba: u64, sectors: u32, destination: u64) -> bool {
    let backend = backend();
    if backend == Backend::Ahci {
        return ahci::read_into(lba, sectors, destination);
    }
    let mut chunk = [0u8; 512 * 8];
    let mut done = 0u32;
    while done < sectors {
        let count = (sectors - done).min(8) as usize;
        let ok = match backend {
            Backend::Nvme => nvme::read(lba + done as u64, count as u32, &mut chunk),
            Backend::Virtio => virtio_blk::read(lba + done as u64, count, &mut chunk),
            Backend::Usb => xhci::storage_read(lba + done as u64, count, &mut chunk),
            Backend::Sd => sdhci::read(lba + done as u64, count, &mut chunk),
            _ => false,
        };
        if !ok {
            return false;
        }
        unsafe {
            core::ptr::copy_nonoverlapping(
                chunk.as_ptr(),
                (destination as usize + done as usize * 512) as *mut u8,
                count * 512,
            );
        }
        done += count as u32;
    }
    true
}
