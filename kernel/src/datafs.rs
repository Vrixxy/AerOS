//! Real persistent storage behind the VFS: FAT volumes mounted into the
//! tree. `/home` is the user's own volume (a disk labelled `AEROSHOME`);
//! USB sticks and SD cards that carry a FAT volume appear under
//! `/media/<label>` as they are plugged in. The VFS routes any path below a
//! mount point here; descriptors from this module have the top bit set so
//! they can't be confused with the in-memory tree's.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::fatfs::{self, Box64, Disk, FsError, Node};
use crate::sync::TicketLock;
use crate::vfs::{DirectoryEntry, Metadata, VfsError};

pub const HANDLE_FLAG: u32 = 0x8000_0000;
const MAX_HANDLES: usize = 24;
/// Mount 0 is /home; 1.. are removable media.
const MOUNTS: usize = 4;
const HOME_MOUNT: usize = 0;
/// Pseudo mount number for the `/media` directory itself.
pub const MEDIA_ROOT: usize = usize::MAX;
const HOME_LABEL: &[u8; 9] = b"AEROSHOME";
const BLANK_MARKER: &[u8; 16] = b"AEROS-DATA-BLANK";
const NAME_BYTES: usize = 24;

/// Set when a removable disk appeared or vanished; `poll` then rescans.
pub static MEDIA_DIRTY: AtomicBool = AtomicBool::new(false);
static EPOCH: AtomicU32 = AtomicU32::new(1);

struct Mount {
    used: bool,
    disk: Disk,
    epoch: u32,
    name: [u8; NAME_BYTES],
    name_len: usize,
    fs: Option<Box64>,
}

const EMPTY_MOUNT: Mount = Mount {
    used: false,
    disk: Disk::Usb,
    epoch: 0,
    name: [0; NAME_BYTES],
    name_len: 0,
    fs: None,
};

struct Handle {
    used: bool,
    generation: u16,
    mount: usize,
    epoch: u32,
    node: Node,
    directory: bool,
    /// The `/media` listing itself (entries are the mounts).
    media_root: bool,
    /// File position, or the next directory entry index.
    cursor: u64,
    writable: bool,
}

const EMPTY_HANDLE: Handle = Handle {
    used: false,
    generation: 1,
    mount: 0,
    epoch: 0,
    node: Node::EMPTY,
    directory: false,
    media_root: false,
    cursor: 0,
    writable: false,
};

struct State {
    mounts: [Mount; MOUNTS],
    handles: [Handle; MAX_HANDLES],
    /// Disks the user ejected: not mounted again until they are unplugged.
    ejected: [Option<Disk>; 2],
}

static STATE: TicketLock<State> = TicketLock::new(State {
    mounts: [EMPTY_MOUNT; MOUNTS],
    handles: [EMPTY_HANDLE; MAX_HANDLES],
    ejected: [None; 2],
});

/// What mounting the home volume did.
#[derive(Clone, Copy)]
pub struct HomeReport {
    pub present: bool,
    pub formatted: bool,
    pub fat32: bool,
    pub clusters: u32,
    pub free_clusters: u32,
    pub verified: bool,
}

fn map_error(error: FsError) -> VfsError {
    match error {
        FsError::NotFound => VfsError::NotFound,
        FsError::Exists => VfsError::Exists,
        FsError::NotDirectory => VfsError::NotDirectory,
        FsError::IsDirectory => VfsError::IsDirectory,
        FsError::NotEmpty => VfsError::NotEmpty,
        FsError::ReadOnly => VfsError::PermissionDenied,
        FsError::NoSpace => VfsError::NodeLimit,
        FsError::InvalidName | FsError::Unsupported => VfsError::InvalidPath,
        FsError::TooLarge => VfsError::FileTooLarge,
        FsError::Io | FsError::Corrupt => VfsError::Busy,
    }
}

fn names_equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

/// Which mount a path belongs to and the path inside it (`/` for its root).
/// `/media` alone routes to [`MEDIA_ROOT`].
pub fn route(path: &str) -> Option<(usize, &str)> {
    let state = STATE.lock();
    let below = |prefix: &str| -> Option<&str> {
        let rest = path.strip_prefix(prefix)?;
        match rest.as_bytes().first() {
            None => Some("/"),
            Some(b'/') => Some(rest),
            _ => None,
        }
    };
    if let Some(rest) = below("/home") {
        return state.mounts[HOME_MOUNT].used.then_some((HOME_MOUNT, rest));
    }
    let rest = below("/media")?;
    let trimmed = rest.trim_start_matches('/');
    if trimmed.is_empty() {
        return state
            .mounts
            .iter()
            .skip(1)
            .any(|mount| mount.used)
            .then_some((MEDIA_ROOT, "/"));
    }
    let (name, inside) = match trimmed.find('/') {
        Some(at) => (&trimmed[..at], &trimmed[at..]),
        None => (trimmed, "/"),
    };
    state
        .mounts
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, mount)| {
            mount.used && names_equal(&mount.name[..mount.name_len], name.as_bytes())
        })
        .map(|(index, _)| (index, inside))
}

