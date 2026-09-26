//! JPEG decoding: baseline and progressive Huffman-coded images (8-bit,
//! grayscale or YCbCr/RGB with any usual subsampling), restart intervals,
//! EXIF orientation. Integer arithmetic only (no floating point in the kernel).

// Picture library for the Photos/Files/Screenshot apps (their UI is pending).
#![allow(dead_code)]

use crate::image::{Image, ImageError, MAX_EDGE, MAX_PIXELS};
use crate::memory::PageBuffer;

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

#[derive(Clone, Copy)]
struct Huffman {
    valid: bool,
    /// Largest code of each length (or -1), and where its symbols start.
    maxcode: [i32; 18],
    valptr: [i32; 17],
    mincode: [i32; 17],
    values: [u8; 256],
    /// 8-bit lookahead: (length << 8) | symbol, 0 when the code is longer.
    lookahead: [u16; 256],
}

impl Huffman {
    const EMPTY: Self = Self {
        valid: false,
        maxcode: [-1; 18],
        valptr: [0; 17],
        mincode: [0; 17],
        values: [0; 256],
        lookahead: [0; 256],
    };

    fn build(counts: &[u8; 16], values: &[u8]) -> Option<Self> {
        let mut table = Self::EMPTY;
        let total: usize = counts.iter().map(|c| *c as usize).sum();
        if total > 256 || values.len() < total {
            return None;
        }
        table.values[..total].copy_from_slice(&values[..total]);
        let mut code = 0i32;
        let mut index = 0i32;
        for length in 1..=16usize {
            table.valptr[length] = index;
            table.mincode[length] = code;
            let count = counts[length - 1] as i32;
            if count > 0 {
                for offset in 0..count {
                    let value = code + offset;
                    if length <= 8 {
                        let base = (value as usize) << (8 - length);
                        let symbol = table.values[(index + offset) as usize];
                        for fill in 0..(1usize << (8 - length)) {
                            if base + fill < 256 {
                                table.lookahead[base + fill] = (length as u16) << 8 | symbol as u16;
                            }
                        }
                    }
                }
                code += count;
                index += count;
                table.maxcode[length] = code - 1;
            } else {
                table.maxcode[length] = -1;
            }
            if code > (1 << length) {
                return None;
            }
            code <<= 1;
        }
        table.maxcode[17] = i32::MAX;
        table.valid = true;
        Some(table)
    }
}

struct Component {
    id: u8,
    h: usize,
    v: usize,
    quant: usize,
    /// Size in blocks, padded to whole MCUs.
    blocks_w: usize,
    blocks_h: usize,
    /// Blocks actually coded in a non-interleaved scan.
    real_w: usize,
    real_h: usize,
    /// Offset of this component's first coefficient (in i16 units).
    coefficient_offset: usize,
    dc_dest: usize,
    ac_dest: usize,
    predictor: i32,
}

struct Reader<'a> {
    data: &'a [u8],
    position: usize,
    accumulator: u32,
    count: u32,
    hit_marker: bool,
    eob_run: u32,
}

impl<'a> Reader<'a> {
    fn fill(&mut self) {
        while self.count <= 24 {
            let mut byte = 0u8;
            if !self.hit_marker && self.position < self.data.len() {
                let value = self.data[self.position];
                if value == 0xff {
                    match self.data.get(self.position + 1) {
                        Some(0) => {
                            byte = 0xff;
                            self.position += 2;
                        }
                        Some(_) | None => {
                            // A marker: stay put and feed zeros.
                            self.hit_marker = true;
                        }
                    }
                } else {
                    byte = value;
                    self.position += 1;
                }
            } else {
                self.hit_marker = true;
            }
            self.accumulator |= (byte as u32) << (24 - self.count);
            self.count += 8;
        }
    }

    fn bits(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        if self.count < n {
            self.fill();
        }
        let value = self.accumulator >> (32 - n);
        self.accumulator <<= n;
        self.count -= n;
        value
    }

    fn bit(&mut self) -> u32 {
        self.bits(1)
    }

