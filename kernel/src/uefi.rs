use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::framebuffer::{FrameBufferInfo, PixelFormat};
use crate::memory::BootMemoryMap;

pub type Handle = *mut c_void;
pub type Status = usize;

const SUCCESS: Status = 0;
const SYSTEM_TABLE_SIGNATURE: u64 = 0x5453_5953_2049_4249;
const MAP_CAPACITY: usize = 256 * 1024;
const EXIT_ATTEMPTS: usize = 8;

static GOP_GUID: Guid = Guid::new(
    0x9042a9de,
    0x23dc,
    0x4a38,
    [0x96, 0xfb, 0x7a, 0xde, 0xd0, 0x80, 0x51, 0x6a],
);
const ACPI2_GUID: Guid = Guid::new(
    0x8868e871,
    0xe4f1,
    0x11d3,
    [0xbc, 0x22, 0x00, 0x80, 0xc7, 0x3c, 0x88, 0x81],
);
const ACPI1_GUID: Guid = Guid::new(
    0xeb9d2d30,
    0x2d88,
    0x11d3,
    [0x9a, 0x16, 0x00, 0x90, 0x27, 0x3f, 0xc1, 0x4d],
);

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

impl Guid {
    const fn new(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Self {
        Self {
            data1,
            data2,
            data3,
            data4,
        }
    }
}

#[repr(C)]
struct TableHeader {
    signature: u64,
    revision: u32,
    header_size: u32,
    crc32: u32,
    reserved: u32,
}

#[repr(C)]
pub struct SystemTable {
    header: TableHeader,
    firmware_vendor: *mut u16,
    firmware_revision: u32,
    console_in_handle: Handle,
    console_in: *mut c_void,
    console_out_handle: Handle,
    console_out: *mut c_void,
    standard_error_handle: Handle,
    standard_error: *mut c_void,
    runtime_services: *mut c_void,
    boot_services: *mut BootServices,
    table_entries: usize,
    configuration_table: *mut ConfigurationTable,
}

type GetMemoryMap = unsafe extern "efiapi" fn(
    *mut usize,
    *mut MemoryDescriptor,
    *mut usize,
    *mut usize,
    *mut u32,
) -> Status;
type ExitBootServices = unsafe extern "efiapi" fn(Handle, usize) -> Status;
type AllocatePages = unsafe extern "efiapi" fn(u32, u32, usize, *mut u64) -> Status;
type LocateProtocol =
    unsafe extern "efiapi" fn(*const Guid, *mut c_void, *mut *mut c_void) -> Status;

#[repr(C)]
struct BootServices {
    header: TableHeader,
    raise_tpl: usize,
    restore_tpl: usize,
    allocate_pages: AllocatePages,
    free_pages: usize,
    get_memory_map: GetMemoryMap,
    allocate_pool: usize,
    free_pool: usize,
    create_event: usize,
    set_timer: usize,
    wait_for_event: usize,
    signal_event: usize,
    close_event: usize,
    check_event: usize,
    install_protocol_interface: usize,
    reinstall_protocol_interface: usize,
    uninstall_protocol_interface: usize,
    handle_protocol: usize,
    reserved: usize,
    register_protocol_notify: usize,
    locate_handle: usize,
    locate_device_path: usize,
    install_configuration_table: usize,
    load_image: usize,
    start_image: usize,
    exit: usize,
    unload_image: usize,
    exit_boot_services: ExitBootServices,
    get_next_monotonic_count: usize,
    stall: usize,
    set_watchdog_timer: usize,
    connect_controller: usize,
    disconnect_controller: usize,
    open_protocol: usize,
    close_protocol: usize,
    open_protocol_information: usize,
    protocols_per_handle: usize,
    locate_handle_buffer: usize,
    locate_protocol: LocateProtocol,
    install_multiple_protocol_interfaces: usize,
    uninstall_multiple_protocol_interfaces: usize,
    calculate_crc32: usize,
    copy_mem: usize,
    set_mem: usize,
    create_event_ex: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MemoryDescriptor {
    memory_type: u32,
    physical_start: u64,
    virtual_start: u64,
    number_of_pages: u64,
    attribute: u64,
}

#[repr(C)]
struct ConfigurationTable {
    vendor_guid: Guid,
    vendor_table: *mut c_void,
}

#[repr(C)]
struct GraphicsOutputProtocol {
    query_mode: usize,
    set_mode: usize,
    blt: usize,
    mode: *mut GraphicsOutputProtocolMode,
}

#[repr(C)]
struct GraphicsOutputProtocolMode {
    max_mode: u32,
    mode: u32,
    info: *mut GraphicsOutputModeInformation,
    size_of_info: usize,
    frame_buffer_base: u64,
    frame_buffer_size: usize,
}

#[repr(C)]
struct GraphicsOutputModeInformation {
    version: u32,
    horizontal_resolution: u32,
    vertical_resolution: u32,
    pixel_format: u32,
    pixel_information: PixelBitmask,
    pixels_per_scan_line: u32,
}

#[repr(C)]
struct PixelBitmask {
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
    reserved_mask: u32,
}

#[repr(align(16))]
struct MapBuffer(UnsafeCell<[u8; MAP_CAPACITY]>);

unsafe impl Sync for MapBuffer {}

static MEMORY_MAP: MapBuffer = MapBuffer(UnsafeCell::new([0; MAP_CAPACITY]));

pub struct BootState {
    pub framebuffer: FrameBufferInfo,
    pub memory: BootMemoryMap,
    pub rsdp: u64,
    pub ap_trampoline: u64,
}

#[derive(Clone, Copy)]
pub enum BootStage {
    SystemTable,
    BootServices,
    GraphicsProtocol,
    GraphicsMode,
    PixelFormat,
    ApTrampoline,
    MemoryMap,
    ExitBootServices,
}

impl BootStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SystemTable => "system-table",
            Self::BootServices => "boot-services",
            Self::GraphicsProtocol => "graphics-protocol",
            Self::GraphicsMode => "graphics-mode",
            Self::PixelFormat => "pixel-format",
            Self::ApTrampoline => "ap-trampoline",
            Self::MemoryMap => "memory-map",
            Self::ExitBootServices => "exit-boot-services",
        }
    }
}

