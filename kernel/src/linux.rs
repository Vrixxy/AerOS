//! Linux drivers in AerOS. The C side (kernel/linux: Linux-shaped headers and
//! the functions behind them, plus the drivers) is built by build.rs into one
//! position-independent ELF module. It is loaded here at boot and handed
//! `LinuxOps`, the few AerOS services everything on the C side is built on.

use core::alloc::Layout;
use core::cell::UnsafeCell;

use crate::memory::FrameAllocator;
use crate::pci::{PciDevice, PciInventory};
use crate::serial;

static MODULE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/aeros-linux.elf"));

const OPS_VERSION: usize = 6;
const MAX_DMA_BUFFERS: usize = 256;
const LOG_BYTES: usize = 16 * 1024;
const MAX_DEVICES: usize = 64;

#[repr(C)]
struct LinuxOps {
    version: usize,
    log: extern "sysv64" fn(*const u8, usize),
    alloc: extern "sysv64" fn(usize, usize) -> *mut u8,
    free: extern "sysv64" fn(*mut u8),
    nanoseconds: extern "sysv64" fn() -> u64,
    pci_count: extern "sysv64" fn() -> usize,
    pci_get: extern "sysv64" fn(usize, *mut PciInfo) -> i32,
    pci_read: extern "sysv64" fn(u32, u32, u32) -> u32,
    pci_write: extern "sysv64" fn(u32, u32, u32, u32),
    map_mmio: extern "sysv64" fn(u64, u64) -> *mut u8,
    virt_to_phys: extern "sysv64" fn(*const u8) -> u64,
    acpi_table: extern "sysv64" fn(*const u8, *mut usize) -> *const u8,
    framebuffer: extern "sysv64" fn(*mut u64, *mut u64) -> i32,
    dma_alloc: extern "sysv64" fn(usize, *mut u64) -> *mut u8,
    dma_free: extern "sysv64" fn(*mut u8),
    dma_allow: extern "sysv64" fn(u64, usize) -> i32,
    firmware: extern "sysv64" fn(*const u8, *mut usize) -> *const u8,
    page_alloc: extern "sysv64" fn(u64) -> u64,
    page_free: extern "sysv64" fn(u64, u64),
    options: extern "sysv64" fn() -> u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PciInfo {
    bus: u8,
    slot: u8,
    function: u8,
    revision: u8,
    vendor: u16,
    device: u16,
    subsystem_vendor: u16,
    subsystem_device: u16,
    class_code: u32,
    irq_line: u32,
    bars: [u64; 6],
    bar_sizes: [u64; 6],
    bar_flags: [u32; 6],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Report {
    pub initcalls: u32,
    pub initcall_failures: u32,
    pub drivers: u32,
    pub bound: u32,
    poll: Option<extern "sysv64" fn()>,
}

static POLL: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Runs the Linux side's due timers, queued work and interrupt handlers;
/// the desktop loop calls this every pass.
pub fn poll() {
    let address = POLL.load(core::sync::atomic::Ordering::Acquire);
    if address != 0 {
        // SAFETY: the module's `aeros_linux_poll`, set by `initialize`.
        let poll: extern "sysv64" fn() = unsafe { core::mem::transmute(address) };
        poll();
    }
}

struct Devices(UnsafeCell<([PciInfo; MAX_DEVICES], usize)>);

// SAFETY: filled once at boot, before the module runs, and only read after.
unsafe impl Sync for Devices {}

struct Log(UnsafeCell<([u8; LOG_BYTES], usize)>);

// SAFETY: the module only runs on the boot processor, during `initialize`.
unsafe impl Sync for Log {}

/// Everything the module printed, saved as LINUX.LOG on the boot partition
/// so a run on real hardware can be read back.
static LOG: Log = Log(UnsafeCell::new(([0; LOG_BYTES], 0)));

/// The firmware framebuffer: address and size in bytes.
static SCREEN: crate::sync::TicketLock<(u64, u64)> = crate::sync::TicketLock::new((0, 0));

static ACPI: core::sync::atomic::AtomicPtr<crate::acpi::AcpiInfo> =
    core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

fn keep(bytes: &[u8]) {
    // SAFETY: see `Log`.
    let (buffer, length) = unsafe { &mut *LOG.0.get() };
    let take = bytes.len().min(LOG_BYTES - *length);
    buffer[*length..*length + take].copy_from_slice(&bytes[..take]);
    *length += take;
}

static DEVICES: Devices = Devices(UnsafeCell::new(([PciInfo::ZERO; MAX_DEVICES], 0)));

impl PciInfo {
    const ZERO: Self = Self {
        bus: 0,
        slot: 0,
        function: 0,
        revision: 0,
        vendor: 0,
        device: 0,
        subsystem_vendor: 0,
        subsystem_device: 0,
        class_code: 0,
        irq_line: 0,
        bars: [0; 6],
        bar_sizes: [0; 6],
        bar_flags: [0; 6],
    };
}

extern "sysv64" fn log(text: *const u8, length: usize) {
    // SAFETY: the module passes a buffer of `length` bytes.
    let bytes = unsafe { core::slice::from_raw_parts(text, length) };
    let line = core::str::from_utf8(bytes)
        .unwrap_or("<not utf-8>")
        .trim_end();
    serial::text("linux: ");
    serial::text(line);
    serial::text("\n");
    keep(line.as_bytes());
    keep(b"\r\n");
}

extern "sysv64" fn alloc(size: usize, align: usize) -> *mut u8 {
    Layout::from_size_align(size.max(1), align.max(8))
        .ok()
        .and_then(|layout| crate::heap::HEAP.allocate(layout))
        .map_or(core::ptr::null_mut(), |pointer| pointer.as_ptr())
}

extern "sysv64" fn free(pointer: *mut u8) {
    if let Some(pointer) = core::ptr::NonNull::new(pointer) {
        let _ = crate::heap::HEAP.deallocate(pointer);
    }
}

extern "sysv64" fn nanoseconds() -> u64 {
    crate::time::monotonic_nanoseconds()
}

extern "sysv64" fn pci_count() -> usize {
    // SAFETY: see `Devices`.
    unsafe { (*DEVICES.0.get()).1 }
}

extern "sysv64" fn pci_get(index: usize, info: *mut PciInfo) -> i32 {
    // SAFETY: see `Devices`; the module passes room for one `PciInfo`.
    let (devices, count) = unsafe { &*DEVICES.0.get() };
    if index >= *count || info.is_null() {
        return 0;
    }
    unsafe { *info = devices[index] };
    1
}

fn split(bdf: u32) -> (u8, u8, u8) {
    ((bdf >> 8) as u8, ((bdf >> 3) & 0x1f) as u8, (bdf & 7) as u8)
}

/// PCI Express memory-mapped config space (the ACPI MCFG table): base,
/// first and last bus. Reaches all 4 KiB of a function's config space,
/// where amdgpu finds its extended capabilities.
static ECAM: crate::sync::TicketLock<Option<(u64, u8, u8)>> = crate::sync::TicketLock::new(None);

fn find_ecam(acpi: &crate::acpi::AcpiInfo) -> Option<(u64, u8, u8)> {
    // SAFETY: `find_table` checks the table; entries are read within it.
    let (address, length) = unsafe { crate::acpi::find_table(acpi, *b"MCFG") }?;
    let table = unsafe { core::slice::from_raw_parts(address as usize as *const u8, length) };
    let mut at = 44;
    while at + 16 <= length {
        let base = read::<u64>(table, at)?;
        let segment = read::<u16>(table, at + 8)?;
        let (first, last) = (table[at + 10], table[at + 11]);
        let size = (u64::from(last.saturating_sub(first)) + 1) << 20;
        if segment == 0 && crate::arch::paging::kernel_range_mapped(base, size) {
            return Some((base, first, last));
        }
        at += 16;
    }
    None
}

fn ecam_address(bdf: u32, offset: u32, bytes: u32) -> Option<*mut u8> {
    let (base, first, last) = (*ECAM.lock())?;
    let bus = (bdf >> 8) as u8;
    if bus < first || bus > last || offset + bytes > 4096 || !offset.is_multiple_of(bytes) {
        return None;
    }
    let address = base + (((bus - first) as u64) << 20) + (((bdf & 0xff) as u64) << 12);
    Some((address + offset as u64) as usize as *mut u8)
}

extern "sysv64" fn pci_read(bdf: u32, offset: u32, bytes: u32) -> u32 {
    if let Some(address) = ecam_address(bdf, offset, bytes) {
        // SAFETY: inside the mapped MCFG window, aligned to its size.
        return unsafe {
            match bytes {
                1 => core::ptr::read_volatile(address) as u32,
                2 => core::ptr::read_volatile(address as *const u16) as u32,
                _ => core::ptr::read_volatile(address as *const u32),
            }
        };
    }
    if offset >= 0x100 {
        return u32::MAX;
    }
    let (bus, slot, function) = split(bdf);
    crate::pci::config_read(bus, slot, function, offset as u16, bytes as u8) as u32
}

extern "sysv64" fn pci_write(bdf: u32, offset: u32, bytes: u32, value: u32) {
    if let Some(address) = ecam_address(bdf, offset, bytes) {
        // SAFETY: as in `pci_read`.
        unsafe {
            match bytes {
                1 => core::ptr::write_volatile(address, value as u8),
                2 => core::ptr::write_volatile(address as *mut u16, value as u16),
                _ => core::ptr::write_volatile(address as *mut u32, value),
            }
        }
        return;
    }
    if offset >= 0x100 {
        return;
    }
    let (bus, slot, function) = split(bdf);
    crate::pci::config_write(
        bus,
        slot,
        function,
        offset as u16,
        bytes as u8,
        value as u64,
    );
}

/// Device memory is identity-mapped, like the rest of the kernel's view.
extern "sysv64" fn map_mmio(physical: u64, _length: u64) -> *mut u8 {
    physical as usize as *mut u8
}

extern "sysv64" fn virt_to_phys(address: *const u8) -> u64 {
    address as u64
}

extern "sysv64" fn acpi_table(signature: *const u8, length: *mut usize) -> *const u8 {
    let info = ACPI.load(core::sync::atomic::Ordering::Acquire);
    if info.is_null() || signature.is_null() {
        return core::ptr::null();
    }
    // SAFETY: the module passes a four-byte signature; `info` is the boot
    // ACPI description, which lives as long as the kernel.
    let name = unsafe { core::ptr::read_unaligned(signature as *const [u8; 4]) };
    match unsafe { crate::acpi::find_table(&*info, name) } {
        Some((address, size)) => {
            if !length.is_null() {
                unsafe { *length = size };
            }
            address as usize as *const u8
        }
        None => core::ptr::null(),
    }
}

extern "sysv64" fn framebuffer(base: *mut u64, size: *mut u64) -> i32 {
    let (address, length) = *SCREEN.lock();
    if address == 0 {
        return 0;
    }
    // SAFETY: the module passes room for both values.
    unsafe {
        *base = address;
        *size = length;
    }
    1
}

/// Memory handed out by `dma_alloc_coherent`, kept until it is freed.
static DMA: crate::sync::TicketLock<[Option<crate::memory::PageBuffer>; MAX_DMA_BUFFERS]> =
    crate::sync::TicketLock::new([const { None }; MAX_DMA_BUFFERS]);

extern "sysv64" fn dma_alloc(size: usize, physical: *mut u64) -> *mut u8 {
    let mut buffers = DMA.lock();
    let Some(slot) = buffers.iter_mut().find(|slot| slot.is_none()) else {
        return core::ptr::null_mut();
    };
    let Some(buffer) = crate::memory::PageBuffer::new(size) else {
        return core::ptr::null_mut();
    };
    let address = buffer.physical();
    if !crate::iommu::allow(address, size.div_ceil(4096) as u64) {
        return core::ptr::null_mut();
    }
    *slot = Some(buffer);
    if !physical.is_null() {
        // SAFETY: the module passes room for the address.
        unsafe { *physical = address };
    }
    address as usize as *mut u8
}

extern "sysv64" fn dma_free(cpu: *mut u8) {
    let mut buffers = DMA.lock();
    if let Some(slot) = buffers.iter_mut().find(|slot| {
        slot.as_ref()
            .is_some_and(|buffer| buffer.physical() == cpu as u64)
    }) {
        *slot = None;
    }
}

extern "sysv64" fn dma_allow(physical: u64, size: usize) -> i32 {
    let first = physical & !0xfff;
    let pages = (physical + size as u64 - first).div_ceil(4096);
    crate::iommu::allow(first, pages) as i32
}

/// Zeroed, device-reachable pages for `alloc_pages`, untracked here: the
/// token is the physical address with bit 0 saying which frame pool it
/// came from, and the module hands it back with the count to free it.
extern "sysv64" fn page_alloc(pages: u64) -> u64 {
    let pages = pages.max(1);
    let Some(buffer) = crate::memory::PageBuffer::new((pages * 4096) as usize) else {
        return 0;
    };
    if !crate::iommu::allow(buffer.physical(), pages) {
        return 0;
    }
    let (address, _, primary) = buffer.into_raw();
    address | primary as u64
}

extern "sysv64" fn page_free(token: u64, pages: u64) {
    // SAFETY: the token and count are what page_alloc returned and was given.
    drop(unsafe {
        crate::memory::PageBuffer::from_raw(token & !0xfff, pages.max(1), token & 1 != 0)
    });
}

/// FIRMWARE.PAK from the boot partition, read on the first request.
static FIRMWARE: crate::sync::TicketLock<(bool, Option<crate::memory::PageBuffer>)> =
    crate::sync::TicketLock::new((false, None));

extern "sysv64" fn firmware(name: *const u8, size: *mut usize) -> *const u8 {
    let mut pack = FIRMWARE.lock();
    if !pack.0 {
        pack.0 = true;
        pack.1 = crate::fat::root_file_size(b"FIRMWAREPAK").and_then(|length| {
            let mut buffer = crate::memory::PageBuffer::new(length as usize)?;
            crate::fat::read_root_file(b"FIRMWAREPAK", buffer.as_mut_slice())?;
            Some(buffer)
        });
    }
    let Some(buffer) = pack.1.as_ref() else {
        return core::ptr::null();
    };
    // SAFETY: the module passes a NUL-terminated name.
    let wanted = unsafe { core::ffi::CStr::from_ptr(name as *const core::ffi::c_char) }.to_bytes();
    let bytes = buffer.as_slice();
    if bytes.get(..4) != Some(b"AEFW") {
        return core::ptr::null();
    }
    let count = read::<u32>(bytes, 4).unwrap_or(0) as usize;
    for index in 0..count {
        let at = 8 + 64 * index;
        let Some(entry) = bytes.get(at..at + 64) else {
            break;
        };
        let length = entry[..56].iter().position(|&byte| byte == 0).unwrap_or(56);
        if &entry[..length] != wanted {
            continue;
        }
        let offset = read::<u32>(entry, 56).unwrap_or(0) as usize;
        let file_size = read::<u32>(entry, 60).unwrap_or(0) as usize;
        if offset
            .checked_add(file_size)
            .is_none_or(|end| end > bytes.len())
        {
            return core::ptr::null();
        }
        if !size.is_null() {
            // SAFETY: the module passes room for the size.
            unsafe { *size = file_size };
        }
        return bytes[offset..].as_ptr();
    }
    core::ptr::null()
}

/// Bit 0: amdgpu may drive the GPU (an `AMDGPU.ON` file on the boot partition).
extern "sysv64" fn options() -> u64 {
    let mut files = [crate::fat::FatFileEntry::EMPTY; 64];
    let count = crate::fat::list_root_files(&mut files);
    u64::from(
        files[..count]
            .iter()
            .any(|file| &file.name == b"AMDGPU  ON "),
    )
}

static OPS: LinuxOps = LinuxOps {
    version: OPS_VERSION,
    log,
    alloc,
    free,
    nanoseconds,
    pci_count,
    pci_get,
    pci_read,
    pci_write,
    map_mmio,
    virt_to_phys,
    acpi_table,
    framebuffer,
    dma_alloc,
    dma_free,
    dma_allow,
    firmware,
    page_alloc,
    page_free,
    options,
};

/// A BAR's size, found the way the PCI spec says (write all ones, read the
/// mask back) with decoding switched off meanwhile.
fn bar_size(device: &PciDevice, index: usize) -> (u64, u32) {
    let (bus, slot, function) = (device.bus, device.slot, device.function);
    let offset = 0x10 + 4 * index as u16;
    let raw = device.bars[index];
    let io = raw & 1 != 0;
    let wide = !io && raw & 0b110 == 0b100;
    let flags = io as u32 | (wide as u32) << 1 | ((!io && raw & 8 != 0) as u32) << 2;
    let command = crate::pci::config_read(bus, slot, function, 4, 2);
    crate::pci::config_write(bus, slot, function, 4, 2, command & !3);
    crate::pci::config_write(bus, slot, function, offset, 4, 0xffff_ffff);
    let low = crate::pci::config_read(bus, slot, function, offset, 4);
    crate::pci::config_write(bus, slot, function, offset, 4, raw as u64);
    let mut mask = low & if io { !3 } else { !0xf };
    if wide {
        let high_raw = device.bars[index + 1] as u64;
        crate::pci::config_write(bus, slot, function, offset + 4, 4, 0xffff_ffff);
        let high = crate::pci::config_read(bus, slot, function, offset + 4, 4);
        crate::pci::config_write(bus, slot, function, offset + 4, 4, high_raw);
        mask |= high << 32;
    } else {
        mask |= 0xffff_ffff_0000_0000;
    }
    crate::pci::config_write(bus, slot, function, 4, 2, command);
    let size = if mask & 0xffff_ffff == 0 {
        0
    } else {
        (!mask).wrapping_add(1) & if wide { u64::MAX } else { 0xffff_ffff }
    };
    (size, flags)
}

fn collect(pci: &PciInventory) {
    // SAFETY: see `Devices`; the module isn't running yet.
    let (devices, count) = unsafe { &mut *DEVICES.0.get() };
    *count = 0;
    for device in pci.devices() {
        if *count == MAX_DEVICES || device.header_type & 0x7f != 0 {
            continue;
        }
        let subsystem = crate::pci::config_read(device.bus, device.slot, device.function, 0x2c, 4);
        let mut info = PciInfo {
            bus: device.bus,
            slot: device.slot,
            function: device.function,
            revision: device.revision,
            vendor: device.vendor,
            device: device.device,
            subsystem_vendor: subsystem as u16,
            subsystem_device: (subsystem >> 16) as u16,
            class_code: (device.class as u32) << 16
                | (device.subclass as u32) << 8
                | device.programming_interface as u32,
            irq_line: device.interrupt_line as u32,
            ..PciInfo::ZERO
        };
        let mut index = 0;
        while index < 6 {
            let raw = device.bars[index];
            let (size, flags) = if raw == 0 {
                (0, 0)
            } else {
                bar_size(device, index)
            };
            info.bars[index] = if flags & 1 != 0 {
                (raw & !3) as u64
            } else {
                crate::pci::bar_address(device, index as u8, 0).unwrap_or(0)
            };
            info.bar_sizes[index] = size;
            info.bar_flags[index] = flags;
            index += if flags & 2 != 0 { 2 } else { 1 };
        }
        devices[*count] = info;
        *count += 1;
    }
}

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const DT_RELA: u64 = 7;
const DT_RELASZ: u64 = 8;
const DT_RELAENT: u64 = 9;
const R_X86_64_RELATIVE: u32 = 8;

fn read<T: Copy>(bytes: &[u8], offset: usize) -> Option<T> {
    let end = offset.checked_add(core::mem::size_of::<T>())?;
    let slice = bytes.get(offset..end)?;
    // SAFETY: `slice` holds `size_of::<T>()` bytes; T is plain data.
    Some(unsafe { core::ptr::read_unaligned(slice.as_ptr() as *const T) })
}

/// Copies the module into fresh memory, applies its relocations and makes
/// it runnable. Returns the load base and the entry point.
fn place(frames: &mut FrameAllocator) -> Option<(u64, u64)> {
    let image = MODULE;
    if image.get(..4)? != b"\x7fELF" || read::<u16>(image, 16)? != 3 {
        return None;
    }
    let entry = read::<u64>(image, 24)?;
    let header_offset = read::<u64>(image, 32)? as usize;
    let header_count = read::<u16>(image, 56)? as usize;
    let mut span = 0u64;
    for index in 0..header_count {
        let at = header_offset + 56 * index;
        if read::<u32>(image, at)? == PT_LOAD {
            let end = read::<u64>(image, at + 16)?.checked_add(read::<u64>(image, at + 40)?)?;
            span = span.max(end);
        }
    }
    let pages = span.div_ceil(4096);
    // Its own 2 MiB-aligned block, so making it executable doesn't open up
    // anything else that shares a large page with it.
    let base = frames
        .allocate_contiguous(pages.next_multiple_of(512), 512)?
        .address();
    // SAFETY: fresh, identity-mapped frames that nothing else uses.
    let memory = unsafe { core::slice::from_raw_parts_mut(base as *mut u8, span as usize) };
    memory.fill(0);
    let mut dynamic = None;
    for index in 0..header_count {
        let at = header_offset + 56 * index;
        let kind = read::<u32>(image, at)?;
        let offset = read::<u64>(image, at + 8)? as usize;
        let address = read::<u64>(image, at + 16)? as usize;
        let file_size = read::<u64>(image, at + 32)? as usize;
        if kind == PT_LOAD {
            memory
                .get_mut(address..address + file_size)?
                .copy_from_slice(image.get(offset..offset + file_size)?);
        } else if kind == PT_DYNAMIC {
            dynamic = Some(address);
        }
    }
    let (mut rela, mut rela_size, mut rela_entry) = (0usize, 0usize, 24usize);
    if let Some(mut at) = dynamic {
        loop {
            let tag = read::<u64>(memory, at)?;
            let value = read::<u64>(memory, at + 8)? as usize;
            match tag {
                0 => break,
                DT_RELA => rela = value,
                DT_RELASZ => rela_size = value,
                DT_RELAENT => rela_entry = value,
                _ => {}
            }
            at += 16;
        }
    }
    let mut at = rela;
    while at < rela + rela_size {
        let target = read::<u64>(memory, at)? as usize;
        let kind = read::<u64>(memory, at + 8)? as u32;
        let addend = read::<i64>(memory, at + 16)?;
        if kind != R_X86_64_RELATIVE {
            serial::format(format_args!(
                "AEROS_LINUX_DRIVERS unsupported relocation {kind}\n"
            ));
            return None;
        }
        let value = (base as i64).wrapping_add(addend) as u64;
        memory
            .get_mut(target..target + 8)?
            .copy_from_slice(&value.to_le_bytes());
        at += rela_entry;
    }
    match crate::arch::paging::make_kernel_executable(frames, base, base + span) {
        None => {
            serial::line("AEROS_LINUX_DRIVERS executable=false");
            return None;
        }
        Some(opened) if opened > 2 * 1024 * 1024 => serial::format(format_args!(
            "AEROS_LINUX_DRIVERS opened_page={opened:#x}
"
        )),
        Some(_) => {}
    }
    Some((base, base + entry))
}

/// Loads the Linux driver module and runs its drivers' init functions,
/// which bind to the PCI devices they support.
pub fn initialize(
    acpi: &crate::acpi::AcpiInfo,
    pci: &PciInventory,
    frames: &mut FrameAllocator,
    screen: crate::framebuffer::FrameBufferInfo,
) -> Option<Report> {
    *SCREEN.lock() = (screen.address as u64, screen.size as u64);
    ACPI.store(
        acpi as *const _ as *mut crate::acpi::AcpiInfo,
        core::sync::atomic::Ordering::Release,
    );
    *ECAM.lock() = find_ecam(acpi);
    collect(pci);
    let Some((base, entry)) = place(frames) else {
        serial::line("AEROS_LINUX_DRIVERS loaded=false");
        return None;
    };
    // SAFETY: `entry` is the module's `aeros_linux_entry`, now relocated
    // and executable; it follows the System V calling convention.
    let entry: extern "sysv64" fn(*const LinuxOps, *mut Report) -> i32 =
        unsafe { core::mem::transmute(entry as usize) };
    let mut report = Report::default();
    let result = entry(&OPS, &mut report);
    serial::format(format_args!(
        "AEROS_LINUX_DRIVERS loaded=true base={base:#x} size={} ecam={} result={result} initcalls={} failed={} drivers={} bound={}\n",
        MODULE.len(),
        ECAM.lock().is_some(),
        report.initcalls,
        report.initcall_failures,
        report.drivers,
        report.bound
    ));
    if result == 0
        && let Some(poll) = report.poll
    {
        POLL.store(poll as usize, core::sync::atomic::Ordering::Release);
    }
    // SAFETY: see `Log`; the module has finished.
    let (buffer, length) = unsafe { &*LOG.0.get() };
    if *length != 0 {
        let _ = crate::fat::write_root_file(b"LINUX   LOG", &buffer[..*length]);
    }
    (result == 0).then_some(report)
}
