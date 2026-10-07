//! Boot-test known-answer checks for the TLS primitives: published vectors
//! for the hashes, key schedule, X25519 and both record ciphers, and four
//! signatures made by OpenSSL for the RSA and ECDSA verifiers.

use super::aead::{self, Suite};
use super::ecdsa::{self, Curve};
use super::hash::{HashAlg, expand_label, hkdf_extract, hmac_sha256};
use super::rsa;
use super::vectors::{MESSAGE, VECTORS};
use super::x25519;

pub struct Report {
    pub hashes: bool,
    pub key_schedule: bool,
    pub x25519: bool,
    pub aead: bool,
    pub signatures: usize,
    pub rejects_bad_signatures: bool,
}

impl Report {
    pub fn verified(&self) -> bool {
        self.hashes
            && self.key_schedule
            && self.x25519
            && self.aead
            && self.signatures == VECTORS.len()
            && self.rejects_bad_signatures
    }
}

fn unhex(text: &str, out: &mut [u8]) -> Option<usize> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(2) || bytes.len() / 2 > out.len() {
        return None;
    }
    let nibble = |digit: u8| match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        _ => None,
    };
    for (index, pair) in bytes.chunks_exact(2).enumerate() {
        out[index] = nibble(pair[0])? << 4 | nibble(pair[1])?;
    }
    Some(bytes.len() / 2)
}

fn matches(text: &str, actual: &[u8]) -> bool {
    let mut expected = [0u8; 160];
    unhex(text, &mut expected) == Some(actual.len()) && expected[..actual.len()] == *actual
}

fn hashes() -> bool {
    let key: [u8; 100] = core::array::from_fn(|index| index as u8);
    matches(
        "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7",
        &crate::ed25519::sha384(b"abc"),
    ) && matches(
        "14d1a30bf7efdf7b1f80545f96cb2d6b03c37e8c0640dff2bdb98478521b4e4a",
        &hmac_sha256(&key, &[b"The quick ", b"brown fox"]),
    )
}

fn key_schedule() -> bool {
    let mut input = [0u8; 22];
    input.fill(0x0b);
    let mut salt = [0u8; 13];
    for (index, byte) in salt.iter_mut().enumerate() {
        *byte = index as u8;
    }
    let prk = hkdf_extract(&salt, &input);
    let mut secret = [0u8; 32];
    unhex(
        "b3eddb126e067f35a780b3abf45e2d8f3b1a950738f52e9600746a0e27a55a21",
        &mut secret,
    );
    let mut key = [0u8; 16];
    let mut iv = [0u8; 12];
    expand_label(&secret, b"key", b"", &mut key);
    expand_label(&secret, b"iv", b"", &mut iv);
    matches(
        "077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5",
        &prk,
    ) && matches("dbfaa693d1762c5b666af5d950258d01", &key)
        && matches("5bd3c71b836e0b76bb73265f", &iv)
}

fn x25519_vectors() -> bool {
    let mut alice = [0u8; 32];
    let mut bob = [0u8; 32];
    unhex(
        "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a",
        &mut alice,
    );
    unhex(
        "5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb",
        &mut bob,
    );
    let alice_public = x25519::public_key(&alice);
    let bob_public = x25519::public_key(&bob);
    let shared = "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742";
    matches(
        "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a",
        &alice_public,
    ) && x25519::shared_secret(&alice, &bob_public).is_some_and(|s| matches(shared, &s))
        && x25519::shared_secret(&bob, &alice_public).is_some_and(|s| matches(shared, &s))
        && x25519::shared_secret(&alice, &[0; 32]).is_none()
}

