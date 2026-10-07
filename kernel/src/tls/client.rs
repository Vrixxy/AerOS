//! A TLS 1.3 client (RFC 8446) over any byte transport.
//!
//! One handshake shape is supported: X25519 key exchange, the
//! TLS_CHACHA20_POLY1305_SHA256 and TLS_AES_128_GCM_SHA256 suites, server
//! authentication by certificate chain, no client certificate, no
//! resumption, no 0-RTT. Everything lives in the `Session`, which is plain
//! data, so a kernel can keep a few of them in static memory.

use core::cmp::min;

use super::aead::{self, Suite, TAG_LEN};
use super::ecdsa::Curve;
use super::hash::{
    HASH_LEN, HashAlg, constant_time_eq, derive_secret, expand_label, hkdf_extract, hmac_sha256,
};
use super::x509::{self, PublicKey, SignatureAlgorithm, TrustAnchor};
use super::x25519;
use crate::auth::Sha256;

pub const MAX_RECORD: usize = 16384;
const RECORD_BUFFER: usize = 5 + MAX_RECORD + 256;
const HANDSHAKE_BUFFER: usize = 24 * 1024;
const OUT_PLAIN: usize = 4096;
const OUT_BUFFER: usize = 5 + OUT_PLAIN + 1 + TAG_LEN;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    /// The transport failed or the peer went away mid-record.
    Transport,
    Protocol,
    Decrypt,
    Unsupported,
    Certificate(x509::Error),
    BadSignature,
    BadFinished,
    /// The server sent a fatal alert (its description code).
    Alert(u8),
    TooLarge,
}

pub trait Transport {
    fn send(&mut self, data: &[u8]) -> Result<(), Error>;
    /// Reads at least one byte; `Ok(0)` means the peer closed the stream.
    fn receive(&mut self, buffer: &mut [u8]) -> Result<usize, Error>;
}

pub struct Config<'a> {
    pub host: &'a str,
    /// Seconds since 1970, for certificate validity.
    pub now: u64,
    pub anchors: &'a [TrustAnchor<'a>],
    pub random: fn(&mut [u8]),
}

#[derive(Clone, Copy)]
struct Keys {
    secret: [u8; 32],
    key: [u8; 32],
    iv: [u8; 12],
    sequence: u64,
    active: bool,
}

impl Keys {
    const EMPTY: Self = Self {
        secret: [0; 32],
        key: [0; 32],
        iv: [0; 12],
        sequence: 0,
        active: false,
    };

    fn from_secret(secret: &[u8; 32], suite: Suite) -> Self {
        let mut keys = Self::EMPTY;
        keys.secret = *secret;
        expand_label(secret, b"key", b"", &mut keys.key[..suite.key_len()]);
        expand_label(secret, b"iv", b"", &mut keys.iv);
        keys.active = true;
        keys
    }

    fn nonce(&self) -> [u8; 12] {
        let mut nonce = self.iv;
        for (slot, byte) in nonce[4..].iter_mut().zip(self.sequence.to_be_bytes()) {
            *slot ^= byte;
        }
        nonce
    }
}

pub struct Session {
    suite: Suite,
    read: Keys,
    write: Keys,
    record: [u8; RECORD_BUFFER],
    pending: (usize, usize),
    handshake: [u8; HANDSHAKE_BUFFER],
    handshake_length: usize,
    out: [u8; OUT_BUFFER],
    closed: bool,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

struct Cursor<'a> {
    data: &'a [u8],
}

impl<'a> Cursor<'a> {
    fn bytes(&mut self, count: usize) -> Option<&'a [u8]> {
        if self.data.len() < count {
            return None;
        }
        let (head, rest) = self.data.split_at(count);
        self.data = rest;
        Some(head)
    }

    fn u8(&mut self) -> Option<u8> {
        self.bytes(1).map(|b| b[0])
    }

    fn u16(&mut self) -> Option<u16> {
        self.bytes(2).map(|b| u16::from_be_bytes([b[0], b[1]]))
    }

    fn u24(&mut self) -> Option<usize> {
        self.bytes(3)
            .map(|b| (usize::from(b[0]) << 16) | (usize::from(b[1]) << 8) | usize::from(b[2]))
    }

    fn vec8(&mut self) -> Option<&'a [u8]> {
        let length = usize::from(self.u8()?);
        self.bytes(length)
    }

    fn vec16(&mut self) -> Option<&'a [u8]> {
        let length = usize::from(self.u16()?);
        self.bytes(length)
    }

    fn vec24(&mut self) -> Option<&'a [u8]> {
        let length = self.u24()?;
        self.bytes(length)
    }

    fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

