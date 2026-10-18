//! The big kernel lock that lets user programs run on several processors.
//!
//! Kernel mode is not reentrant across processors: the scheduler, the Linux
//! process state and most drivers assume one processor at a time. So a
//! processor takes this lock whenever it enters the kernel from user mode (a
//! system call, an interrupt or a fault) and gives it up when it returns to
//! user mode, which runs in parallel. A task that waits in the kernel gives
//! the lock up on every yield, so a waiter never keeps the processor that
//! would run the task it waits for out of the kernel.
//!
//! Until `enable` is called nothing here does anything, and the kernel behaves
//! as it did with one processor running user programs.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::acpi::MAX_PROCESSORS;

const TSC_AUX: u32 = 0xc000_0103;

#[repr(C)]
pub struct BigLock {
    /// The ticket being served; the holder bumps it to release. At offset 0
    /// and 8 so assembly can release without calling anything.
    serving: AtomicU32,
    next: AtomicU32,
    /// Logical processor holding the lock plus one, 0 when free.
    owner: AtomicU32,
}

static LOCK: BigLock = BigLock {
    serving: AtomicU32::new(0),
    next: AtomicU32::new(0),
    owner: AtomicU32::new(0),
};

static ACTIVE: AtomicBool = AtomicBool::new(false);
static RDTSCP: AtomicBool = AtomicBool::new(false);
static ACQUISITIONS: AtomicU64 = AtomicU64::new(0);
static CONTENDED: AtomicU64 = AtomicU64::new(0);
static SPINS: AtomicU64 = AtomicU64::new(0);
static RELAXES: AtomicU64 = AtomicU64::new(0);
static TRY_FAILED: AtomicU64 = AtomicU64::new(0);
static ENTRIES: [AtomicU64; MAX_PROCESSORS] = [const { AtomicU64::new(0) }; MAX_PROCESSORS];

/// Whether user programs may run on more than one processor.
pub fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// Makes this processor (the one that calls it) the holder and starts the
/// scheme. The caller keeps running kernel code as before.
pub fn enable() {
    reset_counters();
    LOCK.owner.store(0, Ordering::Release);
    lock_unconditionally(this_cpu());
    ACTIVE.store(true, Ordering::Release);
}

/// Ends the scheme; the caller must hold the lock.
#[cfg(feature = "boot-test")]
pub fn disable() {
    ACTIVE.store(false, Ordering::Release);
    unlock();
}

fn reset_counters() {
    ACQUISITIONS.store(0, Ordering::Relaxed);
    CONTENDED.store(0, Ordering::Relaxed);
    SPINS.store(0, Ordering::Relaxed);
    RELAXES.store(0, Ordering::Relaxed);
    for entry in &ENTRIES {
        entry.store(0, Ordering::Relaxed);
    }
}

/// Makes `rdtscp` report `logical` in ECX on this processor, if it has the
/// instruction. Call once per processor, early.
pub fn init_cpu_id(logical: usize, has_rdtscp: bool) {
    if !has_rdtscp {
        return;
    }
    // SAFETY: IA32_TSC_AUX is only read back by `this_cpu`.
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") TSC_AUX,
            in("eax") logical as u32,
            in("edx") 0u32,
            options(nostack, preserves_flags)
        );
    }
    RDTSCP.store(true, Ordering::Release);
}

/// The logical number of the processor this runs on.
pub fn this_cpu() -> usize {
    if RDTSCP.load(Ordering::Relaxed) {
        let aux: u32;
        // SAFETY: `rdtscp` has no side effects beyond its outputs.
        unsafe {
            core::arch::asm!(
                "rdtscp",
                out("eax") _,
                out("edx") _,
                out("ecx") aux,
                options(nomem, nostack, preserves_flags)
            );
        }
        (aux as usize).min(MAX_PROCESSORS - 1)
    } else {
        crate::smp::current_logical().unwrap_or(0)
    }
}

