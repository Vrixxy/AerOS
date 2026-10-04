//! A ring of the most recent Linux-ABI syscalls (task, number, first
//! argument, time), shown by the privileged `strace` shell command.

use crate::sync::TicketLock;

const CAPACITY: usize = 64;

#[derive(Clone, Copy)]
pub struct Entry {
    pub sequence: u64,
    pub task: u64,
    pub number: u64,
    pub argument: u64,
    pub at_ms: u64,
}

struct Ring {
    entries: [Option<Entry>; CAPACITY],
    next: usize,
    total: u64,
}

impl Ring {
    const fn new() -> Self {
        Self {
            entries: [None; CAPACITY],
            next: 0,
            total: 0,
        }
    }

    fn push(&mut self, task: u64, number: u64, argument: u64, at_ms: u64) {
        self.total += 1;
        self.entries[self.next] = Some(Entry {
            sequence: self.total,
            task,
            number,
            argument,
            at_ms,
        });
        self.next = (self.next + 1) % CAPACITY;
    }

    /// Oldest first.
    fn for_each(&self, mut visit: impl FnMut(&Entry)) {
        for offset in 0..CAPACITY {
            if let Some(entry) = &self.entries[(self.next + offset) % CAPACITY] {
                visit(entry);
            }
        }
    }
}

static RING: TicketLock<Ring> = TicketLock::new(Ring::new());

/// Called on every syscall, so it never blocks: an entry is dropped if the
/// ring is being read at that instant.
pub fn record(task: u64, number: u64, argument: u64) {
    if let Some(mut ring) = RING.try_lock() {
        ring.push(
            task,
            number,
            argument,
            crate::time::monotonic_nanoseconds() / 1_000_000,
        );
    }
}

pub fn total() -> u64 {
    RING.lock().total
}

pub fn for_each(visit: impl FnMut(&Entry)) {
    RING.lock().for_each(visit);
}

pub fn self_test() -> bool {
    let mut ring = Ring::new();
    for index in 0..(CAPACITY as u64 + 5) {
        ring.push(1, index, index * 2, 0);
    }
    let (mut count, mut first, mut last) = (0, 0, 0);
    ring.for_each(|entry| {
        if count == 0 {
            first = entry.number;
        }
        last = entry.number;
        count += 1;
    });
    count == CAPACITY
        && first == 5
        && last == CAPACITY as u64 + 4
        && ring.total == CAPACITY as u64 + 5
}
