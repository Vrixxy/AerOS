use crate::framebuffer::{Color, FrameBuffer};
use crate::sync::TicketLock;
use crate::truetype::{GlyphBitmap, TrueType};

const HEADER_SIZE: usize = 32;
const MAGIC: &[u8; 8] = b"AERFNT01";
const PLUS_JAKARTA_TTF: &[u8] = include_bytes!("../../assets/fonts/PlusJakartaSans-Regular.ttf");
const ROBOTO_MONO_TTF: &[u8] = include_bytes!("../../assets/fonts/RobotoMono-Variable.ttf");
const PLUS_JAKARTA_ATLAS: &[u8] =
    include_bytes!("../../assets/fonts/PlusJakartaSans-Regular.aerfont");
const ROBOTO_MONO_ATLAS: &[u8] = include_bytes!("../../assets/fonts/RobotoMono-Regular.aerfont");

#[derive(Clone, Copy)]
pub struct RasterFont {
    data: &'static [u8],
    first: u16,
    last: u16,
    source_width: usize,
    source_height: usize,
    bitmap_offset: usize,
    advance_offset: usize,
    /// The real font, for characters the ASCII atlas doesn't have.
    ttf: Option<TrueType>,
    /// Baseline row of the atlas cells (rows from the cell's top).
    baseline_row: i32,
}

/// A rendered non-atlas glyph kept for reuse (bitmaps up to 64x64).
#[derive(Clone, Copy)]
struct CachedGlyph {
    font: usize,
    code_point: u32,
    pixels_per_em: i32,
    width: usize,
    height: usize,
    left: i32,
    top: i32,
    advance: i32,
    coverage: [u8; 64 * 64],
}

const EMPTY_GLYPH: CachedGlyph = CachedGlyph {
    font: 0,
    code_point: 0,
    pixels_per_em: 0,
    width: 0,
    height: 0,
    left: 0,
    top: 0,
    advance: 0,
    coverage: [0; 64 * 64],
};

struct GlyphCache {
    entries: [CachedGlyph; 48],
    next: usize,
}

static GLYPH_CACHE: TicketLock<GlyphCache> = TicketLock::new(GlyphCache {
    entries: [EMPTY_GLYPH; 48],
    next: 0,
});

impl RasterFont {
    pub fn parse(data: &'static [u8]) -> Option<Self> {
        if data.len() < HEADER_SIZE || &data[..8] != MAGIC {
            return None;
        }
        let first = read_u16(data, 8)?;
        let last = read_u16(data, 10)?;
        let source_width = read_u16(data, 12)? as usize;
        let source_height = read_u16(data, 14)? as usize;
        let bitmap_offset = read_u32(data, 16)? as usize;
        let advance_offset = read_u32(data, 20)? as usize;
        if first > last || source_width == 0 || source_height == 0 {
            return None;
        }
        let glyph_count = (last - first) as usize + 1;
        let glyph_pixels = source_width.checked_mul(source_height)?;
        let bitmap_bytes = glyph_count.checked_mul(glyph_pixels)?;
        let bitmap_end = bitmap_offset.checked_add(bitmap_bytes)?;
        let advances_end = advance_offset.checked_add(glyph_count.checked_mul(2)?)?;
        if bitmap_offset < HEADER_SIZE
            || advance_offset < bitmap_end
            || bitmap_end > data.len()
            || advances_end > data.len()
        {
            return None;
        }
        Some(Self {
            data,
            first,
            last,
            source_width,
            source_height,
            bitmap_offset,
            advance_offset,
            ttf: None,
            baseline_row: 0,
        })
    }

    /// Adds the TrueType font (and the atlas baseline row it was rendered with)
    /// so characters beyond ASCII can be drawn.
    pub fn with_truetype(mut self, ttf: &'static [u8], baseline_row: i32) -> Self {
        self.ttf = TrueType::parse(ttf);
        self.baseline_row = baseline_row;
        self
    }

