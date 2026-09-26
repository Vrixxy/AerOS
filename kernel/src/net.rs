use crate::e1000::NetworkReport;
use crate::sync::TicketLock;
use core::sync::atomic::{AtomicU64, Ordering};

const PAYLOAD: &[u8; 16] = b"AEROS-NET-VERIFY";
const DNS_QUERY: &[u8; 27] =
    b"\xa3\x11\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00\x09localhost\x00\x00\x01\x00\x01";
const MAX_UDP_PAYLOAD: usize = 1472;

#[derive(Clone, Copy)]
struct NetworkConfig {
    mac: [u8; 6],
    gateway_mac: [u8; 6],
    local_ip: [u8; 4],
    dns_ip: [u8; 4],
    ready: bool,
}

impl NetworkConfig {
    const EMPTY: Self = Self {
        mac: [0; 6],
        gateway_mac: [0; 6],
        local_ip: [0; 4],
        dns_ip: [0; 4],
        ready: false,
    };
}

#[derive(Clone, Copy)]
pub struct UdpDatagram {
    pub bytes: usize,
    pub source: [u8; 4],
    pub source_port: u16,
}

#[derive(Clone, Copy)]
pub struct PingReport {
    pub destination: [u8; 4],
    pub bytes: usize,
    pub elapsed_ns: u64,
    pub verified: bool,
}

#[derive(Clone, Copy)]
pub struct DnsLookup {
    pub server: [u8; 4],
    pub address: [u8; 4],
    pub answers: u16,
    pub verified: bool,
}

#[derive(Clone, Copy)]
pub struct HttpReport {
    pub connected: bool,
    pub received: bool,
    pub status: u16,
    pub bytes: usize,
}

#[derive(Clone, Copy)]
struct TcpSegment {
    sequence: u32,
    acknowledgement: u32,
    flags: u8,
    bytes: usize,
}

static CONFIG: TicketLock<NetworkConfig> = TicketLock::new(NetworkConfig::EMPTY);
static IPV4_ID: AtomicU64 = AtomicU64::new(0x4000);

#[derive(Clone, Copy)]
pub struct InternetReport {
    pub ipv4_tx: bool,
    pub ipv4_rx: bool,
    pub header_checksum: bool,
    pub icmp_checksum: bool,
    pub echo_reply: bool,
    pub reply_bytes: usize,
    pub udp_tx: bool,
    pub udp_rx: bool,
    pub udp_checksum: bool,
    pub dns_response: bool,
    pub dns_answers: u16,
    pub dns_address: [u8; 4],
    pub dns_verified: bool,
    pub dhcp_discover: bool,
    pub dhcp_request: bool,
    pub dhcp_ack: bool,
    pub local_ip: [u8; 4],
    pub gateway_ip: [u8; 4],
    pub dns_ip: [u8; 4],
    pub lease_seconds: u32,
    pub dhcp_verified: bool,
    pub verified: bool,
}

#[derive(Clone, Copy)]
struct DhcpLease {
    discover: bool,
    request: bool,
    ack: bool,
    address: [u8; 4],
    gateway: [u8; 4],
    dns: [u8; 4],
    lease_seconds: u32,
    verified: bool,
}

impl DhcpLease {
    const EMPTY: Self = Self {
        discover: false,
        request: false,
        ack: false,
        address: [0; 4],
        gateway: [0; 4],
        dns: [0; 4],
        lease_seconds: 0,
        verified: false,
    };
}

#[derive(Clone, Copy)]
struct DhcpReply {
    kind: u8,
    address: [u8; 4],
    gateway: [u8; 4],
    dns: [u8; 4],
    server: [u8; 4],
    lease_seconds: u32,
}

