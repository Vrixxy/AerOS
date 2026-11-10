/// Cursor following state. Everything eases with plain exponential smoothing, so nothing wobbles.
pub struct Sim {
    pub mouse: [f32; 2],
    pub par: [f32; 2],
    pub field: f32,
    target: [f32; 2],
    par_target: [f32; 2],
    field_target: f32,
    force: u32,
}

impl Sim {
    pub fn new() -> Self {
        Self { mouse: [0.89, 0.5], par: [0.0; 2], field: 0.0, target: [0.89, 0.5], par_target: [0.0; 2], field_target: 0.0, force: 3 }
    }

    pub fn wake(&mut self) {
        self.force = 3;
    }

    /// Returns true while something is still moving and a redraw is needed.
    pub fn update(&mut self, dt: f32, cursor: Option<(f32, f32)>, res: (f32, f32)) -> bool {
        let (w, h) = res;
        let shift = 0.5 * (w / h - 1.7777778);
        let mut inside = false;
        if let Some((cx, cy)) = cursor {
            if cx >= 0.0 && cy >= 0.0 && cx < w && cy < h {
                inside = true;
                if self.field_target < 0.5 && self.field < 0.05 {
                    self.mouse = [cx / h - shift, cy / h];
                }
                self.target = [cx / h - shift, cy / h];
                self.par_target = [(cx / w - 0.5) * 2.0, (cy / h - 0.5) * 2.0];
            }
        }
        self.field_target = if inside { 1.0 } else { 0.0 };
        if !inside {
            self.par_target = [0.0, 0.0];
        }
        let km = 1.0 - (-dt * 7.0).exp();
        let kf = 1.0 - (-dt * 3.2).exp();
        let kp = 1.0 - (-dt * 2.2).exp();
        for i in 0..2 {
            self.mouse[i] += (self.target[i] - self.mouse[i]) * km;
            self.par[i] += (self.par_target[i] - self.par[i]) * kp;
        }
        self.field += (self.field_target - self.field) * kf;
        let moving = (self.target[0] - self.mouse[0]).abs() > 0.0004
            || (self.target[1] - self.mouse[1]).abs() > 0.0004
            || (self.par_target[0] - self.par[0]).abs() > 0.001
            || (self.par_target[1] - self.par[1]).abs() > 0.001
            || (self.field_target - self.field).abs() > 0.002;
        let active = moving || self.force > 0;
        if self.force > 0 {
            self.force -= 1;
        }
        active
    }
}
