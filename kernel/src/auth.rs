//! Account security: salted, iterated password hashing (PBKDF2-HMAC-SHA256),
//! constant-time comparison, a password policy and a failed-login lockout.
//!
//! The desktop never keeps a password itself; it keeps a `Credential` (random
//! salt + derived key) and wipes every buffer that held the typed text.

use core::sync::atomic::{Ordering, compiler_fence};

pub const SALT_LEN: usize = 16;
pub const KEY_LEN: usize = 32;
/// Work factor. Tuned so a check takes tens of milliseconds, which is
/// invisible to the owner and expensive for a guessing loop.
pub const ITERATIONS: u32 = 40_000;
pub const MIN_PASSWORD: usize = 8;

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

#[derive(Clone)]
pub struct Sha256 {
    state: [u32; 8],
    block: [u8; 64],
    filled: usize,
    total: u64,
}

impl Sha256 {
    pub fn new() -> Self {
        Self {
            state: H0,
            block: [0; 64],
            filled: 0,
            total: 0,
        }
    }

    fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (index, chunk) in block.chunks_exact(4).enumerate() {
            w[index] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(choose)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(majority);
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

    pub fn update(&mut self, mut data: &[u8]) {
        self.total = self.total.wrapping_add(data.len() as u64);
        while !data.is_empty() {
            let take = (64 - self.filled).min(data.len());
            self.block[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == 64 {
                let block = self.block;
                Self::compress(&mut self.state, &block);
                self.filled = 0;
            }
        }
    }

    pub fn finish(mut self) -> [u8; 32] {
        let bits = self.total.wrapping_mul(8);
        self.update(&[0x80]);
        while self.filled != 56 {
            self.update(&[0]);
        }
        self.update(&bits.to_be_bytes());
        let mut out = [0u8; 32];
        for (chunk, word) in out.chunks_exact_mut(4).zip(self.state) {
            chunk.copy_from_slice(&word.to_be_bytes());
        }
        wipe(&mut self.block);
        out
    }
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finish()
}

/// HMAC-SHA256 with the two pad states precomputed, so PBKDF2's inner loop
/// costs two compressions per iteration.
struct Hmac {
    inner: Sha256,
    outer: Sha256,
}

impl Hmac {
    fn new(key: &[u8]) -> Self {
        let mut padded = [0u8; 64];
        if key.len() > 64 {
            padded[..32].copy_from_slice(&sha256(key));
        } else {
            padded[..key.len()].copy_from_slice(key);
        }
        let mut inner = Sha256::new();
        let mut outer = Sha256::new();
        let mut ipad = [0x36u8; 64];
        let mut opad = [0x5cu8; 64];
        for index in 0..64 {
            ipad[index] ^= padded[index];
            opad[index] ^= padded[index];
        }
        inner.update(&ipad);
        outer.update(&opad);
        wipe(&mut padded);
        wipe(&mut ipad);
        wipe(&mut opad);
        Self { inner, outer }
    }

    fn mac(&self, parts: &[&[u8]]) -> [u8; 32] {
        let mut inner = self.inner.clone();
        for part in parts {
            inner.update(part);
        }
        let digest = inner.finish();
        let mut outer = self.outer.clone();
        outer.update(&digest);
        outer.finish()
    }
}

/// PBKDF2-HMAC-SHA256 for a single 32-byte output block.
pub fn pbkdf2(password: &[u8], salt: &[u8], iterations: u32) -> [u8; KEY_LEN] {
    let mac = Hmac::new(password);
    let mut u = mac.mac(&[salt, &1u32.to_be_bytes()]);
    let mut out = u;
    for _ in 1..iterations.max(1) {
        u = mac.mac(&[&u]);
        for (byte, next) in out.iter_mut().zip(u) {
            *byte ^= next;
        }
    }
    wipe(&mut u);
    out
}

/// Comparison whose time does not depend on where the inputs differ.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = (a.len() ^ b.len()) as u32;
    let len = a.len().min(b.len());
    for index in 0..len {
        diff |= (a[index] ^ b[index]) as u32;
    }
    core::hint::black_box(diff) == 0
}

/// Overwrite a buffer in a way the optimiser may not remove.
pub fn wipe(buffer: &mut [u8]) {
    for byte in buffer.iter_mut() {
        // SAFETY: `byte` is a valid, exclusive reference.
        unsafe { core::ptr::write_volatile(byte, 0) };
    }
    compiler_fence(Ordering::SeqCst);
}

#[derive(Clone, Copy)]
pub struct Credential {
    salt: [u8; SALT_LEN],
    key: [u8; KEY_LEN],
    iterations: u32,
}

impl Credential {
    /// Derive a credential with a fresh random salt (`ITERATIONS` outside tests).
    pub fn new(password: &[u8], iterations: u32) -> Self {
        let mut salt = [0u8; SALT_LEN];
        if !crate::random::fill(&mut salt) {
            // No hardware entropy: still never reuse a fixed salt.
            for chunk in salt.chunks_mut(8) {
                let word = crate::random::next_u64().to_le_bytes();
                chunk.copy_from_slice(&word[..chunk.len()]);
            }
        }
        Self::with_salt(password, salt, iterations)
    }

