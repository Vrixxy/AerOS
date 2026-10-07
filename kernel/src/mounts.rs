//! The mount table. The root and `/tmp` are in-memory trees, `/proc` and
//! `/sys` are generated, `/home` and `/media/<label>` are FAT volumes, and
//! this module adds bind mounts: a directory of the RAM tree can be replaced
//! by a directory of a FAT volume. The VFS rewrites a path under a bind to the
//! volume path before it does anything else, so every call that works on
//! `/home` works on the bound directory too. At boot `/var`, `/opt`, `/srv`
//! and `/root` are bound to `/home/.root/...`, which makes them persistent.

use core::fmt::Write;

use crate::sync::TicketLock;
use crate::{datafs, vfs};

const MAX_BINDS: usize = 8;
const PATH: usize = 96;
const MAPPED: usize = 160;
const ROOT_DIRECTORY: &str = "/home/.root";
const PERSISTENT: [&str; 4] = ["var", "opt", "srv", "root"];

#[derive(Clone, Copy)]
struct Bind {
    used: bool,
    target: [u8; PATH],
    target_length: usize,
    source: [u8; PATH],
    source_length: usize,
}

impl Bind {
    const EMPTY: Bind = Bind {
        used: false,
        target: [0; PATH],
        target_length: 0,
        source: [0; PATH],
        source_length: 0,
    };

    fn target(&self) -> &str {
        core::str::from_utf8(&self.target[..self.target_length]).unwrap_or("")
    }

    fn source(&self) -> &str {
        core::str::from_utf8(&self.source[..self.source_length]).unwrap_or("")
    }
}

static BINDS: TicketLock<[Bind; MAX_BINDS]> = TicketLock::new([Bind::EMPTY; MAX_BINDS]);

pub struct Mapped {
    bytes: [u8; MAPPED],
    length: usize,
}

