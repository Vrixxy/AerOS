use crate::acpi::AcpiInfo;
use crate::sync::TicketLock;

const REGISTER_SELECT: u64 = 0;
const REGISTER_WINDOW: u64 = 0x10;
const REDIRECTION_BASE: u32 = 0x10;
const TIMER_VECTOR: u8 = 50;

static ACCESS: TicketLock<()> = TicketLock::new(());

#[derive(Clone, Copy)]
pub struct IoApicReport {
    pub present: bool,
    pub address: u64,
    pub id: u8,
    pub version: u8,
    pub redirections: u8,
    pub gsi_base: u32,
    pub timer_gsi: u32,
    pub timer_vector: u8,
    pub active_low: bool,
    pub level_triggered: bool,
    pub ticks: u64,
    pub verified: bool,
}

pub fn initialize(acpi: &AcpiInfo, destination: u32) -> IoApicReport {
    let address = acpi.io_apic_address;
    if address == 0 || address & 0xfff != 0 || destination > u8::MAX as u32 {
        return empty(address);
    }
    let _access = ACCESS.lock();
    let id_register = read(address, 0);
    let version_register = read(address, 1);
    let id = (id_register >> 24) as u8;
    let version = version_register as u8;
    let maximum_entry = ((version_register >> 16) & 0xff) as u8;
    let redirections = maximum_entry.saturating_add(1);
    if version == 0 || redirections == 0 {
        return empty(address);
    }
    for entry in 0..redirections as u32 {
        let register = REDIRECTION_BASE + entry * 2;
        write(address, register, read(address, register) | (1 << 16));
    }
    let route = acpi.legacy_route(0);
    if route.gsi < acpi.io_apic_gsi_base {
        return empty(address);
    }
    let entry = route.gsi - acpi.io_apic_gsi_base;
    if entry >= redirections as u32 {
        return empty(address);
    }
    let polarity = route.flags & 3;
    let trigger = (route.flags >> 2) & 3;
    if polarity == 2 || trigger == 2 {
        return empty(address);
    }
    let active_low = polarity == 3;
    let level_triggered = trigger == 3;
    let mut low = TIMER_VECTOR as u32;
    if active_low {
        low |= 1 << 13;
    }
    if level_triggered {
        low |= 1 << 15;
    }
    let register = REDIRECTION_BASE + entry * 2;
    write(address, register, low | (1 << 16));
    write(address, register + 1, destination << 24);
    write(address, register, low);
    let programmed_low = read(address, register);
    let programmed_high = read(address, register + 1);
    let ticks = crate::arch::interrupts::ioapic_timer_self_test(100, 4).unwrap_or(0);
    write(address, register, programmed_low | (1 << 16));
    let verified = programmed_low & 0xff == TIMER_VECTOR as u32
        && programmed_low & (1 << 16) == 0
        && programmed_high >> 24 == destination
        && ticks >= 4;
    IoApicReport {
        present: true,
        address,
        id,
        version,
        redirections,
        gsi_base: acpi.io_apic_gsi_base,
        timer_gsi: route.gsi,
        timer_vector: TIMER_VECTOR,
        active_low,
        level_triggered,
        ticks,
        verified,
    }
}

pub fn route_legacy_irq(acpi: &AcpiInfo, irq: u8, destination: u32, vector: u8) -> bool {
    let address = acpi.io_apic_address;
    if address == 0 || address & 0xfff != 0 || destination > u8::MAX as u32 || vector < 32 {
        return false;
    }
    let _access = ACCESS.lock();
    let version = read(address, 1);
    let redirections = ((version >> 16) & 0xff).saturating_add(1);
    let route = acpi.legacy_route(irq);
    if route.gsi < acpi.io_apic_gsi_base {
        return false;
    }
    let entry = route.gsi - acpi.io_apic_gsi_base;
    if entry >= redirections {
        return false;
    }
    let polarity = route.flags & 3;
    let trigger = (route.flags >> 2) & 3;
    if polarity == 2 || trigger == 2 {
        return false;
    }
    let mut low = vector as u32;
    if polarity == 3 {
        low |= 1 << 13;
    }
    if trigger == 3 {
        low |= 1 << 15;
    }
    let register = REDIRECTION_BASE + entry * 2;
    write(address, register, low | (1 << 16));
    write(address, register + 1, destination << 24);
    write(address, register, low);
    let observed_low = read(address, register);
    let observed_high = read(address, register + 1);
    observed_low & 0xff == vector as u32
        && observed_low & (1 << 16) == 0
        && observed_high >> 24 == destination
}

