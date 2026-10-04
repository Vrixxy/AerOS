//! Job scheduling for the application processors: every AP has its own run
//! queue, jobs carry a nice value and a CPU affinity mask, a new job goes to
//! the least loaded CPU its mask allows, and an AP with nothing to run takes
//! the best job it is allowed to run from the busiest queue. Jobs run to
//! completion with interrupts enabled; the user-task scheduler stays on the
//! bootstrap processor, which owns all per-process Linux state.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::acpi::MAX_PROCESSORS;
use crate::sync::TicketLock;

pub const MAX_JOBS: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Free,
    Queued,
    Running,
    Done,
}

#[derive(Clone, Copy)]
struct Job {
    state: State,
    id: u32,
    function: Option<fn(u64)>,
    argument: u64,
    nice: i8,
    affinity: u64,
    cpu: u8,
    waited: u8,
    ran_on: u8,
}

impl Job {
    const EMPTY: Job = Job {
        state: State::Free,
        id: 0,
        function: None,
        argument: 0,
        nice: 0,
        affinity: 0,
        cpu: 0,
        waited: 0,
        ran_on: 0,
    };
}

struct Table {
    jobs: [Job; MAX_JOBS],
    next_id: u32,
    paused: u64,
    executed: [u64; MAX_PROCESSORS],
    steals: u64,
}

static TABLE: TicketLock<Table> = TicketLock::new(Table {
    jobs: [Job::EMPTY; MAX_JOBS],
    next_id: 1,
    paused: 0,
    executed: [0; MAX_PROCESSORS],
    steals: 0,
});

static SPAWNED: AtomicU64 = AtomicU64::new(0);

/// The CPU in `allowed` with the fewest queued or running jobs (lowest number
/// on a tie).
fn choose_cpu(loads: &[u16], allowed: u64) -> Option<usize> {
    loads
        .iter()
        .enumerate()
        .filter(|(cpu, _)| allowed & (1 << cpu) != 0)
        .min_by_key(|(cpu, load)| (**load, *cpu))
        .map(|(cpu, _)| cpu)
}

/// The CPU a starving `thief` should take from: the one with the most queued
/// jobs, as long as that is more than one.
fn steal_victim(queued: &[u16], thief: usize) -> Option<usize> {
    queued
        .iter()
        .enumerate()
        .filter(|(cpu, count)| *cpu != thief && **count > 1)
        .max_by_key(|(cpu, count)| (**count, core::cmp::Reverse(*cpu)))
        .map(|(cpu, _)| cpu)
}

fn queue_lengths(table: &Table) -> ([u16; MAX_PROCESSORS], [u16; MAX_PROCESSORS]) {
    let mut queued = [0u16; MAX_PROCESSORS];
    let mut load = [0u16; MAX_PROCESSORS];
    for job in &table.jobs {
        match job.state {
            State::Queued => {
                queued[job.cpu as usize] += 1;
                load[job.cpu as usize] += 1;
            }
            State::Running => load[job.cpu as usize] += 1,
            _ => {}
        }
    }
    (queued, load)
}

fn executors() -> u64 {
    crate::smp::online_mask() & !1
}

/// Queues `function(argument)` on a CPU in `affinity` (bit n = logical CPU n;
/// the bootstrap processor never runs jobs). `None` when no application
/// processor is allowed or the table is full.
pub fn spawn(function: fn(u64), argument: u64, nice: i8, affinity: u64) -> Option<u32> {
    let allowed = executors() & affinity;
    if allowed == 0 {
        return None;
    }
    let (id, cpu) = {
        let mut table = TABLE.lock();
        let (_, load) = queue_lengths(&table);
        let cpu = choose_cpu(&load, allowed)?;
        let id = table.next_id;
        let slot = table.jobs.iter_mut().find(|job| job.state == State::Free)?;
        *slot = Job {
            state: State::Queued,
            id,
            function: Some(function),
            argument,
            nice: nice.clamp(-20, 19),
            affinity,
            cpu: cpu as u8,
            waited: 0,
            ran_on: 0,
        };
        table.next_id = table.next_id.wrapping_add(1).max(1);
        (id, cpu)
    };
    SPAWNED.fetch_add(1, Ordering::Relaxed);
    crate::smp::wake(cpu);
    Some(id)
}

/// Waits (up to `timeout_ns`) for a job to finish, frees its slot and returns
/// the CPU it ran on.
pub fn wait(id: u32, timeout_ns: u64) -> Option<usize> {
    let start = crate::time::monotonic_nanoseconds();
    loop {
        {
            let mut table = TABLE.lock();
            if let Some(job) = table
                .jobs
                .iter_mut()
                .find(|job| job.id == id && job.state != State::Free)
                && job.state == State::Done
            {
                let cpu = job.ran_on as usize;
                *job = Job::EMPTY;
                return Some(cpu);
            }
        }
        if crate::time::monotonic_nanoseconds().wrapping_sub(start) >= timeout_ns {
            return None;
        }
        core::hint::spin_loop();
    }
}

