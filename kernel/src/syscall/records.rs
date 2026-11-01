//! `fcntl` record locks: advisory locks on byte ranges of a file. A POSIX
//! lock (`F_SETLK`, `F_SETLKW`, `F_GETLK`) belongs to the process and goes
//! when it closes any descriptor of the file or exits; an open file
//! description lock (`F_OFD_*`) belongs to the open file, like `flock`.

use super::*;

const RECORDS: usize = 64;
const F_RDLCK: i16 = 0;
const F_WRLCK: i16 = 1;
const F_UNLCK: i16 = 2;
/// Marks an owner that is an open file (its VFS handle) rather than a process.
const OPEN_FILE: u64 = 1 << 40;
const EOF: u64 = u64::MAX;

#[derive(Clone, Copy)]
struct Record {
    used: bool,
    file: u64,
    owner: u64,
    /// The process that set it (reported by `F_GETLK`).
    pid: u32,
    start: u64,
    /// One past the last byte, or `EOF` for "to the end, however far".
    end: u64,
    write: bool,
}

impl Record {
    const EMPTY: Self = Self {
        used: false,
        file: 0,
        owner: 0,
        pid: 0,
        start: 0,
        end: 0,
        write: false,
    };
}

static RECORDS_HELD: TicketLock<[Record; RECORDS]> = TicketLock::new([Record::EMPTY; RECORDS]);

/// Removes `owner`'s locks on `file` within `start..end`, keeping the parts
/// outside it. `Err(37)` if a split needs a slot and none is free.
fn carve(
    records: &mut [Record; RECORDS],
    file: u64,
    owner: u64,
    start: u64,
    end: u64,
) -> Result<(), u64> {
    for index in 0..RECORDS {
        let record = records[index];
        if !record.used || record.file != file || record.owner != owner {
            continue;
        }
        if record.end <= start || record.start >= end {
            continue;
        }
        let left = (record.start < start).then_some((record.start, start));
        let right = (record.end > end).then_some((end, record.end));
        records[index] = Record::EMPTY;
        for (from, to) in [left, right].into_iter().flatten() {
            let slot = records.iter().position(|slot| !slot.used).ok_or(37u64)?;
            records[slot] = Record {
                start: from,
                end: to,
                ..record
            };
        }
    }
    Ok(())
}

/// The first lock another owner holds on `file` that conflicts with the
/// asked-for one.
fn conflict(
    records: &[Record; RECORDS],
    file: u64,
    owner: u64,
    start: u64,
    end: u64,
    write: bool,
) -> Option<Record> {
    records.iter().copied().find(|record| {
        record.used
            && record.file == file
            && record.owner != owner
            && record.start < end
            && start < record.end
            && (write || record.write)
    })
}

/// Drops every POSIX lock process `pid` holds on `file` (it closed a
/// descriptor of it), or on every file for `None` (it exited).
pub(super) fn release_process(pid: u64, file: Option<u64>) {
    for record in RECORDS_HELD.lock().iter_mut() {
        if record.used && record.owner == pid && file.is_none_or(|file| record.file == file) {
            *record = Record::EMPTY;
        }
    }
}

/// The open file `handle` is gone: its open file description locks go too.
pub(super) fn release_open_file(handle: u32) {
    release_process(OPEN_FILE | u64::from(handle), None);
}

/// The inode of a descriptor that names a file, for lock bookkeeping.
pub(super) fn file_of(process_fd: &ProcessFd) -> Option<u64> {
    let named = process_fd.standard == 0
        && !(process_fd.socket
            || process_fd.pipe
            || process_fd.unix_socket
            || process_fd.unix_listener
            || process_fd.tcp
            || process_fd.epoll);
    named.then(|| {
        vfs::descriptor_metadata(process_fd.handle)
            .ok()
            .map(|metadata| metadata.inode)
    })?
}

/// `fcntl` commands 5..7 (POSIX) and 36..38 (open file description) with the
/// `struct flock` at `address`.
pub(super) fn command(process_fd: &ProcessFd, command: u64, address: u64) -> u64 {
    let mut raw = [0u8; 32];
    if !user::copy_from_user(address, &mut raw) {
        return error(14);
    }
    let kind = i16::from_le_bytes([raw[0], raw[1]]);
    let whence = i16::from_le_bytes([raw[2], raw[3]]);
    let offset = i64::from_le_bytes(raw[8..16].try_into().unwrap_or([0; 8]));
    let length = i64::from_le_bytes(raw[16..24].try_into().unwrap_or([0; 8]));
    let Some(file) = file_of(process_fd) else {
        return error(9);
    };
    let base = match whence {
        0 => 0,
        1 => match vfs::seek(process_fd.handle, 0, 1) {
            Ok(position) => position as i64,
            Err(failure) => return vfs_error(failure),
        },
        2 => match vfs::descriptor_metadata(process_fd.handle) {
            Ok(metadata) => metadata.size as i64,
            Err(failure) => return vfs_error(failure),
        },
        _ => return error(22),
    };
    let first = base.saturating_add(offset);
    let (start, end) = match length {
        0 => (first, i64::MAX),
        len if len > 0 => (first, first.saturating_add(len)),
        len => (first.saturating_add(len), first),
    };
    if start < 0 {
        return error(22);
    }
    let (start, end) = (start as u64, if end == i64::MAX { EOF } else { end as u64 });
    let pid = crate::scheduler::current_group();
    let owner = if command >= 36 {
        OPEN_FILE | u64::from(process_fd.handle)
    } else {
        pid
    };
    if command == 5 || command == 36 {
        let found = conflict(
            &RECORDS_HELD.lock(),
            file,
            owner,
            start,
            end,
            kind == F_WRLCK,
        );
        let mut reply = raw;
        match found {
            Some(record) => {
                let kind = if record.write { F_WRLCK } else { F_RDLCK };
                reply[0..2].copy_from_slice(&kind.to_le_bytes());
                reply[2..4].copy_from_slice(&0i16.to_le_bytes());
                reply[8..16].copy_from_slice(&(record.start as i64).to_le_bytes());
                let length = if record.end == EOF {
                    0
                } else {
                    (record.end - record.start) as i64
                };
                reply[16..24].copy_from_slice(&length.to_le_bytes());
                let pid = if record.owner & OPEN_FILE != 0 {
                    -1
                } else {
                    record.pid as i32
                };
                reply[24..28].copy_from_slice(&pid.to_le_bytes());
            }
            None => reply[0..2].copy_from_slice(&F_UNLCK.to_le_bytes()),
        }
        return if user::copy_to_user(address, &reply) {
            0
        } else {
            error(14)
        };
    }
    if !matches!(kind, F_RDLCK | F_WRLCK | F_UNLCK) {
        return error(22);
    }
    let wait = command == 7 || command == 38;
    loop {
        let mut records = RECORDS_HELD.lock();
        if kind != F_UNLCK && conflict(&records, file, owner, start, end, kind == F_WRLCK).is_some()
        {
            drop(records);
            if !wait {
                return error(11);
            }
            if interrupted() {
                return error(4);
            }
            yield_in_syscall();
            continue;
        }
        if let Err(code) = carve(&mut records, file, owner, start, end) {
            return error(code);
        }
        if kind == F_UNLCK {
            return 0;
        }
        let Some(slot) = records.iter().position(|slot| !slot.used) else {
            return error(37);
        };
        records[slot] = Record {
            used: true,
            file,
            owner,
            pid: pid as u32,
            start,
            end,
            write: kind == F_WRLCK,
        };
        return 0;
    }
}