    /// Looks a non-atlas glyph up (rendering it if needed): bitmap, offsets and advance.
    fn truetype_glyph(&self, character: char, height: i32) -> Option<CachedGlyph> {
        let ttf = self.ttf.as_ref()?;
        let pixels_per_em = (height * 4 + 2) / 5;
        let font = self.data.as_ptr() as usize;
        let code_point = character as u32;
        let mut cache = GLYPH_CACHE.lock();
        if let Some(hit) = cache.entries.iter().find(|entry| {
            entry.font == font
                && entry.code_point == code_point
                && entry.pixels_per_em == pixels_per_em
        }) {
            return Some(*hit);
        }
        let glyph = ttf.glyph_index(code_point)?;
        let mut bitmap = GlyphBitmap::empty();
        if !ttf.rasterize(glyph, pixels_per_em, &mut bitmap) {
            return None;
        }
        let mut entry = CachedGlyph {
            font,
            code_point,
            pixels_per_em,
            width: bitmap.width,
            height: bitmap.height,
            left: bitmap.left,
            top: bitmap.top,
            advance: (ttf.advance(glyph) as i32 * pixels_per_em / ttf.units_per_em as i32).max(1),
            coverage: [0; 64 * 64],
        };
        if bitmap.width <= 64 && bitmap.height <= 64 {
            for row in 0..bitmap.height {
                entry.coverage[row * 64..row * 64 + bitmap.width].copy_from_slice(
                    &bitmap.coverage[row * bitmap.width..(row + 1) * bitmap.width],
                );
            }
            let slot = cache.next;
            cache.entries[slot] = entry;
            cache.next = (slot + 1) % cache.entries.len();
            return Some(entry);
        }
        // Too big to cache: draw straight from the rendering (width/height
        // beyond 64 are reported as an empty bitmap here).
        entry.width = 0;
        entry.height = 0;
        Some(entry)
    }

    /// Draws one non-atlas character; returns its advance.
    fn draw_truetype(
        &self,
        frame: &mut FrameBuffer,
        x: i32,
        y: i32,
        character: char,
        height: i32,
        color: Color,
    ) -> i32 {
        let Some(glyph) = self.truetype_glyph(character, height) else {
            return height / 2;
        };
        let pen_x = x + 2 * height / self.source_height as i32;
        let baseline = y + self.baseline_row * height / self.source_height as i32;
        for row in 0..glyph.height {
            for column in 0..glyph.width {
                let alpha = glyph.coverage[row * 64 + column];
                if alpha != 0 {
                    frame.blend(
                        pen_x + glyph.left + column as i32,
                        baseline - glyph.top + row as i32,
                        color,
                        alpha,
                    );
                }
            }
        }
        glyph.advance
    }

    pub fn draw(
        &self,
        frame: &mut FrameBuffer,
        x: i32,
        y: i32,
        text: &str,
        height: i32,
        color: Color,
    ) {
        if height <= 0 {
            return;
        }
        let mut cursor_x = x;
        let mut cursor_y = y;
        let drawn_width = scaled(self.source_width, height, self.source_height).max(1);
        let line_height = height + (height / 4).max(2);
        for character in text.chars() {
            if character == '\n' {
                cursor_x = x;
                cursor_y += line_height;
                continue;
            }
            let index = if character.is_ascii() {
                self.glyph_index(character as u8)
            } else {
                None
            };
            let Some(index) = index else {
                cursor_x += if self.ttf.is_some() && (character as u32) > 32 {
                    self.draw_truetype(frame, cursor_x, cursor_y, character, height, color)
                } else {
                    drawn_width / 2
                };
                continue;
            };
            let glyph_pixels = self.source_width * self.source_height;
            let glyph_start = self.bitmap_offset + index * glyph_pixels;
            for destination_y in 0..height {
                let source_y =
                    sample_position(destination_y as usize, height as usize, self.source_height);
                let y0 = source_y >> 16;
                let y1 = (y0 + 1).min(self.source_height - 1);
                let fy = (source_y & 0xffff) as u32;
                for destination_x in 0..drawn_width {
                    let source_x = sample_position(
                        destination_x as usize,
                        drawn_width as usize,
                        self.source_width,
                    );
                    let x0 = source_x >> 16;
                    let x1 = (x0 + 1).min(self.source_width - 1);
                    let fx = (source_x & 0xffff) as u32;
                    let top = interpolate_alpha(
                        self.data[glyph_start + y0 * self.source_width + x0],
                        self.data[glyph_start + y0 * self.source_width + x1],
                        fx,
                    );
                    let bottom = interpolate_alpha(
                        self.data[glyph_start + y1 * self.source_width + x0],
                        self.data[glyph_start + y1 * self.source_width + x1],
                        fx,
                    );
                    let alpha = interpolate_alpha(top, bottom, fy);
                    if alpha != 0 {
                        frame.blend(
                            cursor_x + destination_x,
                            cursor_y + destination_y,
                            color,
                            alpha,
                        );
                    }
                }
            }
            cursor_x += self.advance(index, height);
        }
    }

