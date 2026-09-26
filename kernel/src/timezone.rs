//! Time zones: a table of common zones with their standard UTC offset and
//! daylight-saving rule, and the calculation of the offset in effect at a
//! given moment. (Zones that changed their rules historically are simply
//! treated with today's rules.)

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Dst {
    None,
    /// Second Sunday of March .. first Sunday of November, at 02:00 local.
    Us,
    /// Last Sunday of March .. last Sunday of October, at 01:00 UTC.
    Eu,
    /// First Sunday of October .. first Sunday of April (southern summer).
    Australia,
    /// Last Sunday of September .. first Sunday of April.
    NewZealand,
}

#[derive(Clone, Copy)]
pub struct Zone {
    pub name: &'static str,
    /// Standard-time offset from UTC in minutes.
    pub offset: i16,
    pub dst: Dst,
}

const fn zone(name: &'static str, offset: i16, dst: Dst) -> Zone {
    Zone { name, offset, dst }
}

pub const ZONES: [Zone; 46] = [
    zone("UTC", 0, Dst::None),
    zone("London", 0, Dst::Eu),
    zone("Dublin", 0, Dst::Eu),
    zone("Lisbon", 0, Dst::Eu),
    zone("Reykjavik", 0, Dst::None),
    zone("Paris", 60, Dst::Eu),
    zone("Berlin", 60, Dst::Eu),
    zone("Madrid", 60, Dst::Eu),
    zone("Rome", 60, Dst::Eu),
    zone("Amsterdam", 60, Dst::Eu),
    zone("Stockholm", 60, Dst::Eu),
    zone("Warsaw", 60, Dst::Eu),
    zone("Athens", 120, Dst::Eu),
    zone("Helsinki", 120, Dst::Eu),
    zone("Kyiv", 120, Dst::Eu),
    zone("Cairo", 120, Dst::None),
    zone("Johannesburg", 120, Dst::None),
    zone("Istanbul", 180, Dst::None),
    zone("Moscow", 180, Dst::None),
    zone("Nairobi", 180, Dst::None),
    zone("Dubai", 240, Dst::None),
    zone("Karachi", 300, Dst::None),
    zone("Delhi", 330, Dst::None),
    zone("Dhaka", 360, Dst::None),
    zone("Bangkok", 420, Dst::None),
    zone("Jakarta", 420, Dst::None),
    zone("Singapore", 480, Dst::None),
    zone("Hong Kong", 480, Dst::None),
    zone("Beijing", 480, Dst::None),
    zone("Perth", 480, Dst::None),
    zone("Tokyo", 540, Dst::None),
    zone("Seoul", 540, Dst::None),
    zone("Adelaide", 570, Dst::Australia),
    zone("Sydney", 600, Dst::Australia),
    zone("Brisbane", 600, Dst::None),
    zone("Auckland", 720, Dst::NewZealand),
    zone("Honolulu", -600, Dst::None),
    zone("Anchorage", -540, Dst::Us),
    zone("Los Angeles", -480, Dst::Us),
    zone("Denver", -420, Dst::Us),
    zone("Phoenix", -420, Dst::None),
    zone("Chicago", -360, Dst::Us),
    zone("Mexico City", -360, Dst::None),
    zone("New York", -300, Dst::Us),
    zone("Sao Paulo", -180, Dst::None),
    zone("Buenos Aires", -180, Dst::None),
];

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The civil year a day number falls in.
fn year_of(days: i64) -> i64 {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400;
    if month <= 2 { year + 1 } else { year }
}

/// 0 = Sunday.
fn weekday(days: i64) -> i64 {
    (days + 4).rem_euclid(7)
}

/// Day number of the `n`-th (1-based) Sunday of a month.
fn nth_sunday(year: i64, month: i64, n: i64) -> i64 {
    let first = days_from_civil(year, month, 1);
    first + (7 - weekday(first)) % 7 + (n - 1) * 7
}

