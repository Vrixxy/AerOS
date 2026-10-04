use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::CpuInfo;
use crate::sync::TicketLock;

struct Generator {
    key: [u32; 8],
    nonce: [u32; 3],
    counter: u32,
    block: [u8; 64],
    available: usize,
}

impl Generator {
    const EMPTY: Self = Self {
        key: [0; 8],
        nonce: [0; 3],
        counter: 0,
        block: [0; 64],
        available: 0,
    };

    fn refill(&mut self) {
        self.block = chacha20_block(&self.key, &self.nonce, self.counter);
        self.counter = self.counter.wrapping_add(1);
        if self.counter == 0 {
            self.nonce[0] = self.nonce[0].wrapping_add(1);
        }
        self.available = self.block.len();
    }

    fn fill(&mut self, destination: &mut [u8]) {
        let mut written = 0;
        while written < destination.len() {
            if self.available == 0 {
                self.refill();
            }
            let offset = self.block.len() - self.available;
            let count = self.available.min(destination.len() - written);
            destination[written..written + count]
                .copy_from_slice(&self.block[offset..offset + count]);
            self.block[offset..offset + count].fill(0);
            self.available -= count;
            written += count;
        }
    }
}

#[derive(Clone, Copy)]
pub struct RandomReport {
    pub rdrand: bool,
    pub rdseed: bool,
    pub hardware_words: u64,
    pub sample_a: u64,
    pub sample_b: u64,
    pub verified: bool,
}

static GENERATOR: TicketLock<Generator> = TicketLock::new(Generator::EMPTY);
static READY: AtomicBool = AtomicBool::new(false);
static HARDWARE_WORDS: AtomicU64 = AtomicU64::new(0);

pub fn initialize(cpu: &CpuInfo) -> RandomReport {
    let mut material = [0u64; 12];
    let material_address = &material as *const _ as usize as u64;
    for (index, value) in material.iter_mut().enumerate() {
        let hardware = if cpu.rdseed {
            hardware_word(true)
        } else {
            None
        }
        .or_else(|| {
            if cpu.rdrand {
                hardware_word(false)
            } else {
                None
            }
        });
        if let Some(word) = hardware {
            HARDWARE_WORDS.fetch_add(1, Ordering::Relaxed);
            *value = word;
        } else {
            *value = read_tsc()
                ^ crate::time::monotonic_nanoseconds().rotate_left(index as u32 * 5)
                ^ material_address.rotate_right(index as u32 * 3);
        }
    }
    let mut generator = Generator::EMPTY;
    for index in 0..8 {
        let mixed = splitmix64(
            material[index]
                ^ material[(index + 4) % material.len()]
                ^ (index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15),
        );
        generator.key[index] = (mixed ^ mixed >> 32) as u32;
    }
    for index in 0..3 {
        let mixed = splitmix64(material[index + 8] ^ read_tsc());
        generator.nonce[index] = (mixed ^ mixed >> 32) as u32;
    }
    generator.counter = splitmix64(material[0] ^ material[11]) as u32;
    *GENERATOR.lock() = generator;
    READY.store(true, Ordering::Release);
    material.fill(0);
    let sample_a = next_u64();
    let sample_b = next_u64();
    let hardware_words = HARDWARE_WORDS.load(Ordering::Acquire);
    RandomReport {
        rdrand: cpu.rdrand,
        rdseed: cpu.rdseed,
        hardware_words,
        sample_a,
        sample_b,
        verified: (cpu.rdrand || cpu.rdseed)
            && hardware_words >= 8
            && sample_a != 0
            && sample_b != 0
            && sample_a != sample_b,
    }
}

pub fn fill(destination: &mut [u8]) -> bool {
    if !READY.load(Ordering::Acquire) {
        return false;
    }
    GENERATOR.lock().fill(destination);
    true
}

pub fn next_u64() -> u64 {
    let mut bytes = [0u8; 8];
    if !fill(&mut bytes) {
        return read_tsc();
    }
    u64::from_le_bytes(bytes)
}