    fn decode(&mut self, table: &Huffman) -> Result<u8, ImageError> {
        if self.count < 16 {
            self.fill();
        }
        let look = table.lookahead[(self.accumulator >> 24) as usize];
        if look != 0 {
            let length = (look >> 8) as u32;
            self.accumulator <<= length;
            self.count -= length;
            return Ok(look as u8);
        }
        let mut code = (self.accumulator >> 24) as i32;
        let mut length = 8usize;
        // Continue bit by bit past the lookahead.
        let mut extra_bits = 8u32;
        loop {
            length += 1;
            if length > 16 {
                return Err(ImageError::Corrupt);
            }
            code = (code << 1) | ((self.accumulator >> (31 - extra_bits)) & 1) as i32;
            extra_bits += 1;
            if table.maxcode[length] >= 0
                && code <= table.maxcode[length]
                && code >= table.mincode[length]
            {
                self.accumulator <<= length as u32;
                self.count -= length as u32;
                let index = table.valptr[length] + code - table.mincode[length];
                return Ok(table.values[index as usize & 255]);
            }
        }
    }

    /// Receives and sign-extends an `s`-bit coefficient.
    fn receive_extend(&mut self, s: u32) -> i32 {
        if s == 0 {
            return 0;
        }
        let value = self.bits(s) as i32;
        if value < (1 << (s - 1)) {
            value - (1 << s) + 1
        } else {
            value
        }
    }

    fn restart(&mut self) {
        self.accumulator = 0;
        self.count = 0;
        self.eob_run = 0;
        // Skip to (and past) the RSTn marker.
        let mut at = self.position;
        while at + 1 < self.data.len() {
            if self.data[at] == 0xff && (0xd0..=0xd7).contains(&self.data[at + 1]) {
                at += 2;
                break;
            }
            if self.data[at] == 0xff && self.data[at + 1] != 0 && self.data[at + 1] != 0xff {
                break; // some other marker: leave it for the segment parser
            }
            at += 1;
        }
        self.position = at;
        self.hit_marker = false;
    }
}

struct Decoder<'a> {
    data: &'a [u8],
    width: usize,
    height: usize,
    progressive: bool,
    components: [Component; 3],
    component_count: usize,
    quant: [[u16; 64]; 4],
    dc_tables: [Huffman; 4],
    ac_tables: [Huffman; 4],
    restart_interval: usize,
    max_h: usize,
    max_v: usize,
    mcus_x: usize,
    mcus_y: usize,
    orientation: u8,
    adobe_transform: Option<u8>,
    coefficients: Option<PageBuffer>,
}

const EMPTY_COMPONENT: Component = Component {
    id: 0,
    h: 1,
    v: 1,
    quant: 0,
    blocks_w: 0,
    blocks_h: 0,
    real_w: 0,
    real_h: 0,
    coefficient_offset: 0,
    dc_dest: 0,
    ac_dest: 0,
    predictor: 0,
};

fn be16(data: &[u8], at: usize) -> Option<usize> {
    Some(((*data.get(at)? as usize) << 8) | *data.get(at + 1)? as usize)
}

/// (width, height) without decoding.
pub fn dimensions(data: &[u8]) -> Option<(usize, usize)> {
    let mut at = 2;
    while at + 4 <= data.len() {
        if data[at] != 0xff {
            at += 1;
            continue;
        }
        let marker = data[at + 1];
        if marker == 0xff || marker == 0 || (0xd0..=0xd8).contains(&marker) {
            at += if marker == 0xff { 1 } else { 2 };
            continue;
        }
        let length = be16(data, at + 2)?;
        if (0xc0..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker) {
            let height = be16(data, at + 5)?;
            let width = be16(data, at + 7)?;
            let orientation = exif_orientation(data).unwrap_or(1);
            return Some(if orientation >= 5 {
                (height, width)
            } else {
                (width, height)
            });
        }
        at += 2 + length;
    }
    None
}

/// The EXIF orientation tag (1..=8) from the first APP1 segment, if any.
fn exif_orientation(data: &[u8]) -> Option<u8> {
    let mut at = 2;
    while at + 4 <= data.len() && data[at] == 0xff {
        let marker = data[at + 1];
        let length = be16(data, at + 2)?;
        if marker == 0xe1 && data.get(at + 4..at + 10) == Some(b"Exif\0\0") {
            let tiff = at + 10;
            let little = match data.get(tiff..tiff + 2)? {
                b"II" => true,
                b"MM" => false,
                _ => return None,
            };
            let read16 = |offset: usize| -> Option<u32> {
                let bytes = data.get(tiff + offset..tiff + offset + 2)?;
                Some(if little {
                    u16::from_le_bytes([bytes[0], bytes[1]]) as u32
                } else {
                    u16::from_be_bytes([bytes[0], bytes[1]]) as u32
                })
            };
            let read32 = |offset: usize| -> Option<u32> {
                let bytes = data.get(tiff + offset..tiff + offset + 4)?;
                Some(if little {
                    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
                } else {
                    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
                })
            };
            let directory = read32(4)? as usize;
            let entries = read16(directory)? as usize;
            for entry in 0..entries.min(64) {
                let base = directory + 2 + entry * 12;
                if read16(base)? == 0x0112 {
                    let value = read16(base + 8)? as u8;
                    return (1..=8).contains(&value).then_some(value);
                }
            }
            return None;
        }
        at += 2 + length;
    }
    None
}

