use core::arch::asm;

use crate::arch::CpuInfo;
use crate::elf::ElfImage;
use crate::memory::FrameAllocator;
use crate::sync::TicketLock;

const PAGE_SIZE: u64 = 4096;
const ENTRY_COUNT: usize = 512;
const ADDRESS_MASK: u64 = 0x000f_ffff_ffff_f000;
const PRESENT: u64 = 1;
const WRITABLE: u64 = 1 << 1;
const USER: u64 = 1 << 2;
const NO_EXECUTE: u64 = 1 << 63;
const EFER_MSR: u32 = 0xc000_0080;
const EFER_NXE: u64 = 1 << 11;
const CR0_WRITE_PROTECT: u64 = 1 << 16;
const USER_IMAGE_PAGES: usize = 509;
const USER_GUARD_PAGE: usize = 509;
const USER_STACK_FIRST_PAGE: usize = 510;
const USER_STACK_PAGES: usize = 2;
const ANONYMOUS_PAGES: usize = 64;
const ANONYMOUS_FIRST_PAGE: usize = 128;
const BRK_PAGES: usize = 16;
const DEMAND_REGION_PAGES: usize = 32;

#[derive(Clone, Copy)]
pub struct PagingState {
    pub root_physical: u64,
    pub previous_root: u64,
    pub probe_virtual: u64,
    pub probe_physical: u64,
    pub nx_enabled: bool,
    pub write_protect_enabled: bool,
    pub verified: bool,
    leaf_table_physical: u64,
}

/// A copy of the boot-time `PagingState`, stashed by `main.rs` right after
/// `initialize()` succeeds so long-lived interactive code (the shell's
/// `run` command, launched arbitrarily later from the Terminal app) can
/// spawn real scheduler processes without needing the original local
/// variable threaded all the way down from the boot sequence.
static BOOT_PAGING_STATE: TicketLock<Option<PagingState>> = TicketLock::new(None);

pub fn set_boot_state(state: &PagingState) {
    *BOOT_PAGING_STATE.lock() = Some(*state);
}

pub fn boot_state() -> Option<PagingState> {
    *BOOT_PAGING_STATE.lock()
}

pub struct HeapMapping {
    pub virtual_base: u64,
    pub physical_base: u64,
    pub size: usize,
}

pub struct UserMapping {
    pub entry: u64,
    pub stack_top: u64,
    pub code_physical: u64,
    pub stack_physical: u64,
    pub verified: bool,
    allocation_base: u64,
    allocation_pages: u64,
    root_index: usize,
}

pub struct UserImageMapping {
    pub load_bias: u64,
    pub entry: u64,
    pub stack_top: u64,
    pub mapped_pages: usize,
    pub executable_pages: usize,
    pub writable_pages: usize,
    pub stack_pages: usize,
    pub stack_physical: u64,
    pub guard_page: u64,
    pub verified: bool,
    anonymous_physical: u64,
    page_table_physical: u64,
    nx_enabled: bool,
    allocation_base: u64,
    allocation_pages: u64,
    root_index: usize,
}

pub struct UserReapReport {
    pub released_pages: u64,
    pub address_space_removed: bool,
    pub scrubbed: bool,
    pub verified: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UserMemoryError {
    InvalidArgument,
    OutOfMemory,
    NotMapped,
}

struct UserMemoryState {
    base: u64,
    table: u64,
    physical: [u64; ANONYMOUS_PAGES],
    used: [bool; ANONYMOUS_PAGES],
    protection: [u8; ANONYMOUS_PAGES],
    current_brk: u64,
    nx_enabled: bool,
    active: bool,
}

impl UserMemoryState {
    const EMPTY: Self = Self {
        base: 0,
        table: 0,
        physical: [0; ANONYMOUS_PAGES],
        used: [false; ANONYMOUS_PAGES],
        protection: [0; ANONYMOUS_PAGES],
        current_brk: 0,
        nx_enabled: false,
        active: false,
    };
}

static USER_MEMORY: TicketLock<UserMemoryState> = TicketLock::new(UserMemoryState::EMPTY);

struct DemandRegionState {
    base: u64,
    table: u64,
    physical: [u64; DEMAND_REGION_PAGES],
    nx_enabled: bool,
    faults: u64,
    active: bool,
}

impl DemandRegionState {
    const EMPTY: Self = Self {
        base: 0,
        table: 0,
        physical: [0; DEMAND_REGION_PAGES],
        nx_enabled: false,
        faults: 0,
        active: false,
    };
}

static DEMAND_REGION: TicketLock<DemandRegionState> = TicketLock::new(DemandRegionState::EMPTY);

#[derive(Clone, Copy)]
pub struct DemandRegionStats {
    pub reserved_pages: usize,
    pub committed_pages: usize,
    pub faults_handled: u64,
}

pub fn initialize(frames: &mut FrameAllocator, cpu: &CpuInfo) -> Option<PagingState> {
    let block = frames.allocate_contiguous(5, 1)?;
    let root = block.address();
    let pdpt = root.checked_add(PAGE_SIZE)?;
    let directory = pdpt.checked_add(PAGE_SIZE)?;
    let table = directory.checked_add(PAGE_SIZE)?;
    let probe = table.checked_add(PAGE_SIZE)?;
    unsafe {
        zero_page(root);
        zero_page(pdpt);
        zero_page(directory);
        zero_page(table);
        zero_page(probe);
    }

    let previous = read_cr3() & ADDRESS_MASK;
    if previous == 0 {
        return None;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            previous as usize as *const u64,
            root as usize as *mut u64,
            ENTRY_COUNT,
        );
    }
    make_managed_ram_writable(previous, frames);
    let root_table = root as usize as *mut u64;
    let high_index = (256..ENTRY_COUNT)
        .find(|index| unsafe { core::ptr::read_volatile(root_table.add(*index)) & PRESENT == 0 })?;
    let probe_virtual = canonical_base(high_index);
    let nx_enabled = cpu.nx && enable_nx();
    let leaf_flags = PRESENT | WRITABLE | if nx_enabled { NO_EXECUTE } else { 0 };
    unsafe {
        core::ptr::write_volatile(root_table.add(high_index), pdpt | PRESENT | WRITABLE);
        core::ptr::write_volatile(pdpt as usize as *mut u64, directory | PRESENT | WRITABLE);
        core::ptr::write_volatile(directory as usize as *mut u64, table | PRESENT | WRITABLE);
        core::ptr::write_volatile(table as usize as *mut u64, probe | leaf_flags);
        write_cr3(root);
    }
    let write_protect_enabled = enable_write_protect();
    let pattern = 0xa3e0_5c71_d94b_268fu64;
    unsafe {
        core::ptr::write_volatile(probe_virtual as usize as *mut u64, pattern);
    }
    let observed = unsafe { core::ptr::read_volatile(probe_virtual as usize as *const u64) };
    Some(PagingState {
        root_physical: root,
        previous_root: previous,
        probe_virtual,
        probe_physical: probe,
        nx_enabled,
        write_protect_enabled,
        verified: observed == pattern,
        leaf_table_physical: table,
    })
}

/// The firmware may map part of a 2 MiB region read-only (for example next
/// to runtime-services code) even though the kernel owns most of it as
/// ordinary RAM. Kernel stacks and heap allocated there would then fault on
/// their first write (a double fault, since the fault frame itself cannot be
/// pushed), so every page of managed RAM is made writable in the inherited
/// identity map.
fn make_managed_ram_writable(root: u64, frames: &FrameAllocator) {
    for (start, end) in frames.managed_ranges() {
        make_range_writable(root, start, end);
    }
    // The loaded kernel image is not "managed" RAM, but its data and BSS live
    // in memory the firmware may have mapped read-only the same way (AP
    // startup stacks, big buffers): make every writable PE section writable.
    if let Some((base, size)) = kernel_image() {
        let headers = base as usize as *const u8;
        // SAFETY: `kernel_image` validated the DOS/PE headers.
        unsafe {
            let pe =
                base as usize + core::ptr::read_unaligned(headers.add(0x3c) as *const u32) as usize;
            let sections = core::ptr::read_unaligned((pe + 6) as *const u16) as usize;
            let optional = core::ptr::read_unaligned((pe + 20) as *const u16) as usize;
            let table = pe + 24 + optional;
            for index in 0..sections {
                let header = table + index * 40;
                let virtual_size = core::ptr::read_unaligned((header + 8) as *const u32) as u64;
                let virtual_address = core::ptr::read_unaligned((header + 12) as *const u32) as u64;
                let characteristics = core::ptr::read_unaligned((header + 36) as *const u32);
                if characteristics & 0x8000_0000 != 0 && virtual_address < size {
                    make_range_writable(
                        root,
                        base + virtual_address,
                        base + virtual_address + virtual_size.max(1),
                    );
                }
            }
        }
    }
}

/// Finds this kernel's own PE image (base, size of image) by walking back
/// from a function inside it to the DOS/PE headers.
fn kernel_image() -> Option<(u64, u64)> {
    let mut page = (kernel_image as *const () as usize as u64) & !(PAGE_SIZE - 1);
    for _ in 0..65_536 {
        // SAFETY: the kernel image is identity-mapped RAM; reads stay below it.
        let (magic, pe_offset) = unsafe {
            (
                core::ptr::read_unaligned(page as usize as *const u16),
                core::ptr::read_unaligned((page + 0x3c) as usize as *const u32) as u64,
            )
        };
        if magic == 0x5a4d && (0x40..0x400).contains(&pe_offset) {
            let signature =
                unsafe { core::ptr::read_unaligned((page + pe_offset) as usize as *const u32) };
            if signature == 0x0000_4550 {
                let size = unsafe {
                    core::ptr::read_unaligned((page + pe_offset + 24 + 56) as usize as *const u32)
                } as u64;
                return (size > 0).then_some((page, size));
            }
        }
        page = page.checked_sub(PAGE_SIZE)?;
    }
    None
}

