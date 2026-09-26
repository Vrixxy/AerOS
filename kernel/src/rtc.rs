use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::arch;

static UNIX_BASE: AtomicU64 = AtomicU64::new(0);
static MONOTONIC_BASE: AtomicU64 = AtomicU64::new(0);
/// Index into `timezone::ZONES` (0 = UTC).
static ZONE: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
struct Snapshot {
    second: u8,
    minute: u8,
    hour: u8,
    day: u8,
    month: u8,
    year: u8,
    century: u8,
    status_b: u8,
}

#[derive(Clone, Copy)]
pub struct RtcReport {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub unix_seconds: u64,
    pub verified: bool,
}

#[derive(Clone, Copy)]
pub struct UtcDateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
}

pub fn initialize() -> RtcReport {
    let mut first = read_snapshot();
    let mut stable = None;
    for _ in 0..8 {
        let second = read_snapshot();
        if first == second {
            stable = Some(second);
            break;
        }
        first = second;
    }
    let Some(snapshot) = stable else {
        return empty_report();
    };
    let binary = snapshot.status_b & 4 != 0;
    let hour_24 = snapshot.status_b & 2 != 0;
    let second = decode(snapshot.second, binary);
    let minute = decode(snapshot.minute, binary);
    let mut hour = decode(snapshot.hour & 0x7f, binary);
    if !hour_24 {
        let afternoon = snapshot.hour & 0x80 != 0;
        if hour == 12 {
            hour = 0;
        }
        if afternoon {
            hour = hour.saturating_add(12);
        }
    }
    let day = decode(snapshot.day, binary);
    let month = decode(snapshot.month, binary);
    let short_year = decode(snapshot.year, binary) as u16;
    let century = decode(snapshot.century, binary) as u16;
    let year = if (19..=99).contains(&century) {
        century * 100 + short_year
    } else if short_year >= 70 {
        1900 + short_year
    } else {
        2000 + short_year
    };
    let unix_seconds = to_unix(year, month, day, hour, minute, second).unwrap_or(0);
    let verified = unix_seconds != 0;
    if verified {
        UNIX_BASE.store(unix_seconds, Ordering::Release);
        MONOTONIC_BASE.store(crate::time::monotonic_nanoseconds(), Ordering::Release);
    }
    RtcReport {
        year,
        month,
        day,
        hour,
        minute,
        second,
        unix_seconds,
        verified,
    }
}

pub fn unix_seconds() -> u64 {
    let base = UNIX_BASE.load(Ordering::Acquire);
    let monotonic = crate::time::monotonic_nanoseconds();
    let started = MONOTONIC_BASE.load(Ordering::Acquire);
    base.saturating_add(monotonic.saturating_sub(started) / 1_000_000_000)
}

pub fn unix_nanoseconds() -> u128 {
    let base = UNIX_BASE.load(Ordering::Acquire) as u128 * 1_000_000_000;
    let monotonic = crate::time::monotonic_nanoseconds();
    let started = MONOTONIC_BASE.load(Ordering::Acquire);
    base.saturating_add(monotonic.saturating_sub(started) as u128)
}

/// Sets the running clock (not the hardware clock).
pub fn set_unix_seconds(seconds: u64) {
    UNIX_BASE.store(seconds, Ordering::Release);
    MONOTONIC_BASE.store(crate::time::monotonic_nanoseconds(), Ordering::Release);
}

pub fn timezone_index() -> usize {
    ZONE.load(Ordering::Acquire) as usize
}

pub fn set_timezone_index(index: usize) {
    if index < crate::timezone::ZONES.len() {
        ZONE.store(index as u32, Ordering::Release);
    }
}

/// Offset of the chosen time zone from UTC right now, in minutes.
pub fn local_offset_minutes() -> i32 {
    crate::timezone::offset_minutes(timezone_index(), unix_seconds())
}

/// Wall-clock seconds in the chosen time zone (Unix seconds shifted by its offset).
pub fn local_seconds() -> u64 {
    (unix_seconds() as i64 + local_offset_minutes() as i64 * 60).max(0) as u64
}

#[allow(dead_code)]
pub fn utc_date_time() -> UtcDateTime {
    date_time_at(unix_seconds())
}