struct Writer<'a> {
    buffer: &'a mut [u8],
    position: usize,
}

impl<'a> Writer<'a> {
    fn put(&mut self, bytes: &[u8]) {
        self.buffer[self.position..self.position + bytes.len()].copy_from_slice(bytes);
        self.position += bytes.len();
    }

    fn put_u16(&mut self, value: u16) {
        self.put(&value.to_be_bytes());
    }

    /// Reserves a two-byte length; `finish_u16` fills it in.
    fn start_u16(&mut self) -> usize {
        self.position += 2;
        self.position
    }

    fn finish_u16(&mut self, start: usize) {
        let length = (self.position - start) as u16;
        self.buffer[start - 2..start].copy_from_slice(&length.to_be_bytes());
    }
}

const HELLO_RETRY_MAGIC: [u8; 32] = [
    0xcf, 0x21, 0xad, 0x74, 0xe5, 0x9a, 0x61, 0x11, 0xbe, 0x1d, 0x8c, 0x02, 0x1e, 0x65, 0xb8, 0x91,
    0xc2, 0xa2, 0x11, 0x16, 0x7a, 0xbb, 0x8c, 0x5e, 0x07, 0x9e, 0x09, 0xe2, 0xc8, 0xa8, 0x33, 0x9c,
];

const TYPE_CHANGE_CIPHER_SPEC: u8 = 20;
const TYPE_ALERT: u8 = 21;
const TYPE_HANDSHAKE: u8 = 22;
const TYPE_APPLICATION: u8 = 23;

const HS_CLIENT_HELLO: u8 = 1;
const HS_SERVER_HELLO: u8 = 2;
const HS_NEW_SESSION_TICKET: u8 = 4;
const HS_ENCRYPTED_EXTENSIONS: u8 = 8;
const HS_CERTIFICATE: u8 = 11;
const HS_CERTIFICATE_REQUEST: u8 = 13;
const HS_CERTIFICATE_VERIFY: u8 = 15;
const HS_FINISHED: u8 = 20;
const HS_KEY_UPDATE: u8 = 24;

// Only one lives at a time, on the stack, so the size gap costs nothing.
#[allow(clippy::large_enum_variant)]
enum OwnedKey {
    Rsa {
        modulus: [u8; 512],
        modulus_length: usize,
        exponent: [u8; 8],
        exponent_length: usize,
    },
    Ec {
        curve: Curve,
        point: [u8; 97],
        point_length: usize,
    },
    Ed25519([u8; 32]),
}

impl OwnedKey {
    fn copy_of(key: &PublicKey<'_>) -> Option<Self> {
        Some(match *key {
            PublicKey::Rsa { modulus, exponent } => {
                let mut copy = Self::Rsa {
                    modulus: [0; 512],
                    modulus_length: modulus.len(),
                    exponent: [0; 8],
                    exponent_length: exponent.len(),
                };
                if let Self::Rsa {
                    modulus: m,
                    exponent: e,
                    ..
                } = &mut copy
                {
                    m.get_mut(..modulus.len())?.copy_from_slice(modulus);
                    e.get_mut(..exponent.len())?.copy_from_slice(exponent);
                }
                copy
            }
            PublicKey::Ec { curve, point } => {
                let mut stored = [0u8; 97];
                stored.get_mut(..point.len())?.copy_from_slice(point);
                Self::Ec {
                    curve,
                    point: stored,
                    point_length: point.len(),
                }
            }
            PublicKey::Ed25519(public) => {
                let mut stored = [0u8; 32];
                stored.copy_from_slice(public);
                Self::Ed25519(stored)
            }
        })
    }

    fn view(&self) -> PublicKey<'_> {
        match self {
            Self::Rsa {
                modulus,
                modulus_length,
                exponent,
                exponent_length,
            } => PublicKey::Rsa {
                modulus: &modulus[..*modulus_length],
                exponent: &exponent[..*exponent_length],
            },
            Self::Ec {
                curve,
                point,
                point_length,
            } => PublicKey::Ec {
                curve: *curve,
                point: &point[..*point_length],
            },
            Self::Ed25519(public) => PublicKey::Ed25519(public),
        }
    }
}