    pub fn text_width(&self, text: &str, height: i32) -> i32 {
        if height <= 0 {
            return 0;
        }
        text.split('\n')
            .map(|line| {
                line.chars()
                    .map(|character| {
                        if character.is_ascii()
                            && let Some(index) = self.glyph_index(character as u8)
                        {
                            self.advance(index, height)
                        } else if self.ttf.is_some() && (character as u32) > 32 {
                            self.truetype_glyph(character, height)
                                .map_or(height / 2, |glyph| glyph.advance)
                        } else {
                            height / 2
                        }
                    })
                    .sum()
            })
            .max()
            .unwrap_or(0)
    }

    /// The pre-rendered bitmap (`source_width * source_height` coverage bytes)
    /// of an ASCII character.
    #[cfg(feature = "boot-test")]
    fn atlas_glyph(&self, byte: u8) -> Option<&[u8]> {
        let index = self.glyph_index(byte)?;
        let size = self.source_width * self.source_height;
        let start = self.bitmap_offset + index * size;
        self.data.get(start..start + size)
    }

    fn glyph_index(&self, byte: u8) -> Option<usize> {
        let value = byte as u16;
        if value < self.first || value > self.last {
            None
        } else {
            Some((value - self.first) as usize)
        }
    }

    fn advance(&self, index: usize, height: i32) -> i32 {
        let offset = self.advance_offset + index * 2;
        let raw = u16::from_le_bytes([self.data[offset], self.data[offset + 1]]) as usize;
        scaled(raw.max(1), height, self.source_height).max(1)
    }
}

pub struct FontCatalog {
    ui: Option<RasterFont>,
    mono: Option<RasterFont>,
    ui_ttf_valid: bool,
    mono_ttf_valid: bool,
}

impl FontCatalog {
    pub fn load() -> Self {
        Self {
            ui: RasterFont::parse(PLUS_JAKARTA_ATLAS)
                .map(|font| font.with_truetype(PLUS_JAKARTA_TTF, 67)),
            mono: RasterFont::parse(ROBOTO_MONO_ATLAS)
                .map(|font| font.with_truetype(ROBOTO_MONO_TTF, 68)),
            ui_ttf_valid: valid_sfnt(PLUS_JAKARTA_TTF),
            mono_ttf_valid: valid_sfnt(ROBOTO_MONO_TTF),
        }
    }

    pub fn ui(&self) -> Option<RasterFont> {
        self.ui
    }

    pub fn mono(&self) -> Option<RasterFont> {
        self.mono
    }

    pub fn ui_ready(&self) -> bool {
        self.ui.is_some() && self.ui_ttf_valid
    }

    pub fn mono_ready(&self) -> bool {
        self.mono.is_some() && self.mono_ttf_valid
    }
}

fn valid_sfnt(data: &[u8]) -> bool {
    data.len() > 1024
        && (data.starts_with(&[0, 1, 0, 0])
            || data.starts_with(b"OTTO")
            || data.starts_with(b"true"))
}

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes([
        *data.get(offset)?,
        *data.get(offset + 1)?,
    ]))
}

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *data.get(offset)?,
        *data.get(offset + 1)?,
        *data.get(offset + 2)?,
        *data.get(offset + 3)?,
    ]))
}

fn scaled(value: usize, height: i32, source_height: usize) -> i32 {
    ((value as i64 * height as i64 + source_height as i64 - 1) / source_height as i64) as i32
}

fn sample_position(destination: usize, destination_size: usize, source_size: usize) -> usize {
    if destination_size <= 1 || source_size <= 1 {
        return 0;
    }
    destination * (source_size - 1) * 65_536 / (destination_size - 1)
}

fn interpolate_alpha(first: u8, second: u8, amount: u32) -> u8 {
    let inverse = 65_536u32.saturating_sub(amount);
    ((first as u32 * inverse + second as u32 * amount) >> 16) as u8
}