/// Candidate disks for the home volume: every disk that isn't the boot disk.
fn candidates() -> [Option<Disk>; 8] {
    let mut list = [None; 8];
    let mut count = 0;
    let mut add = |disk: Disk| {
        if count < list.len() && disk.sectors() > 0 {
            list[count] = Some(disk);
            count += 1;
        }
    };
    for index in 0..crate::ahci::disk_count() {
        if index != crate::ahci::boot_disk() {
            add(Disk::Ahci(index));
        }
    }
    add(Disk::Usb);
    add(Disk::Sd);
    add(Disk::Nvme);
    add(Disk::Virtio);
    list
}

fn disk_read(disk: Disk, lba: u64, out: &mut [u8; 512]) -> bool {
    fatfs::read_sector_raw(disk, lba, out)
}

/// Whether sector 0 is a FAT boot sector (a "superfloppy" volume).
fn is_boot_sector(sector: &[u8; 512]) -> bool {
    sector[510] == 0x55
        && sector[511] == 0xaa
        && u16::from_le_bytes([sector[11], sector[12]]) == 512
        && sector[13].is_power_of_two()
        && (1..=4).contains(&sector[16])
}

/// Where the FAT volume on a disk starts: 0 for a bare volume, else the
/// first FAT partition of an MBR.
fn volume_start(sector: &[u8; 512]) -> Option<u64> {
    if is_boot_sector(sector) {
        return Some(0);
    }
    if sector[510] != 0x55 || sector[511] != 0xaa {
        return None;
    }
    (0..4).find_map(|entry| {
        let at = 446 + entry * 16;
        let kind = sector[at + 4];
        let first = u32::from_le_bytes([
            sector[at + 8],
            sector[at + 9],
            sector[at + 10],
            sector[at + 11],
        ]);
        (matches!(
            kind,
            0x04 | 0x06 | 0x0b | 0x0c | 0x0e | 0x14 | 0x16 | 0x1b | 0x1c | 0x1e
        ) && first > 0)
            .then_some(first as u64)
    })
}

/// Looks for the home disk and mounts it (formatting a disk that carries the
/// blank-disk marker the launcher writes), then mounts any removable media.
pub fn initialize() -> HomeReport {
    let mut report = HomeReport {
        present: false,
        formatted: false,
        fat32: false,
        clusters: 0,
        free_clusters: 0,
        verified: false,
    };
    for disk in candidates().into_iter().flatten() {
        let mut first = [0u8; 512];
        if !disk_read(disk, 0, &mut first) {
            continue;
        }
        let blank = first[..16] == BLANK_MARKER[..];
        let fat_label = if is_boot_sector(&first) {
            // FAT16 keeps the label at 43, FAT32 at 71.
            let at = if u16::from_le_bytes([first[22], first[23]]) != 0 {
                43
            } else {
                71
            };
            first[at..at + 9] == HOME_LABEL[..]
        } else {
            false
        };
        if !blank && !fat_label {
            continue;
        }
        report.present = true;
        if blank {
            if fatfs::Fs::format(disk, 0, disk.sectors(), HOME_LABEL).is_err() {
                continue;
            }
            report.formatted = true;
        }
        let Ok(mut fs) = fatfs::Fs::mount(disk, 0) else {
            continue;
        };
        if let Ok(info) = fs.info() {
            report.fat32 = info.fat32;
            report.clusters = info.clusters;
            report.free_clusters = info.free_clusters;
        }
        install(HOME_MOUNT, disk, b"home", fs);
        // The usual folders exist from the first run.
        for folder in [
            "/home/Documents",
            "/home/Downloads",
            "/home/Notes",
            "/home/Pictures",
            "/home/Music",
            "/home/Trash",
        ] {
            let _ = crate::vfs::create_directory(folder, 0o755);
        }
        report.verified = true;
        break;
    }
    scan_media();
    report
}

fn install(slot: usize, disk: Disk, name: &[u8], fs: Box64) {
    let mut state = STATE.lock();
    let mount = &mut state.mounts[slot];
    mount.used = true;
    mount.disk = disk;
    mount.epoch = EPOCH.fetch_add(1, Ordering::AcqRel);
    let length = name.len().min(NAME_BYTES);
    mount.name[..length].copy_from_slice(&name[..length]);
    mount.name_len = length;
    mount.fs = Some(fs);
}

/// Drops a mount; its open descriptors stop working.
fn unmount(slot: usize) {
    let mut state = STATE.lock();
    let epoch = state.mounts[slot].epoch;
    for handle in state.handles.iter_mut() {
        if handle.used && handle.mount == slot && handle.epoch == epoch {
            handle.used = false;
        }
    }
    state.mounts[slot] = EMPTY_MOUNT;
}

