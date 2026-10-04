//! IPv6 for the card the kernel drives: stateless address configuration from
//! a Router Advertisement, a small neighbour cache, answers to Neighbor
//! Solicitations and echo requests for the machine's own addresses, and the
//! framing and parsing of TCP and UDP over IPv6. Like the rest of the network
//! stack nothing runs on its own: `tcpnet::poll` hands every frame it takes
//! from the card to `handle_frame`.

use crate::ip::{self, Address};
use crate::net::{self, ParsedTcp, ParsedUdp};
use crate::sync::TicketLock;
use crate::tcp::Outgoing;

const ETHERTYPE: [u8; 2] = [0x86, 0xdd];
const ICMP_ROUTER_SOLICITATION: u8 = 133;
const ICMP_ROUTER_ADVERTISEMENT: u8 = 134;
const ICMP_NEIGHBOR_SOLICITATION: u8 = 135;
const ICMP_NEIGHBOR_ADVERTISEMENT: u8 = 136;
const ICMP_ECHO_REQUEST: u8 = 128;
const ICMP_ECHO_REPLY: u8 = 129;
const NEIGHBORS: usize = 8;
const ALL_NODES: Address = [0xff, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01];
const ALL_ROUTERS: Address = [0xff, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x02];

pub enum Parsed {
    Tcp(ParsedTcp),
    Udp(ParsedUdp),
}

#[derive(Clone, Copy)]
struct Neighbor {
    address: Address,
    mac: [u8; 6],
    used: bool,
    stamp: u64,
}

#[derive(Clone, Copy)]
struct EchoReply {
    source: Address,
    identifier: u16,
    sequence: u16,
}

struct State {
    mac: [u8; 6],
    link_local: Address,
    global: Option<Address>,
    prefix: [u8; 8],
    router: Option<Address>,
    router_mac: Option<[u8; 6]>,
    dns: Option<Address>,
    neighbors: [Neighbor; NEIGHBORS],
    echo_reply: Option<EchoReply>,
}

static STATE: TicketLock<State> = TicketLock::new(State {
    mac: [0; 6],
    link_local: [0; 16],
    global: None,
    prefix: [0; 8],
    router: None,
    router_mac: None,
    dns: None,
    neighbors: [Neighbor {
        address: [0; 16],
        mac: [0; 6],
        used: false,
        stamp: 0,
    }; NEIGHBORS],
    echo_reply: None,
});

#[cfg(feature = "boot-test")]
static CAPTURE: TicketLock<Option<([u8; 1514], usize)>> = TicketLock::new(None);
#[cfg(feature = "boot-test")]
static CAPTURING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

#[derive(Clone, Copy)]
pub struct Info {
    pub link_local: Address,
    pub global: Option<Address>,
    pub router: Option<Address>,
    pub router_mac: Option<[u8; 6]>,
    pub dns: Option<Address>,
}

pub fn info() -> Info {
    let state = STATE.lock();
    Info {
        link_local: state.link_local,
        global: state.global,
        router: state.router,
        router_mac: state.router_mac,
        dns: state.dns,
    }
}

/// Whether `address` is one of this machine's IPv6 addresses.
pub fn is_own(address: &Address) -> bool {
    let state = STATE.lock();
    state.mac != [0; 6] && (*address == state.link_local || state.global == Some(*address))
}

/// Sets the link-layer address and the link-local address, which need no
/// network exchange.
pub fn start(mac: [u8; 6]) {
    let mut state = STATE.lock();
    state.mac = mac;
    state.link_local = net::link_local_address(mac);
}

pub struct Configuration {
    pub solicited: bool,
    pub advertised: bool,
    pub router: Address,
    pub neighbor_solicited: bool,
    pub neighbor_resolved: bool,
    pub router_mac: [u8; 6],
    pub global: Option<Address>,
}

