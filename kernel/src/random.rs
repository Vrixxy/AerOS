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
        let mut state = [
            0x6170_7865,
            0x3320_646e,
            0x7962_2d32,
            0x6b20_6574,
            self.key[0],
            self.key[1],
            self.key[2],
            self.key[3],
            self.key[4],
            self.key[5],
            self.key[6],
            self.key[7],
            self.counter,
            self.nonce[0],
            self.nonce[1],
            self.nonce[2],
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
        for index in 0..16 {
            let word = state[index].wrapping_add(initial[index]);
            self.block[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
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
