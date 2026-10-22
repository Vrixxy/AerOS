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
    // A private copy of the identity map's PDPT and first-GiB directory (the
    // firmware's own pages may sit in memory a fixed-address process's
    // window shadows), which fixed-address processes start their tables from.
    if let Some(copy) = frames.allocate_contiguous(2, 1) {
        let pdpt_copy = copy.address();
        let directory_copy = pdpt_copy + PAGE_SIZE;
        // SAFETY: two fresh frames, and the inherited tables are identity-mapped.
        let built = unsafe {
            zero_page(pdpt_copy);
            zero_page(directory_copy);
            let first_pml4 = core::ptr::read_volatile(previous as usize as *const u64);
            if first_pml4 & PRESENT == 0 {
                false
            } else {
                let source = (first_pml4 & ADDRESS_MASK) as usize as *const u64;
                core::ptr::copy_nonoverlapping(source, pdpt_copy as usize as *mut u64, ENTRY_COUNT);
                let first = core::ptr::read_volatile(source);
                let destination = directory_copy as usize as *mut u64;
                if first & PRESENT == 0 {
                    false
                } else if first & (1 << 7) != 0 {
                    let gigabyte = first & 0x000f_ffff_c000_0000;
                    let flags = first & !0x000f_ffff_ffff_f000 & !(1 << 7);
                    for index in 0..ENTRY_COUNT as u64 {
                        core::ptr::write_volatile(
                            destination.add(index as usize),
                            (gigabyte + index * 0x20_0000) | flags | (1 << 7),
                        );
                    }
                    true
                } else {
                    core::ptr::copy_nonoverlapping(
                        (first & ADDRESS_MASK) as usize as *const u64,
                        destination,
                        ENTRY_COUNT,
                    );
                    true
                }
            }
        };
        if built {
            // SAFETY: as above; entry 0 of the copy now names the copied directory.
            unsafe {
                core::ptr::write_volatile(
                    pdpt_copy as usize as *mut u64,
                    directory_copy | PRESENT | WRITABLE,
                );
            }
            IDENTITY_PDPT.store(pdpt_copy, core::sync::atomic::Ordering::Release);
        }
    }
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