/// Router discovery and address configuration: solicit a Router
/// Advertisement, take the prefix, the router's link-layer address and any
/// DNS server from it, resolve the router with a Neighbor Solicitation,
/// build the global address from the prefix and announce it.
pub fn configure(mac: [u8; 6]) -> Configuration {
    start(mac);
    let link_local = STATE.lock().link_local;
    let mut result = Configuration {
        solicited: false,
        advertised: false,
        router: [0; 16],
        neighbor_solicited: false,
        neighbor_resolved: false,
        router_mac: [0; 6],
        global: None,
    };
    let mut solicitation = [0u8; 16];
    solicitation[0] = ICMP_ROUTER_SOLICITATION;
    solicitation[8] = 1;
    solicitation[9] = 1;
    solicitation[10..16].copy_from_slice(&mac);
    let mut advert = [0u8; 512];
    let mut advert_length = 0;
    for _ in 0..3 {
        result.solicited |= net::send_icmpv6(
            mac,
            link_local,
            ALL_ROUTERS,
            [0x33, 0x33, 0, 0, 0, 0x02],
            255,
            &solicitation,
        );
        if let Some(message) =
            net::wait_icmpv6(mac, ICMP_ROUTER_ADVERTISEMENT, 1_000_000_000, &mut advert)
        {
            result.advertised = true;
            result.router = message.source;
            advert_length = message.bytes.min(advert.len());
            break;
        }
    }
    if !result.advertised {
        return result;
    }
    learn_router(&result.router, &advert[..advert_length]);

    let (multicast, multicast_mac) = net::solicited_node_multicast(result.router);
    let mut neighbor_solicitation = [0u8; 32];
    neighbor_solicitation[0] = ICMP_NEIGHBOR_SOLICITATION;
    neighbor_solicitation[8..24].copy_from_slice(&result.router);
    neighbor_solicitation[24] = 1;
    neighbor_solicitation[25] = 1;
    neighbor_solicitation[26..32].copy_from_slice(&mac);
    result.neighbor_solicited = net::send_icmpv6(
        mac,
        link_local,
        multicast,
        multicast_mac,
        255,
        &neighbor_solicitation,
    );
    if result.neighbor_solicited {
        let mut payload = [0u8; 64];
        if let Some(message) = net::wait_icmpv6(
            mac,
            ICMP_NEIGHBOR_ADVERTISEMENT,
            2_000_000_000,
            &mut payload,
        ) && message.source == result.router
            && message.bytes >= 32
            && payload[24] == 2
        {
            result.router_mac.copy_from_slice(&payload[26..32]);
            result.neighbor_resolved = true;
            let mut state = STATE.lock();
            state.router_mac = Some(result.router_mac);
            remember(&mut state, result.router, result.router_mac);
        }
    }
    let global = STATE.lock().global;
    result.global = global;
    if let Some(global) = global
        && result.neighbor_resolved
    {
        let mut announcement = [0u8; 32];
        announcement[0] = ICMP_NEIGHBOR_ADVERTISEMENT;
        announcement[4] = 0x20;
        announcement[8..24].copy_from_slice(&global);
        announcement[24] = 2;
        announcement[25] = 1;
        announcement[26..32].copy_from_slice(&mac);
        let _ = net::send_icmpv6(
            mac,
            global,
            ALL_NODES,
            [0x33, 0x33, 0, 0, 0, 0x01],
            255,
            &announcement,
        );
    }
    result
}

/// Takes the prefix, the router's link-layer address and a DNS server out of
/// a Router Advertisement body (starting at the ICMPv6 type byte).
fn learn_router(router: &Address, message: &[u8]) {
    let mut state = STATE.lock();
    state.router = Some(*router);
    let mut offset = 16;
    while offset + 2 <= message.len() {
        let kind = message[offset];
        let length = message[offset + 1] as usize * 8;
        if length == 0 || offset + length > message.len() {
            break;
        }
        let option = &message[offset..offset + length];
        match kind {
            1 if length == 8 => {
                let mut mac = [0u8; 6];
                mac.copy_from_slice(&option[2..8]);
                state.router_mac = Some(mac);
            }
            3 if length == 32 && option[2] == 64 && option[3] & 0x40 != 0 => {
                let mut prefix = [0u8; 8];
                prefix.copy_from_slice(&option[16..24]);
                state.prefix = prefix;
                state.global = Some(net::slaac_address(prefix, state.mac));
            }
            25 if length >= 24 => {
                let mut dns = [0u8; 16];
                dns.copy_from_slice(&option[8..24]);
                state.dns = Some(dns);
            }
            _ => {}
        }
        offset += length;
    }
}

