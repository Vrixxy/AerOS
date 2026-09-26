//! DEFLATE (RFC 1951) decompression and the zlib wrapper (RFC 1950), for PNG
//! and friends. The whole output is written into one caller-provided buffer,
//! so back-references read straight from what was already produced.

// Picture library for the Photos/Files/Screenshot apps (their UI is pending).
#![allow(dead_code)]

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InflateError {
    /// The compressed data ended early or is malformed.
    Corrupt,
    /// The output buffer is too small.
    Overflow,
}

const MAX_BITS: usize = 15;
const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DISTANCE_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DISTANCE_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const CODE_LENGTH_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// A canonical Huffman code as (count per length, symbols in code order),
/// plus a lookup table for the common short codes.
struct Huffman {
    count: [u16; MAX_BITS + 1],
    symbol: [u16; 288],
    /// `fast[bits]` = (symbol << 4 | length) for codes up to FAST_BITS long, 0 = slow path.
    fast: [u16; 1 << FAST_BITS],
}

const FAST_BITS: usize = 9;

impl Huffman {
    fn new(lengths: &[u8]) -> Result<Self, InflateError> {
        let mut table = Huffman {
            count: [0; MAX_BITS + 1],
            symbol: [0; 288],
            fast: [0; 1 << FAST_BITS],
        };
        for length in lengths {
            table.count[*length as usize] += 1;
        }
        if table.count[0] as usize == lengths.len() {
            return Ok(table); // no codes: valid only if never used
        }
        // Reject over-subscribed sets.
        let mut left = 1i32;
        for length in 1..=MAX_BITS {
            left <<= 1;
            left -= table.count[length] as i32;
            if left < 0 {
                return Err(InflateError::Corrupt);
            }
        }
        let mut offsets = [0u16; MAX_BITS + 2];
        for length in 1..=MAX_BITS {
            offsets[length + 1] = offsets[length] + table.count[length];
        }
        for (symbol, length) in lengths.iter().enumerate() {
            if *length != 0 {
                table.symbol[offsets[*length as usize] as usize] = symbol as u16;
                offsets[*length as usize] += 1;
            }
        }
        // Fast table: walk the canonical codes and fill every bit pattern
        // (LSB-first, as the stream delivers them) that starts with a short code.
        let mut code = 0u32;
        let mut index = 0usize;
        for length in 1..=FAST_BITS {
            for _ in 0..table.count[length] {
                let symbol = table.symbol[index];
                index += 1;
                // Codes are packed MSB-first; the stream is LSB-first, so reverse.
                let reversed = code.reverse_bits() >> (32 - length);
                let mut fill = reversed as usize;
                while fill < (1 << FAST_BITS) {
                    table.fast[fill] = symbol << 4 | length as u16;
                    fill += 1 << length;
                }
                code += 1;
            }
            code <<= 1;
        }
        Ok(table)
    }
}

