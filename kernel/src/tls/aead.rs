//! The two TLS 1.3 record ciphers: ChaCha20-Poly1305 (RFC 8439) and
//! AES-128-GCM (NIST SP 800-38D). Both take a 12-byte nonce, seal in place
//! and return a 16-byte tag.
//!
//! The AES S-box is a table lookup, so AES is not constant-time against cache
//! attacks; ChaCha20-Poly1305 has no secret-dependent indexing and is the
//! suite to prefer.

use super::hash::constant_time_eq;

pub const TAG_LEN: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Suite {
    Aes128Gcm,
    ChaCha20Poly1305,
}

impl Suite {
    pub const fn key_len(self) -> usize {
        match self {
            Self::Aes128Gcm => 16,
            Self::ChaCha20Poly1305 => 32,
        }
    }
}

pub fn seal(
    suite: Suite,
    key: &[u8],
    nonce: &[u8; 12],
    aad: &[u8],
    data: &mut [u8],
) -> [u8; TAG_LEN] {
    match suite {
        Suite::Aes128Gcm => gcm_seal(key, nonce, aad, data),
        Suite::ChaCha20Poly1305 => chacha_seal(key, nonce, aad, data),
    }
}

/// Decrypts in place; `false` (with `data` scrambled) when the tag is wrong.
pub fn open(
    suite: Suite,
    key: &[u8],
    nonce: &[u8; 12],
    aad: &[u8],
    data: &mut [u8],
    tag: &[u8],
) -> bool {
    let expected = match suite {
        Suite::Aes128Gcm => gcm_tag(key, nonce, aad, data),
        Suite::ChaCha20Poly1305 => chacha_tag(key, nonce, aad, data),
    };
    if !constant_time_eq(&expected, tag) {
        return false;
    }
    match suite {
        Suite::Aes128Gcm => gcm_crypt(key, nonce, data),
        Suite::ChaCha20Poly1305 => chacha_xor(key, nonce, 1, data),
    }
    true
}

// ---------------------------------------------------------------- ChaCha20

fn quarter(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] = (state[d] ^ state[a]).rotate_left(16);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_left(12);
    state[a] = state[a].wrapping_add(state[b]);
    state[d] = (state[d] ^ state[a]).rotate_left(8);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_left(7);
}

fn chacha_block(key: &[u8], nonce: &[u8; 12], counter: u32) -> [u8; 64] {
    let mut state = [0u32; 16];
    state[..4].copy_from_slice(&[0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574]);
    for index in 0..8 {
        state[4 + index] = u32::from_le_bytes([
            key[index * 4],
            key[index * 4 + 1],
            key[index * 4 + 2],
            key[index * 4 + 3],
        ]);
    }
    state[12] = counter;
    for index in 0..3 {
        state[13 + index] = u32::from_le_bytes([
            nonce[index * 4],
            nonce[index * 4 + 1],
            nonce[index * 4 + 2],
            nonce[index * 4 + 3],
        ]);
    }
    let start = state;
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
    let mut out = [0u8; 64];
    for index in 0..16 {
        out[index * 4..index * 4 + 4]
            .copy_from_slice(&state[index].wrapping_add(start[index]).to_le_bytes());
    }
    out
}

fn chacha_xor(key: &[u8], nonce: &[u8; 12], counter: u32, data: &mut [u8]) {
    for (index, chunk) in data.chunks_mut(64).enumerate() {
        let block = chacha_block(key, nonce, counter.wrapping_add(index as u32));
        for (byte, mask) in chunk.iter_mut().zip(block.iter()) {
            *byte ^= mask;
        }
    }
}

// ---------------------------------------------------------------- Poly1305

struct Poly1305 {
    r: [u64; 5],
    h: [u64; 5],
    pad: [u32; 4],
    buffer: [u8; 16],
    used: usize,
}

impl Poly1305 {
    fn new(key: &[u8; 32]) -> Self {
        let word = |offset: usize| {
            u32::from_le_bytes([
                key[offset],
                key[offset + 1],
                key[offset + 2],
                key[offset + 3],
            ]) as u64
        };
        let (t0, t1, t2, t3) = (word(0), word(4), word(8), word(12));
        Self {
            r: [
                t0 & 0x3ff_ffff,
                ((t0 >> 26) | (t1 << 6)) & 0x3ff_ff03,
                ((t1 >> 20) | (t2 << 12)) & 0x3ff_c0ff,
                ((t2 >> 14) | (t3 << 18)) & 0x3f0_3fff,
                (t3 >> 8) & 0x00f_ffff,
            ],
            h: [0; 5],
            pad: [
                word(16) as u32,
                word(20) as u32,
                word(24) as u32,
                word(28) as u32,
            ],
            buffer: [0; 16],
            used: 0,
        }
    }