    fn with_salt(password: &[u8], salt: [u8; SALT_LEN], iterations: u32) -> Self {
        Self {
            salt,
            key: pbkdf2(password, &salt, iterations),
            iterations,
        }
    }

    pub const STORED_LEN: usize = SALT_LEN + KEY_LEN + 4;

    /// Salt, derived key and work factor, for saving the account to disk.
    pub fn to_bytes(self) -> [u8; Self::STORED_LEN] {
        let mut out = [0u8; Self::STORED_LEN];
        out[..SALT_LEN].copy_from_slice(&self.salt);
        out[SALT_LEN..SALT_LEN + KEY_LEN].copy_from_slice(&self.key);
        out[SALT_LEN + KEY_LEN..].copy_from_slice(&self.iterations.to_le_bytes());
        out
    }

    #[cfg_attr(feature = "boot-test", allow(dead_code))]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::STORED_LEN {
            return None;
        }
        let mut salt = [0u8; SALT_LEN];
        let mut key = [0u8; KEY_LEN];
        salt.copy_from_slice(&bytes[..SALT_LEN]);
        key.copy_from_slice(&bytes[SALT_LEN..SALT_LEN + KEY_LEN]);
        let iterations = u32::from_le_bytes(bytes[SALT_LEN + KEY_LEN..].try_into().ok()?);
        (iterations != 0).then_some(Self {
            salt,
            key,
            iterations,
        })
    }

    pub fn verify(&self, attempt: &[u8]) -> bool {
        let mut derived = pbkdf2(attempt, &self.salt, self.iterations);
        let ok = constant_time_eq(&derived, &self.key);
        wipe(&mut derived);
        ok
    }
}

/// Why a proposed password was refused (shown to the user during setup).
pub fn check_password(password: &[u8], username: &[u8]) -> Result<(), &'static str> {
    if password.len() < MIN_PASSWORD {
        return Err("Use at least 8 characters");
    }
    let first = password[0];
    if password.iter().all(|&byte| byte == first) {
        return Err("Avoid repeating one character");
    }
    if !username.is_empty() && password.eq_ignore_ascii_case(username) {
        return Err("Password must differ from the username");
    }
    const COMMON: [&[u8]; 12] = [
        b"password",
        b"12345678",
        b"123456789",
        b"qwertyui",
        b"qwerty123",
        b"iloveyou",
        b"password1",
        b"11111111",
        b"abcd1234",
        b"letmein1",
        b"admin123",
        b"aerospass",
    ];
    if COMMON
        .iter()
        .any(|common| password.eq_ignore_ascii_case(common))
    {
        return Err("That password is too common");
    }
    let mut classes = 0;
    classes += password.iter().any(u8::is_ascii_lowercase) as u32;
    classes += password.iter().any(u8::is_ascii_uppercase) as u32;
    classes += password.iter().any(u8::is_ascii_digit) as u32;
    classes += password.iter().any(|byte| !byte.is_ascii_alphanumeric()) as u32;
    if classes < 2 && password.len() < 12 {
        return Err("Mix letters with digits or symbols");
    }
    Ok(())
}