pub fn self_test(link: &NetworkReport) -> InternetReport {
    let lease = dhcp_test(link.mac);
    *CONFIG.lock() = NetworkConfig {
        mac: link.mac,
        gateway_mac: link.gateway_mac,
        local_ip: lease.address,
        dns_ip: lease.dns,
        ready: link.verified && lease.verified,
    };
    let local_ip = lease.address;
    let gateway_ip = lease.gateway;
    let mut packet = [0u8; 58];
    packet[..6].copy_from_slice(&link.gateway_mac);
    packet[6..12].copy_from_slice(&link.mac);
    packet[12..14].copy_from_slice(&[0x08, 0x00]);
    packet[14] = 0x45;
    packet[16..18].copy_from_slice(&(44u16).to_be_bytes());
    packet[18..20].copy_from_slice(&0xa3e0u16.to_be_bytes());
    packet[20..22].copy_from_slice(&0x4000u16.to_be_bytes());
    packet[22] = 64;
    packet[23] = 1;
    packet[26..30].copy_from_slice(&local_ip);
    packet[30..34].copy_from_slice(&gateway_ip);
    let ip_checksum = checksum(&packet[14..34]);
    packet[24..26].copy_from_slice(&ip_checksum.to_be_bytes());
    packet[34] = 8;
    packet[35] = 0;
    packet[38..40].copy_from_slice(&0x71d9u16.to_be_bytes());
    packet[40..42].copy_from_slice(&1u16.to_be_bytes());
    packet[42..].copy_from_slice(PAYLOAD);
    let icmp_checksum = checksum(&packet[34..]);
    packet[36..38].copy_from_slice(&icmp_checksum.to_be_bytes());
    let ipv4_tx = crate::nic::transmit(&packet);
    let mut ipv4_rx = false;
    let mut header_checksum = false;
    let mut icmp_valid = false;
    let mut echo_reply = false;
    let mut reply_bytes = 0;
    let mut received = [0u8; 2048];
    if ipv4_tx {
        for _ in 0..8 {
            let Some(length) = crate::nic::receive(&mut received) else {
                break;
            };
            reply_bytes = length;
            if length < 42
                || received[..6] != link.mac
                || received[6..12] != link.gateway_mac
                || received[12..14] != [0x08, 0x00]
                || received[14] >> 4 != 4
            {
                continue;
            }
            let header_bytes = (received[14] as usize & 0x0f) * 4;
            let total_bytes = u16::from_be_bytes([received[16], received[17]]) as usize;
            if header_bytes < 20
                || total_bytes < header_bytes + 8
                || 14 + total_bytes > length
                || received[23] != 1
                || received[26..30] != gateway_ip
                || received[30..34] != local_ip
                || u16::from_be_bytes([received[20], received[21]]) & 0x1fff != 0
            {
                continue;
            }
            ipv4_rx = true;
            header_checksum = checksum(&received[14..14 + header_bytes]) == 0;
            let icmp = &received[14 + header_bytes..14 + total_bytes];
            icmp_valid = checksum(icmp) == 0;
            echo_reply = icmp[0] == 0
                && icmp[1] == 0
                && icmp[4..6] == 0x71d9u16.to_be_bytes()
                && icmp[6..8] == 1u16.to_be_bytes()
                && icmp.get(8..) == Some(PAYLOAD.as_slice());
            if echo_reply {
                break;
            }
        }
    }
    let dns = dns_test();
    InternetReport {
        ipv4_tx,
        ipv4_rx,
        header_checksum,
        icmp_checksum: icmp_valid,
        echo_reply,
        reply_bytes,
        udp_tx: dns.udp_tx,
        udp_rx: dns.udp_rx,
        udp_checksum: dns.udp_checksum,
        dns_response: dns.response,
        dns_answers: dns.answers,
        dns_address: dns.address,
        dns_verified: dns.verified,
        dhcp_discover: lease.discover,
        dhcp_request: lease.request,
        dhcp_ack: lease.ack,
        local_ip,
        gateway_ip,
        dns_ip: lease.dns,
        lease_seconds: lease.lease_seconds,
        dhcp_verified: lease.verified,
        verified: lease.verified
            && ipv4_tx
            && ipv4_rx
            && header_checksum
            && icmp_valid
            && echo_reply,
    }
}

fn dhcp_test(mac: [u8; 6]) -> DhcpLease {
    let transaction = 0x4145_524fu32;
    let mut message = [0u8; 300];
    build_dhcp_message(&mut message, mac, transaction, 1, [0; 4], [0; 4]);
    let discover_sent = send_dhcp(&message, transaction as u16);
    let Some(offer) = discover_sent
        .then(|| receive_dhcp(mac, transaction, 2))
        .flatten()
    else {
        return DhcpLease::EMPTY;
    };
    build_dhcp_message(
        &mut message,
        mac,
        transaction,
        3,
        offer.address,
        offer.server,
    );
    let request_sent = send_dhcp(&message, transaction as u16 ^ 0x5a5a);
    let Some(ack) = request_sent
        .then(|| receive_dhcp(mac, transaction, 5))
        .flatten()
    else {
        return DhcpLease {
            discover: true,
            request: request_sent,
            ..DhcpLease::EMPTY
        };
    };
    let address = if ack.address == [0; 4] {
        offer.address
    } else {
        ack.address
    };
    let gateway = if ack.gateway == [0; 4] {
        offer.gateway
    } else {
        ack.gateway
    };
    let dns = if ack.dns == [0; 4] {
        offer.dns
    } else {
        ack.dns
    };
    let lease_seconds = ack.lease_seconds.max(offer.lease_seconds);
    let verified = ack.kind == 5
        && address == offer.address
        && valid_unicast(address)
        && valid_unicast(gateway)
        && valid_unicast(dns)
        && lease_seconds != 0;
    DhcpLease {
        discover: true,
        request: request_sent,
        ack: true,
        address,
        gateway,
        dns,
        lease_seconds,
        verified,
    }
}