/// Masks the entry a legacy interrupt line is routed to.
#[cfg(feature = "s3-early")]
pub fn mask_legacy_irq(acpi: &AcpiInfo, irq: u8) {
    let address = acpi.io_apic_address;
    if address == 0 || address & 0xfff != 0 {
        return;
    }
    let _access = ACCESS.lock();
    let route = acpi.legacy_route(irq);
    if route.gsi < acpi.io_apic_gsi_base {
        return;
    }
    let register = REDIRECTION_BASE + (route.gsi - acpi.io_apic_gsi_base) * 2;
    write(address, register, read(address, register) | (1 << 16));
}

pub const SAVED_ENTRIES: usize = 64;

/// The redirection table, kept across a sleep that resets the controller.
#[derive(Clone, Copy)]
pub struct Table {
    entries: [(u32, u32); SAVED_ENTRIES],
    count: usize,
}

impl Table {
    pub const EMPTY: Table = Table {
        entries: [(0, 0); SAVED_ENTRIES],
        count: 0,
    };
}

pub fn save(acpi: &AcpiInfo) -> Table {
    let mut table = Table::EMPTY;
    let address = acpi.io_apic_address;
    if address == 0 || address & 0xfff != 0 {
        return table;
    }
    let _access = ACCESS.lock();
    let count = (((read(address, 1) >> 16) & 0xff) as usize + 1).min(SAVED_ENTRIES);
    for entry in 0..count {
        let register = REDIRECTION_BASE + entry as u32 * 2;
        table.entries[entry] = (read(address, register), read(address, register + 1));
    }
    table.count = count;
    table
}

/// Writes the table back, every entry masked first so nothing fires half set.
pub fn restore(acpi: &AcpiInfo, table: &Table) -> bool {
    let address = acpi.io_apic_address;
    if address == 0 || address & 0xfff != 0 || table.count == 0 {
        return false;
    }
    let _access = ACCESS.lock();
    for (entry, (low, high)) in table.entries[..table.count].iter().enumerate() {
        let register = REDIRECTION_BASE + entry as u32 * 2;
        write(address, register, low | (1 << 16));
        write(address, register + 1, *high);
        write(address, register, *low);
    }
    table.entries[..table.count]
        .iter()
        .enumerate()
        .all(|(entry, (low, high))| {
            let register = REDIRECTION_BASE + entry as u32 * 2;
            // The delivery-status and remote-IRR bits are the hardware's.
            read(address, register) & 0xffff_afff == low & 0xffff_afff
                && read(address, register + 1) == *high
        })
}

fn empty(address: u64) -> IoApicReport {
    IoApicReport {
        present: false,
        address,
        id: 0,
        version: 0,
        redirections: 0,
        gsi_base: 0,
        timer_gsi: 0,
        timer_vector: TIMER_VECTOR,
        active_low: false,
        level_triggered: false,
        ticks: 0,
        verified: false,
    }
}

fn read(address: u64, register: u32) -> u32 {
    unsafe {
        core::ptr::write_volatile((address + REGISTER_SELECT) as usize as *mut u32, register);
        core::ptr::read_volatile((address + REGISTER_WINDOW) as usize as *const u32)
    }
}

fn write(address: u64, register: u32, value: u32) {
    unsafe {
        core::ptr::write_volatile((address + REGISTER_SELECT) as usize as *mut u32, register);
        core::ptr::write_volatile((address + REGISTER_WINDOW) as usize as *mut u32, value);
    }
}
