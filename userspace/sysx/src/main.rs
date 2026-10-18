//! Exercises the newer Linux syscalls with real user pointers: mremap, splice,
//! tee, signalfd, rt_sigtimedwait/pending, mknod, getdents, openat2, syslog,
//! inotify, POSIX timers, System V queues, semaphores and shared memory, MAP_SHARED. Each step prints `AEROS_SYSX_STEP <name> ok|FAILED`; the first
//! failure exits with its own code, success exits with 89.

use std::arch::asm;

fn sys(number: usize, args: [usize; 6]) -> isize {
    let result: isize;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") number as isize => result,
            in("rdi") args[0],
            in("rsi") args[1],
            in("rdx") args[2],
            in("r10") args[3],
            in("r8") args[4],
            in("r9") args[5],
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack)
        );
    }
    result
}

fn step(name: &str, ok: bool, code: i32) {
    println!("AEROS_SYSX_STEP {name} {}", if ok { "ok" } else { "FAILED" });
    if !ok {
        std::process::exit(code);
    }
}

fn cstr(text: &str) -> Vec<u8> {
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    bytes
}

const MREMAP: usize = 25;
const MMAP: usize = 9;
const READ: usize = 0;
const OPEN: usize = 2;
const CLOSE: usize = 3;
const GETDENTS: usize = 78;
const KILL: usize = 62;
const GETPID: usize = 39;
const LINK: usize = 86;
const MKNOD: usize = 133;
const SIGPROCMASK: usize = 14;
const SIGPENDING: usize = 127;
const SIGTIMEDWAIT: usize = 128;
const SYSLOG: usize = 103;
const SPLICE: usize = 275;
const TEE: usize = 276;
const VMSPLICE: usize = 278;
const SIGNALFD4: usize = 289;
const PIPE2: usize = 293;
const INOTIFY_INIT1: usize = 294;
const INOTIFY_ADD_WATCH: usize = 254;
const GETRUSAGE: usize = 98;
const SETITIMER: usize = 38;
const TIMES: usize = 100;
const CLOCK_GETTIME: usize = 228;
const FORK: usize = 57;
const RT_SIGACTION: usize = 13;
const RT_SIGQUEUEINFO: usize = 129;
const WRITE: usize = 1;
const PROCESS_VM_READV: usize = 310;
const PROCESS_VM_WRITEV: usize = 311;
const PIDFD_OPEN: usize = 434;
const PIDFD_SEND_SIGNAL: usize = 424;
const EXECVEAT: usize = 322;
const WAIT4: usize = 61;
const EXIT_GROUP: usize = 231;
const SHMGET: usize = 29;
const SHMAT: usize = 30;
const SHMCTL: usize = 31;
const SHMDT: usize = 67;
const SEMGET: usize = 64;
const SEMOP: usize = 65;
const SEMCTL: usize = 66;
const MSGGET: usize = 68;
const MSGSND: usize = 69;
const MSGRCV: usize = 70;
const MSGCTL: usize = 71;
const TIMER_CREATE: usize = 222;
const TIMER_SETTIME: usize = 223;
const TIMER_GETTIME: usize = 224;
const TIMER_DELETE: usize = 226;
const OPENAT2: usize = 437;
const AT_FDCWD: isize = -100;

extern "C" fn segv_handler(signal: i32, info: *const u8, _context: *const u8) {
    let address = unsafe { core::ptr::read_unaligned(info.add(16) as *const u64) };
    let code = unsafe { core::ptr::read_unaligned(info.add(8) as *const i32) };
    let verdict = if signal != 11 {
        90
    } else if address != 0x5151_0000 {
        91
    } else if code != 1 && code != 2 {
        92
    } else {
        let marker = 0u8;
        let here = &marker as *const u8 as usize;
        let start = unsafe { core::ptr::addr_of!(ALT_STACK) } as usize;
        let mut current = [0u64; 4];
        sys(13, [11, 0, current.as_mut_ptr() as usize, 8, 0, 0]);
        if here < start || here >= start + 16384 {
            93
        } else if current[0] != 0 {
            94
        } else {
            77
        }
    };
    sys(231, [verdict, 0, 0, 0, 0, 0]);
}

static mut ALT_STACK: [u8; 16384] = [0; 16384];
static CHILD_SIGNALS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static CHILD_SIGNAL_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

