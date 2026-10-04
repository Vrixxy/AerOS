use std::arch::asm;
use std::cell::Cell;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier, Condvar, Mutex};
use std::thread;
use std::time::Duration;

const STACK: usize = 32 * 1024;

thread_local! {
    static LOCAL: Cell<usize> = const { Cell::new(0) };
}

fn spawn<F, T>(body: F) -> thread::JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    thread::Builder::new()
        .stack_size(STACK)
        .spawn(body)
        .expect("spawn a thread")
}

fn step(name: &str, ok: bool, code: i32) {
    println!("AEROS_THREADS_STEP {name} {}", if ok { "ok" } else { "FAILED" });
    if !ok {
        std::process::exit(code);
    }
}

fn syscall0(number: usize) -> usize {
    let result: usize;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") number => result,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack)
        );
    }
    result
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

const FUTEX: usize = 202;
const PRIVATE: usize = 128;
const WAIT: usize = 0;
const WAKE: usize = 1;
const CMP_REQUEUE: usize = 4;
const WAKE_OP: usize = 5;
const LOCK_PI: usize = 6;
const UNLOCK_PI: usize = 7;
const TRYLOCK_PI: usize = 8;

fn futex(word: &AtomicU32, operation: usize, value: usize, timeout: usize, other: usize, third: usize) -> isize {
    syscall6(
        FUTEX,
        [word as *const AtomicU32 as usize, operation, value, timeout, other, third],
    )
}

fn address_of(word: &AtomicU32) -> usize {
    word as *const AtomicU32 as usize
}

#[repr(C)]
struct RobustHead {
    next: usize,
    offset: isize,
    pending: usize,
}

