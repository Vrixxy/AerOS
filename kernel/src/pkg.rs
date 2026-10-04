//! Signed packages with versioned installs and rollback.
//!
//! A package file is a short text header followed by the raw bytes of its
//! files:
//!
//! ```text
//! AEROSPKG1
//! name=<name>
//! version=<version>
//! file=<relative path>;<octal mode>;<size>;<sha-256 in hex>   (up to 16)
//! data
//! <file bytes, in header order, back to back>
//! ```
//!
//! The 64-byte Ed25519 signature next to it covers the whole package file and
//! must verify against one of the keys the administrator trusted. Files are
//! installed under `<root>/apps/<name>/<version>/`, every one checked against
//! its declared hash as it is written. The previous version stays in place so
//! `rollback` is only a change of pointer in `<root>/db/<name>`; a third
//! version removes the oldest.

use core::fmt::Write;

use crate::auth::Sha256;
use crate::ed25519::{self, Verifier};
use crate::vfs::{self, VfsError};

pub const MAX_FILES: usize = 16;
const MAX_KEYS: usize = 8;
const MAX_HEADER: usize = 2048;
const MAX_MANIFEST: usize = 4096;
const NAME_MAX: usize = 24;
const VERSION_MAX: usize = 16;
const PATH_MAX: usize = 96;
const FILE_MAX: u64 = 1 << 20;
const MAGIC: &str = "AEROSPKG1";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    NoTrustedKeys,
    BadSignature,
    Malformed,
    BadName,
    BadPath,
    TooLarge,
    HashMismatch,
    AlreadyInstalled,
    NotInstalled,
    NoPrevious,
    Missing,
    Storage(VfsError),
}

impl Failure {
    pub fn message(self) -> &'static str {
        match self {
            Self::NoTrustedKeys => "no trusted keys (add one with `pkg trust`)",
            Self::BadSignature => "signature does not match any trusted key",
            Self::Malformed => "malformed package",
            Self::BadName => "invalid package name or version",
            Self::BadPath => "invalid file path in package",
            Self::TooLarge => "package too large or too many files",
            Self::HashMismatch => "file contents do not match the package's hash",
            Self::AlreadyInstalled => "that version is already installed",
            Self::NotInstalled => "package is not installed",
            Self::NoPrevious => "no previous version to roll back to",
            Self::Missing => "an installed file is missing",
            Self::Storage(_) => "storage error",
        }
    }
}

impl From<VfsError> for Failure {
    fn from(error: VfsError) -> Self {
        Self::Storage(error)
    }
}

#[derive(Clone, Copy)]
pub struct Buf<const N: usize> {
    bytes: [u8; N],
    length: usize,
}

impl<const N: usize> Buf<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            length: 0,
        }
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("")
    }

    fn from_str(text: &str) -> Self {
        let mut buffer = Self::new();
        let _ = buffer.write_str(text);
        buffer
    }
}

impl<const N: usize> Write for Buf<N> {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let count = text.len().min(N - self.length);
        self.bytes[self.length..self.length + count].copy_from_slice(&text.as_bytes()[..count]);
        self.length += count;
        if count < text.len() {
            Err(core::fmt::Error)
        } else {
            Ok(())
        }
    }
}

type Path = Buf<192>;

#[derive(Clone, Copy)]
struct Entry {
    path: Buf<PATH_MAX>,
    mode: u16,
    size: u64,
    hash: [u8; 32],
}

impl Entry {
    const EMPTY: Self = Self {
        path: Buf::new(),
        mode: 0,
        size: 0,
        hash: [0; 32],
    };
}

struct Header {
    name: Buf<NAME_MAX>,
    version: Buf<VERSION_MAX>,
    entries: [Entry; MAX_FILES],
    count: usize,
    data_start: u64,
}

pub struct Installed {
    pub name: Buf<NAME_MAX>,
    pub version: Buf<VERSION_MAX>,
    pub files: usize,
}

fn text_to_hash(text: &str) -> Option<[u8; 32]> {
    ed25519::decode_hex::<32>(text)
}

fn name_valid(text: &str, limit: usize, extra: &[char]) -> bool {
    !text.is_empty()
        && text.len() <= limit
        && text
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || extra.contains(&character))
}