/// Date and time in the chosen time zone.
pub fn local_date_time() -> UtcDateTime {
    date_time_at(local_seconds())
}

/// Writes the time into the battery-backed hardware clock (as UTC), so it
/// survives a restart without network.
#[allow(dead_code)] // the Settings app sets the clock through this
pub fn set_hardware_time(unix: u64) -> bool {
    let days = unix / 86_400;
    let seconds_of_day = unix % 86_400;
    let date = date_time_at(unix);
    let weekday = ((days + 4) % 7 + 1) as u8; // 1 = Sunday
    let second = (seconds_of_day % 60) as u8;
    let status_b = read_register(0x0b);
    let binary = status_b & 4 != 0;
    let encode = |value: u8| {
        if binary {
            value
        } else {
            (value / 10) << 4 | (value % 10)
        }
    };
    // Stop the clock updating while the registers are rewritten; 24-hour mode.
    write_register(0x0b, status_b | 0x80 | 0x02);
    write_register(0x00, encode(second));
    write_register(0x02, encode(date.minute));
    write_register(0x04, encode(date.hour));
    write_register(0x06, weekday);
    write_register(0x07, encode(date.day));
    write_register(0x08, encode(date.month));
    write_register(0x09, encode((date.year % 100) as u8));
    write_register(0x32, encode((date.year / 100) as u8));
    write_register(0x0b, (status_b | 0x02) & !0x80);
    set_unix_seconds(unix);
    true
}

#[allow(dead_code)]
fn write_register(register: u8, value: u8) {
    unsafe {
        arch::outb(0x70, 0x80 | register);
        arch::outb(0x71, value);
        arch::outb(0x70, 0);
    }
}

fn date_time_at(seconds: u64) -> UtcDateTime {
    let mut days = seconds / 86_400;
    let seconds_of_day = seconds % 86_400;
    let mut year = 1970u16;
    loop {
        let year_days = 365 + u64::from(is_leap(year));
        if days < year_days || year == 9999 {
            break;
        }
        days -= year_days;
        year += 1;
    }
    let month_days = [31u8, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u8;
    loop {
        let current =
            month_days[month as usize - 1] as u64 + u64::from(month == 2 && is_leap(year));
        if days < current || month == 12 {
            break;
        }
        days -= current;
        month += 1;
    }
    UtcDateTime {
        year,
        month,
        day: days as u8 + 1,
        hour: (seconds_of_day / 3_600) as u8,
        minute: (seconds_of_day / 60 % 60) as u8,
    }
}

fn read_snapshot() -> Snapshot {
    for _ in 0..1_000_000 {
        if read_register(0x0a) & 0x80 == 0 {
            break;
        }
        core::hint::spin_loop();
    }
    Snapshot {
        second: read_register(0x00),
        minute: read_register(0x02),
        hour: read_register(0x04),
        day: read_register(0x07),
        month: read_register(0x08),
        year: read_register(0x09),
        century: read_register(0x32),
        status_b: read_register(0x0b),
    }
}

fn read_register(register: u8) -> u8 {
    unsafe {
        arch::outb(0x70, 0x80 | register);
        let value = arch::inb(0x71);
        arch::outb(0x70, 0);
        value
    }
}

fn decode(value: u8, binary: bool) -> u8 {
    if binary {
        value
    } else {
        (value & 0x0f).saturating_add((value >> 4).saturating_mul(10))
    }
}

fn to_unix(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> Option<u64> {
    if !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let month_days = [31u8, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let leap = is_leap(year);
    let maximum = month_days[month as usize - 1] + u8::from(leap && month == 2);
    if day == 0 || day > maximum {
        return None;
    }
    let mut days = 0u64;
    for candidate in 1970..year {
        days += 365 + u64::from(is_leap(candidate));
    }
    for candidate in 1..month {
        days += month_days[candidate as usize - 1] as u64;
        if candidate == 2 && leap {
            days += 1;
        }
    }
    days = days.checked_add(day as u64 - 1)?;
    days.checked_mul(86_400)?
        .checked_add(hour as u64 * 3600)?
        .checked_add(minute as u64 * 60)?
        .checked_add(second as u64)
}

fn is_leap(year: u16) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

fn empty_report() -> RtcReport {
    RtcReport {
        year: 0,
        month: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
        unix_seconds: 0,
        verified: false,
    }
}
