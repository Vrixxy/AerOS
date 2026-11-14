//! Face-tracking seam (the "memoji" half). Implement `FaceTracker` for a
//! webcam backend (e.g. MediaPipe / ONNX face-mesh -> blendshapes) and feed
//! its output to `Command::SetExpression` / `Command::SetPose`.

use crate::fmath::sin;
use crate::rig::{Expression, Pose};

#[derive(Clone, Copy, Debug)]
pub struct TrackedFace {
    pub expression: Expression,
    pub pose: Pose,
}

pub trait FaceTracker {
    /// Latest tracked face, or `None` when no face is visible / no new frame.
    fn poll(&mut self) -> Option<TrackedFace>;
}

/// Placeholder that sweeps head yaw so the pipeline can be tested without a camera.
#[derive(Default)]
pub struct DemoTracker {
    t: f32,
}

impl FaceTracker for DemoTracker {
    fn poll(&mut self) -> Option<TrackedFace> {
        self.t += 1.0 / 30.0;
        Some(TrackedFace {
            expression: Expression::NEUTRAL.lerp(&Expression::HAPPY, (sin(self.t) + 1.0) / 2.0),
            pose: Pose {
                yaw: sin(self.t) * 0.6,
                pitch: sin(self.t * 0.7) * 0.3,
                roll: 0.0,
            },
        })
    }
}