fn path_valid(text: &str) -> bool {
    if text.is_empty() || text.len() > PATH_MAX || text.starts_with('/') {
        return false;
    }
    text.split('/').count() <= 8
        && text.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .chars()
                    .all(|character| character.is_ascii_graphic() && character != ';')
        })
}

pub(crate) fn read_all(path: &str, buffer: &mut [u8]) -> Result<usize, Failure> {
    let handle = vfs::open_file(path, false, false, false, 0, false)?;
    let mut total = 0;
    let result = loop {
        if total == buffer.len() {
            break Ok(total);
        }
        match vfs::read(handle, &mut buffer[total..]) {
            Ok(0) => break Ok(total),
            Ok(count) => total += count,
            Err(error) => break Err(Failure::Storage(error)),
        }
    };
    let _ = vfs::close(handle);
    result
}

fn write_all(path: &str, content: &[u8]) -> Result<(), Failure> {
    let handle = vfs::open_file(path, true, false, true, 0o644, true)?;
    let result = vfs::write(handle, content, false);
    let _ = vfs::close(handle);
    result.map(|_| ()).map_err(Failure::Storage)
}

pub(crate) fn make_directories(path: &str) -> Result<(), Failure> {
    let mut built = Path::new();
    for part in path.split('/').filter(|part| !part.is_empty()) {
        let _ = write!(built, "/{part}");
        match vfs::create_directory(built.as_str(), 0o755) {
            Ok(()) | Err(VfsError::Exists) => {}
            Err(error) => {
                let exists = vfs::metadata(built.as_str())
                    .is_ok_and(|metadata| metadata.mode & 0o170000 == 0o040000);
                if !exists {
                    return Err(Failure::Storage(error));
                }
            }
        }
    }
    Ok(())
}

fn join(parts: &[&str]) -> Path {
    let mut path = Path::new();
    for part in parts {
        let _ = path.write_str(part);
    }
    path
}

fn parse_header(raw: &[u8], length: usize) -> Result<Header, Failure> {
    let marker = b"\ndata\n";
    let end = raw[..length]
        .windows(marker.len())
        .position(|window| window == marker)
        .ok_or(Failure::Malformed)?;
    let text = core::str::from_utf8(&raw[..end]).map_err(|_| Failure::Malformed)?;
    let mut lines = text.split('\n');
    if lines.next() != Some(MAGIC) {
        return Err(Failure::Malformed);
    }
    let mut header = Header {
        name: Buf::new(),
        version: Buf::new(),
        entries: [Entry::EMPTY; MAX_FILES],
        count: 0,
        data_start: (end + marker.len()) as u64,
    };
    let mut have_name = false;
    let mut have_version = false;
    for line in lines {
        if let Some(name) = line.strip_prefix("name=") {
            if have_name || !name_valid(name, NAME_MAX, &['-', '_']) {
                return Err(Failure::BadName);
            }
            header.name = Buf::from_str(name);
            have_name = true;
        } else if let Some(version) = line.strip_prefix("version=") {
            if have_version || !name_valid(version, VERSION_MAX, &['.', '-', '_']) {
                return Err(Failure::BadName);
            }
            header.version = Buf::from_str(version);
            have_version = true;
        } else if let Some(spec) = line.strip_prefix("file=") {
            if header.count == MAX_FILES {
                return Err(Failure::TooLarge);
            }
            header.entries[header.count] = parse_entry(spec)?;
            header.count += 1;
        } else {
            return Err(Failure::Malformed);
        }
    }
    if !have_name || !have_version || header.count == 0 {
        return Err(Failure::Malformed);
    }
    Ok(header)
}

fn parse_entry(spec: &str) -> Result<Entry, Failure> {
    let mut fields = spec.split(';');
    let (Some(path), Some(mode), Some(size), Some(hash), None) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return Err(Failure::Malformed);
    };
    if !path_valid(path) {
        return Err(Failure::BadPath);
    }
    let mode = u16::from_str_radix(mode, 8).map_err(|_| Failure::Malformed)?;
    let size: u64 = size.parse().map_err(|_| Failure::Malformed)?;
    if mode > 0o777 {
        return Err(Failure::Malformed);
    }
    if size > FILE_MAX {
        return Err(Failure::TooLarge);
    }
    Ok(Entry {
        path: Buf::from_str(path),
        mode,
        size,
        hash: text_to_hash(hash).ok_or(Failure::Malformed)?,
    })
}