pub struct BootError {
    pub stage: BootStage,
    pub status: Status,
}

pub unsafe fn take_control(
    image: Handle,
    system_table: *mut SystemTable,
) -> Result<BootState, BootError> {
    if system_table.is_null()
        || unsafe { (*system_table).header.signature } != SYSTEM_TABLE_SIGNATURE
    {
        return Err(failure(BootStage::SystemTable, 1));
    }
    let services = unsafe { (*system_table).boot_services };
    if services.is_null() {
        return Err(failure(BootStage::BootServices, 2));
    }
    let framebuffer = unsafe { locate_framebuffer(services) }?;
    let rsdp = unsafe { find_rsdp(system_table) };
    let mut ap_trampoline = 0x000f_f000u64;
    let trampoline_status = unsafe { ((*services).allocate_pages)(1, 2, 1, &mut ap_trampoline) };
    if trampoline_status != SUCCESS
        || ap_trampoline == 0
        || ap_trampoline >= 0x10_0000
        || ap_trampoline & 0xfff != 0
    {
        return Err(BootError {
            stage: BootStage::ApTrampoline,
            status: if trampoline_status == SUCCESS {
                failure(BootStage::ApTrampoline, 10).status
            } else {
                trampoline_status
            },
        });
    }

    let mut last_exit_status = failure(BootStage::ExitBootServices, 4).status;
    for _ in 0..EXIT_ATTEMPTS {
        let mut map_size = MAP_CAPACITY;
        let mut map_key = 0usize;
        let mut descriptor_size = 0usize;
        let mut descriptor_version = 0u32;
        let map_pointer = MEMORY_MAP.0.get().cast::<MemoryDescriptor>();
        let map_status = unsafe {
            ((*services).get_memory_map)(
                &mut map_size,
                map_pointer,
                &mut map_key,
                &mut descriptor_size,
                &mut descriptor_version,
            )
        };
        if map_status != SUCCESS {
            return Err(BootError {
                stage: BootStage::MemoryMap,
                status: map_status,
            });
        }
        if descriptor_size < core::mem::size_of::<MemoryDescriptor>() || descriptor_size == 0 {
            return Err(failure(BootStage::MemoryMap, 3));
        }
        let exit_status = unsafe { ((*services).exit_boot_services)(image, map_key) };
        if exit_status == SUCCESS {
            let memory = unsafe { collect_memory_map(map_size, descriptor_size) };
            return Ok(BootState {
                framebuffer,
                memory,
                rsdp,
                ap_trampoline,
            });
        }
        last_exit_status = exit_status;
    }
    Err(BootError {
        stage: BootStage::ExitBootServices,
        status: last_exit_status,
    })
}

