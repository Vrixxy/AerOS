//! A small TrueType (glyf) rasterizer: parses the tables it needs
//! (head, maxp, hhea, hmtx, loca, glyf, cmap), builds glyph outlines
//! (simple and composite glyphs, quadratic curves) and renders them with
//! exact-area anti-aliasing. Enough to draw any character the bundled fonts
//! contain at any size, not just the pre-rendered ASCII atlas.

/// Largest glyph bitmap edge in pixels.
pub const MAX_EDGE: usize = 128;

// Metrics accessors (`advance`, ascent/descent) feed the text layout.
#[allow(dead_code)]
#[derive(Clone, Copy)]
pub struct TrueType {
    data: &'static [u8],
    pub units_per_em: u16,
    pub ascent: i16,
    pub descent: i16,
    glyph_count: u16,
    long_loca: bool,
    loca: usize,
    glyf: usize,
    hmtx: usize,
    metric_count: u16,
    cmap: usize,
    cmap_format: u16,
}

/// One rendered glyph: 8-bit coverage, plus where it sits relative to the
/// pen position on the baseline.
pub struct GlyphBitmap {
    pub width: usize,
    pub height: usize,
    /// Pixels from the pen to the bitmap's left edge.
    pub left: i32,
    /// Pixels from the baseline up to the bitmap's top edge.
    pub top: i32,
    pub coverage: [u8; MAX_EDGE * MAX_EDGE],
}

impl GlyphBitmap {
    pub const fn empty() -> Self {
        Self {
            width: 0,
            height: 0,
            left: 0,
            top: 0,
            coverage: [0; MAX_EDGE * MAX_EDGE],
        }
    }
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*data.get(at)?, *data.get(at + 1)?]))
}

fn i16_at(data: &[u8], at: usize) -> Option<i16> {
    u16_at(data, at).map(|v| v as i16)
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *data.get(at)?,
        *data.get(at + 1)?,
        *data.get(at + 2)?,
        *data.get(at + 3)?,
    ]))
}

/// A point of a glyph outline in font units.
#[derive(Clone, Copy)]
struct Point {
    x: i32,
    y: i32,
    on_curve: bool,
}

const MAX_POINTS: usize = 512;
const MAX_CONTOURS: usize = 64;

struct Outline {
    points: [Point; MAX_POINTS],
    count: usize,
    /// Index one past the last point of each contour.
    ends: [usize; MAX_CONTOURS],
    contours: usize,
}

impl Outline {
    const fn new() -> Self {
        Self {
            points: [Point {
                x: 0,
                y: 0,
                on_curve: false,
            }; MAX_POINTS],
            count: 0,
            ends: [0; MAX_CONTOURS],
            contours: 0,
        }
    }
}

impl TrueType {
    pub fn parse(data: &'static [u8]) -> Option<Self> {
        let version = u32_at(data, 0)?;
        if version != 0x0001_0000 && version != 0x7472_7565 {
            return None; // not TrueType outlines (e.g. CFF)
        }
        let table_count = u16_at(data, 4)? as usize;
        let find = |tag: &[u8; 4]| -> Option<usize> {
            (0..table_count).find_map(|index| {
                let entry = 12 + index * 16;
                (data.get(entry..entry + 4)? == tag)
                    .then(|| u32_at(data, entry + 8).map(|offset| offset as usize))
                    .flatten()
            })
        };
        let head = find(b"head")?;
        let maxp = find(b"maxp")?;
        let hhea = find(b"hhea")?;
        let cmap_table = find(b"cmap")?;
        let units_per_em = u16_at(data, head + 18)?;
        let long_loca = i16_at(data, head + 50)? != 0;
        let glyph_count = u16_at(data, maxp + 4)?;
        let cmap_at = Self::pick_cmap(data, cmap_table)?;
        Some(Self {
            data,
            units_per_em,
            ascent: i16_at(data, hhea + 4)?,
            descent: i16_at(data, hhea + 6)?,
            glyph_count,
            long_loca,
            loca: find(b"loca")?,
            glyf: find(b"glyf")?,
            hmtx: find(b"hmtx")?,
            metric_count: u16_at(data, hhea + 34)?,
            cmap: cmap_at.0,
            cmap_format: cmap_at.1,
        })
    }

