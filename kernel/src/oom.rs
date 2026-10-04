//! Memory-pressure levels and a minimal out-of-memory policy: when a process
//! cannot get memory while the system is critically low, the newest other
//! user task is terminated to release its pages.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::memory::{TRACKED_FREE_PAGES, TRACKED_TOTAL_PAGES};

static KILLS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pressure {
    Normal,
    Low,
    Critical,
}

impl Pressure {
    pub fn label(self) -> &'static str {
        match self {
            Pressure::Normal => "normal",
            Pressure::Low => "low",
            Pressure::Critical => "critical",
        }
    }
}

/// Low at 10% free or less, critical at 3% free or less.
pub fn classify(free_pages: u64, total_pages: u64) -> Pressure {
    if total_pages == 0 {
        return Pressure::Normal;
    }
    if free_pages.saturating_mul(100) <= total_pages.saturating_mul(3) {
        Pressure::Critical
    } else if free_pages.saturating_mul(10) <= total_pages {
        Pressure::Low
    } else {
        Pressure::Normal
    }
}

pub fn pressure() -> Pressure {
    classify(
        TRACKED_FREE_PAGES.load(Ordering::Relaxed),
        TRACKED_TOTAL_PAGES.load(Ordering::Relaxed),
    )
}

pub fn kills() -> u64 {
    KILLS.load(Ordering::Relaxed)
}

/// The task other than `current` holding the most pages (newest wins a tie);
/// task 0 is the kernel itself and is never chosen.
pub fn select_victim(tasks: &[(u64, u64)], current: u64) -> Option<u64> {
    tasks
        .iter()
        .copied()
        .filter(|(id, _)| *id != 0 && *id != current)
        .max_by_key(|(id, pages)| (*pages, *id))
        .map(|(id, _)| id)
}

/// Called when an allocation for a process failed. Does nothing unless
/// pressure is critical; otherwise terminates the largest other user task.
pub fn relieve() -> Option<u64> {
    if pressure() != Pressure::Critical {
        return None;
    }
    let released = crate::block::reclaim(usize::MAX) + crate::slab::reclaim();
    if released > 0 && pressure() != Pressure::Critical {
        return None;
    }
    if crate::scheduler::swap_out_pages(256) > 0 && pressure() != Pressure::Critical {
        return None;
    }
    let current = crate::scheduler::current_task_id();
    let mut tasks = [(0u64, 0u64); 32];
    let mut count = 0;
    crate::scheduler::list_tasks(|task| {
        if count < tasks.len() {
            tasks[count] = (task.id, task.resident_pages);
            count += 1;
        }
    });
    let victim = select_victim(&tasks[..count], current)?;
    if crate::process::request_exit(victim, 128 + 9) {
        KILLS.fetch_add(1, Ordering::Relaxed);
        crate::serial::format(format_args!("AEROS_OOM_KILL task={victim}\n"));
        Some(victim)
    } else {
        None
    }
}

pub fn self_test() -> bool {
    let levels = classify(100, 1000) == Pressure::Low
        && classify(30, 1000) == Pressure::Critical
        && classify(101, 1000) == Pressure::Normal
        && classify(0, 0) == Pressure::Normal
        && classify(0, 1000) == Pressure::Critical;
    let victim = select_victim(&[(0, 50), (3, 10), (9, 10), (5, 40)], 4) == Some(5)
        && select_victim(&[(3, 7), (8, 7)], 4) == Some(8)
        && select_victim(&[(0, 9), (9, 9)], 9).is_none()
        && select_victim(&[], 1).is_none();
    let quiet = classify(
        TRACKED_FREE_PAGES.load(Ordering::Relaxed),
        TRACKED_TOTAL_PAGES.load(Ordering::Relaxed),
    ) != Pressure::Critical;
    levels && victim && quiet && relieve().is_none()
}
