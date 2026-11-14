//! The battery as the desktop shows it. Reading it runs the firmware's AML,
//! so it is read at most every half minute (sooner when the firmware reports
//! a change) and drawing a frame only looks at the cached value.

use crate::acpi_devices::{self, MAX_BATTERIES};
use crate::sync::TicketLock;
use core::fmt::Write;

const REFRESH_NS: u64 = 30_000_000_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Level {
    pub percent: u8,
    pub charging: bool,
    /// Until empty (or full while charging), when the firmware says.
    pub minutes: Option<u32>,
}

struct Cache {
    scanned: bool,
    nodes: [u16; MAX_BATTERIES],
    count: usize,
    read_ns: u64,
    changes: u32,
    level: Option<Level>,
}

static CACHE: TicketLock<Cache> = TicketLock::new(Cache {
    scanned: false,
    nodes: [0; MAX_BATTERIES],
    count: 0,
    read_ns: 0,
    changes: 0,
    level: None,
});

/// The battery's charge, or nothing on a machine without one.
pub fn level(now: u64) -> Option<Level> {
    let changes = crate::acpi_events::device_changes();
    let (scanned, nodes, count) = {
        let cache = CACHE.lock();
        if cache.scanned && cache.count == 0 {
            return None;
        }
        let fresh = cache.read_ns != 0
            && now.saturating_sub(cache.read_ns) < REFRESH_NS
            && cache.changes == changes;
        if fresh {
            return cache.level;
        }
        (cache.scanned, cache.nodes, cache.count)
    };
    if !crate::acpi_ns::report().loaded {
        let mut cache = CACHE.lock();
        cache.scanned = true;
        cache.count = 0;
        return None;
    }
    let (nodes, count, level) = crate::acpi_ns::with(|aml| {
        let (nodes, count) = if scanned {
            (nodes, count)
        } else {
            let inventory = acpi_devices::scan(aml);
            (inventory.batteries, inventory.battery_count)
        };
        let mut level = None;
        for node in &nodes[..count] {
            let battery = acpi_devices::read_battery(aml, *node);
            if let Some(percent) = battery.percent() {
                level = Some(Level {
                    percent,
                    charging: battery.charging,
                    minutes: battery.minutes(),
                });
                break;
            }
        }
        (nodes, count, level)
    });
    let mut cache = CACHE.lock();
    cache.scanned = true;
    cache.nodes = nodes;
    cache.count = count;
    cache.read_ns = now.max(1);
    cache.changes = changes;
    cache.level = level;
    level
}

/// "76%, charging", "76%, 2h 10m left" or "No battery".
pub fn describe(level: Option<Level>, out: &mut impl Write) {
    let _ = match level {
        None => write!(out, "No battery"),
        Some(level) if level.charging && level.percent >= 100 => write!(out, "Full"),
        Some(level) if level.charging => write!(out, "{}%, charging", level.percent),
        Some(Level {
            percent,
            minutes: Some(minutes),
            ..
        }) => write!(
            out,
            "{percent}%, {}h {:02}m left",
            minutes / 60,
            minutes % 60
        ),
        Some(level) => write!(out, "{}%", level.percent),
    };
}
