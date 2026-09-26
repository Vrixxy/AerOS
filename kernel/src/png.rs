//! PNG decoding (all colour types and bit depths, tRNS, Adam7 interlacing)
//! and encoding (8-bit RGB/RGBA with adaptive filters and DEFLATE).

// Picture library for the Photos/Files/Screenshot apps (their UI is pending).
#![allow(dead_code)]

use crate::image::{Image, ImageError, MAX_EDGE, MAX_PIXELS};
use crate::inflate;
use crate::memory::PageBuffer;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

pub fn crc32(seed: u32, data: &[u8]) -> u32 {
    let mut crc = !seed;
    for byte in data {
        crc = CRC_TABLE[((crc ^ *byte as u32) & 0xff) as usize] ^ (crc >> 8);
    }
    !crc
}

struct Header {
    width: usize,
    height: usize,
    depth: u8,
    color: u8,
    interlaced: bool,
}

impl Header {
    fn channels(&self) -> usize {
        match self.color {
            0 | 3 => 1,
            2 => 3,
            4 => 2,
            _ => 4,
        }
    }

    fn bits_per_pixel(&self) -> usize {
        self.channels() * self.depth as usize
    }
}

/// (x start, y start, x step, y step) of each Adam7 pass.
const ADAM7: [(usize, usize, usize, usize); 7] = [
    (0, 0, 8, 8),
    (4, 0, 8, 8),
    (0, 4, 4, 8),
    (2, 0, 4, 4),
    (0, 2, 2, 4),
    (1, 0, 2, 2),
    (0, 1, 1, 2),
];

fn pass_size(header: &Header, pass: usize) -> (usize, usize) {
    if !header.interlaced {
        return (header.width, header.height);
    }
    let (x0, y0, dx, dy) = ADAM7[pass];
    (
        header.width.saturating_sub(x0).div_ceil(dx),
        header.height.saturating_sub(y0).div_ceil(dy),
    )
}

