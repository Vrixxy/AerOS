//! The ACPI namespace: the firmware's DSDT and SSDTs run through the AML
//! interpreter, with port I/O, mapped memory and PCI configuration space
//! behind it. Used for the soft-off sleep type, PCI interrupt routing and
//! resource queries, and the `acpi` shell command.

use core::fmt::Write;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::acpi::{self, AcpiInfo};
use crate::aml::{self, Aml, Hooks, NodeKind, Resource, Value};
use crate::arch;
use crate::ec;
use crate::sync::TicketLock;

const HOOKS: Hooks = Hooks {
    io_read,
    io_write,
    mem_read,
    mem_write,
    pci_read,
    pci_write,
    ec_read,
    ec_write,
    now_ns,
};

static AML: TicketLock<Aml> = TicketLock::new(Aml::new(HOOKS));
static REPORT: TicketLock<Report> = TicketLock::new(Report::EMPTY);

#[derive(Clone, Copy)]
pub struct Report {
    pub loaded: bool,
    pub tables: usize,
    pub nodes: usize,
    pub devices: usize,
    pub methods: usize,
    pub load_errors: u32,
    pub apic_mode: bool,
}

impl Report {
    const EMPTY: Self = Self {
        loaded: false,
        tables: 0,
        nodes: 0,
        devices: 0,
        methods: 0,
        load_errors: 0,
        apic_mode: false,
    };
}

fn io_read(port: u16, bytes: u8) -> u64 {
    // SAFETY: the firmware's own AML decides which ports it reads.
    unsafe {
        match bytes {
            1 => arch::inb(port) as u64,
            2 => arch::inw(port) as u64,
            _ => arch::inl(port) as u64,
        }
    }
}

fn io_write(port: u16, bytes: u8, value: u64) {
    // SAFETY: as above.
    unsafe {
        match bytes {
            1 => arch::outb(port, value as u8),
            2 => arch::outw(port, value as u16),
            _ => arch::outl(port, value as u32),
        }
    }
}

fn mem_read(address: u64, bytes: u8) -> Option<u64> {
    if !arch::paging::kernel_range_mapped(address, bytes as u64) {
        return None;
    }
    let pointer = address as usize;
    // SAFETY: the range is mapped; a device register or firmware table.
    unsafe {
        Some(match bytes {
            1 => core::ptr::read_volatile(pointer as *const u8) as u64,
            2 => core::ptr::read_volatile(pointer as *const u16) as u64,
            4 => core::ptr::read_volatile(pointer as *const u32) as u64,
            _ => core::ptr::read_volatile(pointer as *const u64),
        })
    }
}

fn mem_write(address: u64, bytes: u8, value: u64) -> bool {
    if !arch::paging::kernel_range_mapped(address, bytes as u64) {
        return false;
    }
    let pointer = address as usize;
    // SAFETY: as above.
    unsafe {
        match bytes {
            1 => core::ptr::write_volatile(pointer as *mut u8, value as u8),
            2 => core::ptr::write_volatile(pointer as *mut u16, value as u16),
            4 => core::ptr::write_volatile(pointer as *mut u32, value as u32),
            _ => core::ptr::write_volatile(pointer as *mut u64, value),
        }
    }
    true
}

fn pci_read(bus: u8, slot: u8, function: u8, offset: u16, bytes: u8) -> u64 {
    crate::pci::config_read(bus, slot, function, offset, bytes)
}

fn pci_write(bus: u8, slot: u8, function: u8, offset: u16, bytes: u8, value: u64) {
    crate::pci::config_write(bus, slot, function, offset, bytes, value);
}

fn now_ns() -> u64 {
    crate::time::monotonic_nanoseconds()
}

static EC_DATA: AtomicU32 = AtomicU32::new(0);
static EC_COMMAND: AtomicU32 = AtomicU32::new(0);
static EC_LOCK: TicketLock<()> = TicketLock::new(());

