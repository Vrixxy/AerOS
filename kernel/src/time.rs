use core::hint::spin_loop;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static HPET_BASE: AtomicU64 = AtomicU64::new(0);
static PERIOD_FS: AtomicU64 = AtomicU64::new(0);
static COUNTER_64: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
pub struct TimeReport {
    pub present: bool,
    pub base: u64,
    pub period_fs: u64,
    pub counter_64: bool,
    pub timers: u8,
    pub start: u64,
    pub end: u64,
    pub nanoseconds: u64,
    pub verified: bool,
}

pub fn initialize(base: u64) -> TimeReport {
    if base == 0 || base & 7 != 0 {
        return empty_report();
    }
    let capabilities = unsafe { read64(base, 0) };
    let period_fs = capabilities >> 32;
    let counter_64 = capabilities & (1 << 13) != 0;
    let timers = ((capabilities >> 8) & 0x1f) as u8 + 1;
    if period_fs == 0 || period_fs > 100_000_000 {
        return empty_report();
    }
    unsafe {
        let configuration = read64(base, 0x10);
        write64(base, 0x10, configuration & !3);
        write64(base, 0xf0, 0);
        write64(base, 0x10, configuration & !2 | 1);
    }
    HPET_BASE.store(base, Ordering::Release);
    PERIOD_FS.store(period_fs, Ordering::Release);
    COUNTER_64.store(counter_64, Ordering::Release);
    let start = counter();
    let mut end = start;
    for _ in 0..5_000_000 {
        end = counter();
        if end.wrapping_sub(start) >= 1000 {
            break;
        }
        spin_loop();
    }
    let nanoseconds = ticks_to_nanoseconds(end.wrapping_sub(start));
    TimeReport {
        present: true,
        base,
        period_fs,
        counter_64,
        timers,
        start,
        end,
        nanoseconds,
        verified: end != start && timers != 0 && nanoseconds != 0,
    }
}

pub fn monotonic_nanoseconds() -> u64 {
    ticks_to_nanoseconds(counter())
}

fn counter() -> u64 {
    let base = HPET_BASE.load(Ordering::Acquire);
    if base == 0 {
        return 0;
    }
    let value = unsafe { read64(base, 0xf0) };
    if COUNTER_64.load(Ordering::Acquire) {
        value
    } else {
        value as u32 as u64
    }
}

fn ticks_to_nanoseconds(ticks: u64) -> u64 {
    let period = PERIOD_FS.load(Ordering::Acquire);
    ((ticks as u128 * period as u128) / 1_000_000).min(u64::MAX as u128) as u64
}

fn empty_report() -> TimeReport {
    TimeReport {
        present: false,
        base: 0,
        period_fs: 0,
        counter_64: false,
        timers: 0,
        start: 0,
        end: 0,
        nanoseconds: 0,
        verified: false,
    }
}

unsafe fn read64(base: u64, offset: usize) -> u64 {
    unsafe { core::ptr::read_volatile((base as usize + offset) as *const u64) }
}

unsafe fn write64(base: u64, offset: usize, value: u64) {
    unsafe {
        core::ptr::write_volatile((base as usize + offset) as *mut u64, value);
    }
}
