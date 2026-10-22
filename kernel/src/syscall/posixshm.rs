//! POSIX shared memory: a file in `/dev/shm` mapped `MAP_SHARED` is one set
//! of frames for every process that maps it (found by the file's inode), the
//! way `shm_open`, `sem_open` and Python's multiprocessing use it. The frames
//! start as the file's content; what is written through a mapping is not
//! written back to the file.

use super::*;

/// Where `/dev/shm` lives (see `mounts::resolve`).
pub(crate) const BACKING: &str = "/tmp/.dev/shm";
const OBJECTS: usize = 16;
const OBJECT_PAGES: usize = 512;

struct Object {
    used: bool,
    inode: u64,
    pages: usize,
    frames: [u64; OBJECT_PAGES],
}

impl Object {
    const EMPTY: Self = Self {
        used: false,
        inode: 0,
        pages: 0,
        frames: [0; OBJECT_PAGES],
    };

    fn release(&mut self) {
        for frame in &self.frames[..self.pages] {
            crate::arch::paging::release_frame(*frame);
        }
        self.used = false;
        self.inode = 0;
        self.pages = 0;
    }
}

static TABLE: TicketLock<[Object; OBJECTS]> = TicketLock::new([Object::EMPTY; OBJECTS]);

fn in_backing(path: &str) -> bool {
    path.strip_prefix(BACKING)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// Whether an open file is in `/dev/shm`.
pub(super) fn is_shared_file(handle: u32) -> bool {
    vfs::descriptor_path(handle)
        .is_some_and(|(bytes, length)| core::str::from_utf8(&bytes[..length]).is_ok_and(in_backing))
}

/// Maps `pages` pages of the shared file `handle`, from page `first`, into
/// the running process; returns the address or an error code.
pub(super) fn map(handle: u32, first: usize, pages: usize, protection: u8) -> u64 {
    let Ok(metadata) = vfs::descriptor_metadata(handle) else {
        return error(9);
    };
    let Some(needed) = first
        .checked_add(pages)
        .filter(|needed| *needed <= OBJECT_PAGES)
    else {
        return error(12);
    };
    let mut frames = [0u64; OBJECT_PAGES];
    {
        let mut table = TABLE.lock();
        let slot = match table
            .iter()
            .position(|object| object.used && object.inode == metadata.inode)
        {
            Some(slot) => slot,
            None => {
                let Some(slot) = table.iter().position(|object| !object.used) else {
                    return error(23);
                };
                let object = &mut table[slot];
                object.used = true;
                object.inode = metadata.inode;
                object.pages = 0;
                slot
            }
        };
        let object = &mut table[slot];
        // Pages the object does not have yet start as the file's content.
        while object.pages < needed {
            let Some(frame) = crate::memory::allocate_global() else {
                return error(12);
            };
            let address = frame.address();
            // SAFETY: a frame just taken from the allocator, mapped 1:1.
            let page =
                unsafe { core::slice::from_raw_parts_mut(address as usize as *mut u8, 4096) };
            page.fill(0);
            let _ = vfs::read_at(handle, object.pages * 4096, page);
            object.frames[object.pages] = address;
            object.pages += 1;
        }
        for (held, frame) in object.frames[first..needed].iter().enumerate() {
            if !crate::arch::paging::retain_frame(*frame) {
                for taken in &frames[..held] {
                    crate::arch::paging::release_frame(*taken);
                }
                return error(12);
            }
            frames[held] = *frame;
        }
    }
    match crate::scheduler::linux_mmap_shared_for_current_task(
        pages,
        protection,
        Some(&frames[..pages]),
    ) {
        Some(Some(address)) => address,
        failed => {
            for frame in &frames[..pages] {
                crate::arch::paging::release_frame(*frame);
            }
            error(if failed.is_none() { 38 } else { 12 })
        }
    }
}

/// The inode of a `/dev/shm` file about to be removed.
pub(super) fn inode_before_removal(path: &str) -> Option<u64> {
    let in_shm = path
        .strip_prefix("/dev/shm")
        .is_some_and(|rest| rest.starts_with('/'))
        || in_backing(path);
    if !in_shm {
        return None;
    }
    vfs::metadata(path).ok().map(|metadata| metadata.inode)
}

/// The file is gone: its frames stay only as long as mappings hold them.
pub(super) fn forget(inode: u64) {
    let mut table = TABLE.lock();
    if let Some(object) = table
        .iter_mut()
        .find(|object| object.used && object.inode == inode)
    {
        object.release();
    }
}

/// `link` inside `/dev/shm`, all `sem_open` needs of it: the new name takes
/// the file over (its temporary name is removed right after anyway).
pub(super) fn link(source: &str, destination: &str) -> Option<u64> {
    let shm = |path: &str| {
        path.strip_prefix("/dev/shm")
            .is_some_and(|rest| rest.len() > 1 && rest.starts_with('/'))
    };
    if !shm(source) || !shm(destination) {
        return None;
    }
    Some(match vfs::rename_noreplace(source, destination) {
        Ok(()) => 0,
        Err(failure) => vfs_error(failure),
    })
}
