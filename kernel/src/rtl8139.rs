//! Realtek RTL8139 Fast Ethernet (polled, memory-mapped registers).

use crate::memory::FrameAllocator;
use crate::nic::{NicReport, arp_probe};
use crate::pci::PciInventory;
use crate::sync::TicketLock;

const VENDOR: u16 = 0x10ec;
const DEVICE: u16 = 0x8139;

const IDR0: usize = 0x00;
const TSD0: usize = 0x10;
const TSAD0: usize = 0x20;
const RBSTART: usize = 0x30;
const CR: usize = 0x37;
const CAPR: usize = 0x38;
const IMR: usize = 0x3c;
const ISR: usize = 0x3e;
const TCR: usize = 0x40;
const RCR: usize = 0x44;
const CONFIG1: usize = 0x52;
const BMSR: usize = 0x64;

const CR_RESET: u8 = 0x10;
const CR_RX_ENABLE: u8 = 0x08;
const CR_TX_ENABLE: u8 = 0x04;
const CR_BUFFER_EMPTY: u8 = 0x01;

const TSD_TOK: u32 = 1 << 15;
/// The receive ring is 8 KiB plus 16 bytes of header slack; with WRAP set the
/// chip writes a packet that crosses the end contiguously, so a further
/// 1.5 KiB (rounded up to pages) follows it.
const RX_RING: usize = 8192;
const RX_BUFFER_PAGES: u64 = 4;
const TX_BUFFER: usize = 2048;

struct State {
    mmio: u64,
    rx: u64,
    tx: u64,
    rx_offset: usize,
    tx_index: usize,
    #[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
    mac: [u8; 6],
    ready: bool,
}

static NIC: TicketLock<State> = TicketLock::new(State {
    mmio: 0,
    rx: 0,
    tx: 0,
    rx_offset: 0,
    tx_index: 0,
    mac: [0; 6],
    ready: false,
});

fn read8(base: u64, offset: usize) -> u8 {
    unsafe { core::ptr::read_volatile((base as usize + offset) as *const u8) }
}

fn read16(base: u64, offset: usize) -> u16 {
    unsafe { core::ptr::read_volatile((base as usize + offset) as *const u16) }
}

fn read32(base: u64, offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base as usize + offset) as *const u32) }
}

fn write8(base: u64, offset: usize, value: u8) {
    unsafe { core::ptr::write_volatile((base as usize + offset) as *mut u8, value) }
}

fn write16(base: u64, offset: usize, value: u16) {
    unsafe { core::ptr::write_volatile((base as usize + offset) as *mut u16, value) }
}

fn write32(base: u64, offset: usize, value: u32) {
    unsafe { core::ptr::write_volatile((base as usize + offset) as *mut u32, value) }
}

pub fn initialize(pci: &PciInventory, frames: &mut FrameAllocator) -> NicReport {
    let Some(device) = pci
        .devices()
        .iter()
        .copied()
        .find(|device| device.vendor == VENDOR && device.device == DEVICE)
    else {
        return NicReport::EMPTY;
    };
    // BAR1 is the memory-mapped register window.
    let bar = device.bars[1];
    if bar & 1 != 0 || bar & 0xffff_fff0 == 0 || !pci.enable_memory_bus_master(device) {
        return NicReport::EMPTY;
    }
    let mmio = (bar & 0xffff_fff0) as u64;
    let mut report = NicReport {
        present: true,
        mmio,
        ..NicReport::EMPTY
    };
    // Power on and reset.
    write8(mmio, CONFIG1, 0);
    write8(mmio, CR, CR_RESET);
    let mut waited = 0u32;
    while read8(mmio, CR) & CR_RESET != 0 {
        waited += 1;
        if waited > 10_000_000 {
            return report;
        }
        core::hint::spin_loop();
    }
    let mut mac = [0u8; 6];
    for (index, byte) in mac.iter_mut().enumerate() {
        *byte = read8(mmio, IDR0 + index);
    }
    report.mac = mac;
    // The ring and the four transmit buffers must sit below 4 GiB.
    let Some(dma) = frames.allocate_contiguous(RX_BUFFER_PAGES + 2, 1) else {
        return report;
    };
    let rx = dma.address();
    let tx = rx + RX_BUFFER_PAGES * 4096;
    if tx + 2 * 4096 > 1 << 32 {
        return report;
    }
    unsafe {
        core::ptr::write_bytes(
            rx as usize as *mut u8,
            0,
            ((RX_BUFFER_PAGES + 2) * 4096) as usize,
        )
    };
    write32(mmio, RBSTART, rx as u32);
    write16(mmio, IMR, 0);
    write16(mmio, ISR, 0xffff);
    // Accept broadcast, multicast and our unicast address; WRAP; 8K ring;
    // unlimited DMA bursts.
    write32(mmio, RCR, 0x0e | 1 << 7 | 7 << 8);
    write32(mmio, TCR, 3 << 24 | 7 << 8);
    write8(mmio, CR, CR_RX_ENABLE | CR_TX_ENABLE);
    report.link = read16(mmio, BMSR) & 4 != 0;
    *NIC.lock() = State {
        mmio,
        rx,
        tx,
        rx_offset: 0,
        tx_index: 0,
        mac,
        ready: true,
    };
    let gateway = arp_probe(mac, transmit, try_receive);
    report.tx = true;
    report.arp_reply = gateway.is_some();
    let (gateway_ip, gateway_mac) = gateway.unwrap_or(([0; 4], [0; 6]));
    report.gateway = gateway_ip;
    report.gateway_mac = gateway_mac;
    report.verified = report.arp_reply && mac != [0; 6];
    report
}

