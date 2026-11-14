//! Tiny anti-aliased vector rasterizer: no dependencies, `no_std` + `alloc`.
//!
//! Coverage is accumulated with signed areas (font-rs style) so edges get
//! exact horizontal AA. Fills use |winding| clamped to 1, so every filled
//! path must be a single contour or consistently oriented contours. Strokes
//! are unions of oriented quads and round joins.

use crate::fmath::{abs, ceil, cos, floor, sin, sqrt};
use crate::model::Rgb;
use alloc::vec;
use alloc::vec::Vec;
use core::f32::consts::{PI, TAU};

#[derive(Clone, Copy, Debug)]
pub struct Transform {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl Transform {
    pub const fn identity() -> Self {
        Transform {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 0.0,
            f: 0.0,
        }
    }
    pub const fn from_scale(sx: f32, sy: f32) -> Self {
        Transform {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: sy,
            e: 0.0,
            f: 0.0,
        }
    }
    pub const fn from_translate(tx: f32, ty: f32) -> Self {
        Transform {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: tx,
            f: ty,
        }
    }
    pub fn from_rotate_at(deg: f32, cx: f32, cy: f32) -> Self {
        let r = deg * PI / 180.0;
        let (cs, sn) = (cos(r), sin(r));
        Transform {
            a: cs,
            b: sn,
            c: -sn,
            d: cs,
            e: cx - cs * cx + sn * cy,
            f: cy - sn * cx - cs * cy,
        }
    }
    /// Apply `self` first, then `o`.
    pub fn post_concat(self, o: Transform) -> Transform {
        Transform {
            a: o.a * self.a + o.c * self.b,
            c: o.a * self.c + o.c * self.d,
            e: o.a * self.e + o.c * self.f + o.e,
            b: o.b * self.a + o.d * self.b,
            d: o.b * self.c + o.d * self.d,
            f: o.b * self.e + o.d * self.f + o.f,
        }
    }
    fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }
    fn scale_factor(&self) -> f32 {
        sqrt(abs(self.a * self.d - self.b * self.c))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn from_xywh(x: f32, y: f32, w: f32, h: f32) -> Option<Rect> {
        (w > 0.0 && h > 0.0).then_some(Rect { x, y, w, h })
    }
}

#[derive(Clone, Copy, Debug)]
enum Seg {
    Move(f32, f32),
    Line(f32, f32),
    Quad(f32, f32, f32, f32),
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
}

#[derive(Clone, Debug)]
pub struct Path {
    segs: Vec<Seg>,
}

#[derive(Default)]
pub struct PathBuilder {
    segs: Vec<Seg>,
}

impl PathBuilder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn move_to(&mut self, x: f32, y: f32) {
        self.segs.push(Seg::Move(x, y));
    }
    pub fn line_to(&mut self, x: f32, y: f32) {
        self.segs.push(Seg::Line(x, y));
    }
    pub fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.segs.push(Seg::Quad(cx, cy, x, y));
    }
    pub fn cubic_to(&mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) {
        self.segs.push(Seg::Cubic(c1x, c1y, c2x, c2y, x, y));
    }
    pub fn close(&mut self) {
        self.segs.push(Seg::Close);
    }
    pub fn finish(self) -> Option<Path> {
        (self.segs.len() > 1).then_some(Path { segs: self.segs })
    }
    pub fn from_rect(r: Rect) -> Path {
        let mut p = PathBuilder::new();
        p.move_to(r.x, r.y);
        p.line_to(r.x + r.w, r.y);
        p.line_to(r.x + r.w, r.y + r.h);
        p.line_to(r.x, r.y + r.h);
        p.close();
        Path { segs: p.segs }
    }
    pub fn from_oval(r: Rect) -> Option<Path> {
        let (rx, ry) = (r.w / 2.0, r.h / 2.0);
        let (cx, cy) = (r.x + rx, r.y + ry);
        let (kx, ky) = (rx * 0.5523, ry * 0.5523);
        let mut p = PathBuilder::new();
        p.move_to(cx + rx, cy);
        p.cubic_to(cx + rx, cy + ky, cx + kx, cy + ry, cx, cy + ry);
        p.cubic_to(cx - kx, cy + ry, cx - rx, cy + ky, cx - rx, cy);
        p.cubic_to(cx - rx, cy - ky, cx - kx, cy - ry, cx, cy - ry);
        p.cubic_to(cx + kx, cy - ry, cx + rx, cy - ky, cx + rx, cy);
        p.close();
        p.finish()
    }
}

