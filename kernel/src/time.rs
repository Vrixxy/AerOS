use core::hint::spin_loop;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

static HPET_BASE: AtomicU64 = AtomicU64::new(0);
static PERIOD_FS: AtomicU64 = AtomicU64::new(0);
static COUNTER_64: AtomicBool = AtomicBool::new(false);

// The fast clock. Reading the HPET is a memory-mapped access to a device;
// under a hypervisor each read costs tens of microseconds, and the kernel
// reads the clock on every system call. The time stamp counter is calibrated
// against the HPET at boot and then read instead; the HPET stays the
// reference (and the fallback when the counter is not steady enough).
//
// The three values below change together, so readers use a sequence number:
// odd while an update is under way.
static TSC_ENABLED: AtomicBool = AtomicBool::new(false);
static TSC_SEQ: AtomicU32 = AtomicU32::new(0);
static TSC_BASE: AtomicU64 = AtomicU64::new(0);
static NS_BASE: AtomicU64 = AtomicU64::new(0);
/// Nanoseconds per counter tick, 32.32 fixed point.
static TSC_MULT: AtomicU64 = AtomicU64::new(0);
/// Where the first calibration ended, for the long-baseline refinement.
static TSC_ORIGIN: AtomicU64 = AtomicU64::new(0);
static NS_ORIGIN: AtomicU64 = AtomicU64::new(0);
static TICKS: AtomicU32 = AtomicU32::new(0);
static RESYNC_BUSY: AtomicBool = AtomicBool::new(false);

const CALIBRATION_WINDOW_NS: u64 = 40_000_000;
/// Timer ticks (100 Hz) between refinements: ten seconds.
const RESYNC_TICKS: u32 = 1000;

fn read_tsc() -> u64 {
    unsafe { core::arch::x86_64::_rdtsc() }
}

fn publish(tsc: u64, nanoseconds: u64, mult: u64) {
    let sequence = TSC_SEQ.load(Ordering::Relaxed);
    TSC_SEQ.store(sequence.wrapping_add(1), Ordering::Release);
    TSC_BASE.store(tsc, Ordering::Release);
    NS_BASE.store(nanoseconds, Ordering::Release);
    TSC_MULT.store(mult, Ordering::Release);
    TSC_SEQ.store(sequence.wrapping_add(2), Ordering::Release);
}

/// The clock reading for counter value `tsc`.
fn nanoseconds_at(tsc: u64) -> u64 {
    loop {
        let before = TSC_SEQ.load(Ordering::Acquire);
        if before & 1 != 0 {
            core::hint::spin_loop();
            continue;
        }
        let base = TSC_BASE.load(Ordering::Acquire);
        let origin = NS_BASE.load(Ordering::Acquire);
        let mult = TSC_MULT.load(Ordering::Acquire);
        if TSC_SEQ.load(Ordering::Acquire) == before {
            let delta = tsc.wrapping_sub(base);
            return origin.wrapping_add(((u128::from(delta) * u128::from(mult)) >> 32) as u64);
        }
    }
}

fn tsc_nanoseconds() -> u64 {
    nanoseconds_at(read_tsc())
}

/// Measures the counter against the HPET over a short window: returns
/// `(counter ticks, HPET nanoseconds, counter value at the end, HPET
/// nanoseconds at the end)`.
fn measure_window() -> (u64, u64, u64, u64) {
    let start_ticks = counter();
    let start_tsc = read_tsc();
    let mut end_ticks = start_ticks;
    for _ in 0..50_000_000u32 {
        end_ticks = counter();
        if ticks_to_nanoseconds(end_ticks.wrapping_sub(start_ticks)) >= CALIBRATION_WINDOW_NS {
            break;
        }
        spin_loop();
    }
    let end_tsc = read_tsc();
    (
        end_tsc.wrapping_sub(start_tsc),
        ticks_to_nanoseconds(end_ticks.wrapping_sub(start_ticks)),
        end_tsc,
        ticks_to_nanoseconds(end_ticks),
    )
}

/// Calibrates the counter against the HPET twice and uses it only if the two
/// rates agree: a counter that changes speed (frequency scaling without an
/// invariant counter) is not used.
fn calibrate_tsc() {
    let (first_ticks, first_ns, _, _) = measure_window();
    let (second_ticks, second_ns, end_tsc, end_ns) = measure_window();
    if first_ns == 0 || second_ns == 0 || first_ticks == 0 || second_ticks == 0 {
        return;
    }
    // ns per tick in 32.32.
    let first = (u128::from(first_ns) << 32) / u128::from(first_ticks);
    let second = (u128::from(second_ns) << 32) / u128::from(second_ticks);
    let spread = first.abs_diff(second);
    // Within 2 per cent, and a plausible rate (100 MHz to 10 GHz).
    let rate_ok = second > (1u128 << 32) / 10 && second < (1u128 << 32) * 10;
    if spread * 50 > first || !rate_ok {
        return;
    }
    let mult = second as u64;
    TSC_ORIGIN.store(end_tsc, Ordering::Relaxed);
    NS_ORIGIN.store(end_ns, Ordering::Relaxed);
    publish(end_tsc, end_ns, mult);
    TSC_ENABLED.store(true, Ordering::Release);
}

/// The counter's rate in hertz when the fast clock is in use.
pub fn tsc_hz() -> Option<u64> {
    if !TSC_ENABLED.load(Ordering::Acquire) {
        return None;
    }
    let mult = TSC_MULT.load(Ordering::Acquire);
    (mult != 0).then(|| ((1u128 << 32) * 1_000_000_000 / u128::from(mult)) as u64)
}

/// Called from the timer interrupt. Every ten seconds the rate is refined
/// from the whole time since boot, so the error of the short boot-time
/// calibration shrinks and the counter cannot drift away from the HPET. The
/// clock is re-based at its current value, so it never goes backwards.
pub fn tick() {
    if !TSC_ENABLED.load(Ordering::Acquire) {
        return;
    }
    if !TICKS
        .fetch_add(1, Ordering::Relaxed)
        .is_multiple_of(RESYNC_TICKS)
    {
        return;
    }
    if RESYNC_BUSY.swap(true, Ordering::Acquire) {
        return;
    }
    let hpet_ns = ticks_to_nanoseconds(counter());
    let tsc = read_tsc();
    let origin_tsc = TSC_ORIGIN.load(Ordering::Relaxed);
    let origin_ns = NS_ORIGIN.load(Ordering::Relaxed);
    let elapsed_tsc = tsc.wrapping_sub(origin_tsc);
    let elapsed_ns = hpet_ns.wrapping_sub(origin_ns);
    // Wait until the baseline is long enough to beat the short calibration.
    if elapsed_tsc > 0 && elapsed_ns > 2_000_000_000 {
        let mult = ((u128::from(elapsed_ns) << 32) / u128::from(elapsed_tsc)) as u64;
        let current = TSC_MULT.load(Ordering::Acquire);
        // A big jump means something odd (a stalled or migrated virtual CPU):
        // keep the old rate and let the next round look again.
        if mult.abs_diff(current) * 50 < current {
            let now = nanoseconds_at(tsc);
            publish(tsc, now, mult);
        }
    }
    RESYNC_BUSY.store(false, Ordering::Release);
}

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
    calibrate_tsc();
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
    if TSC_ENABLED.load(Ordering::Relaxed) {
        return tsc_nanoseconds();
    }
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