/// Mounts removable FAT disks that appeared and drops those that vanished.
pub fn scan_media() {
    // Vanished disks first.
    for slot in 1..MOUNTS {
        let gone = {
            let state = STATE.lock();
            state.mounts[slot].used && state.mounts[slot].disk.sectors() == 0
        };
        if gone {
            unmount(slot);
        }
    }
    // An ejected disk that was unplugged may be mounted again next time.
    {
        let mut state = STATE.lock();
        for slot in 0..state.ejected.len() {
            if state.ejected[slot].is_some_and(|disk| disk.sectors() == 0) {
                state.ejected[slot] = None;
            }
        }
    }
    for disk in [Disk::Usb, Disk::Sd] {
        if disk.sectors() == 0 || STATE.lock().ejected.contains(&Some(disk)) {
            continue;
        }
        let known = STATE
            .lock()
            .mounts
            .iter()
            .any(|mount| mount.used && mount.disk == disk);
        if known {
            continue;
        }
        let mut first = [0u8; 512];
        if !disk_read(disk, 0, &mut first) {
            continue;
        }
        let Some(start) = volume_start(&first) else {
            continue;
        };
        let Ok(fs) = fatfs::Fs::mount(disk, start) else {
            continue;
        };
        // A stick labelled like the home volume is the home volume.
        let label = fs.label();
        if label[..9] == HOME_LABEL[..] && !STATE.lock().mounts[HOME_MOUNT].used {
            install(HOME_MOUNT, disk, b"home", fs);
            continue;
        }
        let mut name = [0u8; NAME_BYTES];
        let mut length = 0;
        for byte in label.iter().take_while(|byte| **byte != 0) {
            if length < NAME_BYTES - 3
                && (byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'-')
            {
                name[length] = byte.to_ascii_lowercase();
                length += 1;
            }
        }
        if length == 0 {
            let fallback: &[u8] = if disk == Disk::Sd { b"sd" } else { b"usb" };
            name[..fallback.len()].copy_from_slice(fallback);
            length = fallback.len();
        }
        // Make the name unique among the mounts.
        let taken =
            |candidate: &[u8]| {
                STATE.lock().mounts.iter().any(|mount| {
                    mount.used && names_equal(&mount.name[..mount.name_len], candidate)
                })
            };
        let base = length;
        let mut suffix = 2u8;
        while taken(&name[..length]) && suffix < 10 {
            length = base;
            name[length] = b'-';
            name[length + 1] = b'0' + suffix;
            length += 2;
            suffix += 1;
        }
        let free = STATE
            .lock()
            .mounts
            .iter()
            .skip(1)
            .position(|mount| !mount.used);
        if let Some(index) = free {
            install(index + 1, disk, &name[..length], fs);
            crate::serial::format(format_args!(
                "AEROS_MEDIA mounted /media/{}\n",
                core::str::from_utf8(&name[..length]).unwrap_or("?")
            ));
        }
    }
}

/// Called from the main loop: acts on removable-media changes.
pub fn poll() {
    if MEDIA_DIRTY.swap(false, Ordering::AcqRel) {
        scan_media();
    }
}

fn with_fs<T>(
    mount: usize,
    operation: impl FnOnce(&mut fatfs::Fs) -> Result<T, FsError>,
) -> Result<T, VfsError> {
    let mut state = STATE.lock();
    let Some(fs) = state
        .mounts
        .get_mut(mount)
        .and_then(|mount| mount.fs.as_mut())
    else {
        return Err(VfsError::NotFound);
    };
    operation(fs).map_err(map_error)
}

fn allocate(
    state: &mut State,
    mount: usize,
    node: Node,
    directory: bool,
    writable: bool,
) -> Result<u32, VfsError> {
    let slot = state
        .handles
        .iter()
        .position(|handle| !handle.used)
        .ok_or(VfsError::HandleLimit)?;
    let generation = state.handles[slot].generation.max(1);
    let epoch = state.mounts.get(mount).map_or(0, |mount| mount.epoch);
    state.handles[slot] = Handle {
        used: true,
        generation,
        mount,
        epoch,
        node,
        directory,
        media_root: mount == MEDIA_ROOT,
        cursor: 0,
        writable,
    };
    Ok(HANDLE_FLAG | (generation as u32 & 0x7fff) << 16 | slot as u32)
}

fn lookup(state: &mut State, descriptor: u32) -> Result<&mut Handle, VfsError> {
    let slot = descriptor as usize & 0xffff;
    let generation = ((descriptor >> 16) & 0x7fff) as u16;
    let mounts = &state.mounts;
    match state.handles.get_mut(slot) {
        Some(handle)
            if handle.used
                && handle.generation & 0x7fff == generation
                && (handle.media_root
                    || mounts
                        .get(handle.mount)
                        .is_some_and(|mount| mount.used && mount.epoch == handle.epoch)) =>
        {
            Ok(handle)
        }
        _ => Err(VfsError::BadDescriptor),
    }
}