/// Renders characters with the TrueType rasterizer at the atlas's size and
/// compares them with the atlas glyphs (made by the host's text renderer from
/// the same fonts): the shapes must overlap almost perfectly at the best
/// alignment. Also checks that non-ASCII characters exist and render.
#[cfg(feature = "boot-test")]
pub fn truetype_self_test() {
    use crate::serial;
    use crate::truetype::{GlyphBitmap, TrueType};
    let fonts: [(&str, &'static [u8], &'static [u8]); 2] = [
        ("ui", PLUS_JAKARTA_TTF, PLUS_JAKARTA_ATLAS),
        ("mono", ROBOTO_MONO_TTF, ROBOTO_MONO_ATLAS),
    ];
    let mut bitmap = GlyphBitmap::empty();
    let mut all_ok = true;
    for (name, ttf_data, atlas_data) in fonts {
        let (Some(ttf), Some(atlas)) = (TrueType::parse(ttf_data), RasterFont::parse(atlas_data))
        else {
            serial::format(format_args!(
                "AEROS_TRUETYPE font={} parsed=false verified=false\n",
                name
            ));
            all_ok = false;
            continue;
        };
        let mut worst = 1000u32;
        let mut shift_report = (0i32, 0i32);
        for character in ['A', 'B', 'g', 'a', '@', '%', '8', 'W'] {
            let Some(glyph) = ttf.glyph_index(character as u32) else {
                worst = 0;
                continue;
            };
            let Some(reference) = atlas.atlas_glyph(character as u8) else {
                worst = 0;
                continue;
            };
            if !ttf.rasterize(glyph, 64, &mut bitmap) {
                worst = 0;
                continue;
            }
            // Best overlap over a range of alignments.
            let (mut best, mut best_shift) = (0u32, (0i32, 0i32));
            for dy in -10i32..=45 {
                for dx in -8i32..=8 {
                    let (mut inside, mut union) = (0u32, 0u32);
                    for y in 0..atlas.source_height as i32 {
                        for x in 0..atlas.source_width as i32 {
                            let expected =
                                reference[y as usize * atlas.source_width + x as usize] > 127;
                            let (bx, by) = (x - dx, y - dy);
                            let ours = bx >= 0
                                && by >= 0
                                && (bx as usize) < bitmap.width
                                && (by as usize) < bitmap.height
                                && bitmap.coverage[by as usize * bitmap.width + bx as usize] > 127;
                            if expected && ours {
                                inside += 1;
                            }
                            if expected || ours {
                                union += 1;
                            }
                        }
                    }
                    let score = (inside * 1000).checked_div(union).unwrap_or(0);
                    if score > best {
                        best = score;
                        best_shift = (dx, dy);
                    }
                }
            }
            worst = worst.min(best);
            shift_report = best_shift;
        }
        // Non-ASCII coverage: Latin-1 and Latin Extended-A; Roboto Mono also has Greek and Cyrillic.
        let latin = ['\u{e9}', '\u{fc}', '\u{f1}', '\u{df}', '\u{15f}'];
        let non_ascii = latin.iter().all(|c| {
            ttf.glyph_index(*c as u32).is_some_and(|g| {
                ttf.rasterize(g, 32, &mut bitmap) && bitmap.width > 0 && bitmap.height > 0
            })
        });
        let scripts = if name == "mono" {
            ['\u{416}', '\u{3a3}']
                .iter()
                .all(|c| ttf.glyph_index(*c as u32).is_some())
        } else {
            true
        };
        let ok = worst >= 800 && non_ascii && scripts;
        all_ok &= ok;
        serial::format(format_args!(
            "AEROS_TRUETYPE font={} min_overlap_permille={} last_shift={},{} non_ascii={} scripts={} verified={}\n",
            name, worst, shift_report.0, shift_report.1, non_ascii, scripts, ok
        ));
    }
    // The layout side: accented characters have widths like their base letters.
    let catalog = FontCatalog::load();
    let layout_ok = match (catalog.ui(), catalog.mono()) {
        (Some(ui), Some(mono)) => {
            let plain = ui.text_width("cafe", 32);
            let accented = ui.text_width("caf\u{e9}", 32);
            let mono_plain = mono.text_width("e", 32);
            let mono_accented = mono.text_width("\u{e9}", 32);
            (accented - plain).abs() <= 3 && (mono_accented - mono_plain).abs() <= 2 && accented > 0
        }
        _ => false,
    };
    serial::format(format_args!("AEROS_TEXT_UNICODE layout={}\n", layout_ok));
    all_ok &= layout_ok;
    if !all_ok {
        serial::line("AEROS_TRUETYPE_INVARIANT_FAILURE");
        crate::arch::halt_forever();
    }
}
