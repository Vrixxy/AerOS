//! Realtek RTL8111/8168/8169 gigabit Ethernet (the NIC on most PC
//! motherboards and many laptops), polled, descriptor rings in memory.
//!
//! QEMU has no model of this chip, so unlike the other NIC drivers this one
//! has only been checked against the datasheet-level programming sequence
//! (reset, ring setup, receiver/transmitter enable), not against a device.

use crate::memory::FrameAllocator;
use crate::nic::{NicReport, arp_probe};
use crate::pci::PciInventory;
use crate::sync::TicketLock;

const VENDOR: u16 = 0x10ec;
const DEVICES: [u16; 4] = [0x8168, 0x8161, 0x8169, 0x8167];

const IDR0: usize = 0x00;
const MAR0: usize = 0x08;
const TNPDS: usize = 0x20;
const CR: usize = 0x37;
const TPPOLL: usize = 0x38;
const TCR: usize = 0x40;
const RCR: usize = 0x44;
const CFG9346: usize = 0x50;
const PHY_STATUS: usize = 0x6c;
const RMS: usize = 0xda;
const CPLUS: usize = 0xe0;
const RDSAR: usize = 0xe4;
const MTPS: usize = 0xec;

const CR_RESET: u8 = 0x10;
const CR_RX_ENABLE: u8 = 0x08;
const CR_TX_ENABLE: u8 = 0x04;

const DESC_OWN: u32 = 1 << 31;
const DESC_EOR: u32 = 1 << 30;
const DESC_FS: u32 = 1 << 29;
const DESC_LS: u32 = 1 << 28;
const RX_ERROR: u32 = 1 << 21;

const RING: usize = 16;
const BUFFER: usize = 2048;

#[repr(C)]
#[derive(Clone, Copy)]
struct Descriptor {
    flags: u32,
    vlan: u32,
    address: u64,
}

struct State {
    mmio: u64,
    tx_ring: u64,
    rx_ring: u64,
    tx_buffers: u64,
    rx_buffers: u64,
    tx_index: usize,
    rx_index: usize,
    #[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
    mac: [u8; 6],
    ready: bool,
}

static NIC: TicketLock<State> = TicketLock::new(State {
    mmio: 0,
    tx_ring: 0,
    rx_ring: 0,
    tx_buffers: 0,
    rx_buffers: 0,
    tx_index: 0,
    rx_index: 0,
    mac: [0; 6],
    ready: false,
});

fn read8(base: u64, offset: usize) -> u8 {
    unsafe { core::ptr::read_volatile((base as usize + offset) as *const u8) }
}

