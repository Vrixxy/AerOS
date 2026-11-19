//! The rest of the x86-64 Linux table. Where the answer is simple on one
//! machine with one memory node, the call does the real thing (host names,
//! I/O priority, memory policy, clock state, futex2, `fchmodat2`, protection
//! keys). Where a feature is absent the call fails the way Linux does when
//! that feature is configured out or needs a privilege AerOS does not hand
//! out, so programs take their usual fallback.

use super::*;

const ENOSYS: u64 = 38;
const EPERM: u64 = 1;
const EINVAL: u64 = 22;
const ENODATA: u64 = 61;
const EOPNOTSUPP: u64 = 95;
const ENOSPC: u64 = 28;

/// Calls whose subsystem AerOS does not have (or that Linux itself retired):
/// refused like a kernel built without them.
const ABSENT: &[u64] = &[
    134, // uselib
    136, // ustat
    139, // sysfs
    154, // modify_ldt
    156, // _sysctl
    174, // create_module
    177, // get_kernel_syms
    178, // query_module
    180, // nfsservctl
    181, 182, 183, 184, 185, // getpmsg, putpmsg, afs_syscall, tuxcall, security
    205, 211, // set_thread_area, get_thread_area (32-bit TLS)
    212, // lookup_dcookie
    214, 215, // epoll_ctl_old, epoll_wait_old
    216, // remap_file_pages
    236, // vserver
    300, 301, // fanotify_init, fanotify_mark
    312, // kcmp
    321, // bpf
    323, // userfaultfd
    425, 426, 427, // io_uring_setup, _enter, _register
    428, 429, 430, 431, 432, 433, // open_tree, move_mount, fsopen, fsconfig, fsmount, fspick
    438, // pidfd_getfd
    442, // mount_setattr
    453, // map_shadow_stack
    457, 458, // statmount, listmount
    459, 460, 461, // lsm_get_self_attr, lsm_set_self_attr, lsm_list_modules
    462, // mseal
    467, // open_tree_attr
];

/// Administrative calls that need a privilege no AerOS process holds.
const PRIVILEGED: &[u64] = &[
    153, // vhangup
    155, // pivot_root
    163, // acct
    167, 168, // swapon, swapoff
    172, 173, // iopl, ioperm
    175, 176, 313, // init_module, delete_module, finit_module
    179, 443, // quotactl, quotactl_fd
    246, 320, // kexec_load, kexec_file_load
];

const fn uts_name(text: &[u8]) -> [u8; 65] {
    let mut name = [0u8; 65];
    let mut index = 0;
    while index < text.len() && index < 64 {
        name[index] = text[index];
        index += 1;
    }
    name
}

static HOSTNAME: TicketLock<[u8; 65]> = TicketLock::new(uts_name(b"aeros"));
static DOMAINNAME: TicketLock<[u8; 65]> = TicketLock::new(uts_name(b"(none)"));

/// The host name `uname` reports, without the terminating zero.
pub(super) fn hostname(out: &mut [u8; 65]) -> usize {
    *out = *HOSTNAME.lock();
    out.iter().position(|byte| *byte == 0).unwrap_or(64)
}

pub(super) fn domainname(out: &mut [u8; 65]) -> usize {
    *out = *DOMAINNAME.lock();
    out.iter().position(|byte| *byte == 0).unwrap_or(64)
}

fn set_uts(target: &TicketLock<[u8; 65]>, address: u64, length: u64) -> u64 {
    if !has_capability(crate::capability::CAP_SYS_ADMIN) {
        return error(EPERM);
    }
    if length > 64 {
        return error(EINVAL);
    }
    let mut name = [0u8; 65];
    if !user::copy_from_user(address, &mut name[..length as usize]) {
        return error(14);
    }
    *target.lock() = name;
    0
}

/// `ioprio_set`/`ioprio_get` for processes (class and level, kept per pid).
static IO_PRIORITY: TicketLock<[(u64, u16); 32]> = TicketLock::new([(0, 0); 32]);
const IOPRIO_DEFAULT: u16 = (2 << 13) | 4;