fn scheme_algorithm(scheme: u16, key: &PublicKey<'_>) -> Option<SignatureAlgorithm> {
    match (scheme, key) {
        (0x0804, PublicKey::Rsa { .. }) => Some(SignatureAlgorithm::RsaPss(HashAlg::Sha256)),
        (0x0805, PublicKey::Rsa { .. }) => Some(SignatureAlgorithm::RsaPss(HashAlg::Sha384)),
        (0x0806, PublicKey::Rsa { .. }) => Some(SignatureAlgorithm::RsaPss(HashAlg::Sha512)),
        (
            0x0403,
            PublicKey::Ec {
                curve: Curve::P256, ..
            },
        ) => Some(SignatureAlgorithm::Ecdsa(HashAlg::Sha256)),
        (
            0x0503,
            PublicKey::Ec {
                curve: Curve::P384, ..
            },
        ) => Some(SignatureAlgorithm::Ecdsa(HashAlg::Sha384)),
        (0x0807, PublicKey::Ed25519(_)) => Some(SignatureAlgorithm::Ed25519),
        _ => None,
    }
}

fn read_exact<T: Transport>(transport: &mut T, mut buffer: &mut [u8]) -> Result<(), Error> {
    while !buffer.is_empty() {
        let count = transport.receive(buffer)?;
        if count == 0 {
            return Err(Error::Transport);
        }
        buffer = &mut buffer[count..];
    }
    Ok(())
}

fn is_ip_literal(host: &str) -> bool {
    host.bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'.')
        || host.contains(':')
}

impl Session {
    pub const fn new() -> Self {
        Self {
            suite: Suite::ChaCha20Poly1305,
            read: Keys::EMPTY,
            write: Keys::EMPTY,
            record: [0; RECORD_BUFFER],
            pending: (0, 0),
            handshake: [0; HANDSHAKE_BUFFER],
            handshake_length: 0,
            out: [0; OUT_BUFFER],
            closed: false,
        }
    }

    #[cfg(test)]
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    fn reset(&mut self) {
        self.read = Keys::EMPTY;
        self.write = Keys::EMPTY;
        self.pending = (0, 0);
        self.handshake_length = 0;
        self.closed = false;
    }

    // -------------------------------------------------------- record layer

    /// The next record's content type and where its payload sits in
    /// `self.record`. Change-cipher-spec records are skipped.
    fn read_record<T: Transport>(
        &mut self,
        transport: &mut T,
    ) -> Result<(u8, usize, usize), Error> {
        loop {
            read_exact(transport, &mut self.record[..5])?;
            let outer = self.record[0];
            let length = usize::from(u16::from_be_bytes([self.record[3], self.record[4]]));
            if self.record[1] != 3 || length > MAX_RECORD + 256 {
                return Err(Error::Protocol);
            }
            read_exact(transport, &mut self.record[5..5 + length])?;
            if outer == TYPE_CHANGE_CIPHER_SPEC {
                if length != 1 || self.record[5] != 1 {
                    return Err(Error::Protocol);
                }
                continue;
            }
            if !self.read.active {
                if length > MAX_RECORD {
                    return Err(Error::Protocol);
                }
                return Ok((outer, 5, 5 + length));
            }
            if outer != TYPE_APPLICATION || length < TAG_LEN + 1 {
                return Err(Error::Protocol);
            }
            let end = 5 + length;
            let header = [
                self.record[0],
                self.record[1],
                self.record[2],
                self.record[3],
                self.record[4],
            ];
            let mut tag = [0u8; TAG_LEN];
            tag.copy_from_slice(&self.record[end - TAG_LEN..end]);
            let nonce = self.read.nonce();
            let opened = aead::open(
                self.suite,
                &self.read.key[..self.suite.key_len()],
                &nonce,
                &header,
                &mut self.record[5..end - TAG_LEN],
                &tag,
            );
            if !opened {
                return Err(Error::Decrypt);
            }
            self.read.sequence += 1;
            let mut stop = end - TAG_LEN;
            while stop > 5 && self.record[stop - 1] == 0 {
                stop -= 1;
            }
            if stop == 5 {
                return Err(Error::Protocol);
            }
            let inner = self.record[stop - 1];
            return Ok((inner, 5, stop - 1));
        }
    }