fn read16(base: u64, offset: usize) -> u16 {
    unsafe { core::ptr::read_volatile((base as usize + offset) as *const u16) }
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

fn descriptor(ring: u64, index: usize) -> *mut Descriptor {
    (ring as usize + index * core::mem::size_of::<Descriptor>()) as *mut Descriptor
}

fn arm_receive(ring: u64, buffers: u64, index: usize) {
    let last = if index == RING - 1 { DESC_EOR } else { 0 };
    unsafe {
        core::ptr::write_volatile(
            descriptor(ring, index),
            Descriptor {
                flags: DESC_OWN | last | BUFFER as u32,
                vlan: 0,
                address: buffers + (index * BUFFER) as u64,
            },
        );
    }
}

pub fn initialize(pci: &PciInventory, frames: &mut FrameAllocator) -> NicReport {
    let Some(device) = pci
        .devices()
        .iter()
        .copied()
        .find(|device| device.vendor == VENDOR && DEVICES.contains(&device.device))
    else {
        return NicReport::EMPTY;
    };
    // The register window is the first memory BAR (BAR2 on PCIe parts).
    let mut mmio = 0u64;
    let mut index = 0usize;
    while index < 6 {
        let bar = device.bars[index];
        if bar & 1 == 0 && bar & 0xffff_fff0 != 0 {
            mmio = (bar & 0xffff_fff0) as u64;
            if bar & 0x6 == 0x4 && index + 1 < 6 {
                mmio |= (device.bars[index + 1] as u64) << 32;
            }
            break;
        }
        index += if bar & 0x6 == 0x4 { 2 } else { 1 };
    }
    if mmio == 0 || !pci.enable_memory_bus_master(device) {
        return NicReport::EMPTY;
    }
    let mut report = NicReport {
        present: true,
        mmio,
        ..NicReport::EMPTY
    };
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
    for (offset, byte) in mac.iter_mut().enumerate() {
        *byte = read8(mmio, IDR0 + offset);
    }
    report.mac = mac;
    // Rings (256-byte aligned) and buffers in one DMA block.
    let pages = (2 * RING * BUFFER / 4096 + 2) as u64;
    let Some(dma) = frames.allocate_contiguous(pages, 1) else {
        return report;
    };
    let base = dma.address();
    unsafe { core::ptr::write_bytes(base as usize as *mut u8, 0, (pages * 4096) as usize) };
    let tx_ring = base;
    let rx_ring = base + 1024;
    let tx_buffers = base + 4096;
    let rx_buffers = tx_buffers + (RING * BUFFER) as u64;
    for slot in 0..RING {
        arm_receive(rx_ring, rx_buffers, slot);
    }

    // Unlock the configuration registers, program the rings, enable the
    // receiver and transmitter, then lock again.
    write8(mmio, CFG9346, 0xc0);
    write16(mmio, RMS, 0x1fff);
    write8(mmio, MTPS, 0x3b);
    let plus = read16(mmio, CPLUS);
    write16(mmio, CPLUS, plus);
    write32(mmio, TNPDS, tx_ring as u32);
    write32(mmio, TNPDS + 4, (tx_ring >> 32) as u32);
    write32(mmio, RDSAR, rx_ring as u32);
    write32(mmio, RDSAR + 4, (rx_ring >> 32) as u32);
    write8(mmio, CR, CR_RX_ENABLE | CR_TX_ENABLE);
    write32(mmio, TCR, 3 << 24 | 7 << 8);
    // Broadcast, multicast, our unicast address; no FIFO threshold; unlimited DMA bursts.
    write32(mmio, RCR, 0x0e | 7 << 13 | 7 << 8);
    for offset in 0..8 {
        write8(mmio, MAR0 + offset, 0xff);
    }
    write8(mmio, CFG9346, 0x00);
    report.link = read8(mmio, PHY_STATUS) & 0x02 != 0;
    *NIC.lock() = State {
        mmio,
        tx_ring,
        rx_ring,
        tx_buffers,
        rx_buffers,
        tx_index: 0,
        rx_index: 0,
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
    let slot = descriptor(state.tx_ring, index);
    if unsafe { core::ptr::read_volatile(slot) }.flags & DESC_OWN != 0 {
        return false;
    }
    let buffer = state.tx_buffers + (index * BUFFER) as u64;
    let padded = packet.len().max(60);
    unsafe {
        core::ptr::write_bytes(buffer as usize as *mut u8, 0, padded);
        core::ptr::copy_nonoverlapping(packet.as_ptr(), buffer as usize as *mut u8, packet.len());
    }
    let last = if index == RING - 1 { DESC_EOR } else { 0 };
    unsafe {
        core::ptr::write_volatile(
            slot,
            Descriptor {
                flags: DESC_OWN | last | DESC_FS | DESC_LS | padded as u32,
                vlan: 0,
                address: buffer,
            },
        );
    }
    write8(state.mmio, TPPOLL, 0x40);
    state.tx_index = (index + 1) % RING;
    for _ in 0..5_000_000 {
        if unsafe { core::ptr::read_volatile(slot) }.flags & DESC_OWN == 0 {
            return true;
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
    let index = state.rx_index;
    let slot = descriptor(state.rx_ring, index);
    let current = unsafe { core::ptr::read_volatile(slot) };
    if current.flags & DESC_OWN != 0 {
        return None;
    }
    // Length includes the 4-byte CRC.
    let length = (current.flags & 0x3fff) as usize;
    let payload = length.saturating_sub(4);
    let usable = current.flags & RX_ERROR == 0 && payload <= destination.len() && payload >= 14;
    if usable {
        unsafe {
            core::ptr::copy_nonoverlapping(
                current.address as usize as *const u8,
                destination.as_mut_ptr(),
                payload,
            );
        }
    }
    arm_receive(state.rx_ring, state.rx_buffers, index);
    state.rx_index = (index + 1) % RING;
    usable.then_some(payload)
}

#[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
pub fn mac_address() -> Option<[u8; 6]> {
    let state = NIC.lock();
    state.ready.then_some(state.mac)
}