pub(crate) fn trusted_keys(root: &str, keys: &mut [[u8; 32]; MAX_KEYS]) -> usize {
    let path = join(&[root, "/trusted.keys"]);
    let mut raw = [0u8; MAX_KEYS * 65 + 8];
    let Ok(length) = read_all(path.as_str(), &mut raw) else {
        return 0;
    };
    let Ok(text) = core::str::from_utf8(&raw[..length]) else {
        return 0;
    };
    let mut count = 0;
    for line in text.lines() {
        if count == MAX_KEYS {
            break;
        }
        if let Some(key) = ed25519::decode_hex::<32>(line.trim()) {
            keys[count] = key;
            count += 1;
        }
    }
    count
}

pub fn trust(root: &str, key: &[u8; 32]) -> Result<(), Failure> {
    make_directories(root)?;
    let mut keys = [[0u8; 32]; MAX_KEYS];
    let count = trusted_keys(root, &mut keys);
    if keys[..count].contains(key) {
        return Ok(());
    }
    if count == MAX_KEYS {
        return Err(Failure::TooLarge);
    }
    let mut content = Buf::<{ MAX_KEYS * 65 + 8 }>::new();
    for existing in &keys[..count] {
        write_hex(&mut content, existing);
        let _ = content.write_char('\n');
    }
    write_hex(&mut content, key);
    let _ = content.write_char('\n');
    let path = join(&[root, "/trusted.keys"]);
    write_all(path.as_str(), &content.bytes[..content.length])
}

fn write_hex(out: &mut impl Write, bytes: &[u8]) {
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
}

pub(crate) fn signature_valid(
    package: &str,
    signature: &[u8; 64],
    keys: &[[u8; 32]],
) -> Result<bool, Failure> {
    for key in keys {
        let Some(mut verifier) = Verifier::new(key, signature) else {
            continue;
        };
        let handle = vfs::open_file(package, false, false, false, 0, false)?;
        let mut chunk = [0u8; 1024];
        let outcome = loop {
            match vfs::read(handle, &mut chunk) {
                Ok(0) => break Ok(()),
                Ok(count) => verifier.update(&chunk[..count]),
                Err(error) => break Err(Failure::Storage(error)),
            }
        };
        let _ = vfs::close(handle);
        outcome?;
        if verifier.finish() {
            return Ok(true);
        }
    }
    Ok(false)
}

struct Db {
    current: Buf<VERSION_MAX>,
    previous: Option<Buf<VERSION_MAX>>,
}

fn read_db(root: &str, name: &str) -> Option<Db> {
    let path = join(&[root, "/db/", name]);
    let mut raw = [0u8; 96];
    let length = read_all(path.as_str(), &mut raw).ok()?;
    let text = core::str::from_utf8(&raw[..length]).ok()?;
    let mut current = None;
    let mut previous = None;
    for line in text.lines() {
        if let Some(version) = line.strip_prefix("current=") {
            current = Some(Buf::from_str(version));
        } else if let Some(version) = line.strip_prefix("previous=")
            && version != "-"
        {
            previous = Some(Buf::from_str(version));
        }
    }
    Some(Db {
        current: current?,
        previous,
    })
}

fn write_db(root: &str, name: &str, db: &Db) -> Result<(), Failure> {
    let directory = join(&[root, "/db"]);
    make_directories(directory.as_str())?;
    let mut content = Buf::<96>::new();
    let _ = writeln!(content, "current={}", db.current.as_str());
    let _ = writeln!(
        content,
        "previous={}",
        db.previous.as_ref().map_or("-", |version| version.as_str())
    );
    let path = join(&[root, "/db/", name]);
    write_all(path.as_str(), &content.bytes[..content.length])
}

fn version_directory(root: &str, name: &str, version: &str) -> Path {
    join(&[root, "/apps/", name, "/", version])
}

fn hash_hex(hash: &[u8; 32]) -> Buf<64> {
    let mut text = Buf::new();
    write_hex(&mut text, hash);
    text
}

