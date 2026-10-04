use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::user;
use crate::sync::TicketLock;
use crate::vfs::{self, VfsError};

pub const SYS_EXIT: u64 = 0;
pub const SYS_ABI_VERSION: u64 = 1;
pub const SYS_FORK: u64 = 2;
pub const SYS_EXECVE: u64 = 3;
pub const SYS_WAIT4: u64 = 4;
pub const SYS_KILL: u64 = 5;
pub const SYS_GETPID: u64 = 6;
pub const SYS_GETPPID: u64 = 7;
pub const SYS_SBRK: u64 = 8;
pub const SYS_WRITE: u64 = 9;
pub const SYS_GETRANDOM: u64 = 10;
/// execve by VFS path (rdi = length, path bytes packed in rsi..r9).
pub const SYS_EXEC_PATH: u64 = 11;
pub const ABI_VERSION: u64 = 0x0001_0000;
pub const EXEC_PROGRAM_MAX: usize = 40;

const LINUX_READ: u64 = 0;
const LINUX_WRITE: u64 = 1;
const LINUX_OPEN: u64 = 2;
const LINUX_CLOSE: u64 = 3;
const LINUX_STAT: u64 = 4;
const LINUX_DUP: u64 = 32;
const LINUX_DUP2: u64 = 33;
const LINUX_FSTAT: u64 = 5;
const LINUX_POLL: u64 = 7;
const LINUX_LSEEK: u64 = 8;
const LINUX_MMAP: u64 = 9;
const LINUX_MPROTECT: u64 = 10;
const LINUX_MUNMAP: u64 = 11;
const LINUX_BRK: u64 = 12;
const LINUX_RT_SIGACTION: u64 = 13;
const LINUX_RT_SIGPROCMASK: u64 = 14;
const LINUX_IOCTL: u64 = 16;
const LINUX_PREAD64: u64 = 17;
const LINUX_PWRITE64: u64 = 18;
const LINUX_PIPE: u64 = 22;
const LINUX_PIPE2: u64 = 293;
const LINUX_MINCORE: u64 = 27;
const LINUX_FLOCK: u64 = 73;
const LINUX_FCHDIR: u64 = 81;
const LINUX_PERSONALITY: u64 = 135;
const LINUX_SCHED_SETSCHEDULER: u64 = 144;
const LINUX_SYNC: u64 = 162;
const LINUX_FALLOCATE: u64 = 285;
const LINUX_PREADV: u64 = 295;
const LINUX_PWRITEV: u64 = 296;
const LINUX_SYNCFS: u64 = 306;
const LINUX_MEMBARRIER: u64 = 324;
const LINUX_CLOSE_RANGE: u64 = 436;
const LINUX_EVENTFD: u64 = 284;
const LINUX_SENDMSG: u64 = 46;
const LINUX_RECVMSG: u64 = 47;
const LINUX_SENDFILE: u64 = 40;
const LINUX_COPY_FILE_RANGE: u64 = 326;
const LINUX_MEMFD_CREATE: u64 = 319;
const LINUX_EVENTFD2: u64 = 290;
const LINUX_TIMERFD_CREATE: u64 = 283;
const LINUX_TIMERFD_SETTIME: u64 = 286;
const LINUX_TIMERFD_GETTIME: u64 = 287;
const LINUX_EPOLL_CREATE1: u64 = 291;
const LINUX_EPOLL_CTL: u64 = 233;
const LINUX_EPOLL_WAIT: u64 = 232;
const LINUX_READV: u64 = 19;
const LINUX_WRITEV: u64 = 20;
const LINUX_ACCESS: u64 = 21;
const LINUX_MSYNC: u64 = 26;
const LINUX_MADVISE: u64 = 28;
const LINUX_SCHED_YIELD: u64 = 24;
const LINUX_PAUSE: u64 = 34;
const LINUX_NANOSLEEP: u64 = 35;
const LINUX_GETITIMER: u64 = 36;
const LINUX_ALARM: u64 = 37;
const LINUX_SETITIMER: u64 = 38;
const LINUX_GETPID: u64 = 39;
const LINUX_SOCKET: u64 = 41;
const LINUX_CONNECT: u64 = 42;
const LINUX_ACCEPT: u64 = 43;
const LINUX_SENDTO: u64 = 44;
const LINUX_RECVFROM: u64 = 45;
const LINUX_BIND: u64 = 49;
const LINUX_LISTEN: u64 = 50;
const LINUX_SOCKETPAIR: u64 = 53;
const LINUX_ACCEPT4: u64 = 288;
const LINUX_UNAME: u64 = 63;
const LINUX_GETTIMEOFDAY: u64 = 96;
const LINUX_FCNTL: u64 = 72;
const LINUX_FSYNC: u64 = 74;
const LINUX_FDATASYNC: u64 = 75;
const LINUX_FTRUNCATE: u64 = 77;
const LINUX_GETDENTS64: u64 = 217;
const LINUX_READLINK: u64 = 89;
const LINUX_GETCWD: u64 = 79;
const LINUX_CHDIR: u64 = 80;
const LINUX_RENAME: u64 = 82;
const LINUX_MKDIR: u64 = 83;
const LINUX_RMDIR: u64 = 84;
const LINUX_UNLINK: u64 = 87;
const LINUX_CHMOD: u64 = 90;
const LINUX_FCHMOD: u64 = 91;
const LINUX_CHOWN: u64 = 92;
const LINUX_FCHOWN: u64 = 93;
const LINUX_LCHOWN: u64 = 94;
const LINUX_UMASK: u64 = 95;
const LINUX_SYSINFO: u64 = 99;
const LINUX_GETUID: u64 = 102;
const LINUX_GETGID: u64 = 104;
const LINUX_GETEUID: u64 = 107;
const LINUX_GETEGID: u64 = 108;
const LINUX_GETPPID: u64 = 110;
const LINUX_SIGALTSTACK: u64 = 131;
const LINUX_ARCH_PRCTL: u64 = 158;
const LINUX_EXIT: u64 = 60;
const LINUX_GETTID: u64 = 186;
const LINUX_TKILL: u64 = 200;
const LINUX_TIME: u64 = 201;
const LINUX_FUTEX: u64 = 202;
const LINUX_PTRACE: u64 = 101;
const LINUX_SCHED_GETAFFINITY: u64 = 204;
const LINUX_SET_TID_ADDRESS: u64 = 218;
const LINUX_EXIT_GROUP: u64 = 231;
const LINUX_TGKILL: u64 = 234;
const LINUX_CLOCK_GETTIME: u64 = 228;
const LINUX_CLOCK_GETRES: u64 = 229;
const LINUX_CLOCK_NANOSLEEP: u64 = 230;
const LINUX_OPENAT: u64 = 257;
const LINUX_MKDIRAT: u64 = 258;
const LINUX_NEWFSTATAT: u64 = 262;
const LINUX_UNLINKAT: u64 = 263;
const LINUX_RENAMEAT: u64 = 264;
const LINUX_READLINKAT: u64 = 267;
const LINUX_FCHMODAT: u64 = 268;
const LINUX_FCHOWNAT: u64 = 260;
const LINUX_SETTIMEOFDAY: u64 = 164;
const LINUX_MOUNT: u64 = 165;
const LINUX_REBOOT: u64 = 169;
const LINUX_CLOCK_SETTIME: u64 = 227;
const LINUX_UMOUNT2: u64 = 166;
const LINUX_SET_ROBUST_LIST: u64 = 273;
const LINUX_GET_ROBUST_LIST: u64 = 274;
const LINUX_SETPGID: u64 = 109;
const LINUX_GETPGID: u64 = 121;
const LINUX_SETSID: u64 = 112;
const LINUX_GETSID: u64 = 124;
const LINUX_DUP3: u64 = 292;
const LINUX_PRLIMIT64: u64 = 302;
const LINUX_RENAMEAT2: u64 = 316;
const LINUX_GETRANDOM: u64 = 318;
const LINUX_RSEQ: u64 = 334;
const LINUX_GETCPU: u64 = 309;
const LINUX_STATX: u64 = 332;
const LINUX_FACCESSAT2: u64 = 439;
const LINUX_LSTAT: u64 = 6;
const LINUX_CREAT: u64 = 85;
const LINUX_SYMLINK: u64 = 88;
const LINUX_GETRLIMIT: u64 = 97;
const LINUX_GETPGRP: u64 = 111;
const LINUX_GETGROUPS: u64 = 115;
const LINUX_GETRESUID: u64 = 118;
const LINUX_GETRESGID: u64 = 120;
const LINUX_SYMLINKAT: u64 = 266;
const LINUX_STATFS: u64 = 137;
const LINUX_FSTATFS: u64 = 138;
const LINUX_SELECT: u64 = 23;
const LINUX_PSELECT6: u64 = 270;
const LINUX_FACCESSAT: u64 = 269;
const LINUX_EPOLL_CREATE: u64 = 213;
const LINUX_EPOLL_PWAIT: u64 = 281;
const LINUX_UTIMENSAT: u64 = 280;
const LINUX_FADVISE64: u64 = 221;
const LINUX_MLOCK: u64 = 149;
const LINUX_MUNLOCK: u64 = 150;
const LINUX_MLOCKALL: u64 = 151;
const LINUX_MUNLOCKALL: u64 = 152;
const LINUX_SETUID: u64 = 105;
const LINUX_SETGID: u64 = 106;
const LINUX_SETGROUPS: u64 = 116;
const LINUX_CHROOT: u64 = 161;
const LINUX_GETPRIORITY: u64 = 140;
const LINUX_SETPRIORITY: u64 = 141;
const LINUX_SCHED_GET_PRIORITY_MAX: u64 = 146;
const LINUX_SCHED_GET_PRIORITY_MIN: u64 = 147;
const LINUX_CAPGET: u64 = 125;
const LINUX_CAPSET: u64 = 126;
const LINUX_SCHED_SETAFFINITY: u64 = 203;
const CAPABILITY_VERSION_3: u32 = 0x2008_0522;
const LINUX_SHUTDOWN: u64 = 48;
const LINUX_GETSOCKNAME: u64 = 51;
const LINUX_GETPEERNAME: u64 = 52;
const LINUX_SETSOCKOPT: u64 = 54;
const LINUX_GETSOCKOPT: u64 = 55;
const SOL_SOCKET: u64 = 1;
const SO_TYPE: u64 = 3;
const SO_ERROR: u64 = 4;
const SO_ACCEPTCONN: u64 = 30;
const LINUX_TRUNCATE: u64 = 76;
const LINUX_GETRUSAGE: u64 = 98;
const LINUX_TIMES: u64 = 100;
const LINUX_SCHED_GETPARAM: u64 = 143;
const LINUX_SCHED_GETSCHEDULER: u64 = 145;
const LINUX_PRCTL: u64 = 157;
const LINUX_PPOLL: u64 = 271;
const PR_SET_NAME: u64 = 15;
const PR_GET_NAME: u64 = 16;
const PR_SET_DUMPABLE: u64 = 4;
const PR_GET_DUMPABLE: u64 = 3;
const PR_SET_NO_NEW_PRIVS: u64 = 38;
const PR_GET_NO_NEW_PRIVS: u64 = 39;
const CLOCK_TICKS_PER_SECOND: u64 = 100;
const AT_FDCWD: i64 = -100;
const AT_SYMLINK_NOFOLLOW: u64 = 0x100;
const AT_EMPTY_PATH: u64 = 0x1000;
const AT_REMOVEDIR: u64 = 0x200;
const RENAME_NOREPLACE: u64 = 1;
const O_CLOEXEC: u64 = 0x80000;
const O_DIRECTORY: u64 = 0x10000;
const O_CREAT: u64 = 0x40;
const O_EXCL: u64 = 0x80;
const O_TRUNC: u64 = 0x200;
const O_APPEND: u64 = 0x400;
const O_LARGEFILE: u64 = 0x8000;
const MAP_PRIVATE: u64 = 0x02;
const MAP_ANONYMOUS: u64 = 0x20;
const MAP_STACK: u64 = 0x20000;
const MAX_PATH: usize = 256;
mod futex;
mod ptrace;

pub use ptrace::Trap;
mod tcpsock;

const IO_CHUNK: usize = 256;
const MAX_IO: usize = 64 * 1024;
const PROCESS_FD_COUNT: usize = 24;
const SOCKET_COUNT: usize = 8;
const MAX_DATAGRAM: usize = 1472;

#[derive(Clone, Copy)]
pub(crate) struct ProcessFd {
    handle: u32,
    standard: u8,
    open: bool,
    close_on_exec: bool,
    socket: bool,
    pipe: bool,
    /// An `epoll_create1()` instance; `handle` indexes `EPOLL_INSTANCES`
    /// instead of a pipe/socket/VFS handle.
    epoll: bool,
    /// One end of a `socketpair(AF_UNIX, SOCK_STREAM, ...)` pair; `handle`
    /// indexes `UNIX_PAIRS` instead of a pipe/socket/VFS handle. Unlike a
    /// pipe's read/write ends (distinguished via `readable`/`writable`,
    /// since each end only goes one direction), both ends of a unix stream
    /// pair are readable AND writable, so `unix_end_b` is what tells them
    /// apart.
    unix_socket: bool,
    unix_end_b: bool,
    /// A `socket(AF_UNIX, SOCK_STREAM, 0)` that has been `bind()` + `listen()`
    /// ed; `handle` indexes `UNIX_LISTENERS` instead of a pair/pipe/socket/VFS
    /// handle. A plain `unix_socket` fd with `handle == UNIX_UNBOUND` is the
    /// other pre-connection state: freshly `socket()`ed, not yet bound or
    /// connected - `connect()` and `bind()` are the only two things that
    /// accept it.
    unix_listener: bool,
    readable: bool,
    writable: bool,
    append: bool,
    /// An anonymous `memfd_create` file: a hidden `/tmp` file removed when
    /// its last descriptor closes.
    memfd: bool,
    /// A TCP socket; `handle` indexes the table in `tcpnet`.
    tcp: bool,
    nonblocking: bool,
    /// An `AF_INET6` socket (dual stack); selects the sockaddr layout.
    inet6: bool,
}

impl ProcessFd {
    const EMPTY: Self = Self {
        handle: 0,
        standard: 0,
        open: false,
        close_on_exec: false,
        socket: false,
        pipe: false,
        epoll: false,
        unix_socket: false,
        unix_end_b: false,
        unix_listener: false,
        readable: false,
        writable: false,
        append: false,
        memfd: false,
        tcp: false,
        nonblocking: false,
        inet6: false,
    };

    const fn standard(kind: u8, readable: bool, writable: bool) -> Self {
        Self {
            handle: 0,
            standard: kind,
            open: true,
            close_on_exec: false,
            socket: false,
            pipe: false,
            epoll: false,
            unix_socket: false,
            unix_end_b: false,
            unix_listener: false,
            readable,
            writable,
            append: false,
            memfd: false,
            tcp: false,
            nonblocking: false,
            inet6: false,
        }
    }
}

const fn initial_process_fds() -> [ProcessFd; PROCESS_FD_COUNT] {
    let mut descriptors = [ProcessFd::EMPTY; PROCESS_FD_COUNT];
    descriptors[0] = ProcessFd::standard(1, true, false);
    descriptors[1] = ProcessFd::standard(2, false, true);
    descriptors[2] = ProcessFd::standard(3, false, true);
    descriptors
}

static PROCESS_FDS: TicketLock<[ProcessFd; PROCESS_FD_COUNT]> =
    TicketLock::new(initial_process_fds());

#[derive(Clone, Copy)]
struct CurrentDirectory {
    bytes: [u8; MAX_PATH],
    length: usize,
}

impl CurrentDirectory {
    const ROOT: Self = {
        let mut bytes = [0; MAX_PATH];
        bytes[0] = b'/';
        Self { bytes, length: 1 }
    };
}

static CURRENT_DIRECTORY: TicketLock<CurrentDirectory> = TicketLock::new(CurrentDirectory::ROOT);

#[derive(Clone, Copy)]
struct UdpSocket {
    open: bool,
    connected: bool,
    local_port: u16,
    remote_port: u16,
    remote: crate::ip::Address,
    /// This socket's slot in the `udp` port table.
    binding: usize,
    options: crate::sockopt::Options,
}

impl UdpSocket {
    const EMPTY: Self = Self {
        open: false,
        connected: false,
        local_port: 0,
        remote_port: 0,
        remote: [0; 16],
        binding: 0,
        options: crate::sockopt::Options::DEFAULT,
    };
}

static SOCKETS: TicketLock<[UdpSocket; SOCKET_COUNT]> =
    TicketLock::new([UdpSocket::EMPTY; SOCKET_COUNT]);

const PIPE_COUNT: usize = 8;
const PIPE_BUFFER_BYTES: usize = 4096;

/// A minimal, deliberately non-blocking pipe: `pipe_read`/`pipe_write`
/// return EAGAIN immediately rather than parking the caller when the
/// buffer is empty/full. A real blocking implementation would need a wait
/// queue integrated with the scheduler; until that exists, non-blocking is
/// the safe choice (callers that want to wait can already `poll()`).
/// Each end IS reference-counted across `dup`/`fork` now (see
/// `remove_process_fd`'s same-process scan plus
/// `scheduler::any_other_task_shares_fd`'s cross-process one, both feeding
/// into `same_open_description`, which already distinguishes a pipe's two
/// ends via `readable`) - `read_open`/`write_open` here only flip once no
/// live fd anywhere still references that end.
#[derive(Clone, Copy)]
struct Pipe {
    used: bool,
    data: [u8; PIPE_BUFFER_BYTES],
    head: usize,
    length: usize,
    read_open: bool,
    write_open: bool,
    /// An `eventfd` counter instead of a byte stream; `counter` replaces
    /// `data`/`length` and the one descriptor is both read and write end.
    event: bool,
    semaphore: bool,
    counter: u64,
    /// A `timerfd`: `counter` holds expirations not yet read, `timer_next` is
    /// the next expiry on the monotonic clock (0 when disarmed).
    timer: bool,
    timer_next: u64,
    timer_interval: u64,
}

impl Pipe {
    const EMPTY: Self = Self {
        used: false,
        data: [0; PIPE_BUFFER_BYTES],
        head: 0,
        length: 0,
        read_open: false,
        write_open: false,
        event: false,
        semaphore: false,
        counter: 0,
        timer: false,
        timer_next: 0,
        timer_interval: 0,
    };
}

static PIPES: TicketLock<[Pipe; PIPE_COUNT]> = TicketLock::new([Pipe::EMPTY; PIPE_COUNT]);

const UNIX_PAIR_COUNT: usize = 8;

#[derive(Clone, Copy)]
struct UnixBuffer {
    data: [u8; PIPE_BUFFER_BYTES],
    head: usize,
    length: usize,
}

impl UnixBuffer {
    const EMPTY: Self = Self {
        data: [0; PIPE_BUFFER_BYTES],
        head: 0,
        length: 0,
    };
}

/// A connected `AF_UNIX`/`SOCK_STREAM` pair from `socketpair()`: two
/// independent byte-stream buffers (one per direction), each shaped exactly
/// like `Pipe`'s. Unlike a pipe, both ends can read AND write, so instead of
/// one buffer with a `read_open`/`write_open` pair, there are two buffers
/// (`a_to_b`, `b_to_a`) and one open flag per *end* (`end_a_open`,
/// `end_b_open`) - refcounted across `dup`/`fork` exactly like `Pipe`, via
/// `same_open_description`'s `unix_end_b` comparison.
#[derive(Clone, Copy)]
struct UnixPair {
    used: bool,
    a_to_b: UnixBuffer,
    b_to_a: UnixBuffer,
    end_a_open: bool,
    end_b_open: bool,
}

impl UnixPair {
    const EMPTY: Self = Self {
        used: false,
        a_to_b: UnixBuffer::EMPTY,
        b_to_a: UnixBuffer::EMPTY,
        end_a_open: false,
        end_b_open: false,
    };
}

static UNIX_PAIRS: TicketLock<[UnixPair; UNIX_PAIR_COUNT]> =
    TicketLock::new([UnixPair::EMPTY; UNIX_PAIR_COUNT]);

/// A `unix_socket` fd not yet connected to a pair, nor bound to a listener -
/// the state `socket(AF_UNIX, SOCK_STREAM, 0)` returns.
const UNIX_UNBOUND: u32 = u32::MAX;
const UNIX_LISTENER_COUNT: usize = 4;
const UNIX_LISTENER_BACKLOG: usize = 4;
/// Matches `sizeof(sockaddr_un.sun_path)` on real Linux, though there's no
/// filesystem entry behind it here - `bind()`/`connect()` just match this
/// byte string against an in-kernel table (closer to Linux's "abstract
/// namespace" sockets, which also never touch the filesystem, than to a
/// real path-backed one). Real path-backed `AF_UNIX` binding would need the
/// VFS to understand a new "socket" node type; out of scope here, the same
/// way `AF_UNIX` only gets `socketpair()`'s anonymous pairs otherwise.
const UNIX_NAME_CAP: usize = 108;

#[derive(Clone, Copy)]
struct UnixListener {
    used: bool,
    listening: bool,
    name: [u8; UNIX_NAME_CAP],
    name_len: u8,
    /// Pending connections: a `UNIX_PAIRS` slot per queued `connect()`,
    /// waiting for this listener's `accept()` to claim it.
    backlog: [Option<usize>; UNIX_LISTENER_BACKLOG],
}

impl UnixListener {
    const EMPTY: Self = Self {
        used: false,
        listening: false,
        name: [0; UNIX_NAME_CAP],
        name_len: 0,
        backlog: [None; UNIX_LISTENER_BACKLOG],
    };

    fn name(&self) -> &[u8] {
        &self.name[..self.name_len as usize]
    }
}

static UNIX_LISTENERS: TicketLock<[UnixListener; UNIX_LISTENER_COUNT]> =
    TicketLock::new([UnixListener::EMPTY; UNIX_LISTENER_COUNT]);

const EPOLL_INSTANCE_COUNT: usize = 8;
const EPOLL_MAX_WATCHES: usize = 32;
/// `EPOLLIN`; the only readiness bit anything here can report (see
/// `linux_poll`'s identical `readable`/`writable` -> bitmask mapping).
const EPOLLIN: u32 = 0x001;
const EPOLLOUT: u32 = 0x004;

#[derive(Clone, Copy)]
struct EpollWatch {
    /// The watched process fd, or -1 for an empty slot.
    fd: i32,
    events: u32,
    /// Opaque `epoll_data_t` the caller gets back verbatim in `epoll_wait`.
    data: u64,
}

impl EpollWatch {
    const EMPTY: Self = Self {
        fd: -1,
        events: 0,
        data: 0,
    };
}

#[derive(Clone, Copy)]
struct EpollInstance {
    used: bool,
    watches: [EpollWatch; EPOLL_MAX_WATCHES],
}

impl EpollInstance {
    const EMPTY: Self = Self {
        used: false,
        watches: [EpollWatch::EMPTY; EPOLL_MAX_WATCHES],
    };
}

static EPOLL_INSTANCES: TicketLock<[EpollInstance; EPOLL_INSTANCE_COUNT]> =
    TicketLock::new([EpollInstance::EMPTY; EPOLL_INSTANCE_COUNT]);

/// How many times a pipe read/write cooperatively yields to another ready
/// task before giving up with EAGAIN. This is deliberately NOT true
/// blocking (parking the task and waking it on a condition) -- this
/// scheduler has no wait-queue/wake-condition infrastructure, and adding
/// one safely is a much bigger, higher-blast-radius change than this
/// session's remaining budget should spend on the single most
/// safety-critical subsystem in the kernel (every fork/exec/scheduling
/// self-test depends on it). Bounded cooperative retry is the safe
/// middle ground: `scheduler::yield_now()` is already a well-tested
/// primitive that no-ops harmlessly when nothing else is ready to run
/// (including before the scheduler is even initialized), so this can
/// only ever shorten the gap where a producer/consumer on another task
/// would otherwise see a spurious EAGAIN, never hang or corrupt state.
const PIPE_WAIT_ITERATIONS: u32 = 64;

/// `yield_now` for code that may run inside a Linux syscall of a user process:
/// other tasks must see the ring-3 GS layout (and not clobber the saved user
/// rsp) while this one is switched out. Kernel-only tasks just yield.
fn interrupted() -> bool {
    check_itimer().is_some()
        || SIGNAL_PENDING.load(Ordering::Acquire) & !SIGNAL_MASK.load(Ordering::Acquire) != 0
}

fn yield_in_syscall() {
    if crate::tcpnet::active() {
        crate::tcpnet::poll();
    }
    if crate::scheduler::current_task_pid_for_linux().is_none() {
        crate::scheduler::yield_now();
        return;
    }
    let saved_user_rsp = user_stack_pointer();
    // SAFETY: paired swapgs around the yield restore the in-syscall GS state.
    unsafe { core::arch::asm!("swapgs", options(nostack, preserves_flags)) };
    crate::scheduler::yield_now();
    unsafe { core::arch::asm!("swapgs", options(nostack, preserves_flags)) };
    set_user_stack_pointer(saved_user_rsp);
}

fn is_eventfd(slot: usize) -> bool {
    PIPES
        .lock()
        .get(slot)
        .is_some_and(|pipe| pipe.used && pipe.event)
}

fn eventfd_read(slot: usize, destination: &mut [u8]) -> Result<usize, u64> {
    if destination.len() < 8 {
        return Err(22);
    }
    for attempt in 0..PIPE_WAIT_ITERATIONS {
        {
            let mut pipes = PIPES.lock();
            let Some(pipe) = pipes.get_mut(slot) else {
                return Err(9);
            };
            if pipe.counter > 0 {
                let value = if pipe.semaphore { 1 } else { pipe.counter };
                pipe.counter -= value;
                destination[..8].copy_from_slice(&value.to_le_bytes());
                return Ok(8);
            }
        }
        if attempt + 1 < PIPE_WAIT_ITERATIONS {
            yield_in_syscall();
        }
    }
    Err(11)
}

fn eventfd_write(slot: usize, source: &[u8]) -> Result<usize, u64> {
    if source.len() < 8 {
        return Err(22);
    }
    let mut encoded = [0u8; 8];
    encoded.copy_from_slice(&source[..8]);
    let value = u64::from_le_bytes(encoded);
    if value == u64::MAX {
        return Err(22);
    }
    for attempt in 0..PIPE_WAIT_ITERATIONS {
        {
            let mut pipes = PIPES.lock();
            let Some(pipe) = pipes.get_mut(slot) else {
                return Err(9);
            };
            if value <= u64::MAX - 1 - pipe.counter {
                pipe.counter += value;
                return Ok(8);
            }
        }
        if attempt + 1 < PIPE_WAIT_ITERATIONS {
            yield_in_syscall();
        }
    }
    Err(11)
}

/// Claims a pipe slot for a descriptor that is one object (eventfd, timerfd)
/// rather than a read/write pair, and installs it. `writable` descriptors
/// own both ends, read-only ones start with the write end already closed so
/// closing the descriptor frees the slot.
fn create_single_fd_pipe(template: Pipe, close_on_exec: bool, writable: bool) -> Result<u64, u64> {
    let slot = {
        let mut pipes = PIPES.lock();
        let Some(slot) = pipes.iter().position(|pipe| !pipe.used) else {
            return Err(24);
        };
        pipes[slot] = Pipe {
            used: true,
            read_open: true,
            write_open: writable,
            ..template
        };
        slot
    };
    install_process_fd_kind(
        slot as u32,
        close_on_exec,
        false,
        true,
        false,
        false,
        false,
        false,
        true,
        writable,
        false,
    )
    .ok_or_else(|| {
        PIPES.lock()[slot] = Pipe::EMPTY;
        24
    })
}

fn create_eventfd(initial: u32, semaphore: bool, close_on_exec: bool) -> Result<u64, u64> {
    let template = Pipe {
        event: true,
        semaphore,
        counter: initial as u64,
        ..Pipe::EMPTY
    };
    create_single_fd_pipe(template, close_on_exec, true)
}

fn is_timerfd(slot: usize) -> bool {
    PIPES
        .lock()
        .get(slot)
        .is_some_and(|pipe| pipe.used && pipe.timer)
}

/// Folds every expiry up to `now` into the unread count.
fn timer_expire(pipe: &mut Pipe, now: u64) {
    if pipe.timer_next == 0 || now < pipe.timer_next {
        return;
    }
    if pipe.timer_interval == 0 {
        pipe.counter = pipe.counter.saturating_add(1);
        pipe.timer_next = 0;
        return;
    }
    let periods = 1 + (now - pipe.timer_next) / pipe.timer_interval;
    pipe.counter = pipe.counter.saturating_add(periods);
    pipe.timer_next = pipe
        .timer_next
        .saturating_add(periods.saturating_mul(pipe.timer_interval));
}

/// `(time until next expiry, interval)` in nanoseconds.
fn timer_remaining(pipe: &mut Pipe, now: u64) -> (u64, u64) {
    timer_expire(pipe, now);
    if pipe.timer_next == 0 {
        return (0, pipe.timer_interval);
    }
    (
        pipe.timer_next.saturating_sub(now).max(1),
        pipe.timer_interval,
    )
}

/// Arms (or with `value == 0` disarms) the timer and clears unread expiries.
fn timer_arm(pipe: &mut Pipe, value: u64, interval: u64, absolute: bool, now: u64) {
    pipe.counter = 0;
    pipe.timer_interval = interval;
    pipe.timer_next = if value == 0 {
        0
    } else if absolute {
        value
    } else {
        now.saturating_add(value).max(1)
    };
}

fn timerfd_read(slot: usize, destination: &mut [u8]) -> Result<usize, u64> {
    if destination.len() < 8 {
        return Err(22);
    }
    loop {
        let next = {
            let mut pipes = PIPES.lock();
            let Some(pipe) = pipes.get_mut(slot) else {
                return Err(9);
            };
            timer_expire(pipe, crate::time::monotonic_nanoseconds());
            if pipe.counter > 0 {
                destination[..8].copy_from_slice(&pipe.counter.to_le_bytes());
                pipe.counter = 0;
                return Ok(8);
            }
            pipe.timer_next
        };
        if next == 0 {
            return Err(11);
        }
        if sleep_until(next).is_some() {
            return Err(4);
        }
    }
}

fn create_timerfd(close_on_exec: bool) -> Result<u64, u64> {
    let template = Pipe {
        timer: true,
        ..Pipe::EMPTY
    };
    create_single_fd_pipe(template, close_on_exec, false)
}

fn linux_timerfd_create(clock: u64, flags: u64) -> u64 {
    const TFD_NONBLOCK: u64 = 0o4000;
    if !matches!(clock, 0 | 1 | 7) || flags & !(TFD_NONBLOCK | O_CLOEXEC) != 0 {
        return error(22);
    }
    match create_timerfd(flags & O_CLOEXEC != 0) {
        Ok(descriptor) => {
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            descriptor
        }
        Err(failure) => error(failure),
    }
}

/// The pipe slot behind a timerfd descriptor.
fn timerfd_slot(descriptor: u64) -> Result<usize, u64> {
    let process_fd = lookup_process_fd(descriptor).ok_or(9u64)?;
    if !process_fd.pipe || !is_timerfd(process_fd.handle as usize) {
        return Err(22);
    }
    Ok(process_fd.handle as usize)
}

fn write_itimerspec(address: u64, value: u64, interval: u64) -> bool {
    let mut encoded = [0u8; 32];
    encoded[..8].copy_from_slice(&(interval / 1_000_000_000).to_le_bytes());
    encoded[8..16].copy_from_slice(&(interval % 1_000_000_000).to_le_bytes());
    encoded[16..24].copy_from_slice(&(value / 1_000_000_000).to_le_bytes());
    encoded[24..].copy_from_slice(&(value % 1_000_000_000).to_le_bytes());
    user::copy_to_user(address, &encoded)
}

fn linux_timerfd_settime(descriptor: u64, flags: u64, new_value: u64, old_value: u64) -> u64 {
    const TFD_TIMER_ABSTIME: u64 = 1;
    if flags & !TFD_TIMER_ABSTIME != 0 {
        return error(22);
    }
    let slot = match timerfd_slot(descriptor) {
        Ok(slot) => slot,
        Err(failure) => return error(failure),
    };
    let mut encoded = [0u8; 32];
    if !user::copy_from_user(new_value, &mut encoded) {
        return error(14);
    }
    let field = |offset: usize| read_array_u64(&encoded, offset);
    if field(8) >= 1_000_000_000 || field(24) >= 1_000_000_000 {
        return error(22);
    }
    let seconds_to_ns = |seconds: u64, nanoseconds: u64| {
        seconds
            .checked_mul(1_000_000_000)
            .and_then(|total| total.checked_add(nanoseconds))
    };
    let (Some(interval), Some(value)) = (
        seconds_to_ns(field(0), field(8)),
        seconds_to_ns(field(16), field(24)),
    ) else {
        return error(22);
    };
    let now = crate::time::monotonic_nanoseconds();
    let previous = {
        let mut pipes = PIPES.lock();
        let pipe = &mut pipes[slot];
        let previous = timer_remaining(pipe, now);
        timer_arm(pipe, value, interval, flags & TFD_TIMER_ABSTIME != 0, now);
        previous
    };
    if old_value != 0 && !write_itimerspec(old_value, previous.0, previous.1) {
        return error(14);
    }
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_timerfd_gettime(descriptor: u64, current: u64) -> u64 {
    let slot = match timerfd_slot(descriptor) {
        Ok(slot) => slot,
        Err(failure) => return error(failure),
    };
    let (value, interval) = {
        let mut pipes = PIPES.lock();
        timer_remaining(&mut pipes[slot], crate::time::monotonic_nanoseconds())
    };
    if !write_itimerspec(current, value, interval) {
        return error(14);
    }
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_eventfd2(initial: u64, flags: u64) -> u64 {
    const EFD_SEMAPHORE: u64 = 1;
    const EFD_NONBLOCK: u64 = 0o4000;
    if flags & !(EFD_SEMAPHORE | EFD_NONBLOCK | O_CLOEXEC) != 0 {
        return error(22);
    }
    match create_eventfd(
        initial as u32,
        flags & EFD_SEMAPHORE != 0,
        flags & O_CLOEXEC != 0,
    ) {
        Ok(descriptor) => {
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            descriptor
        }
        Err(failure) => error(failure),
    }
}

fn pipe_read(slot: usize, destination: &mut [u8]) -> Result<usize, u64> {
    if is_eventfd(slot) {
        return eventfd_read(slot, destination);
    }
    if is_timerfd(slot) {
        return timerfd_read(slot, destination);
    }
    for attempt in 0..PIPE_WAIT_ITERATIONS {
        {
            let mut pipes = PIPES.lock();
            let Some(pipe) = pipes.get_mut(slot) else {
                return Err(9);
            };
            if !pipe.used {
                return Err(9);
            }
            if pipe.length > 0 {
                let count = pipe.length.min(destination.len());
                for (index, slot) in destination.iter_mut().enumerate().take(count) {
                    *slot = pipe.data[(pipe.head + index) % PIPE_BUFFER_BYTES];
                }
                pipe.head = (pipe.head + count) % PIPE_BUFFER_BYTES;
                pipe.length -= count;
                return Ok(count);
            }
            if !pipe.write_open {
                return Ok(0);
            }
        }
        if attempt + 1 < PIPE_WAIT_ITERATIONS {
            yield_in_syscall();
        }
    }
    Err(11)
}

fn pipe_write(slot: usize, source: &[u8]) -> Result<usize, u64> {
    if is_eventfd(slot) {
        return eventfd_write(slot, source);
    }
    if is_timerfd(slot) {
        return Err(22);
    }
    if source.is_empty() {
        let pipes = PIPES.lock();
        let Some(pipe) = pipes.get(slot) else {
            return Err(9);
        };
        if !pipe.used || !pipe.read_open {
            return Err(32);
        }
        return Ok(0);
    }
    for attempt in 0..PIPE_WAIT_ITERATIONS {
        {
            let mut pipes = PIPES.lock();
            let Some(pipe) = pipes.get_mut(slot) else {
                return Err(9);
            };
            if !pipe.used || !pipe.read_open {
                return Err(32);
            }
            let space = PIPE_BUFFER_BYTES - pipe.length;
            if space > 0 {
                let count = source.len().min(space);
                let tail = (pipe.head + pipe.length) % PIPE_BUFFER_BYTES;
                for (index, byte) in source[..count].iter().enumerate() {
                    pipe.data[(tail + index) % PIPE_BUFFER_BYTES] = *byte;
                }
                pipe.length += count;
                return Ok(count);
            }
        }
        if attempt + 1 < PIPE_WAIT_ITERATIONS {
            yield_in_syscall();
        }
    }
    Err(11)
}

/// Mirrors `pipe_read`, but for one end of a `socketpair()` pair: `end_b`
/// selects which of the pair's two directional buffers is this end's
/// "incoming" side (see `UnixPair`'s doc comment).
fn unix_socket_read(slot: usize, end_b: bool, destination: &mut [u8]) -> Result<usize, u64> {
    for attempt in 0..PIPE_WAIT_ITERATIONS {
        {
            let mut pairs = UNIX_PAIRS.lock();
            let Some(pair) = pairs.get_mut(slot) else {
                return Err(9);
            };
            if !pair.used {
                return Err(9);
            }
            let (buffer, peer_open) = if end_b {
                (&mut pair.a_to_b, pair.end_a_open)
            } else {
                (&mut pair.b_to_a, pair.end_b_open)
            };
            if buffer.length > 0 {
                let count = buffer.length.min(destination.len());
                for (index, slot) in destination.iter_mut().enumerate().take(count) {
                    *slot = buffer.data[(buffer.head + index) % PIPE_BUFFER_BYTES];
                }
                buffer.head = (buffer.head + count) % PIPE_BUFFER_BYTES;
                buffer.length -= count;
                return Ok(count);
            }
            if !peer_open {
                return Ok(0);
            }
        }
        if attempt + 1 < PIPE_WAIT_ITERATIONS {
            yield_in_syscall();
        }
    }
    Err(11)
}

/// Mirrors `pipe_write`, but for one end of a `socketpair()` pair.
fn unix_socket_write(slot: usize, end_b: bool, source: &[u8]) -> Result<usize, u64> {
    if source.is_empty() {
        let pairs = UNIX_PAIRS.lock();
        let Some(pair) = pairs.get(slot) else {
            return Err(9);
        };
        let peer_open = if end_b {
            pair.end_a_open
        } else {
            pair.end_b_open
        };
        if !pair.used || !peer_open {
            return Err(32);
        }
        return Ok(0);
    }
    for attempt in 0..PIPE_WAIT_ITERATIONS {
        {
            let mut pairs = UNIX_PAIRS.lock();
            let Some(pair) = pairs.get_mut(slot) else {
                return Err(9);
            };
            let peer_open = if end_b {
                pair.end_a_open
            } else {
                pair.end_b_open
            };
            if !pair.used || !peer_open {
                return Err(32);
            }
            let buffer = if end_b {
                &mut pair.b_to_a
            } else {
                &mut pair.a_to_b
            };
            let space = PIPE_BUFFER_BYTES - buffer.length;
            if space > 0 {
                let count = source.len().min(space);
                let tail = (buffer.head + buffer.length) % PIPE_BUFFER_BYTES;
                for (index, byte) in source[..count].iter().enumerate() {
                    buffer.data[(tail + index) % PIPE_BUFFER_BYTES] = *byte;
                }
                buffer.length += count;
                return Ok(count);
            }
        }
        if attempt + 1 < PIPE_WAIT_ITERATIONS {
            yield_in_syscall();
        }
    }
    Err(11)
}

#[derive(Clone, Copy)]
struct SignalAction {
    handler: u64,
    flags: u64,
    restorer: u64,
    mask: u64,
}

impl SignalAction {
    const EMPTY: Self = Self {
        handler: 0,
        flags: 0,
        restorer: 0,
        mask: 0,
    };
}

#[derive(Clone, Copy)]
struct AlternateStack {
    pointer: u64,
    flags: u32,
    size: u64,
}

impl AlternateStack {
    const EMPTY: Self = Self {
        pointer: 0,
        flags: 2,
        size: 0,
    };
}

static SIGNAL_ACTIONS: TicketLock<[SignalAction; 64]> = TicketLock::new([SignalAction::EMPTY; 64]);
static SIGNAL_MASK: AtomicU64 = AtomicU64::new(0);
/// Signals raised at this process that are waiting for their handler to run
/// (delivered when the next syscall returns, once unblocked).
static SIGNAL_PENDING: AtomicU64 = AtomicU64::new(0);
/// ITIMER_REAL of this process: monotonic-ns deadline (0 = disarmed) and the
/// reload interval. Expiry raises SIGALRM (see `check_itimer`).
static ITIMER_DEADLINE: AtomicU64 = AtomicU64::new(0);
static ITIMER_INTERVAL: AtomicU64 = AtomicU64::new(0);
static ALTERNATE_STACK: TicketLock<AlternateStack> = TicketLock::new(AlternateStack::EMPTY);
const DEFAULT_UMASK: u64 = 0o022;
static UMASK: AtomicU64 = AtomicU64::new(DEFAULT_UMASK);
/// `seccomp(SECCOMP_SET_MODE_STRICT, ...)`: once set, only `read`/`write`/
/// `exit`/`exit_group`/`rt_sigreturn` are permitted for the rest of this
/// process's life (real Linux allows only `exit`, not `exit_group`, in
/// strict mode; `exit_group` is included here too since that's what a real
/// compiled Rust/musl binary actually calls on exit, and excluding it would
/// make the mode unusable for testing against real binaries). There is no
/// `SECCOMP_SET_MODE_FILTER` (BPF) support - deliberately out of scope, the
/// same way AF_UNIX only gets `socketpair()` rather than full bind/listen.
/// Once on, it can never be turned off, matching real seccomp semantics.
static SECCOMP_STRICT: AtomicBool = AtomicBool::new(false);
/// The SECCOMP_SET_MODE_FILTER program of this process, if any. One filter
/// per process: a second install is refused so a sandboxed process can never
/// replace its filter with a looser one. Inherited across fork and exec.
static SECCOMP_FILTER: TicketLock<Option<crate::seccomp::Filter>> = TicketLock::new(None);
static SECCOMP_FILTERED: AtomicBool = AtomicBool::new(false);
/// Effective and permitted capability sets of this process (see
/// capability.rs); inherited across fork and exec.
static CAP_EFFECTIVE: AtomicU64 = AtomicU64::new(crate::capability::ALL);
static CAP_PERMITTED: AtomicU64 = AtomicU64::new(crate::capability::ALL);
/// `RLIMIT_NOFILE`'s current (soft) value: `prlimit64` can only ever lower
/// this (there's no privileged override here, so the hard limit is always
/// `PROCESS_FD_COUNT`) - enforced in `install_process_fd_kind`, the one
/// choke point every fd allocation (pipe, socket, epoll, unix pair/listener,
/// real file) already goes through.
static NOFILE_LIMIT: AtomicU64 = AtomicU64::new(PROCESS_FD_COUNT as u64);

#[derive(Clone, Copy)]
pub struct ProcessState {
    fds: [ProcessFd; PROCESS_FD_COUNT],
    directory: CurrentDirectory,
    signal_actions: [SignalAction; 64],
    signal_mask: u64,
    signal_pending: u64,
    itimer_deadline: u64,
    itimer_interval: u64,
    alternate_stack: AlternateStack,
    umask: u64,
    seccomp_strict: bool,
    seccomp_filter: Option<crate::seccomp::Filter>,
    capabilities: crate::capability::Capabilities,
    nofile_limit: u64,
}

impl ProcessState {
    /// The parts that belong to one thread: its signal mask, pending signals
    /// and alternate stack.
    pub fn copy_thread_from(&mut self, other: &ProcessState) {
        self.signal_mask = other.signal_mask;
        self.signal_pending = other.signal_pending;
        self.alternate_stack = other.alternate_stack;
    }

    /// Everything the threads of a process have in common.
    pub fn copy_shared_from(&mut self, other: &ProcessState) {
        let (mask, pending, alternate) =
            (self.signal_mask, self.signal_pending, self.alternate_stack);
        *self = *other;
        self.signal_mask = mask;
        self.signal_pending = pending;
        self.alternate_stack = alternate;
    }

    pub const EMPTY: Self = Self {
        fds: initial_process_fds(),
        directory: CurrentDirectory::ROOT,
        signal_actions: [SignalAction::EMPTY; 64],
        signal_mask: 0,
        signal_pending: 0,
        itimer_deadline: 0,
        itimer_interval: 0,
        alternate_stack: AlternateStack::EMPTY,
        umask: DEFAULT_UMASK,
        seccomp_strict: false,
        seccomp_filter: None,
        capabilities: crate::capability::Capabilities::FULL,
        nofile_limit: PROCESS_FD_COUNT as u64,
    };
}

/// Lets `scheduler::any_other_task_shares_fd` inspect a stored (not
/// currently live) task's fd table without needing that whole struct
/// public - the fd table is the only part of `ProcessState` any cross-task
/// check needs.
pub(crate) fn process_state_fds(state: &ProcessState) -> &[ProcessFd; PROCESS_FD_COUNT] {
    &state.fds
}

/// Reads the Linux-ABI process state (fd table, cwd, signal state) out of
/// the shared statics every `dispatch_linux` handler above reads and writes
/// directly. Those handlers are untouched by the per-task work below --
/// instead, this snapshot/restore pair is swapped in and out of the shared
/// statics on every scheduler context switch (see `scheduler::yield_now`/
/// `exit_current`/`fork_current_user_task`), exactly like `UserTaskContext`
/// and the FPU state already are, so the statics transparently represent
/// "whichever task is currently running" instead of one process forever.
pub fn save_process_state() -> ProcessState {
    ProcessState {
        fds: *PROCESS_FDS.lock(),
        directory: *CURRENT_DIRECTORY.lock(),
        signal_actions: *SIGNAL_ACTIONS.lock(),
        signal_mask: SIGNAL_MASK.load(Ordering::Acquire),
        signal_pending: SIGNAL_PENDING.load(Ordering::Acquire),
        itimer_deadline: ITIMER_DEADLINE.load(Ordering::Acquire),
        itimer_interval: ITIMER_INTERVAL.load(Ordering::Acquire),
        alternate_stack: *ALTERNATE_STACK.lock(),
        umask: UMASK.load(Ordering::Acquire),
        seccomp_strict: SECCOMP_STRICT.load(Ordering::Acquire),
        seccomp_filter: *SECCOMP_FILTER.lock(),
        capabilities: current_capabilities(),
        nofile_limit: NOFILE_LIMIT.load(Ordering::Acquire),
    }
}

/// Loads one thread's signal state, leaving the shared state as it is.
pub fn restore_thread_state(state: &ProcessState) {
    SIGNAL_MASK.store(state.signal_mask, Ordering::Release);
    SIGNAL_PENDING.store(state.signal_pending, Ordering::Release);
    *ALTERNATE_STACK.lock() = state.alternate_stack;
}

/// What a thread undoes as it goes: the word it asked to have cleared and
/// woken (what `pthread_join` waits on) and its robust futex list.
pub(crate) fn thread_exit_cleanup(hooks: &crate::scheduler::ExitHooks) {
    let group = crate::scheduler::current_group();
    if hooks.robust_list != 0 {
        futex::robust_cleanup(hooks.robust_list, hooks.tid as u32, group);
    }
    if hooks.clear_child_tid != 0 {
        let _ = user::copy_to_user(hooks.clear_child_tid, &0u32.to_le_bytes());
        futex::wake(group, hooks.clear_child_tid, 1, u32::MAX);
    }
}

pub fn restore_process_state(state: &ProcessState) {
    *PROCESS_FDS.lock() = state.fds;
    *CURRENT_DIRECTORY.lock() = state.directory;
    *SIGNAL_ACTIONS.lock() = state.signal_actions;
    SIGNAL_MASK.store(state.signal_mask, Ordering::Release);
    SIGNAL_PENDING.store(state.signal_pending, Ordering::Release);
    ITIMER_DEADLINE.store(state.itimer_deadline, Ordering::Release);
    ITIMER_INTERVAL.store(state.itimer_interval, Ordering::Release);
    UMASK.store(state.umask, Ordering::Release);
    *ALTERNATE_STACK.lock() = state.alternate_stack;
    SECCOMP_STRICT.store(state.seccomp_strict, Ordering::Release);
    SECCOMP_FILTERED.store(state.seccomp_filter.is_some(), Ordering::Release);
    *SECCOMP_FILTER.lock() = state.seccomp_filter;
    set_capabilities(state.capabilities);
    NOFILE_LIMIT.store(state.nofile_limit, Ordering::Release);
}

/// A scheduler task's exit-time cleanup for Linux-ABI process state. This is
/// deliberately NOT `finish_process()`: that also closes every open,
/// non-standard vfs handle, which is correct for the single-shot `run()`
/// path (exactly one process ever exists there). `fork()` copies the fd
/// table by value, so a forked child's fd entries carry the SAME underlying
/// vfs handle numbers as its parent's - `remove_process_fd` now checks
/// `scheduler::any_other_task_shares_fd` before actually releasing a
/// handle, so closing one sibling's copy no longer silently invalidates
/// another still-live sibling's, but `exit_current()` itself still doesn't
/// call any per-fd close logic on the way out. So this function only
/// resets its own in-memory fd/cwd/signal bookkeeping and leaves the
/// underlying vfs handles open (a bounded, documented leak on exit, not a
/// correctness hazard - and now safe to close explicitly if a future
/// change wires real per-fd cleanup into the exit path).
/// Releases every descriptor the current (exiting) user process still holds:
/// real vfs handles, sockets and pipe ends whose last reference this is go
/// back to their pools, while ones a forked relative still shares are left
/// alone (`remove_process_fd` checks the other live tasks). Deliberately
/// doesn't touch the CLOSES statistic - these aren't close() syscalls.
pub fn close_all_process_fds() {
    for descriptor in 0..PROCESS_FD_COUNT as u64 {
        if let Some((process_fd, last_reference)) = remove_process_fd(descriptor)
            && last_reference
        {
            let _ = release_process_fd(process_fd);
        }
    }
}

pub fn reset_scheduled_process_state() {
    *SIGNAL_ACTIONS.lock() = [SignalAction::EMPTY; 64];
    SIGNAL_MASK.store(0, Ordering::Release);
    SIGNAL_PENDING.store(0, Ordering::Release);
    ITIMER_DEADLINE.store(0, Ordering::Release);
    ITIMER_INTERVAL.store(0, Ordering::Release);
    *ALTERNATE_STACK.lock() = AlternateStack::EMPTY;
    *CURRENT_DIRECTORY.lock() = CurrentDirectory::ROOT;
    *PROCESS_FDS.lock() = initial_process_fds();
    UMASK.store(DEFAULT_UMASK, Ordering::Release);
    SECCOMP_STRICT.store(false, Ordering::Release);
    SECCOMP_FILTERED.store(false, Ordering::Release);
    *SECCOMP_FILTER.lock() = None;
    set_capabilities(crate::capability::Capabilities::FULL);
    NOFILE_LIMIT.store(PROCESS_FD_COUNT as u64, Ordering::Release);
}

/// execve() semantics for the process state that survives across an exec:
/// signal handlers reset to default and the alternate signal stack is torn
/// down, close-on-exec file descriptors are closed, but the rest of the fd
/// table and the current directory are preserved -- unlike `finish_process`
/// (a real process exit), this deliberately leaves most of the live state
/// alone.
pub fn exec_reset_process_state() {
    *SIGNAL_ACTIONS.lock() = [SignalAction::EMPTY; 64];
    SIGNAL_PENDING.store(0, Ordering::Release);
    *ALTERNATE_STACK.lock() = AlternateStack::EMPTY;
    let mut descriptors = PROCESS_FDS.lock();
    for descriptor in descriptors.iter_mut() {
        if descriptor.close_on_exec {
            *descriptor = ProcessFd::EMPTY;
        }
    }
}

static CALLS: AtomicU64 = AtomicU64::new(0);
static BOOTSTRAP_CALLS: AtomicU64 = AtomicU64::new(0);
static LINUX_CALLS: AtomicU64 = AtomicU64::new(0);
static EXITS: AtomicU64 = AtomicU64::new(0);
static UNKNOWN: AtomicU64 = AtomicU64::new(0);
static OPENS: AtomicU64 = AtomicU64::new(0);
static READS: AtomicU64 = AtomicU64::new(0);
static WRITES: AtomicU64 = AtomicU64::new(0);
static CLOSES: AtomicU64 = AtomicU64::new(0);
static IO_BYTES: AtomicU64 = AtomicU64::new(0);
static CLOCK_CALLS: AtomicU64 = AtomicU64::new(0);
static RANDOM_CALLS: AtomicU64 = AtomicU64::new(0);
static RANDOM_BYTES: AtomicU64 = AtomicU64::new(0);
static COMPAT_CALLS: AtomicU64 = AtomicU64::new(0);
static TID_ADDRESS: AtomicU64 = AtomicU64::new(0);
static ROBUST_LIST: AtomicU64 = AtomicU64::new(0);
static MEMORY_CALLS: AtomicU64 = AtomicU64::new(0);
static MMAPS: AtomicU64 = AtomicU64::new(0);
static FILE_MMAPS: AtomicU64 = AtomicU64::new(0);
static MPROTECTS: AtomicU64 = AtomicU64::new(0);
static MUNMAPS: AtomicU64 = AtomicU64::new(0);
static METADATA_CALLS: AtomicU64 = AtomicU64::new(0);
static SEEK_CALLS: AtomicU64 = AtomicU64::new(0);
static PATH_CALLS: AtomicU64 = AtomicU64::new(0);
static RESOURCE_CALLS: AtomicU64 = AtomicU64::new(0);
static RSEQ_CALLS: AtomicU64 = AtomicU64::new(0);
static FUTEX_CALLS: AtomicU64 = AtomicU64::new(0);
static FD_CALLS: AtomicU64 = AtomicU64::new(0);
static DUP_CALLS: AtomicU64 = AtomicU64::new(0);
static LAST_OPEN_FD: AtomicU64 = AtomicU64::new(0);
static SIGNAL_CALLS: AtomicU64 = AtomicU64::new(0);
static RUNTIME_CALLS: AtomicU64 = AtomicU64::new(0);
static DIRECTORY_CALLS: AtomicU64 = AtomicU64::new(0);
static SOCKET_CALLS: AtomicU64 = AtomicU64::new(0);
static DATAGRAMS: AtomicU64 = AtomicU64::new(0);
static NETWORK_BYTES: AtomicU64 = AtomicU64::new(0);
static VECTORED_CALLS: AtomicU64 = AtomicU64::new(0);
static POSITIONAL_CALLS: AtomicU64 = AtomicU64::new(0);
static ACCESS_CALLS: AtomicU64 = AtomicU64::new(0);
static STATX_CALLS: AtomicU64 = AtomicU64::new(0);
static WALL_CLOCK_CALLS: AtomicU64 = AtomicU64::new(0);
static SLEEP_CALLS: AtomicU64 = AtomicU64::new(0);
static CHDIR_CALLS: AtomicU64 = AtomicU64::new(0);
static RELATIVE_PATH_CALLS: AtomicU64 = AtomicU64::new(0);
static POLL_CALLS: AtomicU64 = AtomicU64::new(0);
static CREATE_CALLS: AtomicU64 = AtomicU64::new(0);
static RENAME_CALLS: AtomicU64 = AtomicU64::new(0);
static REMOVE_CALLS: AtomicU64 = AtomicU64::new(0);
static SYNC_CALLS: AtomicU64 = AtomicU64::new(0);
static TRUNCATE_CALLS: AtomicU64 = AtomicU64::new(0);
static CHMOD_CALLS: AtomicU64 = AtomicU64::new(0);
static RSEQ_ADDRESS: AtomicU64 = AtomicU64::new(0);

pub enum SyscallResult {
    Return(u64),
    Exit(u64),
}

pub enum BootstrapResult {
    Return(u64),
    Exit(u64),
    Fork,
    Exec {
        bytes: [u8; EXEC_PROGRAM_MAX],
        length: usize,
    },
    ExecPath {
        bytes: [u8; EXEC_PROGRAM_MAX],
        length: usize,
    },
    Wait {
        pid: u64,
    },
    Kill {
        pid: u64,
    },
    CurrentId,
    ParentId,
    Grow {
        pages: u64,
    },
    Write {
        bytes: [u8; EXEC_PROGRAM_MAX],
        length: usize,
    },
}

#[derive(Clone, Copy)]
pub struct SyscallStats {
    pub calls: u64,
    pub bootstrap_calls: u64,
    pub linux_calls: u64,
    pub exits: u64,
    pub unknown: u64,
    pub opens: u64,
    pub reads: u64,
    pub writes: u64,
    pub closes: u64,
    pub io_bytes: u64,
    pub clock_calls: u64,
    pub random_calls: u64,
    pub random_bytes: u64,
    pub compat_calls: u64,
    pub memory_calls: u64,
    pub mmaps: u64,
    pub file_mmaps: u64,
    pub mprotects: u64,
    pub munmaps: u64,
    pub metadata_calls: u64,
    pub seek_calls: u64,
    pub path_calls: u64,
    pub resource_calls: u64,
    pub rseq_calls: u64,
    pub futex_calls: u64,
    pub fd_calls: u64,
    pub dup_calls: u64,
    pub last_open_fd: u64,
    pub signal_calls: u64,
    pub runtime_calls: u64,
    pub directory_calls: u64,
    pub socket_calls: u64,
    pub datagrams: u64,
    pub network_bytes: u64,
    pub vectored_calls: u64,
    pub positional_calls: u64,
    pub access_calls: u64,
    pub statx_calls: u64,
    pub wall_clock_calls: u64,
    pub sleep_calls: u64,
    pub chdir_calls: u64,
    pub relative_path_calls: u64,
    pub poll_calls: u64,
    pub create_calls: u64,
    pub rename_calls: u64,
    pub remove_calls: u64,
    pub sync_calls: u64,
    pub truncate_calls: u64,
    pub chmod_calls: u64,
}

#[repr(C)]
pub struct LinuxSyscallFrame {
    number: u64,
    argument0: u64,
    argument1: u64,
    argument2: u64,
    argument3: u64,
    argument4: u64,
    argument5: u64,
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    rbp: u64,
    rbx: u64,
    user_rip: u64,
    user_rflags: u64,
}

const LINUX_CLONE: u64 = 56;
const LINUX_CLONE3: u64 = 435;
const LINUX_FORK: u64 = 57;
const LINUX_VFORK: u64 = 58;
const LINUX_EXECVE: u64 = 59;
const LINUX_WAIT4: u64 = 61;
const LINUX_KILL: u64 = 62;
const LINUX_RT_SIGRETURN: u64 = 15;
const LINUX_SECCOMP: u64 = 317;
const SECCOMP_SET_MODE_STRICT: u64 = 0;
const SECCOMP_SET_MODE_FILTER: u64 = 1;

const EXEC_STRING_COUNT: usize = 16;
const EXEC_STRING_MAX: usize = 96;

/// Copies a NULL-terminated user array of C strings (argv/envp) into kernel
/// buffers; a null array pointer means an empty list.
fn copy_string_vector(
    array: u64,
    store: &mut [[u8; EXEC_STRING_MAX]; EXEC_STRING_COUNT],
    lengths: &mut [usize; EXEC_STRING_COUNT],
) -> Result<usize, u64> {
    if array == 0 {
        return Ok(0);
    }
    let mut count = 0;
    loop {
        let mut pointer = [0u8; 8];
        let at = array
            .checked_add(count as u64 * 8)
            .ok_or_else(|| error(14))?;
        if !user::copy_from_user(at, &mut pointer) {
            return Err(error(14));
        }
        let pointer = u64::from_le_bytes(pointer);
        if pointer == 0 {
            return Ok(count);
        }
        if count == EXEC_STRING_COUNT {
            return Err(error(7));
        }
        // One byte is kept for the terminator `copy_string` looks for.
        let length = user::copy_string(pointer, &mut store[count][..EXEC_STRING_MAX - 1])
            .ok_or_else(|| error(7))?;
        lengths[count] = length;
        count += 1;
    }
}

/// The pid the current user process sees: the scheduler's task id for a
/// scheduled process, else the legacy single-process pid.
fn current_process_id() -> u64 {
    crate::scheduler::current_task_pid_for_linux().unwrap_or_else(crate::process::current_pid)
}

/// The thread id: the process id unless the caller is one of several threads.
fn current_thread_id() -> u64 {
    if crate::scheduler::current_task_pid_for_linux().is_some() {
        crate::scheduler::current_tid()
    } else {
        crate::process::current_pid()
    }
}

fn user_stack_pointer() -> u64 {
    let value: u64;
    // SAFETY: inside the syscall entry, GS holds the kernel CPU-local block
    // whose slot 1 is the saved user rsp.
    unsafe {
        core::arch::asm!("mov {}, gs:[8]", out(reg) value, options(nostack, preserves_flags))
    };
    value
}

fn set_user_stack_pointer(value: u64) {
    // SAFETY: same slot as `user_stack_pointer`; restored by the sysret path.
    unsafe { core::arch::asm!("mov gs:[8], {}", in(reg) value, options(nostack, preserves_flags)) };
}

fn fork_snapshot(frame: &LinuxSyscallFrame) -> crate::arch::user::ForkSnapshot {
    crate::arch::user::ForkSnapshot {
        r15: frame.r15,
        r14: frame.r14,
        r13: frame.r13,
        r12: frame.r12,
        r11: frame.user_rflags,
        r10: frame.argument3,
        r9: frame.argument5,
        r8: frame.argument4,
        rdi: frame.argument0,
        rsi: frame.argument1,
        rbp: frame.rbp,
        rdx: frame.argument2,
        rbx: frame.rbx,
        rcx: frame.user_rip,
        rip: frame.user_rip,
        rflags: frame.user_rflags,
        rsp: user_stack_pointer(),
    }
}

fn linux_fork(frame: &LinuxSyscallFrame) -> u64 {
    let snapshot = fork_snapshot(frame);
    crate::scheduler::fork_current_user_task(&snapshot).unwrap_or_else(|| {
        crate::oom::relieve();
        error(12)
    })
}

const CLONE_VM: u64 = 0x100;
const CLONE_FS: u64 = 0x200;
const CLONE_FILES: u64 = 0x400;
const CLONE_SIGHAND: u64 = 0x800;
const CLONE_VFORK: u64 = 0x4000;
const CLONE_THREAD: u64 = 0x1_0000;
const CLONE_SETTLS: u64 = 0x8_0000;
const CLONE_PARENT_SETTID: u64 = 0x10_0000;
const CLONE_CHILD_CLEARTID: u64 = 0x20_0000;
const CLONE_CHILD_SETTID: u64 = 0x100_0000;
const CLONE_NAMESPACES: u64 = 0x7e02_0000;

/// `clone`: a new thread of the running process (`CLONE_THREAD` with the
/// flags a thread library passes), or a new process like `fork` (also what
/// `vfork` and `posix_spawn` ask for, which get a copy-on-write copy instead
/// of a shared one).
fn clone_task(
    frame: &LinuxSyscallFrame,
    flags: u64,
    stack: u64,
    parent_tid: u64,
    child_tid: u64,
    tls: u64,
) -> u64 {
    if flags & CLONE_NAMESPACES != 0 {
        return error(22);
    }
    let snapshot = fork_snapshot(frame);
    if flags & CLONE_THREAD != 0 {
        let needed = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND;
        if flags & needed != needed {
            return error(22);
        }
        let words = [
            (
                flags & (CLONE_PARENT_SETTID | CLONE_CHILD_SETTID) != 0,
                parent_tid,
                CLONE_PARENT_SETTID,
            ),
            (true, child_tid, CLONE_CHILD_SETTID | CLONE_CHILD_CLEARTID),
        ];
        for (check, address, wanted) in words {
            if check && flags & wanted != 0 && !user::range_accessible(address, 4, true) {
                return error(14);
            }
        }
        let request = crate::scheduler::ThreadRequest {
            stack,
            tls: (flags & CLONE_SETTLS != 0).then_some(tls),
            clear_child_tid: if flags & CLONE_CHILD_CLEARTID != 0 {
                child_tid
            } else {
                0
            },
        };
        let Some(tid) = crate::scheduler::clone_thread(&snapshot, &request) else {
            return error(11);
        };
        if flags & CLONE_PARENT_SETTID != 0 {
            let _ = user::copy_to_user(parent_tid, &(tid as u32).to_le_bytes());
        }
        if flags & CLONE_CHILD_SETTID != 0 {
            let _ = user::copy_to_user(child_tid, &(tid as u32).to_le_bytes());
        }
        return tid;
    }
    if flags & CLONE_VM != 0 && flags & CLONE_VFORK == 0 {
        return error(22);
    }
    if flags & 0xff != 17 && flags & 0xff != 0 {
        return error(22);
    }
    if flags & CLONE_PARENT_SETTID != 0 && !user::range_accessible(parent_tid, 4, true) {
        return error(14);
    }
    let mut child = snapshot;
    if stack != 0 {
        child.rsp = stack;
    }
    let Some(id) = crate::scheduler::fork_current_user_task(&child) else {
        crate::oom::relieve();
        return error(12);
    };
    if flags & CLONE_PARENT_SETTID != 0 {
        let _ = user::copy_to_user(parent_tid, &(id as u32).to_le_bytes());
    }
    id
}

/// `clone3`: the same request with its arguments in a structure.
fn clone3_task(frame: &LinuxSyscallFrame, arguments: u64, size: u64) -> u64 {
    if !(64..=4096).contains(&size) {
        return error(22);
    }
    let mut raw = [0u8; 88];
    let known = (size as usize).min(raw.len());
    if !user::copy_from_user(arguments, &mut raw[..known]) {
        return error(14);
    }
    let field = |at: usize| u64::from_le_bytes(raw[at..at + 8].try_into().unwrap_or([0; 8]));
    if known > 64 && (field(64) != 0 || field(72) != 0) {
        return error(22);
    }
    let flags = field(0);
    if flags & 0x1000 != 0 || flags >> 32 != 0 {
        return error(22);
    }
    let stack = match (field(40), field(48)) {
        (0, _) => 0,
        (base, length) => base.wrapping_add(length),
    };
    clone_task(
        frame,
        flags | (field(32) & 0xff),
        stack,
        field(24),
        field(16),
        field(56),
    )
}

/// The `wait4` status of a task that ended with `exit_code`: a death by
/// signal reports the signal number, everything else a normal exit status.
fn wait_status(exit_code: u64) -> u32 {
    match exit_code {
        129..=191 => (exit_code - 128) as u32,
        _ => ((exit_code & 0xff) as u32) << 8,
    }
}

/// Waits for a child to end, or for a task this process traces to stop.
fn linux_wait4(pid: u64, status_address: u64, options: u64) -> u64 {
    const WNOHANG: u64 = 1;
    let group = crate::scheduler::current_group();
    loop {
        let mut candidates = [0u64; 16];
        let count = if pid as i64 == -1 {
            ptrace::candidates(group, &mut candidates)
        } else {
            candidates[0] = pid;
            1
        };
        let mut alive = false;
        for &child in &candidates[..count] {
            let status = match ptrace::poll(group, child) {
                Some(ptrace::Event::Stopped(status)) => Some(status),
                Some(ptrace::Event::Exited(code)) => Some(wait_status(code)),
                None => match crate::scheduler::poll_child(child) {
                    Some(Some(code)) => Some(wait_status(code)),
                    Some(None) => {
                        alive = true;
                        None
                    }
                    None => None,
                },
            };
            if let Some(status) = status {
                if status_address != 0 && !user::copy_to_user(status_address, &status.to_le_bytes())
                {
                    return error(14);
                }
                return child;
            }
        }
        if !alive {
            return error(10);
        }
        if options & WNOHANG != 0 {
            return 0;
        }
        yield_in_syscall();
    }
}

/// Returns true when the process image was replaced (frame now points at
/// the new entry point and stack); the caller must not touch rax then.
fn linux_execve(frame: &mut LinuxSyscallFrame) -> Result<(), u64> {
    let mut buffer = [0u8; MAX_PATH];
    let length = user::copy_string(frame.argument0, &mut buffer).ok_or_else(|| error(14))?;
    let path = core::str::from_utf8(&buffer[..length]).map_err(|_| error(84))?;
    // argv/envp live in the image that is about to be replaced, so they are
    // copied out first (up to 16 strings of 95 bytes each) - before the new
    // image's own bytes are even read, so it does not matter below that a
    // `/home`/`/media` file's bytes only live for the one call that reads
    // them (see `vfs::with_home_file`), unlike the embedded `/bin/*`
    // binaries' `'static` storage.
    let mut arg_store = [[0u8; EXEC_STRING_MAX]; EXEC_STRING_COUNT];
    let mut arg_len = [0usize; EXEC_STRING_COUNT];
    let arg_count = copy_string_vector(frame.argument1, &mut arg_store, &mut arg_len)?;
    let mut env_store = [[0u8; EXEC_STRING_MAX]; EXEC_STRING_COUNT];
    let mut env_len = [0usize; EXEC_STRING_COUNT];
    let env_count = copy_string_vector(frame.argument2, &mut env_store, &mut env_len)?;
    let mut args: [&[u8]; EXEC_STRING_COUNT] = [&[]; EXEC_STRING_COUNT];
    let mut env: [&[u8]; EXEC_STRING_COUNT] = [&[]; EXEC_STRING_COUNT];
    for index in 0..arg_count {
        args[index] = &arg_store[index][..arg_len[index]];
    }
    for index in 0..env_count {
        env[index] = &env_store[index][..env_len[index]];
    }
    let do_exec = |data: &[u8]| -> Option<(u64, u64)> {
        if arg_count == 0 {
            crate::scheduler::exec_current_user_task_path(data, path)
        } else {
            crate::scheduler::exec_current_user_task_path_with(
                data,
                path,
                &args[..arg_count],
                &env[..env_count],
            )
        }
    };
    let result = if crate::datafs::route(path).is_some() {
        vfs::with_home_file(path, |data, _mode| do_exec(data)).map_err(|_| error(2))?
    } else {
        let file = vfs::file(path).map_err(|_| error(2))?;
        do_exec(file.data)
    };
    let (entry, stack_top) = result.ok_or_else(|| error(8))?;
    frame.user_rip = entry;
    set_user_stack_pointer(stack_top);
    Ok(())
}

fn unpack_register_bytes(arguments: &[u64; 6]) -> Option<([u8; EXEC_PROGRAM_MAX], usize)> {
    let length = arguments[0];
    if length == 0 || length as usize > EXEC_PROGRAM_MAX {
        return None;
    }
    let mut bytes = [0u8; EXEC_PROGRAM_MAX];
    let registers = [
        arguments[1],
        arguments[2],
        arguments[3],
        arguments[4],
        arguments[5],
    ];
    for (index, register) in registers.iter().enumerate() {
        bytes[index * 8..index * 8 + 8].copy_from_slice(&register.to_le_bytes());
    }
    Some((bytes, length as usize))
}

pub fn dispatch(number: u64, arguments: [u64; 6]) -> BootstrapResult {
    CALLS.fetch_add(1, Ordering::Relaxed);
    BOOTSTRAP_CALLS.fetch_add(1, Ordering::Relaxed);
    match number {
        SYS_EXIT => {
            EXITS.fetch_add(1, Ordering::Relaxed);
            BootstrapResult::Exit(arguments[0])
        }
        SYS_ABI_VERSION => BootstrapResult::Return(ABI_VERSION),
        SYS_FORK => BootstrapResult::Fork,
        SYS_EXECVE => {
            let Some((bytes, length)) = unpack_register_bytes(&arguments) else {
                return BootstrapResult::Return(error(22));
            };
            BootstrapResult::Exec { bytes, length }
        }
        SYS_EXEC_PATH => {
            let Some((bytes, length)) = unpack_register_bytes(&arguments) else {
                return BootstrapResult::Return(error(22));
            };
            BootstrapResult::ExecPath { bytes, length }
        }
        SYS_WAIT4 => BootstrapResult::Wait { pid: arguments[0] },
        SYS_KILL => BootstrapResult::Kill { pid: arguments[0] },
        SYS_GETPID => BootstrapResult::CurrentId,
        SYS_GETPPID => BootstrapResult::ParentId,
        SYS_SBRK => BootstrapResult::Grow {
            pages: arguments[0],
        },
        SYS_WRITE => {
            let Some((bytes, length)) = unpack_register_bytes(&arguments) else {
                return BootstrapResult::Return(error(22));
            };
            BootstrapResult::Write { bytes, length }
        }
        SYS_GETRANDOM => BootstrapResult::Return(crate::random::next_u64()),
        _ => {
            UNKNOWN.fetch_add(1, Ordering::Relaxed);
            BootstrapResult::Return(error(38))
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn aeros_linux_syscall_dispatch(frame: *mut LinuxSyscallFrame) -> u64 {
    let frame = unsafe { &mut *frame };
    if crate::scheduler::is_traced() {
        return ptrace::traced_syscall(frame);
    }
    linux_syscall(frame)
}

fn linux_syscall(frame: &mut LinuxSyscallFrame) -> u64 {
    CALLS.fetch_add(1, Ordering::Relaxed);
    LINUX_CALLS.fetch_add(1, Ordering::Relaxed);
    let arguments = [
        frame.argument0,
        frame.argument1,
        frame.argument2,
        frame.argument3,
        frame.argument4,
        frame.argument5,
    ];
    crate::trace::record(
        crate::scheduler::current_task_id(),
        frame.number,
        arguments[0],
    );
    if crate::tcpnet::active() {
        crate::tcpnet::poll();
    }
    if SECCOMP_STRICT.load(Ordering::Acquire)
        && !matches!(
            frame.number,
            LINUX_READ | LINUX_WRITE | LINUX_EXIT | LINUX_EXIT_GROUP | LINUX_RT_SIGRETURN
        )
    {
        // Real seccomp strict mode: any other syscall destroys the process
        // outright (SIGKILL), never returning to the caller - checked before
        // any dispatch tier below (including the special-cased fork/kill/
        // wait4/execve handling right below) so nothing can slip through.
        EXITS.fetch_add(1, Ordering::Relaxed);
        crate::audit::record(
            "SECCOMP",
            format_args!(
                "killed pid={} syscall={}",
                crate::scheduler::current_task_id(),
                frame.number
            ),
        );
        user::set_exit_code(128 + 9);
        return 1;
    }
    if SECCOMP_FILTERED.load(Ordering::Acquire) {
        match seccomp_verdict(frame.number, frame.user_rip, &arguments) {
            FilterVerdict::Allow => {}
            FilterVerdict::Errno(errno) => {
                frame.number = error(errno);
                return finish_syscall(frame);
            }
            FilterVerdict::Kill => {
                EXITS.fetch_add(1, Ordering::Relaxed);
                crate::audit::record(
                    "SECCOMP",
                    format_args!(
                        "filter killed pid={} syscall={}",
                        crate::scheduler::current_task_id(),
                        frame.number
                    ),
                );
                user::set_exit_code(128 + 31);
                return 1;
            }
        }
    }
    let saved_user_rsp = user_stack_pointer();
    match frame.number {
        LINUX_FORK | LINUX_VFORK => {
            frame.number = linux_fork(frame);
            return finish_syscall(frame);
        }
        LINUX_CLONE => {
            frame.number = clone_task(
                frame,
                arguments[0],
                arguments[1],
                arguments[2],
                arguments[3],
                arguments[4],
            );
            return finish_syscall(frame);
        }
        LINUX_CLONE3 => {
            frame.number = clone3_task(frame, arguments[0], arguments[1]);
            return finish_syscall(frame);
        }
        LINUX_KILL => {
            let (pid, signal) = (arguments[0], arguments[1]);
            let own = crate::scheduler::current_task_pid_for_linux()
                .unwrap_or_else(crate::process::current_pid);
            let exists = || {
                if crate::scheduler::task_exists(pid) {
                    0
                } else {
                    error(3)
                }
            };
            if signal > 64 {
                frame.number = error(22);
            } else if pid != own && !has_capability(crate::capability::CAP_KILL) {
                frame.number = error(1);
            } else if pid == own {
                SIGNAL_CALLS.fetch_add(1, Ordering::Relaxed);
                if signal == 0 {
                    frame.number = 0;
                } else {
                    match raise_signal(signal) {
                        SyscallResult::Return(value) => frame.number = value,
                        SyscallResult::Exit(code) => {
                            EXITS.fetch_add(1, Ordering::Relaxed);
                            user::set_exit_code(code);
                            return 1;
                        }
                    }
                }
            } else if signal == 0 {
                frame.number = exists();
            } else {
                frame.number = match crate::scheduler::signal_other_task(pid, signal) {
                    Some(RemoteSignal::Ignored | RemoteSignal::Queued) => 0,
                    Some(RemoteSignal::Fatal) => match crate::scheduler::kill_task(pid) {
                        Ok(()) => 0,
                        Err(()) => error(3),
                    },
                    None => error(3),
                };
            }
            return finish_syscall(frame);
        }
        LINUX_SCHED_YIELD => {
            yield_in_syscall();
            RUNTIME_CALLS.fetch_add(1, Ordering::Relaxed);
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            frame.number = 0;
            return finish_syscall(frame);
        }
        LINUX_RT_SIGRETURN => {
            return match signal_return(frame) {
                Ok(SignalResume::Syscall) => 0,
                Ok(SignalResume::Full) => 2,
                Err(()) => {
                    EXITS.fetch_add(1, Ordering::Relaxed);
                    user::set_exit_code(128 + 11);
                    1
                }
            };
        }
        LINUX_WAIT4 => {
            frame.number = linux_wait4(arguments[0], arguments[1], arguments[2]);
            set_user_stack_pointer(saved_user_rsp);
            return finish_syscall(frame);
        }
        LINUX_EXECVE => {
            match linux_execve(frame) {
                Ok(()) => frame.number = 0,
                Err(failure) => frame.number = failure,
            }
            return 0;
        }
        _ => {}
    }
    match dispatch_linux(frame.number, arguments) {
        SyscallResult::Return(value) => {
            frame.number = value;
            finish_syscall(frame)
        }
        SyscallResult::Exit(code) => {
            EXITS.fetch_add(1, Ordering::Relaxed);
            user::set_exit_code(code);
            1
        }
    }
}

/// Common syscall epilogue: hands a pending signal to its handler.
fn finish_syscall(frame: &mut LinuxSyscallFrame) -> u64 {
    if let Some(code) = deliver_signal(frame) {
        EXITS.fetch_add(1, Ordering::Relaxed);
        user::set_exit_code(code);
        return 1;
    }
    0
}

/// Calls the Linux dispatcher with random numbers and arguments from kernel
/// context, where no user memory is mapped, so every pointer is refused and the
/// run exercises argument checking, descriptor lookups and flag handling.
/// Syscalls that end or signal tasks, block, or change the context's own
/// limits and sandbox are skipped; the saved process state and the shared
/// object tables are put back afterwards. Returns how many calls ran.
#[cfg(feature = "boot-test")]
pub(crate) fn fuzz_syscalls(next: &mut dyn FnMut() -> u64, rounds: u32) -> u32 {
    const SKIPPED: [u64; 47] = [
        7, 15, 23, 34, 35, 42, 43, 45, 47, 56, 57, 58, 59, 60, 61, 62, 80, 81, 95, 101, 126, 128,
        130, 141, 157, 160, 164, 165, 166, 169, 200, 202, 227, 230, 231, 232, 234, 247, 270, 271,
        281, 288, 302, 317, 322, 435, 441,
    ];
    let saved = save_process_state();
    let mut executed = 0;
    for _ in 0..rounds {
        let number = next() % 450;
        if SKIPPED.contains(&number) {
            continue;
        }
        let mut arguments = [0u64; 6];
        for argument in &mut arguments {
            let random = next();
            *argument = match random % 9 {
                0 => (random >> 8) % 40,
                1 => u64::MAX,
                2 => (random >> 8) | (1 << 63),
                3 => 0x7fff_ffff_f000,
                4 => 0,
                5 => (random >> 8) % 4096,
                6 => (random >> 8) & !0xfff,
                7 => 0u64.wrapping_sub((random >> 8) % 300),
                _ => random,
            };
        }
        let _ = dispatch_linux(number, arguments);
        executed += 1;
    }
    restore_process_state(&saved);
    *PIPES.lock() = [Pipe::EMPTY; PIPE_COUNT];
    reset_udp_sockets();
    crate::udp::reset();
    *UNIX_PAIRS.lock() = [UnixPair::EMPTY; UNIX_PAIR_COUNT];
    *UNIX_LISTENERS.lock() = [UnixListener::EMPTY; UNIX_LISTENER_COUNT];
    *EPOLL_INSTANCES.lock() = [EpollInstance::EMPTY; EPOLL_INSTANCE_COUNT];
    crate::tcpnet::reset();
    executed
}

fn dispatch_linux(number: u64, arguments: [u64; 6]) -> SyscallResult {
    match number {
        LINUX_READ => SyscallResult::Return(linux_read(arguments[0], arguments[1], arguments[2])),
        LINUX_WRITE => SyscallResult::Return(linux_write(arguments[0], arguments[1], arguments[2])),
        LINUX_OPEN => SyscallResult::Return(linux_openat(
            AT_FDCWD as u64,
            arguments[0],
            arguments[1],
            arguments[2],
        )),
        LINUX_CLOSE => SyscallResult::Return(linux_close(arguments[0])),
        LINUX_STAT => SyscallResult::Return(linux_newfstatat(
            AT_FDCWD as u64,
            arguments[0],
            arguments[1],
            0,
        )),
        LINUX_LSTAT => SyscallResult::Return(linux_newfstatat(
            AT_FDCWD as u64,
            arguments[0],
            arguments[1],
            AT_SYMLINK_NOFOLLOW,
        )),
        LINUX_CREAT => SyscallResult::Return(linux_openat(
            AT_FDCWD as u64,
            arguments[0],
            O_CREAT | O_TRUNC | 1,
            arguments[1],
        )),
        LINUX_SYMLINK => {
            SyscallResult::Return(linux_symlinkat(arguments[0], AT_FDCWD as u64, arguments[1]))
        }
        LINUX_SYMLINKAT => {
            SyscallResult::Return(linux_symlinkat(arguments[0], arguments[1], arguments[2]))
        }
        LINUX_GETRLIMIT => SyscallResult::Return(linux_prlimit64(0, arguments[0], 0, arguments[1])),
        LINUX_GETPGRP => SyscallResult::Return(linux_getpgid(0)),
        LINUX_TRUNCATE => SyscallResult::Return(linux_truncate(arguments[0], arguments[1])),
        LINUX_STATFS => SyscallResult::Return(linux_statfs(arguments[0], arguments[1])),
        LINUX_FSTATFS => SyscallResult::Return(linux_fstatfs(arguments[0], arguments[1])),
        LINUX_SELECT => SyscallResult::Return(linux_select(arguments)),
        LINUX_PSELECT6 => SyscallResult::Return(linux_pselect6(arguments)),
        LINUX_FACCESSAT => SyscallResult::Return(linux_faccessat2(
            arguments[0],
            arguments[1],
            arguments[2],
            0,
        )),
        LINUX_EPOLL_CREATE => SyscallResult::Return(if (arguments[0] as i32) > 0 {
            linux_epoll_create1(0)
        } else {
            error(22)
        }),
        LINUX_EPOLL_PWAIT => SyscallResult::Return(linux_epoll_wait(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        LINUX_UTIMENSAT => SyscallResult::Return(linux_utimensat(arguments[0], arguments[1])),
        LINUX_FADVISE64 => SyscallResult::Return(0),
        LINUX_MLOCK | LINUX_MUNLOCK | LINUX_MLOCKALL | LINUX_MUNLOCKALL => SyscallResult::Return(0),
        LINUX_SETUID | LINUX_SETGID => {
            SyscallResult::Return(if arguments[0] == 0 { 0 } else { error(1) })
        }
        LINUX_SETGROUPS => SyscallResult::Return(if arguments[0] == 0 { 0 } else { error(1) }),
        LINUX_CHROOT => SyscallResult::Return(error(1)),
        LINUX_GETPRIORITY => SyscallResult::Return(linux_getpriority(arguments[0], arguments[1])),
        LINUX_SETPRIORITY => {
            SyscallResult::Return(linux_setpriority(arguments[0], arguments[1], arguments[2]))
        }
        LINUX_SCHED_GET_PRIORITY_MAX | LINUX_SCHED_GET_PRIORITY_MIN => {
            SyscallResult::Return(if arguments[0] <= 5 { 0 } else { error(22) })
        }
        LINUX_CAPGET => SyscallResult::Return(linux_capget(arguments[0], arguments[1])),
        LINUX_CAPSET => SyscallResult::Return(linux_capset(arguments[0], arguments[1])),
        LINUX_SCHED_SETAFFINITY => SyscallResult::Return(linux_sched_setaffinity(
            arguments[0],
            arguments[1],
            arguments[2],
        )),
        LINUX_SHUTDOWN => SyscallResult::Return(linux_shutdown(arguments[0], arguments[1])),
        LINUX_GETSOCKNAME => SyscallResult::Return(linux_getsockname(
            arguments[0],
            arguments[1],
            arguments[2],
            false,
        )),
        LINUX_GETPEERNAME => SyscallResult::Return(linux_getsockname(
            arguments[0],
            arguments[1],
            arguments[2],
            true,
        )),
        LINUX_SETSOCKOPT => SyscallResult::Return(linux_setsockopt(arguments)),
        LINUX_GETSOCKOPT => SyscallResult::Return(linux_getsockopt(arguments)),
        LINUX_GETRUSAGE => SyscallResult::Return(linux_getrusage(arguments[0], arguments[1])),
        LINUX_TIMES => SyscallResult::Return(linux_times(arguments[0])),
        LINUX_SCHED_GETPARAM => SyscallResult::Return(linux_sched_getparam(arguments[1])),
        LINUX_SCHED_GETSCHEDULER => SyscallResult::Return(0),
        LINUX_PRCTL => SyscallResult::Return(linux_prctl(arguments[0], arguments[1])),
        LINUX_PPOLL => SyscallResult::Return(linux_ppoll(arguments[0], arguments[1], arguments[2])),
        LINUX_GETGROUPS => SyscallResult::Return(linux_getgroups(arguments[0])),
        LINUX_GETRESUID | LINUX_GETRESGID => SyscallResult::Return(linux_getres_id(arguments)),
        LINUX_DUP => SyscallResult::Return(linux_dup(arguments[0])),
        LINUX_DUP2 => SyscallResult::Return(linux_dup2(arguments[0], arguments[1])),
        LINUX_FSTAT => SyscallResult::Return(linux_fstat(arguments[0], arguments[1])),
        LINUX_POLL => SyscallResult::Return(linux_poll(arguments[0], arguments[1], arguments[2])),
        LINUX_LSEEK => SyscallResult::Return(linux_lseek(arguments[0], arguments[1], arguments[2])),
        LINUX_MMAP => SyscallResult::Return(linux_mmap(arguments)),
        LINUX_MPROTECT => {
            SyscallResult::Return(linux_mprotect(arguments[0], arguments[1], arguments[2]))
        }
        LINUX_MUNMAP => SyscallResult::Return(linux_munmap(arguments[0], arguments[1])),
        LINUX_BRK => {
            MEMORY_CALLS.fetch_add(1, Ordering::Relaxed);
            let result = crate::scheduler::linux_brk_for_current_task(arguments[0])
                .unwrap_or_else(|| crate::arch::paging::user_brk(arguments[0]));
            SyscallResult::Return(result)
        }
        LINUX_RT_SIGACTION => SyscallResult::Return(linux_rt_sigaction(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        LINUX_RT_SIGPROCMASK => SyscallResult::Return(linux_rt_sigprocmask(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        LINUX_IOCTL => SyscallResult::Return(linux_ioctl(arguments[0], arguments[1], arguments[2])),
        LINUX_PREAD64 => SyscallResult::Return(linux_pread64(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        LINUX_PWRITE64 => SyscallResult::Return(linux_pwrite64(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        LINUX_READV => SyscallResult::Return(linux_readv(arguments[0], arguments[1], arguments[2])),
        LINUX_WRITEV => {
            SyscallResult::Return(linux_writev(arguments[0], arguments[1], arguments[2]))
        }
        LINUX_ACCESS => SyscallResult::Return(linux_faccessat2(
            AT_FDCWD as u64,
            arguments[0],
            arguments[1],
            0,
        )),
        LINUX_MSYNC => SyscallResult::Return(linux_msync(arguments[0], arguments[1], arguments[2])),
        LINUX_MADVISE => {
            SyscallResult::Return(linux_madvise(arguments[0], arguments[1], arguments[2]))
        }
        LINUX_NANOSLEEP => SyscallResult::Return(linux_nanosleep(arguments[0], arguments[1])),
        LINUX_PAUSE => SyscallResult::Return(linux_pause()),
        LINUX_ALARM => SyscallResult::Return(linux_alarm(arguments[0])),
        LINUX_GETITIMER => SyscallResult::Return(linux_getitimer(arguments[0], arguments[1])),
        LINUX_SETITIMER => {
            SyscallResult::Return(linux_setitimer(arguments[0], arguments[1], arguments[2]))
        }
        LINUX_SCHED_YIELD => {
            crate::scheduler::yield_now();
            RUNTIME_CALLS.fetch_add(1, Ordering::Relaxed);
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            SyscallResult::Return(0)
        }
        LINUX_GETPID => SyscallResult::Return(
            crate::scheduler::current_task_pid_for_linux()
                .unwrap_or_else(crate::process::current_pid),
        ),
        LINUX_PIPE => SyscallResult::Return(linux_pipe2(arguments[0], 0)),
        LINUX_PIPE2 => SyscallResult::Return(linux_pipe2(arguments[0], arguments[1])),
        LINUX_MINCORE => {
            SyscallResult::Return(linux_mincore(arguments[0], arguments[1], arguments[2]))
        }
        LINUX_FLOCK => SyscallResult::Return(linux_flock(arguments[0], arguments[1])),
        LINUX_FCHDIR => SyscallResult::Return(linux_fchdir(arguments[0])),
        LINUX_PERSONALITY => SyscallResult::Return(personality_result(arguments[0])),
        LINUX_SCHED_SETSCHEDULER => SyscallResult::Return(linux_sched_setscheduler(
            arguments[0],
            arguments[1],
            arguments[2],
        )),
        LINUX_SYNC => SyscallResult::Return(0),
        LINUX_SYNCFS => SyscallResult::Return(linux_syncfs(arguments[0])),
        LINUX_FALLOCATE => SyscallResult::Return(linux_fallocate(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        LINUX_PREADV => SyscallResult::Return(linux_preadv(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
            arguments[4],
        )),
        LINUX_PWRITEV => SyscallResult::Return(linux_pwritev(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
            arguments[4],
        )),
        LINUX_MEMBARRIER => SyscallResult::Return(linux_membarrier(arguments[0], arguments[1])),
        LINUX_CLOSE_RANGE => {
            SyscallResult::Return(linux_close_range(arguments[0], arguments[1], arguments[2]))
        }
        LINUX_MEMFD_CREATE => SyscallResult::Return(linux_memfd_create(arguments[0], arguments[1])),
        LINUX_SENDFILE => SyscallResult::Return(linux_sendfile(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        LINUX_COPY_FILE_RANGE => SyscallResult::Return(linux_copy_file_range(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
            arguments[4],
            arguments[5],
        )),
        LINUX_EVENTFD => SyscallResult::Return(linux_eventfd2(arguments[0], 0)),
        LINUX_EVENTFD2 => SyscallResult::Return(linux_eventfd2(arguments[0], arguments[1])),
        LINUX_TIMERFD_CREATE => {
            SyscallResult::Return(linux_timerfd_create(arguments[0], arguments[1]))
        }
        LINUX_TIMERFD_SETTIME => SyscallResult::Return(linux_timerfd_settime(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        LINUX_TIMERFD_GETTIME => {
            SyscallResult::Return(linux_timerfd_gettime(arguments[0], arguments[1]))
        }
        LINUX_EPOLL_CREATE1 => SyscallResult::Return(linux_epoll_create1(arguments[0])),
        LINUX_EPOLL_CTL => SyscallResult::Return(linux_epoll_ctl(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        LINUX_EPOLL_WAIT => SyscallResult::Return(linux_epoll_wait(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        LINUX_SOCKET => {
            SyscallResult::Return(linux_socket(arguments[0], arguments[1], arguments[2]))
        }
        LINUX_CONNECT => {
            SyscallResult::Return(linux_connect(arguments[0], arguments[1], arguments[2]))
        }
        LINUX_BIND => SyscallResult::Return(linux_bind(arguments[0], arguments[1], arguments[2])),
        LINUX_LISTEN => SyscallResult::Return(linux_listen(arguments[0], arguments[1])),
        LINUX_ACCEPT => {
            SyscallResult::Return(linux_accept(arguments[0], arguments[1], arguments[2], 0))
        }
        LINUX_ACCEPT4 => SyscallResult::Return(linux_accept(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        LINUX_SENDTO => SyscallResult::Return(linux_sendto(arguments)),
        LINUX_SENDMSG => {
            SyscallResult::Return(linux_sendmsg(arguments[0], arguments[1], arguments[2]))
        }
        LINUX_RECVMSG => {
            SyscallResult::Return(linux_recvmsg(arguments[0], arguments[1], arguments[2]))
        }
        LINUX_RECVFROM => SyscallResult::Return(linux_recvfrom(arguments)),
        LINUX_SOCKETPAIR => SyscallResult::Return(linux_socketpair(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        LINUX_GETUID | LINUX_GETGID | LINUX_GETEUID | LINUX_GETEGID => SyscallResult::Return(0),
        LINUX_GETPPID => SyscallResult::Return(
            crate::scheduler::current_parent_pid_for_linux()
                .unwrap_or_else(crate::process::current_parent),
        ),
        LINUX_UNAME => SyscallResult::Return(linux_uname(arguments[0])),
        LINUX_GETTIMEOFDAY => SyscallResult::Return(linux_gettimeofday(arguments[0], arguments[1])),
        LINUX_FCNTL => SyscallResult::Return(linux_fcntl(arguments[0], arguments[1], arguments[2])),
        LINUX_FSYNC | LINUX_FDATASYNC => SyscallResult::Return(linux_fsync(arguments[0])),
        LINUX_FTRUNCATE => SyscallResult::Return(linux_ftruncate(arguments[0], arguments[1])),
        LINUX_GETDENTS64 => {
            SyscallResult::Return(linux_getdents64(arguments[0], arguments[1], arguments[2]))
        }
        LINUX_GETCWD => SyscallResult::Return(linux_getcwd(arguments[0], arguments[1])),
        LINUX_CHDIR => SyscallResult::Return(linux_chdir(arguments[0])),
        LINUX_RENAME => SyscallResult::Return(linux_renameat(
            AT_FDCWD as u64,
            arguments[0],
            AT_FDCWD as u64,
            arguments[1],
            0,
        )),
        LINUX_MKDIR => {
            SyscallResult::Return(linux_mkdirat(AT_FDCWD as u64, arguments[0], arguments[1]))
        }
        LINUX_RMDIR => {
            SyscallResult::Return(linux_unlinkat(AT_FDCWD as u64, arguments[0], AT_REMOVEDIR))
        }
        LINUX_UNLINK => SyscallResult::Return(linux_unlinkat(AT_FDCWD as u64, arguments[0], 0)),
        LINUX_CHMOD => SyscallResult::Return(linux_chmodat(
            AT_FDCWD as u64,
            arguments[0],
            arguments[1],
            0,
        )),
        LINUX_FCHMOD => SyscallResult::Return(linux_fchmod(arguments[0], arguments[1])),
        LINUX_CHOWN => SyscallResult::Return(linux_fchownat(AT_FDCWD as u64, arguments[0], 0)),
        LINUX_LCHOWN => SyscallResult::Return(linux_fchownat(
            AT_FDCWD as u64,
            arguments[0],
            AT_SYMLINK_NOFOLLOW,
        )),
        LINUX_FCHOWN => SyscallResult::Return(linux_fchown(arguments[0])),
        LINUX_FCHOWNAT => {
            SyscallResult::Return(linux_fchownat(arguments[0], arguments[1], arguments[4]))
        }
        LINUX_SETTIMEOFDAY => SyscallResult::Return(linux_settimeofday(arguments[0], arguments[1])),
        LINUX_CLOCK_SETTIME => {
            SyscallResult::Return(linux_clock_settime(arguments[0], arguments[1]))
        }
        LINUX_REBOOT => {
            SyscallResult::Return(linux_reboot(arguments[0], arguments[1], arguments[2]))
        }
        LINUX_MOUNT => SyscallResult::Return(linux_mount(arguments[0], arguments[1], arguments[3])),
        LINUX_UMOUNT2 => SyscallResult::Return(linux_umount2(arguments[0])),
        LINUX_UMASK => SyscallResult::Return(linux_umask(arguments[0])),
        LINUX_SYSINFO => SyscallResult::Return(linux_sysinfo(arguments[0])),
        LINUX_SIGALTSTACK => SyscallResult::Return(linux_sigaltstack(arguments[0], arguments[1])),
        LINUX_ARCH_PRCTL => SyscallResult::Return(linux_arch_prctl(arguments[0], arguments[1])),
        LINUX_GETTID => {
            RUNTIME_CALLS.fetch_add(1, Ordering::Relaxed);
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            SyscallResult::Return(current_thread_id())
        }
        LINUX_TKILL => linux_tkill(arguments[0], arguments[1]),
        LINUX_FUTEX => SyscallResult::Return(linux_futex(arguments)),
        LINUX_PTRACE => SyscallResult::Return(ptrace::ptrace(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        LINUX_TIME => SyscallResult::Return(linux_time(arguments[0])),
        LINUX_SCHED_GETAFFINITY => SyscallResult::Return(linux_sched_getaffinity(
            arguments[0],
            arguments[1],
            arguments[2],
        )),
        LINUX_EXIT => SyscallResult::Exit(arguments[0] & 0xff),
        LINUX_EXIT_GROUP => {
            crate::scheduler::exit_group(arguments[0] & 0xff);
            SyscallResult::Exit(arguments[0] & 0xff)
        }
        LINUX_TGKILL => linux_tgkill(arguments[0], arguments[1], arguments[2]),
        LINUX_SETPGID => SyscallResult::Return(linux_setpgid(arguments[0], arguments[1])),
        LINUX_GETPGID => SyscallResult::Return(linux_getpgid(arguments[0])),
        LINUX_SETSID => SyscallResult::Return(linux_setsid()),
        LINUX_GETSID => SyscallResult::Return(linux_getsid(arguments[0])),
        LINUX_SECCOMP => {
            SyscallResult::Return(linux_seccomp(arguments[0], arguments[1], arguments[2]))
        }
        LINUX_SET_TID_ADDRESS => SyscallResult::Return(linux_set_tid_address(arguments[0])),
        LINUX_CLOCK_GETTIME => {
            SyscallResult::Return(linux_clock_gettime(arguments[0], arguments[1]))
        }
        LINUX_CLOCK_GETRES => SyscallResult::Return(linux_clock_getres(arguments[0], arguments[1])),
        LINUX_CLOCK_NANOSLEEP => SyscallResult::Return(linux_clock_nanosleep(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        LINUX_OPENAT => SyscallResult::Return(linux_openat(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        LINUX_MKDIRAT => {
            SyscallResult::Return(linux_mkdirat(arguments[0], arguments[1], arguments[2]))
        }
        LINUX_NEWFSTATAT => SyscallResult::Return(linux_newfstatat(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        LINUX_UNLINKAT => {
            SyscallResult::Return(linux_unlinkat(arguments[0], arguments[1], arguments[2]))
        }
        LINUX_RENAMEAT => SyscallResult::Return(linux_renameat(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
            0,
        )),
        LINUX_READLINK => SyscallResult::Return(linux_readlinkat(
            AT_FDCWD as u64,
            arguments[0],
            arguments[1],
            arguments[2],
        )),
        LINUX_READLINKAT => SyscallResult::Return(linux_readlinkat(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        LINUX_FCHMODAT => SyscallResult::Return(linux_chmodat(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        LINUX_GETRANDOM => {
            SyscallResult::Return(linux_getrandom(arguments[0], arguments[1], arguments[2]))
        }
        LINUX_SET_ROBUST_LIST => {
            SyscallResult::Return(linux_set_robust_list(arguments[0], arguments[1]))
        }
        LINUX_GET_ROBUST_LIST => SyscallResult::Return(linux_get_robust_list(
            arguments[0],
            arguments[1],
            arguments[2],
        )),
        LINUX_DUP3 => SyscallResult::Return(linux_dup3(arguments[0], arguments[1], arguments[2])),
        LINUX_PRLIMIT64 => SyscallResult::Return(linux_prlimit64(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        LINUX_RENAMEAT2 => SyscallResult::Return(linux_renameat(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
            arguments[4],
        )),
        LINUX_RSEQ => SyscallResult::Return(linux_rseq(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        LINUX_GETCPU => {
            SyscallResult::Return(linux_getcpu(arguments[0], arguments[1], arguments[2]))
        }
        LINUX_STATX => SyscallResult::Return(linux_statx(arguments)),
        LINUX_FACCESSAT2 => SyscallResult::Return(linux_faccessat2(
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
        )),
        _ => {
            let occurrence = UNKNOWN.fetch_add(1, Ordering::Relaxed);
            if occurrence < 8 {
                crate::serial::format(format_args!(
                    "AEROS_LINUX_ENOSYS number={} argument0={:#x} argument1={:#x}\n",
                    number, arguments[0], arguments[1]
                ));
            }
            SyscallResult::Return(error(38))
        }
    }
}

/// Exercises the pipe ring buffer directly (write, FIFO partial reads,
/// EAGAIN on empty-with-writer-open, EOF once the write end closes, and
/// buffer wraparound) without going through the full syscall-ABI path --
/// `linux_pipe2`/`linux_read`/`linux_write` additionally marshal user
/// pointers via `copy_to_user`/`copy_from_user`, which need a live user
/// address space to target and so aren't exercisable from this bare
/// kernel-boot context; that outer layer is a thin, mechanical wrapper
/// consistent with every other syscall in this file, so the real risk of a
/// subtle bug lives in the ring buffer logic tested here.
static PIPE_TEST_SLOT: AtomicU64 = AtomicU64::new(u64::MAX);
static PIPE_TEST_PRODUCER_PHASE: AtomicU64 = AtomicU64::new(0);
static PIPE_TEST_CONSUMER_PHASE: AtomicU64 = AtomicU64::new(0);
static PIPE_TEST_CONSUMER_OK: AtomicU64 = AtomicU64::new(0);

extern "C" fn pipe_blocking_test_producer() -> ! {
    PIPE_TEST_PRODUCER_PHASE.store(1, Ordering::Release);
    // Yield twice before writing, so the consumer (spawned first, and thus
    // scheduled first) has already found the pipe empty and entered its
    // own retry loop -- this exercises the actual wait-then-succeed path,
    // not just a lucky first check.
    crate::scheduler::yield_now();
    crate::scheduler::yield_now();
    let slot = PIPE_TEST_SLOT.load(Ordering::Acquire) as usize;
    let _ = pipe_write(slot, b"blocked!");
    PIPE_TEST_PRODUCER_PHASE.store(2, Ordering::Release);
    crate::scheduler::exit_current()
}

extern "C" fn pipe_blocking_test_consumer() -> ! {
    PIPE_TEST_CONSUMER_PHASE.store(1, Ordering::Release);
    let slot = PIPE_TEST_SLOT.load(Ordering::Acquire) as usize;
    let mut buffer = [0u8; 8];
    let ok = pipe_read(slot, &mut buffer) == Ok(8) && buffer == *b"blocked!";
    PIPE_TEST_CONSUMER_OK.store(ok as u64, Ordering::Release);
    PIPE_TEST_CONSUMER_PHASE.store(2, Ordering::Release);
    crate::scheduler::exit_current()
}

/// Genuine two-task producer/consumer proof that the bounded cooperative
/// retry in `pipe_read` actually waits and succeeds rather than merely
/// looping harmlessly: the consumer is spawned (and therefore scheduled)
/// first, so it is guaranteed to find the pipe empty and enter its retry
/// loop before the producer -- spawned second -- ever runs. If `pipe_read`
/// didn't cooperatively yield and retry, the consumer would exhaust its
/// wait and return EAGAIN before the producer got a chance to write.
pub(crate) fn pipe_blocking_self_test() -> bool {
    let Some(slot) = allocate_test_pipe(0) else {
        return false;
    };
    PIPE_TEST_SLOT.store(slot as u64, Ordering::Release);
    PIPE_TEST_PRODUCER_PHASE.store(0, Ordering::Release);
    PIPE_TEST_CONSUMER_PHASE.store(0, Ordering::Release);
    PIPE_TEST_CONSUMER_OK.store(0, Ordering::Release);
    if crate::scheduler::spawn(pipe_blocking_test_consumer).is_none()
        || crate::scheduler::spawn(pipe_blocking_test_producer).is_none()
    {
        PIPES.lock()[slot] = Pipe::EMPTY;
        return false;
    }
    for _ in 0..PIPE_WAIT_ITERATIONS as usize + 8 {
        if crate::scheduler::stats().tasks <= 1 {
            break;
        }
        crate::scheduler::yield_now();
    }
    let completed = PIPE_TEST_PRODUCER_PHASE.load(Ordering::Acquire) == 2
        && PIPE_TEST_CONSUMER_PHASE.load(Ordering::Acquire) == 2;
    let consumer_ok = PIPE_TEST_CONSUMER_OK.load(Ordering::Acquire) == 1;
    let reaped = crate::scheduler::reap();
    PIPES.lock()[slot] = Pipe::EMPTY;
    completed && consumer_ok && reaped == 2
}

fn allocate_test_pipe(initial_head: usize) -> Option<usize> {
    let mut pipes = PIPES.lock();
    let slot = pipes.iter().position(|pipe| !pipe.used)?;
    pipes[slot] = Pipe {
        used: true,
        read_open: true,
        write_open: true,
        head: initial_head,
        ..Pipe::EMPTY
    };
    Some(slot)
}

fn timerfd_self_test() -> bool {
    let mut pipe = Pipe {
        timer: true,
        ..Pipe::EMPTY
    };
    timer_arm(&mut pipe, 100, 0, false, 1000);
    let one_shot = pipe.timer_next == 1100
        && timer_remaining(&mut pipe, 1050) == (50, 0)
        && pipe.counter == 0
        && timer_remaining(&mut pipe, 1100) == (0, 0)
        && pipe.counter == 1
        && pipe.timer_next == 0;

    timer_arm(&mut pipe, 100, 10, false, 1000);
    timer_expire(&mut pipe, 1135);
    let periodic =
        pipe.counter == 4 && pipe.timer_next == 1140 && timer_remaining(&mut pipe, 1135) == (5, 10);

    timer_arm(&mut pipe, 500, 0, true, 9999);
    let absolute = pipe.timer_next == 500 && pipe.counter == 0;
    timer_arm(&mut pipe, 0, 7, false, 1000);
    let disarmed = pipe.timer_next == 0 && timer_remaining(&mut pipe, 5000) == (0, 7);

    let Ok(descriptor) = create_timerfd(false) else {
        return false;
    };
    let Ok(slot) = timerfd_slot(descriptor) else {
        return false;
    };
    let mut word = [0u8; 8];
    let idle_ok = fd_ready_events(descriptor) == 0
        && pipe_read(slot, &mut word) == Err(11)
        && pipe_read(slot, &mut [0u8; 4]) == Err(22)
        && pipe_write(slot, &word) == Err(22);
    let started = crate::time::monotonic_nanoseconds();
    timer_arm(&mut PIPES.lock()[slot], 2_000_000, 0, false, started);
    let waiting_ok = fd_ready_events(descriptor) == 0;
    let read_ok = pipe_read(slot, &mut word) == Ok(8)
        && u64::from_le_bytes(word) == 1
        && crate::time::monotonic_nanoseconds() >= started + 2_000_000;
    let ready_after = fd_ready_events(descriptor) == 0;
    let closed = linux_close(descriptor) == 0 && !PIPES.lock()[slot].used;

    one_shot
        && periodic
        && absolute
        && disarmed
        && idle_ok
        && waiting_ok
        && read_ok
        && ready_after
        && closed
}

fn eventfd_self_test() -> bool {
    let Ok(descriptor) = create_eventfd(5, false, false) else {
        return false;
    };
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return false;
    };
    let slot = process_fd.handle as usize;
    let mut word = [0u8; 8];
    let initial_ok = fd_ready_events(descriptor) == EPOLLIN | EPOLLOUT
        && pipe_read(slot, &mut word) == Ok(8)
        && u64::from_le_bytes(word) == 5;
    let empty_ok = fd_ready_events(descriptor) == EPOLLOUT && pipe_read(slot, &mut word) == Err(11);
    let accumulate_ok = pipe_write(slot, &3u64.to_le_bytes()) == Ok(8)
        && pipe_write(slot, &4u64.to_le_bytes()) == Ok(8)
        && pipe_read(slot, &mut word) == Ok(8)
        && u64::from_le_bytes(word) == 7;
    let invalid_ok = pipe_read(slot, &mut [0u8; 4]) == Err(22)
        && pipe_write(slot, &[1, 2, 3]) == Err(22)
        && pipe_write(slot, &u64::MAX.to_le_bytes()) == Err(22);
    let overflow_ok = pipe_write(slot, &(u64::MAX - 1).to_le_bytes()) == Ok(8)
        && pipe_write(slot, &1u64.to_le_bytes()) == Err(11)
        && fd_ready_events(descriptor) == EPOLLIN
        && pipe_read(slot, &mut word) == Ok(8)
        && u64::from_le_bytes(word) == u64::MAX - 1;
    let closed = linux_close(descriptor) == 0 && !PIPES.lock()[slot].used;

    let Ok(semaphore) = create_eventfd(2, true, false) else {
        return false;
    };
    let Some(semaphore_fd) = lookup_process_fd(semaphore) else {
        return false;
    };
    let semaphore_slot = semaphore_fd.handle as usize;
    let semaphore_ok = pipe_read(semaphore_slot, &mut word) == Ok(8)
        && u64::from_le_bytes(word) == 1
        && pipe_read(semaphore_slot, &mut word) == Ok(8)
        && u64::from_le_bytes(word) == 1
        && pipe_read(semaphore_slot, &mut word) == Err(11);
    let semaphore_closed = linux_close(semaphore) == 0;

    initial_ok
        && empty_ok
        && accumulate_ok
        && invalid_ok
        && overflow_ok
        && closed
        && semaphore_ok
        && semaphore_closed
}

pub(crate) fn pipe_self_test() -> bool {
    let Some(slot) = allocate_test_pipe(0) else {
        return false;
    };
    let write_ok = pipe_write(slot, b"hello pipe") == Ok(10);
    let mut head_half = [0u8; 5];
    let first_half_ok = pipe_read(slot, &mut head_half) == Ok(5) && head_half == *b"hello";
    let mut tail_half = [0u8; 5];
    let second_half_ok = pipe_read(slot, &mut tail_half) == Ok(5) && tail_half == *b" pipe";
    let eagain_ok = pipe_read(slot, &mut head_half) == Err(11);
    PIPES.lock()[slot].write_open = false;
    let eof_ok = pipe_read(slot, &mut head_half) == Ok(0);
    PIPES.lock()[slot] = Pipe::EMPTY;

    let Some(wrap_slot) = allocate_test_pipe(PIPE_BUFFER_BYTES - 3) else {
        return false;
    };
    let wrap_write_ok = pipe_write(wrap_slot, b"WRAP") == Ok(4);
    let mut wrap_buffer = [0u8; 4];
    let wrap_read_ok = pipe_read(wrap_slot, &mut wrap_buffer) == Ok(4) && wrap_buffer == *b"WRAP";
    PIPES.lock()[wrap_slot] = Pipe::EMPTY;

    write_ok
        && eventfd_self_test()
        && timerfd_self_test()
        && first_half_ok
        && second_half_ok
        && eagain_ok
        && eof_ok
        && wrap_write_ok
        && wrap_read_ok
}

/// Drives `epoll_create1`/`epoll_ctl`/`epoll_wait`'s core logic directly
/// (`epoll_ctl_apply`/`epoll_scan`, bypassing user-memory copying) against a
/// real pipe with real installed process fds, the same split `pipe_self_test`
/// uses for `pipe_read`/`pipe_write`.
pub(crate) fn epoll_self_test() -> bool {
    let Some(slot) = allocate_test_pipe(0) else {
        return false;
    };
    let Some(read_fd) = install_process_fd_kind(
        slot as u32,
        false,
        false,
        true,
        false,
        false,
        false,
        false,
        true,
        false,
        false,
    ) else {
        PIPES.lock()[slot] = Pipe::EMPTY;
        return false;
    };
    let Some(write_fd) = install_process_fd_kind(
        slot as u32,
        false,
        false,
        true,
        false,
        false,
        false,
        false,
        false,
        true,
        false,
    ) else {
        let _ = linux_close(read_fd);
        PIPES.lock()[slot] = Pipe::EMPTY;
        return false;
    };
    let epfd = linux_epoll_create1(0);
    let create_ok = epfd < 1_000_000;
    let epoll_handle = create_ok
        .then(|| lookup_epoll_fd(epfd))
        .flatten()
        .map(|fd| fd.handle);
    let Some(epoll_handle) = epoll_handle else {
        let _ = linux_close(read_fd);
        let _ = linux_close(write_fd);
        PIPES.lock()[slot] = Pipe::EMPTY;
        return false;
    };
    let marker = 0xdead_beef_u64;
    let add_ok = epoll_ctl_apply(epoll_handle, EPOLL_CTL_ADD, read_fd as i32, EPOLLIN, marker) == 0;
    let duplicate_add_rejected =
        epoll_ctl_apply(epoll_handle, EPOLL_CTL_ADD, read_fd as i32, EPOLLIN, marker) != 0;
    let mut scratch = [(0u32, 0u64); 4];
    let empty_before_write = epoll_scan(epoll_handle, &mut scratch) == Some(0);
    let write_ok = pipe_write(slot, b"hi") == Ok(2);
    let saw_ready =
        epoll_scan(epoll_handle, &mut scratch) == Some(1) && scratch[0] == (EPOLLIN, marker);
    let del_ok = epoll_ctl_apply(epoll_handle, EPOLL_CTL_DEL, read_fd as i32, 0, 0) == 0;
    let empty_after_del = epoll_scan(epoll_handle, &mut scratch) == Some(0);
    let readd_ok = epoll_ctl_apply(epoll_handle, EPOLL_CTL_ADD, read_fd as i32, EPOLLOUT, 7) == 0;
    // The read end is never writable, so watching for EPOLLOUT on it must
    // never report ready, regardless of how full the pipe is.
    let mod_never_ready = epoll_scan(epoll_handle, &mut scratch) == Some(0);
    let mod_ok = epoll_ctl_apply(epoll_handle, EPOLL_CTL_MOD, read_fd as i32, EPOLLIN, 9) == 0;
    let mod_ready_again =
        epoll_scan(epoll_handle, &mut scratch) == Some(1) && scratch[0] == (EPOLLIN, 9);
    let _ = linux_close(read_fd);
    let _ = linux_close(write_fd);
    let _ = linux_close(epfd);
    PIPES.lock()[slot] = Pipe::EMPTY;

    create_ok
        && add_ok
        && duplicate_add_rejected
        && empty_before_write
        && write_ok
        && saw_ready
        && del_ok
        && empty_after_del
        && readd_ok
        && mod_never_ready
        && mod_ok
        && mod_ready_again
}

/// Exercises `AF_UNIX`/`SOCK_STREAM` `socketpair()` directly against the
/// testable cores (`unix_socket_read`/`unix_socket_write`), the same way
/// `pipe_self_test`/`epoll_self_test` bypass `linux_socketpair`'s user-memory
/// copy - there is no active ring-3 address space to copy the fd pair into
/// during a self-test.
pub(crate) fn unix_socketpair_self_test() -> bool {
    let mut pairs = UNIX_PAIRS.lock();
    let Some(slot) = pairs.iter().position(|pair| !pair.used) else {
        return false;
    };
    pairs[slot] = UnixPair {
        used: true,
        end_a_open: true,
        end_b_open: true,
        ..UnixPair::EMPTY
    };
    drop(pairs);
    let Some(fd_a) = install_unix_socket_fd(slot as u32, false, false) else {
        UNIX_PAIRS.lock()[slot] = UnixPair::EMPTY;
        return false;
    };
    let Some(fd_b) = install_unix_socket_fd(slot as u32, true, false) else {
        let _ = linux_close(fd_a);
        UNIX_PAIRS.lock()[slot] = UnixPair::EMPTY;
        return false;
    };

    let a_to_b_ok = unix_socket_write(slot, false, b"ping") == Ok(4);
    let mut ping = [0u8; 4];
    let b_reads_ok = unix_socket_read(slot, true, &mut ping) == Ok(4) && ping == *b"ping";
    let b_to_a_ok = unix_socket_write(slot, true, b"pong!") == Ok(5);
    let mut pong = [0u8; 5];
    let a_reads_ok = unix_socket_read(slot, false, &mut pong) == Ok(5) && pong == *b"pong!";
    let eagain_ok = unix_socket_read(slot, false, &mut pong) == Err(11);

    let a_events_empty = fd_ready_events(fd_a) & EPOLLIN == 0;
    let _ = unix_socket_write(slot, true, b"x");
    let a_events_ready = fd_ready_events(fd_a) & EPOLLIN != 0;
    let mut drain = [0u8; 1];
    let _ = unix_socket_read(slot, false, &mut drain);

    // Closing end B must surface as EOF (not endless EAGAIN) to end A, and
    // as EPIPE for any further write from end A - exactly like a pipe whose
    // other end closed.
    let close_ok = linux_close(fd_b) == 0;
    let eof_ok = unix_socket_read(slot, false, &mut pong) == Ok(0);
    let write_after_peer_closed_fails = unix_socket_write(slot, false, b"y") == Err(32);
    let _ = linux_close(fd_a);

    a_to_b_ok
        && b_reads_ok
        && b_to_a_ok
        && a_reads_ok
        && eagain_ok
        && a_events_empty
        && a_events_ready
        && close_ok
        && eof_ok
        && write_after_peer_closed_fails
}

/// Exercises `bind()`/`listen()`/`connect()`/`accept()` for `AF_UNIX`
/// directly against the testable cores (`unix_bind_apply`/`unix_listen_apply`
/// /`unix_connect_apply`/`unix_accept_apply`), the same way
/// `unix_socketpair_self_test` bypasses `linux_socketpair`'s user-memory
/// copy: two full connect/accept cycles through the same listener (proving
/// it's reusable, not one-shot), a name collision correctly rejected, and a
/// connect to a name nobody bound correctly refused.
pub(crate) fn unix_domain_socket_self_test() -> bool {
    let name = b"AEROS-TEST-SOCK";
    let Some(server_fd) = install_unbound_unix_socket_fd(false) else {
        return false;
    };
    let Some(server_index) = process_fd_index(server_fd) else {
        let _ = linux_close(server_fd);
        return false;
    };
    let bind_ok = unix_bind_apply(server_index, name) == 0;
    let duplicate_bind_rejected = {
        let Some(second_fd) = install_unbound_unix_socket_fd(false) else {
            return false;
        };
        let rejected = process_fd_index(second_fd)
            .map(|index| unix_bind_apply(index, name) != 0)
            .unwrap_or(false);
        let _ = linux_close(second_fd);
        rejected
    };
    let listen_ok = unix_listen_apply(server_index) == 0;

    let refused_before_anyone_connects = {
        let Some(client_fd) = install_unbound_unix_socket_fd(false) else {
            return false;
        };
        let refused = process_fd_index(client_fd)
            .map(|index| unix_connect_apply(index, b"AEROS-NOBODY-HOME") != 0)
            .unwrap_or(false);
        let _ = linux_close(client_fd);
        refused
    };

    // First connect/accept cycle.
    let Some(client_a_fd) = install_unbound_unix_socket_fd(false) else {
        return false;
    };
    let Some(client_a_index) = process_fd_index(client_a_fd) else {
        let _ = linux_close(client_a_fd);
        return false;
    };
    let connect_a_ok = unix_connect_apply(client_a_index, name) == 0;
    let client_a_pair_slot = PROCESS_FDS.lock()[client_a_index].handle;
    let server_handle = PROCESS_FDS.lock()[server_index].handle;
    let Some(accepted_a_pair_slot) = unix_accept_apply(server_handle) else {
        let _ = linux_close(client_a_fd);
        return false;
    };
    let same_pair_a = accepted_a_pair_slot as u32 == client_a_pair_slot;
    let Some(accepted_a_fd) = install_unix_socket_fd(accepted_a_pair_slot as u32, true, false)
    else {
        let _ = linux_close(client_a_fd);
        return false;
    };

    let client_to_server_ok =
        unix_socket_write(client_a_pair_slot as usize, false, b"hello-server") == Ok(12);
    let mut from_client = [0u8; 12];
    let server_reads_ok = unix_socket_read(accepted_a_pair_slot, true, &mut from_client) == Ok(12)
        && &from_client == b"hello-server";
    let server_to_client_ok =
        unix_socket_write(accepted_a_pair_slot, true, b"hi-client!") == Ok(10);
    let mut from_server = [0u8; 10];
    let client_reads_ok = unix_socket_read(client_a_pair_slot as usize, false, &mut from_server)
        == Ok(10)
        && &from_server == b"hi-client!";
    let _ = linux_close(client_a_fd);
    let _ = linux_close(accepted_a_fd);

    // Second cycle through the SAME listener, proving it's reusable.
    let Some(client_b_fd) = install_unbound_unix_socket_fd(false) else {
        return false;
    };
    let Some(client_b_index) = process_fd_index(client_b_fd) else {
        let _ = linux_close(client_b_fd);
        return false;
    };
    let connect_b_ok = unix_connect_apply(client_b_index, name) == 0;
    let client_b_pair_slot = PROCESS_FDS.lock()[client_b_index].handle;
    let Some(accepted_b_pair_slot) = unix_accept_apply(server_handle) else {
        let _ = linux_close(client_b_fd);
        return false;
    };
    let second_cycle_ok = unix_socket_write(client_b_pair_slot as usize, false, b"again") == Ok(5);
    let mut again = [0u8; 5];
    let second_cycle_read_ok =
        unix_socket_read(accepted_b_pair_slot, true, &mut again) == Ok(5) && &again == b"again";
    let accepted_b_fd = install_unix_socket_fd(accepted_b_pair_slot as u32, true, false);
    let _ = linux_close(client_b_fd);
    if let Some(accepted_b_fd) = accepted_b_fd {
        let _ = linux_close(accepted_b_fd);
    } else {
        UNIX_PAIRS.lock()[accepted_b_pair_slot] = UnixPair::EMPTY;
    }
    let _ = linux_close(server_fd);

    bind_ok
        && duplicate_bind_rejected
        && listen_ok
        && refused_before_anyone_connects
        && connect_a_ok
        && same_pair_a
        && client_to_server_ok
        && server_reads_ok
        && server_to_client_ok
        && client_reads_ok
        && connect_b_ok
        && second_cycle_ok
        && second_cycle_read_ok
}

fn linux_pipe2(fds_address: u64, flags: u64) -> u64 {
    if flags & !(O_CLOEXEC | 0x800) != 0 {
        return error(22);
    }
    if !user::range_accessible(fds_address, 8, true) {
        return error(14);
    }
    let close_on_exec = flags & O_CLOEXEC != 0;
    let slot = {
        let mut pipes = PIPES.lock();
        let Some(slot) = pipes.iter().position(|pipe| !pipe.used) else {
            return error(24);
        };
        pipes[slot] = Pipe {
            used: true,
            read_open: true,
            write_open: true,
            ..Pipe::EMPTY
        };
        slot
    };
    let Some(read_fd) = install_process_fd_kind(
        slot as u32,
        close_on_exec,
        false,
        true,
        false,
        false,
        false,
        false,
        true,
        false,
        false,
    ) else {
        PIPES.lock()[slot] = Pipe::EMPTY;
        return error(24);
    };
    let Some(write_fd) = install_process_fd_kind(
        slot as u32,
        close_on_exec,
        false,
        true,
        false,
        false,
        false,
        false,
        false,
        true,
        false,
    ) else {
        let _ = remove_process_fd(read_fd);
        PIPES.lock()[slot] = Pipe::EMPTY;
        return error(24);
    };
    let mut encoded = [0u8; 8];
    encoded[..4].copy_from_slice(&(read_fd as u32).to_le_bytes());
    encoded[4..].copy_from_slice(&(write_fd as u32).to_le_bytes());
    if !user::copy_to_user(fds_address, &encoded) {
        let _ = remove_process_fd(read_fd);
        let _ = remove_process_fd(write_fd);
        PIPES.lock()[slot] = Pipe::EMPTY;
        return error(14);
    }
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_socket(domain: u64, kind: u64, protocol: u64) -> u64 {
    let base_kind = kind & 0xf;
    if domain == 1 {
        // AF_UNIX: only SOCK_STREAM, and only as an unbound socket that
        // `bind()`+`listen()`+`accept()` or `connect()` gives real meaning
        // to afterward - see `UnixListener`'s doc comment for what `bind()`
        // actually does here (no filesystem entry, an in-kernel name table).
        if base_kind != 1 || kind & !(0xf | 0x800 | O_CLOEXEC) != 0 || protocol != 0 {
            return error(93);
        }
        let Some(descriptor) = install_unbound_unix_socket_fd(kind & O_CLOEXEC != 0) else {
            return error(24);
        };
        SOCKET_CALLS.fetch_add(1, Ordering::Relaxed);
        return descriptor;
    }
    if domain != 2 && domain != 10 {
        return error(97);
    }
    let inet6 = domain == 10;
    if base_kind == 1 {
        if kind & !(0xf | 0x800 | O_CLOEXEC) != 0 || protocol != 0 && protocol != 6 {
            return error(93);
        }
        return match tcpsock::create(kind & O_CLOEXEC != 0, kind & 0x800 != 0, inet6) {
            Ok(descriptor) => {
                SOCKET_CALLS.fetch_add(1, Ordering::Relaxed);
                descriptor
            }
            Err(failure) => error(failure),
        };
    }
    if base_kind != 2 || kind & !(0xf | 0x800 | O_CLOEXEC) != 0 || protocol != 0 && protocol != 17 {
        return error(93);
    }
    match create_udp_socket(kind & O_CLOEXEC != 0, kind & 0x800 != 0, inet6) {
        Ok(descriptor) => {
            SOCKET_CALLS.fetch_add(1, Ordering::Relaxed);
            descriptor
        }
        Err(failure) => error(failure),
    }
}

fn create_udp_socket(close_on_exec: bool, nonblocking: bool, inet6: bool) -> Result<u64, u64> {
    let mut sockets = SOCKETS.lock();
    let index = sockets
        .iter()
        .position(|socket| !socket.open)
        .ok_or(24u64)?;
    let binding = crate::udp::bind_socket(0)?;
    crate::tcpnet::activate();
    sockets[index] = UdpSocket {
        open: true,
        connected: false,
        local_port: crate::udp::port_of(binding).unwrap_or(0),
        remote_port: 0,
        remote: [0; 16],
        binding,
        options: crate::sockopt::Options::DEFAULT,
    };
    drop(sockets);
    let Some(descriptor) = install_process_fd(index as u32, close_on_exec, true, true, true, false)
    else {
        crate::udp::release(binding);
        SOCKETS.lock()[index] = UdpSocket::EMPTY;
        return Err(24);
    };
    {
        let mut descriptors = PROCESS_FDS.lock();
        descriptors[descriptor as usize].nonblocking = nonblocking;
        descriptors[descriptor as usize].inet6 = inet6;
    }
    Ok(descriptor)
}

fn reset_udp_sockets() {
    let mut sockets = SOCKETS.lock();
    for socket in sockets.iter_mut() {
        if socket.open {
            crate::udp::release(socket.binding);
        }
        *socket = UdpSocket::EMPTY;
    }
}

/// Ports below 1024 need `CAP_NET_BIND_SERVICE`.
fn bind_port_allowed(port: u16) -> bool {
    port >= 1024 || has_capability(crate::capability::CAP_NET_BIND_SERVICE)
}

fn bind_udp_port(process_fd: ProcessFd, port: u16) -> u64 {
    if !bind_port_allowed(port) {
        return error(13);
    }
    let mut sockets = SOCKETS.lock();
    let Some(socket) = sockets
        .get_mut(process_fd.handle as usize)
        .filter(|socket| socket.open)
    else {
        return error(9);
    };
    if port == 0 {
        return 0;
    }
    match crate::udp::rebind(socket.binding, port) {
        Ok(()) => {
            socket.local_port = port;
            0
        }
        Err(failure) => error(failure),
    }
}

/// Waits for a datagram for `socket` (from its connected peer only, when it
/// has one) unless the call must not block; a blocking call gives up with
/// EAGAIN after the socket's receive time-out. `peek` leaves it queued.
#[cfg(feature = "boot-test")]
fn udp_receive(
    socket: &UdpSocket,
    nonblocking: bool,
    out: &mut [u8],
) -> Result<crate::net::UdpDatagram, u64> {
    udp_receive_with(socket, nonblocking, out, false).map(|(datagram, _)| datagram)
}

fn udp_receive_with(
    socket: &UdpSocket,
    nonblocking: bool,
    out: &mut [u8],
    peek: bool,
) -> Result<(crate::net::UdpDatagram, usize), u64> {
    let filter = socket
        .connected
        .then_some((socket.remote, socket.remote_port));
    let timeout = socket.options.receive_timeout_ns;
    let started = crate::time::monotonic_nanoseconds();
    loop {
        crate::tcpnet::poll();
        let received = if peek {
            crate::udp::peek(socket.binding, filter, out)
        } else {
            crate::udp::take(socket.binding, filter, out)
        };
        if let Some(received) = received {
            return Ok((
                crate::net::UdpDatagram {
                    bytes: received.length,
                    source: received.source,
                    source_port: received.source_port,
                },
                received.full,
            ));
        }
        if nonblocking {
            return Err(11);
        }
        if interrupted() {
            return Err(4);
        }
        if timeout != 0 && crate::time::monotonic_nanoseconds().saturating_sub(started) >= timeout {
            return Err(11);
        }
        yield_in_syscall();
    }
}

/// `socketpair(AF_UNIX, SOCK_STREAM, 0, fds)`: like `linux_socket`'s
/// `AF_UNIX` case, but skips straight past `bind()`/`listen()`/`connect()`/
/// `accept()` to hand back two ends of a fresh `UnixPair` already connected
/// to each other.
fn linux_socketpair(domain: u64, kind: u64, protocol: u64, fds_address: u64) -> u64 {
    if domain != 1 {
        return error(97);
    }
    let base_kind = kind & 0xf;
    if base_kind != 1 || kind & !(0xf | 0x800 | O_CLOEXEC) != 0 || protocol != 0 {
        return error(93);
    }
    if !user::range_accessible(fds_address, 8, true) {
        return error(14);
    }
    let mut pairs = UNIX_PAIRS.lock();
    let Some(slot) = pairs.iter().position(|pair| !pair.used) else {
        return error(24);
    };
    pairs[slot] = UnixPair {
        used: true,
        end_a_open: true,
        end_b_open: true,
        ..UnixPair::EMPTY
    };
    drop(pairs);
    let close_on_exec = kind & O_CLOEXEC != 0;
    let Some(fd_a) = install_unix_socket_fd(slot as u32, false, close_on_exec) else {
        UNIX_PAIRS.lock()[slot] = UnixPair::EMPTY;
        return error(24);
    };
    let Some(fd_b) = install_unix_socket_fd(slot as u32, true, close_on_exec) else {
        let _ = linux_close(fd_a);
        return error(24);
    };
    if !user::copy_to_user(fds_address, &(fd_a as u32).to_le_bytes())
        || !user::copy_to_user(fds_address + 4, &(fd_b as u32).to_le_bytes())
    {
        let _ = linux_close(fd_a);
        let _ = linux_close(fd_b);
        return error(14);
    }
    SOCKET_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_connect(descriptor: u64, address: u64, length: u64) -> u64 {
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return error(9);
    };
    if process_fd.unix_socket && process_fd.handle == UNIX_UNBOUND {
        return unix_connect(descriptor, address, length);
    }
    if process_fd.tcp {
        return tcpsock::connect(process_fd, address, length);
    }
    if !process_fd.socket {
        return error(88);
    }
    let (remote, port) = match read_sockaddr(address, length, process_fd.inet6) {
        Ok(target) => target,
        Err(failure) => return failure,
    };
    let mut sockets = SOCKETS.lock();
    let Some(socket) = sockets.get_mut(process_fd.handle as usize) else {
        return error(9);
    };
    if !socket.open {
        return error(9);
    }
    socket.remote = remote;
    socket.remote_port = port;
    socket.connected = true;
    SOCKET_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn read_unix_name(address: u64, length: u64) -> Option<([u8; UNIX_NAME_CAP], usize)> {
    if length < 2 {
        return None;
    }
    let path_len = usize::try_from(length - 2).ok()?;
    if path_len == 0 || path_len > UNIX_NAME_CAP {
        return None;
    }
    let mut family = [0u8; 2];
    if !user::copy_from_user(address, &mut family) || u16::from_le_bytes(family) != 1 {
        return None;
    }
    let mut name = [0u8; UNIX_NAME_CAP];
    if !user::copy_from_user(address + 2, &mut name[..path_len]) {
        return None;
    }
    // A C caller's `sockaddr_un` is usually oversized with the path
    // NUL-terminated partway through, rather than `length` covering the
    // exact string - trim at the first NUL the same way the kernel would.
    let trimmed = name[..path_len]
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(path_len);
    if trimmed == 0 {
        return None;
    }
    Some((name, trimmed))
}

/// Core of `bind()` for `AF_UNIX`: claims `name` in the in-kernel listener
/// table (see `UnixListener`'s doc comment - there is no filesystem entry)
/// and turns `fd_index`'s socket into a listener fd pointing at it. Public
/// to this module's self-test the same way `epoll_ctl_apply` is.
fn unix_bind_apply(fd_index: usize, name: &[u8]) -> u64 {
    let process_fd = PROCESS_FDS.lock()[fd_index];
    if !process_fd.open || !process_fd.unix_socket || process_fd.handle != UNIX_UNBOUND {
        return error(88);
    }
    let mut listeners = UNIX_LISTENERS.lock();
    if listeners
        .iter()
        .any(|listener| listener.used && listener.name() == name)
    {
        return error(98);
    }
    let Some(slot) = listeners.iter().position(|listener| !listener.used) else {
        return error(24);
    };
    let mut stored_name = [0u8; UNIX_NAME_CAP];
    stored_name[..name.len()].copy_from_slice(name);
    listeners[slot] = UnixListener {
        used: true,
        listening: false,
        name: stored_name,
        name_len: name.len() as u8,
        backlog: [None; UNIX_LISTENER_BACKLOG],
    };
    drop(listeners);
    let mut descriptors = PROCESS_FDS.lock();
    descriptors[fd_index].unix_socket = false;
    descriptors[fd_index].unix_listener = true;
    descriptors[fd_index].handle = slot as u32;
    // A listening socket isn't `read()`/`write()`-able itself - only
    // `accept()`'s children are.
    descriptors[fd_index].readable = false;
    descriptors[fd_index].writable = false;
    0
}

fn unix_listen_apply(fd_index: usize) -> u64 {
    let process_fd = PROCESS_FDS.lock()[fd_index];
    if !process_fd.open || !process_fd.unix_listener {
        return error(88);
    }
    let mut listeners = UNIX_LISTENERS.lock();
    let Some(listener) = listeners.get_mut(process_fd.handle as usize) else {
        return error(9);
    };
    if !listener.used {
        return error(9);
    }
    listener.listening = true;
    0
}

/// Core of `connect()` for an unbound `AF_UNIX` socket: finds the listener
/// bound to `name`, creates a fresh `UnixPair`, gives `fd_index` end A right
/// away, and queues end B on the listener's backlog for its `accept()`.
fn unix_connect_apply(fd_index: usize, name: &[u8]) -> u64 {
    let process_fd = PROCESS_FDS.lock()[fd_index];
    if !process_fd.open || !process_fd.unix_socket || process_fd.handle != UNIX_UNBOUND {
        return error(88);
    }
    let listener_slot = {
        let listeners = UNIX_LISTENERS.lock();
        listeners
            .iter()
            .position(|listener| listener.used && listener.listening && listener.name() == name)
    };
    let Some(listener_slot) = listener_slot else {
        return error(111);
    };
    let mut pairs = UNIX_PAIRS.lock();
    let Some(pair_slot) = pairs.iter().position(|pair| !pair.used) else {
        return error(24);
    };
    pairs[pair_slot] = UnixPair {
        used: true,
        end_a_open: true,
        end_b_open: true,
        ..UnixPair::EMPTY
    };
    drop(pairs);
    let queued = {
        let mut listeners = UNIX_LISTENERS.lock();
        match listeners.get_mut(listener_slot) {
            Some(listener) if listener.used && listener.listening => {
                match listener.backlog.iter().position(|entry| entry.is_none()) {
                    Some(backlog_slot) => {
                        listener.backlog[backlog_slot] = Some(pair_slot);
                        true
                    }
                    None => false,
                }
            }
            _ => false,
        }
    };
    if !queued {
        UNIX_PAIRS.lock()[pair_slot] = UnixPair::EMPTY;
        return error(111);
    }
    let mut descriptors = PROCESS_FDS.lock();
    descriptors[fd_index].unix_end_b = false;
    descriptors[fd_index].handle = pair_slot as u32;
    descriptors[fd_index].readable = true;
    descriptors[fd_index].writable = true;
    0
}

fn unix_connect(descriptor: u64, address: u64, length: u64) -> u64 {
    let Some(index) = process_fd_index(descriptor) else {
        return error(9);
    };
    let Some((name, name_len)) = read_unix_name(address, length) else {
        return error(22);
    };
    let result = unix_connect_apply(index, &name[..name_len]);
    if result == 0 {
        SOCKET_CALLS.fetch_add(1, Ordering::Relaxed);
    }
    result
}

fn linux_bind(descriptor: u64, address: u64, length: u64) -> u64 {
    if let Some(process_fd) = tcpsock::lookup(descriptor) {
        return tcpsock::bind(process_fd, address, length);
    }
    if let Some(process_fd) = lookup_socket_fd(descriptor) {
        let Some(port) = tcpsock::bind_port(address, length, process_fd.inet6) else {
            return error(22);
        };
        return bind_udp_port(process_fd, port);
    }
    let Some(index) = process_fd_index(descriptor) else {
        return error(9);
    };
    let Some((name, name_len)) = read_unix_name(address, length) else {
        return error(22);
    };
    unix_bind_apply(index, &name[..name_len])
}

fn linux_listen(descriptor: u64, _backlog: u64) -> u64 {
    if let Some(process_fd) = tcpsock::lookup(descriptor) {
        return tcpsock::listen(process_fd);
    }
    let Some(index) = process_fd_index(descriptor) else {
        return error(9);
    };
    unix_listen_apply(index)
}

/// Core of `accept()`/`accept4()`: pops a pending connection from the
/// listener's backlog and returns the `UnixPair` slot for its end B.
/// Bounded cooperative-retry, not a real blocking wait queue - the same
/// deliberate simplification as `pipe_read`/`epoll_wait` (see `Pipe`'s doc
/// comment), for the same reason: no wait-queue primitive exists yet.
fn unix_accept_apply(listener_handle: u32) -> Option<usize> {
    let mut listeners = UNIX_LISTENERS.lock();
    let listener = listeners.get_mut(listener_handle as usize)?;
    if !listener.used || !listener.listening {
        return None;
    }
    let backlog_slot = listener.backlog.iter().position(|entry| entry.is_some())?;
    listener.backlog[backlog_slot].take()
}

fn linux_accept(descriptor: u64, address: u64, length: u64, flags: u64) -> u64 {
    if flags & !(O_CLOEXEC | 0x800) != 0 {
        return error(22);
    }
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return error(9);
    };
    if process_fd.tcp {
        return tcpsock::accept(process_fd, address, length, flags);
    }
    if !process_fd.unix_listener {
        return error(88);
    }
    // No meaningful peer address for an abstract-namespace-only AF_UNIX
    // peer (see `UnixListener`'s doc comment) - report a zero-length one,
    // like a real unnamed AF_UNIX socket's peer address.
    if length != 0 {
        if !user::range_accessible(length, 4, true)
            || !user::copy_to_user(length, &0u32.to_le_bytes())
        {
            return error(14);
        }
    } else if address != 0 {
        return error(22);
    }
    let close_on_exec = flags & O_CLOEXEC != 0;
    for attempt in 0..PIPE_WAIT_ITERATIONS {
        if let Some(pair_slot) = unix_accept_apply(process_fd.handle) {
            let Some(new_fd) = install_unix_socket_fd(pair_slot as u32, true, close_on_exec) else {
                UNIX_PAIRS.lock()[pair_slot] = UnixPair::EMPTY;
                return error(24);
            };
            SOCKET_CALLS.fetch_add(1, Ordering::Relaxed);
            return new_fd;
        }
        if attempt + 1 < PIPE_WAIT_ITERATIONS {
            yield_in_syscall();
        }
    }
    error(11)
}

/// The filesystem keeps no per-file timestamps (every `modified` is 0), so
/// this only checks that the path exists and reports success.
fn linux_utimensat(directory: u64, path_address: u64) -> u64 {
    let mut resolved = [0u8; MAX_PATH];
    let (length, _) = match resolve_user_path(directory, path_address, &mut resolved) {
        Ok(value) => value,
        Err(failure) => return failure,
    };
    let Ok(path) = core::str::from_utf8(&resolved[..length]) else {
        return error(84);
    };
    match vfs::metadata(path) {
        Ok(_) => {
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            0
        }
        Err(failure) => vfs_error(failure),
    }
}

/// Only `PRIO_PROCESS` is supported. The raw syscall reports `20 - nice`.
/// Names of the commonly seen Linux syscalls, for the `strace` listing.
pub(crate) fn linux_syscall_name(number: u64) -> &'static str {
    match number {
        LINUX_READ => "read",
        LINUX_WRITE => "write",
        LINUX_SETTIMEOFDAY => "settimeofday",
        LINUX_REBOOT => "reboot",
        LINUX_CLOCK_SETTIME => "clock_settime",
        LINUX_MOUNT => "mount",
        LINUX_UMOUNT2 => "umount2",
        LINUX_OPEN => "open",
        LINUX_CLOSE => "close",
        LINUX_STAT => "stat",
        LINUX_FSTAT => "fstat",
        LINUX_POLL => "poll",
        LINUX_LSEEK => "lseek",
        LINUX_MMAP => "mmap",
        LINUX_MPROTECT => "mprotect",
        LINUX_MUNMAP => "munmap",
        LINUX_BRK => "brk",
        LINUX_RT_SIGACTION => "rt_sigaction",
        LINUX_RT_SIGPROCMASK => "rt_sigprocmask",
        LINUX_IOCTL => "ioctl",
        LINUX_PIPE => "pipe",
        LINUX_MINCORE => "mincore",
        LINUX_FLOCK => "flock",
        LINUX_FCHDIR => "fchdir",
        LINUX_PERSONALITY => "personality",
        LINUX_SCHED_SETSCHEDULER => "sched_setscheduler",
        LINUX_SYNC => "sync",
        LINUX_SYNCFS => "syncfs",
        LINUX_FALLOCATE => "fallocate",
        LINUX_PREADV => "preadv",
        LINUX_PWRITEV => "pwritev",
        LINUX_MEMBARRIER => "membarrier",
        LINUX_CLOSE_RANGE => "close_range",
        LINUX_EVENTFD | LINUX_EVENTFD2 => "eventfd",
        LINUX_SENDMSG => "sendmsg",
        LINUX_RECVMSG => "recvmsg",
        LINUX_MEMFD_CREATE => "memfd_create",
        LINUX_SENDFILE => "sendfile",
        LINUX_COPY_FILE_RANGE => "copy_file_range",
        LINUX_TIMERFD_CREATE => "timerfd_create",
        LINUX_TIMERFD_SETTIME => "timerfd_settime",
        LINUX_TIMERFD_GETTIME => "timerfd_gettime",
        LINUX_DUP => "dup",
        LINUX_DUP2 => "dup2",
        LINUX_NANOSLEEP => "nanosleep",
        LINUX_GETPID => "getpid",
        LINUX_SOCKET => "socket",
        LINUX_CONNECT => "connect",
        LINUX_BIND => "bind",
        LINUX_FORK => "fork",
        LINUX_CLONE => "clone",
        LINUX_CLONE3 => "clone3",
        LINUX_EXECVE => "execve",
        LINUX_EXIT => "exit",
        LINUX_WAIT4 => "wait4",
        LINUX_KILL => "kill",
        LINUX_UNAME => "uname",
        LINUX_FCNTL => "fcntl",
        LINUX_GETCWD => "getcwd",
        LINUX_CHDIR => "chdir",
        LINUX_MKDIR => "mkdir",
        LINUX_UNLINK => "unlink",
        LINUX_ARCH_PRCTL => "arch_prctl",
        LINUX_GETTID => "gettid",
        LINUX_FUTEX => "futex",
        LINUX_PTRACE => "ptrace",
        LINUX_SET_TID_ADDRESS => "set_tid_address",
        LINUX_CLOCK_GETTIME => "clock_gettime",
        LINUX_EXIT_GROUP => "exit_group",
        LINUX_OPENAT => "openat",
        LINUX_NEWFSTATAT => "newfstatat",
        LINUX_SET_ROBUST_LIST => "set_robust_list",
        LINUX_GET_ROBUST_LIST => "get_robust_list",
        LINUX_PRLIMIT64 => "prlimit64",
        LINUX_GETRANDOM => "getrandom",
        LINUX_RSEQ => "rseq",
        LINUX_SECCOMP => "seccomp",
        LINUX_PRCTL => "prctl",
        LINUX_SELECT => "select",
        LINUX_PPOLL => "ppoll",
        _ => "?",
    }
}

fn linux_getpriority(which: u64, who: u64) -> u64 {
    if which > 2 {
        return error(22);
    }
    if which != 0 {
        return error(3);
    }
    let target = if who == 0 {
        crate::scheduler::current_task_id()
    } else {
        who
    };
    match crate::scheduler::nice_of(target) {
        Some(nice) => (20 - i64::from(nice)) as u64,
        None => error(3),
    }
}

fn linux_setpriority(which: u64, who: u64, value: u64) -> u64 {
    if which > 2 {
        return error(22);
    }
    if which != 0 {
        return error(3);
    }
    let target = if who == 0 {
        crate::scheduler::current_task_id()
    } else {
        who
    };
    let nice = (value as i64).clamp(-20, 19) as i8;
    let raises_priority = crate::scheduler::nice_of(target).is_some_and(|current| nice < current);
    if raises_priority && !has_capability(crate::capability::CAP_SYS_NICE) {
        return error(1);
    }
    if crate::scheduler::set_nice(target, nice) {
        COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
        0
    } else {
        error(3)
    }
}

fn current_capabilities() -> crate::capability::Capabilities {
    crate::capability::Capabilities {
        effective: CAP_EFFECTIVE.load(Ordering::Acquire),
        permitted: CAP_PERMITTED.load(Ordering::Acquire),
    }
}

fn set_capabilities(capabilities: crate::capability::Capabilities) {
    CAP_EFFECTIVE.store(capabilities.effective, Ordering::Release);
    CAP_PERMITTED.store(capabilities.permitted, Ordering::Release);
}

fn has_capability(capability: u32) -> bool {
    current_capabilities().has(capability)
}

/// Drops capabilities for the duration of a call and checks that each
/// gated operation then refuses with EPERM and succeeds again afterwards.
pub(crate) fn capability_enforcement_self_test() -> bool {
    use crate::capability::{
        ALL, CAP_CHOWN, CAP_NET_BIND_SERVICE, CAP_SYS_ADMIN, CAP_SYS_BOOT, CAP_SYS_NICE,
        CAP_SYS_TIME,
    };
    let saved = current_capabilities();
    let without = |capability: u32| ALL & !(1 << capability);

    set_capabilities(crate::capability::Capabilities {
        effective: without(CAP_CHOWN),
        permitted: ALL,
    });
    let chown_refused = linux_fchown(1) == error(1) && linux_fchownat(0, 0, 0) == error(1);

    set_capabilities(crate::capability::Capabilities {
        effective: without(CAP_SYS_NICE),
        permitted: ALL,
    });
    let nice_refused = linux_setpriority(0, 0, (-5i64) as u64) == error(1);

    set_capabilities(saved);
    let chown_allowed = linux_fchown(1) != error(1);

    set_capabilities(crate::capability::Capabilities {
        effective: without(CAP_SYS_TIME),
        permitted: ALL,
    });
    let time_refused = linux_clock_settime(0, 0) == error(1)
        && linux_settimeofday(1, 0) == error(1)
        && linux_settimeofday(0, 0) == 0;

    set_capabilities(crate::capability::Capabilities {
        effective: without(CAP_SYS_BOOT),
        permitted: ALL,
    });
    let boot_refused = linux_reboot(REBOOT_MAGIC, REBOOT_MAGIC2[0], 0x0123_4567) == error(1);

    set_capabilities(crate::capability::Capabilities {
        effective: without(CAP_SYS_ADMIN),
        permitted: ALL,
    });
    let admin_refused = linux_mount(0, 0, MS_BIND) == error(1) && linux_umount2(0) == error(1);

    set_capabilities(crate::capability::Capabilities {
        effective: without(CAP_NET_BIND_SERVICE),
        permitted: ALL,
    });
    let low_port_refused = !bind_port_allowed(80)
        && !bind_port_allowed(1023)
        && bind_port_allowed(1024)
        && bind_port_allowed(8080);

    set_capabilities(saved);
    let allowed_again = bind_port_allowed(80)
        && linux_reboot(REBOOT_MAGIC, 1, 0x0123_4567) == error(22)
        && linux_reboot(1, REBOOT_MAGIC2[0], 0x0123_4567) == error(22)
        && linux_reboot(REBOOT_MAGIC, REBOOT_MAGIC2[0], 0x0123_4568) == error(22)
        && linux_clock_settime(1, 0) == error(22)
        && linux_clock_settime(0, 0) == error(14)
        && linux_mount(0, 0, 0) == error(19);
    chown_refused
        && nice_refused
        && chown_allowed
        && time_refused
        && boot_refused
        && admin_refused
        && low_port_refused
        && allowed_again
}

/// Reads the `__user_cap_header_struct` and checks it names this process.
fn read_capability_header(header: u64) -> Result<(), u64> {
    let mut raw = [0u8; 8];
    if !user::copy_from_user(header, &mut raw) {
        return Err(error(14));
    }
    let version = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    if version != CAPABILITY_VERSION_3 {
        let _ = user::copy_to_user(header, &CAPABILITY_VERSION_3.to_le_bytes());
        return Err(error(22));
    }
    let pid = i32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
    if pid != 0 && pid as u64 != crate::scheduler::current_task_id() {
        return Err(error(3));
    }
    Ok(())
}

fn linux_capget(header: u64, data: u64) -> u64 {
    if let Err(failure) = read_capability_header(header) {
        return failure;
    }
    if data != 0 && !user::copy_to_user(data, &current_capabilities().to_data()) {
        return error(14);
    }
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

/// A process can only give capabilities up: the new permitted set must be
/// inside the current one, and the effective set inside the new permitted one.
fn linux_capset(header: u64, data: u64) -> u64 {
    if let Err(failure) = read_capability_header(header) {
        return failure;
    }
    let mut requested = [0u8; 24];
    if !user::copy_from_user(data, &mut requested) {
        return error(14);
    }
    let (effective, permitted) = crate::capability::Capabilities::from_data(&requested);
    match current_capabilities().restrict(effective, permitted) {
        Some(capabilities) => {
            set_capabilities(capabilities);
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            0
        }
        None => error(1),
    }
}
fn linux_sched_setaffinity(pid: u64, size: u64, address: u64) -> u64 {
    if pid != 0 && pid != current_process_id() {
        return error(3);
    }
    if size < 8 {
        return error(22);
    }
    let mut mask = [0u8; 8];
    if !user::copy_from_user(address, &mut mask) {
        return error(14);
    }
    if u64::from_le_bytes(mask) & crate::smp::online_mask() == 0 {
        return error(22);
    }
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn is_socket_fd(process_fd: &ProcessFd) -> bool {
    process_fd.socket || process_fd.unix_socket || process_fd.unix_listener || process_fd.tcp
}

/// A socket address as Linux lays it out: `sockaddr_in` (16 bytes) or, for an
/// `AF_INET6` socket, `sockaddr_in6` (28 bytes); an IPv4 address on an
/// `AF_INET6` socket is the IPv4-mapped one.
fn sockaddr_bytes(address: &crate::ip::Address, port: u16, v6: bool) -> ([u8; 28], usize) {
    let mut bytes = [0u8; 28];
    if v6 {
        bytes[..2].copy_from_slice(&10u16.to_le_bytes());
        bytes[2..4].copy_from_slice(&port.to_be_bytes());
        bytes[8..24].copy_from_slice(address);
        (bytes, 28)
    } else {
        bytes[..2].copy_from_slice(&2u16.to_le_bytes());
        bytes[2..4].copy_from_slice(&port.to_be_bytes());
        bytes[4..8].copy_from_slice(&crate::ip::as_v4(address).unwrap_or([0; 4]));
        (bytes, 16)
    }
}

/// Value of a `SOL_SOCKET` option this kernel can answer truthfully; `None`
/// means the option is not supported.
fn socket_option_value(datagram: bool, listening: bool, option: u64) -> Option<u32> {
    match option {
        SO_TYPE => Some(if datagram { 2 } else { 1 }),
        SO_ERROR => Some(0),
        SO_ACCEPTCONN => Some(u32::from(listening)),
        _ => None,
    }
}

fn linux_shutdown(descriptor: u64, how: u64) -> u64 {
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return error(9);
    };
    if !is_socket_fd(&process_fd) {
        return error(88);
    }
    if how > 2 {
        return error(22);
    }
    if process_fd.tcp {
        return tcpsock::shutdown(process_fd, how);
    }
    SOCKET_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_setsockopt(arguments: [u64; 6]) -> u64 {
    let [descriptor, level, option, value, length, _] = arguments;
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return error(9);
    };
    if !is_socket_fd(&process_fd) {
        return error(88);
    }
    let Ok(length) = usize::try_from(length) else {
        return error(22);
    };
    if length > 64 || (length > 0 && !user::range_accessible(value, length, false)) {
        return error(14);
    }
    let mut data = [0u8; 64];
    if length > 0 && !user::copy_from_user(value, &mut data[..length]) {
        return error(14);
    }
    SOCKET_CALLS.fetch_add(1, Ordering::Relaxed);
    if process_fd.tcp {
        return tcpsock::set_option(&process_fd, level, option, &data[..length]);
    }
    if process_fd.socket && !process_fd.unix_socket {
        let mut sockets = SOCKETS.lock();
        let Some(socket) = sockets
            .get_mut(process_fd.handle as usize)
            .filter(|socket| socket.open)
        else {
            return error(9);
        };
        if level == crate::sockopt::IPPROTO_IPV6 && !process_fd.inet6 {
            return error(92);
        }
        return match socket.options.set(level, option, &data[..length]) {
            Ok(()) => 0,
            Err(failure) => error(failure),
        };
    }
    0
}

fn linux_getsockopt(arguments: [u64; 6]) -> u64 {
    let [descriptor, level, option, value, length_address, _] = arguments;
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return error(9);
    };
    if !is_socket_fd(&process_fd) {
        return error(88);
    }
    let integer = |value: u32| {
        let mut bytes = [0u8; 16];
        bytes[..4].copy_from_slice(&value.to_le_bytes());
        Some((bytes, 4))
    };
    let answer = if process_fd.tcp {
        tcpsock::option_bytes(&process_fd, level, option)
    } else if process_fd.socket && !process_fd.unix_socket {
        let options = SOCKETS
            .lock()
            .get(process_fd.handle as usize)
            .filter(|socket| socket.open)
            .map(|socket| socket.options);
        let Some(options) = options else {
            return error(9);
        };
        match (level, option) {
            (SOL_SOCKET, SO_TYPE) => integer(2),
            (SOL_SOCKET, SO_ERROR) => integer(0),
            (SOL_SOCKET, SO_ACCEPTCONN) => integer(0),
            (SOL_SOCKET, 39) => integer(if process_fd.inet6 { 10 } else { 2 }),
            (SOL_SOCKET, 38) => integer(17),
            (crate::sockopt::IPPROTO_IPV6, _) if !process_fd.inet6 => None,
            _ => options.get(level, option),
        }
    } else if level == SOL_SOCKET {
        socket_option_value(process_fd.socket, process_fd.unix_listener, option).and_then(integer)
    } else {
        None
    };
    let Some((bytes, size)) = answer else {
        return error(92);
    };
    let mut capacity = [0u8; 4];
    if !user::copy_from_user(length_address, &mut capacity) {
        return error(14);
    }
    let count = (u32::from_le_bytes(capacity) as usize).min(size);
    if !user::copy_to_user(value, &bytes[..count])
        || !user::copy_to_user(length_address, &(size as u32).to_le_bytes())
    {
        return error(14);
    }
    SOCKET_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

/// `getsockname` (`peer == false`) and `getpeername` (`peer == true`).
fn linux_getsockname(descriptor: u64, address: u64, length_address: u64, peer: bool) -> u64 {
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return error(9);
    };
    if !is_socket_fd(&process_fd) {
        return error(88);
    }
    let (bytes, size): ([u8; 28], usize) = if process_fd.tcp {
        match tcpsock::address(&process_fd, peer) {
            Ok(found) => found,
            Err(failure) => return error(failure),
        }
    } else if process_fd.socket {
        let sockets = SOCKETS.lock();
        let Some(socket) = sockets.get(process_fd.handle as usize).filter(|s| s.open) else {
            return error(9);
        };
        if peer {
            if !socket.connected {
                return error(107);
            }
            sockaddr_bytes(&socket.remote, socket.remote_port, process_fd.inet6)
        } else {
            let unspecified = if process_fd.inet6 {
                crate::ip::UNSPECIFIED
            } else {
                crate::ip::v4([0; 4])
            };
            sockaddr_bytes(&unspecified, socket.local_port, process_fd.inet6)
        }
    } else {
        if peer {
            return error(107);
        }
        let mut unnamed = [0u8; 28];
        unnamed[..2].copy_from_slice(&1u16.to_le_bytes());
        (unnamed, 2)
    };
    let mut capacity = [0u8; 4];
    if !user::copy_from_user(length_address, &mut capacity) {
        return error(14);
    }
    let count = (u32::from_le_bytes(capacity) as usize).min(size);
    if !user::copy_to_user(address, &bytes[..count])
        || !user::copy_to_user(length_address, &(size as u32).to_le_bytes())
    {
        return error(14);
    }
    SOCKET_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

pub(crate) fn socket_helpers_self_test() -> bool {
    let (address, size) = sockaddr_bytes(&crate::ip::v4([10, 0, 2, 15]), 8080, false);
    let (address6, size6) = sockaddr_bytes(&crate::ip::LOOPBACK6, 8080, true);
    let (mapped, _) = sockaddr_bytes(&crate::ip::v4([10, 0, 2, 15]), 80, true);
    let encoded = size == 16
        && address[..2] == [2, 0]
        && address[2..4] == [0x1f, 0x90]
        && address[4..8] == [10, 0, 2, 15]
        && address[8..16] == [0; 8]
        && size6 == 28
        && address6[..2] == [10, 0]
        && address6[2..4] == [0x1f, 0x90]
        && address6[4..8] == [0; 4]
        && address6[8..23] == [0; 15]
        && address6[23] == 1
        && mapped[18..20] == [0xff, 0xff]
        && mapped[20..24] == [10, 0, 2, 15];
    let options = socket_option_value(true, false, SO_TYPE) == Some(2)
        && socket_option_value(false, false, SO_TYPE) == Some(1)
        && socket_option_value(false, true, SO_ACCEPTCONN) == Some(1)
        && socket_option_value(true, false, SO_ERROR) == Some(0)
        && socket_option_value(true, false, 999).is_none();
    let bad_fd = linux_shutdown(9999, 0) == error(9)
        && linux_setsockopt([9999, 1, 2, 0, 0, 0]) == error(9)
        && linux_getsockopt([9999, 1, 3, 0, 0, 0]) == error(9)
        && linux_getsockname(9999, 0, 0, false) == error(9);
    encoded && options && bad_fd
}

fn linux_sendto(arguments: [u64; 6]) -> u64 {
    let [
        descriptor,
        address,
        requested,
        flags,
        destination,
        destination_length,
    ] = arguments;
    if tcpsock::lookup(descriptor).is_some() {
        if flags & !0x4040 != 0 {
            return error(95);
        }
        return linux_write(descriptor, address, requested);
    }
    if flags & !(0x4000 | 0x40 | 0x800) != 0 {
        return error(95);
    }
    let Some(process_fd) = lookup_socket_fd(descriptor) else {
        return error(9);
    };
    let Ok(requested) = usize::try_from(requested) else {
        return error(90);
    };
    if requested > MAX_DATAGRAM || !user::range_accessible(address, requested, false) {
        return error(90);
    }
    let socket = {
        let sockets = SOCKETS.lock();
        let Some(socket) = sockets.get(process_fd.handle as usize).copied() else {
            return error(9);
        };
        if !socket.open {
            return error(9);
        }
        socket
    };
    let (remote, port) = if destination != 0 {
        match read_sockaddr(destination, destination_length, process_fd.inet6) {
            Ok(target) => target,
            Err(failure) => return failure,
        }
    } else if socket.connected {
        (socket.remote, socket.remote_port)
    } else {
        return error(89);
    };
    let mut payload = [0u8; MAX_DATAGRAM];
    if !user::copy_from_user(address, &mut payload[..requested]) {
        return error(14);
    }
    if !crate::net::send_datagram(&remote, socket.local_port, port, &payload[..requested]) {
        return error(5);
    }
    SOCKET_CALLS.fetch_add(1, Ordering::Relaxed);
    DATAGRAMS.fetch_add(1, Ordering::Relaxed);
    NETWORK_BYTES.fetch_add(requested as u64, Ordering::Relaxed);
    requested as u64
}

fn linux_recvfrom(arguments: [u64; 6]) -> u64 {
    let [
        descriptor,
        address,
        capacity,
        flags,
        source_address,
        source_length,
    ] = arguments;
    if let Some(process_fd) = tcpsock::lookup(descriptor) {
        if flags & !(0x2 | 0x40 | 0x100) != 0 {
            return error(95);
        }
        if flags & 0x40 != 0 && !tcpsock::has_data(&process_fd) {
            return error(11);
        }
        return tcpsock::receive(process_fd, address, capacity, flags);
    }
    if flags & !(0x2 | 0x20 | 0x40) != 0 {
        return error(95);
    }
    let Some(process_fd) = lookup_socket_fd(descriptor) else {
        return error(9);
    };
    let Ok(capacity) = usize::try_from(capacity) else {
        return error(22);
    };
    if capacity > MAX_DATAGRAM || !user::range_accessible(address, capacity, true) {
        return error(14);
    }
    let socket = {
        let sockets = SOCKETS.lock();
        let Some(socket) = sockets.get(process_fd.handle as usize).copied() else {
            return error(9);
        };
        if !socket.open {
            return error(9);
        }
        socket
    };
    let mut payload = [0u8; MAX_DATAGRAM];
    let (datagram, full) = match udp_receive_with(
        &socket,
        process_fd.nonblocking || flags & 0x40 != 0,
        &mut payload[..capacity],
        flags & 0x2 != 0,
    ) {
        Ok(found) => found,
        Err(failure) => return error(failure),
    };
    if !user::copy_to_user(address, &payload[..datagram.bytes]) {
        return error(14);
    }
    if source_address != 0 {
        if source_length == 0 {
            return error(14);
        }
        if !write_sockaddr(
            source_address,
            source_length,
            &datagram.source,
            datagram.source_port,
            process_fd.inet6,
        ) {
            return error(14);
        }
    }
    SOCKET_CALLS.fetch_add(1, Ordering::Relaxed);
    DATAGRAMS.fetch_add(1, Ordering::Relaxed);
    NETWORK_BYTES.fetch_add(datagram.bytes as u64, Ordering::Relaxed);
    if flags & 0x20 != 0 {
        full as u64
    } else {
        datagram.bytes as u64
    }
}

struct MessageHeader {
    name: u64,
    name_length: u32,
    vectors: u64,
    vector_count: u64,
    control_length: u64,
}

/// `struct msghdr` as x86-64 Linux lays it out (56 bytes).
fn read_message_header(address: u64) -> Option<MessageHeader> {
    let mut raw = [0u8; 56];
    if !user::copy_from_user(address, &mut raw) {
        return None;
    }
    Some(MessageHeader {
        name: read_array_u64(&raw, 0),
        name_length: u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]),
        vectors: read_array_u64(&raw, 16),
        vector_count: read_array_u64(&raw, 24),
        control_length: read_array_u64(&raw, 40),
    })
}

fn first_vector(header: &MessageHeader) -> Option<(u64, u64)> {
    (header.vector_count == 1)
        .then(|| read_iovec(header.vectors, 0))
        .flatten()
}

/// `sendmsg` for stream sockets (every vector goes out in order) and, for a
/// datagram socket, a message with a single vector. Ancillary data is not
/// supported.
fn linux_sendmsg(descriptor: u64, message: u64, flags: u64) -> u64 {
    let Some(header) = read_message_header(message) else {
        return error(14);
    };
    if header.control_length != 0 {
        return error(95);
    }
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return error(9);
    };
    if process_fd.socket && !process_fd.unix_socket {
        let Some((base, length)) = first_vector(&header) else {
            return error(22);
        };
        return linux_sendto([
            descriptor,
            base,
            length,
            flags,
            header.name,
            header.name_length as u64,
        ]);
    }
    if !is_socket_fd(&process_fd) || flags & !0x4040 != 0 {
        return error(if is_socket_fd(&process_fd) { 95 } else { 88 });
    }
    vectored_positional(
        header.vector_count,
        0,
        |index| read_iovec(header.vectors, index),
        |base, length, _| linux_write(descriptor, base, length),
    )
}

/// `recvmsg`: a stream socket fills the vectors from what is available (it
/// waits only for the first byte); a datagram socket takes one datagram into
/// the first vector.
fn linux_recvmsg(descriptor: u64, message: u64, flags: u64) -> u64 {
    let Some(header) = read_message_header(message) else {
        return error(14);
    };
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return error(9);
    };
    let received = if process_fd.socket && !process_fd.unix_socket {
        let Some((base, length)) = first_vector(&header) else {
            return error(22);
        };
        let name_length_address = message + 8;
        linux_recvfrom([
            descriptor,
            base,
            length,
            flags,
            header.name,
            if header.name != 0 {
                name_length_address
            } else {
                0
            },
        ])
    } else {
        if !is_socket_fd(&process_fd) {
            return error(88);
        }
        if flags & !0x40 != 0 {
            return error(95);
        }
        let mut first = true;
        vectored_positional(
            header.vector_count,
            0,
            |index| read_iovec(header.vectors, index),
            |base, length, _| {
                if !first && fd_ready_events(descriptor) & EPOLLIN == 0 {
                    return 0;
                }
                first = false;
                linux_read(descriptor, base, length)
            },
        )
    };
    if received as i64 >= 0 {
        let name_failed = (header.name == 0 || process_fd.tcp || process_fd.unix_socket)
            && !user::copy_to_user(message + 8, &0u32.to_le_bytes());
        if name_failed
            || !user::copy_to_user(message + 40, &0u64.to_le_bytes())
            || !user::copy_to_user(message + 48, &0u32.to_le_bytes())
        {
            return error(14);
        }
    }
    received
}

/// The peer address of a `connect` or `sendto`: a `sockaddr_in` for an
/// `AF_INET` socket, a `sockaddr_in6` for an `AF_INET6` one. The unspecified
/// IPv6 address means this machine, as on Linux.
fn read_sockaddr(address: u64, length: u64, v6: bool) -> Result<(crate::ip::Address, u16), u64> {
    let mut family = [0u8; 2];
    if length < 2 || !user::copy_from_user(address, &mut family) {
        return Err(error(22));
    }
    let family = u16::from_le_bytes(family);
    if family != if v6 { 10 } else { 2 } {
        return Err(error(97));
    }
    let mut encoded = [0u8; 28];
    let size = if v6 { 28 } else { 16 };
    if (length as usize) < size || !user::copy_from_user(address, &mut encoded[..size]) {
        return Err(error(22));
    }
    let port = u16::from_be_bytes([encoded[2], encoded[3]]);
    let remote = if v6 {
        let mut remote = [0u8; 16];
        remote.copy_from_slice(&encoded[8..24]);
        if remote == crate::ip::UNSPECIFIED {
            crate::ip::LOOPBACK6
        } else {
            remote
        }
    } else {
        let v4 = [encoded[4], encoded[5], encoded[6], encoded[7]];
        if v4 == [0; 4] || v4[0] >= 224 {
            return Err(error(22));
        }
        crate::ip::v4(v4)
    };
    if port == 0 || crate::ip::is_multicast(&remote) {
        return Err(error(22));
    }
    Ok((remote, port))
}

/// Writes a socket address to user memory honouring the caller's buffer size
/// (`length_address` holds it and receives the full size).
fn write_sockaddr(
    address: u64,
    length_address: u64,
    remote: &crate::ip::Address,
    port: u16,
    v6: bool,
) -> bool {
    let (bytes, size) = sockaddr_bytes(remote, port, v6);
    let mut capacity = [0u8; 4];
    if !user::copy_from_user(length_address, &mut capacity) {
        return false;
    }
    let count = (u32::from_le_bytes(capacity) as usize).min(size);
    user::copy_to_user(address, &bytes[..count])
        && user::copy_to_user(length_address, &(size as u32).to_le_bytes())
}

fn linux_pread64(descriptor: u64, address: u64, requested: u64, offset: u64) -> u64 {
    let Some(process_fd) = lookup_file_fd(descriptor) else {
        return error(9);
    };
    if !process_fd.readable {
        return error(9);
    }
    let (Ok(requested), Ok(offset)) = (usize::try_from(requested), usize::try_from(offset)) else {
        return error(22);
    };
    if requested > MAX_IO || !user::range_accessible(address, requested, true) {
        return error(14);
    }
    let mut total = 0usize;
    let mut chunk = [0u8; IO_CHUNK];
    while total < requested {
        let amount = (requested - total).min(chunk.len());
        let Some(position) = offset.checked_add(total) else {
            return if total == 0 { error(75) } else { total as u64 };
        };
        let read = match vfs::read_at(process_fd.handle, position, &mut chunk[..amount]) {
            Ok(read) => read,
            Err(failure) => {
                return if total == 0 {
                    vfs_error(failure)
                } else {
                    total as u64
                };
            }
        };
        if read == 0 {
            break;
        }
        if !user::copy_to_user(address + total as u64, &chunk[..read]) {
            return if total == 0 { error(14) } else { total as u64 };
        }
        total += read;
        if read < amount {
            break;
        }
    }
    READS.fetch_add(1, Ordering::Relaxed);
    IO_BYTES.fetch_add(total as u64, Ordering::Relaxed);
    POSITIONAL_CALLS.fetch_add(1, Ordering::Relaxed);
    total as u64
}

fn linux_pwrite64(descriptor: u64, address: u64, requested: u64, offset: u64) -> u64 {
    let Some(process_fd) = lookup_file_fd(descriptor) else {
        return error(9);
    };
    if !process_fd.writable {
        return error(9);
    }
    let (Ok(requested), Ok(offset)) = (usize::try_from(requested), usize::try_from(offset)) else {
        return error(22);
    };
    if requested > MAX_IO || !user::range_accessible(address, requested, false) {
        return error(14);
    }
    let mut total = 0usize;
    let mut chunk = [0u8; IO_CHUNK];
    while total < requested {
        let amount = (requested - total).min(chunk.len());
        if !user::copy_from_user(address + total as u64, &mut chunk[..amount]) {
            return if total == 0 { error(14) } else { total as u64 };
        }
        let Some(position) = offset.checked_add(total) else {
            return if total == 0 { error(75) } else { total as u64 };
        };
        match vfs::write_at(process_fd.handle, position, &chunk[..amount]) {
            Ok(written) => {
                total += written;
                if written < amount {
                    break;
                }
            }
            Err(failure) => {
                return if total == 0 {
                    vfs_error(failure)
                } else {
                    total as u64
                };
            }
        }
    }
    WRITES.fetch_add(1, Ordering::Relaxed);
    IO_BYTES.fetch_add(total as u64, Ordering::Relaxed);
    POSITIONAL_CALLS.fetch_add(1, Ordering::Relaxed);
    total as u64
}

fn linux_readv(descriptor: u64, vectors: u64, count: u64) -> u64 {
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return error(9);
    };
    if !process_fd.readable {
        return error(9);
    }
    if process_fd.standard == 1 {
        READS.fetch_add(1, Ordering::Relaxed);
        VECTORED_CALLS.fetch_add(1, Ordering::Relaxed);
        return 0;
    }
    if process_fd.standard != 0
        || process_fd.socket
        || process_fd.pipe
        || process_fd.unix_socket
        || process_fd.tcp
    {
        return error(9);
    }
    let Ok(count) = usize::try_from(count) else {
        return error(22);
    };
    if count > 16 {
        return error(22);
    }
    let mut total = 0usize;
    let mut chunk = [0u8; IO_CHUNK];
    for index in 0..count {
        let Some((base, length)) = read_iovec(vectors, index) else {
            return if total == 0 { error(14) } else { total as u64 };
        };
        let Ok(length) = usize::try_from(length) else {
            return if total == 0 { error(22) } else { total as u64 };
        };
        if total.checked_add(length).is_none_or(|size| size > MAX_IO)
            || !user::range_accessible(base, length, true)
        {
            return if total == 0 { error(14) } else { total as u64 };
        }
        let mut vector_total = 0usize;
        while vector_total < length {
            let amount = (length - vector_total).min(chunk.len());
            let read = match vfs::read(process_fd.handle, &mut chunk[..amount]) {
                Ok(read) => read,
                Err(failure) => {
                    return if total == 0 {
                        vfs_error(failure)
                    } else {
                        total as u64
                    };
                }
            };
            if read == 0 {
                READS.fetch_add(1, Ordering::Relaxed);
                IO_BYTES.fetch_add(total as u64, Ordering::Relaxed);
                VECTORED_CALLS.fetch_add(1, Ordering::Relaxed);
                return total as u64;
            }
            if !user::copy_to_user(base + vector_total as u64, &chunk[..read]) {
                return if total == 0 { error(14) } else { total as u64 };
            }
            vector_total += read;
            total += read;
            if read < amount {
                break;
            }
        }
        if vector_total < length {
            break;
        }
    }
    READS.fetch_add(1, Ordering::Relaxed);
    IO_BYTES.fetch_add(total as u64, Ordering::Relaxed);
    VECTORED_CALLS.fetch_add(1, Ordering::Relaxed);
    total as u64
}

/// Shared by `poll()` and `epoll_wait()`: which of `EPOLLIN`/`EPOLLOUT`
/// (numerically identical to `POLLIN`/`POLLOUT`) a process fd can currently
/// satisfy. `descriptor` must already be known to exist.
fn fd_ready_events(descriptor: u64) -> u32 {
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return 0;
    };
    if process_fd.tcp {
        return tcpsock::ready_events(&process_fd);
    }
    let mut available = 0u32;
    if process_fd.pipe {
        let mut pipe = PIPES.lock()[process_fd.handle as usize];
        if pipe.timer {
            timer_expire(&mut pipe, crate::time::monotonic_nanoseconds());
            return if pipe.counter > 0 { EPOLLIN } else { 0 };
        }
        if pipe.event {
            if pipe.counter > 0 {
                available |= EPOLLIN;
            }
            if pipe.counter < u64::MAX - 1 {
                available |= EPOLLOUT;
            }
            return available;
        }
        if process_fd.readable && pipe.length > 0 {
            available |= EPOLLIN;
        }
        if process_fd.writable && pipe.length < PIPE_BUFFER_BYTES {
            available |= EPOLLOUT;
        }
        return available;
    }
    if process_fd.unix_socket {
        // An unbound socket (not yet `connect()`ed or `bind()`ed) has no
        // backing pair to report readiness for.
        let Some(pair) = UNIX_PAIRS.lock().get(process_fd.handle as usize).copied() else {
            return 0;
        };
        let (incoming, outgoing) = if process_fd.unix_end_b {
            (pair.a_to_b, pair.b_to_a)
        } else {
            (pair.b_to_a, pair.a_to_b)
        };
        if incoming.length > 0 {
            available |= EPOLLIN;
        }
        if outgoing.length < PIPE_BUFFER_BYTES {
            available |= EPOLLOUT;
        }
        return available;
    }
    if process_fd.unix_listener {
        let ready = UNIX_LISTENERS
            .lock()
            .get(process_fd.handle as usize)
            .is_some_and(|listener| listener.backlog.iter().any(|entry| entry.is_some()));
        return if ready { EPOLLIN } else { 0 };
    }
    if process_fd.socket && !process_fd.unix_socket {
        crate::tcpnet::poll();
        let binding = SOCKETS
            .lock()
            .get(process_fd.handle as usize)
            .map(|socket| socket.binding);
        if binding.is_some_and(crate::udp::pending) {
            available |= EPOLLIN;
        }
    }
    if process_fd.readable && !process_fd.socket {
        available |= EPOLLIN;
    }
    if process_fd.writable {
        available |= EPOLLOUT;
    }
    available
}

fn poll_scan(address: u64, count: usize) -> Result<u64, u64> {
    let mut ready = 0u64;
    for index in 0..count {
        let entry = address + (index * 8) as u64;
        let mut encoded = [0u8; 8];
        if !user::copy_from_user(entry, &mut encoded) {
            return Err(error(14));
        }
        let descriptor = i32::from_le_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]);
        let events = i16::from_le_bytes([encoded[4], encoded[5]]) as u16;
        let returned = if descriptor < 0 {
            0
        } else if lookup_process_fd(descriptor as u64).is_some() {
            events & fd_ready_events(descriptor as u64) as u16
        } else {
            0x20
        };
        if returned != 0 {
            ready += 1;
        }
        if !user::copy_to_user(entry + 6, &returned.to_le_bytes()) {
            return Err(error(14));
        }
    }
    Ok(ready)
}

/// `timeout` is in milliseconds; a negative value waits until something is
/// ready (or a signal arrives).
fn linux_poll(address: u64, count: u64, timeout: u64) -> u64 {
    let Ok(count) = usize::try_from(count) else {
        return error(22);
    };
    if count > PROCESS_FD_COUNT {
        return error(22);
    }
    let Some(bytes) = count.checked_mul(8) else {
        return error(22);
    };
    if !user::range_accessible(address, bytes, true) {
        return error(14);
    }
    let timeout = timeout as u32 as i32;
    let deadline = (timeout > 0)
        .then(|| crate::time::monotonic_nanoseconds().saturating_add(timeout as u64 * 1_000_000));
    let ready = loop {
        let ready = match poll_scan(address, count) {
            Ok(ready) => ready,
            Err(failure) => return failure,
        };
        if ready > 0 || timeout == 0 {
            break ready;
        }
        if deadline.is_some_and(|deadline| crate::time::monotonic_nanoseconds() >= deadline) {
            break 0;
        }
        if interrupted() {
            return error(4);
        }
        yield_in_syscall();
    };
    POLL_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    ready
}

const TMPFS_MAGIC: u64 = 0x0102_1994;
const MSDOS_MAGIC: u64 = 0x4d44;
const RAMFS_MAGIC: u64 = 0x8584_58f6;
const TMPFS_CAPACITY_BYTES: u64 = 8 * 4096;

fn path_is_under(path: &str, mount: &str) -> bool {
    path == mount
        || path
            .strip_prefix(mount)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// `(filesystem magic, total bytes, free bytes)` for the filesystem holding
/// `path`, from the same sources `df` reports.
fn filesystem_usage(path: &str) -> (u64, u64, u64) {
    for index in 0..4 {
        if let Some(mount) = crate::datafs::mount_info(index)
            && path_is_under(path, mount.path())
        {
            return (MSDOS_MAGIC, mount.total_bytes, mount.free_bytes);
        }
    }
    let stats = vfs::stats();
    if path_is_under(path, "/tmp") {
        let used = stats.mutable_bytes as u64;
        return (
            TMPFS_MAGIC,
            TMPFS_CAPACITY_BYTES,
            TMPFS_CAPACITY_BYTES.saturating_sub(used),
        );
    }
    (RAMFS_MAGIC, stats.bytes as u64, 0)
}

fn statfs_bytes(magic: u64, total_bytes: u64, free_bytes: u64) -> [u8; 120] {
    let block = 4096u64;
    let mut buffer = [0u8; 120];
    let fields = [
        magic,
        block,
        total_bytes.div_ceil(block),
        free_bytes / block,
        free_bytes / block,
        0,
        0,
        0,
        255,
        block,
    ];
    for (index, value) in fields.iter().enumerate() {
        buffer[index * 8..index * 8 + 8].copy_from_slice(&value.to_le_bytes());
    }
    buffer
}

fn linux_statfs(path_address: u64, address: u64) -> u64 {
    let mut resolved = [0u8; MAX_PATH];
    let (length, _) = match resolve_user_path(AT_FDCWD as u64, path_address, &mut resolved) {
        Ok(value) => value,
        Err(failure) => return failure,
    };
    let Ok(path) = core::str::from_utf8(&resolved[..length]) else {
        return error(84);
    };
    if let Err(failure) = vfs::metadata(path) {
        return vfs_error(failure);
    }
    let (magic, total, free) = filesystem_usage(path);
    if !user::copy_to_user(address, &statfs_bytes(magic, total, free)) {
        return error(14);
    }
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_fstatfs(descriptor: u64, address: u64) -> u64 {
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return error(9);
    };
    let (magic, total, free) = if process_fd.standard != 0
        || process_fd.socket
        || process_fd.pipe
        || process_fd.unix_socket
        || process_fd.unix_listener
        || process_fd.tcp
    {
        (TMPFS_MAGIC, TMPFS_CAPACITY_BYTES, 0)
    } else {
        let Some((buffer, length)) = vfs::descriptor_path(process_fd.handle) else {
            return error(9);
        };
        let Ok(path) = core::str::from_utf8(&buffer[..length]) else {
            return error(84);
        };
        filesystem_usage(path)
    };
    if !user::copy_to_user(address, &statfs_bytes(magic, total, free)) {
        return error(14);
    }
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

pub(crate) fn statfs_self_test() -> bool {
    let bytes = statfs_bytes(TMPFS_MAGIC, 32768, 8192);
    let word =
        |index: usize| u64::from_le_bytes(bytes[index * 8..index * 8 + 8].try_into().unwrap());
    let layout = word(0) == TMPFS_MAGIC
        && word(1) == 4096
        && word(2) == 8
        && word(3) == 2
        && word(4) == 2
        && word(8) == 255
        && word(9) == 4096
        && bytes[80..].iter().all(|byte| *byte == 0);
    let under = path_is_under("/tmp", "/tmp")
        && path_is_under("/tmp/a/b", "/tmp")
        && !path_is_under("/tmpfoo", "/tmp")
        && !path_is_under("/", "/tmp");
    let tmp = filesystem_usage("/tmp/anything");
    let tmp_ok = tmp.0 == TMPFS_MAGIC && tmp.1 == TMPFS_CAPACITY_BYTES && tmp.2 <= tmp.1;
    let root = filesystem_usage("/bin/init");
    let descriptor_ok =
        vfs::open_file("/bin/init", false, false, false, 0, false).is_ok_and(|descriptor| {
            let found = vfs::descriptor_path(descriptor)
                .is_some_and(|(buffer, length)| &buffer[..length] == b"/bin/init");
            let _ = vfs::close(descriptor);
            found
        });
    layout && under && tmp_ok && root.0 == RAMFS_MAGIC && root.2 == 0 && descriptor_ok
}

const FD_SET_BYTES: usize = 128;
const POLL_IN: u32 = 0x1;
const POLL_OUT: u32 = 0x4;
const POLL_ERR_HUP: u32 = 0x18;

type FdSet = [u8; FD_SET_BYTES];

fn fd_set_has(set: &FdSet, fd: usize) -> bool {
    set[fd / 8] & (1 << (fd % 8)) != 0
}

/// Scans the descriptors named in the `select` sets. `ready_events` gives a
/// descriptor's poll-style readiness, or `None` if it is not open. Returns the
/// resulting read and write sets and how many bits are set in them.
fn select_scan(
    nfds: usize,
    read: &FdSet,
    write: &FdSet,
    ready_events: impl Fn(usize) -> Option<u32>,
) -> Option<(FdSet, FdSet, u64)> {
    let mut read_out = [0u8; FD_SET_BYTES];
    let mut write_out = [0u8; FD_SET_BYTES];
    let mut count = 0;
    for fd in 0..nfds.min(FD_SET_BYTES * 8) {
        let wants_read = fd_set_has(read, fd);
        let wants_write = fd_set_has(write, fd);
        if !wants_read && !wants_write {
            continue;
        }
        let events = ready_events(fd)?;
        if wants_read && events & (POLL_IN | POLL_ERR_HUP) != 0 {
            read_out[fd / 8] |= 1 << (fd % 8);
            count += 1;
        }
        if wants_write && events & (POLL_OUT | 0x8) != 0 {
            write_out[fd / 8] |= 1 << (fd % 8);
            count += 1;
        }
    }
    Some((read_out, write_out, count))
}

fn read_fd_set(address: u64, bytes: usize) -> Option<FdSet> {
    let mut set = [0u8; FD_SET_BYTES];
    if address != 0 && !user::copy_from_user(address, &mut set[..bytes]) {
        return None;
    }
    Some(set)
}

/// Shared core of `select` and `pselect6`. `timeout_ms` of `None` waits for
/// readiness up to the same 60 second cap `poll` uses.
fn select_apply(nfds: u64, sets: [u64; 3], timeout_ms: Option<u64>) -> u64 {
    let Ok(nfds) = usize::try_from(nfds) else {
        return error(22);
    };
    if nfds > FD_SET_BYTES * 8 {
        return error(22);
    }
    let bytes = nfds.div_ceil(8);
    let [read_address, write_address, except_address] = sets;
    let (Some(read), Some(write)) = (
        read_fd_set(read_address, bytes),
        read_fd_set(write_address, bytes),
    ) else {
        return error(14);
    };
    let ready_events = |fd: usize| lookup_process_fd(fd as u64).map(|_| fd_ready_events(fd as u64));
    let Some((mut read_out, mut write_out, mut count)) =
        select_scan(nfds, &read, &write, ready_events)
    else {
        return error(9);
    };
    let wait_ms = timeout_ms.unwrap_or(60_000).min(60_000);
    if count == 0 && wait_ms > 0 {
        let deadline = crate::time::monotonic_nanoseconds().saturating_add(wait_ms * 1_000_000);
        while count == 0 && crate::time::monotonic_nanoseconds() < deadline {
            if interrupted() {
                return error(4);
            }
            yield_in_syscall();
            match select_scan(nfds, &read, &write, ready_events) {
                Some((r, w, c)) => (read_out, write_out, count) = (r, w, c),
                None => return error(9),
            }
        }
    }
    let empty = [0u8; FD_SET_BYTES];
    for (address, set) in [
        (read_address, &read_out),
        (write_address, &write_out),
        (except_address, &empty),
    ] {
        if address != 0 && !user::copy_to_user(address, &set[..bytes]) {
            return error(14);
        }
    }
    POLL_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    count
}

fn linux_select(arguments: [u64; 6]) -> u64 {
    let timeout_ms = if arguments[4] == 0 {
        None
    } else {
        let mut timeval = [0u8; 16];
        if !user::copy_from_user(arguments[4], &mut timeval) {
            return error(14);
        }
        let seconds = read_array_u64(&timeval, 0);
        let microseconds = read_array_u64(&timeval, 8);
        if microseconds >= 1_000_000 {
            return error(22);
        }
        Some(
            seconds
                .saturating_mul(1000)
                .saturating_add(microseconds / 1000),
        )
    };
    select_apply(
        arguments[0],
        [arguments[1], arguments[2], arguments[3]],
        timeout_ms,
    )
}

fn linux_pselect6(arguments: [u64; 6]) -> u64 {
    let timeout_ms = if arguments[4] == 0 {
        None
    } else {
        match read_timespec(arguments[4]) {
            Some(nanoseconds) => Some(nanoseconds.div_ceil(1_000_000)),
            None => return error(22),
        }
    };
    select_apply(
        arguments[0],
        [arguments[1], arguments[2], arguments[3]],
        timeout_ms,
    )
}

pub(crate) fn select_self_test() -> bool {
    let mut read = [0u8; FD_SET_BYTES];
    let mut write = [0u8; FD_SET_BYTES];
    for fd in [1usize, 3] {
        read[fd / 8] |= 1 << (fd % 8);
    }
    for fd in [2usize, 3] {
        write[fd / 8] |= 1 << (fd % 8);
    }
    let table = |fd: usize| match fd {
        1 => Some(POLL_IN),
        2 => Some(POLL_OUT),
        3 => Some(0),
        _ => None,
    };
    let scanned = select_scan(4, &read, &write, table)
        .is_some_and(|(r, w, count)| count == 2 && r[0] == 0b0010 && w[0] == 0b0100);
    let bad_fd = {
        let mut set = [0u8; FD_SET_BYTES];
        set[1] = 1;
        select_scan(10, &set, &[0; FD_SET_BYTES], table).is_none()
    };
    let hangup_readable = {
        let mut set = [0u8; FD_SET_BYTES];
        set[0] = 1;
        select_scan(1, &set, &[0; FD_SET_BYTES], |_| Some(0x10))
            .is_some_and(|(r, _, count)| count == 1 && r[0] == 1)
    };
    let too_many = select_apply(2000, [0, 0, 0], Some(0)) == error(22);
    scanned && bad_fd && hangup_readable && too_many
}

const EPOLL_CTL_ADD: u64 = 1;
const EPOLL_CTL_DEL: u64 = 2;
const EPOLL_CTL_MOD: u64 = 3;

fn linux_epoll_create1(flags: u64) -> u64 {
    if flags & !O_CLOEXEC != 0 {
        return error(22);
    }
    let mut instances = EPOLL_INSTANCES.lock();
    let Some(index) = instances.iter().position(|instance| !instance.used) else {
        return error(24);
    };
    instances[index] = EpollInstance {
        used: true,
        watches: [EpollWatch::EMPTY; EPOLL_MAX_WATCHES],
    };
    drop(instances);
    let Some(descriptor) = install_epoll_fd(index as u32, flags & O_CLOEXEC != 0) else {
        EPOLL_INSTANCES.lock()[index] = EpollInstance::EMPTY;
        return error(24);
    };
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    descriptor
}

/// Core of `epoll_ctl`, decoded arguments only (no user-memory copying) so
/// a self-test can drive it directly - the same split `pipe_self_test`
/// uses to call `pipe_read`/`pipe_write` instead of the full
/// `linux_read`/`linux_write` syscalls.
fn epoll_ctl_apply(epoll_handle: u32, op: u64, fd: i32, events: u32, data: u64) -> u64 {
    let mut instances = EPOLL_INSTANCES.lock();
    let Some(instance) = instances.get_mut(epoll_handle as usize) else {
        return error(9);
    };
    match op {
        EPOLL_CTL_DEL => {
            let Some(watch) = instance.watches.iter_mut().find(|watch| watch.fd == fd) else {
                return error(2);
            };
            *watch = EpollWatch::EMPTY;
            0
        }
        EPOLL_CTL_ADD => {
            if instance.watches.iter().any(|watch| watch.fd == fd) {
                return error(17);
            }
            let Some(slot) = instance.watches.iter_mut().find(|watch| watch.fd == -1) else {
                return error(28);
            };
            *slot = EpollWatch { fd, events, data };
            0
        }
        EPOLL_CTL_MOD => {
            let Some(slot) = instance.watches.iter_mut().find(|watch| watch.fd == fd) else {
                return error(2);
            };
            slot.events = events;
            slot.data = data;
            0
        }
        _ => error(22),
    }
}

fn linux_epoll_ctl(epfd: u64, op: u64, fd: u64, event_address: u64) -> u64 {
    let Some(epoll_fd) = lookup_epoll_fd(epfd) else {
        return error(9);
    };
    if fd > i32::MAX as u64 {
        return error(9);
    }
    if op == EPOLL_CTL_DEL {
        let result = epoll_ctl_apply(epoll_fd.handle, op, fd as i32, 0, 0);
        if result == 0 {
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
        }
        return result;
    }
    if op != EPOLL_CTL_ADD && op != EPOLL_CTL_MOD {
        return error(22);
    }
    if lookup_process_fd(fd).is_none() {
        return error(9);
    }
    let mut encoded = [0u8; 12];
    if !user::copy_from_user(event_address, &mut encoded) {
        return error(14);
    }
    let events = u32::from_le_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]);
    let Ok(data) = encoded[4..12].try_into() else {
        return error(14);
    };
    let data = u64::from_le_bytes(data);
    let result = epoll_ctl_apply(epoll_fd.handle, op, fd as i32, events, data);
    if result == 0 {
        COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    }
    result
}

/// Core of `epoll_wait`: scans `epoll_handle`'s watch list for anything
/// matching its requested events right now and packs `(revents, data)`
/// pairs into `output`, with no user-memory copying - see `epoll_ctl_apply`.
fn epoll_scan(epoll_handle: u32, output: &mut [(u32, u64)]) -> Option<usize> {
    let watches = {
        let instances = EPOLL_INSTANCES.lock();
        let instance = instances.get(epoll_handle as usize)?;
        instance.watches
    };
    let mut count = 0usize;
    for watch in watches.iter() {
        if watch.fd < 0 || count >= output.len() {
            continue;
        }
        let ready = fd_ready_events(watch.fd as u64) & watch.events;
        if ready == 0 {
            continue;
        }
        output[count] = (ready, watch.data);
        count += 1;
    }
    Some(count)
}

/// `epoll_wait` with nothing ready yet cooperatively retries against a real
/// wall-clock deadline (same idea as `linux_poll`'s `wait_until`, but
/// yielding to other tasks instead of spinning) - `timeout_ms < 0` waits
/// indefinitely, `== 0` polls once, matching real epoll_wait semantics.
fn linux_epoll_wait(epfd: u64, events_address: u64, max_events: u64, timeout_ms: u64) -> u64 {
    let Some(epoll_fd) = lookup_epoll_fd(epfd) else {
        return error(9);
    };
    let Ok(max_events) = usize::try_from(max_events) else {
        return error(22);
    };
    if max_events == 0 {
        return error(22);
    }
    let max_events = max_events.min(EPOLL_MAX_WATCHES);
    let Some(bytes) = max_events.checked_mul(12) else {
        return error(22);
    };
    if !user::range_accessible(events_address, bytes, true) {
        return error(14);
    }
    let timeout = timeout_ms as u32 as i32;
    let deadline = (timeout > 0)
        .then(|| crate::time::monotonic_nanoseconds().saturating_add(timeout as u64 * 1_000_000));
    let mut scratch = [(0u32, 0u64); EPOLL_MAX_WATCHES];
    loop {
        let Some(count) = epoll_scan(epoll_fd.handle, &mut scratch[..max_events]) else {
            return error(9);
        };
        for (index, (ready, data)) in scratch[..count].iter().enumerate() {
            let entry = events_address + (index * 12) as u64;
            if !user::copy_to_user(entry, &ready.to_le_bytes())
                || !user::copy_to_user(entry + 4, &data.to_le_bytes())
            {
                return error(14);
            }
        }
        if count > 0 || timeout == 0 {
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            return count as u64;
        }
        if let Some(deadline) = deadline
            && crate::time::monotonic_nanoseconds() >= deadline
        {
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            return 0;
        }
        yield_in_syscall();
    }
}

fn linux_tkill(thread: u64, signal: u64) -> SyscallResult {
    if signal > 64 {
        return SyscallResult::Return(error(22));
    }
    if thread == 0 {
        return SyscallResult::Return(error(3));
    }
    SIGNAL_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    if thread == current_thread_id() {
        return if signal == 0 {
            SyscallResult::Return(0)
        } else {
            raise_signal(signal)
        };
    }
    let Some(group) = crate::scheduler::group_of_tid(thread) else {
        return SyscallResult::Return(error(3));
    };
    if signal == 0 {
        return SyscallResult::Return(0);
    }
    match crate::scheduler::signal_other_task(thread, signal) {
        Some(RemoteSignal::Ignored | RemoteSignal::Queued) => SyscallResult::Return(0),
        Some(RemoteSignal::Fatal) | None => {
            if group == crate::scheduler::current_group() {
                crate::scheduler::exit_group(128 + signal);
                SyscallResult::Exit(128 + signal)
            } else {
                match crate::scheduler::kill_task(group) {
                    Ok(()) => SyscallResult::Return(0),
                    Err(()) => SyscallResult::Return(error(1)),
                }
            }
        }
    }
}

/// What sending a signal to a task that is not running did.
pub enum RemoteSignal {
    Ignored,
    Queued,
    Fatal,
}

/// Applies a signal to a stored (not currently running) process: a handler
/// gets it queued for its next syscall return, SIG_IGN and default-ignored
/// signals vanish, anything else is fatal.
pub fn signal_stored_state(state: &mut ProcessState, signal: u64, traced: bool) -> RemoteSignal {
    if traced && signal != 9 {
        state.signal_pending |= 1 << (signal - 1);
        return RemoteSignal::Queued;
    }
    let action = state.signal_actions[signal as usize - 1];
    let catchable = signal != 9 && signal != 19;
    if catchable && (action.handler == 1 || (action.handler == 0 && default_ignored(signal))) {
        RemoteSignal::Ignored
    } else if catchable && action.handler > 1 {
        state.signal_pending |= 1 << (signal - 1);
        RemoteSignal::Queued
    } else {
        RemoteSignal::Fatal
    }
}

/// A forked child starts with no signals waiting and no interval timer.
pub fn clear_pending_signals(state: &mut ProcessState) {
    state.signal_pending = 0;
    state.itimer_deadline = 0;
    state.itimer_interval = 0;
}

/// Signals whose default action is to do nothing.
fn default_ignored(signal: u64) -> bool {
    matches!(signal, 17 | 18 | 23 | 28)
}

/// A signal sent to the running process: ignored, queued for its handler,
/// or (default action) fatal with status 128 + signal.
fn raise_signal(signal: u64) -> SyscallResult {
    if signal != 9 && crate::scheduler::is_traced() {
        SIGNAL_PENDING.fetch_or(1 << (signal - 1), Ordering::AcqRel);
        return SyscallResult::Return(0);
    }
    let action = SIGNAL_ACTIONS.lock()[signal as usize - 1];
    let catchable = signal != 9 && signal != 19;
    if catchable && action.handler == 1 {
        return SyscallResult::Return(0);
    }
    if catchable && action.handler > 1 {
        SIGNAL_PENDING.fetch_or(1 << (signal - 1), Ordering::AcqRel);
        return SyscallResult::Return(0);
    }
    if catchable && default_ignored(signal) {
        return SyscallResult::Return(0);
    }
    SyscallResult::Exit(128 + signal)
}

const SA_RESTORER: u64 = 0x0400_0000;
const SA_NODEFER: u64 = 0x4000_0000;
/// Words saved on the user stack for a signal handler (see `deliver_signal`).
const SIGNAL_SAVED_WORDS: usize = 20;
const SIGNAL_SIGINFO_BYTES: usize = 128;
const SIGNAL_CONTEXT_BYTES: usize = 256;
/// Frame saved at a syscall return (registers a syscall preserves).
const SIGNAL_FRAME_MAGIC: u64 = 0x5349_4746_5241_4d45;
/// Frame saved when a timer interrupt caught the process in user mode: every
/// register is kept, and `rt_sigreturn` comes back through `iretq`.
const SIGNAL_ASYNC_MAGIC: u64 = 0x5349_4741_5359_4e43;

/// Lays the handler frame on the user stack below the red zone: the
/// restorer address, the `saved` words (mask goes in word 16) and a zeroed
/// siginfo with the signal number. Blocks the handler's signals and returns
/// the new user stack pointer, or an exit status when the frame can't be
/// written.
fn push_signal_frame(
    signal: u64,
    action: SignalAction,
    user_rsp: u64,
    saved: &mut [u64; SIGNAL_SAVED_WORDS],
) -> Result<u64, u64> {
    if action.flags & SA_RESTORER == 0 || action.restorer == 0 {
        return Err(128 + 11);
    }
    let block = 8 + SIGNAL_SAVED_WORDS * 8 + SIGNAL_SIGINFO_BYTES + SIGNAL_CONTEXT_BYTES;
    let Some(base) = user_rsp.checked_sub(128 + block as u64) else {
        return Err(128 + 11);
    };
    // Handler entry needs rsp = 8 (mod 16), as right after a `call`.
    let sp = (base & !15) - 8;
    let old_mask = SIGNAL_MASK.load(Ordering::Acquire);
    saved[16] = old_mask;
    let mut bytes = [0u8; 8 + SIGNAL_SAVED_WORDS * 8 + SIGNAL_SIGINFO_BYTES];
    bytes[..8].copy_from_slice(&action.restorer.to_le_bytes());
    for (index, word) in saved.iter().enumerate() {
        bytes[8 + index * 8..16 + index * 8].copy_from_slice(&word.to_le_bytes());
    }
    let info_at = 8 + SIGNAL_SAVED_WORDS * 8;
    bytes[info_at..info_at + 4].copy_from_slice(&(signal as u32).to_le_bytes());
    if !user::copy_to_user(sp, &bytes) {
        return Err(128 + 11);
    }
    let mut mask = old_mask | action.mask;
    if action.flags & SA_NODEFER == 0 {
        mask |= 1 << (signal - 1);
    }
    SIGNAL_MASK.store(mask & !unblockable_signals(), Ordering::Release);
    Ok(sp)
}

/// Expires the ITIMER_REAL if it is due: SIGALRM is queued for its handler
/// (or dropped when ignored); with the default action the process has to die,
/// which the exit status says. Runs before every delivery attempt.
fn check_itimer() -> Option<u64> {
    let deadline = ITIMER_DEADLINE.load(Ordering::Acquire);
    if deadline == 0 {
        return None;
    }
    let now = crate::time::monotonic_nanoseconds();
    if now < deadline {
        return None;
    }
    let interval = ITIMER_INTERVAL.load(Ordering::Acquire);
    let next = if interval == 0 {
        0
    } else {
        deadline + interval * (1 + (now - deadline) / interval)
    };
    ITIMER_DEADLINE.store(next, Ordering::Release);
    const SIGALRM: u64 = 14;
    let action = SIGNAL_ACTIONS.lock()[SIGALRM as usize - 1];
    match action.handler {
        1 => None,
        0 => (!default_ignored(SIGALRM)).then_some(128 + SIGALRM),
        _ => {
            SIGNAL_PENDING.fetch_or(1 << (SIGALRM - 1), Ordering::AcqRel);
            None
        }
    }
}

/// The next pending, unblocked signal with a real handler (signals without
/// one are consumed here: ignored or already handled). A traced task lets its
/// tracer look at the signal first; the one it lets through (maybe another)
/// has its default action when the task has no handler. Errors carry the exit
/// status of a task the signal ends.
fn take_signal(context: &mut ptrace::Context) -> Result<Option<(u64, SignalAction)>, u64> {
    let ready = SIGNAL_PENDING.load(Ordering::Acquire) & !SIGNAL_MASK.load(Ordering::Acquire);
    if ready == 0 {
        return Ok(None);
    }
    let mut signal = ready.trailing_zeros() as u64 + 1;
    SIGNAL_PENDING.fetch_and(!(1 << (signal - 1)), Ordering::AcqRel);
    let traced = crate::scheduler::is_traced();
    if traced {
        signal = match ptrace::signal_stop(context, signal) {
            Err(()) => return Err(128 + 9),
            Ok(0) => return Ok(None),
            Ok(injected) => injected,
        };
    }
    let action = SIGNAL_ACTIONS.lock()[signal as usize - 1];
    if action.handler > 1 {
        return Ok(Some((signal, action)));
    }
    if traced && action.handler == 0 && !default_ignored(signal) && !matches!(signal, 19..=22) {
        return Err(128 + signal);
    }
    Ok(None)
}

/// Runs the handler of one pending, unblocked signal on return from a
/// syscall: the user registers are saved on the user stack below the red
/// zone and the frame is redirected to the handler; its `ret` lands on the
/// `sa_restorer`, which calls rt_sigreturn. Returns an exit status when the
/// process has to die instead (no usable handler frame).
fn deliver_signal(frame: &mut LinuxSyscallFrame) -> Option<u64> {
    if let Some(code) = check_itimer() {
        return Some(code);
    }
    let (signal, action) = match take_signal(&mut ptrace::Context::Syscall(&mut *frame)) {
        Ok(Some(taken)) => taken,
        Ok(None) => return None,
        Err(code) => return Some(code),
    };
    let user_rsp = user_stack_pointer();
    let mut saved: [u64; SIGNAL_SAVED_WORDS] = [
        frame.user_rip,
        user_rsp,
        frame.user_rflags,
        frame.number,
        frame.argument0,
        frame.argument1,
        frame.argument2,
        frame.argument3,
        frame.argument4,
        frame.argument5,
        frame.r15,
        frame.r14,
        frame.r13,
        frame.r12,
        frame.rbp,
        frame.rbx,
        0,
        SIGNAL_FRAME_MAGIC,
        0,
        0,
    ];
    let sp = match push_signal_frame(signal, action, user_rsp, &mut saved) {
        Ok(sp) => sp,
        Err(code) => return Some(code),
    };
    frame.user_rip = action.handler;
    frame.argument0 = signal;
    frame.argument1 = sp + 8 + (SIGNAL_SAVED_WORDS * 8) as u64;
    frame.argument2 = frame.argument1 + SIGNAL_SIGINFO_BYTES as u64;
    frame.number = 0;
    set_user_stack_pointer(sp);
    None
}

/// Timer-interrupt counterpart of `deliver_signal`: a process spinning in
/// user mode has no syscall return to carry its handler, so the tick that
/// interrupted it redirects it instead. All registers are saved (the handler
/// may clobber any of them) and `rt_sigreturn` restores them via `iretq`.
/// Returns an exit status when the process has to die instead.
pub fn deliver_async_signal(regs: &mut user::UserRegs) -> Option<u64> {
    if let Some(code) = check_itimer() {
        return Some(code);
    }
    if SIGNAL_PENDING.load(Ordering::Acquire) & !SIGNAL_MASK.load(Ordering::Acquire) == 0 {
        return None;
    }
    let (signal, action) = match take_signal(&mut ptrace::Context::Interrupt(&mut *regs)) {
        Ok(Some(taken)) => taken,
        Ok(None) => return None,
        Err(code) => return Some(code),
    };
    let mut saved: [u64; SIGNAL_SAVED_WORDS] = [
        regs.rip,
        regs.rsp,
        regs.rflags,
        regs.rax,
        regs.rbx,
        regs.rcx,
        regs.rdx,
        regs.rbp,
        regs.rsi,
        regs.rdi,
        regs.r8,
        regs.r9,
        regs.r10,
        regs.r11,
        regs.r12,
        regs.r13,
        0,
        SIGNAL_ASYNC_MAGIC,
        regs.r14,
        regs.r15,
    ];
    let sp = match push_signal_frame(signal, action, regs.rsp, &mut saved) {
        Ok(sp) => sp,
        Err(code) => return Some(code),
    };
    ASYNC_SIGNALS.fetch_add(1, Ordering::Relaxed);
    regs.rip = action.handler;
    regs.rsp = sp;
    regs.rdi = signal;
    regs.rsi = sp + 8 + (SIGNAL_SAVED_WORDS * 8) as u64;
    regs.rdx = regs.rsi + SIGNAL_SIGINFO_BYTES as u64;
    regs.rax = 0;
    None
}

/// A breakpoint, single-step or fault trap of a traced task in user mode.
pub fn debug_trap(regs: &mut user::UserRegs, signal: u64, fault: bool) -> Trap {
    ptrace::trap(regs, signal, fault)
}

/// Handlers started by a timer tick instead of a syscall return.
static ASYNC_SIGNALS: AtomicU64 = AtomicU64::new(0);

pub fn async_signal_count() -> u64 {
    ASYNC_SIGNALS.load(Ordering::Relaxed)
}

/// How `rt_sigreturn` has to resume the process.
enum SignalResume {
    /// Through the normal `sysret` path (a syscall-return frame).
    Syscall,
    /// Through `iretq` with every register restored (a timer-return frame).
    Full,
}

/// rt_sigreturn: puts back the registers `deliver_signal` or
/// `deliver_async_signal` saved (the stack pointer sits just past the
/// restorer address the handler returned to).
fn signal_return(frame: &mut LinuxSyscallFrame) -> Result<SignalResume, ()> {
    let mut bytes = [0u8; SIGNAL_SAVED_WORDS * 8];
    if !user::copy_from_user(user_stack_pointer(), &mut bytes) {
        return Err(());
    }
    let word = |index: usize| {
        u64::from_le_bytes(bytes[index * 8..index * 8 + 8].try_into().unwrap_or([0; 8]))
    };
    let magic = word(17);
    if (magic != SIGNAL_FRAME_MAGIC && magic != SIGNAL_ASYNC_MAGIC)
        || !user::range_accessible(word(0), 1, false)
    {
        return Err(());
    }
    SIGNAL_MASK.store(word(16) & !unblockable_signals(), Ordering::Release);
    let flags = (word(2) & 0x0000_0cd5) | 0x202;
    if magic == SIGNAL_ASYNC_MAGIC {
        frame.number = word(3);
        frame.rbx = word(4);
        frame.argument2 = word(6);
        frame.rbp = word(7);
        frame.argument1 = word(8);
        frame.argument0 = word(9);
        frame.argument4 = word(10);
        frame.argument5 = word(11);
        frame.argument3 = word(12);
        frame.r12 = word(14);
        frame.r13 = word(15);
        frame.r14 = word(18);
        frame.r15 = word(19);
        crate::arch::syscall_entry::set_iret_return(word(5), word(13), word(0), word(1), flags);
        return Ok(SignalResume::Full);
    }
    frame.user_rip = word(0);
    frame.user_rflags = flags;
    frame.number = word(3);
    frame.argument0 = word(4);
    frame.argument1 = word(5);
    frame.argument2 = word(6);
    frame.argument3 = word(7);
    frame.argument4 = word(8);
    frame.argument5 = word(9);
    frame.r15 = word(10);
    frame.r14 = word(11);
    frame.r13 = word(12);
    frame.r12 = word(13);
    frame.rbp = word(14);
    frame.rbx = word(15);
    set_user_stack_pointer(word(1));
    Ok(SignalResume::Syscall)
}

fn linux_tgkill(group: u64, thread: u64, signal: u64) -> SyscallResult {
    if crate::scheduler::group_of_tid(thread) != Some(group) {
        return SyscallResult::Return(error(3));
    }
    linux_tkill(thread, signal)
}

fn linux_writev(descriptor: u64, vectors: u64, count: u64) -> u64 {
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return error(9);
    };
    if !process_fd.writable
        || process_fd.socket
        || process_fd.pipe
        || process_fd.unix_socket
        || process_fd.tcp
        || process_fd.standard == 1
    {
        return error(9);
    }
    let Ok(count) = usize::try_from(count) else {
        return error(22);
    };
    if count > 16 {
        return error(22);
    }
    let mut total = 0usize;
    let mut chunk = [0u8; IO_CHUNK];
    for index in 0..count {
        let Some((base, length)) = read_iovec(vectors, index) else {
            return if total == 0 { error(14) } else { total as u64 };
        };
        let Ok(length) = usize::try_from(length) else {
            return if total == 0 { error(22) } else { total as u64 };
        };
        if total.checked_add(length).is_none_or(|size| size > MAX_IO)
            || !user::range_accessible(base, length, false)
        {
            return if total == 0 { error(14) } else { total as u64 };
        }
        let mut vector_total = 0usize;
        while vector_total < length {
            let amount = (length - vector_total).min(chunk.len());
            if !user::copy_from_user(base + vector_total as u64, &mut chunk[..amount]) {
                return if total == 0 { error(14) } else { total as u64 };
            }
            if process_fd.standard != 0 {
                for byte in &chunk[..amount] {
                    crate::serial::byte(*byte);
                }
                vector_total += amount;
                total += amount;
                continue;
            }
            match vfs::write(process_fd.handle, &chunk[..amount], process_fd.append) {
                Ok(written) => {
                    vector_total += written;
                    total += written;
                    if written < amount {
                        break;
                    }
                }
                Err(failure) => {
                    return if total == 0 {
                        vfs_error(failure)
                    } else {
                        total as u64
                    };
                }
            }
        }
        if vector_total < length {
            break;
        }
    }
    WRITES.fetch_add(1, Ordering::Relaxed);
    IO_BYTES.fetch_add(total as u64, Ordering::Relaxed);
    VECTORED_CALLS.fetch_add(1, Ordering::Relaxed);
    total as u64
}

fn read_iovec(address: u64, index: usize) -> Option<(u64, u64)> {
    let mut encoded = [0u8; 16];
    user::copy_from_user(address.checked_add((index * 16) as u64)?, &mut encoded)
        .then(|| (read_array_u64(&encoded, 0), read_array_u64(&encoded, 8)))
}

fn linux_faccessat2(directory: u64, path_address: u64, mode: u64, flags: u64) -> u64 {
    if mode & !7 != 0 || flags & !(AT_SYMLINK_NOFOLLOW | 0x200) != 0 {
        return error(22);
    }
    let mut resolved = [0u8; MAX_PATH];
    let (length, relative) = match resolve_user_path(directory, path_address, &mut resolved) {
        Ok(value) => value,
        Err(failure) => return failure,
    };
    let Ok(path) = core::str::from_utf8(&resolved[..length]) else {
        return error(84);
    };
    let metadata = match vfs::metadata(path) {
        Ok(metadata) => metadata,
        Err(failure) => return vfs_error(failure),
    };
    if mode & 2 != 0 {
        return error(30);
    }
    if mode & 4 != 0 && metadata.mode & 0o444 == 0 || mode & 1 != 0 && metadata.mode & 0o111 == 0 {
        return error(13);
    }
    ACCESS_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    if relative {
        RELATIVE_PATH_CALLS.fetch_add(1, Ordering::Relaxed);
    }
    0
}

fn linux_statx(arguments: [u64; 6]) -> u64 {
    let [directory, path_address, flags, _, address, _] = arguments;
    if flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH) != 0 {
        return error(22);
    }
    let mut input = [0u8; MAX_PATH];
    let Some(length) = user::copy_string(path_address, &mut input) else {
        return error(14);
    };
    let mut relative = false;
    let metadata = if length == 0 && flags & AT_EMPTY_PATH != 0 {
        let Some(process_fd) = lookup_file_fd(directory) else {
            return error(9);
        };
        match vfs::descriptor_metadata(process_fd.handle) {
            Ok(metadata) => metadata,
            Err(failure) => return vfs_error(failure),
        }
    } else {
        let mut resolved = [0u8; MAX_PATH];
        let (length, was_relative) = match resolve_user_path(directory, path_address, &mut resolved)
        {
            Ok(value) => value,
            Err(failure) => return failure,
        };
        relative = was_relative;
        let Ok(path) = core::str::from_utf8(&resolved[..length]) else {
            return error(84);
        };
        match vfs::metadata(path) {
            Ok(metadata) => metadata,
            Err(failure) => return vfs_error(failure),
        }
    };
    let mut statx = [0u8; 256];
    statx[..4].copy_from_slice(&0x7ffu32.to_le_bytes());
    statx[4..8].copy_from_slice(&4096u32.to_le_bytes());
    statx[16..20].copy_from_slice(&1u32.to_le_bytes());
    statx[28..30].copy_from_slice(&(metadata.mode as u16).to_le_bytes());
    statx[32..40].copy_from_slice(&metadata.inode.to_le_bytes());
    statx[40..48].copy_from_slice(&metadata.size.to_le_bytes());
    // atime, btime, ctime, mtime.
    for at in [64usize, 80, 96, 112] {
        statx[at..at + 8].copy_from_slice(&metadata.modified.to_le_bytes());
    }
    statx[48..56].copy_from_slice(&metadata.size.div_ceil(512).to_le_bytes());
    if !user::copy_to_user(address, &statx) {
        return error(14);
    }
    METADATA_CALLS.fetch_add(1, Ordering::Relaxed);
    STATX_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    if relative {
        RELATIVE_PATH_CALLS.fetch_add(1, Ordering::Relaxed);
    }
    0
}

/// Shared by both mmap backends below: streams `process_fd`'s content
/// (starting at `offset`) into an already-allocated mapping of `length`
/// bytes at `address`, via whichever backend-specific `write` closure the
/// caller supplies (process-space vs singleton use different underlying
/// write primitives, but the chunked-read-then-write loop is identical).
/// The `Err` value is already a ready-to-return `error()`/`vfs_error()`
/// code, matching this file's usual convention.
fn mmap_fill_from_file(
    length: usize,
    offset: u64,
    process_fd: ProcessFd,
    mut write: impl FnMut(usize, &[u8]) -> bool,
) -> Result<(), u64> {
    let Ok(file_offset) = usize::try_from(offset) else {
        return Err(error(75));
    };
    let mut total = 0usize;
    let mut chunk = [0u8; IO_CHUNK];
    while total < length {
        let amount = (length - total).min(chunk.len());
        let Some(position) = file_offset.checked_add(total) else {
            return Err(error(75));
        };
        let read = match vfs::read_at(process_fd.handle, position, &mut chunk[..amount]) {
            Ok(read) => read,
            Err(failure) => return Err(vfs_error(failure)),
        };
        if read == 0 {
            break;
        }
        if !write(total, &chunk[..read]) {
            return Err(error(14));
        }
        total += read;
        if read < amount {
            break;
        }
    }
    Ok(())
}

fn linux_mmap(arguments: [u64; 6]) -> u64 {
    let [address, length, protection, flags, descriptor, offset] = arguments;
    let anonymous = flags & MAP_ANONYMOUS != 0;
    if address != 0
        || flags & MAP_PRIVATE == 0
        || flags & !(MAP_PRIVATE | MAP_ANONYMOUS | MAP_STACK) != 0
        || protection > u8::MAX as u64
    {
        return error(22);
    }
    let file = if anonymous {
        if descriptor != u64::MAX || offset != 0 {
            return error(22);
        }
        None
    } else {
        if offset & 0xfff != 0 {
            return error(22);
        }
        let Some(process_fd) = lookup_file_fd(descriptor) else {
            return error(9);
        };
        if !process_fd.readable {
            return error(13);
        }
        Some(process_fd)
    };
    let Ok(length) = usize::try_from(length) else {
        return error(12);
    };
    let pages = length.div_ceil(4096).max(1);

    // Tasks with their own `process_space` (real forked/exec'd processes)
    // get file-backed mmap mapped into their own dedicated region, not the
    // shared singleton below - `None` here means this task has no
    // `process_space` at all, so it falls through to that singleton path.
    if let Some(result) = crate::scheduler::linux_mmap_for_current_task(pages, protection as u8) {
        let Some(mapped_address) = result else {
            crate::oom::relieve();
            return error(12);
        };
        if let Some(process_fd) = file {
            let filled = mmap_fill_from_file(length, offset, process_fd, |chunk_offset, chunk| {
                crate::scheduler::linux_mmap_write_for_current_task(
                    mapped_address,
                    chunk_offset,
                    chunk,
                ) == Some(true)
            });
            if let Err(failure) = filled {
                let _ = crate::scheduler::linux_munmap_for_current_task(mapped_address, pages);
                return failure;
            }
            FILE_MMAPS.fetch_add(1, Ordering::Relaxed);
        }
        MEMORY_CALLS.fetch_add(1, Ordering::Relaxed);
        MMAPS.fetch_add(1, Ordering::Relaxed);
        return mapped_address;
    }

    match crate::arch::paging::user_mmap(length, protection as u8) {
        Ok(mapped_address) => {
            if let Some(process_fd) = file {
                let filled =
                    mmap_fill_from_file(length, offset, process_fd, |chunk_offset, chunk| {
                        crate::arch::paging::user_write_mapping(mapped_address, chunk_offset, chunk)
                            .is_ok()
                    });
                if let Err(failure) = filled {
                    let _ = crate::arch::paging::user_munmap(mapped_address, length);
                    return failure;
                }
                FILE_MMAPS.fetch_add(1, Ordering::Relaxed);
            }
            MEMORY_CALLS.fetch_add(1, Ordering::Relaxed);
            MMAPS.fetch_add(1, Ordering::Relaxed);
            mapped_address
        }
        Err(crate::arch::paging::UserMemoryError::OutOfMemory) => {
            crate::oom::relieve();
            error(12)
        }
        Err(_) => error(22),
    }
}

fn linux_mprotect(address: u64, length: u64, protection: u64) -> u64 {
    if protection > u8::MAX as u64 {
        return error(22);
    }
    let Ok(length) = usize::try_from(length) else {
        return error(22);
    };
    let pages = length.div_ceil(4096).max(1);
    if let Some(result) =
        crate::scheduler::linux_mprotect_for_current_task(address, pages, protection as u8)
    {
        return match result {
            true => {
                MEMORY_CALLS.fetch_add(1, Ordering::Relaxed);
                MPROTECTS.fetch_add(1, Ordering::Relaxed);
                0
            }
            false => error(22),
        };
    }
    match crate::arch::paging::user_mprotect(address, length, protection as u8) {
        Ok(()) => {
            MEMORY_CALLS.fetch_add(1, Ordering::Relaxed);
            MPROTECTS.fetch_add(1, Ordering::Relaxed);
            0
        }
        Err(_) => error(22),
    }
}

fn linux_munmap(address: u64, length: u64) -> u64 {
    let Ok(length) = usize::try_from(length) else {
        return error(22);
    };
    let pages = length.div_ceil(4096).max(1);
    if let Some(result) = crate::scheduler::linux_munmap_for_current_task(address, pages) {
        return match result {
            true => {
                MEMORY_CALLS.fetch_add(1, Ordering::Relaxed);
                MUNMAPS.fetch_add(1, Ordering::Relaxed);
                0
            }
            false => error(22),
        };
    }
    match crate::arch::paging::user_munmap(address, length) {
        Ok(()) => {
            MEMORY_CALLS.fetch_add(1, Ordering::Relaxed);
            MUNMAPS.fetch_add(1, Ordering::Relaxed);
            0
        }
        Err(_) => error(22),
    }
}

fn linux_rt_sigaction(signal: u64, action: u64, old_action: u64, set_size: u64) -> u64 {
    if !(1..=64).contains(&signal) || set_size != 8 {
        return error(22);
    }
    let index = signal as usize - 1;
    let current = SIGNAL_ACTIONS.lock()[index];
    if old_action != 0 {
        let encoded = encode_signal_action(current);
        if !user::copy_to_user(old_action, &encoded) {
            return error(14);
        }
    }
    if action != 0 {
        if signal == 9 || signal == 19 {
            return error(22);
        }
        let mut encoded = [0u8; 32];
        if !user::copy_from_user(action, &mut encoded) {
            return error(14);
        }
        let configured = SignalAction {
            handler: read_array_u64(&encoded, 0),
            flags: read_array_u64(&encoded, 8),
            restorer: read_array_u64(&encoded, 16),
            mask: read_array_u64(&encoded, 24) & !unblockable_signals(),
        };
        if configured.handler > 1 && !user::range_accessible(configured.handler, 1, false) {
            return error(14);
        }
        if configured.restorer != 0 && !user::range_accessible(configured.restorer, 1, false) {
            return error(14);
        }
        SIGNAL_ACTIONS.lock()[index] = configured;
    }
    SIGNAL_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_rt_sigprocmask(how: u64, set: u64, old_set: u64, set_size: u64) -> u64 {
    if set_size != 8 {
        return error(22);
    }
    let current = SIGNAL_MASK.load(Ordering::Acquire);
    if old_set != 0 && !user::copy_to_user(old_set, &current.to_le_bytes()) {
        return error(14);
    }
    if set != 0 {
        let mut encoded = [0u8; 8];
        if !user::copy_from_user(set, &mut encoded) {
            return error(14);
        }
        let requested = u64::from_le_bytes(encoded) & !unblockable_signals();
        let updated = match how {
            0 => current | requested,
            1 => current & !requested,
            2 => requested,
            _ => return error(22),
        };
        SIGNAL_MASK.store(updated, Ordering::Release);
    }
    SIGNAL_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_sigaltstack(stack: u64, old_stack: u64) -> u64 {
    let current = *ALTERNATE_STACK.lock();
    if old_stack != 0 {
        let encoded = encode_alternate_stack(current);
        if !user::copy_to_user(old_stack, &encoded) {
            return error(14);
        }
    }
    if stack != 0 {
        let mut encoded = [0u8; 24];
        if !user::copy_from_user(stack, &mut encoded) {
            return error(14);
        }
        let configured = AlternateStack {
            pointer: read_array_u64(&encoded, 0),
            flags: read_array_u32(&encoded, 8),
            size: read_array_u64(&encoded, 16),
        };
        if configured.flags != 0 && configured.flags != 2 {
            return error(22);
        }
        if configured.flags == 0
            && (configured.pointer == 0
                || configured.size < 2048
                || usize::try_from(configured.size).map_or(true, |size| {
                    !user::range_accessible(configured.pointer, size, true)
                }))
        {
            return error(12);
        }
        *ALTERNATE_STACK.lock() = configured;
    }
    SIGNAL_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_ioctl(descriptor: u64, request: u64, address: u64) -> u64 {
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return error(9);
    };
    if is_socket_fd(&process_fd) && matches!(request, 0x541b | 0x5421) {
        return socket_ioctl(descriptor, &process_fd, request, address);
    }
    if process_fd.standard == 0 {
        return error(25);
    }
    let result = match request {
        0x5413 => {
            let mut window = [0u8; 8];
            window[..2].copy_from_slice(&25u16.to_le_bytes());
            window[2..4].copy_from_slice(&80u16.to_le_bytes());
            user::copy_to_user(address, &window)
        }
        0x5401 => {
            let mut termios = [0u8; 36];
            termios[8..12].copy_from_slice(&0x0000_04b0u32.to_le_bytes());
            user::copy_to_user(address, &termios)
        }
        _ => return error(25),
    };
    if !result {
        return error(14);
    }
    RUNTIME_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

/// `FIONREAD` (bytes ready to read) and `FIONBIO` (switch non-blocking mode)
/// on a socket.
fn socket_ioctl(descriptor: u64, process_fd: &ProcessFd, request: u64, address: u64) -> u64 {
    if request == 0x5421 {
        let mut flag = [0u8; 4];
        if !user::copy_from_user(address, &mut flag) {
            return error(14);
        }
        if let Some(index) = process_fd_index(descriptor) {
            PROCESS_FDS.lock()[index].nonblocking = u32::from_le_bytes(flag) != 0;
        }
        return 0;
    }
    crate::tcpnet::poll();
    let ready = if process_fd.tcp {
        crate::tcpnet::with_tcp(|tcp, _, _| tcp.available(process_fd.handle as usize))
    } else if process_fd.socket && !process_fd.unix_socket {
        SOCKETS
            .lock()
            .get(process_fd.handle as usize)
            .filter(|socket| socket.open)
            .map_or(0, |socket| crate::udp::next_length(socket.binding))
    } else {
        0
    };
    if !user::copy_to_user(address, &(ready as u32).to_le_bytes()) {
        return error(14);
    }
    0
}

/// AerOS only ever creates `MAP_PRIVATE` mappings (see `linux_mmap`'s flag
/// check) -- there is no `MAP_SHARED` file-backed mapping whose dirty pages
/// would need flushing back to disk. `msync` is therefore a real no-op
/// here, not a stub standing in for missing functionality: it validates
/// its arguments and the target range the same way a working
/// implementation would, then reports success since there is nothing a
/// private mapping could need synced.
fn linux_msync(address: u64, length: u64, flags: u64) -> u64 {
    const MS_ASYNC: u64 = 1;
    const MS_INVALIDATE: u64 = 2;
    const MS_SYNC: u64 = 4;
    let Ok(length) = usize::try_from(length) else {
        return error(22);
    };
    if address & 0xfff != 0
        || flags & !(MS_ASYNC | MS_INVALIDATE | MS_SYNC) != 0
        || flags & MS_ASYNC != 0 && flags & MS_SYNC != 0
        || !user::range_accessible(address, length, false)
    {
        return error(22);
    }
    SYNC_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_madvise(address: u64, length: u64, advice: u64) -> u64 {
    let Ok(length) = usize::try_from(length) else {
        return error(22);
    };
    if address & 0xfff != 0
        || length == 0
        || advice > 4
        || !user::range_accessible(address, length, false)
    {
        return error(22);
    }
    RUNTIME_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_getcwd(address: u64, size: u64) -> u64 {
    let directory = *CURRENT_DIRECTORY.lock();
    let required = directory.length + 1;
    let Ok(size) = usize::try_from(size) else {
        return error(34);
    };
    if size < required {
        return error(34);
    }
    if !user::copy_to_user(address, &directory.bytes[..directory.length])
        || !user::copy_to_user(address + directory.length as u64, &[0])
    {
        return error(14);
    }
    RUNTIME_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    required as u64
}

fn set_current_directory(path: &[u8]) {
    let mut directory = CURRENT_DIRECTORY.lock();
    directory.bytes.fill(0);
    directory.bytes[..path.len()].copy_from_slice(path);
    directory.length = path.len();
}

fn linux_fchdir(descriptor: u64) -> u64 {
    let Some(process_fd) = lookup_file_fd(descriptor) else {
        return error(9);
    };
    let metadata = match vfs::descriptor_metadata(process_fd.handle) {
        Ok(metadata) => metadata,
        Err(failure) => return vfs_error(failure),
    };
    if metadata.mode & 0o170000 != 0o040000 {
        return error(20);
    }
    let Some((path, length)) = vfs::descriptor_path(process_fd.handle) else {
        return error(9);
    };
    set_current_directory(&path[..length]);
    CHDIR_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_chdir(path_address: u64) -> u64 {
    let mut resolved = [0u8; MAX_PATH];
    let (length, relative) = match resolve_user_path(AT_FDCWD as u64, path_address, &mut resolved) {
        Ok(value) => value,
        Err(failure) => return failure,
    };
    let Ok(path) = core::str::from_utf8(&resolved[..length]) else {
        return error(84);
    };
    let metadata = match vfs::metadata(path) {
        Ok(metadata) => metadata,
        Err(failure) => return vfs_error(failure),
    };
    if metadata.mode & 0o170000 != 0o040000 {
        return error(20);
    }
    set_current_directory(&resolved[..length]);
    CHDIR_CALLS.fetch_add(1, Ordering::Relaxed);
    PATH_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    if relative {
        RELATIVE_PATH_CALLS.fetch_add(1, Ordering::Relaxed);
    }
    0
}

/// Sets the process's file-mode creation mask, returning the previous
/// value (POSIX `umask()` semantics). Applied by [`linux_mkdirat`] and
/// [`linux_openat`]'s `O_CREAT` path to whatever mode the caller requested.
fn linux_umask(new_mask: u64) -> u64 {
    let masked = new_mask & 0o777;
    let previous = UMASK.swap(masked, Ordering::AcqRel);
    RUNTIME_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    previous
}

fn apply_umask(mode: u16) -> u16 {
    mode & !(UMASK.load(Ordering::Acquire) as u16)
}

/// Builds a real glibc/kernel `struct sysinfo` (x86_64 layout: 112 bytes,
/// with a 4-byte implicit gap at offset 84 before `totalhigh` - verified
/// against `offsetof` on real glibc headers via a throwaway C probe, not
/// written from memory). `loads`, `sharedram`, `bufferram`,
/// `totalswap`/`freeswap` and `totalhigh`/`freehigh` are honestly zero:
/// AerOS tracks none of load averages, shared/buffer memory, swap, or high
/// memory. Everything else (uptime, ram totals, live process count) is
/// real, live kernel state. Split out from [`linux_sysinfo`] so the self
/// test can check the encoding without a mapped user address to write to.
fn build_sysinfo_bytes() -> [u8; 112] {
    let uptime_seconds = crate::time::monotonic_nanoseconds() / 1_000_000_000;
    let allocator = crate::memory::global_stats().unwrap_or(crate::memory::AllocatorStats {
        free_pages: 0,
        allocated_pages: 0,
        free_ranges: 0,
        managed_regions: 0,
    });
    let total_ram = allocator
        .free_pages
        .saturating_add(allocator.allocated_pages)
        .saturating_mul(4096);
    let free_ram = allocator.free_pages.saturating_mul(4096);
    let table = crate::process::stats();
    let procs = table
        .spawned
        .saturating_sub(table.reaped)
        .min(u16::MAX as u64) as u16;

    let mut buffer = [0u8; 112];
    buffer[0..8].copy_from_slice(&(uptime_seconds as i64).to_le_bytes());
    buffer[32..40].copy_from_slice(&total_ram.to_le_bytes());
    buffer[40..48].copy_from_slice(&free_ram.to_le_bytes());
    buffer[80..82].copy_from_slice(&procs.to_le_bytes());
    buffer[104..108].copy_from_slice(&1u32.to_le_bytes());
    buffer
}

fn linux_sysinfo(address: u64) -> u64 {
    if !user::copy_to_user(address, &build_sysinfo_bytes()) {
        return error(14);
    }
    RUNTIME_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

/// Exercises [`build_sysinfo_bytes`] directly (no mapped user address
/// exists in kernel self-test context) and checks the encoding against
/// the empirically-verified `struct sysinfo` field offsets.
pub(crate) fn sysinfo_self_test() -> bool {
    let buffer = build_sysinfo_bytes();
    let uptime = u64::from_le_bytes(buffer[0..8].try_into().unwrap());
    let total_ram = u64::from_le_bytes(buffer[32..40].try_into().unwrap());
    let free_ram = u64::from_le_bytes(buffer[40..48].try_into().unwrap());
    let mem_unit = u32::from_le_bytes(buffer[104..108].try_into().unwrap());
    uptime > 0
        && total_ram > 0
        && free_ram <= total_ram
        && mem_unit == 1
        && buffer[108..112] == [0u8; 4]
}

const MS_BIND: u64 = 0x1000;

fn mount_error(failure: crate::mounts::MountError) -> u64 {
    use crate::mounts::MountError;
    match failure {
        MountError::InvalidPath | MountError::NotMounted => error(22),
        MountError::NotDirectory => error(20),
        MountError::NotAllowed => error(1),
        MountError::Busy => error(16),
        MountError::Full => error(12),
    }
}

/// `mount(source, target, fstype, flags, data)`: only `MS_BIND` of a volume
/// directory over a directory of the root tree is supported.
fn linux_mount(source_address: u64, target_address: u64, flags: u64) -> u64 {
    if !has_capability(crate::capability::CAP_SYS_ADMIN) {
        return error(1);
    }
    if flags & MS_BIND == 0 {
        return error(19);
    }
    let mut source = [0u8; MAX_PATH];
    let mut target = [0u8; MAX_PATH];
    let (source_length, _) = match resolve_user_path(AT_FDCWD as u64, source_address, &mut source) {
        Ok(value) => value,
        Err(failure) => return failure,
    };
    let (target_length, _) = match resolve_user_path(AT_FDCWD as u64, target_address, &mut target) {
        Ok(value) => value,
        Err(failure) => return failure,
    };
    let (Ok(source), Ok(target)) = (
        core::str::from_utf8(&source[..source_length]),
        core::str::from_utf8(&target[..target_length]),
    ) else {
        return error(84);
    };
    match crate::mounts::bind(target, source) {
        Ok(()) => {
            PATH_CALLS.fetch_add(1, Ordering::Relaxed);
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            0
        }
        Err(failure) => mount_error(failure),
    }
}

fn linux_umount2(target_address: u64) -> u64 {
    if !has_capability(crate::capability::CAP_SYS_ADMIN) {
        return error(1);
    }
    let mut target = [0u8; MAX_PATH];
    let (length, _) = match resolve_user_path(AT_FDCWD as u64, target_address, &mut target) {
        Ok(value) => value,
        Err(failure) => return failure,
    };
    let Ok(target) = core::str::from_utf8(&target[..length]) else {
        return error(84);
    };
    match crate::mounts::unbind(target) {
        Ok(()) => 0,
        Err(failure) => mount_error(failure),
    }
}

fn linux_mkdirat(directory: u64, path_address: u64, mode: u64) -> u64 {
    if mode & !0o777 != 0 {
        return error(22);
    }
    let mut resolved = [0u8; MAX_PATH];
    let (length, relative) = match resolve_user_path(directory, path_address, &mut resolved) {
        Ok(value) => value,
        Err(failure) => return failure,
    };
    let Ok(path) = core::str::from_utf8(&resolved[..length]) else {
        return error(84);
    };
    match vfs::create_directory(path, apply_umask(mode as u16)) {
        Ok(()) => {
            CREATE_CALLS.fetch_add(1, Ordering::Relaxed);
            PATH_CALLS.fetch_add(1, Ordering::Relaxed);
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            if relative {
                RELATIVE_PATH_CALLS.fetch_add(1, Ordering::Relaxed);
            }
            0
        }
        Err(failure) => vfs_error(failure),
    }
}

fn linux_renameat(
    source_directory: u64,
    source_address: u64,
    destination_directory: u64,
    destination_address: u64,
    flags: u64,
) -> u64 {
    if flags & !RENAME_NOREPLACE != 0 {
        return error(22);
    }
    let mut source = [0u8; MAX_PATH];
    let mut destination = [0u8; MAX_PATH];
    let (source_length, source_relative) =
        match resolve_user_path(source_directory, source_address, &mut source) {
            Ok(value) => value,
            Err(failure) => return failure,
        };
    let (destination_length, destination_relative) =
        match resolve_user_path(destination_directory, destination_address, &mut destination) {
            Ok(value) => value,
            Err(failure) => return failure,
        };
    let Ok(source_path) = core::str::from_utf8(&source[..source_length]) else {
        return error(84);
    };
    let Ok(destination_path) = core::str::from_utf8(&destination[..destination_length]) else {
        return error(84);
    };
    let renamed = if flags & RENAME_NOREPLACE != 0 {
        vfs::rename_noreplace(source_path, destination_path)
    } else {
        vfs::rename(source_path, destination_path)
    };
    match renamed {
        Ok(()) => {
            RENAME_CALLS.fetch_add(1, Ordering::Relaxed);
            PATH_CALLS.fetch_add(1, Ordering::Relaxed);
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            if source_relative || destination_relative {
                RELATIVE_PATH_CALLS.fetch_add(1, Ordering::Relaxed);
            }
            0
        }
        Err(failure) => vfs_error(failure),
    }
}

fn linux_unlinkat(directory: u64, path_address: u64, flags: u64) -> u64 {
    if flags & !AT_REMOVEDIR != 0 {
        return error(22);
    }
    let mut resolved = [0u8; MAX_PATH];
    let (length, relative) = match resolve_user_path(directory, path_address, &mut resolved) {
        Ok(value) => value,
        Err(failure) => return failure,
    };
    let Ok(path) = core::str::from_utf8(&resolved[..length]) else {
        return error(84);
    };
    match vfs::remove(path, flags & AT_REMOVEDIR != 0) {
        Ok(()) => {
            REMOVE_CALLS.fetch_add(1, Ordering::Relaxed);
            PATH_CALLS.fetch_add(1, Ordering::Relaxed);
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            if relative {
                RELATIVE_PATH_CALLS.fetch_add(1, Ordering::Relaxed);
            }
            0
        }
        Err(failure) => vfs_error(failure),
    }
}

fn memfd_self_test() -> bool {
    let (Ok(first), Ok(second)) = (create_memfd(false), create_memfd(true)) else {
        return false;
    };
    let (Some(a), Some(b)) = (lookup_process_fd(first), lookup_process_fd(second)) else {
        return false;
    };
    let path_of = |handle: u32| {
        vfs::descriptor_path(handle).map(|(buffer, length)| {
            let mut copy = [0u8; 128];
            copy[..length].copy_from_slice(&buffer[..length]);
            (copy, length)
        })
    };
    let (Some((first_path, first_length)), Some((second_path, second_length))) =
        (path_of(a.handle), path_of(b.handle))
    else {
        return false;
    };
    let exists =
        |path: &[u8]| core::str::from_utf8(path).is_ok_and(|path| vfs::metadata(path).is_ok());
    let first_path = &first_path[..first_length];
    let second_path = &second_path[..second_length];
    let distinct = first_path != second_path && first_path.starts_with(b"/tmp/.memfd-");
    let written = vfs::write(a.handle, b"memfd data", false) == Ok(10);
    let mut readback = [0u8; 10];
    let read_ok = vfs::read_at(a.handle, 0, &mut readback) == Ok(10) && &readback == b"memfd data";
    let alive = exists(first_path) && exists(second_path);
    let duplicate = linux_dup(first);
    let first_closed = linux_close(first) == 0 && exists(first_path);
    let last_closed = linux_close(duplicate) == 0 && !exists(first_path);
    let second_closed = linux_close(second) == 0 && !exists(second_path);
    let flags_ok = linux_memfd_create(0, 8) == error(22);
    distinct
        && written
        && read_ok
        && alive
        && first_closed
        && last_closed
        && second_closed
        && flags_ok
}

/// Runs `body` and puts the statistics it touched back, so kernel-side tests
/// do not disturb the exact counts later checks expect from user processes.
fn preserving_statistics<R>(body: impl FnOnce() -> R) -> R {
    let counters = [&SOCKET_CALLS, &CLOSES, &COMPAT_CALLS, &IO_BYTES];
    let saved = counters.map(|counter| counter.load(Ordering::Relaxed));
    let result = body();
    for (counter, value) in counters.iter().zip(saved) {
        counter.store(value, Ordering::Relaxed);
    }
    result
}

#[cfg(feature = "boot-test")]
pub(crate) fn tcp_net_echo_self_test(port: u16, timeout_ns: u64) -> (bool, usize, bool) {
    let report = preserving_statistics(|| tcpsock::net_echo_test(port, timeout_ns));
    (report.accepted, report.echoed, report.closed)
}

pub(crate) fn proc_capabilities() -> crate::capability::Capabilities {
    current_capabilities()
}

/// 0 none, 1 strict, 2 filter - the value `/proc/self/status` reports.
pub(crate) fn proc_seccomp_mode() -> u8 {
    if SECCOMP_STRICT.load(Ordering::Acquire) {
        1
    } else if SECCOMP_FILTERED.load(Ordering::Acquire) {
        2
    } else {
        0
    }
}

#[cfg(feature = "boot-test")]
pub(crate) fn udp_socket_self_test() -> bool {
    preserving_statistics(|| {
        crate::udp::reset();
        let verified = udp_scenario();
        reset_udp_sockets();
        crate::udp::reset();
        verified
    })
}

#[cfg(feature = "boot-test")]
fn udp_scenario() -> bool {
    let loopback4 = [127, 0, 0, 1];
    let loopback = crate::ip::v4(loopback4);
    let (Ok(server_fd), Ok(client_fd)) = (
        create_udp_socket(false, true, false),
        create_udp_socket(false, true, false),
    ) else {
        return false;
    };
    let (Some(server), Some(client)) = (lookup_process_fd(server_fd), lookup_process_fd(client_fd))
    else {
        return false;
    };
    let socket_of = |process_fd: ProcessFd| SOCKETS.lock()[process_fd.handle as usize];
    let nonblocking = server.nonblocking && client.nonblocking;
    let bound = bind_udp_port(server, 6101) == 0
        && bind_udp_port(client, 6101) == error(98)
        && socket_of(server).local_port == 6101;
    let client_port = socket_of(client).local_port;
    let mut buffer = [0u8; 64];

    let quiet = fd_ready_events(server_fd) & EPOLLIN == 0
        && fd_ready_events(server_fd) & EPOLLOUT != 0
        && matches!(udp_receive(&socket_of(server), true, &mut buffer), Err(11));
    let sent = crate::net::send_udp(loopback4, client_port, 6101, b"ping");
    let readable = fd_ready_events(server_fd) & EPOLLIN != 0;
    let received = udp_receive(&socket_of(server), true, &mut buffer).is_ok_and(|datagram| {
        datagram.bytes == 4
            && &buffer[..4] == b"ping"
            && datagram.source == loopback
            && datagram.source_port == client_port
    });
    let drained = fd_ready_events(server_fd) & EPOLLIN == 0;

    {
        let mut sockets = SOCKETS.lock();
        let socket = &mut sockets[server.handle as usize];
        socket.connected = true;
        socket.remote = loopback;
        socket.remote_port = 9999;
    }
    let stranger = crate::net::send_udp(loopback4, client_port, 6101, b"stranger");
    let filtered =
        stranger && matches!(udp_receive(&socket_of(server), true, &mut buffer), Err(11));
    let friend = crate::udp::deliver(6101, loopback, 9999, b"friend")
        && udp_receive(&socket_of(server), true, &mut buffer)
            .is_ok_and(|datagram| datagram.bytes == 6 && &buffer[..6] == b"friend");

    let reply = crate::net::send_udp(loopback4, 6101, client_port, b"pong")
        && udp_receive(&socket_of(client), true, &mut buffer)
            .is_ok_and(|datagram| datagram.bytes == 4 && datagram.source_port == 6101);

    let closed = linux_close(server_fd) == 0 && crate::udp::slot_of_port(6101).is_none();
    let reuse = create_udp_socket(false, false, false)
        .ok()
        .and_then(lookup_process_fd)
        .is_some_and(|process_fd| {
            let rebound = bind_udp_port(process_fd, 6101) == 0;
            let descriptor = PROCESS_FDS
                .lock()
                .iter()
                .position(|candidate| {
                    candidate.open && candidate.socket && candidate.handle == process_fd.handle
                })
                .map_or(u64::MAX, |index| index as u64);
            rebound && linux_close(descriptor) == 0
        });
    let client_closed = linux_close(client_fd) == 0;
    nonblocking
        && bound
        && quiet
        && sent
        && readable
        && received
        && drained
        && filtered
        && friend
        && reply
        && closed
        && reuse
        && client_closed
}

pub(crate) fn tcp_socket_self_test() -> bool {
    preserving_statistics(tcpsock::self_test)
}

/// Socket options, time-outs, `MSG_PEEK` and `MSG_TRUNC` for stream and
/// datagram sockets.
#[cfg(feature = "boot-test")]
pub(crate) fn socket_options_self_test() -> bool {
    preserving_statistics(|| {
        let streams = tcpsock::options_self_test();
        crate::udp::reset();
        let datagrams = udp_options_scenario();
        reset_udp_sockets();
        crate::udp::reset();
        let options = crate::sockopt::self_test();
        options && streams && datagrams
    })
}

#[cfg(feature = "boot-test")]
fn udp_options_scenario() -> bool {
    let (Ok(server_fd), Ok(client_fd)) = (
        create_udp_socket(false, false, false),
        create_udp_socket(false, false, false),
    ) else {
        return false;
    };
    let (Some(server), Some(client)) = (lookup_process_fd(server_fd), lookup_process_fd(client_fd))
    else {
        return false;
    };
    let socket_of = |process_fd: ProcessFd| SOCKETS.lock()[process_fd.handle as usize];
    let bound = bind_udp_port(server, 6301) == 0;
    let mut timeout = [0u8; 16];
    timeout[8..].copy_from_slice(&60_000i64.to_le_bytes());
    SOCKETS.lock()[server.handle as usize]
        .options
        .set(SOL_SOCKET, 20, &timeout)
        .ok();
    let client_port = socket_of(client).local_port;
    let mut buffer = [0u8; 32];
    let started = crate::time::monotonic_nanoseconds();
    let timed_out = matches!(udp_receive(&socket_of(server), false, &mut buffer), Err(11))
        && (50_000_000..1_000_000_000)
            .contains(&crate::time::monotonic_nanoseconds().saturating_sub(started));
    let sent = crate::net::send_udp([127, 0, 0, 1], client_port, 6301, b"nine byte");
    let peeked = udp_receive_with(&socket_of(server), true, &mut buffer, true)
        .is_ok_and(|(datagram, full)| datagram.bytes == 9 && full == 9);
    let ready = crate::udp::next_length(socket_of(server).binding) == 9;
    let truncated = udp_receive_with(&socket_of(server), true, &mut buffer[..4], false)
        .is_ok_and(|(datagram, full)| datagram.bytes == 4 && full == 9 && &buffer[..4] == b"nine");
    let consumed = matches!(udp_receive(&socket_of(server), true, &mut buffer), Err(11));
    let closed = linux_close(server_fd) == 0 && linux_close(client_fd) == 0;
    bound && timed_out && sent && peeked && ready && truncated && consumed && closed
}

#[cfg(feature = "boot-test")]
pub(crate) fn inet6_socket_self_test() -> bool {
    preserving_statistics(|| {
        let streams = tcpsock::inet6_self_test();
        crate::udp::reset();
        let datagrams = udp6_scenario();
        reset_udp_sockets();
        crate::udp::reset();
        streams && datagrams
    })
}

/// Datagrams over `AF_INET6` sockets: replies find their way back to the
/// sender's address, a connected socket only hears its peer, and an IPv4
/// datagram arrives on an `AF_INET6` socket as an IPv4-mapped address.
#[cfg(feature = "boot-test")]
fn udp6_scenario() -> bool {
    use crate::ip;
    let (Ok(server_fd), Ok(client_fd)) = (
        create_udp_socket(false, true, true),
        create_udp_socket(false, true, true),
    ) else {
        return false;
    };
    let (Some(server), Some(client)) = (lookup_process_fd(server_fd), lookup_process_fd(client_fd))
    else {
        return false;
    };
    let socket_of = |process_fd: ProcessFd| SOCKETS.lock()[process_fd.handle as usize];
    let bound = bind_udp_port(server, 6201) == 0;
    let client_port = socket_of(client).local_port;
    let mut buffer = [0u8; 64];
    let sent = crate::net::send_datagram(&ip::LOOPBACK6, client_port, 6201, b"v6 ping");
    let received = udp_receive(&socket_of(server), true, &mut buffer).is_ok_and(|datagram| {
        datagram.bytes == 7
            && &buffer[..7] == b"v6 ping"
            && datagram.source == ip::LOOPBACK6
            && datagram.source_port == client_port
    });
    let reply = crate::net::send_datagram(&ip::LOOPBACK6, 6201, client_port, b"v6 pong")
        && udp_receive(&socket_of(client), true, &mut buffer)
            .is_ok_and(|datagram| datagram.bytes == 7 && datagram.source_port == 6201);
    {
        let mut sockets = SOCKETS.lock();
        let socket = &mut sockets[server.handle as usize];
        socket.connected = true;
        socket.remote = ip::LOOPBACK6;
        socket.remote_port = 9999;
    }
    let stranger = crate::net::send_datagram(&ip::LOOPBACK6, client_port, 6201, b"stranger");
    let filtered =
        stranger && matches!(udp_receive(&socket_of(server), true, &mut buffer), Err(11));
    SOCKETS.lock()[server.handle as usize].connected = false;
    let drained = udp_receive(&socket_of(server), true, &mut buffer)
        .is_ok_and(|datagram| datagram.bytes == 8 && &buffer[..8] == b"stranger");
    let mapped = drained
        && crate::net::send_udp([127, 0, 0, 1], client_port, 6201, b"v4")
        && udp_receive(&socket_of(server), true, &mut buffer)
            .is_ok_and(|datagram| datagram.bytes == 2 && datagram.source == ip::v4([127, 0, 0, 1]));
    let (bytes, size) = sockaddr_bytes(&socket_of(client).remote, 0, process_fd_v6(&client));
    let layout = size == 28 && bytes[..2] == [10, 0];
    let closed = linux_close(server_fd) == 0 && linux_close(client_fd) == 0;
    bound && sent && received && reply && filtered && mapped && layout && closed
}

#[cfg(feature = "boot-test")]
fn process_fd_v6(process_fd: &ProcessFd) -> bool {
    process_fd.inet6
}

pub(crate) fn misc_syscalls_self_test() -> bool {
    let flock_ok = [1, 2, 8, 1 | 4, 2 | 4, 8 | 4]
        .iter()
        .all(|operation| flock_operation_valid(*operation))
        && [0, 3, 7, 16, 32]
            .iter()
            .all(|operation| !flock_operation_valid(*operation));

    let mut descriptors = [0u64; 3];
    for descriptor in &mut descriptors {
        match create_eventfd(0, false, false) {
            Ok(created) => *descriptor = created,
            Err(_) => return false,
        }
    }
    let [first, second, third] = descriptors;
    let consecutive = third == second + 1 && second > first;
    let invalid_range =
        linux_close_range(5, 4, 0) == error(22) && linux_close_range(0, 1, 8) == error(22);
    let cloexec = linux_close_range(first, first, 4) == 0
        && lookup_process_fd(first).is_some_and(|process_fd| process_fd.close_on_exec);
    let range_closed = consecutive
        && linux_close_range(second, third, 0) == 0
        && lookup_process_fd(second).is_none()
        && lookup_process_fd(third).is_none()
        && lookup_process_fd(first).is_some();
    let first_closed = linux_close(first) == 0;
    if !consecutive {
        let _ = linux_close(second);
        let _ = linux_close(third);
    }

    let path = "/tmp/fallocate.test";
    let fallocate_ok = vfs::open_file(path, true, false, true, 0o600, true).is_ok_and(|handle| {
        let size = |handle: u32| vfs::descriptor_metadata(handle).map_or(u64::MAX, |m| m.size);
        let grows = fallocate_apply(handle, 0, 10, 90) == 0 && size(handle) == 100;
        let keeps = fallocate_apply(handle, 1, 0, 500) == 0 && size(handle) == 100;
        let shrinks_never = fallocate_apply(handle, 0, 0, 50) == 0 && size(handle) == 100;
        let refused = fallocate_apply(handle, 2, 0, 8) == error(95)
            && fallocate_apply(handle, 0, 0, 0) == error(22);
        let _ = vfs::close(handle);
        let removed = vfs::remove(path, false).is_ok();
        grows && keeps && shrinks_never && refused && removed
    });

    let memfd_ok = memfd_self_test() && copy_self_test();
    let personality_ok = personality_result(0xffff_ffff) == 0
        && personality_result(0) == 0
        && personality_result(8) == error(22);
    let policy_ok = scheduler_policy_result(0, 0) == 0
        && scheduler_policy_result(3, 0) == 0
        && scheduler_policy_result(5, 0) == 0
        && scheduler_policy_result(0, 1) == error(22)
        && scheduler_policy_result(1, 10) == error(1)
        && scheduler_policy_result(2, 10) == error(1)
        && scheduler_policy_result(0x4000_0000, 0) == 0
        && scheduler_policy_result(9, 0) == error(22);
    let barrier_ok = linux_membarrier(0, 0) == 0 && linux_membarrier(1, 0) == error(22);

    let iovecs = [(100u64, 4u64), (200, 4), (300, 0), (400, 4)];
    let lookup = |index: usize| iovecs.get(index).copied();
    let mut positions = [0u64; 4];
    let mut calls = 0usize;
    let total = vectored_positional(4, 10, lookup, |_, length, position| {
        positions[calls] = position;
        calls += 1;
        length
    });
    let vectors_ok = total == 12 && calls == 3 && positions[..3] == [10, 14, 18];
    let mut short_calls = 0;
    let short = vectored_positional(4, 0, lookup, |_, length, _| {
        short_calls += 1;
        if short_calls == 2 { length - 2 } else { length }
    });
    let short_ok = short == 6 && short_calls == 2;
    let failing = vectored_positional(4, 0, lookup, |_, _, _| error(5)) == error(5);
    let later_failure = {
        let mut seen = 0;
        vectored_positional(4, 0, lookup, |_, length, _| {
            seen += 1;
            if seen == 1 { length } else { error(5) }
        }) == 4
    };
    let too_many = vectored_positional(17, 0, lookup, |_, length, _| length) == error(22);
    let bad_vector = vectored_positional(2, 0, |_| None, |_, length, _| length) == error(14);

    flock_ok
        && invalid_range
        && cloexec
        && range_closed
        && first_closed
        && fallocate_ok
        && memfd_ok
        && personality_ok
        && policy_ok
        && barrier_ok
        && vectors_ok
        && short_ok
        && failing
        && later_failure
        && too_many
        && bad_vector
}

fn errno_of(failure: VfsError) -> u64 {
    0u64.wrapping_sub(vfs_error(failure))
}

/// Writes to any descriptor `write` accepts, from a kernel buffer.
fn write_kernel_bytes(output: ProcessFd, offset: Option<u64>, data: &[u8]) -> Result<usize, u64> {
    if output.standard != 0 {
        for byte in data {
            crate::serial::byte(*byte);
        }
        return Ok(data.len());
    }
    if output.pipe {
        return pipe_write(output.handle as usize, data);
    }
    if output.unix_socket {
        return unix_socket_write(output.handle as usize, output.unix_end_b, data);
    }
    if output.tcp {
        return tcpsock::write_bytes(output, data);
    }
    match offset {
        Some(position) => vfs::write_at(output.handle, position as usize, data),
        None => vfs::write(output.handle, data, output.append),
    }
    .map_err(errno_of)
}

/// Copies up to `count` bytes from a file to any writable descriptor, like
/// `sendfile` and `copy_file_range`. With an offset the descriptor's own
/// position is left alone; the returned offsets are where each side ended.
fn copy_file_data(
    input: ProcessFd,
    mut in_offset: Option<u64>,
    output: ProcessFd,
    mut out_offset: Option<u64>,
    count: usize,
) -> Result<(usize, Option<u64>, Option<u64>), u64> {
    let mut chunk = [0u8; IO_CHUNK];
    let mut total = 0usize;
    while total < count {
        let amount = (count - total).min(chunk.len());
        let read = match in_offset {
            Some(position) => vfs::read_at(input.handle, position as usize, &mut chunk[..amount]),
            None => vfs::read(input.handle, &mut chunk[..amount]),
        };
        let read = match read {
            Ok(0) => break,
            Ok(read) => read,
            Err(failure) if total == 0 => return Err(errno_of(failure)),
            Err(_) => break,
        };
        let mut written = 0usize;
        while written < read {
            let position = out_offset.map(|start| start + written as u64);
            match write_kernel_bytes(output, position, &chunk[written..read]) {
                Ok(0) => break,
                Ok(count) => written += count,
                Err(failure) if total + written == 0 => return Err(failure),
                Err(_) => break,
            }
        }
        total += written;
        if let Some(position) = &mut in_offset {
            *position += written as u64;
        }
        if let Some(position) = &mut out_offset {
            *position += written as u64;
        }
        if written < read || read < amount {
            break;
        }
    }
    Ok((total, in_offset, out_offset))
}

fn read_offset(address: u64) -> Result<Option<u64>, u64> {
    if address == 0 {
        return Ok(None);
    }
    let mut encoded = [0u8; 8];
    if !user::copy_from_user(address, &mut encoded) {
        return Err(error(14));
    }
    let offset = u64::from_le_bytes(encoded);
    if offset as i64 <= -1 {
        return Err(error(22));
    }
    Ok(Some(offset))
}

fn write_offset(address: u64, offset: Option<u64>) -> bool {
    match offset {
        Some(value) if address != 0 => user::copy_to_user(address, &value.to_le_bytes()),
        _ => true,
    }
}

fn linux_sendfile(output: u64, input: u64, offset_address: u64, count: u64) -> u64 {
    let (Some(source), Some(destination)) = (lookup_file_fd(input), lookup_process_fd(output))
    else {
        return error(9);
    };
    if !source.readable
        || !destination.writable
        || destination.standard == 1
        || (destination.socket && !destination.unix_socket)
    {
        return error(9);
    }
    let offset = match read_offset(offset_address) {
        Ok(offset) => offset,
        Err(failure) => return failure,
    };
    let count = usize::try_from(count).unwrap_or(usize::MAX).min(MAX_IO);
    match copy_file_data(source, offset, destination, None, count) {
        Ok((copied, end, _)) => {
            if !write_offset(offset_address, end) {
                return error(14);
            }
            IO_BYTES.fetch_add(copied as u64, Ordering::Relaxed);
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            copied as u64
        }
        Err(failure) => error(failure),
    }
}

fn linux_copy_file_range(
    input: u64,
    in_offset_address: u64,
    output: u64,
    out_offset_address: u64,
    count: u64,
    flags: u64,
) -> u64 {
    if flags != 0 {
        return error(22);
    }
    let (Some(source), Some(destination)) = (lookup_file_fd(input), lookup_file_fd(output)) else {
        return error(9);
    };
    if !source.readable || !destination.writable {
        return error(9);
    }
    let (in_offset, out_offset) = match (
        read_offset(in_offset_address),
        read_offset(out_offset_address),
    ) {
        (Ok(first), Ok(second)) => (first, second),
        (Err(failure), _) | (_, Err(failure)) => return failure,
    };
    let count = usize::try_from(count).unwrap_or(usize::MAX).min(MAX_IO);
    match copy_file_data(source, in_offset, destination, out_offset, count) {
        Ok((copied, in_end, out_end)) => {
            if !write_offset(in_offset_address, in_end)
                || !write_offset(out_offset_address, out_end)
            {
                return error(14);
            }
            IO_BYTES.fetch_add(copied as u64, Ordering::Relaxed);
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            copied as u64
        }
        Err(failure) => error(failure),
    }
}

fn copy_self_test() -> bool {
    let source_path = "/tmp/copy-source.test";
    let target_path = "/tmp/copy-target.test";
    let Ok(source_handle) = vfs::open_file(source_path, true, false, true, 0o600, true) else {
        return false;
    };
    let Ok(target_handle) = vfs::open_file(target_path, true, false, true, 0o600, true) else {
        let _ = vfs::close(source_handle);
        return false;
    };
    let pattern = |index: usize| (index % 251) as u8;
    let mut data = [0u8; 700];
    for (index, byte) in data.iter_mut().enumerate() {
        *byte = pattern(index);
    }
    let seeded = vfs::write(source_handle, &data, false) == Ok(700);
    let install = |handle: u32| {
        install_process_fd(handle, false, false, true, true, false).and_then(|descriptor| {
            lookup_process_fd(descriptor).map(|process_fd| (descriptor, process_fd))
        })
    };
    let (Some((source_fd, source)), Some((target_fd, target))) =
        (install(source_handle), install(target_handle))
    else {
        return false;
    };

    let offsets = copy_file_data(source, Some(100), target, Some(10), 600);
    let offsets_ok = matches!(offsets, Ok((600, Some(700), Some(610))));
    let mut readback = [0u8; 610];
    let target_ok = vfs::read_at(target_handle, 0, &mut readback) == Ok(610)
        && readback[..10].iter().all(|byte| *byte == 0)
        && readback[10..]
            .iter()
            .enumerate()
            .all(|(index, byte)| *byte == pattern(100 + index));
    let clamped = matches!(
        copy_file_data(source, Some(650), target, Some(0), 500),
        Ok((50, Some(700), Some(50)))
    );
    let at_end = matches!(
        copy_file_data(source, Some(700), target, Some(0), 10),
        Ok((0, Some(700), Some(0)))
    );
    let sequential = vfs::seek(source_handle, 0, 0) == Ok(0)
        && vfs::seek(target_handle, 0, 0) == Ok(0)
        && matches!(
            copy_file_data(source, None, target, None, 300),
            Ok((300, None, None))
        )
        && vfs::seek(source_handle, 0, 1) == Ok(300)
        && vfs::seek(target_handle, 0, 1) == Ok(300);

    let pipe_ok = allocate_test_pipe(0).is_some_and(|slot| {
        let pipe_fd = install_process_fd_kind(
            slot as u32,
            false,
            false,
            true,
            false,
            false,
            false,
            false,
            true,
            true,
            false,
        );
        let result = pipe_fd
            .and_then(|descriptor| lookup_process_fd(descriptor).map(|fd| (descriptor, fd)))
            .is_some_and(|(descriptor, pipe_fd)| {
                let mut received = [0u8; 100];
                let ok = matches!(
                    copy_file_data(source, Some(0), pipe_fd, None, 100),
                    Ok((100, Some(100), None))
                ) && pipe_read(slot, &mut received) == Ok(100)
                    && received
                        .iter()
                        .enumerate()
                        .all(|(index, byte)| *byte == pattern(index));
                let _ = remove_process_fd(descriptor);
                ok
            });
        PIPES.lock()[slot] = Pipe::EMPTY;
        result
    });

    let _ = remove_process_fd(source_fd);
    let _ = remove_process_fd(target_fd);
    let _ = vfs::close(source_handle);
    let _ = vfs::close(target_handle);
    let removed =
        vfs::remove(source_path, false).is_ok() && vfs::remove(target_path, false).is_ok();
    seeded && offsets_ok && target_ok && clamped && at_end && sequential && pipe_ok && removed
}

static MEMFD_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A hidden tmpfs file behind a descriptor. Sealing is not supported, so
/// `MFD_ALLOW_SEALING` is accepted and has no effect.
fn create_memfd(close_on_exec: bool) -> Result<u64, u64> {
    let mut path = [0u8; 32];
    let prefix = b"/tmp/.memfd-";
    path[..prefix.len()].copy_from_slice(prefix);
    let mut digits = [0u8; 20];
    let mut value = MEMFD_COUNTER.fetch_add(1, Ordering::AcqRel);
    let mut count = 0;
    loop {
        digits[count] = b'0' + (value % 10) as u8;
        count += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    let length = prefix.len() + count;
    for index in 0..count {
        path[prefix.len() + index] = digits[count - 1 - index];
    }
    let path = core::str::from_utf8(&path[..length]).map_err(|_| 22u64)?;
    let handle = vfs::open_file(path, true, true, false, 0o600, true)
        .map_err(|failure| 0u64.wrapping_sub(vfs_error(failure)))?;
    let Some(descriptor) = install_process_fd(handle, close_on_exec, false, true, true, false)
    else {
        let _ = vfs::close(handle);
        let _ = vfs::remove(path, false);
        return Err(24);
    };
    PROCESS_FDS.lock()[descriptor as usize].memfd = true;
    Ok(descriptor)
}

fn linux_memfd_create(name_address: u64, flags: u64) -> u64 {
    const MFD_CLOEXEC: u64 = 1;
    const MFD_ALLOW_SEALING: u64 = 2;
    if flags & !(MFD_CLOEXEC | MFD_ALLOW_SEALING) != 0 {
        return error(22);
    }
    let mut name = [0u8; 250];
    if user::copy_string(name_address, &mut name).is_none() {
        return error(14);
    }
    match create_memfd(flags & MFD_CLOEXEC != 0) {
        Ok(descriptor) => {
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            descriptor
        }
        Err(failure) => error(failure),
    }
}

fn flock_operation_valid(operation: u64) -> bool {
    const LOCK_NB: u64 = 4;
    matches!(operation & !LOCK_NB, 1 | 2 | 8)
}

/// Advisory locks are accepted and not enforced.
fn linux_flock(descriptor: u64, operation: u64) -> u64 {
    if lookup_process_fd(descriptor).is_none() {
        return error(9);
    }
    if !flock_operation_valid(operation) {
        return error(22);
    }
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_syncfs(descriptor: u64) -> u64 {
    if lookup_process_fd(descriptor).is_none() {
        return error(9);
    }
    SYNC_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_close_range(first: u64, last: u64, flags: u64) -> u64 {
    const CLOSE_RANGE_UNSHARE: u64 = 2;
    const CLOSE_RANGE_CLOEXEC: u64 = 4;
    if flags & !(CLOSE_RANGE_UNSHARE | CLOSE_RANGE_CLOEXEC) != 0 || first > last {
        return error(22);
    }
    let end = last.min(PROCESS_FD_COUNT as u64 - 1);
    for descriptor in first..=end {
        if flags & CLOSE_RANGE_CLOEXEC != 0 {
            if let Some(index) = process_fd_index(descriptor) {
                PROCESS_FDS.lock()[index].close_on_exec = true;
            }
        } else if lookup_process_fd(descriptor).is_some() {
            let _ = linux_close(descriptor);
        }
    }
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

/// Runs `transfer(base, length, position)` over each iovec in turn, as
/// `preadv`/`pwritev` do with `pread64`/`pwrite64`.
fn vectored_positional(
    count: u64,
    offset: u64,
    iovec: impl Fn(usize) -> Option<(u64, u64)>,
    mut transfer: impl FnMut(u64, u64, u64) -> u64,
) -> u64 {
    if count > 16 {
        return error(22);
    }
    let mut total = 0u64;
    for index in 0..count as usize {
        let Some((base, length)) = iovec(index) else {
            return if total == 0 { error(14) } else { total };
        };
        if length == 0 {
            continue;
        }
        let Some(position) = offset.checked_add(total) else {
            return if total == 0 { error(75) } else { total };
        };
        let moved = transfer(base, length, position);
        if moved as i64 <= 0 {
            return if total == 0 { moved } else { total };
        }
        total += moved;
        if moved < length {
            break;
        }
    }
    total
}

fn linux_preadv(descriptor: u64, vectors: u64, count: u64, low: u64, high: u64) -> u64 {
    vectored_positional(
        count,
        low | high << 32,
        |index| read_iovec(vectors, index),
        |base, length, position| linux_pread64(descriptor, base, length, position),
    )
}

fn linux_pwritev(descriptor: u64, vectors: u64, count: u64, low: u64, high: u64) -> u64 {
    vectored_positional(
        count,
        low | high << 32,
        |index| read_iovec(vectors, index),
        |base, length, position| linux_pwrite64(descriptor, base, length, position),
    )
}

/// Mode 0 grows the file to cover the range; `FALLOC_FL_KEEP_SIZE` only
/// promises space, which a RAM or FAT file here does not reserve.
fn fallocate_apply(handle: u32, mode: u64, offset: u64, length: u64) -> u64 {
    const FALLOC_FL_KEEP_SIZE: u64 = 1;
    if length == 0 || offset as i64 <= -1 || length as i64 <= 0 {
        return error(22);
    }
    if mode & !FALLOC_FL_KEEP_SIZE != 0 {
        return error(95);
    }
    if mode & FALLOC_FL_KEEP_SIZE != 0 {
        return 0;
    }
    let Some(end) = offset
        .checked_add(length)
        .and_then(|end| usize::try_from(end).ok())
    else {
        return error(27);
    };
    let size = match vfs::descriptor_metadata(handle) {
        Ok(metadata) => metadata.size,
        Err(failure) => return vfs_error(failure),
    };
    if end as u64 > size
        && let Err(failure) = vfs::truncate(handle, end)
    {
        return vfs_error(failure);
    }
    0
}

fn linux_fallocate(descriptor: u64, mode: u64, offset: u64, length: u64) -> u64 {
    let Some(process_fd) = lookup_file_fd(descriptor) else {
        return error(9);
    };
    if !process_fd.writable {
        return error(9);
    }
    let result = fallocate_apply(process_fd.handle, mode, offset, length);
    if result == 0 {
        COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    }
    result
}

/// `0` (`PER_LINUX`) is the only execution domain; asking for another fails.
fn personality_result(requested: u64) -> u64 {
    if requested == 0xffff_ffff || requested == 0 {
        0
    } else {
        error(22)
    }
}

/// Only the default time-sharing policies exist; real-time ones need
/// privilege this scheduler cannot honour, so they are refused.
fn scheduler_policy_result(policy: u64, priority: u64) -> u64 {
    match policy & !0x4000_0000 {
        0 | 3 | 5 if priority == 0 => 0,
        0 | 3 | 5 => error(22),
        1 | 2 => error(1),
        _ => error(22),
    }
}

fn linux_sched_setscheduler(pid: u64, policy: u64, parameter: u64) -> u64 {
    if (pid as i64) < 0 {
        return error(22);
    }
    let mut encoded = [0u8; 4];
    if !user::copy_from_user(parameter, &mut encoded) {
        return error(14);
    }
    let priority = i32::from_le_bytes(encoded);
    scheduler_policy_result(policy, priority as u32 as u64)
}

fn linux_mincore(address: u64, length: u64, vector: u64) -> u64 {
    if address & 0xfff != 0 {
        return error(22);
    }
    let Ok(length) = usize::try_from(length) else {
        return error(12);
    };
    let pages = length.div_ceil(4096);
    if !user::range_accessible(address, length, false)
        || !user::range_accessible(vector, pages, true)
    {
        return error(12);
    }
    let resident = [1u8; 64];
    let mut written = 0usize;
    while written < pages {
        let amount = (pages - written).min(resident.len());
        if !user::copy_to_user(vector + written as u64, &resident[..amount]) {
            return error(14);
        }
        written += amount;
    }
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

/// Only the query command exists, and it reports no supported commands.
fn linux_membarrier(command: u64, flags: u64) -> u64 {
    if flags != 0 || command != 0 {
        return error(22);
    }
    0
}

fn linux_fsync(descriptor: u64) -> u64 {
    let Some(process_fd) = lookup_file_fd(descriptor) else {
        return error(9);
    };
    if let Err(failure) = vfs::descriptor_metadata(process_fd.handle) {
        return vfs_error(failure);
    }
    SYNC_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_ftruncate(descriptor: u64, length: u64) -> u64 {
    let Some(process_fd) = lookup_file_fd(descriptor) else {
        return error(9);
    };
    if !process_fd.writable {
        return error(22);
    }
    let Ok(length) = usize::try_from(length) else {
        return error(27);
    };
    match vfs::truncate(process_fd.handle, length) {
        Ok(()) => {
            TRUNCATE_CALLS.fetch_add(1, Ordering::Relaxed);
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            0
        }
        Err(failure) => vfs_error(failure),
    }
}

fn linux_truncate(path_address: u64, length: u64) -> u64 {
    let Ok(length) = usize::try_from(length) else {
        return error(27);
    };
    let mut resolved = [0u8; MAX_PATH];
    let (path_length, _) = match resolve_user_path(AT_FDCWD as u64, path_address, &mut resolved) {
        Ok(value) => value,
        Err(failure) => return failure,
    };
    let Ok(path) = core::str::from_utf8(&resolved[..path_length]) else {
        return error(84);
    };
    let handle = match vfs::open_file(path, false, false, false, 0, true) {
        Ok(handle) => handle,
        Err(failure) => return vfs_error(failure),
    };
    let result = vfs::truncate(handle, length);
    let _ = vfs::close(handle);
    match result {
        Ok(()) => {
            TRUNCATE_CALLS.fetch_add(1, Ordering::Relaxed);
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            0
        }
        Err(failure) => vfs_error(failure),
    }
}

fn linux_fchmod(descriptor: u64, mode: u64) -> u64 {
    if mode & !0o777 != 0 {
        return error(22);
    }
    let Some(process_fd) = lookup_file_fd(descriptor) else {
        return error(9);
    };
    match vfs::fchmod(process_fd.handle, mode as u16) {
        Ok(()) => {
            CHMOD_CALLS.fetch_add(1, Ordering::Relaxed);
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            0
        }
        Err(failure) => vfs_error(failure),
    }
}

/// AerOS has no multi-user model -- `getuid`/`getgid`/`geteuid`/`getegid`
/// all unconditionally report 0, and `vfs::Node` has no owner fields to
/// change. `chown` and friends are therefore implemented as "validate the
/// target exists (and the fd/path arguments are well-formed), then
/// succeed" rather than actually mutating anything, the same reasoning
/// already applied to `msync`. This still gives real callers (tar, cp -p,
/// install scripts) a correct ENOENT instead of ENOSYS.
fn linux_fchownat(directory: u64, path_address: u64, flags: u64) -> u64 {
    if !has_capability(crate::capability::CAP_CHOWN) {
        return error(1);
    }
    if flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH) != 0 {
        return error(22);
    }
    let mut input = [0u8; MAX_PATH];
    let Some(length) = user::copy_string(path_address, &mut input) else {
        return error(14);
    };
    if length == 0 && flags & AT_EMPTY_PATH != 0 {
        let Some(process_fd) = lookup_file_fd(directory) else {
            return error(9);
        };
        if let Err(failure) = vfs::descriptor_metadata(process_fd.handle) {
            return vfs_error(failure);
        }
        METADATA_CALLS.fetch_add(1, Ordering::Relaxed);
        COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
        return 0;
    }
    let mut resolved = [0u8; MAX_PATH];
    let (path_length, relative) = match resolve_user_path(directory, path_address, &mut resolved) {
        Ok(value) => value,
        Err(failure) => return failure,
    };
    let Ok(path) = core::str::from_utf8(&resolved[..path_length]) else {
        return error(84);
    };
    match vfs::metadata(path) {
        Ok(_) => {
            METADATA_CALLS.fetch_add(1, Ordering::Relaxed);
            PATH_CALLS.fetch_add(1, Ordering::Relaxed);
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            if relative {
                RELATIVE_PATH_CALLS.fetch_add(1, Ordering::Relaxed);
            }
            0
        }
        Err(failure) => vfs_error(failure),
    }
}

fn linux_fchown(descriptor: u64) -> u64 {
    if !has_capability(crate::capability::CAP_CHOWN) {
        return error(1);
    }
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return error(9);
    };
    if !process_fd.open {
        return error(9);
    }
    METADATA_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_chmodat(directory: u64, path_address: u64, mode: u64, flags: u64) -> u64 {
    if mode & !0o777 != 0 || flags != 0 {
        return error(22);
    }
    let mut resolved = [0u8; MAX_PATH];
    let (length, relative) = match resolve_user_path(directory, path_address, &mut resolved) {
        Ok(value) => value,
        Err(failure) => return failure,
    };
    let Ok(path) = core::str::from_utf8(&resolved[..length]) else {
        return error(84);
    };
    match vfs::chmod(path, mode as u16) {
        Ok(()) => {
            CHMOD_CALLS.fetch_add(1, Ordering::Relaxed);
            PATH_CALLS.fetch_add(1, Ordering::Relaxed);
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            if relative {
                RELATIVE_PATH_CALLS.fetch_add(1, Ordering::Relaxed);
            }
            0
        }
        Err(failure) => vfs_error(failure),
    }
}

fn resolve_user_path(
    directory: u64,
    path_address: u64,
    output: &mut [u8; MAX_PATH],
) -> Result<(usize, bool), u64> {
    let mut input = [0u8; MAX_PATH];
    let length = user::copy_string(path_address, &mut input).ok_or_else(|| error(14))?;
    let path = core::str::from_utf8(&input[..length]).map_err(|_| error(84))?;
    if path.is_empty() {
        return Err(error(2));
    }
    let relative = !path.starts_with('/');
    if relative && directory as i64 != AT_FDCWD {
        return Err(error(9));
    }
    let mut component_starts = [0usize; 32];
    let mut depth = 0usize;
    let mut output_length = 1usize;
    output.fill(0);
    output[0] = b'/';
    if relative {
        let current = *CURRENT_DIRECTORY.lock();
        let current_path =
            core::str::from_utf8(&current.bytes[..current.length]).map_err(|_| error(84))?;
        append_path_components(
            current_path,
            output,
            &mut output_length,
            &mut component_starts,
            &mut depth,
        )?;
    }
    append_path_components(
        path,
        output,
        &mut output_length,
        &mut component_starts,
        &mut depth,
    )?;
    Ok((output_length, relative))
}

fn append_path_components(
    path: &str,
    output: &mut [u8; MAX_PATH],
    output_length: &mut usize,
    component_starts: &mut [usize; 32],
    depth: &mut usize,
) -> Result<(), u64> {
    for component in path.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            if *depth > 0 {
                *depth -= 1;
                *output_length = component_starts[*depth];
            }
            continue;
        }
        if *depth == component_starts.len()
            || component
                .as_bytes()
                .iter()
                .any(|byte| *byte < 0x20 || *byte == 0x7f)
        {
            return Err(error(36));
        }
        let separator = usize::from(*output_length > 1);
        let end = output_length
            .checked_add(separator)
            .and_then(|value| value.checked_add(component.len()))
            .filter(|value| *value < MAX_PATH)
            .ok_or_else(|| error(36))?;
        component_starts[*depth] = *output_length;
        if separator != 0 {
            output[*output_length] = b'/';
            *output_length += 1;
        }
        output[*output_length..end].copy_from_slice(component.as_bytes());
        *output_length = end;
        *depth += 1;
    }
    Ok(())
}

fn linux_sched_getaffinity(pid: u64, size: u64, address: u64) -> u64 {
    if pid != 0 && pid != current_process_id() {
        return error(3);
    }
    if size < 8 {
        return error(22);
    }
    if !user::copy_to_user(address, &crate::smp::online_mask().to_le_bytes()) {
        return error(14);
    }
    RUNTIME_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    8
}

fn linux_getcpu(cpu_address: u64, node_address: u64, cache_address: u64) -> u64 {
    if cache_address != 0 {
        return error(14);
    }
    if cpu_address != 0 && !user::copy_to_user(cpu_address, &0u32.to_le_bytes()) {
        return error(14);
    }
    if node_address != 0 && !user::copy_to_user(node_address, &0u32.to_le_bytes()) {
        return error(14);
    }
    RUNTIME_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn encode_signal_action(action: SignalAction) -> [u8; 32] {
    let mut encoded = [0u8; 32];
    encoded[..8].copy_from_slice(&action.handler.to_le_bytes());
    encoded[8..16].copy_from_slice(&action.flags.to_le_bytes());
    encoded[16..24].copy_from_slice(&action.restorer.to_le_bytes());
    encoded[24..].copy_from_slice(&action.mask.to_le_bytes());
    encoded
}

fn encode_alternate_stack(stack: AlternateStack) -> [u8; 24] {
    let mut encoded = [0u8; 24];
    encoded[..8].copy_from_slice(&stack.pointer.to_le_bytes());
    encoded[8..12].copy_from_slice(&stack.flags.to_le_bytes());
    encoded[16..].copy_from_slice(&stack.size.to_le_bytes());
    encoded
}

fn read_array_u64(source: &[u8], offset: usize) -> u64 {
    let mut encoded = [0u8; 8];
    encoded.copy_from_slice(&source[offset..offset + 8]);
    u64::from_le_bytes(encoded)
}

fn read_array_u32(source: &[u8], offset: usize) -> u32 {
    let mut encoded = [0u8; 4];
    encoded.copy_from_slice(&source[offset..offset + 4]);
    u32::from_le_bytes(encoded)
}

fn unblockable_signals() -> u64 {
    (1 << 8) | (1 << 18)
}

fn linux_uname(address: u64) -> u64 {
    let mut utsname = [0u8; 390];
    write_uts_field(&mut utsname, 0, b"Linux");
    write_uts_field(&mut utsname, 1, b"aeros");
    write_uts_field(&mut utsname, 2, b"6.8.0-aeros");
    write_uts_field(&mut utsname, 3, b"#1 AerOS SMP");
    write_uts_field(&mut utsname, 4, b"x86_64");
    write_uts_field(&mut utsname, 5, b"(none)");
    if !user::copy_to_user(address, &utsname) {
        return error(14);
    }
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_arch_prctl(code: u64, address: u64) -> u64 {
    let result = match code {
        0x1001 => user::set_thread_base(true, address),
        0x1002 => user::set_thread_base(false, address),
        0x1003 => user::copy_to_user(address, &user::thread_base(false).to_le_bytes()),
        0x1004 => user::copy_to_user(address, &user::thread_base(true).to_le_bytes()),
        _ => return error(22),
    };
    if !result {
        return error(14);
    }
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

/// `seccomp(SECCOMP_SET_MODE_FILTER, 0, &sock_fprog)`: `args` points at a
/// `{ u16 len; u64 filter }` record naming an array of classic-BPF
/// instructions. See `SECCOMP_FILTER` for the one-filter-per-process rule.
fn seccomp_install_filter(flags: u64, args: u64) -> u64 {
    if flags != 0 {
        return error(22);
    }
    let mut header = [0u8; 16];
    if !user::copy_from_user(args, &mut header) {
        return error(14);
    }
    let length = u16::from_le_bytes([header[0], header[1]]) as usize;
    let mut pointer = [0u8; 8];
    pointer.copy_from_slice(&header[8..16]);
    if length == 0 || length > crate::seccomp::MAX_INSTRUCTIONS {
        return error(22);
    }
    let mut raw = [0u8; crate::seccomp::MAX_INSTRUCTIONS * 8];
    if !user::copy_from_user(u64::from_le_bytes(pointer), &mut raw[..length * 8]) {
        return error(14);
    }
    let mut program = [crate::seccomp::Instruction::EMPTY; crate::seccomp::MAX_INSTRUCTIONS];
    for (slot, bytes) in program.iter_mut().zip(raw[..length * 8].chunks_exact(8)) {
        *slot = crate::seccomp::Instruction::from_bytes(bytes);
    }
    let Some(filter) = crate::seccomp::Filter::new(&program[..length]) else {
        return error(22);
    };
    let mut installed = SECCOMP_FILTER.lock();
    if installed.is_some() {
        return error(1);
    }
    *installed = Some(filter);
    SECCOMP_FILTERED.store(true, Ordering::Release);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

/// What an installed filter decides for a syscall: run it, fail it with an
/// errno, or kill the process.
enum FilterVerdict {
    Allow,
    Errno(u64),
    Kill,
}

fn seccomp_verdict(number: u64, instruction_pointer: u64, arguments: &[u64; 6]) -> FilterVerdict {
    let Some(filter) = *SECCOMP_FILTER.lock() else {
        return FilterVerdict::Allow;
    };
    let data = crate::seccomp::data_bytes(number as u32, instruction_pointer, arguments);
    let result = crate::seccomp::run(&filter, &data);
    match result & 0xffff_0000 {
        crate::seccomp::RET_ALLOW | crate::seccomp::RET_LOG => FilterVerdict::Allow,
        crate::seccomp::RET_ERRNO => FilterVerdict::Errno(u64::from((result & 0xffff).min(4095))),
        crate::seccomp::RET_TRACE => FilterVerdict::Errno(38),
        _ => FilterVerdict::Kill,
    }
}

/// `seccomp(SECCOMP_SET_MODE_STRICT, 0, NULL)` - see `SECCOMP_STRICT`'s
/// doc comment for exactly which syscalls remain permitted afterward.
fn linux_seccomp(operation: u64, flags: u64, args: u64) -> u64 {
    if operation == SECCOMP_SET_MODE_FILTER {
        return seccomp_install_filter(flags, args);
    }
    if operation != SECCOMP_SET_MODE_STRICT {
        return error(38);
    }
    if flags != 0 || args != 0 {
        return error(22);
    }
    SECCOMP_STRICT.store(true, Ordering::Release);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_setpgid(pid: u64, pgid: u64) -> u64 {
    let target = if pid == 0 {
        crate::scheduler::current_task_id()
    } else {
        pid
    };
    match crate::scheduler::set_pgid_for(target, pgid) {
        Ok(()) => {
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            0
        }
        Err(errno) => error(errno),
    }
}

fn linux_getpgid(pid: u64) -> u64 {
    let target = if pid == 0 {
        crate::scheduler::current_task_id()
    } else {
        pid
    };
    match crate::scheduler::pgid_of(target) {
        Some(pgid) => {
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            pgid
        }
        None => error(3),
    }
}

fn linux_setsid() -> u64 {
    match crate::scheduler::set_new_session() {
        Ok(sid) => {
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            sid
        }
        Err(errno) => error(errno),
    }
}

fn linux_getsid(pid: u64) -> u64 {
    let target = if pid == 0 {
        crate::scheduler::current_task_id()
    } else {
        pid
    };
    match crate::scheduler::sid_of(target) {
        Some(sid) => {
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            sid
        }
        None => error(3),
    }
}

/// Proves `setpgid`/`getpgid`/`setsid`/`getsid` against this self-test's own
/// kernel task (born its own group AND session leader, like every task -
/// see `Task`'s doc comment in scheduler.rs), which makes `setsid_rejected`
/// below a deterministic check every run, not a timing coincidence: this
/// task is ALWAYS already a leader, so `setsid()` must ALWAYS fail. Doesn't
/// exercise fork() inheriting a parent's group/session (a two-line, directly
/// adjacent copy of the already-proven-correct `parent_id` inheritance right
/// next to it in `fork_current_user_task` - see that function) - just the
/// syscalls' own validation and error paths.
pub(crate) fn process_group_self_test() -> bool {
    let self_id = crate::scheduler::current_task_id();

    let getpgid_self_ok = linux_getpgid(0) == self_id;
    let getsid_self_ok = linux_getsid(0) == self_id;
    let setsid_rejected = linux_setsid() == error(1);
    let setpgid_self_noop_ok = linux_setpgid(0, 0) == 0;

    let bogus_pgid = self_id.wrapping_add(1_000_000);
    let setpgid_bogus_group_rejected = linux_setpgid(0, bogus_pgid) == error(1);

    let bogus_pid = self_id.wrapping_add(2_000_000);
    let setpgid_bogus_pid_rejected = linux_setpgid(bogus_pid, 0) == error(3);
    let getpgid_bogus_pid_rejected = linux_getpgid(bogus_pid) == error(3);
    let getsid_bogus_pid_rejected = linux_getsid(bogus_pid) == error(3);

    let unchanged_after = linux_getpgid(0) == self_id && linux_getsid(0) == self_id;

    getpgid_self_ok
        && getsid_self_ok
        && setsid_rejected
        && setpgid_self_noop_ok
        && setpgid_bogus_group_rejected
        && setpgid_bogus_pid_rejected
        && getpgid_bogus_pid_rejected
        && getsid_bogus_pid_rejected
        && unchanged_after
}

fn linux_set_tid_address(address: u64) -> u64 {
    if address != 0 && !user::range_accessible(address, 4, true) {
        return error(14);
    }
    TID_ADDRESS.store(address, Ordering::Release);
    crate::scheduler::set_clear_child_tid(address);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    current_thread_id()
}

fn linux_set_robust_list(address: u64, length: u64) -> u64 {
    if length != 24 || !user::range_accessible(address, length as usize, true) {
        return error(22);
    }
    ROBUST_LIST.store(address, Ordering::Release);
    crate::scheduler::set_robust_list(address);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_get_robust_list(pid: u64, head_address: u64, length_address: u64) -> u64 {
    if pid != 0 && pid != current_thread_id() {
        return error(3);
    }
    if !user::copy_to_user(head_address, &crate::scheduler::robust_list().to_le_bytes())
        || !user::copy_to_user(length_address, &24u64.to_le_bytes())
    {
        return error(14);
    }
    0
}

fn linux_rseq(address: u64, length: u64, flags: u64, signature: u64) -> u64 {
    if flags == 1 {
        if RSEQ_ADDRESS.load(Ordering::Acquire) != address {
            return error(22);
        }
        RSEQ_ADDRESS.store(0, Ordering::Release);
        RSEQ_CALLS.fetch_add(1, Ordering::Relaxed);
        COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
        return 0;
    }
    if flags != 0
        || length != 32
        || signature != 0x5305_3053
        || address & 31 != 0
        || !user::range_accessible(address, length as usize, true)
    {
        return error(22);
    }
    if RSEQ_ADDRESS
        .compare_exchange(0, address, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return error(16);
    }
    RSEQ_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_futex(arguments: [u64; 6]) -> u64 {
    futex::operate(arguments)
}

/// `RLIMIT_NOFILE` (resource 7) is the only limit actually enforced anywhere
/// (`install_process_fd_kind`) - `setrlimit`-ing any other resource is
/// rejected the same way it always was (EPERM), since nothing would honor
/// it. There's no privileged override, so the hard limit is always
/// `PROCESS_FD_COUNT`: a process can only ever lower its own soft limit,
/// never raise the ceiling.
fn linux_prlimit64(pid: u64, resource: u64, new_limit: u64, old_limit: u64) -> u64 {
    if pid != 0 && pid != current_process_id() {
        return error(3);
    }
    if new_limit != 0 {
        if resource != 7 {
            return error(1);
        }
        let mut requested = [0u8; 16];
        if !user::copy_from_user(new_limit, &mut requested) {
            return error(14);
        }
        let mut soft_bytes = [0u8; 8];
        soft_bytes.copy_from_slice(&requested[..8]);
        let soft = u64::from_le_bytes(soft_bytes);
        let mut hard_bytes = [0u8; 8];
        hard_bytes.copy_from_slice(&requested[8..]);
        let hard = u64::from_le_bytes(hard_bytes);
        if soft > hard || hard > PROCESS_FD_COUNT as u64 {
            return error(22);
        }
        NOFILE_LIMIT.store(soft, Ordering::Release);
    }
    let (current, maximum) = match resource {
        3 => (8 * 1024 * 1024u64, 8 * 1024 * 1024u64),
        7 => (
            NOFILE_LIMIT.load(Ordering::Acquire),
            PROCESS_FD_COUNT as u64,
        ),
        9 => (2 * 1024 * 1024u64, 2 * 1024 * 1024u64),
        0..=15 => (u64::MAX, u64::MAX),
        _ => return error(22),
    };
    if old_limit != 0 {
        let mut limit = [0u8; 16];
        limit[..8].copy_from_slice(&current.to_le_bytes());
        limit[8..].copy_from_slice(&maximum.to_le_bytes());
        if !user::copy_to_user(old_limit, &limit) {
            return error(14);
        }
    }
    RESOURCE_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

/// Proves `RLIMIT_NOFILE` is genuinely enforced by `install_process_fd_kind`,
/// not just a number `prlimit64` echoes back: lowers the limit to exactly
/// the 3 standard descriptors, confirms a brand new fd is refused (EMFILE),
/// then restores the original limit and confirms the identical allocation
/// now succeeds - proving this isn't a one-way ratchet that breaks fd
/// allocation for the rest of boot.
pub(crate) fn rlimit_nofile_self_test() -> bool {
    let original = NOFILE_LIMIT.load(Ordering::Acquire);
    NOFILE_LIMIT.store(3, Ordering::Release);
    let blocked = linux_epoll_create1(0) == error(24);
    NOFILE_LIMIT.store(original, Ordering::Release);
    let allowed = linux_epoll_create1(0);
    let allowed_ok = allowed < 1_000_000;
    if allowed_ok {
        let _ = linux_close(allowed);
    }
    blocked && allowed_ok
}

/// Checks the symlink plumbing behind `lstat`/`symlink`/`readlink`: a link
/// is described as a link by `symlink_metadata`, as its target by `metadata`,
/// and its stored target string round-trips.
pub(crate) fn symlink_syscall_self_test() -> bool {
    let link = "/tmp/.aeros-symlink-probe";
    let _ = vfs::remove(link, false);
    if vfs::symlink(link, "/bin/init").is_err() {
        return false;
    }
    let link_mode = vfs::symlink_metadata(link).map(|m| m.mode & 0o170000);
    let target_mode = vfs::metadata(link).map(|m| m.mode & 0o170000);
    let mut buffer = [0u8; 32];
    let stored = vfs::readlink(link, &mut buffer);
    let removed = vfs::remove(link, false).is_ok();
    link_mode == Ok(0o120000)
        && target_mode == Ok(0o100000)
        && stored == Ok(b"/bin/init".len())
        && &buffer[..b"/bin/init".len()] == b"/bin/init"
        && removed
        && vfs::symlink_metadata(link).is_err()
}

fn linux_fstat(descriptor: u64, address: u64) -> u64 {
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return error(9);
    };
    let metadata = if process_fd.standard != 0 {
        vfs::Metadata {
            inode: process_fd.standard as u64,
            mode: 0o020666,
            size: 0,
            modified: 0,
        }
    } else if process_fd.socket {
        vfs::Metadata {
            inode: process_fd.handle as u64 + 0x1000,
            mode: 0o140777,
            size: 0,
            modified: 0,
        }
    } else if process_fd.pipe {
        vfs::Metadata {
            inode: process_fd.handle as u64 + 0x2000,
            mode: 0o010666,
            size: 0,
            modified: 0,
        }
    } else if process_fd.unix_socket {
        vfs::Metadata {
            inode: process_fd.handle as u64 + 0x3000,
            mode: 0o140777,
            size: 0,
            modified: 0,
        }
    } else if process_fd.unix_listener {
        vfs::Metadata {
            inode: process_fd.handle as u64 + 0x4000,
            mode: 0o140777,
            size: 0,
            modified: 0,
        }
    } else if process_fd.tcp {
        vfs::Metadata {
            inode: process_fd.handle as u64 + 0x5000,
            mode: 0o140777,
            size: 0,
            modified: 0,
        }
    } else {
        match vfs::descriptor_metadata(process_fd.handle) {
            Ok(metadata) => metadata,
            Err(failure) => return vfs_error(failure),
        }
    };
    write_linux_stat(address, metadata)
}

fn linux_newfstatat(directory: u64, path_address: u64, address: u64, flags: u64) -> u64 {
    if flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH) != 0 {
        return error(22);
    }
    let mut input = [0u8; MAX_PATH];
    let Some(length) = user::copy_string(path_address, &mut input) else {
        return error(14);
    };
    let mut relative = false;
    let metadata = if length == 0 && flags & AT_EMPTY_PATH != 0 {
        let Some(process_fd) = lookup_file_fd(directory) else {
            return error(9);
        };
        match vfs::descriptor_metadata(process_fd.handle) {
            Ok(metadata) => metadata,
            Err(failure) => return vfs_error(failure),
        }
    } else {
        let mut resolved = [0u8; MAX_PATH];
        let (length, was_relative) = match resolve_user_path(directory, path_address, &mut resolved)
        {
            Ok(value) => value,
            Err(failure) => return failure,
        };
        relative = was_relative;
        let Ok(path) = core::str::from_utf8(&resolved[..length]) else {
            return error(84);
        };
        let found = if flags & AT_SYMLINK_NOFOLLOW != 0 {
            vfs::symlink_metadata(path)
        } else {
            vfs::metadata(path)
        };
        match found {
            Ok(metadata) => metadata,
            Err(failure) => return vfs_error(failure),
        }
    };
    let result = write_linux_stat(address, metadata);
    if result == 0 && relative {
        RELATIVE_PATH_CALLS.fetch_add(1, Ordering::Relaxed);
    }
    result
}

fn write_linux_stat(address: u64, metadata: vfs::Metadata) -> u64 {
    let mut stat = [0u8; 144];
    stat[..8].copy_from_slice(&1u64.to_le_bytes());
    stat[8..16].copy_from_slice(&metadata.inode.to_le_bytes());
    stat[16..24].copy_from_slice(&1u64.to_le_bytes());
    stat[24..28].copy_from_slice(&metadata.mode.to_le_bytes());
    stat[48..56].copy_from_slice(&metadata.size.to_le_bytes());
    stat[56..64].copy_from_slice(&4096u64.to_le_bytes());
    stat[64..72].copy_from_slice(&metadata.size.div_ceil(512).to_le_bytes());
    // atime, mtime, ctime (seconds; the nanosecond fields stay zero).
    for at in [72usize, 88, 104] {
        stat[at..at + 8].copy_from_slice(&metadata.modified.to_le_bytes());
    }
    if !user::copy_to_user(address, &stat) {
        return error(14);
    }
    METADATA_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_lseek(descriptor: u64, offset: u64, whence: u64) -> u64 {
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return error(9);
    };
    if process_fd.standard != 0
        || process_fd.socket
        || process_fd.pipe
        || process_fd.unix_socket
        || process_fd.unix_listener
        || process_fd.tcp
    {
        return error(29);
    }
    match vfs::seek(process_fd.handle, offset as i64, whence) {
        Ok(position) => {
            SEEK_CALLS.fetch_add(1, Ordering::Relaxed);
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            position as u64
        }
        Err(failure) => vfs_error(failure),
    }
}

fn linux_readlinkat(directory: u64, path_address: u64, address: u64, capacity: u64) -> u64 {
    let Ok(capacity) = usize::try_from(capacity) else {
        return error(22);
    };
    if capacity == 0 {
        return error(22);
    }
    let mut path = [0u8; MAX_PATH];
    let Some(length) = user::copy_string(path_address, &mut path) else {
        return error(14);
    };
    let Ok(path) = core::str::from_utf8(&path[..length]) else {
        return error(84);
    };
    let mut target = [0u8; MAX_PATH];
    let count = if directory as i64 == AT_FDCWD && path == "/proc/self/exe" {
        let exe = b"/bin/init";
        target[..exe.len()].copy_from_slice(exe);
        exe.len()
    } else {
        let mut resolved = [0u8; MAX_PATH];
        let (resolved_length, _) = match resolve_user_path(directory, path_address, &mut resolved) {
            Ok(value) => value,
            Err(failure) => return failure,
        };
        let Ok(resolved) = core::str::from_utf8(&resolved[..resolved_length]) else {
            return error(84);
        };
        match vfs::readlink(resolved, &mut target) {
            Ok(count) => count,
            Err(failure) => return vfs_error(failure),
        }
    };
    let count = count.min(capacity);
    if !user::copy_to_user(address, &target[..count]) {
        return error(14);
    }
    PATH_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    count as u64
}

fn linux_symlinkat(target_address: u64, directory: u64, link_address: u64) -> u64 {
    let mut target = [0u8; MAX_PATH];
    let Some(target_length) = user::copy_string(target_address, &mut target) else {
        return error(14);
    };
    let Ok(target) = core::str::from_utf8(&target[..target_length]) else {
        return error(84);
    };
    if target.is_empty() {
        return error(2);
    }
    let mut resolved = [0u8; MAX_PATH];
    let (length, relative) = match resolve_user_path(directory, link_address, &mut resolved) {
        Ok(value) => value,
        Err(failure) => return failure,
    };
    let Ok(link) = core::str::from_utf8(&resolved[..length]) else {
        return error(84);
    };
    match vfs::symlink(link, target) {
        Ok(()) => {
            CREATE_CALLS.fetch_add(1, Ordering::Relaxed);
            PATH_CALLS.fetch_add(1, Ordering::Relaxed);
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            if relative {
                RELATIVE_PATH_CALLS.fetch_add(1, Ordering::Relaxed);
            }
            0
        }
        Err(failure) => vfs_error(failure),
    }
}

fn linux_getrusage(who: u64, address: u64) -> u64 {
    if !(-1..=1).contains(&(who as i64)) {
        return error(22);
    }
    if !user::copy_to_user(address, &[0u8; 144]) {
        return error(14);
    }
    RESOURCE_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_times(address: u64) -> u64 {
    let ticks = crate::time::monotonic_nanoseconds() / (1_000_000_000 / CLOCK_TICKS_PER_SECOND);
    if address != 0 && !user::copy_to_user(address, &[0u8; 32]) {
        return error(14);
    }
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    ticks
}

fn linux_sched_getparam(address: u64) -> u64 {
    if !user::copy_to_user(address, &0u32.to_le_bytes()) {
        return error(14);
    }
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_prctl(option: u64, argument: u64) -> u64 {
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    match option {
        PR_SET_NAME | PR_SET_DUMPABLE | PR_SET_NO_NEW_PRIVS => 0,
        PR_GET_DUMPABLE => 1,
        PR_GET_NO_NEW_PRIVS => 0,
        PR_GET_NAME => {
            let mut name = [0u8; 16];
            name[..5].copy_from_slice(b"aeros");
            if user::copy_to_user(argument, &name) {
                0
            } else {
                error(14)
            }
        }
        _ => error(22),
    }
}

fn linux_ppoll(address: u64, count: u64, timeout_address: u64) -> u64 {
    let timeout_ms = if timeout_address == 0 {
        u64::from(u32::MAX)
    } else {
        let Some(nanoseconds) = read_timespec(timeout_address) else {
            return error(22);
        };
        nanoseconds.div_ceil(1_000_000).min(60_000)
    };
    linux_poll(address, count, timeout_ms)
}

fn linux_getgroups(size: u64) -> u64 {
    if (size as i64) < 0 {
        return error(22);
    }
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_getres_id(arguments: [u64; 6]) -> u64 {
    let zero = 0u32.to_le_bytes();
    if !arguments[..3]
        .iter()
        .all(|address| user::copy_to_user(*address, &zero))
    {
        return error(14);
    }
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn write_uts_field(buffer: &mut [u8; 390], field: usize, value: &[u8]) {
    let offset = field * 65;
    let count = value.len().min(64);
    buffer[offset..offset + count].copy_from_slice(&value[..count]);
}

fn linux_getrandom(address: u64, requested: u64, flags: u64) -> u64 {
    if flags & !1 != 0 {
        return error(22);
    }
    let Ok(requested) = usize::try_from(requested) else {
        return error(22);
    };
    if requested > MAX_IO || !user::range_accessible(address, requested, true) {
        return error(14);
    }
    let mut total = 0usize;
    let mut chunk = [0u8; IO_CHUNK];
    while total < requested {
        let amount = (requested - total).min(chunk.len());
        if !crate::random::fill(&mut chunk[..amount])
            || !user::copy_to_user(address + total as u64, &chunk[..amount])
        {
            chunk.fill(0);
            return if total == 0 { error(5) } else { total as u64 };
        }
        chunk[..amount].fill(0);
        total += amount;
    }
    RANDOM_CALLS.fetch_add(1, Ordering::Relaxed);
    RANDOM_BYTES.fetch_add(total as u64, Ordering::Relaxed);
    total as u64
}

/// Nanoseconds on one of the clocks `clock_gettime` knows: the wall clock
/// (0, 5), or time since boot for the monotonic, raw, coarse, boot-time and
/// CPU-time clocks (the last two report elapsed time, not CPU time).
fn clock_nanoseconds(clock: u64) -> Result<u128, u64> {
    match clock {
        0 | 5 => match crate::rtc::unix_nanoseconds() {
            0 => Err(error(5)),
            wall => Ok(wall),
        },
        1..=4 | 6 | 7 => Ok(u128::from(crate::time::monotonic_nanoseconds())),
        _ => Err(error(22)),
    }
}

const REBOOT_MAGIC: u64 = 0xfee1_dead;
const REBOOT_MAGIC2: [u64; 4] = [672_274_793, 85_072_278, 369_367_448, 537_993_216];

/// `reboot(magic, magic2, cmd)`: restart, halt or power off the machine.
fn linux_reboot(magic: u64, magic2: u64, command: u64) -> u64 {
    if !has_capability(crate::capability::CAP_SYS_BOOT) {
        return error(1);
    }
    if magic & 0xffff_ffff != REBOOT_MAGIC || !REBOOT_MAGIC2.contains(&(magic2 & 0xffff_ffff)) {
        return error(22);
    }
    match command & 0xffff_ffff {
        0x0123_4567 | 0xa1b2_c3d4 => crate::arch::reboot(),
        0x4321_fedc | 0xcdef_0123 => crate::power::shutdown(&crate::power::current()),
        0x89ab_cdef | 0 => 0,
        _ => error(22),
    }
}

fn set_wall_clock(seconds: i64, nanoseconds: i64) -> u64 {
    if seconds < 0 || !(0..1_000_000_000).contains(&nanoseconds) {
        return error(22);
    }
    crate::rtc::set_unix_seconds(seconds as u64);
    0
}

fn read_clock_value(address: u64, scale: i64) -> Option<(i64, i64)> {
    let mut raw = [0u8; 16];
    if !user::copy_from_user(address, &mut raw) {
        return None;
    }
    let seconds = i64::from_le_bytes(raw[..8].try_into().ok()?);
    let fraction = i64::from_le_bytes(raw[8..].try_into().ok()?);
    Some((seconds, fraction.checked_mul(scale)?))
}

fn linux_clock_settime(clock: u64, address: u64) -> u64 {
    if clock != 0 {
        return error(22);
    }
    if !has_capability(crate::capability::CAP_SYS_TIME) {
        return error(1);
    }
    match read_clock_value(address, 1) {
        Some((seconds, nanoseconds)) => set_wall_clock(seconds, nanoseconds),
        None => error(14),
    }
}

fn linux_settimeofday(time_address: u64, zone_address: u64) -> u64 {
    if (time_address != 0 || zone_address != 0) && !has_capability(crate::capability::CAP_SYS_TIME)
    {
        return error(1);
    }
    if time_address == 0 {
        return 0;
    }
    match read_clock_value(time_address, 1000) {
        Some((seconds, nanoseconds)) => set_wall_clock(seconds, nanoseconds),
        None => error(14),
    }
}

fn linux_clock_gettime(clock: u64, address: u64) -> u64 {
    let nanoseconds = match clock_nanoseconds(clock) {
        Ok(value) => value,
        Err(failure) => return failure,
    };
    let seconds = (nanoseconds / 1_000_000_000) as u64;
    let remainder = (nanoseconds % 1_000_000_000) as u64;
    let mut timespec = [0u8; 16];
    timespec[..8].copy_from_slice(&seconds.to_le_bytes());
    timespec[8..].copy_from_slice(&remainder.to_le_bytes());
    if !user::copy_to_user(address, &timespec) {
        return error(14);
    }
    CLOCK_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_clock_getres(clock: u64, address: u64) -> u64 {
    if clock > 7 {
        return error(22);
    }
    if address != 0 {
        let mut timespec = [0u8; 16];
        timespec[8..12].copy_from_slice(&1u32.to_le_bytes());
        if !user::copy_to_user(address, &timespec) {
            return error(14);
        }
    }
    CLOCK_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_time(address: u64) -> u64 {
    let seconds = crate::rtc::unix_seconds();
    if seconds == 0 {
        return error(5);
    }
    if address != 0 && !user::copy_to_user(address, &seconds.to_le_bytes()) {
        return error(14);
    }
    WALL_CLOCK_CALLS.fetch_add(1, Ordering::Relaxed);
    seconds
}

fn linux_gettimeofday(time_address: u64, zone_address: u64) -> u64 {
    let nanoseconds = crate::rtc::unix_nanoseconds();
    if nanoseconds == 0 {
        return error(5);
    }
    if time_address != 0 {
        let mut value = [0u8; 16];
        value[..8].copy_from_slice(&((nanoseconds / 1_000_000_000) as u64).to_le_bytes());
        value[8..].copy_from_slice(&(((nanoseconds % 1_000_000_000) / 1000) as u64).to_le_bytes());
        if !user::copy_to_user(time_address, &value) {
            return error(14);
        }
    }
    if zone_address != 0 && !user::copy_to_user(zone_address, &[0; 8]) {
        return error(14);
    }
    WALL_CLOCK_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

/// Nanoseconds until the interval timer next fires (0 = disarmed).
fn itimer_remaining() -> u64 {
    let deadline = ITIMER_DEADLINE.load(Ordering::Acquire);
    if deadline == 0 {
        return 0;
    }
    // A timer that is due but not yet expired still reports a sliver left.
    deadline
        .saturating_sub(crate::time::monotonic_nanoseconds())
        .max(1)
}

fn arm_itimer(value: u64, interval: u64) {
    let deadline = if value == 0 {
        0
    } else {
        crate::time::monotonic_nanoseconds().saturating_add(value)
    };
    ITIMER_INTERVAL.store(if value == 0 { 0 } else { interval }, Ordering::Release);
    ITIMER_DEADLINE.store(deadline, Ordering::Release);
}

/// alarm(seconds): arms a one-shot SIGALRM, returns the seconds (rounded up)
/// the previous alarm had left.
fn linux_alarm(seconds: u64) -> u64 {
    let previous = itimer_remaining().div_ceil(1_000_000_000);
    arm_itimer(seconds.saturating_mul(1_000_000_000), 0);
    SLEEP_CALLS.fetch_add(1, Ordering::Relaxed);
    previous
}

fn timeval_ns(bytes: &[u8; 32], offset: usize) -> u64 {
    read_array_u64(bytes, offset)
        .saturating_mul(1_000_000_000)
        .saturating_add(read_array_u64(bytes, offset + 8).saturating_mul(1000))
}

fn write_timeval(bytes: &mut [u8; 32], offset: usize, nanoseconds: u64) {
    bytes[offset..offset + 8].copy_from_slice(&(nanoseconds / 1_000_000_000).to_le_bytes());
    bytes[offset + 8..offset + 16]
        .copy_from_slice(&((nanoseconds % 1_000_000_000) / 1000).to_le_bytes());
}

fn current_itimerval() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    write_timeval(&mut bytes, 0, ITIMER_INTERVAL.load(Ordering::Acquire));
    write_timeval(&mut bytes, 16, itimer_remaining());
    bytes
}

fn linux_getitimer(which: u64, current: u64) -> u64 {
    if which != 0 {
        return error(22);
    }
    if !user::copy_to_user(current, &current_itimerval()) {
        return error(14);
    }
    0
}

fn linux_setitimer(which: u64, new_value: u64, old_value: u64) -> u64 {
    if which != 0 {
        return error(22);
    }
    let mut request = [0u8; 32];
    if new_value != 0 && !user::copy_from_user(new_value, &mut request) {
        return error(14);
    }
    if old_value != 0 && !user::copy_to_user(old_value, &current_itimerval()) {
        return error(14);
    }
    if new_value != 0 {
        arm_itimer(timeval_ns(&request, 16), timeval_ns(&request, 0));
    }
    SLEEP_CALLS.fetch_add(1, Ordering::Relaxed);
    0
}

/// pause(): waits (yielding) until a signal is ready for delivery, then fails
/// with EINTR so the handler runs at syscall return.
fn linux_pause() -> u64 {
    loop {
        if check_itimer().is_some()
            || SIGNAL_PENDING.load(Ordering::Acquire) & !SIGNAL_MASK.load(Ordering::Acquire) != 0
        {
            return error(4);
        }
        yield_in_syscall();
    }
}

fn linux_nanosleep(request: u64, remaining: u64) -> u64 {
    let Some(duration) = read_timespec(request) else {
        return error(22);
    };
    if duration > 60_000_000_000 {
        return error(22);
    }
    let deadline = crate::time::monotonic_nanoseconds().saturating_add(duration);
    let left = sleep_until(deadline);
    if remaining != 0 && !write_timespec(remaining, left.unwrap_or(0)) {
        return error(14);
    }
    SLEEP_CALLS.fetch_add(1, Ordering::Relaxed);
    if left.is_some() { error(4) } else { 0 }
}

fn linux_clock_nanosleep(clock: u64, flags: u64, request: u64, remaining: u64) -> u64 {
    if !matches!(clock, 0 | 1 | 4 | 7) || flags > 1 {
        return error(22);
    }
    let Some(requested) = read_timespec(request) else {
        return error(22);
    };
    let duration = if flags == 0 {
        requested
    } else if clock == 0 {
        requested.saturating_sub(crate::rtc::unix_nanoseconds().min(u64::MAX as u128) as u64)
    } else {
        requested.saturating_sub(crate::time::monotonic_nanoseconds())
    };
    if duration > 60_000_000_000 {
        return error(22);
    }
    let deadline = crate::time::monotonic_nanoseconds().saturating_add(duration);
    let left = sleep_until(deadline);
    if remaining != 0 && flags == 0 && !write_timespec(remaining, left.unwrap_or(0)) {
        return error(14);
    }
    SLEEP_CALLS.fetch_add(1, Ordering::Relaxed);
    if left.is_some() { error(4) } else { 0 }
}

fn read_timespec(address: u64) -> Option<u64> {
    let mut encoded = [0u8; 16];
    if !user::copy_from_user(address, &mut encoded) {
        return None;
    }
    let seconds = read_array_u64(&encoded, 0);
    let nanoseconds = read_array_u64(&encoded, 8);
    if nanoseconds >= 1_000_000_000 {
        return None;
    }
    seconds.checked_mul(1_000_000_000)?.checked_add(nanoseconds)
}

/// Sleeps until `deadline`, yielding the CPU, but wakes early when a signal
/// is ready for delivery (its handler then runs at syscall return). Returns
/// the nanoseconds left when interrupted.
fn sleep_until(deadline: u64) -> Option<u64> {
    loop {
        let now = crate::time::monotonic_nanoseconds();
        if now >= deadline {
            return None;
        }
        if check_itimer().is_some()
            || SIGNAL_PENDING.load(Ordering::Acquire) & !SIGNAL_MASK.load(Ordering::Acquire) != 0
        {
            return Some(deadline - now);
        }
        yield_in_syscall();
    }
}

fn write_timespec(address: u64, nanoseconds: u64) -> bool {
    let mut encoded = [0u8; 16];
    encoded[..8].copy_from_slice(&(nanoseconds / 1_000_000_000).to_le_bytes());
    encoded[8..].copy_from_slice(&(nanoseconds % 1_000_000_000).to_le_bytes());
    user::copy_to_user(address, &encoded)
}

fn linux_openat(directory: u64, path_address: u64, flags: u64, mode: u64) -> u64 {
    let access = flags & 3;
    if access == 3
        || flags
            & !(3 | O_CLOEXEC | O_DIRECTORY | O_CREAT | O_EXCL | O_TRUNC | O_APPEND | O_LARGEFILE)
            != 0
        || mode & !0o777 != 0
        || flags & O_TRUNC != 0 && access == 0
    {
        return error(22);
    }
    let mut resolved = [0u8; MAX_PATH];
    let (length, relative) = match resolve_user_path(directory, path_address, &mut resolved) {
        Ok(value) => value,
        Err(failure) => return failure,
    };
    let Ok(path) = core::str::from_utf8(&resolved[..length]) else {
        return error(84);
    };
    let readable = access != 1;
    let writable = access != 0;
    let opened = if flags & O_DIRECTORY != 0 {
        if flags & (O_CREAT | O_TRUNC) != 0 || writable {
            return error(21);
        }
        vfs::open_directory(path)
    } else {
        vfs::open_file(
            path,
            flags & O_CREAT != 0,
            flags & O_EXCL != 0,
            flags & O_TRUNC != 0,
            apply_umask(mode as u16),
            writable,
        )
    };
    match opened {
        Ok(handle) => {
            let Some(descriptor) = install_process_fd(
                handle,
                flags & O_CLOEXEC != 0,
                false,
                readable,
                writable,
                flags & O_APPEND != 0,
            ) else {
                let _ = vfs::close(handle);
                return error(24);
            };
            OPENS.fetch_add(1, Ordering::Relaxed);
            LAST_OPEN_FD.store(descriptor, Ordering::Release);
            if relative {
                RELATIVE_PATH_CALLS.fetch_add(1, Ordering::Relaxed);
            }
            descriptor
        }
        Err(failure) => vfs_error(failure),
    }
}

fn linux_getdents64(descriptor: u64, address: u64, capacity: u64) -> u64 {
    let Some(process_fd) = lookup_file_fd(descriptor) else {
        return error(9);
    };
    let Ok(capacity) = usize::try_from(capacity) else {
        return error(22);
    };
    if capacity < 24 || !user::range_accessible(address, capacity, true) {
        return error(14);
    }
    let entry = match vfs::next_directory_entry(process_fd.handle) {
        Ok(Some(entry)) => entry,
        Ok(None) => return 0,
        Err(failure) => return vfs_error(failure),
    };
    let record_length = (19usize + entry.name_len as usize + 1).next_multiple_of(8);
    if record_length > capacity || record_length > 288 {
        return error(22);
    }
    let mut record = [0u8; 288];
    record[..8].copy_from_slice(&entry.inode.to_le_bytes());
    record[8..16].copy_from_slice(&1i64.to_le_bytes());
    record[16..18].copy_from_slice(&(record_length as u16).to_le_bytes());
    record[18] = entry.kind;
    record[19..19 + entry.name_len as usize]
        .copy_from_slice(&entry.name[..entry.name_len as usize]);
    if !user::copy_to_user(address, &record[..record_length]) {
        return error(14);
    }
    DIRECTORY_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    record_length as u64
}

fn linux_read(descriptor: u64, address: u64, requested: u64) -> u64 {
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return error(9);
    };
    if !process_fd.readable {
        return error(9);
    }
    if process_fd.standard == 1 {
        READS.fetch_add(1, Ordering::Relaxed);
        return 0;
    }
    if process_fd.standard != 0 || (process_fd.socket && !process_fd.unix_socket) {
        return error(9);
    }
    let Ok(requested) = usize::try_from(requested) else {
        return error(22);
    };
    if requested > MAX_IO || !user::range_accessible(address, requested, true) {
        return error(14);
    }
    let mut total = 0usize;
    let mut chunk = [0u8; IO_CHUNK];
    while total < requested {
        let amount = (requested - total).min(chunk.len());
        let read = if process_fd.pipe {
            match pipe_read(process_fd.handle as usize, &mut chunk[..amount]) {
                Ok(read) => read,
                Err(failure) => {
                    return if total == 0 {
                        error(failure)
                    } else {
                        total as u64
                    };
                }
            }
        } else if process_fd.tcp {
            match tcpsock::read_bytes(process_fd, &mut chunk[..amount], total == 0) {
                Ok(read) => read,
                Err(failure) => {
                    return if total == 0 {
                        error(failure)
                    } else {
                        total as u64
                    };
                }
            }
        } else if process_fd.unix_socket {
            match unix_socket_read(
                process_fd.handle as usize,
                process_fd.unix_end_b,
                &mut chunk[..amount],
            ) {
                Ok(read) => read,
                Err(failure) => {
                    return if total == 0 {
                        error(failure)
                    } else {
                        total as u64
                    };
                }
            }
        } else {
            match vfs::read(process_fd.handle, &mut chunk[..amount]) {
                Ok(read) => read,
                Err(failure) => {
                    return if total == 0 {
                        vfs_error(failure)
                    } else {
                        total as u64
                    };
                }
            }
        };
        if read == 0 {
            break;
        }
        if !user::copy_to_user(address + total as u64, &chunk[..read]) {
            return if total == 0 { error(14) } else { total as u64 };
        }
        total += read;
        if read < amount {
            break;
        }
    }
    READS.fetch_add(1, Ordering::Relaxed);
    IO_BYTES.fetch_add(total as u64, Ordering::Relaxed);
    total as u64
}

fn linux_write(descriptor: u64, address: u64, requested: u64) -> u64 {
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return error(9);
    };
    if !process_fd.writable
        || (process_fd.socket && !process_fd.unix_socket)
        || process_fd.standard == 1
    {
        return error(9);
    }
    let Ok(requested) = usize::try_from(requested) else {
        return error(22);
    };
    if requested > MAX_IO || !user::range_accessible(address, requested, false) {
        return error(14);
    }
    let mut total = 0usize;
    let mut chunk = [0u8; IO_CHUNK];
    while total < requested {
        let amount = (requested - total).min(chunk.len());
        if !user::copy_from_user(address + total as u64, &mut chunk[..amount]) {
            return if total == 0 { error(14) } else { total as u64 };
        }
        if process_fd.standard != 0 {
            for byte in &chunk[..amount] {
                crate::serial::byte(*byte);
            }
        } else if process_fd.pipe {
            match pipe_write(process_fd.handle as usize, &chunk[..amount]) {
                Ok(written) => {
                    total += written;
                    if written < amount {
                        break;
                    }
                    continue;
                }
                Err(failure) => {
                    return if total == 0 {
                        error(failure)
                    } else {
                        total as u64
                    };
                }
            }
        } else if process_fd.tcp {
            match tcpsock::write_bytes(process_fd, &chunk[..amount]) {
                Ok(written) => {
                    total += written;
                    if written < amount {
                        break;
                    }
                    continue;
                }
                Err(failure) => {
                    return if total == 0 {
                        error(failure)
                    } else {
                        total as u64
                    };
                }
            }
        } else if process_fd.unix_socket {
            match unix_socket_write(
                process_fd.handle as usize,
                process_fd.unix_end_b,
                &chunk[..amount],
            ) {
                Ok(written) => {
                    total += written;
                    if written < amount {
                        break;
                    }
                    continue;
                }
                Err(failure) => {
                    return if total == 0 {
                        error(failure)
                    } else {
                        total as u64
                    };
                }
            }
        } else {
            match vfs::write(process_fd.handle, &chunk[..amount], process_fd.append) {
                Ok(written) => {
                    total += written;
                    if written < amount {
                        break;
                    }
                    continue;
                }
                Err(failure) => {
                    return if total == 0 {
                        vfs_error(failure)
                    } else {
                        total as u64
                    };
                }
            }
        }
        total += amount;
    }
    WRITES.fetch_add(1, Ordering::Relaxed);
    IO_BYTES.fetch_add(total as u64, Ordering::Relaxed);
    total as u64
}

fn linux_close(descriptor: u64) -> u64 {
    let Some((process_fd, last_reference)) = remove_process_fd(descriptor) else {
        return error(9);
    };
    if last_reference {
        let result = release_process_fd(process_fd);
        if result != 0 {
            return result;
        }
    }
    CLOSES.fetch_add(1, Ordering::Relaxed);
    0
}

fn linux_dup(descriptor: u64) -> u64 {
    let result = duplicate_process_fd(descriptor, 0, false);
    if result as i64 >= 0 {
        DUP_CALLS.fetch_add(1, Ordering::Relaxed);
        COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    }
    result
}

fn linux_dup2(descriptor: u64, target: u64) -> u64 {
    if lookup_process_fd(descriptor).is_none() {
        return error(9);
    }
    let result = if descriptor == target {
        target
    } else {
        duplicate_process_fd_exact(descriptor, target, false)
    };
    if result as i64 >= 0 {
        DUP_CALLS.fetch_add(1, Ordering::Relaxed);
        COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    }
    result
}

fn linux_dup3(descriptor: u64, target: u64, flags: u64) -> u64 {
    if descriptor == target || flags & !O_CLOEXEC != 0 {
        return error(22);
    }
    let result = duplicate_process_fd_exact(descriptor, target, flags & O_CLOEXEC != 0);
    if result as i64 >= 0 {
        DUP_CALLS.fetch_add(1, Ordering::Relaxed);
        COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    }
    result
}

fn linux_fcntl(descriptor: u64, command: u64, argument: u64) -> u64 {
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return error(9);
    };
    let result = match command {
        0 => duplicate_process_fd(descriptor, argument, false),
        1 => process_fd.close_on_exec as u64,
        2 => {
            if !set_close_on_exec(descriptor, argument & 1 != 0) {
                return error(9);
            }
            0
        }
        3 => {
            let access = if process_fd.readable && process_fd.writable {
                2
            } else if process_fd.writable {
                1
            } else {
                0
            };
            access
                | (u64::from(process_fd.append) * O_APPEND)
                | (u64::from(process_fd.nonblocking) * 0x800)
        }
        4 => {
            if argument & !(O_APPEND | 0x800) != 0 {
                return error(22);
            }
            set_status_flags(process_fd, argument & O_APPEND != 0, argument & 0x800 != 0);
            0
        }
        1030 => duplicate_process_fd(descriptor, argument, true),
        _ => return error(22),
    };
    if result as i64 >= 0 {
        if matches!(command, 0 | 1030) {
            DUP_CALLS.fetch_add(1, Ordering::Relaxed);
        }
        FD_CALLS.fetch_add(1, Ordering::Relaxed);
        COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    }
    result
}

fn install_process_fd(
    handle: u32,
    close_on_exec: bool,
    socket: bool,
    readable: bool,
    writable: bool,
    append: bool,
) -> Option<u64> {
    install_process_fd_kind(
        handle,
        close_on_exec,
        socket,
        false,
        false,
        false,
        false,
        false,
        readable,
        writable,
        append,
    )
}

fn install_epoll_fd(handle: u32, close_on_exec: bool) -> Option<u64> {
    install_process_fd_kind(
        handle,
        close_on_exec,
        false,
        false,
        true,
        false,
        false,
        false,
        false,
        false,
        false,
    )
}

fn install_unix_socket_fd(handle: u32, end_b: bool, close_on_exec: bool) -> Option<u64> {
    install_process_fd_kind(
        handle,
        close_on_exec,
        false,
        false,
        false,
        true,
        end_b,
        false,
        true,
        true,
        false,
    )
}

/// A fresh `socket(AF_UNIX, SOCK_STREAM, 0)`: not yet bound or connected.
fn install_unbound_unix_socket_fd(close_on_exec: bool) -> Option<u64> {
    install_process_fd_kind(
        UNIX_UNBOUND,
        close_on_exec,
        false,
        false,
        false,
        true,
        false,
        false,
        true,
        true,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn install_process_fd_kind(
    handle: u32,
    close_on_exec: bool,
    socket: bool,
    pipe: bool,
    epoll: bool,
    unix_socket: bool,
    unix_end_b: bool,
    unix_listener: bool,
    readable: bool,
    writable: bool,
    append: bool,
) -> Option<u64> {
    let mut descriptors = PROCESS_FDS.lock();
    let index = descriptors.iter().position(|descriptor| !descriptor.open)?;
    // RLIMIT_NOFILE: the returned fd number itself must stay below the
    // current limit, not just "some slot is free" - `position` always finds
    // the lowest free slot, so if that one is already at/past the limit,
    // every slot at or above it is too.
    let limit = NOFILE_LIMIT.load(Ordering::Acquire);
    if index as u64 >= limit {
        // Dropped before logging: `audit::record` can do real file I/O
        // (writing `/data/AUDIT.LOG`), which must never happen while still
        // holding this lock - every other fd operation in the kernel blocks
        // on it too.
        drop(descriptors);
        crate::audit::record(
            "RLIMIT_NOFILE",
            format_args!(
                "denied pid={} limit={limit}",
                crate::scheduler::current_task_id()
            ),
        );
        return None;
    }
    descriptors[index] = ProcessFd {
        handle,
        standard: 0,
        open: true,
        close_on_exec,
        socket,
        pipe,
        epoll,
        unix_socket,
        unix_end_b,
        unix_listener,
        readable,
        writable,
        append,
        memfd: false,
        tcp: false,
        nonblocking: false,
        inet6: false,
    };
    Some(index as u64)
}

fn process_fd_index(descriptor: u64) -> Option<usize> {
    let index = usize::try_from(descriptor).ok()?;
    (index < PROCESS_FD_COUNT).then_some(index)
}

fn lookup_process_fd(descriptor: u64) -> Option<ProcessFd> {
    let index = process_fd_index(descriptor)?;
    let process_fd = PROCESS_FDS.lock()[index];
    process_fd.open.then_some(process_fd)
}

fn lookup_file_fd(descriptor: u64) -> Option<ProcessFd> {
    lookup_process_fd(descriptor).filter(|process_fd| {
        !process_fd.socket
            && !process_fd.pipe
            && !process_fd.epoll
            && !process_fd.unix_socket
            && !process_fd.unix_listener
            && !process_fd.tcp
            && process_fd.standard == 0
    })
}

fn lookup_epoll_fd(descriptor: u64) -> Option<ProcessFd> {
    lookup_process_fd(descriptor).filter(|process_fd| process_fd.epoll)
}

fn lookup_socket_fd(descriptor: u64) -> Option<ProcessFd> {
    lookup_process_fd(descriptor).filter(|process_fd| process_fd.socket)
}

fn remove_process_fd(descriptor: u64) -> Option<(ProcessFd, bool)> {
    let index = process_fd_index(descriptor)?;
    let (process_fd, shared_in_this_process) = {
        let mut descriptors = PROCESS_FDS.lock();
        let process_fd = descriptors[index];
        if !process_fd.open {
            return None;
        }
        descriptors[index] = ProcessFd::EMPTY;
        let shared = descriptors
            .iter()
            .any(|candidate| same_open_description(*candidate, process_fd));
        (process_fd, shared)
    };
    // A fork()'d sibling's fd table carries the same underlying handle
    // number as its parent's, but lives in a DIFFERENT process's table
    // (only reachable while that process is the one running) - closing a
    // handle here must not tear it down while any other live task (this
    // process's own ancestors/descendants sharing the same open file
    // description, via fork) still has it open.
    let shared_elsewhere = crate::scheduler::any_other_task_shares_fd(|candidate| {
        same_open_description(*candidate, process_fd)
    });
    let last_reference = process_fd.standard == 0 && !shared_in_this_process && !shared_elsewhere;
    Some((process_fd, last_reference))
}

fn same_open_description(left: ProcessFd, right: ProcessFd) -> bool {
    left.open
        && right.open
        && left.standard == 0
        && right.standard == 0
        && left.socket == right.socket
        && left.pipe == right.pipe
        && left.epoll == right.epoll
        && left.unix_socket == right.unix_socket
        && left.unix_listener == right.unix_listener
        && left.tcp == right.tcp
        && left.handle == right.handle
        // A pipe's read end and write end share one `handle` (the pipe
        // slot) but are distinct open file descriptions; comparing
        // `readable` too keeps dup()s of the *same* end grouped together
        // without conflating the two ends of one pipe. Harmless for every
        // other fd kind, since dup() always copies these flags verbatim.
        && (!left.pipe || left.readable == right.readable)
        // Same idea for a `socketpair()` end: both ends are readable AND
        // writable, so `unix_end_b` (not `readable`) is what tells them
        // apart.
        //
        // Two independent, still-unbound `unix_socket` fds (handle ==
        // `UNIX_UNBOUND`) both compare equal here even though they aren't
        // dup()s of each other - harmless, since `release_process_fd`
        // already treats `UNIX_UNBOUND` as "nothing to release" regardless,
        // but worth knowing if this function ever grows a new caller that
        // assumes it means "definitely the same fd".
        && (!left.unix_socket || left.unix_end_b == right.unix_end_b)
}

fn release_process_fd(process_fd: ProcessFd) -> u64 {
    if process_fd.standard != 0 {
        return 0;
    }
    if process_fd.socket {
        let mut sockets = SOCKETS.lock();
        let Some(socket) = sockets.get_mut(process_fd.handle as usize) else {
            return error(9);
        };
        if !socket.open {
            return error(9);
        }
        crate::udp::release(socket.binding);
        *socket = UdpSocket::EMPTY;
        return 0;
    }
    if process_fd.pipe {
        let mut pipes = PIPES.lock();
        let Some(pipe) = pipes.get_mut(process_fd.handle as usize) else {
            return error(9);
        };
        if !pipe.used {
            return error(9);
        }
        if process_fd.readable {
            pipe.read_open = false;
        }
        if process_fd.writable {
            pipe.write_open = false;
        }
        if !pipe.read_open && !pipe.write_open {
            *pipe = Pipe::EMPTY;
        }
        return 0;
    }
    if process_fd.unix_socket {
        // Never connected or bound - nothing to release.
        if process_fd.handle == UNIX_UNBOUND {
            return 0;
        }
        let mut pairs = UNIX_PAIRS.lock();
        let Some(pair) = pairs.get_mut(process_fd.handle as usize) else {
            return error(9);
        };
        if !pair.used {
            return error(9);
        }
        if process_fd.unix_end_b {
            pair.end_b_open = false;
        } else {
            pair.end_a_open = false;
        }
        if !pair.end_a_open && !pair.end_b_open {
            *pair = UnixPair::EMPTY;
        }
        return 0;
    }
    if process_fd.unix_listener {
        let mut listeners = UNIX_LISTENERS.lock();
        let Some(listener) = listeners.get_mut(process_fd.handle as usize) else {
            return error(9);
        };
        if !listener.used {
            return error(9);
        }
        // Any connection still sitting in the backlog, never `accept()`ed,
        // is orphaned along with the listener - free its pair too.
        for pending in listener.backlog.iter().flatten() {
            UNIX_PAIRS.lock()[*pending] = UnixPair::EMPTY;
        }
        *listener = UnixListener::EMPTY;
        return 0;
    }
    if process_fd.tcp {
        tcpsock::release(&process_fd);
        return 0;
    }
    if process_fd.epoll {
        let mut instances = EPOLL_INSTANCES.lock();
        let Some(instance) = instances.get_mut(process_fd.handle as usize) else {
            return error(9);
        };
        if !instance.used {
            return error(9);
        }
        *instance = EpollInstance::EMPTY;
        return 0;
    }
    match close_file_handle(process_fd.handle, process_fd.memfd) {
        Ok(()) => 0,
        Err(failure) => vfs_error(failure),
    }
}

fn close_file_handle(handle: u32, memfd: bool) -> Result<(), VfsError> {
    let path = if memfd {
        vfs::descriptor_path(handle)
    } else {
        None
    };
    let result = vfs::close(handle);
    if let Some((buffer, length)) = path
        && let Ok(path) = core::str::from_utf8(&buffer[..length])
    {
        let _ = vfs::remove(path, false);
    }
    result
}

fn duplicate_process_fd(descriptor: u64, minimum: u64, close_on_exec: bool) -> u64 {
    let Ok(minimum) = usize::try_from(minimum) else {
        return error(22);
    };
    if minimum >= PROCESS_FD_COUNT {
        return error(22);
    }
    let mut descriptors = PROCESS_FDS.lock();
    let Some(source_index) = process_fd_index(descriptor) else {
        return error(9);
    };
    let source = descriptors[source_index];
    if !source.open {
        return error(9);
    }
    let Some(target) = descriptors[minimum..]
        .iter()
        .position(|candidate| !candidate.open)
        .map(|index| index + minimum)
    else {
        return error(24);
    };
    descriptors[target] = ProcessFd {
        close_on_exec,
        ..source
    };
    target as u64
}

fn duplicate_process_fd_exact(descriptor: u64, target: u64, close_on_exec: bool) -> u64 {
    let Some(source_index) = process_fd_index(descriptor) else {
        return error(9);
    };
    let Some(target_index) = process_fd_index(target) else {
        return error(9);
    };
    let replaced = {
        let mut descriptors = PROCESS_FDS.lock();
        let source = descriptors[source_index];
        if !source.open {
            return error(9);
        }
        let previous = descriptors[target_index];
        descriptors[target_index] = ProcessFd {
            close_on_exec,
            ..source
        };
        let last_reference = previous.standard == 0
            && !descriptors
                .iter()
                .any(|candidate| same_open_description(*candidate, previous));
        previous.open.then_some((previous, last_reference))
    };
    if let Some((previous, true)) = replaced {
        let result = release_process_fd(previous);
        if result != 0 {
            return result;
        }
    }
    target
}

fn set_status_flags(process_fd: ProcessFd, append: bool, nonblocking: bool) {
    if process_fd.tcp || (process_fd.socket && !process_fd.unix_socket) {
        let mut descriptors = PROCESS_FDS.lock();
        for descriptor in descriptors.iter_mut() {
            if same_open_description(*descriptor, process_fd) {
                descriptor.nonblocking = nonblocking;
            }
        }
        return;
    }
    if process_fd.standard != 0
        || process_fd.socket
        || process_fd.unix_socket
        || process_fd.unix_listener
    {
        return;
    }
    let mut descriptors = PROCESS_FDS.lock();
    for descriptor in descriptors.iter_mut() {
        if same_open_description(*descriptor, process_fd) {
            descriptor.append = append;
        }
    }
}

fn set_close_on_exec(descriptor: u64, enabled: bool) -> bool {
    let Some(index) = process_fd_index(descriptor) else {
        return false;
    };
    let mut descriptors = PROCESS_FDS.lock();
    if !descriptors[index].open {
        return false;
    }
    descriptors[index].close_on_exec = enabled;
    true
}

pub fn finish_process() {
    let clear_tid = TID_ADDRESS.swap(0, Ordering::AcqRel);
    if clear_tid != 0 {
        let _ = user::copy_to_user(clear_tid, &0u32.to_le_bytes());
    }
    ROBUST_LIST.store(0, Ordering::Release);
    RSEQ_ADDRESS.store(0, Ordering::Release);
    SIGNAL_MASK.store(0, Ordering::Release);
    *SIGNAL_ACTIONS.lock() = [SignalAction::EMPTY; 64];
    *ALTERNATE_STACK.lock() = AlternateStack::EMPTY;
    *CURRENT_DIRECTORY.lock() = CurrentDirectory::ROOT;
    let mut handles = [0u32; PROCESS_FD_COUNT];
    let mut memfds = [false; PROCESS_FD_COUNT];
    let mut count = 0usize;
    {
        let mut descriptors = PROCESS_FDS.lock();
        for descriptor in descriptors.iter_mut() {
            if descriptor.open
                && descriptor.standard == 0
                && !descriptor.socket
                && !descriptor.pipe
                && !descriptor.tcp
                && !descriptor.unix_socket
                && !descriptor.unix_listener
                && !descriptor.epoll
                && !handles[..count].contains(&descriptor.handle)
            {
                handles[count] = descriptor.handle;
                memfds[count] = descriptor.memfd;
                count += 1;
            }
        }
        *descriptors = initial_process_fds();
    }
    reset_udp_sockets();
    *PIPES.lock() = [Pipe::EMPTY; PIPE_COUNT];
    for (handle, memfd) in handles[..count].iter().zip(&memfds[..count]) {
        let _ = close_file_handle(*handle, *memfd);
    }
}

fn vfs_error(failure: VfsError) -> u64 {
    error(match failure {
        VfsError::InvalidPath | VfsError::PersistNameUnsupported => 22,
        VfsError::Traversal => 13,
        VfsError::NameTooLong | VfsError::DepthExceeded => 36,
        VfsError::NotFound => 2,
        VfsError::NotDirectory => 20,
        VfsError::IsDirectory => 21,
        VfsError::Exists => 17,
        VfsError::PermissionDenied => 13,
        VfsError::NodeLimit => 28,
        VfsError::HandleLimit => 24,
        VfsError::BadDescriptor => 9,
        VfsError::OffsetOverflow => 22,
        VfsError::FileTooLarge => 27,
        VfsError::NotEmpty => 39,
        VfsError::Busy => 16,
        VfsError::TooManyLinks => 40,
    })
}

fn error(errno: u64) -> u64 {
    0u64.wrapping_sub(errno)
}

pub fn stats() -> SyscallStats {
    SyscallStats {
        calls: CALLS.load(Ordering::Acquire),
        bootstrap_calls: BOOTSTRAP_CALLS.load(Ordering::Acquire),
        linux_calls: LINUX_CALLS.load(Ordering::Acquire),
        exits: EXITS.load(Ordering::Acquire),
        unknown: UNKNOWN.load(Ordering::Acquire),
        opens: OPENS.load(Ordering::Acquire),
        reads: READS.load(Ordering::Acquire),
        writes: WRITES.load(Ordering::Acquire),
        closes: CLOSES.load(Ordering::Acquire),
        io_bytes: IO_BYTES.load(Ordering::Acquire),
        clock_calls: CLOCK_CALLS.load(Ordering::Acquire),
        random_calls: RANDOM_CALLS.load(Ordering::Acquire),
        random_bytes: RANDOM_BYTES.load(Ordering::Acquire),
        compat_calls: COMPAT_CALLS.load(Ordering::Acquire),
        memory_calls: MEMORY_CALLS.load(Ordering::Acquire),
        mmaps: MMAPS.load(Ordering::Acquire),
        file_mmaps: FILE_MMAPS.load(Ordering::Acquire),
        mprotects: MPROTECTS.load(Ordering::Acquire),
        munmaps: MUNMAPS.load(Ordering::Acquire),
        metadata_calls: METADATA_CALLS.load(Ordering::Acquire),
        seek_calls: SEEK_CALLS.load(Ordering::Acquire),
        path_calls: PATH_CALLS.load(Ordering::Acquire),
        resource_calls: RESOURCE_CALLS.load(Ordering::Acquire),
        rseq_calls: RSEQ_CALLS.load(Ordering::Acquire),
        futex_calls: FUTEX_CALLS.load(Ordering::Acquire),
        fd_calls: FD_CALLS.load(Ordering::Acquire),
        dup_calls: DUP_CALLS.load(Ordering::Acquire),
        last_open_fd: LAST_OPEN_FD.load(Ordering::Acquire),
        signal_calls: SIGNAL_CALLS.load(Ordering::Acquire),
        runtime_calls: RUNTIME_CALLS.load(Ordering::Acquire),
        directory_calls: DIRECTORY_CALLS.load(Ordering::Acquire),
        socket_calls: SOCKET_CALLS.load(Ordering::Acquire),
        datagrams: DATAGRAMS.load(Ordering::Acquire),
        network_bytes: NETWORK_BYTES.load(Ordering::Acquire),
        vectored_calls: VECTORED_CALLS.load(Ordering::Acquire),
        positional_calls: POSITIONAL_CALLS.load(Ordering::Acquire),
        access_calls: ACCESS_CALLS.load(Ordering::Acquire),
        statx_calls: STATX_CALLS.load(Ordering::Acquire),
        wall_clock_calls: WALL_CLOCK_CALLS.load(Ordering::Acquire),
        sleep_calls: SLEEP_CALLS.load(Ordering::Acquire),
        chdir_calls: CHDIR_CALLS.load(Ordering::Acquire),
        relative_path_calls: RELATIVE_PATH_CALLS.load(Ordering::Acquire),
        poll_calls: POLL_CALLS.load(Ordering::Acquire),
        create_calls: CREATE_CALLS.load(Ordering::Acquire),
        rename_calls: RENAME_CALLS.load(Ordering::Acquire),
        remove_calls: REMOVE_CALLS.load(Ordering::Acquire),
        sync_calls: SYNC_CALLS.load(Ordering::Acquire),
        truncate_calls: TRUNCATE_CALLS.load(Ordering::Acquire),
        chmod_calls: CHMOD_CALLS.load(Ordering::Acquire),
    }
}
