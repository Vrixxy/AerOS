//! Modern (virtio 1.0+) virtio-pci transport: PCI-capability-addressed MMIO
//! config regions instead of the legacy transport's single I/O-port block
//! (`virtio.rs`). Newer virtio device types (input among them) only ever
//! implement this transport - `virtio-multitouch-pci` in QEMU, for one, has
//! no I/O-space BAR at all, so `virtio::Legacy::find` can never see it.
//!
//! The split-virtqueue ring format itself (descriptor table / available
//! ring / used ring) is identical between transports, so this reuses
//! `virtio::Queue` unchanged - only how the driver tells the device where
//! the rings are, and how it rings the doorbell, differs.

use crate::memory::FrameAllocator;
use crate::pci::{self, PciDevice, PciInventory, VendorCapability};
use crate::virtio::{Queue, VENDOR};

const PAGE_SIZE: u64 = 4096;

const CFG_COMMON: u8 = 1;
const CFG_NOTIFY: u8 = 2;
const CFG_ISR: u8 = 3;
const CFG_DEVICE: u8 = 4;

const STATUS_ACKNOWLEDGE: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;
const STATUS_FEATURES_OK: u8 = 8;

/// Bit 32 of the (64-bit) feature space - every modern device requires the
/// driver to accept this specific bit before `FEATURES_OK` is allowed to
/// stick, since it is what marks the driver as speaking virtio 1.0+ rather
/// than assuming legacy behaviour.
const FEATURE_VERSION_1: u32 = 1 << 0; // bit 32 overall, bit 0 of feature-select-1's window

/// A virtio device on the modern (capability-addressed MMIO) transport.
#[derive(Clone, Copy)]
pub struct Modern {
    common: u64,
    notify: u64,
    notify_multiplier: u32,
    /// Present only if the device advertised a DEVICE_CFG capability - not
    /// every device type needs one, but virtio-input's `virtio_input_config`
    /// lives here.
    device_config: u64,
}

impl Modern {
    /// Locates a modern-transport virtio device of `device_id` and resolves
    /// its four standard config capabilities. `None` if the device is not
    /// present, is legacy-only (no COMMON_CFG capability), or a capability
    /// points at an I/O-space BAR (spec-disallowed - `bar_address` already
    /// refuses to resolve one).
    pub fn find(pci: &PciInventory, device_id: u16) -> Option<(PciDevice, Self)> {
        let device = pci
            .devices()
            .iter()
            .copied()
            .find(|device| device.vendor == VENDOR && device.device == device_id)?;
        if !pci.enable_memory_bus_master(device) {
            return None;
        }
        let mut common = None;
        let mut notify = None;
        let mut notify_multiplier = 0u32;
        let mut isr = None;
        let mut device_config = None;
        pci::walk_vendor_capabilities(&device, |capability: VendorCapability| {
            let Some(address) = pci::bar_address(&device, capability.bar, capability.offset) else {
                return;
            };
            // `length` (the capability's own claimed size of its BAR
            // region) guards against trusting a truncated COMMON_CFG that
            // does not actually reach as far as `queue_device` (offset
            // 0x38): reading/writing past it would just hit whatever else
            // lives in that BAR, silently, rather than a real config
            // register.
            match capability.cfg_type {
                CFG_COMMON if capability.length >= 0x38 => common = Some(address),
                CFG_NOTIFY => {
                    notify = Some(address);
                    notify_multiplier = capability.notify_multiplier;
                }
                CFG_ISR => isr = Some(address),
                CFG_DEVICE => device_config = Some(address),
                _ => {}
            }
        });
        // ISR_CFG is mandatory for every modern virtio device per the spec,
        // even though a polling-only driver like this one never reads it -
        // its absence means whatever answered as vendor:device is not
        // really a compliant modern virtio device, so treat that the same
        // as any other required capability missing.
        isr?;
        Some((
            device,
            Self {
                common: common?,
                notify: notify?,
                notify_multiplier,
                device_config: device_config.unwrap_or(0),
            },
        ))
    }

    fn read8(&self, offset: u64) -> u8 {
        unsafe { core::ptr::read_volatile((self.common + offset) as *const u8) }
    }

    fn write8(&self, offset: u64, value: u8) {
        unsafe { core::ptr::write_volatile((self.common + offset) as *mut u8, value) }
    }

    fn read16(&self, offset: u64) -> u16 {
        unsafe { core::ptr::read_volatile((self.common + offset) as *const u16) }
    }

    fn write16(&self, offset: u64, value: u16) {
        unsafe { core::ptr::write_volatile((self.common + offset) as *mut u16, value) }
    }

    fn read32(&self, offset: u64) -> u32 {
        unsafe { core::ptr::read_volatile((self.common + offset) as *const u32) }
    }