fn remove_entries(directory: &Path, entries: &[Entry]) {
    for entry in entries {
        let path = join(&[directory.as_str(), "/", entry.path.as_str()]);
        let _ = vfs::remove(path.as_str(), false);
    }
    for entry in entries {
        let mut depth = entry.path.as_str().matches('/').count();
        while depth > 0 {
            let mut prefix = Path::from_str(directory.as_str());
            for part in entry.path.as_str().split('/').take(depth) {
                let _ = write!(prefix, "/{part}");
            }
            let _ = vfs::remove(prefix.as_str(), true);
            depth -= 1;
        }
    }
    let manifest = join(&[directory.as_str(), "/MANIFEST"]);
    let _ = vfs::remove(manifest.as_str(), false);
    let _ = vfs::remove(directory.as_str(), true);
}

fn read_manifest(directory: &Path, entries: &mut [Entry; MAX_FILES]) -> Result<usize, Failure> {
    let path = join(&[directory.as_str(), "/MANIFEST"]);
    let mut raw = [0u8; MAX_MANIFEST];
    let length = read_all(path.as_str(), &mut raw).map_err(|failure| match failure {
        Failure::Storage(VfsError::NotFound) => Failure::Missing,
        other => other,
    })?;
    let text = core::str::from_utf8(&raw[..length]).map_err(|_| Failure::Malformed)?;
    let mut count = 0;
    for line in text.lines() {
        if count == MAX_FILES {
            return Err(Failure::TooLarge);
        }
        entries[count] = parse_entry(line)?;
        count += 1;
    }
    Ok(count)
}

fn remove_version(root: &str, name: &str, version: &str) {
    let directory = version_directory(root, name, version);
    let mut entries = [Entry::EMPTY; MAX_FILES];
    let count = read_manifest(&directory, &mut entries).unwrap_or(0);
    remove_entries(&directory, &entries[..count]);
    let name_directory = join(&[root, "/apps/", name]);
    let _ = vfs::remove(name_directory.as_str(), true);
}

pub fn install(root: &str, package: &str, signature_path: &str) -> Result<Installed, Failure> {
    let mut keys = [[0u8; 32]; MAX_KEYS];
    let key_count = trusted_keys(root, &mut keys);
    if key_count == 0 {
        return Err(Failure::NoTrustedKeys);
    }
    let mut signature = [0u8; 64];
    let mut raw_signature = [0u8; 65];
    if read_all(signature_path, &mut raw_signature)? != 64 {
        return Err(Failure::BadSignature);
    }
    signature.copy_from_slice(&raw_signature[..64]);
    if !signature_valid(package, &signature, &keys[..key_count])? {
        return Err(Failure::BadSignature);
    }

    let mut raw = [0u8; MAX_HEADER];
    let length = read_all(package, &mut raw)?;
    let header = parse_header(&raw, length)?;
    let entries = &header.entries[..header.count];
    let total: u64 = entries.iter().map(|entry| entry.size).sum();
    if vfs::metadata(package)?.size != header.data_start + total {
        return Err(Failure::Malformed);
    }
    let name = header.name.as_str();
    let version = header.version.as_str();
    let existing = read_db(root, name);
    if let Some(db) = &existing
        && (db.current.as_str() == version
            || db
                .previous
                .as_ref()
                .is_some_and(|old| old.as_str() == version))
    {
        return Err(Failure::AlreadyInstalled);
    }

    let directory = version_directory(root, name, version);
    let mut written = 0;
    let result = copy_files(package, &header, &directory, &mut written);
    if let Err(failure) = result {
        remove_entries(&directory, &entries[..written]);
        let name_directory = join(&[root, "/apps/", name]);
        let _ = vfs::remove(name_directory.as_str(), true);
        return Err(failure);
    }

    let mut manifest = Buf::<MAX_MANIFEST>::new();
    for entry in entries {
        let _ = writeln!(
            manifest,
            "{};{:o};{};{}",
            entry.path.as_str(),
            entry.mode,
            entry.size,
            hash_hex(&entry.hash).as_str()
        );
    }
    let manifest_path = join(&[directory.as_str(), "/MANIFEST"]);
    write_all(manifest_path.as_str(), &manifest.bytes[..manifest.length])?;

    let retired = existing.as_ref().and_then(|db| db.previous);
    let db = Db {
        current: header.version,
        previous: existing.map(|db| db.current),
    };
    write_db(root, name, &db)?;
    if let Some(old) = retired {
        remove_version(root, name, old.as_str());
    }
    Ok(Installed {
        name: header.name,
        version: header.version,
        files: header.count,
    })
}