/// Whether every page of the physical range is mapped (present) at the same
/// address in the kernel's current address space.
pub fn kernel_range_mapped(address: u64, length: u64) -> bool {
    const HUGE: u64 = 1 << 7;
    let mut page = address & !(PAGE_SIZE - 1);
    let end = address.saturating_add(length.max(1));
    while page < end {
        let mut table = read_cr3() & ADDRESS_MASK;
        for level in 0..4u64 {
            let index = ((page >> (39 - 9 * level)) & 0x1ff) as usize;
            // SAFETY: `table` is a page-table page reached from CR3.
            let entry =
                unsafe { core::ptr::read_volatile((table as usize as *const u64).add(index)) };
            if entry & PRESENT == 0 {
                return false;
            }
            if (level == 1 || level == 2) && entry & HUGE != 0 {
                break;
            }
            table = entry & ADDRESS_MASK;
        }
        page += PAGE_SIZE;
    }
    true
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

/// Maps a dedicated, contiguous virtual region for the kernel heap, sized in
/// `pages`. Unlike the original version of this function (which squeezed the
/// heap into the handful of spare leaf entries in `PagingState`'s own
/// single-page "probe" table - a hard ceiling just under 2 MiB, since one
/// leaf table only has `ENTRY_COUNT` entries total), this builds a full,
/// dedicated PDPT/PD/multi-PT chain under its own fresh PML4 slot, the same
/// shape `create_process_from_elf` already builds per-process just for
/// kernel-only (no USER bit) mappings instead. `pages` may be anything up to
/// `ENTRY_COUNT * ENTRY_COUNT` (one PD's worth of page tables, 512 GiB) -
/// every size this kernel actually asks for fits in a single PD's worth of
/// PTs. Physical pages are allocated one at a time (`frames.allocate()`),
/// not as one big contiguous block, so a large heap never needs to find one
/// huge run of contiguous physical memory this early in boot.
pub fn map_heap(
    state: &PagingState,
    frames: &mut FrameAllocator,
    pages: usize,
) -> Option<HeapMapping> {
    if pages == 0 {
        return None;
    }
    let table_count = pages.div_ceil(ENTRY_COUNT);
    if table_count == 0 || table_count > ENTRY_COUNT {
        return None;
    }
    let root_table = state.root_physical as usize as *mut u64;
    let heap_index = (256..ENTRY_COUNT)
        .find(|index| unsafe { core::ptr::read_volatile(root_table.add(*index)) & PRESENT == 0 })?;
    let block = frames
        .allocate_contiguous(2 + table_count as u64, 1)?
        .address();
    let pdpt = block;
    let directory = pdpt.checked_add(PAGE_SIZE)?;
    let first_table = directory.checked_add(PAGE_SIZE)?;
    unsafe {
        zero_page(pdpt);
        zero_page(directory);
    }
    let branch_flags = PRESENT | WRITABLE;
    let leaf_flags = PRESENT | WRITABLE | if state.nx_enabled { NO_EXECUTE } else { 0 };
    let mut first_physical = 0u64;
    let mut mapped = 0usize;
    for table_slot in 0..table_count {
        let table = first_table.checked_add(table_slot as u64 * PAGE_SIZE)?;
        unsafe {
            zero_page(table);
            core::ptr::write_volatile(
                (directory as usize as *mut u64).add(table_slot),
                table | branch_flags,
            );
        }
        let entries_here = (pages - mapped).min(ENTRY_COUNT);
        for entry in 0..entries_here {
            let physical = frames.allocate()?.address();
            if mapped == 0 && entry == 0 {
                first_physical = physical;
            }
            unsafe {
                zero_page(physical);
                core::ptr::write_volatile(
                    (table as usize as *mut u64).add(entry),
                    physical | leaf_flags,
                );
            }
        }
        mapped += entries_here;
    }
    unsafe {
        core::ptr::write_volatile(root_table.add(heap_index), pdpt | branch_flags);
        core::ptr::write_volatile(pdpt as usize as *mut u64, directory | branch_flags);
        write_cr3(state.root_physical);
    }
    Some(HeapMapping {
        virtual_base: canonical_base(heap_index),
        physical_base: first_physical,
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

/// Proves `choose_user_index` genuinely produces different addresses across
/// allocations, not a fixed slot that happens to look random. This covers
/// `map_probe_code` / `map_user_probe` / `map_user_image`: the one-off
/// pre-scheduler boot probes and the very first `/bin/init` load, all mapped
/// before the scheduler exists. See `scheduled_process_aslr_self_test` for
/// the equivalent proof on the scheduled multi-process path
/// (`create_process`/`fork_process`/`exec_process`), which used to sit at
/// the same fixed slot for every process until `ProcessAddressSpace::user_slot`
/// replaced the old `PROCESS_USER_SLOT` constant.
pub fn aslr_self_test(state: &PagingState, frames: &mut FrameAllocator) -> bool {
    let program: [u8; 7] = [0xb8, 0x00, 0x00, 0x00, 0x00, 0xcd, 0x80];
    let Some(first) = map_probe_code(state, frames, &program) else {
        return false;
    };
    let Some(second) = map_probe_code(state, frames, &program) else {
        destroy_user_probe(state, frames, &first);
        return false;
    };
    // `choose_user_index` only ever returns a slot that's still free, and
    // `first`'s slot is still marked present (not torn down yet) - so this
    // is a deterministic proof, not a 1/255 chance of a false failure.
    let distinct = first.entry != second.entry;
    let first_reap = destroy_user_probe(state, frames, &first);
    let second_reap = destroy_user_probe(state, frames, &second);
    distinct && first_reap.verified && second_reap.verified
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

/// A process's whole address space (code/data, stack, heap, mmap) lives in
/// ONE leaf page table (512 entries), the same design the original
/// single-page `create_process` used - just with room reserved for a real,
/// multi-segment ELF image instead of one raw page. Code always reserves
/// this full slot range regardless of how much a given image actually
/// needs (unused trailing slots are simply left unmapped), so heap/mmap
/// never have to move when `exec_process` swaps in a differently-sized
/// image. 200 pages (800 KB) comfortably covers `aeros-std-smoke` (the
/// biggest real userspace binary in this repo, ~380 KB) with headroom.
/// Page indexes a program's image may occupy: 16 MiB, enough for a 12 MiB
/// fixed-address image linked at 0x400000.
const PROCESS_CODE_PAGES: usize = 4096;
const PROCESS_GUARD_INDEX: usize = PROCESS_CODE_PAGES;
// Tests that read a stack marker must use PROCESS_STACK_BYTES, not a
// hardcoded one-page offset. Real std-linked ELFs need more than one page.
pub const PROCESS_STACK_PAGES: usize = 4;
pub const PROCESS_STACK_BYTES: u64 = PROCESS_STACK_PAGES as u64 * PAGE_SIZE;
/// How many pages below the initial mapping the stack may grow into on
/// demand (see `process_stack_fault`) - 8 MiB on top of the 16 KiB every
/// process starts with. This is why the stack sits at the top of the
/// table with its growth range below it, instead of right after code: there
/// has to be room to grow into that nothing else already occupies.
pub const PROCESS_STACK_MAX_GROWTH_PAGES: usize = 2048;

const FRAME_REF_SLOTS: usize = 32768;
/// Software-available PTE bit: the page is shared copy-on-write (mapped
/// read-only until a write fault gives the writer a private copy).
const COW: u64 = 1 << 9;
/// Only in `regions`: a page of a shared mapping (shown as `s`).
const SHARED_MARK: u64 = 1 << 60;

/// How many address spaces map each shared physical frame (a frame that is
/// not here has a single owner). Linear probing; an entry goes when its
/// count would reach zero and the ones after it move up, so the table has no
/// tombstones and a lookup stops at the first empty slot.
static FRAME_REFS: TicketLock<[(u64, u32); FRAME_REF_SLOTS]> =
    TicketLock::new([(0, 0); FRAME_REF_SLOTS]);

fn frame_home(physical: u64) -> usize {
    ((physical >> 12).wrapping_mul(0x9e37_79b1) as usize) % FRAME_REF_SLOTS
}

/// Where `physical` is, or else the empty slot it would go in (`None` when
/// the table is full).
fn frame_slot(
    table: &[(u64, u32); FRAME_REF_SLOTS],
    physical: u64,
) -> Result<usize, Option<usize>> {
    let mut index = frame_home(physical);
    for _ in 0..FRAME_REF_SLOTS {
        match table[index].0 {
            entry if entry == physical => return Ok(index),
            0 => return Err(Some(index)),
            _ => index = (index + 1) % FRAME_REF_SLOTS,
        }
    }
    Err(None)
}

/// Empties `hole` and moves up the entries after it that probed past it.
fn frame_remove(table: &mut [(u64, u32); FRAME_REF_SLOTS], mut hole: usize) {
    table[hole] = (0, 0);
    let mut index = hole;
    loop {
        index = (index + 1) % FRAME_REF_SLOTS;
        let physical = table[index].0;
        if physical == 0 {
            return;
        }
        let home = frame_home(physical);
        let stays = if hole <= index {
            hole < home && home <= index
        } else {
            hole < home || home <= index
        };
        if !stays {
            table[hole] = table[index];
            table[index] = (0, 0);
            hole = index;
        }
    }
}

/// Adds an owner to a frame: a fresh (untracked) frame starts at one owner.
fn frame_retain(physical: u64) -> bool {
    frame_add(physical, 1)
}

/// Makes an owned (possibly untracked) frame shared with one more owner.
fn frame_share(physical: u64) -> bool {
    frame_add(physical, 2)
}

fn frame_add(physical: u64, untracked_count: u32) -> bool {
    let mut table = FRAME_REFS.lock();
    match frame_slot(&table, physical) {
        Ok(index) => {
            table[index].1 += 1;
            true
        }
        Err(Some(index)) => {
            table[index] = (physical, untracked_count);
            true
        }
        Err(None) => false,
    }
}

/// Drops one owner; the frame goes back to the allocator with the last one
/// (a frame that was never shared is simply freed).
fn frame_release(physical: u64) -> bool {
    let mut table = FRAME_REFS.lock();
    if let Ok(index) = frame_slot(&table, physical) {
        if table[index].1 > 1 {
            table[index].1 -= 1;
            return false;
        }
        frame_remove(&mut table, index);
    }
    drop(table);
    crate::memory::release_global_address(physical)
}

pub fn code_page_refcount(physical: u64) -> u32 {
    let table = FRAME_REFS.lock();
    frame_slot(&table, physical).map_or(0, |index| table[index].1)
}

const PROCESS_HEAP_PAGES: usize = 8192;
const PROCESS_HEAP_FIRST_INDEX: usize = PROCESS_GUARD_INDEX + 1;
/// One page stays unmapped between the heap and the stack's growth range.
const PROCESS_STACK_LOWEST_INDEX: usize = PROCESS_HEAP_FIRST_INDEX + PROCESS_HEAP_PAGES + 1;
const PROCESS_STACK_FIRST_INDEX: usize =
    PROCESS_STACK_LOWEST_INDEX + PROCESS_STACK_MAX_GROWTH_PAGES;
/// The mapping area is last, so it can be as big as the rest of the window
/// allows. A fixed-address (`ET_EXEC`) process has only the first
/// `FIXED_TABLE_PAGES` tables (the rest of its address space is the identity
/// map the kernel needs), so its mapping area ends there.
const PROCESS_MMAP_FIRST_INDEX: usize = PROCESS_STACK_FIRST_INDEX + PROCESS_STACK_PAGES;
/// Page tables a process's window spans: contiguous, so that a page's entry
/// is `table[index]` whatever its index, and linked at directory entries
/// 0.. so the window is that many 2 MiB.
const PROCESS_TABLE_PAGES: usize = 128;
const FIXED_TABLE_PAGES: usize = 64;
const PROCESS_RESERVED_PAGES: usize = PROCESS_TABLE_PAGES * ENTRY_COUNT;
const FIXED_RESERVED_PAGES: usize = FIXED_TABLE_PAGES * ENTRY_COUNT;
const PROCESS_MMAP_PAGES: usize = PROCESS_RESERVED_PAGES - PROCESS_MMAP_FIRST_INDEX;
const FIXED_MMAP_PAGES: usize = FIXED_RESERVED_PAGES - PROCESS_MMAP_FIRST_INDEX;

/// How many mapping pages a process may use: all of them, or the part inside
/// the window of a fixed-address program.
fn mmap_limit(user_slot: usize) -> usize {
    if user_slot == 0 {
        FIXED_MMAP_PAGES
    } else {
        PROCESS_MMAP_PAGES
    }
}
/// What `allocate_process_tables` takes in one block: root, PDPT, directory,
/// the tables and the initial stack.
const PROCESS_ALLOCATION_PAGES: u64 = (3 + PROCESS_TABLE_PAGES + PROCESS_STACK_PAGES) as u64;
/// Page-table entry flags for a code page, from its two attributes.
const CODE_WRITABLE: u8 = 1;
const CODE_EXECUTABLE: u8 = 2;

fn code_page_flags(attributes: u8, nx_enabled: bool) -> u64 {
    let mut flags = PRESENT | USER;
    if attributes & CODE_WRITABLE != 0 {
        flags |= WRITABLE;
    }
    if attributes & CODE_EXECUTABLE == 0 && nx_enabled {
        flags |= NO_EXECUTE;
    }
    flags
}

#[derive(Clone, Copy)]
pub struct ProcessAddressSpace {
    pub root_physical: u64,
    /// Which PML4 index (of the 255 candidates `choose_user_index` picks
    /// from) this process's whole user range hangs off - chosen fresh by
    /// `create_process`, inherited unchanged by `fork_process` (the child
    /// shares the parent's exact virtual layout, COW pages and all), and
    /// left alone by `exec_process` (a new image reuses the same slot; see
    /// `aslr_self_test`'s doc comment for why re-randomizing per-image was
    /// judged not worth the added risk to the syscall bounds-checking that
    /// caches `user_start`/`user_end` for a task's whole lifetime).
    pub user_slot: usize,
    pub entry: u64,
    pub stack_top: u64,
    pub stack_physical: u64,
    pub nx_enabled: bool,
    pub verified: bool,
    pub owns_code: bool,
    allocation_base: u64,
    allocation_pages: u64,
    pub heap_mapped: usize,
    /// One past the highest mapping page ever used (walks over the mapping
    /// area stop here), and a page below which every one is in use (where a
    /// search for free pages starts).
    mmap_high: usize,
    mmap_hint: usize,
}

/// What a process records about each page of its window lives in the
/// page-table entry itself, so the record stays small whatever the window's
/// size: a present entry holds the frame, a swapped one (`SWAPPED`, not
/// present) the swap slot, and bits the processor ignores hold the rest - an
/// image page's attributes or a mapping's protection (`SOFT_PROTECTION`) and
/// whether a mapping is shared (`SOFT_SHARED`). A page in use never has a
/// zero entry.
const SOFT_SHIFT: u32 = 52;
const SOFT_PROTECTION: u64 = 0x7 << SOFT_SHIFT;
/// A page of a `MAP_SHARED` or System V shared mapping: it stays writable
/// and shared across `fork` instead of becoming copy-on-write.
const SOFT_SHARED: u64 = 1 << 55;
const SOFT_BITS: u64 = SOFT_PROTECTION | SOFT_SHARED;

/// What a page's entry says it holds: its frame, `SWAP_ENTRY | slot` for an
/// evicted page, or 0.
fn entry_value(entry: u64) -> u64 {
    if entry & PRESENT != 0 {
        entry & ADDRESS_MASK
    } else if entry & SWAPPED != 0 {
        SWAP_ENTRY | ((entry & ADDRESS_MASK) >> 12)
    } else {
        0
    }
}

fn soft_protection(entry: u64) -> u8 {
    ((entry & SOFT_PROTECTION) >> SOFT_SHIFT) as u8
}

fn with_protection(protection: u8) -> u64 {
    (u64::from(protection) << SOFT_SHIFT) & SOFT_PROTECTION
}

/// The entry of a mapping page.
fn mmap_entry(physical: u64, protection: u8, shared: bool, nx_enabled: bool) -> u64 {
    physical
        | translate_protection(protection, nx_enabled)
        | with_protection(protection)
        | if shared { SOFT_SHARED } else { 0 }
}

/// The entry of an image page.
fn code_entry(physical: u64, attributes: u8, nx_enabled: bool) -> u64 {
    physical | code_page_flags(attributes, nx_enabled) | with_protection(attributes)
}

impl ProcessAddressSpace {
    /// The window's page tables, contiguous after the root, PDPT and
    /// directory in the process's allocation (`allocate_process_tables`).
    fn table(&self) -> *mut u64 {
        (self.allocation_base + 3 * PAGE_SIZE) as usize as *mut u64
    }

    fn pte(&self, slot: usize) -> u64 {
        unsafe { core::ptr::read_volatile(self.table().add(slot)) }
    }

    fn set_pte(&self, slot: usize, entry: u64) {
        unsafe { core::ptr::write_volatile(self.table().add(slot), entry) }
    }

    /// The frame of image page `page`, or 0.
    pub fn code_frame(&self, page: usize) -> u64 {
        if page >= PROCESS_CODE_PAGES {
            return 0;
        }
        entry_value(self.pte(page))
    }

    /// The frame (or `SWAP_ENTRY | slot`) of heap page `index`.
    pub fn heap_frame(&self, index: usize) -> u64 {
        if index >= self.heap_mapped {
            return 0;
        }
        entry_value(self.pte(PROCESS_HEAP_FIRST_INDEX + index))
    }

    fn mmap_pte(&self, index: usize) -> u64 {
        self.pte(PROCESS_MMAP_FIRST_INDEX + index)
    }

    fn mmap_used(&self, index: usize) -> bool {
        self.mmap_pte(index) != 0
    }

    fn mmap_range_free(&self, start: usize, pages: usize) -> bool {
        (start..start + pages).all(|index| !self.mmap_used(index))
    }

    fn mmap_range_used(&self, start: usize, pages: usize) -> bool {
        (start..start + pages).all(|index| self.mmap_used(index))
    }

    /// The frame (or `SWAP_ENTRY | slot`) of mapping page `index`, or 0.
    pub fn mmap_frame(&self, index: usize) -> u64 {
        if index >= mmap_limit(self.user_slot) {
            return 0;
        }
        entry_value(self.mmap_pte(index))
    }

    fn set_mmap(&mut self, index: usize, entry: u64) {
        if entry == 0 {
            self.mmap_hint = self.mmap_hint.min(index);
        } else {
            self.mmap_high = self.mmap_high.max(index + 1);
        }
        self.set_pte(PROCESS_MMAP_FIRST_INDEX + index, entry);
        unsafe { invalidate(mmap_address(self.user_slot, index)) };
    }

    /// The frame (or `SWAP_ENTRY | slot`) of the grown stack page `depth`
    /// pages below the initial stack, or 0.
    pub fn stack_grown_frame(&self, depth: usize) -> u64 {
        if depth >= PROCESS_STACK_MAX_GROWTH_PAGES {
            return 0;
        }
        entry_value(self.pte(PROCESS_STACK_FIRST_INDEX - 1 - depth))
    }

    /// How deep the stack has grown below its initial pages.
    pub fn stack_grown_pages(&self) -> usize {
        (0..PROCESS_STACK_MAX_GROWTH_PAGES)
            .rev()
            .find(|depth| self.stack_grown_frame(*depth) != 0)
            .map_or(0, |depth| depth + 1)
    }

    /// Pages currently backing this process: code, the initial stack page,
    /// grown stack pages, heap and anonymous mappings. Copy-on-write pages
    /// shared with a relative are counted in full.
    /// Calls `visit(start, end, permissions, name)` for every contiguous
    /// mapped region in ascending address order: image pages with equal
    /// permissions, heap, anonymous mappings with equal protection, and the
    /// stack including its grown pages. `permissions` is the four-letter form
    /// `/proc/<pid>/maps` uses.
    pub fn regions(&self, visit: &mut dyn FnMut(u64, u64, [u8; 4], &'static str)) {
        let base = process_virtual_base(self.user_slot);
        let permissions = |flags: u64| {
            [
                b'r',
                if flags & WRITABLE != 0 { b'w' } else { b'-' },
                if flags & NO_EXECUTE == 0 { b'x' } else { b'-' },
                if flags & SHARED_MARK != 0 { b's' } else { b'p' },
            ]
        };
        let walk = |first_slot: usize,
                    count: usize,
                    key: &dyn Fn(u64) -> u64,
                    name: &'static str,
                    visit: &mut dyn FnMut(u64, u64, [u8; 4], &'static str)| {
            let mut index = 0;
            while index < count {
                let entry = self.pte(first_slot + index);
                if entry == 0 {
                    index += 1;
                    continue;
                }
                let start = index;
                let flags = key(entry);
                while index < count && {
                    let next = self.pte(first_slot + index);
                    next != 0 && key(next) == flags
                } {
                    index += 1;
                }
                visit(
                    base + (first_slot + start) as u64 * PAGE_SIZE,
                    base + (first_slot + index) as u64 * PAGE_SIZE,
                    permissions(flags),
                    name,
                );
            }
        };
        let nx_enabled = self.nx_enabled;
        walk(
            0,
            PROCESS_CODE_PAGES,
            &|entry| code_page_flags(soft_protection(entry), nx_enabled),
            "",
            visit,
        );
        walk(
            PROCESS_HEAP_FIRST_INDEX,
            self.heap_mapped,
            &|_| PRESENT | USER | WRITABLE | NO_EXECUTE,
            "[heap]",
            visit,
        );
        walk(
            PROCESS_MMAP_FIRST_INDEX,
            self.mmap_high,
            &|entry| {
                translate_protection(soft_protection(entry), nx_enabled)
                    | if entry & SOFT_SHARED != 0 {
                        SHARED_MARK
                    } else {
                        0
                    }
            },
            "",
            visit,
        );
        let stack_base = process_stack_base(self.user_slot);
        let grown = self.stack_grown_pages() as u64;
        if self.stack_physical != 0 {
            visit(
                stack_base - grown * PAGE_SIZE,
                stack_base + PROCESS_STACK_BYTES,
                permissions(PRESENT | USER | WRITABLE | NO_EXECUTE),
                "[stack]",
            );
        }
    }

    pub fn resident_pages(&self) -> u64 {
        let present = |first: usize, count: usize| {
            (first..first + count)
                .filter(|slot| self.pte(*slot) & PRESENT != 0)
                .count() as u64
        };
        present(0, PROCESS_CODE_PAGES)
            + u64::from(self.stack_physical != 0)
            + present(PROCESS_STACK_LOWEST_INDEX, PROCESS_STACK_MAX_GROWTH_PAGES)
            + present(PROCESS_HEAP_FIRST_INDEX, self.heap_mapped)
            + present(PROCESS_MMAP_FIRST_INDEX, self.mmap_high)
    }
}

/// The virtual base of a process's dedicated address space, keyed by its own
/// randomized `user_slot` (see `ProcessAddressSpace::user_slot`) rather than
/// a fixed constant - two processes' `user_slot`s differ, so this genuinely
/// varies between them. Callers that need the true start of a process's
/// mapped range (e.g. syscall pointer-validation bounds) should use this
/// instead of `ProcessAddressSpace::entry`, which can sit at a nonzero
/// offset from the base for a real ELF image whose entry point isn't at the
/// very first mapped page.
pub fn process_virtual_base(user_slot: usize) -> u64 {
    canonical_base(user_slot)
}

/// Checks `regions` against a freshly created process: ascending, inside the
/// reserved range, no overlap, the image first and the stack last and ending
/// at `stack_top`.
pub fn regions_self_test(state: &PagingState) -> bool {
    let program: [u8; 7] = [0xb8, 0x00, 0x00, 0x00, 0x00, 0xcd, 0x80];
    let Some(space) = create_process(state, &program) else {
        return false;
    };
    let base = process_virtual_base(space.user_slot);
    let end = process_reserved_end(base);
    let mut count = 0;
    let mut previous_end = base;
    let mut ascending = true;
    let mut first_start = 0;
    let mut last: Option<(u64, u64, &str)> = None;
    space.regions(&mut |start, stop, _, name| {
        if count == 0 {
            first_start = start;
        }
        ascending &= start >= previous_end && stop > start && stop <= end;
        previous_end = stop;
        last = Some((start, stop, name));
        count += 1;
    });
    let destroyed = destroy_process(&space);
    count >= 2
        && ascending
        && first_start == base
        && last
            == Some((
                process_stack_base(space.user_slot),
                process_stack_base(space.user_slot) + PROCESS_STACK_BYTES,
                "[stack]",
            ))
        && destroyed
}

pub fn process_reserved_end(base: u64) -> u64 {
    // Base 0 is a fixed-address program's window, which is shorter.
    let pages = if base == 0 {
        FIXED_RESERVED_PAGES
    } else {
        PROCESS_RESERVED_PAGES
    };
    base + pages as u64 * PAGE_SIZE
}

/// Virtual base of a process's one-page stack region, for callers (the
/// Linux-ABI argc/argv/envp/auxv stack builder) that need to write into
/// `space.stack_physical` at the matching virtual addresses.
pub fn process_stack_base(user_slot: usize) -> u64 {
    process_virtual_base(user_slot) + PROCESS_STACK_FIRST_INDEX as u64 * PAGE_SIZE
}

/// Proves `create_process` gives processes genuinely randomized virtual
/// layouts - the scheduled multi-process counterpart to `aslr_self_test`
/// (see its doc comment). `fork_process` inherits its parent's `user_slot`
/// unchanged (a forked child shares the parent's exact COW-mapped pages, so
/// it must keep the same addresses) and `exec_process` never re-randomizes
/// an existing process's slot either, so only creation needs checking here.
///
/// Unlike `aslr_self_test`, each `create_process` call gets its own fresh,
/// independent PML4 (a copy of the kernel's template, not a shared table),
/// so two calls' `choose_user_index` picks are genuinely independent draws -
/// checking just two of them for inequality would have a real ~1/255 chance
/// of a false failure on pure coincidence, exactly the false-positive risk
/// `aslr_self_test`'s doc comment notes it *avoids* by sharing one table.
/// Sampling `SAMPLES` independent draws and requiring at least one pair to
/// differ instead makes an all-coincide false failure `(1/255)^(SAMPLES-1)`,
/// not a realistic risk, while still only ever holding one process's worth
/// of frames at a time (create, record, destroy, repeat).
pub fn scheduled_process_aslr_self_test(state: &PagingState) -> bool {
    let program: [u8; 7] = [0xb8, 0x00, 0x00, 0x00, 0x00, 0xcd, 0x80];
    const SAMPLES: usize = 8;
    let mut slots = [0usize; SAMPLES];
    for slot in slots.iter_mut() {
        let Some(space) = create_process(state, &program) else {
            ASLR_TEST_FAILURE.store(1, core::sync::atomic::Ordering::Relaxed);
            return false;
        };
        if !space.verified {
            ASLR_TEST_FAILURE.store(2, core::sync::atomic::Ordering::Relaxed);
            return false;
        }
        if !destroy_process(&space) {
            ASLR_TEST_FAILURE.store(3, core::sync::atomic::Ordering::Relaxed);
            return false;
        }
        *slot = space.user_slot;
    }
    let spread = slots.iter().any(|slot| *slot != slots[0]);
    if !spread {
        ASLR_TEST_FAILURE.store(4, core::sync::atomic::Ordering::Relaxed);
    }
    spread
}

/// Why `scheduled_process_aslr_self_test` last failed: 1 no address space,
/// 2 hierarchy flags wrong, 3 release refused, 4 every slot identical.
pub static ASLR_TEST_FAILURE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Tries to load `code` as a real multi-segment ELF binary first; if that
/// parse fails (every existing scheduler self-test probe is a raw machine
/// code blob, not a real ELF file), falls back to the original one-page
/// behavior those tests depend on unchanged.
pub fn create_process(state: &PagingState, code: &[u8]) -> Option<ProcessAddressSpace> {
    create_process_with(state, code, None)
}

/// `create_process`, with the bytes of the program's interpreter when it
/// has one (a dynamically linked program).
pub fn create_process_with(
    state: &PagingState,
    code: &[u8],
    interpreter: Option<&[u8]>,
) -> Option<ProcessAddressSpace> {
    match crate::elf::ElfImage::parse(code) {
        Ok(image) => match interpreter {
            None => create_process_from_elf(state, &image, None),
            Some(bytes) => {
                let interpreter = crate::elf::ElfImage::parse(bytes).ok()?;
                create_process_from_elf(state, &image, Some(&interpreter))
            }
        },
        Err(_) => create_process_raw(state, code),
    }
}

fn allocate_process_tables(stack_physical_pages: u64) -> Option<(u64, u64, u64, u64, u64)> {
    let block = crate::memory::allocate_global_contiguous(
        3 + PROCESS_TABLE_PAGES as u64 + stack_physical_pages,
        1,
    )?
    .address();
    let pml4 = block;
    let pdpt = pml4.checked_add(PAGE_SIZE)?;
    let directory = pdpt.checked_add(PAGE_SIZE)?;
    let table = directory.checked_add(PAGE_SIZE)?;
    let stack_physical = table.checked_add(PROCESS_TABLE_PAGES as u64 * PAGE_SIZE)?;
    unsafe {
        zero_page(pml4);
        zero_page(pdpt);
        zero_page(directory);
        for page in 0..PROCESS_TABLE_PAGES as u64 {
            zero_page(table + page * PAGE_SIZE);
        }
        for page in 0..stack_physical_pages {
            zero_page(stack_physical + page * PAGE_SIZE);
        }
    }
    Some((block, pml4, pdpt, directory, table))
}

/// Whether the physical memory below `LOW_WINDOW_END` is free of anything the
/// kernel needs to reach (see `LOW_WINDOW_END`); decided once at boot.
static LOW_WINDOW_CLEAN: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
/// Physical address of the private copy of the identity map's PDPT (0 when
/// it could not be made).
static IDENTITY_PDPT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

fn identity_pdpt() -> Option<u64> {
    match IDENTITY_PDPT.load(core::sync::atomic::Ordering::Acquire) {
        0 => None,
        address => Some(address),
    }
}

/// The kernel image's own address range (for the boot-time check that a
/// fixed-address window does not shadow it).
pub fn kernel_image_range() -> Option<(u64, u64)> {
    kernel_image()
}

/// A fixed-address (`ET_EXEC`) program's window sits at virtual address 0 and
/// so covers the first `FIXED_TABLE_PAGES` * 2 MiB (128 MiB) of the identity map the
/// kernel reaches physical memory through. In that process's own page tables
/// the window replaces those entries, which is only safe when nothing the
/// kernel touches lives in the same physical addresses: the frame allocator
/// never hands out memory below this limit (`memory::LOW_MEMORY_LIMIT`) and
/// the boot code checks the firmware map and the kernel image (`set_low_window_clean`).
pub const LOW_WINDOW_END: u64 = FIXED_TABLE_PAGES as u64 * ENTRY_COUNT as u64 * PAGE_SIZE;

/// Runs `f` on the kernel's own page tables. A fixed-address process's
/// tables replace the identity map below `LOW_WINDOW_END` with its window,
/// so physical memory there (firmware data the AML or the sleep code reads)
/// is only reachable this way while such a process's tables are loaded.
pub fn with_kernel_tables<T>(f: impl FnOnce() -> T) -> T {
    let current = read_cr3();
    let Some(state) = boot_state() else {
        return f();
    };
    if current & ADDRESS_MASK == state.root_physical {
        return f();
    }
    let flags: u64;
    unsafe { asm!("pushfq", "pop {}", out(reg) flags, options(preserves_flags)) };
    crate::arch::disable_interrupts();
    unsafe { write_cr3(state.root_physical) };
    let result = f();
    unsafe { write_cr3(current) };
    if flags & (1 << 9) != 0 {
        crate::arch::enable_interrupts();
    }
    result
}

pub fn set_low_window_clean(clean: bool) {
    LOW_WINDOW_CLEAN.store(clean, core::sync::atomic::Ordering::Release);
}

pub fn low_window_clean() -> bool {
    LOW_WINDOW_CLEAN.load(core::sync::atomic::Ordering::Acquire)
}

/// Builds the paging hierarchy of a process's window under `user_slot` of
/// `new_root`: PDPT entry 0 -> `directory`, whose entries 0.. -> the tables.
/// For slot 0 (a fixed-address program) the PDPT and directory start as a
/// copy of `identity_pdpt` (the identity map: entry 0 of it is a directory,
/// or a 1 GiB page which is spelled out as 2 MiB pages) so the rest of the
/// first GiB, and everything above, stays reachable for the kernel.
fn link_hierarchy(
    user_slot: usize,
    new_root: *mut u64,
    pdpt: u64,
    directory: u64,
    table: u64,
    identity_pdpt: Option<u64>,
) -> bool {
    let branch_flags = PRESENT | WRITABLE | USER;
    unsafe {
        if user_slot == 0 {
            let Some(source) = identity_pdpt else {
                return false;
            };
            let source = source as usize as *const u64;
            core::ptr::copy_nonoverlapping(source, pdpt as usize as *mut u64, ENTRY_COUNT);
            let first = core::ptr::read_volatile(source);
            if first & PRESENT == 0 {
                return false;
            }
            let destination = directory as usize as *mut u64;
            if first & (1 << 7) != 0 {
                let gigabyte = first & 0x000f_ffff_c000_0000;
                let flags = first & !0x000f_ffff_ffff_f000 & !(1 << 7);
                for index in 0..ENTRY_COUNT as u64 {
                    core::ptr::write_volatile(
                        destination.add(index as usize),
                        (gigabyte + index * 0x20_0000) | flags | (1 << 7),
                    );
                }
            } else {
                core::ptr::copy_nonoverlapping(
                    (first & ADDRESS_MASK) as usize as *const u64,
                    destination,
                    ENTRY_COUNT,
                );
            }
        }
        core::ptr::write_volatile(new_root.add(user_slot), pdpt | branch_flags);
        core::ptr::write_volatile(pdpt as usize as *mut u64, directory | branch_flags);
        let tables = if user_slot == 0 {
            FIXED_TABLE_PAGES
        } else {
            PROCESS_TABLE_PAGES
        };
        for index in 0..tables as u64 {
            core::ptr::write_volatile(
                (directory as usize as *mut u64).add(index as usize),
                (table + index * PAGE_SIZE) | branch_flags,
            );
        }
    }
    true
}

fn link_process_root(
    nx_enabled: bool,
    user_slot: usize,
    new_root: *mut u64,
    pdpt: u64,
    directory: u64,
    table: u64,
    identity_pdpt: Option<u64>,
) -> bool {
    let stack_physical = table + PROCESS_TABLE_PAGES as u64 * PAGE_SIZE;
    let stack_flags = PRESENT | WRITABLE | USER | if nx_enabled { NO_EXECUTE } else { 0 };
    if !link_hierarchy(user_slot, new_root, pdpt, directory, table, identity_pdpt) {
        return false;
    }
    unsafe {
        for page in 0..PROCESS_STACK_PAGES as u64 {
            core::ptr::write_volatile(
                (table as usize as *mut u64).add(PROCESS_STACK_FIRST_INDEX + page as usize),
                (stack_physical + page * PAGE_SIZE) | stack_flags,
            );
        }
    }
    true
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
    let stack_physical = table.checked_add(PROCESS_TABLE_PAGES as u64 * PAGE_SIZE)?;
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
    let user_slot = choose_user_index(new_root)?;
    unsafe {
        core::ptr::write_volatile(
            table as usize as *mut u64,
            code_physical_page | PRESENT | USER | with_protection(CODE_EXECUTABLE),
        );
    }
    if !link_process_root(
        state.nx_enabled,
        user_slot,
        new_root,
        pdpt,
        directory,
        table,
        None,
    ) {
        return None;
    }
    let entries = unsafe {
        [
            core::ptr::read_volatile(new_root.add(user_slot)),
            core::ptr::read_volatile(pdpt as usize as *const u64),
            core::ptr::read_volatile(directory as usize as *const u64),
            core::ptr::read_volatile(table as usize as *const u64),
            core::ptr::read_volatile((table as usize as *const u64).add(PROCESS_STACK_FIRST_INDEX)),
        ]
    };
    let hierarchy_user = entries[..4].iter().all(|entry| entry & USER != 0);
    let code_protected = entries[3] & WRITABLE == 0;
    let stack_protected = entries[4] & WRITABLE != 0;
    let virtual_base = canonical_base(user_slot);
    Some(ProcessAddressSpace {
        root_physical: pml4,
        user_slot,
        entry: virtual_base,
        stack_top: virtual_base
            + (PROCESS_STACK_FIRST_INDEX + PROCESS_STACK_PAGES) as u64 * PAGE_SIZE
            - 16,
        stack_physical,
        nx_enabled: state.nx_enabled,
        verified: hierarchy_user && code_protected && stack_protected,
        owns_code: true,
        allocation_base: block,
        allocation_pages: PROCESS_ALLOCATION_PAGES,
        heap_mapped: 0,
        mmap_high: 0,
        mmap_hint: 0,
    })
}

/// The image pages a program (and its interpreter, `bias` bytes after it)
/// occupies, as the attributes of each: `Some` for a page the image needs,
/// `None` when the segments do not fit the image area, overlap with
/// conflicting permissions or leave the entry point unmapped. `visit` gets
/// every needed page with its attributes.
fn image_pages(
    parts: &[(Option<&crate::elf::ElfImage<'_>>, u64); 2],
    start_entry: u64,
    visit: &mut dyn FnMut(usize, u8),
) -> Option<usize> {
    let mut ranges = [(0usize, 0usize, false, false); 32];
    let mut count = 0;
    for (part, offset) in parts {
        let Some(part) = part else {
            continue;
        };
        for segment in part.segments() {
            let first =
                usize::try_from(segment.virtual_address.checked_add(*offset)? / PAGE_SIZE).ok()?;
            let end = segment.memory_end().checked_add(offset + PAGE_SIZE - 1)? / PAGE_SIZE;
            let end = usize::try_from(end).ok()?;
            if first >= end || end > PROCESS_CODE_PAGES || count == ranges.len() {
                return None;
            }
            ranges[count] = (first, end, segment.writable, segment.executable);
            count += 1;
        }
    }
    let ranges = &ranges[..count];
    let attributes = |page: usize| -> Option<u8> {
        let mut attributes = None;
        for &(first, end, writable, executable) in ranges {
            if (first..end).contains(&page) {
                let mut value = attributes.unwrap_or(0);
                if writable {
                    value |= CODE_WRITABLE;
                }
                if executable {
                    value |= CODE_EXECUTABLE;
                }
                attributes = Some(value);
            }
        }
        attributes
    };
    let low = ranges.iter().map(|range| range.0).min()?;
    let high = ranges.iter().map(|range| range.1).max()?;
    let mut pages = 0;
    for page in low..high {
        if let Some(value) = attributes(page) {
            if value == CODE_WRITABLE | CODE_EXECUTABLE {
                return None;
            }
            pages += 1;
        }
    }
    let entry_page = usize::try_from(start_entry / PAGE_SIZE).ok()?;
    if attributes(entry_page).is_none_or(|value| value & CODE_EXECUTABLE == 0) {
        return None;
    }
    for page in low..high {
        if let Some(value) = attributes(page) {
            visit(page, value);
        }
    }
    Some(pages)
}

/// Copies every segment's file bytes into the image pages `frame` names.
fn copy_segments(
    parts: &[(Option<&crate::elf::ElfImage<'_>>, u64); 2],
    frame: &dyn Fn(usize) -> u64,
) -> Option<()> {
    for (part, offset) in parts {
        let Some(part) = part else {
            continue;
        };
        for segment in part.segments() {
            let mut source = segment.file_offset;
            let mut virtual_offset =
                usize::try_from(segment.virtual_address.checked_add(*offset)?).ok()?;
            let mut remaining = segment.file_size;
            while remaining != 0 {
                let page = virtual_offset / PAGE_SIZE as usize;
                let within_page = virtual_offset & (PAGE_SIZE as usize - 1);
                let count = remaining.min(PAGE_SIZE as usize - within_page);
                let physical = frame(page);
                if physical == 0 {
                    return None;
                }
                let bytes = part.bytes().get(source..source.checked_add(count)?)?;
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        bytes.as_ptr(),
                        (physical + within_page as u64) as usize as *mut u8,
                        count,
                    );
                }
                source += count;
                virtual_offset = virtual_offset.checked_add(count)?;
                remaining -= count;
            }
        }
    }
    Some(())
}

/// A real multi-segment ELF loader for the per-task scheduler path, ported
/// from the proven single-shot `map_user_image` (same segment-walking
/// algorithm) but targeting this process's own dedicated page table
/// instead of one shared singleton slot, so it composes with fork/exec.
fn create_process_from_elf(
    state: &PagingState,
    image: &crate::elf::ElfImage<'_>,
    interpreter: Option<&crate::elf::ElfImage<'_>>,
) -> Option<ProcessAddressSpace> {
    // A dynamically linked program without its interpreter cannot run; with
    // one, the interpreter sits after the program and starts first.
    let bias = match interpreter {
        Some(interpreter) => {
            if interpreter.is_fixed() {
                return None;
            }
            interpreter_bias(image)
        }
        None if image.interpreter().is_some() => return None,
        None => 0,
    };
    let parts = [(Some(image), 0u64), (interpreter, bias)];
    let start_entry = match interpreter {
        Some(interpreter) => bias + interpreter.entry(),
        None => image.entry(),
    };
    image_pages(&parts, start_entry, &mut |_, _| {})?;
    // A fixed-address program needs the window at virtual address 0.
    if image.is_fixed() && !low_window_clean() {
        return None;
    }

    let (block, pml4, pdpt, directory, table) =
        allocate_process_tables(PROCESS_STACK_PAGES as u64)?;
    let stack_physical = table.checked_add(PROCESS_TABLE_PAGES as u64 * PAGE_SIZE)?;
    let kernel_root = state.root_physical as usize as *const u64;
    let new_root = pml4 as usize as *mut u64;
    unsafe { core::ptr::copy_nonoverlapping(kernel_root, new_root, ENTRY_COUNT) };
    let identity_pdpt = identity_pdpt();
    let user_slot = if image.is_fixed() {
        0
    } else {
        choose_user_index(new_root)?
    };
    let table_pointer = table as usize as *mut u64;
    let entry_at = |page: usize| unsafe { core::ptr::read_volatile(table_pointer.add(page)) };
    let mut allocated = true;
    image_pages(&parts, start_entry, &mut |page, attributes| {
        if !allocated {
            return;
        }
        let Some(frame) = crate::memory::allocate_global() else {
            allocated = false;
            return;
        };
        let physical = frame.address();
        unsafe { zero_page(physical) };
        frame_retain(physical);
        unsafe {
            core::ptr::write_volatile(
                table_pointer.add(page),
                code_entry(physical, attributes, state.nx_enabled),
            );
        }
    })?;
    if !allocated {
        return None;
    }
    copy_segments(&parts, &|page| entry_value(entry_at(page)))?;
    if !link_process_root(
        state.nx_enabled,
        user_slot,
        new_root,
        pdpt,
        directory,
        table,
        identity_pdpt,
    ) {
        return None;
    }

    let hierarchy_valid = unsafe {
        core::ptr::read_volatile(new_root.add(user_slot)) & (PRESENT | USER) == PRESENT | USER
            && core::ptr::read_volatile(pdpt as usize as *const u64) & (PRESENT | USER)
                == PRESENT | USER
            && core::ptr::read_volatile(directory as usize as *const u64) & (PRESENT | USER)
                == PRESENT | USER
    };
    let mut leaves_valid = true;
    let mut needed = 0;
    image_pages(&parts, start_entry, &mut |page, attributes| {
        let entry = entry_at(page);
        needed += 1;
        leaves_valid &= entry & (PRESENT | USER) == PRESENT | USER
            && (entry & WRITABLE != 0) == (attributes & CODE_WRITABLE != 0)
            && (!state.nx_enabled
                || (entry & NO_EXECUTE == 0) == (attributes & CODE_EXECUTABLE != 0));
    })?;
    let present = (0..PROCESS_CODE_PAGES)
        .filter(|page| entry_at(*page) & PRESENT != 0)
        .count();
    leaves_valid &= present == needed;
    let guard_entry = entry_at(PROCESS_GUARD_INDEX);
    let stacks_valid = (0..PROCESS_STACK_PAGES).all(|page| {
        let entry = entry_at(PROCESS_STACK_FIRST_INDEX + page);
        entry & (PRESENT | WRITABLE | USER) == PRESENT | WRITABLE | USER
            && (!state.nx_enabled || entry & NO_EXECUTE != 0)
    });
    let virtual_base = canonical_base(user_slot);
    Some(ProcessAddressSpace {
        root_physical: pml4,
        user_slot,
        entry: virtual_base.checked_add(start_entry)?,
        stack_top: virtual_base
            + (PROCESS_STACK_FIRST_INDEX + PROCESS_STACK_PAGES) as u64 * PAGE_SIZE
            - 16,
        stack_physical,
        nx_enabled: state.nx_enabled,
        verified: hierarchy_valid && leaves_valid && guard_entry & PRESENT == 0 && stacks_valid,
        owns_code: true,
        allocation_base: block,
        allocation_pages: PROCESS_ALLOCATION_PAGES,
        heap_mapped: 0,
        mmap_high: 0,
        mmap_hint: 0,
    })
}

pub fn fork_process(parent: &ProcessAddressSpace) -> Option<ProcessAddressSpace> {
    crate::scheduler::stop_siblings(parent.root_physical);
    let (block, pml4, pdpt, directory, table) =
        allocate_process_tables(PROCESS_STACK_PAGES as u64)?;
    let stack_physical = table.checked_add(PROCESS_TABLE_PAGES as u64 * PAGE_SIZE)?;
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
    let parent_table = process_table_physical(parent.root_physical, parent.user_slot)?;
    let virtual_base = canonical_base(parent.user_slot);
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
    let child_table = table as usize as *mut u64;
    let set_child = |slot: usize, entry: u64| unsafe {
        core::ptr::write_volatile(child_table.add(slot), entry);
    };
    for page in 0..PROCESS_CODE_PAGES {
        let physical = parent.code_frame(page);
        if physical == 0 {
            continue;
        }
        let entry = share_page(page, physical, parent.pte(page))?;
        set_child(page, entry);
    }
    // A fixed-address parent's PDPT and directory hold the identity map of
    // the first GiB; the child's start as copies of them.
    let identity_pdpt = (parent.user_slot == 0).then_some(parent.allocation_base + PAGE_SIZE);
    if !link_process_root(
        parent.nx_enabled,
        parent.user_slot,
        new_root,
        pdpt,
        directory,
        table,
        identity_pdpt,
    ) {
        return None;
    }
    for index in 0..parent.heap_mapped {
        let slot = PROCESS_HEAP_FIRST_INDEX + index;
        let entry = share_page(slot, parent.heap_frame(index), parent.pte(slot))?;
        set_child(slot, entry);
    }
    for index in 0..parent.mmap_high {
        let entry = parent.mmap_pte(index);
        if entry == 0 {
            continue;
        }
        let slot = PROCESS_MMAP_FIRST_INDEX + index;
        if entry & PRESENT == 0 {
            // A reservation (evicted pages were brought back before the fork).
            set_child(slot, entry);
            continue;
        }
        if entry & SOFT_SHARED != 0 {
            if !frame_share(entry & ADDRESS_MASK) {
                return None;
            }
            set_child(slot, entry);
            continue;
        }
        let entry = share_page(slot, entry & ADDRESS_MASK, entry)?;
        set_child(slot, entry);
    }
    // Grown stack pages share the same COW treatment as heap/mmap (unlike
    // the fixed initial block above, which stays eager-copied).
    for depth in 0..PROCESS_STACK_MAX_GROWTH_PAGES {
        let physical = parent.stack_grown_frame(depth);
        if physical == 0 {
            continue;
        }
        let slot = PROCESS_STACK_FIRST_INDEX - 1 - depth;
        let entry = share_page(slot, physical, parent.pte(slot))?;
        set_child(slot, entry);
    }
    let hierarchy_user = unsafe {
        core::ptr::read_volatile(new_root.add(parent.user_slot)) & USER != 0
            && core::ptr::read_volatile(pdpt as usize as *const u64) & USER != 0
            && core::ptr::read_volatile(directory as usize as *const u64) & USER != 0
    };
    let code_matches = (0..PROCESS_CODE_PAGES).all(|page| {
        let entry = unsafe { core::ptr::read_volatile(child_table.add(page)) };
        let physical = parent.code_frame(page);
        if physical == 0 {
            entry == 0
        } else {
            entry & PRESENT != 0 && entry & ADDRESS_MASK == physical
        }
    });
    let stacks_valid = (0..PROCESS_STACK_PAGES).all(|page| {
        let entry =
            unsafe { core::ptr::read_volatile(child_table.add(PROCESS_STACK_FIRST_INDEX + page)) };
        entry & (PRESENT | WRITABLE | USER) == PRESENT | WRITABLE | USER
    });
    Some(ProcessAddressSpace {
        root_physical: pml4,
        user_slot: parent.user_slot,
        entry: parent.entry,
        stack_top: parent.stack_top,
        stack_physical,
        nx_enabled: parent.nx_enabled,
        verified: hierarchy_user && code_matches && stacks_valid,
        owns_code: false,
        allocation_base: block,
        allocation_pages: PROCESS_ALLOCATION_PAGES,
        heap_mapped: parent.heap_mapped,
        mmap_high: parent.mmap_high,
        mmap_hint: parent.mmap_hint,
    })
}

fn process_table_physical(root_physical: u64, user_slot: usize) -> Option<u64> {
    unsafe {
        let root = root_physical as usize as *const u64;
        let pdpt_entry = core::ptr::read_volatile(root.add(user_slot));
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
    exec_process_with(space, code, None)
}

/// Where a dynamically linked program's interpreter is loaded, relative to
/// the program's own base: the first page after the program's image, with one
/// unmapped page between them.
pub fn interpreter_bias(image: &crate::elf::ElfImage<'_>) -> u64 {
    let end = image
        .segments()
        .iter()
        .map(|segment| segment.memory_end())
        .max()
        .unwrap_or(0);
    (end.div_ceil(PAGE_SIZE) + 1) * PAGE_SIZE
}

/// `exec_process`, with the bytes of the program's interpreter when it has
/// one (a dynamically linked program: the interpreter runs first and loads
/// the shared libraries itself).
pub fn exec_process_with(
    space: &mut ProcessAddressSpace,
    code: &[u8],
    interpreter: Option<&[u8]>,
) -> bool {
    crate::scheduler::stop_siblings(space.root_physical);
    match crate::elf::ElfImage::parse(code) {
        Ok(image) => match interpreter {
            None => exec_process_elf(space, &image, None),
            Some(bytes) => match crate::elf::ElfImage::parse(bytes) {
                Ok(interpreter) => exec_process_elf(space, &image, Some(&interpreter)),
                Err(_) => false,
            },
        },
        Err(_) => exec_process_raw(space, code),
    }
}

/// Shared by both exec paths: unmaps and releases every code page the
/// process currently has (there may be more or fewer than the incoming
/// image needs - code always reserves the same fixed slot range, so this
/// never has to touch heap/mmap/stack indices), tears down the old
/// heap/mmap regions (execve() replaces the whole image), then leaves
/// `space` ready for the caller to map the new pages into the same table.
fn exec_teardown(space: &mut ProcessAddressSpace) {
    for stack_page in 0..PROCESS_STACK_PAGES as u64 {
        unsafe { zero_page(space.stack_physical + stack_page * PAGE_SIZE) };
    }
    // Grown stack pages are potentially COW-shared (see fork_process), so -
    // unlike the fixed block just zeroed above - they are unmapped and
    // refcount-released like everything else, never overwritten in place.
    let ranges = [
        (0, PROCESS_CODE_PAGES),
        (PROCESS_HEAP_FIRST_INDEX, space.heap_mapped),
        (PROCESS_MMAP_FIRST_INDEX, space.mmap_high),
        (PROCESS_STACK_LOWEST_INDEX, PROCESS_STACK_MAX_GROWTH_PAGES),
    ];
    release_ranges(space, &ranges);
    space.heap_mapped = 0;
    space.mmap_high = 0;
    space.mmap_hint = 0;
}

/// Unmaps every page in the `(first slot, count)` ranges and gives back what
/// it held.
fn release_ranges(space: &ProcessAddressSpace, ranges: &[(usize, usize)]) {
    let virtual_base = canonical_base(space.user_slot);
    for &(first, count) in ranges {
        for slot in first..first + count {
            let entry = space.pte(slot);
            if entry == 0 {
                continue;
            }
            space.set_pte(slot, 0);
            unsafe { invalidate(virtual_base + slot as u64 * PAGE_SIZE) };
            release_entry(entry_value(entry));
        }
    }
}

/// Moves the process's window between a random slot (position-independent
/// programs) and slot 0 (fixed-address programs) by rebuilding the PDPT and
/// directory in place; the tables, and what they map, stay as they are.
fn relink_window(space: &mut ProcessAddressSpace, fixed: bool) -> bool {
    let Some(state) = boot_state() else {
        return false;
    };
    let kernel_root = state.root_physical as usize as *const u64;
    let identity_pdpt = identity_pdpt();
    let root = space.root_physical as usize as *mut u64;
    let pdpt = space.allocation_base + PAGE_SIZE;
    let directory = pdpt + PAGE_SIZE;
    let table = directory + PAGE_SIZE;
    let old_slot = space.user_slot;
    let new_slot = if fixed {
        if !low_window_clean() {
            return false;
        }
        0
    } else {
        let Some(slot) = choose_user_index(root) else {
            return false;
        };
        slot
    };
    unsafe {
        let kernel_first = core::ptr::read_volatile(kernel_root);
        // The old entry goes first: when it was the identity map's own slot
        // the kernel must never see its tables zeroed under it.
        core::ptr::write_volatile(
            root.add(old_slot),
            if old_slot == 0 { kernel_first } else { 0 },
        );
        zero_page(pdpt);
        zero_page(directory);
        if !link_hierarchy(new_slot, root, pdpt, directory, table, identity_pdpt) {
            return false;
        }
        // Everything cached for the old addresses goes.
        write_cr3(space.root_physical);
    }
    space.user_slot = new_slot;
    true
}

fn exec_process_raw(space: &mut ProcessAddressSpace, code: &[u8]) -> bool {
    if code.is_empty() || code.len() > PAGE_SIZE as usize {
        return false;
    }
    let Some(frame) = crate::memory::allocate_global() else {
        return false;
    };
    let new_code_physical = frame.address();
    unsafe {
        zero_page(new_code_physical);
        core::ptr::copy_nonoverlapping(
            code.as_ptr(),
            new_code_physical as usize as *mut u8,
            code.len(),
        );
    }
    exec_teardown(space);
    if space.user_slot == 0 && !relink_window(space, false) {
        return false;
    }
    frame_retain(new_code_physical);
    space.set_pte(
        0,
        new_code_physical | PRESENT | USER | with_protection(CODE_EXECUTABLE),
    );
    unsafe { invalidate(canonical_base(space.user_slot)) };
    space.owns_code = true;
    true
}

/// Frames taken before an exec tears the old image down, so running out of
/// memory leaves the old image intact: a chain threaded through the first
/// word of each frame (`None` when not all of them could be had).
struct FrameChain {
    head: u64,
}

impl FrameChain {
    fn take(count: usize) -> Option<Self> {
        let mut chain = FrameChain { head: 0 };
        for _ in 0..count {
            let Some(frame) = crate::memory::allocate_global() else {
                chain.release();
                return None;
            };
            let physical = frame.address();
            unsafe { core::ptr::write_volatile(physical as usize as *mut u64, chain.head) };
            chain.head = physical;
        }
        Some(chain)
    }

    /// The next frame, zeroed.
    fn pop(&mut self) -> Option<u64> {
        let physical = self.head;
        if physical == 0 {
            return None;
        }
        self.head = unsafe { core::ptr::read_volatile(physical as usize as *const u64) };
        unsafe { zero_page(physical) };
        Some(physical)
    }

    fn release(&mut self) {
        while let Some(physical) = self.pop() {
            crate::memory::release_global_address(physical);
        }
    }
}

fn exec_process_elf(
    space: &mut ProcessAddressSpace,
    image: &crate::elf::ElfImage<'_>,
    interpreter: Option<&crate::elf::ElfImage<'_>>,
) -> bool {
    crate::scheduler::stop_siblings(space.root_physical);
    // The interpreter (which must be position-independent) sits after the
    // program, at an offset both are placed by.
    let bias = match interpreter {
        Some(interpreter) => {
            if interpreter.is_fixed() {
                return false;
            }
            interpreter_bias(image)
        }
        None => 0,
    };
    let parts = [(Some(image), 0u64), (interpreter, bias)];
    // The program starts at the interpreter's entry point when it has one.
    let start_entry = match interpreter {
        Some(interpreter) => bias + interpreter.entry(),
        None => image.entry(),
    };
    let Some(pages) = image_pages(&parts, start_entry, &mut |_, _| {}) else {
        return false;
    };
    if pages == 0 || image.is_fixed() && !low_window_clean() {
        return false;
    }
    // Every frame of the new image is taken before the old one goes, so a
    // shortage leaves the process running its previous image.
    let Some(mut frames) = FrameChain::take(pages) else {
        return false;
    };
    exec_teardown(space);
    // The new image may need the other kind of window (fixed or not).
    if image.is_fixed() != (space.user_slot == 0) && !relink_window(space, image.is_fixed()) {
        frames.release();
        return false;
    }
    let virtual_base = canonical_base(space.user_slot);
    let nx_enabled = space.nx_enabled;
    let target = &*space;
    image_pages(&parts, start_entry, &mut |page, attributes| {
        let Some(physical) = frames.pop() else {
            return;
        };
        frame_retain(physical);
        target.set_pte(page, code_entry(physical, attributes, nx_enabled));
        unsafe { invalidate(virtual_base + page as u64 * PAGE_SIZE) };
    });
    frames.release();
    if copy_segments(&parts, &|page| space.code_frame(page)).is_none() {
        return false;
    }
    space.entry = virtual_base + start_entry;
    space.owns_code = true;
    true
}

pub fn destroy_process(space: &ProcessAddressSpace) -> bool {
    let ranges = [
        (0, PROCESS_CODE_PAGES),
        (PROCESS_HEAP_FIRST_INDEX, space.heap_mapped),
        (PROCESS_MMAP_FIRST_INDEX, space.mmap_high),
        (PROCESS_STACK_LOWEST_INDEX, PROCESS_STACK_MAX_GROWTH_PAGES),
    ];
    release_ranges(space, &ranges);
    // The page tables hold only addresses; the initial stack held the
    // process's data and is wiped as everything else it gives back is.
    for page in 0..PROCESS_STACK_PAGES as u64 {
        unsafe { zero_page(space.stack_physical + page * PAGE_SIZE) };
    }
    crate::memory::release_global_contiguous_unwiped(space.allocation_base, space.allocation_pages)
}

pub fn process_brk(space: &mut ProcessAddressSpace, additional_pages: usize) -> Option<u64> {
    let user_slot = space.user_slot;
    let brk_address = |mapped: usize| {
        process_virtual_base(user_slot) + (PROCESS_HEAP_FIRST_INDEX + mapped) as u64 * PAGE_SIZE
    };
    if additional_pages == 0 {
        return Some(brk_address(space.heap_mapped));
    }
    if space.heap_mapped + additional_pages > PROCESS_HEAP_PAGES {
        return None;
    }
    let flags = PRESENT | WRITABLE | USER | if space.nx_enabled { NO_EXECUTE } else { 0 };
    for _ in 0..additional_pages {
        let frame = crate::memory::allocate_global()?;
        let page = frame.address();
        unsafe { zero_page(page) };
        space.set_pte(PROCESS_HEAP_FIRST_INDEX + space.heap_mapped, page | flags);
        unsafe { invalidate(brk_address(space.heap_mapped)) };
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

fn mmap_address(user_slot: usize, index: usize) -> u64 {
    process_virtual_base(user_slot) + (PROCESS_MMAP_FIRST_INDEX + index) as u64 * PAGE_SIZE
}

fn mmap_index_for_address(user_slot: usize, address: u64) -> Option<usize> {
    let base = mmap_address(user_slot, 0);
    if address < base || address & (PAGE_SIZE - 1) != 0 {
        return None;
    }
    let index = usize::try_from((address - base) / PAGE_SIZE).ok()?;
    (index < mmap_limit(user_slot)).then_some(index)
}

/// The first run of `pages` unused mapping pages.
fn find_free_run(space: &mut ProcessAddressSpace, pages: usize) -> Option<usize> {
    let limit = mmap_limit(space.user_slot);
    if pages == 0 || pages > limit {
        return None;
    }
    let mut run = 0;
    for index in space.mmap_hint..limit {
        if space.mmap_used(index) {
            run = 0;
            continue;
        }
        run += 1;
        if run == pages {
            return Some(index + 1 - pages);
        }
    }
    None
}

/// Finds `pages` contiguous unused slots in this process's mapping area,
/// maps fresh zeroed physical pages with the requested protection, and
/// returns the new mapping's base virtual address. File-backed mmap is
/// layered on top via `process_mmap_write` (the syscall layer allocates
/// through here first either way, then streams file content in only for the
/// file-backed case).
pub fn process_mmap(space: &mut ProcessAddressSpace, pages: usize, protection: u8) -> Option<u64> {
    let start = find_free_run(space, pages)?;
    for index in start..start + pages {
        space.set_mmap(index, fresh_mmap_entry(protection));
        // A small mapping is about to be used whole: its frames now save a
        // fault per page.
        if pages <= EAGER_MAPPING_PAGES && protection & 7 != 0 && !populate_mmap(space, index) {
            return None;
        }
    }
    if start == space.mmap_hint {
        space.mmap_hint = start + pages;
    }
    Some(mmap_address(space.user_slot, start))
}

const EAGER_MAPPING_PAGES: usize = 16;

/// The entry of a new private mapping page: no frame until it is touched.
fn fresh_mmap_entry(protection: u8) -> u64 {
    RESERVED | with_protection(protection)
}

/// Gives a not-yet-touched mapping page its zeroed frame. False for a
/// `PROT_NONE` page, or when there is no memory.
fn populate_mmap(space: &mut ProcessAddressSpace, index: usize) -> bool {
    let entry = space.mmap_pte(index);
    if entry & PRESENT != 0 {
        return true;
    }
    let protection = soft_protection(entry);
    if entry & RESERVED == 0 || protection & 7 == 0 {
        return false;
    }
    let Some(frame) = crate::memory::allocate_global() else {
        return false;
    };
    let page = frame.address();
    unsafe { zero_page(page) };
    space.set_mmap(index, mmap_entry(page, protection, false, space.nx_enabled));
    true
}

/// `MAP_FIXED`: maps `pages` fresh zeroed pages at `address`, which must lie
/// inside the mapping area. Pages that are already mapped there are replaced
/// (unless `replace` is false, which refuses instead).
pub fn process_mmap_fixed(
    space: &mut ProcessAddressSpace,
    address: u64,
    pages: usize,
    protection: u8,
    replace: bool,
) -> Option<u64> {
    let start = mmap_index_for_address(space.user_slot, address)?;
    if pages == 0 || start + pages > mmap_limit(space.user_slot) {
        return None;
    }
    let free = space.mmap_range_free(start, pages);
    if !replace && !free {
        return None;
    }
    if !free {
        crate::scheduler::stop_siblings(space.root_physical);
    }
    for index in start..start + pages {
        let old = space.mmap_pte(index);
        if old != 0 {
            // The old translation goes before its frame does, so a failed
            // allocation below never leaves a mapping of a freed frame.
            space.set_mmap(index, 0);
            release_entry(entry_value(old));
        }
        space.set_mmap(index, fresh_mmap_entry(protection));
    }
    Some(mmap_address(space.user_slot, start))
}

/// A mapping that several address spaces see the same memory through, kept
/// writable across `fork`. With `frames` the pages are those frames, each
/// already holding a reference for this mapping (see `retain_frame`);
/// without, fresh zeroed pages.
pub fn process_mmap_shared(
    space: &mut ProcessAddressSpace,
    pages: usize,
    protection: u8,
    frames: Option<&[u64]>,
) -> Option<u64> {
    if frames.is_some_and(|f| f.len() != pages) {
        return None;
    }
    let start = find_free_run(space, pages)?;
    for offset in 0..pages {
        let page = match frames {
            Some(frames) => frames[offset],
            None => {
                let frame = crate::memory::allocate_global()?;
                let page = frame.address();
                unsafe { zero_page(page) };
                page
            }
        };
        space.set_mmap(
            start + offset,
            mmap_entry(page, protection, true, space.nx_enabled),
        );
    }
    Some(mmap_address(space.user_slot, start))
}

/// The physical address of `address` when it lies in a present page of a
/// shared mapping (what a futex there is known by in every process).
pub fn process_shared_physical(space: &ProcessAddressSpace, address: u64) -> Option<u64> {
    let base = mmap_address(space.user_slot, 0);
    let index = usize::try_from(address.checked_sub(base)? / PAGE_SIZE).ok()?;
    if index >= mmap_limit(space.user_slot) {
        return None;
    }
    let entry = space.mmap_pte(index);
    (entry & (PRESENT | SOFT_SHARED) == PRESENT | SOFT_SHARED)
        .then(|| (entry & ADDRESS_MASK) + address % PAGE_SIZE)
}

/// The frame behind the first page of the mapping at `address`.
pub fn process_mmap_frame(space: &ProcessAddressSpace, address: u64) -> Option<u64> {
    let index = mmap_index_for_address(space.user_slot, address)?;
    let entry = space.mmap_pte(index);
    (entry & PRESENT != 0).then_some(entry & ADDRESS_MASK)
}

/// Adds an owner to a frame (a mapping about to be made of it).
pub fn retain_frame(physical: u64) -> bool {
    frame_share(physical)
}

/// Drops an owner; the last one gives the frame back.
pub fn release_frame(physical: u64) {
    frame_release(physical);
}

/// Writes file content into an already-mapped `process_mmap` region -
/// the file-backed half of file-backed mmap: the syscall layer allocates
/// zeroed pages via `process_mmap` first (identical to anonymous mmap),
/// then streams the file's bytes in here. `address` must be a base
/// returned by `process_mmap`; `offset` is the byte offset within that
/// mapping (not the file), so multi-chunk reads can call this repeatedly.
pub fn process_mmap_write(
    space: &mut ProcessAddressSpace,
    address: u64,
    offset: usize,
    source: &[u8],
) -> bool {
    if source.is_empty() {
        return true;
    }
    let Some(start_index) = mmap_index_for_address(space.user_slot, address) else {
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
    if end > mmap_limit(space.user_slot) * PAGE_SIZE as usize {
        return false;
    }
    let mut written = 0usize;
    while written < source.len() {
        let current = absolute_offset + written;
        let page = current / PAGE_SIZE as usize;
        let within = current % PAGE_SIZE as usize;
        if space.mmap_pte(page) & (PRESENT | SWAPPED) == SWAPPED {
            let at = mmap_address(space.user_slot, page);
            process_swap_in(space, at);
        }
        if !populate_mmap(space, page) {
            return false;
        }
        let entry = space.mmap_pte(page);
        let count = (PAGE_SIZE as usize - within).min(source.len() - written);
        let destination = (entry & ADDRESS_MASK) + within as u64;
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
    crate::scheduler::stop_siblings(space.root_physical);
    let base = canonical_base(space.user_slot);
    if address >= base && address < base + PROCESS_CODE_PAGES as u64 * PAGE_SIZE {
        return process_mprotect_image(space, address, pages, protection);
    }
    let Some(start) = mmap_index_for_address(space.user_slot, address) else {
        return false;
    };
    if pages == 0 || start + pages > mmap_limit(space.user_slot) {
        return false;
    }
    if !space.mmap_range_used(start, pages) {
        return false;
    }
    process_swap_in_all(space);
    let flags = translate_protection(protection, space.nx_enabled);
    for index in start..start + pages {
        let entry = space.mmap_pte(index);
        if entry & PRESENT == 0 {
            // An untouched page keeps waiting for its frame, with the new protection.
            if entry & RESERVED != 0 {
                space.set_mmap(index, RESERVED | with_protection(protection));
            }
            continue;
        }
        let physical = entry & ADDRESS_MASK;
        let shared_mapping = entry & SOFT_SHARED != 0;
        // A page still shared with another process must stay copy-on-write
        // even when it becomes writable, or the write would hit both copies.
        let shared = entry & COW != 0 || code_page_refcount(physical) > 1;
        let flags = if shared && !shared_mapping && flags & WRITABLE != 0 {
            (flags & !WRITABLE) | COW
        } else {
            flags
        };
        space.set_mmap(
            index,
            physical | flags | with_protection(protection) | (entry & SOFT_SHARED),
        );
    }
    true
}

/// `mprotect` on pages of the program image (a C runtime makes its
/// relocated data read-only this way, "RELRO"): each page must belong to the
/// image; a page that is still shared with a relative stays copy-on-write.
fn process_mprotect_image(
    space: &mut ProcessAddressSpace,
    address: u64,
    pages: usize,
    protection: u8,
) -> bool {
    if address & (PAGE_SIZE - 1) != 0 || pages == 0 {
        return false;
    }
    let base = canonical_base(space.user_slot);
    let first = ((address - base) / PAGE_SIZE) as usize;
    if first + pages > PROCESS_CODE_PAGES
        || (first..first + pages).any(|page| space.code_frame(page) == 0)
    {
        return false;
    }
    process_swap_in_all(space);
    let mut attributes = 0;
    if protection & 2 != 0 {
        attributes |= CODE_WRITABLE;
    }
    if protection & 4 != 0 {
        attributes |= CODE_EXECUTABLE;
    }
    if attributes & (CODE_WRITABLE | CODE_EXECUTABLE) == CODE_WRITABLE | CODE_EXECUTABLE {
        return false;
    }
    for index in first..first + pages {
        let entry = space.pte(index);
        let physical = entry & ADDRESS_MASK;
        let mut flags = code_page_flags(attributes, space.nx_enabled);
        let shared = entry & COW != 0 || code_page_refcount(physical) > 1;
        if shared && flags & WRITABLE != 0 {
            flags = (flags & !WRITABLE) | COW;
        }
        space.set_pte(index, physical | flags | with_protection(attributes));
        unsafe { invalidate(base + index as u64 * PAGE_SIZE) };
    }
    true
}

/// A page-table entry of an evicted page: not present, holding its swap slot
/// (and, in bits the processor ignores, the flags it goes back with).
const SWAPPED: u64 = 1 << 10;
/// A private mapping page with no frame yet: it gets a zeroed one when it is
/// first touched, unless its protection (kept in the entry) is `PROT_NONE`.
/// C libraries reserve large ranges and use them a little at a time.
const RESERVED: u64 = 1 << 11;
/// How `entry_value` reports an evicted page.
const SWAP_ENTRY: u64 = 1 << 63;
/// The flag bits an evicted page keeps for when it comes back.
const SWAP_KEPT: u64 = SOFT_BITS | WRITABLE | USER | NO_EXECUTE;
const SWAP_CANDIDATES: usize =
    PROCESS_HEAP_PAGES + PROCESS_MMAP_PAGES + PROCESS_STACK_MAX_GROWTH_PAGES;

fn release_entry(entry: u64) {
    if entry & SWAP_ENTRY != 0 {
        crate::swap::release(entry & !SWAP_ENTRY);
    } else if entry != 0 {
        frame_release(entry);
    }
}

/// The page-table slot of candidate `n` of the evictable pages (heap, then
/// mmap, then grown stack), when that page is in use.
fn swap_candidate_slot(space: &ProcessAddressSpace, n: usize) -> Option<usize> {
    let slot = if n < PROCESS_HEAP_PAGES {
        (n < space.heap_mapped).then_some(PROCESS_HEAP_FIRST_INDEX + n)?
    } else if n < PROCESS_HEAP_PAGES + PROCESS_MMAP_PAGES {
        let index = n - PROCESS_HEAP_PAGES;
        (index < mmap_limit(space.user_slot)).then_some(PROCESS_MMAP_FIRST_INDEX + index)?
    } else {
        PROCESS_STACK_FIRST_INDEX - 1 - (n - PROCESS_HEAP_PAGES - PROCESS_MMAP_PAGES)
    };
    (space.pte(slot) != 0).then_some(slot)
}

/// Whether `slot` is a heap, mapping or grown-stack page (the pages that
/// can be evicted).
fn swappable_slot(space: &ProcessAddressSpace, slot: usize) -> bool {
    (PROCESS_HEAP_FIRST_INDEX..PROCESS_HEAP_FIRST_INDEX + space.heap_mapped).contains(&slot)
        || (PROCESS_MMAP_FIRST_INDEX..PROCESS_MMAP_FIRST_INDEX + mmap_limit(space.user_slot))
            .contains(&slot)
        || (PROCESS_STACK_LOWEST_INDEX..PROCESS_STACK_FIRST_INDEX).contains(&slot)
}

/// Writes one page of a process that is not running to swap and gives its
/// frame back, trying candidates from `start` on. Returns where to continue,
/// or `None` when nothing could be evicted (no swap, full, or nothing
/// eligible: pages shared with a relative stay in memory).
pub fn process_swap_out(space: &mut ProcessAddressSpace, start: usize) -> Option<usize> {
    crate::scheduler::stop_siblings(space.root_physical);
    let base = canonical_base(space.user_slot);
    for step in 0..SWAP_CANDIDATES {
        let n = (start + step) % SWAP_CANDIDATES;
        let Some(slot) = swap_candidate_slot(space, n) else {
            continue;
        };
        let pte = space.pte(slot);
        let frame = pte & ADDRESS_MASK;
        if pte & (PRESENT | COW) != PRESENT
            || pte & SOFT_SHARED != 0
            || code_page_refcount(frame) > 1
        {
            continue;
        }
        let swap_slot = crate::swap::store(frame)?;
        space.set_pte(slot, SWAPPED | (swap_slot << 12) | (pte & SWAP_KEPT));
        unsafe { invalidate(base + slot as u64 * PAGE_SIZE) };
        frame_release(frame);
        return Some(n + 1);
    }
    None
}

/// Brings the evicted page at `address` back. False when it was not evicted
/// or there is no memory or the disk fails.
pub fn process_swap_in(space: &mut ProcessAddressSpace, address: u64) -> bool {
    let base = canonical_base(space.user_slot);
    if address < base {
        return false;
    }
    let slot = ((address - base) / PAGE_SIZE) as usize;
    if !swappable_slot(space, slot) {
        return false;
    }
    let pte = space.pte(slot);
    if pte & (PRESENT | RESERVED) == RESERVED && slot >= PROCESS_MMAP_FIRST_INDEX {
        return populate_mmap(space, slot - PROCESS_MMAP_FIRST_INDEX);
    }
    if pte & (PRESENT | SWAPPED) != SWAPPED {
        return false;
    }
    let swap_slot = (pte & ADDRESS_MASK) >> 12;
    let Some(frame) = crate::memory::allocate_global() else {
        return false;
    };
    let physical = frame.address();
    if !crate::swap::load(swap_slot, physical) {
        crate::memory::release_global_address(physical);
        return false;
    }
    space.set_pte(slot, physical | PRESENT | (pte & SWAP_KEPT));
    unsafe { invalidate(base + slot as u64 * PAGE_SIZE) };
    crate::swap::release(swap_slot);
    true
}

/// Brings every evicted page of the process back (before it is forked or
/// its mappings are reshaped).
pub fn process_swap_in_all(space: &mut ProcessAddressSpace) {
    let base = canonical_base(space.user_slot);
    let ranges = [
        (PROCESS_HEAP_FIRST_INDEX, space.heap_mapped),
        (PROCESS_MMAP_FIRST_INDEX, space.mmap_high),
        (PROCESS_STACK_LOWEST_INDEX, PROCESS_STACK_MAX_GROWTH_PAGES),
    ];
    for (first, count) in ranges {
        for slot in first..first + count {
            if space.pte(slot) & (PRESENT | SWAPPED) == SWAPPED {
                process_swap_in(space, base + slot as u64 * PAGE_SIZE);
            }
        }
    }
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

/// Whether `slot` is a page the process records: image, heap, mapping or
/// grown stack.
fn tracked_slot(space: &ProcessAddressSpace, slot: usize) -> bool {
    slot < PROCESS_CODE_PAGES || swappable_slot(space, slot)
}

/// Write fault on a present, read-only, copy-on-write page of this process:
/// give the writer a private copy (or, if it is the last owner, just make the
/// page writable again). Returns false when the fault is not a COW fault.
pub fn process_cow_fault(space: &mut ProcessAddressSpace, address: u64) -> bool {
    crate::scheduler::stop_siblings(space.root_physical);
    let base = canonical_base(space.user_slot);
    if address < base {
        return false;
    }
    let slot = ((address - base) / PAGE_SIZE) as usize;
    if slot >= PROCESS_RESERVED_PAGES || !tracked_slot(space, slot) {
        return false;
    }
    let entry = space.pte(slot);
    if entry & (PRESENT | COW) != (PRESENT | COW) {
        return false;
    }
    let old = entry & ADDRESS_MASK;
    COW_FAULTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    let writable = (entry & !COW & !ADDRESS_MASK) | WRITABLE;
    let virtual_address = base + slot as u64 * PAGE_SIZE;
    if code_page_refcount(old) <= 1 {
        space.set_pte(slot, old | writable);
        unsafe { invalidate(virtual_address) };
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
    }
    space.set_pte(slot, copy | writable);
    unsafe { invalidate(virtual_address) };
    COW_COPIES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    frame_release(old);
    true
}

/// The physical address behind a user address of a process, if that page is
/// present.
pub fn process_translate(space: &ProcessAddressSpace, address: u64) -> Option<u64> {
    let base = canonical_base(space.user_slot);
    if address < base {
        return None;
    }
    let slot = ((address - base) / PAGE_SIZE) as usize;
    if slot >= PROCESS_RESERVED_PAGES {
        return None;
    }
    let entry = space.pte(slot);
    (entry & PRESENT != 0).then(|| (entry & ADDRESS_MASK) + (address - base) % PAGE_SIZE)
}

/// Makes the page holding `address` the process's own, so a debugger can
/// change it: a copy-on-write page is copied, and a read-only page still
/// shared with another process (program text after a fork) gets a private
/// copy with the same permissions.
pub fn process_make_private(space: &mut ProcessAddressSpace, address: u64) -> bool {
    crate::scheduler::stop_siblings(space.root_physical);
    let base = canonical_base(space.user_slot);
    if address < base {
        return false;
    }
    let slot = ((address - base) / PAGE_SIZE) as usize;
    if slot >= PROCESS_RESERVED_PAGES {
        return false;
    }
    let entry = space.pte(slot);
    if entry & PRESENT == 0 {
        return false;
    }
    if entry & COW != 0 {
        return process_cow_fault(space, address);
    }
    let old = entry & ADDRESS_MASK;
    if code_page_refcount(old) <= 1 {
        return true;
    }
    if entry & SOFT_SHARED != 0 && slot >= PROCESS_MMAP_FIRST_INDEX {
        return true;
    }
    if !tracked_slot(space, slot) {
        return false;
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
    }
    space.set_pte(slot, copy | (entry & !ADDRESS_MASK));
    unsafe { invalidate(base + slot as u64 * PAGE_SIZE) };
    frame_release(old);
    true
}

/// On-demand user stack growth: a not-present fault just below whatever is
/// currently mapped, still inside the reserved growth zone, gets a fresh
/// zeroed frame instead of crashing the process. Growth only ever proceeds
/// one page below whatever is already mapped - a fault further down (past
/// an unmapped gap) is a genuine overflow past `PROCESS_STACK_MAX_GROWTH_PAGES`,
/// not growth, and is refused so it falls through to the normal SIGSEGV path.
pub fn process_stack_fault(space: &mut ProcessAddressSpace, address: u64) -> bool {
    let base = canonical_base(space.user_slot);
    if address < base {
        return false;
    }
    let slot = ((address - base) / PAGE_SIZE) as usize;
    if !(PROCESS_STACK_LOWEST_INDEX..PROCESS_STACK_FIRST_INDEX).contains(&slot) {
        return false;
    }
    let depth = PROCESS_STACK_FIRST_INDEX - 1 - slot;
    if space.stack_grown_frame(depth) != 0 {
        return false;
    }
    let flags = PRESENT | WRITABLE | USER | if space.nx_enabled { NO_EXECUTE } else { 0 };
    // Every page from the lowest one mapped down to the faulting one, so a
    // function with a large frame can touch memory several pages below the
    // stack pointer, as on Linux.
    for below in 0..=depth {
        if space.stack_grown_frame(below) != 0 {
            continue;
        }
        let Some(frame) = crate::memory::allocate_global() else {
            return false;
        };
        let physical = frame.address();
        let index = PROCESS_STACK_FIRST_INDEX - 1 - below;
        unsafe { zero_page(physical) };
        space.set_pte(index, physical | flags);
        unsafe { invalidate(base + index as u64 * PAGE_SIZE) };
    }
    true
}

pub fn process_munmap(space: &mut ProcessAddressSpace, address: u64, pages: usize) -> bool {
    crate::scheduler::stop_siblings(space.root_physical);
    let Some(start) = mmap_index_for_address(space.user_slot, address) else {
        return false;
    };
    if pages == 0 || start + pages > mmap_limit(space.user_slot) {
        return false;
    }
    // Unmapping what is not mapped is not an error.
    for index in start..start + pages {
        let entry = space.mmap_pte(index);
        if entry == 0 {
            continue;
        }
        space.set_mmap(index, 0);
        release_entry(entry_value(entry));
    }
    true
}

pub fn map_user_image(
    state: &PagingState,
    frames: &mut FrameAllocator,
    image: &ElfImage<'_>,
) -> Option<UserImageMapping> {
    // This mapper places an image at an arbitrary base; a fixed-address one
    // would run at the wrong addresses.
    if image.is_fixed() {
        return None;
    }
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
        // An evicted page comes back, and an untouched mapping page gets its
        // frame, so a syscall can fill a buffer that is not in memory yet.
        if level == indices.len() - 1
            && entry & PRESENT == 0
            && entry & (SWAPPED | RESERVED) != 0
            && crate::scheduler::handle_swap_fault_current(address)
        {
            entry = unsafe { core::ptr::read_volatile((table as usize as *const u64).add(*index)) };
        }
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
