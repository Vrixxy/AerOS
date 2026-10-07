//! X.509 certificates: parsing, hostname matching and chain validation up to
//! a built-in trust anchor.
//!
//! Supported: RSA (PKCS#1 v1.5 and, for handshakes, PSS), ECDSA on P-256 and
//! P-384, Ed25519. Not supported, and refused rather than ignored: name
//! constraints, unknown critical extensions, SHA-1 signatures. There is no
//! revocation checking (no CRL, OCSP or stapling).

use super::der::{self, Reader};
use super::ecdsa::{self, Curve};
use super::hash::HashAlg;
use super::rsa;

pub const MAX_CHAIN: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    Parse,
    NotYetValid,
    Expired,
    HostName,
    Untrusted,
    Signature,
    Constraint,
    UnsupportedAlgorithm,
    CriticalExtension,
}

#[derive(Clone, Copy)]
pub enum PublicKey<'a> {
    Rsa { modulus: &'a [u8], exponent: &'a [u8] },
    Ec { curve: Curve, point: &'a [u8] },
    Ed25519(&'a [u8]),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SignatureAlgorithm {
    RsaPkcs1(HashAlg),
    RsaPss(HashAlg),
    Ecdsa(HashAlg),
    Ed25519,
}

#[derive(Clone, Copy)]
pub struct Certificate<'a> {
    pub tbs: &'a [u8],
    pub issuer: &'a [u8],
    pub subject: &'a [u8],
    pub not_before: u64,
    pub not_after: u64,
    pub key: PublicKey<'a>,
    pub spki: &'a [u8],
    pub algorithm: SignatureAlgorithm,
    pub signature: &'a [u8],
    pub is_ca: bool,
    pub path_length: Option<u32>,
    pub key_usage: Option<u8>,
    pub server_auth: bool,
    pub names: &'a [u8],
}

pub struct TrustAnchor<'a> {
    pub subject: &'a [u8],
    pub spki: &'a [u8],
}

const OID_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
const OID_RSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b];
const OID_RSA_SHA384: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0c];
const OID_RSA_SHA512: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0d];
const OID_EC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const OID_P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const OID_P384: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x22];
const OID_ECDSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
const OID_ECDSA_SHA384: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03];
const OID_ECDSA_SHA512: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x04];
const OID_ED25519: &[u8] = &[0x2b, 0x65, 0x70];

const OID_BASIC_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x13];
const OID_KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x0f];
const OID_ALT_NAME: &[u8] = &[0x55, 0x1d, 0x11];
const OID_NAME_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x1e];
const OID_EXT_KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x25];
const OID_SERVER_AUTH: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x01];
const OID_ANY_EKU: &[u8] = &[0x55, 0x1d, 0x25, 0x00];

/// Extensions that may appear, critical or not, without changing the answer
/// this validator gives.
const HARMLESS_EXTENSIONS: &[&[u8]] = &[
    &[0x55, 0x1d, 0x0e],
    &[0x55, 0x1d, 0x23],
    &[0x55, 0x1d, 0x1f],
    &[0x55, 0x1d, 0x20],
    &[0x55, 0x1d, 0x21],
    &[0x55, 0x1d, 0x24],
    &[0x55, 0x1d, 0x36],
    &[0x55, 0x1d, 0x12],
    &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x01, 0x01],
    &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x01, 0x0b],
    &[0x2b, 0x06, 0x01, 0x04, 0x01, 0xd6, 0x79, 0x02, 0x04, 0x02],
];

fn algorithm_from(oid: &[u8]) -> Option<SignatureAlgorithm> {
    Some(match oid {
        x if x == OID_RSA_SHA256 => SignatureAlgorithm::RsaPkcs1(HashAlg::Sha256),
        x if x == OID_RSA_SHA384 => SignatureAlgorithm::RsaPkcs1(HashAlg::Sha384),
        x if x == OID_RSA_SHA512 => SignatureAlgorithm::RsaPkcs1(HashAlg::Sha512),
        x if x == OID_ECDSA_SHA256 => SignatureAlgorithm::Ecdsa(HashAlg::Sha256),
        x if x == OID_ECDSA_SHA384 => SignatureAlgorithm::Ecdsa(HashAlg::Sha384),
        x if x == OID_ECDSA_SHA512 => SignatureAlgorithm::Ecdsa(HashAlg::Sha512),
        x if x == OID_ED25519 => SignatureAlgorithm::Ed25519,
        _ => return None,
    })
}

