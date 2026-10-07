//! RSA signature checks: PKCS#1 v1.5 (certificate chains) and PSS (TLS 1.3
//! CertificateVerify). Public-key operations only.

use super::bignum::{Big, Mont};
use super::hash::HashAlg;

const MIN_MODULUS_BYTES: usize = 256;
const MAX_MODULUS_BYTES: usize = 512;

/// `signature ^ e mod n` as `modulus.len()` big-endian bytes.
fn public_operation<const N: usize>(
    modulus: &[u8],
    exponent: u64,
    signature: &[u8],
    out: &mut [u8; MAX_MODULUS_BYTES],
) -> bool {
    let (Some(n), Some(s)) = (
        Big::<N>::from_be_bytes(modulus),
        Big::<N>::from_be_bytes(signature),
    ) else {
        return false;
    };
    let Some(context) = Mont::new(n) else {
        return false;
    };
    if s.compare(&n) != core::cmp::Ordering::Less {
        return false;
    }
    let base = context.to_mont(&s);
    let result = context.to_plain(&context.pow(&base, &Big::from_u64(exponent)));
    result.write_be(&mut out[..modulus.len()]);
    true
}

fn strip(bytes: &[u8]) -> &[u8] {
    let skip = bytes.iter().take_while(|byte| **byte == 0).count();
    &bytes[skip..]
}

fn recover(
    modulus: &[u8],
    exponent: &[u8],
    signature: &[u8],
    out: &mut [u8; MAX_MODULUS_BYTES],
) -> Option<usize> {
    let modulus = strip(modulus);
    let exponent = strip(exponent);
    if modulus.len() < MIN_MODULUS_BYTES
        || modulus.len() > MAX_MODULUS_BYTES
        || modulus[modulus.len() - 1] & 1 == 0
        || exponent.is_empty()
        || exponent.len() > 8
        || signature.len() != modulus.len()
    {
        return None;
    }
    let mut value = 0u64;
    for byte in exponent {
        value = (value << 8) | u64::from(*byte);
    }
    if value < 3 || value & 1 == 0 {
        return None;
    }
    let done = if modulus.len() <= 256 {
        public_operation::<32>(modulus, value, signature, out)
    } else {
        public_operation::<64>(modulus, value, signature, out)
    };
    done.then_some(modulus.len())
}

fn digest_info_prefix(hash: HashAlg) -> &'static [u8] {
    match hash {
        HashAlg::Sha256 => &[
            0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
            0x01, 0x05, 0x00, 0x04, 0x20,
        ],
        HashAlg::Sha384 => &[
            0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
            0x02, 0x05, 0x00, 0x04, 0x30,
        ],
        HashAlg::Sha512 => &[
            0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
            0x03, 0x05, 0x00, 0x04, 0x40,
        ],
    }
}

/// RSASSA-PKCS1-v1_5: the recovered block must equal the encoding of the
/// digest exactly.
pub fn verify_pkcs1(
    modulus: &[u8],
    exponent: &[u8],
    hash: HashAlg,
    message: &[u8],
    signature: &[u8],
) -> bool {
    let mut block = [0u8; MAX_MODULUS_BYTES];
    let Some(length) = recover(modulus, exponent, signature, &mut block) else {
        return false;
    };
    let prefix = digest_info_prefix(hash);
    let (digest, digest_len) = hash.digest(message);
    let info = prefix.len() + digest_len;
    if length < info + 11 {
        return false;
    }
    let padding = length - info - 3;
    let block = &block[..length];
    block[0] == 0
        && block[1] == 1
        && block[2..2 + padding].iter().all(|byte| *byte == 0xff)
        && block[2 + padding] == 0
        && block[3 + padding..3 + padding + prefix.len()] == *prefix
        && block[3 + padding + prefix.len()..] == digest[..digest_len]
}

fn mgf1(hash: HashAlg, seed: &[u8], out: &mut [u8]) {
    let mut input = [0u8; 64 + 4];
    input[..seed.len()].copy_from_slice(seed);
    for (counter, chunk) in out.chunks_mut(hash.len()).enumerate() {
        input[seed.len()..seed.len() + 4].copy_from_slice(&(counter as u32).to_be_bytes());
        let (digest, _) = hash.digest(&input[..seed.len() + 4]);
        chunk.copy_from_slice(&digest[..chunk.len()]);
    }
}

/// RSASSA-PSS with MGF1 over the same hash and a salt as long as the digest
/// (the only form TLS 1.3 allows).
pub fn verify_pss(
    modulus: &[u8],
    exponent: &[u8],
    hash: HashAlg,
    message: &[u8],
    signature: &[u8],
) -> bool {
    let mut block = [0u8; MAX_MODULUS_BYTES];
    let Some(length) = recover(modulus, exponent, signature, &mut block) else {
        return false;
    };
    let modulus = strip(modulus);
    let bits = (modulus.len() - 1) * 8 + (8 - modulus[0].leading_zeros() as usize) - 1;
    let em_len = bits.div_ceil(8);
    let hash_len = hash.len();
    let full = &block[..length];
    let em = if em_len == length {
        full
    } else if full[0] == 0 {
        &full[1..]
    } else {
        return false;
    };
    if em_len < 2 * hash_len + 2 || em[em_len - 1] != 0xbc {
        return false;
    }
    let top_mask = 0xffu8 >> (8 * em_len - bits);
    let db_len = em_len - hash_len - 1;
    if em[0] & !top_mask != 0 {
        return false;
    }
    let (masked, rest) = em.split_at(db_len);
    let seed = &rest[..hash_len];
    let mut db = [0u8; MAX_MODULUS_BYTES];
    mgf1(hash, seed, &mut db[..db_len]);
    for (byte, masked) in db[..db_len].iter_mut().zip(masked.iter()) {
        *byte ^= masked;
    }
    db[0] &= top_mask;
    let zeros = db_len - hash_len - 1;
    if db[..zeros].iter().any(|byte| *byte != 0) || db[zeros] != 1 {
        return false;
    }
    let salt = &db[zeros + 1..db_len];
    let (message_hash, _) = hash.digest(message);
    let mut prefixed = [0u8; 8 + 64 + 64];
    prefixed[8..8 + hash_len].copy_from_slice(&message_hash[..hash_len]);
    prefixed[8 + hash_len..8 + 2 * hash_len].copy_from_slice(salt);
    let (check, _) = hash.digest(&prefixed[..8 + 2 * hash_len]);
    check[..hash_len] == *seed
}