struct Bits<'a> {
    data: &'a [u8],
    position: usize,
    buffer: u64,
    count: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            position: 0,
            buffer: 0,
            count: 0,
        }
    }

    fn refill(&mut self) {
        while self.count <= 56 {
            let byte = if self.position < self.data.len() {
                self.data[self.position]
            } else if self.position < self.data.len() + 8 {
                0 // reading a little past the end is fine; consuming it is caught by `consumed_past_end`
            } else {
                break;
            };
            self.position += 1;
            self.buffer |= (byte as u64) << self.count;
            self.count += 8;
        }
    }

    fn take(&mut self, bits: u32) -> u32 {
        if bits == 0 {
            return 0;
        }
        if self.count < bits {
            self.refill();
        }
        let value = (self.buffer & ((1u64 << bits) - 1)) as u32;
        self.buffer >>= bits;
        self.count = self.count.saturating_sub(bits);
        value
    }

    /// True once bits beyond the real input have been consumed.
    fn overrun(&self) -> bool {
        // Bytes pulled in minus whole bytes still buffered, versus the input length.
        self.position.saturating_sub((self.count / 8) as usize) > self.data.len()
    }

    fn align_to_byte(&mut self) {
        let drop = self.count % 8;
        self.buffer >>= drop;
        self.count -= drop;
    }

    fn decode(&mut self, table: &Huffman) -> Result<u16, InflateError> {
        if self.count < MAX_BITS as u32 {
            self.refill();
        }
        let entry = table.fast[(self.buffer & ((1 << FAST_BITS) - 1)) as usize];
        if entry != 0 {
            let length = (entry & 15) as u32;
            self.buffer >>= length;
            self.count -= length;
            return Ok(entry >> 4);
        }
        // Slow path: canonical decode bit by bit.
        let mut code = 0i32;
        let mut first = 0i32;
        let mut index = 0i32;
        for length in 1..=MAX_BITS {
            code |= (self.buffer & 1) as i32;
            self.buffer >>= 1;
            self.count = self.count.saturating_sub(1);
            let count = table.count[length] as i32;
            if code - count < first {
                return Ok(table.symbol[(index + (code - first)) as usize]);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err(InflateError::Corrupt)
    }
}

/// Decompresses a raw DEFLATE stream into `output`; returns the bytes written.
pub fn inflate(input: &[u8], output: &mut [u8]) -> Result<usize, InflateError> {
    let mut bits = Bits::new(input);
    let mut written = 0usize;
    loop {
        let last = bits.take(1);
        let kind = bits.take(2);
        match kind {
            0 => {
                bits.align_to_byte();
                let length = bits.take(16) as usize;
                let inverse = bits.take(16) as usize;
                if length != !inverse & 0xffff {
                    return Err(InflateError::Corrupt);
                }
                if written + length > output.len() {
                    return Err(InflateError::Overflow);
                }
                for _ in 0..length {
                    output[written] = bits.take(8) as u8;
                    written += 1;
                }
            }
            1 | 2 => {
                let (literal, distance) = if kind == 1 {
                    let mut lengths = [0u8; 288];
                    lengths[..144].fill(8);
                    lengths[144..256].fill(9);
                    lengths[256..280].fill(7);
                    lengths[280..].fill(8);
                    (Huffman::new(&lengths)?, Huffman::new(&[5u8; 30])?)
                } else {
                    read_dynamic_tables(&mut bits)?
                };
                written = inflate_block(&mut bits, &literal, &distance, output, written)?;
            }
            _ => return Err(InflateError::Corrupt),
        }
        if bits.overrun() {
            return Err(InflateError::Corrupt);
        }
        if last == 1 {
            return Ok(written);
        }
    }
}

fn read_dynamic_tables(bits: &mut Bits) -> Result<(Huffman, Huffman), InflateError> {
    let literal_count = bits.take(5) as usize + 257;
    let distance_count = bits.take(5) as usize + 1;
    let code_length_count = bits.take(4) as usize + 4;
    if literal_count > 286 || distance_count > 30 {
        return Err(InflateError::Corrupt);
    }
    let mut lengths = [0u8; 320];
    for order in CODE_LENGTH_ORDER.iter().take(code_length_count) {
        lengths[*order] = bits.take(3) as u8;
    }
    let code_lengths = Huffman::new(&lengths[..19])?;
    let mut lengths = [0u8; 320];
    let mut index = 0;
    while index < literal_count + distance_count {
        let symbol = bits.decode(&code_lengths)?;
        if symbol < 16 {
            lengths[index] = symbol as u8;
            index += 1;
        } else {
            let (value, repeat) = match symbol {
                16 => {
                    if index == 0 {
                        return Err(InflateError::Corrupt);
                    }
                    (lengths[index - 1], 3 + bits.take(2) as usize)
                }
                17 => (0, 3 + bits.take(3) as usize),
                _ => (0, 11 + bits.take(7) as usize),
            };
            if index + repeat > literal_count + distance_count {
                return Err(InflateError::Corrupt);
            }
            for _ in 0..repeat {
                lengths[index] = value;
                index += 1;
            }
        }
        if bits.overrun() {
            return Err(InflateError::Corrupt);
        }
    }
    if lengths[256] == 0 {
        return Err(InflateError::Corrupt); // no end-of-block code
    }
    Ok((
        Huffman::new(&lengths[..literal_count])?,
        Huffman::new(&lengths[literal_count..literal_count + distance_count])?,
    ))
}

fn inflate_block(
    bits: &mut Bits,
    literal: &Huffman,
    distance: &Huffman,
    output: &mut [u8],
    mut written: usize,
) -> Result<usize, InflateError> {
    loop {
        let symbol = bits.decode(literal)? as usize;
        if symbol < 256 {
            if written == output.len() {
                return Err(InflateError::Overflow);
            }
            output[written] = symbol as u8;
            written += 1;
        } else if symbol == 256 {
            return Ok(written);
        } else {
            let symbol = symbol - 257;
            if symbol >= 29 {
                return Err(InflateError::Corrupt);
            }
            let length =
                LENGTH_BASE[symbol] as usize + bits.take(LENGTH_EXTRA[symbol] as u32) as usize;
            let distance_symbol = bits.decode(distance)? as usize;
            if distance_symbol >= 30 {
                return Err(InflateError::Corrupt);
            }
            let back = DISTANCE_BASE[distance_symbol] as usize
                + bits.take(DISTANCE_EXTRA[distance_symbol] as u32) as usize;
            if back > written {
                return Err(InflateError::Corrupt);
            }
            if written + length > output.len() {
                return Err(InflateError::Overflow);
            }
            if back >= length {
                output.copy_within(written - back..written - back + length, written);
            } else {
                for offset in 0..length {
                    output[written + offset] = output[written - back + offset];
                }
            }
            written += length;
        }
        if bits.overrun() {
            return Err(InflateError::Corrupt);
        }
    }
}

/// Decompresses a zlib stream (2-byte header, DEFLATE data, Adler-32 trailer).
pub fn zlib_decompress(input: &[u8], output: &mut [u8]) -> Result<usize, InflateError> {
    if input.len() < 6 {
        return Err(InflateError::Corrupt);
    }
    let (cmf, flags) = (input[0], input[1]);
    if cmf & 0x0f != 8 || !(cmf as u16 * 256 + flags as u16).is_multiple_of(31) || flags & 0x20 != 0
    {
        return Err(InflateError::Corrupt);
    }
    inflate(&input[2..], output)
}

/// Adler-32 checksum (zlib trailer).
pub fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for byte in chunk {
            a += *byte as u32;
            b += a;
        }
        a %= 65_521;
        b %= 65_521;
    }
    b << 16 | a
}
