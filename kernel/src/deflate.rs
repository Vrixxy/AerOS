//! DEFLATE compression: LZ77 matching (hash chains over a 32 KiB window) and
//! the fixed Huffman code, wrapped as a zlib stream. Fast and simple rather
//! than optimal; screenshots (large flat areas) compress well anyway.

// Picture library for the Photos/Files/Screenshot apps (their UI is pending).
#![allow(dead_code)]

use crate::inflate::adler32;
use crate::memory::PageBuffer;

const WINDOW: usize = 32_768;
const HASH_BITS: usize = 15;
const MAX_MATCH: usize = 258;
const MIN_MATCH: usize = 3;
const MAX_CHAIN: usize = 24;

struct BitWriter<'a> {
    out: &'a mut [u8],
    position: usize,
    buffer: u64,
    count: u32,
    failed: bool,
}

impl<'a> BitWriter<'a> {
    fn put(&mut self, value: u32, bits: u32) {
        self.buffer |= (value as u64) << self.count;
        self.count += bits;
        while self.count >= 8 {
            if self.position >= self.out.len() {
                self.failed = true;
                self.count = 0;
                self.buffer = 0;
                return;
            }
            self.out[self.position] = self.buffer as u8;
            self.position += 1;
            self.buffer >>= 8;
            self.count -= 8;
        }
    }

    /// Huffman codes go out most-significant bit first.
    fn put_code(&mut self, code: u32, bits: u32) {
        self.put(code.reverse_bits() >> (32 - bits), bits);
    }

    fn flush(&mut self) {
        if self.count > 0 {
            self.put(0, 8 - self.count);
        }
    }
}

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

/// Writes a literal/length symbol with the fixed code.
fn put_symbol(writer: &mut BitWriter, symbol: u32) {
    match symbol {
        0..=143 => writer.put_code(0x30 + symbol, 8),
        144..=255 => writer.put_code(0x190 + symbol - 144, 9),
        256..=279 => writer.put_code(symbol - 256, 7),
        _ => writer.put_code(0xc0 + symbol - 280, 8),
    }
}

fn hash3(bytes: &[u8]) -> usize {
    let value = bytes[0] as u32 | (bytes[1] as u32) << 8 | (bytes[2] as u32) << 16;
    (value.wrapping_mul(0x9e37_79b1) >> (32 - HASH_BITS)) as usize
}

/// Compresses `input` as a zlib stream into `output`; returns its length, or
/// `None` when `output` is too small.
pub fn zlib_compress(input: &[u8], output: &mut [u8]) -> Option<usize> {
    let mut head = PageBuffer::new((1 << HASH_BITS) * 4)?;
    let mut previous = PageBuffer::new(WINDOW * 4)?;
    let head_table = unsafe {
        core::slice::from_raw_parts_mut(
            head.as_mut_slice().as_mut_ptr() as *mut u32,
            1 << HASH_BITS,
        )
    };
    let previous_table = unsafe {
        core::slice::from_raw_parts_mut(previous.as_mut_slice().as_mut_ptr() as *mut u32, WINDOW)
    };
    // Positions are stored +1 so 0 means "none".
    let mut writer = BitWriter {
        out: output,
        position: 0,
        buffer: 0,
        count: 0,
        failed: false,
    };
    writer.put(0x78, 8); // CMF: deflate, 32K window
    writer.put(0x9c, 8); // FLG: default compression, check bits
    writer.put(1, 1); // final block
    writer.put(1, 2); // fixed Huffman
    let mut position = 0usize;
    while position < input.len() {
        let mut best_length = 0usize;
        let mut best_distance = 0usize;
        if position + MIN_MATCH <= input.len() {
            let hash = hash3(&input[position..]);
            let mut candidate = head_table[hash] as usize;
            let mut chain = 0;
            let limit = (input.len() - position).min(MAX_MATCH);
            while candidate != 0 && chain < MAX_CHAIN {
                let at = candidate - 1;
                if position - at > WINDOW {
                    break;
                }
                let mut length = 0;
                while length < limit && input[at + length] == input[position + length] {
                    length += 1;
                }
                if length > best_length {
                    best_length = length;
                    best_distance = position - at;
                    if length == limit {
                        break;
                    }
                }
                candidate = previous_table[at % WINDOW] as usize;
                chain += 1;
            }
        }
        let advance = if best_length >= MIN_MATCH {
            let length_index = LENGTH_BASE
                .iter()
                .rposition(|base| *base as usize <= best_length)
                .unwrap_or(0);
            put_symbol(&mut writer, 257 + length_index as u32);
            writer.put(
                (best_length - LENGTH_BASE[length_index] as usize) as u32,
                LENGTH_EXTRA[length_index] as u32,
            );
            let distance_index = DISTANCE_BASE
                .iter()
                .rposition(|base| *base as usize <= best_distance)
                .unwrap_or(0);
            writer.put_code(distance_index as u32, 5);
            writer.put(
                (best_distance - DISTANCE_BASE[distance_index] as usize) as u32,
                DISTANCE_EXTRA[distance_index] as u32,
            );
            best_length
        } else {
            put_symbol(&mut writer, input[position] as u32);
            1
        };
        // Index every position we skipped over.
        for offset in 0..advance {
            let at = position + offset;
            if at + MIN_MATCH <= input.len() {
                let hash = hash3(&input[at..]);
                previous_table[at % WINDOW] = head_table[hash];
                head_table[hash] = at as u32 + 1;
            }
        }
        position += advance;
        if writer.failed {
            return None;
        }
    }
    put_symbol(&mut writer, 256);
    writer.flush();
    let checksum = adler32(input);
    for byte in checksum.to_be_bytes() {
        writer.put(byte as u32, 8);
    }
    if writer.failed {
        return None;
    }
    Some(writer.position)
}