    fn write_record<T: Transport>(
        &mut self,
        transport: &mut T,
        content_type: u8,
        data: &[u8],
    ) -> Result<(), Error> {
        if data.len() > OUT_PLAIN {
            return Err(Error::TooLarge);
        }
        let total = if self.write.active {
            let length = data.len() + 1 + TAG_LEN;
            let header = [TYPE_APPLICATION, 3, 3, (length >> 8) as u8, length as u8];
            self.out[..5].copy_from_slice(&header);
            self.out[5..5 + data.len()].copy_from_slice(data);
            self.out[5 + data.len()] = content_type;
            let nonce = self.write.nonce();
            let tag = aead::seal(
                self.suite,
                &self.write.key[..self.suite.key_len()],
                &nonce,
                &header,
                &mut self.out[5..5 + data.len() + 1],
            );
            self.out[5 + data.len() + 1..5 + length].copy_from_slice(&tag);
            self.write.sequence += 1;
            5 + length
        } else {
            self.out[..5].copy_from_slice(&[
                content_type,
                3,
                3,
                (data.len() >> 8) as u8,
                data.len() as u8,
            ]);
            self.out[5..5 + data.len()].copy_from_slice(data);
            5 + data.len()
        };
        transport.send(&self.out[..total])
    }

    /// The next whole handshake message sits at `handshake[..total]`;
    /// returns its type and `total`.
    fn next_handshake<T: Transport>(&mut self, transport: &mut T) -> Result<(u8, usize), Error> {
        loop {
            if self.handshake_length >= 4 {
                let body = (usize::from(self.handshake[1]) << 16)
                    | (usize::from(self.handshake[2]) << 8)
                    | usize::from(self.handshake[3]);
                let total = 4 + body;
                if total > HANDSHAKE_BUFFER {
                    return Err(Error::TooLarge);
                }
                if self.handshake_length >= total {
                    return Ok((self.handshake[0], total));
                }
            }
            let (kind, start, end) = self.read_record(transport)?;
            match kind {
                TYPE_HANDSHAKE => {
                    let count = end - start;
                    if self.handshake_length + count > HANDSHAKE_BUFFER {
                        return Err(Error::TooLarge);
                    }
                    self.handshake[self.handshake_length..self.handshake_length + count]
                        .copy_from_slice(&self.record[start..end]);
                    self.handshake_length += count;
                }
                TYPE_ALERT => return Err(self.alert_error(start, end)),
                _ => return Err(Error::Protocol),
            }
        }
    }

    fn consume_handshake(&mut self, total: usize) {
        self.handshake.copy_within(total..self.handshake_length, 0);
        self.handshake_length -= total;
    }

    fn alert_error(&self, start: usize, end: usize) -> Error {
        if end - start >= 2 {
            Error::Alert(self.record[start + 1])
        } else {
            Error::Protocol
        }
    }

    // ----------------------------------------------------------- handshake

    fn build_client_hello(
        &mut self,
        config: &Config<'_>,
        public: &[u8; 32],
        random: &[u8; 32],
        session_id: &[u8; 32],
    ) -> usize {
        let mut writer = Writer {
            buffer: &mut self.out,
            position: 0,
        };
        writer.put(&[HS_CLIENT_HELLO, 0, 0, 0]);
        writer.put(&[3, 3]);
        writer.put(random);
        writer.put(&[32]);
        writer.put(session_id);
        writer.put(&[0, 4, 0x13, 0x03, 0x13, 0x01]);
        writer.put(&[1, 0]);
        let extensions = writer.start_u16();

        if !is_ip_literal(config.host) {
            writer.put_u16(0);
            let body = writer.start_u16();
            let list = writer.start_u16();
            writer.put(&[0]);
            writer.put_u16(config.host.len() as u16);
            writer.put(config.host.as_bytes());
            writer.finish_u16(list);
            writer.finish_u16(body);
        }
        writer.put(&[0, 10, 0, 4, 0, 2, 0, 0x1d]);
        writer.put_u16(13);
        let body = writer.start_u16();
        writer.put(&[0, 12, 4, 3, 5, 3, 8, 4, 8, 5, 8, 6, 8, 7]);
        writer.finish_u16(body);
        writer.put_u16(0x32);
        let body = writer.start_u16();
        writer.put(&[0, 18, 4, 3, 5, 3, 8, 4, 8, 5, 8, 6, 8, 7, 4, 1, 5, 1, 6, 1]);
        writer.finish_u16(body);
        writer.put(&[0, 43, 0, 3, 2, 3, 4]);
        writer.put_u16(51);
        let body = writer.start_u16();
        let shares = writer.start_u16();
        writer.put(&[0, 0x1d, 0, 32]);
        writer.put(public);
        writer.finish_u16(shares);
        writer.finish_u16(body);
        writer.finish_u16(extensions);

        let length = writer.position;
        let body = length - 4;
        self.out[1] = (body >> 16) as u8;
        self.out[2] = (body >> 8) as u8;
        self.out[3] = body as u8;
        length
    }

