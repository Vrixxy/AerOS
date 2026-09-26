use core::hint::spin_loop;
use core::sync::atomic::{Ordering, compiler_fence};

use crate::memory::FrameAllocator;
use crate::pci::PciInventory;
use crate::sync::TicketLock;

const PAGE_SIZE: u64 = 4096;
const DESCRIPTORS: usize = 8;
const BUFFER_SIZE: usize = 2048;
const WAIT_LIMIT: usize = 40_000_000;
const CTRL: usize = 0x0000;
const STATUS: usize = 0x0008;
const ICR: usize = 0x00c0;
const IMC: usize = 0x00d8;
const RCTL: usize = 0x0100;
const TCTL: usize = 0x0400;
const TIPG: usize = 0x0410;
const RDBAL: usize = 0x2800;
const RDBAH: usize = 0x2804;
const RDLEN: usize = 0x2808;
const RDH: usize = 0x2810;
const RDT: usize = 0x2818;
const TDBAL: usize = 0x3800;
const TDBAH: usize = 0x3804;
const TDLEN: usize = 0x3808;
const TDH: usize = 0x3810;
const TDT: usize = 0x3818;
const RAL: usize = 0x5400;
const RAH: usize = 0x5404;

#[repr(C)]
#[derive(Clone, Copy)]
struct ReceiveDescriptor {
    address: u64,
    length: u16,
    checksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TransmitDescriptor {
    address: u64,
    length: u16,
    checksum_offset: u8,
    command: u8,
    status: u8,
    checksum_start: u8,
    special: u16,
}

#[derive(Clone, Copy)]
pub struct NetworkReport {
    pub driver: &'static str,
    pub present: bool,
    pub mmio: u64,
    pub mac: [u8; 6],
    pub gateway_mac: [u8; 6],
    pub link: bool,
    pub full_duplex: bool,
    pub speed_mbps: u32,
    pub tx: bool,
    pub rx: bool,
    pub rx_bytes: u16,
    pub arp_reply: bool,
    pub verified: bool,
}

impl NetworkReport {
    /// The link report for a NIC other than the Intel one, when it is the
    /// interface the network stack ends up using.
    pub fn from_nic(nic: &crate::nic::NicReport, driver: &'static str) -> Self {
        Self {
            driver,
            present: nic.present,
            mmio: nic.mmio,
            mac: nic.mac,
            gateway_mac: nic.gateway_mac,
            link: nic.link,
            full_duplex: true,
            speed_mbps: 100,
            tx: nic.tx,
            rx: nic.arp_reply,
            rx_bytes: 60,
            arp_reply: nic.arp_reply,
            verified: nic.verified,
        }
    }

