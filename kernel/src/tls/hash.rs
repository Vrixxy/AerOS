//! HMAC-SHA256 and the HKDF helpers of the TLS 1.3 key schedule (RFC 8446
//! section 7.1). Only SHA-256 suites are offered, so only SHA-256 is needed
//! for keys; SHA-384 and SHA-512 appear only in certificate signatures.

use crate::auth::{Sha256, sha256};

pub const HASH_LEN: usize = 32;

pub fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&sha256(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut pad = [0u8; 64];
    for (slot, byte) in pad.iter_mut().zip(block.iter()) {
        *slot = byte ^ 0x36;
    }
    let mut inner = Sha256::new();
    inner.update(&pad);
    for part in parts {
        inner.update(part);
    }
    let inner_digest = inner.finish();
    for (slot, byte) in pad.iter_mut().zip(block.iter()) {
        *slot = byte ^ 0x5c;
    }
    let mut outer = Sha256::new();
    outer.update(&pad);
    outer.update(&inner_digest);
    outer.finish()
}

pub fn hkdf_extract(salt: &[u8], input: &[u8]) -> [u8; 32] {
    hmac_sha256(salt, &[input])
}

/// HKDF-Expand-Label. Every TLS 1.3 output with SHA-256 fits one HMAC block.
pub fn expand_label(secret: &[u8; 32], label: &[u8], context: &[u8], out: &mut [u8]) {
    let mut info = [0u8; 96];
    let mut length = 0;
    let mut put = |bytes: &[u8]| {
        info[length..length + bytes.len()].copy_from_slice(bytes);
        length += bytes.len();
    };
    put(&(out.len() as u16).to_be_bytes());
    put(&[(6 + label.len()) as u8]);
    put(b"tls13 ");
    put(label);
    put(&[context.len() as u8]);
    put(context);
    let block = hmac_sha256(secret, &[&info[..length], &[1]]);
    let count = out.len().min(32);
    out[..count].copy_from_slice(&block[..count]);
}

/// Derive-Secret: the label expanded over a transcript hash.
pub fn derive_secret(secret: &[u8; 32], label: &[u8], transcript_hash: &[u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    expand_label(secret, label, transcript_hash, &mut out);
    out
}

pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    crate::auth::constant_time_eq(a, b)
}

/// The digests certificates and handshake signatures are made with.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HashAlg {
    Sha256,
    Sha384,
    Sha512,
}

impl HashAlg {
    pub const fn len(self) -> usize {
        match self {
            Self::Sha256 => 32,
            Self::Sha384 => 48,
            Self::Sha512 => 64,
        }
    }

    pub fn digest(self, data: &[u8]) -> ([u8; 64], usize) {
        let mut out = [0u8; 64];
        match self {
            Self::Sha256 => out[..32].copy_from_slice(&sha256(data)),
            Self::Sha384 => out[..48].copy_from_slice(&crate::ed25519::sha384(data)),
            Self::Sha512 => out.copy_from_slice(&crate::ed25519::sha512(data)),
        }
        (out, self.len())
    }
}