pub fn parse_spki(spki: &[u8]) -> Option<PublicKey<'_>> {
    let mut outer = Reader::new(spki);
    let body = outer.expect(der::SEQUENCE)?;
    let mut fields = Reader::new(body);
    let algorithm = fields.expect(der::SEQUENCE)?;
    let bits = fields.expect(der::BIT_STRING)?;
    if !fields.is_empty() || !outer.is_empty() || bits.first() != Some(&0) {
        return None;
    }
    let key = &bits[1..];
    let mut parts = Reader::new(algorithm);
    let oid = parts.expect(der::OID)?;
    if oid == OID_RSA {
        let mut outer = Reader::new(key);
        let body = outer.expect(der::SEQUENCE)?;
        let mut numbers = Reader::new(body);
        let modulus = der::unsigned_integer(numbers.expect(der::INTEGER)?)?;
        let exponent = der::unsigned_integer(numbers.expect(der::INTEGER)?)?;
        Some(PublicKey::Rsa { modulus, exponent })
    } else if oid == OID_EC_KEY {
        let curve_oid = parts.expect(der::OID)?;
        let curve = if curve_oid == OID_P256 {
            Curve::P256
        } else if curve_oid == OID_P384 {
            Curve::P384
        } else {
            return None;
        };
        Some(PublicKey::Ec { curve, point: key })
    } else if oid == OID_ED25519 && key.len() == 32 {
        Some(PublicKey::Ed25519(key))
    } else {
        None
    }
}

/// Seconds since 1970 for a calendar time, or `None` if it is not a date.
fn unix_time(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64) -> Option<u64> {
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
        || year < 1970
    {
        return None;
    }
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let year_of_era = shifted - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some((days * 86_400 + hour * 3600 + minute * 60 + second) as u64)
}

fn parse_time(element: der::Tlv<'_>) -> Option<u64> {
    let text = element.value;
    let digits = |from: usize, count: usize| -> Option<i64> {
        let mut value = 0i64;
        for byte in text.get(from..from + count)? {
            if !byte.is_ascii_digit() {
                return None;
            }
            value = value * 10 + i64::from(byte - b'0');
        }
        Some(value)
    };
    match (element.tag, text.len()) {
        (der::UTC_TIME, 13) if text[12] == b'Z' => {
            let two = digits(0, 2)?;
            let year = if two >= 50 { 1900 + two } else { 2000 + two };
            unix_time(year, digits(2, 2)?, digits(4, 2)?, digits(6, 2)?, digits(8, 2)?, digits(10, 2)?)
        }
        (der::GENERALIZED_TIME, 15) if text[14] == b'Z' => unix_time(
            digits(0, 4)?,
            digits(4, 2)?,
            digits(6, 2)?,
            digits(8, 2)?,
            digits(10, 2)?,
            digits(12, 2)?,
        ),
        _ => None,
    }
}