    fn write32(&self, offset: u64, value: u32) {
        unsafe { core::ptr::write_volatile((self.common + offset) as *mut u32, value) }
    }

    fn write64(&self, offset: u64, value: u64) {
        unsafe { core::ptr::write_volatile((self.common + offset) as *mut u64, value) }
    }

    pub fn device_status(&self) -> u8 {
        self.read8(0x14)
    }

    /// One byte of the device's own config space (`virtio_input_config` for
    /// an input device) - `DEVICE_CFG` is optional per the spec, so callers
    /// check `has_device_config` first.
    pub fn has_device_config(&self) -> bool {
        self.device_config != 0
    }

    pub fn device_config_read8(&self, offset: u64) -> u8 {
        unsafe { core::ptr::read_volatile((self.device_config + offset) as *const u8) }
    }

    pub fn device_config_write8(&self, offset: u64, value: u8) {
        unsafe { core::ptr::write_volatile((self.device_config + offset) as *mut u8, value) }
    }

    /// A little-endian 32-bit field of the device's own config space - used
    /// to read `virtio_input_absinfo`'s `min`/`max` (an `i32` pair) once
    /// `select`/`subsel` have been written to choose which axis it
    /// describes.
    pub fn device_config_read32(&self, offset: u64) -> i32 {
        unsafe { core::ptr::read_volatile((self.device_config + offset) as *const i32) }
    }

    /// Resets the device, negotiates `wanted` (the low 32 bits of the
    /// feature space - virtio-input needs none of its own beyond
    /// `VERSION_1`, which this always adds), and sets `FEATURES_OK`,
    /// confirming the device kept that bit set (its own way of saying the
    /// negotiation was acceptable). Queues are set up by the caller
    /// afterward, then `driver_ok` finishes initialisation - same shape as
    /// `virtio::Legacy::begin`/`driver_ok`.
    pub fn begin(&self, wanted: u32) -> bool {
        self.write8(0x14, 0);
        while self.read8(0x14) != 0 {
            core::hint::spin_loop();
        }
        self.write8(0x14, STATUS_ACKNOWLEDGE);
        self.write8(0x14, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
        self.write32(0x00, 0);
        let low_available = self.read32(0x04);
        self.write32(0x08, 0);
        self.write32(0x0c, low_available & wanted);
        self.write32(0x00, 1);
        let high_available = self.read32(0x04);
        self.write32(0x08, 1);
        self.write32(0x0c, high_available & FEATURE_VERSION_1);
        if high_available & FEATURE_VERSION_1 == 0 {
            return false;
        }
        self.write8(
            0x14,
            STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK,
        );
        self.device_status() & STATUS_FEATURES_OK != 0
    }

    pub fn driver_ok(&self) {
        self.write8(
            0x14,
            STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK,
        );
    }

    /// Sets up virtqueue `index`: same physical ring layout `virtio::Legacy`
    /// uses (so the descriptor-table/available/used code in `virtio::Queue`
    /// applies unchanged), but told to the device as three separate 64-bit
    /// MMIO registers instead of one combined page-aligned base address,
    /// and enabled explicitly afterward. Returns the queue plus the
    /// `queue_notify_off` `notify` needs to ring the right doorbell.
    pub fn queue(&self, index: u16, frames: &mut FrameAllocator) -> Option<(Queue, u16)> {
        self.write16(0x16, index);
        let size = self.read16(0x18) as usize;
        if size == 0 || !size.is_power_of_two() || size > 1024 {
            return None;
        }
        let notify_off = self.read16(0x1e);
        let descriptors = 16 * size;
        let available = 6 + 2 * size;
        let used_offset = (descriptors + available).next_multiple_of(PAGE_SIZE as usize);
        let total = used_offset + 6 + 8 * size;
        let pages = (total as u64).div_ceil(PAGE_SIZE);
        let block = frames.allocate_dma(pages, 1)?;
        let base = block.address();
        unsafe {
            core::ptr::write_bytes(base as usize as *mut u8, 0, (pages * PAGE_SIZE) as usize)
        };
        let used = base + used_offset as u64;
        self.write64(0x20, base);
        self.write64(0x28, base + descriptors as u64);
        self.write64(0x30, used);
        self.write16(0x1c, 1);
        Some((
            Queue {
                size,
                descriptors: base,
                available: base + descriptors as u64,
                used,
                next_available: 0,
                last_used: 0,
            },
            notify_off,
        ))
    }

    /// Rings the doorbell for a queue, given the `queue_notify_off` its
    /// `queue()` call returned.
    pub fn notify(&self, queue_notify_off: u16) {
        let address = self.notify + queue_notify_off as u64 * self.notify_multiplier as u64;
        unsafe { core::ptr::write_volatile(address as *mut u16, 0) };
    }
}