    const EMPTY: Self = Self {
        driver: "none",
        present: false,
        mmio: 0,
        mac: [0; 6],
        gateway_mac: [0; 6],
        link: false,
        full_duplex: false,
        speed_mbps: 0,
        tx: false,
        rx: false,
        rx_bytes: 0,
        arp_reply: false,
        verified: false,
    };
}

struct NetworkState {
    mmio: u64,
    transmit_ring: u64,
    receive_ring: u64,
    transmit_buffers: u64,
    receive_buffers: u64,
    transmit_tail: usize,
    receive_head: usize,
    ready: bool,
    mac: [u8; 6],
}

impl NetworkState {
    const EMPTY: Self = Self {
        mmio: 0,
        transmit_ring: 0,
        receive_ring: 0,
        transmit_buffers: 0,
        receive_buffers: 0,
        transmit_tail: 0,
        receive_head: 0,
        ready: false,
        mac: [0; 6],
    };
}

static NETWORK: TicketLock<NetworkState> = TicketLock::new(NetworkState::EMPTY);

pub fn initialize(pci: &PciInventory, frames: &mut FrameAllocator) -> NetworkReport {
    let Some(device) = pci.find_class(0x02, 0x00, 0x00) else {
        return NetworkReport::EMPTY;
    };
    if device.vendor != 0x8086
        || !matches!(device.device, 0x100e | 0x100f | 0x10d3)
        || device.bars[0] & 1 != 0
        || device.bars[0] & 0xffff_fff0 == 0
        || !pci.enable_memory_bus_master(device)
    {
        return NetworkReport::EMPTY;
    }
    let mmio = (device.bars[0] & 0xffff_fff0) as u64;
    let address_low = unsafe { read_register(mmio, RAL) };
    let address_high = unsafe { read_register(mmio, RAH) };
    let mac = [
        address_low as u8,
        (address_low >> 8) as u8,
        (address_low >> 16) as u8,
        (address_low >> 24) as u8,
        address_high as u8,
        (address_high >> 8) as u8,
    ];
    if address_high & (1 << 31) == 0
        || mac.iter().all(|byte| *byte == 0)
        || mac.iter().all(|byte| *byte == 0xff)
    {
        return NetworkReport::EMPTY;
    }
    let Some(memory) = frames.allocate_contiguous(9, 1) else {
        return NetworkReport::EMPTY;
    };
    let descriptor_page = memory.address();
    let transmit_ring = descriptor_page;
    let receive_ring = descriptor_page + 1024;
    let receive_buffers = descriptor_page + PAGE_SIZE;
    let transmit_buffer = descriptor_page + PAGE_SIZE * 5;
    unsafe {
        core::ptr::write_bytes(
            descriptor_page as usize as *mut u8,
            0,
            (PAGE_SIZE * 9) as usize,
        );
    }
    for index in 0..DESCRIPTORS {
        let descriptor = ReceiveDescriptor {
            address: receive_buffers + (index * BUFFER_SIZE) as u64,
            length: 0,
            checksum: 0,
            status: 0,
            errors: 0,
            special: 0,
        };
        unsafe {
            core::ptr::write_volatile(
                (receive_ring as usize as *mut ReceiveDescriptor).add(index),
                descriptor,
            );
        }
    }
    let status = unsafe { read_register(mmio, STATUS) };
    let link = status & 2 != 0;
    let full_duplex = status & 1 != 0;
    let speed_mbps = match status >> 6 & 3 {
        0 => 10,
        1 => 100,
        2 | 3 => 1000,
        _ => 0,
    };
    unsafe {
        write_register(mmio, IMC, u32::MAX);
        let _ = read_register(mmio, ICR);
        write_register(mmio, TDBAL, transmit_ring as u32);
        write_register(mmio, TDBAH, (transmit_ring >> 32) as u32);
        write_register(
            mmio,
            TDLEN,
            (DESCRIPTORS * core::mem::size_of::<TransmitDescriptor>()) as u32,
        );
        write_register(mmio, TDH, 0);
        write_register(mmio, TDT, 0);
        write_register(mmio, TIPG, 10 | (8 << 10) | (6 << 20));
        write_register(mmio, TCTL, (1 << 1) | (1 << 3) | (0x10 << 4) | (0x40 << 12));
        write_register(mmio, RDBAL, receive_ring as u32);
        write_register(mmio, RDBAH, (receive_ring >> 32) as u32);
        write_register(
            mmio,
            RDLEN,
            (DESCRIPTORS * core::mem::size_of::<ReceiveDescriptor>()) as u32,
        );
        write_register(mmio, RDH, 0);
        write_register(mmio, RDT, (DESCRIPTORS - 1) as u32);
        write_register(mmio, RCTL, (1 << 1) | (1 << 15) | (1 << 26));
        let control = read_register(mmio, CTRL);
        write_register(mmio, CTRL, control | (1 << 6));
    }
    let frame = unsafe { core::slice::from_raw_parts_mut(transmit_buffer as usize as *mut u8, 60) };
    frame[..6].fill(0xff);
    frame[6..12].copy_from_slice(&mac);
    frame[12..14].copy_from_slice(&[0x08, 0x06]);
    frame[14..16].copy_from_slice(&[0x00, 0x01]);
    frame[16..18].copy_from_slice(&[0x08, 0x00]);
    frame[18] = 6;
    frame[19] = 4;
    frame[20..22].copy_from_slice(&[0x00, 0x01]);
    frame[22..28].copy_from_slice(&mac);
    frame[28..32].copy_from_slice(&[10, 0, 2, 15]);
    frame[32..38].fill(0);
    frame[38..42].copy_from_slice(&[10, 0, 2, 2]);
    frame[42..].fill(0);
    let transmit = TransmitDescriptor {
        address: transmit_buffer,
        length: frame.len() as u16,
        checksum_offset: 0,
        command: 0x0b,
        status: 0,
        checksum_start: 0,
        special: 0,
    };
    unsafe {
        core::ptr::write_volatile(transmit_ring as usize as *mut TransmitDescriptor, transmit);
    }
    compiler_fence(Ordering::SeqCst);
    unsafe {
        write_register(mmio, TDT, 1);
    }
    let mut tx = false;
    for _ in 0..WAIT_LIMIT {
        let descriptor = unsafe {
            core::ptr::read_volatile(transmit_ring as usize as *const TransmitDescriptor)
        };
        if descriptor.status & 1 != 0 {
            tx = true;
            break;
        }
        spin_loop();
    }
    let mut rx = false;
    let mut rx_bytes = 0u16;
    let mut arp_reply = false;
    let mut gateway_mac = [0u8; 6];
    for _ in 0..WAIT_LIMIT {
        let descriptor =
            unsafe { core::ptr::read_volatile(receive_ring as usize as *const ReceiveDescriptor) };
        if descriptor.status & 1 != 0 {
            rx = descriptor.errors == 0 && descriptor.length >= 42;
            rx_bytes = descriptor.length;
            if rx {
                let packet = unsafe {
                    core::slice::from_raw_parts(
                        receive_buffers as usize as *const u8,
                        rx_bytes as usize,
                    )
                };
                arp_reply = packet.get(0..6) == Some(mac.as_slice())
                    && packet.get(12..14) == Some([0x08, 0x06].as_slice())
                    && packet.get(20..22) == Some([0x00, 0x02].as_slice())
                    && packet.get(28..32) == Some([10, 0, 2, 2].as_slice());
                if arp_reply {
                    gateway_mac.copy_from_slice(&packet[6..12]);
                }
            }
            break;
        }
        spin_loop();
    }
    let verified = link && tx && rx && arp_reply;
    if verified {
        unsafe {
            (*(receive_ring as usize as *mut ReceiveDescriptor)).status = 0;
            compiler_fence(Ordering::SeqCst);
            write_register(mmio, RDT, 0);
        }
        *NETWORK.lock() = NetworkState {
            mmio,
            transmit_ring,
            receive_ring,
            transmit_buffers: transmit_buffer,
            receive_buffers,
            transmit_tail: 1,
            receive_head: 1,
            ready: true,
            mac,
        };
    }
    NetworkReport {
        driver: match device.device {
            0x10d3 => "e1000e",
            _ => "e1000",
        },
        present: true,
        mmio,
        mac,
        gateway_mac,
        link,
        full_duplex,
        speed_mbps,
        tx,
        rx,
        rx_bytes,
        arp_reply,
        verified,
    }
}

pub fn transmit(packet: &[u8]) -> bool {
    if !(14..=1514).contains(&packet.len()) {
        return false;
    }
    let mut network = NETWORK.lock();
    if !network.ready {
        return false;
    }
    let index = network.transmit_tail;
    let descriptor_address = network.transmit_ring + index as u64 * 16;
    let previous = unsafe {
        core::ptr::read_volatile(descriptor_address as usize as *const TransmitDescriptor)
    };
    if previous.address != 0 && previous.status & 1 == 0 {
        return false;
    }
    let buffer = network.transmit_buffers + (index * BUFFER_SIZE) as u64;
    unsafe {
        core::ptr::copy_nonoverlapping(packet.as_ptr(), buffer as usize as *mut u8, packet.len());
        if packet.len() < 60 {
            core::ptr::write_bytes(
                (buffer as usize as *mut u8).add(packet.len()),
                0,
                60 - packet.len(),
            );
        }
    }
    let descriptor = TransmitDescriptor {
        address: buffer,
        length: packet.len().max(60) as u16,
        checksum_offset: 0,
        command: 0x0b,
        status: 0,
        checksum_start: 0,
        special: 0,
    };
    unsafe {
        core::ptr::write_volatile(
            descriptor_address as usize as *mut TransmitDescriptor,
            descriptor,
        );
    }
    compiler_fence(Ordering::SeqCst);
    network.transmit_tail = (index + 1) % DESCRIPTORS;
    unsafe {
        write_register(network.mmio, TDT, network.transmit_tail as u32);
    }
    for _ in 0..WAIT_LIMIT {
        let observed = unsafe {
            core::ptr::read_volatile(descriptor_address as usize as *const TransmitDescriptor)
        };
        if observed.status & 1 != 0 {
            return true;
        }
        spin_loop();
    }
    false
}

/// The NIC's own MAC address, once the driver is up.
#[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
pub fn mac_address() -> Option<[u8; 6]> {
    let network = NETWORK.lock();
    network.ready.then_some(network.mac)
}

/// Non-blocking `receive`: looks at the next RX descriptor once and returns
/// immediately when nothing has arrived (the blocking variant spins for tens
/// of millions of iterations on an idle link, which would stall a guest).
#[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
pub fn try_receive(destination: &mut [u8]) -> Option<usize> {
    let mut network = NETWORK.lock();
    if !network.ready {
        return None;
    }
    let index = network.receive_head;
    let descriptor_address = network.receive_ring + index as u64 * 16;
    let descriptor = unsafe {
        core::ptr::read_volatile(descriptor_address as usize as *const ReceiveDescriptor)
    };
    if descriptor.status & 1 == 0 {
        return None;
    }
    let count = descriptor.length as usize;
    let usable = descriptor.errors == 0 && count <= destination.len();
    if usable {
        let buffer = network.receive_buffers + (index * BUFFER_SIZE) as u64;
        unsafe {
            core::ptr::copy_nonoverlapping(
                buffer as usize as *const u8,
                destination.as_mut_ptr(),
                count,
            );
        }
    }
    // Consume the descriptor either way so a bad frame can't wedge the ring.
    unsafe {
        (*(descriptor_address as usize as *mut ReceiveDescriptor)).status = 0;
    }
    compiler_fence(Ordering::SeqCst);
    unsafe {
        write_register(network.mmio, RDT, index as u32);
    }
    network.receive_head = (index + 1) % DESCRIPTORS;
    usable.then_some(count)
}

pub fn receive(destination: &mut [u8]) -> Option<usize> {
    let mut network = NETWORK.lock();
    if !network.ready {
        return None;
    }
    let index = network.receive_head;
    let descriptor_address = network.receive_ring + index as u64 * 16;
    for _ in 0..WAIT_LIMIT {
        let descriptor = unsafe {
            core::ptr::read_volatile(descriptor_address as usize as *const ReceiveDescriptor)
        };
        if descriptor.status & 1 != 0 {
            if descriptor.errors != 0 || descriptor.length as usize > destination.len() {
                return None;
            }
            let count = descriptor.length as usize;
            let buffer = network.receive_buffers + (index * BUFFER_SIZE) as u64;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    buffer as usize as *const u8,
                    destination.as_mut_ptr(),
                    count,
                );
                (*(descriptor_address as usize as *mut ReceiveDescriptor)).status = 0;
            }
            compiler_fence(Ordering::SeqCst);
            unsafe {
                write_register(network.mmio, RDT, index as u32);
            }
            network.receive_head = (index + 1) % DESCRIPTORS;
            return Some(count);
        }
        spin_loop();
    }
    None
}

unsafe fn read_register(base: u64, offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base as usize + offset) as *const u32) }
}

unsafe fn write_register(base: u64, offset: usize, value: u32) {
    unsafe {
        core::ptr::write_volatile((base as usize + offset) as *mut u32, value);
    }
}