/// Called from an application processor's idle loop: runs the best queued job
/// for `cpu`, taking one from a busier CPU's queue when its own is empty.
/// Returns whether a job ran.
pub fn run_next(cpu: usize) -> bool {
    let picked = {
        let mut table = TABLE.lock();
        if cpu >= MAX_PROCESSORS || table.paused & (1 << cpu) != 0 {
            return false;
        }
        let index = pick(&mut table, cpu).or_else(|| steal(&mut table, cpu));
        index.map(|index| {
            let job = &mut table.jobs[index];
            job.state = State::Running;
            job.cpu = cpu as u8;
            (index, job.function, job.argument)
        })
    };
    let Some((index, function, argument)) = picked else {
        return false;
    };
    crate::arch::enable_interrupts();
    if let Some(function) = function {
        function(argument);
    }
    crate::arch::disable_interrupts();
    let mut table = TABLE.lock();
    table.executed[cpu] += 1;
    let job = &mut table.jobs[index];
    job.state = State::Done;
    job.ran_on = cpu as u8;
    true
}

fn pick(table: &mut Table, cpu: usize) -> Option<usize> {
    let mut candidates = [(0usize, 0i8, 0u8); MAX_JOBS];
    let mut count = 0;
    for (index, job) in table.jobs.iter().enumerate() {
        if job.state == State::Queued && job.cpu as usize == cpu {
            candidates[count] = (index, job.nice, job.waited);
            count += 1;
        }
    }
    let chosen = crate::scheduler::pick_weighted(&candidates[..count])?;
    for &(index, _, _) in &candidates[..count] {
        if index != chosen {
            table.jobs[index].waited = table.jobs[index].waited.saturating_add(1);
        }
    }
    Some(chosen)
}

fn steal(table: &mut Table, thief: usize) -> Option<usize> {
    let (queued, _) = queue_lengths(table);
    let victim = steal_victim(&queued, thief)?;
    let mut candidates = [(0usize, 0i8, 0u8); MAX_JOBS];
    let mut count = 0;
    for (index, job) in table.jobs.iter().enumerate() {
        if job.state == State::Queued
            && job.cpu as usize == victim
            && job.affinity & (1 << thief) != 0
        {
            candidates[count] = (index, job.nice, job.waited);
            count += 1;
        }
    }
    let chosen = crate::scheduler::pick_weighted(&candidates[..count])?;
    table.jobs[chosen].cpu = thief as u8;
    table.steals += 1;
    Some(chosen)
}

/// Jobs each processor has finished, for `/proc/schedstat`.
pub fn jobs_executed(cpu: usize) -> u64 {
    TABLE.lock().executed.get(cpu).copied().unwrap_or(0)
}

pub fn steals() -> u64 {
    TABLE.lock().steals
}

#[cfg(feature = "boot-test")]
fn set_paused(mask: u64, paused: bool) {
    let mut table = TABLE.lock();
    if paused {
        table.paused |= mask;
    } else {
        table.paused &= !mask;
    }
}

#[cfg(feature = "boot-test")]
fn queued_on(cpu: usize) -> u16 {
    let table = TABLE.lock();
    queue_lengths(&table).0[cpu]
}

#[cfg(feature = "boot-test")]
fn wake_all() {
    for cpu in 1..MAX_PROCESSORS {
        if executors() & (1 << cpu) != 0 {
            crate::smp::wake(cpu);
        }
    }
}

#[cfg(feature = "boot-test")]
static LOG: [AtomicU64; 32] = [const { AtomicU64::new(0) }; 32];
#[cfg(feature = "boot-test")]
static LOG_LENGTH: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "boot-test")]
static CPU_SEEN: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "boot-test")]
fn logging_job(argument: u64) {
    let position = LOG_LENGTH.fetch_add(1, Ordering::AcqRel) as usize;
    if let Some(slot) = LOG.get(position) {
        slot.store(argument, Ordering::Release);
    }
}

#[cfg(feature = "boot-test")]
fn location_job(_: u64) {
    if let Some(cpu) = crate::smp::current_logical() {
        CPU_SEEN.fetch_or(1 << cpu, Ordering::AcqRel);
    }
}

