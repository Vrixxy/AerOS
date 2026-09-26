//! Virtio block device (legacy transport, one request at a time, polled).

use crate::memory::FrameAllocator;
use crate::pci::PciInventory;
use crate::sync::TicketLock;
use crate::virtio::{DESC_NEXT, DESC_WRITE, Legacy, Queue, REG_CONFIG};

const DEVICE_ID: u16 = 0x1001;
const SECTOR: usize = 512;
const MAX_SECTORS_PER_REQUEST: usize = 8;

#[derive(Clone, Copy)]
pub struct VirtioBlkReport {
    pub present: bool,
    pub io: u16,
    pub sectors: u64,
    pub read: bool,
    pub write_probe: bool,
    pub verified: bool,
}

impl VirtioBlkReport {
    pub const EMPTY: Self = Self {
        present: false,
        io: 0,
        sectors: 0,
        read: false,
        write_probe: false,
        verified: false,
    };
}

struct BlkState {
    device: Option<Legacy>,
    queue: Queue,
    dma: u64,
    sectors: u64,
}

static BLK: TicketLock<BlkState> = TicketLock::new(BlkState {
    device: None,
    queue: Queue::EMPTY,
    dma: 0,
    sectors: 0,
});

impl BlkState {
    fn transfer(&mut self, write: bool, lba: u64, count: usize, buffer: &mut [u8]) -> bool {
        let Some(device) = self.device else {
            return false;
        };
        let bytes = count * SECTOR;
        if count == 0
            || count > MAX_SECTORS_PER_REQUEST
            || buffer.len() < bytes
            || lba
                .checked_add(count as u64)
                .is_none_or(|end| end > self.sectors)
        {
            return false;
        }
        let header = self.dma;
        let status = self.dma + 16;
        let data = self.dma + 512;
        unsafe {
            core::ptr::write_volatile(header as usize as *mut u32, write as u32);
            core::ptr::write_volatile((header + 4) as usize as *mut u32, 0);
            core::ptr::write_volatile((header + 8) as usize as *mut u64, lba);
            core::ptr::write_volatile(status as usize as *mut u8, 0xff);
            if write {
                core::ptr::copy_nonoverlapping(buffer.as_ptr(), data as usize as *mut u8, bytes);
            }
        }
        self.queue.descriptor(0, header, 16, DESC_NEXT, 1);
        self.queue.descriptor(
            1,
            data,
            bytes as u32,
            DESC_NEXT | if write { 0 } else { DESC_WRITE },
            2,
        );
        self.queue.descriptor(2, status, 1, DESC_WRITE, 0);
        self.queue.submit(0);
        device.notify(0);
        if self.queue.wait_used().is_none() {
            return false;
        }
        if unsafe { core::ptr::read_volatile(status as usize as *const u8) } != 0 {
            return false;
        }
        if !write {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    data as usize as *const u8,
                    buffer.as_mut_ptr(),
                    bytes,
                )
            };
        }
        true
    }
}

/// Reads `count` (at most 8) 512-byte sectors starting at `lba`.
pub fn read(lba: u64, count: usize, buffer: &mut [u8]) -> bool {
    BLK.lock().transfer(false, lba, count, buffer)
}

pub fn write(lba: u64, count: usize, buffer: &[u8]) -> bool {
    let mut copy = [0u8; SECTOR * MAX_SECTORS_PER_REQUEST];
    let bytes = count * SECTOR;
    if bytes > copy.len() || buffer.len() < bytes {
        return false;
    }
    copy[..bytes].copy_from_slice(&buffer[..bytes]);
    BLK.lock().transfer(true, lba, count, &mut copy)
}

pub fn sectors() -> u64 {
    BLK.lock().sectors
}

pub fn initialize(pci: &PciInventory, frames: &mut FrameAllocator) -> VirtioBlkReport {
    let Some((_, device)) = Legacy::find(pci, DEVICE_ID) else {
        return VirtioBlkReport::EMPTY;
    };
    let mut report = VirtioBlkReport {
        present: true,
        io: device.io,
        ..VirtioBlkReport::EMPTY
    };
    device.begin(0);
    let Some(queue) = device.queue(0, frames) else {
        return report;
    };
    let Some(dma) = frames.allocate_contiguous(2, 1) else {
        return report;
    };
    device.driver_ok();
    report.sectors =
        device.read32(REG_CONFIG) as u64 | (device.read32(REG_CONFIG + 4) as u64) << 32;
    *BLK.lock() = BlkState {
        device: Some(device),
        queue,
        dma: dma.address(),
        sectors: report.sectors,
    };
    let mut sector = [0u8; SECTOR];
    report.read = read(0, 1, &mut sector) && &sector[..16] == b"AEROS-VIRTIO-BLK";
    let mut pattern = [0u8; SECTOR];
    for (index, byte) in pattern.iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(13).wrapping_add(5);
    }
    let mut readback = [0u8; SECTOR];
    report.write_probe = report.sectors > 2
        && write(2, 1, &pattern)
        && read(2, 1, &mut readback)
        && readback == pattern;
    report.verified = report.sectors > 0 && report.read && report.write_probe;
    report
}