fn remember(state: &mut State, address: Address, mac: [u8; 6]) {
    let now = crate::time::monotonic_nanoseconds();
    let slot = state
        .neighbors
        .iter()
        .position(|entry| entry.used && entry.address == address)
        .or_else(|| state.neighbors.iter().position(|entry| !entry.used))
        .unwrap_or_else(|| {
            state
                .neighbors
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.stamp)
                .map_or(0, |(index, _)| index)
        });
    state.neighbors[slot] = Neighbor {
        address,
        mac,
        used: true,
        stamp: now,
    };
}

fn transmit(frame: &[u8]) -> bool {
    #[cfg(feature = "boot-test")]
    if CAPTURING.load(core::sync::atomic::Ordering::Acquire) {
        let mut copy = [0u8; 1514];
        let length = frame.len().min(copy.len());
        copy[..length].copy_from_slice(&frame[..length]);
        *CAPTURE.lock() = Some((copy, length));
        return true;
    }
    crate::nic::transmit(frame)
}

/// The source address to use towards `destination`.
fn source_for(destination: &Address) -> Option<(Address, [u8; 6])> {
    let state = STATE.lock();
    if state.mac == [0; 6] {
        return None;
    }
    let source = match state.global {
        Some(global) if !ip::is_link_local(destination) => global,
        _ => state.link_local,
    };
    Some((source, state.mac))
}

fn frame_to(
    destination_mac: [u8; 6],
    source_mac: [u8; 6],
    source: &Address,
    destination: &Address,
    next_header: u8,
    hop_limit: u8,
    upper: &[u8],
) -> bool {
    if upper.len() > 1500 - 40 {
        return false;
    }
    let mut packet = [0u8; 1514];
    packet[..6].copy_from_slice(&destination_mac);
    packet[6..12].copy_from_slice(&source_mac);
    packet[12..14].copy_from_slice(&ETHERTYPE);
    packet[14] = 0x60;
    packet[18..20].copy_from_slice(&(upper.len() as u16).to_be_bytes());
    packet[20] = next_header;
    packet[21] = hop_limit;
    packet[22..38].copy_from_slice(source);
    packet[38..54].copy_from_slice(destination);
    packet[54..54 + upper.len()].copy_from_slice(upper);
    transmit(&packet[..54 + upper.len()])
}

/// The link-layer address to send to for `destination`: multicast maps
/// directly, on-link addresses come from the neighbour cache (asking when
/// unknown), everything else goes through the router.
pub fn resolve_mac(destination: &Address) -> Option<[u8; 6]> {
    if ip::is_multicast(destination) {
        return Some([
            0x33,
            0x33,
            destination[12],
            destination[13],
            destination[14],
            destination[15],
        ]);
    }
    let (on_link, router_mac) = {
        let state = STATE.lock();
        let on_link = ip::is_link_local(destination)
            || (state.global.is_some() && destination[..8] == state.prefix);
        (on_link, state.router_mac)
    };
    if !on_link {
        return router_mac;
    }
    if let Some(mac) = lookup(destination) {
        return Some(mac);
    }
    solicit(destination)
}

fn lookup(address: &Address) -> Option<[u8; 6]> {
    STATE
        .lock()
        .neighbors
        .iter()
        .find(|entry| entry.used && entry.address == *address)
        .map(|entry| entry.mac)
}

/// Sends a Neighbor Solicitation and waits briefly for the answer. Other
/// frames that arrive meanwhile are dropped; TCP will retransmit.
fn solicit(target: &Address) -> Option<[u8; 6]> {
    let (source, mac) = source_for(&[0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])?;
    let (multicast, multicast_mac) = net::solicited_node_multicast(*target);
    let mut message = [0u8; 32];
    message[0] = ICMP_NEIGHBOR_SOLICITATION;
    message[8..24].copy_from_slice(target);
    message[24] = 1;
    message[25] = 1;
    message[26..32].copy_from_slice(&mac);
    if !net::send_icmpv6(mac, source, multicast, multicast_mac, 255, &message) {
        return None;
    }
    let deadline = crate::time::monotonic_nanoseconds().saturating_add(300_000_000);
    let mut frame = [0u8; 2048];
    while crate::time::monotonic_nanoseconds() < deadline {
        if let Some(length) = crate::nic::try_receive(&mut frame) {
            let _ = handle_frame(&frame[..length]);
            if let Some(found) = lookup(target) {
                return Some(found);
            }
        }
        core::hint::spin_loop();
    }
    None
}