pub fn decode(data: &[u8]) -> Result<Image, ImageError> {
    let mut decoder = Decoder {
        data,
        width: 0,
        height: 0,
        progressive: false,
        components: [EMPTY_COMPONENT, EMPTY_COMPONENT, EMPTY_COMPONENT],
        component_count: 0,
        quant: [[1; 64]; 4],
        dc_tables: [Huffman::EMPTY; 4],
        ac_tables: [Huffman::EMPTY; 4],
        restart_interval: 0,
        max_h: 1,
        max_v: 1,
        mcus_x: 0,
        mcus_y: 0,
        orientation: exif_orientation(data).unwrap_or(1),
        adobe_transform: None,
        coefficients: None,
    };
    decoder.parse()?;
    decoder.render()
}

impl Decoder<'_> {
    fn parse(&mut self) -> Result<(), ImageError> {
        let data = self.data;
        let mut at = 2usize;
        let mut have_frame = false;
        let mut scans = 0;
        loop {
            // Find the next marker.
            while at < data.len() && data[at] != 0xff {
                at += 1;
            }
            while at < data.len() && data[at] == 0xff {
                at += 1;
            }
            if at >= data.len() {
                break;
            }
            let marker = data[at];
            at += 1;
            match marker {
                0xd8 | 0x01 | 0xd0..=0xd7 | 0x00 => continue,
                0xd9 => break,
                _ => {}
            }
            let length = be16(data, at).ok_or(ImageError::Corrupt)?;
            if length < 2 || at + length > data.len() {
                return Err(ImageError::Corrupt);
            }
            let body = &data[at + 2..at + length];
            match marker {
                0xdb => self.read_quant(body)?,
                0xc4 => self.read_huffman(body)?,
                0xdd => {
                    self.restart_interval = be16(body, 0).ok_or(ImageError::Corrupt)?;
                }
                0xee if body.len() >= 12 && &body[..5] == b"Adobe" => {
                    self.adobe_transform = Some(body[11]);
                }
                0xc0..=0xc2 => {
                    if have_frame {
                        return Err(ImageError::Unsupported);
                    }
                    self.progressive = marker == 0xc2;
                    self.read_frame(body)?;
                    have_frame = true;
                }
                0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                    return Err(ImageError::Unsupported);
                }
                0xda => {
                    if !have_frame {
                        return Err(ImageError::Corrupt);
                    }
                    let end = self.decode_scan(body, at + length)?;
                    scans += 1;
                    at = end;
                    continue;
                }
                _ => {}
            }
            at += length;
        }
        if !have_frame || scans == 0 {
            return Err(ImageError::Corrupt);
        }
        Ok(())
    }

    fn read_quant(&mut self, mut body: &[u8]) -> Result<(), ImageError> {
        while !body.is_empty() {
            let precision = body[0] >> 4;
            let id = (body[0] & 15) as usize;
            if id > 3 {
                return Err(ImageError::Corrupt);
            }
            let size = if precision == 0 { 64 } else { 128 };
            if body.len() < 1 + size {
                return Err(ImageError::Corrupt);
            }
            for index in 0..64 {
                let value = if precision == 0 {
                    body[1 + index] as u16
                } else {
                    (body[1 + index * 2] as u16) << 8 | body[2 + index * 2] as u16
                };
                self.quant[id][ZIGZAG[index]] = value;
            }
            body = &body[1 + size..];
        }
        Ok(())
    }

    fn read_huffman(&mut self, mut body: &[u8]) -> Result<(), ImageError> {
        while body.len() >= 17 {
            let class = body[0] >> 4;
            let id = (body[0] & 15) as usize;
            if id > 3 || class > 1 {
                return Err(ImageError::Corrupt);
            }
            let mut counts = [0u8; 16];
            counts.copy_from_slice(&body[1..17]);
            let total: usize = counts.iter().map(|c| *c as usize).sum();
            if body.len() < 17 + total {
                return Err(ImageError::Corrupt);
            }
            let table =
                Huffman::build(&counts, &body[17..17 + total]).ok_or(ImageError::Corrupt)?;
            if class == 0 {
                self.dc_tables[id] = table;
            } else {
                self.ac_tables[id] = table;
            }
            body = &body[17 + total..];
        }
        Ok(())
    }

    fn read_frame(&mut self, body: &[u8]) -> Result<(), ImageError> {
        if body.len() < 6 || body[0] != 8 {
            return Err(ImageError::Unsupported); // only 8-bit samples
        }
        self.height = be16(body, 1).ok_or(ImageError::Corrupt)?;
        self.width = be16(body, 3).ok_or(ImageError::Corrupt)?;
        let count = body[5] as usize;
        if self.width == 0 || self.height == 0 || self.width > MAX_EDGE || self.height > MAX_EDGE {
            return Err(ImageError::TooLarge);
        }
        if self.width * self.height > MAX_PIXELS {
            return Err(ImageError::TooLarge);
        }
        if !(count == 1 || count == 3) || body.len() < 6 + count * 3 {
            return Err(ImageError::Unsupported);
        }
        self.component_count = count;
        for index in 0..count {
            let entry = &body[6 + index * 3..9 + index * 3];
            let (h, v) = ((entry[1] >> 4) as usize, (entry[1] & 15) as usize);
            if !(1..=4).contains(&h) || !(1..=4).contains(&v) || entry[2] > 3 {
                return Err(ImageError::Corrupt);
            }
            self.components[index] = Component {
                id: entry[0],
                h,
                v,
                quant: entry[2] as usize,
                ..EMPTY_COMPONENT
            };
        }
        self.max_h = self.components[..count]
            .iter()
            .map(|c| c.h)
            .max()
            .unwrap_or(1);
        self.max_v = self.components[..count]
            .iter()
            .map(|c| c.v)
            .max()
            .unwrap_or(1);
        if count == 1 {
            // A lone component is never subsampled in its own scan.
            self.components[0].h = 1;
            self.components[0].v = 1;
            self.max_h = 1;
            self.max_v = 1;
        }
        self.mcus_x = self.width.div_ceil(8 * self.max_h);
        self.mcus_y = self.height.div_ceil(8 * self.max_v);
        let mut total = 0usize;
        for index in 0..count {
            let (h, v) = (self.components[index].h, self.components[index].v);
            let component = &mut self.components[index];
            component.blocks_w = self.mcus_x * h;
            component.blocks_h = self.mcus_y * v;
            let comp_width = (self.width * h).div_ceil(self.max_h);
            let comp_height = (self.height * v).div_ceil(self.max_v);
            component.real_w = comp_width.div_ceil(8);
            component.real_h = comp_height.div_ceil(8);
            component.coefficient_offset = total;
            total += component.blocks_w * component.blocks_h * 64;
        }
        let buffer = PageBuffer::new(total * 2).ok_or(ImageError::OutOfMemory)?;
        self.coefficients = Some(buffer);
        Ok(())
    }

    /// Decodes one scan; returns the offset just past its entropy-coded data.
    fn decode_scan(&mut self, header: &[u8], data_start: usize) -> Result<usize, ImageError> {
        let count = *header.first().ok_or(ImageError::Corrupt)? as usize;
        if count == 0 || count > self.component_count || header.len() < 1 + count * 2 + 3 {
            return Err(ImageError::Corrupt);
        }
        let mut scan_components = [0usize; 3];
        for index in 0..count {
            let id = header[1 + index * 2];
            let tables = header[2 + index * 2];
            let slot = self.components[..self.component_count]
                .iter()
                .position(|c| c.id == id)
                .ok_or(ImageError::Corrupt)?;
            self.components[slot].dc_dest = (tables >> 4) as usize & 3;
            self.components[slot].ac_dest = (tables & 15) as usize & 3;
            self.components[slot].predictor = 0;
            scan_components[index] = slot;
        }
        let spectral_start = header[1 + count * 2] as usize;
        let spectral_end = header[2 + count * 2] as usize;
        let approx_high = (header[3 + count * 2] >> 4) as u32;
        let approx_low = (header[3 + count * 2] & 15) as u32;
        if spectral_start > 63 || spectral_end > 63 || spectral_start > spectral_end {
            return Err(ImageError::Corrupt);
        }
        let mut coefficients = self.coefficients.take().ok_or(ImageError::Corrupt)?;
        let result = self.decode_scan_blocks(
            &mut coefficients,
            &scan_components[..count],
            spectral_start,
            spectral_end,
            approx_high,
            approx_low,
            data_start,
        );
        self.coefficients = Some(coefficients);
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn decode_scan_blocks(
        &mut self,
        coefficients: &mut PageBuffer,
        scan: &[usize],
        ss: usize,
        se: usize,
        ah: u32,
        al: u32,
        data_start: usize,
    ) -> Result<usize, ImageError> {
        let data = self.data;
        let blocks: &mut [i16] = unsafe {
            core::slice::from_raw_parts_mut(
                coefficients.as_mut_slice().as_mut_ptr() as *mut i16,
                coefficients.len() / 2,
            )
        };
        let mut reader = Reader {
            data,
            position: data_start,
            accumulator: 0,
            count: 0,
            hit_marker: false,
            eob_run: 0,
        };
        let progressive = self.progressive;
        let interleaved = scan.len() > 1;
        let (units_x, units_y) = if interleaved {
            (self.mcus_x, self.mcus_y)
        } else {
            let c = &self.components[scan[0]];
            (c.real_w, c.real_h)
        };
        let mut until_restart = self.restart_interval;
        for unit_y in 0..units_y {
            for unit_x in 0..units_x {
                if self.restart_interval != 0 {
                    if until_restart == 0 {
                        reader.restart();
                        for &slot in scan {
                            self.components[slot].predictor = 0;
                        }
                        until_restart = self.restart_interval;
                    }
                    until_restart -= 1;
                }
                for &slot in scan {
                    let (h, v) = if interleaved {
                        (self.components[slot].h, self.components[slot].v)
                    } else {
                        (1, 1)
                    };
                    for block_y in 0..v {
                        for block_x in 0..h {
                            let (bx, by) = if interleaved {
                                (unit_x * h + block_x, unit_y * v + block_y)
                            } else {
                                (unit_x, unit_y)
                            };
                            let component = &self.components[slot];
                            let offset =
                                component.coefficient_offset + (by * component.blocks_w + bx) * 64;
                            let block = &mut blocks[offset..offset + 64];
                            let dc = self.dc_tables[component.dc_dest];
                            let ac = self.ac_tables[component.ac_dest];
                            let mut predictor = component.predictor;
                            if !progressive {
                                if !dc.valid || !ac.valid {
                                    return Err(ImageError::Corrupt);
                                }
                                decode_sequential(&mut reader, block, &dc, &ac, &mut predictor)?;
                            } else if ss == 0 {
                                if ah == 0 {
                                    if !dc.valid {
                                        return Err(ImageError::Corrupt);
                                    }
                                    let s = reader.decode(&dc)? as u32;
                                    let diff = reader.receive_extend(s);
                                    predictor += diff;
                                    block[0] = (predictor << al) as i16;
                                } else if reader.bit() != 0 {
                                    block[0] |= 1 << al;
                                }
                            } else if !ac.valid {
                                return Err(ImageError::Corrupt);
                            } else if ah == 0 {
                                decode_ac_first(&mut reader, block, &ac, ss, se, al)?;
                            } else {
                                decode_ac_refine(&mut reader, block, &ac, ss, se, al)?;
                            }
                            self.components[slot].predictor = predictor;
                        }
                    }
                }
            }
        }
        // The scan ends at the next marker.
        let mut at = reader.position;
        while at + 1 < data.len() {
            if data[at] == 0xff
                && data[at + 1] != 0
                && !(0xd0..=0xd7).contains(&data[at + 1])
                && data[at + 1] != 0xff
            {
                return Ok(at);
            }
            at += 1;
        }
        Ok(data.len())
    }

    /// Dequantizes, transforms and colour-converts everything into the picture.
    fn render(&mut self) -> Result<Image, ImageError> {
        let coefficients = self.coefficients.take().ok_or(ImageError::Corrupt)?;
        let blocks: &[i16] = unsafe {
            core::slice::from_raw_parts(
                coefficients.as_slice().as_ptr() as *const i16,
                coefficients.len() / 2,
            )
        };
        // One sample plane per component.
        let mut planes: [Option<PageBuffer>; 3] = [None, None, None];
        for (index, slot) in planes.iter_mut().enumerate().take(self.component_count) {
            let component = &self.components[index];
            let plane_width = component.blocks_w * 8;
            let mut plane = PageBuffer::new(plane_width * component.blocks_h * 8)
                .ok_or(ImageError::OutOfMemory)?;
            let quant = &self.quant[component.quant];
            {
                let samples = plane.as_mut_slice();
                for by in 0..component.blocks_h {
                    for bx in 0..component.blocks_w {
                        let offset =
                            component.coefficient_offset + (by * component.blocks_w + bx) * 64;
                        let mut output = [0u8; 64];
                        idct_block(&blocks[offset..offset + 64], quant, &mut output);
                        for row in 0..8 {
                            let at = (by * 8 + row) * plane_width + bx * 8;
                            samples[at..at + 8].copy_from_slice(&output[row * 8..row * 8 + 8]);
                        }
                    }
                }
            }
            *slot = Some(plane);
        }
        drop(coefficients);

        // Output dimensions after the EXIF rotation.
        let (out_w, out_h) = if self.orientation >= 5 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        };
        let mut image = Image::new(out_w, out_h)?;
        let rgb_direct = self.adobe_transform == Some(0)
            || (self.component_count == 3
                && self.components[0].id == b'R'
                && self.components[1].id == b'G'
                && self.components[2].id == b'B');
        let output = image.rgba_mut();
        let widths: [usize; 3] = [
            self.components[0].blocks_w * 8,
            self.components[1].blocks_w * 8,
            self.components[2].blocks_w * 8,
        ];
        for y in 0..self.height {
            for x in 0..self.width {
                let luma = planes[0].as_ref().map(|p| p.as_slice()).unwrap_or(&[]);
                let (r, g, b);
                if self.component_count == 1 {
                    let value = luma[y * widths[0] + x];
                    r = value;
                    g = value;
                    b = value;
                } else {
                    let first = sample_component(self, &planes, 0, widths[0], x, y);
                    let second = sample_component(self, &planes, 1, widths[1], x, y);
                    let third = sample_component(self, &planes, 2, widths[2], x, y);
                    if rgb_direct {
                        (r, g, b) = (first, second, third);
                    } else {
                        let cb = second as i32 - 128;
                        let cr = third as i32 - 128;
                        let luma = first as i32;
                        r = clamp8(luma + ((91_881 * cr + 32_768) >> 16));
                        g = clamp8(luma - ((22_554 * cb + 46_802 * cr + 32_768) >> 16));
                        b = clamp8(luma + ((116_130 * cb + 32_768) >> 16));
                    }
                }
                let (ox, oy) = orient(self.orientation, x, y, self.width, self.height);
                let at = (oy * out_w + ox) * 4;
                output[at] = r;
                output[at + 1] = g;
                output[at + 2] = b;
                output[at + 3] = 255;
            }
        }
        Ok(image)
    }
}