fn copy_files(
    package: &str,
    header: &Header,
    directory: &Path,
    written: &mut usize,
) -> Result<(), Failure> {
    make_directories(directory.as_str())?;
    let source = vfs::open_file(package, false, false, false, 0, false)?;
    let mut offset = header.data_start;
    let mut outcome = Ok(());
    for entry in &header.entries[..header.count] {
        outcome = copy_one(source, offset, entry, directory);
        *written += 1;
        if outcome.is_err() {
            break;
        }
        offset += entry.size;
    }
    let _ = vfs::close(source);
    outcome
}

fn copy_one(source: u32, offset: u64, entry: &Entry, directory: &Path) -> Result<(), Failure> {
    let path = join(&[directory.as_str(), "/", entry.path.as_str()]);
    if let Some(slash) = path.as_str().rfind('/') {
        make_directories(&path.as_str()[..slash])?;
    }
    let output = vfs::open_file(path.as_str(), true, false, true, entry.mode, true)?;
    let mut hasher = Sha256::new();
    let mut chunk = [0u8; 512];
    let mut copied = 0u64;
    let outcome = loop {
        if copied == entry.size {
            break Ok(());
        }
        let amount = ((entry.size - copied) as usize).min(chunk.len());
        match vfs::read_at(source, (offset + copied) as usize, &mut chunk[..amount]) {
            Ok(0) => break Err(Failure::Malformed),
            Ok(count) => {
                hasher.update(&chunk[..count]);
                if let Err(error) = vfs::write(output, &chunk[..count], false) {
                    break Err(Failure::Storage(error));
                }
                copied += count as u64;
            }
            Err(error) => break Err(Failure::Storage(error)),
        }
    };
    let _ = vfs::close(output);
    outcome?;
    if hasher.finish() != entry.hash {
        return Err(Failure::HashMismatch);
    }
    vfs::chmod(path.as_str(), entry.mode)?;
    Ok(())
}

pub fn rollback(root: &str, name: &str) -> Result<Buf<VERSION_MAX>, Failure> {
    let db = read_db(root, name).ok_or(Failure::NotInstalled)?;
    let previous = db.previous.ok_or(Failure::NoPrevious)?;
    write_db(
        root,
        name,
        &Db {
            current: previous,
            previous: Some(db.current),
        },
    )?;
    Ok(previous)
}

pub fn remove(root: &str, name: &str) -> Result<(), Failure> {
    if !name_valid(name, NAME_MAX, &['-', '_']) {
        return Err(Failure::BadName);
    }
    let db = read_db(root, name).ok_or(Failure::NotInstalled)?;
    remove_version(root, name, db.current.as_str());
    if let Some(previous) = &db.previous {
        remove_version(root, name, previous.as_str());
    }
    let path = join(&[root, "/db/", name]);
    vfs::remove(path.as_str(), false)?;
    Ok(())
}

pub fn verify(root: &str, name: &str) -> Result<usize, Failure> {
    let db = read_db(root, name).ok_or(Failure::NotInstalled)?;
    let directory = version_directory(root, name, db.current.as_str());
    let mut entries = [Entry::EMPTY; MAX_FILES];
    let count = read_manifest(&directory, &mut entries)?;
    for entry in &entries[..count] {
        let path = join(&[directory.as_str(), "/", entry.path.as_str()]);
        let handle =
            vfs::open_file(path.as_str(), false, false, false, 0, false).map_err(|error| {
                if error == VfsError::NotFound {
                    Failure::Missing
                } else {
                    Failure::Storage(error)
                }
            })?;
        let mut hasher = Sha256::new();
        let mut chunk = [0u8; 512];
        let mut total = 0u64;
        let outcome = loop {
            match vfs::read(handle, &mut chunk) {
                Ok(0) => break Ok(()),
                Ok(read) => {
                    hasher.update(&chunk[..read]);
                    total += read as u64;
                }
                Err(error) => break Err(Failure::Storage(error)),
            }
        };
        let _ = vfs::close(handle);
        outcome?;
        if total != entry.size || hasher.finish() != entry.hash {
            return Err(Failure::HashMismatch);
        }
    }
    Ok(count)
}

pub fn path_of(root: &str, name: &str) -> Result<Path, Failure> {
    let db = read_db(root, name).ok_or(Failure::NotInstalled)?;
    Ok(version_directory(root, name, db.current.as_str()))
}

