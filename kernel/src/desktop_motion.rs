//! Spring physics for every moving thing on the desktop. A motion is a mass
//! on a spring (stiffness, damping) integrated in 1 ms steps from the moment
//! it was last aimed, so any frame can read where it is and how fast it is
//! going, and aiming it somewhere new mid-flight keeps both.

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Spring {
    stiffness: i64,
    damping: i64,
}

/// Quick with a little overshoot, for panels growing out of a button.
pub(super) const SNAPPY: Spring = Spring {
    stiffness: 380,
    damping: 30,
};
/// Settles without bouncing, for things going away.
pub(super) const SETTLE: Spring = Spring {
    stiffness: 300,
    damping: 36,
};

const UNIT: i64 = 1_000;
const MAX_STEPS: u64 = 3_000;

#[derive(Clone, Copy)]
pub(super) struct Motion {
    from: i64,
    velocity: i64,
    target: i64,
    start_ns: u64,
    spring: Spring,
}

impl Motion {
    /// At rest at `value` (thousandths).
    pub(super) const fn at(value: i32) -> Self {
        Self {
            from: value as i64 * UNIT,
            velocity: 0,
            target: value as i64 * UNIT,
            start_ns: 0,
            spring: SNAPPY,
        }
    }

    /// Position (millionths) and velocity (millionths per second) at `now`,
    /// and whether it has come to rest.
    fn sample(&self, now_ns: u64) -> (i64, i64, bool) {
        let steps = (now_ns.saturating_sub(self.start_ns) / 1_000_000).min(MAX_STEPS);
        let (mut x, mut v) = (self.from, self.velocity);
        for _ in 0..steps {
            if (x - self.target).abs() < 300 && v.abs() < 30_000 {
                return (self.target, 0, true);
            }
            let a = -self.spring.stiffness * (x - self.target) - self.spring.damping * v;
            v += a / 1_000;
            x += v / 1_000;
        }
        let rest = (x - self.target).abs() < 300 && v.abs() < 30_000;
        (if rest { self.target } else { x }, v, rest || steps == MAX_STEPS)
    }

    /// Where it is now, in thousandths (may overshoot the target).
    pub(super) fn value(&self, now_ns: u64) -> i32 {
        (self.sample(now_ns).0 / UNIT) as i32
    }

    pub(super) fn settled(&self, now_ns: u64) -> bool {
        self.sample(now_ns).2
    }

    /// Heads for `target` from wherever it is, keeping its current speed.
    pub(super) fn aim(&mut self, target: i32, spring: Spring, now_ns: u64) {
        let (x, v, _) = self.sample(now_ns);
        *self = Self {
            from: x,
            velocity: v,
            target: target as i64 * UNIT,
            start_ns: now_ns,
            spring,
        };
    }
}

/// A spring released from rest at 0 towards 1000, read at `progress`
/// (0..=1000) of its settling time; `damping` is the damping ratio in
/// thousandths (1000 = critical, lower overshoots). Lets the timed
/// animations follow spring physics instead of fixed easing curves.
pub(super) fn spring_curve(progress: u32, damping: i64) -> i32 {
    if progress >= 1_000 {
        return 1_000;
    }
    let (mut x, mut v) = (0i64, 0i64);
    for _ in 0..progress {
        let a = 81 * (1_000_000 - x) - 18 * damping * v / 1_000;
        v += a / 1_000;
        x += v / 1_000;
    }
    (x / 1_000) as i32
}

/// Quick and firm, for things following a click (toggles, highlights).
pub(super) const QUICK: Spring = Spring {
    stiffness: 700,
    damping: 46,
};

#[derive(Clone, Copy)]
struct Tween {
    key: u64,
    target: i32,
    motion: Motion,
    used_ns: u64,
}

static TWEENS: crate::sync::TicketLock<[Tween; 64]> = crate::sync::TicketLock::new(
    [Tween {
        key: 0,
        target: 0,
        motion: Motion::at(0),
        used_ns: 0,
    }; 64],
);
/// Frames keep coming until then: some tween is still moving.
static TWEENS_UNTIL_NS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Implicit animation: whatever a frame draws under `key` springs from the
/// last value it drew to `target` instead of jumping there. The first time
/// a key is seen it starts at rest on its target. Callers make keys unique
/// with `tween_key`.
pub(super) fn tween(key: u64, target: i32, spring: Spring, now_ns: u64) -> i32 {
    let mut table = TWEENS.lock();
    if let Some(entry) = table.iter_mut().find(|entry| entry.used_ns != 0 && entry.key == key) {
        if entry.target != target {
            entry.motion.aim(target, spring, now_ns);
            entry.target = target;
        }
        entry.used_ns = now_ns.max(1);
        if !entry.motion.settled(now_ns) {
            TWEENS_UNTIL_NS.fetch_max(now_ns + 400_000_000, core::sync::atomic::Ordering::Relaxed);
        }
        return entry.motion.value(now_ns);
    }
    if let Some(slot) = table.iter_mut().min_by_key(|entry| entry.used_ns) {
        *slot = Tween {
            key,
            target,
            motion: Motion::at(target),
            used_ns: now_ns.max(1),
        };
    }
    target
}

/// `tween` for an on/off state: 0 (off) to 1000 (on).
pub(super) fn tween_bool(key: u64, on: bool, now_ns: u64) -> i32 {
    tween(key, if on { 1_000 } else { 0 }, QUICK, now_ns)
}

pub(super) const fn tween_key(kind: u32, index: u32) -> u64 {
    ((kind as u64) << 32) | index as u64
}

pub(super) fn tweens_moving(now_ns: u64) -> bool {
    now_ns < TWEENS_UNTIL_NS.load(core::sync::atomic::Ordering::Relaxed)
}

/// 0 -> 1000 along a quick spring each time `value` drawn under `key`
/// changes (1000 while it stays put): drives content that fades or rises
/// in when a page, tab or folder switches.
pub(super) fn entrance(key: u64, value: i32, now_ns: u64) -> i32 {
    let mut table = TWEENS.lock();
    let key = key ^ 0x8000_0000_0000_0000;
    if let Some(entry) = table.iter_mut().find(|entry| entry.used_ns != 0 && entry.key == key) {
        if entry.target != value {
            entry.target = value;
            entry.motion = Motion::at(0);
            entry.motion.aim(1_000, QUICK, now_ns);
        }
        entry.used_ns = now_ns.max(1);
        if !entry.motion.settled(now_ns) {
            TWEENS_UNTIL_NS.fetch_max(now_ns + 400_000_000, core::sync::atomic::Ordering::Relaxed);
        }
        return entry.motion.value(now_ns).clamp(0, 1_000);
    }
    if let Some(slot) = table.iter_mut().min_by_key(|entry| entry.used_ns) {
        *slot = Tween {
            key,
            target: value,
            motion: Motion::at(1_000),
            used_ns: now_ns.max(1),
        };
    }
    1_000
}

/// The share of an `entrance` that item `index` of a list has made: later
/// items start a little later, so a list cascades in instead of popping.
pub(super) fn stagger(progress: i32, index: usize) -> i32 {
    let delay = (index as i32 * 55).min(450);
    ((progress - delay) * 1_000 / (1_000 - delay)).clamp(0, 1_000)
}

/// Keeps frames coming until `until_ns` for a timed effect outside the
/// tween table.
pub(super) fn hold(until_ns: u64) {
    TWEENS_UNTIL_NS.fetch_max(until_ns, core::sync::atomic::Ordering::Relaxed);
}