#[repr(C)]
struct RobustEntry {
    next: usize,
    word: AtomicU32,
}
fn main() {
    let values: Vec<usize> = (0..4usize)
        .map(|index| spawn(move || index * 10 + 1))
        .collect::<Vec<_>>()
        .into_iter()
        .map(|handle| handle.join().unwrap_or(usize::MAX))
        .collect();
    step("join", values == [1, 11, 21, 31], 1);

    let counter = Arc::new(AtomicUsize::new(0));
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let counter = counter.clone();
            spawn(move || {
                for _ in 0..20_000 {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            })
        })
        .collect();
    for worker in workers {
        let _ = worker.join();
    }
    step("atomics", counter.load(Ordering::Relaxed) == 80_000, 2);

    let list = Arc::new(Mutex::new(Vec::new()));
    let workers: Vec<_> = (0..4usize)
        .map(|worker| {
            let list = list.clone();
            spawn(move || {
                for item in 0..500usize {
                    list.lock().unwrap().push(worker * 1000 + item);
                    if item % 100 == 0 {
                        thread::yield_now();
                    }
                }
            })
        })
        .collect();
    for worker in workers {
        let _ = worker.join();
    }
    let collected = list.lock().unwrap();
    let expected: usize = (0..4usize)
        .map(|worker| (0..500usize).map(|item| worker * 1000 + item).sum::<usize>())
        .sum();
    step(
        "mutex",
        collected.len() == 2000 && collected.iter().sum::<usize>() == expected,
        3,
    );
    drop(collected);

    let turn = Arc::new((Mutex::new(0u32), Condvar::new()));
    let partner = {
        let turn = turn.clone();
        spawn(move || {
            for round in 0..200u32 {
                let (lock, signal) = &*turn;
                let mut state = lock.lock().unwrap();
                while *state != round * 2 + 1 {
                    state = signal.wait(state).unwrap();
                }
                *state += 1;
                signal.notify_all();
            }
        })
    };
    for round in 0..200u32 {
        let (lock, signal) = &*turn;
        let mut state = lock.lock().unwrap();
        while *state != round * 2 {
            state = signal.wait(state).unwrap();
        }
        *state += 1;
        signal.notify_all();
    }
    let _ = partner.join();
    step("condvar", *turn.0.lock().unwrap() == 400, 4);

    LOCAL.set(7);
    let readers: Vec<_> = (1..=3usize)
        .map(|index| {
            spawn(move || {
                LOCAL.set(index * 100);
                for _ in 0..50 {
                    thread::yield_now();
                }
                LOCAL.get()
            })
        })
        .collect();
    let seen: Vec<usize> = readers
        .into_iter()
        .map(|handle| handle.join().unwrap_or(0))
        .collect();
    step("thread-local", seen == [100, 200, 300] && LOCAL.get() == 7, 5);

    let order = Arc::new(Mutex::new(Vec::new()));
    let sleepers: Vec<_> = [30u64, 10, 20]
        .into_iter()
        .enumerate()
        .map(|(index, milliseconds)| {
            let order = order.clone();
            spawn(move || {
                thread::sleep(Duration::from_millis(milliseconds));
                order.lock().unwrap().push(index);
            })
        })
        .collect();
    for sleeper in sleepers {
        let _ = sleeper.join();
    }
    step("sleep", *order.lock().unwrap() == [1, 2, 0], 6);

    let (sender, receiver) = mpsc::channel::<usize>();
    let producers: Vec<_> = (0..3usize)
        .map(|producer| {
            let sender = sender.clone();
            spawn(move || {
                for message in 0..100usize {
                    let _ = sender.send(producer * 1000 + message);
                }
            })
        })
        .collect();
    drop(sender);
    let received: usize = receiver.iter().sum();
    for producer in producers {
        let _ = producer.join();
    }
    let sent: usize = (0..3usize)
        .map(|producer| (0..100usize).map(|message| producer * 1000 + message).sum::<usize>())
        .sum();
    step("channel", received == sent, 7);

    let barrier = Arc::new(Barrier::new(4));
    let before = Arc::new(AtomicUsize::new(0));
    let waiters: Vec<_> = (0..3)
        .map(|_| {
            let (barrier, before) = (barrier.clone(), before.clone());
            spawn(move || {
                before.fetch_add(1, Ordering::SeqCst);
                barrier.wait();
                before.load(Ordering::SeqCst)
            })
        })
        .collect();
    before.fetch_add(1, Ordering::SeqCst);
    barrier.wait();
    let at_barrier: Vec<usize> = waiters
        .into_iter()
        .map(|waiter| waiter.join().unwrap_or(0))
        .collect();
    step("barrier", at_barrier == [4, 4, 4], 8);

    let mut data = vec![1u64; 1000];
    let (left, right) = data.split_at_mut(500);
    thread::scope(|scope| {
        let build = || thread::Builder::new().stack_size(STACK);
        let _ = build().spawn_scoped(scope, || left.iter_mut().for_each(|value| *value *= 3));
        let _ = build().spawn_scoped(scope, || right.iter_mut().for_each(|value| *value *= 5));
    });
    step("scope", data.iter().sum::<u64>() == 500 * 3 + 500 * 5, 9);

    let mut total = 0usize;
    for index in 0..20usize {
        total += spawn(move || index).join().unwrap_or(0);
    }
    step("sequential", total == (0..20).sum::<usize>(), 10);

    let shared = spawn(|| std::fs::write("/tmp/threads-file", b"written by a thread").is_ok())
        .join()
        .unwrap_or(false);
    let read_back = std::fs::read("/tmp/threads-file").unwrap_or_default();
    let _ = std::fs::remove_file("/tmp/threads-file");
    step("shared-files", shared && read_back == b"written by a thread", 11);

    let (process, thread_id) = (syscall0(39), syscall0(186));
    let other = spawn(|| (syscall0(39), syscall0(186))).join().unwrap_or((0, 0));
    step(
        "identity",
        other.0 == process && other.1 != thread_id && process == thread_id,
        12,
    );

    static QUEUE_A: AtomicU32 = AtomicU32::new(0);
    static QUEUE_B: AtomicU32 = AtomicU32::new(0);
    static WOKEN: AtomicUsize = AtomicUsize::new(0);
    let waiters: Vec<_> = (0..3)
        .map(|_| {
            spawn(|| {
                futex(&QUEUE_A, WAIT | PRIVATE, 0, 0, 0, 0);
                WOKEN.fetch_add(1, Ordering::SeqCst);
            })
        })
        .collect();
    thread::sleep(Duration::from_millis(60));
    let moved = futex(&QUEUE_A, CMP_REQUEUE | PRIVATE, 1, 100, address_of(&QUEUE_B), 0);
    thread::sleep(Duration::from_millis(30));
    let first = WOKEN.load(Ordering::SeqCst);
    let rest = futex(&QUEUE_B, WAKE | PRIVATE, 10, 0, 0, 0);
    for waiter in waiters {
        let _ = waiter.join();
    }
    step(
        "futex-requeue",
        moved == 3 && first == 1 && rest == 2 && WOKEN.load(Ordering::SeqCst) == 3,
        13,
    );

    static WAKE_A: AtomicU32 = AtomicU32::new(0);
    static WAKE_B: AtomicU32 = AtomicU32::new(0);
    let on_a = spawn(|| futex(&WAKE_A, WAIT | PRIVATE, 0, 0, 0, 0));
    let on_b = spawn(|| futex(&WAKE_B, WAIT | PRIVATE, 0, 0, 0, 0));
    thread::sleep(Duration::from_millis(60));
    let woken = futex(&WAKE_A, WAKE_OP | PRIVATE, 1, 1, address_of(&WAKE_B), 5 << 12);
    let _ = (on_a.join(), on_b.join());
    step("futex-wake-op", woken == 2 && WAKE_B.load(Ordering::SeqCst) == 5, 14);

    static LOCK: AtomicU32 = AtomicU32::new(0);
    let mine = syscall0(186) as u32;
    let first_lock = futex(&LOCK, LOCK_PI | PRIVATE, 0, 0, 0, 0);
    let contender = spawn(|| {
        let tid = syscall0(186) as u32;
        let got = futex(&LOCK, LOCK_PI | PRIVATE, 0, 0, 0, 0);
        let owner = LOCK.load(Ordering::SeqCst) & 0x3fff_ffff;
        let released = futex(&LOCK, UNLOCK_PI | PRIVATE, 0, 0, 0, 0);
        (tid, got, owner, released)
    });
    thread::sleep(Duration::from_millis(60));
    let flagged = LOCK.load(Ordering::SeqCst) & 0x8000_0000 != 0;
    let busy = futex(&LOCK, TRYLOCK_PI | PRIVATE, 0, 0, 0, 0);
    let unlocked = futex(&LOCK, UNLOCK_PI | PRIVATE, 0, 0, 0, 0);
    let (tid, got, owner, released) = contender.join().unwrap_or((0, -1, 0, -1));
    step(
        "futex-pi",
        first_lock == 0
            && flagged
            && busy == -35
            && unlocked == 0
            && got == 0
            && owner == tid
            && released == 0
            && LOCK.load(Ordering::SeqCst) == 0
            && mine != tid,
        15,
    );

    let head = Box::leak(Box::new(RobustHead {
        next: 0,
        offset: 8,
        pending: 0,
    }));
    let entry = Box::leak(Box::new(RobustEntry {
        next: 0,
        word: AtomicU32::new(0),
    }));
    let head_address = head as *mut RobustHead as usize;
    let entry_address = entry as *mut RobustEntry as usize;
    head.next = entry_address;
    entry.next = head_address;
    let entry_word = &entry.word;
    let dying = spawn(move || {
        let tid = syscall0(186) as u32;
        let registered = syscall6(273, [head_address, 24, 0, 0, 0, 0]);
        entry_word.store(tid, Ordering::SeqCst);
        registered
    });
    let registered = dying.join().unwrap_or(-1);
    let word = entry.word.load(Ordering::SeqCst);
    step(
        "robust-list",
        registered == 0 && word & 0x4000_0000 != 0 && word & 0x3fff_ffff == 0,
        16,
    );
    println!("AEROS_THREADS ok");
    std::process::exit(76);
}
