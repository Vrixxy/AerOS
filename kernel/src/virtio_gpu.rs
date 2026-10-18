//! virtio-gpu driver (2D mode only, modern transport, one command in flight
//! at a time, polled) - real kernel mode setting: this queries the actual
//! display geometry QEMU configured (`GET_DISPLAY_INFO`) and drives the
//! visible output through the virtio-gpu 2D resource/scanout protocol
//! (`RESOURCE_CREATE_2D` / `RESOURCE_ATTACH_BACKING` / `SET_SCANOUT` /
//! `TRANSFER_TO_HOST_2D` / `RESOURCE_FLUSH`) instead of leaving the display
//! as whatever UEFI's GOP happened to leave configured.
//!
//! Scoped to 2D plus the cursor plane - no VIRGL/3D context, so the
//! compositing itself (`aerui`'s rounded-rect/frost-blur drawing) is still
//! CPU-rendered into the in-memory framebuffer. The GPU does take over two
//! things: `publish()` copies only the rectangle that changed into the
//! scanout resource's backing memory and flushes just that rectangle, and
//! the pointer is a hardware cursor (`install_cursor`/`move_cursor`, the
//! second virtqueue), so moving it costs no repaint and no frame transfer. A
//! from-scratch 3D/VIRGL command encoder (Gallium/TGSI-shaped) is a much
//! larger, separate project.
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

use crate::framebuffer::{Dirty, FrameBuffer};
use crate::memory::FrameAllocator;
use crate::pci::PciInventory;
use crate::serial;
use crate::sync::TicketLock;
use crate::virtio::{DESC_NEXT, DESC_WRITE, Queue};
use crate::virtio_modern::Modern;

mod virgl;
pub use virgl::Grade;

/// `VIRTIO_GPU_F_VIRGL`: 3D (virgl) command submission.
const FEATURE_VIRGL: u32 = 1;

const DEVICE_ID: u16 = 0x1050; // virtio device type 16 (gpu) + 0x1040
const CONTROL_QUEUE: u16 = 0;
const CURSOR_QUEUE: u16 = 1;

const CMD_GET_DISPLAY_INFO: u32 = 0x0100;
const CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
const CMD_RESOURCE_UNREF: u32 = 0x0102;
const CMD_SET_SCANOUT: u32 = 0x0103;
const CMD_RESOURCE_FLUSH: u32 = 0x0104;
const CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
const CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
const CMD_UPDATE_CURSOR: u32 = 0x0300;
const CMD_MOVE_CURSOR: u32 = 0x0301;

const RESP_OK_NODATA: u32 = 0x1100;
const RESP_OK_DISPLAY_INFO: u32 = 0x1101;

/// `VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM` - memory byte order B,G,R,X, exactly
/// what `FrameBuffer::copy_bgrx8888` produces regardless of the source
/// `PixelFormat` the firmware's GOP actually reported.
const FORMAT_B8G8R8X8_UNORM: u32 = 2;
/// `VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM` - what the cursor image uses (alpha).
const FORMAT_B8G8R8A8_UNORM: u32 = 1;

