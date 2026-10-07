use tls_host::ed25519::{sha384, sha512};
use tls_host::tls::aead::{Suite, open, seal};
use tls_host::tls::hash::{expand_label, hkdf_extract, hmac_sha256};
use tls_host::tls::x25519;

fn hex(text: &str) -> Vec<u8> {
    let text: String = text.split_whitespace().collect();
    (0..text.len() / 2)
        .map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap())
        .collect()
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn sha384_and_sha512() {
    assert_eq!(
        to_hex(&sha384(b"abc")),
        "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7"
    );
    assert_eq!(
        to_hex(&sha384(b"")),
        "38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b"
    );
    assert!(to_hex(&sha512(b"abc")).starts_with("ddaf35a193617abacc417349ae204131"));
}

#[test]
fn hmac_sha256_matches_python() {
    let key: Vec<u8> = (0..100).collect();
    assert_eq!(
        to_hex(&hmac_sha256(&key, &[b"The quick ", b"brown fox"])),
        "14d1a30bf7efdf7b1f80545f96cb2d6b03c37e8c0640dff2bdb98478521b4e4a"
    );
    assert_eq!(
        to_hex(&hmac_sha256(b"key", &[b"abc"])),
        "9c196e32dc0175f86f4b1cb89289d6619de6bee699e4c378e68309ed97a1a6ab"
    );
}

#[test]
fn hkdf_rfc5869_case_1() {
    let ikm = hex("0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b");
    let salt = hex("000102030405060708090a0b0c");
    let prk = hkdf_extract(&salt, &ikm);
    assert_eq!(
        to_hex(&prk),
        "077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5"
    );
}

#[test]
fn tls13_expand_label_rfc8448() {
    // RFC 8448 section 3: the client handshake traffic secret and the keys
    // derived from it.
    let secret: [u8; 32] = hex("b3eddb126e067f35a780b3abf45e2d8f3b1a950738f52e9600746a0e27a55a21")
        .try_into()
        .unwrap();
    let mut key = [0u8; 16];
    expand_label(&secret, b"key", b"", &mut key);
    assert_eq!(to_hex(&key), "dbfaa693d1762c5b666af5d950258d01");
    let mut iv = [0u8; 12];
    expand_label(&secret, b"iv", b"", &mut iv);
    assert_eq!(to_hex(&iv), "5bd3c71b836e0b76bb73265f");
}

#[test]
fn x25519_rfc7748() {
    let scalar: [u8; 32] = hex("a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4")
        .try_into()
        .unwrap();
    let point: [u8; 32] = hex("e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c")
        .try_into()
        .unwrap();
    assert_eq!(
        to_hex(&x25519::scalar_mult(&scalar, &point)),
        "c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552"
    );
    let alice: [u8; 32] = hex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a")
        .try_into()
        .unwrap();
    let bob: [u8; 32] = hex("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb")
        .try_into()
        .unwrap();
    let alice_public = x25519::public_key(&alice);
    let bob_public = x25519::public_key(&bob);
    assert_eq!(
        to_hex(&alice_public),
        "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a"
    );
    assert_eq!(
        to_hex(&bob_public),
        "de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f"
    );
    let shared = "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742";
    assert_eq!(to_hex(&x25519::shared_secret(&alice, &bob_public).unwrap()), shared);
    assert_eq!(to_hex(&x25519::shared_secret(&bob, &alice_public).unwrap()), shared);
    assert!(x25519::shared_secret(&alice, &[0; 32]).is_none());
}

#[test]
fn chacha20_poly1305_rfc8439() {
    let key = hex("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f");
    let nonce: [u8; 12] = hex("070000004041424344454647").try_into().unwrap();
    let aad = hex("50515253c0c1c2c3c4c5c6c7");
    let plaintext = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
    let mut data = plaintext.to_vec();
    let tag = seal(Suite::ChaCha20Poly1305, &key, &nonce, &aad, &mut data);
    assert_eq!(
        to_hex(&data),
        "d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d63dbea45e8ca9671282fafb69da92728b1a71de0a9e060b2905d6a5b67ecd3b3692ddbd7f2d778b8c9803aee328091b58fab324e4fad675945585808b4831d7bc3ff4def08e4b7a9de576d26586cec64b6116"
    );
    assert_eq!(to_hex(&tag), "1ae10b594f09e26a7e902ecbd0600691");
    assert!(open(Suite::ChaCha20Poly1305, &key, &nonce, &aad, &mut data, &tag));
    assert_eq!(data, plaintext);
    let mut bad = tag;
    bad[0] ^= 1;
    assert!(!open(Suite::ChaCha20Poly1305, &key, &nonce, &aad, &mut data, &bad));
}

#[test]
fn aes128_gcm_nist_cases() {
    let zero_key = [0u8; 16];
    let zero_nonce = [0u8; 12];
    let mut empty: [u8; 0] = [];
    let tag = seal(Suite::Aes128Gcm, &zero_key, &zero_nonce, &[], &mut empty);
    assert_eq!(to_hex(&tag), "58e2fccefa7e3061367f1d57a4e7455a");
    let mut block = [0u8; 16];
    let tag = seal(Suite::Aes128Gcm, &zero_key, &zero_nonce, &[], &mut block);
    assert_eq!(to_hex(&block), "0388dace60b6a392f328c2b971b2fe78");
    assert_eq!(to_hex(&tag), "ab6e47d42cec13bdf53a67b21257bddf");

    // Test case 4 of the GCM specification: 60 bytes with additional data.
    let key = hex("feffe9928665731c6d6a8f9467308308");
    let nonce: [u8; 12] = hex("cafebabefacedbaddecaf888").try_into().unwrap();
    let aad = hex("feedfacedeadbeeffeedfacedeadbeefabaddad2");
    let mut data = hex(
        "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a72\
         1c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b39",
    );
    let tag = seal(Suite::Aes128Gcm, &key, &nonce, &aad, &mut data);
    assert_eq!(
        to_hex(&data),
        "42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e21d514b25466931c7d8f6a5aac84aa051ba30b396a0aac973d58e091"
    );
    assert_eq!(to_hex(&tag), "5bc94fbc3221a5db94fae95ae7121a47");
    assert!(open(Suite::Aes128Gcm, &key, &nonce, &aad, &mut data, &tag));
}

#[cfg(feature = "boot-test")]
#[test]
fn kernel_self_test_passes() {
    let report = tls_host::tls::selftest::run();
    assert!(report.hashes && report.key_schedule && report.x25519 && report.aead);
    assert_eq!(report.signatures, 4);
    assert!(report.rejects_bad_signatures);
    assert!(report.verified());
}