unsafe fn locate_framebuffer(services: *mut BootServices) -> Result<FrameBufferInfo, BootError> {
    let mut interface: *mut c_void = core::ptr::null_mut();
    let status = unsafe {
        ((*services).locate_protocol)(&raw const GOP_GUID, core::ptr::null_mut(), &mut interface)
    };
    if status != SUCCESS || interface.is_null() {
        return Err(BootError {
            stage: BootStage::GraphicsProtocol,
            status: if status == SUCCESS {
                failure(BootStage::GraphicsProtocol, 9).status
            } else {
                status
            },
        });
    }
    let graphics = interface.cast::<GraphicsOutputProtocol>();
    let mode = unsafe { (*graphics).mode };
    if mode.is_null() {
        return Err(failure(BootStage::GraphicsMode, 5));
    }
    let info = unsafe { (*mode).info };
    if info.is_null() {
        return Err(failure(BootStage::GraphicsMode, 6));
    }
    let format = match unsafe { (*info).pixel_format } {
        0 => PixelFormat::Rgb,
        1 => PixelFormat::Bgr,
        2 => PixelFormat::Bitmask {
            red: unsafe { (*info).pixel_information.red_mask },
            green: unsafe { (*info).pixel_information.green_mask },
            blue: unsafe { (*info).pixel_information.blue_mask },
        },
        _ => return Err(failure(BootStage::PixelFormat, 7)),
    };
    let address = unsafe { (*mode).frame_buffer_base };
    let size = unsafe { (*mode).frame_buffer_size };
    let width = unsafe { (*info).horizontal_resolution as usize };
    let height = unsafe { (*info).vertical_resolution as usize };
    let stride = unsafe { (*info).pixels_per_scan_line as usize };
    if address == 0 || size < stride.saturating_mul(height).saturating_mul(4) {
        return Err(failure(BootStage::GraphicsMode, 8));
    }
    Ok(FrameBufferInfo {
        address: address as usize as *mut u32,
        size,
        width,
        height,
        stride,
        format,
    })
}

unsafe fn find_rsdp(system_table: *mut SystemTable) -> u64 {
    let count = unsafe { (*system_table).table_entries }.min(4096);
    let tables = unsafe { (*system_table).configuration_table };
    if tables.is_null() {
        return 0;
    }
    let mut acpi1 = 0u64;
    for index in 0..count {
        let entry = unsafe { &*tables.add(index) };
        if entry.vendor_guid == ACPI2_GUID {
            return entry.vendor_table as usize as u64;
        }
        if entry.vendor_guid == ACPI1_GUID {
            acpi1 = entry.vendor_table as usize as u64;
        }
    }
    acpi1
}

unsafe fn collect_memory_map(size: usize, stride: usize) -> BootMemoryMap {
    let mut map = BootMemoryMap::empty();
    let base = MEMORY_MAP.0.get().cast::<u8>();
    for index in 0..size / stride {
        let descriptor = unsafe {
            core::ptr::read_unaligned(base.add(index * stride).cast::<MemoryDescriptor>())
        };
        map.push_uefi(
            descriptor.memory_type,
            descriptor.physical_start,
            descriptor.number_of_pages,
        );
    }
    map
}

fn failure(stage: BootStage, code: usize) -> BootError {
    BootError {
        stage,
        status: (1usize << (usize::BITS - 1)) | code,
    }
}

// ---------------------------------------------------------------- image ASLR

/// What the relocation did. The fields live in the image's own data, so the
/// copy gets them written before it starts and the original keeps its own
/// (all zero, with `reason` set) when it did not move.
#[repr(C)]
pub struct Relocation {
    /// Where the image was before it moved (0 = it did not move).
    origin: AtomicUsize,
    relocations: AtomicU32,
    slots: AtomicU32,
    /// 0 = hardware random numbers, 1 = the timestamp counter only.
    entropy: AtomicU32,
    /// Why it did not move (0 = it did).
    reason: AtomicU32,
    freed: AtomicU32,
}

static RELOCATION: Relocation = Relocation {
    origin: AtomicUsize::new(0),
    relocations: AtomicU32::new(0),
    slots: AtomicU32::new(0),
    entropy: AtomicU32::new(0),
    reason: AtomicU32::new(0),
    freed: AtomicU32::new(0),
};