/// The embedded controller's ports, which AML reaches through its
/// `EmbeddedControl` operation regions.
pub fn set_embedded_controller(data: u16, command: u16) {
    EC_DATA.store(data as u32, Ordering::Release);
    EC_COMMAND.store(command as u32, Ordering::Release);
}

struct EcPorts {
    data: u16,
    command: u16,
}

impl ec::Bus for EcPorts {
    fn status(&mut self) -> u8 {
        // SAFETY: the controller's own ports, from its `_CRS`.
        unsafe { arch::inb(self.command) }
    }

    fn read_data(&mut self) -> u8 {
        // SAFETY: as above.
        unsafe { arch::inb(self.data) }
    }

    fn write_command(&mut self, value: u8) {
        // SAFETY: as above.
        unsafe { arch::outb(self.command, value) }
    }

    fn write_data(&mut self, value: u8) {
        // SAFETY: as above.
        unsafe { arch::outb(self.data, value) }
    }

    fn now_ns(&mut self) -> u64 {
        crate::time::monotonic_nanoseconds()
    }
}

fn ec_ports() -> Option<EcPorts> {
    let command = EC_COMMAND.load(Ordering::Acquire);
    (command != 0).then(|| EcPorts {
        data: EC_DATA.load(Ordering::Acquire) as u16,
        command: command as u16,
    })
}

fn ec_read(offset: u8) -> Option<u8> {
    let mut ports = ec_ports()?;
    let _serialized = EC_LOCK.lock();
    ec::read(&mut ports, offset)
}

fn ec_write(offset: u8, value: u8) -> bool {
    let Some(mut ports) = ec_ports() else {
        return false;
    };
    let _serialized = EC_LOCK.lock();
    ec::write(&mut ports, offset, value)
}

/// The next event the embedded controller is holding, if any.
pub fn ec_query() -> Option<u8> {
    let mut ports = ec_ports()?;
    let _serialized = EC_LOCK.lock();
    ec::query(&mut ports)
}

/// Runs `\_SB._INI` and the `_INI` method of every device the firmware says
/// is present, in load order. Laptop tables use them to record which
/// operating system they are talking to.
fn run_initialisers(aml: &mut Aml) {
    let system_bus = aml.find("\\_SB_");
    for index in 0..aml.node_count() {
        let node = index as u16;
        if aml.node_kind(node) != NodeKind::Device && Some(node) != system_bus {
            continue;
        }
        if let Some(status) = aml.child_node(node, b"_STA")
            && let Ok(value) = aml.evaluate_node(status, &[])
            && let Some(bits) = aml.integer(value)
            && bits & 0b1001 == 0
        {
            continue;
        }
        if let Some(initialiser) = aml.child_node(node, b"_INI") {
            let _ = aml.evaluate_node(initialiser, &[]);
        }
    }
}

/// Loads the DSDT and every SSDT and tells the firmware the system uses the
/// APIC (`_PIC`).
pub fn initialize(acpi: &AcpiInfo) -> Report {
    let mut aml = AML.lock();
    aml.reset();
    let mut tables = 0;
    if acpi.dsdt_address != 0 {
        let base = acpi.dsdt_address as usize as *const u8;
        // SAFETY: the FADT named this address; `validate_sdt` checks the table.
        if let Some(length) = unsafe { acpi::validate_sdt(base, Some(*b"DSDT")) }
            && aml.load(acpi.dsdt_address, length).is_ok()
        {
            tables += 1;
        }
    }
    // SAFETY: root-table entries are firmware tables, checked by `each_table`.
    unsafe {
        acpi::each_table(acpi, |signature, address, length| {
            if signature == *b"SSDT" && aml.load(address, length).is_ok() {
                tables += 1;
            }
        });
    }
    let apic_mode = tables != 0 && aml.evaluate("\\_PIC", &[1]).is_ok();
    if tables != 0 {
        run_initialisers(&mut aml);
    }
    let mut devices = 0;
    let mut methods = 0;
    for index in 0..aml.node_count() {
        match aml.node_kind(index as u16) {
            NodeKind::Device => devices += 1,
            NodeKind::Method => methods += 1,
            _ => {}
        }
    }
    let report = Report {
        loaded: tables != 0,
        tables,
        nodes: aml.node_count(),
        devices,
        methods,
        load_errors: aml.load_errors,
        apic_mode,
    };
    *REPORT.lock() = report;
    report
}