/// Failed-login throttle: three free tries, then 5 s, 10 s, 20 s ... up to
/// five minutes, growing with every further miss.
#[derive(Clone, Copy)]
pub struct Lockout {
    failures: u32,
    locked_until_ns: u64,
}

const FREE_TRIES: u32 = 3;
const BASE_DELAY_S: u64 = 5;
const MAX_DELAY_S: u64 = 300;

impl Lockout {
    pub const fn new() -> Self {
        Self {
            failures: 0,
            locked_until_ns: 0,
        }
    }

    pub fn locked_until_ns(&self) -> u64 {
        self.locked_until_ns
    }

    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// Whole seconds left before another attempt is accepted (0 = open).
    pub fn seconds_left(&self, now_ns: u64) -> u64 {
        self.locked_until_ns
            .saturating_sub(now_ns)
            .div_ceil(1_000_000_000)
    }

    pub fn is_locked(&self, now_ns: u64) -> bool {
        now_ns < self.locked_until_ns
    }

    pub fn record_failure(&mut self, now_ns: u64) {
        self.failures = self.failures.saturating_add(1);
        if self.failures > FREE_TRIES {
            let doublings = (self.failures - FREE_TRIES - 1).min(6);
            let delay = (BASE_DELAY_S << doublings).min(MAX_DELAY_S);
            self.locked_until_ns = now_ns.saturating_add(delay * 1_000_000_000);
        }
    }

    pub fn record_success(&mut self) {
        *self = Self::new();
    }
}

/// Known-answer checks for the primitives plus the policy and lockout rules.
pub fn self_test() -> bool {
    // FIPS 180-2: SHA-256("abc").
    let abc = sha256(b"abc");
    let sha_ok = abc[..4] == [0xba, 0x78, 0x16, 0xbf] && abc[28..] == [0xf2, 0x00, 0x15, 0xad];
    // RFC 7914 test vector: PBKDF2-HMAC-SHA256("passwd", "salt", c=1).
    let kdf = pbkdf2(b"passwd", b"salt", 1);
    let kdf_ok = kdf[..4] == [0x55, 0xac, 0x04, 0x6e] && kdf[28..] == [0xc2, 0x0d, 0xac, 0xbc];
    let credential = Credential::with_salt(b"correct horse 9", [7; SALT_LEN], 64);
    let verify_ok = credential.verify(b"correct horse 9") && !credential.verify(b"correct horse 8");
    let eq_ok = constant_time_eq(b"abc", b"abc")
        && !constant_time_eq(b"abc", b"abd")
        && !constant_time_eq(b"abc", b"abcd");
    let policy_ok = check_password(b"short", b"").is_err()
        && check_password(b"aaaaaaaaaa", b"").is_err()
        && check_password(b"Password", b"").is_err()
        && check_password(b"hunter2hunter2", b"hunter2hunter2").is_err()
        && check_password(b"tr1cky-Pass", b"aer").is_ok();
    let mut lockout = Lockout::new();
    let mut now = 1_000_000_000_000u64;
    for _ in 0..3 {
        lockout.record_failure(now);
    }
    let free = !lockout.is_locked(now);
    lockout.record_failure(now);
    let first = lockout.seconds_left(now) == 5;
    now += 6_000_000_000;
    lockout.record_failure(now);
    let second = lockout.seconds_left(now) == 10;
    for _ in 0..30 {
        lockout.record_failure(now);
    }
    let capped = lockout.seconds_left(now) == MAX_DELAY_S;
    lockout.record_success();
    let reset = !lockout.is_locked(now) && lockout.failures() == 0;
    sha_ok
        && kdf_ok
        && verify_ok
        && eq_ok
        && policy_ok
        && free
        && first
        && second
        && capped
        && reset
}