impl<'a> Certificate<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, Error> {
        Self::parse_inner(data).ok_or(Error::Parse)?
    }

    fn parse_inner(data: &'a [u8]) -> Option<Result<Self, Error>> {
        let mut top = Reader::new(data);
        let body = top.expect(der::SEQUENCE)?;
        if !top.is_empty() {
            return None;
        }
        let mut outer = Reader::new(body);
        let tbs = outer.expect_tlv(der::SEQUENCE)?;
        let outer_algorithm = outer.expect(der::SEQUENCE)?;
        let bits = outer.expect(der::BIT_STRING)?;
        if !outer.is_empty() || bits.first() != Some(&0) {
            return None;
        }

        let mut fields = Reader::new(tbs.value);
        if fields.peek_tag() == Some(0xa0) {
            fields.read()?;
        }
        fields.expect(der::INTEGER)?;
        let inner_algorithm = fields.expect(der::SEQUENCE)?;
        if inner_algorithm != outer_algorithm {
            return None;
        }
        let issuer = fields.expect_tlv(der::SEQUENCE)?.raw;
        let mut validity = Reader::new(fields.expect(der::SEQUENCE)?);
        let not_before = parse_time(validity.read()?)?;
        let not_after = parse_time(validity.read()?)?;
        let subject = fields.expect_tlv(der::SEQUENCE)?.raw;
        let spki = fields.expect_tlv(der::SEQUENCE)?.raw;
        let key = parse_spki(spki);

        let mut is_ca = false;
        let mut path_length = None;
        let mut key_usage = None;
        let mut server_auth = true;
        let mut names: &[u8] = &[];
        let mut critical_problem = None;
        while !fields.is_empty() {
            let element = fields.read()?;
            if element.tag != 0xa3 {
                continue;
            }
            let mut wrapper = Reader::new(element.value);
            let mut list = Reader::new(wrapper.expect(der::SEQUENCE)?);
            while !list.is_empty() {
                let mut extension = Reader::new(list.expect(der::SEQUENCE)?);
                let oid = extension.expect(der::OID)?;
                let critical = extension.expect(der::BOOLEAN).is_some_and(|v| v != [0]);
                let value = extension.expect(der::OCTET_STRING)?;
                if oid == OID_BASIC_CONSTRAINTS {
                    let mut body = Reader::new(Reader::new(value).expect(der::SEQUENCE)?);
                    is_ca = body.expect(der::BOOLEAN).is_some_and(|v| v != [0]);
                    path_length = body
                        .expect(der::INTEGER)
                        .and_then(der::unsigned_integer)
                        .filter(|bytes| bytes.len() <= 2)
                        .map(|bytes| bytes.iter().fold(0u32, |sum, b| (sum << 8) | u32::from(*b)));
                } else if oid == OID_KEY_USAGE {
                    let bits = Reader::new(value).expect(der::BIT_STRING)?;
                    key_usage = Some(*bits.get(1)?);
                } else if oid == OID_ALT_NAME {
                    names = Reader::new(value).expect(der::SEQUENCE)?;
                } else if oid == OID_EXT_KEY_USAGE {
                    server_auth = false;
                    let mut usages = Reader::new(Reader::new(value).expect(der::SEQUENCE)?);
                    while !usages.is_empty() {
                        let usage = usages.expect(der::OID)?;
                        if usage == OID_SERVER_AUTH || usage == OID_ANY_EKU {
                            server_auth = true;
                        }
                    }
                } else if oid == OID_NAME_CONSTRAINTS
                    || (critical && !HARMLESS_EXTENSIONS.contains(&oid))
                {
                    critical_problem = Some(Error::CriticalExtension);
                }
            }
        }
        if let Some(problem) = critical_problem {
            return Some(Err(problem));
        }
        let mut algorithm_reader = Reader::new(outer_algorithm);
        let Some(algorithm) = algorithm_from(algorithm_reader.expect(der::OID)?) else {
            return Some(Err(Error::UnsupportedAlgorithm));
        };
        let Some(key) = key else {
            return Some(Err(Error::UnsupportedAlgorithm));
        };
        Some(Ok(Self {
            tbs: tbs.raw,
            issuer,
            subject,
            not_before,
            not_after,
            key,
            spki,
            algorithm,
            signature: &bits[1..],
            is_ca,
            path_length,
            key_usage,
            server_auth,
            names,
        }))
    }

    pub fn check_time(&self, now: u64) -> Result<(), Error> {
        if now < self.not_before {
            Err(Error::NotYetValid)
        } else if now > self.not_after {
            Err(Error::Expired)
        } else {
            Ok(())
        }
    }

    /// The certificate may sign other certificates.
    fn is_signing_ca(&self) -> bool {
        self.is_ca && self.key_usage.is_none_or(|usage| usage & 0x04 != 0)
    }

    pub fn matches_host(&self, host: &str) -> bool {
        let address = parse_ipv4(host);
        let mut list = Reader::new(self.names);
        while let Some(name) = list.read() {
            match (name.tag, address) {
                (0x87, Some(octets)) if name.value == octets => return true,
                (0x82, None) if dns_name_matches(name.value, host.as_bytes()) => return true,
                _ => {}
            }
        }
        false
    }
}

fn parse_ipv4(host: &str) -> Option<[u8; 4]> {
    let mut octets = [0u8; 4];
    let mut count = 0;
    for part in host.split('.') {
        if count == 4 || part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        octets[count] = part.parse().ok()?;
        count += 1;
    }
    (count == 4).then_some(octets)
}