pub fn send_tcp(segment: &Outgoing) -> bool {
    let Some((source, mac)) = source_for(&segment.remote) else {
        return false;
    };
    let Some(destination_mac) = resolve_mac(&segment.remote) else {
        return false;
    };
    let mut tcp = [0u8; 1460];
    let length = net::build_tcp_segment(
        segment.local_port,
        segment.remote_port,
        segment.seq,
        segment.ack,
        segment.flags,
        segment.window,
        segment.mss,
        &segment.sack,
        segment.payload,
        &mut tcp,
    );
    if length == 0 {
        return false;
    }
    let sum = net::transport_checksum_v6(source, segment.remote, 6, &tcp[..length]);
    tcp[16..18].copy_from_slice(&sum.to_be_bytes());
    frame_to(
        destination_mac,
        mac,
        &source,
        &segment.remote,
        6,
        64,
        &tcp[..length],
    )
}

pub fn send_udp(
    destination: &Address,
    source_port: u16,
    destination_port: u16,
    payload: &[u8],
) -> bool {
    if payload.len() > 1440 {
        return false;
    }
    let Some((source, mac)) = source_for(destination) else {
        return false;
    };
    let Some(destination_mac) = resolve_mac(destination) else {
        return false;
    };
    let mut udp = [0u8; 8 + 1440];
    let length = 8 + payload.len();
    udp[..2].copy_from_slice(&source_port.to_be_bytes());
    udp[2..4].copy_from_slice(&destination_port.to_be_bytes());
    udp[4..6].copy_from_slice(&(length as u16).to_be_bytes());
    udp[8..length].copy_from_slice(payload);
    let sum = net::transport_checksum_v6(source, *destination, 17, &udp[..length]);
    udp[6..8].copy_from_slice(&if sum == 0 { 0xffff } else { sum }.to_be_bytes());
    frame_to(
        destination_mac,
        mac,
        &source,
        destination,
        17,
        64,
        &udp[..length],
    )
}

/// Sends an echo request and waits for the matching reply; the elapsed time
/// in nanoseconds.
pub fn ping(destination: &Address, identifier: u16, sequence: u16, timeout_ns: u64) -> Option<u64> {
    let (source, mac) = source_for(destination)?;
    let destination_mac = resolve_mac(destination)?;
    let mut message = [0u8; 8 + 16];
    message[0] = ICMP_ECHO_REQUEST;
    message[4..6].copy_from_slice(&identifier.to_be_bytes());
    message[6..8].copy_from_slice(&sequence.to_be_bytes());
    message[8..].copy_from_slice(b"AEROS-V6-PING-OK");
    STATE.lock().echo_reply = None;
    let started = crate::time::monotonic_nanoseconds();
    if !net::send_icmpv6(mac, source, *destination, destination_mac, 64, &message) {
        return None;
    }
    crate::tcpnet::activate();
    loop {
        crate::tcpnet::poll();
        let reply = STATE.lock().echo_reply;
        if let Some(reply) = reply
            && reply.source == *destination
            && reply.identifier == identifier
            && reply.sequence == sequence
        {
            return Some(crate::time::monotonic_nanoseconds().saturating_sub(started));
        }
        if crate::time::monotonic_nanoseconds().saturating_sub(started) >= timeout_ns {
            return None;
        }
        core::hint::spin_loop();
    }
}

