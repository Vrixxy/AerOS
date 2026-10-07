mod chain_data {
    include!("chain_data.rs");
}

use chain_data::CHAINS;
use tls_host::tls::x509::{self, Certificate, Error, TrustAnchor};

fn hex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap())
        .collect()
}

fn leak(bytes: Vec<u8>) -> &'static [u8] {
    Box::leak(bytes.into_boxed_slice())
}

fn anchor_of(root: &str) -> TrustAnchor {
    let der = leak(hex(root));
    let certificate = Certificate::parse(der).unwrap();
    TrustAnchor {
        subject: certificate.subject,
        spki: certificate.spki,
    }
}

fn chain_of(case: &chain_data::Chain) -> Vec<&'static [u8]> {
    let mut chain = vec![leak(hex(case.leaf))];
    if !case.middle.is_empty() {
        chain.push(leak(hex(case.middle)));
    }
    chain
}

const NOW: u64 = 1_800_000_000;

fn find(name: &str) -> &'static chain_data::Chain {
    CHAINS.iter().find(|case| case.name == name).unwrap()
}

#[test]
fn good_chains_validate() {
    for name in ["rsa-rsa-rsa", "ec-p384-ec", "rsa-ec-ec", "direct-ed25519"] {
        let case = find(name);
        let anchors = [anchor_of(case.root)];
        let chain = chain_of(case);
        let result = x509::verify_chain(&chain, "aeros.test", NOW, &anchors);
        assert!(result.is_ok(), "{name}: {:?}", result.err());
    }
}

#[test]
fn host_names() {
    let case = find("rsa-rsa-rsa");
    let anchors = [anchor_of(case.root)];
    let chain = chain_of(case);
    let check = |host: &str| x509::verify_chain(&chain, host, NOW, &anchors).err();
    assert_eq!(check("aeros.test"), None);
    assert_eq!(check("AEROS.test"), None);
    assert_eq!(check("a.wild.test"), None);
    assert_eq!(check("10.0.2.2"), None);
    assert_eq!(check("wild.test"), Some(Error::HostName));
    assert_eq!(check("a.b.wild.test"), Some(Error::HostName));
    assert_eq!(check("other.test"), Some(Error::HostName));
    assert_eq!(check("10.0.2.3"), Some(Error::HostName));
    assert_eq!(check("xaeros.test"), Some(Error::HostName));
}

#[test]
fn time_window() {
    let case = find("ec-p384-ec");
    let anchors = [anchor_of(case.root)];
    let chain = chain_of(case);
    assert_eq!(
        x509::verify_chain(&chain, "aeros.test", 1_000_000_000, &anchors).err(),
        Some(Error::NotYetValid)
    );
    assert_eq!(
        x509::verify_chain(&chain, "aeros.test", 4_000_000_000, &anchors).err(),
        Some(Error::Expired)
    );
}

#[test]
fn untrusted_and_tampered_chains_fail() {
    let case = find("rsa-ec-ec");
    let chain = chain_of(case);
    let wrong = [anchor_of(find("other-root").root)];
    assert_eq!(
        x509::verify_chain(&chain, "aeros.test", NOW, &wrong).err(),
        Some(Error::Untrusted)
    );
    assert_eq!(
        x509::verify_chain(&chain, "aeros.test", NOW, &[]).err(),
        Some(Error::Untrusted)
    );
    let anchors = [anchor_of(case.root)];
    let mut broken = hex(case.leaf);
    let last = broken.len() - 5;
    broken[last] ^= 1;
    let broken_chain = vec![leak(broken), chain[1]];
    assert!(x509::verify_chain(&broken_chain, "aeros.test", NOW, &anchors).is_err());
    // A leaf alone has no path to the anchor.
    assert_eq!(
        x509::verify_chain(&chain[..1], "aeros.test", NOW, &anchors).err(),
        Some(Error::Untrusted)
    );
}

#[test]
fn intermediate_that_is_not_a_ca_is_refused() {
    let case = find("not-a-ca");
    let anchors = [anchor_of(case.root)];
    let chain = chain_of(case);
    assert_eq!(
        x509::verify_chain(&chain, "aeros.test", NOW, &anchors).err(),
        Some(Error::Constraint)
    );
}

#[test]
fn garbage_does_not_parse() {
    let case = find("rsa-rsa-rsa");
    let der = hex(case.leaf);
    for cut in [0, 1, 4, 10, der.len() / 2, der.len() - 1] {
        assert!(Certificate::parse(&der[..cut]).is_err());
    }
    let mut longer = der.clone();
    longer.push(0);
    assert!(Certificate::parse(&longer).is_err());
    for index in (0..der.len()).step_by(7) {
        let mut flipped = der.clone();
        flipped[index] ^= 0xff;
        let _ = Certificate::parse(&flipped);
    }
}
