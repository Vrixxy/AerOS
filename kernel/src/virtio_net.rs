//! Virtio network device (legacy transport, polled): raw Ethernet frames.

use crate::memory::FrameAllocator;
use crate::pci::PciInventory;
use crate::sync::TicketLock;
use crate::virtio::{DESC_WRITE, Legacy, Queue, REG_CONFIG};

const DEVICE_ID: u16 = 0x1000;
const FEATURE_MAC: u32 = 1 << 5;
const HEADER: usize = 10;
const BUFFER: usize = 2048;
const RX_BUFFERS: usize = 16;

#[derive(Clone, Copy)]
pub struct VirtioNetReport {
    pub present: bool,
    pub io: u16,
    pub mac: [u8; 6],
    pub arp_reply: bool,
    pub verified: bool,
}

impl VirtioNetReport {
    pub const EMPTY: Self = Self {
        present: false,
        io: 0,
        mac: [0; 6],
        arp_reply: false,
        verified: false,
    };
}

struct NetState {
    device: Option<Legacy>,
    receive: Queue,
    transmit: Queue,
    rx_buffers: u64,
    tx_buffer: u64,
}

static NET: TicketLock<NetState> = TicketLock::new(NetState {
    device: None,
    receive: Queue::EMPTY,
    transmit: Queue::EMPTY,
    rx_buffers: 0,
    tx_buffer: 0,
});

impl NetState {
    fn post_receive(&mut self, index: usize) {
        let address = self.rx_buffers + (index * BUFFER) as u64;
        self.receive
            .descriptor(index, address, BUFFER as u32, DESC_WRITE, 0);
        self.receive.submit(index as u16);
    }
}

/// Sends one Ethernet frame.
pub fn send(frame: &[u8]) -> bool {
    let mut state = NET.lock();
    let Some(device) = state.device else {
        return false;
    };
    if frame.is_empty() || frame.len() + HEADER > BUFFER {
        return false;
    }
    let buffer = state.tx_buffer;
    unsafe {
        core::ptr::write_bytes(buffer as usize as *mut u8, 0, HEADER);
        core::ptr::copy_nonoverlapping(
            frame.as_ptr(),
            (buffer as usize + HEADER) as *mut u8,
            frame.len(),
        );
    }
    state
        .transmit
        .descriptor(0, buffer, (HEADER + frame.len()) as u32, 0, 0);
    state.transmit.submit(0);
    device.notify(1);
    state.transmit.wait_used().is_some()
}

/// Copies the next received frame into `out` (returns its length).
pub fn receive(out: &mut [u8]) -> Option<usize> {
    let mut state = NET.lock();
    state.device?;
    let (id, length) = state.receive.pop_used()?;
    let length = length as usize;
    let index = id as usize;
    let copied = if length > HEADER && index < RX_BUFFERS {
        let frame = (length - HEADER).min(out.len());
        let source = state.rx_buffers + (index * BUFFER + HEADER) as u64;
        unsafe {
            core::ptr::copy_nonoverlapping(source as usize as *const u8, out.as_mut_ptr(), frame)
        };
        Some(frame)
    } else {
        None
    };
    if index < RX_BUFFERS {
        state.post_receive(index);
        if let Some(device) = state.device {
            device.notify(0);
        }
    }
    copied
}

pub fn initialize(pci: &PciInventory, frames: &mut FrameAllocator) -> VirtioNetReport {
    let Some((_, device)) = Legacy::find(pci, DEVICE_ID) else {
        return VirtioNetReport::EMPTY;
    };
    let mut report = VirtioNetReport {
        present: true,
        io: device.io,
        ..VirtioNetReport::EMPTY
    };
    let accepted = device.begin(FEATURE_MAC);
    let (Some(receive_queue), Some(transmit)) = (device.queue(0, frames), device.queue(1, frames))
    else {
        return report;
    };
    let pages = (RX_BUFFERS * BUFFER) as u64 / 4096 + 1;
    let Some(dma) = frames.allocate_contiguous(pages, 1) else {
        return report;
    };
    device.driver_ok();
    if accepted & FEATURE_MAC != 0 {
        for (index, byte) in report.mac.iter_mut().enumerate() {
            *byte = device.read8(REG_CONFIG + index as u16);
        }
    }
    let mut state = NetState {
        device: Some(device),
        receive: receive_queue,
        transmit,
        rx_buffers: dma.address(),
        tx_buffer: dma.address() + (RX_BUFFERS * BUFFER) as u64,
    };
    for index in 0..RX_BUFFERS {
        state.post_receive(index);
    }
    device.notify(0);
    *NET.lock() = state;

    // Self-test: ask for the gateway's MAC (ARP) and wait for the answer
    // from QEMU's user-mode network (the test setup gives this NIC 10.9.0.0/24).
    let mut request = [0u8; 42];
    request[..6].fill(0xff);
    request[6..12].copy_from_slice(&report.mac);
    request[12..14].copy_from_slice(&0x0806u16.to_be_bytes());
    request[14..16].copy_from_slice(&1u16.to_be_bytes());
    request[16..18].copy_from_slice(&0x0800u16.to_be_bytes());
    request[18] = 6;
    request[19] = 4;
    request[20..22].copy_from_slice(&1u16.to_be_bytes());
    request[22..28].copy_from_slice(&report.mac);
    request[28..32].copy_from_slice(&[10, 9, 0, 15]);
    request[38..42].copy_from_slice(&[10, 9, 0, 2]);
    let sent = send(&request);
    let start = crate::time::monotonic_nanoseconds();
    let mut frame = [0u8; 256];
    while sent
        && !report.arp_reply
        && crate::time::monotonic_nanoseconds().saturating_sub(start) < 1_500_000_000
    {
        if let Some(length) = receive(&mut frame)
            && length >= 42
            && frame[12..14] == 0x0806u16.to_be_bytes()
            && frame[20..22] == 2u16.to_be_bytes()
            && frame[28..32] == [10, 9, 0, 2]
        {
            report.arp_reply = true;
        }
    }
    report.verified = sent && report.arp_reply && report.mac != [0; 6];
    report
}
