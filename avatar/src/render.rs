//! CPU vector renderer (tiny-skia). Design space is 512x512, scaled to `size`.
//! Procedural parts live here for now; swap in SVG/asset-driven parts later
//! without touching the model or the UI.

use crate::fmath::{cos, sin};
use crate::gfx::{Path, PathBuilder, Rect, Surface, Transform};
use crate::model::*;
use crate::rig::{Expression, Pose};

const DESIGN: f32 = 512.0;
const CX: f32 = 256.0;
const CY: f32 = 236.0;
const INK: Rgb = Rgb(40, 32, 34);

struct Ctx {
    sf: Surface,
    base: Transform,
}

impl Ctx {
    fn fill(&mut self, path: &Option<Path>, c: Rgb, a: u8, xf: Transform) {
        if let Some(p) = path {
            self.sf.fill_path(p, xf.post_concat(self.base), c, a);
        }
    }
    fn stroke(&mut self, path: &Option<Path>, c: Rgb, w: f32, xf: Transform) {
        if let Some(p) = path {
            self.sf.stroke_path(p, xf.post_concat(self.base), w, c, 255);
        }
    }
}

fn path(f: impl FnOnce(&mut PathBuilder)) -> Option<Path> {
    let mut pb = PathBuilder::new();
    f(&mut pb);
    pb.finish()
}

fn oval(cx: f32, cy: f32, rx: f32, ry: f32) -> Option<Path> {
    Rect::from_xywh(cx - rx, cy - ry, rx * 2.0, ry * 2.0).and_then(PathBuilder::from_oval)
}

fn quad(x0: f32, y0: f32, cx: f32, cy: f32, x1: f32, y1: f32) -> Option<Path> {
    path(|p| {
        p.move_to(x0, y0);
        p.quad_to(cx, cy, x1, y1);
    })
}

/// (half width, half height, top ctrl, jaw ctrl, chin width, chin drop)
fn head_params(s: HeadShape) -> (f32, f32, f32, f32, f32, f32) {
    match s {
        HeadShape::Round => (120.0, 128.0, 0.72, 0.80, 0.55, 0.0),
        HeadShape::Oval => (104.0, 142.0, 0.70, 0.85, 0.50, 4.0),
        HeadShape::Square => (118.0, 122.0, 0.92, 0.95, 0.80, 0.0),
        HeadShape::Heart => (122.0, 124.0, 0.78, 0.30, 0.28, 14.0),
    }
}

fn head_path(s: HeadShape) -> Option<Path> {
    let (hw, hh, tk, jk, cw, chin) = head_params(s);
    path(|p| {
        p.move_to(CX, CY - hh);
        p.cubic_to(CX + hw * tk, CY - hh, CX + hw, CY - hh * 0.6, CX + hw, CY);
        p.cubic_to(
            CX + hw,
            CY + hh * jk,
            CX + hw * cw,
            CY + hh + chin * 0.5,
            CX,
            CY + hh + chin,
        );
        p.cubic_to(
            CX - hw * cw,
            CY + hh + chin * 0.5,
            CX - hw,
            CY + hh * jk,
            CX - hw,
            CY,
        );
        p.cubic_to(CX - hw, CY - hh * 0.6, CX - hw * tk, CY - hh, CX, CY - hh);
        p.close();
    })
}

fn draw_body(c: &mut Ctx, av: &Avatar) {
    let id = Transform::identity();
    let neck = Rect::from_xywh(226.0, 320.0, 60.0, 100.0).map(PathBuilder::from_rect);
    c.fill(&neck, av.skin.darken(0.12), 255, id);
    let shoulders = path(|p| {
        p.move_to(30.0, 512.0);
        p.quad_to(50.0, 424.0, 170.0, 408.0);
        p.line_to(342.0, 408.0);
        p.quad_to(462.0, 424.0, 482.0, 512.0);
        p.close();
    });
    c.fill(&shoulders, av.outfit_color, 255, id);
    match av.outfit {
        Outfit::Tee => {
            let neckline = path(|p| {
                p.move_to(212.0, 408.0);
                p.quad_to(256.0, 458.0, 300.0, 408.0);
                p.close();
            });
            c.fill(&neckline, av.skin.darken(0.12), 255, id);
        }
        Outfit::Hoodie => {
            let hood = path(|p| {
                p.move_to(196.0, 408.0);
                p.quad_to(256.0, 480.0, 316.0, 408.0);
                p.quad_to(256.0, 436.0, 196.0, 408.0);
                p.close();
            });
            c.fill(&hood, av.outfit_color.darken(0.3), 255, id);
            c.stroke(
                &quad(236.0, 440.0, 232.0, 470.0, 236.0, 496.0),
                Rgb(245, 245, 245),
                4.0,
                id,
            );
            c.stroke(
                &quad(276.0, 440.0, 280.0, 470.0, 276.0, 496.0),
                Rgb(245, 245, 245),
                4.0,
                id,
            );
        }
        Outfit::Collar => {
            for s in [-1.0f32, 1.0] {
                let col = path(|p| {
                    p.move_to(CX + s * 40.0, 408.0);
                    p.line_to(CX, 452.0);
                    p.line_to(CX + s * 20.0, 402.0);
                    p.close();
                });
                c.fill(&col, Rgb(245, 245, 245), 255, id);
            }
        }
    }
}