fn make_range_writable(root: u64, start: u64, end: u64) {
    const HUGE: u64 = 1 << 7;
    const TWO_MIB: u64 = 2 * 1024 * 1024;
    const ONE_GIB: u64 = 1024 * 1024 * 1024;
    let mut address = start & !(PAGE_SIZE - 1);
    while address < end {
        let indices = [
            ((address >> 39) & 0x1ff) as usize,
            ((address >> 30) & 0x1ff) as usize,
            ((address >> 21) & 0x1ff) as usize,
            ((address >> 12) & 0x1ff) as usize,
        ];
        let mut table = root;
        let mut step = PAGE_SIZE;
        for (level, index) in indices.iter().enumerate() {
            let slot = unsafe { (table as usize as *mut u64).add(*index) };
            let entry = unsafe { core::ptr::read_volatile(slot) };
            if entry & PRESENT == 0 {
                step = match level {
                    0 => 512 * ONE_GIB,
                    1 => ONE_GIB,
                    2 => TWO_MIB,
                    _ => PAGE_SIZE,
                };
                break;
            }
            let leaf = level == 3 || (level > 0 && entry & HUGE != 0);
            if leaf {
                if entry & WRITABLE == 0 {
                    unsafe { core::ptr::write_volatile(slot, entry | WRITABLE) };
                }
                step = match level {
                    1 => ONE_GIB,
                    2 => TWO_MIB,
                    _ => PAGE_SIZE,
                };
                break;
            }
            if entry & WRITABLE == 0 {
                unsafe { core::ptr::write_volatile(slot, entry | WRITABLE) };
            }
            table = entry & ADDRESS_MASK;
        }
        address = (address & !(step - 1)).saturating_add(step);
    }
}

pub fn demand_init(state: &PagingState, frames: &mut FrameAllocator) -> bool {
    let root = state.root_physical as usize as *mut u64;
    let Some(user_index) = choose_user_index(root) else {
        return false;
    };
    let Some(block) = frames.allocate_contiguous(3, 1) else {
        return false;
    };
    let block = block.address();
    let pdpt = block;
    let Some(directory) = pdpt.checked_add(PAGE_SIZE) else {
        return false;
    };
    let Some(table) = directory.checked_add(PAGE_SIZE) else {
        return false;
    };
    unsafe {
        zero_page(pdpt);
        zero_page(directory);
        zero_page(table);
    }
    let base = canonical_base(user_index);
    let branch_flags = PRESENT | WRITABLE;
    unsafe {
        core::ptr::write_volatile(root.add(user_index), pdpt | branch_flags);
        core::ptr::write_volatile(pdpt as usize as *mut u64, directory | branch_flags);
        core::ptr::write_volatile(directory as usize as *mut u64, table | branch_flags);
        write_cr3(state.root_physical);
    }
    *DEMAND_REGION.lock() = DemandRegionState {
        base,
        table,
        physical: [0; DEMAND_REGION_PAGES],
        nx_enabled: state.nx_enabled,
        faults: 0,
        active: true,
    };
    true
}

pub fn demand_base() -> u64 {
    DEMAND_REGION.lock().base
}

pub fn demand_stats() -> DemandRegionStats {
    let region = DEMAND_REGION.lock();
    DemandRegionStats {
        reserved_pages: DEMAND_REGION_PAGES,
        committed_pages: region.physical.iter().filter(|entry| **entry != 0).count(),
        faults_handled: region.faults,
    }
}

pub fn handle_demand_fault(address: u64) -> bool {
    let mut region = DEMAND_REGION.lock();
    if !region.active || address < region.base {
        return false;
    }
    let index = ((address - region.base) / PAGE_SIZE) as usize;
    if index >= DEMAND_REGION_PAGES || region.physical[index] != 0 {
        return false;
    }
    let Some(frame) = crate::memory::allocate_global() else {
        return false;
    };
    let physical = frame.address();
    let flags = PRESENT | WRITABLE | if region.nx_enabled { NO_EXECUTE } else { 0 };
    unsafe {
        core::ptr::write_bytes(physical as usize as *mut u8, 0, PAGE_SIZE as usize);
        core::ptr::write_volatile(
            (region.table as usize as *mut u64).add(index),
            physical | flags,
        );
        invalidate(region.base + index as u64 * PAGE_SIZE);
    }
    region.physical[index] = physical;
    region.faults += 1;
    true
}

pub fn demand_release(address: u64) -> bool {
    let mut region = DEMAND_REGION.lock();
    if !region.active || address < region.base {
        return false;
    }
    let index = ((address - region.base) / PAGE_SIZE) as usize;
    if index >= DEMAND_REGION_PAGES || region.physical[index] == 0 {
        return false;
    }
    let physical = region.physical[index];
    unsafe {
        core::ptr::write_volatile((region.table as usize as *mut u64).add(index), 0);
        invalidate(region.base + index as u64 * PAGE_SIZE);
    }
    region.physical[index] = 0;
    drop(region);
    crate::memory::release_global_address(physical)
}

pub fn demand_self_test() -> bool {
    let before = demand_stats();
    if before.committed_pages != 0 || before.faults_handled != 0 {
        return false;
    }
    let base = demand_base();
    if base == 0 {
        return false;
    }
    let global_before = crate::memory::global_stats();
    let first = base;
    let second = base + DEMAND_REGION_PAGES as u64 / 2 * PAGE_SIZE;
    let pattern_a = 0x4145_524f_5300_0001u64;
    let pattern_b = 0x4445_4d41_4e44_0002u64;
    unsafe {
        core::ptr::write_volatile(first as usize as *mut u64, pattern_a);
        core::ptr::write_volatile(second as usize as *mut u64, pattern_b);
    }
    let observed_a = unsafe { core::ptr::read_volatile(first as usize as *const u64) };
    let observed_b = unsafe { core::ptr::read_volatile(second as usize as *const u64) };
    let global_after_write = crate::memory::global_stats();
    let frames_consumed = match (global_before, global_after_write) {
        (Some(before), Some(after)) => before.free_pages.saturating_sub(after.free_pages) == 2,
        _ => false,
    };
    let released = demand_release(second);
    let global_after_release = crate::memory::global_stats();
    let frame_returned = match (global_after_write, global_after_release) {
        (Some(before), Some(after)) => after.free_pages == before.free_pages + 1,
        _ => false,
    };
    let refaulted_zero = unsafe { core::ptr::read_volatile(second as usize as *const u64) } == 0;
    unsafe {
        core::ptr::write_volatile(second as usize as *mut u64, pattern_b);
    }
    let refaulted_written = unsafe { core::ptr::read_volatile(second as usize as *const u64) };
    let first_untouched = unsafe { core::ptr::read_volatile(first as usize as *const u64) };
    let final_stats = demand_stats();
    observed_a == pattern_a
        && observed_b == pattern_b
        && frames_consumed
        && released
        && frame_returned
        && refaulted_zero
        && refaulted_written == pattern_b
        && first_untouched == pattern_a
        && final_stats.committed_pages == 2
        && final_stats.faults_handled == 3
        && final_stats.reserved_pages == DEMAND_REGION_PAGES
}

pub fn map_heap(
    state: &PagingState,
    frames: &mut FrameAllocator,
    pages: usize,
) -> Option<HeapMapping> {
    if pages == 0 || pages >= ENTRY_COUNT {
        return None;
    }
    let physical = frames.allocate_contiguous(pages as u64, 1)?.address();
    let flags = PRESENT | WRITABLE | if state.nx_enabled { NO_EXECUTE } else { 0 };
    let table = state.leaf_table_physical as usize as *mut u64;
    for index in 0..pages {
        let address = physical.checked_add(index as u64 * PAGE_SIZE)?;
        unsafe {
            core::ptr::write_volatile(table.add(index + 1), address | flags);
        }
    }
    unsafe {
        write_cr3(state.root_physical);
    }
    Some(HeapMapping {
        virtual_base: state.probe_virtual + PAGE_SIZE,
        physical_base: physical,
        size: pages * PAGE_SIZE as usize,
    })
}

pub fn map_user_probe(state: &PagingState, frames: &mut FrameAllocator) -> Option<UserMapping> {
    let program: [u8; 19] = [
        0xb8, 0x01, 0x00, 0x00, 0x00, 0xcd, 0x80, 0xb8, 0x00, 0x00, 0x00, 0x00, 0xbf, 0x2a, 0x00,
        0x00, 0x00, 0xcd, 0x80,
    ];
    map_probe_code(state, frames, &program)
}

pub fn map_probe_code(
    state: &PagingState,
    frames: &mut FrameAllocator,
    program: &[u8],
) -> Option<UserMapping> {
    if program.len() > PAGE_SIZE as usize {
        return None;
    }
    let root = state.root_physical as usize as *mut u64;
    let user_index = choose_user_index(root)?;
    let block = frames.allocate_contiguous(5, 1)?.address();
    let pdpt = block;
    let directory = pdpt.checked_add(PAGE_SIZE)?;
    let table = directory.checked_add(PAGE_SIZE)?;
    let code = table.checked_add(PAGE_SIZE)?;
    let stack = code.checked_add(PAGE_SIZE)?;
    unsafe {
        zero_page(pdpt);
        zero_page(directory);
        zero_page(table);
        zero_page(code);
        zero_page(stack);
    }
    let virtual_base = canonical_base(user_index);
    let branch_flags = PRESENT | WRITABLE | USER;
    let code_flags = PRESENT | USER;
    let stack_flags = PRESENT | WRITABLE | USER | if state.nx_enabled { NO_EXECUTE } else { 0 };
    unsafe {
        core::ptr::copy_nonoverlapping(program.as_ptr(), code as usize as *mut u8, program.len());
        core::ptr::write_volatile(root.add(user_index), pdpt | branch_flags);
        core::ptr::write_volatile(pdpt as usize as *mut u64, directory | branch_flags);
        core::ptr::write_volatile(directory as usize as *mut u64, table | branch_flags);
        core::ptr::write_volatile(table as usize as *mut u64, code | code_flags);
        core::ptr::write_volatile((table as usize as *mut u64).add(2), stack | stack_flags);
        write_cr3(state.root_physical);
    }
    let entries = unsafe {
        [
            core::ptr::read_volatile(root.add(user_index)),
            core::ptr::read_volatile(pdpt as usize as *const u64),
            core::ptr::read_volatile(directory as usize as *const u64),
            core::ptr::read_volatile(table as usize as *const u64),
            core::ptr::read_volatile((table as usize as *const u64).add(2)),
        ]
    };
    let hierarchy_user = entries[..4].iter().all(|entry| entry & USER != 0);
    let code_protected = entries[3] & WRITABLE == 0 && entries[3] & NO_EXECUTE == 0;
    let stack_protected =
        entries[4] & WRITABLE != 0 && (!state.nx_enabled || entries[4] & NO_EXECUTE != 0);
    Some(UserMapping {
        entry: virtual_base,
        stack_top: virtual_base + PAGE_SIZE * 3 - 16,
        code_physical: code,
        stack_physical: stack,
        verified: hierarchy_user && code_protected && stack_protected,
        allocation_base: block,
        allocation_pages: 5,
        root_index: user_index,
    })
}

const PROCESS_USER_SLOT: usize = 1;

