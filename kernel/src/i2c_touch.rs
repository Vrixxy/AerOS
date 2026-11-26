//! Touchscreens that speak HID over I2C (ACPI `PNP0C50`) on a DesignWare I2C
//! controller, such as AMD's `AMDI0010`. A tap is a left click and a drag is a
//! drag. The interrupt line is read as a GPIO level instead of taking the IRQ.

use crate::aml::{Aml, NodeKind, Resource};
use crate::shell::Text;
use crate::sync::TicketLock;
use crate::xhci::Pointer;
use core::fmt::Write;

const CON: u64 = 0x00;
const TAR: u64 = 0x04;
const DATA_CMD: u64 = 0x10;
const FS_HCNT: u64 = 0x1c;
const FS_LCNT: u64 = 0x20;
const INTR_MASK: u64 = 0x30;
const RAW_INTR: u64 = 0x34;
const RX_TL: u64 = 0x38;
const TX_TL: u64 = 0x3c;
const CLR_INTR: u64 = 0x40;
const CLR_TX_ABRT: u64 = 0x54;
const ENABLE: u64 = 0x6c;
const STATUS: u64 = 0x70;
const TXFLR: u64 = 0x74;
const RXFLR: u64 = 0x78;
const ABRT_SOURCE: u64 = 0x80;
const ENABLE_STATUS: u64 = 0x9c;
const COMP_PARAM: u64 = 0xf4;
const COMP_TYPE: u64 = 0xfc;
const TX_ABORT: u32 = 1 << 6;

const START_DELAY_NS: u64 = 3_000_000_000;
const POLL_NS: u64 = 8_000_000;
const MAX_CANDIDATES: usize = 6;
const MAX_INPUT: usize = 512;

fn now() -> u64 {
    crate::time::monotonic_nanoseconds()
}

fn pause_ms(milliseconds: u64) {
    let end = now() + milliseconds * 1_000_000;
    while now() < end {
        core::hint::spin_loop();
    }
}

struct Bus {
    base: u64,
    tx_depth: u32,
    rx_depth: u32,
}

impl Bus {
    fn get(&self, offset: u64) -> u32 {
        // SAFETY: the controller's registers from its `_CRS`, identity-mapped.
        unsafe { core::ptr::read_volatile((self.base + offset) as *const u32) }
    }

    fn set(&self, offset: u64, value: u32) {
        // SAFETY: as in `get`.
        unsafe { core::ptr::write_volatile((self.base + offset) as *mut u32, value) }
    }

    fn wait(&self, milliseconds: u64, mut done: impl FnMut(&Self) -> bool) -> bool {
        let end = now() + milliseconds * 1_000_000;
        loop {
            if done(self) {
                return true;
            }
            if now() > end {
                return false;
            }
            core::hint::spin_loop();
        }
    }

    fn enable(&self, on: bool) -> bool {
        self.set(ENABLE, on as u32);
        self.wait(25, |bus| bus.get(ENABLE_STATUS) & 1 == on as u32)
    }

    fn open(base: u64, address: u16, log: &mut Text<1024>) -> Option<Self> {
        let mut bus = Self {
            base,
            tx_depth: 8,
            rx_depth: 8,
        };
        let kind = bus.get(COMP_TYPE);
        let _ = writeln!(log, "  controller {base:#x} type {kind:#010x}");
        if kind == 0 || kind == u32::MAX {
            let _ = writeln!(log, "  controller is switched off");
            return None;
        }
        let param = bus.get(COMP_PARAM);
        if param != 0 && param != u32::MAX {
            bus.tx_depth = ((param >> 16) & 0xff) + 1;
            bus.rx_depth = ((param >> 8) & 0xff) + 1;
        }
        if !bus.enable(false) {
            let _ = writeln!(log, "  controller would not stop");
            return None;
        }
        bus.set(CON, 0x65);
        bus.set(FS_HCNT, 0x8c);
        bus.set(FS_LCNT, 0xf0);
        bus.set(TAR, address as u32 & 0x3ff);
        bus.set(INTR_MASK, 0);
        bus.set(RX_TL, 0);
        bus.set(TX_TL, 0);
        if !bus.enable(true) {
            let _ = writeln!(log, "  controller would not start");
            return None;
        }
        Some(bus)
    }

