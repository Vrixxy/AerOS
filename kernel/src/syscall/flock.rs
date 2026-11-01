//! `flock`: whole-file advisory locks. A lock belongs to an open file (the
//! descriptors `dup` and `fork` make share it) and goes when the last of
//! them closes; separate `open`s of one file compete for it.

use super::*;

const LOCKS: usize = 64;
const LOCK_EX: u64 = 2;
const LOCK_NB: u64 = 4;
const LOCK_UN: u64 = 8;

#[derive(Clone, Copy)]
struct Held {
    used: bool,
    /// The file (its inode).
    file: u64,
    /// The open file holding it (its VFS handle).
    owner: u32,
    exclusive: bool,
}

impl Held {
    const EMPTY: Self = Self {
        used: false,
        file: 0,
        owner: 0,
        exclusive: false,
    };
}

static HELD: TicketLock<[Held; LOCKS]> = TicketLock::new([Held::EMPTY; LOCKS]);

/// Takes `owner`'s lock on `file`, or turns the one it holds into the kind
/// asked for; `Err(11)` while another open file holds a conflicting one.
fn try_lock(file: u64, owner: u32, exclusive: bool) -> Result<(), u64> {
    let mut held = HELD.lock();
    let conflict = held.iter().any(|lock| {
        lock.used && lock.file == file && lock.owner != owner && (exclusive || lock.exclusive)
    });
    if conflict {
        return Err(11);
    }
    if let Some(mine) = held
        .iter_mut()
        .find(|lock| lock.used && lock.file == file && lock.owner == owner)
    {
        mine.exclusive = exclusive;
        return Ok(());
    }
    let Some(slot) = held.iter_mut().find(|lock| !lock.used) else {
        return Err(37);
    };
    *slot = Held {
        used: true,
        file,
        owner,
        exclusive,
    };
    Ok(())
}

/// The open file `owner` is gone: its locks go with it.
pub(super) fn release(owner: u32) {
    for lock in HELD.lock().iter_mut() {
        if lock.used && lock.owner == owner {
            *lock = Held::EMPTY;
        }
    }
}

/// `flock(fd, operation)` on a file descriptor; anything that is not a file
/// (a pipe, a socket) takes no lock, as nothing else can open it by name.
pub(super) fn apply(process_fd: &ProcessFd, operation: u64) -> u64 {
    let is_file = process_fd.standard == 0
        && !(process_fd.socket
            || process_fd.pipe
            || process_fd.unix_socket
            || process_fd.unix_listener
            || process_fd.tcp
            || process_fd.epoll);
    if !is_file {
        return 0;
    }
    let file = match vfs::descriptor_metadata(process_fd.handle) {
        Ok(metadata) => metadata.inode,
        Err(failure) => return vfs_error(failure),
    };
    let owner = process_fd.handle;
    if operation & !LOCK_NB == LOCK_UN {
        for lock in HELD.lock().iter_mut() {
            if lock.used && lock.file == file && lock.owner == owner {
                *lock = Held::EMPTY;
            }
        }
        return 0;
    }
    let exclusive = operation & !LOCK_NB == LOCK_EX;
    loop {
        match try_lock(file, owner, exclusive) {
            Ok(()) => return 0,
            Err(11) if operation & LOCK_NB == 0 => {
                if interrupted() {
                    return error(4);
                }
                yield_in_syscall();
            }
            Err(code) => return error(code),
        }
    }
}
