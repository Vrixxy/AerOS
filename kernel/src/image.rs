//! Decoded pictures and the entry points for reading/writing image files:
//! PNG, JPEG and BMP in, PNG out. Pixels are straight-alpha RGBA bytes
//! (the layout `aerui::draw_rgba_scaled` takes).

// Picture library for the Photos/Files/Screenshot apps (their UI is pending).
#![allow(dead_code)]

use crate::memory::PageBuffer;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImageError {
    /// Not a format this decoder knows.
    Unsupported,
    /// Damaged or truncated file.
    Corrupt,
    /// Bigger than the decoder allows.
    TooLarge,
    /// Not enough free memory for the pixels.
    OutOfMemory,
}

/// Largest picture edge and pixel count accepted (a 100 MP photo is 400 MB).
pub const MAX_EDGE: usize = 16_384;
pub const MAX_PIXELS: usize = 64 * 1024 * 1024;

pub struct Image {
    pub width: usize,
    pub height: usize,
    pixels: PageBuffer,
}

impl Image {
    /// A transparent black picture of the given size.
    pub fn new(width: usize, height: usize) -> Result<Self, ImageError> {
        if width == 0 || height == 0 || width > MAX_EDGE || height > MAX_EDGE {
            return Err(ImageError::Corrupt);
        }
        let pixels = width.checked_mul(height).ok_or(ImageError::TooLarge)?;
        if pixels > MAX_PIXELS {
            return Err(ImageError::TooLarge);
        }
        let buffer = PageBuffer::new(pixels * 4).ok_or(ImageError::OutOfMemory)?;
        Ok(Self {
            width,
            height,
            pixels: buffer,
        })
    }

    /// RGBA bytes, row by row (`width * height * 4`).
    pub fn rgba(&self) -> &[u8] {
        self.pixels.as_slice()
    }

    pub fn rgba_mut(&mut self) -> &mut [u8] {
        self.pixels.as_mut_slice()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Png,
    Jpeg,
    Bmp,
}

/// Recognizes a format from the file's first bytes.
pub fn detect(data: &[u8]) -> Option<Format> {
    if data.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some(Format::Png)
    } else if data.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(Format::Jpeg)
    } else if data.starts_with(b"BM") {
        Some(Format::Bmp)
    } else {
        None
    }
}

/// Decodes any supported picture file.
pub fn decode(data: &[u8]) -> Result<Image, ImageError> {
    match detect(data) {
        Some(Format::Png) => crate::png::decode(data),
        Some(Format::Jpeg) => crate::jpeg::decode(data),
        Some(Format::Bmp) => decode_bmp(data),
        None => Err(ImageError::Unsupported),
    }
}

/// Just the dimensions (for listings), without decoding the pixels.
pub fn dimensions(data: &[u8]) -> Option<(usize, usize)> {
    match detect(data)? {
        Format::Png => {
            if data.len() < 24 {
                return None;
            }
            Some((
                u32::from_be_bytes([data[16], data[17], data[18], data[19]]) as usize,
                u32::from_be_bytes([data[20], data[21], data[22], data[23]]) as usize,
            ))
        }
        Format::Jpeg => crate::jpeg::dimensions(data),
        Format::Bmp => {
            if data.len() < 26 {
                return None;
            }
            let width = i32::from_le_bytes([data[18], data[19], data[20], data[21]]);
            let height = i32::from_le_bytes([data[22], data[23], data[24], data[25]]);
            Some((
                width.unsigned_abs() as usize,
                height.unsigned_abs() as usize,
            ))
        }
    }
}

fn le16(data: &[u8], at: usize) -> Option<u32> {
    Some(u16::from_le_bytes([*data.get(at)?, *data.get(at + 1)?]) as u32)
}

fn le32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *data.get(at)?,
        *data.get(at + 1)?,
        *data.get(at + 2)?,
        *data.get(at + 3)?,
    ]))
}