fn ciphers() -> bool {
    // RFC 8439 section 2.8.2.
    let mut key = [0u8; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = 0x80 + index as u8;
    }
    let nonce = [0x07, 0, 0, 0, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47];
    let mut aad = [0u8; 12];
    unhex("50515253c0c1c2c3c4c5c6c7", &mut aad);
    let text = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
    let mut data = [0u8; 114];
    data.copy_from_slice(text);
    let tag = aead::seal(Suite::ChaCha20Poly1305, &key, &nonce, &aad, &mut data);
    let chacha = matches("1ae10b594f09e26a7e902ecbd0600691", &tag)
        && aead::open(Suite::ChaCha20Poly1305, &key, &nonce, &aad, &mut data, &tag)
        && data == *text;
    let mut flipped = tag;
    flipped[3] ^= 1;
    let chacha_rejects = !aead::open(Suite::ChaCha20Poly1305, &key, &nonce, &aad, &mut data, &flipped);

    // NIST GCM test cases 2 and 4.
    let zero_key = [0u8; 16];
    let zero_nonce = [0u8; 12];
    let mut block = [0u8; 16];
    let tag = aead::seal(Suite::Aes128Gcm, &zero_key, &zero_nonce, &[], &mut block);
    let gcm_two = matches("0388dace60b6a392f328c2b971b2fe78", &block)
        && matches("ab6e47d42cec13bdf53a67b21257bddf", &tag);
    let mut gcm_key = [0u8; 16];
    unhex("feffe9928665731c6d6a8f9467308308", &mut gcm_key);
    let mut gcm_nonce = [0u8; 12];
    unhex("cafebabefacedbaddecaf888", &mut gcm_nonce);
    let mut gcm_aad = [0u8; 20];
    unhex("feedfacedeadbeeffeedfacedeadbeefabaddad2", &mut gcm_aad);
    let mut plain = [0u8; 60];
    unhex(
        "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b39",
        &mut plain,
    );
    let tag = aead::seal(Suite::Aes128Gcm, &gcm_key, &gcm_nonce, &gcm_aad, &mut plain);
    let gcm_four = matches("5bc94fbc3221a5db94fae95ae7121a47", &tag)
        && aead::open(Suite::Aes128Gcm, &gcm_key, &gcm_nonce, &gcm_aad, &mut plain, &tag);
    chacha && chacha_rejects && gcm_two && gcm_four
}

fn check(vector: &super::vectors::Vector, message: &[u8], signature: &[u8]) -> bool {
    let mut key = [0u8; 256];
    let Some(key_length) = unhex(vector.key, &mut key) else {
        return false;
    };
    let key = &key[..key_length];
    let hash = match vector.hash {
        "sha256" => HashAlg::Sha256,
        _ => HashAlg::Sha384,
    };
    match (vector.kind, vector.variant) {
        ("rsa", "pkcs1") => rsa::verify_pkcs1(key, &[1, 0, 1], hash, message, signature),
        ("rsa", _) => rsa::verify_pss(key, &[1, 0, 1], hash, message, signature),
        (_, curve) => {
            let curve = if curve == "prime256v1" {
                Curve::P256
            } else {
                Curve::P384
            };
            let (digest, length) = hash.digest(message);
            ecdsa::verify(curve, key, &digest[..length], signature)
        }
    }
}

pub fn run() -> Report {
    let mut good = 0;
    let mut rejects = true;
    for vector in VECTORS {
        let mut signature = [0u8; 512];
        let Some(length) = unhex(vector.signature, &mut signature) else {
            continue;
        };
        let signature = &mut signature[..length];
        if check(vector, MESSAGE, signature) {
            good += 1;
        }
        let mut altered = [0u8; 64];
        altered[..MESSAGE.len()].copy_from_slice(MESSAGE);
        altered[3] ^= 1;
        rejects &= !check(vector, &altered[..MESSAGE.len()], signature);
        signature[length - 1] ^= 1;
        rejects &= !check(vector, MESSAGE, signature);
    }
    Report {
        hashes: hashes(),
        key_schedule: key_schedule(),
        x25519: x25519_vectors(),
        aead: ciphers(),
        signatures: good,
        rejects_bad_signatures: rejects,
    }
}