    /// The best Unicode cmap subtable: (offset, format).
    fn pick_cmap(data: &[u8], table: usize) -> Option<(usize, u16)> {
        let count = u16_at(data, table + 2)? as usize;
        let mut best: Option<(usize, u16, u32)> = None;
        for index in 0..count {
            let entry = table + 4 + index * 8;
            let platform = u16_at(data, entry)?;
            let encoding = u16_at(data, entry + 2)?;
            let offset = table + u32_at(data, entry + 4)? as usize;
            let format = u16_at(data, offset)?;
            // Windows Unicode full (3,10) > Windows BMP (3,1) > Unicode platform (0,*).
            let rank = match (platform, encoding, format) {
                (3, 10, 12) => 4,
                (0, _, 12) => 3,
                (3, 1, 4) => 2,
                (0, _, 4) => 1,
                _ => 0,
            };
            if rank > 0 && best.is_none_or(|(_, _, current)| rank > current) {
                best = Some((offset, format, rank));
            }
        }
        best.map(|(offset, format, _)| (offset, format))
    }

    /// The glyph for a Unicode code point (None when the font lacks it).
    pub fn glyph_index(&self, code_point: u32) -> Option<u16> {
        let data = self.data;
        let table = self.cmap;
        let glyph = match self.cmap_format {
            4 => {
                if code_point > 0xffff {
                    return None;
                }
                let segments = (u16_at(data, table + 6)? / 2) as usize;
                let ends = table + 14;
                let starts = ends + segments * 2 + 2;
                let deltas = starts + segments * 2;
                let ranges = deltas + segments * 2;
                let mut found = None;
                for segment in 0..segments {
                    let end = u16_at(data, ends + segment * 2)? as u32;
                    if code_point > end {
                        continue;
                    }
                    let start = u16_at(data, starts + segment * 2)? as u32;
                    if code_point < start {
                        break;
                    }
                    let delta = u16_at(data, deltas + segment * 2)?;
                    let range_at = ranges + segment * 2;
                    let range = u16_at(data, range_at)? as usize;
                    let glyph = if range == 0 {
                        (code_point as u16).wrapping_add(delta)
                    } else {
                        let at = range_at + range + (code_point - start) as usize * 2;
                        let raw = u16_at(data, at)?;
                        if raw == 0 { 0 } else { raw.wrapping_add(delta) }
                    };
                    found = Some(glyph);
                    break;
                }
                found?
            }
            12 => {
                let groups = u32_at(data, table + 12)? as usize;
                let (mut low, mut high) = (0usize, groups);
                let mut found = None;
                while low < high {
                    let middle = (low + high) / 2;
                    let group = table + 16 + middle * 12;
                    let start = u32_at(data, group)?;
                    let end = u32_at(data, group + 4)?;
                    if code_point < start {
                        high = middle;
                    } else if code_point > end {
                        low = middle + 1;
                    } else {
                        found = Some((u32_at(data, group + 8)? + (code_point - start)) as u16);
                        break;
                    }
                }
                found?
            }
            _ => return None,
        };
        (glyph != 0 && glyph < self.glyph_count).then_some(glyph)
    }

    /// Horizontal advance in font units.
    #[allow(dead_code)]
    pub fn advance(&self, glyph: u16) -> u16 {
        let index = glyph.min(self.metric_count.saturating_sub(1)) as usize;
        u16_at(self.data, self.hmtx + index * 4).unwrap_or(0)
    }

    fn glyph_range(&self, glyph: u16) -> Option<(usize, usize)> {
        let index = glyph as usize;
        let (start, end) = if self.long_loca {
            (
                u32_at(self.data, self.loca + index * 4)? as usize,
                u32_at(self.data, self.loca + index * 4 + 4)? as usize,
            )
        } else {
            (
                u16_at(self.data, self.loca + index * 2)? as usize * 2,
                u16_at(self.data, self.loca + index * 2 + 2)? as usize * 2,
            )
        };
        Some((self.glyf + start, self.glyf + end))
    }

