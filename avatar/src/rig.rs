/// Blendshape-style face controls. Ranges noted per field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Expression {
    /// 0 closed .. 1 open
    pub eye_open_l: f32,
    pub eye_open_r: f32,
    /// -1 lowered .. 1 raised
    pub brow_l: f32,
    pub brow_r: f32,
    /// 0 relaxed .. 1 furrowed (angry)
    pub brow_furrow: f32,
    /// -1 frown .. 1 smile
    pub smile: f32,
    /// 0 closed .. 1 wide open
    pub mouth_open: f32,
    /// pupil offset, -1..1
    pub gaze_x: f32,
    pub gaze_y: f32,
}

impl Default for Expression {
    fn default() -> Self {
        Expression::NEUTRAL
    }
}

impl Expression {
    pub const NEUTRAL: Expression = Expression {
        eye_open_l: 1.0,
        eye_open_r: 1.0,
        brow_l: 0.0,
        brow_r: 0.0,
        brow_furrow: 0.0,
        smile: 0.15,
        mouth_open: 0.0,
        gaze_x: 0.0,
        gaze_y: 0.0,
    };

    pub const HAPPY: Expression = Expression {
        smile: 0.9,
        brow_l: 0.2,
        brow_r: 0.2,
        eye_open_l: 0.85,
        eye_open_r: 0.85,
        ..Self::NEUTRAL
    };
    pub const LAUGH: Expression = Expression {
        smile: 1.0,
        mouth_open: 0.7,
        brow_l: 0.4,
        brow_r: 0.4,
        eye_open_l: 0.3,
        eye_open_r: 0.3,
        ..Self::NEUTRAL
    };
    pub const SAD: Expression = Expression {
        smile: -0.7,
        brow_l: 0.5,
        brow_r: 0.5,
        brow_furrow: 0.3,
        eye_open_l: 0.8,
        eye_open_r: 0.8,
        gaze_y: 0.4,
        ..Self::NEUTRAL
    };
    pub const ANGRY: Expression = Expression {
        smile: -0.5,
        brow_l: -0.5,
        brow_r: -0.5,
        brow_furrow: 1.0,
        eye_open_l: 0.75,
        eye_open_r: 0.75,
        ..Self::NEUTRAL
    };
    pub const SURPRISED: Expression = Expression {
        smile: 0.0,
        mouth_open: 0.8,
        brow_l: 1.0,
        brow_r: 1.0,
        eye_open_l: 1.0,
        eye_open_r: 1.0,
        ..Self::NEUTRAL
    };
    pub const WINK: Expression = Expression {
        eye_open_r: 0.0,
        smile: 0.8,
        brow_r: 0.3,
        ..Self::NEUTRAL
    };

    pub const PRESETS: &'static [(&'static str, Expression)] = &[
        ("neutral", Self::NEUTRAL),
        ("happy", Self::HAPPY),
        ("laugh", Self::LAUGH),
        ("sad", Self::SAD),
        ("angry", Self::ANGRY),
        ("surprised", Self::SURPRISED),
        ("wink", Self::WINK),
    ];

    pub fn preset(name: &str) -> Option<Expression> {
        Self::PRESETS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, e)| *e)
    }

    pub fn lerp(&self, o: &Expression, t: f32) -> Expression {
        let l = |a: f32, b: f32| a + (b - a) * t;
        Expression {
            eye_open_l: l(self.eye_open_l, o.eye_open_l),
            eye_open_r: l(self.eye_open_r, o.eye_open_r),
            brow_l: l(self.brow_l, o.brow_l),
            brow_r: l(self.brow_r, o.brow_r),
            brow_furrow: l(self.brow_furrow, o.brow_furrow),
            smile: l(self.smile, o.smile),
            mouth_open: l(self.mouth_open, o.mouth_open),
            gaze_x: l(self.gaze_x, o.gaze_x),
            gaze_y: l(self.gaze_y, o.gaze_y),
        }
    }
}

/// Head orientation, each axis -1..1 (roughly +-30 degrees).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pose {
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
}
