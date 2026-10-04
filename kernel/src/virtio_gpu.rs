//! virtio-gpu driver (2D mode only, modern transport, one command in flight
//! at a time, polled) - real kernel mode setting: this queries the actual
//! display geometry QEMU configured (`GET_DISPLAY_INFO`) and drives the
//! visible output through the virtio-gpu 2D resource/scanout protocol
//! (`RESOURCE_CREATE_2D` / `RESOURCE_ATTACH_BACKING` / `SET_SCANOUT` /
//! `TRANSFER_TO_HOST_2D` / `RESOURCE_FLUSH`) instead of leaving the display
//! as whatever UEFI's GOP happened to leave configured.
//!
//! Scoped to 2D only - no VIRGL/3D context, no GPU-side acceleration of the
//! actual compositing (`aerui`'s rounded-rect/frost-blur drawing is still
//! 100% CPU-rendered into the same in-memory framebuffer it always was).
//! What changes is how the finished frame reaches the screen: `publish()`
//! copies it into this resource's own backing memory and flushes it through
//! the GPU's own display pipeline, real kernel-driven mode setting rather
//! than relying on firmware-configured GOP state. A from-scratch 3D/VIRGL
//! command encoder (Gallium/TGSI-shaped) is a much larger, separate project.
//!
//! Two distinct resources exist, deliberately never confused:
//! - `TEST_RESOURCE_ID`: a throwaway 32x32 resource `initialize`'s own
//!   self-test creates, exercises (create/attach/transfer/flush) and tears
//!   down again before returning - proves the whole command pipeline works
//!   against the real device without ever touching the real scanout, so it
//!   is safe to run unconditionally at boot (including a real interactive
//!   boot, not just `test.ps1`), with nothing visibly flashing on screen.
//! - `DISPLAY_RESOURCE_ID`: created once by `bind()`, sized to the real
//!   `FrameBuffer`'s actual dimensions, and the only resource ever handed to
//!   `SET_SCANOUT` - this is what actually appears on screen.

use crate::framebuffer::FrameBuffer;
use crate::memory::FrameAllocator;
use crate::pci::PciInventory;
use crate::sync::TicketLock;
use crate::virtio::{DESC_NEXT, DESC_WRITE, Queue};
use crate::virtio_modern::Modern;

const DEVICE_ID: u16 = 0x1050; // virtio device type 16 (gpu) + 0x1040
const CONTROL_QUEUE: u16 = 0;

const CMD_GET_DISPLAY_INFO: u32 = 0x0100;
const CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
const CMD_RESOURCE_UNREF: u32 = 0x0102;
const CMD_SET_SCANOUT: u32 = 0x0103;
const CMD_RESOURCE_FLUSH: u32 = 0x0104;
const CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
const CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;

const RESP_OK_NODATA: u32 = 0x1100;
const RESP_OK_DISPLAY_INFO: u32 = 0x1101;

/// `VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM` - memory byte order B,G,R,X, exactly
/// what `FrameBuffer::copy_bgrx8888` produces regardless of the source
/// `PixelFormat` the firmware's GOP actually reported.
const FORMAT_B8G8R8X8_UNORM: u32 = 2;

const TEST_RESOURCE_ID: u32 = 0xff;
const DISPLAY_RESOURCE_ID: u32 = 1;
const SCANOUT_ID: u32 = 0;

/// Command/response scratch buffer layout within one shared page: generous
/// headroom for the largest fixed command (`AttachBacking`, 48 bytes) and
/// the largest response (`RespDisplayInfo`, 408 bytes), far apart enough
/// that neither could ever overlap the other.
const CMD_OFFSET: u64 = 0;
const RESP_OFFSET: u64 = 512;

#[repr(C)]
#[derive(Clone, Copy)]
struct CtrlHeader {
    cmd_type: u32,
    flags: u32,
    fence_id: u64,
    ctx_id: u32,
    ring_idx: u8,
    padding: [u8; 3],
}

