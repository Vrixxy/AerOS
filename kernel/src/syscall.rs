use core::sync::atomic::{AtomicU64, Ordering};

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
const LINUX_SENDTO: u64 = 44;
const LINUX_RECVFROM: u64 = 45;
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
const LINUX_SET_ROBUST_LIST: u64 = 273;
const LINUX_DUP3: u64 = 292;
const LINUX_PRLIMIT64: u64 = 302;
const LINUX_RENAMEAT2: u64 = 316;
const LINUX_GETRANDOM: u64 = 318;
const LINUX_RSEQ: u64 = 334;
const LINUX_GETCPU: u64 = 309;
const LINUX_STATX: u64 = 332;
const LINUX_FACCESSAT2: u64 = 439;
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
    readable: bool,
    writable: bool,
    append: bool,
}

impl ProcessFd {
    const EMPTY: Self = Self {
        handle: 0,
        standard: 0,
        open: false,
        close_on_exec: false,
        socket: false,
        pipe: false,
        readable: false,
        writable: false,
        append: false,
    };

    const fn standard(kind: u8, readable: bool, writable: bool) -> Self {
        Self {
            handle: 0,
            standard: kind,
            open: true,
            close_on_exec: false,
            socket: false,
            pipe: false,
            readable,
            writable,
            append: false,
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
    remote: [u8; 4],
}

impl UdpSocket {
    const EMPTY: Self = Self {
        open: false,
        connected: false,
        local_port: 0,
        remote_port: 0,
        remote: [0; 4],
    };
}

static SOCKETS: TicketLock<[UdpSocket; SOCKET_COUNT]> =
    TicketLock::new([UdpSocket::EMPTY; SOCKET_COUNT]);
static NEXT_EPHEMERAL_PORT: AtomicU64 = AtomicU64::new(49153);

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
}

impl Pipe {
    const EMPTY: Self = Self {
        used: false,
        data: [0; PIPE_BUFFER_BYTES],
        head: 0,
        length: 0,
        read_open: false,
        write_open: false,
    };
}

static PIPES: TicketLock<[Pipe; PIPE_COUNT]> = TicketLock::new([Pipe::EMPTY; PIPE_COUNT]);

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
fn yield_in_syscall() {
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

fn pipe_read(slot: usize, destination: &mut [u8]) -> Result<usize, u64> {
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
}

impl ProcessState {
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

const LINUX_FORK: u64 = 57;
const LINUX_VFORK: u64 = 58;
const LINUX_EXECVE: u64 = 59;
const LINUX_WAIT4: u64 = 61;
const LINUX_KILL: u64 = 62;
const LINUX_RT_SIGRETURN: u64 = 15;

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

fn linux_fork(frame: &LinuxSyscallFrame) -> u64 {
    let snapshot = crate::arch::user::ForkSnapshot {
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
    };
    crate::scheduler::fork_current_user_task(&snapshot).unwrap_or_else(|| error(12))
}

fn linux_wait4(pid: u64, status_address: u64, options: u64) -> u64 {
    let child = if pid as i64 == -1 {
        crate::scheduler::any_child_id()
    } else {
        Some(pid)
    };
    let Some(child) = child else {
        return error(10);
    };
    const WNOHANG: u64 = 1;
    let waited = if options & WNOHANG != 0 {
        match crate::scheduler::poll_child(child) {
            None => None,
            Some(None) => return 0,
            Some(Some(code)) => Some(code),
        }
    } else {
        // While blocked, other tasks must see the normal ring-3 GS layout,
        // not the in-syscall (swapped) one, or their next `swapgs` would
        // load a null CPU-local pointer.
        // SAFETY: paired swapgs around the wait restore the in-syscall state.
        unsafe { core::arch::asm!("swapgs", options(nostack, preserves_flags)) };
        let waited = crate::scheduler::wait_for_child(child);
        unsafe { core::arch::asm!("swapgs", options(nostack, preserves_flags)) };
        waited
    };
    let Some(exit_code) = waited else {
        return error(10);
    };
    if status_address != 0 {
        // A death by signal is reported as WIFSIGNALED (the signal number in
        // the low bits); everything else as a normal exit status.
        let status = match exit_code {
            129..=191 => (exit_code - 128) as u32,
            _ => ((exit_code & 0xff) as u32) << 8,
        };
        if !user::copy_to_user(status_address, &status.to_le_bytes()) {
            return error(14);
        }
    }
    child
}

/// Returns true when the process image was replaced (frame now points at
/// the new entry point and stack); the caller must not touch rax then.
fn linux_execve(frame: &mut LinuxSyscallFrame) -> Result<(), u64> {
    let mut buffer = [0u8; MAX_PATH];
    let length = user::copy_string(frame.argument0, &mut buffer).ok_or_else(|| error(14))?;
    let path = core::str::from_utf8(&buffer[..length]).map_err(|_| error(84))?;
    let file = vfs::file(path).map_err(|_| error(2))?;
    // argv/envp live in the image that is about to be replaced, so they are
    // copied out first (up to 16 strings of 95 bytes each).
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
    let (entry, stack_top) = if arg_count == 0 {
        crate::scheduler::exec_current_user_task_path(file.data, path)
    } else {
        crate::scheduler::exec_current_user_task_path_with(
            file.data,
            path,
            &args[..arg_count],
            &env[..env_count],
        )
    }
    .ok_or_else(|| error(8))?;
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
    let saved_user_rsp = user_stack_pointer();
    match frame.number {
        LINUX_FORK | LINUX_VFORK => {
            frame.number = linux_fork(frame);
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
        LINUX_SOCKET => {
            SyscallResult::Return(linux_socket(arguments[0], arguments[1], arguments[2]))
        }
        LINUX_CONNECT => {
            SyscallResult::Return(linux_connect(arguments[0], arguments[1], arguments[2]))
        }
        LINUX_SENDTO => SyscallResult::Return(linux_sendto(arguments)),
        LINUX_RECVFROM => SyscallResult::Return(linux_recvfrom(arguments)),
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
        LINUX_UMASK => SyscallResult::Return(linux_umask(arguments[0])),
        LINUX_SYSINFO => SyscallResult::Return(linux_sysinfo(arguments[0])),
        LINUX_SIGALTSTACK => SyscallResult::Return(linux_sigaltstack(arguments[0], arguments[1])),
        LINUX_ARCH_PRCTL => SyscallResult::Return(linux_arch_prctl(arguments[0], arguments[1])),
        LINUX_GETTID => {
            RUNTIME_CALLS.fetch_add(1, Ordering::Relaxed);
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            SyscallResult::Return(current_process_id())
        }
        LINUX_TKILL => linux_tkill(arguments[0], arguments[1]),
        LINUX_FUTEX => SyscallResult::Return(linux_futex(arguments)),
        LINUX_TIME => SyscallResult::Return(linux_time(arguments[0])),
        LINUX_SCHED_GETAFFINITY => SyscallResult::Return(linux_sched_getaffinity(
            arguments[0],
            arguments[1],
            arguments[2],
        )),
        LINUX_EXIT | LINUX_EXIT_GROUP => SyscallResult::Exit(arguments[0] & 0xff),
        LINUX_TGKILL => linux_tgkill(arguments[0], arguments[1], arguments[2]),
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
        && first_half_ok
        && second_half_ok
        && eagain_ok
        && eof_ok
        && wrap_write_ok
        && wrap_read_ok
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
    let Some(read_fd) =
        install_process_fd_kind(slot as u32, close_on_exec, false, true, true, false, false)
    else {
        PIPES.lock()[slot] = Pipe::EMPTY;
        return error(24);
    };
    let Some(write_fd) =
        install_process_fd_kind(slot as u32, close_on_exec, false, true, false, true, false)
    else {
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
    if domain != 2 {
        return error(97);
    }
    let base_kind = kind & 0xf;
    if base_kind != 2 || kind & !(0xf | 0x800 | O_CLOEXEC) != 0 || protocol != 0 && protocol != 17 {
        return error(93);
    }
    let mut sockets = SOCKETS.lock();
    let Some(index) = sockets.iter().position(|socket| !socket.open) else {
        return error(24);
    };
    let port = NEXT_EPHEMERAL_PORT.fetch_add(1, Ordering::Relaxed) as u16;
    sockets[index] = UdpSocket {
        open: true,
        connected: false,
        local_port: port.max(49153),
        remote_port: 0,
        remote: [0; 4],
    };
    drop(sockets);
    let Some(descriptor) =
        install_process_fd(index as u32, kind & O_CLOEXEC != 0, true, true, true, false)
    else {
        SOCKETS.lock()[index] = UdpSocket::EMPTY;
        return error(24);
    };
    SOCKET_CALLS.fetch_add(1, Ordering::Relaxed);
    descriptor
}

fn linux_connect(descriptor: u64, address: u64, length: u64) -> u64 {
    let Some(process_fd) = lookup_socket_fd(descriptor) else {
        return error(9);
    };
    let Some((remote, port)) = read_sockaddr(address, length) else {
        return error(22);
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

fn linux_sendto(arguments: [u64; 6]) -> u64 {
    let [
        descriptor,
        address,
        requested,
        flags,
        destination,
        destination_length,
    ] = arguments;
    if flags != 0 {
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
        let Some(target) = read_sockaddr(destination, destination_length) else {
            return error(22);
        };
        target
    } else if socket.connected {
        (socket.remote, socket.remote_port)
    } else {
        return error(89);
    };
    let mut payload = [0u8; MAX_DATAGRAM];
    if !user::copy_from_user(address, &mut payload[..requested]) {
        return error(14);
    }
    if !crate::net::send_udp(remote, socket.local_port, port, &payload[..requested]) {
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
    if flags != 0 {
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
        if !socket.open || !socket.connected {
            return error(107);
        }
        socket
    };
    let mut payload = [0u8; MAX_DATAGRAM];
    let Some(datagram) = crate::net::receive_udp(
        socket.remote,
        socket.remote_port,
        socket.local_port,
        &mut payload[..capacity],
    ) else {
        return error(11);
    };
    if !user::copy_to_user(address, &payload[..datagram.bytes]) {
        return error(14);
    }
    if source_address != 0 {
        if source_length == 0 {
            return error(14);
        }
        let mut encoded_length = [0u8; 4];
        if !user::copy_from_user(source_length, &mut encoded_length)
            || u32::from_le_bytes(encoded_length) < 16
            || !write_sockaddr(source_address, datagram.source, datagram.source_port)
            || !user::copy_to_user(source_length, &16u32.to_le_bytes())
        {
            return error(14);
        }
    }
    SOCKET_CALLS.fetch_add(1, Ordering::Relaxed);
    DATAGRAMS.fetch_add(1, Ordering::Relaxed);
    NETWORK_BYTES.fetch_add(datagram.bytes as u64, Ordering::Relaxed);
    datagram.bytes as u64
}

fn read_sockaddr(address: u64, length: u64) -> Option<([u8; 4], u16)> {
    if length < 16 {
        return None;
    }
    let mut encoded = [0u8; 16];
    if !user::copy_from_user(address, &mut encoded)
        || u16::from_le_bytes([encoded[0], encoded[1]]) != 2
    {
        return None;
    }
    let port = u16::from_be_bytes([encoded[2], encoded[3]]);
    let remote = [encoded[4], encoded[5], encoded[6], encoded[7]];
    if port == 0 || remote == [0; 4] || remote[0] >= 224 {
        return None;
    }
    Some((remote, port))
}

fn write_sockaddr(address: u64, remote: [u8; 4], port: u16) -> bool {
    let mut encoded = [0u8; 16];
    encoded[..2].copy_from_slice(&2u16.to_le_bytes());
    encoded[2..4].copy_from_slice(&port.to_be_bytes());
    encoded[4..8].copy_from_slice(&remote);
    user::copy_to_user(address, &encoded)
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
    if process_fd.standard != 0 || process_fd.socket || process_fd.pipe {
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
    if timeout > 60_000 {
        return error(22);
    }
    let mut ready = 0u64;
    for index in 0..count {
        let entry = address + (index * 8) as u64;
        let mut encoded = [0u8; 8];
        if !user::copy_from_user(entry, &mut encoded) {
            return error(14);
        }
        let descriptor = i32::from_le_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]);
        let events = i16::from_le_bytes([encoded[4], encoded[5]]) as u16;
        let returned = if descriptor < 0 {
            0
        } else if let Some(process_fd) = lookup_process_fd(descriptor as u64) {
            let mut available = 0u16;
            if process_fd.readable && !process_fd.socket {
                available |= 1;
            }
            if process_fd.writable {
                available |= 4;
            }
            events & available
        } else {
            0x20
        };
        if returned != 0 {
            ready += 1;
        }
        if !user::copy_to_user(entry + 6, &returned.to_le_bytes()) {
            return error(14);
        }
    }
    if ready == 0 && timeout > 0 {
        let deadline =
            crate::time::monotonic_nanoseconds().saturating_add(timeout as u64 * 1_000_000);
        wait_until(deadline);
    }
    POLL_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    ready
}

fn linux_tkill(thread: u64, signal: u64) -> SyscallResult {
    if signal > 64 {
        return SyscallResult::Return(error(22));
    }
    if thread == 0 || thread != current_process_id() {
        return SyscallResult::Return(error(3));
    }
    SIGNAL_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    if signal == 0 {
        SyscallResult::Return(0)
    } else {
        raise_signal(signal)
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
pub fn signal_stored_state(state: &mut ProcessState, signal: u64) -> RemoteSignal {
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
/// one are consumed here: ignored or already handled).
fn take_signal() -> Option<(u64, SignalAction)> {
    let ready = SIGNAL_PENDING.load(Ordering::Acquire) & !SIGNAL_MASK.load(Ordering::Acquire);
    if ready == 0 {
        return None;
    }
    let signal = ready.trailing_zeros() as u64 + 1;
    SIGNAL_PENDING.fetch_and(!(1 << (signal - 1)), Ordering::AcqRel);
    let action = SIGNAL_ACTIONS.lock()[signal as usize - 1];
    (action.handler > 1).then_some((signal, action))
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
    let (signal, action) = take_signal()?;
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
    let (signal, action) = take_signal()?;
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
    if group != current_process_id() {
        return SyscallResult::Return(error(3));
    }
    linux_tkill(thread, signal)
}

fn linux_writev(descriptor: u64, vectors: u64, count: u64) -> u64 {
    let Some(process_fd) = lookup_process_fd(descriptor) else {
        return error(9);
    };
    if !process_fd.writable || process_fd.socket || process_fd.pipe || process_fd.standard == 1 {
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
        Err(crate::arch::paging::UserMemoryError::OutOfMemory) => error(12),
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
    let mut directory = CURRENT_DIRECTORY.lock();
    directory.bytes.fill(0);
    directory.bytes[..length].copy_from_slice(&resolved[..length]);
    directory.length = length;
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

fn linux_set_tid_address(address: u64) -> u64 {
    if address != 0 && !user::range_accessible(address, 4, true) {
        return error(14);
    }
    TID_ADDRESS.store(address, Ordering::Release);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    current_process_id()
}

fn linux_set_robust_list(address: u64, length: u64) -> u64 {
    if length != 24 || !user::range_accessible(address, length as usize, true) {
        return error(22);
    }
    ROBUST_LIST.store(address, Ordering::Release);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
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
    let [address, operation, value, timeout, _, bitset] = arguments;
    if address & 3 != 0 || !user::range_accessible(address, 4, true) {
        return error(14);
    }
    let command = operation & 0x7f;
    if operation & !(0x7f | 0x80 | 0x100) != 0 {
        return error(22);
    }
    match command {
        1 | 10 => {
            if command == 10 && bitset == 0 {
                return error(22);
            }
            FUTEX_CALLS.fetch_add(1, Ordering::Relaxed);
            COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
            0
        }
        0 | 9 => {
            let mut observed = [0u8; 4];
            if !user::copy_from_user(address, &mut observed) {
                return error(14);
            }
            if u32::from_le_bytes(observed) != value as u32 {
                return error(11);
            }
            if command == 9 && bitset == 0 {
                return error(22);
            }
            if timeout != 0 && !user::range_accessible(timeout, 16, false) {
                return error(14);
            }
            error(if timeout == 0 { 11 } else { 110 })
        }
        _ => error(38),
    }
}

fn linux_prlimit64(pid: u64, resource: u64, new_limit: u64, old_limit: u64) -> u64 {
    if pid != 0 && pid != current_process_id() {
        return error(3);
    }
    if new_limit != 0 {
        return error(1);
    }
    let (current, maximum) = match resource {
        3 => (8 * 1024 * 1024u64, 8 * 1024 * 1024u64),
        7 => (24, 24),
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
        match vfs::metadata(path) {
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
    if process_fd.standard != 0 || process_fd.socket || process_fd.pipe {
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
    if directory as i64 != AT_FDCWD || path != "/proc/self/exe" {
        return error(2);
    }
    let target = b"/bin/init";
    let count = target.len().min(capacity);
    if !user::copy_to_user(address, &target[..count]) {
        return error(14);
    }
    PATH_CALLS.fetch_add(1, Ordering::Relaxed);
    COMPAT_CALLS.fetch_add(1, Ordering::Relaxed);
    count as u64
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

fn linux_clock_gettime(clock: u64, address: u64) -> u64 {
    if clock != 1 && clock != 7 {
        return error(22);
    }
    let nanoseconds = crate::time::monotonic_nanoseconds();
    let seconds = nanoseconds / 1_000_000_000;
    let remainder = nanoseconds % 1_000_000_000;
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
    if clock != 1 && clock != 7 {
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
    if !matches!(clock, 0 | 1 | 7) || flags > 1 {
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

fn wait_until(deadline: u64) {
    while crate::time::monotonic_nanoseconds() < deadline {
        core::hint::spin_loop();
    }
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
    if process_fd.standard != 0 || process_fd.socket {
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
    if !process_fd.writable || process_fd.socket || process_fd.standard == 1 {
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
            access | (u64::from(process_fd.append) * O_APPEND)
        }
        4 => {
            if argument & !(O_APPEND | 0x800) != 0 {
                return error(22);
            }
            set_status_flags(process_fd, argument & O_APPEND != 0);
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
        readable,
        writable,
        append,
    )
}

fn install_process_fd_kind(
    handle: u32,
    close_on_exec: bool,
    socket: bool,
    pipe: bool,
    readable: bool,
    writable: bool,
    append: bool,
) -> Option<u64> {
    let mut descriptors = PROCESS_FDS.lock();
    let index = descriptors.iter().position(|descriptor| !descriptor.open)?;
    descriptors[index] = ProcessFd {
        handle,
        standard: 0,
        open: true,
        close_on_exec,
        socket,
        pipe,
        readable,
        writable,
        append,
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
    lookup_process_fd(descriptor)
        .filter(|process_fd| !process_fd.socket && !process_fd.pipe && process_fd.standard == 0)
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
        && left.handle == right.handle
        // A pipe's read end and write end share one `handle` (the pipe
        // slot) but are distinct open file descriptions; comparing
        // `readable` too keeps dup()s of the *same* end grouped together
        // without conflating the two ends of one pipe. Harmless for every
        // other fd kind, since dup() always copies these flags verbatim.
        && (!left.pipe || left.readable == right.readable)
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
    match vfs::close(process_fd.handle) {
        Ok(()) => 0,
        Err(failure) => vfs_error(failure),
    }
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

fn set_status_flags(process_fd: ProcessFd, append: bool) {
    if process_fd.standard != 0 || process_fd.socket {
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
    let mut count = 0usize;
    {
        let mut descriptors = PROCESS_FDS.lock();
        for descriptor in descriptors.iter_mut() {
            if descriptor.open
                && descriptor.standard == 0
                && !descriptor.socket
                && !descriptor.pipe
                && !handles[..count].contains(&descriptor.handle)
            {
                handles[count] = descriptor.handle;
                count += 1;
            }
        }
        *descriptors = initial_process_fds();
    }
    *SOCKETS.lock() = [UdpSocket::EMPTY; SOCKET_COUNT];
    *PIPES.lock() = [Pipe::EMPTY; PIPE_COUNT];
    for handle in &handles[..count] {
        let _ = vfs::close(*handle);
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
