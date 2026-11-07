//! NVMe controller driver: polled admin queue plus one I/O queue pair. Small
//! transfers go through a bounce page; `read_phys`/`write_phys` move up to
//! `max_bytes` per command straight to or from physical memory (PRP list).

use core::hint::spin_loop;

use crate::memory::FrameAllocator;
use crate::pci::PciInventory;
use crate::sync::TicketLock;

const PAGE_SIZE: u64 = 4096;
const QUEUE_ENTRIES: usize = 16;
const SQ_ENTRY_BYTES: usize = 64;
const CQ_ENTRY_BYTES: usize = 16;
const READY_TIMEOUT: usize = 20_000_000;
const COMMAND_TIMEOUT: usize = 40_000_000;
const MAX_TRANSFER: u32 = 128 * 1024;

const REG_CAP: usize = 0x00;
const REG_CC: usize = 0x14;
const REG_CSTS: usize = 0x1c;
const REG_AQA: usize = 0x24;
const REG_ASQ: usize = 0x28;
const REG_ACQ: usize = 0x30;
const DOORBELLS: usize = 0x1000;

const ADMIN_CREATE_IO_SQ: u32 = 0x01;
const ADMIN_CREATE_IO_CQ: u32 = 0x05;
const ADMIN_IDENTIFY: u32 = 0x06;
const IO_WRITE: u32 = 0x01;
const IO_READ: u32 = 0x02;

#[derive(Clone, Copy)]
pub struct NvmeReport {
    pub present: bool,
    pub base: u64,
    pub version: u32,
    pub queue_entries_max: u32,
    pub identify: bool,
    pub io_queue: bool,
    pub read: bool,
    pub write_probe: bool,
    pub sectors: u64,
    pub sector_bytes: u32,
    pub model: [u8; 40],
    pub model_length: usize,
    pub verified: bool,
}

impl NvmeReport {
    pub const EMPTY: Self = Self {
        present: false,
        base: 0,
        version: 0,
        queue_entries_max: 0,
        identify: false,
        io_queue: false,
        read: false,
        write_probe: false,
        sectors: 0,
        sector_bytes: 0,
        model: [0; 40],
        model_length: 0,
        verified: false,
    };
}

#[derive(Clone, Copy)]
struct Queue {
    submission: u64,
    completion: u64,
    tail: usize,
    head: usize,
    phase: bool,
}

impl Queue {
    const EMPTY: Self = Self {
        submission: 0,
        completion: 0,
        tail: 0,
        head: 0,
        phase: true,
    };
}

struct NvmeState {
    base: u64,
    stride: usize,
    queues: [Queue; 2],
    data: u64,
    prp_list: u64,
    max_bytes: u32,
    command_id: u16,
    sectors: u64,
    sector_bytes: u32,
    ready: bool,
}

impl NvmeState {
    const EMPTY: Self = Self {
        base: 0,
        stride: 4,
        queues: [Queue::EMPTY; 2],
        data: 0,
        prp_list: 0,
        max_bytes: PAGE_SIZE as u32,
        command_id: 0,
        sectors: 0,
        sector_bytes: 0,
        ready: false,
    };
}

static NVME: TicketLock<NvmeState> = TicketLock::new(NvmeState::EMPTY);

unsafe fn read32(base: u64, offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base as usize + offset) as *const u32) }
}

unsafe fn write32(base: u64, offset: usize, value: u32) {
    unsafe { core::ptr::write_volatile((base as usize + offset) as *mut u32, value) }
}

unsafe fn read64(base: u64, offset: usize) -> u64 {
    unsafe { read32(base, offset) as u64 | (read32(base, offset + 4) as u64) << 32 }
}

unsafe fn write64(base: u64, offset: usize, value: u64) {
    unsafe {
        write32(base, offset, value as u32);
        write32(base, offset + 4, (value >> 32) as u32);
    }
}

