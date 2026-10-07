//! HTTPS over the TCP engine: the TLS 1.3 client from `tls` on a socket from
//! `tcpnet`. One session at a time (its buffers are static).

use crate::ip::Address;
use crate::sync::TicketLock;
use crate::tcp::{EAGAIN, State};
use crate::tcpnet;
use crate::tls::client::{Config, Error as TlsError, Session, Transport};
use crate::tls::x509::TrustAnchor;

const CONNECT_NS: u64 = 10_000_000_000;
const IDLE_NS: u64 = 10_000_000_000;

static SESSION: TicketLock<Session> = TicketLock::new(Session::new());

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Failure {
    /// Another HTTPS request is in progress.
    Busy,
    /// The random generator is not seeded; no keys can be made safely.
    NoEntropy,
    BadRequest,
    Connect,
    Tls(TlsError),
}

impl Failure {
    pub fn describe(self) -> &'static str {
        use crate::tls::x509::Error as Cert;
        match self {
            Self::Busy => "another HTTPS request is running",
            Self::NoEntropy => "no random numbers yet",
            Self::BadRequest => "bad host or path",
            Self::Connect => "could not connect",
            Self::Tls(TlsError::Transport) => "connection lost during the TLS handshake",
            Self::Tls(TlsError::Certificate(Cert::HostName)) => {
                "certificate is for a different name"
            }
            Self::Tls(TlsError::Certificate(Cert::Expired)) => "certificate has expired",
            Self::Tls(TlsError::Certificate(Cert::NotYetValid)) => {
                "certificate is not valid yet (is the clock right?)"
            }
            Self::Tls(TlsError::Certificate(Cert::Untrusted)) => {
                "certificate does not lead to a trusted root"
            }
            Self::Tls(TlsError::Certificate(_)) => "certificate was refused",
            Self::Tls(TlsError::BadSignature) => "server's handshake signature is wrong",
            Self::Tls(TlsError::BadFinished) => "handshake integrity check failed",
            Self::Tls(TlsError::Alert(_)) => "server sent a TLS alert",
            Self::Tls(_) => "TLS protocol error",
        }
    }
}

struct Stream {
    index: usize,
}

impl Stream {
    fn connect(address: Address, port: u16) -> Option<Self> {
        let index = tcpnet::with_tcp(|tcp, _, _| tcp.socket())?;
        tcpnet::activate();
        let stream = Self { index };
        if tcpnet::with_tcp(|tcp, sink, now| tcp.connect(index, address, port, now, sink)).is_err() {
            return None;
        }
        let deadline = tcpnet::now().saturating_add(CONNECT_NS);
        loop {
            tcpnet::poll();
            match tcpnet::with_tcp(|tcp, _, _| tcp.state(index)) {
                Some(State::Established) => return Some(stream),
                Some(State::SynSent | State::SynReceived) if tcpnet::now() < deadline => {}
                _ => return None,
            }
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let index = self.index;
        tcpnet::with_tcp(|tcp, sink, now| tcp.close(index, now, sink));
        for _ in 0..4 {
            tcpnet::poll();
        }
    }
}

impl Transport for Stream {
    fn send(&mut self, data: &[u8]) -> Result<(), TlsError> {
        let mut sent = 0;
        let mut deadline = tcpnet::now().saturating_add(IDLE_NS);
        while sent < data.len() {
            if tcpnet::now() >= deadline {
                return Err(TlsError::Transport);
            }
            tcpnet::poll();
            match tcpnet::with_tcp(|tcp, sink, now| tcp.write(self.index, &data[sent..], now, sink))
            {
                Ok(count) => {
                    sent += count;
                    deadline = tcpnet::now().saturating_add(IDLE_NS);
                }
                Err(EAGAIN) => {}
                Err(_) => return Err(TlsError::Transport),
            }
        }
        Ok(())
    }

