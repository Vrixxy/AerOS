//! virtio-input driver: real touchscreen support, over the modern
//! (capability-addressed MMIO) virtio-pci transport in `virtio_modern.rs` -
//! the device type QEMU's `virtio-multitouch-pci` (and any real touch
//! digitizer a hypervisor passes through this way) implements has no
//! legacy-transport fallback at all.
//!
//! Only the primary contact is tracked (AerOS's UI has no multi-touch
//! gestures to feed), but both event styles real touch hardware uses are
//! handled: single-touch (`ABS_X`/`ABS_Y` + `BTN_TOUCH`) and multi-touch
//! protocol B (`ABS_MT_POSITION_X/Y` + `ABS_MT_TRACKING_ID`, slot 0 only).
//! Either way, a touch is delivered to the desktop exactly like a USB
//! tablet's stylus tap already is - `mouse::inject_absolute(buttons=1, ...)`
//! on contact-down, `buttons=0` on contact-up - so `desktop.rs`'s existing
//! jump-heuristic `touch_mode` detection and every app already built
//! against it need no changes at all to pick up real touch hardware.

use crate::memory::FrameAllocator;
use crate::pci::PciInventory;
use crate::sync::TicketLock;
use crate::virtio::{DESC_WRITE, Queue};
use crate::virtio_modern::Modern;

const DEVICE_ID: u16 = 0x1052; // virtio device type 18 (input) + 0x1040
const EVENT_QUEUE: u16 = 0;
/// Receive buffers kept posted at once - each holds exactly one 8-byte
/// `virtio_input_event`, so 16 is generous slack against a burst of events
/// (a full multi-touch-protocol-B frame is only a handful) arriving between
/// two `poll` calls.
const EVENT_SLOTS: usize = 16;
const EVENT_BYTES: u64 = 8;

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_ABS: u16 = 0x03;
const BTN_TOUCH: u16 = 0x14a;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_MT_SLOT: u16 = 0x2f;
const ABS_MT_TRACKING_ID: u16 = 0x39;
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;

const CFG_ABS_INFO: u8 = 0x12;

#[derive(Clone, Copy)]
struct AxisRange {
    min: i32,
    max: i32,
}

impl AxisRange {
    const EMPTY: Self = Self { min: 0, max: 0 };

    fn span(self) -> i32 {
        self.max - self.min
    }

    /// Scales a raw sample into the 0..=0xffff range `mouse::inject_absolute`
    /// expects, matching the XHCI absolute-pointer driver's own `scale`.
    fn scale(self, value: i32) -> u32 {
        let span = (self.span() as i64).max(1);
        ((value as i64 - self.min as i64).clamp(0, span) * 0xffff / span) as u32
    }
}

struct Touch {
    modern: Modern,
    queue: Queue,
    notify_off: u16,
    buffers: u64,
    x_range: AxisRange,
    y_range: AxisRange,
    /// Pending single-touch/multi-touch-slot-0 sample, applied to the mouse
    /// pipeline on `SYN_REPORT` (a real device batches several `EV_ABS`
    /// updates before that sync, and half-applied state read mid-frame
    /// would show the pointer stuttering between old and new axes).
    active: bool,
    x: i32,
    y: i32,
    /// Non-zero once at least one real `ABS_X`/`ABS_Y` or
    /// `ABS_MT_POSITION_X`/`Y` pair has arrived - guards against reporting
    /// a phantom touch at (min, min) from a `SYN_REPORT` that carried no
    /// position update yet (the very first frame after `DRIVER_OK`, or a
    /// pure `BTN_TOUCH`-only frame on a single-touch device).
    have_position: bool,
    /// Which multi-touch slot is currently selected - only slot 0 (the
    /// primary contact) is tracked; other slots' updates are parsed (so the
    /// event stream stays in sync) but not applied to the mouse pipeline.
    slot: i32,
}

static TOUCH: TicketLock<Option<Touch>> = TicketLock::new(None);

#[derive(Clone, Copy)]
pub struct TouchReport {
    pub present: bool,
    pub queue_ready: bool,
    pub abs_x_span: i32,
    pub abs_y_span: i32,
    pub verified: bool,
}

/// Reads one axis's `virtio_input_absinfo.min`/`.max` via the device's own
/// config space (`select` = `CFG_ABS_INFO`, `subsel` = the `ABS_*` code).
fn query_axis(modern: &Modern, code: u16) -> AxisRange {
    if !modern.has_device_config() {
        // Spec-required for virtio-input, but a broken/non-compliant device
        // could still answer to the right vendor:device id without one -
        // reading/writing offset 0 of a null "BAR" would otherwise fault.
        return AxisRange::EMPTY;
    }
    modern.device_config_write8(1, code as u8);
    modern.device_config_write8(0, CFG_ABS_INFO);
    let size = modern.device_config_read8(2);
    if size < 8 {
        return AxisRange::EMPTY;
    }
    AxisRange {
        min: modern.device_config_read32(8),
        max: modern.device_config_read32(12),
    }
}