/// One ChaCha20 block (RFC 8439 section 2.3): the standalone core `Generator`
/// (the internal CSPRNG instance) is built on, exposed for callers that want
/// ChaCha20 as a general-purpose stream cipher with their own key - not
/// mixed with the CSPRNG's internally-seeded state at all.
fn chacha20_block(key: &[u32; 8], nonce: &[u32; 3], counter: u32) -> [u8; 64] {
    let mut state = [
        0x6170_7865,
        0x3320_646e,
        0x7962_2d32,
        0x6b20_6574,
        key[0],
        key[1],
        key[2],
        key[3],
        key[4],
        key[5],
        key[6],
        key[7],
        counter,
        nonce[0],
        nonce[1],
        nonce[2],
    ];
    let initial = state;
    for _ in 0..10 {
        quarter(&mut state, 0, 4, 8, 12);
        quarter(&mut state, 1, 5, 9, 13);
        quarter(&mut state, 2, 6, 10, 14);
        quarter(&mut state, 3, 7, 11, 15);
        quarter(&mut state, 0, 5, 10, 15);
        quarter(&mut state, 1, 6, 11, 12);
        quarter(&mut state, 2, 7, 8, 13);
        quarter(&mut state, 3, 4, 9, 14);
    }
    let mut block = [0u8; 64];
    for index in 0..16 {
        let word = state[index].wrapping_add(initial[index]);
        block[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    block
}

/// XORs `data` in place with the ChaCha20 keystream for `key`/`nonce`
/// starting at block `counter`. Encryption and decryption are the same
/// operation for a stream cipher, provided both sides use the same
/// key/nonce/counter - callers must never reuse a (key, nonce) pair across
/// two different plaintexts, or the keystream can be cancelled out between
/// them.
pub fn chacha20_xor(key: &[u8; 32], nonce: &[u8; 12], counter: u32, data: &mut [u8]) {
    let key_words: [u32; 8] = core::array::from_fn(|index| {
        u32::from_le_bytes(key[index * 4..index * 4 + 4].try_into().unwrap_or([0; 4]))
    });
    let nonce_words: [u32; 3] = core::array::from_fn(|index| {
        u32::from_le_bytes(nonce[index * 4..index * 4 + 4].try_into().unwrap_or([0; 4]))
    });
    for (block_index, chunk) in data.chunks_mut(64).enumerate() {
        let block = chacha20_block(
            &key_words,
            &nonce_words,
            counter.wrapping_add(block_index as u32),
        );
        for (byte, keystream) in chunk.iter_mut().zip(block.iter()) {
            *byte ^= keystream;
        }
    }
}

/// RFC 8439 section 2.4.2's test vector: encrypts the well-known plaintext
/// with the well-known key/nonce/counter=1 and checks the ciphertext matches
/// exactly, then decrypts (the same XOR again) and checks that round-trips
/// back to the plaintext - proves `chacha20_xor` really is standard
/// ChaCha20, not just "some XOR cipher that happens to round-trip".
pub(crate) fn chacha20_self_test() -> bool {
    let key: [u8; 32] = core::array::from_fn(|index| index as u8);
    let nonce: [u8; 12] = [0, 0, 0, 0, 0, 0, 0, 0x4a, 0, 0, 0, 0];
    let plaintext = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
    let expected_ciphertext: [u8; 114] = [
        0x6e, 0x2e, 0x35, 0x9a, 0x25, 0x68, 0xf9, 0x80, 0x41, 0xba, 0x07, 0x28, 0xdd, 0x0d, 0x69,
        0x81, 0xe9, 0x7e, 0x7a, 0xec, 0x1d, 0x43, 0x60, 0xc2, 0x0a, 0x27, 0xaf, 0xcc, 0xfd, 0x9f,
        0xae, 0x0b, 0xf9, 0x1b, 0x65, 0xc5, 0x52, 0x47, 0x33, 0xab, 0x8f, 0x59, 0x3d, 0xab, 0xcd,
        0x62, 0xb3, 0x57, 0x16, 0x39, 0xd6, 0x24, 0xe6, 0x51, 0x52, 0xab, 0x8f, 0x53, 0x0c, 0x35,
        0x9f, 0x08, 0x61, 0xd8, 0x07, 0xca, 0x0d, 0xbf, 0x50, 0x0d, 0x6a, 0x61, 0x56, 0xa3, 0x8e,
        0x08, 0x8a, 0x22, 0xb6, 0x5e, 0x52, 0xbc, 0x51, 0x4d, 0x16, 0xcc, 0xf8, 0x06, 0x81, 0x8c,
        0xe9, 0x1a, 0xb7, 0x79, 0x37, 0x36, 0x5a, 0xf9, 0x0b, 0xbf, 0x74, 0xa3, 0x5b, 0xe6, 0xb4,
        0x0b, 0x8e, 0xed, 0xf2, 0x78, 0x5e, 0x42, 0x87, 0x4d,
    ];
    let mut buffer = *plaintext;
    chacha20_xor(&key, &nonce, 1, &mut buffer);
    let encrypt_ok = buffer == expected_ciphertext;
    chacha20_xor(&key, &nonce, 1, &mut buffer);
    let round_trip_ok = buffer == *plaintext;
    encrypt_ok && round_trip_ok
}

fn quarter(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(16);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(12);
    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(8);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(7);
}

fn hardware_word(seed: bool) -> Option<u64> {
    for _ in 0..32 {
        let value: u64;
        let valid: u8;
        unsafe {
            if seed {
                asm!("rdseed {}", "setc {}", out(reg) value, out(reg_byte) valid, options(nomem, nostack));
            } else {
                asm!("rdrand {}", "setc {}", out(reg) value, out(reg_byte) valid, options(nomem, nostack));
            }
        }
        if valid != 0 {
            return Some(value);
        }
    }
    None
}

fn read_tsc() -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        asm!("rdtsc", out("eax") low, out("edx") high, options(nomem, nostack));
    }
    low as u64 | (high as u64) << 32
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ value >> 30).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ value >> 27).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ value >> 31
}