fn answer_neighbor_solicitation(source: &Address, body: &[u8], frame_mac: [u8; 6]) {
    if body.len() < 24 {
        return;
    }
    let mut target = [0u8; 16];
    target.copy_from_slice(&body[8..24]);
    if !is_own(&target) {
        return;
    }
    let mac = STATE.lock().mac;
    let mut reply = [0u8; 32];
    reply[0] = ICMP_NEIGHBOR_ADVERTISEMENT;
    reply[4] = 0x60;
    reply[8..24].copy_from_slice(&target);
    reply[24] = 2;
    reply[25] = 1;
    reply[26..32].copy_from_slice(&mac);
    let unspecified = *source == [0; 16];
    let destination = if unspecified { ALL_NODES } else { *source };
    let destination_mac = if unspecified {
        [0x33, 0x33, 0, 0, 0, 0x01]
    } else {
        frame_mac
    };
    if unspecified {
        reply[4] = 0x20;
    }
    let mut message = reply;
    let sum = net::transport_checksum_v6(target, destination, 58, &message);
    message[2..4].copy_from_slice(&sum.to_be_bytes());
    let mut packet = [0u8; 14 + 40 + 32];
    packet[..6].copy_from_slice(&destination_mac);
    packet[6..12].copy_from_slice(&mac);
    packet[12..14].copy_from_slice(&ETHERTYPE);
    packet[14] = 0x60;
    packet[18..20].copy_from_slice(&32u16.to_be_bytes());
    packet[20] = 58;
    packet[21] = 255;
    packet[22..38].copy_from_slice(&target);
    packet[38..54].copy_from_slice(&destination);
    packet[54..].copy_from_slice(&message);
    let _ = transmit(&packet);
}

fn answer_echo(source: &Address, destination: &Address, body: &[u8], frame_mac: [u8; 6]) {
    if body.len() < 8 || body.len() > 1024 {
        return;
    }
    let mac = STATE.lock().mac;
    let mut reply = [0u8; 1024];
    reply[..body.len()].copy_from_slice(body);
    reply[0] = ICMP_ECHO_REPLY;
    reply[2] = 0;
    reply[3] = 0;
    let own = if ip::is_multicast(destination) {
        match source_for(source) {
            Some((address, _)) => address,
            None => return,
        }
    } else {
        *destination
    };
    let sum = net::transport_checksum_v6(own, *source, 58, &reply[..body.len()]);
    reply[2..4].copy_from_slice(&sum.to_be_bytes());
    let _ = frame_to(frame_mac, mac, &own, source, 58, 64, &reply[..body.len()]);
}