fn ioprio_target(which: u64, who: u64) -> Result<u64, u64> {
    match which {
        1 => Ok(if who == 0 { current_process_id() } else { who }),
        2 | 3 => Ok(current_process_id()),
        _ => Err(error(EINVAL)),
    }
}

fn ioprio_set(which: u64, who: u64, priority: u64) -> u64 {
    let pid = match ioprio_target(which, who) {
        Ok(pid) => pid,
        Err(failure) => return failure,
    };
    let class = (priority >> 13) & 7;
    let level = priority & 0x1fff;
    if class > 3 || (class != 0 && level > 7) {
        return error(EINVAL);
    }
    if class == 1 && !has_capability(crate::capability::CAP_SYS_NICE) {
        return error(EPERM);
    }
    let mut table = IO_PRIORITY.lock();
    let slot = table
        .iter()
        .position(|(owner, _)| *owner == pid)
        .or_else(|| table.iter().position(|(owner, _)| *owner == 0))
        .unwrap_or(0);
    table[slot] = (pid, priority as u16);
    0
}

fn ioprio_get(which: u64, who: u64) -> u64 {
    let pid = match ioprio_target(which, who) {
        Ok(pid) => pid,
        Err(failure) => return failure,
    };
    IO_PRIORITY
        .lock()
        .iter()
        .find(|(owner, _)| *owner == pid)
        .map_or(IOPRIO_DEFAULT, |(_, value)| *value)
        .into()
}

/// One memory node: every policy is satisfied by node 0.
fn mempolicy_mode_valid(mode: u64) -> bool {
    let flags = 0x8000 | 0x4000 | 0x2000;
    mode & !flags <= 6
}

fn get_mempolicy(mode: u64, nodemask: u64, maxnode: u64, _address: u64, flags: u64) -> u64 {
    const MPOL_F_NODE: u64 = 1;
    const MPOL_F_ADDR: u64 = 2;
    const MPOL_F_MEMS_ALLOWED: u64 = 4;
    if flags & !(MPOL_F_NODE | MPOL_F_ADDR | MPOL_F_MEMS_ALLOWED) != 0 {
        return error(EINVAL);
    }
    if mode != 0 && !user::copy_to_user(mode, &0i32.to_le_bytes()) {
        return error(14);
    }
    if nodemask != 0 && maxnode != 0 {
        let bytes = maxnode.div_ceil(64).min(16) as usize * 8;
        let mut mask = [0u8; 128];
        if flags & MPOL_F_MEMS_ALLOWED != 0 {
            mask[0] = 1;
        }
        if !user::copy_to_user(nodemask, &mask[..bytes]) {
            return error(14);
        }
    }
    0
}

fn move_pages(count: u64, nodes: u64, status: u64) -> u64 {
    for index in 0..count.min(1 << 20) {
        let mut node = [0u8; 4];
        if nodes != 0 && !user::copy_from_user(nodes + index * 4, &mut node) {
            return error(14);
        }
        let result: i32 = if i32::from_le_bytes(node) == 0 {
            0
        } else {
            -19
        };
        if status != 0 && !user::copy_to_user(status + index * 4, &result.to_le_bytes()) {
            return error(14);
        }
    }
    0
}

/// `adjtimex`/`clock_adjtime` on CLOCK_REALTIME: reports the clock as free
/// running (unsynchronised, no offset); adjusting it needs CAP_SYS_TIME.
fn adjtimex(address: u64) -> u64 {
    let mut timex = [0u8; 208];
    if !user::copy_from_user(address, &mut timex) {
        return error(14);
    }
    let modes = u32::from_le_bytes(timex[..4].try_into().unwrap_or_default());
    if modes != 0 && !has_capability(crate::capability::CAP_SYS_TIME) {
        return error(EPERM);
    }
    let wall = crate::rtc::unix_nanoseconds();
    let put = |timex: &mut [u8; 208], at: usize, value: i64| {
        timex[at..at + 8].copy_from_slice(&value.to_le_bytes());
    };
    put(&mut timex, 8, 0);
    put(&mut timex, 16, 0);
    put(&mut timex, 24, 16_000_000);
    put(&mut timex, 32, 16_000_000);
    timex[40..44].copy_from_slice(&0x40i32.to_le_bytes());
    put(&mut timex, 48, 2);
    put(&mut timex, 56, 1);
    put(&mut timex, 64, 32_768_000);
    put(&mut timex, 72, (wall / 1_000_000_000) as i64);
    put(&mut timex, 80, ((wall % 1_000_000_000) / 1000) as i64);
    put(&mut timex, 88, 10_000);
    if !user::copy_to_user(address, &timex) {
        return error(14);
    }
    5
}