/// A process's whole address space (code/data, stack, heap, mmap) lives in
/// ONE leaf page table (512 entries), the same design the original
/// single-page `create_process` used - just with room reserved for a real,
/// multi-segment ELF image instead of one raw page. Code always reserves
/// this full slot range regardless of how much a given image actually
/// needs (unused trailing slots are simply left unmapped), so heap/mmap
/// never have to move when `exec_process` swaps in a differently-sized
/// image. 200 pages (800 KB) comfortably covers `aeros-std-smoke` (the
/// biggest real userspace binary in this repo, ~380 KB) with headroom.
const PROCESS_CODE_PAGES: usize = 200;
const PROCESS_GUARD_INDEX: usize = PROCESS_CODE_PAGES;
const PROCESS_STACK_FIRST_INDEX: usize = PROCESS_CODE_PAGES + 1;
// Tests that read a stack marker must use PROCESS_STACK_BYTES, not a
// hardcoded one-page offset. Real std-linked ELFs need more than one page.
pub const PROCESS_STACK_PAGES: usize = 4;
pub const PROCESS_STACK_BYTES: u64 = PROCESS_STACK_PAGES as u64 * PAGE_SIZE;

const FRAME_REF_SLOTS: usize = 8192;
/// Software-available PTE bit: the page is shared copy-on-write (mapped
/// read-only until a write fault gives the writer a private copy).
const COW: u64 = 1 << 9;

/// How many address spaces map each shared physical frame. Entries are never
/// emptied (so probe chains stay intact); a count of 0 marks a reusable slot.
static FRAME_REFS: TicketLock<[(u64, u32); FRAME_REF_SLOTS]> =
    TicketLock::new([(0, 0); FRAME_REF_SLOTS]);

fn frame_slot(
    table: &[(u64, u32); FRAME_REF_SLOTS],
    physical: u64,
) -> (Option<usize>, Option<usize>) {
    let mut index = ((physical >> 12).wrapping_mul(0x9e37_79b1) as usize) % FRAME_REF_SLOTS;
    let mut reusable = None;
    for _ in 0..FRAME_REF_SLOTS {
        let entry = table[index];
        if entry.0 == physical {
            return (Some(index), reusable);
        }
        if entry.0 == 0 {
            return (None, reusable.or(Some(index)));
        }
        if entry.1 == 0 && reusable.is_none() {
            reusable = Some(index);
        }
        index = (index + 1) % FRAME_REF_SLOTS;
    }
    (None, reusable)
}

/// Adds an owner to a frame: a fresh (untracked) frame starts at one owner.
fn frame_retain(physical: u64) -> bool {
    let mut table = FRAME_REFS.lock();
    match frame_slot(&table, physical) {
        (Some(index), _) if table[index].1 > 0 => {
            table[index].1 += 1;
            true
        }
        (Some(index), _) => {
            table[index].1 = 1;
            true
        }
        (None, Some(index)) => {
            table[index] = (physical, 1);
            true
        }
        (None, None) => false,
    }
}

/// Makes an owned (possibly untracked) frame shared with one more owner.
fn frame_share(physical: u64) -> bool {
    let mut table = FRAME_REFS.lock();
    match frame_slot(&table, physical) {
        (Some(index), _) if table[index].1 > 0 => {
            table[index].1 += 1;
            true
        }
        (Some(index), _) => {
            table[index].1 = 2;
            true
        }
        (None, Some(index)) => {
            table[index] = (physical, 2);
            true
        }
        (None, None) => false,
    }
}

/// Drops one owner; the frame goes back to the allocator with the last one
/// (a frame that was never shared is simply freed).
fn frame_release(physical: u64) -> bool {
    let mut table = FRAME_REFS.lock();
    if let (Some(index), _) = frame_slot(&table, physical)
        && table[index].1 > 0
    {
        table[index].1 -= 1;
        if table[index].1 > 0 {
            return false;
        }
    }
    drop(table);
    crate::memory::release_global_address(physical)
}

pub fn code_page_refcount(physical: u64) -> u32 {
    let table = FRAME_REFS.lock();
    match frame_slot(&table, physical) {
        (Some(index), _) => table[index].1,
        _ => 0,
    }
}

const PROCESS_HEAP_PAGES: usize = 64;
const PROCESS_HEAP_FIRST_INDEX: usize = PROCESS_STACK_FIRST_INDEX + PROCESS_STACK_PAGES;
const PROCESS_MMAP_PAGES: usize = 128;
const PROCESS_MMAP_FIRST_INDEX: usize = PROCESS_HEAP_FIRST_INDEX + PROCESS_HEAP_PAGES;
const PROCESS_RESERVED_PAGES: usize = PROCESS_MMAP_FIRST_INDEX + PROCESS_MMAP_PAGES;

#[derive(Clone, Copy)]
pub struct ProcessAddressSpace {
    pub root_physical: u64,
    pub entry: u64,
    pub stack_top: u64,
    /// Physical address of each mapped code/data page, indexed by virtual
    /// page number from `entry`'s page (slot 0) up to `PROCESS_CODE_PAGES`;
    /// 0 = not mapped. The legacy single-page raw-blob path (every existing
    /// scheduler self-test probe - none of them are real ELF files) only
    /// ever populates slot 0, so it's indistinguishable in shape from a
    /// one-page ELF image; both `fork_process`/`exec_process`/
    /// `destroy_process` handle every case with the same generic loop.
    pub code_physical: [u64; PROCESS_CODE_PAGES],
    /// The exact PTE flag bits (PRESENT|USER|WRITABLE|NO_EXECUTE) each
    /// mapped code page was given, so fork can reproduce them without
    /// re-deriving permissions from segment metadata it no longer has.
    code_flags: [u64; PROCESS_CODE_PAGES],
    pub stack_physical: u64,
    pub nx_enabled: bool,
    pub verified: bool,
    pub owns_code: bool,
    allocation_base: u64,
    allocation_pages: u64,
    pub heap_mapped: usize,
    pub heap_physical: [u64; PROCESS_HEAP_PAGES],
    mmap_used: [bool; PROCESS_MMAP_PAGES],
    mmap_protection: [u8; PROCESS_MMAP_PAGES],
    pub mmap_physical: [u64; PROCESS_MMAP_PAGES],
}

/// The virtual base every process's dedicated address space starts at.
/// Fixed and identical across all processes (isolation comes from each
/// having its own root page table at this same numeric address, not from
/// the address itself varying) - callers that need the true start of a
/// process's mapped range (e.g. syscall pointer-validation bounds) should
/// use this instead of `ProcessAddressSpace::entry`, which can sit at a
/// nonzero offset from the base for a real ELF image whose entry point
/// isn't at the very first mapped page.
pub fn process_virtual_base() -> u64 {
    canonical_base(PROCESS_USER_SLOT)
}

pub fn process_reserved_end(base: u64) -> u64 {
    base + PROCESS_RESERVED_PAGES as u64 * PAGE_SIZE
}

/// Virtual base of a process's one-page stack region, for callers (the
/// Linux-ABI argc/argv/envp/auxv stack builder) that need to write into
/// `space.stack_physical` at the matching virtual addresses.
pub fn process_stack_base() -> u64 {
    process_virtual_base() + PROCESS_STACK_FIRST_INDEX as u64 * PAGE_SIZE
}

/// Tries to load `code` as a real multi-segment ELF binary first; if that
/// parse fails (every existing scheduler self-test probe is a raw machine
/// code blob, not a real ELF file), falls back to the original one-page
/// behavior those tests depend on unchanged.
pub fn create_process(state: &PagingState, code: &[u8]) -> Option<ProcessAddressSpace> {
    match crate::elf::ElfImage::parse(code) {
        Ok(image) => create_process_from_elf(state, &image),
        Err(_) => create_process_raw(state, code),
    }
}

fn allocate_process_tables(stack_physical_pages: u64) -> Option<(u64, u64, u64, u64, u64)> {
    let block = crate::memory::allocate_global_contiguous(4 + stack_physical_pages, 1)?.address();
    let pml4 = block;
    let pdpt = pml4.checked_add(PAGE_SIZE)?;
    let directory = pdpt.checked_add(PAGE_SIZE)?;
    let table = directory.checked_add(PAGE_SIZE)?;
    let stack_physical = table.checked_add(PAGE_SIZE)?;
    unsafe {
        zero_page(pml4);
        zero_page(pdpt);
        zero_page(directory);
        zero_page(table);
        for page in 0..stack_physical_pages {
            zero_page(stack_physical + page * PAGE_SIZE);
        }
    }
    Some((block, pml4, pdpt, directory, table))
}

fn link_process_root(
    nx_enabled: bool,
    new_root: *mut u64,
    pdpt: u64,
    directory: u64,
    table: u64,
    stack_physical: u64,
) {
    let branch_flags = PRESENT | WRITABLE | USER;
    let stack_flags = PRESENT | WRITABLE | USER | if nx_enabled { NO_EXECUTE } else { 0 };
    unsafe {
        core::ptr::write_volatile(new_root.add(PROCESS_USER_SLOT), pdpt | branch_flags);
        core::ptr::write_volatile(pdpt as usize as *mut u64, directory | branch_flags);
        core::ptr::write_volatile(directory as usize as *mut u64, table | branch_flags);
        for page in 0..PROCESS_STACK_PAGES as u64 {
            core::ptr::write_volatile(
                (table as usize as *mut u64).add(PROCESS_STACK_FIRST_INDEX + page as usize),
                (stack_physical + page * PAGE_SIZE) | stack_flags,
            );
        }
    }
}