fn raw_size(header: &Header) -> usize {
    let passes = if header.interlaced { 7 } else { 1 };
    let mut total = 0;
    for pass in 0..passes {
        let (width, height) = pass_size(header, pass);
        if width > 0 && height > 0 {
            total += (width * header.bits_per_pixel()).div_ceil(8) * height + height;
        }
    }
    total
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (a16, b16, c16) = (a as i32, b as i32, c as i32);
    let estimate = a16 + b16 - c16;
    let (pa, pb, pc) = (
        (estimate - a16).abs(),
        (estimate - b16).abs(),
        (estimate - c16).abs(),
    );
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Reverses the PNG filter of one scanline in place. `previous` is the
/// unfiltered line above (all zeros for the first line).
fn unfilter(filter: u8, line: &mut [u8], previous: &[u8], bpp: usize) -> Result<(), ImageError> {
    match filter {
        0 => {}
        1 => {
            for i in bpp..line.len() {
                line[i] = line[i].wrapping_add(line[i - bpp]);
            }
        }
        2 => {
            for i in 0..line.len() {
                line[i] = line[i].wrapping_add(previous[i]);
            }
        }
        3 => {
            for i in 0..line.len() {
                let left = if i >= bpp { line[i - bpp] as u16 } else { 0 };
                line[i] = line[i].wrapping_add(((left + previous[i] as u16) / 2) as u8);
            }
        }
        4 => {
            for i in 0..line.len() {
                let left = if i >= bpp { line[i - bpp] } else { 0 };
                let up_left = if i >= bpp { previous[i - bpp] } else { 0 };
                line[i] = line[i].wrapping_add(paeth(left, previous[i], up_left));
            }
        }
        _ => return Err(ImageError::Corrupt),
    }
    Ok(())
}

pub fn decode(data: &[u8]) -> Result<Image, ImageError> {
    if !data.starts_with(&SIGNATURE) {
        return Err(ImageError::Unsupported);
    }
    let mut header: Option<Header> = None;
    let mut palette = [[0u8, 0, 0, 255]; 256];
    let mut palette_len = 0usize;
    // tRNS for gray / RGB: the colour (16-bit samples) that is transparent.
    let mut transparent: Option<[u16; 3]> = None;
    let mut idat_total = 0usize;
    let mut offset = 8;
    // First pass over the chunks: header, palette, transparency, IDAT size.
    while offset + 12 <= data.len() {
        let length = u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        let kind = &data[offset + 4..offset + 8];
        let body_start = offset + 8;
        let body_end = body_start.checked_add(length).ok_or(ImageError::Corrupt)?;
        if body_end + 4 > data.len() {
            return Err(ImageError::Corrupt);
        }
        let body = &data[body_start..body_end];
        match kind {
            b"IHDR" => {
                if length != 13 {
                    return Err(ImageError::Corrupt);
                }
                let width = u32::from_be_bytes([body[0], body[1], body[2], body[3]]) as usize;
                let height = u32::from_be_bytes([body[4], body[5], body[6], body[7]]) as usize;
                if width == 0 || height == 0 || width > MAX_EDGE || height > MAX_EDGE {
                    return Err(ImageError::TooLarge);
                }
                if width * height > MAX_PIXELS {
                    return Err(ImageError::TooLarge);
                }
                let (depth, color) = (body[8], body[9]);
                let valid = matches!(
                    (color, depth),
                    (0, 1 | 2 | 4 | 8 | 16)
                        | (2, 8 | 16)
                        | (3, 1 | 2 | 4 | 8)
                        | (4, 8 | 16)
                        | (6, 8 | 16)
                );
                if !valid || body[10] != 0 || body[11] != 0 || body[12] > 1 {
                    return Err(ImageError::Unsupported);
                }
                header = Some(Header {
                    width,
                    height,
                    depth,
                    color,
                    interlaced: body[12] == 1,
                });
            }
            b"PLTE" => {
                if !length.is_multiple_of(3) || length > 768 {
                    return Err(ImageError::Corrupt);
                }
                palette_len = length / 3;
                for index in 0..palette_len {
                    palette[index] = [
                        body[index * 3],
                        body[index * 3 + 1],
                        body[index * 3 + 2],
                        255,
                    ];
                }
            }
            b"tRNS" => match header.as_ref().map(|h| h.color) {
                Some(3) => {
                    for (index, alpha) in body.iter().enumerate().take(256) {
                        palette[index][3] = *alpha;
                    }
                }
                Some(0) if length >= 2 => {
                    transparent = Some([u16::from_be_bytes([body[0], body[1]]), 0, 0]);
                }
                Some(2) if length >= 6 => {
                    transparent = Some([
                        u16::from_be_bytes([body[0], body[1]]),
                        u16::from_be_bytes([body[2], body[3]]),
                        u16::from_be_bytes([body[4], body[5]]),
                    ]);
                }
                _ => {}
            },
            b"IDAT" => idat_total += length,
            _ => {}
        }
        if kind == b"IEND" {
            break;
        }
        offset = body_end + 4;
    }
    let header = header.ok_or(ImageError::Corrupt)?;
    if idat_total == 0 || (header.color == 3 && palette_len == 0) {
        return Err(ImageError::Corrupt);
    }

    // Gather the compressed data (IDAT chunks are one zlib stream).
    let mut compressed = PageBuffer::new(idat_total).ok_or(ImageError::OutOfMemory)?;
    {
        let target = compressed.as_mut_slice();
        let (mut at, mut offset) = (0usize, 8usize);
        while offset + 12 <= data.len() {
            let length = u32::from_be_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]) as usize;
            if &data[offset + 4..offset + 8] == b"IDAT" {
                target[at..at + length].copy_from_slice(&data[offset + 8..offset + 8 + length]);
                at += length;
            }
            offset += 12 + length;
        }
    }
    let expected = raw_size(&header);
    let mut raw = PageBuffer::new(expected).ok_or(ImageError::OutOfMemory)?;
    let produced = inflate::zlib_decompress(compressed.as_slice(), raw.as_mut_slice())
        .map_err(|_| ImageError::Corrupt)?;
    drop(compressed);
    if produced != expected {
        return Err(ImageError::Corrupt);
    }

    let mut image = Image::new(header.width, header.height)?;
    let bits_per_pixel = header.bits_per_pixel();
    let bpp = bits_per_pixel.div_ceil(8).max(1);
    let raw_bytes = raw.as_mut_slice();
    let output = image.rgba_mut();
    let passes = if header.interlaced { 7 } else { 1 };
    let mut cursor = 0usize;
    let mut previous = PageBuffer::new(header.width * 8 + 8).ok_or(ImageError::OutOfMemory)?;
    #[allow(clippy::needless_range_loop)]
    for pass in 0..passes {
        let (pass_width, pass_height) = pass_size(&header, pass);
        if pass_width == 0 || pass_height == 0 {
            continue;
        }
        let row_bytes = (pass_width * bits_per_pixel).div_ceil(8);
        previous.as_mut_slice()[..row_bytes].fill(0);
        for row in 0..pass_height {
            let start = cursor + 1;
            let filter = raw_bytes[cursor];
            let (before, line_and_after) = raw_bytes.split_at_mut(start);
            let _ = before;
            let line = &mut line_and_after[..row_bytes];
            unfilter(filter, line, &previous.as_slice()[..row_bytes], bpp)?;
            previous.as_mut_slice()[..row_bytes].copy_from_slice(line);
            // Convert this line to RGBA.
            let (x0, y0, dx, dy) = if header.interlaced {
                ADAM7[pass]
            } else {
                (0, 0, 1, 1)
            };
            let y = y0 + row * dy;
            for column in 0..pass_width {
                let x = x0 + column * dx;
                let pixel = pixel_rgba(&header, line, column, &palette, palette_len, transparent);
                let at = (y * header.width + x) * 4;
                output[at..at + 4].copy_from_slice(&pixel);
            }
            cursor = start + row_bytes;
        }
    }
    Ok(image)
}