/// Runs a filesystem operation for the mount a handle belongs to.
fn with_handle_fs<T>(
    state: &mut State,
    mount: usize,
    operation: impl FnOnce(&mut fatfs::Fs) -> Result<T, FsError>,
) -> Result<T, VfsError> {
    let Some(fs) = state
        .mounts
        .get_mut(mount)
        .and_then(|mount| mount.fs.as_mut())
    else {
        return Err(VfsError::NotFound);
    };
    operation(fs).map_err(map_error)
}

pub fn open_file(
    mount: usize,
    rest: &str,
    create: bool,
    exclusive: bool,
    truncate: bool,
    write: bool,
) -> Result<u32, VfsError> {
    if mount == MEDIA_ROOT {
        return Err(VfsError::IsDirectory);
    }
    let mut state = STATE.lock();
    let node = with_handle_fs(&mut state, mount, |fs| {
        let path = rest.as_bytes();
        let mut node = match fs.resolve(path) {
            Ok(node) => {
                if create && exclusive {
                    return Err(FsError::Exists);
                }
                node
            }
            Err(FsError::NotFound) if create => {
                let (parent, name) = fs.resolve_parent(path)?;
                fs.create_file(parent.as_dir(), name)?
            }
            Err(error) => return Err(error),
        };
        if node.is_directory() {
            return Err(FsError::IsDirectory);
        }
        if (write || truncate) && node.read_only() {
            return Err(FsError::ReadOnly);
        }
        if truncate && node.size > 0 {
            fs.truncate(&mut node, 0)?;
        }
        Ok(node)
    })?;
    allocate(&mut state, mount, node, false, write || truncate)
}

pub fn open_directory(mount: usize, rest: &str) -> Result<u32, VfsError> {
    let mut state = STATE.lock();
    if mount == MEDIA_ROOT {
        return allocate(&mut state, MEDIA_ROOT, Node::EMPTY, true, false);
    }
    let node = with_handle_fs(&mut state, mount, |fs| fs.resolve(rest.as_bytes()))?;
    if !node.is_directory() {
        return Err(VfsError::NotDirectory);
    }
    allocate(&mut state, mount, node, true, false)
}

pub fn next_directory_entry(descriptor: u32) -> Result<Option<DirectoryEntry>, VfsError> {
    let mut state = STATE.lock();
    let (directory, media_root, mount, node, mut cursor) = {
        let handle = lookup(&mut state, descriptor)?;
        (
            handle.directory,
            handle.media_root,
            handle.mount,
            handle.node,
            handle.cursor as u32,
        )
    };
    if !directory {
        return Err(VfsError::NotDirectory);
    }
    if media_root {
        // Entries are the mounted media, one per call.
        let mut index = (cursor as usize).max(1);
        while index < MOUNTS && !state.mounts[index].used {
            index += 1;
        }
        if index >= MOUNTS {
            lookup(&mut state, descriptor)?.cursor = MOUNTS as u64;
            return Ok(None);
        }
        lookup(&mut state, descriptor)?.cursor = index as u64 + 1;
        let mut name = [0u8; crate::vfs::MAX_NAME];
        let length = state.mounts[index].name_len;
        name[..length].copy_from_slice(&state.mounts[index].name[..length]);
        return Ok(Some(DirectoryEntry {
            inode: 0x7000 + index as u64,
            kind: 4,
            name,
            name_len: length as u8,
        }));
    }
    let listed = with_handle_fs(&mut state, mount, |fs| {
        fs.list_next(node.as_dir(), &mut cursor)
    })?;
    lookup(&mut state, descriptor)?.cursor = cursor as u64;
    let Some(entry) = listed else {
        return Ok(None);
    };
    let mut name = [0u8; crate::vfs::MAX_NAME];
    let length = entry.name_len.min(name.len());
    name[..length].copy_from_slice(&entry.name()[..length]);
    Ok(Some(DirectoryEntry {
        inode: entry.node.first_cluster as u64 | (entry.node.index as u64) << 32,
        kind: if entry.node.is_directory() { 4 } else { 8 },
        name,
        name_len: length as u8,
    }))
}

pub fn read_at(descriptor: u32, offset: usize, out: &mut [u8]) -> Result<usize, VfsError> {
    let mut state = STATE.lock();
    let (mut node, mount) = {
        let handle = lookup(&mut state, descriptor)?;
        if handle.directory {
            return Err(VfsError::IsDirectory);
        }
        (handle.node, handle.mount)
    };
    let read = with_handle_fs(&mut state, mount, |fs| {
        fs.read_at(&mut node, offset as u64, out)
    })?;
    lookup(&mut state, descriptor)?.node = node;
    Ok(read)
}