/// Uncompressed BMP (BITMAPINFOHEADER and later): 1/4/8-bit palette, 16-bit
/// 5-5-5, 24-bit and 32-bit (with or without alpha).
fn decode_bmp(data: &[u8]) -> Result<Image, ImageError> {
    let pixel_offset = le32(data, 10).ok_or(ImageError::Corrupt)? as usize;
    let header_size = le32(data, 14).ok_or(ImageError::Corrupt)? as usize;
    if header_size < 40 {
        return Err(ImageError::Unsupported);
    }
    let width = le32(data, 18).ok_or(ImageError::Corrupt)? as i32;
    let raw_height = le32(data, 22).ok_or(ImageError::Corrupt)? as i32;
    let bits = le16(data, 28).ok_or(ImageError::Corrupt)?;
    let compression = le32(data, 30).ok_or(ImageError::Corrupt)?;
    let colors_used = le32(data, 46).ok_or(ImageError::Corrupt)? as usize;
    // 0 = BI_RGB, 3 = BI_BITFIELDS (only the usual 8-8-8(-8) masks are handled).
    if compression != 0 && compression != 3 {
        return Err(ImageError::Unsupported);
    }
    if width <= 0 || raw_height == 0 {
        return Err(ImageError::Corrupt);
    }
    let (width, height) = (width as usize, raw_height.unsigned_abs() as usize);
    let top_down = raw_height < 0;
    let mut image = Image::new(width, height)?;
    let palette_at = 14 + header_size;
    let palette_len = if bits <= 8 {
        if colors_used == 0 {
            1 << bits
        } else {
            colors_used
        }
    } else {
        0
    };
    let row_bytes = (width * bits as usize).div_ceil(32) * 4;
    let output = image.rgba_mut();
    for row in 0..height {
        let source_row = if top_down { row } else { height - 1 - row };
        let start = pixel_offset + source_row * row_bytes;
        let line = data
            .get(start..start + row_bytes)
            .ok_or(ImageError::Corrupt)?;
        for x in 0..width {
            let at = (row * width + x) * 4;
            let (r, g, b, a) = match bits {
                1 | 4 | 8 => {
                    let per_byte = 8 / bits as usize;
                    let byte = line[x / per_byte];
                    let shift = 8 - bits as usize * (x % per_byte + 1);
                    let index = ((byte >> shift) & ((1 << bits) - 1)) as usize;
                    if index >= palette_len {
                        return Err(ImageError::Corrupt);
                    }
                    let entry = data
                        .get(palette_at + index * 4..palette_at + index * 4 + 4)
                        .ok_or(ImageError::Corrupt)?;
                    (entry[2], entry[1], entry[0], 255)
                }
                16 => {
                    let value = u16::from_le_bytes([line[x * 2], line[x * 2 + 1]]);
                    let expand = |v: u16| ((v << 3) | (v >> 2)) as u8;
                    (
                        expand((value >> 10) & 31),
                        expand((value >> 5) & 31),
                        expand(value & 31),
                        255,
                    )
                }
                24 => (line[x * 3 + 2], line[x * 3 + 1], line[x * 3], 255),
                32 => {
                    let alpha = if compression == 3 {
                        line[x * 4 + 3]
                    } else {
                        255
                    };
                    (line[x * 4 + 2], line[x * 4 + 1], line[x * 4], alpha)
                }
                _ => return Err(ImageError::Unsupported),
            };
            output[at] = r;
            output[at + 1] = g;
            output[at + 2] = b;
            output[at + 3] = a;
        }
    }
    Ok(image)
}

#[cfg(feature = "boot-test")]
const TEST_PNG: &[u8] = include_bytes!("../../assets/wallpapers/aeros-mountains.png");
#[cfg(feature = "boot-test")]
const PNG_CORPUS: &[u8] = include_bytes!("../../assets/test/png-corpus.bin");
#[cfg(feature = "boot-test")]
const TEST_BASELINE: &[u8] = include_bytes!("../../assets/test/photo-small.jpg");
#[cfg(feature = "boot-test")]
const TEST_PROGRESSIVE: &[u8] = include_bytes!("../../assets/test/photo-small-progressive.jpg");
#[cfg(feature = "boot-test")]
const TEST_JPEG: &[u8] = include_bytes!("../../assets/wallpapers/aeros-mountains.jpeg");