/// The RGBA value of pixel `column` of an unfiltered scanline.
fn pixel_rgba(
    header: &Header,
    line: &[u8],
    column: usize,
    palette: &[[u8; 4]; 256],
    palette_len: usize,
    transparent: Option<[u16; 3]>,
) -> [u8; 4] {
    let depth = header.depth as usize;
    let sample = |channel: usize| -> u16 {
        let index = column * header.channels() + channel;
        match depth {
            8 => line[index] as u16,
            16 => u16::from_be_bytes([line[index * 2], line[index * 2 + 1]]),
            _ => {
                let bit = index * depth;
                let byte = line[bit / 8];
                let shift = 8 - depth - bit % 8;
                ((byte >> shift) & ((1 << depth) - 1)) as u16
            }
        }
    };
    let to8 = |value: u16| -> u8 {
        match depth {
            16 => (value >> 8) as u8,
            8 => value as u8,
            _ => (value as u32 * 255 / ((1u32 << depth) - 1)) as u8,
        }
    };
    match header.color {
        0 => {
            let value = sample(0);
            let gray = to8(value);
            let alpha = if transparent.is_some_and(|t| t[0] == value) {
                0
            } else {
                255
            };
            [gray, gray, gray, alpha]
        }
        2 => {
            let (r, g, b) = (sample(0), sample(1), sample(2));
            let alpha = if transparent.is_some_and(|t| t == [r, g, b]) {
                0
            } else {
                255
            };
            [to8(r), to8(g), to8(b), alpha]
        }
        3 => {
            let index = sample(0) as usize;
            if index < palette_len {
                palette[index]
            } else {
                [0, 0, 0, 255]
            }
        }
        4 => {
            let gray = to8(sample(0));
            [gray, gray, gray, to8(sample(1))]
        }
        _ => [
            to8(sample(0)),
            to8(sample(1)),
            to8(sample(2)),
            to8(sample(3)),
        ],
    }
}

// ------------------------------------------------------------------ encoder