pub struct RelocationReport {
    pub moved: bool,
    pub origin: usize,
    pub relocations: u32,
    pub slots: u32,
    pub entropy: &'static str,
    pub reason: &'static str,
}

pub fn relocation_report() -> RelocationReport {
    let reason = RELOCATION.reason.load(Ordering::Relaxed);
    RelocationReport {
        moved: RELOCATION.origin.load(Ordering::Relaxed) != 0,
        origin: RELOCATION.origin.load(Ordering::Relaxed),
        relocations: RELOCATION.relocations.load(Ordering::Relaxed),
        slots: RELOCATION.slots.load(Ordering::Relaxed),
        entropy: if RELOCATION.entropy.load(Ordering::Relaxed) == 0 {
            "rdrand"
        } else {
            "tsc"
        },
        reason: match reason {
            0 => "none",
            1 => "no-boot-services",
            2 => "image-not-found",
            3 => "no-relocation-table",
            4 => "no-free-slot",
            5 => "allocation-failed",
            6 => "relocation-failed",
            7 => "memory-map-failed",
            _ => "unknown",
        },
    }
}

fn hardware_random() -> (u64, bool) {
    let cpuid = core::arch::x86_64::__cpuid(1);
    let mut mixed = 0u64;
    let mut hardware = false;
    if cpuid.ecx & (1 << 30) != 0 {
        for _ in 0..16 {
            let value: u64;
            let valid: u8;
            unsafe {
                core::arch::asm!("rdrand {}", "setc {}", out(reg) value, out(reg_byte) valid, options(nomem, nostack));
            }
            if valid != 0 {
                mixed ^= value;
                hardware = true;
                break;
            }
        }
    }
    let tsc = unsafe { core::arch::x86_64::_rdtsc() };
    // The counter is mixed in either way, so a weak generator still varies.
    (
        mixed ^ tsc.rotate_left(17).wrapping_mul(0x9e37_79b9_7f4a_7c15),
        hardware,
    )
}

/// The start of the loaded image whose entry point is `entry`, found by
/// walking back page by page to a PE header that says so.
pub fn image_base(entry: usize) -> Option<usize> {
    let mut page = entry & !0xfff;
    for _ in 0..(160 * 1024 * 1024 / 4096) {
        let header = unsafe { core::slice::from_raw_parts(page as *const u8, 4096) };
        if let Some(pe) = crate::aslr::parse(header)
            && page + pe.entry_rva == entry
        {
            return Some(page);
        }
        page = page.checked_sub(4096)?;
    }
    None
}