fn build_dhcp_message(
    message: &mut [u8; 300],
    mac: [u8; 6],
    transaction: u32,
    kind: u8,
    requested: [u8; 4],
    server: [u8; 4],
) {
    message.fill(0);
    message[0] = 1;
    message[1] = 1;
    message[2] = 6;
    message[4..8].copy_from_slice(&transaction.to_be_bytes());
    message[10..12].copy_from_slice(&0x8000u16.to_be_bytes());
    message[28..34].copy_from_slice(&mac);
    message[236..240].copy_from_slice(&[99, 130, 83, 99]);
    let mut cursor = 240;
    push_dhcp_option(message, &mut cursor, 53, &[kind]);
    let mut client = [0u8; 7];
    client[0] = 1;
    client[1..].copy_from_slice(&mac);
    push_dhcp_option(message, &mut cursor, 61, &client);
    if kind == 3 {
        push_dhcp_option(message, &mut cursor, 50, &requested);
        push_dhcp_option(message, &mut cursor, 54, &server);
    }
    push_dhcp_option(message, &mut cursor, 55, &[1, 3, 6, 51, 54]);
    push_dhcp_option(message, &mut cursor, 57, &576u16.to_be_bytes());
    message[cursor] = 255;
}

fn push_dhcp_option(message: &mut [u8; 300], cursor: &mut usize, kind: u8, value: &[u8]) {
    if *cursor + value.len() + 3 > message.len() {
        return;
    }
    message[*cursor] = kind;
    message[*cursor + 1] = value.len() as u8;
    message[*cursor + 2..*cursor + 2 + value.len()].copy_from_slice(value);
    *cursor += value.len() + 2;
}

fn send_dhcp(message: &[u8; 300], identity: u16) -> bool {
    let mut packet = [0u8; 342];
    packet[..6].fill(0xff);
    packet[6..12].copy_from_slice(&message[28..34]);
    packet[12..14].copy_from_slice(&[0x08, 0x00]);
    packet[14] = 0x45;
    packet[16..18].copy_from_slice(&328u16.to_be_bytes());
    packet[18..20].copy_from_slice(&identity.to_be_bytes());
    packet[20..22].copy_from_slice(&0x4000u16.to_be_bytes());
    packet[22] = 64;
    packet[23] = 17;
    packet[30..34].fill(0xff);
    let ip_checksum = checksum(&packet[14..34]);
    packet[24..26].copy_from_slice(&ip_checksum.to_be_bytes());
    packet[34..36].copy_from_slice(&68u16.to_be_bytes());
    packet[36..38].copy_from_slice(&67u16.to_be_bytes());
    packet[38..40].copy_from_slice(&308u16.to_be_bytes());
    packet[42..].copy_from_slice(message);
    let computed = udp_checksum([0; 4], [255; 4], &packet[34..]);
    packet[40..42].copy_from_slice(&if computed == 0 { u16::MAX } else { computed }.to_be_bytes());
    crate::nic::transmit(&packet)
}

fn receive_dhcp(mac: [u8; 6], transaction: u32, expected_kind: u8) -> Option<DhcpReply> {
    let mut packet = [0u8; 2048];
    for _ in 0..12 {
        let length = crate::nic::receive(&mut packet)?;
        if length < 282
            || packet[..6] != mac && packet[..6] != [0xff; 6]
            || packet[12..14] != [0x08, 0x00]
            || packet[14] >> 4 != 4
        {
            continue;
        }
        let header_bytes = (packet[14] as usize & 0x0f) * 4;
        let total_bytes = u16::from_be_bytes([packet[16], packet[17]]) as usize;
        if header_bytes < 20
            || total_bytes < header_bytes + 248
            || 14 + total_bytes > length
            || packet[23] != 17
            || checksum(&packet[14..14 + header_bytes]) != 0
        {
            continue;
        }
        let udp = 14 + header_bytes;
        let udp_length = u16::from_be_bytes([packet[udp + 4], packet[udp + 5]]) as usize;
        if udp_length < 248
            || udp + udp_length > 14 + total_bytes
            || packet[udp..udp + 2] != 67u16.to_be_bytes()
            || packet[udp + 2..udp + 4] != 68u16.to_be_bytes()
        {
            continue;
        }
        let observed = u16::from_be_bytes([packet[udp + 6], packet[udp + 7]]);
        let source = [packet[26], packet[27], packet[28], packet[29]];
        let destination = [packet[30], packet[31], packet[32], packet[33]];
        if observed != 0 && udp_checksum(source, destination, &packet[udp..udp + udp_length]) != 0 {
            continue;
        }
        let message = &packet[udp + 8..udp + udp_length];
        if message.len() < 240
            || message[0] != 2
            || message[4..8] != transaction.to_be_bytes()
            || message[28..34] != mac
            || message[236..240] != [99, 130, 83, 99]
        {
            continue;
        }
        let mut reply = DhcpReply {
            kind: 0,
            address: [message[16], message[17], message[18], message[19]],
            gateway: [0; 4],
            dns: [0; 4],
            server: [0; 4],
            lease_seconds: 0,
        };
        let mut cursor = 240;
        while cursor < message.len() {
            let option = message[cursor];
            cursor += 1;
            if option == 255 {
                break;
            }
            if option == 0 {
                continue;
            }
            let size = *message.get(cursor)? as usize;
            cursor += 1;
            let end = cursor.checked_add(size)?;
            let value = message.get(cursor..end)?;
            match option {
                3 if size >= 4 => reply.gateway.copy_from_slice(&value[..4]),
                6 if size >= 4 => reply.dns.copy_from_slice(&value[..4]),
                51 if size == 4 => {
                    reply.lease_seconds = u32::from_be_bytes(value.try_into().ok()?);
                }
                53 if size == 1 => reply.kind = value[0],
                54 if size == 4 => reply.server.copy_from_slice(value),
                _ => {}
            }
            cursor = end;
        }
        if reply.kind == expected_kind {
            return Some(reply);
        }
    }
    None
}

