mod fixtures;

use fixtures::{CASES, MESSAGE};
use tls_host::tls::ecdsa::{self, Curve};
use tls_host::tls::hash::HashAlg;
use tls_host::tls::rsa;

fn hex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap())
        .collect()
}

fn hash_of(name: &str) -> HashAlg {
    match name {
        "sha256" => HashAlg::Sha256,
        "sha384" => HashAlg::Sha384,
        other => panic!("{other}"),
    }
}

fn check(case: &fixtures::Case, message: &[u8], signature: &[u8]) -> bool {
    let key = hex(case.key);
    let hash = hash_of(case.hash);
    match (case.kind, case.variant) {
        ("rsa", "pkcs1") => rsa::verify_pkcs1(&key, &hex(case.exponent), hash, message, signature),
        ("rsa", "pss") => rsa::verify_pss(&key, &hex(case.exponent), hash, message, signature),
        ("ec", curve) => {
            let curve = if curve == "prime256v1" {
                Curve::P256
            } else {
                Curve::P384
            };
            let (digest, length) = hash.digest(message);
            ecdsa::verify(curve, &key, &digest[..length], signature)
        }
        _ => panic!(),
    }
}

#[test]
fn openssl_signatures_verify() {
    assert!(CASES.len() >= 16);
    for case in CASES {
        let signature = hex(case.signature);
        assert!(
            check(case, MESSAGE, &signature),
            "{} {} {} should verify",
            case.kind,
            case.variant,
            case.hash
        );
    }
}

#[test]
fn altered_message_or_signature_is_rejected() {
    for case in CASES {
        let signature = hex(case.signature);
        let mut message = MESSAGE.to_vec();
        message[3] ^= 1;
        assert!(
            !check(case, &message, &signature),
            "{} {} {} accepted a changed message",
            case.kind,
            case.variant,
            case.hash
        );
        let mut bad = signature.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(
            !check(case, MESSAGE, &bad),
            "{} {} {} accepted a changed signature",
            case.kind,
            case.variant,
            case.hash
        );
        assert!(!check(case, MESSAGE, &signature[..signature.len() - 1]));
    }
}

#[test]
fn wrong_key_is_rejected() {
    // Signature of one case against the key of another with the same kind.
    let a = &CASES[0];
    let b = CASES
        .iter()
        .find(|case| case.kind == "rsa" && case.variant == "pkcs1" && case.key != a.key)
        .unwrap();
    let swapped = fixtures::Case {
        kind: a.kind,
        variant: a.variant,
        hash: a.hash,
        key: b.key,
        exponent: b.exponent,
        signature: a.signature,
    };
    assert!(!check(&swapped, MESSAGE, &hex(a.signature)));
}