/// The cursor plane is always a 64x64 image on this device.
pub const CURSOR_DIM: usize = 64;
const CURSOR_RESOURCE_ID: u32 = 2;
const CURSOR_SLOT_BYTES: usize = 64;
const CURSOR_SLOTS: usize = 8;

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
struct CursorPos {
    scanout_id: u32,
    x: u32,
    y: u32,
    padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CursorCommand {
    hdr: CtrlHeader,
    pos: CursorPos,
    resource_id: u32,
    hot_x: u32,
    hot_y: u32,
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
    /// Hardware cursor plane: its own queue, a page of command slots, and
    /// the image installed by `install_cursor`.
    cursor_queue: Queue,
    cursor_notify_off: u16,
    cursor_commands: u64,
    cursor_next: usize,
    cursor_inflight: usize,
    cursor_ready: bool,
    cursor_shown: bool,
    cursor_x: u32,
    cursor_y: u32,
    cursor_hot: (u32, u32),
    cursor_backing: u64,
    virgl_offered: bool,
    virgl: Option<virgl::State>,
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
    exchange_raw(gpu, gpu.control + CMD_OFFSET, command_len, response_len)
}

/// Like `exchange`, for a command already sitting in memory at
/// `command_address` (a 3D command stream is much larger than the shared
/// scratch page); the response still lands at `RESP_OFFSET`.
fn exchange_raw(
    gpu: &mut GpuState,
    command_address: u64,
    command_len: u32,
    response_len: u32,
) -> bool {
    gpu.queue
        .descriptor(0, command_address, command_len, DESC_NEXT, 1);
    gpu.queue
        .descriptor(1, gpu.control + RESP_OFFSET, response_len, DESC_WRITE, 0);
    gpu.queue.submit(0);
    gpu.modern.notify(gpu.notify_off);
    gpu.queue.wait_used().is_some()
}

fn create_2d(gpu: &mut GpuState, resource_id: u32, width: u32, height: u32) -> bool {
    create_2d_format(gpu, resource_id, FORMAT_B8G8R8X8_UNORM, width, height)
}

fn create_2d_format(
    gpu: &mut GpuState,
    resource_id: u32,
    format: u32,
    width: u32,
    height: u32,
) -> bool {
    let command = ResourceCreate2d {
        hdr: CtrlHeader::new(CMD_RESOURCE_CREATE_2D),
        resource_id,
        format,
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
    transfer_rect(
        gpu,
        resource_id,
        gpu.width.max(width),
        (0, 0, width, height),
    )
}

/// Copies one rectangle of the resource's backing memory to the host;
/// `stride_pixels` is the resource's width (the backing is tightly packed).
fn transfer_rect(
    gpu: &mut GpuState,
    resource_id: u32,
    stride_pixels: u32,
    (x, y, width, height): (u32, u32, u32, u32),
) -> bool {
    let command = TransferToHost2d {
        hdr: CtrlHeader::new(CMD_TRANSFER_TO_HOST_2D),
        r: Rect {
            x,
            y,
            width,
            height,
        },
        offset: (y as u64 * stride_pixels as u64 + x as u64) * 4,
        resource_id,
        padding: 0,
    };
    exchange(gpu, command, 24) && response_type(gpu.control) == RESP_OK_NODATA
}

fn resource_flush(gpu: &mut GpuState, resource_id: u32, width: u32, height: u32) -> bool {
    flush_rect(gpu, resource_id, (0, 0, width, height))
}

fn flush_rect(
    gpu: &mut GpuState,
    resource_id: u32,
    (x, y, width, height): (u32, u32, u32, u32),
) -> bool {
    let command = ResourceFlush {
        hdr: CtrlHeader::new(CMD_RESOURCE_FLUSH),
        r: Rect {
            x,
            y,
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
    if !modern.begin(FEATURE_VIRGL) {
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
    let virgl_offered = modern.offered_low() & FEATURE_VIRGL != 0;
    let (cursor_queue, cursor_notify_off, cursor_commands) =
        match (modern.queue(CURSOR_QUEUE, frames), frames.allocate()) {
            (Some((cursor_queue, notify)), Some(page)) => {
                let page = page.address();
                unsafe { core::ptr::write_bytes(page as usize as *mut u8, 0, 4096) };
                (cursor_queue, notify, page)
            }
            _ => (Queue::EMPTY, 0, 0),
        };
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
        cursor_queue,
        cursor_notify_off,
        cursor_commands,
        cursor_next: 0,
        cursor_inflight: 0,
        cursor_ready: false,
        cursor_shown: false,
        cursor_x: 0,
        cursor_y: 0,
        cursor_hot: (0, 0),
        cursor_backing: 0,
        virgl_offered,
        virgl: None,
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
    // 3D first: with it negotiated the host turns every resource into a GL
    // texture, and what formats that supports is only learned by trying.
    if gpu.virgl_offered {
        let state = virgl::init(gpu, width, height, frames);
        gpu.virgl = match state {
            Some(state) => {
                let test = virgl::self_test(gpu, &state);
                serial::format(format_args!(
                    "AEROS_VIRGL selftest={} sample={:02x}{:02x}{:02x}{:02x}
",
                    test.ok, test.sample[0], test.sample[1], test.sample[2], test.sample[3]
                ));
                test.ok.then_some(state)
            }
            None => {
                serial::line("AEROS_VIRGL context=false");
                None
            }
        };
    }
    if gpu.virgl.is_some() {
        // With 3D the host would have to export a GL texture to scan out our
        // resource, which not every host can do; a failed attempt also
        // leaves the display blank. So the firmware framebuffer stays the
        // display and the GPU is used purely for the glass.
        serial::line("AEROS_GPU_SCANOUT mode=firmware");
        return false;
    }
    let created = create_2d_format(
        gpu,
        DISPLAY_RESOURCE_ID,
        FORMAT_B8G8R8X8_UNORM,
        width,
        height,
    );
    let attached = created && attach_backing(gpu, DISPLAY_RESOURCE_ID, backing, bytes as u32);
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
    let scanout = attached && exchange(gpu, command, 24);
    if !scanout || response_type(gpu.control) != RESP_OK_NODATA {
        return false;
    }
    gpu.backing = backing;
    gpu.width = width;
    gpu.height = height;
    gpu.bound = true;
    true
}

/// Brings the device back after a sleep that cost it its state: queues over
/// the same memory, then the display resource and the cursor image created
/// again. The 3D context is gone, so the glass falls back to the CPU.
pub fn resume() -> bool {
    let mut guard = GPU.lock();
    let Some(gpu) = guard.as_mut() else {
        return true;
    };
    if !gpu.modern.begin(0) {
        return false;
    }
    gpu.queue.reset();
    gpu.modern.restore_queue(CONTROL_QUEUE, &gpu.queue);
    if gpu.cursor_queue.size != 0 {
        gpu.cursor_queue.reset();
        gpu.modern.restore_queue(CURSOR_QUEUE, &gpu.cursor_queue);
    }
    gpu.modern.driver_ok();
    gpu.virgl = None;
    gpu.virgl_offered = false;
    gpu.cursor_next = 0;
    gpu.cursor_inflight = 0;
    let mut ok = true;
    if gpu.bound {
        let (width, height) = (gpu.width, gpu.height);
        let bytes = width as u64 * height as u64 * 4;
        let created = create_2d_format(
            gpu,
            DISPLAY_RESOURCE_ID,
            FORMAT_B8G8R8X8_UNORM,
            width,
            height,
        );
        let attached =
            created && attach_backing(gpu, DISPLAY_RESOURCE_ID, gpu.backing, bytes as u32);
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
        ok &=
            attached && exchange(gpu, command, 24) && response_type(gpu.control) == RESP_OK_NODATA;
    }
    if gpu.cursor_ready && gpu.cursor_backing != 0 {
        let dim = CURSOR_DIM as u32;
        let bytes = (CURSOR_DIM * CURSOR_DIM * 4) as u32;
        let backing = gpu.cursor_backing;
        let restored = create_2d_format(gpu, CURSOR_RESOURCE_ID, FORMAT_B8G8R8A8_UNORM, dim, dim)
            && attach_backing(gpu, CURSOR_RESOURCE_ID, backing, bytes)
            && transfer_rect(gpu, CURSOR_RESOURCE_ID, dim, (0, 0, dim, dim));
        gpu.cursor_shown = restored && cursor_command(gpu, CMD_UPDATE_CURSOR, CURSOR_RESOURCE_ID);
        ok &= restored;
    }
    ok
}

/// GPU glass timing since the last call (see `virgl::take_stats`).
pub fn frost_stats() -> [u64; 7] {
    virgl::take_stats()
}

/// True once the GPU can blur and grade a rectangle (`frost_pass`).
pub fn frost_available() -> bool {
    GPU.lock().as_ref().is_some_and(|gpu| gpu.virgl.is_some())
}

/// Blurs (box, `radius` texels each side) and grades a `width` x `height`
/// rectangle on the GPU. `upload` writes BGRA source rows into the given
/// buffer (`stride` bytes per row); `download` reads the result the same
/// way. False when there is no 3D support or a step failed (nothing was
/// downloaded then).
pub fn frost_pass(
    width: u32,
    height: u32,
    radius: u32,
    grade: Grade,
    upload: &mut dyn FnMut(&mut [u8], usize),
    download: &mut dyn FnMut(&[u8], usize),
) -> bool {
    let mut guard = GPU.lock();
    let Some(gpu) = guard.as_mut() else {
        return false;
    };
    let Some(state) = gpu.virgl.take() else {
        return false;
    };
    let done = virgl::frost_pass(gpu, &state, width, height, radius, grade, upload, download);
    gpu.virgl = Some(state);
    done
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
    let Dirty::Region {
        x,
        y,
        width,
        height,
    } = frame.copy_bgrx8888_changed(destination)
    else {
        return;
    };
    let rect = (x as u32, y as u32, width as u32, height as u32);
    let stride = gpu.width;
    if !transfer_rect(gpu, DISPLAY_RESOURCE_ID, stride, rect) {
        return;
    }
    let _ = flush_rect(gpu, DISPLAY_RESOURCE_ID, rect);
}

/// Shows `frame` on the device's scanout in full, from the panic handler: it
/// gives up rather than wait if the device lock is held.
pub fn publish_panic(frame: &FrameBuffer) {
    let Some(mut guard) = GPU.try_lock() else {
        return;
    };
    let Some(gpu) = guard.as_mut() else {
        return;
    };
    if !gpu.bound {
        return;
    }
    let bytes = (gpu.width as usize) * (gpu.height as usize) * 4;
    let destination =
        unsafe { core::slice::from_raw_parts_mut(gpu.backing as usize as *mut u8, bytes) };
    frame.copy_bgrx8888(destination);
    let rect = (0, 0, gpu.width, gpu.height);
    let stride = gpu.width;
    if transfer_rect(gpu, DISPLAY_RESOURCE_ID, stride, rect) {
        let _ = flush_rect(gpu, DISPLAY_RESOURCE_ID, rect);
    }
}

fn cursor_command(gpu: &mut GpuState, kind: u32, resource_id: u32) -> bool {
    if gpu.cursor_queue.size == 0 {
        return false;
    }
    while gpu.cursor_queue.pop_used().is_some() {
        gpu.cursor_inflight = gpu.cursor_inflight.saturating_sub(1);
    }
    let slots = gpu.cursor_queue.size.min(CURSOR_SLOTS);
    if gpu.cursor_inflight >= slots {
        return false;
    }
    let slot = gpu.cursor_next % slots;
    gpu.cursor_next = gpu.cursor_next.wrapping_add(1);
    let command = CursorCommand {
        hdr: CtrlHeader::new(kind),
        pos: CursorPos {
            scanout_id: SCANOUT_ID,
            x: gpu.cursor_x,
            y: gpu.cursor_y,
            padding: 0,
        },
        resource_id,
        hot_x: gpu.cursor_hot.0,
        hot_y: gpu.cursor_hot.1,
        padding: 0,
    };
    let address = gpu.cursor_commands + (slot * CURSOR_SLOT_BYTES) as u64;
    unsafe { core::ptr::write_volatile(address as *mut CursorCommand, command) };
    gpu.cursor_queue.descriptor(
        slot,
        address,
        core::mem::size_of::<CursorCommand>() as u32,
        0,
        0,
    );
    gpu.cursor_queue.submit(slot as u16);
    gpu.modern.notify(gpu.cursor_notify_off);
    gpu.cursor_inflight += 1;
    true
}

/// Hands the GPU a 64x64 ARGB cursor image (0xAARRGGBB, straight alpha) with
/// its hot spot; from then on `move_cursor` positions it with no CPU
/// drawing. False if there is no GPU, no cursor queue, or any step failed.
pub fn install_cursor(pixels: &[u32], hot_x: u32, hot_y: u32, frames: &mut FrameAllocator) -> bool {
    let mut guard = GPU.lock();
    let Some(gpu) = guard.as_mut() else {
        return false;
    };
    if gpu.cursor_queue.size == 0 || pixels.len() < CURSOR_DIM * CURSOR_DIM {
        return false;
    }
    let bytes = (CURSOR_DIM * CURSOR_DIM * 4) as u64;
    let Some(backing) = frames.allocate_dma(bytes.div_ceil(4096), 1) else {
        return false;
    };
    let backing = backing.address();
    for (index, value) in pixels[..CURSOR_DIM * CURSOR_DIM].iter().enumerate() {
        unsafe { core::ptr::write_volatile((backing as usize + index * 4) as *mut u32, *value) };
    }
    let dim = CURSOR_DIM as u32;
    if !create_2d_format(gpu, CURSOR_RESOURCE_ID, FORMAT_B8G8R8A8_UNORM, dim, dim)
        || !attach_backing(gpu, CURSOR_RESOURCE_ID, backing, bytes as u32)
        || !transfer_rect(gpu, CURSOR_RESOURCE_ID, dim, (0, 0, dim, dim))
    {
        return false;
    }
    gpu.cursor_hot = (hot_x, hot_y);
    gpu.cursor_backing = backing;
    gpu.cursor_ready = true;
    gpu.cursor_shown = cursor_command(gpu, CMD_UPDATE_CURSOR, CURSOR_RESOURCE_ID);
    gpu.cursor_ready
}

/// True once the GPU draws the pointer itself.
pub fn cursor_active() -> bool {
    GPU.lock().as_ref().is_some_and(|gpu| gpu.cursor_ready)
}

/// Moves the hardware cursor to a screen position.
pub fn move_cursor(x: i32, y: i32) {
    let mut guard = GPU.lock();
    let Some(gpu) = guard.as_mut() else {
        return;
    };
    if !gpu.cursor_ready {
        return;
    }
    gpu.cursor_x = x.max(0) as u32;
    gpu.cursor_y = y.max(0) as u32;
    let kind = if gpu.cursor_shown {
        CMD_MOVE_CURSOR
    } else {
        CMD_UPDATE_CURSOR
    };
    if cursor_command(gpu, kind, CURSOR_RESOURCE_ID) {
        gpu.cursor_shown = true;
    }
}

/// Shows or hides the hardware cursor (hidden while a guest draws its own).
pub fn set_cursor_visible(visible: bool) {
    let mut guard = GPU.lock();
    let Some(gpu) = guard.as_mut() else {
        return;
    };
    if !gpu.cursor_ready || gpu.cursor_shown == visible {
        return;
    }
    let resource = if visible { CURSOR_RESOURCE_ID } else { 0 };
    if cursor_command(gpu, CMD_UPDATE_CURSOR, resource) {
        gpu.cursor_shown = visible;
    }
}