    fn block(&mut self, bytes: &[u8; 16], high_bit: u64) {
        let word = |offset: usize| {
            u32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ]) as u64
        };
        let (t0, t1, t2, t3) = (word(0), word(4), word(8), word(12));
        let [r0, r1, r2, r3, r4] = self.r;
        let (s1, s2, s3, s4) = (r1 * 5, r2 * 5, r3 * 5, r4 * 5);
        let h0 = self.h[0] + (t0 & 0x3ff_ffff);
        let h1 = self.h[1] + (((t0 >> 26) | (t1 << 6)) & 0x3ff_ffff);
        let h2 = self.h[2] + (((t1 >> 20) | (t2 << 12)) & 0x3ff_ffff);
        let h3 = self.h[3] + (((t2 >> 14) | (t3 << 18)) & 0x3ff_ffff);
        let h4 = self.h[4] + ((t3 >> 8) | high_bit);

        let d0 = h0 * r0 + h1 * s4 + h2 * s3 + h3 * s2 + h4 * s1;
        let mut d1 = h0 * r1 + h1 * r0 + h2 * s4 + h3 * s3 + h4 * s2;
        let mut d2 = h0 * r2 + h1 * r1 + h2 * r0 + h3 * s4 + h4 * s3;
        let mut d3 = h0 * r3 + h1 * r2 + h2 * r1 + h3 * r0 + h4 * s4;
        let mut d4 = h0 * r4 + h1 * r3 + h2 * r2 + h3 * r1 + h4 * r0;

        let mut carry = d0 >> 26;
        let mut n0 = d0 & 0x3ff_ffff;
        d1 += carry;
        carry = d1 >> 26;
        let n1 = d1 & 0x3ff_ffff;
        d2 += carry;
        carry = d2 >> 26;
        let n2 = d2 & 0x3ff_ffff;
        d3 += carry;
        carry = d3 >> 26;
        let n3 = d3 & 0x3ff_ffff;
        d4 += carry;
        carry = d4 >> 26;
        let n4 = d4 & 0x3ff_ffff;
        n0 += carry * 5;
        let carry = n0 >> 26;
        n0 &= 0x3ff_ffff;
        self.h = [n0, n1 + carry, n2, n3, n4];
    }

    fn update(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            let take = (16 - self.used).min(data.len());
            self.buffer[self.used..self.used + take].copy_from_slice(&data[..take]);
            self.used += take;
            data = &data[take..];
            if self.used == 16 {
                let block = self.buffer;
                self.block(&block, 1 << 24);
                self.used = 0;
            }
        }
    }

    fn finish(mut self) -> [u8; 16] {
        if self.used > 0 {
            let mut block = [0u8; 16];
            block[..self.used].copy_from_slice(&self.buffer[..self.used]);
            block[self.used] = 1;
            self.block(&block, 0);
        }
        let [mut h0, mut h1, mut h2, mut h3, mut h4] = self.h;
        let mut carry = h1 >> 26;
        h1 &= 0x3ff_ffff;
        h2 += carry;
        carry = h2 >> 26;
        h2 &= 0x3ff_ffff;
        h3 += carry;
        carry = h3 >> 26;
        h3 &= 0x3ff_ffff;
        h4 += carry;
        carry = h4 >> 26;
        h4 &= 0x3ff_ffff;
        h0 += carry * 5;
        carry = h0 >> 26;
        h0 &= 0x3ff_ffff;
        h1 += carry;

        let mut g0 = h0 + 5;
        carry = g0 >> 26;
        g0 &= 0x3ff_ffff;
        let mut g1 = h1 + carry;
        carry = g1 >> 26;
        g1 &= 0x3ff_ffff;
        let mut g2 = h2 + carry;
        carry = g2 >> 26;
        g2 &= 0x3ff_ffff;
        let mut g3 = h3 + carry;
        carry = g3 >> 26;
        g3 &= 0x3ff_ffff;
        let g4 = (h4 + carry).wrapping_sub(1 << 26);

        let keep_g = (g4 >> 63).wrapping_sub(1);
        let keep_h = !keep_g;
        let h0 = (h0 & keep_h) | (g0 & keep_g);
        let h1 = (h1 & keep_h) | (g1 & keep_g);
        let h2 = (h2 & keep_h) | (g2 & keep_g);
        let h3 = (h3 & keep_h) | (g3 & keep_g);
        let h4 = (h4 & keep_h) | (g4 & keep_g);

        let f0 = (h0 | (h1 << 26)) & 0xffff_ffff;
        let f1 = ((h1 >> 6) | (h2 << 20)) & 0xffff_ffff;
        let f2 = ((h2 >> 12) | (h3 << 14)) & 0xffff_ffff;
        let f3 = ((h3 >> 18) | (h4 << 8)) & 0xffff_ffff;

        let mut sum = f0 + u64::from(self.pad[0]);
        let mut tag = [0u8; 16];
        tag[0..4].copy_from_slice(&(sum as u32).to_le_bytes());
        sum = f1 + u64::from(self.pad[1]) + (sum >> 32);
        tag[4..8].copy_from_slice(&(sum as u32).to_le_bytes());
        sum = f2 + u64::from(self.pad[2]) + (sum >> 32);
        tag[8..12].copy_from_slice(&(sum as u32).to_le_bytes());
        sum = f3 + u64::from(self.pad[3]) + (sum >> 32);
        tag[12..16].copy_from_slice(&(sum as u32).to_le_bytes());
        tag
    }
}