/// Decodes the bundled wallpaper files and prints signatures of the results
/// (`tools/test.ps1` recomputes them with the host's decoders and compares),
/// plus encode/decode round trips.
#[cfg(feature = "boot-test")]
pub fn self_test() {
    use crate::serial;
    // PNG: exact decode, then encode -> decode must reproduce the pixels.
    let mut png_ok = false;
    let mut png_crc = 0u32;
    let mut roundtrip = false;
    let mut encoded_bytes = 0usize;
    let (mut png_w, mut png_h) = (0, 0);
    if let Ok(image) = decode(TEST_PNG) {
        png_w = image.width;
        png_h = image.height;
        png_crc = crate::png::crc32(0, image.rgba());
        png_ok = true;
        if let Ok((file, length)) =
            crate::png::encode(image.width, image.height, image.rgba(), true)
        {
            encoded_bytes = length;
            if let Ok(again) = decode(&file.as_slice()[..length]) {
                roundtrip = again.width == image.width
                    && again.height == image.height
                    && again.rgba() == image.rgba();
            }
        }
    }
    serial::format(format_args!(
        "AEROS_IMAGE_PNG width={} height={} crc={:#010x} roundtrip={} encoded_bytes={} verified={}
",
        png_w,
        png_h,
        png_crc,
        roundtrip,
        encoded_bytes,
        png_ok && roundtrip
    ));
    if !(png_ok && roundtrip) {
        serial::line("AEROS_IMAGE_INVARIANT_FAILURE");
        crate::arch::halt_forever();
    }

    // JPEG: decode the 9 MP baseline photo; print a 32x18 grid of cell luma averages.
    let start = crate::time::monotonic_nanoseconds();
    let decoded = decode(TEST_JPEG);
    let elapsed_ms = crate::time::monotonic_nanoseconds().saturating_sub(start) / 1_000_000;
    match decoded {
        Ok(image) => {
            let (columns, rows) = (32usize, 18usize);
            serial::format(format_args!(
                "AEROS_IMAGE_JPEG width={} height={} decode_ms={} grid=",
                image.width, image.height, elapsed_ms
            ));
            let pixels = image.rgba();
            for row in 0..rows {
                for column in 0..columns {
                    let (x0, x1) = (
                        column * image.width / columns,
                        (column + 1) * image.width / columns,
                    );
                    let (y0, y1) = (row * image.height / rows, (row + 1) * image.height / rows);
                    let mut sum = 0u64;
                    let mut count = 0u64;
                    // Sample every 7th pixel: plenty for an average.
                    let mut y = y0;
                    while y < y1 {
                        let mut x = x0;
                        while x < x1 {
                            let at = (y * image.width + x) * 4;
                            sum += (pixels[at] as u64 * 299
                                + pixels[at + 1] as u64 * 587
                                + pixels[at + 2] as u64 * 114)
                                / 1000;
                            count += 1;
                            x += 7;
                        }
                        y += 7;
                    }
                    serial::format(format_args!("{:02x}", (sum / count.max(1)) as u8));
                }
            }
            serial::line(" verified=true");
        }
        Err(error) => {
            serial::format(format_args!(
                "AEROS_IMAGE_JPEG error={:?} verified=false
",
                error
            ));
            serial::line("AEROS_IMAGE_INVARIANT_FAILURE");
            crate::arch::halt_forever();
        }
    }

    // The PNG corpus: every colour type, bit depth, interlace mode and filter.
    let (mut cases, mut passed) = (0u32, 0u32);
    let mut at = 0usize;
    while at + 8 <= PNG_CORPUS.len() {
        let length = u32::from_le_bytes([
            PNG_CORPUS[at],
            PNG_CORPUS[at + 1],
            PNG_CORPUS[at + 2],
            PNG_CORPUS[at + 3],
        ]) as usize;
        let crc = u32::from_le_bytes([
            PNG_CORPUS[at + 4],
            PNG_CORPUS[at + 5],
            PNG_CORPUS[at + 6],
            PNG_CORPUS[at + 7],
        ]);
        let file = &PNG_CORPUS[at + 8..at + 8 + length];
        cases += 1;
        if decode(file).is_ok_and(|image| crate::png::crc32(0, image.rgba()) == crc) {
            passed += 1;
        }
        at += 8 + length;
    }
    serial::format(format_args!(
        "AEROS_IMAGE_PNG_CORPUS cases={} passed={} verified={}
",
        cases,
        passed,
        cases > 0 && cases == passed
    ));
    if cases == 0 || cases != passed {
        serial::line("AEROS_IMAGE_INVARIANT_FAILURE");
        crate::arch::halt_forever();
    }

    // Progressive JPEG: the same coefficients as the baseline file (see
    // tools/make-progressive.py), so the two must decode to identical pixels.
    let progressive_ok = match (decode(TEST_BASELINE), decode(TEST_PROGRESSIVE)) {
        (Ok(base), Ok(progressive)) => {
            base.width == progressive.width
                && base.height == progressive.height
                && base.rgba() == progressive.rgba()
        }
        _ => false,
    };
    serial::format(format_args!(
        "AEROS_IMAGE_PROGRESSIVE identical={}
",
        progressive_ok
    ));
    if !progressive_ok {
        serial::line("AEROS_IMAGE_INVARIANT_FAILURE");
        crate::arch::halt_forever();
    }

    // BMP: a hand-made 2x2, 24-bit, bottom-up picture.
    let mut bmp = [0u8; 70];
    bmp[..2].copy_from_slice(b"BM");
    bmp[2..6].copy_from_slice(&70u32.to_le_bytes());
    bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
    bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
    bmp[18..22].copy_from_slice(&2i32.to_le_bytes());
    bmp[22..26].copy_from_slice(&2i32.to_le_bytes());
    bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
    bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
    // Bottom row first (BGR): blue, white; then top row: red, green.
    bmp[54..60].copy_from_slice(&[255, 0, 0, 255, 255, 255]);
    bmp[62..68].copy_from_slice(&[0, 0, 255, 0, 255, 0]);
    let bmp_ok = decode(&bmp).is_ok_and(|image| {
        image.width == 2
            && image.height == 2
            && image.rgba()[..4] == [255, 0, 0, 255]
            && image.rgba()[4..8] == [0, 255, 0, 255]
            && image.rgba()[8..12] == [0, 0, 255, 255]
            && image.rgba()[12..16] == [255, 255, 255, 255]
    });
    serial::format(format_args!(
        "AEROS_IMAGE_BMP verified={}
",
        bmp_ok
    ));
    if !bmp_ok {
        serial::line("AEROS_IMAGE_INVARIANT_FAILURE");
        crate::arch::halt_forever();
    }
}
