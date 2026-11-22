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