impl CtrlHeader {
    const fn new(cmd_type: u32) -> Self {
        Self {
            cmd_type,
            flags: 0,
            fence_id: 0,
            ctx_id: 0,
            ring_idx: 0,
            padding: [0; 3],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Rect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DisplayOne {
    r: Rect,
    enabled: u32,
    flags: u32,
}

#[repr(C)]
struct RespDisplayInfo {
    hdr: CtrlHeader,
    modes: [DisplayOne; 16],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ResourceCreate2d {
    hdr: CtrlHeader,
    resource_id: u32,
    format: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MemEntry {
    addr: u64,
    length: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AttachBacking {
    hdr: CtrlHeader,
    resource_id: u32,
    nr_entries: u32,
    entry: MemEntry,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SetScanout {
    hdr: CtrlHeader,
    r: Rect,
    scanout_id: u32,
    resource_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TransferToHost2d {
    hdr: CtrlHeader,
    r: Rect,
    offset: u64,
    resource_id: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ResourceFlush {
    hdr: CtrlHeader,
    r: Rect,
    resource_id: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ResourceUnref {
    hdr: CtrlHeader,
    resource_id: u32,
    padding: u32,
}

struct GpuState {
    modern: Modern,
    queue: Queue,
    notify_off: u16,
    control: u64,
    /// Set once `bind()` has created `DISPLAY_RESOURCE_ID` and handed it to
    /// `SET_SCANOUT` - `publish()` is a no-op until then.
    bound: bool,
    backing: u64,
    width: u32,
    height: u32,
}

static GPU: TicketLock<Option<GpuState>> = TicketLock::new(None);

#[derive(Clone, Copy)]
pub struct GpuReport {
    pub present: bool,
    pub queue_ready: bool,
    pub display_width: u32,
    pub display_height: u32,
    pub resource_created: bool,
    pub backing_attached: bool,
    pub transfer_ok: bool,
    pub flush_ok: bool,
    pub verified: bool,
}

impl GpuReport {
    const ABSENT: Self = Self {
        present: false,
        queue_ready: false,
        display_width: 0,
        display_height: 0,
        resource_created: false,
        backing_attached: false,
        transfer_ok: false,
        flush_ok: false,
        verified: true, // no virtio-gpu device attached is not a failure
    };

    const fn failed(present: bool) -> Self {
        Self {
            present,
            queue_ready: false,
            display_width: 0,
            display_height: 0,
            resource_created: false,
            backing_attached: false,
            transfer_ok: false,
            flush_ok: false,
            verified: false,
        }
    }
}

fn response_type(control: u64) -> u32 {
    unsafe { core::ptr::read_volatile((control + RESP_OFFSET) as *const u32) }
}

/// Writes `command` into the shared command buffer, submits a
/// command/response descriptor pair, rings the doorbell, and waits for the
/// device to finish - the two-descriptor shape every virtio-gpu control
/// command uses (device-readable command in, device-writable response out),
/// matching the exact chain pattern `virtio_blk`'s request/status pair
/// already established for this codebase's other polled virtio drivers.
fn exchange<T>(gpu: &mut GpuState, command: T, response_len: u32) -> bool {
    let command_len = core::mem::size_of::<T>() as u32;
    unsafe {
        core::ptr::write_volatile((gpu.control + CMD_OFFSET) as *mut T, command);
    }
    gpu.queue
        .descriptor(0, gpu.control + CMD_OFFSET, command_len, DESC_NEXT, 1);
    gpu.queue
        .descriptor(1, gpu.control + RESP_OFFSET, response_len, DESC_WRITE, 0);
    gpu.queue.submit(0);
    gpu.modern.notify(gpu.notify_off);
    gpu.queue.wait_used().is_some()
}

fn create_2d(gpu: &mut GpuState, resource_id: u32, width: u32, height: u32) -> bool {
    let command = ResourceCreate2d {
        hdr: CtrlHeader::new(CMD_RESOURCE_CREATE_2D),
        resource_id,
        format: FORMAT_B8G8R8X8_UNORM,
        width,
        height,
    };
    exchange(gpu, command, 24) && response_type(gpu.control) == RESP_OK_NODATA
}

fn attach_backing(gpu: &mut GpuState, resource_id: u32, addr: u64, length: u32) -> bool {
    let command = AttachBacking {
        hdr: CtrlHeader::new(CMD_RESOURCE_ATTACH_BACKING),
        resource_id,
        nr_entries: 1,
        entry: MemEntry {
            addr,
            length,
            padding: 0,
        },
    };
    exchange(gpu, command, 24) && response_type(gpu.control) == RESP_OK_NODATA
}

fn transfer_to_host_2d(gpu: &mut GpuState, resource_id: u32, width: u32, height: u32) -> bool {
    let command = TransferToHost2d {
        hdr: CtrlHeader::new(CMD_TRANSFER_TO_HOST_2D),
        r: Rect {
            x: 0,
            y: 0,
            width,
            height,
        },
        offset: 0,
        resource_id,
        padding: 0,
    };
    exchange(gpu, command, 24) && response_type(gpu.control) == RESP_OK_NODATA
}

fn resource_flush(gpu: &mut GpuState, resource_id: u32, width: u32, height: u32) -> bool {
    let command = ResourceFlush {
        hdr: CtrlHeader::new(CMD_RESOURCE_FLUSH),
        r: Rect {
            x: 0,
            y: 0,
            width,
            height,
        },
        resource_id,
        padding: 0,
    };
    exchange(gpu, command, 24) && response_type(gpu.control) == RESP_OK_NODATA
}

fn resource_unref(gpu: &mut GpuState, resource_id: u32) -> bool {
    let command = ResourceUnref {
        hdr: CtrlHeader::new(CMD_RESOURCE_UNREF),
        resource_id,
        padding: 0,
    };
    exchange(gpu, command, 24) && response_type(gpu.control) == RESP_OK_NODATA
}

/// Queries `GET_DISPLAY_INFO` and returns scanout 0's rectangle - zero width
/// and height if the device reported it disabled (or the command failed),
/// callers fall back to a sensible default in that case.
fn display_info(gpu: &mut GpuState) -> (u32, u32) {
    let command = CtrlHeader::new(CMD_GET_DISPLAY_INFO);
    if !exchange(gpu, command, core::mem::size_of::<RespDisplayInfo>() as u32)
        || response_type(gpu.control) != RESP_OK_DISPLAY_INFO
    {
        return (0, 0);
    }
    let base = gpu.control + RESP_OFFSET + 24; // skip CtrlHeader, land on modes[0]
    let enabled = unsafe { core::ptr::read_volatile((base + 16) as *const u32) };
    if enabled == 0 {
        return (0, 0);
    }
    let width = unsafe { core::ptr::read_volatile((base + 8) as *const u32) };
    let height = unsafe { core::ptr::read_volatile((base + 12) as *const u32) };
    (width, height)
}

/// Detects the device, negotiates it, sets up the control queue, and proves
/// the whole command pipeline works end-to-end against a throwaway 32x32
/// test resource (`TEST_RESOURCE_ID`) that is fully torn down again before
/// returning - safe to run unconditionally at boot (including a real
/// interactive one, not just `test.ps1`), since nothing it does is ever
/// handed to `SET_SCANOUT` and nothing visible changes on screen. Call
/// `bind()` afterward (once the real `FrameBuffer`'s dimensions are known)
/// to actually take over the display.
pub fn initialize(pci: &PciInventory, frames: &mut FrameAllocator) -> GpuReport {
    let Some((_, modern)) = Modern::find(pci, DEVICE_ID) else {
        return GpuReport::ABSENT;
    };
    if !modern.begin(0) {
        return GpuReport::failed(true);
    }
    let Some((queue, notify_off)) = modern.queue(CONTROL_QUEUE, frames) else {
        return GpuReport::failed(true);
    };
    let Some(control) = frames.allocate() else {
        return GpuReport::failed(true);
    };
    let control = control.address();
    unsafe { core::ptr::write_bytes(control as usize as *mut u8, 0, 4096) };
    modern.driver_ok();
    let mut gpu = GpuState {
        modern,
        queue,
        notify_off,
        control,
        bound: false,
        backing: 0,
        width: 0,
        height: 0,
    };

    let (display_width, display_height) = display_info(&mut gpu);

    let Some(test_page) = frames.allocate() else {
        return GpuReport {
            present: true,
            queue_ready: true,
            display_width,
            display_height,
            resource_created: false,
            backing_attached: false,
            transfer_ok: false,
            flush_ok: false,
            verified: false,
        };
    };
    let test_page = test_page.address();
    unsafe {
        for index in 0..4096usize {
            core::ptr::write_volatile(
                (test_page as usize + index) as *mut u8,
                (index as u8).wrapping_mul(7).wrapping_add(3),
            );
        }
    }
    // 32x32 B8G8R8X8 is exactly one 4096-byte page - deliberately chosen so
    // the self-test's backing needs only the one frame already allocated.
    let resource_created = create_2d(&mut gpu, TEST_RESOURCE_ID, 32, 32);
    let backing_attached =
        resource_created && attach_backing(&mut gpu, TEST_RESOURCE_ID, test_page, 4096);
    let transfer_ok = backing_attached && transfer_to_host_2d(&mut gpu, TEST_RESOURCE_ID, 32, 32);
    let flush_ok = transfer_ok && resource_flush(&mut gpu, TEST_RESOURCE_ID, 32, 32);
    if resource_created {
        let _ = resource_unref(&mut gpu, TEST_RESOURCE_ID);
    }
    let verified = resource_created && backing_attached && transfer_ok && flush_ok;
    *GPU.lock() = Some(gpu);
    GpuReport {
        present: true,
        queue_ready: true,
        display_width,
        display_height,
        resource_created,
        backing_attached,
        transfer_ok,
        flush_ok,
        verified,
    }
}

/// Creates `DISPLAY_RESOURCE_ID` sized to `frame`'s real dimensions,
/// allocates its backing memory, and hands it to `SET_SCANOUT` - from this
/// point on, `publish(frame)` is what actually reaches the screen; GOP
/// writes into `frame` itself still happen exactly as before (nothing about
/// how `aerui`/`desktop` render changes) but are no longer what the display
/// hardware scans out. A no-op (returns `false`) if no virtio-gpu device was
/// found, or if it's already bound.
pub fn bind(frame: &FrameBuffer, frames: &mut FrameAllocator) -> bool {
    let mut guard = GPU.lock();
    let Some(gpu) = guard.as_mut() else {
        return false;
    };
    if gpu.bound {
        return false;
    }
    let width = frame.width() as u32;
    let height = frame.height() as u32;
    let Some(bytes) = (width as u64)
        .checked_mul(height as u64)
        .and_then(|pixels| pixels.checked_mul(4))
    else {
        return false;
    };
    let pages = bytes.div_ceil(4096);
    let Some(backing) = frames.allocate_dma(pages, 1) else {
        return false;
    };
    let backing = backing.address();
    unsafe { core::ptr::write_bytes(backing as usize as *mut u8, 0, (pages * 4096) as usize) };
    if !create_2d(gpu, DISPLAY_RESOURCE_ID, width, height) {
        return false;
    }
    if !attach_backing(gpu, DISPLAY_RESOURCE_ID, backing, bytes as u32) {
        return false;
    }
    let command = SetScanout {
        hdr: CtrlHeader::new(CMD_SET_SCANOUT),
        r: Rect {
            x: 0,
            y: 0,
            width,
            height,
        },
        scanout_id: SCANOUT_ID,
        resource_id: DISPLAY_RESOURCE_ID,
    };
    if !exchange(gpu, command, 24) || response_type(gpu.control) != RESP_OK_NODATA {
        return false;
    }
    gpu.backing = backing;
    gpu.width = width;
    gpu.height = height;
    gpu.bound = true;
    true
}

/// Copies `frame`'s current contents into the live display resource's
/// backing memory and flushes the whole surface through the GPU - call this
/// after a real repaint (same cadence as the existing redraw/present calls,
/// not every cursor-only tick, since each call is a real `width*height*4`
/// copy plus a device round trip). A no-op before `bind()` has succeeded, or
/// if no virtio-gpu device is present at all.
pub fn publish(frame: &FrameBuffer) {
    let mut guard = GPU.lock();
    let Some(gpu) = guard.as_mut() else {
        return;
    };
    if !gpu.bound {
        return;
    }
    let bytes = (gpu.width as usize) * (gpu.height as usize) * 4;
    let destination =
        unsafe { core::slice::from_raw_parts_mut(gpu.backing as usize as *mut u8, bytes) };
    if !frame.copy_bgrx8888(destination) {
        return;
    }
    if !transfer_to_host_2d(gpu, DISPLAY_RESOURCE_ID, gpu.width, gpu.height) {
        return;
    }
    let _ = resource_flush(gpu, DISPLAY_RESOURCE_ID, gpu.width, gpu.height);
}
