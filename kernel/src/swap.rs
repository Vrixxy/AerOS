//! Swap: a disk area that holds user pages evicted from memory. The area is
//! a run of 4 KiB slots; a page is written to a free slot when it is evicted
//! and read back (and the slot freed) when its owner touches it again. The
//! page-table side is in `arch/paging.rs`; this module only stores pages.
//!
//! An area is taken from a partition that was formatted as swap (the
//! `SWAPSPACE2` signature in its first page), so a disk that holds something
//! else is never touched.

use crate::block::Disk;
use crate::sync::TicketLock;

const PAGE: usize = 4096;
const SECTORS_PER_PAGE: u64 = 8;
const MAX_SLOTS: usize = 16384;
const SIGNATURE_AT: usize = 4086;

struct State {
    disk: Option<Disk>,
    first_lba: u64,
    slots: u32,
    used: [u64; MAX_SLOTS / 64],
    in_use: u32,
    written: u64,
    read: u64,
    failures: u64,
}

static STATE: TicketLock<State> = TicketLock::new(State {
    disk: None,
    first_lba: 0,
    slots: 0,
    used: [0; MAX_SLOTS / 64],
    in_use: 0,
    written: 0,
    read: 0,
    failures: 0,
});

#[derive(Clone, Copy)]
pub struct Stats {
    pub slots: u32,
    pub in_use: u32,
    pub written: u64,
    pub read: u64,
    pub failures: u64,
}

/// Uses `sectors` sectors from `first_lba` as swap, with no header: every
/// whole page of it is a slot. Returns the number of slots.
pub fn configure(disk: Disk, first_lba: u64, sectors: u64) -> u32 {
    let slots = (sectors / SECTORS_PER_PAGE).min(MAX_SLOTS as u64) as u32;
    let mut state = STATE.lock();
    state.disk = (slots != 0).then_some(disk);
    state.first_lba = first_lba;
    state.slots = slots;
    state.used = [0; MAX_SLOTS / 64];
    state.in_use = 0;
    slots
}

/// Takes a partition that carries a swap header as the swap area (its first
/// page is the header, the rest are slots). Returns the number of slots.
pub fn configure_partition(disk: Disk, first_lba: u64, sectors: u64) -> u32 {
    let mut header = [0u8; PAGE];
    if sectors < 2 * SECTORS_PER_PAGE
        || !disk.read_run(first_lba, SECTORS_PER_PAGE as usize, &mut header)
        || &header[SIGNATURE_AT..SIGNATURE_AT + 10] != b"SWAPSPACE2"
    {
        return 0;
    }
    configure(
        disk,
        first_lba + SECTORS_PER_PAGE,
        sectors - SECTORS_PER_PAGE,
    )
}

pub fn active() -> bool {
    STATE.lock().disk.is_some()
}

pub fn stats() -> Stats {
    let state = STATE.lock();
    Stats {
        slots: state.slots,
        in_use: state.in_use,
        written: state.written,
        read: state.read,
        failures: state.failures,
    }
}

/// Writes the page at `physical` to a free slot. `None` when swap is off,
/// full, or the disk refuses.
pub fn store(physical: u64) -> Option<u64> {
    let (disk, lba, slot) = {
        let mut state = STATE.lock();
        let disk = state.disk?;
        let slot = (0..state.slots as usize)
            .find(|slot| state.used[slot / 64] & (1 << (slot % 64)) == 0)?;
        state.used[slot / 64] |= 1 << (slot % 64);
        state.in_use += 1;
        (disk, state.first_lba + slot as u64 * SECTORS_PER_PAGE, slot)
    };
    // SAFETY: `physical` is an identity-mapped page the caller owns.
    let page = unsafe { core::slice::from_raw_parts(physical as usize as *const u8, PAGE) };
    if disk.write_run(lba, SECTORS_PER_PAGE as usize, page) {
        let mut state = STATE.lock();
        state.written += 1;
        Some(slot as u64)
    } else {
        let mut state = STATE.lock();
        state.used[slot / 64] &= !(1 << (slot % 64));
        state.in_use -= 1;
        state.failures += 1;
        None
    }
}

/// Reads slot `slot` into the page at `physical`.
pub fn load(slot: u64, physical: u64) -> bool {
    let (disk, lba) = {
        let state = STATE.lock();
        let Some(disk) = state.disk else {
            return false;
        };
        if slot >= state.slots as u64 {
            return false;
        }
        (disk, state.first_lba + slot * SECTORS_PER_PAGE)
    };
    // SAFETY: `physical` is an identity-mapped page the caller owns.
    let page = unsafe { core::slice::from_raw_parts_mut(physical as usize as *mut u8, PAGE) };
    let ok = disk.read_run(lba, SECTORS_PER_PAGE as usize, page);
    let mut state = STATE.lock();
    if ok {
        state.read += 1;
    } else {
        state.failures += 1;
    }
    ok
}

/// Frees a slot.
pub fn release(slot: u64) {
    let mut state = STATE.lock();
    let slot = slot as usize;
    if slot < state.slots as usize && state.used[slot / 64] & (1 << (slot % 64)) != 0 {
        state.used[slot / 64] &= !(1 << (slot % 64));
        state.in_use -= 1;
    }
}
