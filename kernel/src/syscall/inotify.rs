//! inotify: an instance is a byte pipe the kernel fills with `inotify_event`
//! records; watches name a path and the events wanted. The syscall layer
//! reports what the processes do (create, write, delete, rename, ...), so
//! changes made through other means (the desktop, the shell) are not seen.

use super::*;

pub(super) const IN_ACCESS: u32 = 0x1;
pub(super) const IN_MODIFY: u32 = 0x2;
pub(super) const IN_ATTRIB: u32 = 0x4;
pub(super) const IN_CLOSE_WRITE: u32 = 0x8;
pub(super) const IN_CLOSE_NOWRITE: u32 = 0x10;
pub(super) const IN_OPEN: u32 = 0x20;
pub(super) const IN_MOVED_FROM: u32 = 0x40;
pub(super) const IN_MOVED_TO: u32 = 0x80;
pub(super) const IN_CREATE: u32 = 0x100;
pub(super) const IN_DELETE: u32 = 0x200;
pub(super) const IN_DELETE_SELF: u32 = 0x400;
pub(super) const IN_MOVE_SELF: u32 = 0x800;
pub(super) const IN_ISDIR: u32 = 0x4000_0000;
const IN_MASK_ADD: u32 = 0x2000_0000;
const IN_ALL_EVENTS: u32 = 0xfff;

const WATCHES: usize = 32;
const PATH_BYTES: usize = 128;
const RECORD_HEADER: usize = 16;

#[derive(Clone, Copy)]
struct Watch {
    used: bool,
    slot: usize,
    wd: i32,
    mask: u32,
    length: usize,
    path: [u8; PATH_BYTES],
}

impl Watch {
    const EMPTY: Self = Self {
        used: false,
        slot: 0,
        wd: 0,
        mask: 0,
        length: 0,
        path: [0; PATH_BYTES],
    };

    fn path(&self) -> &[u8] {
        &self.path[..self.length]
    }
}

static WATCH_TABLE: TicketLock<[Watch; WATCHES]> = TicketLock::new([Watch::EMPTY; WATCHES]);
static WATCH_COUNT: AtomicU64 = AtomicU64::new(0);
static COOKIE: AtomicU64 = AtomicU64::new(1);

pub(super) fn any_watches() -> bool {
    WATCH_COUNT.load(Ordering::Acquire) != 0
}

pub(super) fn next_cookie() -> u32 {
    COOKIE.fetch_add(1, Ordering::Relaxed) as u32
}

fn is_instance(slot: usize) -> bool {
    PIPES
        .lock()
        .get(slot)
        .is_some_and(|pipe| pipe.used && pipe.inotify)
}

pub(super) fn create(close_on_exec: bool) -> Result<u64, u64> {
    let template = Pipe {
        inotify: true,
        ..Pipe::EMPTY
    };
    create_single_fd_pipe(template, close_on_exec, true)
}

fn refresh_count(table: &[Watch; WATCHES]) {
    let used = table.iter().filter(|watch| watch.used).count();
    WATCH_COUNT.store(used as u64, Ordering::Release);
}

/// Drops the watches of an instance that was closed.
pub(super) fn drop_instance(slot: usize) {
    let mut table = WATCH_TABLE.lock();
    for watch in table
        .iter_mut()
        .filter(|watch| watch.used && watch.slot == slot)
    {
        *watch = Watch::EMPTY;
    }
    refresh_count(&table);
}

/// Adds or updates a watch of `path` on the instance at `slot`; returns the
/// watch descriptor.
pub(super) fn add_watch(slot: usize, path: &str, mask: u32) -> Result<i32, u64> {
    if !is_instance(slot) {
        return Err(22);
    }
    let events = mask & IN_ALL_EVENTS;
    if events == 0 || path.len() > PATH_BYTES {
        return Err(22);
    }
    let mut table = WATCH_TABLE.lock();
    if let Some(watch) = table
        .iter_mut()
        .find(|watch| watch.used && watch.slot == slot && watch.path() == path.as_bytes())
    {
        watch.mask = if mask & IN_MASK_ADD != 0 {
            watch.mask | events
        } else {
            events
        };
        return Ok(watch.wd);
    }
    let next = table
        .iter()
        .filter(|watch| watch.used && watch.slot == slot)
        .map(|watch| watch.wd)
        .max()
        .unwrap_or(0)
        + 1;
    let Some(entry) = table.iter_mut().find(|watch| !watch.used) else {
        return Err(28);
    };
    *entry = Watch {
        used: true,
        slot,
        wd: next,
        mask: events,
        length: path.len(),
        path: [0; PATH_BYTES],
    };
    entry.path[..path.len()].copy_from_slice(path.as_bytes());
    refresh_count(&table);
    Ok(next)
}

