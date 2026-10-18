use crate::sync::TicketLock;
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

/// One "vendor-specific" (id 0x09) PCI capability - the shape every modern
/// (virtio 1.0+) virtio-pci device advertises one of per config region
/// (common/notify/ISR/device), each pointing at a byte range inside one of
/// the device's own BARs. `notify_multiplier` is only meaningful when
/// `cfg_type == NOTIFY_CFG` (2); it is the capability's own extra trailing
/// field, read from the 4 bytes right after the base 16-byte structure.
#[derive(Clone, Copy)]
pub struct VendorCapability {
    pub cfg_type: u8,
    pub bar: u8,
    pub offset: u32,
    pub length: u32,
    pub notify_multiplier: u32,
}

/// Walks the capability list looking for vendor-specific (id 0x09)
/// capabilities, calling `visit` with each one's raw bytes decoded. Same
/// walk/loop-guard shape as `capability_count` (bounded, revisit-proof).
pub fn walk_vendor_capabilities(device: &PciDevice, mut visit: impl FnMut(VendorCapability)) {
    let status = read16(device.bus, device.slot, device.function, 0x06);
    if status & 0x10 == 0 {
        return;
    }
    let mut pointer = read8(device.bus, device.slot, device.function, 0x34) & 0xfc;
    let mut visited = 0u64;
    let mut steps = 0u8;
    while (0x40..=0xfc).contains(&pointer) && steps < 48 {
        let bit = ((pointer - 0x40) / 4) as u64;
        if visited & (1u64 << bit) != 0 {
            break;
        }
        visited |= 1u64 << bit;
        steps += 1;
        let id = read8(device.bus, device.slot, device.function, pointer);
        if id == 0x09 {
            let cap_len = read8(device.bus, device.slot, device.function, pointer + 2);
            let cfg_type = read8(device.bus, device.slot, device.function, pointer + 3);
            let bar = read8(device.bus, device.slot, device.function, pointer + 4);
            let offset = read32(device.bus, device.slot, device.function, pointer + 8);
            let length = read32(device.bus, device.slot, device.function, pointer + 12);
            let notify_multiplier = if cfg_type == 2 && cap_len >= 20 {
                read32(device.bus, device.slot, device.function, pointer + 16)
            } else {
                0
            };
            visit(VendorCapability {
                cfg_type,
                bar,
                offset,
                length,
                notify_multiplier,
            });
        }
        pointer = read8(
            device.bus,
            device.slot,
            device.function,
            pointer.saturating_add(1),
        ) & 0xfc;
    }
}

/// The physical address a capability's `bar`/`offset` resolves to, handling
/// a 64-bit BAR pair (the low BAR's bit 2 set means the next BAR holds its
/// high 32 bits, per the PCI spec) the way a 32-bit-only `bars[n]` read
/// alone cannot.
pub fn bar_address(device: &PciDevice, bar: u8, offset: u32) -> Option<u64> {
    let index = bar as usize;
    let raw = *device.bars.get(index)?;
    if raw & 1 != 0 {
        return None; // an I/O BAR, not a memory one - no modern virtio cap uses this
    }
    let base = if raw & 0b100 != 0 {
        let high = *device.bars.get(index + 1)? as u64;
        (raw as u64 & !0xf) | (high << 32)
    } else {
        raw as u64 & !0xf
    };
    Some(base + offset as u64)
}

const SAVED_FUNCTIONS: usize = 128;

#[derive(Clone, Copy)]
struct SavedFunction {
    bus: u8,
    slot: u8,
    function: u8,
    /// The standard header: ID, command/status, class, BARs, bus numbers,
    /// expansion ROM, interrupt line (16 dwords).
    header: [u32; 16],
}

struct SavedConfig {
    count: usize,
    functions: [SavedFunction; SAVED_FUNCTIONS],
}

static SAVED: TicketLock<SavedConfig> = TicketLock::new(SavedConfig {
    count: 0,
    functions: [SavedFunction {
        bus: 0,
        slot: 0,
        function: 0,
        header: [0; 16],
    }; SAVED_FUNCTIONS],
});