fn clamp8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

/// Where pixel (x, y) of the stored picture lands after applying an EXIF
/// orientation (1 = as stored).
fn orient(orientation: u8, x: usize, y: usize, w: usize, h: usize) -> (usize, usize) {
    match orientation {
        2 => (w - 1 - x, y),
        3 => (w - 1 - x, h - 1 - y),
        4 => (x, h - 1 - y),
        5 => (y, x),
        6 => (h - 1 - y, x),
        7 => (h - 1 - y, w - 1 - x),
        8 => (y, w - 1 - x),
        _ => (x, y),
    }
}

/// One component's sample at full-resolution position (x, y): direct when the
/// component is not subsampled, else smoothly interpolated ("fancy"
/// upsampling for the usual 2x factors, nearest otherwise).
fn sample_component(
    decoder: &Decoder,
    planes: &[Option<PageBuffer>; 3],
    index: usize,
    stride: usize,
    x: usize,
    y: usize,
) -> u8 {
    let component = &decoder.components[index];
    let plane = planes[index].as_ref().map(|p| p.as_slice()).unwrap_or(&[]);
    let (hs, vs) = (decoder.max_h / component.h, decoder.max_v / component.v);
    if hs == 1 && vs == 1 {
        return plane[y * stride + x];
    }
    let comp_w = (decoder.width * component.h).div_ceil(decoder.max_h);
    let comp_h = (decoder.height * component.v).div_ceil(decoder.max_v);
    let at = |cx: usize, cy: usize| plane[cy.min(comp_h - 1) * stride + cx.min(comp_w - 1)] as i32;
    if hs == 2 && (vs == 1 || vs == 2) {
        // Triangle filter: 3/4 of the nearer chroma sample, 1/4 of the further.
        let cx = x / 2;
        let far_x = if x.is_multiple_of(2) {
            cx.saturating_sub(1)
        } else {
            cx + 1
        };
        if vs == 1 {
            let cy = y;
            let value = (3 * at(cx, cy) + at(far_x, cy) + 2) >> 2;
            return value as u8;
        }
        let cy = y / 2;
        let far_y = if y.is_multiple_of(2) {
            cy.saturating_sub(1)
        } else {
            cy + 1
        };
        let near = 3 * at(cx, cy) + at(far_x, cy);
        let far = 3 * at(cx, far_y) + at(far_x, far_y);
        return ((3 * near + far + 8) >> 4) as u8;
    }
    plane[(y / vs).min(comp_h - 1) * stride + (x / hs).min(comp_w - 1)]
}