pub struct Listing {
    pub name: Buf<NAME_MAX>,
    pub current: Buf<VERSION_MAX>,
    pub previous: Option<Buf<VERSION_MAX>>,
}

pub fn for_each(root: &str, mut visit: impl FnMut(&Listing)) -> Result<(), Failure> {
    let directory = join(&[root, "/db"]);
    let handle = match vfs::open_directory(directory.as_str()) {
        Ok(handle) => handle,
        Err(VfsError::NotFound) => return Ok(()),
        Err(error) => return Err(Failure::Storage(error)),
    };
    let mut names = [Buf::<NAME_MAX>::new(); 16];
    let mut count = 0;
    while let Ok(Some(entry)) = vfs::next_directory_entry(handle) {
        let name = core::str::from_utf8(&entry.name[..entry.name_len as usize]).unwrap_or("");
        if name == "." || name == ".." || count == names.len() {
            continue;
        }
        names[count] = Buf::from_str(name);
        count += 1;
    }
    let _ = vfs::close(handle);
    for name in &names[..count] {
        if let Some(db) = read_db(root, name.as_str()) {
            visit(&Listing {
                name: *name,
                current: db.current,
                previous: db.previous,
            });
        }
    }
    Ok(())
}

#[cfg(feature = "boot-test")]
pub fn fuzz_header(raw: &[u8]) {
    let _ = parse_header(raw, raw.len());
}