/// The original single-page process loader: `code` is a flat,
/// already-position-independent blob run from offset 0, with no
/// segment/permission structure. `create_process` only reaches this when
/// ELF parsing fails, which is exactly what every existing self-test probe
/// (`fork_probe`, `execve_probe`, etc.) hits, so their behavior is
/// unchanged - they just occupy code slot 0 of the now-array-shaped
/// `code_physical` instead of a single scalar field.
fn create_process_raw(state: &PagingState, code: &[u8]) -> Option<ProcessAddressSpace> {
    if code.is_empty() || code.len() > PAGE_SIZE as usize {
        return None;
    }
    let (block, pml4, pdpt, directory, table) =
        allocate_process_tables(PROCESS_STACK_PAGES as u64)?;
    let stack_physical = table.checked_add(PAGE_SIZE)?;
    let code_physical_page = crate::memory::allocate_global()?.address();
    unsafe { zero_page(code_physical_page) };
    frame_retain(code_physical_page);
    let kernel_root = state.root_physical as usize as *const u64;
    let new_root = pml4 as usize as *mut u64;
    unsafe {
        core::ptr::copy_nonoverlapping(kernel_root, new_root, ENTRY_COUNT);
        core::ptr::copy_nonoverlapping(
            code.as_ptr(),
            code_physical_page as usize as *mut u8,
            code.len(),
        );
    }
    let code_page_flags = PRESENT | USER;
    unsafe {
        core::ptr::write_volatile(
            table as usize as *mut u64,
            code_physical_page | code_page_flags,
        );
    }
    link_process_root(
        state.nx_enabled,
        new_root,
        pdpt,
        directory,
        table,
        stack_physical,
    );
    let entries = unsafe {
        [
            core::ptr::read_volatile(new_root.add(PROCESS_USER_SLOT)),
            core::ptr::read_volatile(pdpt as usize as *const u64),
            core::ptr::read_volatile(directory as usize as *const u64),
            core::ptr::read_volatile(table as usize as *const u64),
            core::ptr::read_volatile((table as usize as *const u64).add(PROCESS_STACK_FIRST_INDEX)),
        ]
    };
    let hierarchy_user = entries[..4].iter().all(|entry| entry & USER != 0);
    let code_protected = entries[3] & WRITABLE == 0;
    let stack_protected = entries[4] & WRITABLE != 0;
    let mut code_physical = [0u64; PROCESS_CODE_PAGES];
    let mut code_flags = [0u64; PROCESS_CODE_PAGES];
    code_physical[0] = code_physical_page;
    code_flags[0] = code_page_flags;
    let virtual_base = canonical_base(PROCESS_USER_SLOT);
    Some(ProcessAddressSpace {
        root_physical: pml4,
        entry: virtual_base,
        stack_top: virtual_base
            + (PROCESS_STACK_FIRST_INDEX + PROCESS_STACK_PAGES) as u64 * PAGE_SIZE
            - 16,
        code_physical,
        code_flags,
        stack_physical,
        nx_enabled: state.nx_enabled,
        verified: hierarchy_user && code_protected && stack_protected,
        owns_code: true,
        allocation_base: block,
        allocation_pages: 4 + PROCESS_STACK_PAGES as u64,
        heap_mapped: 0,
        heap_physical: [0; PROCESS_HEAP_PAGES],
        mmap_used: [false; PROCESS_MMAP_PAGES],
        mmap_protection: [0; PROCESS_MMAP_PAGES],
        mmap_physical: [0; PROCESS_MMAP_PAGES],
    })
}

/// A real multi-segment ELF loader for the per-task scheduler path, ported
/// from the proven single-shot `map_user_image` (same segment-walking
/// algorithm) but targeting this process's own dedicated page table
/// instead of one shared singleton slot, so it composes with fork/exec.
fn create_process_from_elf(
    state: &PagingState,
    image: &crate::elf::ElfImage<'_>,
) -> Option<ProcessAddressSpace> {
    let mut required = [false; PROCESS_CODE_PAGES];
    let mut writable = [false; PROCESS_CODE_PAGES];
    let mut executable = [false; PROCESS_CODE_PAGES];
    for segment in image.segments() {
        let first = usize::try_from(segment.virtual_address / PAGE_SIZE).ok()?;
        let end = segment.memory_end().checked_add(PAGE_SIZE - 1)? / PAGE_SIZE;
        let end = usize::try_from(end).ok()?;
        if first >= end || end > PROCESS_CODE_PAGES {
            return None;
        }
        for page in first..end {
            required[page] = true;
            writable[page] |= segment.writable;
            executable[page] |= segment.executable;
            if writable[page] && executable[page] {
                return None;
            }
        }
    }
    let entry_page = usize::try_from(image.entry() / PAGE_SIZE).ok()?;
    if entry_page >= PROCESS_CODE_PAGES || !executable[entry_page] {
        return None;
    }
    if !required.iter().any(|needed| *needed) {
        return None;
    }

    let (block, pml4, pdpt, directory, table) =
        allocate_process_tables(PROCESS_STACK_PAGES as u64)?;
    let stack_physical = table.checked_add(PAGE_SIZE)?;
    let kernel_root = state.root_physical as usize as *const u64;
    let new_root = pml4 as usize as *mut u64;
    unsafe { core::ptr::copy_nonoverlapping(kernel_root, new_root, ENTRY_COUNT) };

    let mut code_physical = [0u64; PROCESS_CODE_PAGES];
    let mut code_flags = [0u64; PROCESS_CODE_PAGES];
    for page in 0..PROCESS_CODE_PAGES {
        if !required[page] {
            continue;
        }
        let physical = crate::memory::allocate_global()?.address();
        unsafe { zero_page(physical) };
        frame_retain(physical);
        let mut flags = PRESENT | USER;
        if writable[page] {
            flags |= WRITABLE;
        }
        if !executable[page] && state.nx_enabled {
            flags |= NO_EXECUTE;
        }
        code_physical[page] = physical;
        code_flags[page] = flags;
        unsafe {
            core::ptr::write_volatile((table as usize as *mut u64).add(page), physical | flags);
        }
    }
    for segment in image.segments() {
        let mut source = segment.file_offset;
        let mut virtual_offset = segment.virtual_address as usize;
        let mut remaining = segment.file_size;
        while remaining != 0 {
            let page = virtual_offset / PAGE_SIZE as usize;
            let within_page = virtual_offset & (PAGE_SIZE as usize - 1);
            let count = remaining.min(PAGE_SIZE as usize - within_page);
            let destination =
                code_physical[page].checked_add(within_page as u64)? as usize as *mut u8;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    image.bytes().as_ptr().add(source),
                    destination,
                    count,
                );
            }
            source = source.checked_add(count)?;
            virtual_offset = virtual_offset.checked_add(count)?;
            remaining -= count;
        }
    }
    link_process_root(
        state.nx_enabled,
        new_root,
        pdpt,
        directory,
        table,
        stack_physical,
    );

    let hierarchy_valid = unsafe {
        core::ptr::read_volatile(new_root.add(PROCESS_USER_SLOT)) & (PRESENT | USER)
            == PRESENT | USER
            && core::ptr::read_volatile(pdpt as usize as *const u64) & (PRESENT | USER)
                == PRESENT | USER
            && core::ptr::read_volatile(directory as usize as *const u64) & (PRESENT | USER)
                == PRESENT | USER
    };
    let leaves_valid = (0..PROCESS_CODE_PAGES).all(|page| {
        let entry = unsafe { core::ptr::read_volatile((table as usize as *const u64).add(page)) };
        if !required[page] {
            entry & PRESENT == 0
        } else {
            entry & (PRESENT | USER) == PRESENT | USER
                && (entry & WRITABLE != 0) == writable[page]
                && (!state.nx_enabled || (entry & NO_EXECUTE == 0) == executable[page])
        }
    });
    let guard_entry = unsafe {
        core::ptr::read_volatile((table as usize as *const u64).add(PROCESS_GUARD_INDEX))
    };
    let stacks_valid = (0..PROCESS_STACK_PAGES).all(|page| {
        let entry = unsafe {
            core::ptr::read_volatile(
                (table as usize as *const u64).add(PROCESS_STACK_FIRST_INDEX + page),
            )
        };
        entry & (PRESENT | WRITABLE | USER) == PRESENT | WRITABLE | USER
            && (!state.nx_enabled || entry & NO_EXECUTE != 0)
    });
    let virtual_base = canonical_base(PROCESS_USER_SLOT);
    Some(ProcessAddressSpace {
        root_physical: pml4,
        entry: virtual_base.checked_add(image.entry())?,
        stack_top: virtual_base
            + (PROCESS_STACK_FIRST_INDEX + PROCESS_STACK_PAGES) as u64 * PAGE_SIZE
            - 16,
        code_physical,
        code_flags,
        stack_physical,
        nx_enabled: state.nx_enabled,
        verified: hierarchy_valid && leaves_valid && guard_entry & PRESENT == 0 && stacks_valid,
        owns_code: true,
        allocation_base: block,
        allocation_pages: 4 + PROCESS_STACK_PAGES as u64,
        heap_mapped: 0,
        heap_physical: [0; PROCESS_HEAP_PAGES],
        mmap_used: [false; PROCESS_MMAP_PAGES],
        mmap_protection: [0; PROCESS_MMAP_PAGES],
        mmap_physical: [0; PROCESS_MMAP_PAGES],
    })
}

pub fn fork_process(parent: &ProcessAddressSpace) -> Option<ProcessAddressSpace> {
    let (block, pml4, pdpt, directory, table) =
        allocate_process_tables(PROCESS_STACK_PAGES as u64)?;
    let stack_physical = table.checked_add(PAGE_SIZE)?;
    unsafe {
        core::ptr::copy_nonoverlapping(
            parent.stack_physical as usize as *const u8,
            stack_physical as usize as *mut u8,
            PROCESS_STACK_PAGES * PAGE_SIZE as usize,
        );
    }
    let parent_root = parent.root_physical as usize as *const u64;
    let new_root = pml4 as usize as *mut u64;
    unsafe {
        core::ptr::copy_nonoverlapping(parent_root, new_root, ENTRY_COUNT);
    }
    // Everything mapped page by page is shared, not copied: read-only pages
    // stay as they are, and writable ones (.data/.bss, heap, anonymous mmap)
    // become read-only copy-on-write in BOTH address spaces. The first write
    // by either process faults and gets its own private copy
    // (`process_cow_fault`), so fork() costs page-table work, not memory.
    let parent_table = process_table_physical(parent.root_physical)?;
    let virtual_base = canonical_base(PROCESS_USER_SLOT);
    let share_page = |slot: usize, physical: u64, logical_flags: u64| -> Option<u64> {
        let parent_entry_ptr = unsafe { (parent_table as usize as *mut u64).add(slot) };
        let mut entry = unsafe { core::ptr::read_volatile(parent_entry_ptr) };
        if entry & PRESENT == 0 {
            entry = physical | logical_flags;
        }
        if !frame_share(physical) {
            return None;
        }
        if entry & WRITABLE != 0 {
            entry = (entry & !WRITABLE) | COW;
            unsafe {
                core::ptr::write_volatile(parent_entry_ptr, entry);
                invalidate(virtual_base + slot as u64 * PAGE_SIZE);
            }
        }
        Some(entry)
    };
    let code_physical = parent.code_physical;
    for (page, physical) in code_physical.iter().enumerate() {
        if *physical == 0 {
            continue;
        }
        let entry = share_page(page, *physical, *physical | parent.code_flags[page])?;
        unsafe {
            core::ptr::write_volatile((table as usize as *mut u64).add(page), entry);
        }
    }
    link_process_root(
        parent.nx_enabled,
        new_root,
        pdpt,
        directory,
        table,
        stack_physical,
    );
    let stack_flags = PRESENT | WRITABLE | USER | if parent.nx_enabled { NO_EXECUTE } else { 0 };
    let heap_physical = parent.heap_physical;
    for (index, physical) in heap_physical.iter().enumerate().take(parent.heap_mapped) {
        let slot = PROCESS_HEAP_FIRST_INDEX + index;
        let entry = share_page(slot, *physical, *physical | stack_flags)?;
        unsafe {
            core::ptr::write_volatile((table as usize as *mut u64).add(slot), entry);
        }
    }
    let mmap_used = parent.mmap_used;
    let mmap_protection = parent.mmap_protection;
    let mmap_physical = parent.mmap_physical;
    for index in 0..PROCESS_MMAP_PAGES {
        if !mmap_used[index] {
            continue;
        }
        let slot = PROCESS_MMAP_FIRST_INDEX + index;
        let flags = translate_protection(mmap_protection[index], parent.nx_enabled);
        let entry = share_page(slot, mmap_physical[index], mmap_physical[index] | flags)?;
        unsafe {
            core::ptr::write_volatile((table as usize as *mut u64).add(slot), entry);
        }
    }
    let hierarchy_user = unsafe {
        core::ptr::read_volatile(new_root.add(PROCESS_USER_SLOT)) & USER != 0
            && core::ptr::read_volatile(pdpt as usize as *const u64) & USER != 0
            && core::ptr::read_volatile(directory as usize as *const u64) & USER != 0
    };
    let code_matches = (0..PROCESS_CODE_PAGES).all(|page| {
        let entry = unsafe { core::ptr::read_volatile((table as usize as *const u64).add(page)) };
        if code_physical[page] == 0 {
            entry == 0
        } else {
            entry & PRESENT != 0 && entry & ADDRESS_MASK == code_physical[page]
        }
    });
    let stacks_valid = (0..PROCESS_STACK_PAGES).all(|page| {
        let entry = unsafe {
            core::ptr::read_volatile(
                (table as usize as *const u64).add(PROCESS_STACK_FIRST_INDEX + page),
            )
        };
        entry & (PRESENT | WRITABLE | USER) == PRESENT | WRITABLE | USER
    });
    Some(ProcessAddressSpace {
        root_physical: pml4,
        entry: parent.entry,
        stack_top: parent.stack_top,
        code_physical,
        code_flags: parent.code_flags,
        stack_physical,
        nx_enabled: parent.nx_enabled,
        verified: hierarchy_user && code_matches && stacks_valid,
        owns_code: false,
        allocation_base: block,
        allocation_pages: 4 + PROCESS_STACK_PAGES as u64,
        heap_mapped: parent.heap_mapped,
        heap_physical,
        mmap_used,
        mmap_protection,
        mmap_physical,
    })
}