fn valid_unicast(address: [u8; 4]) -> bool {
    address != [0; 4] && address != [255; 4] && address[0] < 224
}

/// Whether the interface is configured (link up, address leased).
pub fn is_ready() -> bool {
    CONFIG.lock().ready
}

pub fn send_udp(
    destination: [u8; 4],
    source_port: u16,
    destination_port: u16,
    payload: &[u8],
) -> bool {
    if payload.len() > MAX_UDP_PAYLOAD || source_port == 0 || destination_port == 0 {
        return false;
    }
    let config = *CONFIG.lock();
    if !config.ready {
        return false;
    }
    let packet_bytes = 42 + payload.len();
    let mut packet = [0u8; 1514];
    packet[..6].copy_from_slice(&config.gateway_mac);
    packet[6..12].copy_from_slice(&config.mac);
    packet[12..14].copy_from_slice(&[0x08, 0x00]);
    packet[14] = 0x45;
    packet[16..18].copy_from_slice(&((28 + payload.len()) as u16).to_be_bytes());
    let identity = IPV4_ID.fetch_add(1, Ordering::Relaxed) as u16;
    packet[18..20].copy_from_slice(&identity.to_be_bytes());
    packet[20..22].copy_from_slice(&0x4000u16.to_be_bytes());
    packet[22] = 64;
    packet[23] = 17;
    packet[26..30].copy_from_slice(&config.local_ip);
    packet[30..34].copy_from_slice(&destination);
    let ip_checksum = checksum(&packet[14..34]);
    packet[24..26].copy_from_slice(&ip_checksum.to_be_bytes());
    packet[34..36].copy_from_slice(&source_port.to_be_bytes());
    packet[36..38].copy_from_slice(&destination_port.to_be_bytes());
    packet[38..40].copy_from_slice(&((8 + payload.len()) as u16).to_be_bytes());
    packet[42..packet_bytes].copy_from_slice(payload);
    let computed = udp_checksum(config.local_ip, destination, &packet[34..packet_bytes]);
    packet[40..42].copy_from_slice(&if computed == 0 { u16::MAX } else { computed }.to_be_bytes());
    crate::nic::transmit(&packet[..packet_bytes])
}

pub fn receive_udp(
    source: [u8; 4],
    source_port: u16,
    destination_port: u16,
    payload: &mut [u8],
) -> Option<UdpDatagram> {
    if payload.len() > MAX_UDP_PAYLOAD || source_port == 0 || destination_port == 0 {
        return None;
    }
    let config = *CONFIG.lock();
    if !config.ready {
        return None;
    }
    let mut packet = [0u8; 2048];
    for _ in 0..12 {
        let length = crate::nic::receive(&mut packet)?;
        if length < 42
            || packet[..6] != config.mac
            || packet[12..14] != [0x08, 0x00]
            || packet[14] >> 4 != 4
        {
            continue;
        }
        let header_bytes = (packet[14] as usize & 0x0f) * 4;
        let total_bytes = u16::from_be_bytes([packet[16], packet[17]]) as usize;
        if header_bytes < 20
            || total_bytes < header_bytes + 8
            || 14 + total_bytes > length
            || packet[23] != 17
            || packet[26..30] != source
            || packet[30..34] != config.local_ip
            || u16::from_be_bytes([packet[20], packet[21]]) & 0x3fff != 0
            || checksum(&packet[14..14 + header_bytes]) != 0
        {
            continue;
        }
        let udp_start = 14 + header_bytes;
        let udp_length =
            u16::from_be_bytes([packet[udp_start + 4], packet[udp_start + 5]]) as usize;
        if udp_length < 8
            || udp_start + udp_length > 14 + total_bytes
            || packet[udp_start..udp_start + 2] != source_port.to_be_bytes()
            || packet[udp_start + 2..udp_start + 4] != destination_port.to_be_bytes()
        {
            continue;
        }
        let observed = u16::from_be_bytes([packet[udp_start + 6], packet[udp_start + 7]]);
        if observed != 0
            && udp_checksum(
                source,
                config.local_ip,
                &packet[udp_start..udp_start + udp_length],
            ) != 0
        {
            continue;
        }
        let bytes = udp_length - 8;
        if bytes > payload.len() {
            return None;
        }
        payload[..bytes].copy_from_slice(&packet[udp_start + 8..udp_start + udp_length]);
        return Some(UdpDatagram {
            bytes,
            source,
            source_port,
        });
    }
    None
}