#[cfg(feature = "boot-test")]
pub fn self_test() -> bool {
    use crate::pkg_vectors as vectors;

    const ROOT: &str = "/tmp/pkgtest";
    let stage = |name: &str, content: &[u8]| -> bool { write_all(name, content).is_ok() };
    let install = |package: &[u8], signature: &[u8]| -> Result<Installed, Failure> {
        stage("/tmp/pkgtest.pkg", package)
            .then_some(())
            .ok_or(Failure::Malformed)?;
        stage("/tmp/pkgtest.sig", signature)
            .then_some(())
            .ok_or(Failure::Malformed)?;
        install(ROOT, "/tmp/pkgtest.pkg", "/tmp/pkgtest.sig")
    };
    let file_is = |path: &str, expected: &[u8]| {
        let mut raw = [0u8; 128];
        read_all(path, &mut raw).is_ok_and(|length| &raw[..length] == expected)
    };
    let current = |name: &str| {
        read_db(ROOT, name).map(|db| {
            (
                Buf::<VERSION_MAX>::from_str(db.current.as_str()),
                db.previous
                    .map(|version| Buf::<VERSION_MAX>::from_str(version.as_str())),
            )
        })
    };

    let _ = remove(ROOT, "demo");
    let no_keys = matches!(
        install(vectors::V1, vectors::V1_SIG),
        Err(Failure::NoTrustedKeys)
    );

    let trusted = trust(ROOT, &vectors::KEY).is_ok() && trust(ROOT, &vectors::KEY).is_ok();
    let mut keys = [[0u8; 32]; MAX_KEYS];
    let one_key = trusted_keys(ROOT, &mut keys) == 1 && keys[0] == vectors::KEY;

    let wrong_key = matches!(
        install(vectors::V1, vectors::V1_SIG_OTHER),
        Err(Failure::BadSignature)
    );
    let mut tampered = [0u8; 512];
    tampered[..vectors::V1.len()].copy_from_slice(vectors::V1);
    tampered[vectors::V1.len() - 3] ^= 1;
    let tamper_rejected = matches!(
        install(&tampered[..vectors::V1.len()], vectors::V1_SIG),
        Err(Failure::BadSignature)
    );
    let short_signature = matches!(
        install(vectors::V1, &vectors::V1_SIG[..63]),
        Err(Failure::BadSignature)
    );

    let first = install(vectors::V1, vectors::V1_SIG);
    let first_ok = first.as_ref().is_ok_and(|installed| {
        installed.name.as_str() == "demo"
            && installed.version.as_str() == "1.0.0"
            && installed.files == 2
    }) && file_is("/tmp/pkgtest/apps/demo/1.0.0/bin/hello", vectors::HELLO_V1)
        && file_is(
            "/tmp/pkgtest/apps/demo/1.0.0/share/doc/readme.txt",
            vectors::README_V1,
        )
        && vfs::metadata("/tmp/pkgtest/apps/demo/1.0.0/bin/hello")
            .is_ok_and(|metadata| metadata.mode & 0o777 == 0o755)
        && current("demo")
            .is_some_and(|(version, previous)| version.as_str() == "1.0.0" && previous.is_none())
        && verify(ROOT, "demo") == Ok(2);
    let duplicate = matches!(
        install(vectors::V1, vectors::V1_SIG),
        Err(Failure::AlreadyInstalled)
    );

    let second_ok = install(vectors::V2, vectors::V2_SIG).is_ok()
        && current("demo").is_some_and(|(version, previous)| {
            version.as_str() == "1.1.0" && previous.is_some_and(|old| old.as_str() == "1.0.0")
        })
        && file_is("/tmp/pkgtest/apps/demo/1.0.0/bin/hello", vectors::HELLO_V1)
        && file_is("/tmp/pkgtest/apps/demo/1.1.0/bin/hello", vectors::HELLO_V2);

    let rolled = rollback(ROOT, "demo").is_ok_and(|version| version.as_str() == "1.0.0")
        && current("demo").is_some_and(|(version, previous)| {
            version.as_str() == "1.0.0" && previous.is_some_and(|old| old.as_str() == "1.1.0")
        })
        && verify(ROOT, "demo") == Ok(2)
        && path_of(ROOT, "demo").is_ok_and(|path| path.as_str() == "/tmp/pkgtest/apps/demo/1.0.0");
    let forward = rollback(ROOT, "demo").is_ok_and(|version| version.as_str() == "1.1.0");

    let third_ok = install(vectors::V3, vectors::V3_SIG).is_ok()
        && current("demo").is_some_and(|(version, previous)| {
            version.as_str() == "1.2.0" && previous.is_some_and(|old| old.as_str() == "1.1.0")
        })
        && vfs::metadata("/tmp/pkgtest/apps/demo/1.0.0").is_err()
        && vfs::metadata("/tmp/pkgtest/apps/demo/1.1.0/bin/hello").is_ok();

    let bad_hash = matches!(
        install(vectors::BAD_HASH, vectors::BAD_HASH_SIG),
        Err(Failure::HashMismatch)
    ) && vfs::metadata("/tmp/pkgtest/apps/bad").is_err()
        && read_db(ROOT, "bad").is_none();
    let traversal = matches!(
        install(vectors::TRAVERSAL, vectors::TRAVERSAL_SIG),
        Err(Failure::BadPath)
    ) && vfs::metadata("/tmp/escape").is_err();
    let truncated = matches!(
        install(vectors::TRUNCATED, vectors::TRUNCATED_SIG),
        Err(Failure::Malformed)
    );

    let mut listed = 0;
    let listing_ok = for_each(ROOT, |entry| {
        if entry.name.as_str() == "demo" && entry.current.as_str() == "1.2.0" {
            listed += 1;
        }
    })
    .is_ok()
        && listed == 1;

    let broken = vfs::open_file(
        "/tmp/pkgtest/apps/demo/1.2.0/bin/hello",
        true,
        false,
        true,
        0o755,
        true,
    )
    .map(|handle| {
        let _ = vfs::write(handle, b"tampered", false);
        let _ = vfs::close(handle);
    })
    .is_ok()
        && verify(ROOT, "demo") == Err(Failure::HashMismatch);

    let removed = remove(ROOT, "demo").is_ok()
        && read_db(ROOT, "demo").is_none()
        && vfs::metadata("/tmp/pkgtest/apps/demo").is_err()
        && matches!(remove(ROOT, "demo"), Err(Failure::NotInstalled))
        && matches!(rollback(ROOT, "demo"), Err(Failure::NotInstalled));
    let _ = vfs::remove("/tmp/pkgtest.pkg", false);
    let _ = vfs::remove("/tmp/pkgtest.sig", false);
    let _ = vfs::remove("/tmp/pkgtest/trusted.keys", false);
    let _ = vfs::remove("/tmp/pkgtest/db", true);
    let _ = vfs::remove("/tmp/pkgtest/apps", true);
    let _ = vfs::remove("/tmp/pkgtest", true);

    no_keys
        && trusted
        && one_key
        && wrong_key
        && tamper_rejected
        && short_signature
        && first_ok
        && duplicate
        && second_ok
        && rolled
        && forward
        && third_ok
        && bad_hash
        && traversal
        && truncated
        && listing_ok
        && broken
        && removed
}
