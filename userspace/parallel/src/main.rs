//! Threads of one process running on several processors at once: checks that
//! they really overlap, and that changes to the shared page tables (fork's
//! copy-on-write, unmap and re-map at the same address) reach every thread
//! straight away. Exits with 78 when everything held and two threads were
//! seen on different processors together, 79 when everything held but they
//! never overlapped (one processor), anything else on a failure.

use std::arch::asm;
use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

const STACK: usize = 16 * 1024;

fn spawn<F: FnOnce() + Send + 'static>(body: F) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .stack_size(STACK)
        .spawn(body)
        .expect("spawn a thread")
}

fn step(name: &str, ok: bool, code: i32) {
    println!("AEROS_PARALLEL_STEP {name} {}", if ok { "ok" } else { "FAILED" });
    if !ok {
        std::process::exit(code);
    }
}

fn syscall6(number: usize, args: [usize; 6]) -> isize {
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

fn cpu() -> usize {
    let aux: u32;
    unsafe {
        asm!("rdtscp", out("eax") _, out("edx") _, out("ecx") aux, options(nomem, nostack));
    }
    (aux & 0xfff) as usize
}

const MMAP: usize = 9;
const MUNMAP: usize = 11;
const FORK: usize = 57;
const WAIT4: usize = 61;
const EXIT_GROUP: usize = 231;

static PUBLISHED: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(0)];
static COUNT: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
static OVERLAPS: AtomicU64 = AtomicU64::new(0);
static STOP: AtomicBool = AtomicBool::new(false);

/// One of two threads that publish which processor they are on and a running
/// count; each looks for the other's count moving, on another processor,
/// while its own processor stayed the same.
fn watcher(me: usize) {
    let other = 1 - me;
    let mut local = 0u64;
    while !STOP.load(Ordering::Relaxed) {
        let mine = cpu();
        PUBLISHED[me].store(mine, Ordering::Relaxed);
        local += 1;
        COUNT[me].store(local, Ordering::Release);
        let before = COUNT[other].load(Ordering::Acquire);
        for _ in 0..4000 {
            spin_loop();
        }
        let after = COUNT[other].load(Ordering::Acquire);
        let elsewhere = PUBLISHED[other].load(Ordering::Relaxed);
        if after > before && cpu() == mine && elsewhere != mine {
            OVERLAPS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(0);
static WRITING: AtomicBool = AtomicBool::new(true);

/// A thread that keeps incrementing a counter in a page the main thread's
/// `fork` is about to share with the child.
fn writer() {
    while WRITING.load(Ordering::Relaxed) {
        COUNTER.fetch_add(1, Ordering::Relaxed);
    }
}

fn fork_keeps_the_child_frozen() -> bool {
    let helper = spawn(writer);
    let mut good = true;
    for _ in 0..30 {
        let pid = syscall6(FORK, [0; 6]);
        if pid == 0 {
            let first = COUNTER.load(Ordering::Relaxed);
            for _ in 0..400_000 {
                spin_loop();
            }
            let second = COUNTER.load(Ordering::Relaxed);
            syscall6(EXIT_GROUP, [usize::from(first != second), 0, 0, 0, 0, 0]);
            loop {}
        }
        if pid < 0 {
            good = false;
            break;
        }
        let mut status = 0i32;
        let waited = syscall6(WAIT4, [pid as usize, &mut status as *mut i32 as usize, 0, 0, 0, 0]);
        if waited != pid || status != 0 {
            good = false;
            break;
        }
    }
    WRITING.store(false, Ordering::Relaxed);
    let _ = helper.join();
    good
}

static ADDRESS: AtomicUsize = AtomicUsize::new(0);
static EXPECTED: AtomicUsize = AtomicUsize::new(0);
static ALLOW: AtomicBool = AtomicBool::new(false);
static PARKED: AtomicBool = AtomicBool::new(false);
static MISMATCHES: AtomicU64 = AtomicU64::new(0);
static READS: AtomicU64 = AtomicU64::new(0);
static DONE: AtomicBool = AtomicBool::new(false);

/// Reads the first byte of the page the main thread keeps unmapping and
/// mapping again; it must always find the byte the main thread last wrote.
fn reader() {
    while !DONE.load(Ordering::Relaxed) {
        if ALLOW.load(Ordering::Acquire) {
            let address = ADDRESS.load(Ordering::Acquire);
            let value = unsafe { std::ptr::read_volatile(address as *const u8) } as usize;
            if value != EXPECTED.load(Ordering::Acquire) {
                MISMATCHES.fetch_add(1, Ordering::Relaxed);
            }
            READS.fetch_add(1, Ordering::Relaxed);
        } else {
            PARKED.store(true, Ordering::Release);
        }
    }
}

fn map_page(fill: u8) -> usize {
    let address = syscall6(MMAP, [0, 4096, 3, 0x22, usize::MAX, 0]);
    if address < 0 {
        return 0;
    }
    unsafe { std::ptr::write_bytes(address as *mut u8, fill, 4096) };
    address as usize
}

fn remap_is_seen_at_once() -> bool {
    let helper = spawn(reader);
    let mut good = true;
    let mut address = map_page(0xa0);
    ADDRESS.store(address, Ordering::Release);
    EXPECTED.store(0xa0, Ordering::Release);
    ALLOW.store(true, Ordering::Release);
    for round in 0..150usize {
        // Let the reader use the page for a while, so its processor caches it.
        let reads = READS.load(Ordering::Acquire);
        while READS.load(Ordering::Acquire) < reads + 50 {
            spin_loop();
        }
        // Stop it touching the page, unmap it, and map a different page.
        PARKED.store(false, Ordering::Release);
        ALLOW.store(false, Ordering::Release);
        while !PARKED.load(Ordering::Acquire) {
            spin_loop();
        }
        syscall6(MUNMAP, [address, 4096, 0, 0, 0, 0]);
        let fill = 0x10 + (round % 0xc0) as u8;
        let next = map_page(fill);
        if next == 0 {
            good = false;
            break;
        }
        address = next;
        ADDRESS.store(address, Ordering::Release);
        EXPECTED.store(fill as usize, Ordering::Release);
        ALLOW.store(true, Ordering::Release);
    }
    ALLOW.store(false, Ordering::Release);
    DONE.store(true, Ordering::Release);
    let _ = helper.join();
    good && MISMATCHES.load(Ordering::Relaxed) == 0
}

fn main() {
    println!("AEROS_PARALLEL_BEGIN");
    let first = spawn(|| watcher(0));
    let second = spawn(|| watcher(1));
    thread::sleep(Duration::from_millis(400));
    STOP.store(true, Ordering::Relaxed);
    let _ = first.join();
    let _ = second.join();
    let overlaps = OVERLAPS.load(Ordering::Relaxed);
    println!("AEROS_PARALLEL_OVERLAPS {overlaps}");

    println!("AEROS_PARALLEL_FORK");
    step("fork-while-writing", fork_keeps_the_child_frozen(), 1);
    println!("AEROS_PARALLEL_REMAP");
    step("remap-seen-at-once", remap_is_seen_at_once(), 2);

    // The process ends while two of its threads are still running elsewhere.
    for _ in 0..2 {
        spawn(|| loop {
            spin_loop();
        });
    }
    thread::sleep(Duration::from_millis(50));
    println!("AEROS_PARALLEL_DONE");
    std::process::exit(if overlaps > 0 { 78 } else { 79 });
}
