//! Loading-screen model: what is loading, how far along it is, and what to
//! say about it. It holds no drawing code - `desktop::draw_loading` renders a
//! `LoadingView` (that is where the real UI goes) - so any part of the system
//! can report progress the same way.

/// What the loading screen is for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LoadingKind {
    /// AerOS itself starting up (not wired to the boot path yet).
    #[allow(dead_code)]
    System,
    /// The Linux guest starting (its window or seamless mode was opened
    /// before its desktop was up).
    Linux,
}

/// Everything the UI needs for one frame of a loading screen.
#[derive(Clone, Copy)]
pub struct LoadingView {
    #[allow(dead_code)] // the current design doesn't show them (yet)
    pub kind: LoadingKind,
    /// Short name of the current step, e.g. "Starting the Linux desktop".
    #[allow(dead_code)]
    pub stage: &'static str,
    /// 0..=1000; an estimate, not a promise.
    pub progress_permille: u16,
    /// Seconds since loading began (for spinners, "taking longer than
    /// usual" hints and so on).
    pub elapsed_secs: u32,
}

impl LoadingView {
    /// Progress as a whole percentage (for the UI to use).
    #[allow(dead_code)]
    pub fn percent(&self) -> u32 {
        self.progress_permille as u32 / 10
    }

    /// Loading has run for a suspiciously long time.
    pub fn is_slow(&self) -> bool {
        self.elapsed_secs > 90
    }
}

/// The Linux guest's loading state, derived from what the hypervisor and the
/// guest agent have published so far. `None` once its desktop is up (the
/// windows table exists), or when nothing asked for Linux.
pub fn linux_view(started_ns: u64, now_ns: u64) -> Option<LoadingView> {
    if crate::svm::linux_windows().is_some() {
        return None;
    }
    let elapsed_secs = (now_ns.saturating_sub(started_ns) / 1_000_000_000) as u32;
    let (stage, floor) = if !crate::svm::linux_ready() {
        ("Preparing Linux", 0)
    } else if elapsed_secs < 8 {
        ("Starting the kernel", 60)
    } else if elapsed_secs < 25 {
        ("Starting services", 300)
    } else {
        ("Starting the desktop", 600)
    };
    // Creeps towards 95% so the bar never looks stuck or finished early;
    // 60 s is about a normal start.
    let creep = (elapsed_secs.min(60) * 950 / 60) as u16;
    Some(LoadingView {
        kind: LoadingKind::Linux,
        stage,
        progress_permille: creep.max(floor).min(950),
        elapsed_secs,
    })
}

/// Tips shown under the logo, rotated every few seconds.
const TIPS: [&str; 1] = ["Tip: [Insert Tip here]"];

/// The tip to show after `elapsed_secs` of loading.
pub fn tip(elapsed_secs: u32) -> &'static str {
    TIPS[(elapsed_secs / 8) as usize % TIPS.len()]
}
