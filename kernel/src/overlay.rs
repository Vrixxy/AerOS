//! The union root. With the home volume mounted, the read-only boot image is
//! the lower layer and `/home/.root/upper` the upper one, and every path that
//! is not `/tmp`, `/data`, `/proc`, `/sys`, `/home` or `/media` (nor under a
//! bind mount) reads through both: the upper copy wins, a write to an image
//! file first copies it up, and removing an image file records a whiteout.
//! Whiteouts and "opaque" marks (a directory recreated over a removed one
//! hides what the image has below it) are kept in `/home/.root/overlay`, one
//! `w<path>` or `o<path>` line each.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::datafs;
use crate::mounts::Mapped;
use crate::sync::TicketLock;
use crate::vfs::{self, DirectoryEntry, Metadata, VfsError};

const UPPER: &str = "/home/.root/upper";
const MARKS_FILE: &str = "/home/.root/overlay";
const MAX_MARKS: usize = 64;
const PATH: usize = 100;
const MAX_DIRS: usize = 6;
const FLAG: u32 = 0x4000_0000;
const OTHER_ROOTS: [&str; 6] = ["tmp", "data", "proc", "sys", "home", "media"];
const DIRECTORY: u32 = 0o040000;
const KIND: u32 = 0o170000;

static ENABLED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
struct Path {
    bytes: [u8; PATH],
    length: usize,
}

impl Path {
    const fn root() -> Self {
        Path {
            bytes: [0; PATH],
            length: 0,
        }
    }

    fn as_str(&self) -> &str {
        if self.length == 0 {
            return "/";
        }
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("/")
    }

    fn is_root(&self) -> bool {
        self.length == 0
    }

    fn push(&mut self, name: &[u8]) -> Option<()> {
        if name.is_empty() || self.length + 1 + name.len() > PATH {
            return None;
        }
        self.bytes[self.length] = b'/';
        self.bytes[self.length + 1..self.length + 1 + name.len()].copy_from_slice(name);
        self.length += 1 + name.len();
        Some(())
    }

    fn pop(&mut self) -> bool {
        if self.length == 0 {
            return false;
        }
        while self.length > 0 && self.bytes[self.length - 1] != b'/' {
            self.length -= 1;
        }
        self.length = self.length.saturating_sub(1);
        true
    }

    fn parent(&self) -> Path {
        let mut parent = *self;
        parent.pop();
        parent
    }

    fn child(&self, name: &[u8]) -> Option<Path> {
        let mut child = *self;
        child.push(name)?;
        Some(child)
    }

    fn first(&self) -> &str {
        self.as_str()[1..].split('/').next().unwrap_or("")
    }
}

fn normalize(path: &str) -> Option<Path> {
    if !path.starts_with('/') {
        return None;
    }
    let mut out = Path::root();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if !out.pop() {
                    return None;
                }
            }
            name => out.push(name.as_bytes())?,
        }
    }
    Some(out)
}

fn mounted() -> bool {
    datafs::route("/home").is_some()
}

fn target(path: &str) -> Option<Path> {
    if !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    let normal = normalize(path)?;
    if !normal.is_root() && OTHER_ROOTS.contains(&normal.first()) {
        return None;
    }
    mounted().then_some(normal)
}

fn upper_of(path: &Path) -> Mapped {
    Mapped::join(UPPER, if path.is_root() { "" } else { path.as_str() })
}

fn is_directory(metadata: &Metadata) -> bool {
    metadata.mode & KIND == DIRECTORY
}

#[derive(Clone, Copy)]
struct Mark {
    used: bool,
    opaque: bool,
    length: usize,
    path: [u8; PATH],
}

impl Mark {
    const EMPTY: Mark = Mark {
        used: false,
        opaque: false,
        length: 0,
        path: [0; PATH],
    };

    fn path(&self) -> &str {
        core::str::from_utf8(&self.path[..self.length]).unwrap_or("")
    }
}