/// Copies the image to a random free place and re-applies its relocations
/// there. Returns the entry point inside the copy, or `None` (and the reason
/// in the report) if the image stays where the firmware put it. Must run
/// before anything else has touched the image data, on the old stack.
///
/// # Safety
/// Firmware boot services must still be running.
pub unsafe fn relocate(system_table: *mut SystemTable, entry: usize) -> Option<usize> {
    if RELOCATION.origin.load(Ordering::Relaxed) != 0 {
        return None;
    }
    let fail = |reason: u32| {
        RELOCATION.reason.store(reason, Ordering::Relaxed);
        None
    };
    if system_table.is_null()
        || unsafe { (*system_table).header.signature } != SYSTEM_TABLE_SIGNATURE
    {
        return fail(1);
    }
    let services = unsafe { (*system_table).boot_services };
    if services.is_null() {
        return fail(1);
    }
    let Some(base) = image_base(entry) else {
        return fail(2);
    };
    let Some(pe) =
        crate::aslr::parse(unsafe { core::slice::from_raw_parts(base as *const u8, 4096) })
    else {
        return fail(2);
    };
    if pe.reloc_rva == 0 || pe.reloc_size == 0 {
        return fail(3);
    }
    let size = pe.size_of_image.next_multiple_of(4096);

    let mut map_size = MAP_CAPACITY;
    let mut map_key = 0usize;
    let mut descriptor_size = 0usize;
    let mut descriptor_version = 0u32;
    let map_pointer = MEMORY_MAP.0.get().cast::<MemoryDescriptor>();
    let status = unsafe {
        ((*services).get_memory_map)(
            &mut map_size,
            map_pointer,
            &mut map_key,
            &mut descriptor_size,
            &mut descriptor_version,
        )
    };
    if status != SUCCESS || descriptor_size < core::mem::size_of::<MemoryDescriptor>() {
        return fail(7);
    }
    // Free regions at or above 16 MiB; low memory is left alone.
    const FLOOR: u64 = 16 * 1024 * 1024;
    let mut regions = [(0u64, 0u64); 192];
    let mut count = 0;
    let map_base = MEMORY_MAP.0.get().cast::<u8>();
    for index in 0..map_size / descriptor_size {
        let descriptor = unsafe {
            core::ptr::read_unaligned(
                map_base
                    .add(index * descriptor_size)
                    .cast::<MemoryDescriptor>(),
            )
        };
        if descriptor.memory_type == 7 && count < regions.len() {
            let start = descriptor.physical_start.max(FLOOR);
            let end = descriptor.physical_start + descriptor.number_of_pages * 4096;
            if end > start {
                regions[count] = (start, end);
                count += 1;
            }
        }
    }
    let (mut random, hardware) = hardware_random();
    RELOCATION
        .entropy
        .store(u32::from(!hardware), Ordering::Relaxed);
    let mut chosen = None;
    let mut slots = 0u64;
    for _ in 0..8 {
        let Some((start, total)) =
            crate::aslr::pick_slot(regions[..count].iter().copied(), size as u64, random)
        else {
            return fail(4);
        };
        slots = total;
        let mut address = start;
        // Address allocation, as loader code (so it stays executable).
        let status =
            unsafe { ((*services).allocate_pages)(2, 1, size / 4096, &mut address as *mut u64) };
        if status == SUCCESS && address == start {
            chosen = Some(start as usize);
            break;
        }
        random = random
            .rotate_left(23)
            .wrapping_mul(0x2545_f491_4f6c_dd1d)
            .wrapping_add(1);
    }
    let Some(new_base) = chosen else {
        return fail(5);
    };

    // Copy what the image holds (headers and each section's data); the rest,
    // including the large zeroed working memory, is zero in the new place.
    unsafe {
        core::ptr::write_bytes(new_base as *mut u8, 0, size);
        core::ptr::copy_nonoverlapping(base as *const u8, new_base as *mut u8, pe.size_of_headers);
        for section in &pe.sections[..pe.section_count] {
            let bytes = section.raw_size.min(section.virtual_size);
            core::ptr::copy_nonoverlapping(
                (base + section.rva) as *const u8,
                (new_base + section.rva) as *mut u8,
                bytes,
            );
        }
    }
    let delta = (new_base as u64).wrapping_sub(base as u64);
    let applied = match unsafe {
        crate::aslr::apply_relocations(
            new_base as *mut u8,
            pe.size_of_image,
            pe.reloc_rva,
            pe.reloc_size,
            delta,
        )
    } {
        Ok(applied) => applied,
        Err(_) => {
            let free_pages: unsafe extern "efiapi" fn(u64, usize) -> Status =
                unsafe { core::mem::transmute((*services).free_pages) };
            unsafe { free_pages(new_base as u64, size / 4096) };
            return fail(6);
        }
    };
    // The copy starts with its own record filled in.
    let record = (core::ptr::addr_of!(RELOCATION) as usize - base + new_base) as *const Relocation;
    unsafe {
        (*record).origin.store(base, Ordering::Relaxed);
        (*record).relocations.store(applied, Ordering::Relaxed);
        (*record)
            .slots
            .store(slots.min(u64::from(u32::MAX)) as u32, Ordering::Relaxed);
        (*record)
            .entropy
            .store(u32::from(!hardware), Ordering::Relaxed);
        (*record).reason.store(0, Ordering::Relaxed);
    }
    Some(new_base + pe.entry_rva)
}

/// In the copy: hands the firmware the pages of the original image back.
///
/// # Safety
/// Firmware boot services must still be running.
pub unsafe fn release_origin(system_table: *mut SystemTable) {
    let origin = RELOCATION.origin.load(Ordering::Relaxed);
    if origin == 0 || RELOCATION.freed.swap(1, Ordering::Relaxed) != 0 || system_table.is_null() {
        return;
    }
    let services = unsafe { (*system_table).boot_services };
    if services.is_null() {
        return;
    }
    let Some(base) = image_base(release_origin as *const () as usize) else {
        return;
    };
    let Some(pe) =
        crate::aslr::parse(unsafe { core::slice::from_raw_parts(base as *const u8, 4096) })
    else {
        return;
    };
    let free_pages: unsafe extern "efiapi" fn(u64, usize) -> Status =
        unsafe { core::mem::transmute((*services).free_pages) };
    unsafe { free_pages(origin as u64, pe.size_of_image.div_ceil(4096)) };
}
