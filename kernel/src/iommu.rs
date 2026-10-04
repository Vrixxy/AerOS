//! AMD-Vi DMA remapping. Every PCI function the kernel found gets a device
//! table entry pointing at one shared domain whose page table maps just the
//! memory drivers set aside for DMA (identity-mapped) plus whatever a driver
//! lends for one transfer; every other function, and every other address, is
//! blocked and shows up in the event log. Interrupts pass through unmapped.
//!
//! The tables are built before the drivers start and kept up to date while
//! the unit is off; `enable` switches it on once the drivers are running and
//! falls back to leaving it off if the boot disk cannot be read afterwards.

use core::ptr::{read_volatile, write_volatile};

use crate::acpi::AcpiInfo;
use crate::memory::FrameAllocator;
use crate::pci::PciInventory;
use crate::sync::TicketLock;

const PAGE: u64 = 4096;
const POOL_PAGES: u64 = 192;
const DEVICES: usize = 0x1_0000;
const DEVICE_TABLE_PAGES: u64 = (DEVICES as u64 * 32) / PAGE;
const BUFFER_BYTES: u64 = PAGE;
const BUFFER_ENTRIES: u64 = BUFFER_BYTES / 16;
const DOMAIN: u64 = 1;
const MAX_UNITY: usize = 8;
const MAX_UNITY_BYTES: u64 = 256 << 20;

const DEVICE_TABLE_BASE: u64 = 0x00;
const COMMAND_BASE: u64 = 0x08;
const EVENT_BASE: u64 = 0x10;
const CONTROL: u64 = 0x18;
const COMMAND_HEAD: u64 = 0x2000;
const COMMAND_TAIL: u64 = 0x2008;
const EVENT_HEAD: u64 = 0x2010;
const EVENT_TAIL: u64 = 0x2018;
const STATUS: u64 = 0x2020;
const STATUS_EVENT_OVERFLOW: u64 = 1;

const CONTROL_ENABLE: u64 = 1;
const CONTROL_EVENT_LOG: u64 = 1 << 2;
const CONTROL_COMMAND: u64 = 1 << 12;
const LENGTH_256_ENTRIES: u64 = 8 << 56;

const DTE_VALID: u64 = 1;
const DTE_TRANSLATION: u64 = 1 << 1;
const DTE_MODE_4_LEVELS: u64 = 4 << 9;
const DTE_READ: u64 = 1 << 61;
const DTE_WRITE: u64 = 1 << 62;
const DTE_INTERRUPTS_PASS: u64 = 1 << 60;
const ADDRESS_MASK: u64 = 0x000f_ffff_ffff_f000;

const PTE_PRESENT: u64 = 1;
const PTE_READ: u64 = 1 << 61;
const PTE_WRITE: u64 = 1 << 62;
const REFERENCE_SHIFT: u32 = 52;
const REFERENCE_MASK: u64 = 0x7f << REFERENCE_SHIFT;

const COMMAND_COMPLETION_WAIT: u32 = 1;
const COMMAND_INVALIDATE_DEVICE: u32 = 2;
const COMMAND_INVALIDATE_PAGES: u32 = 3;
const EVENT_IO_PAGE_FAULT: u64 = 2;

#[derive(Clone, Copy, Default)]
pub struct Fault {
    pub device: u16,
    pub address: u64,
    pub raw: [u64; 2],
}

#[derive(Clone, Copy)]
pub struct Report {
    pub present: bool,
    pub base: u64,
    pub devices: usize,
    pub unity_ranges: usize,
}

impl Report {
    const ABSENT: Self = Self {
        present: false,
        base: 0,
        devices: 0,
        unity_ranges: 0,
    };
}

struct Iommu {
    present: bool,
    enabled: bool,
    mmio: u64,
    root: u64,
    empty_root: u64,
    device_table: u64,
    command: u64,
    event: u64,
    semaphore: u64,
    command_tail: u64,
    pool_next: u64,
    pool_end: u64,
    mapped_pages: u64,
    faults: u64,
    other_events: u64,
    last_fault: Fault,
    scanned: [u16; 256],
    scanned_count: usize,
}

