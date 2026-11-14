//! Idle animation (blink, breathing sway) and expression tweening.

use crate::fmath::sin;
use crate::rig::{Expression, Pose};

pub struct Idle {
    t: f32,
    next_blink: f32,
    blink_start: Option<f32>,
}

impl Default for Idle {
    fn default() -> Self {
        Idle {
            t: 0.0,
            next_blink: 2.5,
            blink_start: None,
        }
    }
}

impl Idle {
    /// Advance by `dt` seconds and layer idle motion on top of `base`.
    pub fn tick(&mut self, dt: f32, base: &Expression, pose: &Pose) -> (Expression, Pose) {
        self.t += dt;
        if self.blink_start.is_none() && self.t >= self.next_blink {
            self.blink_start = Some(self.t);
        }
        let mut ex = *base;
        if let Some(s) = self.blink_start {
            let p = (self.t - s) / 0.18;
            if p >= 1.0 {
                self.blink_start = None;
                self.next_blink = self.t + 2.0 + crate::fmath::abs(sin(self.t * 7.3)) * 3.0;
            } else {
                let k = 1.0 - sin(p * core::f32::consts::PI);
                ex.eye_open_l *= k;
                ex.eye_open_r *= k;
            }
        }
        let mut p = *pose;
        p.roll += sin(self.t * 0.9) * 0.04;
        p.pitch += sin(self.t * 1.3) * 0.03;
        (ex, p)
    }
}

/// Eases from one expression to another over `duration` seconds.
pub struct Tween {
    from: Expression,
    to: Expression,
    t: f32,
    duration: f32,
}

impl Tween {
    pub fn new(from: Expression, to: Expression, duration: f32) -> Self {
        Tween {
            from,
            to,
            t: 0.0,
            duration: duration.max(1e-3),
        }
    }
    pub fn tick(&mut self, dt: f32) -> Expression {
        self.t = (self.t + dt).min(self.duration);
        let k = self.t / self.duration;
        self.from.lerp(&self.to, k * k * (3.0 - 2.0 * k))
    }
    pub fn done(&self) -> bool {
        self.t >= self.duration
    }
}
