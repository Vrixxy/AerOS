//! A small stateless packet filter checked once, centrally, inside
//! `nic::try_receive`/`nic::receive` - every existing networking self-test
//! and driver already funnels through one of those two functions (confirmed
//! by reading `nic.rs`: `transmit`/`try_receive` are the only points that
//! dispatch to whichever NIC driver is active), so filtering there applies
//! uniformly without touching any of them individually. Default-deny-empty:
//! with no rules installed (the boot-time default, and the state every
//! existing self-test runs under), `blocks()`'s very first check returns
//! `false` before parsing a single byte - behavior is identical to before
//! this existed unless something explicitly calls `add_rule`.
//!
//! Deliberately simple: destination protocol (TCP/UDP/any) plus destination
//! port (or any port), IPv4 only, no stateful connection tracking, no NAT,
//! no outbound filtering (`transmit` is untouched - a real firewall often
//! filters both directions, but this kernel's own outbound traffic is
//! trusted code, not something that needs blocking from itself). A dropped
//! packet is indistinguishable from one that simply hasn't arrived yet to
//! every existing caller (`try_receive` returning `None`), so no caller's
//! own bounded-retry loop needs to know this exists.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::sync::TicketLock;

const MAX_RULES: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Tcp,
    Udp,
    Any,
}

#[derive(Clone, Copy)]
struct Rule {
    used: bool,
    protocol: Protocol,
    /// Destination port to match, or 0 for "any port".
    port: u16,
}

impl Rule {
    const EMPTY: Self = Self {
        used: false,
        protocol: Protocol::Any,
        port: 0,
    };
}

static RULES: TicketLock<[Rule; MAX_RULES]> = TicketLock::new([Rule::EMPTY; MAX_RULES]);
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// Adds a DENY rule (destination `protocol`, destination `port` or 0 for
/// any port). Returns false if the rule table is full.
pub fn add_rule(protocol: Protocol, port: u16) -> bool {
    let mut rules = RULES.lock();
    let Some(slot) = rules.iter().position(|rule| !rule.used) else {
        return false;
    };
    rules[slot] = Rule {
        used: true,
        protocol,
        port,
    };
    true
}

pub fn clear_rules() {
    *RULES.lock() = [Rule::EMPTY; MAX_RULES];
}

pub fn rule_count() -> usize {
    RULES.lock().iter().filter(|rule| rule.used).count()
}

pub fn dropped_count() -> u64 {
    DROPPED.load(Ordering::Relaxed)
}

/// Destination (protocol, port) of an IPv4 TCP/UDP frame - `None` for
/// anything else (ARP, IPv6, other IP protocols, or a frame too short to
/// hold a full header), which `blocks()` always lets through: this filter
/// only ever matches what it can confidently parse, never guesses.
fn parse_destination(packet: &[u8]) -> Option<(Protocol, u16)> {
    if packet.len() < 34 || packet[12..14] != [0x08, 0x00] {
        return None;
    }
    let ip_bytes = (packet[14] as usize & 0x0f) * 4;
    if ip_bytes < 20 || packet.len() < 14 + ip_bytes + 4 {
        return None;
    }
    let protocol = match packet[23] {
        6 => Protocol::Tcp,
        17 => Protocol::Udp,
        _ => return None,
    };
    let transport_start = 14 + ip_bytes;
    // Destination port sits at the same +2 byte offset in both the TCP and
    // UDP header.
    let port = u16::from_be_bytes([packet[transport_start + 2], packet[transport_start + 3]]);
    Some((protocol, port))
}

/// True if `packet` (a raw Ethernet frame, exactly what `nic::try_receive`
/// hands back) matches a DENY rule and should be dropped before any caller
/// ever sees it.
pub fn blocks(packet: &[u8]) -> bool {
    let rules = RULES.lock();
    if rules.iter().all(|rule| !rule.used) {
        return false;
    }
    let Some((protocol, port)) = parse_destination(packet) else {
        return false;
    };
    let blocked = rules.iter().any(|rule| {
        rule.used
            && (rule.protocol == Protocol::Any || rule.protocol == protocol)
            && (rule.port == 0 || rule.port == port)
    });
    if blocked {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
    blocked
}

#[derive(Clone, Copy)]
pub struct FirewallReport {
    pub empty_passes: bool,
    pub port_match_blocks: bool,
    pub port_mismatch_passes: bool,
    pub protocol_mismatch_passes: bool,
    pub any_port_blocks_all: bool,
    pub cleared_passes_again: bool,
    pub dropped_counted: bool,
    pub verified: bool,
}

/// Builds a minimal, valid Ethernet+IPv4+TCP/UDP frame (header-only, no
/// payload) for testing `parse_destination`/`blocks()` against known,
/// hand-verified byte offsets, without needing any real NIC traffic. 14
/// (Ethernet) + 20 (IPv4, no options) + 4 (enough to reach the destination
/// port, at +2 in both TCP's and UDP's header) = 38 bytes.
fn build_frame(protocol: u8, destination_port: u16) -> [u8; 38] {
    let mut frame = [0u8; 38];
    frame[12] = 0x08;
    frame[13] = 0x00; // EtherType IPv4
    frame[14] = 0x45; // IPv4, 20-byte header
    frame[23] = protocol;
    frame[36..38].copy_from_slice(&destination_port.to_be_bytes());
    frame
}

pub fn self_test() -> FirewallReport {
    clear_rules();
    let tcp_80 = build_frame(6, 80);
    let tcp_81 = build_frame(6, 81);
    let udp_80 = build_frame(17, 80);
    let arp = [0u8; 38]; // EtherType 0x0000, matches no IPv4 branch at all

    let empty_passes = !blocks(&tcp_80) && !blocks(&udp_80) && !blocks(&arp);

    let before_dropped = dropped_count();
    let rule_added = add_rule(Protocol::Tcp, 80);
    let port_match_blocks = rule_added && blocks(&tcp_80);
    let port_mismatch_passes = !blocks(&tcp_81);
    let protocol_mismatch_passes = !blocks(&udp_80);
    let dropped_counted = dropped_count() == before_dropped + 1;

    clear_rules();
    let any_port_blocks_all = {
        let rule_added = add_rule(Protocol::Any, 0);
        // `arp` is deliberately excluded: `parse_destination` never
        // recognizes it as TCP/UDP at all, so no rule - however broad - can
        // ever match it. That is by design (see `parse_destination`'s doc
        // comment), not something an "any protocol, any port" rule
        // overrides.
        rule_added && blocks(&tcp_80) && blocks(&tcp_81) && blocks(&udp_80) && !blocks(&arp)
    };

    clear_rules();
    let cleared_passes_again = rule_count() == 0 && !blocks(&tcp_80);

    FirewallReport {
        empty_passes,
        port_match_blocks,
        port_mismatch_passes,
        protocol_mismatch_passes,
        any_port_blocks_all,
        cleared_passes_again,
        dropped_counted,
        verified: empty_passes
            && port_match_blocks
            && port_mismatch_passes
            && protocol_mismatch_passes
            && any_port_blocks_all
            && cleared_passes_again
            && dropped_counted,
    }
}