fn decode_sequential(
    reader: &mut Reader,
    block: &mut [i16],
    dc: &Huffman,
    ac: &Huffman,
    predictor: &mut i32,
) -> Result<(), ImageError> {
    let s = reader.decode(dc)? as u32;
    if s > 16 {
        return Err(ImageError::Corrupt);
    }
    *predictor += reader.receive_extend(s);
    block[0] = *predictor as i16;
    let mut k = 1;
    while k < 64 {
        let rs = reader.decode(ac)?;
        let (r, s) = ((rs >> 4) as usize, (rs & 15) as u32);
        if s == 0 {
            if r != 15 {
                break; // end of block
            }
            k += 16;
            continue;
        }
        k += r;
        if k > 63 {
            return Err(ImageError::Corrupt);
        }
        block[ZIGZAG[k]] = reader.receive_extend(s) as i16;
        k += 1;
    }
    Ok(())
}

fn decode_ac_first(
    reader: &mut Reader,
    block: &mut [i16],
    ac: &Huffman,
    ss: usize,
    se: usize,
    al: u32,
) -> Result<(), ImageError> {
    if reader.eob_run > 0 {
        reader.eob_run -= 1;
        return Ok(());
    }
    let mut k = ss;
    while k <= se {
        let rs = reader.decode(ac)?;
        let (r, s) = ((rs >> 4) as usize, (rs & 15) as u32);
        if s != 0 {
            k += r;
            if k > 63 {
                return Err(ImageError::Corrupt);
            }
            let value = reader.receive_extend(s);
            block[ZIGZAG[k]] = (value << al) as i16;
        } else if r == 15 {
            k += 15;
        } else {
            reader.eob_run = 1 << r;
            if r != 0 {
                reader.eob_run += reader.bits(r as u32);
            }
            reader.eob_run -= 1;
            break;
        }
        k += 1;
    }
    Ok(())
}

