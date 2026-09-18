use core::cell::UnsafeCell;
use core::ffi::c_void;

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