fn process_table_physical(root_physical: u64) -> Option<u64> {
    unsafe {
        let root = root_physical as usize as *const u64;
        let pdpt_entry = core::ptr::read_volatile(root.add(PROCESS_USER_SLOT));
        if pdpt_entry & PRESENT == 0 {
            return None;
        }
        let pdpt = (pdpt_entry & ADDRESS_MASK) as usize as *const u64;
        let directory_entry = core::ptr::read_volatile(pdpt);
        if directory_entry & PRESENT == 0 {
            return None;
        }
        let directory = (directory_entry & ADDRESS_MASK) as usize as *const u64;
        let table_entry = core::ptr::read_volatile(directory);
        if table_entry & PRESENT == 0 {
            return None;
        }
        Some(table_entry & ADDRESS_MASK)
    }
}

pub fn exec_process(space: &mut ProcessAddressSpace, code: &[u8]) -> bool {
    match crate::elf::ElfImage::parse(code) {
        Ok(image) => exec_process_elf(space, &image),
        Err(_) => exec_process_raw(space, code),
    }
}

/// Shared by both exec paths: unmaps and releases every code page the
/// process currently has (there may be more or fewer than the incoming
/// image needs - code always reserves the same fixed slot range, so this
/// never has to touch heap/mmap/stack indices), tears down the old
/// heap/mmap regions (execve() replaces the whole image), then leaves
/// `space` ready for the caller to map the new pages into the same table.
fn exec_teardown(space: &mut ProcessAddressSpace, table: u64) {
    let virtual_base = canonical_base(PROCESS_USER_SLOT);
    unsafe {
        for page in 0..PROCESS_CODE_PAGES {
            if space.code_physical[page] == 0 {
                continue;
            }
            core::ptr::write_volatile((table as usize as *mut u64).add(page), 0);
            invalidate(virtual_base + page as u64 * PAGE_SIZE);
        }
        for stack_page in 0..PROCESS_STACK_PAGES as u64 {
            zero_page(space.stack_physical + stack_page * PAGE_SIZE);
        }
        for index in 0..space.heap_mapped {
            core::ptr::write_volatile(
                (table as usize as *mut u64).add(PROCESS_HEAP_FIRST_INDEX + index),
                0,
            );
            invalidate(virtual_base + (PROCESS_HEAP_FIRST_INDEX + index) as u64 * PAGE_SIZE);
        }
        for (index, used) in space.mmap_used.iter().enumerate() {
            if !used {
                continue;
            }
            core::ptr::write_volatile(
                (table as usize as *mut u64).add(PROCESS_MMAP_FIRST_INDEX + index),
                0,
            );
            invalidate(virtual_base + (PROCESS_MMAP_FIRST_INDEX + index) as u64 * PAGE_SIZE);
        }
    }
    for page in space.code_physical.iter() {
        if *page != 0 {
            frame_release(*page);
        }
    }
    for page in space.heap_physical.iter().take(space.heap_mapped) {
        frame_release(*page);
    }
    for (index, used) in space.mmap_used.iter().enumerate() {
        if *used {
            frame_release(space.mmap_physical[index]);
        }
    }
    space.code_physical = [0; PROCESS_CODE_PAGES];
    space.code_flags = [0; PROCESS_CODE_PAGES];
    space.heap_mapped = 0;
    space.heap_physical = [0; PROCESS_HEAP_PAGES];
    space.mmap_used = [false; PROCESS_MMAP_PAGES];
    space.mmap_protection = [0; PROCESS_MMAP_PAGES];
    space.mmap_physical = [0; PROCESS_MMAP_PAGES];
}

fn exec_process_raw(space: &mut ProcessAddressSpace, code: &[u8]) -> bool {
    if code.is_empty() || code.len() > PAGE_SIZE as usize {
        return false;
    }
    let Some(table) = process_table_physical(space.root_physical) else {
        return false;
    };
    let Some(frame) = crate::memory::allocate_global() else {
        return false;
    };
    let new_code_physical = frame.address();
    let code_flags = PRESENT | USER;
    unsafe {
        zero_page(new_code_physical);
        core::ptr::copy_nonoverlapping(
            code.as_ptr(),
            new_code_physical as usize as *mut u8,
            code.len(),
        );
    }
    exec_teardown(space, table);
    frame_retain(new_code_physical);
    unsafe {
        core::ptr::write_volatile(table as usize as *mut u64, new_code_physical | code_flags);
        invalidate(canonical_base(PROCESS_USER_SLOT));
    }
    space.code_physical[0] = new_code_physical;
    space.code_flags[0] = code_flags;
    space.owns_code = true;
    true
}

fn exec_process_elf(space: &mut ProcessAddressSpace, image: &crate::elf::ElfImage<'_>) -> bool {
    let mut required = [false; PROCESS_CODE_PAGES];
    let mut writable = [false; PROCESS_CODE_PAGES];
    let mut executable = [false; PROCESS_CODE_PAGES];
    for segment in image.segments() {
        let Ok(first) = usize::try_from(segment.virtual_address / PAGE_SIZE) else {
            return false;
        };
        let Some(end_bytes) = segment.memory_end().checked_add(PAGE_SIZE - 1) else {
            return false;
        };
        let Ok(end) = usize::try_from(end_bytes / PAGE_SIZE) else {
            return false;
        };
        if first >= end || end > PROCESS_CODE_PAGES {
            return false;
        }
        for page in first..end {
            required[page] = true;
            writable[page] |= segment.writable;
            executable[page] |= segment.executable;
            if writable[page] && executable[page] {
                return false;
            }
        }
    }
    let Ok(entry_page) = usize::try_from(image.entry() / PAGE_SIZE) else {
        return false;
    };
    if entry_page >= PROCESS_CODE_PAGES || !executable[entry_page] || !required.iter().any(|n| *n) {
        return false;
    }
    let Some(table) = process_table_physical(space.root_physical) else {
        return false;
    };

    // Allocate and populate every new page before touching `space` at all,
    // so a mid-way allocation failure leaves the process's previous image
    // completely intact instead of half-torn-down.
    let mut new_physical = [0u64; PROCESS_CODE_PAGES];
    let mut new_flags = [0u64; PROCESS_CODE_PAGES];
    for page in 0..PROCESS_CODE_PAGES {
        if !required[page] {
            continue;
        }
        let Some(frame) = crate::memory::allocate_global() else {
            for allocated in new_physical.iter() {
                if *allocated != 0 {
                    crate::memory::release_global_address(*allocated);
                }
            }
            return false;
        };
        let physical = frame.address();
        unsafe { zero_page(physical) };
        let mut flags = PRESENT | USER;
        if writable[page] {
            flags |= WRITABLE;
        }
        if !executable[page] && space.nx_enabled {
            flags |= NO_EXECUTE;
        }
        new_physical[page] = physical;
        new_flags[page] = flags;
    }
    for segment in image.segments() {
        let mut source = segment.file_offset;
        let mut virtual_offset = segment.virtual_address as usize;
        let mut remaining = segment.file_size;
        while remaining != 0 {
            let page = virtual_offset / PAGE_SIZE as usize;
            let within_page = virtual_offset & (PAGE_SIZE as usize - 1);
            let count = remaining.min(PAGE_SIZE as usize - within_page);
            let Some(destination) = new_physical[page].checked_add(within_page as u64) else {
                for allocated in new_physical.iter() {
                    if *allocated != 0 {
                        crate::memory::release_global_address(*allocated);
                    }
                }
                return false;
            };
            unsafe {
                core::ptr::copy_nonoverlapping(
                    image.bytes().as_ptr().add(source),
                    destination as usize as *mut u8,
                    count,
                );
            }
            source += count;
            virtual_offset += count;
            remaining -= count;
        }
    }

    exec_teardown(space, table);
    let virtual_base = canonical_base(PROCESS_USER_SLOT);
    unsafe {
        for page in 0..PROCESS_CODE_PAGES {
            if new_physical[page] == 0 {
                continue;
            }
            frame_retain(new_physical[page]);
            core::ptr::write_volatile(
                (table as usize as *mut u64).add(page),
                new_physical[page] | new_flags[page],
            );
            invalidate(virtual_base + page as u64 * PAGE_SIZE);
        }
    }
    space.code_physical = new_physical;
    space.code_flags = new_flags;
    space.entry = virtual_base + image.entry();
    space.owns_code = true;
    true
}