fn decode_ac_refine(
    reader: &mut Reader,
    block: &mut [i16],
    ac: &Huffman,
    ss: usize,
    se: usize,
    al: u32,
) -> Result<(), ImageError> {
    let p1 = 1i16 << al;
    let m1 = -1i16 << al;
    let mut k = ss;
    if reader.eob_run == 0 {
        while k <= se {
            let rs = reader.decode(ac)?;
            let mut r = (rs >> 4) as i32;
            let s = (rs & 15) as u32;
            let mut new_value = 0i16;
            if s != 0 {
                new_value = if reader.bit() != 0 { p1 } else { m1 };
            } else if r != 15 {
                reader.eob_run = 1 << r;
                if r != 0 {
                    reader.eob_run += reader.bits(r as u32);
                }
                break;
            }
            // Skip over already-nonzero coefficients (refining them) and `r` zero ones.
            while k <= se {
                let position = ZIGZAG[k];
                if block[position] != 0 {
                    if reader.bit() != 0 && (block[position] & p1) == 0 {
                        if block[position] >= 0 {
                            block[position] += p1;
                        } else {
                            block[position] += m1;
                        }
                    }
                } else {
                    r -= 1;
                    if r < 0 {
                        break;
                    }
                }
                k += 1;
            }
            if new_value != 0 && k <= se {
                block[ZIGZAG[k]] = new_value;
            }
            k += 1;
        }
    }
    if reader.eob_run > 0 {
        // Rest of the band: only refinement bits.
        while k <= se {
            let position = ZIGZAG[k];
            if block[position] != 0 && reader.bit() != 0 && (block[position] & p1) == 0 {
                if block[position] >= 0 {
                    block[position] += p1;
                } else {
                    block[position] += m1;
                }
            }
            k += 1;
        }
        reader.eob_run -= 1;
    }
    Ok(())
}