/// Encodes 8-bit RGBA (or RGB when `alpha` is false: the source still holds
/// four bytes per pixel and the alpha channel is dropped) as a PNG file.
/// Returns the file bytes in a new buffer plus their length.
pub fn encode(
    width: usize,
    height: usize,
    rgba: &[u8],
    alpha: bool,
) -> Result<(PageBuffer, usize), ImageError> {
    if width == 0 || height == 0 || rgba.len() < width * height * 4 {
        return Err(ImageError::Corrupt);
    }
    let channels = if alpha { 4 } else { 3 };
    let row_bytes = width * channels;
    // Filtered scanlines.
    let mut filtered = PageBuffer::new((row_bytes + 1) * height).ok_or(ImageError::OutOfMemory)?;
    {
        let target = filtered.as_mut_slice();
        let mut row_buffer = PageBuffer::new(row_bytes * 2).ok_or(ImageError::OutOfMemory)?;
        let mut candidate = PageBuffer::new(row_bytes).ok_or(ImageError::OutOfMemory)?;
        let (current_area, previous_area) = row_buffer.as_mut_slice().split_at_mut(row_bytes);
        previous_area.fill(0);
        for y in 0..height {
            // Pack the source row.
            for x in 0..width {
                let from = (y * width + x) * 4;
                current_area[x * channels..x * channels + channels]
                    .copy_from_slice(&rgba[from..from + channels]);
            }
            // Try every filter; keep the one with the smallest sum of magnitudes.
            let mut best = (u64::MAX, 0u8);
            for filter in 0..5u8 {
                let out = candidate.as_mut_slice();
                for i in 0..row_bytes {
                    let left = if i >= channels {
                        current_area[i - channels]
                    } else {
                        0
                    };
                    let up = previous_area[i];
                    let up_left = if i >= channels {
                        previous_area[i - channels]
                    } else {
                        0
                    };
                    let predicted = match filter {
                        0 => 0,
                        1 => left,
                        2 => up,
                        3 => ((left as u16 + up as u16) / 2) as u8,
                        _ => paeth(left, up, up_left),
                    };
                    out[i] = current_area[i].wrapping_sub(predicted);
                }
                let cost: u64 = out.iter().map(|b| (*b as i8).unsigned_abs() as u64).sum();
                if cost < best.0 {
                    best = (cost, filter);
                    let base = y * (row_bytes + 1);
                    target[base] = filter;
                    target[base + 1..base + 1 + row_bytes].copy_from_slice(out);
                }
            }
            previous_area.copy_from_slice(current_area);
        }
    }
    // Compress.
    let mut compressed = PageBuffer::new(filtered.len() + filtered.len() / 8 + 1024)
        .ok_or(ImageError::OutOfMemory)?;
    let compressed_len =
        crate::deflate::zlib_compress(filtered.as_slice(), compressed.as_mut_slice())
            .ok_or(ImageError::TooLarge)?;
    drop(filtered);

    let total = 8 + (12 + 13) + (12 + compressed_len) + 12;
    let mut file = PageBuffer::new(total).ok_or(ImageError::OutOfMemory)?;
    let out = file.as_mut_slice();
    out[..8].copy_from_slice(&SIGNATURE);
    let mut at = 8;
    let mut chunk = |kind: &[u8; 4], body: &[u8], at: &mut usize| {
        out[*at..*at + 4].copy_from_slice(&(body.len() as u32).to_be_bytes());
        out[*at + 4..*at + 8].copy_from_slice(kind);
        out[*at + 8..*at + 8 + body.len()].copy_from_slice(body);
        let crc = crc32(crc32(0, kind), body);
        out[*at + 8 + body.len()..*at + 12 + body.len()].copy_from_slice(&crc.to_be_bytes());
        *at += 12 + body.len();
    };
    let mut header = [0u8; 13];
    header[..4].copy_from_slice(&(width as u32).to_be_bytes());
    header[4..8].copy_from_slice(&(height as u32).to_be_bytes());
    header[8] = 8;
    header[9] = if alpha { 6 } else { 2 };
    chunk(b"IHDR", &header, &mut at);
    chunk(b"IDAT", &compressed.as_slice()[..compressed_len], &mut at);
    chunk(b"IEND", &[], &mut at);
    Ok((file, at))
}