    /// Appends a glyph's contours (transformed) to `outline`.
    fn load(
        &self,
        glyph: u16,
        outline: &mut Outline,
        matrix: [i32; 4],
        offset: (i32, i32),
        depth: u32,
    ) -> Option<()> {
        if depth > 4 {
            return None;
        }
        let (start, end) = self.glyph_range(glyph)?;
        if start == end {
            return Some(()); // empty glyph (space)
        }
        let data = self.data;
        let contours = i16_at(data, start)?;
        if contours >= 0 {
            let contours = contours as usize;
            if outline.contours + contours > MAX_CONTOURS {
                return None;
            }
            let mut at = start + 10;
            let mut last_end = 0usize;
            let first_point = outline.count;
            for contour in 0..contours {
                last_end = u16_at(data, at + contour * 2)? as usize + 1;
                outline.ends[outline.contours + contour] = first_point + last_end;
            }
            at += contours * 2;
            let instructions = u16_at(data, at)? as usize;
            at += 2 + instructions;
            let points = last_end;
            if outline.count + points > MAX_POINTS {
                return None;
            }
            // Flags (run-length coded).
            let mut flags = [0u8; MAX_POINTS];
            let mut index = 0;
            while index < points {
                let flag = *data.get(at)?;
                at += 1;
                flags[index] = flag;
                index += 1;
                if flag & 8 != 0 {
                    let repeat = *data.get(at)? as usize;
                    at += 1;
                    for _ in 0..repeat {
                        if index >= points {
                            return None;
                        }
                        flags[index] = flag;
                        index += 1;
                    }
                }
            }
            let mut xs = [0i32; MAX_POINTS];
            let mut value = 0i32;
            for index in 0..points {
                let flag = flags[index];
                if flag & 2 != 0 {
                    let delta = *data.get(at)? as i32;
                    at += 1;
                    value += if flag & 16 != 0 { delta } else { -delta };
                } else if flag & 16 == 0 {
                    value += i16_at(data, at)? as i32;
                    at += 2;
                }
                xs[index] = value;
            }
            let mut ys = [0i32; MAX_POINTS];
            value = 0;
            for index in 0..points {
                let flag = flags[index];
                if flag & 4 != 0 {
                    let delta = *data.get(at)? as i32;
                    at += 1;
                    value += if flag & 32 != 0 { delta } else { -delta };
                } else if flag & 32 == 0 {
                    value += i16_at(data, at)? as i32;
                    at += 2;
                }
                ys[index] = value;
            }
            for index in 0..points {
                // 16.16 matrix, then the offset.
                let x = ((xs[index] as i64 * matrix[0] as i64
                    + ys[index] as i64 * matrix[2] as i64)
                    >> 16) as i32
                    + offset.0;
                let y = ((xs[index] as i64 * matrix[1] as i64
                    + ys[index] as i64 * matrix[3] as i64)
                    >> 16) as i32
                    + offset.1;
                outline.points[outline.count + index] = Point {
                    x,
                    y,
                    on_curve: flags[index] & 1 != 0,
                };
            }
            outline.count += points;
            outline.contours += contours;
            return Some(());
        }
        // Composite glyph.
        let mut at = start + 10;
        loop {
            let flags = u16_at(data, at)?;
            let component = u16_at(data, at + 2)?;
            at += 4;
            let (dx, dy);
            if flags & 1 != 0 {
                dx = i16_at(data, at)? as i32;
                dy = i16_at(data, at + 2)? as i32;
                at += 4;
            } else {
                dx = *data.get(at)? as i8 as i32;
                dy = *data.get(at + 1)? as i8 as i32;
                at += 2;
            }
            if flags & 2 == 0 {
                return None; // point-matching composites aren't supported
            }
            let f2dot14 = |raw: i16| raw as i32 * 4; // 2.14 -> 16.16
            let mut local = [0x10000, 0, 0, 0x10000];
            if flags & 0x0008 != 0 {
                let scale = f2dot14(i16_at(data, at)?);
                local = [scale, 0, 0, scale];
                at += 2;
            } else if flags & 0x0040 != 0 {
                local = [
                    f2dot14(i16_at(data, at)?),
                    0,
                    0,
                    f2dot14(i16_at(data, at + 2)?),
                ];
                at += 4;
            } else if flags & 0x0080 != 0 {
                local = [
                    f2dot14(i16_at(data, at)?),
                    f2dot14(i16_at(data, at + 2)?),
                    f2dot14(i16_at(data, at + 4)?),
                    f2dot14(i16_at(data, at + 6)?),
                ];
                at += 8;
            }
            // Compose with the parent transform.
            let compose = |a: [i32; 4], b: [i32; 4]| -> [i32; 4] {
                let m = |x: i32, y: i32| ((x as i64 * y as i64) >> 16) as i32;
                [
                    m(a[0], b[0]) + m(a[1], b[2]),
                    m(a[0], b[1]) + m(a[1], b[3]),
                    m(a[2], b[0]) + m(a[3], b[2]),
                    m(a[2], b[1]) + m(a[3], b[3]),
                ]
            };
            let combined = compose(local, matrix);
            let moved = (
                offset.0
                    + (((dx as i64 * matrix[0] as i64 + dy as i64 * matrix[2] as i64) >> 16)
                        as i32),
                offset.1
                    + (((dx as i64 * matrix[1] as i64 + dy as i64 * matrix[3] as i64) >> 16)
                        as i32),
            );
            self.load(component, outline, combined, moved, depth + 1)?;
            if flags & 0x0020 == 0 {
                break;
            }
        }
        Some(())
    }