struct DnsReport {
    udp_tx: bool,
    udp_rx: bool,
    udp_checksum: bool,
    response: bool,
    answers: u16,
    address: [u8; 4],
    verified: bool,
}

fn dns_test() -> DnsReport {
    let config = *CONFIG.lock();
    let udp_tx = send_udp(config.dns_ip, 49152, 53, DNS_QUERY);
    let mut received = [0u8; MAX_UDP_PAYLOAD];
    let datagram = udp_tx
        .then(|| receive_udp(config.dns_ip, 53, 49152, &mut received))
        .flatten();
    let udp_rx = datagram.is_some();
    let checksum_valid = udp_rx;
    let parsed = datagram
        .map(|datagram| parse_dns(&received[..datagram.bytes], 0xa311))
        .unwrap_or((false, 0, [0; 4]));
    DnsReport {
        udp_tx,
        udp_rx,
        udp_checksum: checksum_valid,
        response: parsed.0,
        answers: parsed.1,
        address: parsed.2,
        verified: udp_tx && udp_rx && checksum_valid && parsed.0 && parsed.1 != 0,
    }
}

pub fn ping(destination: [u8; 4]) -> PingReport {
    let config = *CONFIG.lock();
    if !config.ready || !valid_unicast(destination) {
        return PingReport {
            destination,
            bytes: 0,
            elapsed_ns: 0,
            verified: false,
        };
    }
    let sequence = IPV4_ID.fetch_add(1, Ordering::Relaxed) as u16;
    let identity = 0xaea0u16;
    let mut packet = [0u8; 58];
    packet[..6].copy_from_slice(&config.gateway_mac);
    packet[6..12].copy_from_slice(&config.mac);
    packet[12..14].copy_from_slice(&[0x08, 0x00]);
    packet[14] = 0x45;
    packet[16..18].copy_from_slice(&44u16.to_be_bytes());
    packet[18..20].copy_from_slice(&sequence.to_be_bytes());
    packet[20..22].copy_from_slice(&0x4000u16.to_be_bytes());
    packet[22] = 64;
    packet[23] = 1;
    packet[26..30].copy_from_slice(&config.local_ip);
    packet[30..34].copy_from_slice(&destination);
    let ip_checksum = checksum(&packet[14..34]);
    packet[24..26].copy_from_slice(&ip_checksum.to_be_bytes());
    packet[34] = 8;
    packet[38..40].copy_from_slice(&identity.to_be_bytes());
    packet[40..42].copy_from_slice(&sequence.to_be_bytes());
    packet[42..].copy_from_slice(PAYLOAD);
    let icmp_checksum = checksum(&packet[34..]);
    packet[36..38].copy_from_slice(&icmp_checksum.to_be_bytes());
    let start = crate::time::monotonic_nanoseconds();
    if !crate::nic::transmit(&packet) {
        return PingReport {
            destination,
            bytes: 0,
            elapsed_ns: 0,
            verified: false,
        };
    }
    let mut received = [0u8; 2048];
    for _ in 0..32 {
        let Some(length) = crate::nic::receive(&mut received) else {
            core::hint::spin_loop();
            continue;
        };
        if length < 42 || received[12..14] != [0x08, 0x00] || received[14] >> 4 != 4 {
            continue;
        }
        let header_bytes = (received[14] as usize & 0x0f) * 4;
        let total_bytes = u16::from_be_bytes([received[16], received[17]]) as usize;
        if header_bytes < 20
            || total_bytes < header_bytes + 8
            || 14 + total_bytes > length
            || received[23] != 1
            || received[26..30] != destination
            || received[30..34] != config.local_ip
            || checksum(&received[14..14 + header_bytes]) != 0
        {
            continue;
        }
        let icmp = &received[14 + header_bytes..14 + total_bytes];
        if checksum(icmp) == 0
            && icmp[0] == 0
            && icmp[4..6] == identity.to_be_bytes()
            && icmp[6..8] == sequence.to_be_bytes()
            && icmp.get(8..) == Some(PAYLOAD.as_slice())
        {
            return PingReport {
                destination,
                bytes: icmp.len().saturating_sub(8),
                elapsed_ns: crate::time::monotonic_nanoseconds().saturating_sub(start),
                verified: true,
            };
        }
    }
    PingReport {
        destination,
        bytes: 0,
        elapsed_ns: crate::time::monotonic_nanoseconds().saturating_sub(start),
        verified: false,
    }
}