pub fn read(descriptor: u32, out: &mut [u8]) -> Result<usize, VfsError> {
    let cursor = {
        let mut state = STATE.lock();
        lookup(&mut state, descriptor)?.cursor as usize
    };
    let read = read_at(descriptor, cursor, out)?;
    let mut state = STATE.lock();
    lookup(&mut state, descriptor)?.cursor = (cursor + read) as u64;
    Ok(read)
}

pub fn write_at(descriptor: u32, offset: usize, source: &[u8]) -> Result<usize, VfsError> {
    let mut state = STATE.lock();
    let (mut node, mount) = {
        let handle = lookup(&mut state, descriptor)?;
        if handle.directory {
            return Err(VfsError::IsDirectory);
        }
        if !handle.writable {
            return Err(VfsError::PermissionDenied);
        }
        (handle.node, handle.mount)
    };
    let written = with_handle_fs(&mut state, mount, |fs| {
        fs.write_at(&mut node, offset as u64, source)
    })?;
    lookup(&mut state, descriptor)?.node = node;
    Ok(written)
}

pub fn write(descriptor: u32, source: &[u8], append: bool) -> Result<usize, VfsError> {
    let cursor = {
        let mut state = STATE.lock();
        let handle = lookup(&mut state, descriptor)?;
        if append {
            handle.node.size as usize
        } else {
            handle.cursor as usize
        }
    };
    let written = write_at(descriptor, cursor, source)?;
    let mut state = STATE.lock();
    lookup(&mut state, descriptor)?.cursor = (cursor + written) as u64;
    Ok(written)
}

pub fn seek(descriptor: u32, offset: i64, whence: u64) -> Result<usize, VfsError> {
    let mut state = STATE.lock();
    let handle = lookup(&mut state, descriptor)?;
    let base = match whence {
        0 => 0i128,
        1 => handle.cursor as i128,
        2 => handle.node.size as i128,
        _ => return Err(VfsError::InvalidPath),
    };
    let target = base + offset as i128;
    if target < 0 || target > u32::MAX as i128 {
        return Err(VfsError::OffsetOverflow);
    }
    handle.cursor = target as u64;
    Ok(target as usize)
}

pub fn truncate(descriptor: u32, length: usize) -> Result<(), VfsError> {
    let mut state = STATE.lock();
    let (mut node, mount) = {
        let handle = lookup(&mut state, descriptor)?;
        if !handle.writable {
            return Err(VfsError::PermissionDenied);
        }
        (handle.node, handle.mount)
    };
    with_handle_fs(&mut state, mount, |fs| {
        fs.truncate(&mut node, length as u64)
    })?;
    lookup(&mut state, descriptor)?.node = node;
    Ok(())
}

pub fn close(descriptor: u32) -> Result<(), VfsError> {
    let mut state = STATE.lock();
    let slot = descriptor as usize & 0xffff;
    lookup(&mut state, descriptor)?;
    let handle = &mut state.handles[slot];
    handle.used = false;
    handle.generation = (handle.generation.wrapping_add(1) & 0x7fff).max(1);
    Ok(())
}

fn node_metadata(node: &Node) -> Metadata {
    let kind = if node.is_directory() {
        0o040000
    } else {
        0o100000
    };
    let permissions = match (node.is_directory(), node.read_only()) {
        (true, false) => 0o755,
        (true, true) => 0o555,
        (false, false) => 0o644,
        (false, true) => 0o444,
    };
    Metadata {
        inode: node.first_cluster as u64 | (node.index as u64) << 32 | 1 << 62,
        mode: kind | permissions,
        size: node.size as u64,
        modified: fatfs::fat_to_unix(node.date, node.time),
    }
}

pub fn metadata(mount: usize, rest: &str) -> Result<Metadata, VfsError> {
    if mount == MEDIA_ROOT {
        return Ok(Metadata {
            inode: 0x7000,
            mode: 0o040755,
            size: 0,
            modified: 0,
        });
    }
    with_fs(mount, |fs| fs.resolve(rest.as_bytes())).map(|node| node_metadata(&node))
}

pub fn descriptor_metadata(descriptor: u32) -> Result<Metadata, VfsError> {
    let mut state = STATE.lock();
    let handle = lookup(&mut state, descriptor)?;
    Ok(node_metadata(&handle.node))
}

pub fn create_directory(mount: usize, rest: &str) -> Result<(), VfsError> {
    if mount == MEDIA_ROOT {
        return Err(VfsError::PermissionDenied);
    }
    with_fs(mount, |fs| {
        let (parent, name) = fs.resolve_parent(rest.as_bytes())?;
        fs.create_dir(parent.as_dir(), name).map(|_| ())
    })
}