static MARKS: TicketLock<[Mark; MAX_MARKS]> = TicketLock::new([Mark::EMPTY; MAX_MARKS]);

fn under(path: &str, base: &str) -> Option<bool> {
    let rest = path.strip_prefix(base)?;
    if rest.is_empty() {
        Some(true)
    } else {
        rest.starts_with('/').then_some(false)
    }
}

/// True when the boot image's copy of `path` is hidden by a mark.
fn hidden(path: &str) -> bool {
    MARKS
        .lock()
        .iter()
        .filter(|mark| mark.used)
        .any(|mark| match under(path, mark.path()) {
            Some(exact) => !(exact && mark.opaque),
            None => false,
        })
}

fn mark_of(path: &str) -> Option<bool> {
    MARKS
        .lock()
        .iter()
        .find(|mark| mark.used && mark.path() == path)
        .map(|mark| mark.opaque)
}

fn insert_mark(path: &str, opaque: bool) -> Result<(), VfsError> {
    let mut marks = MARKS.lock();
    if !opaque {
        for mark in marks.iter_mut() {
            if mark.used && mark.path() != path && under(mark.path(), path) == Some(false) {
                *mark = Mark::EMPTY;
            }
        }
    }
    if let Some(mark) = marks
        .iter_mut()
        .find(|mark| mark.used && mark.path() == path)
    {
        mark.opaque = opaque;
        return Ok(());
    }
    let Some(slot) = marks.iter_mut().find(|mark| !mark.used) else {
        return Err(VfsError::NodeLimit);
    };
    slot.used = true;
    slot.opaque = opaque;
    slot.length = path.len();
    slot.path[..path.len()].copy_from_slice(path.as_bytes());
    Ok(())
}

fn clear_mark(path: &str) {
    for mark in MARKS.lock().iter_mut() {
        if mark.used && mark.path() == path {
            *mark = Mark::EMPTY;
        }
    }
}

fn save() {
    let marks = *MARKS.lock();
    if marks.iter().all(|mark| !mark.used) {
        let _ = vfs::remove(MARKS_FILE, false);
        return;
    }
    let Ok(file) = vfs::open_file(MARKS_FILE, true, false, true, 0o644, true) else {
        return;
    };
    for mark in marks.iter().filter(|mark| mark.used) {
        let mut line = [0u8; PATH + 2];
        line[0] = if mark.opaque { b'o' } else { b'w' };
        line[1..=mark.length].copy_from_slice(&mark.path[..mark.length]);
        line[mark.length + 1] = b'\n';
        let _ = vfs::write(file, &line[..mark.length + 2], false);
    }
    let _ = vfs::close(file);
}

fn load() {
    *MARKS.lock() = [Mark::EMPTY; MAX_MARKS];
    let Ok(file) = vfs::open_file(MARKS_FILE, false, false, false, 0, false) else {
        return;
    };
    let mut chunk = [0u8; 256];
    let mut line = [0u8; PATH + 2];
    let mut length = 0;
    while let Ok(count) = vfs::read(file, &mut chunk) {
        if count == 0 {
            break;
        }
        for byte in &chunk[..count] {
            if *byte != b'\n' {
                if length < line.len() {
                    line[length] = *byte;
                    length += 1;
                }
                continue;
            }
            if length > 2
                && let Ok(text) = core::str::from_utf8(&line[1..length])
                && text.starts_with('/')
            {
                let _ = insert_mark(text, line[0] == b'o');
            }
            length = 0;
        }
    }
    let _ = vfs::close(file);
}

/// Starts the union once the home volume carries the upper layer.
pub fn enable() {
    let _ = vfs::create_directory(UPPER, 0o755);
    load();
    ENABLED.store(true, Ordering::Relaxed);
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed) && mounted()
}

pub fn mark_count() -> usize {
    MARKS.lock().iter().filter(|mark| mark.used).count()
}

fn upper_metadata(path: &Path) -> Option<Metadata> {
    vfs::metadata(upper_of(path).as_str()).ok()
}