    pub fn connect<T: Transport>(
        &mut self,
        transport: &mut T,
        config: &Config<'_>,
    ) -> Result<(), Error> {
        self.reset();
        if config.host.is_empty() || config.host.len() > 253 {
            return Err(Error::Unsupported);
        }
        let mut secret = [0u8; 32];
        let mut random = [0u8; 32];
        let mut session_id = [0u8; 32];
        (config.random)(&mut secret);
        (config.random)(&mut random);
        (config.random)(&mut session_id);
        let public = x25519::public_key(&secret);

        let hello_length = self.build_client_hello(config, &public, &random, &session_id);
        let mut transcript = Sha256::new();
        let mut hello = [0u8; 512];
        hello
            .get_mut(..hello_length)
            .ok_or(Error::TooLarge)?
            .copy_from_slice(&self.out[..hello_length]);
        transcript.update(&hello[..hello_length]);
        self.write_record(transport, TYPE_HANDSHAKE, &hello[..hello_length])?;

        let (kind, total) = self.next_handshake(transport)?;
        if kind != HS_SERVER_HELLO {
            return Err(Error::Protocol);
        }
        transcript.update(&self.handshake[..total]);
        let (suite, server_share) = parse_server_hello(&self.handshake[4..total], &session_id)?;
        self.consume_handshake(total);
        self.suite = suite;
        let shared = x25519::shared_secret(&secret, &server_share).ok_or(Error::Protocol)?;

        let empty_hash = crate::auth::sha256(b"");
        let early = hkdf_extract(&[0; 32], &[0; 32]);
        let derived = derive_secret(&early, b"derived", &empty_hash);
        let handshake_secret = hkdf_extract(&derived, &shared);
        let hello_hash = transcript.clone().finish();
        let client_secret = derive_secret(&handshake_secret, b"c hs traffic", &hello_hash);
        let server_secret = derive_secret(&handshake_secret, b"s hs traffic", &hello_hash);
        self.read = Keys::from_secret(&server_secret, suite);
        let client_handshake = Keys::from_secret(&client_secret, suite);

        // EncryptedExtensions
        let (kind, total) = self.next_handshake(transport)?;
        if kind != HS_ENCRYPTED_EXTENSIONS {
            return Err(Error::Protocol);
        }
        transcript.update(&self.handshake[..total]);
        self.consume_handshake(total);

        let (mut kind, mut total) = self.next_handshake(transport)?;
        let mut client_certificate_requested = false;
        if kind == HS_CERTIFICATE_REQUEST {
            client_certificate_requested = true;
            transcript.update(&self.handshake[..total]);
            self.consume_handshake(total);
            (kind, total) = self.next_handshake(transport)?;
        }
        if kind != HS_CERTIFICATE {
            return Err(Error::Protocol);
        }
        transcript.update(&self.handshake[..total]);
        let leaf = verify_certificate_message(&self.handshake[4..total], config)?;
        self.consume_handshake(total);
        let certificate_hash = transcript.clone().finish();

        let (kind, total) = self.next_handshake(transport)?;
        if kind != HS_CERTIFICATE_VERIFY {
            return Err(Error::Protocol);
        }
        verify_certificate_verify(&self.handshake[4..total], &leaf, &certificate_hash)?;
        transcript.update(&self.handshake[..total]);
        self.consume_handshake(total);
        let verify_hash = transcript.clone().finish();

        let (kind, total) = self.next_handshake(transport)?;
        if kind != HS_FINISHED || total != 4 + HASH_LEN {
            return Err(Error::Protocol);
        }
        let mut finished_key = [0u8; 32];
        expand_label(&server_secret, b"finished", b"", &mut finished_key);
        let expected = hmac_sha256(&finished_key, &[&verify_hash]);
        if !constant_time_eq(&expected, &self.handshake[4..total]) {
            return Err(Error::BadFinished);
        }
        transcript.update(&self.handshake[..total]);
        self.consume_handshake(total);
        if self.handshake_length != 0 {
            return Err(Error::Protocol);
        }
        let finished_hash = transcript.clone().finish();

        // Our answer goes out under the handshake keys.
        self.write_record(transport, TYPE_CHANGE_CIPHER_SPEC, &[1])?;
        self.write = client_handshake;
        if client_certificate_requested {
            self.write_record(
                transport,
                TYPE_HANDSHAKE,
                &[HS_CERTIFICATE, 0, 0, 4, 0, 0, 0, 0],
            )?;
        }
        let mut client_finished_key = [0u8; 32];
        expand_label(&client_secret, b"finished", b"", &mut client_finished_key);
        let mut message = [0u8; 4 + HASH_LEN];
        message[..4].copy_from_slice(&[HS_FINISHED, 0, 0, HASH_LEN as u8]);
        message[4..].copy_from_slice(&hmac_sha256(&client_finished_key, &[&finished_hash]));
        self.write_record(transport, TYPE_HANDSHAKE, &message)?;

        let derived = derive_secret(&handshake_secret, b"derived", &empty_hash);
        let master = hkdf_extract(&derived, &[0; 32]);
        let client_application = derive_secret(&master, b"c ap traffic", &finished_hash);
        let server_application = derive_secret(&master, b"s ap traffic", &finished_hash);
        self.write = Keys::from_secret(&client_application, suite);
        self.read = Keys::from_secret(&server_application, suite);
        Ok(())
    }

