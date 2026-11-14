use crate::gfx::Surface;
use crate::model::{Avatar, Rgb};
use crate::render::render;
use crate::rig::{Expression, Pose};

/// Contact sheet of every expression preset, one cell each.
pub fn sticker_sheet(av: &Avatar, cell: u32, cols: u32, bg: Option<Rgb>) -> Surface {
    let n = Expression::PRESETS.len() as u32;
    let cols = cols.max(1).min(n);
    let rows = n.div_ceil(cols);
    let mut sheet = Surface::new((cell * cols) as usize, (cell * rows) as usize);
    for (i, (_, ex)) in Expression::PRESETS.iter().enumerate() {
        let i = i as u32;
        let tile = render(av, ex, &Pose::default(), cell, bg);
        sheet.draw_surface(
            &tile,
            (cell * (i % cols)) as i32,
            (cell * (i / cols)) as i32,
            None,
        );
    }
    sheet
}