fn wait_for(mut condition: impl FnMut() -> bool, limit: usize) -> bool {
    for _ in 0..limit {
        if condition() {
            return true;
        }
        spin_loop();
    }
    false
}

impl NvmeState {
    /// Runs one command on queue `queue_id` (0 = admin, 1 = I/O) and returns
    /// its completion status (0 = success).
    fn submit(&mut self, queue_id: usize, mut command: [u32; 16]) -> Option<u32> {
        self.command_id = self.command_id.wrapping_add(1);
        command[0] = (command[0] & 0xffff) | (self.command_id as u32) << 16;
        let queue = &mut self.queues[queue_id];
        let slot = (queue.submission + (queue.tail * SQ_ENTRY_BYTES) as u64) as usize;
        for (index, word) in command.iter().enumerate() {
            unsafe { core::ptr::write_volatile((slot + index * 4) as *mut u32, *word) };
        }
        queue.tail = (queue.tail + 1) % QUEUE_ENTRIES;
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        let base = self.base;
        let doorbell = DOORBELLS + 2 * queue_id * self.stride;
        unsafe { write32(base, doorbell, queue.tail as u32) };
        let entry = (queue.completion + (queue.head * CQ_ENTRY_BYTES) as u64) as usize;
        let expected = queue.phase;
        let mut status = None;
        let completed = wait_for(
            || {
                let dword3 = unsafe { core::ptr::read_volatile((entry + 12) as *const u32) };
                if (dword3 >> 16) & 1 == expected as u32 {
                    status = Some(dword3 >> 17);
                    true
                } else {
                    false
                }
            },
            COMMAND_TIMEOUT,
        );
        if !completed {
            return None;
        }
        queue.head = (queue.head + 1) % QUEUE_ENTRIES;
        if queue.head == 0 {
            queue.phase = !queue.phase;
        }
        let completion_doorbell = DOORBELLS + (2 * queue_id + 1) * self.stride;
        unsafe { write32(base, completion_doorbell, queue.head as u32) };
        status
    }

    fn transfer_phys(&mut self, write: bool, lba: u64, sectors: u32, phys: u64) -> bool {
        let bytes = sectors as u64 * self.sector_bytes as u64;
        if !self.ready
            || sectors == 0
            || bytes > self.max_bytes as u64
            || !phys.is_multiple_of(4)
            || lba
                .checked_add(sectors as u64)
                .is_none_or(|end| end > self.sectors)
        {
            return false;
        }
        let first_page = phys & !(PAGE_SIZE - 1);
        let pages = ((phys - first_page) + bytes).div_ceil(PAGE_SIZE);
        let prp2 = match pages {
            1 => 0,
            2 => first_page + PAGE_SIZE,
            _ => {
                for index in 1..pages {
                    let entry = (self.prp_list + (index - 1) * 8) as usize as *mut u64;
                    unsafe { core::ptr::write_volatile(entry, first_page + index * PAGE_SIZE) };
                }
                self.prp_list
            }
        };
        let mut command = [0u32; 16];
        command[0] = if write { IO_WRITE } else { IO_READ };
        command[1] = 1;
        command[6] = phys as u32;
        command[7] = (phys >> 32) as u32;
        command[8] = prp2 as u32;
        command[9] = (prp2 >> 32) as u32;
        command[10] = lba as u32;
        command[11] = (lba >> 32) as u32;
        command[12] = sectors - 1;
        self.submit(1, command) == Some(0)
    }