pub(super) fn remove_watch(slot: usize, wd: i32) -> Result<(), u64> {
    if !is_instance(slot) {
        return Err(22);
    }
    let mut table = WATCH_TABLE.lock();
    let Some(watch) = table
        .iter_mut()
        .find(|watch| watch.used && watch.slot == slot && watch.wd == wd)
    else {
        return Err(22);
    };
    *watch = Watch::EMPTY;
    refresh_count(&table);
    Ok(())
}

fn queue_event(slot: usize, wd: i32, mask: u32, cookie: u32, name: &[u8]) {
    let padded = if name.is_empty() {
        0
    } else {
        (name.len() + 1).next_multiple_of(RECORD_HEADER)
    };
    let total = RECORD_HEADER + padded;
    let mut record = [0u8; RECORD_HEADER + 64];
    if padded > 64 {
        return;
    }
    record[..4].copy_from_slice(&wd.to_le_bytes());
    record[4..8].copy_from_slice(&mask.to_le_bytes());
    record[8..12].copy_from_slice(&cookie.to_le_bytes());
    record[12..16].copy_from_slice(&(padded as u32).to_le_bytes());
    record[RECORD_HEADER..RECORD_HEADER + name.len()].copy_from_slice(name);
    let mut guard = PIPES.lock();
    let table = &mut *guard;
    let Some(pipe) = table.pipes.get_mut(slot) else {
        return;
    };
    if !pipe.used || !pipe.inotify || PIPE_BUFFER_BYTES - pipe.length < total {
        return;
    }
    ring_put(&mut table.data[slot], pipe.head, &mut pipe.length, &record[..total]);
}

/// Reports `mask` events about `path` (an absolute path; add `IN_ISDIR` for a
/// directory) to the watches that want them: those of the object itself, with
/// no name, and those of its directory, with its name.
pub(super) fn notify(path: &str, mask: u32, cookie: u32) {
    if !any_watches() {
        return;
    }
    let event = mask & !IN_ISDIR;
    let directory_flag = mask & IN_ISDIR;
    let (parent, name) = match path.rfind('/') {
        Some(0) => ("/", &path[1..]),
        Some(split) => (&path[..split], &path[split + 1..]),
        None => return,
    };
    let mut matches = [(0usize, 0i32, 0u32, false); WATCHES];
    let mut found = 0;
    {
        let table = WATCH_TABLE.lock();
        for watch in table.iter().filter(|watch| watch.used) {
            let on_self = watch.path() == path.as_bytes();
            let on_child = watch.path() == parent.as_bytes() && !path.is_empty() && path != "/";
            if !(on_self || on_child) {
                continue;
            }
            let wanted = if on_child {
                event
            } else {
                match event {
                    IN_DELETE => IN_DELETE_SELF,
                    IN_MOVED_FROM => IN_MOVE_SELF,
                    IN_CREATE | IN_MOVED_TO => 0,
                    other => other,
                }
            };
            if wanted != 0 && watch.mask & wanted != 0 {
                matches[found] = (watch.slot, watch.wd, wanted | directory_flag, on_child);
                found += 1;
            }
        }
    }
    for (slot, wd, reported, named) in &matches[..found] {
        queue_event(
            *slot,
            *wd,
            *reported,
            cookie,
            if *named { name.as_bytes() } else { &[] },
        );
    }
}

/// `notify` for a file given by its open handle.
pub(super) fn notify_handle(handle: u32, mask: u32) {
    if !any_watches() {
        return;
    }
    let Some((buffer, length)) = vfs::descriptor_path(handle) else {
        return;
    };
    if let Ok(path) = core::str::from_utf8(&buffer[..length]) {
        notify(path, mask, 0);
    }
}

pub(super) fn linux_init(flags: u64) -> u64 {
    if flags & !0o2004000 != 0 {
        return error(22);
    }
    match create(flags & 0o2000000 != 0) {
        Ok(descriptor) => descriptor,
        Err(failure) => error(failure),
    }
}