/// Processes one frame from the card. Neighbour discovery, router
/// advertisements and echo requests are handled here; TCP and UDP segments
/// for this machine are returned for the caller to deliver.
pub fn handle_frame(packet: &[u8]) -> Option<Parsed> {
    if packet.len() < 14 + 40 || packet[12..14] != ETHERTYPE || packet[14] >> 4 != 6 {
        return None;
    }
    let (mac, link_local, global) = {
        let state = STATE.lock();
        (state.mac, state.link_local, state.global)
    };
    if mac == [0; 6] || (packet[..6] != mac && packet[0] != 0x33) {
        return None;
    }
    let payload_length = u16::from_be_bytes([packet[18], packet[19]]) as usize;
    if 54 + payload_length > packet.len() || payload_length == 0 {
        return None;
    }
    let mut source = [0u8; 16];
    source.copy_from_slice(&packet[22..38]);
    let mut destination = [0u8; 16];
    destination.copy_from_slice(&packet[38..54]);
    let for_us = destination == link_local
        || global == Some(destination)
        || destination == ALL_NODES
        || (ip::is_multicast(&destination)
            && destination[11] == 1
            && destination[12] == 0xff
            && (destination[13..] == link_local[13..]
                || global.is_some_and(|address| destination[13..] == address[13..])));
    if !for_us {
        return None;
    }
    let body = &packet[54..54 + payload_length];
    let mut frame_mac = [0u8; 6];
    frame_mac.copy_from_slice(&packet[6..12]);
    match packet[20] {
        58 => {
            if body.len() < 4 || net::transport_checksum_v6(source, destination, 58, body) != 0 {
                return None;
            }
            let discovery = packet[21] == 255;
            match body[0] {
                ICMP_NEIGHBOR_SOLICITATION if discovery => {
                    if body.len() >= 32 && body[24] == 1 && body[25] == 1 && source != [0; 16] {
                        let mut learned = [0u8; 6];
                        learned.copy_from_slice(&body[26..32]);
                        remember(&mut STATE.lock(), source, learned);
                    }
                    answer_neighbor_solicitation(&source, body, frame_mac);
                }
                ICMP_NEIGHBOR_ADVERTISEMENT
                    if discovery && body.len() >= 32 && body[24] == 2 && body[25] == 1 =>
                {
                    let mut target = [0u8; 16];
                    target.copy_from_slice(&body[8..24]);
                    let mut learned = [0u8; 6];
                    learned.copy_from_slice(&body[26..32]);
                    let mut state = STATE.lock();
                    remember(&mut state, target, learned);
                    if state.router == Some(target) {
                        state.router_mac = Some(learned);
                    }
                }
                ICMP_ROUTER_ADVERTISEMENT if discovery && ip::is_link_local(&source) => {
                    learn_router(&source, body);
                }
                ICMP_ECHO_REQUEST => answer_echo(&source, &destination, body, frame_mac),
                ICMP_ECHO_REPLY if body.len() >= 8 => {
                    STATE.lock().echo_reply = Some(EchoReply {
                        source,
                        identifier: u16::from_be_bytes([body[4], body[5]]),
                        sequence: u16::from_be_bytes([body[6], body[7]]),
                    });
                }
                _ => {}
            }
            None
        }
        6 => {
            if body.len() < 20 || net::transport_checksum_v6(source, destination, 6, body) != 0 {
                return None;
            }
            let header = (body[12] as usize >> 4) * 4;
            if header < 20 || header > body.len() {
                return None;
            }
            let (mss, sack) = net::tcp_options(&body[20..header]);
            Some(Parsed::Tcp(ParsedTcp {
                remote: source,
                remote_port: u16::from_be_bytes([body[0], body[1]]),
                local_port: u16::from_be_bytes([body[2], body[3]]),
                sequence: u32::from_be_bytes([body[4], body[5], body[6], body[7]]),
                acknowledgement: u32::from_be_bytes([body[8], body[9], body[10], body[11]]),
                flags: body[13],
                window: u16::from_be_bytes([body[14], body[15]]),
                mss,
                sack,
                payload_start: 54 + header,
                payload_len: body.len() - header,
            }))
        }
        17 => {
            if body.len() < 8 {
                return None;
            }
            let length = u16::from_be_bytes([body[4], body[5]]) as usize;
            if length < 8
                || length > body.len()
                || net::transport_checksum_v6(source, destination, 17, &body[..length]) != 0
            {
                return None;
            }
            Some(Parsed::Udp(ParsedUdp {
                remote: source,
                remote_port: u16::from_be_bytes([body[0], body[1]]),
                local_port: u16::from_be_bytes([body[2], body[3]]),
                payload_start: 54 + 8,
                payload_len: length - 8,
            }))
        }
        _ => None,
    }
}

#[cfg(feature = "boot-test")]
fn capture_next() -> Option<([u8; 1514], usize)> {
    CAPTURE.lock().take()
}

#[cfg(feature = "boot-test")]
fn solicitation_frame(
    mac: [u8; 6],
    source: Address,
    destination: Address,
    destination_mac: [u8; 6],
    target: Address,
    hop_limit: u8,
) -> ([u8; 1514], usize) {
    let mut message = [0u8; 32];
    message[0] = ICMP_NEIGHBOR_SOLICITATION;
    message[8..24].copy_from_slice(&target);
    message[24] = 1;
    message[25] = 1;
    message[26..32].copy_from_slice(&mac);
    let sum = net::transport_checksum_v6(source, destination, 58, &message);
    message[2..4].copy_from_slice(&sum.to_be_bytes());
    let mut packet = [0u8; 1514];
    packet[..6].copy_from_slice(&destination_mac);
    packet[6..12].copy_from_slice(&mac);
    packet[12..14].copy_from_slice(&ETHERTYPE);
    packet[14] = 0x60;
    packet[18..20].copy_from_slice(&32u16.to_be_bytes());
    packet[20] = 58;
    packet[21] = hop_limit;
    packet[22..38].copy_from_slice(&source);
    packet[38..54].copy_from_slice(&destination);
    packet[54..86].copy_from_slice(&message);
    (packet, 86)
}

