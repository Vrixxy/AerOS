//! The kernel's one TCP table and its connection to the network: frames from
//! the card are parsed and fed to the engine, and segments the engine emits go
//! to the card, or straight back in when the peer is this host (127.x.x.x or
//! the machine's own address). Nothing runs on a timer; `poll` is called by
//! every socket operation, which also drives retransmission.

use crate::ip::{self, Address};
use crate::sync::TicketLock;
use crate::tcp::{Incoming, MSS, Outgoing, Sack, Sink, Tcp};
use core::sync::atomic::{AtomicBool, Ordering};

pub static TCP: TicketLock<Tcp> = TicketLock::new(Tcp::new());
static ACTIVE: AtomicBool = AtomicBool::new(false);

const LOOPBACK_FRAMES: usize = 24;

#[derive(Clone, Copy)]
struct Frame {
    remote: Address,
    local_port: u16,
    remote_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    window: u16,
    mss: u16,
    sack: Sack,
    length: usize,
    data: [u8; MSS],
}

impl Frame {
    const EMPTY: Self = Self {
        remote: [0; 16],
        local_port: 0,
        remote_port: 0,
        seq: 0,
        ack: 0,
        flags: 0,
        window: 0,
        mss: 0,
        sack: Sack {
            permitted: false,
            count: 0,
            blocks: [(0, 0); crate::tcp::MAX_SACK],
        },
        length: 0,
        data: [0; MSS],
    };
}

struct Loopback {
    frames: [Frame; LOOPBACK_FRAMES],
    head: usize,
    count: usize,
}

static LOOPBACK: TicketLock<Loopback> = TicketLock::new(Loopback {
    frames: [Frame::EMPTY; LOOPBACK_FRAMES],
    head: 0,
    count: 0,
});

pub fn is_local(address: Address) -> bool {
    match ip::as_v4(&address) {
        Some(v4) => v4[0] == 127 || v4 == crate::net::local_address(),
        None => address == ip::LOOPBACK6 || crate::ipv6::is_own(&address),
    }
}

struct Hardware;

impl Sink for Hardware {
    fn transmit(&mut self, segment: &Outgoing) -> bool {
        if is_local(segment.remote) {
            let mut queue = LOOPBACK.lock();
            if queue.count == LOOPBACK_FRAMES {
                return true;
            }
            let slot = (queue.head + queue.count) % LOOPBACK_FRAMES;
            let frame = &mut queue.frames[slot];
            frame.remote = segment.remote;
            frame.local_port = segment.local_port;
            frame.remote_port = segment.remote_port;
            frame.seq = segment.seq;
            frame.ack = segment.ack;
            frame.flags = segment.flags;
            frame.window = segment.window;
            frame.mss = segment.mss;
            frame.sack = segment.sack;
            frame.length = segment.payload.len();
            frame.data[..frame.length].copy_from_slice(segment.payload);
            queue.count += 1;
            return true;
        }
        let Some(remote) = ip::as_v4(&segment.remote) else {
            return crate::ipv6::send_tcp(segment);
        };
        crate::net::send_tcp_window(
            remote,
            segment.local_port,
            segment.remote_port,
            segment.seq,
            segment.ack,
            segment.flags,
            segment.window,
            segment.mss,
            &segment.sack,
            segment.payload,
        )
    }
}

/// Marks that at least one socket has existed, so background polling starts.
pub fn activate() {
    ACTIVE.store(true, Ordering::Release);
}

pub fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

pub fn now() -> u64 {
    crate::time::monotonic_nanoseconds()
}

/// Runs `body` on the table with the hardware as the segment sink.
pub fn with_tcp<R>(body: impl FnOnce(&mut Tcp, &mut dyn Sink, u64) -> R) -> R {
    let mut tcp = TCP.lock();
    body(&mut tcp, &mut Hardware, now())
}

/// Delivers pending loopback segments and card frames, then runs timers.
pub fn poll() {
    for _ in 0..LOOPBACK_FRAMES * 2 {
        let frame = {
            let mut queue = LOOPBACK.lock();
            if queue.count == 0 {
                break;
            }
            let frame = queue.frames[queue.head];
            queue.head = (queue.head + 1) % LOOPBACK_FRAMES;
            queue.count -= 1;
            frame
        };
        let mut tcp = TCP.lock();
        tcp.input(
            &Incoming {
                remote: frame.remote,
                remote_port: frame.local_port,
                local_port: frame.remote_port,
                seq: frame.seq,
                ack: frame.ack,
                flags: frame.flags,
                window: frame.window,
                mss: frame.mss,
                sack: frame.sack,
                payload: &frame.data[..frame.length],
            },
            now(),
            &mut Hardware,
        );
    }
    if crate::net::is_ready() {
        let mut packet = [0u8; 2048];
        for _ in 0..16 {
            let Some(length) = crate::nic::try_receive(&mut packet) else {
                break;
            };
            let parsed = match crate::net::parse_tcp(&packet[..length]) {
                Some(parsed) => Some(crate::ipv6::Parsed::Tcp(parsed)),
                None => match crate::net::parse_udp(&packet[..length]) {
                    Some(datagram) => Some(crate::ipv6::Parsed::Udp(datagram)),
                    None => crate::ipv6::handle_frame(&packet[..length]),
                },
            };
            match parsed {
                Some(crate::ipv6::Parsed::Tcp(parsed)) => {
                    let payload =
                        &packet[parsed.payload_start..parsed.payload_start + parsed.payload_len];
                    let mut tcp = TCP.lock();
                    tcp.input(
                        &Incoming {
                            remote: parsed.remote,
                            remote_port: parsed.remote_port,
                            local_port: parsed.local_port,
                            seq: parsed.sequence,
                            ack: parsed.acknowledgement,
                            flags: parsed.flags,
                            window: parsed.window,
                            mss: parsed.mss,
                            sack: parsed.sack,
                            payload,
                        },
                        now(),
                        &mut Hardware,
                    );
                }
                Some(crate::ipv6::Parsed::Udp(datagram)) => {
                    crate::udp::deliver(
                        datagram.local_port,
                        datagram.remote,
                        datagram.remote_port,
                        &packet
                            [datagram.payload_start..datagram.payload_start + datagram.payload_len],
                    );
                }
                None => {}
            }
        }
    }
    TCP.lock().tick(now(), &mut Hardware);
}

pub fn reset() {
    ACTIVE.store(false, Ordering::Release);
    TCP.lock().reset();
    let mut queue = LOOPBACK.lock();
    queue.head = 0;
    queue.count = 0;
}
