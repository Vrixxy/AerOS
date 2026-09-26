use crate::{arch, serial};

const MAX_DEVICES: usize = 256;

#[derive(Clone, Copy)]
pub struct PciDevice {
    pub bus: u8,
    pub slot: u8,
    pub function: u8,
    pub vendor: u16,
    pub device: u16,
    pub class: u8,
    pub subclass: u8,
    pub programming_interface: u8,
    pub revision: u8,
    pub header_type: u8,
    pub interrupt_line: u8,
    pub bars: [u32; 6],
    pub capability_count: u8,
}

impl PciDevice {
    const EMPTY: Self = Self {
        bus: 0,
        slot: 0,
        function: 0,
        vendor: 0xffff,
        device: 0xffff,
        class: 0xff,
        subclass: 0xff,
        programming_interface: 0,
        revision: 0,
        header_type: 0,
        interrupt_line: 0xff,
        bars: [0; 6],
        capability_count: 0,
    };
}

#[derive(Clone, Copy)]
pub struct PciSummary {
    pub devices: usize,
    pub dropped: usize,
    pub storage: usize,
    pub network: usize,
    pub display: usize,
    pub usb: usize,
    pub bridges: usize,
    pub virtio: usize,
    pub capabilities: usize,
    pub verified: bool,
}

pub struct PciInventory {
    devices: [PciDevice; MAX_DEVICES],
    count: usize,
    dropped: usize,
}

impl PciInventory {
    pub fn scan() -> Self {
        let mut inventory = Self {
            devices: [PciDevice::EMPTY; MAX_DEVICES],
            count: 0,
            dropped: 0,
        };
        for bus in 0u16..=255 {
            for slot in 0u8..32 {
                let vendor = read16(bus as u8, slot, 0, 0);
                if vendor == 0xffff {
                    continue;
                }
                let header = read8(bus as u8, slot, 0, 0x0e);
                inventory.capture(bus as u8, slot, 0);
                if header & 0x80 != 0 {
                    for function in 1u8..8 {
                        if read16(bus as u8, slot, function, 0) != 0xffff {
                            inventory.capture(bus as u8, slot, function);
                        }
                    }
                }
            }
        }
        inventory
    }

    pub fn summary(&self) -> PciSummary {
        let mut summary = PciSummary {
            devices: self.count,
            dropped: self.dropped,
            storage: 0,
            network: 0,
            display: 0,
            usb: 0,
            bridges: 0,
            virtio: 0,
            capabilities: 0,
            verified: self.count != 0 && self.dropped == 0,
        };
        for device in &self.devices[..self.count] {
            summary.storage += usize::from(device.class == 0x01);
            summary.network += usize::from(device.class == 0x02);
            summary.display += usize::from(device.class == 0x03);
            summary.usb += usize::from(device.class == 0x0c && device.subclass == 0x03);
            summary.bridges += usize::from(device.class == 0x06 && device.subclass == 0x04);
            summary.virtio += usize::from(device.vendor == 0x1af4);
            summary.capabilities += device.capability_count as usize;
        }
        for left in 0..self.count {
            for right in left + 1..self.count {
                let a = self.devices[left];
                let b = self.devices[right];
                if a.bus == b.bus && a.slot == b.slot && a.function == b.function {
                    summary.verified = false;
                }
            }
        }
        summary
    }

    pub fn find_class(&self, class: u8, subclass: u8, interface: u8) -> Option<PciDevice> {
        self.devices[..self.count].iter().copied().find(|device| {
            device.class == class
                && device.subclass == subclass
                && device.programming_interface == interface
        })
    }

    pub fn devices(&self) -> &[PciDevice] {
        &self.devices[..self.count]
    }

    pub fn enable_memory_bus_master(&self, device: PciDevice) -> bool {
        let command = read16(device.bus, device.slot, device.function, 0x04);
        write16(
            device.bus,
            device.slot,
            device.function,
            0x04,
            command | 0x0006,
        );
        let observed = read16(device.bus, device.slot, device.function, 0x04);
        observed & 0x0006 == 0x0006
    }

