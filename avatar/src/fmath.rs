//! Float helpers for `no_std` (core has no floor/sqrt/sin without libm).

use core::f32::consts::{PI, TAU};

pub fn abs(x: f32) -> f32 {
    if x < 0.0 { -x } else { x }
}

pub fn floor(x: f32) -> f32 {
    let i = x as i32 as f32;
    if i > x { i - 1.0 } else { i }
}

pub fn ceil(x: f32) -> f32 {
    let i = x as i32 as f32;
    if i < x { i + 1.0 } else { i }
}

pub fn sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut g = if x > 1.0 { x * 0.5 } else { 1.0 };
    for _ in 0..24 {
        g = 0.5 * (g + x / g);
    }
    g
}

/// Parabolic approximation with one refinement step; error about 1e-3.
pub fn sin(x: f32) -> f32 {
    let x = x - TAU * floor((x + PI) / TAU);
    let y = (4.0 / PI) * x - (4.0 / (PI * PI)) * x * abs(x);
    0.225 * (y * abs(y) - y) + y
}

pub fn cos(x: f32) -> f32 {
    sin(x + PI / 2.0)
}