fn chacha_tag(key: &[u8], nonce: &[u8; 12], aad: &[u8], ciphertext: &[u8]) -> [u8; 16] {
    let block = chacha_block(key, nonce, 0);
    let mut poly_key = [0u8; 32];
    poly_key.copy_from_slice(&block[..32]);
    let mut mac = Poly1305::new(&poly_key);
    let zeros = [0u8; 16];
    mac.update(aad);
    mac.update(&zeros[..(16 - aad.len() % 16) % 16]);
    mac.update(ciphertext);
    mac.update(&zeros[..(16 - ciphertext.len() % 16) % 16]);
    mac.update(&(aad.len() as u64).to_le_bytes());
    mac.update(&(ciphertext.len() as u64).to_le_bytes());
    mac.finish()
}

fn chacha_seal(key: &[u8], nonce: &[u8; 12], aad: &[u8], data: &mut [u8]) -> [u8; 16] {
    chacha_xor(key, nonce, 1, data);
    chacha_tag(key, nonce, aad, data)
}

// --------------------------------------------------------------------- AES

const SBOX: [u8; 256] = make_sbox();

const fn make_sbox() -> [u8; 256] {
    let mut sbox = [0u8; 256];
    let mut p: u8 = 1;
    let mut q: u8 = 1;
    loop {
        p = p ^ (p << 1) ^ if p & 0x80 != 0 { 0x1b } else { 0 };
        q ^= q << 1;
        q ^= q << 2;
        q ^= q << 4;
        if q & 0x80 != 0 {
            q ^= 0x09;
        }
        let x = q ^ q.rotate_left(1) ^ q.rotate_left(2) ^ q.rotate_left(3) ^ q.rotate_left(4);
        sbox[p as usize] = x ^ 0x63;
        if p == 1 {
            break;
        }
    }
    sbox[0] = 0x63;
    sbox
}

struct Aes128 {
    round_keys: [[u8; 16]; 11],
}

impl Aes128 {
    fn new(key: &[u8]) -> Self {
        let mut words = [[0u8; 4]; 44];
        for index in 0..4 {
            words[index].copy_from_slice(&key[index * 4..index * 4 + 4]);
        }
        let mut rcon = 1u8;
        for index in 4..44 {
            let mut temp = words[index - 1];
            if index % 4 == 0 {
                temp = [
                    SBOX[temp[1] as usize] ^ rcon,
                    SBOX[temp[2] as usize],
                    SBOX[temp[3] as usize],
                    SBOX[temp[0] as usize],
                ];
                rcon = xtime(rcon);
            }
            for byte in 0..4 {
                words[index][byte] = words[index - 4][byte] ^ temp[byte];
            }
        }
        let mut round_keys = [[0u8; 16]; 11];
        for (round, key) in round_keys.iter_mut().enumerate() {
            for column in 0..4 {
                key[column * 4..column * 4 + 4].copy_from_slice(&words[round * 4 + column]);
            }
        }
        Self { round_keys }
    }