    /// Renders a glyph at `pixels_per_em`. Returns false for a glyph that
    /// can't be drawn; a blank glyph (space) succeeds with an empty bitmap.
    pub fn rasterize(&self, glyph: u16, pixels_per_em: i32, bitmap: &mut GlyphBitmap) -> bool {
        bitmap.width = 0;
        bitmap.height = 0;
        bitmap.left = 0;
        bitmap.top = 0;
        if pixels_per_em <= 0 || pixels_per_em as usize > MAX_EDGE - 8 {
            return false;
        }
        let mut outline = Outline::new();
        if self
            .load(glyph, &mut outline, [0x10000, 0, 0, 0x10000], (0, 0), 0)
            .is_none()
        {
            return false;
        }
        if outline.count == 0 {
            return true;
        }
        // Bounding box in font units.
        let (mut x_min, mut x_max, mut y_min, mut y_max) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
        for point in &outline.points[..outline.count] {
            x_min = x_min.min(point.x);
            x_max = x_max.max(point.x);
            y_min = y_min.min(point.y);
            y_max = y_max.max(point.y);
        }
        let scale = pixels_per_em as f32 / self.units_per_em as f32;
        let left = floor(x_min as f32 * scale) as i32 - 1;
        let right = ceil(x_max as f32 * scale) as i32 + 1;
        let top = ceil(y_max as f32 * scale) as i32 + 1;
        let bottom = floor(y_min as f32 * scale) as i32 - 1;
        let (width, height) = ((right - left) as usize, (top - bottom) as usize);
        if width == 0 || height == 0 || width > MAX_EDGE || height > MAX_EDGE {
            return false;
        }
        // Accumulation buffer (signed area per pixel).
        let mut area = [0f32; MAX_EDGE * MAX_EDGE + 8];
        let to_pixels = |p: &Point| -> (f32, f32) {
            (
                p.x as f32 * scale - left as f32,
                top as f32 - p.y as f32 * scale,
            )
        };
        let mut start = 0;
        for contour in 0..outline.contours {
            let end = outline.ends[contour];
            let points = &outline.points[start..end];
            start = end;
            if points.len() < 2 {
                continue;
            }
            flatten_contour(points, &to_pixels, &mut area, width, height);
        }
        // Prefix-sum each row into coverage.
        for y in 0..height {
            let mut accumulated = 0f32;
            for x in 0..width {
                accumulated += area[y * width + x];
                let value = abs(accumulated).min(1.0);
                bitmap.coverage[y * width + x] = (value * 255.0 + 0.5) as u8;
            }
        }
        bitmap.width = width;
        bitmap.height = height;
        bitmap.left = left;
        bitmap.top = top;
        true
    }
}

fn floor(value: f32) -> f32 {
    let truncated = value as i32 as f32;
    if truncated > value {
        truncated - 1.0
    } else {
        truncated
    }
}

fn ceil(value: f32) -> f32 {
    let truncated = value as i32 as f32;
    if truncated < value {
        truncated + 1.0
    } else {
        truncated
    }
}

fn abs(value: f32) -> f32 {
    if value < 0.0 { -value } else { value }
}