pub fn remove(mount: usize, rest: &str, directory: bool) -> Result<(), VfsError> {
    if mount == MEDIA_ROOT {
        return Err(VfsError::PermissionDenied);
    }
    with_fs(mount, |fs| {
        let node = fs.resolve(rest.as_bytes())?;
        if node.is_directory() != directory {
            return Err(if directory {
                FsError::NotDirectory
            } else {
                FsError::IsDirectory
            });
        }
        if node.dir == 0 && node.index == 0 && node.first_index == 0 && node.first_cluster == 0 {
            return Err(FsError::InvalidName); // the root itself
        }
        fs.remove(&node)
    })
}

/// Renames within one mount.
pub fn rename(
    mount: usize,
    source: &str,
    destination: &str,
    replace: bool,
) -> Result<(), VfsError> {
    if mount == MEDIA_ROOT {
        return Err(VfsError::PermissionDenied);
    }
    with_fs(mount, |fs| {
        let node = fs.resolve(source.as_bytes())?;
        let (parent, name) = fs.resolve_parent(destination.as_bytes())?;
        fs.rename(&node, parent.as_dir(), name, replace).map(|_| ())
    })
}

pub fn chmod(mount: usize, rest: &str, mode: u16) -> Result<(), VfsError> {
    if mount == MEDIA_ROOT {
        return Err(VfsError::PermissionDenied);
    }
    with_fs(mount, |fs| {
        let mut node = fs.resolve(rest.as_bytes())?;
        fs.set_read_only(&mut node, mode & 0o222 == 0)
    })
}

pub fn fchmod(descriptor: u32, mode: u16) -> Result<(), VfsError> {
    let mut state = STATE.lock();
    let (mut node, mount) = {
        let handle = lookup(&mut state, descriptor)?;
        (handle.node, handle.mount)
    };
    with_handle_fs(&mut state, mount, |fs| {
        fs.set_read_only(&mut node, mode & 0o222 == 0)
    })?;
    lookup(&mut state, descriptor)?.node = node;
    Ok(())
}

/// One mounted persistent volume, for `df`/`mount` and the Files/Settings apps.
#[derive(Clone, Copy)]
pub struct MountInfo {
    pub path: [u8; 32],
    pub path_len: usize,
    pub device: &'static str,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub fat32: bool,
}

impl MountInfo {
    pub fn path(&self) -> &str {
        core::str::from_utf8(&self.path[..self.path_len]).unwrap_or("?")
    }
}

/// Human name of the device a disk is, in the usual Linux style.
pub fn device_name(disk: Disk) -> &'static str {
    match disk {
        Disk::Ahci(0) => "sda",
        Disk::Ahci(1) => "sdb",
        Disk::Ahci(_) => "sdc",
        Disk::Nvme => "nvme0n1",
        Disk::Usb => "sdu",
        Disk::Sd => "mmcblk0",
        Disk::Virtio => "vda",
    }
}

/// Details of the `index`-th mount slot (0 = /home, then media), if used.
pub fn mount_info(index: usize) -> Option<MountInfo> {
    let mut state = STATE.lock();
    let mount = state.mounts.get_mut(index)?;
    if !mount.used {
        return None;
    }
    let mut path = [0u8; 32];
    let prefix: &[u8] = if index == HOME_MOUNT {
        b"/home"
    } else {
        b"/media/"
    };
    path[..prefix.len()].copy_from_slice(prefix);
    let name: &[u8] = if index == HOME_MOUNT {
        b""
    } else {
        &mount.name[..mount.name_len]
    };
    path[prefix.len()..prefix.len() + name.len()].copy_from_slice(name);
    let length = prefix.len() + name.len();
    let device = device_name(mount.disk);
    let info = mount.fs.as_mut()?.info().ok()?;
    let cluster = info.bytes_per_cluster as u64;
    Some(MountInfo {
        path,
        path_len: length,
        device,
        total_bytes: info.clusters as u64 * cluster,
        free_bytes: info.free_clusters as u64 * cluster,
        fat32: info.fat32,
    })
}

/// Safely removes a `/media/<name>` volume (nothing may be open on it). The
/// disk stays unmounted until it is unplugged.
pub fn eject(name: &str) -> Result<(), VfsError> {
    let mut state = STATE.lock();
    let Some(slot) = state
        .mounts
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, mount)| {
            mount.used && names_equal(&mount.name[..mount.name_len], name.as_bytes())
        })
        .map(|(index, _)| index)
    else {
        return Err(VfsError::NotFound);
    };
    let epoch = state.mounts[slot].epoch;
    if state
        .handles
        .iter()
        .any(|handle| handle.used && handle.mount == slot && handle.epoch == epoch)
    {
        return Err(VfsError::Busy);
    }
    let disk = state.mounts[slot].disk;
    state.mounts[slot] = EMPTY_MOUNT;
    if let Some(entry) = state.ejected.iter_mut().find(|entry| entry.is_none()) {
        *entry = Some(disk);
    }
    Ok(())
}