impl Iommu {
    const ABSENT: Self = Self {
        present: false,
        enabled: false,
        mmio: 0,
        root: 0,
        empty_root: 0,
        device_table: 0,
        command: 0,
        event: 0,
        semaphore: 0,
        command_tail: 0,
        pool_next: 0,
        pool_end: 0,
        mapped_pages: 0,
        faults: 0,
        other_events: 0,
        last_fault: Fault {
            device: 0,
            address: 0,
            raw: [0; 2],
        },
        scanned: [0; 256],
        scanned_count: 0,
    };

    fn read(&self, register: u64) -> u64 {
        // SAFETY: `mmio` is the register block the firmware table named.
        unsafe { read_volatile((self.mmio + register) as usize as *const u64) }
    }

    fn write(&self, register: u64, value: u64) {
        // SAFETY: as above.
        unsafe { write_volatile((self.mmio + register) as usize as *mut u64, value) };
    }

    fn take_node(&mut self) -> Option<u64> {
        if self.pool_next >= self.pool_end {
            return None;
        }
        let node = self.pool_next;
        self.pool_next += PAGE;
        // SAFETY: a page of the pool reserved at start-up, identity-mapped.
        unsafe { core::ptr::write_bytes(node as usize as *mut u8, 0, PAGE as usize) };
        Some(node)
    }

    /// The leaf entry for `iova`, creating the tables on the way when asked to.
    fn leaf(&mut self, iova: u64, create: bool) -> Option<*mut u64> {
        let mut table = self.root;
        for level in (2..=4u64).rev() {
            let index = (iova >> (12 + 9 * (level - 1))) & 0x1ff;
            let slot = (table as usize as *mut u64).wrapping_add(index as usize);
            // SAFETY: `table` is a node of this domain's page table.
            let entry = unsafe { read_volatile(slot) };
            if entry & PTE_PRESENT == 0 {
                if !create {
                    return None;
                }
                let node = self.take_node()?;
                let next = node | PTE_PRESENT | PTE_READ | PTE_WRITE | ((level - 1) << 9);
                // SAFETY: as above.
                unsafe { write_volatile(slot, next) };
                table = node;
            } else {
                table = entry & ADDRESS_MASK;
            }
        }
        Some((table as usize as *mut u64).wrapping_add(((iova >> 12) & 0x1ff) as usize))
    }

    fn map(&mut self, iova: u64, physical: u64, pages: u64) -> bool {
        for done in 0..pages {
            let at = iova + done * PAGE;
            let target = physical + done * PAGE;
            let Some(slot) = self.leaf(at, true) else {
                self.unmap(iova, done);
                return false;
            };
            // SAFETY: a leaf slot of this domain's page table.
            let entry = unsafe { read_volatile(slot) };
            if entry & PTE_PRESENT != 0 {
                let references = (entry & REFERENCE_MASK) >> REFERENCE_SHIFT;
                if entry & ADDRESS_MASK != target || references == 0x7f {
                    self.unmap(iova, done);
                    return false;
                }
                let bumped = (entry & !REFERENCE_MASK) | ((references + 1) << REFERENCE_SHIFT);
                // SAFETY: as above.
                unsafe { write_volatile(slot, bumped) };
            } else {
                let fresh = target | PTE_PRESENT | PTE_READ | PTE_WRITE | (1 << REFERENCE_SHIFT);
                // SAFETY: as above.
                unsafe { write_volatile(slot, fresh) };
                self.mapped_pages += 1;
            }
        }
        true
    }

    fn unmap(&mut self, iova: u64, pages: u64) {
        let mut removed = false;
        for done in 0..pages {
            let Some(slot) = self.leaf(iova + done * PAGE, false) else {
                continue;
            };
            // SAFETY: a leaf slot of this domain's page table.
            let entry = unsafe { read_volatile(slot) };
            if entry & PTE_PRESENT == 0 {
                continue;
            }
            let references = (entry & REFERENCE_MASK) >> REFERENCE_SHIFT;
            if references > 1 {
                let lowered = (entry & !REFERENCE_MASK) | ((references - 1) << REFERENCE_SHIFT);
                // SAFETY: as above.
                unsafe { write_volatile(slot, lowered) };
            } else {
                // SAFETY: as above.
                unsafe { write_volatile(slot, 0) };
                self.mapped_pages = self.mapped_pages.saturating_sub(1);
                removed = true;
            }
        }
        if removed {
            self.invalidate_domain();
        }
    }