pub fn destroy_process(space: &ProcessAddressSpace) -> bool {
    for page in space.code_physical.iter() {
        if *page != 0 {
            let _ = frame_release(*page);
        }
    }
    for page in space.heap_physical.iter().take(space.heap_mapped) {
        frame_release(*page);
    }
    for (index, used) in space.mmap_used.iter().enumerate() {
        if *used {
            frame_release(space.mmap_physical[index]);
        }
    }
    crate::memory::release_global_contiguous(space.allocation_base, space.allocation_pages)
}

pub fn process_brk(space: &mut ProcessAddressSpace, additional_pages: usize) -> Option<u64> {
    let brk_address = |mapped: usize| {
        process_virtual_base() + (PROCESS_HEAP_FIRST_INDEX + mapped) as u64 * PAGE_SIZE
    };
    if additional_pages == 0 {
        return Some(brk_address(space.heap_mapped));
    }
    if space.heap_mapped + additional_pages > PROCESS_HEAP_PAGES {
        return None;
    }
    let table = process_table_physical(space.root_physical)?;
    let flags = PRESENT | WRITABLE | USER | if space.nx_enabled { NO_EXECUTE } else { 0 };
    for _ in 0..additional_pages {
        let frame = crate::memory::allocate_global()?;
        let page = frame.address();
        unsafe {
            zero_page(page);
            core::ptr::write_volatile(
                (table as usize as *mut u64).add(PROCESS_HEAP_FIRST_INDEX + space.heap_mapped),
                page | flags,
            );
            invalidate(brk_address(space.heap_mapped));
        }
        space.heap_physical[space.heap_mapped] = page;
        space.heap_mapped += 1;
    }
    Some(brk_address(space.heap_mapped))
}

fn translate_protection(protection: u8, nx_enabled: bool) -> u64 {
    let mut flags = PRESENT | USER;
    if protection & 2 != 0 {
        flags |= WRITABLE;
    }
    if nx_enabled && protection & 4 == 0 {
        flags |= NO_EXECUTE;
    }
    flags
}

fn mmap_address(index: usize) -> u64 {
    process_virtual_base() + (PROCESS_MMAP_FIRST_INDEX + index) as u64 * PAGE_SIZE
}

fn mmap_index_for_address(address: u64) -> Option<usize> {
    let base = mmap_address(0);
    if address < base || address & (PAGE_SIZE - 1) != 0 {
        return None;
    }
    let index = usize::try_from((address - base) / PAGE_SIZE).ok()?;
    (index < PROCESS_MMAP_PAGES).then_some(index)
}

/// Finds `pages` contiguous unused slots in this process's own dedicated
/// mmap region (32 pages, right after its heap, in the same per-process
/// leaf page table), maps fresh zeroed physical pages with the requested
/// protection, and returns the new mapping's base virtual address.
/// File-backed mmap is layered on top via `process_mmap_write` (the
/// syscall layer allocates through here first either way, then streams
/// file content in only for the file-backed case) - callers without a
/// `process_space` keep using the original `user_mmap` singleton path.
pub fn process_mmap(space: &mut ProcessAddressSpace, pages: usize, protection: u8) -> Option<u64> {
    if pages == 0 || pages > PROCESS_MMAP_PAGES {
        return None;
    }
    let start = (0..=PROCESS_MMAP_PAGES - pages).find(|&start| {
        space.mmap_used[start..start + pages]
            .iter()
            .all(|used| !used)
    })?;
    let table = process_table_physical(space.root_physical)?;
    let flags = translate_protection(protection, space.nx_enabled);
    for index in start..start + pages {
        let frame = crate::memory::allocate_global()?;
        let page = frame.address();
        unsafe {
            zero_page(page);
            core::ptr::write_volatile(
                (table as usize as *mut u64).add(PROCESS_MMAP_FIRST_INDEX + index),
                page | flags,
            );
            invalidate(mmap_address(index));
        }
        space.mmap_used[index] = true;
        space.mmap_protection[index] = protection;
        space.mmap_physical[index] = page;
    }
    Some(mmap_address(start))
}

/// Writes file content into an already-mapped `process_mmap` region -
/// the file-backed half of file-backed mmap: the syscall layer allocates
/// zeroed pages via `process_mmap` first (identical to anonymous mmap),
/// then streams the file's bytes in here. `address` must be a base
/// returned by `process_mmap`; `offset` is the byte offset within that
/// mapping (not the file), so multi-chunk reads can call this repeatedly.
pub fn process_mmap_write(
    space: &ProcessAddressSpace,
    address: u64,
    offset: usize,
    source: &[u8],
) -> bool {
    if source.is_empty() {
        return true;
    }
    let Some(start_index) = mmap_index_for_address(address) else {
        return false;
    };
    let Some(absolute_offset) = start_index
        .checked_mul(PAGE_SIZE as usize)
        .and_then(|base| base.checked_add(offset))
    else {
        return false;
    };
    let Some(end) = absolute_offset.checked_add(source.len()) else {
        return false;
    };
    if end > PROCESS_MMAP_PAGES * PAGE_SIZE as usize {
        return false;
    }
    let mut written = 0usize;
    while written < source.len() {
        let current = absolute_offset + written;
        let page = current / PAGE_SIZE as usize;
        let within = current % PAGE_SIZE as usize;
        if !space.mmap_used[page] {
            return false;
        }
        let count = (PAGE_SIZE as usize - within).min(source.len() - written);
        let destination = space.mmap_physical[page] + within as u64;
        unsafe {
            core::ptr::copy_nonoverlapping(
                source.as_ptr().add(written),
                destination as usize as *mut u8,
                count,
            );
        }
        written += count;
    }
    true
}

pub fn process_mprotect(
    space: &mut ProcessAddressSpace,
    address: u64,
    pages: usize,
    protection: u8,
) -> bool {
    let Some(start) = mmap_index_for_address(address) else {
        return false;
    };
    if pages == 0 || start + pages > PROCESS_MMAP_PAGES {
        return false;
    }
    if space.mmap_used[start..start + pages]
        .iter()
        .any(|used| !used)
    {
        return false;
    }
    let Some(table) = process_table_physical(space.root_physical) else {
        return false;
    };
    let flags = translate_protection(protection, space.nx_enabled);
    for index in start..start + pages {
        space.mmap_protection[index] = protection;
        let slot = (table as usize as *mut u64).wrapping_add(PROCESS_MMAP_FIRST_INDEX + index);
        // A page still shared with another process must stay copy-on-write
        // even when it becomes writable, or the write would hit both copies.
        let shared = unsafe { core::ptr::read_volatile(slot) } & COW != 0
            || code_page_refcount(space.mmap_physical[index]) > 1;
        let flags = if shared && flags & WRITABLE != 0 {
            (flags & !WRITABLE) | COW
        } else {
            flags
        };
        unsafe {
            core::ptr::write_volatile(slot, space.mmap_physical[index] | flags);
            invalidate(mmap_address(index));
        }
    }
    true
}

static COW_FAULTS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static COW_COPIES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// (faults handled, private copies made) since boot.
pub fn cow_stats() -> (u64, u64) {
    (
        COW_FAULTS.load(core::sync::atomic::Ordering::Relaxed),
        COW_COPIES.load(core::sync::atomic::Ordering::Relaxed),
    )
}

/// Write fault on a present, read-only, copy-on-write page of this process:
/// give the writer a private copy (or, if it is the last owner, just make the
/// page writable again). Returns false when the fault is not a COW fault.
pub fn process_cow_fault(space: &mut ProcessAddressSpace, address: u64) -> bool {
    let base = canonical_base(PROCESS_USER_SLOT);
    if address < base {
        return false;
    }
    let slot = ((address - base) / PAGE_SIZE) as usize;
    let Some(table) = process_table_physical(space.root_physical) else {
        return false;
    };
    if slot >= PROCESS_RESERVED_PAGES {
        return false;
    }
    let entry_ptr = unsafe { (table as usize as *mut u64).add(slot) };
    let entry = unsafe { core::ptr::read_volatile(entry_ptr) };
    if entry & (PRESENT | COW) != (PRESENT | COW) {
        return false;
    }
    let old = entry & ADDRESS_MASK;
    let heap_slot = slot
        .checked_sub(PROCESS_HEAP_FIRST_INDEX)
        .filter(|i| *i < space.heap_mapped);
    let mmap_slot = slot
        .checked_sub(PROCESS_MMAP_FIRST_INDEX)
        .filter(|i| *i < PROCESS_MMAP_PAGES && space.mmap_used[*i]);
    let owner_ok = if slot < PROCESS_CODE_PAGES {
        space.code_physical[slot] == old
    } else if let Some(index) = heap_slot {
        space.heap_physical[index] == old
    } else if let Some(index) = mmap_slot {
        space.mmap_physical[index] == old
    } else {
        false
    };
    if !owner_ok {
        return false;
    }
    COW_FAULTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let writable = (entry & !COW & !ADDRESS_MASK) | WRITABLE;
    let virtual_address = base + slot as u64 * PAGE_SIZE;
    if code_page_refcount(old) <= 1 {
        unsafe {
            core::ptr::write_volatile(entry_ptr, old | writable);
            invalidate(virtual_address);
        }
        return true;
    }
    let Some(frame) = crate::memory::allocate_global() else {
        return false;
    };
    let copy = frame.address();
    unsafe {
        core::ptr::copy_nonoverlapping(
            old as usize as *const u8,
            copy as usize as *mut u8,
            PAGE_SIZE as usize,
        );
        core::ptr::write_volatile(entry_ptr, copy | writable);
        invalidate(virtual_address);
    }
    COW_COPIES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if slot < PROCESS_CODE_PAGES {
        space.code_physical[slot] = copy;
    } else if let Some(index) = heap_slot {
        space.heap_physical[index] = copy;
    } else if let Some(index) = mmap_slot {
        space.mmap_physical[index] = copy;
    }
    frame_release(old);
    true
}

pub fn process_munmap(space: &mut ProcessAddressSpace, address: u64, pages: usize) -> bool {
    let Some(start) = mmap_index_for_address(address) else {
        return false;
    };
    if pages == 0 || start + pages > PROCESS_MMAP_PAGES {
        return false;
    }
    if space.mmap_used[start..start + pages]
        .iter()
        .any(|used| !used)
    {
        return false;
    }
    let Some(table) = process_table_physical(space.root_physical) else {
        return false;
    };
    for index in start..start + pages {
        unsafe {
            core::ptr::write_volatile(
                (table as usize as *mut u64).add(PROCESS_MMAP_FIRST_INDEX + index),
                0,
            );
            invalidate(mmap_address(index));
        }
        frame_release(space.mmap_physical[index]);
        space.mmap_used[index] = false;
        space.mmap_protection[index] = 0;
        space.mmap_physical[index] = 0;
    }
    true
}