/// (total, free) bytes of a mounted volume, for the Files app / storage settings.
#[allow(dead_code)]
pub fn space(mount: usize) -> Option<(u64, u64)> {
    let mut state = STATE.lock();
    let fs = state.mounts.get_mut(mount)?.fs.as_mut()?;
    let info = fs.info().ok()?;
    let cluster = info.bytes_per_cluster as u64;
    Some((
        info.clusters as u64 * cluster,
        info.free_clusters as u64 * cluster,
    ))
}

#[cfg(feature = "boot-test")]
/// Result of the VFS-level test of the home mount.
pub struct HomeTest {
    pub tree: bool,
    pub file_io: bool,
    pub listing: bool,
    pub rename_paths: bool,
    pub cross_mount: bool,
    pub read_only: bool,
    pub remount: bool,
    pub verified: bool,
}

#[cfg(feature = "boot-test")]
fn pattern_byte(index: usize) -> u8 {
    (index as u32)
        .wrapping_mul(40503)
        .wrapping_add(17)
        .to_le_bytes()[1]
}

#[cfg(feature = "boot-test")]
/// Drives the mount only through the public `vfs` API, the way programs do:
/// nested folders, a multi-cluster file with a long name, listing, renames
/// (within the mount and across to the RAM tree), read-only files, delete,
/// then unmount + mount again from the disk and read the data back.
pub fn self_test() -> HomeTest {
    use crate::vfs;
    let mut report = HomeTest {
        tree: false,
        file_io: false,
        listing: false,
        rename_paths: false,
        cross_mount: false,
        read_only: false,
        remount: false,
        verified: false,
    };
    let folder = "/home/Documents/Test Folder";
    report.tree = vfs::create_directory(folder, 0o755).is_ok()
        && vfs::create_directory(folder, 0o755) == Err(VfsError::Exists)
        && vfs::metadata(folder).is_ok_and(|m| m.mode & 0o170000 == 0o040000);

    let file = "/home/Documents/Test Folder/hello, world & friends.txt";
    let length = 90_000usize;
    let mut written = false;
    if let Ok(descriptor) = vfs::open_file(file, true, true, false, 0o644, true) {
        let mut chunk = [0u8; 700];
        let mut done = 0;
        written = true;
        while done < length {
            let count = (length - done).min(chunk.len());
            for (offset, byte) in chunk[..count].iter_mut().enumerate() {
                *byte = pattern_byte(done + offset);
            }
            written &= vfs::write(descriptor, &chunk[..count], false) == Ok(count);
            done += count;
        }
        written &= vfs::close(descriptor).is_ok();
    }
    let mut read_back = false;
    if let Ok(descriptor) = vfs::open_file(file, false, false, false, 0, false) {
        let mut chunk = [0u8; 999];
        let mut done = 0;
        read_back = true;
        while let Ok(count) = vfs::read(descriptor, &mut chunk) {
            if count == 0 {
                break;
            }
            read_back &= chunk[..count]
                .iter()
                .enumerate()
                .all(|(offset, byte)| *byte == pattern_byte(done + offset));
            done += count;
        }
        read_back &= done == length;
        // Seek + positioned read.
        let mut probe = [0u8; 8];
        read_back &= vfs::seek(descriptor, 12_345, 0) == Ok(12_345)
            && vfs::read(descriptor, &mut probe) == Ok(8)
            && probe
                .iter()
                .enumerate()
                .all(|(i, b)| *b == pattern_byte(12_345 + i));
        let _ = vfs::close(descriptor);
    }
    report.file_io = written
        && read_back
        && vfs::metadata(file).is_ok_and(|m| {
            m.size == length as u64 && m.mode & 0o170000 == 0o100000 && m.modified > 1_700_000_000
        });

    // Directory listing shows the long name intact.
    let mut found = false;
    if let Ok(descriptor) = vfs::open_directory(folder) {
        while let Ok(Some(entry)) = vfs::next_directory_entry(descriptor) {
            if &entry.name[..entry.name_len as usize] == b"hello, world & friends.txt" {
                found = entry.kind == 8;
            }
        }
        let _ = vfs::close(descriptor);
    }
    report.listing = found;

    // Rename within the mount (to another folder), then across to /tmp and back.
    let moved = "/home/Notes/Moved Note.txt";
    report.rename_paths = vfs::rename(file, moved).is_ok()
        && matches!(vfs::metadata(file), Err(VfsError::NotFound))
        && vfs::metadata(moved).is_ok_and(|m| m.size == length as u64);
    // Across mounts: a small file (the RAM tree holds 4 KiB files) goes to
    // /tmp and back.
    let small = "/home/Notes/Small Note.txt";
    let scratch = "/tmp/from-home.txt";
    let mut cross = false;
    if let Ok(descriptor) = vfs::open_file(small, true, true, false, 0o644, true) {
        cross = vfs::write(descriptor, &[0x5a; 3000], false) == Ok(3000);
        cross &= vfs::close(descriptor).is_ok();
    }
    report.cross_mount = cross
        && vfs::rename(small, scratch).is_ok()
        && vfs::metadata(scratch).is_ok_and(|m| m.size == 3000)
        && matches!(vfs::metadata(small), Err(VfsError::NotFound))
        && vfs::rename(scratch, "/home/Notes/Back Home.txt").is_ok()
        && vfs::metadata("/home/Notes/Back Home.txt").is_ok_and(|m| m.size == 3000)
        && vfs::remove("/home/Notes/Back Home.txt", false).is_ok();

    // Read-only files refuse writes.
    let target = moved;
    let locked = vfs::chmod(target, 0o444).is_ok()
        && vfs::open_file(target, false, false, false, 0, true) == Err(VfsError::PermissionDenied)
        && vfs::chmod(target, 0o644).is_ok()
        && vfs::open_file(target, false, false, false, 0, true)
            .is_ok_and(|d| vfs::close(d).is_ok());
    report.read_only = locked;

    // Persistence: unmount, mount again from the disk, read it all back.
    unmount(HOME_MOUNT);
    let again = initialize();
    let mut survived = again.verified && !again.formatted;
    if survived {
        survived &= vfs::metadata(target).is_ok_and(|m| m.size == length as u64);
        if let Ok(descriptor) = vfs::open_file(target, false, false, false, 0, false) {
            let mut chunk = [0u8; 1000];
            let mut done = 0;
            while let Ok(count) = vfs::read(descriptor, &mut chunk) {
                if count == 0 {
                    break;
                }
                survived &= chunk[..count]
                    .iter()
                    .enumerate()
                    .all(|(offset, byte)| *byte == pattern_byte(done + offset));
                done += count;
            }
            survived &= done == length;
            let _ = vfs::close(descriptor);
        } else {
            survived = false;
        }
    }
    report.remount = survived;

    // Clean up.
    let cleaned = vfs::remove(target, false).is_ok()
        && vfs::remove(folder, true).is_ok()
        && matches!(
            vfs::open_directory("/home/Documents/Test Folder"),
            Err(VfsError::NotFound)
        );
    report.verified = report.tree
        && report.file_io
        && report.listing
        && report.rename_paths
        && report.cross_mount
        && report.read_only
        && report.remount
        && cleaned;
    report
}

