//! System monitor data: CPU load, memory, uptime and the list of running
//! things (the desktop, the Linux VM and its windows), for the System
//! monitor app and Settings.
//!
//! Load comes from accounting: time the desktop loop spends halted waiting
//! for interrupts is idle, everything else is busy; time spent running the
//! Linux guest is counted separately.

// The System monitor app (UI pending) is the consumer of everything here.
#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, Ordering};

use crate::sync::TicketLock;

pub const MAX_PROCESSES: usize = 20;
const WINDOW_NS: u64 = 1_000_000_000;

/// Physical RAM the firmware handed over (usable + reclaimable), set at boot.
static TOTAL_RAM: AtomicU64 = AtomicU64::new(0);
static IDLE_NS: AtomicU64 = AtomicU64::new(0);
static GUEST_NS: AtomicU64 = AtomicU64::new(0);

pub fn set_total_ram(bytes: u64) {
    TOTAL_RAM.store(bytes, Ordering::Relaxed);
}

/// Called around the desktop loop's halt: time spent there is idle time.
pub fn add_idle(nanoseconds: u64) {
    IDLE_NS.fetch_add(nanoseconds, Ordering::Relaxed);
}

/// Called around a slice of running the Linux guest.
#[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
pub fn add_guest(nanoseconds: u64) {
    GUEST_NS.fetch_add(nanoseconds, Ordering::Relaxed);
}

struct Window {
    start_ns: u64,
    idle_start: u64,
    guest_start: u64,
    load_permille: u32,
    guest_permille: u32,
}

static WINDOW: TicketLock<Window> = TicketLock::new(Window {
    start_ns: 0,
    idle_start: 0,
    guest_start: 0,
    load_permille: 0,
    guest_permille: 0,
});

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Desktop,
    LinuxVm,
    LinuxWindow,
}

#[derive(Clone, Copy)]
pub struct Process {
    pub name: [u8; 48],
    pub name_len: usize,
    pub kind: Kind,
    /// CPU share in permille of the whole machine (0 when not measured).
    pub cpu_permille: u32,
    pub memory_bytes: u64,
}

impl Process {
    const EMPTY: Self = Self {
        name: [0; 48],
        name_len: 0,
        kind: Kind::Desktop,
        cpu_permille: 0,
        memory_bytes: 0,
    };

    fn new(name: &[u8], kind: Kind, cpu_permille: u32, memory_bytes: u64) -> Self {
        let mut process = Self::EMPTY;
        let length = name.len().min(process.name.len());
        process.name[..length].copy_from_slice(&name[..length]);
        process.name_len = length;
        process.kind = kind;
        process.cpu_permille = cpu_permille;
        process.memory_bytes = memory_bytes;
        process
    }

    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("?")
    }
}

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub uptime_secs: u64,
    pub cpu_count: usize,
    /// Busy share of the last second, in permille.
    pub cpu_permille: u32,
    /// Part of that spent running the Linux guest.
    pub guest_permille: u32,
    pub memory_total: u64,
    pub memory_used: u64,
    pub memory_free: u64,
    pub heap_total: u64,
    pub heap_used: u64,
    pub processes: [Process; MAX_PROCESSES],
    pub process_count: usize,
}

/// Rolls the one-second load window forward when it is due.
fn update_window(now: u64) -> (u32, u32) {
    let mut window = WINDOW.lock();
    let idle = IDLE_NS.load(Ordering::Relaxed);
    let guest = GUEST_NS.load(Ordering::Relaxed);
    if window.start_ns == 0 {
        window.start_ns = now;
        window.idle_start = idle;
        window.guest_start = guest;
        return (0, 0);
    }
    let wall = now.saturating_sub(window.start_ns);
    if wall >= WINDOW_NS {
        let idle_delta = idle.saturating_sub(window.idle_start).min(wall);
        let guest_delta = guest.saturating_sub(window.guest_start).min(wall);
        window.load_permille = ((wall - idle_delta) * 1000 / wall) as u32;
        window.guest_permille = (guest_delta * 1000 / wall) as u32;
        window.start_ns = now;
        window.idle_start = idle;
        window.guest_start = guest;
    }
    (window.load_permille, window.guest_permille)
}

pub fn snapshot() -> Snapshot {
    let now = crate::time::monotonic_nanoseconds();
    let (load, guest_load) = update_window(now);
    let memory_free = crate::memory::TRACKED_FREE_PAGES.load(Ordering::Relaxed) * 4096;
    let memory_total = TOTAL_RAM.load(Ordering::Relaxed).max(memory_free);
    let heap = crate::heap::HEAP.stats();
    let mut snapshot = Snapshot {
        uptime_secs: now / 1_000_000_000,
        cpu_count: crate::smp::online_mask().count_ones() as usize,
        cpu_permille: load,
        guest_permille: guest_load.min(load),
        memory_total,
        memory_used: memory_total.saturating_sub(memory_free),
        memory_free,
        heap_total: heap.total_bytes as u64,
        heap_used: heap.total_bytes.saturating_sub(heap.free_bytes) as u64,
        processes: [Process::EMPTY; MAX_PROCESSES],
        process_count: 0,
    };
    let mut push = |process: Process| {
        if snapshot.process_count < MAX_PROCESSES {
            snapshot.processes[snapshot.process_count] = process;
            snapshot.process_count += 1;
        }
    };
    let desktop_share = load.saturating_sub(guest_load);
    push(Process::new(
        b"AerOS Desktop",
        Kind::Desktop,
        desktop_share,
        snapshot_heap(&heap),
    ));
    if crate::svm::linux_ready() {
        push(Process::new(
            b"Linux VM",
            Kind::LinuxVm,
            guest_load.min(load),
            crate::svm::linux_memory_bytes(),
        ));
        if let Some(windows) = crate::svm::linux_windows() {
            for window in windows.windows.iter().take(windows.count) {
                let length = window
                    .title
                    .iter()
                    .position(|b| *b == 0)
                    .unwrap_or(window.title.len());
                let title: &[u8] = if length == 0 {
                    b"Linux window"
                } else {
                    &window.title[..length]
                };
                push(Process::new(title, Kind::LinuxWindow, 0, 0));
            }
        }
    }
    snapshot
}

fn snapshot_heap(heap: &crate::heap::HeapStats) -> u64 {
    heap.total_bytes.saturating_sub(heap.free_bytes) as u64
}