/// Exercises the responders and the framing without a network: frames built
/// here go through `handle_frame` and what the machine transmits in answer is
/// captured instead of sent.
#[cfg(feature = "boot-test")]
pub fn offline_self_test() -> bool {
    use core::sync::atomic::Ordering;
    let saved = *STATE.lock().neighbors.first().unwrap_or(&Neighbor {
        address: [0; 16],
        mac: [0; 6],
        used: false,
        stamp: 0,
    });
    let _ = saved;
    let (mac, link_local, global) = {
        let state = STATE.lock();
        (state.mac, state.link_local, state.global)
    };
    if mac == [0; 6] {
        return false;
    }
    let peer_mac = [0x02, 0x00, 0x5e, 0x00, 0x53, 0x77];
    let peer = ip::parse("fe80::200:5eff:fe00:5377").unwrap_or([0; 16]);
    let (multicast, multicast_mac) = net::solicited_node_multicast(link_local);
    CAPTURING.store(true, Ordering::Release);
    let _ = capture_next();

    let (frame, length) =
        solicitation_frame(peer_mac, peer, multicast, multicast_mac, link_local, 255);
    let consumed = handle_frame(&frame[..length]).is_none();
    let answered = capture_next().is_some_and(|(reply, reply_length)| {
        reply_length == 86
            && reply[..6] == peer_mac
            && reply[6..12] == mac
            && reply[20] == 58
            && reply[21] == 255
            && reply[22..38] == link_local
            && reply[38..54] == peer
            && reply[54] == ICMP_NEIGHBOR_ADVERTISEMENT
            && reply[58] & 0x60 == 0x60
            && reply[62..78] == link_local
            && reply[78] == 2
            && reply[80..86] == mac
            && net::transport_checksum_v6(link_local, peer, 58, &reply[54..86]) == 0
    });
    let learned = lookup(&peer) == Some(peer_mac);

    let (frame, length) =
        solicitation_frame(peer_mac, peer, multicast, multicast_mac, link_local, 64);
    let _ = handle_frame(&frame[..length]);
    let spoof_ignored = capture_next().is_none();

    let other = ip::parse("fe80::1234").unwrap_or([0; 16]);
    let (frame, length) = solicitation_frame(peer_mac, peer, multicast, multicast_mac, other, 255);
    let _ = handle_frame(&frame[..length]);
    let foreign_ignored = capture_next().is_none();

    let mut echo = [0u8; 8 + 12];
    echo[0] = ICMP_ECHO_REQUEST;
    echo[4..6].copy_from_slice(&0x1234u16.to_be_bytes());
    echo[6..8].copy_from_slice(&7u16.to_be_bytes());
    echo[8..].copy_from_slice(b"hello, ipv6!");
    let sum = net::transport_checksum_v6(peer, link_local, 58, &echo);
    echo[2..4].copy_from_slice(&sum.to_be_bytes());
    let mut packet = [0u8; 14 + 40 + 20];
    packet[..6].copy_from_slice(&mac);
    packet[6..12].copy_from_slice(&peer_mac);
    packet[12..14].copy_from_slice(&ETHERTYPE);
    packet[14] = 0x60;
    packet[18..20].copy_from_slice(&20u16.to_be_bytes());
    packet[20] = 58;
    packet[21] = 64;
    packet[22..38].copy_from_slice(&peer);
    packet[38..54].copy_from_slice(&link_local);
    packet[54..].copy_from_slice(&echo);
    let _ = handle_frame(&packet);
    let echoed = capture_next().is_some_and(|(reply, reply_length)| {
        reply_length == 74
            && reply[..6] == peer_mac
            && reply[54] == ICMP_ECHO_REPLY
            && reply[58..60] == 0x1234u16.to_be_bytes()
            && reply[60..62] == 7u16.to_be_bytes()
            && reply[62..74] == *b"hello, ipv6!"
            && net::transport_checksum_v6(link_local, peer, 58, &reply[54..74]) == 0
    });
    packet[60] ^= 0x01;
    let _ = handle_frame(&packet);
    let corrupt_ignored = capture_next().is_none();

    let tcp_out = Outgoing {
        remote: peer,
        local_port: 4000,
        remote_port: 80,
        seq: 1,
        ack: 2,
        flags: crate::tcp::ACK | crate::tcp::PSH,
        window: 1000,
        mss: 0,
        sack: crate::tcp::Sack::default(),
        payload: b"payload",
    };
    let sent = send_tcp(&tcp_out);
    let framed = capture_next().is_some_and(|(frame, frame_length)| {
        let mut flipped = frame;
        let parsed = handle_frame(&frame[..frame_length]);
        // Looped back at us it is addressed to the peer, so it is not ours.
        let _ = flipped.iter_mut().next();
        sent && frame_length == 54 + 20 + 7
            && frame[20] == 6
            && frame[22..38] == link_local
            && frame[38..54] == peer
            && net::transport_checksum_v6(link_local, peer, 6, &frame[54..frame_length]) == 0
            && parsed.is_none()
    });

    let mut inbound = [0u8; 14 + 40 + 20 + 5];
    inbound[..6].copy_from_slice(&mac);
    inbound[6..12].copy_from_slice(&peer_mac);
    inbound[12..14].copy_from_slice(&ETHERTYPE);
    inbound[14] = 0x60;
    inbound[18..20].copy_from_slice(&25u16.to_be_bytes());
    inbound[20] = 6;
    inbound[21] = 64;
    inbound[22..38].copy_from_slice(&peer);
    inbound[38..54].copy_from_slice(&link_local);
    inbound[54..56].copy_from_slice(&4321u16.to_be_bytes());
    inbound[56..58].copy_from_slice(&80u16.to_be_bytes());
    inbound[58..62].copy_from_slice(&100u32.to_be_bytes());
    inbound[62..66].copy_from_slice(&200u32.to_be_bytes());
    inbound[66] = 5 << 4;
    inbound[67] = crate::tcp::ACK;
    inbound[68..70].copy_from_slice(&512u16.to_be_bytes());
    inbound[74..79].copy_from_slice(b"world");
    let sum = net::transport_checksum_v6(peer, link_local, 6, &inbound[54..79]);
    inbound[70..72].copy_from_slice(&sum.to_be_bytes());
    let tcp_in = matches!(
        handle_frame(&inbound),
        Some(Parsed::Tcp(parsed))
            if parsed.remote == peer
                && parsed.remote_port == 4321
                && parsed.local_port == 80
                && parsed.sequence == 100
                && parsed.acknowledgement == 200
                && parsed.window == 512
                && parsed.payload_len == 5
                && &inbound[parsed.payload_start..parsed.payload_start + 5] == b"world"
    );
    inbound[75] ^= 0x40;
    let tcp_corrupt = handle_frame(&inbound).is_none();

    let udp_ok = {
        let sent = send_udp(&peer, 5000, 5353, b"datagram");
        sent && capture_next().is_some_and(|(frame, length)| {
            length == 54 + 8 + 8
                && frame[20] == 17
                && net::transport_checksum_v6(link_local, peer, 17, &frame[54..length]) == 0
        })
    };
    CAPTURING.store(false, Ordering::Release);
    let _ = capture_next();
    let _ = global;
    consumed
        && answered
        && learned
        && spoof_ignored
        && foreign_ignored
        && echoed
        && corrupt_ignored
        && framed
        && tcp_in
        && tcp_corrupt
        && udp_ok
}

/// Calls `visit(address, mac)` for every neighbour the machine has learned.
pub fn each_neighbor(mut visit: impl FnMut(&Address, [u8; 6])) {
    let neighbors = STATE.lock().neighbors;
    for entry in neighbors.iter().filter(|entry| entry.used) {
        visit(&entry.address, entry.mac);
    }
}

/// The source address this machine would use towards `destination`.
pub fn source_address(destination: &Address) -> Option<Address> {
    source_for(destination).map(|(address, _)| address)
}