impl Mapped {
    pub fn of(path: &str) -> Self {
        let mut mapped = Mapped {
            bytes: [0; MAPPED],
            length: 0,
        };
        let count = path.len().min(MAPPED);
        mapped.bytes[..count].copy_from_slice(&path.as_bytes()[..count]);
        mapped.length = count;
        mapped
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("/")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MountError {
    InvalidPath,
    NotDirectory,
    NotAllowed,
    Busy,
    NotMounted,
    Full,
}

fn below(path: &str, prefix: &str) -> Option<usize> {
    let rest = path.strip_prefix(prefix)?;
    (rest.is_empty() || rest.starts_with('/')).then_some(prefix.len())
}

/// The path on the volume a bound path stands for, or `None` when the path is
/// not under any bind.
pub fn resolve(path: &str) -> Option<Mapped> {
    let binds = BINDS.lock();
    for bind in binds.iter().filter(|bind| bind.used) {
        if let Some(at) = below(path, bind.target()) {
            let rest = &path[at..];
            let mut mapped = Mapped {
                bytes: [0; MAPPED],
                length: 0,
            };
            for part in [bind.source(), rest] {
                let count = part.len().min(MAPPED - mapped.length);
                mapped.bytes[mapped.length..mapped.length + count]
                    .copy_from_slice(&part.as_bytes()[..count]);
                mapped.length += count;
            }
            return Some(mapped);
        }
    }
    None
}

/// True for the exact path of a bind mount; such a directory cannot be
/// removed or renamed while the bind exists.
pub fn is_mount_point(path: &str) -> bool {
    let trimmed = if path.len() > 1 {
        path.trim_end_matches('/')
    } else {
        path
    };
    BINDS
        .lock()
        .iter()
        .any(|bind| bind.used && bind.target() == trimmed)
}

fn normalized(path: &str) -> bool {
    path.starts_with('/')
        && (path.len() == 1 || !path.ends_with('/'))
        && !path.contains("//")
        && !path.split('/').any(|part| part == "." || part == "..")
        && path.len() < PATH
}

fn on_volume(path: &str) -> bool {
    below(path, "/home").is_some() || below(path, "/media").is_some()
}

/// Makes `source` (a directory on a FAT volume) appear at `target` (a
/// directory of the RAM tree).
pub fn bind(target: &str, source: &str) -> Result<(), MountError> {
    if !normalized(target) || !normalized(source) || target == "/" {
        return Err(MountError::InvalidPath);
    }
    if on_volume(target)
        || !on_volume(source)
        || ["/tmp", "/data", "/proc", "/sys"]
            .iter()
            .any(|reserved| below(target, reserved).is_some())
    {
        return Err(MountError::NotAllowed);
    }
    let directory =
        |path: &str| vfs::metadata(path).is_ok_and(|metadata| metadata.mode & 0o170000 == 0o040000);
    if !directory(source) || !directory(target) {
        return Err(MountError::NotDirectory);
    }
    let mut binds = BINDS.lock();
    if binds.iter().any(|bind| {
        bind.used
            && (below(target, bind.target()).is_some() || below(bind.target(), target).is_some())
    }) {
        return Err(MountError::Busy);
    }
    let Some(slot) = binds.iter_mut().find(|bind| !bind.used) else {
        return Err(MountError::Full);
    };
    *slot = Bind::EMPTY;
    slot.used = true;
    slot.target[..target.len()].copy_from_slice(target.as_bytes());
    slot.target_length = target.len();
    slot.source[..source.len()].copy_from_slice(source.as_bytes());
    slot.source_length = source.len();
    Ok(())
}

pub fn unbind(target: &str) -> Result<(), MountError> {
    let trimmed = if target.len() > 1 {
        target.trim_end_matches('/')
    } else {
        target
    };
    let mut binds = BINDS.lock();
    match binds
        .iter_mut()
        .find(|bind| bind.used && bind.target() == trimmed)
    {
        Some(bind) => {
            *bind = Bind::EMPTY;
            Ok(())
        }
        None => Err(MountError::NotMounted),
    }
}

/// Calls `visit(target, source)` for every bind mount.
pub fn each_bind(mut visit: impl FnMut(&str, &str)) {
    let binds = *BINDS.lock();
    for bind in binds.iter().filter(|bind| bind.used) {
        visit(bind.target(), bind.source());
    }
}

/// Creates the persistent directories on the home volume and binds them over
/// `/var`, `/opt`, `/srv` and `/root`. Returns how many binds are active.
pub fn persistent_root() -> usize {
    if datafs::route("/home").is_none() {
        return 0;
    }
    let _ = vfs::create_directory(ROOT_DIRECTORY, 0o755);
    for name in PERSISTENT {
        let mut source = [0u8; PATH];
        let mut length = 0;
        for part in [ROOT_DIRECTORY, "/", name] {
            source[length..length + part.len()].copy_from_slice(part.as_bytes());
            length += part.len();
        }
        let Ok(source) = core::str::from_utf8(&source[..length]) else {
            continue;
        };
        let _ = vfs::create_directory(source, 0o755);
        let mut target = [0u8; 16];
        target[0] = b'/';
        target[1..=name.len()].copy_from_slice(name.as_bytes());
        if let Ok(target) = core::str::from_utf8(&target[..=name.len()]) {
            let _ = bind(target, source);
        }
    }
    let mut active = 0;
    each_bind(|_, _| active += 1);
    active
}

pub fn procfs_text(out: &mut impl Write) {
    let _ = write!(out, "rootfs / rootfs ro 0 0\ntmpfs /tmp tmpfs rw 0 0\n");
    let _ = write!(out, "proc /proc proc ro 0 0\nsysfs /sys sysfs ro 0 0\n");
    for index in 0..4 {
        if let Some(mount) = datafs::mount_info(index) {
            let _ = writeln!(out, "none {} {} rw 0 0", mount.path(), mount.kind);
        }
    }
    each_bind(|target, source| {
        let _ = writeln!(out, "{source} {target} none rw,bind 0 0");
    });
}

#[cfg(feature = "boot-test")]
#[derive(Clone, Copy, Default)]
pub struct Report {
    pub bound: usize,
    pub same_file: bool,
    pub listing: bool,
    pub rename_inside: bool,
    pub protected: bool,
    pub refused: bool,
    pub unbind: bool,
    pub persists: bool,
    pub verified: bool,
}

#[cfg(feature = "boot-test")]
fn read_all(path: &str, expected: &[u8]) -> bool {
    let Ok(handle) = vfs::open_file(path, false, false, false, 0, false) else {
        return false;
    };
    let mut buffer = [0u8; 256];
    let mut matched = 0;
    let mut good = true;
    while let Ok(count) = vfs::read(handle, &mut buffer) {
        if count == 0 {
            break;
        }
        good &= expected.get(matched..matched + count) == Some(&buffer[..count]);
        matched += count;
    }
    let _ = vfs::close(handle);
    good && matched == expected.len()
}

#[cfg(feature = "boot-test")]
pub fn self_test() -> Report {
    use vfs::VfsError;
    let mut report = Report {
        bound: persistent_root(),
        ..Report::default()
    };
    let payload = [0xa7u8; 700];
    let mut written = false;
    if let Ok(handle) = vfs::open_file("/var/mounts.txt", true, true, false, 0o644, true) {
        written = vfs::write(handle, &payload, false) == Ok(700);
        written &= vfs::close(handle).is_ok();
    }
    report.same_file = written
        && vfs::metadata("/home/.root/var/mounts.txt").is_ok_and(|metadata| metadata.size == 700)
        && read_all("/var/mounts.txt", &payload);
    let mut listed = false;
    if let Ok(directory) = vfs::open_directory("/var") {
        while let Ok(Some(entry)) = vfs::next_directory_entry(directory) {
            listed |= entry.name[..entry.name_len as usize].eq_ignore_ascii_case(b"mounts.txt");
        }
        let _ = vfs::close(directory);
    }
    report.listing = listed && vfs::create_directory("/var/log", 0o755).is_ok();
    report.rename_inside = vfs::rename("/var/mounts.txt", "/var/log/mounts.txt").is_ok()
        && matches!(vfs::metadata("/var/mounts.txt"), Err(VfsError::NotFound))
        && read_all("/var/log/mounts.txt", &payload);
    report.protected = is_mount_point("/srv")
        && vfs::remove("/srv", true) == Err(VfsError::Busy)
        && vfs::rename("/srv", "/ss") == Err(VfsError::Busy)
        && vfs::rename("/var/log", "/srv") == Err(VfsError::Busy)
        && vfs::metadata("/home/.root/srv")
            .is_ok_and(|metadata| metadata.mode & 0o170000 == 0o040000);
    report.refused = bind("/tmp", "/home/Notes") == Err(MountError::NotAllowed)
        && bind("/var", "/home/Notes") == Err(MountError::Busy)
        && bind("/opt", "/tmp") == Err(MountError::NotAllowed)
        && bind("/opt", "/home/Nothing") == Err(MountError::NotDirectory)
        && bind("var", "/home/Notes") == Err(MountError::InvalidPath)
        && bind("/opt/..", "/home/Notes") == Err(MountError::InvalidPath)
        && unbind("/nowhere") == Err(MountError::NotMounted);
    report.unbind = unbind("/srv").is_ok()
        && resolve("/srv/x").is_none()
        && bind("/srv", "/home/.root/srv").is_ok()
        && resolve("/srv/x").is_some_and(|mapped| mapped.as_str() == "/home/.root/srv/x");

    datafs::unmount(0);
    let again = datafs::initialize();
    report.persists = again.verified
        && !again.formatted
        && read_all("/var/log/mounts.txt", &payload)
        && persistent_root() >= PERSISTENT.len();
    let cleaned = vfs::remove("/var/log/mounts.txt", false).is_ok()
        && vfs::remove("/var/log", true).is_ok()
        && matches!(vfs::metadata("/var/log"), Err(VfsError::NotFound));
    report.verified = report.bound >= PERSISTENT.len()
        && report.same_file
        && report.listing
        && report.rename_inside
        && report.protected
        && report.refused
        && report.unbind
        && report.persists
        && cleaned;
    report
}
