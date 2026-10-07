//! Boot-test checks for AerFS inside the kernel: power cut at (nearly) every
//! sector write of a scripted sequence on the RAM disk, then the whole VFS
//! path through `/media` with a remount.

use crate::blockdev;
use crate::fatfs::{Disk, FsError};
use crate::vfs::VfsError;
use crate::volume::Volume;
use crate::{datafs, vfs};

const SECTORS: u64 = blockdev::RAM_DISK_SECTORS;

fn pattern(seed: u8, index: usize) -> u8 {
    ((index % 251) as u8).wrapping_mul(3) ^ seed
}

#[derive(Clone, Copy)]
enum Step {
    Mkdir(&'static str),
    Create(&'static str),
    Write(&'static str, usize, usize, u8),
    Truncate(&'static str, usize),
    Remove(&'static str),
    Rename(&'static str, &'static str),
}

/// Every step is one AerFS transaction.
const SCRIPT: [Step; 15] = [
    Step::Mkdir("/docs"),
    Step::Create("/docs/a"),
    Step::Write("/docs/a", 0, 9000, 1),
    Step::Create("/docs/b"),
    Step::Write("/docs/b", 0, 60, 2),
    Step::Write("/docs/a", 4090, 300, 3),
    Step::Truncate("/docs/a", 5000),
    Step::Rename("/docs/b", "/docs/c"),
    Step::Create("/docs/d"),
    Step::Write("/docs/d", 0, 3000, 4),
    Step::Rename("/docs/d", "/docs/a"),
    Step::Write("/docs/a", 2_200_000, 700, 5),
    Step::Truncate("/docs/a", 100),
    Step::Remove("/docs/c"),
    Step::Remove("/docs/a"),
];

fn apply(volume: &mut Volume, step: &Step) -> Result<(), FsError> {
    match *step {
        Step::Mkdir(path) => {
            let (dir, name) = volume.resolve_parent(path.as_bytes())?;
            volume.create_dir(dir.as_dir(), name).map(|_| ())
        }
        Step::Create(path) => {
            let (dir, name) = volume.resolve_parent(path.as_bytes())?;
            volume.create_file(dir.as_dir(), name).map(|_| ())
        }
        Step::Write(path, offset, length, seed) => {
            let mut node = volume.resolve(path.as_bytes())?;
            let mut data = [0u8; 9000];
            for (index, byte) in data[..length].iter_mut().enumerate() {
                *byte = pattern(seed, index);
            }
            volume
                .write_at(&mut node, offset as u64, &data[..length])
                .map(|_| ())
        }
        Step::Truncate(path, length) => {
            let mut node = volume.resolve(path.as_bytes())?;
            volume.truncate(&mut node, length as u64)
        }
        Step::Remove(path) => {
            let node = volume.resolve(path.as_bytes())?;
            volume.remove(&node)
        }
        Step::Rename(from, to) => {
            let node = volume.resolve(from.as_bytes())?;
            let (dir, name) = volume.resolve_parent(to.as_bytes())?;
            volume.rename(&node, dir.as_dir(), name, true).map(|_| ())
        }
    }
}

fn fnv(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// Adds one hash per entry (path, kind and content) into `total`.
fn walk(
    volume: &mut Volume,
    dir: u32,
    path: &mut [u8; 96],
    length: usize,
    total: &mut u64,
) -> bool {
    let mut cursor = 0;
    loop {
        let entry = match volume.list_next(dir, &mut cursor) {
            Ok(Some(entry)) => entry,
            Ok(None) => return true,
            Err(_) => return false,
        };
        let name = entry.name();
        if length + 1 + name.len() > path.len() {
            return false;
        }
        path[length] = b'/';
        path[length + 1..length + 1 + name.len()].copy_from_slice(name);
        let here = length + 1 + name.len();
        let mut hash = fnv(0xcbf2_9ce4_8422_2325, &path[..here]);
        hash = fnv(hash, &[u8::from(entry.node.is_directory())]);
        if entry.node.is_directory() {
            *total = total.wrapping_add(hash);
            if !walk(volume, entry.node.as_dir(), path, here, total) {
                return false;
            }
            continue;
        }
        let mut node = entry.node;
        let mut chunk = [0u8; 1024];
        let mut position = 0u64;
        loop {
            match volume.read_at(&mut node, position, &mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    hash = fnv(hash, &chunk[..count]);
                    position += count as u64;
                }
                Err(_) => return false,
            }
        }
        hash = fnv(hash, &position.to_le_bytes());
        *total = total.wrapping_add(hash);
    }
}

fn fingerprint(volume: &mut Volume) -> Option<u64> {
    let mut path = [0u8; 96];
    let mut total = 0u64;
    walk(volume, 1, &mut path, 0, &mut total).then_some(total)
}

fn format() -> Option<Volume> {
    Volume::format_aerfs(Disk::Ram, 0, SECTORS, b"CRASH").ok()
}

pub struct CrashReport {
    pub cases: u32,
    pub mount_failures: u32,
    pub fsck_failures: u32,
    pub torn: u32,
    pub verified: bool,
}

/// Cuts the power after N sector writes for N across the whole script,
/// remounts, checks the filesystem and compares its contents with the state
/// before or after the step that was running.
pub fn crash() -> CrashReport {
    let mut report = CrashReport {
        cases: 0,
        mount_failures: 0,
        fsck_failures: 0,
        torn: 0,
        verified: false,
    };
    let Some(mut volume) = format() else {
        return report;
    };
    let mut states = [0u64; SCRIPT.len() + 1];
    let mut writes = [0u64; SCRIPT.len() + 1];
    let Some(first) = fingerprint(&mut volume) else {
        return report;
    };
    states[0] = first;
    writes[0] = blockdev::ram_writes();
    for (index, step) in SCRIPT.iter().enumerate() {
        if apply(&mut volume, step).is_err() {
            return report;
        }
        let Some(state) = fingerprint(&mut volume) else {
            return report;
        };
        states[index + 1] = state;
        writes[index + 1] = blockdev::ram_writes();
    }
    drop(volume);
    let base = writes[0];
    let total = writes[SCRIPT.len()];
    let stride = (total - base) / 300 + 1;
    let mut cut = base;
    let mut inside = 0u32;
    while cut < total {
        let Some(mut volume) = format() else {
            report.mount_failures += 1;
            break;
        };
        blockdev::inject_power_loss_after((cut - base) as u32);
        let mut in_flight = SCRIPT.len();
        for (index, step) in SCRIPT.iter().enumerate() {
            let _ = apply(&mut volume, step);
            if blockdev::power_is_out() {
                in_flight = index;
                break;
            }
        }
        let dropped = blockdev::clear_power_loss();
        inside += u32::from(dropped > 0);
        drop(volume);
        match Volume::mount_aerfs(Disk::Ram, 0, SECTORS) {
            Err(_) => report.mount_failures += 1,
            Ok(mut after) => {
                if after.fsck(false).is_ok_and(|found| found.damaged()) {
                    report.fsck_failures += 1;
                }
                let found = fingerprint(&mut after);
                let before = states[in_flight];
                let done = states[(in_flight + 1).min(SCRIPT.len())];
                if found != Some(before) && found != Some(done) {
                    report.torn += 1;
                }
            }
        }
        report.cases += 1;
        cut += stride;
    }
    report.verified = report.cases >= 60
        && inside >= 40
        && report.mount_failures == 0
        && report.fsck_failures == 0
        && report.torn == 0;
    report
}

pub struct VfsReport {
    pub written: bool,
    pub read_back: bool,
    pub listing: bool,
    pub renamed: bool,
    pub truncated: bool,
    pub fsck_clean: bool,
    pub persists: bool,
    pub verified: bool,
}

fn read_and_check(path: &str, seed: u8, expected: usize) -> bool {
    let Ok(handle) = vfs::open_file(path, false, false, false, 0, false) else {
        return false;
    };
    let mut chunk = [0u8; 700];
    let mut done = 0usize;
    let mut good = true;
    while let Ok(count) = vfs::read(handle, &mut chunk) {
        if count == 0 {
            break;
        }
        good &= chunk[..count]
            .iter()
            .enumerate()
            .all(|(offset, byte)| *byte == pattern(seed, done + offset));
        done += count;
    }
    let _ = vfs::close(handle);
    good && done == expected
}

struct Path {
    bytes: [u8; 96],
    length: usize,
}

impl Path {
    fn new(mount: &str, tail: &str) -> Self {
        let mut path = Self {
            bytes: [0; 96],
            length: 0,
        };
        for part in ["/media/", mount, tail] {
            path.bytes[path.length..path.length + part.len()].copy_from_slice(part.as_bytes());
            path.length += part.len();
        }
        path
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("")
    }
}

/// The whole stack: VFS, `/media` routing, the volume layer and AerFS.
pub fn through_vfs() -> VfsReport {
    let mut report = VfsReport {
        written: false,
        read_back: false,
        listing: false,
        renamed: false,
        truncated: false,
        fsck_clean: false,
        persists: false,
        verified: false,
    };
    let Ok((name, length)) = datafs::format_aerfs(Disk::Ram, b"AERTEST") else {
        return report;
    };
    let mount = core::str::from_utf8(&name[..length]).unwrap_or("");
    let directory = Path::new(mount, "/docs");
    let target = Path::new(mount, "/docs/a.bin");
    let moved = Path::new(mount, "/docs/b.bin");

    let mut written = vfs::create_directory(directory.as_str(), 0o755).is_ok();
    match vfs::open_file(target.as_str(), true, true, false, 0o644, true) {
        Ok(handle) => {
            let mut chunk = [0u8; 700];
            let mut done = 0;
            while done < 20_000 {
                let count = (20_000 - done).min(700);
                for (offset, byte) in chunk[..count].iter_mut().enumerate() {
                    *byte = pattern(7, done + offset);
                }
                written &= vfs::write(handle, &chunk[..count], false) == Ok(count);
                done += count;
            }
            written &= vfs::close(handle).is_ok();
        }
        Err(_) => written = false,
    }
    report.written =
        written && vfs::metadata(target.as_str()).is_ok_and(|metadata| metadata.size == 20_000);
    report.read_back = read_and_check(target.as_str(), 7, 20_000);

    let mut listed = false;
    if let Ok(handle) = vfs::open_directory(directory.as_str()) {
        while let Ok(Some(entry)) = vfs::next_directory_entry(handle) {
            listed |= &entry.name[..entry.name_len as usize] == b"a.bin";
        }
        let _ = vfs::close(handle);
    }
    report.listing = listed;

    report.renamed = vfs::rename(target.as_str(), moved.as_str()).is_ok()
        && matches!(vfs::metadata(target.as_str()), Err(VfsError::NotFound))
        && read_and_check(moved.as_str(), 7, 20_000);

    if let Ok(handle) = vfs::open_file(moved.as_str(), false, false, false, 0, true) {
        let cut = vfs::truncate(handle, 5000).is_ok();
        let _ = vfs::close(handle);
        report.truncated = cut
            && vfs::metadata(moved.as_str()).is_ok_and(|metadata| metadata.size == 5000)
            && read_and_check(moved.as_str(), 7, 5000);
    }

    let slot = datafs::route(directory.as_str()).map(|(slot, _)| slot);
    report.fsck_clean = slot.is_some_and(|slot| {
        datafs::fsck(slot, false).is_ok_and(|found| !found.damaged() && found.files >= 1)
    });

    // Unmount and mount again: the contents come back from the disk.
    if let Some(slot) = slot {
        datafs::unmount(slot);
    }
    report.persists =
        datafs::mount_aerfs(Disk::Ram).is_ok() && read_and_check(moved.as_str(), 7, 5000);

    let _ = vfs::remove(moved.as_str(), false);
    let _ = vfs::remove(directory.as_str(), true);
    if let Some((slot, _)) = datafs::route(directory.as_str()) {
        datafs::unmount(slot);
    }
    report.verified = report.written
        && report.read_back
        && report.listing
        && report.renamed
        && report.truncated
        && report.fsck_clean
        && report.persists;
    report
}

pub struct HomeReport {
    pub adopted: bool,
    pub folders: bool,
    pub system_directories: bool,
    pub written: bool,
    pub fsck_clean: bool,
    pub persists: bool,
    pub restored: bool,
    pub verified: bool,
}

fn read_text(path: &str, expected: &[u8]) -> bool {
    let Ok(handle) = vfs::open_file(path, false, false, false, 0, false) else {
        return false;
    };
    let mut buffer = [0u8; 128];
    let count = vfs::read(handle, &mut buffer).unwrap_or(0);
    let _ = vfs::close(handle);
    &buffer[..count] == expected
}

/// An AerFS volume labelled `AEROSHOME` becomes `/home` when none is mounted:
/// the usual folders and the persistent system directories appear on it, data
/// survives a remount, and the FAT home comes back afterwards.
pub fn as_home() -> HomeReport {
    let mut report = HomeReport {
        adopted: false,
        folders: false,
        system_directories: false,
        written: false,
        fsck_clean: false,
        persists: false,
        restored: false,
        verified: false,
    };
    let is_directory =
        |path: &str| vfs::metadata(path).is_ok_and(|metadata| metadata.mode & 0o170000 == 0o040000);
    datafs::unmount(0);
    let attached = datafs::format_aerfs(Disk::Ram, b"AEROSHOME");
    report.adopted = matches!(attached, Ok((name, 4)) if &name[..4] == b"home")
        && datafs::mount_info(0).is_some_and(|mount| mount.kind == "aerfs");
    report.folders = is_directory("/home/Documents") && is_directory("/home/Trash");
    report.system_directories = is_directory("/home/.root/etc")
        && is_directory("/home/.root/var")
        && read_text("/etc/hostname", b"aeros\n")
        && read_text(
            "/home/.root/etc/hosts",
            b"127.0.0.1 localhost\n::1 localhost\n",
        );

    let target = "/var/aerhome.bin";
    let mut written = false;
    if let Ok(handle) = vfs::open_file(target, true, true, false, 0o644, true) {
        let mut chunk = [0u8; 700];
        let mut done = 0;
        written = true;
        while done < 9000 {
            let count = (9000 - done).min(700);
            for (offset, byte) in chunk[..count].iter_mut().enumerate() {
                *byte = pattern(9, done + offset);
            }
            written &= vfs::write(handle, &chunk[..count], false) == Ok(count);
            done += count;
        }
        written &= vfs::close(handle).is_ok();
    }
    report.written = written && read_and_check(target, 9, 9000);
    report.fsck_clean =
        datafs::fsck(0, false).is_ok_and(|found| !found.damaged() && found.files >= 1);

    datafs::unmount(0);
    report.persists = datafs::mount_aerfs(Disk::Ram).is_ok()
        && datafs::mount_info(0).is_some_and(|mount| mount.kind == "aerfs")
        && read_and_check(target, 9, 9000)
        && read_text("/etc/hostname", b"aeros\n");
    let _ = vfs::remove(target, false);

    datafs::unmount(0);
    let again = datafs::initialize();
    report.restored = again.verified
        && datafs::mount_info(0).is_some_and(|mount| mount.kind == "vfat")
        && read_text("/etc/hostname", b"aeros\n");
    report.verified = report.adopted
        && report.folders
        && report.system_directories
        && report.written
        && report.fsck_clean
        && report.persists
        && report.restored;
    report
}
