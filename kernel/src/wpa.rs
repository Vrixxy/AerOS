//! WPA2-Personal (CCMP) key handling: the pairwise master key from a
//! passphrase, the pairwise transient key, and the supplicant side of the
//! EAPOL-Key four-way handshake. Core-only, so the host harness runs it
//! against published vectors. There is no Wi-Fi driver behind it yet.

#![allow(dead_code, clippy::needless_range_loop)]

const SHA1_BLOCK: usize = 64;

pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut state: [u32; 5] = [
        0x6745_2301,
        0xefcd_ab89,
        0x98ba_dcfe,
        0x1032_5476,
        0xc3d2_e1f0,
    ];
    let bit_length = (data.len() as u64) * 8;
    let mut block = [0u8; SHA1_BLOCK];
    let mut offset = 0;
    while offset + SHA1_BLOCK <= data.len() {
        sha1_block(&mut state, &data[offset..offset + SHA1_BLOCK]);
        offset += SHA1_BLOCK;
    }
    let rest = data.len() - offset;
    block[..rest].copy_from_slice(&data[offset..]);
    block[rest] = 0x80;
    if rest >= SHA1_BLOCK - 8 {
        sha1_block(&mut state, &block);
        block = [0; SHA1_BLOCK];
    }
    block[SHA1_BLOCK - 8..].copy_from_slice(&bit_length.to_be_bytes());
    sha1_block(&mut state, &block);
    let mut out = [0u8; 20];
    for (chunk, word) in out.chunks_exact_mut(4).zip(state) {
        chunk.copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn sha1_block(state: &mut [u32; 5], block: &[u8]) {
    let mut w = [0u32; 80];
    for (index, chunk) in block.chunks_exact(4).enumerate() {
        w[index] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
    }
    for index in 16..80 {
        w[index] = (w[index - 3] ^ w[index - 8] ^ w[index - 14] ^ w[index - 16]).rotate_left(1);
    }
    let [mut a, mut b, mut c, mut d, mut e] = *state;
    for (index, word) in w.iter().enumerate() {
        let (f, k) = match index {
            0..=19 => ((b & c) | (!b & d), 0x5a82_7999),
            20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
            40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
            _ => (b ^ c ^ d, 0xca62_c1d6),
        };
        let next = a
            .rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(*word);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = next;
    }
    for (slot, value) in state.iter_mut().zip([a, b, c, d, e]) {
        *slot = slot.wrapping_add(value);
    }
}

pub fn hmac_sha1(key: &[u8], parts: &[&[u8]]) -> [u8; 20] {
    let mut padded = [0u8; SHA1_BLOCK];
    if key.len() > SHA1_BLOCK {
        padded[..20].copy_from_slice(&sha1(key));
    } else {
        padded[..key.len()].copy_from_slice(key);
    }
    let mut inner = [0u8; SHA1_BLOCK + 512];
    let mut outer = [0u8; SHA1_BLOCK + 20];
    for index in 0..SHA1_BLOCK {
        inner[index] = padded[index] ^ 0x36;
        outer[index] = padded[index] ^ 0x5c;
    }
    let mut length = SHA1_BLOCK;
    for part in parts {
        inner[length..length + part.len()].copy_from_slice(part);
        length += part.len();
    }
    outer[SHA1_BLOCK..].copy_from_slice(&sha1(&inner[..length]));
    sha1(&outer)
}

/// PBKDF2 with HMAC-SHA1 (RFC 2898).
pub fn pbkdf2_sha1(password: &[u8], salt: &[u8], iterations: u32, out: &mut [u8]) {
    for (block_index, chunk) in out.chunks_mut(20).enumerate() {
        let counter = (block_index as u32 + 1).to_be_bytes();
        let mut u = hmac_sha1(password, &[salt, &counter]);
        let mut total = u;
        for _ in 1..iterations {
            u = hmac_sha1(password, &[&u]);
            for (sum, byte) in total.iter_mut().zip(u) {
                *sum ^= byte;
            }
        }
        chunk.copy_from_slice(&total[..chunk.len()]);
    }
}

/// The pairwise master key for a network: 8 to 63 printable ASCII characters,
/// the SSID as salt, 4096 rounds.
pub fn passphrase_to_pmk(passphrase: &str, ssid: &[u8]) -> Option<[u8; 32]> {
    let bytes = passphrase.as_bytes();
    if !(8..=63).contains(&bytes.len())
        || ssid.is_empty()
        || ssid.len() > 32
        || !bytes.iter().all(|byte| (0x20..0x7f).contains(byte))
    {
        return None;
    }
    let mut pmk = [0u8; 32];
    pbkdf2_sha1(bytes, ssid, 4096, &mut pmk);
    Some(pmk)
}

/// The IEEE 802.11 pseudo-random function built on HMAC-SHA1.
pub fn prf(key: &[u8], label: &[u8], data: &[u8], out: &mut [u8]) {
    for (index, chunk) in out.chunks_mut(20).enumerate() {
        let block = hmac_sha1(key, &[label, &[0], data, &[index as u8]]);
        chunk.copy_from_slice(&block[..chunk.len()]);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ptk {
    pub kck: [u8; 16],
    pub kek: [u8; 16],
    pub tk: [u8; 16],
}

pub fn derive_ptk(
    pmk: &[u8; 32],
    authenticator: &[u8; 6],
    supplicant: &[u8; 6],
    anonce: &[u8; 32],
    snonce: &[u8; 32],
) -> Ptk {
    let mut data = [0u8; 76];
    let (low, high) = if authenticator <= supplicant {
        (authenticator, supplicant)
    } else {
        (supplicant, authenticator)
    };
    data[..6].copy_from_slice(low);
    data[6..12].copy_from_slice(high);
    let (low, high) = if anonce <= snonce {
        (anonce, snonce)
    } else {
        (snonce, anonce)
    };
    data[12..44].copy_from_slice(low);
    data[44..76].copy_from_slice(high);
    let mut key = [0u8; 48];
    prf(pmk, b"Pairwise key expansion", &data, &mut key);
    let mut ptk = Ptk {
        kck: [0; 16],
        kek: [0; 16],
        tk: [0; 16],
    };
    ptk.kck.copy_from_slice(&key[..16]);
    ptk.kek.copy_from_slice(&key[16..32]);
    ptk.tk.copy_from_slice(&key[32..48]);
    ptk
}

// ----- AES-128 (decryption direction, for the key unwrap) ------------------

const fn xtime(value: u8) -> u8 {
    (value << 1) ^ if value & 0x80 != 0 { 0x1b } else { 0 }
}

const fn build_sbox() -> [u8; 256] {
    let mut sbox = [0u8; 256];
    let mut p: u8 = 1;
    let mut q: u8 = 1;
    loop {
        p ^= xtime(p);
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

const SBOX: [u8; 256] = build_sbox();

const fn build_inverse_sbox() -> [u8; 256] {
    let mut inverse = [0u8; 256];
    let mut index = 0;
    while index < 256 {
        inverse[SBOX[index] as usize] = index as u8;
        index += 1;
    }
    inverse
}

const INVERSE_SBOX: [u8; 256] = build_inverse_sbox();

fn multiply(mut a: u8, mut b: u8) -> u8 {
    let mut product = 0;
    while b != 0 {
        if b & 1 != 0 {
            product ^= a;
        }
        a = xtime(a);
        b >>= 1;
    }
    product
}

pub fn expand_key(key: &[u8; 16]) -> [[u8; 16]; 11] {
    let mut words = [[0u8; 4]; 44];
    for (index, word) in words.iter_mut().take(4).enumerate() {
        word.copy_from_slice(&key[index * 4..index * 4 + 4]);
    }
    let mut round_constant = 1u8;
    for index in 4..44 {
        let mut temp = words[index - 1];
        if index % 4 == 0 {
            temp = [
                SBOX[temp[1] as usize] ^ round_constant,
                SBOX[temp[2] as usize],
                SBOX[temp[3] as usize],
                SBOX[temp[0] as usize],
            ];
            round_constant = xtime(round_constant);
        }
        for byte in 0..4 {
            words[index][byte] = words[index - 4][byte] ^ temp[byte];
        }
    }
    let mut keys = [[0u8; 16]; 11];
    for (round, key) in keys.iter_mut().enumerate() {
        for column in 0..4 {
            key[column * 4..column * 4 + 4].copy_from_slice(&words[round * 4 + column]);
        }
    }
    keys
}

fn add_round_key(state: &mut [u8; 16], key: &[u8; 16]) {
    for (byte, k) in state.iter_mut().zip(key) {
        *byte ^= k;
    }
}

pub fn encrypt_block(keys: &[[u8; 16]; 11], block: &mut [u8; 16]) {
    add_round_key(block, &keys[0]);
    for round in 1..=10 {
        for byte in block.iter_mut() {
            *byte = SBOX[*byte as usize];
        }
        // ShiftRows (column-major state).
        let copy = *block;
        for column in 0..4 {
            for row in 0..4 {
                block[column * 4 + row] = copy[((column + row) % 4) * 4 + row];
            }
        }
        if round != 10 {
            for column in 0..4 {
                let s = [
                    block[column * 4],
                    block[column * 4 + 1],
                    block[column * 4 + 2],
                    block[column * 4 + 3],
                ];
                block[column * 4] = multiply(s[0], 2) ^ multiply(s[1], 3) ^ s[2] ^ s[3];
                block[column * 4 + 1] = s[0] ^ multiply(s[1], 2) ^ multiply(s[2], 3) ^ s[3];
                block[column * 4 + 2] = s[0] ^ s[1] ^ multiply(s[2], 2) ^ multiply(s[3], 3);
                block[column * 4 + 3] = multiply(s[0], 3) ^ s[1] ^ s[2] ^ multiply(s[3], 2);
            }
        }
        add_round_key(block, &keys[round]);
    }
}

pub fn decrypt_block(keys: &[[u8; 16]; 11], block: &mut [u8; 16]) {
    add_round_key(block, &keys[10]);
    for round in (0..10).rev() {
        // InvShiftRows.
        let copy = *block;
        for column in 0..4 {
            for row in 0..4 {
                block[((column + row) % 4) * 4 + row] = copy[column * 4 + row];
            }
        }
        for byte in block.iter_mut() {
            *byte = INVERSE_SBOX[*byte as usize];
        }
        add_round_key(block, &keys[round]);
        if round != 0 {
            for column in 0..4 {
                let s = [
                    block[column * 4],
                    block[column * 4 + 1],
                    block[column * 4 + 2],
                    block[column * 4 + 3],
                ];
                block[column * 4] = multiply(s[0], 14)
                    ^ multiply(s[1], 11)
                    ^ multiply(s[2], 13)
                    ^ multiply(s[3], 9);
                block[column * 4 + 1] = multiply(s[0], 9)
                    ^ multiply(s[1], 14)
                    ^ multiply(s[2], 11)
                    ^ multiply(s[3], 13);
                block[column * 4 + 2] = multiply(s[0], 13)
                    ^ multiply(s[1], 9)
                    ^ multiply(s[2], 14)
                    ^ multiply(s[3], 11);
                block[column * 4 + 3] = multiply(s[0], 11)
                    ^ multiply(s[1], 13)
                    ^ multiply(s[2], 9)
                    ^ multiply(s[3], 14);
            }
        }
    }
}

/// AES key unwrap (RFC 3394). `wrapped` is the 8-byte-aligned ciphertext;
/// the plaintext (8 bytes shorter) lands in `out`. False when the integrity
/// check fails.
pub fn aes_unwrap(kek: &[u8; 16], wrapped: &[u8], out: &mut [u8]) -> bool {
    let blocks = wrapped.len() / 8;
    if !wrapped.len().is_multiple_of(8) || blocks < 3 || out.len() < wrapped.len() - 8 {
        return false;
    }
    let keys = expand_key(kek);
    let mut a = [0u8; 8];
    a.copy_from_slice(&wrapped[..8]);
    out[..wrapped.len() - 8].copy_from_slice(&wrapped[8..]);
    let n = blocks - 1;
    for j in (0..6).rev() {
        for i in (1..=n).rev() {
            let t = (n * j + i) as u64;
            for (byte, tb) in a.iter_mut().zip(t.to_be_bytes()) {
                *byte ^= tb;
            }
            let mut block = [0u8; 16];
            block[..8].copy_from_slice(&a);
            block[8..].copy_from_slice(&out[(i - 1) * 8..i * 8]);
            decrypt_block(&keys, &mut block);
            a.copy_from_slice(&block[..8]);
            out[(i - 1) * 8..i * 8].copy_from_slice(&block[8..]);
        }
    }
    a == [0xa6; 8]
}

/// AES key wrap (RFC 3394); the authenticator side, used by the tests.
pub fn aes_wrap(kek: &[u8; 16], plain: &[u8], out: &mut [u8]) {
    let keys = expand_key(kek);
    let n = plain.len() / 8;
    let mut a = [0xa6u8; 8];
    out[8..8 + plain.len()].copy_from_slice(plain);
    for j in 0..6 {
        for i in 1..=n {
            let mut block = [0u8; 16];
            block[..8].copy_from_slice(&a);
            block[8..].copy_from_slice(&out[i * 8..(i + 1) * 8]);
            encrypt_block(&keys, &mut block);
            let t = (n * j + i) as u64;
            a.copy_from_slice(&block[..8]);
            for (byte, tb) in a.iter_mut().zip(t.to_be_bytes()) {
                *byte ^= tb;
            }
            out[i * 8..(i + 1) * 8].copy_from_slice(&block[8..]);
        }
    }
    out[..8].copy_from_slice(&a);
}

// ----- EAPOL-Key ------------------------------------------------------------

const EAPOL_KEY: u8 = 3;
const DESCRIPTOR_RSN: u8 = 2;
const HEADER: usize = 4;
const FIXED: usize = 95;
const MIC_AT: usize = HEADER + 77;

pub const INFO_VERSION_2: u16 = 2;
pub const INFO_PAIRWISE: u16 = 1 << 3;
pub const INFO_INSTALL: u16 = 1 << 6;
pub const INFO_ACK: u16 = 1 << 7;
pub const INFO_MIC: u16 = 1 << 8;
pub const INFO_SECURE: u16 = 1 << 9;
pub const INFO_ENCRYPTED: u16 = 1 << 12;

pub struct KeyFrame<'a> {
    pub info: u16,
    pub replay: u64,
    pub nonce: [u8; 32],
    pub mic: [u8; 16],
    pub data: &'a [u8],
}

pub fn parse_key_frame(frame: &[u8]) -> Option<KeyFrame<'_>> {
    if frame.len() < HEADER + FIXED || frame[1] != EAPOL_KEY || frame[HEADER] != DESCRIPTOR_RSN {
        return None;
    }
    let body = u16::from_be_bytes([frame[2], frame[3]]) as usize;
    if HEADER + body > frame.len() || body < FIXED {
        return None;
    }
    let data_length = u16::from_be_bytes([frame[HEADER + 93], frame[HEADER + 94]]) as usize;
    if FIXED + data_length > body {
        return None;
    }
    let mut nonce = [0u8; 32];
    nonce.copy_from_slice(&frame[HEADER + 13..HEADER + 45]);
    let mut mic = [0u8; 16];
    mic.copy_from_slice(&frame[MIC_AT..MIC_AT + 16]);
    Some(KeyFrame {
        info: u16::from_be_bytes([frame[HEADER + 1], frame[HEADER + 2]]),
        replay: u64::from_be_bytes(frame[HEADER + 5..HEADER + 13].try_into().ok()?),
        nonce,
        mic,
        data: &frame[HEADER + FIXED..HEADER + FIXED + data_length],
    })
}

pub const MAX_FRAME: usize = 256;

/// Writes a pairwise EAPOL-Key frame into `out` and, when `kck` is given,
/// its MIC. Returns the length.
pub fn build_key_frame(
    out: &mut [u8; MAX_FRAME],
    info: u16,
    replay: u64,
    nonce: &[u8; 32],
    data: &[u8],
    kck: Option<&[u8; 16]>,
) -> usize {
    let body = FIXED + data.len();
    out[..HEADER + body].fill(0);
    out[0] = 2;
    out[1] = EAPOL_KEY;
    out[2..4].copy_from_slice(&(body as u16).to_be_bytes());
    out[HEADER] = DESCRIPTOR_RSN;
    out[HEADER + 1..HEADER + 3].copy_from_slice(&info.to_be_bytes());
    out[HEADER + 3..HEADER + 5].copy_from_slice(&16u16.to_be_bytes());
    out[HEADER + 5..HEADER + 13].copy_from_slice(&replay.to_be_bytes());
    out[HEADER + 13..HEADER + 45].copy_from_slice(nonce);
    out[HEADER + 93..HEADER + 95].copy_from_slice(&(data.len() as u16).to_be_bytes());
    out[HEADER + FIXED..HEADER + FIXED + data.len()].copy_from_slice(data);
    let length = HEADER + body;
    if let Some(kck) = kck {
        let mic = hmac_sha1(kck, &[&out[..length]]);
        out[MIC_AT..MIC_AT + 16].copy_from_slice(&mic[..16]);
    }
    length
}

pub fn mic_matches(kck: &[u8; 16], frame: &[u8], mic: &[u8; 16]) -> bool {
    let mut copy = [0u8; MAX_FRAME];
    if frame.len() > MAX_FRAME {
        return false;
    }
    copy[..frame.len()].copy_from_slice(frame);
    copy[MIC_AT..MIC_AT + 16].fill(0);
    let expected = hmac_sha1(kck, &[&copy[..frame.len()]]);
    let mut difference = 0u8;
    for (a, b) in expected[..16].iter().zip(mic) {
        difference |= a ^ b;
    }
    difference == 0
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    Malformed,
    UnexpectedMessage,
    BadMic,
    Replay,
    BadKeyData,
    WrongNonce,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    WaitingForMessage1,
    WaitingForMessage3,
    Done,
}

/// What handling a frame produced.
pub enum Reply {
    /// Send this many bytes of the output buffer to the access point.
    Send(usize),
    /// The handshake is over: install the pairwise key and the group key;
    /// the last message is `send` bytes of the buffer.
    Complete {
        send: usize,
        tk: [u8; 16],
        gtk: [u8; 32],
        gtk_length: usize,
        gtk_index: u8,
    },
}

pub struct Supplicant {
    pmk: [u8; 32],
    authenticator: [u8; 6],
    address: [u8; 6],
    snonce: [u8; 32],
    anonce: [u8; 32],
    rsn: [u8; 64],
    rsn_length: usize,
    ptk: Option<Ptk>,
    replay: u64,
    state: State,
}

impl Supplicant {
    /// `rsn_information` is the RSN element this station put in its
    /// association request; the access point compares it in message 2.
    pub fn new(
        pmk: [u8; 32],
        authenticator: [u8; 6],
        address: [u8; 6],
        snonce: [u8; 32],
        rsn_information: &[u8],
    ) -> Option<Self> {
        if rsn_information.len() > 64 {
            return None;
        }
        let mut rsn = [0u8; 64];
        rsn[..rsn_information.len()].copy_from_slice(rsn_information);
        Some(Self {
            pmk,
            authenticator,
            address,
            snonce,
            anonce: [0; 32],
            rsn,
            rsn_length: rsn_information.len(),
            ptk: None,
            replay: 0,
            state: State::WaitingForMessage1,
        })
    }

    pub fn handle(&mut self, frame: &[u8], out: &mut [u8; MAX_FRAME]) -> Result<Reply, Error> {
        let key = parse_key_frame(frame).ok_or(Error::Malformed)?;
        let pairwise = key.info & INFO_PAIRWISE != 0;
        if !pairwise || key.info & 7 != INFO_VERSION_2 || key.info & INFO_ACK == 0 {
            return Err(Error::UnexpectedMessage);
        }
        match self.state {
            State::WaitingForMessage1 | State::WaitingForMessage3
                if key.info & INFO_MIC == 0 && key.info & INFO_INSTALL == 0 =>
            {
                // Message 1 (also when the access point starts over).
                self.anonce = key.nonce;
                let ptk = derive_ptk(
                    &self.pmk,
                    &self.authenticator,
                    &self.address,
                    &self.anonce,
                    &self.snonce,
                );
                self.ptk = Some(ptk);
                self.replay = key.replay;
                self.state = State::WaitingForMessage3;
                let send = build_key_frame(
                    out,
                    INFO_VERSION_2 | INFO_PAIRWISE | INFO_MIC,
                    key.replay,
                    &self.snonce,
                    &self.rsn[..self.rsn_length],
                    Some(&ptk.kck),
                );
                Ok(Reply::Send(send))
            }
            State::WaitingForMessage3 if key.info & INFO_MIC != 0 => {
                let ptk = self.ptk.ok_or(Error::UnexpectedMessage)?;
                if key.info & (INFO_INSTALL | INFO_SECURE | INFO_ENCRYPTED)
                    != (INFO_INSTALL | INFO_SECURE | INFO_ENCRYPTED)
                {
                    return Err(Error::UnexpectedMessage);
                }
                if key.nonce != self.anonce {
                    return Err(Error::WrongNonce);
                }
                if key.replay <= self.replay {
                    return Err(Error::Replay);
                }
                if !mic_matches(&ptk.kck, frame, &key.mic) {
                    return Err(Error::BadMic);
                }
                let mut plain = [0u8; MAX_FRAME];
                if !aes_unwrap(&ptk.kek, key.data, &mut plain) {
                    return Err(Error::BadKeyData);
                }
                let (gtk, gtk_length, gtk_index) =
                    find_group_key(&plain[..key.data.len() - 8]).ok_or(Error::BadKeyData)?;
                self.replay = key.replay;
                self.state = State::Done;
                let send = build_key_frame(
                    out,
                    INFO_VERSION_2 | INFO_PAIRWISE | INFO_MIC | INFO_SECURE,
                    key.replay,
                    &[0; 32],
                    &[],
                    Some(&ptk.kck),
                );
                Ok(Reply::Complete {
                    send,
                    tk: ptk.tk,
                    gtk,
                    gtk_length,
                    gtk_index,
                })
            }
            _ => Err(Error::UnexpectedMessage),
        }
    }
}

/// The group key in message 3's key data: a vendor element with the OUI
/// 00-0F-AC and data type 1 (GTK KDE), after the RSN element.
fn find_group_key(data: &[u8]) -> Option<([u8; 32], usize, u8)> {
    let mut at = 0;
    while at + 2 <= data.len() {
        let kind = data[at];
        let length = data[at + 1] as usize;
        if kind == 0xdd && length == 0 {
            break; // padding
        }
        let body = data.get(at + 2..at + 2 + length)?;
        if kind == 0xdd && body.len() >= 6 && body[..4] == [0x00, 0x0f, 0xac, 0x01] {
            let key = &body[6..];
            if key.is_empty() || key.len() > 32 {
                return None;
            }
            let mut gtk = [0u8; 32];
            gtk[..key.len()].copy_from_slice(key);
            return Some((gtk, key.len(), body[4] & 3));
        }
        at += 2 + length;
    }
    None
}

/// Known-answer checks for the boot-test: the IEEE passphrase vector, the
/// SHA-1 vector and the RFC 3394 key wrap.
pub fn self_test() -> bool {
    let pmk = passphrase_to_pmk("password", b"IEEE");
    let expected = [
        0xf4, 0x2c, 0x6f, 0xc5, 0x2d, 0xf0, 0xeb, 0xef, 0x9e, 0xbb, 0x4b, 0x90, 0xb3, 0x8a, 0x5f,
        0x90, 0x2e, 0x83, 0xfe, 0x1b, 0x13, 0x5a, 0x70, 0xe2, 0x3a, 0xed, 0x76, 0x2e, 0x97, 0x10,
        0xa1, 0x2e,
    ];
    let abc = sha1(b"abc");
    let wrapped = [
        0x1f, 0xa6, 0x8b, 0x0a, 0x81, 0x12, 0xb4, 0x47, 0xae, 0xf3, 0x4b, 0xd8, 0xfb, 0x5a, 0x7b,
        0x82, 0x9d, 0x3e, 0x86, 0x23, 0x71, 0xd2, 0xcf, 0xe5,
    ];
    let mut key = [0u8; 16];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = index as u8;
    }
    let mut plain = [0u8; 16];
    pmk == Some(expected)
        && abc[..4] == [0xa9, 0x99, 0x3e, 0x36]
        && aes_unwrap(&key, &wrapped, &mut plain)
        && plain[0] == 0x00
        && plain[1] == 0x11
        && plain[15] == 0xff
}
