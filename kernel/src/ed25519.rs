//! Ed25519 signature verification (RFC 8032) with its SHA-512, for checking
//! signed updates. Verification only: there is no signing and no secret key
//! handling here. The field arithmetic follows the compact TweetNaCl design
//! (16 limbs of 16 bits), which is slow but small and easy to audit.

type Gf = [i64; 16];

const GF0: Gf = [0; 16];
const GF1: Gf = [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
const D: Gf = [
    0x78a3, 0x1359, 0x4dca, 0x75eb, 0xd8ab, 0x4141, 0x0a4d, 0x0070, 0xe898, 0x7779, 0x4079, 0x8cc7,
    0xfe73, 0x2b6f, 0x6cee, 0x5203,
];
const D2: Gf = [
    0xf159, 0x26b2, 0x9b94, 0xebd6, 0xb156, 0x8283, 0x149a, 0x00e0, 0xd130, 0xeef3, 0x80f2, 0x198e,
    0xfce7, 0x56df, 0xd9dc, 0x2406,
];
const X: Gf = [
    0xd51a, 0x8f25, 0x2d60, 0xc956, 0xa7b2, 0x9525, 0xc760, 0x692c, 0xdc5c, 0xfdd6, 0xe231, 0xc0a4,
    0x53fe, 0xcd6e, 0x36d3, 0x2169,
];
const Y: Gf = [
    0x6658, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666,
    0x6666, 0x6666, 0x6666, 0x6666,
];
const I: Gf = [
    0xa0b0, 0x4a0e, 0x1b27, 0xc4ee, 0xe478, 0xad2f, 0x1806, 0x2f43, 0xd7a7, 0x3dfb, 0x0099, 0x2b4d,
    0xdf0b, 0x4fc1, 0x2480, 0x2b83,
];

/// The group order, little-endian.
const L: [i64; 32] = [
    0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
];

const SHA512_K: [u64; 80] = [
    0x428a2f98d728ae22,
    0x7137449123ef65cd,
    0xb5c0fbcfec4d3b2f,
    0xe9b5dba58189dbbc,
    0x3956c25bf348b538,
    0x59f111f1b605d019,
    0x923f82a4af194f9b,
    0xab1c5ed5da6d8118,
    0xd807aa98a3030242,
    0x12835b0145706fbe,
    0x243185be4ee4b28c,
    0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f,
    0x80deb1fe3b1696b1,
    0x9bdc06a725c71235,
    0xc19bf174cf692694,
    0xe49b69c19ef14ad2,
    0xefbe4786384f25e3,
    0x0fc19dc68b8cd5b5,
    0x240ca1cc77ac9c65,
    0x2de92c6f592b0275,
    0x4a7484aa6ea6e483,
    0x5cb0a9dcbd41fbd4,
    0x76f988da831153b5,
    0x983e5152ee66dfab,
    0xa831c66d2db43210,
    0xb00327c898fb213f,
    0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2,
    0xd5a79147930aa725,
    0x06ca6351e003826f,
    0x142929670a0e6e70,
    0x27b70a8546d22ffc,
    0x2e1b21385c26c926,
    0x4d2c6dfc5ac42aed,
    0x53380d139d95b3df,
    0x650a73548baf63de,
    0x766a0abb3c77b2a8,
    0x81c2c92e47edaee6,
    0x92722c851482353b,
    0xa2bfe8a14cf10364,
    0xa81a664bbc423001,
    0xc24b8b70d0f89791,
    0xc76c51a30654be30,
    0xd192e819d6ef5218,
    0xd69906245565a910,
    0xf40e35855771202a,
    0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8,
    0x1e376c085141ab53,
    0x2748774cdf8eeb99,
    0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63,
    0x4ed8aa4ae3418acb,
    0x5b9cca4f7763e373,
    0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc,
    0x78a5636f43172f60,
    0x84c87814a1f0ab72,
    0x8cc702081a6439ec,
    0x90befffa23631e28,
    0xa4506cebde82bde9,
    0xbef9a3f7b2c67915,
    0xc67178f2e372532b,
    0xca273eceea26619c,
    0xd186b8c721c0c207,
    0xeada7dd6cde0eb1e,
    0xf57d4f7fee6ed178,
    0x06f067aa72176fba,
    0x0a637dc5a2c898a6,
    0x113f9804bef90dae,
    0x1b710b35131c471b,
    0x28db77f523047d84,
    0x32caab7b40c72493,
    0x3c9ebe0a15c9bebc,
    0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6,
    0x597f299cfc657e2a,
    0x5fcb6fab3ad6faec,
    0x6c44198c4a475817,
];
const SHA512_H0: [u64; 8] = [
    0x6a09e667f3bcc908,
    0xbb67ae8584caa73b,
    0x3c6ef372fe94f82b,
    0xa54ff53a5f1d36f1,
    0x510e527fade682d1,
    0x9b05688c2b3e6c1f,
    0x1f83d9abfb41bd6b,
    0x5be0cd19137e2179,
];

pub struct Sha512 {
    state: [u64; 8],
    buffer: [u8; 128],
    used: usize,
    total: u128,
}

impl Sha512 {
    pub fn new() -> Self {
        Self {
            state: SHA512_H0,
            buffer: [0; 128],
            used: 0,
            total: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total += data.len() as u128;
        while !data.is_empty() {
            let take = (128 - self.used).min(data.len());
            self.buffer[self.used..self.used + take].copy_from_slice(&data[..take]);
            self.used += take;
            data = &data[take..];
            if self.used == 128 {
                let block = self.buffer;
                compress(&mut self.state, &block);
                self.used = 0;
            }
        }
    }

    pub fn finish(mut self) -> [u8; 64] {
        let bits = self.total * 8;
        self.buffer[self.used] = 0x80;
        self.used += 1;
        if self.used > 112 {
            self.buffer[self.used..].fill(0);
            let block = self.buffer;
            compress(&mut self.state, &block);
            self.used = 0;
        }
        self.buffer[self.used..112].fill(0);
        self.buffer[112..].copy_from_slice(&bits.to_be_bytes());
        let block = self.buffer;
        compress(&mut self.state, &block);
        let mut digest = [0u8; 64];
        for (index, word) in self.state.iter().enumerate() {
            digest[index * 8..index * 8 + 8].copy_from_slice(&word.to_be_bytes());
        }
        digest
    }
}

fn compress(state: &mut [u64; 8], block: &[u8; 128]) {
    let mut w = [0u64; 80];
    for index in 0..16 {
        let mut word = [0u8; 8];
        word.copy_from_slice(&block[index * 8..index * 8 + 8]);
        w[index] = u64::from_be_bytes(word);
    }
    for index in 16..80 {
        let s0 =
            w[index - 15].rotate_right(1) ^ w[index - 15].rotate_right(8) ^ (w[index - 15] >> 7);
        let s1 =
            w[index - 2].rotate_right(19) ^ w[index - 2].rotate_right(61) ^ (w[index - 2] >> 6);
        w[index] = w[index - 16]
            .wrapping_add(s0)
            .wrapping_add(w[index - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for index in 0..80 {
        let big_s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
        let choose = (e & f) ^ (!e & g);
        let t1 = h
            .wrapping_add(big_s1)
            .wrapping_add(choose)
            .wrapping_add(SHA512_K[index])
            .wrapping_add(w[index]);
        let big_s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
        let majority = (a & b) ^ (a & c) ^ (b & c);
        let t2 = big_s0.wrapping_add(majority);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *slot = slot.wrapping_add(value);
    }
}

fn carry(o: &mut Gf) {
    for i in 0..16 {
        o[i] += 1 << 16;
        let c = o[i] >> 16;
        let next = (i + 1) % 16;
        o[next] += c - 1 + if i == 15 { 37 * (c - 1) } else { 0 };
        o[i] -= c << 16;
    }
}

fn select(p: &mut Gf, q: &mut Gf, bit: i64) {
    let mask = !(bit - 1);
    for i in 0..16 {
        let t = mask & (p[i] ^ q[i]);
        p[i] ^= t;
        q[i] ^= t;
    }
}

fn pack25519(n: &Gf) -> [u8; 32] {
    let mut t = *n;
    carry(&mut t);
    carry(&mut t);
    carry(&mut t);
    for _ in 0..2 {
        let mut m = [0i64; 16];
        m[0] = t[0] - 0xffed;
        for i in 1..15 {
            m[i] = t[i] - 0xffff - ((m[i - 1] >> 16) & 1);
            m[i - 1] &= 0xffff;
        }
        m[15] = t[15] - 0x7fff - ((m[14] >> 16) & 1);
        let b = (m[15] >> 16) & 1;
        m[14] &= 0xffff;
        select(&mut t, &mut m, 1 - b);
    }
    let mut out = [0u8; 32];
    for i in 0..16 {
        out[2 * i] = (t[i] & 0xff) as u8;
        out[2 * i + 1] = (t[i] >> 8) as u8;
    }
    out
}

fn unpack25519(n: &[u8; 32]) -> Gf {
    let mut o = [0i64; 16];
    for i in 0..16 {
        o[i] = n[2 * i] as i64 + ((n[2 * i + 1] as i64) << 8);
    }
    o[15] &= 0x7fff;
    o
}

fn differs(a: &Gf, b: &Gf) -> bool {
    pack25519(a) != pack25519(b)
}

fn parity(a: &Gf) -> u8 {
    pack25519(a)[0] & 1
}

fn fadd(a: &Gf, b: &Gf) -> Gf {
    let mut o = [0i64; 16];
    for i in 0..16 {
        o[i] = a[i] + b[i];
    }
    o
}

fn sub(a: &Gf, b: &Gf) -> Gf {
    let mut o = [0i64; 16];
    for i in 0..16 {
        o[i] = a[i] - b[i];
    }
    o
}

fn mul(a: &Gf, b: &Gf) -> Gf {
    let mut t = [0i64; 31];
    for i in 0..16 {
        for j in 0..16 {
            t[i + j] += a[i] * b[j];
        }
    }
    for i in 0..15 {
        t[i] += 38 * t[i + 16];
    }
    let mut o = [0i64; 16];
    o.copy_from_slice(&t[..16]);
    carry(&mut o);
    carry(&mut o);
    o
}

fn square(a: &Gf) -> Gf {
    mul(a, a)
}

fn invert(i: &Gf) -> Gf {
    let mut c = *i;
    for a in (0..=253).rev() {
        c = square(&c);
        if a != 2 && a != 4 {
            c = mul(&c, i);
        }
    }
    c
}

fn pow2523(i: &Gf) -> Gf {
    let mut c = *i;
    for a in (0..=250).rev() {
        c = square(&c);
        if a != 1 {
            c = mul(&c, i);
        }
    }
    c
}

type Point = [Gf; 4];

fn point_add(p: &mut Point, q: &Point) {
    let a = mul(&sub(&p[1], &p[0]), &sub(&q[1], &q[0]));
    let b = mul(&fadd(&p[0], &p[1]), &fadd(&q[0], &q[1]));
    let c = mul(&mul(&p[3], &q[3]), &D2);
    let d0 = mul(&p[2], &q[2]);
    let d = fadd(&d0, &d0);
    let e = sub(&b, &a);
    let f = sub(&d, &c);
    let g = fadd(&d, &c);
    let h = fadd(&b, &a);
    p[0] = mul(&e, &f);
    p[1] = mul(&h, &g);
    p[2] = mul(&g, &f);
    p[3] = mul(&e, &h);
}

fn conditional_swap(p: &mut Point, q: &mut Point, bit: i64) {
    for i in 0..4 {
        select(&mut p[i], &mut q[i], bit);
    }
}

fn pack_point(p: &Point) -> [u8; 32] {
    let zi = invert(&p[2]);
    let tx = mul(&p[0], &zi);
    let ty = mul(&p[1], &zi);
    let mut out = pack25519(&ty);
    out[31] ^= parity(&tx) << 7;
    out
}

fn scalar_mult(mut q: Point, scalar: &[u8]) -> Point {
    let mut p: Point = [GF0, GF1, GF1, GF0];
    for i in (0..=255usize).rev() {
        let bit = ((scalar[i / 8] >> (i & 7)) & 1) as i64;
        conditional_swap(&mut p, &mut q, bit);
        let copy = p;
        point_add(&mut q, &copy);
        let copy = p;
        point_add(&mut p, &copy);
        conditional_swap(&mut p, &mut q, bit);
    }
    p
}

fn scalar_base(scalar: &[u8]) -> Point {
    scalar_mult([X, Y, GF1, mul(&X, &Y)], scalar)
}

fn reduce_mod_l(hash: &[u8; 64]) -> [u8; 32] {
    let mut x = [0i64; 64];
    for (slot, byte) in x.iter_mut().zip(hash) {
        *slot = *byte as i64;
    }
    let mut out = [0u8; 32];
    let mut i = 63;
    while i >= 32 {
        let mut carry = 0i64;
        let mut j = i - 32;
        while j < i - 12 {
            x[j] += carry - 16 * x[i] * L[j - (i - 32)];
            carry = (x[j] + 128) >> 8;
            x[j] -= carry << 8;
            j += 1;
        }
        x[j] += carry;
        x[i] = 0;
        i -= 1;
    }
    let mut carry = 0i64;
    for j in 0..32 {
        x[j] += carry - (x[31] >> 4) * L[j];
        carry = x[j] >> 8;
        x[j] &= 255;
    }
    for j in 0..32 {
        x[j] -= carry * L[j];
    }
    for i in 0..32 {
        x[i + 1] += x[i] >> 8;
        out[i] = (x[i] & 255) as u8;
    }
    out
}

/// Decodes a public key and returns the point with its x coordinate negated,
/// or `None` if the bytes are not a valid curve point.
fn unpack_negated(key: &[u8; 32]) -> Option<Point> {
    let r1 = unpack25519(key);
    let num0 = square(&r1);
    let den0 = mul(&num0, &D);
    let num = sub(&num0, &GF1);
    let den = fadd(&GF1, &den0);
    let den2 = square(&den);
    let den4 = square(&den2);
    let den6 = mul(&den4, &den2);
    let mut t = mul(&den6, &num);
    t = mul(&t, &den);
    t = pow2523(&t);
    t = mul(&t, &num);
    t = mul(&t, &den);
    t = mul(&t, &den);
    let mut r0 = mul(&t, &den);
    let mut check = mul(&square(&r0), &den);
    if differs(&check, &num) {
        r0 = mul(&r0, &I);
    }
    check = mul(&square(&r0), &den);
    if differs(&check, &num) {
        return None;
    }
    if parity(&r0) == key[31] >> 7 {
        r0 = sub(&GF0, &r0);
    }
    let r3 = mul(&r0, &r1);
    Some([r0, r1, GF1, r3])
}

/// A signature must have `S < L`, which rules out the malleable second
/// encoding of the same signature.
fn scalar_is_canonical(s: &[u8]) -> bool {
    for i in (0..32).rev() {
        let limb = s[i] as i64;
        if limb != L[i] {
            return limb < L[i];
        }
    }
    false
}

/// Streaming verifier: feed the message with `update`, then `finish`.
pub struct Verifier {
    hasher: Sha512,
    negated_key: Point,
    signature: [u8; 64],
}

impl Verifier {
    pub fn new(public_key: &[u8; 32], signature: &[u8; 64]) -> Option<Self> {
        if !scalar_is_canonical(&signature[32..]) {
            return None;
        }
        let negated_key = unpack_negated(public_key)?;
        let mut hasher = Sha512::new();
        hasher.update(&signature[..32]);
        hasher.update(public_key);
        Some(Self {
            hasher,
            negated_key,
            signature: *signature,
        })
    }

    pub fn update(&mut self, data: &[u8]) {
        self.hasher.update(data);
    }

    pub fn finish(self) -> bool {
        let challenge = reduce_mod_l(&self.hasher.finish());
        let mut sum = scalar_mult(self.negated_key, &challenge);
        let base = scalar_base(&self.signature[32..]);
        point_add(&mut sum, &base);
        let packed = pack_point(&sum);
        let mut difference = 0u8;
        for (a, b) in packed.iter().zip(&self.signature[..32]) {
            difference |= a ^ b;
        }
        difference == 0
    }
}

pub fn verify(public_key: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    let Some(mut verifier) = Verifier::new(public_key, signature) else {
        return false;
    };
    verifier.update(message);
    verifier.finish()
}

pub fn decode_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let bytes = text.as_bytes();
    if bytes.len() != N * 2 {
        return None;
    }
    let digit = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    };
    let mut out = [0u8; N];
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = digit(bytes[index * 2])? << 4 | digit(bytes[index * 2 + 1])?;
    }
    Some(out)
}

const VECTORS: [(&str, &str); 3] = [
    (
        "4bc914be7cd8e0f3fb8e73e2373a4c79fe5783fe5c8e36ddaf39f563e6059fa2",
        "31a35e0c6908906faf25b223d8a5264c96d5af4b5d02b5f1b24f4231a4065477c7f522491d21a12452c3eef86c4bb71a9aebbe6ff760b957e8fd53f1fdd48c07",
    ),
    (
        "5fcb5f6bd3f7f5e073b5df5cbc90fad2214d7b73749fce0929d4343d41407077",
        "8988962a10e046d28eadafb4a902501fb1e1546419dcda935dd16b609999133a6e011e1e9186107014fb32b716addc0defb7bc55a832f817133e78406eae5009",
    ),
    (
        "5accbb620d02ff862d89e0b33260b2187c6d2a6668d14a01dbcf27f6a854d673",
        "1eda21d562491d2f52f60ccb7df09526989baf386caa2a9f2df89176b8b831e413b0a4e2a6dd013fa5ea5a6059c266d2155a74b96b50a29b74f0fff93388cc0f",
    ),
];

/// The message each entry of `VECTORS` signs.
pub fn vector_message(index: usize, buffer: &mut [u8; 300]) -> usize {
    match index {
        0 => {
            buffer[0] = b'a';
            1
        }
        1 => {
            buffer[..3].copy_from_slice(b"abc");
            3
        }
        _ => {
            for (position, byte) in buffer.iter_mut().enumerate() {
                *byte = ((position * 7 + 3) % 256) as u8;
            }
            300
        }
    }
}

pub fn vector(index: usize) -> Option<([u8; 32], [u8; 64])> {
    let (key, signature) = VECTORS.get(index)?;
    Some((decode_hex(key)?, decode_hex(signature)?))
}

pub fn self_test() -> bool {
    let mut digest = Sha512::new();
    digest.update(b"ab");
    digest.update(b"c");
    let sha_ok = digest.finish()[..8] == [0xdd, 0xaf, 0x35, 0xa1, 0x93, 0x61, 0x7a, 0xba];
    let mut long = Sha512::new();
    for _ in 0..4 {
        long.update(&[0x61; 100]);
    }
    let long_ok = long.finish()[..4] == [0x09, 0x55, 0xbf, 0x56];

    let mut message = [0u8; 300];
    let mut all_valid = true;
    let mut all_rejected = true;
    for index in 0..VECTORS.len() {
        let Some((key, signature)) = vector(index) else {
            return false;
        };
        let length = vector_message(index, &mut message);
        all_valid &= verify(&key, &message[..length], &signature);

        let mut tampered_message = message;
        tampered_message[0] ^= 1;
        all_rejected &= !verify(&key, &tampered_message[..length], &signature);
        let mut tampered_signature = signature;
        tampered_signature[5] ^= 0x40;
        all_rejected &= !verify(&key, &message[..length], &tampered_signature);
        let mut non_canonical = signature;
        non_canonical[63] = 0xff;
        all_rejected &= !verify(&key, &message[..length], &non_canonical);
        let Some((other_key, _)) = vector((index + 1) % VECTORS.len()) else {
            return false;
        };
        all_rejected &= !verify(&other_key, &message[..length], &signature);
    }
    let invalid_key = !verify(&[0xff; 32], b"abc", &[0; 64]);
    sha_ok && long_ok && all_valid && all_rejected && invalid_key
}