    fn encrypt(&self, block: &mut [u8; 16]) {
        add_round_key(block, &self.round_keys[0]);
        for round in 1..10 {
            sub_shift(block);
            mix_columns(block);
            add_round_key(block, &self.round_keys[round]);
        }
        sub_shift(block);
        add_round_key(block, &self.round_keys[10]);
    }
}

const fn xtime(value: u8) -> u8 {
    (value << 1) ^ if value & 0x80 != 0 { 0x1b } else { 0 }
}

fn add_round_key(block: &mut [u8; 16], key: &[u8; 16]) {
    for (byte, round) in block.iter_mut().zip(key.iter()) {
        *byte ^= round;
    }
}

fn sub_shift(block: &mut [u8; 16]) {
    let mut out = [0u8; 16];
    for column in 0..4 {
        for row in 0..4 {
            out[column * 4 + row] = SBOX[block[((column + row) % 4) * 4 + row] as usize];
        }
    }
    *block = out;
}

fn mix_columns(block: &mut [u8; 16]) {
    for column in 0..4 {
        let a: [u8; 4] = [
            block[column * 4],
            block[column * 4 + 1],
            block[column * 4 + 2],
            block[column * 4 + 3],
        ];
        let all = a[0] ^ a[1] ^ a[2] ^ a[3];
        for row in 0..4 {
            block[column * 4 + row] = a[row] ^ all ^ xtime(a[row] ^ a[(row + 1) % 4]);
        }
    }
}

// --------------------------------------------------------------------- GCM

fn ghash_multiply(x: u128, y: u128) -> u128 {
    let mut z = 0u128;
    let mut v = y;
    for bit in 0..128 {
        let take = 0u128.wrapping_sub((x >> (127 - bit)) & 1);
        z ^= v & take;
        let low = 0u128.wrapping_sub(v & 1);
        v >>= 1;
        v ^= (0xe1u128 << 120) & low;
    }
    z
}

fn ghash_blocks(hash_key: u128, state: &mut u128, data: &[u8]) {
    for chunk in data.chunks(16) {
        let mut block = [0u8; 16];
        block[..chunk.len()].copy_from_slice(chunk);
        *state = ghash_multiply(*state ^ u128::from_be_bytes(block), hash_key);
    }
}

fn gcm_counter_block(nonce: &[u8; 12], counter: u32) -> [u8; 16] {
    let mut block = [0u8; 16];
    block[..12].copy_from_slice(nonce);
    block[12..].copy_from_slice(&counter.to_be_bytes());
    block
}

fn gcm_crypt(key: &[u8], nonce: &[u8; 12], data: &mut [u8]) {
    let aes = Aes128::new(key);
    for (index, chunk) in data.chunks_mut(16).enumerate() {
        let mut block = gcm_counter_block(nonce, 2 + index as u32);
        aes.encrypt(&mut block);
        for (byte, mask) in chunk.iter_mut().zip(block.iter()) {
            *byte ^= mask;
        }
    }
}

fn gcm_tag(key: &[u8], nonce: &[u8; 12], aad: &[u8], ciphertext: &[u8]) -> [u8; 16] {
    let aes = Aes128::new(key);
    let mut zero = [0u8; 16];
    aes.encrypt(&mut zero);
    let hash_key = u128::from_be_bytes(zero);
    let mut state = 0u128;
    ghash_blocks(hash_key, &mut state, aad);
    ghash_blocks(hash_key, &mut state, ciphertext);
    let lengths = ((aad.len() as u128 * 8) << 64) | (ciphertext.len() as u128 * 8);
    state = ghash_multiply(state ^ lengths, hash_key);
    let mut mask = gcm_counter_block(nonce, 1);
    aes.encrypt(&mut mask);
    (state ^ u128::from_be_bytes(mask)).to_be_bytes()
}

fn gcm_seal(key: &[u8], nonce: &[u8; 12], aad: &[u8], data: &mut [u8]) -> [u8; 16] {
    gcm_crypt(key, nonce, data);
    gcm_tag(key, nonce, aad, data)
}