    // -------------------------------------------------------- application

    pub fn write<T: Transport>(&mut self, transport: &mut T, mut data: &[u8]) -> Result<(), Error> {
        if self.closed {
            return Err(Error::Transport);
        }
        while !data.is_empty() {
            let count = min(data.len(), OUT_PLAIN);
            self.write_record(transport, TYPE_APPLICATION, &data[..count])?;
            data = &data[count..];
        }
        Ok(())
    }

    /// Reads application data; `Ok(0)` after the server's close_notify.
    pub fn read<T: Transport>(
        &mut self,
        transport: &mut T,
        out: &mut [u8],
    ) -> Result<usize, Error> {
        loop {
            if self.pending.0 < self.pending.1 {
                let count = min(out.len(), self.pending.1 - self.pending.0);
                out[..count].copy_from_slice(&self.record[self.pending.0..self.pending.0 + count]);
                self.pending.0 += count;
                return Ok(count);
            }
            if self.closed {
                return Ok(0);
            }
            let (kind, start, end) = match self.read_record(transport) {
                Ok(record) => record,
                Err(Error::Transport) => {
                    self.closed = true;
                    return Err(Error::Transport);
                }
                Err(other) => return Err(other),
            };
            match kind {
                TYPE_APPLICATION => self.pending = (start, end),
                TYPE_ALERT => {
                    if end - start == 2 && self.record[start + 1] == 0 {
                        self.closed = true;
                        return Ok(0);
                    }
                    return Err(self.alert_error(start, end));
                }
                TYPE_HANDSHAKE => {
                    let count = end - start;
                    if self.handshake_length + count > HANDSHAKE_BUFFER {
                        return Err(Error::TooLarge);
                    }
                    self.handshake[self.handshake_length..self.handshake_length + count]
                        .copy_from_slice(&self.record[start..end]);
                    self.handshake_length += count;
                    self.post_handshake(transport)?;
                }
                _ => return Err(Error::Protocol),
            }
        }
    }

    fn post_handshake<T: Transport>(&mut self, transport: &mut T) -> Result<(), Error> {
        while self.handshake_length >= 4 {
            let body = (usize::from(self.handshake[1]) << 16)
                | (usize::from(self.handshake[2]) << 8)
                | usize::from(self.handshake[3]);
            let total = 4 + body;
            if total > HANDSHAKE_BUFFER {
                return Err(Error::TooLarge);
            }
            if self.handshake_length < total {
                break;
            }
            match self.handshake[0] {
                HS_NEW_SESSION_TICKET => {}
                HS_KEY_UPDATE => {
                    if total != 5 || self.handshake[4] > 1 {
                        return Err(Error::Protocol);
                    }
                    let request = self.handshake[4] == 1;
                    self.read = next_keys(&self.read, self.suite);
                    if request {
                        self.write_record(transport, TYPE_HANDSHAKE, &[HS_KEY_UPDATE, 0, 0, 1, 0])?;
                        self.write = next_keys(&self.write, self.suite);
                    }
                }
                _ => return Err(Error::Protocol),
            }
            self.consume_handshake(total);
        }
        Ok(())
    }

    /// Sends close_notify. The transport is left to the caller.
    pub fn close<T: Transport>(&mut self, transport: &mut T) {
        if !self.closed && self.write.active {
            let _ = self.write_record(transport, TYPE_ALERT, &[1, 0]);
        }
        self.closed = true;
    }
}

fn next_keys(keys: &Keys, suite: Suite) -> Keys {
    let mut secret = [0u8; 32];
    expand_label(&keys.secret, b"traffic upd", b"", &mut secret);
    Keys::from_secret(&secret, suite)
}