fn dns_name_matches(pattern: &[u8], host: &[u8]) -> bool {
    if host.is_empty() || pattern.is_empty() {
        return false;
    }
    let host = host.strip_suffix(b".").unwrap_or(host);
    if let Some(rest) = pattern.strip_prefix(b"*.") {
        if rest.iter().filter(|b| **b == b'.').count() < 1 || rest.contains(&b'*') {
            return false;
        }
        let Some(dot) = host.iter().position(|b| *b == b'.') else {
            return false;
        };
        return dot > 0 && host[dot + 1..].eq_ignore_ascii_case(rest);
    }
    !pattern.contains(&b'*') && pattern.eq_ignore_ascii_case(host)
}

/// Checks `signature` over `message` with `key`.
pub fn verify_signature(
    key: &PublicKey<'_>,
    algorithm: SignatureAlgorithm,
    message: &[u8],
    signature: &[u8],
) -> bool {
    match (key, algorithm) {
        (PublicKey::Rsa { modulus, exponent }, SignatureAlgorithm::RsaPkcs1(hash)) => {
            rsa::verify_pkcs1(modulus, exponent, hash, message, signature)
        }
        (PublicKey::Rsa { modulus, exponent }, SignatureAlgorithm::RsaPss(hash)) => {
            rsa::verify_pss(modulus, exponent, hash, message, signature)
        }
        (PublicKey::Ec { curve, point }, SignatureAlgorithm::Ecdsa(hash)) => {
            let (digest, length) = hash.digest(message);
            ecdsa::verify(*curve, point, &digest[..length], signature)
        }
        (PublicKey::Ed25519(public), SignatureAlgorithm::Ed25519) => {
            match (<&[u8; 32]>::try_from(*public), <&[u8; 64]>::try_from(signature)) {
                (Ok(public), Ok(signature)) => crate::ed25519::verify(public, message, signature),
                _ => false,
            }
        }
        _ => false,
    }
}

fn anchors_named<'s, 'a: 's>(
    anchors: &'s [TrustAnchor<'a>],
    name: &'s [u8],
) -> impl Iterator<Item = &'s TrustAnchor<'a>> + 's {
    anchors.iter().filter(move |anchor| anchor.subject == name)
}

/// Validates a server's chain (leaf first) for `host` at time `now`.
pub fn verify_chain<'a>(
    chain: &[&'a [u8]],
    host: &str,
    now: u64,
    anchors: &[TrustAnchor<'_>],
) -> Result<Certificate<'a>, Error> {
    if chain.is_empty() || chain.len() > MAX_CHAIN {
        return Err(Error::Parse);
    }
    let mut parsed: [Option<Certificate<'a>>; MAX_CHAIN] = [None; MAX_CHAIN];
    for (slot, data) in parsed.iter_mut().zip(chain.iter()) {
        *slot = Some(Certificate::parse(data)?);
    }
    let leaf = parsed[0].ok_or(Error::Parse)?;
    if !leaf.server_auth {
        return Err(Error::Constraint);
    }
    if !leaf.matches_host(host) {
        return Err(Error::HostName);
    }
    for index in 0..chain.len() {
        let current = parsed[index].ok_or(Error::Parse)?;
        current.check_time(now)?;
        for anchor in anchors_named(anchors, current.issuer) {
            let Some(key) = parse_spki(anchor.spki) else {
                continue;
            };
            if verify_signature(&key, current.algorithm, current.tbs, current.signature) {
                return Ok(leaf);
            }
        }
        for anchor in anchors_named(anchors, current.subject) {
            if anchor.spki == current.spki {
                return Ok(leaf);
            }
        }
        let Some(next) = parsed.get(index + 1).copied().flatten() else {
            return Err(Error::Untrusted);
        };
        if next.subject != current.issuer {
            return Err(Error::Untrusted);
        }
        if !next.is_signing_ca() {
            return Err(Error::Constraint);
        }
        if next.path_length.is_some_and(|limit| (index as u32) > limit) {
            return Err(Error::Constraint);
        }
        if !verify_signature(&next.key, current.algorithm, current.tbs, current.signature) {
            return Err(Error::Signature);
        }
    }
    Err(Error::Untrusted)
}