    fn push(&mut self, words: [u32; 4]) {
        let slot = (self.command + self.command_tail * 16) as usize as *mut u32;
        for (index, word) in words.iter().enumerate() {
            // SAFETY: an entry of the command ring.
            unsafe { write_volatile(slot.add(index), *word) };
        }
        self.command_tail = (self.command_tail + 1) % BUFFER_ENTRIES;
        self.write(COMMAND_TAIL, self.command_tail * 16);
    }

    /// Waits until the unit has worked through everything queued so far.
    fn drain(&mut self) -> bool {
        let semaphore = self.semaphore as usize as *mut u64;
        // SAFETY: the scratch word the unit stores its completion into.
        unsafe { write_volatile(semaphore, 0) };
        let address = self.semaphore;
        self.push([
            (address as u32 & !7) | 1,
            ((address >> 32) as u32 & 0xf_ffff) | (COMMAND_COMPLETION_WAIT << 28),
            1,
            0,
        ]);
        for _ in 0..2_000_000 {
            // SAFETY: as above.
            if unsafe { read_volatile(semaphore) } == 1 {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    fn invalidate_domain(&mut self) {
        if !self.enabled {
            return;
        }
        for domain in [DOMAIN, 0] {
            self.push([
                0,
                domain as u32 | (COMMAND_INVALIDATE_PAGES << 28),
                0xffff_f001,
                0x7fff_ffff,
            ]);
        }
        self.drain();
    }

    fn invalidate_device(&mut self, device: u16) {
        self.push([device as u32, COMMAND_INVALIDATE_DEVICE << 28, 0, 0]);
    }

    fn collect_events(&mut self) {
        if !self.enabled {
            return;
        }
        let mut head = self.read(EVENT_HEAD);
        let tail = self.read(EVENT_TAIL);
        while head != tail {
            let entry = (self.event + head) as usize as *const u64;
            // SAFETY: an entry of the event ring.
            let raw = unsafe { [read_volatile(entry), read_volatile(entry.add(1))] };
            if raw[0] >> 60 == EVENT_IO_PAGE_FAULT {
                self.faults += 1;
                self.last_fault = Fault {
                    device: raw[0] as u16,
                    address: raw[1],
                    raw,
                };
            } else {
                self.other_events += 1;
            }
            head = (head + 16) % BUFFER_BYTES;
        }
        self.write(EVENT_HEAD, head);
        if self.read(STATUS) & STATUS_EVENT_OVERFLOW != 0 {
            self.write(STATUS, STATUS_EVENT_OVERFLOW);
        }
    }
}

static IOMMU: TicketLock<Iommu> = TicketLock::new(Iommu::ABSENT);

fn write_entry(table: u64, device: usize, words: [u64; 3]) {
    let entry = (table + device as u64 * 32) as usize as *mut u64;
    // SAFETY: an entry of the device table, which covers every device id.
    unsafe {
        write_volatile(entry.add(3), 0);
        write_volatile(entry.add(2), words[2]);
        write_volatile(entry.add(1), words[1]);
        write_volatile(entry, words[0]);
    }
}

fn domain_entry(root: u64) -> [u64; 3] {
    [
        DTE_VALID
            | DTE_TRANSLATION
            | DTE_MODE_4_LEVELS
            | DTE_READ
            | DTE_WRITE
            | (root & ADDRESS_MASK),
        DOMAIN,
        DTE_INTERRUPTS_PASS,
    ]
}

fn read_u16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn read_u64(bytes: &[u8], at: usize) -> u64 {
    let mut word = [0u8; 8];
    word.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(word)
}

/// The first IOMMU of an IVRS table, the address ranges firmware wants mapped
/// one-to-one, and the table's device entries.
struct Ivrs<'a> {
    base: u64,
    entries: &'a [u8],
    unity: [(u64, u64); MAX_UNITY],
    unity_count: usize,
}

fn parse_ivrs(table: &[u8]) -> Option<Ivrs<'_>> {
    let mut offset = 48;
    let mut found: Option<(u64, &[u8])> = None;
    let mut unity = [(0u64, 0u64); MAX_UNITY];
    let mut unity_count = 0;
    while offset + 4 <= table.len() {
        let kind = table[offset];
        let length = read_u16(table, offset + 2) as usize;
        if length < 4 || offset + length > table.len() {
            break;
        }
        let block = &table[offset..offset + length];
        match kind {
            0x10 | 0x11 | 0x40 if found.is_none() && length >= 24 => {
                let header = if kind == 0x10 { 24 } else { 40 };
                if length >= header {
                    found = Some((read_u64(block, 8), &block[header..]));
                }
            }
            0x20..=0x22 if length >= 32 && unity_count < MAX_UNITY => {
                let start = read_u64(block, 16);
                let span = read_u64(block, 24);
                if span != 0 && span <= MAX_UNITY_BYTES {
                    unity[unity_count] = (start, span);
                    unity_count += 1;
                }
            }
            _ => {}
        }
        offset += length;
    }
    let (base, entries) = found?;
    (base != 0).then_some(Ivrs {
        base,
        entries,
        unity,
        unity_count,
    })
}

/// Applies the interrupt-forwarding bits an IVHD entry asks for (INIT,
/// ExtINT, NMI, LINT0, LINT1) and gives the devices named by special and
/// alias entries a place in the domain.
fn apply_entries(table: u64, root: u64, entries: &[u8]) {
    let flag_bits = |flags: u8| -> u64 {
        let mut bits = 0u64;
        for (source, target) in [(0, 56), (1, 57), (2, 58), (6, 62), (7, 63)] {
            if flags & (1 << source) != 0 {
                bits |= 1 << target;
            }
        }
        bits
    };
    let or_flags = |device: usize, flags: u8| {
        let entry = (table + device as u64 * 32 + 16) as usize as *mut u64;
        // SAFETY: the third word of an entry of the device table.
        unsafe { write_volatile(entry, read_volatile(entry) | flag_bits(flags)) };
    };
    let join = |device: usize| write_entry(table, device, domain_entry(root));
    let mut offset = 0;
    let mut range_start: Option<(usize, u8)> = None;
    while offset + 4 <= entries.len() {
        let kind = entries[offset];
        let size = match kind {
            0x00..=0x3f => 4,
            0x40..=0x7f => 8,
            _ => {
                if offset + 22 > entries.len() {
                    break;
                }
                22 + entries[offset + 21] as usize
            }
        };
        if offset + size > entries.len() {
            break;
        }
        let entry = &entries[offset..offset + size];
        let device = read_u16(entry, 1) as usize;
        let flags = entry[3];
        match kind {
            0x01 => {
                for each in 0..DEVICES {
                    or_flags(each, flags);
                }
            }
            0x02 => or_flags(device, flags),
            0x03 => range_start = Some((device, flags)),
            0x04 => {
                if let Some((start, start_flags)) = range_start.take() {
                    for each in start..=device {
                        or_flags(each, start_flags);
                    }
                }
            }
            0x42 if size >= 8 => {
                or_flags(device, flags);
                join(read_u16(entry, 5) as usize);
            }
            0x48 if size >= 8 => {
                let special = read_u16(entry, 5) as usize;
                join(special);
                or_flags(special, flags);
            }
            _ => {}
        }
        offset += size;
    }
}

/// Reads the firmware's IVRS table and builds the device table, command and
/// event rings and an empty page table. Nothing is switched on yet.
pub fn initialize(acpi: &AcpiInfo, pci: &PciInventory, frames: &mut FrameAllocator) -> Report {
    // SAFETY: the ACPI tables are identity-mapped firmware memory.
    let Some((address, length)) = (unsafe { crate::acpi::find_table(acpi, *b"IVRS") }) else {
        return Report::ABSENT;
    };
    // SAFETY: `find_table` validated the table's length and checksum.
    let table = unsafe { core::slice::from_raw_parts(address as usize as *const u8, length) };
    let Some(ivrs) = parse_ivrs(table) else {
        return Report::ABSENT;
    };
    let pages = DEVICE_TABLE_PAGES + 3 + 2 + POOL_PAGES;
    let Some(block) = frames.allocate_contiguous(pages, 1) else {
        return Report::ABSENT;
    };
    let start = block.address();
    // SAFETY: the block was just allocated and is identity-mapped RAM.
    unsafe { core::ptr::write_bytes(start as usize as *mut u8, 0, (pages * PAGE) as usize) };
    let device_table = start;
    let command = device_table + DEVICE_TABLE_PAGES * PAGE;
    let event = command + PAGE;
    let semaphore = event + PAGE;
    let empty_root = semaphore + PAGE;
    let root = empty_root + PAGE;
    let pool = root + PAGE;
    let mut unit = Iommu::ABSENT;
    unit.mmio = ivrs.base;
    unit.device_table = device_table;
    unit.command = command;
    unit.event = event;
    unit.semaphore = semaphore;
    unit.root = root;
    unit.empty_root = empty_root;
    unit.pool_next = pool;
    unit.pool_end = pool + POOL_PAGES * PAGE;
    let blocked = [
        DTE_VALID
            | DTE_TRANSLATION
            | DTE_MODE_4_LEVELS
            | DTE_READ
            | DTE_WRITE
            | (empty_root & ADDRESS_MASK),
        0,
        0,
    ];
    for device in 0..DEVICES {
        write_entry(device_table, device, blocked);
    }
    let mut devices = 0;
    for function in pci.devices() {
        let id = (function.bus as usize) << 8
            | (function.slot as usize) << 3
            | function.function as usize;
        write_entry(device_table, id, domain_entry(root));
        if devices < unit.scanned.len() {
            unit.scanned[devices] = id as u16;
            unit.scanned_count = devices + 1;
        }
        devices += 1;
    }
    apply_entries(device_table, root, ivrs.entries);
    unit.present = true;
    for (start, span) in &ivrs.unity[..ivrs.unity_count] {
        let first = start & !(PAGE - 1);
        let end = (start + span).div_ceil(PAGE) * PAGE;
        unit.map(first, first, (end - first) / PAGE);
    }
    *IOMMU.lock() = unit;
    Report {
        present: true,
        base: ivrs.base,
        devices,
        unity_ranges: ivrs.unity_count,
    }
}

/// Switches the unit on. Returns whether translation is now active.
pub fn enable() -> bool {
    let mut unit = IOMMU.lock();
    if !unit.present || unit.enabled {
        return unit.enabled;
    }
    unit.write(
        DEVICE_TABLE_BASE,
        unit.device_table | (DEVICE_TABLE_PAGES - 1),
    );
    unit.write(COMMAND_BASE, unit.command | LENGTH_256_ENTRIES);
    unit.write(COMMAND_HEAD, 0);
    unit.write(COMMAND_TAIL, 0);
    unit.write(EVENT_BASE, unit.event | LENGTH_256_ENTRIES);
    unit.write(EVENT_HEAD, 0);
    unit.write(EVENT_TAIL, 0);
    unit.command_tail = 0;
    let control = unit.read(CONTROL);
    unit.write(CONTROL, control | CONTROL_COMMAND | CONTROL_EVENT_LOG);
    unit.write(
        CONTROL,
        control | CONTROL_COMMAND | CONTROL_EVENT_LOG | CONTROL_ENABLE,
    );
    unit.enabled = true;
    for index in 0..unit.scanned_count {
        let device = unit.scanned[index];
        unit.invalidate_device(device);
    }
    unit.invalidate_domain();
    unit.enabled
}

/// Turns translation off again (the boot disk could not be read with it on).
pub fn disable() {
    let mut unit = IOMMU.lock();
    if unit.enabled {
        let control = unit.read(CONTROL);
        unit.write(CONTROL, control & !CONTROL_ENABLE);
        unit.enabled = false;
    }
}

pub fn present() -> bool {
    IOMMU.lock().present
}

/// Lets devices reach `pages` pages at `physical` from now on.
pub fn allow(physical: u64, pages: u64) -> bool {
    let mut unit = IOMMU.lock();
    !unit.present || unit.map(physical, physical, pages)
}

#[cfg(feature = "boot-test")]
/// Makes `pages` pages at `physical` reachable at `iova` (not an identity
/// mapping), for devices with narrow addressing.
pub fn map_iova(iova: u64, physical: u64, pages: u64) -> bool {
    let mut unit = IOMMU.lock();
    unit.present && unit.map(iova, physical, pages)
}

#[cfg(feature = "boot-test")]
pub fn unmap_iova(iova: u64, pages: u64) {
    IOMMU.lock().unmap(iova, pages);
}

#[cfg(feature = "boot-test")]
/// Whether `iova` is mapped in the shared domain.
pub fn is_mapped(iova: u64) -> bool {
    let mut unit = IOMMU.lock();
    unit.leaf(iova, false)
        // SAFETY: a leaf slot of the domain's page table.
        .is_some_and(|slot| unsafe { read_volatile(slot) } & PTE_PRESENT != 0)
}

/// A range lent to the devices for the duration of one transfer.
pub struct Window {
    first: u64,
    pages: u64,
}

impl Drop for Window {
    fn drop(&mut self) {
        if self.pages != 0 {
            IOMMU.lock().unmap(self.first, self.pages);
        }
    }
}

/// Opens `bytes` bytes at `physical` to the devices until the window is dropped.
pub fn window(physical: u64, bytes: u64) -> Option<Window> {
    let mut unit = IOMMU.lock();
    if !unit.present || bytes == 0 {
        return Some(Window { first: 0, pages: 0 });
    }
    let first = physical & !(PAGE - 1);
    let pages = (physical + bytes).div_ceil(PAGE) - first / PAGE;
    unit.map(first, first, pages)
        .then_some(Window { first, pages })
}

pub struct Status {
    pub mapped_pages: u64,
    pub faults: u64,
    pub other_events: u64,
    pub last_fault: Fault,
}

pub fn status() -> Status {
    let mut unit = IOMMU.lock();
    unit.collect_events();
    Status {
        mapped_pages: unit.mapped_pages,
        faults: unit.faults,
        other_events: unit.other_events,
        last_fault: unit.last_fault,
    }
}

#[cfg(feature = "boot-test")]
#[derive(Clone, Copy, Default)]
pub struct TestReport {
    pub edu: bool,
    pub mapped: bool,
    pub blocked_write: bool,
    pub blocked_read: bool,
    pub revoked: bool,
}

#[cfg(feature = "boot-test")]
impl TestReport {
    pub fn verified(&self) -> bool {
        self.edu && self.mapped && self.blocked_write && self.blocked_read && self.revoked
    }
}

/// Drives QEMU's `edu` test device (which has a DMA engine) through the
/// unit: a mapped page can be read and written at an address that is not its
/// physical one, and unmapped, never-mapped and withdrawn addresses fault.
#[cfg(feature = "boot-test")]
pub fn self_test(pci: &PciInventory) -> TestReport {
    use crate::memory::PageBuffer;

    const EDU_BUFFER: u64 = 0x4_0000;
    const DMA_SOURCE: u64 = 0x80;
    const DMA_DESTINATION: u64 = 0x88;
    const DMA_COUNT: u64 = 0x90;
    const DMA_COMMAND: u64 = 0x98;

    let mut report = TestReport::default();
    let Some(device) = pci
        .devices()
        .iter()
        .copied()
        .find(|device| device.vendor == 0x1234 && device.device == 0x11e8)
    else {
        return report;
    };
    let Some(bar) = crate::pci::bar_address(&device, 0, 0) else {
        return report;
    };
    if !pci.enable_memory_bus_master(device) {
        return report;
    }
    let id = (device.bus as u16) << 8 | (device.slot as u16) << 3 | device.function as u16;
    let read = |offset: u64| -> u64 {
        // SAFETY: the device's first BAR, which `edu` serves as MMIO registers.
        unsafe { read_volatile((bar + offset) as usize as *const u64) }
    };
    let write = |offset: u64, value: u64| {
        // SAFETY: as above.
        unsafe { write_volatile((bar + offset) as usize as *mut u64, value) };
    };
    // SAFETY: as above; the identification register only answers 32-bit reads.
    let identification = unsafe { read_volatile(bar as usize as *const u32) };
    report.edu = identification & 0xff == 0xed;
    let transfer = |source: u64, destination: u64, to_memory: bool| -> bool {
        write(DMA_SOURCE, source);
        write(DMA_DESTINATION, destination);
        write(DMA_COUNT, PAGE);
        write(DMA_COMMAND, 1 | ((to_memory as u64) << 1));
        let started = crate::time::monotonic_nanoseconds();
        while read(DMA_COMMAND) & 1 != 0 {
            if crate::time::monotonic_nanoseconds() - started > 3_000_000_000 {
                return false;
            }
        }
        true
    };
    let Some(mut page) = PageBuffer::new(PAGE as usize) else {
        return report;
    };
    let physical = page.physical();
    let mut iova = 0x0100_0000u64;
    while is_mapped(iova) {
        iova += PAGE;
        if iova >= 0x0fe0_0000 {
            return report;
        }
    }
    let mut pattern = [0u8; PAGE as usize];
    for (index, byte) in pattern.iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(7).wrapping_add(3);
    }
    page.as_mut_slice().copy_from_slice(&pattern);
    if !map_iova(iova, physical, 1) {
        return report;
    }
    // The page can be read and written through its mapped address.
    let first = transfer(iova, EDU_BUFFER, false);
    page.as_mut_slice().fill(0);
    let second = transfer(EDU_BUFFER, iova, true);
    report.mapped = first && second && page.as_slice() == pattern;

