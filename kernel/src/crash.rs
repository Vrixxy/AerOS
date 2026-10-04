//! Records of user processes terminated by a CPU fault, kept in a small ring
//! and shown by the `crashes` shell command.

use crate::sync::TicketLock;

const CAPACITY: usize = 8;

#[derive(Clone, Copy)]
pub struct CrashRecord {
    pub pid: u64,
    pub vector: u64,
    pub error: u64,
    pub address: u64,
    pub rip: u64,
    pub uptime_ms: u64,
}

struct Ring {
    records: [Option<CrashRecord>; CAPACITY],
    next: usize,
    total: u64,
}

impl Ring {
    const fn new() -> Self {
        Self {
            records: [None; CAPACITY],
            next: 0,
            total: 0,
        }
    }

    fn push(&mut self, record: CrashRecord) {
        self.records[self.next] = Some(record);
        self.next = (self.next + 1) % CAPACITY;
        self.total += 1;
    }

    /// Records oldest first.
    fn for_each(&self, mut visit: impl FnMut(&CrashRecord)) {
        for offset in 0..CAPACITY {
            if let Some(record) = &self.records[(self.next + offset) % CAPACITY] {
                visit(record);
            }
        }
    }
}

static RING: TicketLock<Ring> = TicketLock::new(Ring::new());

/// Called from the fault handler, so it never blocks: a record is dropped if
/// the ring is being read at that instant.
pub fn record(pid: u64, vector: u64, error: u64, address: u64, rip: u64) {
    if let Some(mut ring) = RING.try_lock() {
        ring.push(CrashRecord {
            pid,
            vector,
            error,
            address,
            rip,
            uptime_ms: crate::time::monotonic_nanoseconds() / 1_000_000,
        });
    }
}

pub fn total() -> u64 {
    RING.lock().total
}

pub fn for_each(visit: impl FnMut(&CrashRecord)) {
    RING.lock().for_each(visit);
}

pub fn self_test() -> bool {
    let mut ring = Ring::new();
    for index in 0..(CAPACITY as u64 + 3) {
        ring.push(CrashRecord {
            pid: index,
            vector: 14,
            error: 0,
            address: 0,
            rip: 0,
            uptime_ms: 0,
        });
    }
    let mut seen = [0u64; CAPACITY];
    let mut count = 0;
    ring.for_each(|record| {
        seen[count] = record.pid;
        count += 1;
    });
    count == CAPACITY
        && ring.total == CAPACITY as u64 + 3
        && seen[0] == 3
        && seen[CAPACITY - 1] == CAPACITY as u64 + 2
}
