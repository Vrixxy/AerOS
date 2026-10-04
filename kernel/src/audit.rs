//! A minimal, append-only security/audit trail: `record()` keeps the most
//! recent entries in a bounded in-memory ring (always available, even before
//! `/data` is mounted) and best-effort appends the same line to
//! `/data/audit.log` so the trail survives a reboot. Persistence is
//! best-effort - a `record()` call that races early boot (before the data
//! filesystem is mounted) or a full disk should still keep the in-memory
//! copy rather than losing the event entirely or failing whatever triggered
//! it (a login attempt, an AV detection, ...).

use core::fmt::Write as _;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::shell::Text;
use crate::sync::TicketLock;
use crate::vfs;

// `/data` persists names as literal FAT 8.3 short names (see
// `vfs::vfs_name_to_short`): uppercase-only, <=8 base chars, <=3 extension
// chars, no case-folding - "audit.log" would be silently rejected.
const LOG_PATH: &str = "/data/AUDIT.LOG";
const RING_CAPACITY: usize = 32;
const LINE_CAPACITY: usize = 128;

#[derive(Clone, Copy)]
struct Entry {
    used: bool,
    sequence: u64,
    line: Text<LINE_CAPACITY>,
}

impl Entry {
    const EMPTY: Self = Self {
        used: false,
        sequence: 0,
        line: Text::new(),
    };
}

static RING: TicketLock<[Entry; RING_CAPACITY]> = TicketLock::new([Entry::EMPTY; RING_CAPACITY]);
static NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Records a security-relevant event under `category` (e.g. `"LOGIN"`,
/// `"AV"`): never include a password or other secret in `detail`, since this
/// is written to disk and kept in memory indefinitely.
pub fn record(category: &str, detail: core::fmt::Arguments) {
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1;
    let mut line: Text<LINE_CAPACITY> = Text::new();
    let _ = write!(
        line,
        "[{}] t={} {} ",
        sequence,
        crate::rtc::unix_seconds(),
        category
    );
    let _ = line.write_fmt(detail);
    let _ = line.push_byte(b'\n');

    let slot = NEXT_SLOT.fetch_add(1, Ordering::Relaxed) % RING_CAPACITY;
    RING.lock()[slot] = Entry {
        used: true,
        sequence,
        line,
    };

    if let Ok(descriptor) = vfs::open_file(LOG_PATH, true, false, false, 0o600, true) {
        let _ = vfs::write(descriptor, line.as_str().as_bytes(), true);
        let _ = vfs::close(descriptor);
    }
}

/// Visits every currently-held in-memory entry, oldest first.
pub fn recent(mut visit: impl FnMut(u64, &str)) {
    let ring = *RING.lock();
    let mut entries: [Option<Entry>; RING_CAPACITY] = [None; RING_CAPACITY];
    for (slot, entry) in ring.into_iter().enumerate() {
        entries[slot] = entry.used.then_some(entry);
    }
    entries.sort_unstable_by_key(|entry| entry.map(|entry| entry.sequence).unwrap_or(u64::MAX));
    for entry in entries.into_iter().flatten() {
        visit(entry.sequence, entry.line.as_str());
    }
}

/// Records a couple of events, then proves both the in-memory ring AND the
/// on-disk log (a real, separate read back from `/data/audit.log`) reflect
/// them - the persistence half is what actually survives a reboot, so it's
/// worth confirming independently of the in-memory copy.
pub(crate) fn self_test() -> bool {
    record("SELFTEST", format_args!("marker=AEROS_AUDIT_ALPHA"));
    record("SELFTEST", format_args!("marker=AEROS_AUDIT_BETA"));

    let mut saw_alpha = false;
    let mut saw_beta = false;
    recent(|_sequence, line| {
        if line.contains("AEROS_AUDIT_ALPHA") {
            saw_alpha = true;
        }
        if line.contains("AEROS_AUDIT_BETA") {
            saw_beta = true;
        }
    });

    let persisted = read_log_contains("AEROS_AUDIT_ALPHA") && read_log_contains("AEROS_AUDIT_BETA");

    saw_alpha && saw_beta && persisted
}

fn read_log_contains(needle: &str) -> bool {
    let Ok(descriptor) = vfs::open_file_raw(LOG_PATH) else {
        return false;
    };
    let mut contents = [0u8; 4096];
    let mut total = 0usize;
    while let Ok(read) = vfs::read(descriptor, &mut contents[total..]) {
        if read == 0 {
            break;
        }
        total += read;
        if total >= contents.len() {
            break;
        }
    }
    let _ = vfs::close(descriptor);
    core::str::from_utf8(&contents[..total])
        .unwrap_or("")
        .contains(needle)
}