#[cfg(feature = "boot-test")]
pub struct MediaTest {
    pub mounted: bool,
    pub listed: bool,
    pub data: bool,
    pub write: bool,
    pub verified: bool,
}

/// The USB disk left behind by the filesystem self-test (an MBR + FAT16
/// volume labelled AEROSTEST holding a 70000-byte pattern file) must appear
/// under /media, be readable and writable through the normal VFS calls.
#[cfg(feature = "boot-test")]
pub fn media_self_test() -> MediaTest {
    use crate::vfs;
    let mut report = MediaTest {
        mounted: false,
        listed: false,
        data: false,
        write: false,
        verified: false,
    };
    scan_media();
    report.mounted = route("/media/aerostest").is_some();
    if let Ok(descriptor) = vfs::open_directory("/media") {
        while let Ok(Some(entry)) = vfs::next_directory_entry(descriptor) {
            if &entry.name[..entry.name_len as usize] == b"aerostest" {
                report.listed = entry.kind == 4;
            }
        }
        let _ = vfs::close(descriptor);
    }
    let file = "/media/AEROSTEST/Persisted Folder/remember me please.dat";
    if let Ok(descriptor) = vfs::open_file(file, false, false, false, 0, false) {
        let mut chunk = [0u8; 1000];
        let mut done = 0usize;
        let mut good = true;
        while let Ok(count) = vfs::read(descriptor, &mut chunk) {
            if count == 0 {
                break;
            }
            good &= chunk[..count]
                .iter()
                .enumerate()
                .all(|(offset, byte)| *byte == crate::fatfs::pattern(5, done + offset));
            done += count;
        }
        report.data = good && done == 70_000;
        let _ = vfs::close(descriptor);
    }
    let created = "/media/aerostest/Written From AerOS.txt";
    if let Ok(descriptor) = vfs::open_file(created, true, true, false, 0o644, true) {
        let wrote = vfs::write(descriptor, &[0x41; 5000], false) == Ok(5000);
        let _ = vfs::close(descriptor);
        report.write = wrote
            && vfs::metadata(created).is_ok_and(|m| m.size == 5000)
            && vfs::remove(created, false).is_ok();
    }
    report.verified = report.mounted && report.listed && report.data && report.write;
    report
}