pub(super) fn linux_add_watch(descriptor: u64, path_address: u64, mask: u64) -> u64 {
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return error(9);
    };
    if !process_fd.pipe || !is_instance(process_fd.handle as usize) {
        return error(22);
    }
    let mut resolved = [0u8; MAX_PATH];
    let (length, _) = match resolve_user_path(AT_FDCWD as u64, path_address, &mut resolved) {
        Ok(value) => value,
        Err(failure) => return failure,
    };
    let Ok(path) = core::str::from_utf8(&resolved[..length]) else {
        return error(84);
    };
    if vfs::metadata(path).is_err() {
        return error(2);
    }
    match add_watch(process_fd.handle as usize, path, mask as u32) {
        Ok(wd) => wd as u64,
        Err(failure) => error(failure),
    }
}

pub(super) fn linux_rm_watch(descriptor: u64, wd: u64) -> u64 {
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return error(9);
    };
    if !process_fd.pipe {
        return error(22);
    }
    match remove_watch(process_fd.handle as usize, wd as i32) {
        Ok(()) => 0,
        Err(failure) => error(failure),
    }
}

/// Watches driven without user pointers: events of the object itself and of
/// its children, masks, names, the cookie of a rename and instance cleanup.
pub(super) fn self_test() -> bool {
    let Ok(descriptor) = create(false) else {
        return false;
    };
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return false;
    };
    let slot = process_fd.handle as usize;
    let directory = add_watch(slot, "/tmp/iw", IN_CREATE | IN_DELETE | IN_MODIFY);
    let file = add_watch(slot, "/tmp/iw/f", IN_MODIFY | IN_DELETE);
    let again = add_watch(slot, "/tmp/iw", IN_MASK_ADD | IN_MOVED_TO);
    let refused = add_watch(slot, "/tmp/iw", 0) == Err(22) && add_watch(9999, "/x", 1) == Err(22);
    notify("/tmp/iw/f", IN_CREATE, 0);
    notify("/tmp/iw/f", IN_MODIFY, 0);
    notify("/tmp/iw/f", IN_ACCESS, 0);
    notify("/tmp/iw/f", IN_DELETE, 0);
    notify("/tmp/iw/d", IN_CREATE | IN_ISDIR, 0);
    notify("/tmp/iw/g", IN_MOVED_TO, 7);
    notify("/tmp/other", IN_CREATE, 0);
    let mut bytes = [0u8; 512];
    let read = pipe_read(slot, &mut bytes).unwrap_or(0);
    // The file watch asked for IN_DELETE, which on the file itself would be
    // IN_DELETE_SELF: it reports nothing for the delete.
    let mut events: [(i32, u32, u32, [u8; 8]); 8] = [(0, 0, 0, [0; 8]); 8];
    let mut count = 0;
    let mut position = 0;
    while position + RECORD_HEADER <= read && count < events.len() {
        let field = |at: usize| {
            u32::from_le_bytes([
                bytes[position + at],
                bytes[position + at + 1],
                bytes[position + at + 2],
                bytes[position + at + 3],
            ])
        };
        let length = field(12) as usize;
        let mut name = [0u8; 8];
        for (index, byte) in name.iter_mut().enumerate().take(length.min(8)) {
            *byte = bytes[position + RECORD_HEADER + index];
        }
        events[count] = (field(0) as i32, field(4), field(8), name);
        count += 1;
        position += RECORD_HEADER + length;
    }
    let first_wd = directory.unwrap_or(-1);
    let second_wd = file.unwrap_or(-1);
    let named = |index: usize, wd: i32, mask: u32, name: &[u8]| {
        let event = events[index];
        event.0 == wd && event.1 == mask && event.3.starts_with(name) && event.3[name.len()] == 0
    };
    let shape = count == 6
        && named(0, first_wd, IN_CREATE, b"f")
        && named(1, first_wd, IN_MODIFY, b"f")
        && named(2, second_wd, IN_MODIFY, b"")
        && named(3, first_wd, IN_DELETE, b"f")
        && named(4, first_wd, IN_CREATE | IN_ISDIR, b"d")
        && named(5, first_wd, IN_MOVED_TO, b"g")
        && events[5].2 == 7;
    let removed = remove_watch(slot, second_wd) == Ok(()) && remove_watch(slot, second_wd).is_err();
    let closed = linux_close(descriptor) == 0;
    let cleaned = !WATCH_TABLE
        .lock()
        .iter()
        .any(|watch| watch.used && watch.slot == slot);
    directory == Ok(1)
        && file == Ok(2)
        && again == Ok(1)
        && refused
        && shape
        && removed
        && closed
        && cleaned
}
