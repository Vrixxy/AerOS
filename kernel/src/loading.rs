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
    /// Seconds since loading began ("taking longer than usual" hints).
    pub elapsed_secs: u32,
    /// Milliseconds since loading began (for animation).
    pub elapsed_ms: u64,
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
    if crate::svm::linux_windows().is_some_and(|windows| windows.count > 0) {
        return None;
    }
    let elapsed_ms = now_ns.saturating_sub(started_ns) / 1_000_000;
    let elapsed_secs = (elapsed_ms / 1000) as u32;
    let stage = if !crate::svm::linux_ready() {
        "Preparing Linux"
    } else if elapsed_secs < 8 {
        "Starting the kernel"
    } else if elapsed_secs < 25 {
        "Starting services"
    } else {
        "Starting the desktop"
    };
    Some(LoadingView {
        kind: LoadingKind::Linux,
        stage,
        progress_permille: creeping_progress(elapsed_ms),
        elapsed_secs,
        elapsed_ms,
    })
}

/// A smooth estimate: eases out towards 95% over about a minute (a normal
/// start), so the bar keeps moving without ever looking finished early.
pub fn creeping_progress(elapsed_ms: u64) -> u16 {
    let progress = (elapsed_ms.min(60_000) * 1000 / 60_000) as u32;
    let inverse = 1000 - progress;
    let eased = 1000 - inverse * inverse / 1000;
    (eased * 950 / 1000) as u16
}
