use crate::framebuffer::{Color, FrameBuffer};

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
}

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
        })
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
        for byte in text.bytes() {
            if byte == b'\n' {
                cursor_x = x;
                cursor_y += line_height;
                continue;
            }
            let Some(index) = self.glyph_index(byte) else {
                cursor_x += drawn_width / 2;
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
                line.bytes()
                    .map(|byte| {
                        self.glyph_index(byte)
                            .map(|index| self.advance(index, height))
                            .unwrap_or(height / 2)
                    })
                    .sum()
            })
            .max()
            .unwrap_or(0)
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
            ui: RasterFont::parse(PLUS_JAKARTA_ATLAS),
            mono: RasterFont::parse(ROBOTO_MONO_ATLAS),
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