    /// I/O space + bus mastering, for legacy port-I/O devices such as AC'97.
    pub fn enable_io_bus_master(&self, device: PciDevice) -> bool {
        let command = read16(device.bus, device.slot, device.function, 0x04);
        write16(
            device.bus,
            device.slot,
            device.function,
            0x04,
            command | 0x0005,
        );
        let observed = read16(device.bus, device.slot, device.function, 0x04);
        observed & 0x0005 == 0x0005
    }

    pub fn log_devices(&self) {
        for entry in &self.devices[..self.count] {
            serial::format(format_args!(
                "AEROS_PCI_DEVICE bdf={:02x}:{:02x}.{} id={:04x}:{:04x} class={:02x}:{:02x}:{:02x} rev={:02x} header={:02x} irq={} caps={} bar0={:#x} bar1={:#x} bar5={:#x}\n",
                entry.bus,
                entry.slot,
                entry.function,
                entry.vendor,
                entry.device,
                entry.class,
                entry.subclass,
                entry.programming_interface,
                entry.revision,
                entry.header_type,
                entry.interrupt_line,
                entry.capability_count,
                entry.bars[0],
                entry.bars[1],
                entry.bars[5]
            ));
        }
    }

    fn capture(&mut self, bus: u8, slot: u8, function: u8) {
        if self.count == MAX_DEVICES {
            self.dropped += 1;
            return;
        }
        let id = read32(bus, slot, function, 0);
        let class = read32(bus, slot, function, 0x08);
        let header = read8(bus, slot, function, 0x0e);
        let interrupt_line = read8(bus, slot, function, 0x3c);
        let bar_count = match header & 0x7f {
            0 => 6,
            1 => 2,
            _ => 0,
        };
        let mut bars = [0u32; 6];
        for (index, bar) in bars.iter_mut().enumerate().take(bar_count) {
            *bar = read32(bus, slot, function, 0x10 + index as u8 * 4);
        }
        self.devices[self.count] = PciDevice {
            bus,
            slot,
            function,
            vendor: id as u16,
            device: (id >> 16) as u16,
            class: (class >> 24) as u8,
            subclass: (class >> 16) as u8,
            programming_interface: (class >> 8) as u8,
            revision: class as u8,
            header_type: header,
            interrupt_line,
            bars,
            capability_count: capability_count(bus, slot, function),
        };
        self.count += 1;
    }
}

fn capability_count(bus: u8, slot: u8, function: u8) -> u8 {
    let status = read16(bus, slot, function, 0x06);
    if status & 0x10 == 0 {
        return 0;
    }
    let mut pointer = read8(bus, slot, function, 0x34) & 0xfc;
    let mut visited = 0u64;
    let mut count = 0u8;
    while (0x40..=0xfc).contains(&pointer) && count < 48 {
        let bit = ((pointer - 0x40) / 4) as u64;
        if visited & (1u64 << bit) != 0 {
            break;
        }
        visited |= 1u64 << bit;
        count += 1;
        pointer = read8(bus, slot, function, pointer.saturating_add(1)) & 0xfc;
    }
    count
}

fn read8(bus: u8, slot: u8, function: u8, offset: u8) -> u8 {
    let value = read32(bus, slot, function, offset);
    (value >> ((offset & 3) * 8)) as u8
}

fn read16(bus: u8, slot: u8, function: u8, offset: u8) -> u16 {
    let value = read32(bus, slot, function, offset);
    (value >> ((offset & 2) * 8)) as u16
}

fn read32(bus: u8, slot: u8, function: u8, offset: u8) -> u32 {
    let address = 0x8000_0000u32
        | (bus as u32) << 16
        | (slot as u32) << 11
        | (function as u32) << 8
        | (offset as u32 & 0xfc);
    unsafe {
        arch::outl(0xcf8, address);
        arch::inl(0xcfc)
    }
}

fn write16(bus: u8, slot: u8, function: u8, offset: u8, value: u16) {
    let aligned = offset & 0xfc;
    let shift = (offset & 2) * 8;
    let mut current = read32(bus, slot, function, aligned);
    current &= !(0xffffu32 << shift);
    current |= (value as u32) << shift;
    let address = 0x8000_0000u32
        | (bus as u32) << 16
        | (slot as u32) << 11
        | (function as u32) << 8
        | aligned as u32;
    unsafe {
        arch::outl(0xcf8, address);
        arch::outl(0xcfc, current);
    }
}