    fn transfer(&self, write: &[u8], read: &mut [u8]) -> Result<(), u32> {
        let _ = self.get(CLR_INTR);
        let total = write.len() + read.len();
        let (mut sent, mut got) = (0usize, 0usize);
        let end = now() + 60_000_000;
        while sent < total || got < read.len() {
            if self.get(RAW_INTR) & TX_ABORT != 0 {
                let source = self.get(ABRT_SOURCE);
                let _ = self.get(CLR_TX_ABRT);
                return Err(source.max(1));
            }
            while sent < total
                && self.get(TXFLR) < self.tx_depth
                && (sent < write.len() || sent - write.len() - got < self.rx_depth as usize)
            {
                let mut command = if sent < write.len() {
                    write[sent] as u32
                } else {
                    0x100
                };
                if sent == write.len() && !write.is_empty() {
                    command |= 0x400;
                }
                if sent + 1 == total {
                    command |= 0x200;
                }
                self.set(DATA_CMD, command);
                sent += 1;
            }
            while got < read.len() && self.get(RXFLR) > 0 {
                read[got] = self.get(DATA_CMD) as u8;
                got += 1;
            }
            if now() > end {
                return Err(0);
            }
        }
        self.wait(10, |bus| bus.get(STATUS) & (1 << 5) == 0);
        if self.get(RAW_INTR) & TX_ABORT != 0 {
            let source = self.get(ABRT_SOURCE);
            let _ = self.get(CLR_TX_ABRT);
            return Err(source.max(1));
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Candidate {
    id: [u8; 8],
    address: u16,
    bus: u64,
    gpio: Option<(u64, bool)>,
}

struct Device {
    bus: Bus,
    pointer: Pointer,
    max_input: usize,
    gpio: Option<(u64, bool)>,
    last: (u32, u32),
    down: bool,
    errors: u32,
}

enum Phase {
    Waiting,
    Ready(Device),
    Off,
}

struct Touch {
    phase: Phase,
    first_ns: u64,
    last_poll_ns: u64,
    reports: u64,
    log: Text<1024>,
}

static TOUCH: TicketLock<Touch> = TicketLock::new(Touch {
    phase: Phase::Waiting,
    first_ns: 0,
    last_poll_ns: 0,
    reports: 0,
    log: Text::new(),
});

static DESCRIPTOR: TicketLock<[u8; 4096]> = TicketLock::new([0; 4096]);

fn u16_at(bytes: &[u8], at: usize) -> usize {
    match bytes.get(at..at + 2) {
        Some(pair) => u16::from_le_bytes([pair[0], pair[1]]) as usize,
        None => 0,
    }
}

fn source_name(item: &[u8], at: usize) -> &str {
    let tail = item.get(at..).unwrap_or(&[]);
    let end = tail.iter().position(|byte| *byte == 0).unwrap_or(tail.len());
    core::str::from_utf8(&tail[..end]).unwrap_or("")
}

fn evaluate_buffer(aml: &mut Aml, node: u16, name: &[u8; 4], out: &mut [u8]) -> usize {
    let Some(child) = aml.child_node(node, name) else {
        return 0;
    };
    let Ok(value) = aml.evaluate_node(child, &[]) else {
        return 0;
    };
    let Some(bytes) = aml.buffer(value) else {
        return 0;
    };
    let length = bytes.len().min(out.len());
    out[..length].copy_from_slice(&bytes[..length]);
    length
}

fn present(aml: &mut Aml, node: u16) -> bool {
    let Some(child) = aml.child_node(node, b"_STA") else {
        return true;
    };
    match aml.evaluate_node(child, &[]) {
        Ok(value) => aml.integer(value).is_none_or(|status| status & 1 != 0),
        Err(_) => true,
    }
}

fn memory_base(aml: &mut Aml, path: &str) -> Option<u64> {
    let node = aml.find(path)?;
    let mut template = [0u8; 256];
    let length = evaluate_buffer(aml, node, b"_CRS", &mut template);
    let mut base = None;
    crate::aml::resources(&template[..length], |resource| {
        if let (Resource::Memory { minimum, .. }, None) = (resource, base) {
            base = Some(minimum);
        }
    });
    base
}

fn is_hid_over_i2c(id: &[u8]) -> bool {
    id == b"PNP0C50" || id == b"ACPI0C50"
}

fn scan(aml: &mut Aml, out: &mut [Candidate; MAX_CANDIDATES], log: &mut Text<1024>) -> usize {
    let mut count = 0;
    for index in 0..aml.node_count() {
        let node = index as u16;
        if count == out.len() || aml.node_kind(node) != NodeKind::Device {
            continue;
        }
        let mut id = [0u8; 8];
        let mut compatible = [0u8; 8];
        let id_length = aml.device_id(node, b"_HID", &mut id).unwrap_or(0);
        let compatible_length = aml.device_id(node, b"_CID", &mut compatible).unwrap_or(0);
        if !is_hid_over_i2c(&id[..id_length]) && !is_hid_over_i2c(&compatible[..compatible_length]) {
            continue;
        }
        let name = core::str::from_utf8(&id[..id_length]).unwrap_or("?");
        if !present(aml, node) {
            let _ = writeln!(log, "{name}: not fitted");
            continue;
        }
        let mut template = [0u8; 256];
        let length = evaluate_buffer(aml, node, b"_CRS", &mut template);
        let template = &template[..length];
        let mut address = None;
        let mut bus_path = Text::<64>::new();
        let mut gpio_path = Text::<64>::new();
        let mut pin = 0usize;
        let mut active_low = true;
        let mut at = 0;
        while at + 3 <= template.len() {
            let tag = template[at];
            if tag & 0x80 == 0 {
                if tag >> 3 == 0x0f {
                    break;
                }
                at += 1 + (tag & 7) as usize;
                continue;
            }
            let end = (at + 3 + u16_at(template, at + 1)).min(template.len());
            let item = &template[at..end];
            match tag {
                0x8e if address.is_none() && item.get(5) == Some(&1) => {
                    address = Some(u16_at(item, 16) as u16);
                    let _ = bus_path.push_str_checked(source_name(item, 12 + u16_at(item, 10)));
                }
                0x8c if gpio_path.as_str().is_empty() && item.get(4) == Some(&0) => {
                    active_low = (u16_at(item, 7) >> 1) & 3 == 1;
                    pin = u16_at(item, u16_at(item, 14));
                    let _ = gpio_path.push_str_checked(source_name(item, u16_at(item, 17)));
                }
                _ => {}
            }
            at = end;
        }
        let Some(address) = address else {
            let _ = writeln!(log, "{name}: no I2C address in _CRS ({length} bytes)");
            continue;
        };
        let Some(bus) = memory_base(aml, bus_path.as_str()) else {
            let _ = writeln!(log, "{name}: can't find bus {}", bus_path.as_str());
            continue;
        };
        let gpio = memory_base(aml, gpio_path.as_str()).map(|base| (base + pin as u64 * 4, active_low));
        let _ = writeln!(
            log,
            "{name}: address {address:#x} on {} gpio {} pin {pin}",
            bus_path.as_str(),
            gpio_path.as_str()
        );
        out[count] = Candidate {
            id,
            address,
            bus,
            gpio,
        };
        count += 1;
    }
    count
}

fn read_hid_descriptor(bus: &Bus, log: &mut Text<1024>) -> Option<[u8; 30]> {
    for register in [0x0001u16, 0x0020, 0x0000, 0x0010, 0x0030] {
        let mut descriptor = [0u8; 30];
        match bus.transfer(&register.to_le_bytes(), &mut descriptor) {
            Ok(()) if u16_at(&descriptor, 0) == 30 && u16_at(&descriptor, 2) == 0x100 => {
                let _ = writeln!(log, "  HID descriptor at register {register:#x}");
                return Some(descriptor);
            }
            Ok(()) => {}
            Err(source) => {
                let _ = writeln!(log, "  register {register:#x}: no answer ({source:#x})");
                if source == 0 {
                    return None;
                }
            }
        }
    }
    None
}

fn try_candidate(candidate: &Candidate, log: &mut Text<1024>) -> Option<Device> {
    let name = core::str::from_utf8(&candidate.id).unwrap_or("?").trim_end_matches('\0');
    let _ = writeln!(log, "{name}:");
    let bus = Bus::open(candidate.bus, candidate.address, log)?;
    let hid = read_hid_descriptor(&bus, log)?;
    let report_length = u16_at(&hid, 4).min(4096);
    let report_register = u16_at(&hid, 6) as u16;
    let command_register = u16_at(&hid, 16) as u16;
    let max_input = u16_at(&hid, 10).clamp(4, MAX_INPUT);
    let pointer = {
        let mut descriptor = DESCRIPTOR.lock();
        if bus
            .transfer(&report_register.to_le_bytes(), &mut descriptor[..report_length])
            .is_err()
        {
            let _ = writeln!(log, "  report descriptor read failed");
            return None;
        }
        let address = descriptor.as_ptr() as u64;
        let length = report_length as u16;
        crate::xhci::parse_touch(address, length, 0x000d_0004)
            .or_else(|| crate::xhci::parse_touch(address, length, 0x000d_0002))
    };
    let Some(pointer) = pointer else {
        let _ = writeln!(log, "  not a touchscreen ({report_length}-byte descriptor), left alone");
        return None;
    };
    let [low, high] = command_register.to_le_bytes();
    let _ = bus.transfer(&[low, high, 0x00, 0x08], &mut []);
    pause_ms(5);
    let _ = bus.transfer(&[low, high, 0x00, 0x01], &mut []);
    pause_ms(100);
    let mut ack = [0u8; MAX_INPUT];
    let _ = bus.transfer(&[], &mut ack[..max_input]);
    let _ = writeln!(log, "  touchscreen ready, reports up to {max_input} bytes");
    Some(Device {
        bus,
        pointer,
        max_input,
        gpio: candidate.gpio,
        last: (0x8000, 0x8000),
        down: false,
        errors: 0,
    })
}

fn start(touch: &mut Touch) {
    touch.log.clear();
    let mut candidates = [Candidate {
        id: [0; 8],
        address: 0,
        bus: 0,
        gpio: None,
    }; MAX_CANDIDATES];
    let count = crate::acpi_ns::with(|aml| scan(aml, &mut candidates, &mut touch.log));
    if count == 0 {
        let _ = writeln!(touch.log, "No HID-over-I2C devices in ACPI.");
    }
    for candidate in &candidates[..count] {
        if let Some(device) = try_candidate(candidate, &mut touch.log) {
            touch.phase = Phase::Ready(device);
            return;
        }
    }
    touch.phase = Phase::Off;
}

fn pending(device: &Device) -> Option<bool> {
    let (address, active_low) = device.gpio?;
    // SAFETY: the pin's control register in the GPIO controller's MMIO range.
    let level = unsafe { core::ptr::read_volatile(address as *const u32) } >> 16 & 1;
    Some((level == 0) == active_low)
}

fn service(device: &mut Device) -> bool {
    let mut input = [0u8; MAX_INPUT];
    if let Err(source) = device.bus.transfer(&[], &mut input[..device.max_input]) {
        device.errors += 1;
        return device.errors < 50 || source != 0;
    }
    device.errors = 0;
    let length = u16_at(&input, 0).min(device.max_input);
    if length < 3 {
        return true;
    }
    let mut report = [0u8; 64];
    let used = (length - 2).min(64);
    report[..used].copy_from_slice(&input[2..2 + used]);
    if let Some((down, x, y)) = crate::xhci::pointer_sample(&device.pointer, &report) {
        if down {
            device.last = (x, y);
        }
        if down || device.down {
            crate::mouse::inject_absolute(down as u8, device.last.0, device.last.1, 0);
        }
        device.down = down;
    }
    true
}

/// Called from the desktop loop: brings the touchscreen up a few seconds in,
/// then reads a report whenever its interrupt line says one is waiting.
pub fn poll() {
    let Some(mut touch) = TOUCH.try_lock() else {
        return;
    };
    let touch = &mut *touch;
    let now = now();
    if matches!(touch.phase, Phase::Waiting) {
        if touch.first_ns == 0 {
            touch.first_ns = now;
        } else if now - touch.first_ns > START_DELAY_NS {
            start(touch);
        }
        return;
    }
    match &mut touch.phase {
        Phase::Off | Phase::Waiting => {}
        Phase::Ready(device) => {
            let due = match pending(device) {
                Some(waiting) => waiting,
                None => now - touch.last_poll_ns >= POLL_NS,
            };
            if !due {
                return;
            }
            let keep = service(device);
            touch.last_poll_ns = now;
            touch.reports += 1;
            if !keep {
                let _ = writeln!(touch.log, "Stopped: the touchscreen kept failing.");
                touch.phase = Phase::Off;
            }
        }
    }
}

/// The `touchscreen` command's report, one line at a time.
pub fn describe(mut line: impl FnMut(&str)) {
    let touch = TOUCH.lock();
    let state = match touch.phase {
        Phase::Waiting => "starting",
        Phase::Ready(_) => "working",
        Phase::Off => "not working",
    };
    let mut head = Text::<64>::new();
    let _ = write!(head, "Touchscreen: {state} ({} reads)", touch.reports);
    line(head.as_str());
    for text in touch.log.as_str().lines() {
        line(text);
    }
}

/// One word for Settings: "working", "starting" or "not found".
pub fn status() -> &'static str {
    match TOUCH.lock().phase {
        Phase::Waiting => "starting",
        Phase::Ready(_) => "working",
        Phase::Off => "not found",
    }
}