pub fn resolve(name: &str) -> DnsLookup {
    let config = *CONFIG.lock();
    let mut lookup = DnsLookup {
        server: config.dns_ip,
        address: [0; 4],
        answers: 0,
        verified: false,
    };
    if !config.ready {
        return lookup;
    }
    let transaction = IPV4_ID.fetch_add(1, Ordering::Relaxed) as u16;
    let mut query = [0u8; 272];
    query[..2].copy_from_slice(&transaction.to_be_bytes());
    query[2..4].copy_from_slice(&0x0100u16.to_be_bytes());
    query[4..6].copy_from_slice(&1u16.to_be_bytes());
    let Some(mut cursor) = encode_dns_name(name, &mut query, 12) else {
        return lookup;
    };
    if cursor + 4 > query.len() {
        return lookup;
    }
    query[cursor..cursor + 2].copy_from_slice(&1u16.to_be_bytes());
    query[cursor + 2..cursor + 4].copy_from_slice(&1u16.to_be_bytes());
    cursor += 4;
    let source_port = 49_152 + transaction % 16_000;
    if !send_udp(config.dns_ip, source_port, 53, &query[..cursor]) {
        return lookup;
    }
    let mut response = [0u8; MAX_UDP_PAYLOAD];
    let Some(datagram) = receive_udp(config.dns_ip, 53, source_port, &mut response) else {
        return lookup;
    };
    let parsed = parse_dns(&response[..datagram.bytes], transaction);
    lookup.address = parsed.2;
    lookup.answers = parsed.1;
    lookup.verified = parsed.0 && parsed.1 != 0 && parsed.2 != [0; 4];
    lookup
}

pub fn http_get(address: [u8; 4], host: &str, path: &str, output: &mut [u8]) -> HttpReport {
    let mut report = HttpReport {
        connected: false,
        received: false,
        status: 0,
        bytes: 0,
    };
    if !valid_unicast(address)
        || host.is_empty()
        || host.len() > 128
        || path.is_empty()
        || path.len() > 96
        || !path.starts_with('/')
        || path.bytes().any(|byte| !byte.is_ascii_graphic())
        || output.is_empty()
    {
        return report;
    }
    let identity = IPV4_ID.fetch_add(1, Ordering::Relaxed);
    let source_port = 49_152 + identity as u16 % 16_000;
    let mut local_sequence = (identity as u32).wrapping_mul(0x9e37_79b9);
    if !send_tcp(address, source_port, 80, local_sequence, 0, 0x02, &[]) {
        return report;
    }
    let mut payload = [0u8; 1460];
    let Some(syn_ack) = wait_tcp(address, 80, source_port, &mut payload, 2_000_000_000) else {
        return report;
    };
    if syn_ack.flags & 0x12 != 0x12 || syn_ack.acknowledgement != local_sequence.wrapping_add(1) {
        return report;
    }
    local_sequence = local_sequence.wrapping_add(1);
    let mut remote_sequence = syn_ack.sequence.wrapping_add(1);
    if !send_tcp(
        address,
        source_port,
        80,
        local_sequence,
        remote_sequence,
        0x10,
        &[],
    ) {
        return report;
    }
    report.connected = true;
    let mut request = [0u8; 384];
    let prefix = b"GET ";
    let middle = b" HTTP/1.1\r\nHost: ";
    let suffix = b"\r\nConnection: close\r\nUser-Agent: AerOS/0.1\r\nAccept: text/html\r\n\r\n";
    let request_length = prefix.len() + path.len() + middle.len() + host.len() + suffix.len();
    if request_length > request.len() {
        return report;
    }
    let mut cursor = 0;
    for part in [
        prefix.as_slice(),
        path.as_bytes(),
        middle.as_slice(),
        host.as_bytes(),
        suffix.as_slice(),
    ] {
        request[cursor..cursor + part.len()].copy_from_slice(part);
        cursor += part.len();
    }
    if !send_tcp(
        address,
        source_port,
        80,
        local_sequence,
        remote_sequence,
        0x18,
        &request[..request_length],
    ) {
        return report;
    }
    local_sequence = local_sequence.wrapping_add(request_length as u32);
    let deadline = crate::time::monotonic_nanoseconds().saturating_add(3_000_000_000);
    while crate::time::monotonic_nanoseconds() < deadline && report.bytes < output.len() {
        let Some(segment) = wait_tcp(address, 80, source_port, &mut payload, 200_000_000) else {
            continue;
        };
        if segment.sequence == remote_sequence && segment.bytes != 0 {
            let count = segment.bytes.min(output.len() - report.bytes);
            output[report.bytes..report.bytes + count].copy_from_slice(&payload[..count]);
            report.bytes += count;
            remote_sequence = remote_sequence.wrapping_add(segment.bytes as u32);
            report.received = true;
        }
        if segment.flags & 0x01 != 0 {
            remote_sequence = remote_sequence.wrapping_add(1);
        }
        let _ = send_tcp(
            address,
            source_port,
            80,
            local_sequence,
            remote_sequence,
            0x10,
            &[],
        );
        if segment.flags & 0x05 != 0 || report.bytes == output.len() {
            break;
        }
    }
    report.status = http_status(&output[..report.bytes]);
    let _ = send_tcp(
        address,
        source_port,
        80,
        local_sequence,
        remote_sequence,
        0x11,
        &[],
    );
    report
}