fn process_madvise(iovec: u64, count: u64, flags: u64) -> u64 {
    if flags != 0 || count > 1024 {
        return error(EINVAL);
    }
    let mut total = 0u64;
    for index in 0..count {
        let mut entry = [0u8; 16];
        if !user::copy_from_user(iovec + index * 16, &mut entry) {
            return error(14);
        }
        total = total.saturating_add(u64::from_le_bytes(
            entry[8..].try_into().unwrap_or_default(),
        ));
    }
    total
}

fn fchmodat2(directory: u64, path: u64, mode: u64, flags: u64) -> u64 {
    const AT_SYMLINK_NOFOLLOW: u64 = 0x100;
    const AT_EMPTY_PATH: u64 = 0x1000;
    if flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH) != 0 {
        return error(EINVAL);
    }
    let mut first = [0u8; 1];
    if flags & AT_EMPTY_PATH != 0 && user::copy_from_user(path, &mut first) && first[0] == 0 {
        return linux_fchmod(directory, mode);
    }
    if flags & AT_SYMLINK_NOFOLLOW != 0 {
        let mut resolved = [0u8; MAX_PATH];
        let length = match resolve_user_path(directory, path, &mut resolved) {
            Ok((length, _)) => length,
            Err(failure) => return failure,
        };
        if let Ok(path) = core::str::from_utf8(&resolved[..length])
            && vfs::symlink_metadata(path).is_ok_and(|meta| meta.mode & 0o170000 == 0o120000)
        {
            return error(EOPNOTSUPP);
        }
    }
    linux_chmodat(directory, path, mode, 0)
}

/// Extended attributes: the filesystems keep none, so a lookup finds no
/// attribute, a listing is empty and setting one is not supported.
fn xattr(number: u64) -> u64 {
    match number {
        191..=193 | 464 => error(ENODATA),
        194..=196 | 465 => 0,
        197 | 198 | 466 => error(ENODATA),
        _ => error(EOPNOTSUPP),
    }
}

/// `memfd_secret`: anonymous memory behind a descriptor, like `memfd_create`
/// (AerOS has no kernel-wide mapping of user memory to take it out of).
fn memfd_secret(flags: u64) -> u64 {
    const O_CLOEXEC: u64 = 0o2_000_000;
    if flags & !O_CLOEXEC != 0 {
        return error(EINVAL);
    }
    match create_memfd(flags & O_CLOEXEC != 0) {
        Ok(descriptor) => descriptor,
        Err(failure) => error(failure),
    }
}

/// `process_mrelease(pidfd, flags)`: a process's memory is given back as it
/// exits, so there is nothing left to hurry; a live one is refused as on
/// Linux.
fn process_mrelease(descriptor: u64, flags: u64) -> u64 {
    if flags != 0 {
        return error(EINVAL);
    }
    match pidfd_target(descriptor) {
        Ok(pid) if crate::scheduler::task_exists(pid) => error(EINVAL),
        Ok(_) => 0,
        Err(failure) => failure,
    }
}