const CONST_BITS: i32 = 13;
const PASS1_BITS: i32 = 2;
const FIX_0_298631336: i32 = 2446;
const FIX_0_390180644: i32 = 3196;
const FIX_0_541196100: i32 = 4433;
const FIX_0_765366865: i32 = 6270;
const FIX_0_899976223: i32 = 7373;
const FIX_1_175875602: i32 = 9633;
const FIX_1_501321110: i32 = 12299;
const FIX_1_847759065: i32 = 15137;
const FIX_1_961570560: i32 = 16069;
const FIX_2_053119869: i32 = 16819;
const FIX_2_562915447: i32 = 20995;
const FIX_3_072711026: i32 = 25172;

fn descale(value: i32, bits: i32) -> i32 {
    (value + (1 << (bits - 1))) >> bits
}

/// One 1-D pass of the accurate integer IDCT over 8 inputs; returns the 8
/// outputs before the final descaling.
fn idct_1d(i: [i32; 8]) -> [i32; 8] {
    let z2 = i[2];
    let z3 = i[6];
    let z1 = (z2 + z3) * FIX_0_541196100;
    let tmp2 = z1 + z3 * -FIX_1_847759065;
    let tmp3 = z1 + z2 * FIX_0_765366865;
    let tmp0 = (i[0] + i[4]) << CONST_BITS;
    let tmp1 = (i[0] - i[4]) << CONST_BITS;
    let tmp10 = tmp0 + tmp3;
    let tmp13 = tmp0 - tmp3;
    let tmp11 = tmp1 + tmp2;
    let tmp12 = tmp1 - tmp2;
    let (mut t0, mut t1, mut t2, mut t3) = (i[7], i[5], i[3], i[1]);
    let mut z1 = t0 + t3;
    let mut z2 = t1 + t2;
    let mut z3 = t0 + t2;
    let mut z4 = t1 + t3;
    let z5 = (z3 + z4) * FIX_1_175875602;
    t0 *= FIX_0_298631336;
    t1 *= FIX_2_053119869;
    t2 *= FIX_3_072711026;
    t3 *= FIX_1_501321110;
    z1 *= -FIX_0_899976223;
    z2 *= -FIX_2_562915447;
    z3 *= -FIX_1_961570560;
    z4 *= -FIX_0_390180644;
    z3 += z5;
    z4 += z5;
    t0 += z1 + z3;
    t1 += z2 + z4;
    t2 += z2 + z3;
    t3 += z1 + z4;
    [
        tmp10 + t3,
        tmp11 + t2,
        tmp12 + t1,
        tmp13 + t0,
        tmp13 - t0,
        tmp12 - t1,
        tmp11 - t2,
        tmp10 - t3,
    ]
}