fn draw_hair_back(c: &mut Ctx, av: &Avatar, xf: Transform) {
    let col = av.hair_color;
    match av.hair {
        HairStyle::Long => {
            let back = path(|p| {
                p.move_to(124.0, 210.0);
                p.cubic_to(120.0, 80.0, 392.0, 80.0, 388.0, 210.0);
                p.line_to(392.0, 390.0);
                p.quad_to(256.0, 420.0, 120.0, 390.0);
                p.close();
            });
            c.fill(&back, col.darken(0.15), 255, xf);
        }
        HairStyle::Bun => c.fill(&oval(CX, 88.0, 44.0, 40.0), col, 255, xf),
        HairStyle::Curly => {
            for i in 0..11 {
                let a = core::f32::consts::PI * (1.0 + i as f32 / 10.0);
                let (x, y) = (CX + 138.0 * cos(a), 206.0 + 112.0 * sin(a));
                c.fill(&oval(x, y, 30.0, 30.0), col.darken(0.1), 255, xf);
            }
        }
        _ => {}
    }
}

fn draw_hair_front(c: &mut Ctx, av: &Avatar, xf: Transform) {
    let col = av.hair_color;
    match av.hair {
        HairStyle::Bald => {}
        HairStyle::Buzz => {
            let cap = path(|p| {
                p.move_to(138.0, 196.0);
                p.cubic_to(128.0, 92.0, 384.0, 92.0, 374.0, 196.0);
                p.cubic_to(344.0, 150.0, 168.0, 150.0, 138.0, 196.0);
                p.close();
            });
            c.fill(&cap, col, 255, xf);
        }
        HairStyle::Short | HairStyle::Long | HairStyle::Bun => {
            let hair = path(|p| {
                p.move_to(130.0, 216.0);
                p.cubic_to(106.0, 66.0, 406.0, 66.0, 382.0, 216.0);
                p.cubic_to(372.0, 176.0, 340.0, 150.0, 298.0, 146.0);
                p.cubic_to(262.0, 172.0, 196.0, 172.0, 150.0, 190.0);
                p.close();
            });
            c.fill(&hair, col, 255, xf);
        }
        HairStyle::Curly => {
            for i in 0..9 {
                let a = core::f32::consts::PI * (1.05 + i as f32 / 9.0 * 0.9);
                let (x, y) = (CX + 118.0 * cos(a), 196.0 + 92.0 * sin(a));
                c.fill(&oval(x, y, 28.0, 26.0), col, 255, xf);
            }
            c.fill(&oval(CX, 130.0, 100.0, 40.0), col, 255, xf);
        }
    }
}