fn parse_server_hello(body: &[u8], session_id: &[u8; 32]) -> Result<(Suite, [u8; 32]), Error> {
    let mut cursor = Cursor { data: body };
    let protocol = Error::Protocol;
    if cursor.u16().ok_or(protocol)? != 0x0303 {
        return Err(Error::Protocol);
    }
    let random = cursor.bytes(32).ok_or(protocol)?;
    if random == HELLO_RETRY_MAGIC {
        return Err(Error::Unsupported);
    }
    if cursor.vec8().ok_or(protocol)? != session_id {
        return Err(Error::Protocol);
    }
    let suite = match cursor.u16().ok_or(protocol)? {
        0x1301 => Suite::Aes128Gcm,
        0x1303 => Suite::ChaCha20Poly1305,
        _ => return Err(Error::Unsupported),
    };
    if cursor.u8().ok_or(protocol)? != 0 {
        return Err(Error::Protocol);
    }
    let mut extensions = Cursor {
        data: cursor.vec16().ok_or(protocol)?,
    };
    if !cursor.is_empty() {
        return Err(Error::Protocol);
    }
    let mut version = false;
    let mut share = None;
    while !extensions.is_empty() {
        let kind = extensions.u16().ok_or(protocol)?;
        let data = extensions.vec16().ok_or(protocol)?;
        match kind {
            43 => {
                if data != [3, 4] {
                    return Err(Error::Unsupported);
                }
                version = true;
            }
            51 => {
                let mut entry = Cursor { data };
                if entry.u16().ok_or(protocol)? != 0x001d {
                    return Err(Error::Unsupported);
                }
                let key = entry.vec16().ok_or(protocol)?;
                let mut stored = [0u8; 32];
                if key.len() != 32 {
                    return Err(Error::Protocol);
                }
                stored.copy_from_slice(key);
                share = Some(stored);
            }
            _ => return Err(Error::Protocol),
        }
    }
    match (version, share) {
        (true, Some(share)) => Ok((suite, share)),
        _ => Err(Error::Protocol),
    }
}

/// Validates the Certificate message and returns a copy of the leaf's key.
fn verify_certificate_message(body: &[u8], config: &Config<'_>) -> Result<OwnedKey, Error> {
    let protocol = Error::Protocol;
    let mut cursor = Cursor { data: body };
    if !cursor.vec8().ok_or(protocol)?.is_empty() {
        return Err(Error::Protocol);
    }
    let mut list = Cursor {
        data: cursor.vec24().ok_or(protocol)?,
    };
    if !cursor.is_empty() {
        return Err(Error::Protocol);
    }
    let mut chain: [&[u8]; x509::MAX_CHAIN] = [&[]; x509::MAX_CHAIN];
    let mut count = 0;
    while !list.is_empty() {
        let certificate = list.vec24().ok_or(protocol)?;
        list.vec16().ok_or(protocol)?;
        if count == x509::MAX_CHAIN {
            return Err(Error::Certificate(x509::Error::Parse));
        }
        chain[count] = certificate;
        count += 1;
    }
    let leaf = x509::verify_chain(&chain[..count], config.host, config.now, config.anchors)
        .map_err(Error::Certificate)?;
    OwnedKey::copy_of(&leaf.key).ok_or(Error::Certificate(x509::Error::UnsupportedAlgorithm))
}