/// Dequantizes a coefficient block (natural order) and produces 8x8 samples.
fn idct_block(coefficients: &[i16], quant: &[u16; 64], output: &mut [u8; 64]) {
    let mut workspace = [0i32; 64];
    for column in 0..8 {
        let get =
            |row: usize| coefficients[row * 8 + column] as i32 * quant[row * 8 + column] as i32;
        if (1..8).all(|row| coefficients[row * 8 + column] == 0) {
            let dc = get(0) << PASS1_BITS;
            for row in 0..8 {
                workspace[row * 8 + column] = dc;
            }
            continue;
        }
        let input = [
            get(0),
            get(1),
            get(2),
            get(3),
            get(4),
            get(5),
            get(6),
            get(7),
        ];
        let result = idct_1d(input);
        for (row, value) in result.iter().enumerate() {
            workspace[row * 8 + column] = descale(*value, CONST_BITS - PASS1_BITS);
        }
    }
    for row in 0..8 {
        let line = &workspace[row * 8..row * 8 + 8];
        let result = idct_1d([
            line[0], line[1], line[2], line[3], line[4], line[5], line[6], line[7],
        ]);
        for (column, value) in result.iter().enumerate() {
            output[row * 8 + column] = clamp8(descale(*value, CONST_BITS + PASS1_BITS + 3) + 128);
        }
    }
}