fn draw_eye(c: &mut Ctx, av: &Avatar, ex: &Expression, side: f32, xf: Transform) {
    let open = if side < 0.0 {
        ex.eye_open_l
    } else {
        ex.eye_open_r
    };
    let (rx, ry) = match av.eyes {
        EyeStyle::Round => (20.0, 17.0),
        EyeStyle::Almond => (23.0, 12.0),
        EyeStyle::Wide => (24.0, 20.0),
        EyeStyle::Sleepy => (22.0, 10.0),
    };
    let (x, y) = (CX + side * 48.0, CY - 6.0);
    if open < 0.12 {
        c.stroke(&quad(x - rx, y, x, y + 9.0, x + rx, y), INK, 4.0, xf);
        return;
    }
    let h = ry * open;
    c.fill(&oval(x, y, rx, h), Rgb(250, 250, 252), 255, xf);
    let ir = (ry * 0.72).min(rx * 0.5) * (0.55 + 0.45 * open);
    let (gx, gy) = (x + ex.gaze_x * rx * 0.35, y + ex.gaze_y * h * 0.35);
    c.fill(&oval(gx, gy, ir, ir), av.eye_color, 255, xf);
    c.fill(&oval(gx, gy, ir * 0.5, ir * 0.5), INK, 255, xf);
    c.fill(
        &oval(gx - ir * 0.3, gy - ir * 0.35, ir * 0.22, ir * 0.22),
        Rgb(255, 255, 255),
        255,
        xf,
    );
    c.stroke(&quad(x - rx, y, x, y - h * 2.0, x + rx, y), INK, 3.5, xf);
}

fn draw_brow(c: &mut Ctx, av: &Avatar, ex: &Expression, side: f32, xf: Transform) {
    let raise = if side < 0.0 { ex.brow_l } else { ex.brow_r };
    let (len, thick, arch) = match av.brows {
        BrowStyle::Soft => (24.0, 5.0, 4.0),
        BrowStyle::Straight => (26.0, 5.0, 0.0),
        BrowStyle::Thick => (26.0, 9.0, 3.0),
        BrowStyle::Arched => (24.0, 5.0, 9.0),
    };
    let by = CY - 42.0 - raise * 14.0;
    let inner = (CX + side * (50.0 - len), by + ex.brow_furrow * 10.0);
    let outer = (CX + side * (50.0 + len), by - ex.brow_furrow * 3.0);
    let ctrl = (CX + side * 50.0, by - arch);
    c.stroke(
        &quad(inner.0, inner.1, ctrl.0, ctrl.1, outer.0, outer.1),
        av.hair_color,
        thick,
        xf,
    );
}

fn draw_nose(c: &mut Ctx, av: &Avatar, xf: Transform) {
    let ink = av.skin.darken(0.3);
    match av.nose {
        NoseStyle::Button => c.stroke(
            &quad(246.0, 268.0, 256.0, 276.0, 266.0, 268.0),
            ink,
            3.5,
            xf,
        ),
        NoseStyle::Pointed => {
            let n = path(|p| {
                p.move_to(256.0, 236.0);
                p.quad_to(250.0, 264.0, 244.0, 268.0);
                p.quad_to(256.0, 276.0, 268.0, 268.0);
            });
            c.stroke(&n, ink, 3.5, xf);
        }
        NoseStyle::Wide => {
            c.stroke(
                &quad(238.0, 266.0, 256.0, 280.0, 274.0, 266.0),
                ink,
                3.5,
                xf,
            );
            c.fill(&oval(244.0, 270.0, 3.0, 2.5), ink, 255, xf);
            c.fill(&oval(268.0, 270.0, 3.0, 2.5), ink, 255, xf);
        }
    }
}

fn draw_mouth(c: &mut Ctx, av: &Avatar, ex: &Expression, xf: Transform) {
    let (w, lip): (f32, f32) = match av.mouth {
        MouthStyle::Thin => (34.0, 3.5),
        MouthStyle::Regular => (40.0, 5.0),
        MouthStyle::Full => (44.0, 8.0),
    };
    let y = 304.0;
    let corner = y - ex.smile * 12.0;
    let ctrl_up = y + ex.smile * 16.0;
    let (l, r) = (CX - w, CX + w);
    if ex.mouth_open > 0.04 {
        let ctrl_lo = ctrl_up + ex.mouth_open * 70.0;
        let inside = path(|p| {
            p.move_to(l, corner);
            p.quad_to(CX, ctrl_up, r, corner);
            p.quad_to(CX, ctrl_lo, l, corner);
            p.close();
        });
        c.fill(&inside, Rgb(96, 30, 42), 255, xf);
        c.stroke(&inside, av.lip_color, lip.min(5.0), xf);
    } else {
        c.stroke(
            &quad(l, corner, CX, ctrl_up, r, corner),
            av.lip_color,
            lip,
            xf,
        );
    }
}