const STEPS: usize = 16;
type Pt = (f32, f32);

struct Poly {
    pts: Vec<Pt>,
    closed: bool,
}

fn flatten(path: &Path, t: &Transform) -> Vec<Poly> {
    let mut out: Vec<Poly> = Vec::new();
    let mut cur: Option<Poly> = None;
    let mut last = (0.0, 0.0);
    let flush = |cur: &mut Option<Poly>, out: &mut Vec<Poly>| {
        if let Some(p) = cur.take()
            && p.pts.len() > 1
        {
            out.push(p);
        }
    };
    for s in &path.segs {
        match *s {
            Seg::Move(x, y) => {
                flush(&mut cur, &mut out);
                last = t.apply(x, y);
                cur = Some(Poly {
                    pts: vec![last],
                    closed: false,
                });
            }
            Seg::Line(x, y) => {
                last = t.apply(x, y);
                if let Some(p) = cur.as_mut() {
                    p.pts.push(last);
                }
            }
            Seg::Quad(cx, cy, x, y) => {
                let (c, e) = (t.apply(cx, cy), t.apply(x, y));
                if let Some(p) = cur.as_mut() {
                    for i in 1..=STEPS {
                        let u = i as f32 / STEPS as f32;
                        let v = 1.0 - u;
                        p.pts.push((
                            v * v * last.0 + 2.0 * u * v * c.0 + u * u * e.0,
                            v * v * last.1 + 2.0 * u * v * c.1 + u * u * e.1,
                        ));
                    }
                }
                last = e;
            }
            Seg::Cubic(x1, y1, x2, y2, x, y) => {
                let (c1, c2, e) = (t.apply(x1, y1), t.apply(x2, y2), t.apply(x, y));
                if let Some(p) = cur.as_mut() {
                    for i in 1..=STEPS {
                        let u = i as f32 / STEPS as f32;
                        let v = 1.0 - u;
                        let (a, b, c, d) = (v * v * v, 3.0 * v * v * u, 3.0 * v * u * u, u * u * u);
                        p.pts.push((
                            a * last.0 + b * c1.0 + c * c2.0 + d * e.0,
                            a * last.1 + b * c1.1 + c * c2.1 + d * e.1,
                        ));
                    }
                }
                last = e;
            }
            Seg::Close => {
                if let Some(p) = cur.as_mut() {
                    p.closed = true;
                    last = p.pts[0];
                }
                flush(&mut cur, &mut out);
            }
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// Straight-alpha RGBA8 canvas with a reusable coverage buffer.
pub struct Surface {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
    acc: Vec<f32>,
    stride: usize,
    ymin: usize,
    ymax: usize,
}

impl Surface {
    pub fn new(w: usize, h: usize) -> Surface {
        let (w, h) = (w.max(1), h.max(1));
        Surface {
            w,
            h,
            px: vec![0; w * h * 4],
            acc: vec![0.0; (w + 3) * h],
            stride: w + 3,
            ymin: usize::MAX,
            ymax: 0,
        }
    }

    pub fn fill_all(&mut self, c: Rgb, alpha: u8) {
        for p in self.px.as_chunks_mut::<4>().0 {
            *p = [c.0, c.1, c.2, alpha];
        }
    }

    fn line(&mut self, p0: Pt, p1: Pt) {
        let wf = self.w as f32;
        let clampx = |p: Pt| (p.0.clamp(0.0, wf), p.1);
        let (p0, p1) = (clampx(p0), clampx(p1));
        if p0.1 == p1.1 {
            return;
        }
        let (dir, p0, p1) = if p0.1 < p1.1 {
            (1.0, p0, p1)
        } else {
            (-1.0, p1, p0)
        };
        if p1.1 <= 0.0 || p0.1 >= self.h as f32 {
            return;
        }
        let dxdy = (p1.0 - p0.0) / (p1.1 - p0.1);
        let mut x = p0.0;
        if p0.1 < 0.0 {
            x -= p0.1 * dxdy;
        }
        let y0 = if p0.1 < 0.0 { 0 } else { p0.1 as usize };
        let yend = self.h.min(ceil(p1.1) as usize);
        self.ymin = self.ymin.min(y0);
        self.ymax = self.ymax.max(yend.saturating_sub(1));
        for y in y0..yend {
            let ls = y * self.stride;
            let top = if (y as f32) > p0.1 { y as f32 } else { p0.1 };
            let bot = if ((y + 1) as f32) < p1.1 {
                (y + 1) as f32
            } else {
                p1.1
            };
            let dy = bot - top;
            let xnext = x + dxdy * dy;
            let d = dy * dir;
            let (x0, x1) = if x < xnext { (x, xnext) } else { (xnext, x) };
            let x0floor = floor(x0);
            let x0i = x0floor as usize;
            let x1ceil = ceil(x1);
            let x1i = x1ceil as usize;
            if x1i <= x0i + 1 {
                let xmf = 0.5 * (x + xnext) - x0floor;
                self.acc[ls + x0i] += d - d * xmf;
                self.acc[ls + x0i + 1] += d * xmf;
            } else {
                let s = 1.0 / (x1 - x0);
                let x0f = x0 - x0floor;
                let a0 = 0.5 * s * (1.0 - x0f) * (1.0 - x0f);
                let x1f = x1 - x1ceil + 1.0;
                let am = 0.5 * s * x1f * x1f;
                self.acc[ls + x0i] += d * a0;
                if x1i == x0i + 2 {
                    self.acc[ls + x0i + 1] += d * (1.0 - a0 - am);
                } else {
                    let a1 = s * (1.5 - x0f);
                    self.acc[ls + x0i + 1] += d * (a1 - a0);
                    for xi in x0i + 2..x1i - 1 {
                        self.acc[ls + xi] += d * s;
                    }
                    let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                    self.acc[ls + x1i - 1] += d * (1.0 - a2 - am);
                }
                self.acc[ls + x1i] += d * am;
            }
            x = xnext;
        }
    }

    fn poly_edges(&mut self, pts: &[Pt]) {
        for i in 0..pts.len() {
            self.line(pts[i], pts[(i + 1) % pts.len()]);
        }
    }

    fn resolve(&mut self, c: Rgb, alpha: u8) {
        if self.ymin == usize::MAX {
            return;
        }
        let ca = alpha as f32 / 255.0;
        for y in self.ymin..=self.ymax.min(self.h - 1) {
            let mut sum = 0.0;
            for x in 0..self.w {
                sum += self.acc[y * self.stride + x];
                let cov = abs(sum).min(1.0) * ca;
                if cov > 0.002 {
                    blend(&mut self.px[(y * self.w + x) * 4..][..4], c, cov);
                }
            }
            for v in &mut self.acc[y * self.stride..(y + 1) * self.stride] {
                *v = 0.0;
            }
        }
        self.ymin = usize::MAX;
        self.ymax = 0;
    }

    pub fn fill_path(&mut self, path: &Path, t: Transform, c: Rgb, alpha: u8) {
        for poly in flatten(path, &t) {
            self.poly_edges(&poly.pts);
        }
        self.resolve(c, alpha);
    }

    pub fn stroke_path(&mut self, path: &Path, t: Transform, width: f32, c: Rgb, alpha: u8) {
        let hw = (width * t.scale_factor()).max(0.5) / 2.0;
        const N: usize = 16;
        let mut ring = [(0.0f32, 0.0f32); N];
        for (i, r) in ring.iter_mut().enumerate() {
            let a = TAU * i as f32 / N as f32;
            *r = (cos(a) * hw, sin(a) * hw);
        }
        for poly in flatten(path, &t) {
            let n = poly.pts.len();
            let segs = if poly.closed { n } else { n - 1 };
            for i in 0..segs {
                let (p, q) = (poly.pts[i], poly.pts[(i + 1) % n]);
                let (dx, dy) = (q.0 - p.0, q.1 - p.1);
                let len = sqrt(dx * dx + dy * dy);
                if len < 1e-4 {
                    continue;
                }
                let (nx, ny) = (-dy / len * hw, dx / len * hw);
                let mut quad = [
                    (p.0 + nx, p.1 + ny),
                    (q.0 + nx, q.1 + ny),
                    (q.0 - nx, q.1 - ny),
                    (p.0 - nx, p.1 - ny),
                ];
                if signed_area(&quad) < 0.0 {
                    quad.reverse();
                }
                self.poly_edges(&quad);
            }
            for p in &poly.pts {
                let mut circle = [(0.0, 0.0); N];
                for (c, r) in circle.iter_mut().zip(&ring) {
                    *c = (p.0 + r.0, p.1 + r.1);
                }
                if signed_area(&circle) < 0.0 {
                    circle.reverse();
                }
                self.poly_edges(&circle);
            }
        }
        self.resolve(c, alpha);
    }

    /// Composite `src` at (ox, oy); `clip` = (cx, cy, r) limits it to an AA circle.
    pub fn draw_surface(&mut self, src: &Surface, ox: i32, oy: i32, clip: Option<(f32, f32, f32)>) {
        for sy in 0..src.h {
            let dy = oy + sy as i32;
            if dy < 0 || dy >= self.h as i32 {
                continue;
            }
            for sx in 0..src.w {
                let dx = ox + sx as i32;
                if dx < 0 || dx >= self.w as i32 {
                    continue;
                }
                let s = &src.px[(sy * src.w + sx) * 4..][..4];
                let mut a = s[3] as f32 / 255.0;
                if let Some((cx, cy, r)) = clip {
                    let (ex, ey) = (dx as f32 + 0.5 - cx, dy as f32 + 0.5 - cy);
                    a *= (r - sqrt(ex * ex + ey * ey) + 0.5).clamp(0.0, 1.0);
                }
                if a > 0.002 {
                    blend(
                        &mut self.px[(dy as usize * self.w + dx as usize) * 4..][..4],
                        Rgb(s[0], s[1], s[2]),
                        a,
                    );
                }
            }
        }
    }
}

fn signed_area(p: &[Pt]) -> f32 {
    let mut s = 0.0;
    for i in 0..p.len() {
        let (a, b) = (p[i], p[(i + 1) % p.len()]);
        s += a.0 * b.1 - b.0 * a.1;
    }
    s
}

fn blend(px: &mut [u8], c: Rgb, a: f32) {
    let da = px[3] as f32 / 255.0;
    let oa = a + da * (1.0 - a);
    if oa <= 0.0 {
        return;
    }
    let k = da * (1.0 - a);
    px[0] = ((c.0 as f32 * a + px[0] as f32 * k) / oa + 0.5) as u8;
    px[1] = ((c.1 as f32 * a + px[1] as f32 * k) / oa + 0.5) as u8;
    px[2] = ((c.2 as f32 * a + px[2] as f32 * k) / oa + 0.5) as u8;
    px[3] = (oa * 255.0 + 0.5) as u8;
}
