//! Micro-benchmarks that run unchanged on AerOS and on Linux (as /init of a
//! tiny initramfs), so the same operations can be timed on both.

use std::arch::asm;
use std::time::Instant;

const SYS_READ: usize = 0;
const SYS_WRITE: usize = 1;
const SYS_MMAP: usize = 9;
const SYS_MUNMAP: usize = 11;
const SYS_PIPE: usize = 22;
const SYS_GETPID: usize = 39;
const SYS_FORK: usize = 57;
const SYS_WAIT4: usize = 61;
const SYS_MKDIR: usize = 83;
const SYS_MOUNT: usize = 165;
const SYS_REBOOT: usize = 169;
const SYS_EXIT_GROUP: usize = 231;

fn syscall(number: usize, args: [usize; 6]) -> isize {
    let result: isize;
    // SAFETY: raw Linux system call; every caller passes valid arguments.
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

fn report(name: &str, started: Instant, operations: u64) {
    let total = started.elapsed().as_nanos() as u64;
    println!(
        "AEROS_BENCH {name} ops={operations} total_ns={total} ns_per_op={}",
        total / operations.max(1)
    );
}

fn getpid() {
    let operations = 300_000;
    let started = Instant::now();
    for _ in 0..operations {
        syscall(SYS_GETPID, [0; 6]);
    }
    report("getpid", started, operations);
}

fn clock() {
    let operations = 100_000;
    let started = Instant::now();
    let mut sink = 0u128;
    for _ in 0..operations {
        sink = sink.wrapping_add(Instant::now().elapsed().as_nanos());
    }
    std::hint::black_box(sink);
    report("clock_gettime", started, operations);
}

fn fork_wait() {
    let operations = 1_000;
    let started = Instant::now();
    for _ in 0..operations {
        let child = syscall(SYS_FORK, [0; 6]);
        if child == 0 {
            syscall(SYS_EXIT_GROUP, [0; 6]);
        }
        let mut status = 0i32;
        syscall(SYS_WAIT4, [child as usize, &mut status as *mut i32 as usize, 0, 0, 0, 0]);
    }
    report("fork_exit_wait", started, operations);
}

fn pipe_pingpong() {
    let operations = 10_000;
    let mut down = [0i32; 2];
    let mut up = [0i32; 2];
    syscall(SYS_PIPE, [down.as_mut_ptr() as usize, 0, 0, 0, 0, 0]);
    syscall(SYS_PIPE, [up.as_mut_ptr() as usize, 0, 0, 0, 0, 0]);
    let child = syscall(SYS_FORK, [0; 6]);
    let mut byte = [7u8; 1];
    if child == 0 {
        loop {
            if syscall(SYS_READ, [down[0] as usize, byte.as_mut_ptr() as usize, 1, 0, 0, 0]) <= 0 {
                syscall(SYS_EXIT_GROUP, [0; 6]);
            }
            syscall(SYS_WRITE, [up[1] as usize, byte.as_ptr() as usize, 1, 0, 0, 0]);
        }
    }
    let started = Instant::now();
    for _ in 0..operations {
        syscall(SYS_WRITE, [down[1] as usize, byte.as_ptr() as usize, 1, 0, 0, 0]);
        syscall(SYS_READ, [up[0] as usize, byte.as_mut_ptr() as usize, 1, 0, 0, 0]);
    }
    report("pipe_round_trip", started, operations);
    for fd in [down[0], down[1], up[0], up[1]] {
        syscall(3, [fd as usize, 0, 0, 0, 0, 0]);
    }
    let mut status = 0i32;
    syscall(SYS_WAIT4, [child as usize, &mut status as *mut i32 as usize, 0, 0, 0, 0]);
}

fn mmap_touch() {
    let operations = 2_000;
    let pages = 8usize;
    let started = Instant::now();
    for _ in 0..operations {
        let address = syscall(
            SYS_MMAP,
            [0, pages * 4096, 3, 0x22, usize::MAX, 0],
        );
        if address < 0 {
            break;
        }
        for page in 0..pages {
            // SAFETY: the mapping was just created and is writable.
            unsafe { std::ptr::write_volatile((address as usize + page * 4096) as *mut u8, 1) };
        }
        syscall(SYS_MUNMAP, [address as usize, pages * 4096, 0, 0, 0, 0]);
    }
    report("mmap_touch_munmap_8_pages", started, operations);
}

fn file_cycle() {
    let operations = 2_000;
    let block = [0x5au8; 4096];
    let mut back = [0u8; 4096];
    let name = b"/tmp/bench.dat\0";
    let started = Instant::now();
    for _ in 0..operations {
        let fd = syscall(2, [name.as_ptr() as usize, 0x241, 0o644, 0, 0, 0]);
        if fd < 0 {
            break;
        }
        syscall(SYS_WRITE, [fd as usize, block.as_ptr() as usize, 4096, 0, 0, 0]);
        syscall(3, [fd as usize, 0, 0, 0, 0, 0]);
        let fd = syscall(2, [name.as_ptr() as usize, 0, 0, 0, 0, 0]);
        syscall(SYS_READ, [fd as usize, back.as_mut_ptr() as usize, 4096, 0, 0, 0]);
        syscall(3, [fd as usize, 0, 0, 0, 0, 0]);
        syscall(87, [name.as_ptr() as usize, 0, 0, 0, 0, 0]);
    }
    report("tmpfs_create_write_read_unlink_4k", started, operations);
}

fn memory_copy() {
    let operations = 4_000u64;
    let source = vec![0xa5u8; 64 * 1024];
    let mut destination = vec![0u8; 64 * 1024];
    let started = Instant::now();
    for round in 0..operations {
        destination.copy_from_slice(&source);
        destination[(round % 4096) as usize] ^= 1;
        std::hint::black_box(&destination);
    }
    report("memcpy_64k", started, operations);
}

fn compute() {
    let operations = 40_000_000u64;
    let started = Instant::now();
    let mut state = 0x2545_f491_4f6c_dd1du64;
    for _ in 0..operations {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
    }
    std::hint::black_box(state);
    report("cpu_xorshift", started, operations);
}

fn run() {
    println!("AEROS_BENCH_START");
    compute();
    getpid();
    clock();
    memory_copy();
    mmap_touch();
    file_cycle();
    fork_wait();
    pipe_pingpong();
    println!("AEROS_BENCH_DONE");
}

fn main() {
    let linux_init = std::env::args().next().as_deref() == Some("/init");
    if linux_init {
        syscall(SYS_MKDIR, [b"/tmp\0".as_ptr() as usize, 0o777, 0, 0, 0, 0]);
        syscall(
            SYS_MOUNT,
            [
                b"tmpfs\0".as_ptr() as usize,
                b"/tmp\0".as_ptr() as usize,
                b"tmpfs\0".as_ptr() as usize,
                0,
                0,
                0,
            ],
        );
    }
    run();
    if linux_init {
        syscall(SYS_REBOOT, [0xfee1_dead, 672_274_793, 0x4321_fedc, 0, 0, 0]);
        loop {}
    }
}