fn send_tcp(
    destination: [u8; 4],
    source_port: u16,
    destination_port: u16,
    sequence: u32,
    acknowledgement: u32,
    flags: u8,
    payload: &[u8],
) -> bool {
    if payload.len() > 1460 {
        return false;
    }
    let config = *CONFIG.lock();
    if !config.ready {
        return false;
    }
    let mut packet = [0u8; 1514];
    let tcp_length = 20 + payload.len();
    let packet_length = 34 + tcp_length;
    packet[..6].copy_from_slice(&config.gateway_mac);
    packet[6..12].copy_from_slice(&config.mac);
    packet[12..14].copy_from_slice(&[0x08, 0x00]);
    packet[14] = 0x45;
    packet[16..18].copy_from_slice(&((20 + tcp_length) as u16).to_be_bytes());
    packet[18..20].copy_from_slice(&(IPV4_ID.fetch_add(1, Ordering::Relaxed) as u16).to_be_bytes());
    packet[20..22].copy_from_slice(&0x4000u16.to_be_bytes());
    packet[22] = 64;
    packet[23] = 6;
    packet[26..30].copy_from_slice(&config.local_ip);
    packet[30..34].copy_from_slice(&destination);
    let ip_checksum = checksum(&packet[14..34]);
    packet[24..26].copy_from_slice(&ip_checksum.to_be_bytes());
    packet[34..36].copy_from_slice(&source_port.to_be_bytes());
    packet[36..38].copy_from_slice(&destination_port.to_be_bytes());
    packet[38..42].copy_from_slice(&sequence.to_be_bytes());
    packet[42..46].copy_from_slice(&acknowledgement.to_be_bytes());
    packet[46] = 5 << 4;
    packet[47] = flags;
    packet[48..50].copy_from_slice(&64_240u16.to_be_bytes());
    packet[54..packet_length].copy_from_slice(payload);
    let calculated =
        transport_checksum(config.local_ip, destination, 6, &packet[34..packet_length]);
    packet[50..52].copy_from_slice(&calculated.to_be_bytes());
    crate::nic::transmit(&packet[..packet_length])
}

fn wait_tcp(
    source: [u8; 4],
    source_port: u16,
    destination_port: u16,
    payload: &mut [u8],
    timeout_ns: u64,
) -> Option<TcpSegment> {
    let config = *CONFIG.lock();
    let deadline = crate::time::monotonic_nanoseconds().saturating_add(timeout_ns);
    let mut packet = [0u8; 2048];
    while crate::time::monotonic_nanoseconds() < deadline {
        let Some(length) = crate::nic::receive(&mut packet) else {
            core::hint::spin_loop();
            continue;
        };
        if length < 54
            || packet[..6] != config.mac
            || packet[12..14] != [0x08, 0x00]
            || packet[14] >> 4 != 4
            || packet[23] != 6
            || packet[26..30] != source
            || packet[30..34] != config.local_ip
        {
            continue;
        }
        let ip_bytes = (packet[14] as usize & 0x0f) * 4;
        let total_bytes = u16::from_be_bytes([packet[16], packet[17]]) as usize;
        let tcp_start = 14 + ip_bytes;
        if ip_bytes < 20
            || total_bytes < ip_bytes + 20
            || 14 + total_bytes > length
            || tcp_start + 20 > length
            || u16::from_be_bytes([packet[20], packet[21]]) & 0x3fff != 0
            || packet[tcp_start..tcp_start + 2] != source_port.to_be_bytes()
            || packet[tcp_start + 2..tcp_start + 4] != destination_port.to_be_bytes()
            || checksum(&packet[14..14 + ip_bytes]) != 0
            || transport_checksum(
                source,
                config.local_ip,
                6,
                &packet[tcp_start..14 + total_bytes],
            ) != 0
        {
            continue;
        }
        let tcp_bytes = (packet[tcp_start + 12] as usize >> 4) * 4;
        if tcp_bytes < 20 || tcp_start + tcp_bytes > 14 + total_bytes {
            continue;
        }
        let bytes = total_bytes - ip_bytes - tcp_bytes;
        if bytes > payload.len() {
            return None;
        }
        payload[..bytes]
            .copy_from_slice(&packet[tcp_start + tcp_bytes..tcp_start + tcp_bytes + bytes]);
        return Some(TcpSegment {
            sequence: u32::from_be_bytes(packet[tcp_start + 4..tcp_start + 8].try_into().ok()?),
            acknowledgement: u32::from_be_bytes(
                packet[tcp_start + 8..tcp_start + 12].try_into().ok()?,
            ),
            flags: packet[tcp_start + 13],
            bytes,
        });
    }
    None
}