pub fn report() -> Report {
    *REPORT.lock()
}

/// The two `SLP_TYP` values of a sleep state, from its `\_Sx_` package.
pub fn sleep_type(state: u8) -> Option<(u8, u8)> {
    if !REPORT.lock().loaded {
        return None;
    }
    let name = [b'\\', b'_', b'S', b'0' + state, b'_'];
    let path = core::str::from_utf8(&name).ok()?;
    let mut aml = AML.lock();
    let package = aml.evaluate(path, &[]).ok()?;
    let first = aml.integer(aml.package_element(package, 0)?)?;
    let second = aml.integer(aml.package_element(package, 1)?)?;
    Some((first as u8, second as u8))
}

pub fn with<R>(visit: impl FnOnce(&mut Aml) -> R) -> R {
    visit(&mut AML.lock())
}

fn text_of(bytes: &[u8], out: &mut impl Write) {
    for byte in bytes {
        let shown = if (0x20..0x7f).contains(byte) {
            *byte as char
        } else {
            '.'
        };
        let _ = out.write_char(shown);
    }
}

fn write_value(aml: &Aml, value: Value, depth: usize, out: &mut impl Write) {
    if let Some(number) = aml.integer(value) {
        let _ = write!(out, "{number:#x}");
    } else if let Some(count) = aml.package_len(value) {
        if depth >= 2 {
            let _ = write!(out, "[{count} elements]");
            return;
        }
        let _ = write!(out, "[");
        for index in 0..count.min(16) {
            if index != 0 {
                let _ = write!(out, ", ");
            }
            if let Some(element) = aml.package_element(value, index) {
                write_value(aml, element, depth + 1, out);
            }
        }
        if count > 16 {
            let _ = write!(out, ", ... {count} in all");
        }
        let _ = write!(out, "]");
    } else if let Some(node) = aml.value_node(value) {
        let mut path = [0u8; 64];
        let length = aml.node_path(node, &mut path);
        text_of(&path[..length], out);
    } else if let Some(bytes) = aml.buffer(value) {
        match value {
            Value::Str(_) => {
                let _ = write!(out, "\"");
                text_of(bytes, out);
                let _ = write!(out, "\"");
            }
            _ => {
                let _ = write!(out, "buffer({}) ", bytes.len());
                for byte in bytes.iter().take(24) {
                    let _ = write!(out, "{byte:02x}");
                }
                if bytes.len() > 24 {
                    let _ = write!(out, "...");
                }
            }
        }
    } else {
        let _ = write!(out, "(no value)");
    }
}

pub fn describe_value(aml: &Aml, value: Value, out: &mut impl Write) {
    write_value(aml, value, 0, out);
}

/// Prints the resource descriptors of a `_CRS`-style buffer.
pub fn describe_resources(template: &[u8], out: &mut impl Write) {
    aml::resources(template, |resource| match resource {
        Resource::Irq { mask } => {
            let _ = write!(out, "irq mask {mask:#06x}; ");
        }
        Resource::Io {
            minimum,
            maximum,
            length,
        } => {
            let _ = write!(out, "io {minimum:#x}-{maximum:#x} len {length:#x}; ");
        }
        Resource::FixedIo { base, length } => {
            let _ = write!(out, "io {base:#x} len {length:#x}; ");
        }
        Resource::Memory { minimum, length } => {
            let _ = write!(out, "mem {minimum:#x} len {length:#x}; ");
        }
        Resource::ExtendedIrq { first, count } => {
            let _ = write!(out, "gsi {first} x{count}; ");
        }
    });
}