pub fn initialize(pci: &PciInventory, frames: &mut FrameAllocator) -> TouchReport {
    let Some((_, modern)) = Modern::find(pci, DEVICE_ID) else {
        return TouchReport {
            present: false,
            queue_ready: false,
            abs_x_span: 0,
            abs_y_span: 0,
            verified: true, // no touch hardware attached is not a failure
        };
    };
    if !modern.begin(0) {
        return TouchReport {
            present: true,
            queue_ready: false,
            abs_x_span: 0,
            abs_y_span: 0,
            verified: false,
        };
    }
    let x_range = query_axis(&modern, ABS_MT_POSITION_X);
    let y_range = query_axis(&modern, ABS_MT_POSITION_Y);
    // Single-touch-only hardware has no ABS_MT_* axes at all; fall back to
    // the plain ABS_X/ABS_Y pair the same config query exposes.
    let x_range = if x_range.span() > 0 {
        x_range
    } else {
        query_axis(&modern, ABS_X)
    };
    let y_range = if y_range.span() > 0 {
        y_range
    } else {
        query_axis(&modern, ABS_Y)
    };
    let Some((mut queue, notify_off)) = modern.queue(EVENT_QUEUE, frames) else {
        return TouchReport {
            present: true,
            queue_ready: false,
            abs_x_span: x_range.span(),
            abs_y_span: y_range.span(),
            verified: false,
        };
    };
    let Some(buffers) = frames.allocate() else {
        return TouchReport {
            present: true,
            queue_ready: false,
            abs_x_span: x_range.span(),
            abs_y_span: y_range.span(),
            verified: false,
        };
    };
    let buffers = buffers.address();
    unsafe { core::ptr::write_bytes(buffers as usize as *mut u8, 0, 4096) };
    for slot in 0..EVENT_SLOTS.min(queue.size) {
        queue.descriptor(
            slot,
            buffers + slot as u64 * EVENT_BYTES,
            EVENT_BYTES as u32,
            DESC_WRITE,
            0,
        );
        queue.submit(slot as u16);
    }
    modern.notify(notify_off);
    modern.driver_ok();
    let queue_ready = true;
    *TOUCH.lock() = Some(Touch {
        modern,
        queue,
        notify_off,
        buffers,
        x_range,
        y_range,
        active: false,
        x: 0,
        y: 0,
        have_position: false,
        slot: 0,
    });
    TouchReport {
        present: true,
        queue_ready,
        abs_x_span: x_range.span(),
        abs_y_span: y_range.span(),
        verified: x_range.span() > 0 && y_range.span() > 0,
    }
}

/// Drains every event the device has produced since the last call, feeding
/// completed contact-down/move/up frames into `mouse::inject_absolute` -
/// call this from the desktop loop the same way it already polls the
/// keyboard/mouse each iteration.
pub fn poll() {
    let mut guard = TOUCH.lock();
    let Some(touch) = guard.as_mut() else {
        return;
    };
    while let Some((descriptor, _)) = touch.queue.pop_used() {
        let base = touch.buffers + descriptor as u64 * EVENT_BYTES;
        let kind = unsafe { core::ptr::read_volatile(base as *const u16) };
        let code = unsafe { core::ptr::read_volatile((base + 2) as *const u16) };
        let value = unsafe { core::ptr::read_volatile((base + 4) as *const u32) };
        match kind {
            EV_ABS if code == ABS_MT_SLOT => touch.slot = value as i32,
            EV_ABS if code == ABS_MT_TRACKING_ID && touch.slot == 0 => {
                touch.active = value as i32 != -1;
            }
            EV_ABS if code == ABS_MT_POSITION_X && touch.slot == 0 => {
                touch.x = value as i32;
                touch.have_position = true;
            }
            EV_ABS if code == ABS_MT_POSITION_Y && touch.slot == 0 => {
                touch.y = value as i32;
                touch.have_position = true;
            }
            EV_ABS if code == ABS_X => {
                touch.x = value as i32;
                touch.have_position = true;
            }
            EV_ABS if code == ABS_Y => {
                touch.y = value as i32;
                touch.have_position = true;
            }
            EV_KEY if code == BTN_TOUCH => touch.active = value != 0,
            EV_SYN if touch.have_position => {
                let buttons = u8::from(touch.active);
                crate::mouse::inject_absolute(
                    buttons,
                    touch.x_range.scale(touch.x),
                    touch.y_range.scale(touch.y),
                    0,
                );
            }
            _ => {}
        }
        // Recycle the buffer: same descriptor, same slot in the available
        // ring, ready for the device to fill again.
        touch
            .queue
            .descriptor(descriptor as usize, base, EVENT_BYTES as u32, DESC_WRITE, 0);
        touch.queue.submit(descriptor);
    }
    touch.modern.notify(touch.notify_off);
}