    // Its physical address is not mapped: a write there never lands, and
    // a read there delivers nothing of what the page holds. Both are logged.
    let base = status().faults;
    page.as_mut_slice().fill(0x11);
    transfer(EDU_BUFFER, physical, true);
    let seen = status();
    report.blocked_write = seen.faults > base
        && seen.last_fault.device == id
        && page.as_slice().iter().all(|byte| *byte == 0x11);
    let faults = seen.faults;
    page.as_mut_slice().copy_from_slice(&pattern);
    transfer(physical, EDU_BUFFER, false);
    transfer(EDU_BUFFER, iova, true);
    report.blocked_read = status().faults > faults && page.as_slice() != pattern;

    // Taking the mapping away shuts the page off again.
    let faults = status().faults;
    unmap_iova(iova, 1);
    page.as_mut_slice().fill(0x5a);
    transfer(EDU_BUFFER, iova, true);
    report.revoked = status().faults > faults && page.as_slice().iter().all(|byte| *byte == 0x5a);
    report
}

/// The text of `/proc/iommu`.
pub fn describe(out: &mut impl core::fmt::Write) {
    let seen = status();
    let (present, enabled, base) = {
        let unit = IOMMU.lock();
        (unit.present, unit.enabled, unit.mmio)
    };
    if !present {
        let _ = writeln!(out, "iommu: none");
        return;
    }
    let _ = writeln!(
        out,
        "iommu: amd-vi at {base:#x}\nstate: {}\nmapped pages: {}\nfaults: {}\nother events: {}",
        if enabled { "enabled" } else { "disabled" },
        seen.mapped_pages,
        seen.faults,
        seen.other_events
    );
    if seen.faults != 0 {
        let fault = seen.last_fault;
        let _ = writeln!(
            out,
            "last fault: device {:02x}:{:02x}.{} address {:#x} (raw {:#x} {:#x})",
            fault.device >> 8,
            (fault.device >> 3) & 0x1f,
            fault.device & 7,
            fault.address,
            fault.raw[0],
            fault.raw[1]
        );
    }
}
