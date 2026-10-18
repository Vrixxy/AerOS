//! virtio-rng driver (modern transport, one request in flight, polled): hands
//! the host's entropy to the random pool, so a machine whose processor has no
//! RDRAND/RDSEED still starts from a real seed.

use crate::memory::FrameAllocator;
use crate::pci::PciInventory;
use crate::virtio::{DESC_WRITE, Queue};
use crate::virtio_modern::Modern;

const DEVICE_ID: u16 = 0x1044; // virtio device type 4 (entropy) + 0x1040
const REQUEST_QUEUE: u16 = 0;
const REQUEST_BYTES: u32 = 64;

#[derive(Clone, Copy)]
pub struct RngReport {
    pub present: bool,
    pub queue_ready: bool,
    pub bytes: u32,
    pub verified: bool,
}

impl RngReport {
    const ABSENT: Self = Self {
        present: false,
        queue_ready: false,
        bytes: 0,
        // No virtio-rng attached is not a failure.
        verified: true,
    };

    const fn failed(queue_ready: bool) -> Self {
        Self {
            present: true,
            queue_ready,
            bytes: 0,
            verified: false,
        }
    }
}

pub fn initialize(pci: &PciInventory, frames: &mut FrameAllocator) -> RngReport {
    let Some((_, modern)) = Modern::find(pci, DEVICE_ID) else {
        return RngReport::ABSENT;
    };
    if !modern.begin(0) {
        return RngReport::failed(false);
    }
    let Some((mut queue, notify_off)) = modern.queue(REQUEST_QUEUE, frames) else {
        return RngReport::failed(false);
    };
    let Some(page) = frames.allocate() else {
        return RngReport::failed(true);
    };
    let page = page.address();
    unsafe { core::ptr::write_bytes(page as usize as *mut u8, 0, 4096) };
    modern.driver_ok();
    let bytes = request(&modern, &mut queue, notify_off, page);
    let mut sample = [0u8; REQUEST_BYTES as usize];
    for (index, byte) in sample.iter_mut().enumerate() {
        *byte = unsafe { core::ptr::read_volatile((page as usize + index) as *const u8) };
    }
    unsafe { core::ptr::write_bytes(page as usize as *mut u8, 0, 4096) };
    let usable = &sample[..bytes as usize];
    let varied = usable.iter().any(|&byte| byte != usable[0]);
    if bytes >= 16 && varied {
        crate::random::add_entropy(usable);
    }
    sample.fill(0);
    RngReport {
        present: true,
        queue_ready: true,
        bytes,
        verified: bytes >= 16 && varied,
    }
}

/// Asks for `REQUEST_BYTES` of entropy; returns how many arrived.
fn request(modern: &Modern, queue: &mut Queue, notify_off: u16, page: u64) -> u32 {
    queue.descriptor(0, page, REQUEST_BYTES, DESC_WRITE, 0);
    queue.submit(0);
    modern.notify(notify_off);
    match queue.wait_used() {
        Some((_, length)) => length.min(REQUEST_BYTES),
        None => 0,
    }
}
