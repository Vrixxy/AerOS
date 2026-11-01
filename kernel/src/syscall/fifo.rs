//! Named pipes (`mkfifo`): a file whose inode is marked as a FIFO. Opening
//! it connects to one in-kernel pipe shared by everyone who opens that file:
//! a reader waits for a writer and a writer for a reader (or, opened
//! non-blocking, fails with `ENXIO`), and readers see end-of-file once the
//! writers are gone.

use super::*;

const FIFOS: usize = 32;

static NODES: TicketLock<[u64; FIFOS]> = TicketLock::new([0; FIFOS]);

/// Whether any FIFO exists (so an ordinary open need not look further).
pub(super) fn any() -> bool {
    NODES.lock().iter().any(|&inode| inode != 0)
}

pub(super) fn is_fifo(inode: u64) -> bool {
    inode != 0 && NODES.lock().contains(&inode)
}

/// Marks the file `inode` as a FIFO; false when the table is full.
pub(super) fn mark(inode: u64) -> bool {
    let mut nodes = NODES.lock();
    if nodes.contains(&inode) {
        return true;
    }
    match nodes.iter_mut().find(|slot| **slot == 0) {
        Some(slot) => {
            *slot = inode;
            true
        }
        None => false,
    }
}

/// The file `inode` is being removed.
pub(super) fn forget(inode: u64) {
    for slot in NODES.lock().iter_mut() {
        if *slot == inode {
            *slot = 0;
        }
    }
}

/// `stat` of a FIFO reports it as one.
pub(super) fn reported(metadata: vfs::Metadata) -> vfs::Metadata {
    if any() && is_fifo(metadata.inode) {
        vfs::Metadata {
            mode: 0o010000 | (metadata.mode & 0o7777),
            size: 0,
            ..metadata
        }
    } else {
        metadata
    }
}

/// Opens the FIFO `inode` for reading and/or writing.
pub(super) fn open(inode: u64, read: bool, write: bool, nonblocking: bool, cloexec: bool) -> u64 {
    let slot = {
        let mut pipes = PIPES.lock();
        match pipes
            .iter()
            .position(|pipe| pipe.used && pipe.fifo == inode)
        {
            Some(slot) => slot,
            None => {
                let Some(slot) = pipes.iter().position(|pipe| !pipe.used) else {
                    return error(24);
                };
                pipes[slot] = Pipe {
                    used: true,
                    fifo: inode,
                    ..Pipe::EMPTY
                };
                slot
            }
        }
    };
    if read {
        PIPES.lock()[slot].read_open = true;
    }
    loop {
        let ready = {
            let pipes = PIPES.lock();
            let pipe = &pipes[slot];
            match (read, write) {
                (true, false) => pipe.fifo_writer || nonblocking,
                (false, true) => pipe.read_open,
                _ => true,
            }
        };
        if ready {
            break;
        }
        if nonblocking || interrupted() {
            let mut pipes = PIPES.lock();
            let pipe = &mut pipes[slot];
            if read {
                pipe.read_open = false;
            }
            if !pipe.read_open && !pipe.write_open {
                *pipe = Pipe::EMPTY;
            }
            return error(if nonblocking { 6 } else { 4 });
        }
        yield_in_syscall();
    }
    if write {
        let mut pipes = PIPES.lock();
        pipes[slot].write_open = true;
        pipes[slot].fifo_writer = true;
    }
    let Some(descriptor) = install_process_fd_kind(
        slot as u32,
        cloexec,
        false,
        true,
        false,
        false,
        false,
        false,
        read,
        write,
        false,
    ) else {
        return error(24);
    };
    if nonblocking && let Some(index) = process_fd_index(descriptor) {
        PROCESS_FDS.lock()[index].nonblocking = true;
    }
    OPENS.fetch_add(1, Ordering::Relaxed);
    descriptor
}