    fn transfer(&mut self, write: bool, lba: u64, sectors: u32, buffer: &mut [u8]) -> bool {
        let bytes = sectors as usize * self.sector_bytes as usize;
        if !self.ready
            || sectors == 0
            || bytes > PAGE_SIZE as usize
            || buffer.len() < bytes
            || lba
                .checked_add(sectors as u64)
                .is_none_or(|end| end > self.sectors)
        {
            return false;
        }
        if write {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    buffer.as_ptr(),
                    self.data as usize as *mut u8,
                    bytes,
                );
            }
        }
        let mut command = [0u32; 16];
        command[0] = if write { IO_WRITE } else { IO_READ };
        command[1] = 1;
        command[6] = self.data as u32;
        command[7] = (self.data >> 32) as u32;
        command[10] = lba as u32;
        command[11] = (lba >> 32) as u32;
        command[12] = sectors - 1;
        if self.submit(1, command) != Some(0) {
            return false;
        }
        if !write {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.data as usize as *const u8,
                    buffer.as_mut_ptr(),
                    bytes,
                );
            }
        }
        true
    }
}

/// Reads `sectors` (at most one page worth) starting at `lba`.
pub fn read(lba: u64, sectors: u32, buffer: &mut [u8]) -> bool {
    NVME.lock().transfer(false, lba, sectors, buffer)
}

/// Reads into physical memory at `phys` (4-byte aligned), at most
/// `max_sectors()` per call.
pub fn read_phys(lba: u64, sectors: u32, phys: u64) -> bool {
    NVME.lock().transfer_phys(false, lba, sectors, phys)
}

pub fn write_phys(lba: u64, sectors: u32, phys: u64) -> bool {
    NVME.lock().transfer_phys(true, lba, sectors, phys)
}

/// Sectors one `read_phys`/`write_phys` call may move.
pub fn max_sectors() -> u32 {
    let state = NVME.lock();
    state.max_bytes.checked_div(state.sector_bytes).unwrap_or(0)
}

pub fn write(lba: u64, sectors: u32, buffer: &[u8]) -> bool {
    let mut copy = [0u8; PAGE_SIZE as usize];
    let bytes = sectors as usize * NVME.lock().sector_bytes as usize;
    if bytes > copy.len() || buffer.len() < bytes {
        return false;
    }
    copy[..bytes].copy_from_slice(&buffer[..bytes]);
    NVME.lock().transfer(true, lba, sectors, &mut copy)
}

pub fn sectors() -> u64 {
    NVME.lock().sectors
}

pub fn sector_bytes() -> u32 {
    NVME.lock().sector_bytes
}

/// Brings the controller back after a sleep that cost it its state: disabled,
/// the admin queues and the I/O queue pair created again over the same memory.
pub fn resume() -> bool {
    let mut state = NVME.lock();
    if !state.ready {
        return true;
    }
    let base = state.base;
    unsafe { write32(base, REG_CC, read32(base, REG_CC) & !1) };
    if !wait_for(|| unsafe { read32(base, REG_CSTS) } & 1 == 0, READY_TIMEOUT) {
        return false;
    }
    let (admin_sq, admin_cq) = (state.queues[0].submission, state.queues[0].completion);
    let (io_sq, io_cq) = (state.queues[1].submission, state.queues[1].completion);
    unsafe {
        core::ptr::write_bytes(admin_sq as usize as *mut u8, 0, PAGE_SIZE as usize);
        core::ptr::write_bytes(admin_cq as usize as *mut u8, 0, PAGE_SIZE as usize);
        core::ptr::write_bytes(io_sq as usize as *mut u8, 0, PAGE_SIZE as usize);
        core::ptr::write_bytes(io_cq as usize as *mut u8, 0, PAGE_SIZE as usize);
    }
    state.queues[0] = Queue {
        submission: admin_sq,
        completion: admin_cq,
        ..Queue::EMPTY
    };
    state.queues[1] = Queue {
        submission: io_sq,
        completion: io_cq,
        ..Queue::EMPTY
    };
    unsafe {
        let entries = QUEUE_ENTRIES as u32 - 1;
        write32(base, REG_AQA, entries << 16 | entries);
        write64(base, REG_ASQ, admin_sq);
        write64(base, REG_ACQ, admin_cq);
        write32(base, REG_CC, 1 | 6 << 16 | 4 << 20);
    }
    if !wait_for(|| unsafe { read32(base, REG_CSTS) } & 1 == 1, READY_TIMEOUT) {
        return false;
    }
    let mut command = [0u32; 16];
    command[0] = ADMIN_CREATE_IO_CQ;
    command[6] = io_cq as u32;
    command[7] = (io_cq >> 32) as u32;
    command[10] = (QUEUE_ENTRIES as u32 - 1) << 16 | 1;
    command[11] = 1;
    if state.submit(0, command) != Some(0) {
        return false;
    }
    let mut command = [0u32; 16];
    command[0] = ADMIN_CREATE_IO_SQ;
    command[6] = io_sq as u32;
    command[7] = (io_sq >> 32) as u32;
    command[10] = (QUEUE_ENTRIES as u32 - 1) << 16 | 1;
    command[11] = 1 << 16 | 1;
    state.submit(0, command) == Some(0)
}