/// Day number of the last Sunday of a month.
fn last_sunday(year: i64, month: i64) -> i64 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let last = days_from_civil(next_year, next_month, 1) - 1;
    last - weekday(last)
}

/// Whether daylight-saving time is in effect at `unix` for a zone whose
/// standard offset is `offset` minutes.
pub fn dst_active(rule: Dst, offset: i16, unix: i64) -> bool {
    let year = year_of(unix.div_euclid(86_400));
    let standard = offset as i64 * 60;
    let at = |day: i64, seconds: i64| day * 86_400 + seconds;
    match rule {
        Dst::None => false,
        Dst::Us => {
            // Changes at 02:00 local: standard time going in, daylight coming out.
            let start = at(nth_sunday(year, 3, 2), 2 * 3600) - standard;
            let end = at(nth_sunday(year, 11, 1), 2 * 3600) - standard - 3600;
            unix >= start && unix < end
        }
        Dst::Eu => {
            let start = at(last_sunday(year, 3), 3600);
            let end = at(last_sunday(year, 10), 3600);
            unix >= start && unix < end
        }
        Dst::Australia => {
            let start = at(nth_sunday(year, 10, 1), 2 * 3600) - standard;
            let end = at(nth_sunday(year, 4, 1), 3 * 3600) - standard - 3600;
            unix >= start || unix < end
        }
        Dst::NewZealand => {
            let start = at(last_sunday(year, 9), 2 * 3600) - standard;
            let end = at(nth_sunday(year, 4, 1), 3 * 3600) - standard - 3600;
            unix >= start || unix < end
        }
    }
}

/// Offset from UTC in minutes for the zone at index `index` at `unix`.
pub fn offset_minutes(index: usize, unix: u64) -> i32 {
    let zone = ZONES.get(index).copied().unwrap_or(ZONES[0]);
    let extra = if dst_active(zone.dst, zone.offset, unix as i64) {
        60
    } else {
        0
    };
    zone.offset as i32 + extra
}

/// Zone index by name (case-insensitive), for saved settings.
#[allow(dead_code)]
pub fn find(name: &str) -> Option<usize> {
    ZONES
        .iter()
        .position(|zone| zone.name.eq_ignore_ascii_case(name))
}

#[cfg(feature = "boot-test")]
pub struct TimezoneTest {
    pub verified: bool,
}

/// Known instants in several zones (summer and winter, both hemispheres).
#[cfg(feature = "boot-test")]
pub fn self_test() -> TimezoneTest {
    let utc =
        |y: i64, m: i64, d: i64, h: i64| (days_from_civil(y, m, d) * 86_400 + h * 3600) as u64;
    let cases: [(&str, u64, i32); 14] = [
        ("New York", utc(2026, 7, 1, 12), -240),
        ("New York", utc(2026, 1, 15, 12), -300),
        ("New York", utc(2026, 3, 8, 6), -300), // just before 2026-03-08 02:00 EST
        ("New York", utc(2026, 3, 8, 8), -240), // just after the change
        ("New York", utc(2026, 11, 1, 5), -240), // before the 2026-11-01 change
        ("New York", utc(2026, 11, 1, 7), -300),
        ("London", utc(2026, 7, 1, 12), 60),
        ("London", utc(2026, 12, 25, 12), 0),
        ("Berlin", utc(2026, 3, 29, 0), 60),
        ("Berlin", utc(2026, 3, 29, 2), 120),
        ("Sydney", utc(2026, 1, 15, 12), 660),
        ("Sydney", utc(2026, 7, 1, 12), 600),
        ("Auckland", utc(2026, 1, 15, 12), 780),
        ("Tokyo", utc(2026, 7, 1, 12), 540),
    ];
    let verified = cases.iter().all(|(name, unix, expected)| {
        find(name).is_some_and(|i| offset_minutes(i, *unix) == *expected)
    });
    TimezoneTest { verified }
}