#[cfg(feature = "boot-test")]
#[derive(Clone, Copy, Default)]
pub struct Report {
    pub cpus: usize,
    pub placement: bool,
    pub stealing: bool,
    pub priority_order: bool,
    pub affinity: bool,
    pub refused: bool,
    pub balanced: bool,
    pub work_stolen: bool,
    pub verified: bool,
}

#[cfg(feature = "boot-test")]
pub fn self_test() -> Report {
    let mut report = Report {
        cpus: crate::smp::online_mask().count_ones() as usize,
        ..Report::default()
    };
    report.placement = choose_cpu(&[0, 3, 1, 1], 0b1110) == Some(2)
        && choose_cpu(&[0, 3, 1, 1], 0b1000) == Some(3)
        && choose_cpu(&[0, 3, 1, 1], 0).is_none()
        && choose_cpu(&[], 1).is_none();
    report.stealing = steal_victim(&[0, 4, 2, 0], 3) == Some(1)
        && steal_victim(&[0, 1, 1, 0], 3).is_none()
        && steal_victim(&[5, 0, 0, 0], 0).is_none()
        && steal_victim(&[3, 3, 0, 0], 2) == Some(0);
    let aps = executors();
    if aps == 0 {
        report.priority_order = true;
        report.affinity = true;
        report.refused = spawn(logging_job, 0, 0, u64::MAX).is_none();
        report.balanced = true;
        report.work_stolen = true;
        report.verified = report.placement && report.stealing && report.refused;
        return report;
    }
    let first = aps.trailing_zeros() as usize;
    let only_first = 1u64 << first;

    set_paused(aps, true);
    LOG_LENGTH.store(0, Ordering::Release);
    let nices: [i8; 8] = [4, -20, 19, -5, 12, 0, 8, -10];
    let mut ids = [0u32; 8];
    let mut spawned = true;
    for (slot, nice) in ids.iter_mut().zip(nices) {
        match spawn(logging_job, (nice + 20) as u64, nice, only_first) {
            Some(id) => *slot = id,
            None => spawned = false,
        }
    }
    let queued_together = queued_on(first) == 8;
    set_paused(aps, false);
    wake_all();
    let mut finished = true;
    for id in ids {
        finished &= wait(id, 2_000_000_000) == Some(first);
    }
    let mut expected = nices;
    expected.sort_unstable();
    let order_ok = LOG_LENGTH.load(Ordering::Acquire) == 8
        && LOG
            .iter()
            .take(8)
            .zip(expected)
            .all(|(slot, nice)| slot.load(Ordering::Acquire) == (nice + 20) as u64);
    report.priority_order = spawned && queued_together && finished && order_ok;

    CPU_SEEN.store(0, Ordering::Release);
    let pinned = spawn(location_job, 0, 0, only_first);
    let pinned_cpu = pinned.and_then(|id| wait(id, 2_000_000_000));
    report.affinity = pinned_cpu == Some(first) && CPU_SEEN.load(Ordering::Acquire) == only_first;
    report.refused = spawn(logging_job, 0, 0, 1).is_none() && spawn(logging_job, 0, 0, 0).is_none();

    if aps.count_ones() < 2 {
        report.balanced = true;
        report.work_stolen = true;
    } else {
        let second = (aps & !only_first).trailing_zeros() as usize;
        set_paused(aps, true);
        let mut jobs = [0u32; 12];
        let mut all = true;
        for slot in jobs.iter_mut() {
            match spawn(location_job, 0, 0, only_first | (1 << second)) {
                Some(id) => *slot = id,
                None => all = false,
            }
        }
        let (a, b) = (queued_on(first), queued_on(second));
        report.balanced = all && a + b == 12 && a.abs_diff(b) <= 1;
        let steals_before = steals();
        let moved = {
            let mut table = TABLE.lock();
            let mut moved = 0;
            for job in table.jobs.iter_mut() {
                if job.state == State::Queued && job.cpu as usize == second && moved < 6 {
                    job.cpu = first as u8;
                    moved += 1;
                }
            }
            moved
        };
        let _ = moved;
        set_paused(1 << second, false);
        wake_all();
        let mut done = true;
        let mut helped = false;
        let started = crate::time::monotonic_nanoseconds();
        while queued_on(first) != 0
            && crate::time::monotonic_nanoseconds().wrapping_sub(started) < 1_000_000_000
        {
            core::hint::spin_loop();
        }
        set_paused(aps, false);
        wake_all();
        for id in jobs {
            match wait(id, 2_000_000_000) {
                Some(cpu) => helped |= cpu == second,
                None => done = false,
            }
        }
        report.work_stolen = done && helped && steals() > steals_before;
    }
    report.verified = report.placement
        && report.stealing
        && report.priority_order
        && report.affinity
        && report.refused
        && report.balanced
        && report.work_stolen;
    report
}