/// The path text a shell argument names: a leading `\` is optional.
pub fn absolute(argument: &str, out: &mut [u8; 96]) -> usize {
    let mut length = 0;
    if !argument.starts_with('\\') {
        out[0] = b'\\';
        length = 1;
    }
    for byte in argument.bytes() {
        if length < out.len() {
            out[length] = byte;
            length += 1;
        }
    }
    length
}

/// Everything the boot-test checks against QEMU's firmware.
#[cfg(feature = "boot-test")]
pub struct TestReport {
    pub s5: Option<(u8, u8)>,
    pub pci_root: bool,
    pub crs_ok: bool,
    pub prt_entries: usize,
    pub prt_links: bool,
    pub sta_ok: usize,
    pub sta_errors: usize,
    pub com1_ok: bool,
    pub link_crs_ok: bool,
}

#[cfg(feature = "boot-test")]
pub fn self_test() -> TestReport {
    let mut aml = AML.lock();
    let s5 = {
        let found = aml.evaluate("\\_S5_", &[]).ok();
        found.and_then(|package| {
            let first = aml.integer(aml.package_element(package, 0)?)?;
            let second = aml.integer(aml.package_element(package, 1)?)?;
            Some((first as u8, second as u8))
        })
    };
    let mut report = TestReport {
        s5,
        pci_root: false,
        crs_ok: false,
        prt_entries: 0,
        prt_links: false,
        sta_ok: 0,
        sta_errors: 0,
        com1_ok: false,
        link_crs_ok: false,
    };
    if let Some(node) = aml.find("\\_SB_.PCI0") {
        let mut id = [0u8; 8];
        report.pci_root = aml
            .device_id(node, b"_HID", &mut id)
            .is_some_and(|length| &id[..length] == b"PNP0A08");
    }
    if let Ok(value) = aml.evaluate("\\_SB_.PCI0._CRS", &[])
        && let Some(template) = aml.buffer(value)
    {
        let mut config_ports = false;
        aml::resources(template, |resource| {
            if let Resource::Io { minimum: 0xcf8, .. } = resource {
                config_ports = true;
            }
        });
        report.crs_ok = config_ports;
    }
    if let Ok(prt) = aml.evaluate("\\_SB_.PCI0._PRT", &[]) {
        let count = aml.package_len(prt).unwrap_or(0);
        report.prt_entries = count;
        report.prt_links = count > 0
            && (0..count).all(|index| {
                aml.package_element(prt, index)
                    .and_then(|entry| aml.package_element(entry, 2))
                    .and_then(|source| aml.value_node(source))
                    .is_some_and(|node| aml.node_kind(node) == NodeKind::Device)
            });
    }
    if let Ok(value) = aml.evaluate("\\_SB_.PCI0.SF8_.COM1._CRS", &[])
        && let Some(template) = aml.buffer(value)
    {
        let (mut port, mut irq) = (false, false);
        aml::resources(template, |resource| match resource {
            Resource::Io {
                minimum: 0x3f8,
                length: 8,
                ..
            } => port = true,
            Resource::Irq { mask: 0x10 } => irq = true,
            _ => {}
        });
        report.com1_ok = port && irq;
    }
    if let Ok(value) = aml.evaluate("\\_SB_.LNKA._CRS", &[])
        && let Some(template) = aml.buffer(value)
    {
        let mut irqs = 0;
        aml::resources(template, |resource| {
            if let Resource::Irq { .. } | Resource::ExtendedIrq { .. } = resource {
                irqs += 1;
            }
        });
        report.link_crs_ok = irqs != 0;
    }
    for index in 0..aml.node_count() {
        let node = index as u16;
        if aml.node_kind(node) == NodeKind::Method && &aml.node_name(node) == b"_STA" {
            match aml.evaluate_node(node, &[]) {
                Ok(_) => report.sta_ok += 1,
                Err(_) => report.sta_errors += 1,
            }
        }
    }
    report
}
