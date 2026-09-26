//! Virtio over the legacy (I/O-port) PCI transport plus split virtqueues,
//! polled: the shared plumbing for the virtio block and network drivers.

use core::hint::spin_loop;

use crate::arch;
use crate::memory::FrameAllocator;
use crate::pci::{PciDevice, PciInventory};

const PAGE_SIZE: u64 = 4096;
pub const VENDOR: u16 = 0x1af4;
const TIMEOUT: usize = 30_000_000;

const REG_DEVICE_FEATURES: u16 = 0x00;
const REG_GUEST_FEATURES: u16 = 0x04;
const REG_QUEUE_ADDRESS: u16 = 0x08;
const REG_QUEUE_SIZE: u16 = 0x0c;
const REG_QUEUE_SELECT: u16 = 0x0e;
const REG_QUEUE_NOTIFY: u16 = 0x10;
const REG_STATUS: u16 = 0x12;
/// Device-specific configuration starts here (no MSI-X).
pub const REG_CONFIG: u16 = 0x14;

const STATUS_ACKNOWLEDGE: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;

pub const DESC_NEXT: u16 = 1;
pub const DESC_WRITE: u16 = 2;

/// A virtio device on the legacy transport.
#[derive(Clone, Copy)]
pub struct Legacy {
    pub io: u16,
}

impl Legacy {
    pub fn find(pci: &PciInventory, device_id: u16) -> Option<(PciDevice, Self)> {
        let device = pci
            .devices()
            .iter()
            .copied()
            .find(|device| device.vendor == VENDOR && device.device == device_id)?;
        if device.bars[0] & 1 == 0 || !pci.enable_io_bus_master(device) {
            return None;
        }
        Some((
            device,
            Self {
                io: (device.bars[0] & 0xfffc) as u16,
            },
        ))
    }

    pub fn read8(&self, offset: u16) -> u8 {
        unsafe { arch::inb(self.io + offset) }
    }

    pub fn read32(&self, offset: u16) -> u32 {
        unsafe { arch::inl(self.io + offset) }
    }

    pub fn read16(&self, offset: u16) -> u16 {
        let value: u16;
        // SAFETY: a 16-bit port read of a virtio register (the device rejects
        // byte reads of its 16-bit registers).
        unsafe {
            core::arch::asm!(
                "in ax, dx",
                in("dx") self.io + offset,
                out("ax") value,
                options(nomem, nostack, preserves_flags)
            );
        }
        value
    }

    fn write8(&self, offset: u16, value: u8) {
        unsafe { arch::outb(self.io + offset, value) }
    }

    fn write16(&self, offset: u16, value: u16) {
        unsafe { arch::outw(self.io + offset, value) }
    }

    fn write32(&self, offset: u16, value: u32) {
        unsafe { arch::outl(self.io + offset, value) }
    }

    /// Resets the device and negotiates `wanted` features (returns what the
    /// device actually accepted). Queues are set up afterwards, then
    /// `driver_ok` finishes initialisation.
    pub fn begin(&self, wanted: u32) -> u32 {
        self.write8(REG_STATUS, 0);
        self.write8(REG_STATUS, STATUS_ACKNOWLEDGE);
        self.write8(REG_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
        let accepted = self.read32(REG_DEVICE_FEATURES) & wanted;
        self.write32(REG_GUEST_FEATURES, accepted);
        accepted
    }

    pub fn driver_ok(&self) {
        self.write8(
            REG_STATUS,
            STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_DRIVER_OK,
        );
    }

    pub fn notify(&self, queue: u16) {
        self.write16(REG_QUEUE_NOTIFY, queue);
    }

    /// Sets up virtqueue `index` and returns it (None if the device has no
    /// such queue or memory runs out).
    pub fn queue(&self, index: u16, frames: &mut FrameAllocator) -> Option<Queue> {
        self.write16(REG_QUEUE_SELECT, index);
        let size = self.read16(REG_QUEUE_SIZE) as usize;
        if size == 0 || !size.is_power_of_two() || size > 1024 {
            return None;
        }
        let descriptors = 16 * size;
        let available = 6 + 2 * size;
        let used_offset = (descriptors + available).next_multiple_of(PAGE_SIZE as usize);
        let total = used_offset + 6 + 8 * size;
        let pages = (total as u64).div_ceil(PAGE_SIZE);
        let block = frames.allocate_contiguous(pages, 1)?;
        let base = block.address();
        if base + pages * PAGE_SIZE > 0x0fff_ffff_ffff {
            return None;
        }
        unsafe {
            core::ptr::write_bytes(base as usize as *mut u8, 0, (pages * PAGE_SIZE) as usize)
        };
        self.write32(REG_QUEUE_ADDRESS, (base >> 12) as u32);
        Some(Queue {
            size,
            descriptors: base,
            available: base + descriptors as u64,
            used: base + used_offset as u64,
            next_available: 0,
            last_used: 0,
        })
    }
}

/// A split virtqueue.
#[derive(Clone, Copy)]
pub struct Queue {
    pub size: usize,
    descriptors: u64,
    available: u64,
    used: u64,
    next_available: u16,
    last_used: u16,
}

impl Queue {
    pub const EMPTY: Self = Self {
        size: 0,
        descriptors: 0,
        available: 0,
        used: 0,
        next_available: 0,
        last_used: 0,
    };

    /// Fills descriptor `index`.
    pub fn descriptor(&self, index: usize, address: u64, length: u32, flags: u16, next: u16) {
        let entry = (self.descriptors as usize + index * 16) as *mut u8;
        unsafe {
            core::ptr::write_volatile(entry as *mut u64, address);
            core::ptr::write_volatile(entry.add(8) as *mut u32, length);
            core::ptr::write_volatile(entry.add(12) as *mut u16, flags);
            core::ptr::write_volatile(entry.add(14) as *mut u16, next);
        }
    }

    /// Makes the chain starting at descriptor `head` available to the device.
    pub fn submit(&mut self, head: u16) {
        let slot = self.next_available as usize % self.size;
        unsafe {
            core::ptr::write_volatile((self.available as usize + 4 + slot * 2) as *mut u16, head);
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.next_available = self.next_available.wrapping_add(1);
        unsafe {
            core::ptr::write_volatile(
                (self.available as usize + 2) as *mut u16,
                self.next_available,
            );
        }
    }

    /// Pops one finished chain: (descriptor head, bytes written by the device).
    pub fn pop_used(&mut self) -> Option<(u16, u32)> {
        let index = unsafe { core::ptr::read_volatile((self.used as usize + 2) as *const u16) };
        if index == self.last_used {
            return None;
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        let slot = self.last_used as usize % self.size;
        let entry = (self.used as usize + 4 + slot * 8) as *const u32;
        let (id, length) = unsafe {
            (
                core::ptr::read_volatile(entry),
                core::ptr::read_volatile(entry.add(1)),
            )
        };
        self.last_used = self.last_used.wrapping_add(1);
        Some((id as u16, length))
    }

    /// Polls for a finished chain.
    pub fn wait_used(&mut self) -> Option<(u16, u32)> {
        for _ in 0..TIMEOUT {
            if let Some(done) = self.pop_used() {
                return Some(done);
            }
            spin_loop();
        }
        None
    }
}