pub fn initialize(pci: &PciInventory, frames: &mut FrameAllocator) -> NvmeReport {
    let Some(device) = pci.find_class(0x01, 0x08, 0x02) else {
        return NvmeReport::EMPTY;
    };
    if device.bars[0] & 1 != 0 || !pci.enable_memory_bus_master(device) {
        return NvmeReport::EMPTY;
    }
    let mut base = (device.bars[0] & 0xffff_fff0) as u64;
    if device.bars[0] & 0x6 == 0x4 {
        base |= (device.bars[1] as u64) << 32;
    }
    if base == 0 {
        return NvmeReport::EMPTY;
    }
    let mut report = NvmeReport {
        present: true,
        base,
        ..NvmeReport::EMPTY
    };
    let capability = unsafe { read64(base, REG_CAP) };
    report.version = unsafe { read32(base, 0x08) };
    report.queue_entries_max = (capability & 0xffff) as u32 + 1;
    let stride = 4usize << ((capability >> 32) & 0xf);
    let command_set_nvm = capability & (1 << 37) != 0;
    if !command_set_nvm || report.queue_entries_max < QUEUE_ENTRIES as u32 {
        return report;
    }
    // Reset, then bring the controller up with the admin queues.
    unsafe { write32(base, REG_CC, read32(base, REG_CC) & !1) };
    if !wait_for(|| unsafe { read32(base, REG_CSTS) } & 1 == 0, READY_TIMEOUT) {
        return report;
    }
    let Some(dma) = frames.allocate_dma(7, 1) else {
        return report;
    };
    let memory = dma.address();
    unsafe { core::ptr::write_bytes(memory as usize as *mut u8, 0, 7 * PAGE_SIZE as usize) };
    let mut state = NvmeState::EMPTY;
    state.base = base;
    state.stride = stride;
    state.queues[0] = Queue {
        submission: memory,
        completion: memory + PAGE_SIZE,
        ..Queue::EMPTY
    };
    state.queues[1] = Queue {
        submission: memory + 2 * PAGE_SIZE,
        completion: memory + 3 * PAGE_SIZE,
        ..Queue::EMPTY
    };
    state.data = memory + 4 * PAGE_SIZE;
    let identify_page = memory + 5 * PAGE_SIZE;
    state.prp_list = memory + 6 * PAGE_SIZE;
    unsafe {
        let entries = QUEUE_ENTRIES as u32 - 1;
        write32(base, REG_AQA, entries << 16 | entries);
        write64(base, REG_ASQ, state.queues[0].submission);
        write64(base, REG_ACQ, state.queues[0].completion);
        // Enable: 64-byte submission and 16-byte completion entries, 4 KiB pages.
        write32(base, REG_CC, 1 | 6 << 16 | 4 << 20);
    }
    if !wait_for(|| unsafe { read32(base, REG_CSTS) } & 1 == 1, READY_TIMEOUT) {
        return report;
    }

    // Identify controller (model string) and namespace 1 (size, sector size).
    let mut command = [0u32; 16];
    command[0] = ADMIN_IDENTIFY;
    command[6] = identify_page as u32;
    command[7] = (identify_page >> 32) as u32;
    command[10] = 1;
    if state.submit(0, command) != Some(0) {
        return report;
    }
    let identify =
        unsafe { core::slice::from_raw_parts((identify_page + 24) as usize as *const u8, 40) };
    report.model = [0; 40];
    let mut length = 0;
    for (index, byte) in identify.iter().enumerate() {
        report.model[index] = *byte;
        if *byte != b' ' && *byte != 0 {
            length = index + 1;
        }
    }
    report.model_length = length;
    // MDTS: the largest transfer, in units of the minimum page size (0 = none).
    let mdts = unsafe { core::ptr::read_volatile((identify_page + 77) as usize as *const u8) };
    let min_page = PAGE_SIZE << ((capability >> 48) & 0xf);
    state.max_bytes = match mdts {
        0 => MAX_TRANSFER,
        shift => (min_page << shift.min(16)).min(MAX_TRANSFER as u64) as u32,
    };
    let mut command = [0u32; 16];
    command[0] = ADMIN_IDENTIFY;
    command[1] = 1;
    command[6] = identify_page as u32;
    command[7] = (identify_page >> 32) as u32;
    command[10] = 0;
    if state.submit(0, command) != Some(0) {
        return report;
    }
    let namespace = identify_page as usize;
    let size = unsafe { core::ptr::read_volatile(namespace as *const u64) };
    let format = unsafe { core::ptr::read_volatile((namespace + 26) as *const u8) } & 0xf;
    let lba_format =
        unsafe { core::ptr::read_volatile((namespace + 128 + format as usize * 4) as *const u32) };
    let data_size_shift = (lba_format >> 16) & 0xff;
    if size == 0 || !(9..=12).contains(&data_size_shift) {
        return report;
    }
    report.identify = true;
    report.sectors = size;
    report.sector_bytes = 1 << data_size_shift;
    state.sectors = size;
    state.sector_bytes = report.sector_bytes;

    // I/O queue pair: completion queue first, then its submission queue.
    let mut command = [0u32; 16];
    command[0] = ADMIN_CREATE_IO_CQ;
    command[6] = state.queues[1].completion as u32;
    command[7] = (state.queues[1].completion >> 32) as u32;
    command[10] = (QUEUE_ENTRIES as u32 - 1) << 16 | 1;
    command[11] = 1;
    if state.submit(0, command) != Some(0) {
        return report;
    }
    let mut command = [0u32; 16];
    command[0] = ADMIN_CREATE_IO_SQ;
    command[6] = state.queues[1].submission as u32;
    command[7] = (state.queues[1].submission >> 32) as u32;
    command[10] = (QUEUE_ENTRIES as u32 - 1) << 16 | 1;
    command[11] = 1 << 16 | 1;
    if state.submit(0, command) != Some(0) {
        return report;
    }
    report.io_queue = true;
    state.ready = true;
    *NVME.lock() = state;

    // Self-test: sector 0 carries the test signature, then a write/read-back
    // round trip on sector 2 (a scratch sector of the test image).
    let bytes = report.sector_bytes as usize;
    let mut sector = [0u8; 4096];
    report.read =
        read(0, 1, &mut sector[..bytes]) && bytes >= 16 && &sector[..15] == b"AEROS-NVME-TEST";
    let mut pattern = [0u8; 4096];
    for (index, byte) in pattern[..bytes].iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(31).wrapping_add(7);
    }
    let mut readback = [0u8; 4096];
    // Only the test image (signature above) is written: on a real disk
    // sector 2 is the GPT partition array.
    report.write_probe = report.read
        && report.sectors > 2
        && write(2, 1, &pattern[..bytes])
        && read(2, 1, &mut readback[..bytes])
        && readback[..bytes] == pattern[..bytes];
    report.verified = report.identify && report.io_queue && report.read && report.write_probe;
    report
}