fn http_status(response: &[u8]) -> u16 {
    if response.len() < 12 || !response.starts_with(b"HTTP/1.") {
        return 0;
    }
    let digits = &response[9..12];
    if !digits.iter().all(u8::is_ascii_digit) {
        return 0;
    }
    ((digits[0] - b'0') as u16) * 100 + ((digits[1] - b'0') as u16) * 10 + (digits[2] - b'0') as u16
}

fn encode_dns_name(name: &str, output: &mut [u8], mut cursor: usize) -> Option<usize> {
    if name.is_empty() || name.len() > 253 || name.starts_with('.') || name.ends_with('.') {
        return None;
    }
    for label in name.split('.') {
        if label.is_empty()
            || label.len() > 63
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return None;
        }
        *output.get_mut(cursor)? = label.len() as u8;
        cursor += 1;
        let end = cursor.checked_add(label.len())?;
        output
            .get_mut(cursor..end)?
            .copy_from_slice(label.as_bytes());
        cursor = end;
    }
    *output.get_mut(cursor)? = 0;
    Some(cursor + 1)
}

fn parse_dns(packet: &[u8], transaction: u16) -> (bool, u16, [u8; 4]) {
    if packet.len() < 12
        || packet[..2] != transaction.to_be_bytes()
        || packet[2] & 0x80 == 0
        || packet[3] & 0x0f != 0
        || u16::from_be_bytes([packet[4], packet[5]]) != 1
    {
        return (false, 0, [0; 4]);
    }
    let answers = u16::from_be_bytes([packet[6], packet[7]]);
    let Some(question_end) = skip_dns_name(packet, 12).and_then(|offset| offset.checked_add(4))
    else {
        return (false, 0, [0; 4]);
    };
    if question_end > packet.len() {
        return (false, 0, [0; 4]);
    }
    let mut cursor = question_end;
    let mut address = [0u8; 4];
    for _ in 0..answers {
        let Some(name_end) = skip_dns_name(packet, cursor) else {
            return (false, 0, [0; 4]);
        };
        cursor = name_end;
        if cursor + 10 > packet.len() {
            return (false, 0, [0; 4]);
        }
        let kind = u16::from_be_bytes([packet[cursor], packet[cursor + 1]]);
        let class = u16::from_be_bytes([packet[cursor + 2], packet[cursor + 3]]);
        let length = u16::from_be_bytes([packet[cursor + 8], packet[cursor + 9]]) as usize;
        cursor += 10;
        if cursor + length > packet.len() {
            return (false, 0, [0; 4]);
        }
        if kind == 1 && class == 1 && length == 4 {
            address.copy_from_slice(&packet[cursor..cursor + 4]);
        }
        cursor += length;
    }
    (true, answers, address)
}

fn skip_dns_name(packet: &[u8], mut cursor: usize) -> Option<usize> {
    loop {
        let length = *packet.get(cursor)? as usize;
        cursor += 1;
        if length == 0 {
            return Some(cursor);
        }
        if length & 0xc0 == 0xc0 {
            packet.get(cursor)?;
            return Some(cursor + 1);
        }
        if length > 63 {
            return None;
        }
        cursor = cursor.checked_add(length)?;
        if cursor > packet.len() {
            return None;
        }
    }
}

fn udp_checksum(source: [u8; 4], destination: [u8; 4], packet: &[u8]) -> u16 {
    transport_checksum(source, destination, 17, packet)
}

fn transport_checksum(source: [u8; 4], destination: [u8; 4], protocol: u8, packet: &[u8]) -> u16 {
    let mut sum = u16::from_be_bytes([source[0], source[1]]) as u32
        + u16::from_be_bytes([source[2], source[3]]) as u32
        + u16::from_be_bytes([destination[0], destination[1]]) as u32
        + u16::from_be_bytes([destination[2], destination[3]]) as u32
        + protocol as u32
        + packet.len() as u32;
    let mut chunks = packet.chunks_exact(2);
    for chunk in &mut chunks {
        sum = sum.wrapping_add(u16::from_be_bytes([chunk[0], chunk[1]]) as u32);
    }
    if let Some(byte) = chunks.remainder().first() {
        sum = sum.wrapping_add((*byte as u32) << 8);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn checksum(bytes: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut chunks = bytes.chunks_exact(2);
    for chunk in &mut chunks {
        sum = sum.wrapping_add(u16::from_be_bytes([chunk[0], chunk[1]]) as u32);
    }
    if let Some(byte) = chunks.remainder().first() {
        sum = sum.wrapping_add((*byte as u32) << 8);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}