fn lock_unconditionally(cpu: usize) {
    let ticket = LOCK.next.fetch_add(1, Ordering::AcqRel);
    ACQUISITIONS.fetch_add(1, Ordering::Relaxed);
    if LOCK.serving.load(Ordering::Acquire) != ticket {
        CONTENDED.fetch_add(1, Ordering::Relaxed);
        let mut spins = 0u64;
        while LOCK.serving.load(Ordering::Acquire) != ticket {
            spins += 1;
            core::hint::spin_loop();
            #[cfg(feature = "boot-test")]
            if spins == 400_000_000 {
                crate::serial::format(format_args!(
                    "AEROS_LOCK_STUCK cpu={} ticket={} serving={} owner={}
",
                    cpu,
                    ticket,
                    LOCK.serving.load(Ordering::Acquire),
                    LOCK.owner.load(Ordering::Acquire)
                ));
            }
        }
        SPINS.fetch_add(spins, Ordering::Relaxed);
    }
    LOCK.owner.store(cpu as u32 + 1, Ordering::Release);
}

pub fn lock() {
    lock_unconditionally(this_cpu());
}

/// Takes the lock only if nobody holds or waits for it.
pub fn try_lock() -> bool {
    let serving = LOCK.serving.load(Ordering::Acquire);
    if LOCK
        .next
        .compare_exchange(
            serving,
            serving.wrapping_add(1),
            Ordering::AcqRel,
            Ordering::Relaxed,
        )
        .is_err()
    {
        TRY_FAILED.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    ACQUISITIONS.fetch_add(1, Ordering::Relaxed);
    LOCK.owner.store(this_cpu() as u32 + 1, Ordering::Release);
    true
}

pub fn unlock() {
    LOCK.owner.store(0, Ordering::Release);
    LOCK.serving.fetch_add(1, Ordering::AcqRel);
}

/// `(serving, next, owner)` of the lock, for diagnostics.
#[cfg(feature = "boot-test")]
pub fn snapshot() -> (u32, u32, u32) {
    (
        LOCK.serving.load(Ordering::Acquire),
        LOCK.next.load(Ordering::Acquire),
        LOCK.owner.load(Ordering::Acquire),
    )
}

/// Whether another processor is queued for the lock.
pub fn has_waiters() -> bool {
    LOCK.next
        .load(Ordering::Acquire)
        .wrapping_sub(LOCK.serving.load(Ordering::Acquire))
        > 1
}

/// Whether the processor this runs on holds the lock.
pub fn held_by_me() -> bool {
    LOCK.owner.load(Ordering::Acquire) == this_cpu() as u32 + 1
}

/// The address of the lock, for the assembly that releases it just before the
/// `iretq` into user mode.
pub fn release_address() -> *mut u32 {
    &LOCK as *const BigLock as *mut u32
}

/// Gives the lock up for a moment so another processor can enter the kernel.
/// The caller must re-establish its own task afterwards.
pub fn relax() {
    RELAXES.fetch_add(1, Ordering::Relaxed);
    unlock();
    for _ in 0..64 {
        core::hint::spin_loop();
    }
    lock();
}

pub fn note_entry(cpu: usize) {
    if let Some(entry) = ENTRIES.get(cpu) {
        entry.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy)]
pub struct LockStats {
    pub acquisitions: u64,
    pub contended: u64,
    pub spins: u64,
    pub relaxes: u64,
    pub try_failed: u64,
    pub entries: [u64; MAX_PROCESSORS],
}

pub fn stats() -> LockStats {
    let mut entries = [0u64; MAX_PROCESSORS];
    for (slot, entry) in entries.iter_mut().zip(&ENTRIES) {
        *slot = entry.load(Ordering::Relaxed);
    }
    LockStats {
        acquisitions: ACQUISITIONS.load(Ordering::Relaxed),
        contended: CONTENDED.load(Ordering::Relaxed),
        spins: SPINS.load(Ordering::Relaxed),
        relaxes: RELAXES.load(Ordering::Relaxed),
        try_failed: TRY_FAILED.load(Ordering::Relaxed),
        entries,
    }
}
