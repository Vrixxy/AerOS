mod chain_data {
    include!("chain_data.rs");
}

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use chain_data::{CHAINS, Chain};
use tls_host::tls::client::{Config, Error, Session, Transport};
use tls_host::tls::x509::{self, Certificate, TrustAnchor};

const NOW: u64 = 1_800_000_000;

struct Stream(TcpStream);

impl Transport for Stream {
    fn send(&mut self, data: &[u8]) -> Result<(), Error> {
        self.0.write_all(data).map_err(|_| Error::Transport)
    }

    fn receive(&mut self, buffer: &mut [u8]) -> Result<usize, Error> {
        self.0.read(buffer).map_err(|_| Error::Transport)
    }
}

static SEED: AtomicU64 = AtomicU64::new(0x1234_5678_9abc_def1);

fn random(buffer: &mut [u8]) {
    for byte in buffer {
        let mut x = SEED.load(Ordering::Relaxed);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        SEED.store(x, Ordering::Relaxed);
        *byte = (x >> 24) as u8;
    }
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap())
        .collect()
}

fn anchor_of(root: &str) -> TrustAnchor {
    let der: &'static [u8] = Box::leak(hex(root).into_boxed_slice());
    let certificate = Certificate::parse(der).unwrap();
    TrustAnchor {
        subject: certificate.subject,
        spki: certificate.spki,
    }
}

struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_server(chain: &Chain, suite: &str, tag: &str) -> Server {
    let directory = std::env::temp_dir().join(format!("aeros-tls-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let cert = directory.join("leaf.pem");
    let intermediates = directory.join("chain.pem");
    let key = directory.join("key.pem");
    std::fs::write(&cert, chain.leaf_pem).unwrap();
    std::fs::write(&intermediates, chain.chain_pem).unwrap();
    std::fs::write(&key, chain.leaf_key_pem).unwrap();
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut command = Command::new("openssl");
    command.args([
        "s_server",
        "-accept",
        &port.to_string(),
        "-cert",
        cert.to_str().unwrap(),
    ]);
    if !chain.chain_pem.is_empty() {
        command.args(["-cert_chain", intermediates.to_str().unwrap()]);
    }
    let child = command
        .args([
            "-key",
            key.to_str().unwrap(),
            "-tls1_3",
            "-www",
            "-ciphersuites",
            suite,
            "-groups",
            "X25519",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("openssl s_server");
    let server = Server { child, port };
    for _ in 0..100 {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    server
}

fn connect(server: &Server) -> Stream {
    connect_within(server, 10)
}

fn connect_within(server: &Server, seconds: u64) -> Stream {
    let stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(seconds))).unwrap();
    stream.set_write_timeout(Some(Duration::from_secs(10))).unwrap();
    Stream(stream)
}

fn fetch(chain: &Chain, suite: &str, host: &str, now: u64, tag: &str) -> Result<String, Error> {
    let server = start_server(chain, suite, tag);
    let anchors = [anchor_of(chain.root)];
    let config = Config {
        host,
        now,
        anchors: &anchors,
        random,
    };
    let mut stream = connect(&server);
    let mut session = Box::new(Session::new());
    session.connect(&mut stream, &config)?;
    session.write(&mut stream, b"GET / HTTP/1.0\r\n\r\n")?;
    let mut body = Vec::new();
    let mut buffer = [0u8; 2048];
    loop {
        let count = session.read(&mut stream, &mut buffer)?;
        if count == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..count]);
    }
    session.close(&mut stream);
    Ok(String::from_utf8_lossy(&body).into_owned())
}

#[test]
fn handshake_with_openssl_every_chain_and_suite() {
    let mut counter = 0;
    for chain in CHAINS {
        if matches!(chain.name, "not-a-ca" | "other-root") {
            continue;
        }
        for suite in ["TLS_CHACHA20_POLY1305_SHA256", "TLS_AES_128_GCM_SHA256"] {
            counter += 1;
            let reply = fetch(chain, suite, "aeros.test", NOW, &format!("a{counter}"))
                .unwrap_or_else(|error| panic!("{} {suite}: {error:?}", chain.name));
            assert!(
                reply.starts_with("HTTP/1.0 200 ok"),
                "{} {suite}: {}",
                chain.name,
                &reply[..reply.len().min(80)]
            );
            assert!(reply.contains("New, TLSv1.3"), "{} {suite}", chain.name);
        }
    }
}

#[test]
fn certificate_failures_stop_the_handshake() {
    let chain = CHAINS.iter().find(|c| c.name == "ec-p384-ec").unwrap();
    let suite = "TLS_CHACHA20_POLY1305_SHA256";
    assert_eq!(
        fetch(chain, suite, "wrong.test", NOW, "b1").err(),
        Some(Error::Certificate(x509::Error::HostName))
    );
    assert_eq!(
        fetch(chain, suite, "aeros.test", 4_000_000_000, "b2").err(),
        Some(Error::Certificate(x509::Error::Expired))
    );
    // A server whose root is not ours.
    let other = CHAINS.iter().find(|c| c.name == "other-root").unwrap();
    let server = start_server(chain, suite, "b3");
    let anchors = [anchor_of(other.root)];
    let config = Config {
        host: "aeros.test",
        now: NOW,
        anchors: &anchors,
        random,
    };
    let mut stream = connect(&server);
    let mut session = Box::new(Session::new());
    assert_eq!(
        session.connect(&mut stream, &config).err(),
        Some(Error::Certificate(x509::Error::Untrusted))
    );
}

struct Corrupting {
    inner: Stream,
    flip_at: Option<usize>,
    stop_at: Option<usize>,
    seen: usize,
}

impl Transport for Corrupting {
    fn send(&mut self, data: &[u8]) -> Result<(), Error> {
        self.inner.send(data)
    }

    fn receive(&mut self, buffer: &mut [u8]) -> Result<usize, Error> {
        if self.stop_at.is_some_and(|stop| self.seen >= stop) {
            return Ok(0);
        }
        let count = self.inner.receive(buffer)?;
        if let Some(position) = self.flip_at
            && position >= self.seen
            && position < self.seen + count
        {
            buffer[position - self.seen] ^= 0x40;
        }
        self.seen += count;
        Ok(count)
    }
}

#[test]
fn corrupted_or_truncated_server_bytes_never_succeed_or_panic() {
    let chain = CHAINS.iter().find(|c| c.name == "rsa-ec-ec").unwrap();
    let server = start_server(chain, "TLS_CHACHA20_POLY1305_SHA256", "c1");
    let anchors = [anchor_of(chain.root)];
    let config = Config {
        host: "aeros.test",
        now: NOW,
        anchors: &anchors,
        random,
    };
    let mut session = Box::new(Session::new());
    // Bytes the handshake itself consumes; tickets that follow are not read
    // until the application asks for data.
    let mut probe = Corrupting {
        inner: connect(&server),
        flip_at: None,
        stop_at: None,
        seen: 0,
    };
    session.connect(&mut probe, &config).unwrap();
    let flight = probe.seen;
    assert!(flight > 800, "flight is {flight} bytes");
    let mut attempts = 0;
    for position in (0..flight).step_by(23) {
        let mut stream = Corrupting {
            inner: connect_within(&server, 1),
            flip_at: Some(position),
            stop_at: None,
            seen: 0,
        };
        assert!(
            session.connect(&mut stream, &config).is_err(),
            "flipping received byte {position} was accepted"
        );
        attempts += 1;
    }
    for stop in (0..flight).step_by(47) {
        let mut stream = Corrupting {
            inner: connect_within(&server, 1),
            flip_at: None,
            stop_at: Some(stop),
            seen: 0,
        };
        assert!(
            session.connect(&mut stream, &config).is_err(),
            "cutting the stream at {stop} was accepted"
        );
        attempts += 1;
    }
    assert!(attempts > 60, "{attempts} attempts");
    // The same session object is reusable for an honest connection.
    let fresh = start_server(chain, "TLS_CHACHA20_POLY1305_SHA256", "c2");
    let mut stream = connect(&fresh);
    assert!(session.connect(&mut stream, &config).is_ok());
}