    fn receive(&mut self, buffer: &mut [u8]) -> Result<usize, TlsError> {
        let deadline = tcpnet::now().saturating_add(IDLE_NS);
        while tcpnet::now() < deadline {
            tcpnet::poll();
            match tcpnet::with_tcp(|tcp, sink, _| tcp.read(self.index, buffer, sink)) {
                Ok(count) => return Ok(count),
                Err(EAGAIN) => {}
                Err(_) => return Err(TlsError::Transport),
            }
        }
        Err(TlsError::Transport)
    }
}

fn fill_random(buffer: &mut [u8]) {
    let _ = crate::random::fill(buffer);
}

pub struct Reply {
    pub status: u16,
    pub bytes: usize,
}

/// One `GET` over TLS. `host` is both the Host header and the name the
/// certificate must match; `address` is where to connect.
pub fn https_get(
    address: Address,
    port: u16,
    host: &str,
    path: &str,
    output: &mut [u8],
    anchors: &[TrustAnchor<'_>],
) -> Result<Reply, Failure> {
    if host.is_empty()
        || host.len() > 128
        || path.is_empty()
        || path.len() > 256
        || !path.starts_with('/')
        || path.bytes().any(|byte| !byte.is_ascii_graphic())
        || host.bytes().any(|byte| !byte.is_ascii_graphic())
        || output.is_empty()
    {
        return Err(Failure::BadRequest);
    }
    if !crate::random::fill(&mut [0u8; 1]) {
        return Err(Failure::NoEntropy);
    }
    let mut session = SESSION.try_lock().ok_or(Failure::Busy)?;
    let mut stream = Stream::connect(address, port).ok_or(Failure::Connect)?;
    let config = Config {
        host,
        now: crate::rtc::unix_seconds(),
        anchors,
        random: fill_random,
    };
    session.connect(&mut stream, &config).map_err(Failure::Tls)?;

    let mut request = [0u8; 512];
    let mut length = 0;
    for part in [
        b"GET ".as_slice(),
        path.as_bytes(),
        b" HTTP/1.1\r\nHost: ".as_slice(),
        host.as_bytes(),
        b"\r\nConnection: close\r\nUser-Agent: AerOS/0.1\r\nAccept: */*\r\n\r\n".as_slice(),
    ] {
        let Some(slot) = request.get_mut(length..length + part.len()) else {
            return Err(Failure::BadRequest);
        };
        slot.copy_from_slice(part);
        length += part.len();
    }
    session
        .write(&mut stream, &request[..length])
        .map_err(Failure::Tls)?;
    let mut bytes = 0;
    while bytes < output.len() {
        match session.read(&mut stream, &mut output[bytes..]) {
            Ok(0) => break,
            Ok(count) => bytes += count,
            // A server that just drops the connection after the response
            // is common; what arrived is still the response.
            Err(TlsError::Transport) if bytes > 0 => break,
            Err(error) => return Err(Failure::Tls(error)),
        }
    }
    session.close(&mut stream);
    Ok(Reply {
        status: crate::net::http_status(&output[..bytes]),
        bytes,
    })
}

#[cfg(feature = "boot-test")]
pub struct NetReport {
    pub roots: bool,
    pub rsa_chacha: bool,
    pub ec_aes: bool,
    pub wrong_host_refused: bool,
    pub untrusted_refused: bool,
}

#[cfg(feature = "boot-test")]
impl NetReport {
    pub fn verified(&self) -> bool {
        self.roots
            && self.rsa_chacha
            && self.ec_aes
            && self.wrong_host_refused
            && self.untrusted_refused
    }
}

#[cfg(feature = "boot-test")]
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

/// A root certificate the harness left on the virtio test disk: a 32-bit
/// length, then the DER, in four sectors from `lba`.
#[cfg(feature = "boot-test")]
fn read_root(lba: u64, buffer: &mut [u8; 2048]) -> Option<usize> {
    if crate::virtio_blk::sectors() < lba + 4 || !crate::virtio_blk::read(lba, 4, buffer) {
        return None;
    }
    let length = u32::from_le_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
    if length == 0 || length > 2044 {
        return None;
    }
    buffer.copy_within(4..4 + length, 0);
    Some(length)
}

/// Talks to two `openssl s_server` instances on the host (RSA chain with
/// ChaCha20-Poly1305, ECDSA chain with AES-128-GCM) whose roots the harness
/// put on the virtio test disk.
#[cfg(feature = "boot-test")]
pub fn self_test() -> NetReport {
    use crate::tls::x509::Certificate;

    let mut rsa_root = [0u8; 2048];
    let mut ec_root = [0u8; 2048];
    let rsa_length = read_root(100, &mut rsa_root);
    let ec_length = read_root(108, &mut ec_root);
    let mut report = NetReport {
        roots: false,
        rsa_chacha: false,
        ec_aes: false,
        wrong_host_refused: false,
        untrusted_refused: false,
    };
    let (Some(rsa_length), Some(ec_length)) = (rsa_length, ec_length) else {
        return report;
    };
    let (Ok(rsa), Ok(ec)) = (
        Certificate::parse(&rsa_root[..rsa_length]),
        Certificate::parse(&ec_root[..ec_length]),
    ) else {
        return report;
    };
    report.roots = true;
    let rsa_anchor = [TrustAnchor {
        subject: rsa.subject,
        spki: rsa.spki,
    }];
    let ec_anchor = [TrustAnchor {
        subject: ec.subject,
        spki: ec.spki,
    }];
    let gateway = crate::ip::v4([10, 0, 2, 2]);
    let mut page = [0u8; 8192];

    report.rsa_chacha = https_get(gateway, 18_443, "10.0.2.2", "/", &mut page, &rsa_anchor)
        .is_ok_and(|reply| {
            reply.status == 200 && contains(&page[..reply.bytes], b"Cipher is TLS_CHACHA20_POLY1305_SHA256")
        });
    report.ec_aes = https_get(gateway, 18_444, "10.0.2.2", "/", &mut page, &ec_anchor).is_ok_and(
        |reply| reply.status == 200 && contains(&page[..reply.bytes], b"Cipher is TLS_AES_128_GCM_SHA256"),
    );
    report.wrong_host_refused = matches!(
        https_get(gateway, 18_444, "evil.example", "/", &mut page, &ec_anchor),
        Err(Failure::Tls(TlsError::Certificate(crate::tls::x509::Error::HostName)))
    );
    report.untrusted_refused = matches!(
        https_get(gateway, 18_444, "10.0.2.2", "/", &mut page, &rsa_anchor),
        Err(Failure::Tls(TlsError::Certificate(crate::tls::x509::Error::Untrusted)))
    );
    report
}

/// Fetches `/` from a real public server over TLS with the built-in roots;
/// informational only (it needs the internet).
#[cfg(feature = "boot-test")]
pub fn internet_probe(host: &str) -> Result<u16, &'static str> {
    let lookup = crate::net::resolve(host);
    if !lookup.verified {
        return Err("dns failed");
    }
    let mut page = [0u8; 2048];
    https_get(
        crate::ip::v4(lookup.address),
        443,
        host,
        "/",
        &mut page,
        crate::tls::roots::ROOTS,
    )
    .map(|reply| reply.status)
    .map_err(Failure::describe)
}