pub fn map_user_image(
    state: &PagingState,
    frames: &mut FrameAllocator,
    image: &ElfImage<'_>,
) -> Option<UserImageMapping> {
    let mut required = [false; USER_IMAGE_PAGES];
    let mut writable = [false; USER_IMAGE_PAGES];
    let mut executable = [false; USER_IMAGE_PAGES];
    for segment in image.segments() {
        let first = usize::try_from(segment.virtual_address / PAGE_SIZE).ok()?;
        let end = segment.memory_end().checked_add(PAGE_SIZE - 1)? / PAGE_SIZE;
        let end = usize::try_from(end).ok()?;
        if first >= end || end > USER_IMAGE_PAGES {
            return None;
        }
        for page in first..end {
            required[page] = true;
            writable[page] |= segment.writable;
            executable[page] |= segment.executable;
            if writable[page] && executable[page] {
                return None;
            }
        }
    }
    let mapped_pages = required.iter().filter(|needed| **needed).count();
    let total_pages = 3usize
        .checked_add(mapped_pages)?
        .checked_add(USER_STACK_PAGES)?
        .checked_add(ANONYMOUS_PAGES)?;
    let root = state.root_physical as usize as *mut u64;
    let user_index = choose_user_index(root)?;
    let block = frames.allocate_contiguous(total_pages as u64, 1)?.address();
    let pdpt = block;
    let directory = block.checked_add(PAGE_SIZE)?;
    let table = block.checked_add(PAGE_SIZE * 2)?;
    unsafe {
        core::ptr::write_bytes(
            block as usize as *mut u8,
            0,
            total_pages * PAGE_SIZE as usize,
        );
    }
    let load_bias = canonical_base(user_index);
    let branch_flags = PRESENT | WRITABLE | USER;
    let mut page_physical = [0u64; USER_IMAGE_PAGES];
    let mut physical = block.checked_add(PAGE_SIZE * 3)?;
    let mut executable_pages = 0;
    let mut writable_pages = 0;
    for page in 0..USER_IMAGE_PAGES {
        if !required[page] {
            continue;
        }
        page_physical[page] = physical;
        let mut flags = PRESENT | USER;
        if writable[page] {
            flags |= WRITABLE;
            writable_pages += 1;
        }
        if executable[page] {
            executable_pages += 1;
        } else if state.nx_enabled {
            flags |= NO_EXECUTE;
        }
        unsafe {
            core::ptr::write_volatile((table as usize as *mut u64).add(page), physical | flags);
        }
        physical = physical.checked_add(PAGE_SIZE)?;
    }
    for segment in image.segments() {
        let mut source = segment.file_offset;
        let mut virtual_offset = segment.virtual_address as usize;
        let mut remaining = segment.file_size;
        while remaining != 0 {
            let page = virtual_offset / PAGE_SIZE as usize;
            let within_page = virtual_offset & (PAGE_SIZE as usize - 1);
            let count = remaining.min(PAGE_SIZE as usize - within_page);
            let destination =
                page_physical[page].checked_add(within_page as u64)? as usize as *mut u8;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    image.bytes().as_ptr().add(source),
                    destination,
                    count,
                );
            }
            source = source.checked_add(count)?;
            virtual_offset = virtual_offset.checked_add(count)?;
            remaining -= count;
        }
    }
    let stack_physical = physical;
    let anonymous_physical = stack_physical + USER_STACK_PAGES as u64 * PAGE_SIZE;
    let stack_flags = PRESENT | WRITABLE | USER | if state.nx_enabled { NO_EXECUTE } else { 0 };
    for stack_page in 0..USER_STACK_PAGES {
        unsafe {
            core::ptr::write_volatile(
                (table as usize as *mut u64).add(USER_STACK_FIRST_PAGE + stack_page),
                (stack_physical + stack_page as u64 * PAGE_SIZE) | stack_flags,
            );
        }
    }
    unsafe {
        core::ptr::write_volatile(root.add(user_index), pdpt | branch_flags);
        core::ptr::write_volatile(pdpt as usize as *mut u64, directory | branch_flags);
        core::ptr::write_volatile(directory as usize as *mut u64, table | branch_flags);
        write_cr3(state.root_physical);
    }
    let hierarchy_valid = unsafe {
        core::ptr::read_volatile(root.add(user_index)) & (PRESENT | USER) == PRESENT | USER
            && core::ptr::read_volatile(pdpt as usize as *const u64) & (PRESENT | USER)
                == PRESENT | USER
            && core::ptr::read_volatile(directory as usize as *const u64) & (PRESENT | USER)
                == PRESENT | USER
    };
    let leaves_valid = (0..USER_IMAGE_PAGES).all(|page| {
        let entry = unsafe { core::ptr::read_volatile((table as usize as *const u64).add(page)) };
        if !required[page] {
            return entry & PRESENT == 0;
        }
        entry & (PRESENT | USER) == PRESENT | USER
            && (entry & WRITABLE != 0) == writable[page]
            && (!state.nx_enabled || (entry & NO_EXECUTE == 0) == executable[page])
    });
    let guard_entry =
        unsafe { core::ptr::read_volatile((table as usize as *const u64).add(USER_GUARD_PAGE)) };
    let stacks_valid = (0..USER_STACK_PAGES).all(|page| {
        let entry = unsafe {
            core::ptr::read_volatile(
                (table as usize as *const u64).add(USER_STACK_FIRST_PAGE + page),
            )
        };
        entry & (PRESENT | WRITABLE | USER) == PRESENT | WRITABLE | USER
            && (!state.nx_enabled || entry & NO_EXECUTE != 0)
    });
    let entry_page = usize::try_from(image.entry() / PAGE_SIZE).ok()?;
    let entry_valid = entry_page < USER_IMAGE_PAGES && executable[entry_page];
    Some(UserImageMapping {
        load_bias,
        entry: load_bias.checked_add(image.entry())?,
        stack_top: load_bias.checked_add(ENTRY_COUNT as u64 * PAGE_SIZE - 16)?,
        mapped_pages,
        executable_pages,
        writable_pages,
        stack_pages: USER_STACK_PAGES,
        stack_physical,
        guard_page: load_bias + USER_GUARD_PAGE as u64 * PAGE_SIZE,
        verified: hierarchy_valid
            && leaves_valid
            && guard_entry & PRESENT == 0
            && stacks_valid
            && entry_valid,
        anonymous_physical,
        page_table_physical: table,
        nx_enabled: state.nx_enabled,
        allocation_base: block,
        allocation_pages: total_pages as u64,
        root_index: user_index,
    })
}

pub fn destroy_user_probe(
    state: &PagingState,
    frames: &mut FrameAllocator,
    mapping: &UserMapping,
) -> UserReapReport {
    destroy_user_mapping(
        state,
        frames,
        mapping.root_index,
        mapping.allocation_base,
        mapping.allocation_pages,
    )
}

pub fn destroy_user_image(
    state: &PagingState,
    frames: &mut FrameAllocator,
    mapping: &UserImageMapping,
) -> UserReapReport {
    deactivate_user_memory();
    destroy_user_mapping(
        state,
        frames,
        mapping.root_index,
        mapping.allocation_base,
        mapping.allocation_pages,
    )
}

fn destroy_user_mapping(
    state: &PagingState,
    frames: &mut FrameAllocator,
    root_index: usize,
    allocation_base: u64,
    allocation_pages: u64,
) -> UserReapReport {
    let root = state.root_physical as usize as *mut u64;
    let entry = unsafe { core::ptr::read_volatile(root.add(root_index)) };
    if entry & (PRESENT | ADDRESS_MASK) != allocation_base | PRESENT {
        return UserReapReport {
            released_pages: 0,
            address_space_removed: false,
            scrubbed: false,
            verified: false,
        };
    }
    unsafe {
        core::ptr::write_volatile(root.add(root_index), 0);
        write_cr3(state.root_physical);
    }
    let address_space_removed =
        unsafe { core::ptr::read_volatile(root.add(root_index)) & PRESENT == 0 };
    let Some(bytes) = allocation_pages.checked_mul(PAGE_SIZE) else {
        return UserReapReport {
            released_pages: 0,
            address_space_removed,
            scrubbed: false,
            verified: false,
        };
    };
    unsafe {
        core::ptr::write_bytes(allocation_base as usize as *mut u8, 0, bytes as usize);
    }
    let scrubbed = unsafe {
        core::ptr::read_volatile(allocation_base as usize as *const u64) == 0
            && core::ptr::read_volatile((allocation_base + bytes - 8) as usize as *const u64) == 0
    };
    let released = frames.release_contiguous(allocation_base, allocation_pages);
    UserReapReport {
        released_pages: if released { allocation_pages } else { 0 },
        address_space_removed,
        scrubbed,
        verified: address_space_removed && scrubbed && released,
    }
}

pub fn activate_user_memory(mapping: &UserImageMapping) {
    let mut memory = USER_MEMORY.lock();
    *memory = UserMemoryState::EMPTY;
    memory.base = mapping.load_bias + ANONYMOUS_FIRST_PAGE as u64 * PAGE_SIZE;
    memory.table = mapping.page_table_physical;
    memory.current_brk = memory.base;
    memory.nx_enabled = mapping.nx_enabled;
    memory.active = true;
    for index in 0..ANONYMOUS_PAGES {
        memory.physical[index] = mapping.anonymous_physical + index as u64 * PAGE_SIZE;
    }
}

pub fn deactivate_user_memory() {
    *USER_MEMORY.lock() = UserMemoryState::EMPTY;
}

pub fn user_brk(request: u64) -> u64 {
    let mut memory = USER_MEMORY.lock();
    if !memory.active {
        return 0;
    }
    let old = memory.current_brk;
    if request == 0 {
        return old;
    }
    let limit = memory.base + BRK_PAGES as u64 * PAGE_SIZE;
    if request < memory.base || request > limit {
        return old;
    }
    let old_pages = pages_for(old.saturating_sub(memory.base));
    let new_pages = pages_for(request.saturating_sub(memory.base));
    if new_pages > old_pages {
        for index in old_pages..new_pages {
            if memory.used[index] {
                return old;
            }
        }
        for index in old_pages..new_pages {
            map_anonymous_page(&mut memory, index, 3);
        }
    } else {
        for index in new_pages..old_pages {
            clear_anonymous_page(&mut memory, index);
        }
    }
    memory.current_brk = request;
    request
}