/// Copies the standard header of every function, ahead of a sleep. Returns
/// how many functions were saved.
pub fn save_config() -> usize {
    let mut saved = SAVED.lock();
    let mut count = 0;
    for bus in 0u16..=255 {
        for slot in 0u8..32 {
            if read16(bus as u8, slot, 0, 0) == 0xffff {
                continue;
            }
            let multi = read8(bus as u8, slot, 0, 0x0e) & 0x80 != 0;
            for function in 0u8..if multi { 8 } else { 1 } {
                if read16(bus as u8, slot, function, 0) == 0xffff || count == SAVED_FUNCTIONS {
                    continue;
                }
                let mut header = [0u32; 16];
                for (index, word) in header.iter_mut().enumerate() {
                    *word = read32(bus as u8, slot, function, (index * 4) as u8);
                }
                saved.functions[count] = SavedFunction {
                    bus: bus as u8,
                    slot,
                    function,
                    header,
                };
                count += 1;
            }
        }
    }
    saved.count = count;
    count
}

/// Writes back what `save_config` copied wherever a function no longer has
/// it (BARs, bus numbers, expansion ROM, interrupt line, command), with
/// decoding off while the address registers change. Returns the number of
/// registers written. Capability state (MSI, power management) is left to the
/// firmware and to the drivers.
pub fn restore_config() -> usize {
    let saved = SAVED.lock();
    let mut written = 0;
    for function in &saved.functions[..saved.count] {
        let (bus, slot, number) = (function.bus, function.slot, function.function);
        if read16(bus, slot, number, 0) != (function.header[0] & 0xffff) as u16 {
            continue;
        }
        let differs =
            |index: usize| read32(bus, slot, number, (index * 4) as u8) != function.header[index];
        let address_changed = (4..16).any(|index| index != 15 && differs(index));
        let command = (function.header[1] & 0xffff) as u16;
        if address_changed {
            write16(bus, slot, number, 0x04, command & !0x7);
            for index in 4..16 {
                if index == 15 || !differs(index) {
                    continue;
                }
                config_write(
                    bus,
                    slot,
                    number,
                    (index * 4) as u16,
                    4,
                    function.header[index] as u64,
                );
                written += 1;
            }
        }
        // Interrupt line only (the pin is read-only).
        let line = (function.header[15] & 0xff) as u8;
        if read8(bus, slot, number, 0x3c) != line {
            config_write(bus, slot, number, 0x3c, 1, line as u64);
            written += 1;
        }
        if read16(bus, slot, number, 0x04) != command {
            write16(bus, slot, number, 0x04, command);
            written += 1;
        }
    }
    written
}

/// Boot test: damages one idle device's BAR, restores, and checks that it is
/// back and that a second restore has nothing to do.
#[cfg(feature = "boot-test")]
pub fn restore_self_test() -> bool {
    let saved = save_config();
    if saved == 0 {
        return false;
    }
    // The virtio-rng device is only touched once, at start-up.
    let Some(slot) =
        (0u8..32).find(|slot| read16(0, *slot, 0, 0) == 0x1af4 && read16(0, *slot, 0, 2) == 0x1044)
    else {
        return restore_config() == 0;
    };
    let low = read32(0, slot, 0, 0x20);
    let high = read32(0, slot, 0, 0x24);
    config_write(0, slot, 0, 0x20, 4, 0xffff_ffff);
    config_write(0, slot, 0, 0x24, 4, 0xffff_ffff);
    let damaged = read32(0, slot, 0, 0x20) != low;
    let written = restore_config();
    let fixed = read32(0, slot, 0, 0x20) == low && read32(0, slot, 0, 0x24) == high;
    let idle = restore_config() == 0;
    damaged && written >= 2 && fixed && idle
}

/// Reads 1, 2 or 4 bytes of a function's configuration space (the first 256
/// bytes; the extended space is not reachable through the legacy ports).
pub fn config_read(bus: u8, slot: u8, function: u8, offset: u16, bytes: u8) -> u64 {
    if offset > 0xff || !matches!(bytes, 1 | 2 | 4) {
        return 0;
    }
    let word = read32(bus, slot, function, offset as u8) as u64;
    let shifted = word >> ((offset & 3) * 8);
    match bytes {
        1 => shifted & 0xff,
        2 => shifted & 0xffff,
        _ => word,
    }
}

pub fn config_write(bus: u8, slot: u8, function: u8, offset: u16, bytes: u8, value: u64) {
    if offset > 0xff || !matches!(bytes, 1 | 2 | 4) {
        return;
    }
    let aligned = (offset as u8) & 0xfc;
    let shift = (offset & 3) * 8;
    let mask = match bytes {
        1 => 0xffu32,
        2 => 0xffff,
        _ => u32::MAX,
    };
    let mut current = read32(bus, slot, function, aligned);
    current = (current & !(mask << shift)) | ((value as u32 & mask) << shift);
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
