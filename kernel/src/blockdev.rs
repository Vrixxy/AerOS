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

/// The boot disk as the block layer names it.
pub fn boot_disk() -> Option<crate::block::Disk> {
    use crate::block::Disk;
    match backend() {
        Backend::Ahci => Some(Disk::Ahci(ahci::boot_disk())),
        Backend::Nvme => Some(Disk::Nvme),
        Backend::Virtio => Some(Disk::Virtio),
        Backend::Usb => Some(Disk::Usb),
        Backend::Sd => Some(Disk::Sd),
        Backend::None => None,
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

/// An 8 MiB in-memory disk that stands in for the boot disk while the FAT
/// power-loss test runs, so the test never touches the real ESP.
#[cfg(feature = "boot-test")]
pub const RAM_DISK_SECTORS: u64 = 16384;
#[cfg(feature = "boot-test")]
static RAM_DISK: crate::sync::TicketLock<[u8; RAM_DISK_SECTORS as usize * 512]> =
    crate::sync::TicketLock::new([0; RAM_DISK_SECTORS as usize * 512]);
#[cfg(feature = "boot-test")]
static RAM_WRITES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(feature = "boot-test")]
static RAM_ACTIVE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

#[cfg(feature = "boot-test")]
pub fn use_ram_disk(active: bool) {
    RAM_ACTIVE.store(active, core::sync::atomic::Ordering::SeqCst);
}

#[cfg(feature = "boot-test")]
fn ram_disk_active() -> bool {
    RAM_ACTIVE.load(core::sync::atomic::Ordering::SeqCst)
}

/// Reads whole sectors from the RAM disk (for the `fatfs` power-loss test).
#[cfg(feature = "boot-test")]
pub fn ram_read(lba: u64, buffer: &mut [u8]) -> bool {
    let sectors = (buffer.len() / 512) as u64;
    if lba + sectors > RAM_DISK_SECTORS {
        return false;
    }
    let start = lba as usize * 512;
    buffer.copy_from_slice(&RAM_DISK.lock()[start..start + buffer.len()]);
    true
}

/// Writes whole sectors to the RAM disk one at a time, so a simulated power
/// loss can cut a multi-sector write short.
#[cfg(feature = "boot-test")]
pub fn ram_write(lba: u64, data: &[u8]) -> bool {
    let sectors = data.len() / 512;
    if lba + sectors as u64 > RAM_DISK_SECTORS {
        return false;
    }
    for index in 0..sectors {
        if power_lost() {
            return false;
        }
        let start = (lba as usize + index) * 512;
        RAM_DISK.lock()[start..start + 512].copy_from_slice(&data[index * 512..(index + 1) * 512]);
        RAM_WRITES.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
    }
    true
}

/// Sector writes the RAM disk has taken so far.
#[cfg(feature = "boot-test")]
pub fn ram_writes() -> u64 {
    RAM_WRITES.load(core::sync::atomic::Ordering::SeqCst)
}

/// True once an injected power loss has used up its allowed writes.
#[cfg(feature = "boot-test")]
pub fn power_is_out() -> bool {
    WRITES_ALLOWED.load(core::sync::atomic::Ordering::SeqCst) == 0
}

pub fn read_sector(lba: u64, destination: &mut [u8; 512]) -> bool {
    #[cfg(feature = "boot-test")]
    if ram_disk_active() {
        if lba >= RAM_DISK_SECTORS {
            return false;
        }
        let start = lba as usize * 512;
        destination.copy_from_slice(&RAM_DISK.lock()[start..start + 512]);
        return true;
    }
    match backend() {
        Backend::Ahci => ahci::read_sector(lba, destination),
        Backend::Nvme => nvme::read(lba, 1, destination),
        Backend::Virtio => virtio_blk::read(lba, 1, destination),
        Backend::Usb => xhci::storage_read(lba, 1, destination),
        Backend::Sd => sdhci::read(lba, 1, destination),
        Backend::None => false,
    }
}

#[cfg(feature = "boot-test")]
static WRITES_ALLOWED: core::sync::atomic::AtomicI64 = core::sync::atomic::AtomicI64::new(-1);
#[cfg(feature = "boot-test")]
static WRITES_DROPPED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Simulates losing power: after `writes` more successful sector writes,
/// every later write is dropped (and reported as failed) until
/// [`clear_power_loss`].
#[cfg(feature = "boot-test")]
pub fn inject_power_loss_after(writes: u32) {
    WRITES_DROPPED.store(0, core::sync::atomic::Ordering::SeqCst);
    WRITES_ALLOWED.store(i64::from(writes), core::sync::atomic::Ordering::SeqCst);
}

/// Ends a simulated power loss and returns how many writes it dropped.
#[cfg(feature = "boot-test")]
pub fn clear_power_loss() -> u64 {
    WRITES_ALLOWED.store(-1, core::sync::atomic::Ordering::SeqCst);
    WRITES_DROPPED.swap(0, core::sync::atomic::Ordering::SeqCst)
}

#[cfg(feature = "boot-test")]
fn power_lost() -> bool {
    use core::sync::atomic::Ordering;
    match WRITES_ALLOWED.load(Ordering::SeqCst) {
        0 => {
            WRITES_DROPPED.fetch_add(1, Ordering::SeqCst);
            true
        }
        remaining if remaining > 0 => {
            WRITES_ALLOWED.store(remaining - 1, Ordering::SeqCst);
            false
        }
        _ => false,
    }
}

pub fn write_boot_sector(lba: u64, source: &[u8; 512]) -> bool {
    #[cfg(feature = "boot-test")]
    if power_lost() {
        return false;
    }
    #[cfg(feature = "boot-test")]
    if ram_disk_active() {
        if lba >= RAM_DISK_SECTORS {
            return false;
        }
        let start = lba as usize * 512;
        RAM_DISK.lock()[start..start + 512].copy_from_slice(source);
        return true;
    }
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