/// Walks one contour, turning its quadratic curves into short lines.
fn flatten_contour(
    points: &[Point],
    to_pixels: &impl Fn(&Point) -> (f32, f32),
    area: &mut [f32],
    width: usize,
    height: usize,
) {
    let count = points.len();
    let at = |index: usize| points[index % count];
    // Where the outline starts and which points follow it.
    let (start, first, follow) = if at(0).on_curve {
        (to_pixels(&at(0)), 1, count - 1)
    } else if at(count - 1).on_curve {
        (to_pixels(&at(count - 1)), 0, count - 1)
    } else {
        let a = to_pixels(&at(0));
        let b = to_pixels(&at(count - 1));
        (((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0), 0, count)
    };
    let mut current = start;
    let mut index = 0;
    while index < follow {
        let point = at(first + index);
        let position = to_pixels(&point);
        if point.on_curve {
            draw_line(current, position, area, width, height);
            current = position;
            index += 1;
            continue;
        }
        // A control point: the curve ends at the next on-curve point, or at
        // the midpoint towards the next control point.
        if index + 1 < follow {
            let next = at(first + index + 1);
            let next_position = to_pixels(&next);
            if next.on_curve {
                draw_quadratic(current, position, next_position, area, width, height);
                current = next_position;
                index += 2;
            } else {
                let middle = (
                    (position.0 + next_position.0) / 2.0,
                    (position.1 + next_position.1) / 2.0,
                );
                draw_quadratic(current, position, middle, area, width, height);
                current = middle;
                index += 1;
            }
        } else {
            draw_quadratic(current, position, start, area, width, height);
            current = start;
            index += 1;
        }
    }
    draw_line(current, start, area, width, height);
}

fn draw_quadratic(
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    area: &mut [f32],
    width: usize,
    height: usize,
) {
    // Number of segments from how far the control point bends the curve.
    let deviation_x = p0.0 - 2.0 * p1.0 + p2.0;
    let deviation_y = p0.1 - 2.0 * p1.1 + p2.1;
    let distance = abs(deviation_x) + abs(deviation_y);
    let segments = (ceil(distance * 0.75 + 1.0) as usize).clamp(2, 24);
    let mut previous = p0;
    for step in 1..=segments {
        let t = step as f32 / segments as f32;
        let u = 1.0 - t;
        let point = (
            u * u * p0.0 + 2.0 * u * t * p1.0 + t * t * p2.0,
            u * u * p0.1 + 2.0 * u * t * p1.1 + t * t * p2.1,
        );
        draw_line(previous, point, area, width, height);
        previous = point;
    }
}

/// Accumulates one line's signed coverage into the area buffer.
fn draw_line(p0: (f32, f32), p1: (f32, f32), area: &mut [f32], width: usize, height: usize) {
    if p0.1 == p1.1 {
        return;
    }
    let (direction, p0, p1) = if p0.1 < p1.1 {
        (1.0f32, p0, p1)
    } else {
        (-1.0f32, p1, p0)
    };
    let dxdy = (p1.0 - p0.0) / (p1.1 - p0.1);
    let mut x = p0.0;
    let y_start = if p0.1 < 0.0 { 0 } else { p0.1 as usize };
    if p0.1 < 0.0 {
        x -= p0.1 * dxdy;
    }
    let y_end = (ceil(p1.1).max(0.0) as usize).min(height);
    for y in y_start..y_end {
        let line = y * width;
        let dy = (y as f32 + 1.0).min(p1.1) - (y as f32).max(p0.1);
        let x_next = x + dxdy * dy;
        let delta = dy * direction;
        let (x0, x1) = if x < x_next { (x, x_next) } else { (x_next, x) };
        let x0_floor = floor(x0);
        let x0_index = x0_floor as i32;
        let x1_ceil = ceil(x1);
        let x1_index = x1_ceil as i32;
        let clamp = |i: i32| -> Option<usize> {
            (i >= 0 && (i as usize) < width).then_some(line + i as usize)
        };
        let mut add = |i: i32, value: f32| {
            // Coverage left/right of the bitmap doesn't matter.
            if let Some(at) = clamp(i) {
                area[at] += value;
            }
        };
        if x1_index <= x0_index + 1 {
            let x_mid_fraction = 0.5 * (x + x_next) - x0_floor;
            add(x0_index, delta - delta * x_mid_fraction);
            add(x0_index + 1, delta * x_mid_fraction);
        } else {
            let s = 1.0 / (x1 - x0);
            let x0_fraction = x0 - x0_floor;
            let a0 = 0.5 * s * (1.0 - x0_fraction) * (1.0 - x0_fraction);
            let x1_fraction = x1 - x1_ceil + 1.0;
            let am = 0.5 * s * x1_fraction * x1_fraction;
            add(x0_index, delta * a0);
            if x1_index == x0_index + 2 {
                add(x0_index + 1, delta * (1.0 - a0 - am));
            } else {
                let a1 = s * (1.5 - x0_fraction);
                add(x0_index + 1, delta * (a1 - a0));
                for column in x0_index + 2..x1_index - 1 {
                    add(column, delta * s);
                }
                let a2 = a1 + (x1_index - x0_index - 3) as f32 * s;
                add(x1_index - 1, delta * (1.0 - a2 - am));
            }
            add(x1_index, delta * am);
        }
        x = x_next;
    }
}