pub fn user_mmap(length: usize, protection: u8) -> Result<u64, UserMemoryError> {
    validate_protection(protection)?;
    if length == 0 {
        return Err(UserMemoryError::InvalidArgument);
    }
    let pages = length.div_ceil(PAGE_SIZE as usize);
    let mut memory = USER_MEMORY.lock();
    if !memory.active || pages > ANONYMOUS_PAGES - BRK_PAGES {
        return Err(UserMemoryError::OutOfMemory);
    }
    let start = (BRK_PAGES..=ANONYMOUS_PAGES - pages)
        .find(|start| {
            memory.used[*start..*start + pages]
                .iter()
                .all(|used| !*used)
        })
        .ok_or(UserMemoryError::OutOfMemory)?;
    for index in start..start + pages {
        map_anonymous_page(&mut memory, index, protection);
    }
    Ok(memory.base + start as u64 * PAGE_SIZE)
}

pub fn user_write_mapping(
    address: u64,
    offset: usize,
    source: &[u8],
) -> Result<(), UserMemoryError> {
    if source.is_empty() {
        return Ok(());
    }
    let memory = USER_MEMORY.lock();
    if !memory.active || address < memory.base || address & (PAGE_SIZE - 1) != 0 {
        return Err(UserMemoryError::InvalidArgument);
    }
    let mapping_offset =
        usize::try_from(address - memory.base).map_err(|_| UserMemoryError::InvalidArgument)?;
    let start = mapping_offset
        .checked_add(offset)
        .ok_or(UserMemoryError::InvalidArgument)?;
    let end = start
        .checked_add(source.len())
        .ok_or(UserMemoryError::InvalidArgument)?;
    if end > ANONYMOUS_PAGES * PAGE_SIZE as usize {
        return Err(UserMemoryError::InvalidArgument);
    }
    let first_page = start / PAGE_SIZE as usize;
    let last_page = (end - 1) / PAGE_SIZE as usize;
    if memory.used[first_page..=last_page]
        .iter()
        .any(|used| !*used)
    {
        return Err(UserMemoryError::NotMapped);
    }
    let mut copied = 0usize;
    while copied < source.len() {
        let position = start + copied;
        let page = position / PAGE_SIZE as usize;
        let in_page = position % PAGE_SIZE as usize;
        let amount = (source.len() - copied).min(PAGE_SIZE as usize - in_page);
        unsafe {
            core::ptr::copy_nonoverlapping(
                source[copied..copied + amount].as_ptr(),
                (memory.physical[page] as usize + in_page) as *mut u8,
                amount,
            );
        }
        copied += amount;
    }
    Ok(())
}

pub fn user_mprotect(address: u64, length: usize, protection: u8) -> Result<(), UserMemoryError> {
    validate_protection(protection)?;
    let (start, pages) = anonymous_range(address, length)?;
    let mut memory = USER_MEMORY.lock();
    if !memory.active || memory.used[start..start + pages].iter().any(|used| !*used) {
        return Err(UserMemoryError::NotMapped);
    }
    for index in start..start + pages {
        memory.protection[index] = protection;
        write_anonymous_entry(&memory, index);
    }
    Ok(())
}

pub fn user_munmap(address: u64, length: usize) -> Result<(), UserMemoryError> {
    let (start, pages) = anonymous_range(address, length)?;
    if start < BRK_PAGES {
        return Err(UserMemoryError::InvalidArgument);
    }
    let mut memory = USER_MEMORY.lock();
    if !memory.active || memory.used[start..start + pages].iter().any(|used| !*used) {
        return Err(UserMemoryError::NotMapped);
    }
    for index in start..start + pages {
        clear_anonymous_page(&mut memory, index);
    }
    Ok(())
}

fn anonymous_range(address: u64, length: usize) -> Result<(usize, usize), UserMemoryError> {
    if length == 0 || address & (PAGE_SIZE - 1) != 0 {
        return Err(UserMemoryError::InvalidArgument);
    }
    let memory = USER_MEMORY.lock();
    if !memory.active || address < memory.base {
        return Err(UserMemoryError::InvalidArgument);
    }
    let start = usize::try_from((address - memory.base) / PAGE_SIZE)
        .map_err(|_| UserMemoryError::InvalidArgument)?;
    let pages = length.div_ceil(PAGE_SIZE as usize);
    if pages == 0
        || start
            .checked_add(pages)
            .is_none_or(|end| end > ANONYMOUS_PAGES)
    {
        return Err(UserMemoryError::InvalidArgument);
    }
    Ok((start, pages))
}

fn validate_protection(protection: u8) -> Result<(), UserMemoryError> {
    if protection & !7 != 0 || protection & 6 == 6 {
        Err(UserMemoryError::InvalidArgument)
    } else {
        Ok(())
    }
}

fn map_anonymous_page(memory: &mut UserMemoryState, index: usize, protection: u8) {
    unsafe {
        core::ptr::write_bytes(
            memory.physical[index] as usize as *mut u8,
            0,
            PAGE_SIZE as usize,
        );
    }
    memory.used[index] = true;
    memory.protection[index] = protection;
    write_anonymous_entry(memory, index);
}

fn clear_anonymous_page(memory: &mut UserMemoryState, index: usize) {
    let table_index = ANONYMOUS_FIRST_PAGE + index;
    unsafe {
        core::ptr::write_volatile((memory.table as usize as *mut u64).add(table_index), 0);
        core::ptr::write_bytes(
            memory.physical[index] as usize as *mut u8,
            0,
            PAGE_SIZE as usize,
        );
        invalidate(memory.base + index as u64 * PAGE_SIZE);
    }
    memory.used[index] = false;
    memory.protection[index] = 0;
}

fn write_anonymous_entry(memory: &UserMemoryState, index: usize) {
    let protection = memory.protection[index];
    let mut flags = USER;
    if protection != 0 {
        flags |= PRESENT;
    }
    if protection & 2 != 0 {
        flags |= WRITABLE;
    }
    if memory.nx_enabled && protection & 4 == 0 {
        flags |= NO_EXECUTE;
    }
    let table_index = ANONYMOUS_FIRST_PAGE + index;
    unsafe {
        core::ptr::write_volatile(
            (memory.table as usize as *mut u64).add(table_index),
            memory.physical[index] | flags,
        );
        invalidate(memory.base + index as u64 * PAGE_SIZE);
    }
}

fn pages_for(bytes: u64) -> usize {
    bytes.div_ceil(PAGE_SIZE) as usize
}

unsafe fn invalidate(address: u64) {
    unsafe {
        asm!("invlpg [{}]", in(reg) address, options(nostack, preserves_flags));
    }
}

pub fn user_range_accessible(address: u64, length: usize, write: bool) -> bool {
    if length == 0 {
        return true;
    }
    let Some(last) = address.checked_add(length as u64 - 1) else {
        return false;
    };
    if last > 0x0000_7fff_ffff_ffff {
        return false;
    }
    let mut page = address & !(PAGE_SIZE - 1);
    let last_page = last & !(PAGE_SIZE - 1);
    loop {
        if !user_page_accessible(page, write) {
            return false;
        }
        if page == last_page {
            return true;
        }
        let Some(next) = page.checked_add(PAGE_SIZE) else {
            return false;
        };
        page = next;
    }
}

fn user_page_accessible(address: u64, write: bool) -> bool {
    let indices = [
        ((address >> 39) & 0x1ff) as usize,
        ((address >> 30) & 0x1ff) as usize,
        ((address >> 21) & 0x1ff) as usize,
        ((address >> 12) & 0x1ff) as usize,
    ];
    let mut table = read_cr3() & ADDRESS_MASK;
    for (level, index) in indices.iter().enumerate() {
        if table == 0 {
            return false;
        }
        let mut entry =
            unsafe { core::ptr::read_volatile((table as usize as *const u64).add(*index)) };
        // A copy-on-write page is writable once it has been un-shared; do
        // that now so a syscall can fill a buffer that is still shared
        // with a forked relative.
        if write
            && level == indices.len() - 1
            && entry & (PRESENT | USER | WRITABLE | COW) == PRESENT | USER | COW
            && crate::scheduler::handle_cow_fault_current(address)
        {
            entry = unsafe { core::ptr::read_volatile((table as usize as *const u64).add(*index)) };
        }
        if entry & (PRESENT | USER) != PRESENT | USER || write && entry & WRITABLE == 0 {
            return false;
        }
        if level != indices.len() - 1 && entry & (1 << 7) != 0 {
            return false;
        }
        table = entry & ADDRESS_MASK;
    }
    true
}

fn canonical_base(index: usize) -> u64 {
    let address = (index as u64) << 39;
    if address & (1 << 47) != 0 {
        address | 0xffff_0000_0000_0000
    } else {
        address
    }
}

fn choose_user_index(root: *mut u64) -> Option<usize> {
    let start = crate::random::next_u64() as usize % 255 + 1;
    (0..255)
        .map(|offset| (start + offset - 1) % 255 + 1)
        .find(|index| unsafe { core::ptr::read_volatile(root.add(*index)) & PRESENT == 0 })
}

fn read_cr3() -> u64 {
    let value: u64;
    unsafe {
        asm!("mov {}, cr3", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}

unsafe fn write_cr3(value: u64) {
    unsafe {
        asm!("mov cr3, {}", in(reg) value, options(nostack, preserves_flags));
    }
}

pub fn activate_root(root_physical: u64) {
    unsafe {
        write_cr3(root_physical);
    }
}

fn enable_write_protect() -> bool {
    let mut value: u64;
    unsafe {
        asm!("mov {}, cr0", out(reg) value, options(nomem, nostack, preserves_flags));
        value |= CR0_WRITE_PROTECT;
        asm!("mov cr0, {}", in(reg) value, options(nostack, preserves_flags));
        asm!("mov {}, cr0", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value & CR0_WRITE_PROTECT != 0
}

fn enable_nx() -> bool {
    let mut value = read_msr(EFER_MSR);
    value |= EFER_NXE;
    unsafe {
        write_msr(EFER_MSR, value);
    }
    read_msr(EFER_MSR) & EFER_NXE != 0
}

fn read_msr(register: u32) -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") register,
            out("eax") low,
            out("edx") high,
            options(nomem, nostack, preserves_flags)
        );
    }
    low as u64 | (high as u64) << 32
}

unsafe fn write_msr(register: u32, value: u64) {
    unsafe {
        asm!(
            "wrmsr",
            in("ecx") register,
            in("eax") value as u32,
            in("edx") (value >> 32) as u32,
            options(nomem, nostack, preserves_flags)
        );
    }
}

unsafe fn zero_page(address: u64) {
    unsafe {
        core::ptr::write_bytes(address as usize as *mut u8, 0, PAGE_SIZE as usize);
    }
}
