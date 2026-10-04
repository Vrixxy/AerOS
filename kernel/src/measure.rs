//! Measured boot, in software: the boot image and the built-in program
//! archive are hashed at start-up and folded into a running value
//! (`pcr = SHA-256(pcr || digest)`, the way a TPM extends a register), the
//! log goes to the audit log, and the image's digest is compared with the one
//! sealed when the image was installed. With no TPM in the machine the
//! comparison is the only protection against an altered image on disk; the
//! log and register are laid out so a TPM extend can be added next to
//! `record`.

use core::fmt::Write;

use crate::auth::Sha256;
#[cfg(feature = "boot-test")]
use crate::auth::sha256;
use crate::sync::TicketLock;
use crate::vfs;

pub const REFERENCE_PATH: &str = "/data/MEASURE.REF";
const MAX_ENTRIES: usize = 8;

#[derive(Clone, Copy)]
pub struct Entry {
    pub name: &'static str,
    pub digest: [u8; 32],
}

struct Log {
    entries: [Entry; MAX_ENTRIES],
    count: usize,
    register: [u8; 32],
}

static LOG: TicketLock<Log> = TicketLock::new(Log {
    entries: [Entry {
        name: "",
        digest: [0; 32],
    }; MAX_ENTRIES],
    count: 0,
    register: [0; 32],
});

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// Nothing was sealed yet, or the image could not be read.
    NoReference,
    Matches,
    Changed,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::NoReference => "no-reference",
            Self::Matches => "matches",
            Self::Changed => "changed",
        }
    }
}

/// The digest `register` takes after extending with `digest`.
pub fn extend(register: &[u8; 32], digest: &[u8; 32]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(register);
    hash.update(digest);
    hash.finish()
}

/// Adds a measurement to the log and extends the register.
pub fn record(name: &'static str, digest: [u8; 32]) {
    let mut log = LOG.lock();
    if log.count == MAX_ENTRIES {
        return;
    }
    let index = log.count;
    log.entries[index] = Entry { name, digest };
    log.register = extend(&log.register, &digest);
    log.count += 1;
}

pub fn register() -> [u8; 32] {
    LOG.lock().register
}

pub fn entries(mut visit: impl FnMut(&Entry)) {
    let log = LOG.lock();
    for entry in &log.entries[..log.count] {
        visit(entry);
    }
}

fn boot_image_digest() -> Option<[u8; 32]> {
    let disk = crate::blockdev::boot_disk()?;
    let start = crate::fat::volume_start()?;
    let mut volume = crate::fatfs::Fs::mount(disk, start).ok()?;
    crate::update::boot_image_digest(&mut volume)
}

/// SHA-256 of a file, read in pieces.
#[cfg(feature = "boot-test")]
pub fn hash_file(path: &str) -> Option<[u8; 32]> {
    let handle = vfs::open_file(path, false, false, false, 0, false).ok()?;
    let mut hash = Sha256::new();
    let mut chunk = [0u8; 4096];
    let ok = loop {
        match vfs::read(handle, &mut chunk) {
            Ok(0) => break true,
            Ok(count) => hash.update(&chunk[..count]),
            Err(_) => break false,
        }
    };
    let _ = vfs::close(handle);
    ok.then(|| hash.finish())
}

/// Hash of the built-in program archive: every path and every byte.
fn initramfs_digest() -> [u8; 32] {
    let mut hash = Sha256::new();
    for entry in crate::initramfs::entries() {
        hash.update(entry.path.as_bytes());
        hash.update(&(entry.data.len() as u64).to_le_bytes());
        hash.update(entry.data);
    }
    hash.finish()
}

/// Measures the boot image and the program archive. Returns whether the
/// image could be read.
pub fn measure_boot() -> bool {
    record("initramfs", initramfs_digest());
    match boot_image_digest() {
        Some(digest) => {
            record("boot-image", digest);
            true
        }
        None => false,
    }
}

fn hex(digest: &[u8; 32], out: &mut impl Write) {
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
}

pub fn hex_text(digest: &[u8; 32]) -> crate::pkg::Buf<64> {
    let mut text = crate::pkg::Buf::new();
    hex(digest, &mut text);
    text
}

/// Compares the logged boot image with the digest sealed at `reference`.
pub fn compare(reference: &str) -> Status {
    let mut current = None;
    entries(|entry| {
        if entry.name == "boot-image" {
            current = Some(entry.digest);
        }
    });
    let Some(current) = current else {
        return Status::NoReference;
    };
    let mut raw = [0u8; 80];
    let Some(handle) = vfs::open_file(reference, false, false, false, 0, false).ok() else {
        return Status::NoReference;
    };
    let length = vfs::read(handle, &mut raw).unwrap_or(0);
    let _ = vfs::close(handle);
    let sealed = core::str::from_utf8(&raw[..length])
        .ok()
        .and_then(|text| crate::ed25519::decode_hex::<32>(text.trim()));
    match sealed {
        None => Status::NoReference,
        Some(sealed) if sealed == current => Status::Matches,
        Some(_) => Status::Changed,
    }
}

/// Records `digest` as the expected boot image at `reference`.
pub fn seal_digest(reference: &str, digest: &[u8; 32]) -> bool {
    let Ok(handle) = vfs::open_file(reference, true, false, true, 0o644, true) else {
        return false;
    };
    let text = hex_text(digest);
    let written = vfs::write(handle, text.as_str().as_bytes(), false);
    let _ = vfs::close(handle);
    written.is_ok()
}

/// Seals the digest of the boot image measured at start-up.
pub fn seal(reference: &str) -> bool {
    let mut current = None;
    entries(|entry| {
        if entry.name == "boot-image" {
            current = Some(entry.digest);
        }
    });
    current.is_some_and(|digest| seal_digest(reference, &digest))
}

#[cfg(feature = "boot-test")]
pub struct TestReport {
    pub entries: usize,
    pub image_measured: bool,
    pub register_chain: bool,
    pub file_hash: bool,
    pub seal_matches: bool,
    pub tamper_detected: bool,
    pub no_reference: bool,
}

#[cfg(feature = "boot-test")]
pub fn self_test(image_measured: bool) -> TestReport {
    let mut chain = [0u8; 32];
    let mut count = 0;
    entries(|entry| {
        chain = extend(&chain, &entry.digest);
        count += 1;
    });
    let data = b"measured boot test data";
    let path = "/tmp/measure.test";
    let reference = "/tmp/measure.ref";
    let written = vfs::open_file(path, true, false, true, 0o644, true)
        .ok()
        .is_some_and(|handle| {
            let ok = vfs::write(handle, data, false).is_ok();
            let _ = vfs::close(handle);
            ok
        });
    let file_hash = written && hash_file(path) == Some(sha256(data));
    let _ = vfs::remove(path, false);
    let _ = vfs::remove(reference, false);
    let no_reference = compare(reference) == Status::NoReference;
    let sealed = seal(reference);
    let seal_matches = !image_measured || (sealed && compare(reference) == Status::Matches);
    let other = sha256(b"another image");
    let tampered = seal_digest(reference, &other);
    let tamper_detected = !image_measured || (tampered && compare(reference) == Status::Changed);
    let _ = vfs::remove(reference, false);
    TestReport {
        entries: count,
        image_measured,
        register_chain: chain == register() && count != 0,
        file_hash,
        seal_matches,
        tamper_detected,
        no_reference,
    }
}