extern "C" fn chld_handler(_signal: i32, info: *const u8, _context: *const u8) {
    let pid = unsafe { core::ptr::read_unaligned(info.add(16) as *const u32) };
    CHILD_SIGNAL_PID.store(pid, std::sync::atomic::Ordering::SeqCst);
    CHILD_SIGNALS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

#[unsafe(naked)]
unsafe extern "C" fn segv_restorer() {
    core::arch::naked_asm!("mov eax, 15", "syscall")
}

fn pattern(index: usize) -> u8 {
    (index % 251) as u8
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("exec-child") {
        let exe = std::fs::read_link("/proc/self/exe").unwrap_or_default();
        std::process::exit(if exe.to_string_lossy().ends_with("aeros-sysx") {
            88
        } else {
            85
        });
    }
    // mremap: grow by moving, content kept, new pages zero and writable;
    // shrink in place; growth without MAYMOVE refused.
    let page = 4096usize;
    let base = sys(MMAP, [0, page, 3, 0x22, usize::MAX, 0]);
    step("mmap", base > 0, 10);
    let base = base as usize;
    unsafe {
        let region = std::slice::from_raw_parts_mut(base as *mut u8, page);
        for (index, byte) in region.iter_mut().enumerate() {
            *byte = pattern(index);
        }
    }
    let grown = sys(MREMAP, [base, page, 3 * page, 1, 0, 0]);
    let moved_ok = grown > 0 && (grown as usize) & 0xfff == 0;
    let mut kept = false;
    let mut zeroed = false;
    if moved_ok {
        unsafe {
            let region = std::slice::from_raw_parts_mut(grown as usize as *mut u8, 3 * page);
            kept = region[..page]
                .iter()
                .enumerate()
                .all(|(index, byte)| *byte == pattern(index));
            zeroed = region[page..].iter().all(|byte| *byte == 0);
            region[3 * page - 1] = 0x5a;
        }
    }
    step("mremap_grow", moved_ok && kept && zeroed, 11);
    let grown = grown as usize;
    let shrunk = sys(MREMAP, [grown, 3 * page, page, 0, 0, 0]);
    step("mremap_shrink", shrunk == grown as isize, 12);
    let refused = sys(MREMAP, [grown, page, 2 * page, 0, 0, 0]);
    step("mremap_nomove", refused == -12, 13);
    let misaligned = sys(MREMAP, [grown + 1, page, page, 0, 0, 0]);
    step("mremap_args", misaligned == -22, 14);

    // splice: file -> pipe with an offset, tee to a second pipe, pipe -> file.
    let source_path = cstr("/tmp/sx-src");
    let target_path = cstr("/tmp/sx-dst");
    let data: Vec<u8> = (0..3000).map(pattern).collect();
    std::fs::write("/tmp/sx-src", &data).expect("write source");
    let source = sys(OPEN, [source_path.as_ptr() as usize, 0, 0, 0, 0, 0]);
    step("open_source", source >= 0, 20);
    let mut first = [0i32; 2];
    let mut second = [0i32; 2];
    step(
        "pipes",
        sys(PIPE2, [first.as_mut_ptr() as usize, 0, 0, 0, 0, 0]) == 0
            && sys(PIPE2, [second.as_mut_ptr() as usize, 0, 0, 0, 0, 0]) == 0,
        21,
    );
    let mut offset: i64 = 100;
    let moved = sys(
        SPLICE,
        [
            source as usize,
            &mut offset as *mut i64 as usize,
            first[1] as usize,
            0,
            1000,
            0,
        ],
    );
    step("splice_in", moved == 1000 && offset == 1100, 22);
    let teed = sys(TEE, [first[0] as usize, second[1] as usize, 500, 0, 0, 0]);
    step("tee", teed == 500, 23);
    let bad_pipe_offset = sys(
        SPLICE,
        [
            first[0] as usize,
            &mut offset as *mut i64 as usize,
            second[1] as usize,
            0,
            10,
            0,
        ],
    );
    step("splice_espipe", bad_pipe_offset == -29, 24);
    let target = sys(
        OPEN,
        [target_path.as_ptr() as usize, 0o1101, 0o644, 0, 0, 0],
    );
    step("open_target", target >= 0, 25);
    let drained = sys(
        SPLICE,
        [first[0] as usize, 0, target as usize, 0, 1000, 0],
    );
    step("splice_out", drained == 1000, 26);
    let link = std::fs::read_link(format!("/proc/self/fd/{source}")).unwrap_or_default();
    let listed = std::fs::read_dir("/proc/self/fd").map_or(0, |entries| entries.count());
    step(
        "proc_self_fd",
        link.to_string_lossy() == "/tmp/sx-src" && listed >= 4,
        74,
    );
    let _ = sys(CLOSE, [target as usize, 0, 0, 0, 0, 0]);
    let written = std::fs::read("/tmp/sx-dst").unwrap_or_default();
    step("splice_content", written == data[100..1100], 27);
    let mut echo = vec![0u8; 500];
    let echoed = sys(
        READ,
        [second[0] as usize, echo.as_mut_ptr() as usize, 500, 0, 0, 0],
    );
    step("tee_content", echoed == 500 && echo == data[100..600], 28);
    let mut spliced = [0u8; 64];
    let mut gathered = [0u8; 64];
    spliced.copy_from_slice(&data[..64]);
    let vectors = [
        (spliced.as_ptr() as usize, 40usize),
        (unsafe { spliced.as_ptr().add(40) } as usize, 24usize),
    ];
    let put = sys(VMSPLICE, [second[1] as usize, vectors.as_ptr() as usize, 2, 0, 0, 0]);
    let out = [(gathered.as_mut_ptr() as usize, 64usize)];
    let got = sys(VMSPLICE, [second[0] as usize, out.as_ptr() as usize, 1, 0, 0, 0]);
    step("vmsplice", put == 64 && got == 64 && gathered == spliced, 29);

    // signalfd with a blocked signal, then sigpending / sigtimedwait.
    let usr1 = 1u64 << 9;
    let usr2 = 1u64 << 11;
    let block = usr1 | usr2;
    let blocked = sys(
        SIGPROCMASK,
        [0, &block as *const u64 as usize, 0, 8, 0, 0],
    );
    step("sigprocmask", blocked == 0, 30);
    let pid = sys(GETPID, [0; 6]) as usize;
    let descriptor = sys(
        SIGNALFD4,
        [usize::MAX, &usr1 as *const u64 as usize, 8, 0, 0, 0],
    );
    step("signalfd", descriptor >= 0, 31);
    step("kill_self", sys(KILL, [pid, 10, 0, 0, 0, 0]) == 0, 32);
    let mut info = [0u8; 128];
    let got = sys(
        READ,
        [descriptor as usize, info.as_mut_ptr() as usize, 128, 0, 0, 0],
    );
    step(
        "signalfd_read",
        got == 128 && u32::from_le_bytes([info[0], info[1], info[2], info[3]]) == 10,
        33,
    );
    step("kill_again", sys(KILL, [pid, 12, 0, 0, 0, 0]) == 0, 34);
    let mut pending = 0u64;
    let pending_ok = sys(SIGPENDING, [&mut pending as *mut u64 as usize, 8, 0, 0, 0, 0]) == 0
        && pending & usr2 != 0;
    step("sigpending", pending_ok, 35);
    let waited = sys(
        SIGTIMEDWAIT,
        [&usr2 as *const u64 as usize, 0, 0, 8, 0, 0],
    );
    step("sigtimedwait", waited == 12, 36);
    let timeout = [0i64, 1_000_000];
    let none = sys(
        SIGTIMEDWAIT,
        [&usr2 as *const u64 as usize, 0, timeout.as_ptr() as usize, 8, 0, 0],
    );
    step("sigtimedwait_timeout", none == -11, 37);

    // POSIX timer: SIGUSR2 (blocked) arrives after 5 ms, as one expiry.
    let mut event = [0u8; 64];
    event[8..12].copy_from_slice(&12i32.to_le_bytes());
    let mut timer: i32 = -1;
    let created = sys(
        TIMER_CREATE,
        [1, event.as_mut_ptr() as usize, &mut timer as *mut i32 as usize, 0, 0, 0],
    );
    step("timer_create", created == 0 && timer >= 0, 38);
    let spec = [0i64, 0, 0, 5_000_000];
    let armed = sys(
        TIMER_SETTIME,
        [timer as usize, 0, spec.as_ptr() as usize, 0, 0, 0],
    );
    let mut left = [0i64; 4];
    let read = sys(
        TIMER_GETTIME,
        [timer as usize, left.as_mut_ptr() as usize, 0, 0, 0, 0],
    );
    step(
        "timer_settime",
        armed == 0 && read == 0 && left[2] == 0 && left[3] > 0,
        39,
    );
    let wait = [0i64, 500_000_000];
    let fired = sys(
        SIGTIMEDWAIT,
        [&usr2 as *const u64 as usize, 0, wait.as_ptr() as usize, 8, 0, 0],
    );
    let mut after = [1i64; 4];
    let _ = sys(
        TIMER_GETTIME,
        [timer as usize, after.as_mut_ptr() as usize, 0, 0, 0, 0],
    );
    step(
        "timer_expiry",
        fired == 12 && after[2] == 0 && after[3] == 0,
        46,
    );
    let deleted = sys(TIMER_DELETE, [timer as usize, 0, 0, 0, 0, 0]);
    let again = sys(TIMER_DELETE, [timer as usize, 0, 0, 0, 0, 0]);
    step("timer_delete", deleted == 0 && again == -22, 47);

    // System V message queue and semaphores.
    let queue = sys(MSGGET, [0, 0o1000 | 0o600, 0, 0, 0, 0]);
    step("msgget", queue >= 0, 50);
    let mut outgoing = [0u8; 8 + 5];
    outgoing[..8].copy_from_slice(&7i64.to_le_bytes());
    outgoing[8..].copy_from_slice(b"hello");
    let sent = sys(
        MSGSND,
        [queue as usize, outgoing.as_ptr() as usize, 5, 0, 0, 0],
    );
    let mut incoming = [0u8; 8 + 32];
    let received = sys(
        MSGRCV,
        [queue as usize, incoming.as_mut_ptr() as usize, 32, 0, 0, 0],
    );
    step(
        "msg_roundtrip",
        sent == 0
            && received == 5
            && i64::from_le_bytes(incoming[..8].try_into().unwrap()) == 7
            && &incoming[8..13] == b"hello",
        51,
    );
    let none = sys(
        MSGRCV,
        [queue as usize, incoming.as_mut_ptr() as usize, 32, 0, 0o4000, 0],
    );
    step("msg_empty", none == -42, 52);
    step(
        "msgctl_rmid",
        sys(MSGCTL, [queue as usize, 0, 0, 0, 0, 0]) == 0
            && sys(MSGSND, [queue as usize, outgoing.as_ptr() as usize, 5, 0, 0, 0]) == -22,
        53,
    );
    let set = sys(SEMGET, [0, 2, 0o1000 | 0o600, 0, 0, 0]);
    step("semget", set >= 0, 54);
    let up = [0u16, 2, 0];
    let down = [0u16, (-1i16) as u16, 0];
    let block = [0u16, (-3i16) as u16, 0o4000];
    let raised = sys(SEMOP, [set as usize, up.as_ptr() as usize, 1, 0, 0, 0]);
    let lowered = sys(SEMOP, [set as usize, down.as_ptr() as usize, 1, 0, 0, 0]);
    let value = sys(SEMCTL, [set as usize, 0, 12, 0, 0, 0]);
    let refused = sys(SEMOP, [set as usize, block.as_ptr() as usize, 1, 0, 0, 0]);
    step(
        "semop",
        raised == 0 && lowered == 0 && value == 1 && refused == -11,
        55,
    );
    let mut all = [0u16; 2];
    let listed = sys(SEMCTL, [set as usize, 0, 13, all.as_mut_ptr() as usize, 0, 0]);
    step(
        "semctl_getall",
        listed == 0 && all == [1, 0] && sys(SEMCTL, [set as usize, 0, 0, 0, 0, 0]) == 0,
        56,
    );

    // CPU time: a pure user-mode spin shows up in getrusage, times and the
    // process CPU clock.
    let mut accumulator = 0u64;
    for index in 0..300_000_000u64 {
        accumulator = std::hint::black_box(accumulator.wrapping_add(index ^ (index >> 3)));
    }
    std::hint::black_box(accumulator);
    let mut usage = [0u64; 18];
    let usage_ok = sys(GETRUSAGE, [0, usage.as_mut_ptr() as usize, 0, 0, 0, 0]) == 0;
    let user_us = usage[0] * 1_000_000 + usage[1];
    let system_us = usage[2] * 1_000_000 + usage[3];
    let mut ticks = [0u64; 4];
    let _ = sys(TIMES, [ticks.as_mut_ptr() as usize, 0, 0, 0, 0, 0]);
    let mut cpu = [0u64; 2];
    let clock_ok = sys(CLOCK_GETTIME, [2, cpu.as_mut_ptr() as usize, 0, 0, 0, 0]) == 0;
    let cpu_us = cpu[0] * 1_000_000 + cpu[1] / 1000;
    step(
        "cpu_time",
        usage_ok
            && clock_ok
            && user_us > 0
            && user_us + system_us < 30_000_000
            && cpu_us >= user_us
            && ticks[0] > 0,
        67,
    );

    let own_exe = std::fs::read_link("/proc/self/exe").unwrap_or_default();
    let own_exe = own_exe.to_string_lossy();
    step(
        "proc_self_exe",
        own_exe.starts_with("/bin/") && own_exe != "/bin/init",
        73,
    );

    // execveat by path and by descriptor (AT_EMPTY_PATH) starts the program
    // again, which exits 88 when asked to.
    let program = cstr("/bin/aeros-sysx");
    let argument = cstr("exec-child");
    let argv = [program.as_ptr() as usize, argument.as_ptr() as usize, 0];
    let envp = [0usize];
    let empty = cstr("");
    let mut exec_ok = true;
    for by_descriptor in [false, true] {
        let spawned = sys(FORK, [0; 6]);
        if spawned == 0 {
            let result = if by_descriptor {
                let fd = sys(OPEN, [program.as_ptr() as usize, 0, 0, 0, 0, 0]);
                sys(
                    EXECVEAT,
                    [fd as usize, empty.as_ptr() as usize, argv.as_ptr() as usize, envp.as_ptr() as usize, 0x1000, 0],
                )
            } else {
                sys(
                    EXECVEAT,
                    [AT_FDCWD as usize, program.as_ptr() as usize, argv.as_ptr() as usize, envp.as_ptr() as usize, 0, 0],
                )
            };
            std::process::exit(if result < 0 { 87 } else { 86 });
        }
        let mut status = 0i32;
        let waited = sys(
            WAIT4,
            [spawned as usize, &mut status as *mut i32 as usize, 0, 0, 0, 0],
        );
        exec_ok &= spawned > 0 && waited == spawned && (status >> 8) & 0xff == 88;
    }
    step("execveat", exec_ok, 70);

    // process_vm_readv / process_vm_writev against a forked child that has
    // the same layout: the child's copy of the buffer is read, then replaced.
    let mut shared_buffer = [0u8; 32];
    for (index, byte) in shared_buffer.iter_mut().enumerate() {
        *byte = 0x40 + index as u8;
    }
    let mut gate = [0i32; 2];
    let _ = sys(PIPE2, [gate.as_mut_ptr() as usize, 0, 0, 0, 0, 0]);
    let peer = sys(FORK, [0; 6]);
    if peer == 0 {
        let mut go = [0u8; 1];
        loop {
            if sys(READ, [gate[0] as usize, go.as_mut_ptr() as usize, 1, 0, 0, 0]) == 1 {
                break;
            }
        }
        let expected: Vec<u8> = (0..32).map(|index| 0xa0 + index as u8).collect();
        let held = std::hint::black_box(&shared_buffer);
        sys(EXIT_GROUP, [if held[..] == expected[..] { 91 } else { 92 }, 0, 0, 0, 0, 0]);
    }
    shared_buffer.fill(0);
    let mut fetched = [0u8; 32];
    let local = [(fetched.as_mut_ptr() as usize, 32usize)];
    let remote = [(shared_buffer.as_ptr() as usize, 32usize)];
    let read_back = sys(
        PROCESS_VM_READV,
        [peer as usize, local.as_ptr() as usize, 1, remote.as_ptr() as usize, 1, 0],
    );
    let wanted: Vec<u8> = (0..32).map(|index| 0x40 + index as u8).collect();
    let replacement: Vec<u8> = (0..32).map(|index| 0xa0 + index as u8).collect();
    let local_write = [(replacement.as_ptr() as usize, 32usize)];
    let written_to = sys(
        PROCESS_VM_WRITEV,
        [peer as usize, local_write.as_ptr() as usize, 1, remote.as_ptr() as usize, 1, 0],
    );
    let _ = sys(WRITE, [gate[1] as usize, b"g".as_ptr() as usize, 1, 0, 0, 0]);
    let mut outcome = 0i32;
    let reaped = sys(
        WAIT4,
        [peer as usize, &mut outcome as *mut i32 as usize, 0, 0, 0, 0],
    );
    step(
        "process_vm",
        read_back == 32
            && fetched[..] == wanted[..]
            && written_to == 32
            && reaped == peer
            && (outcome >> 8) & 0xff == 91,
        72,
    );

    // SIGCHLD reaches a handler when a child ends, carrying the child's pid.
    let chld_action = [
        chld_handler as usize as u64,
        0x0400_0004u64,
        segv_restorer as usize as u64,
        0u64,
    ];
    let chld_installed = sys(RT_SIGACTION, [17, chld_action.as_ptr() as usize, 0, 8, 0, 0]);
    let short_lived = sys(FORK, [0; 6]);
    if short_lived == 0 {
        sys(EXIT_GROUP, [0; 6]);
    }
    let mut short_status = 0i32;
    let short_reaped = sys(
        WAIT4,
        [short_lived as usize, &mut short_status as *mut i32 as usize, 0, 0, 0, 0],
    );
    // The handler runs on the way out of wait4 or the next syscall.
    let _ = sys(GETPID, [0; 6]);
    step(
        "sigchld",
        chld_installed == 0
            && short_reaped == short_lived
            && CHILD_SIGNALS.load(std::sync::atomic::Ordering::SeqCst) >= 1
            && CHILD_SIGNAL_PID.load(std::sync::atomic::Ordering::SeqCst) == short_lived as u32,
        80,
    );
    let ignore = [1u64, 0, 0, 0];
    let _ = sys(RT_SIGACTION, [17, ignore.as_ptr() as usize, 0, 8, 0, 0]);

    // A write to a pipe with no reader ends the process with SIGPIPE.
    let mut broken = [0i32; 2];
    let _ = sys(PIPE2, [broken.as_mut_ptr() as usize, 0, 0, 0, 0, 0]);
    let _ = sys(CLOSE, [broken[0] as usize, 0, 0, 0, 0, 0]);
    let writer = sys(FORK, [0; 6]);
    if writer == 0 {
        // The Rust runtime starts with SIGPIPE ignored; put the default back.
        let default_action = [0u64; 4];
        let _ = sys(RT_SIGACTION, [13, default_action.as_ptr() as usize, 0, 8, 0, 0]);
        let _ = sys(WRITE, [broken[1] as usize, b"x".as_ptr() as usize, 1, 0, 0, 0]);
        sys(EXIT_GROUP, [95, 0, 0, 0, 0, 0]);
    }
    let _ = sys(CLOSE, [broken[1] as usize, 0, 0, 0, 0, 0]);
    let mut pipe_status = 0i32;
    let pipe_reaped = sys(
        WAIT4,
        [writer as usize, &mut pipe_status as *mut i32 as usize, 0, 0, 0, 0],
    );
    step(
        "sigpipe",
        writer > 0 && pipe_reaped == writer && pipe_status & 0x7f == 13,
        81,
    );

    // pidfd: becomes readable when the process has ended; a signal reaches
    // it through the descriptor.
    let watched = sys(FORK, [0; 6]);
    if watched == 0 {
        for _ in 0..2_000_000 {
            std::hint::black_box(0u64);
        }
        sys(EXIT_GROUP, [0; 6]);
    }
    let pidfd = sys(PIDFD_OPEN, [watched as usize, 0, 0, 0, 0, 0]);
    let ghost = sys(PIDFD_OPEN, [4_000_000, 0, 0, 0, 0, 0]);
    let alive = sys(PIDFD_SEND_SIGNAL, [pidfd as usize, 0, 0, 0, 0, 0]);
    let mut status = 0i32;
    let waited = sys(
        WAIT4,
        [watched as usize, &mut status as *mut i32 as usize, 0, 0, 0, 0],
    );
    step(
        "pidfd",
        watched > 0 && pidfd >= 0 && ghost == -3 && alive == 0 && waited == watched,
        71,
    );

    // ITIMER_PROF fires after 30 ms of CPU time; SIGPROF is blocked and
    // collected by sigtimedwait once the spin has used the time up.
    let prof = 1u64 << 26;
    let mut previous_mask = 0u64;
    let blocked_prof = sys(
        SIGPROCMASK,
        [
            0,
            &prof as *const u64 as usize,
            &mut previous_mask as *mut u64 as usize,
            8,
            0,
            0,
        ],
    );
    let timer_value = [0i64, 0, 0, 30_000];
    let armed_prof = sys(SETITIMER, [2, timer_value.as_ptr() as usize, 0, 0, 0, 0]);
    let mut busy = 0u64;
    for index in 0..150_000_000u64 {
        busy = std::hint::black_box(busy.wrapping_add(index));
    }
    std::hint::black_box(busy);
    let delivered = sys(SIGTIMEDWAIT, [&prof as *const u64 as usize, 0, 0, 8, 0, 0]);
    step(
        "itimer_prof",
        blocked_prof == 0 && armed_prof == 0 && delivered == 27,
        69,
    );

    // A waited-for child's CPU time shows up as the children's.
    let burner = sys(FORK, [0; 6]);
    if burner == 0 {
        let mut spin = 0u64;
        for index in 0..150_000_000u64 {
            spin = std::hint::black_box(spin.wrapping_add(index ^ (index >> 5)));
        }
        std::hint::black_box(spin);
        sys(EXIT_GROUP, [0; 6]);
    }
    let mut burned = 0i32;
    let reaped = sys(
        WAIT4,
        [burner as usize, &mut burned as *mut i32 as usize, 0, 0, 0, 0],
    );
    let mut children = [0u64; 18];
    let children_ok = sys(GETRUSAGE, [usize::MAX, children.as_mut_ptr() as usize, 0, 0, 0, 0]) == 0;
    let mut tms = [0u64; 4];
    let _ = sys(TIMES, [tms.as_mut_ptr() as usize, 0, 0, 0, 0, 0]);
    step(
        "children_cpu_time",
        burner > 0
            && reaped == burner
            && children_ok
            && children[0] * 1_000_000 + children[1] > 0
            && tms[2] > 0,
        68,
    );

    // MAP_SHARED memory stays shared across fork; a private mapping does not.
    let shared = sys(MMAP, [0, page, 3, 0x21, usize::MAX, 0]);
    let private = sys(MMAP, [0, page, 3, 0x22, usize::MAX, 0]);
    step("map_shared", shared > 0 && private > 0, 60);
    let (shared, private) = (shared as usize as *mut u8, private as usize as *mut u8);
    unsafe {
        *shared = 0x11;
        *private = 0x11;
    }
    let child = sys(FORK, [0; 6]);
    if child == 0 {
        unsafe {
            *shared = 0x22;
            *private = 0x33;
        }
        sys(EXIT_GROUP, [0; 6]);
    }
    let mut status = 0i32;
    let waited = sys(
        WAIT4,
        [child as usize, &mut status as *mut i32 as usize, 0, 0, 0, 0],
    );
    let (seen_shared, seen_private) = unsafe { (*shared, *private) };
    step(
        "map_shared_fork",
        child > 0 && waited == child && seen_shared == 0x22 && seen_private == 0x11,
        61,
    );

    // System V shared memory: attach, fork, child writes, parent reads.
    let segment = sys(SHMGET, [0, 8192, 0o1000 | 0o600, 0, 0, 0]);
    step("shmget", segment >= 0, 62);
    let attached = sys(SHMAT, [segment as usize, 0, 0, 0, 0, 0]);
    step("shmat", attached > 0, 63);
    let window = attached as usize as *mut u8;
    unsafe { *window.add(4097) = 0x5c };
    let mut record = [0u8; 112];
    let counted = sys(
        SHMCTL,
        [segment as usize, 2, record.as_mut_ptr() as usize, 0, 0, 0],
    );
    let size = u64::from_le_bytes(record[48..56].try_into().unwrap());
    let attaches = u64::from_le_bytes(record[88..96].try_into().unwrap());
    step("shmctl_stat", counted == 0 && size == 8192 && attaches == 1, 64);
    let child = sys(FORK, [0; 6]);
    if child == 0 {
        unsafe {
            let inherited = *window.add(4097);
            *window.add(4098) = inherited.wrapping_add(1);
        }
        sys(EXIT_GROUP, [0; 6]);
    }
    let waited = sys(
        WAIT4,
        [child as usize, &mut status as *mut i32 as usize, 0, 0, 0, 0],
    );
    let echoed = unsafe { *window.add(4098) };
    step(
        "shm_fork",
        child > 0 && waited == child && echoed == 0x5d,
        65,
    );
    let detached = sys(SHMDT, [attached as usize, 0, 0, 0, 0, 0]);
    let removed = sys(SHMCTL, [segment as usize, 0, 0, 0, 0, 0]);
    let gone = sys(SHMAT, [segment as usize, 0, 0, 0, 0, 0]);
    step(
        "shmdt_rmid",
        detached == 0 && removed == 0 && gone == -22,
        66,
    );

    // siginfo details: sigqueue's value and code, and a timer's value, reach
    // the signalfd record.
    let mut queued = [0u8; 128];
    queued[..4].copy_from_slice(&10i32.to_le_bytes());
    queued[8..12].copy_from_slice(&(-1i32).to_le_bytes());
    queued[24..32].copy_from_slice(&0x1234_5678_9abc_def0u64.to_le_bytes());
    let me = sys(GETPID, [0; 6]);
    let sent_ok = sys(
        RT_SIGQUEUEINFO,
        [me as usize, 10, queued.as_ptr() as usize, 0, 0, 0],
    ) == 0;
    let mut record = [0u8; 128];
    let taken = sys(
        READ,
        [descriptor as usize, record.as_mut_ptr() as usize, 128, 0, 0, 0],
    );
    let queued_code = i32::from_le_bytes([record[8], record[9], record[10], record[11]]);
    let queued_value = u64::from_le_bytes(record[48..56].try_into().unwrap());
    step(
        "siginfo_queue",
        sent_ok && taken == 128 && queued_code == -1 && queued_value == 0x1234_5678_9abc_def0,
        75,
    );
    let mut event = [0u8; 64];
    event[..8].copy_from_slice(&0x00c0_ffee_u64.to_le_bytes());
    event[8..12].copy_from_slice(&10i32.to_le_bytes());
    let mut timer_id: i32 = -1;
    let made = sys(
        TIMER_CREATE,
        [1, event.as_mut_ptr() as usize, &mut timer_id as *mut i32 as usize, 0, 0, 0],
    );
    let spec = [0i64, 0, 0, 3_000_000];
    let started = sys(
        TIMER_SETTIME,
        [timer_id as usize, 0, spec.as_ptr() as usize, 0, 0, 0],
    );
    let mut fired = [0u8; 128];
    let mut got = -11isize;
    for _ in 0..2_000_000 {
        got = sys(
            READ,
            [descriptor as usize, fired.as_mut_ptr() as usize, 128, 0, 0, 0],
        );
        if got == 128 {
            break;
        }
    }
    let timer_code = i32::from_le_bytes([fired[8], fired[9], fired[10], fired[11]]);
    let timer_value = u64::from_le_bytes(fired[48..56].try_into().unwrap());
    let _ = sys(TIMER_DELETE, [timer_id as usize, 0, 0, 0, 0, 0]);
    step(
        "siginfo_timer",
        made == 0 && started == 0 && got == 128 && timer_code == -2 && timer_value == 0x00c0_ffee,
        76,
    );

    // A SIGSEGV handler runs for a fault, with the signal number and the
    // faulting address; the process exits with 77 from inside it.
    let crasher = sys(FORK, [0; 6]);
    if crasher == 0 {
        let stack = [
            unsafe { core::ptr::addr_of_mut!(ALT_STACK) } as usize as u64,
            0u64,
            16384u64,
        ];
        let stack_set = sys(131, [stack.as_ptr() as usize, 0, 0, 0, 0, 0]);
        if stack_set != 0 {
            sys(EXIT_GROUP, [67, 0, 0, 0, 0, 0]);
        }
        let action = [
            segv_handler as usize as u64,
            0x0400_0004u64 | 0x0800_0000 | 0x8000_0000,
            segv_restorer as usize as u64,
            0u64,
        ];
        let installed = sys(
            RT_SIGACTION,
            [11, action.as_ptr() as usize, 0, 8, 0, 0],
        );
        if installed != 0 {
            sys(EXIT_GROUP, [66, 0, 0, 0, 0, 0]);
        }
        unsafe { core::ptr::write_volatile(0x5151_0000usize as *mut u8, 1) };
        sys(EXIT_GROUP, [65, 0, 0, 0, 0, 0]);
    }
    let mut crashed = 0i32;
    let collected = sys(
        WAIT4,
        [crasher as usize, &mut crashed as *mut i32 as usize, 0, 0, 0, 0],
    );
    step(
        "segv_handler",
        crasher > 0 && collected == crasher && (crashed >> 8) & 0xff == 77,
        79,
    );

    // mknod / link / getdents / openat2 / syslog / inotify.
    let node = cstr("/tmp/sx-node");
    let _ = std::fs::remove_file("/tmp/sx-node");
    step(
        "mknod",
        sys(MKNOD, [node.as_ptr() as usize, 0o100644, 0, 0, 0, 0]) == 0
            && std::fs::metadata("/tmp/sx-node").is_ok(),
        40,
    );
    let other = cstr("/tmp/sx-link");
    step(
        "link_refused",
        sys(LINK, [node.as_ptr() as usize, other.as_ptr() as usize, 0, 0, 0, 0]) == -1,
        41,
    );
    let root = cstr("/tmp");
    let directory = sys(OPEN, [root.as_ptr() as usize, 0o200000, 0, 0, 0, 0]);
    let mut entry = [0u8; 288];
    let read = sys(
        GETDENTS,
        [directory as usize, entry.as_mut_ptr() as usize, 288, 0, 0, 0],
    );
    let length = u16::from_le_bytes([entry[16], entry[17]]) as usize;
    step(
        "getdents",
        directory >= 0 && read > 0 && length == read as usize && length % 8 == 0,
        42,
    );
    let hostname = cstr("/etc/aeros-release");
    let how = [0u64, 0, 0];
    let opened = sys(
        OPENAT2,
        [
            AT_FDCWD as usize,
            hostname.as_ptr() as usize,
            how.as_ptr() as usize,
            24,
            0,
            0,
        ],
    );
    let restricted = [0u64, 0, 1];
    let denied = sys(
        OPENAT2,
        [
            AT_FDCWD as usize,
            hostname.as_ptr() as usize,
            restricted.as_ptr() as usize,
            24,
            0,
            0,
        ],
    );
    step("openat2", opened >= 0 && denied == -38, 43);
    let mut log = vec![0u8; 512];
    let logged = sys(SYSLOG, [3, log.as_mut_ptr() as usize, 512, 0, 0, 0]);
    step("syslog", logged > 0 && logged <= 512, 44);
    let watch = sys(INOTIFY_INIT1, [0, 0, 0, 0, 0, 0]);
    let wd = sys(
        INOTIFY_ADD_WATCH,
        [watch as usize, root.as_ptr() as usize, 0x2, 0, 0, 0],
    );
    step("inotify", watch >= 0 && wd > 0, 45);
    let _ = std::fs::remove_file("/tmp/sx-ino");
    std::fs::write("/tmp/sx-ino", b"watched").expect("write watched file");
    let mut events = [0u8; 256];
    let received = sys(
        READ,
        [watch as usize, events.as_mut_ptr() as usize, 256, 0, 0, 0],
    );
    let event_mask = u32::from_le_bytes([events[4], events[5], events[6], events[7]]);
    let name_length = u32::from_le_bytes([events[12], events[13], events[14], events[15]]) as usize;
    step(
        "inotify_event",
        received >= 16
            && i32::from_le_bytes([events[0], events[1], events[2], events[3]]) == wd as i32
            && event_mask == 0x2
            && name_length >= 7
            && &events[16..23] == b"sx-ino\0",
        46,
    );
    let _ = std::fs::remove_file("/tmp/sx-ino");

    let _ = std::fs::remove_file("/tmp/sx-src");
    let _ = std::fs::remove_file("/tmp/sx-dst");
    let _ = std::fs::remove_file("/tmp/sx-node");
    println!("AEROS_SYSX done");
    std::process::exit(89);
}