pub fn transmit(packet: &[u8]) -> bool {
    if !(14..=1514).contains(&packet.len()) {
        return false;
    }
    let mut state = NIC.lock();
    if !state.ready {
        return false;
    }
    let index = state.tx_index;
    let buffer = state.tx + (index * TX_BUFFER) as u64;
    let padded = packet.len().max(60);
    unsafe {
        core::ptr::write_bytes(buffer as usize as *mut u8, 0, padded);
        core::ptr::copy_nonoverlapping(packet.as_ptr(), buffer as usize as *mut u8, packet.len());
    }
    write32(state.mmio, TSAD0 + index * 4, buffer as u32);
    // Writing the length (with OWN clear) starts the transfer.
    write32(state.mmio, TSD0 + index * 4, padded as u32);
    state.tx_index = (index + 1) % 4;
    for _ in 0..5_000_000 {
        let status = read32(state.mmio, TSD0 + index * 4);
        if status & TSD_TOK != 0 {
            return true;
        }
        // Bit 30: transmit aborted.
        if status & 0x4000_0000 != 0 {
            return false;
        }
        core::hint::spin_loop();
    }
    false
}

pub fn try_receive(destination: &mut [u8]) -> Option<usize> {
    let mut state = NIC.lock();
    if !state.ready {
        return None;
    }
    if read8(state.mmio, CR) & CR_BUFFER_EMPTY != 0 {
        return None;
    }
    let base = state.rx + state.rx_offset as u64;
    let header = unsafe { core::ptr::read_volatile(base as usize as *const u32) };
    let status = header as u16;
    // Length includes the 4-byte CRC.
    let length = (header >> 16) as usize;
    let good = status & 1 != 0 && (18..=1792).contains(&length);
    let payload = length.saturating_sub(4);
    let copied = if good && payload <= destination.len() {
        unsafe {
            core::ptr::copy_nonoverlapping(
                (base + 4) as usize as *const u8,
                destination.as_mut_ptr(),
                payload,
            );
        }
        Some(payload)
    } else {
        None
    };
    if !good {
        // A damaged ring position: restart the receiver rather than guess.
        write8(state.mmio, CR, CR_TX_ENABLE);
        state.rx_offset = 0;
        unsafe { core::ptr::write_bytes(state.rx as usize as *mut u8, 0, RX_RING) };
        write32(state.mmio, RBSTART, state.rx as u32);
        write16(state.mmio, CAPR, 0xfff0);
        write8(state.mmio, CR, CR_RX_ENABLE | CR_TX_ENABLE);
        return None;
    }
    state.rx_offset = (state.rx_offset + length + 4 + 3) & !3;
    if state.rx_offset >= RX_RING {
        state.rx_offset -= RX_RING;
    }
    write16(state.mmio, CAPR, (state.rx_offset as u16).wrapping_sub(16));
    write16(state.mmio, ISR, 0xffff);
    copied
}

#[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
pub fn mac_address() -> Option<[u8; 6]> {
    let state = NIC.lock();
    state.ready.then_some(state.mac)
}