fn verify_certificate_verify(
    body: &[u8],
    leaf: &OwnedKey,
    transcript_hash: &[u8; 32],
) -> Result<(), Error> {
    let protocol = Error::Protocol;
    let mut cursor = Cursor { data: body };
    let scheme = cursor.u16().ok_or(protocol)?;
    let signature = cursor.vec16().ok_or(protocol)?;
    if !cursor.is_empty() {
        return Err(Error::Protocol);
    }
    let key = leaf.view();
    let algorithm = scheme_algorithm(scheme, &key).ok_or(Error::Unsupported)?;
    let mut content = [0x20u8; 64 + 33 + 1 + 32];
    content[64..97].copy_from_slice(b"TLS 1.3, server CertificateVerify");
    content[97] = 0;
    content[98..].copy_from_slice(transcript_hash);
    if x509::verify_signature(&key, algorithm, &content, signature) {
        Ok(())
    } else {
        Err(Error::BadSignature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Wire {
        inbound: Vec<u8>,
        outbound: Vec<u8>,
    }

    impl Transport for Wire {
        fn send(&mut self, data: &[u8]) -> Result<(), Error> {
            self.outbound.extend_from_slice(data);
            Ok(())
        }

        fn receive(&mut self, buffer: &mut [u8]) -> Result<usize, Error> {
            let count = buffer.len().min(self.inbound.len());
            buffer[..count].copy_from_slice(&self.inbound[..count]);
            self.inbound.drain(..count);
            Ok(count)
        }
    }

    fn pair(suite: Suite) -> (Box<Session>, Box<Session>) {
        let client_secret = [0x11; 32];
        let server_secret = [0x22; 32];
        let mut client = Box::new(Session::new());
        let mut server = Box::new(Session::new());
        client.suite = suite;
        server.suite = suite;
        client.write = Keys::from_secret(&client_secret, suite);
        client.read = Keys::from_secret(&server_secret, suite);
        server.write = Keys::from_secret(&server_secret, suite);
        server.read = Keys::from_secret(&client_secret, suite);
        (client, server)
    }

    fn deliver(from: &mut Wire, to: &mut Wire) {
        to.inbound.append(&mut from.outbound);
    }

    fn read_all(session: &mut Session, wire: &mut Wire) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buffer = [0u8; 100];
        while !wire.inbound.is_empty() || session.pending.0 < session.pending.1 {
            let count = session.read(wire, &mut buffer).unwrap();
            if count == 0 {
                break;
            }
            out.extend_from_slice(&buffer[..count]);
        }
        out
    }

    #[test]
    fn application_data_both_ways_and_key_update() {
        for suite in [Suite::ChaCha20Poly1305, Suite::Aes128Gcm] {
            let (mut client, mut server) = pair(suite);
            let (mut client_wire, mut server_wire) = (Wire::default(), Wire::default());

            client.write(&mut client_wire, b"hello server").unwrap();
            deliver(&mut client_wire, &mut server_wire);
            assert_eq!(read_all(&mut server, &mut server_wire), b"hello server");

            // The server rotates its keys and asks the client to do the same.
            server
                .write_record(
                    &mut server_wire,
                    TYPE_HANDSHAKE,
                    &[HS_KEY_UPDATE, 0, 0, 1, 1],
                )
                .unwrap();
            server.write = next_keys(&server.write, suite);
            server.write(&mut server_wire, b"after the update").unwrap();
            deliver(&mut server_wire, &mut client_wire);
            assert_eq!(read_all(&mut client, &mut client_wire), b"after the update");

            // The client answered with a KeyUpdate and switched; the server
            // must still be able to read what comes next.
            client.write(&mut client_wire, b"and back").unwrap();
            deliver(&mut client_wire, &mut server_wire);
            assert_eq!(read_all(&mut server, &mut server_wire), b"and back");

            // Large writes are split into records.
            let big: Vec<u8> = (0..10_000u32).map(|i| i as u8).collect();
            server.write(&mut server_wire, &big).unwrap();
            deliver(&mut server_wire, &mut client_wire);
            assert_eq!(read_all(&mut client, &mut client_wire), big);
        }
    }

    #[test]
    fn tampering_and_replay_are_rejected() {
        let (mut client, mut server) = pair(Suite::ChaCha20Poly1305);
        let (mut client_wire, mut server_wire) = (Wire::default(), Wire::default());
        client.write(&mut client_wire, b"secret").unwrap();
        let record = client_wire.outbound.clone();

        let mut bad = record.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        server_wire.inbound = bad;
        let mut buffer = [0u8; 16];
        assert_eq!(
            server.read(&mut server_wire, &mut buffer),
            Err(Error::Decrypt)
        );

        let (_, mut server) = pair(Suite::ChaCha20Poly1305);
        server_wire.inbound = record.clone();
        assert_eq!(server.read(&mut server_wire, &mut buffer), Ok(6));
        // The same record again carries the old sequence number.
        server_wire.inbound = record;
        assert_eq!(
            server.read(&mut server_wire, &mut buffer),
            Err(Error::Decrypt)
        );
    }

    #[test]
    fn close_notify_ends_the_stream() {
        let (mut client, mut server) = pair(Suite::Aes128Gcm);
        let (mut client_wire, mut server_wire) = (Wire::default(), Wire::default());
        client.close(&mut client_wire);
        deliver(&mut client_wire, &mut server_wire);
        let mut buffer = [0u8; 8];
        assert_eq!(server.read(&mut server_wire, &mut buffer), Ok(0));
        assert!(server.is_closed());
    }
}