/// `cachestat(fd, range, cstat, flags)`: file data lives in memory once
/// read, so every page of the range inside the file counts as cached.
fn cachestat(descriptor: u64, range: u64, out: u64, flags: u64) -> u64 {
    const SEEK_SET: u64 = 0;
    const SEEK_CUR: u64 = 1;
    const SEEK_END: u64 = 2;
    if flags != 0 {
        return error(EINVAL);
    }
    let mut request = [0u8; 16];
    if !user::copy_from_user(range, &mut request) {
        return error(14);
    }
    let offset = u64::from_le_bytes(request[..8].try_into().unwrap_or([0; 8]));
    let length = u64::from_le_bytes(request[8..].try_into().unwrap_or([0; 8]));
    let here = linux_lseek(descriptor, 0, SEEK_CUR);
    if (here as i64) < 0 {
        return if here == error(29) { error(95) } else { here };
    }
    let size = linux_lseek(descriptor, 0, SEEK_END);
    let _ = linux_lseek(descriptor, here, SEEK_SET);
    if (size as i64) < 0 {
        return size;
    }
    let end = if length == 0 { size } else { offset.saturating_add(length).min(size) };
    let pages = end.saturating_sub(offset).div_ceil(4096);
    let mut answer = [0u8; 40];
    answer[..8].copy_from_slice(&pages.to_le_bytes());
    if !user::copy_to_user(out, &answer) {
        return error(14);
    }
    0
}

/// Handles the calls `dispatch_linux` has no arm for; `None` means unknown.
pub(super) fn dispatch(number: u64, a: [u64; 6]) -> Option<u64> {
    let result = match number {
        159 => adjtimex(a[0]),
        206 => aio::setup(a[0], a[1]),
        207 => aio::destroy(a[0]),
        208 | 333 => aio::get_events(a[0], a[1], a[2], a[3]),
        209 => aio::submit(a[0], a[1], a[2]),
        210 => aio::cancel(a[0]),
        240 => mq::open(a[0], a[1], a[2], a[3]),
        241 => mq::unlink(a[0]),
        242 => mq::send(a[0], a[1], a[2], a[3], a[4]),
        243 => mq::receive(a[0], a[1], a[2], a[3], a[4]),
        244 => mq::notify(a[0], a[1]),
        245 => mq::get_set_attributes(a[0], a[1], a[2]),
        248 => keys::add_key(a[0], a[1], a[2], a[3], a[4]),
        249 => keys::request_key(a[0], a[1]),
        250 => keys::keyctl(a[0], a),
        298 => perf::open(a[0], a[1], a[2], a[3], a[4]),
        447 => memfd_secret(a[0]),
        448 => process_mrelease(a[0], a[1]),
        451 => cachestat(a[0], a[1], a[2], a[3]),
        305 => {
            if a[0] != 0 {
                error(EOPNOTSUPP)
            } else {
                adjtimex(a[1])
            }
        }
        170 => set_uts(&HOSTNAME, a[0], a[1]),
        171 => set_uts(&DOMAINNAME, a[0], a[1]),
        188..=198 | 463..=466 => xattr(number),
        237 => {
            if mempolicy_mode_valid(a[2]) {
                0
            } else {
                error(EINVAL)
            }
        }
        238 => {
            if mempolicy_mode_valid(a[0]) {
                0
            } else {
                error(EINVAL)
            }
        }
        239 => get_mempolicy(a[0], a[1], a[2], a[3], a[4]),
        251 => ioprio_set(a[0], a[1], a[2]),
        252 => ioprio_get(a[0], a[1]),
        256 => 0,
        279 => move_pages(a[1], a[3], a[4]),
        303 | 304 => error(EOPNOTSUPP),
        329 => {
            if a[3] as i32 == -1 {
                linux_mprotect(a[0], a[1], a[2])
            } else {
                error(EINVAL)
            }
        }
        330 => error(ENOSPC),
        331 => error(EINVAL),
        440 => process_madvise(a[1], a[2], a[4]),
        444..=446 => error(EOPNOTSUPP),
        449 => futex::waitv(a[0], a[1], a[2], a[3], a[4]),
        450 => {
            if a[2] == 0 {
                0
            } else {
                error(EINVAL)
            }
        }
        452 => fchmodat2(a[0], a[1], a[2], a[3]),
        454 => futex::wake2(a[0], a[1], a[2], a[3]),
        455 => futex::wait2(a[0], a[1], a[2], a[3], a[4], a[5]),
        456 => futex::requeue2(a[0], a[1], a[2], a[3]),
        _ if PRIVILEGED.contains(&number) => error(EPERM),
        _ if ABSENT.contains(&number) => error(ENOSYS),
        _ => return None,
    };
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    Some(result)
}
