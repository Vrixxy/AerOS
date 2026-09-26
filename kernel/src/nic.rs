//! Shared pieces for the simple Ethernet drivers (Realtek and friends): the
//! ARP self-test they all run, and the facade the network stack uses to reach
//! whichever NIC came up (`e1000` first, then the others).

use core::sync::atomic::{AtomicU8, Ordering};

use crate::{e1000, rtl8139, rtl8168, virtio_net};

/// What a secondary NIC driver reports after probing.
#[derive(Clone, Copy)]
pub struct NicReport {
    pub present: bool,
    pub mmio: u64,
    pub mac: [u8; 6],
    pub link: bool,
    pub tx: bool,
    pub arp_reply: bool,
    /// Gateway that answered the ARP probe.
    pub gateway: [u8; 4],
    pub gateway_mac: [u8; 6],
    pub verified: bool,
}

impl NicReport {
    pub const EMPTY: Self = Self {
        present: false,
        mmio: 0,
        mac: [0; 6],
        link: false,
        tx: false,
        arp_reply: false,
        gateway: [0; 4],
        gateway_mac: [0; 6],
        verified: false,
    };
}

/// The gateways QEMU's user-mode networks hand out (the default 10.0.2.0/24
/// and the extra ones the test setup gives its secondary NICs).
const GATEWAYS: [[u8; 4]; 3] = [[10, 0, 2, 2], [10, 9, 0, 2], [10, 10, 0, 2]];

/// Asks each candidate gateway for its MAC (ARP) and waits for an answer:
/// proves a NIC can both transmit and receive. Returns the gateway (IP, MAC) that answered.
pub fn arp_probe(
    mac: [u8; 6],
    mut send: impl FnMut(&[u8]) -> bool,
    mut receive: impl FnMut(&mut [u8]) -> Option<usize>,
) -> Option<([u8; 4], [u8; 6])> {
    let mut frame = [0u8; 256];
    for gateway in GATEWAYS {
        let mut request = [0u8; 60];
        request[..6].fill(0xff);
        request[6..12].copy_from_slice(&mac);
        request[12..14].copy_from_slice(&0x0806u16.to_be_bytes());
        request[14..16].copy_from_slice(&1u16.to_be_bytes());
        request[16..18].copy_from_slice(&0x0800u16.to_be_bytes());
        request[18] = 6;
        request[19] = 4;
        request[20..22].copy_from_slice(&1u16.to_be_bytes());
        request[22..28].copy_from_slice(&mac);
        request[28..32].copy_from_slice(&[gateway[0], gateway[1], gateway[2], 15]);
        request[38..42].copy_from_slice(&gateway);
        if !send(&request) {
            continue;
        }
        let start = crate::time::monotonic_nanoseconds();
        while crate::time::monotonic_nanoseconds().saturating_sub(start) < 400_000_000 {
            if let Some(length) = receive(&mut frame)
                && length >= 42
                && frame[12..14] == 0x0806u16.to_be_bytes()
                && frame[20..22] == 2u16.to_be_bytes()
                && frame[28..32] == gateway
            {
                let mut gateway_mac = [0u8; 6];
                gateway_mac.copy_from_slice(&frame[6..12]);
                return Some((gateway, gateway_mac));
            }
        }
    }
    None
}

const NONE: u8 = 0;
const E1000: u8 = 1;
const RTL8139: u8 = 2;
const RTL8168: u8 = 3;
const VIRTIO: u8 = 4;

static PRIMARY: AtomicU8 = AtomicU8::new(NONE);

/// Picks the NIC the network stack (which lives on 10.0.2.0/24) talks to.
pub fn select_primary(e1000_ready: bool, rtl8139: &NicReport, rtl8168: &NicReport) {
    let default_net = |report: &NicReport| report.verified && report.gateway == [10, 0, 2, 2];
    let choice = if e1000_ready {
        E1000
    } else if default_net(rtl8139) {
        RTL8139
    } else if default_net(rtl8168) {
        RTL8168
    } else {
        NONE
    };
    PRIMARY.store(choice, Ordering::Release);
}

pub fn transmit(packet: &[u8]) -> bool {
    match PRIMARY.load(Ordering::Acquire) {
        E1000 => e1000::transmit(packet),
        RTL8139 => rtl8139::transmit(packet),
        RTL8168 => rtl8168::transmit(packet),
        VIRTIO => virtio_net::send(packet),
        _ => false,
    }
}

/// Non-blocking receive.
pub fn try_receive(destination: &mut [u8]) -> Option<usize> {
    match PRIMARY.load(Ordering::Acquire) {
        E1000 => e1000::try_receive(destination),
        RTL8139 => rtl8139::try_receive(destination),
        RTL8168 => rtl8168::try_receive(destination),
        VIRTIO => virtio_net::receive(destination),
        _ => None,
    }
}

/// Receive that waits (bounded) for a frame.
pub fn receive(destination: &mut [u8]) -> Option<usize> {
    if PRIMARY.load(Ordering::Acquire) == E1000 {
        return e1000::receive(destination);
    }
    for _ in 0..40_000_000u32 {
        if let Some(length) = try_receive(destination) {
            return Some(length);
        }
        core::hint::spin_loop();
    }
    None
}

#[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
pub fn mac_address() -> Option<[u8; 6]> {
    match PRIMARY.load(Ordering::Acquire) {
        E1000 => e1000::mac_address(),
        RTL8139 => rtl8139::mac_address(),
        RTL8168 => rtl8168::mac_address(),
        _ => None,
    }
}