fn lower_metadata(path: &Path) -> Option<Metadata> {
    if hidden(path.as_str()) {
        return None;
    }
    vfs::lower_metadata(path.as_str()).ok()
}

fn merged_metadata(path: &Path) -> Result<Metadata, VfsError> {
    upper_metadata(path)
        .or_else(|| lower_metadata(path))
        .ok_or(VfsError::NotFound)
}

/// True when a program at `path` is not the boot image's own file: the upper
/// layer replaces it, or a whiteout removes it.
pub fn replaced(path: &str) -> bool {
    let Some(path) = target(path) else {
        return false;
    };
    upper_metadata(&path).is_some() || hidden(path.as_str())
}

pub fn upper_file(path: &str) -> Option<Mapped> {
    let path = target(path)?;
    let metadata = upper_metadata(&path)?;
    (!is_directory(&metadata)).then(|| upper_of(&path))
}

pub fn metadata(path: &str) -> Option<Result<Metadata, VfsError>> {
    let path = target(path)?;
    Some(merged_metadata(&path))
}

/// Creates the upper copies of `directory` and the directories above it.
fn ensure_upper(directory: &Path) -> Result<(), VfsError> {
    match vfs::create_directory(UPPER, 0o755) {
        Ok(()) | Err(VfsError::Exists) => {}
        Err(error) => return Err(error),
    }
    let mut walked = Path::root();
    for part in directory
        .as_str()
        .split('/')
        .filter(|part| !part.is_empty())
    {
        walked.push(part.as_bytes()).ok_or(VfsError::NameTooLong)?;
        if upper_metadata(&walked).is_some() {
            continue;
        }
        match vfs::create_directory(upper_of(&walked).as_str(), 0o755) {
            Ok(()) | Err(VfsError::Exists) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Turns a whiteout on `path` into an opaque mark before something new takes
/// its place, so what the image has below it stays hidden.
fn revive(path: &Path) -> Result<(), VfsError> {
    if mark_of(path.as_str()) == Some(false) {
        insert_mark(path.as_str(), true)?;
        save();
    }
    Ok(())
}

fn parent_directory(path: &Path) -> Result<Path, VfsError> {
    let parent = path.parent();
    let metadata = merged_metadata(&parent)?;
    if !is_directory(&metadata) {
        return Err(VfsError::NotDirectory);
    }
    Ok(parent)
}

fn copy_up(path: &Path, truncate: bool) -> Result<(), VfsError> {
    let input = vfs::lower_open_raw(path.as_str())?;
    let mode = vfs::lower_metadata(path.as_str())
        .map(|metadata| metadata.mode as u16 & 0o777)
        .unwrap_or(0o644);
    let outcome = (|| {
        ensure_upper(&path.parent())?;
        let upper = upper_of(path);
        let output = vfs::open_file(upper.as_str(), true, false, true, mode, true)?;
        let mut buffer = [0u8; 2048];
        let mut result = Ok(());
        while result.is_ok() && !truncate {
            match vfs::read(input, &mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let mut done = 0;
                    while done < count {
                        match vfs::write(output, &buffer[done..count], false) {
                            Ok(0) => {
                                result = Err(VfsError::FileTooLarge);
                                break;
                            }
                            Ok(written) => done += written,
                            Err(error) => {
                                result = Err(error);
                                break;
                            }
                        }
                    }
                    if result.is_err() {
                        break;
                    }
                }
                Err(error) => {
                    result = Err(error);
                    break;
                }
            }
        }
        let _ = vfs::close(output);
        if result.is_err() {
            let _ = vfs::remove(upper.as_str(), false);
        }
        result
    })();
    let _ = vfs::close(input);
    outcome
}

pub fn open_file(
    path: &str,
    create: bool,
    exclusive: bool,
    truncate: bool,
    mode: u16,
    write: bool,
) -> Option<Result<u32, VfsError>> {
    let path = target(path)?;
    Some(open_merged_file(
        &path, create, exclusive, truncate, mode, write,
    ))
}

fn open_merged_file(
    path: &Path,
    create: bool,
    exclusive: bool,
    truncate: bool,
    mode: u16,
    write: bool,
) -> Result<u32, VfsError> {
    let upper = upper_metadata(path);
    let lower = lower_metadata(path);
    if let Some(metadata) = upper.or(lower) {
        if create && exclusive {
            return Err(VfsError::Exists);
        }
        if is_directory(&metadata) {
            return Err(VfsError::IsDirectory);
        }
        if upper.is_some() {
            return vfs::open_file(upper_of(path).as_str(), false, false, truncate, mode, write);
        }
        if !write && !truncate {
            return vfs::lower_open_file(path.as_str(), false, false, false, mode, false);
        }
        copy_up(path, truncate)?;
        return vfs::open_file(upper_of(path).as_str(), false, false, truncate, mode, write);
    }
    if !create {
        return Err(VfsError::NotFound);
    }
    let parent = parent_directory(path)?;
    revive(path)?;
    ensure_upper(&parent)?;
    vfs::open_file(
        upper_of(path).as_str(),
        true,
        exclusive,
        truncate,
        mode,
        write,
    )
}

pub fn open_raw(path: &str) -> Option<Result<u32, VfsError>> {
    let path = target(path)?;
    Some(if upper_metadata(&path).is_some() {
        vfs::open_file_raw(upper_of(&path).as_str())
    } else if lower_metadata(&path).is_some() {
        vfs::lower_open_raw(path.as_str())
    } else {
        Err(VfsError::NotFound)
    })
}

#[derive(Clone, Copy)]
struct Listing {
    used: bool,
    generation: u16,
    lower: Option<u32>,
    upper: Option<u32>,
    path: Path,
}

impl Listing {
    const EMPTY: Listing = Listing {
        used: false,
        generation: 1,
        lower: None,
        upper: None,
        path: Path::root(),
    };
}

static LISTINGS: TicketLock<[Listing; MAX_DIRS]> = TicketLock::new([Listing::EMPTY; MAX_DIRS]);

pub fn owns(descriptor: u32) -> bool {
    descriptor & datafs::HANDLE_FLAG == 0 && descriptor & FLAG != 0
}

fn slot_of(descriptor: u32) -> usize {
    descriptor as usize & 0xffff
}

fn generation_of(descriptor: u32) -> u16 {
    (descriptor >> 16) as u16 & 0x3fff
}

fn snapshot(descriptor: u32) -> Result<Listing, VfsError> {
    let listings = LISTINGS.lock();
    match listings.get(slot_of(descriptor)) {
        Some(listing) if listing.used && listing.generation == generation_of(descriptor) => {
            Ok(*listing)
        }
        _ => Err(VfsError::BadDescriptor),
    }
}

pub fn open_directory(path: &str) -> Option<Result<u32, VfsError>> {
    let path = target(path)?;
    Some(open_merged_directory(&path))
}

fn open_merged_directory(path: &Path) -> Result<u32, VfsError> {
    let metadata = merged_metadata(path)?;
    if !is_directory(&metadata) {
        return Err(VfsError::NotDirectory);
    }
    let upper = match upper_metadata(path) {
        Some(found) if is_directory(&found) => Some(vfs::open_directory(upper_of(path).as_str())?),
        _ => None,
    };
    let lower = match lower_metadata(path) {
        Some(found) if is_directory(&found) => Some(vfs::lower_open_directory(path.as_str())?),
        _ => None,
    };
    if upper.is_none() && mark_count() == 0 {
        return lower.ok_or(VfsError::NotFound);
    }
    if lower.is_none() {
        return upper.ok_or(VfsError::NotFound);
    }
    let mut listings = LISTINGS.lock();
    let Some(slot) = listings.iter().position(|listing| !listing.used) else {
        drop(listings);
        for descriptor in [lower, upper].into_iter().flatten() {
            let _ = vfs::close(descriptor);
        }
        return Err(VfsError::HandleLimit);
    };
    let generation = listings[slot].generation.max(1);
    listings[slot] = Listing {
        used: true,
        generation,
        lower,
        upper,
        path: *path,
    };
    Ok(FLAG | u32::from(generation) << 16 | slot as u32)
}

pub fn next_entry(descriptor: u32) -> Result<Option<DirectoryEntry>, VfsError> {
    loop {
        let listing = snapshot(descriptor)?;
        let slot = slot_of(descriptor);
        if let Some(lower) = listing.lower {
            let Some(entry) = vfs::next_directory_entry(lower)? else {
                let _ = vfs::close(lower);
                LISTINGS.lock()[slot].lower = None;
                continue;
            };
            let name = &entry.name[..entry.name_len as usize];
            if let Some(child) = listing.path.child(name)
                && (hidden(child.as_str()) || upper_metadata(&child).is_some())
            {
                continue;
            }
            return Ok(Some(entry));
        }
        if let Some(upper) = listing.upper {
            let Some(entry) = vfs::next_directory_entry(upper)? else {
                let _ = vfs::close(upper);
                LISTINGS.lock()[slot].upper = None;
                continue;
            };
            let name = &entry.name[..entry.name_len as usize];
            if name == b"." || name == b".." {
                continue;
            }
            return Ok(Some(entry));
        }
        return Ok(None);
    }
}

pub fn close(descriptor: u32) -> Result<(), VfsError> {
    let listing = snapshot(descriptor)?;
    for inner in [listing.lower, listing.upper].into_iter().flatten() {
        let _ = vfs::close(inner);
    }
    let mut listings = LISTINGS.lock();
    let entry = &mut listings[slot_of(descriptor)];
    *entry = Listing::EMPTY;
    entry.generation = (listing.generation.wrapping_add(1) & 0x3fff).max(1);
    Ok(())
}

pub fn descriptor_metadata(descriptor: u32) -> Result<Metadata, VfsError> {
    snapshot(descriptor)?;
    Ok(Metadata {
        inode: 0x6000 + slot_of(descriptor) as u64,
        mode: DIRECTORY | 0o755,
        size: 0,
        modified: 0,
    })
}

pub fn descriptor_path(descriptor: u32) -> Option<([u8; 128], usize)> {
    let listing = snapshot(descriptor).ok()?;
    let text = listing.path.as_str();
    let mut bytes = [0u8; 128];
    bytes[..text.len()].copy_from_slice(text.as_bytes());
    Some((bytes, text.len()))
}

fn is_empty_directory(path: &Path) -> Result<bool, VfsError> {
    let descriptor = open_merged_directory(path)?;
    let first = if owns(descriptor) {
        let found = next_entry(descriptor);
        let _ = close(descriptor);
        found
    } else {
        let found = vfs::next_directory_entry(descriptor);
        let _ = vfs::close(descriptor);
        found
    };
    Ok(first?.is_none())
}

pub fn create_directory(path: &str) -> Option<Result<(), VfsError>> {
    let path = target(path)?;
    Some(create_merged_directory(&path))
}

fn create_merged_directory(path: &Path) -> Result<(), VfsError> {
    if path.is_root() || merged_metadata(path).is_ok() {
        return Err(VfsError::Exists);
    }
    let parent = parent_directory(path)?;
    revive(path)?;
    ensure_upper(&parent)?;
    vfs::create_directory(upper_of(path).as_str(), 0o755)
}

pub fn remove(path: &str, directory: bool) -> Option<Result<(), VfsError>> {
    let path = target(path)?;
    Some(remove_merged(&path, directory))
}

fn remove_merged(path: &Path, directory: bool) -> Result<(), VfsError> {
    if path.is_root() {
        return Err(VfsError::Busy);
    }
    let metadata = merged_metadata(path)?;
    let is_dir = is_directory(&metadata);
    if directory && !is_dir {
        return Err(VfsError::NotDirectory);
    }
    if !directory && is_dir {
        return Err(VfsError::IsDirectory);
    }
    if is_dir && !is_empty_directory(path)? {
        return Err(VfsError::NotEmpty);
    }
    if upper_metadata(path).is_some() {
        vfs::remove(upper_of(path).as_str(), directory)?;
    }
    if lower_metadata(path).is_some() {
        insert_mark(path.as_str(), false)?;
    } else {
        clear_mark(path.as_str());
    }
    save();
    Ok(())
}

pub fn rename(source: &str, destination: &str, no_replace: bool) -> Option<Result<(), VfsError>> {
    let from = target(source);
    let to = target(destination);
    let (from, to) = match (from, to) {
        (None, None) => return None,
        (Some(from), Some(to)) => (from, to),
        _ => return Some(vfs::move_across(source, destination, !no_replace)),
    };
    Some(rename_merged(&from, &to, no_replace))
}

fn rename_merged(from: &Path, to: &Path, no_replace: bool) -> Result<(), VfsError> {
    if from.is_root() || to.is_root() {
        return Err(VfsError::Busy);
    }
    let metadata = merged_metadata(from)?;
    if from.as_str() == to.as_str() {
        return Ok(());
    }
    if !is_directory(&metadata) {
        return vfs::move_across(from.as_str(), to.as_str(), !no_replace);
    }
    if lower_metadata(from).is_some() {
        return Err(VfsError::Busy);
    }
    if under(to.as_str(), from.as_str()) == Some(false) {
        return Err(VfsError::InvalidPath);
    }
    if merged_metadata(to).is_ok() {
        return Err(VfsError::Exists);
    }
    let parent = parent_directory(to)?;
    revive(to)?;
    ensure_upper(&parent)?;
    vfs::rename(upper_of(from).as_str(), upper_of(to).as_str())
}

pub fn chmod(path: &str, mode: u16) -> Option<Result<(), VfsError>> {
    let path = target(path)?;
    Some(chmod_merged(&path, mode))
}

fn chmod_merged(path: &Path, mode: u16) -> Result<(), VfsError> {
    let metadata = merged_metadata(path)?;
    if upper_metadata(path).is_none() {
        if is_directory(&metadata) {
            ensure_upper(path)?;
        } else {
            copy_up(path, false)?;
        }
    }
    vfs::chmod(upper_of(path).as_str(), mode)
}

fn clear_tree(directory: &Mapped, depth: usize) -> usize {
    if depth > 12 {
        return 0;
    }
    let mut removed = 0;
    loop {
        let mut names = [[0u8; 64]; 8];
        let mut lengths = [0usize; 8];
        let mut kinds = [0u8; 8];
        let mut found = 0;
        if let Ok(handle) = vfs::open_directory(directory.as_str()) {
            while found < names.len() {
                match vfs::next_directory_entry(handle) {
                    Ok(Some(entry)) => {
                        let length = entry.name_len as usize;
                        if length > names[0].len()
                            || entry.name[..length] == *b"."
                            || entry.name[..length] == *b".."
                        {
                            continue;
                        }
                        names[found][..length].copy_from_slice(&entry.name[..length]);
                        lengths[found] = length;
                        kinds[found] = entry.kind;
                        found += 1;
                    }
                    _ => break,
                }
            }
            let _ = vfs::close(handle);
        }
        if found == 0 {
            return removed;
        }
        let mut progress = false;
        for index in 0..found {
            let Ok(name) = core::str::from_utf8(&names[index][..lengths[index]]) else {
                continue;
            };
            let mut child = Mapped::join(directory.as_str(), "/");
            child = Mapped::join(child.as_str(), name);
            if kinds[index] == 4 {
                removed += clear_tree(&child, depth + 1);
                progress |= vfs::remove(child.as_str(), true).is_ok();
            } else {
                progress |= vfs::remove(child.as_str(), false).is_ok();
                removed += 1;
            }
        }
        if !progress {
            return removed;
        }
    }
}

/// Throws the upper layer away: the system goes back to the boot image.
/// Returns how many files were removed.
pub fn reset() -> usize {
    if !is_enabled() {
        return 0;
    }
    let removed = clear_tree(&Mapped::of(UPPER), 0);
    *MARKS.lock() = [Mark::EMPTY; MAX_MARKS];
    save();
    removed
}

#[cfg(feature = "boot-test")]
#[derive(Clone, Copy, Default)]
pub struct Report {
    pub enabled: bool,
    pub created: bool,
    pub listed: bool,
    pub copy_up: bool,
    pub image_intact: bool,
    pub whiteout: bool,
    pub recreate: bool,
    pub rename: bool,
    pub persists: bool,
    pub reset: bool,
    pub verified: bool,
}

#[cfg(feature = "boot-test")]
fn contents(path: &str) -> Option<([u8; 4096], usize)> {
    let handle = vfs::open_file(path, false, false, false, 0, false).ok()?;
    let mut out = [0u8; 4096];
    let mut length = 0;
    while let Ok(count) = vfs::read(handle, &mut out[length..]) {
        if count == 0 {
            break;
        }
        length += count;
    }
    let _ = vfs::close(handle);
    Some((out, length))
}

#[cfg(feature = "boot-test")]
fn lists(path: &str, wanted: &[u8]) -> bool {
    let Ok(directory) = vfs::open_directory(path) else {
        return false;
    };
    let mut seen = false;
    while let Ok(Some(entry)) = vfs::next_directory_entry(directory) {
        seen |= entry.name[..entry.name_len as usize] == *wanted;
    }
    let _ = vfs::close(directory);
    seen
}

#[cfg(feature = "boot-test")]
pub fn self_test() -> Report {
    let mut report = Report {
        enabled: is_enabled(),
        ..Report::default()
    };
    if !report.enabled {
        return report;
    }
    let _ = reset();
    let payload = [0x5au8; 900];

    let mut made = false;
    if let Ok(handle) = vfs::open_file("/bin/overlay-new", true, true, false, 0o644, true) {
        made = vfs::write(handle, &payload, false) == Ok(900);
        made &= vfs::close(handle).is_ok();
    }
    report.created = made
        && vfs::metadata("/bin/overlay-new").is_ok_and(|metadata| metadata.size == 900)
        && vfs::metadata("/home/.root/upper/bin/overlay-new").is_ok()
        && contents("/bin/overlay-new")
            .is_some_and(|(bytes, length)| length == 900 && bytes[..900] == payload)
        && vfs::open_file("/bin/overlay-new", true, true, false, 0o644, true)
            == Err(VfsError::Exists);
    report.listed = lists("/bin", b"overlay-new")
        && lists("/bin", b"init")
        && lists("/bin", b"fault-probe")
        && lists("/", b"bin")
        && lists("/", b"tmp");

    let original = vfs::lower_open_raw("/bin/fault-probe").ok().map(|handle| {
        let mut bytes = [0u8; 4096];
        let mut length = 0;
        while let Ok(count) = vfs::read(handle, &mut bytes[length..]) {
            if count == 0 {
                break;
            }
            length += count;
        }
        let _ = vfs::close(handle);
        (bytes, length)
    });
    let mut edited = false;
    if let Ok(handle) = vfs::open_file("/bin/fault-probe", false, false, false, 0, true) {
        edited = vfs::write_at(handle, 0, b"AERO") == Ok(4);
        edited &= vfs::close(handle).is_ok();
    }
    report.copy_up = edited
        && original.is_some_and(|(bytes, length)| {
            length > 8
                && contents("/bin/fault-probe").is_some_and(|(now, now_length)| {
                    now_length == length
                        && &now[..4] == b"AERO"
                        && now[4..length] == bytes[4..length]
                })
        })
        && vfs::metadata("/home/.root/upper/bin/fault-probe").is_ok();
    report.image_intact = original.is_some_and(|(bytes, length)| {
        vfs::lower_open_raw("/bin/fault-probe")
            .ok()
            .is_some_and(|handle| {
                let mut again = [0u8; 4096];
                let count = vfs::read(handle, &mut again).unwrap_or(0);
                let _ = vfs::close(handle);
                count == length.min(4096) && again[..count] == bytes[..count]
            })
    }) && vfs::open_file("/bin/overlay-new", false, false, false, 0, false)
        .is_ok();

    let mut removed = vfs::remove("/bin/aeros-swap", false).is_ok();
    removed &= matches!(vfs::metadata("/bin/aeros-swap"), Err(VfsError::NotFound));
    report.whiteout = removed
        && !lists("/bin", b"aeros-swap")
        && lists("/bin", b"aeros-bench")
        && vfs::open_file("/bin/aeros-swap", false, false, false, 0, false)
            == Err(VfsError::NotFound)
        && vfs::file("/bin/aeros-swap").is_err()
        && mark_count() == 1;

    let mut again = false;
    if let Ok(handle) = vfs::open_file("/bin/aeros-swap", true, true, false, 0o644, true) {
        again = vfs::write(handle, b"new", false) == Ok(3);
        again &= vfs::close(handle).is_ok();
    }
    let mut folders = vfs::create_directory("/opt-test", 0o755).is_ok()
        && vfs::create_directory("/opt-test", 0o755) == Err(VfsError::Exists)
        && vfs::create_directory("/missing/deeper", 0o755) == Err(VfsError::NotFound);
    folders &= vfs::remove("/opt-test", true).is_ok()
        && matches!(vfs::metadata("/opt-test"), Err(VfsError::NotFound));
    report.recreate = again
        && contents("/bin/aeros-swap")
            .is_some_and(|(bytes, length)| length == 3 && &bytes[..3] == b"new")
        && mark_of("/bin/aeros-swap") == Some(true)
        && folders;

    let mut renamed = vfs::rename("/bin/overlay-new", "/bin/overlay-moved").is_ok();
    renamed &= matches!(vfs::metadata("/bin/overlay-new"), Err(VfsError::NotFound))
        && contents("/bin/overlay-moved")
            .is_some_and(|(bytes, length)| length == 900 && bytes[..900] == payload);
    let moved_image = vfs::rename("/bin/aeros-bench", "/bin/aeros-bench2").is_ok()
        && vfs::metadata("/bin/aeros-bench").is_err()
        && vfs::metadata("/bin/aeros-bench2").is_ok_and(|metadata| metadata.size > 0)
        && lists("/bin", b"aeros-bench2")
        && !lists("/bin", b"aeros-bench");
    report.rename = renamed && moved_image && vfs::rename("/bin", "/binary") == Err(VfsError::Busy);

    datafs::unmount(0);
    let remounted = datafs::initialize();
    report.persists = remounted.verified
        && !remounted.formatted
        && is_enabled()
        && vfs::metadata("/bin/overlay-moved").is_ok_and(|metadata| metadata.size == 900)
        && contents("/bin/fault-probe").is_some_and(|(now, _)| &now[..4] == b"AERO")
        && contents("/bin/aeros-swap").is_some_and(|(_, length)| length == 3)
        && vfs::metadata("/bin/aeros-bench").is_err()
        && mark_count() >= 2;

    let cleared = reset();
    report.reset = cleared >= 3
        && mark_count() == 0
        && vfs::file("/bin/aeros-swap").is_ok_and(|file| file.data.len() > 3)
        && vfs::file("/bin/aeros-bench").is_ok()
        && vfs::file("/bin/fault-probe").is_ok_and(|file| &file.data[..4] != b"AERO")
        && vfs::metadata("/bin/overlay-moved").is_err()
        && vfs::metadata("/bin/aeros-bench2").is_err()
        && !lists("/bin", b"overlay-moved");

    report.verified = report.enabled
        && report.created
        && report.listed
        && report.copy_up
        && report.image_intact
        && report.whiteout
        && report.recreate
        && report.rename
        && report.persists
        && report.reset;
    report
}