fn draw_facial_hair(c: &mut Ctx, av: &Avatar, xf: Transform, below_mouth: bool) {
    let col = av.hair_color;
    match (av.facial_hair, below_mouth) {
        (FacialHair::Stubble, false) | (FacialHair::Beard, false) => {
            let beard = path(|p| {
                p.move_to(140.0, 262.0);
                p.cubic_to(142.0, 360.0, 200.0, 392.0, 256.0, 392.0);
                p.cubic_to(312.0, 392.0, 370.0, 360.0, 372.0, 262.0);
                p.cubic_to(352.0, 296.0, 318.0, 288.0, 256.0, 290.0);
                p.cubic_to(194.0, 288.0, 160.0, 296.0, 140.0, 262.0);
                p.close();
            });
            let a = if av.facial_hair == FacialHair::Stubble {
                70
            } else {
                255
            };
            c.fill(&beard, col, a, xf);
        }
        (FacialHair::Moustache, true) | (FacialHair::Beard, true) => {
            let m = path(|p| {
                p.move_to(256.0, 286.0);
                p.quad_to(226.0, 274.0, 212.0, 294.0);
                p.quad_to(236.0, 288.0, 256.0, 294.0);
                p.quad_to(276.0, 288.0, 300.0, 294.0);
                p.quad_to(286.0, 274.0, 256.0, 286.0);
                p.close();
            });
            c.fill(&m, col, 255, xf);
        }
        _ => {}
    }
}

fn draw_glasses(c: &mut Ctx, av: &Avatar, xf: Transform) {
    if av.glasses == Glasses::None {
        return;
    }
    let y = CY - 6.0;
    for s in [-1.0f32, 1.0] {
        let x = CX + s * 48.0;
        let frame = match av.glasses {
            Glasses::Round => oval(x, y, 30.0, 30.0),
            _ => Rect::from_xywh(x - 32.0, y - 25.0, 64.0, 50.0).map(PathBuilder::from_rect),
        };
        c.fill(&frame, Rgb(200, 230, 255), 50, xf);
        c.stroke(&frame, Rgb(40, 40, 48), 5.0, xf);
    }
    c.stroke(
        &quad(CX - 18.0, y - 4.0, CX, y - 10.0, CX + 18.0, y - 4.0),
        Rgb(40, 40, 48),
        4.0,
        xf,
    );
}

pub fn render(av: &Avatar, ex: &Expression, pose: &Pose, size: u32, bg: Option<Rgb>) -> Surface {
    let size = size.max(1);
    let s = size as f32 / DESIGN;
    let mut c = Ctx {
        sf: Surface::new(size as usize, size as usize),
        base: Transform::from_scale(s, s),
    };
    let id = Transform::identity();
    if let Some(b) = bg {
        let rect = Rect::from_xywh(0.0, 0.0, DESIGN, DESIGN).map(PathBuilder::from_rect);
        c.fill(&rect, b, 255, id);
    }

    draw_body(&mut c, av);

    let head = Transform::from_rotate_at(pose.roll * 25.0, CX, 340.0)
        .post_concat(Transform::from_translate(pose.yaw * 14.0, 0.0));
    let feat = Transform::from_translate(pose.yaw * 28.0, pose.pitch * 16.0).post_concat(head);

    draw_hair_back(&mut c, av, head);

    let (hw, ..) = head_params(av.head);
    for s in [-1.0f32, 1.0] {
        c.fill(
            &oval(CX + s * hw, CY + 10.0, 20.0, 26.0),
            av.skin.darken(0.06),
            255,
            head,
        );
    }
    let head_p = head_path(av.head);
    c.fill(&head_p, av.skin, 255, head);
    for s in [-1.0f32, 1.0] {
        c.fill(
            &oval(CX + s * 74.0, CY + 52.0, 22.0, 14.0),
            Rgb(235, 120, 120),
            (40.0 + 40.0 * ex.smile.max(0.0)) as u8,
            feat,
        );
    }

    draw_facial_hair(&mut c, av, head, false);
    for side in [-1.0f32, 1.0] {
        draw_eye(&mut c, av, ex, side, feat);
        draw_brow(&mut c, av, ex, side, feat);
    }
    draw_nose(&mut c, av, feat);
    draw_mouth(&mut c, av, ex, feat);
    draw_facial_hair(&mut c, av, feat, true);
    draw_glasses(&mut c, av, feat);
    draw_hair_front(&mut c, av, head);

    c.sf
}

/// Straight (non-premultiplied) RGBA8, ready for a UI texture upload.
pub fn to_rgba8(sf: &Surface) -> alloc::vec::Vec<u8> {
    sf.px.clone()
}
