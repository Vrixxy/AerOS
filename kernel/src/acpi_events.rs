//! The ACPI system control interrupt: the fixed-hardware power button and the
//! general-purpose events whose handler methods the firmware provides. The
//! interrupt handler only reads and acknowledges hardware registers and sets
//! flags; the AML handlers run later from `poll`, in thread context.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::acpi::AcpiInfo;
use crate::acpi_devices::{self, GpeHandler};
use crate::sync::TicketLock;
use crate::{acpi_ns, arch, ioapic};

pub const VECTOR: u8 = 54;
const PM1_PWRBTN: u16 = 1 << 8;
const SCI_EN: u16 = 1;
const MAX_HANDLERS: usize = 64;
const BLOCKS: usize = 2;
/// Interrupts that found nothing to do before the line is shut off.
const STORM_LIMIT: u32 = 4000;

#[derive(Clone, Copy)]
pub struct EventReport {
    pub sci: u16,
    pub acpi_mode: bool,
    pub routed: bool,
    pub power_button: bool,
    pub handlers: usize,
    pub enabled_gpes: usize,
    pub embedded_controller: bool,
}

impl EventReport {
    const EMPTY: Self = Self {
        sci: 0,
        acpi_mode: false,
        routed: false,
        power_button: false,
        handlers: 0,
        enabled_gpes: 0,
        embedded_controller: false,
    };
}

static REPORT: TicketLock<EventReport> = TicketLock::new(EventReport::EMPTY);
static EC: TicketLock<Option<acpi_devices::EcInfo>> = TicketLock::new(None);
static HANDLERS: TicketLock<([GpeHandler; MAX_HANDLERS], usize)> = TicketLock::new((
    [GpeHandler {
        number: 0,
        level: false,
        node: 0,
        ec: false,
    }; MAX_HANDLERS],
    0,
));

static PM1_STATUS: [AtomicU32; BLOCKS] = [const { AtomicU32::new(0) }; BLOCKS];
static PM1_ENABLE: [AtomicU32; BLOCKS] = [const { AtomicU32::new(0) }; BLOCKS];
/// The status registers of GPE block `n`; the enable registers follow them.
static GPE_BASE: [AtomicU32; BLOCKS] = [const { AtomicU32::new(0) }; BLOCKS];
static GPE_BYTES: [AtomicU32; BLOCKS] = [const { AtomicU32::new(0) }; BLOCKS];
static GPE_FIRST: [AtomicU32; BLOCKS] = [const { AtomicU32::new(0) }; BLOCKS];
/// GPEs (by number) whose handler is a level-triggered `_Lxx`.
static LEVEL: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
static PENDING: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
static POWER_BUTTON: AtomicBool = AtomicBool::new(false);
static SPURIOUS: AtomicU32 = AtomicU32::new(0);
static SHUT_OFF: AtomicBool = AtomicBool::new(false);
static INTERRUPTS: AtomicU32 = AtomicU32::new(0);
static EVENTS_RUN: AtomicU32 = AtomicU32::new(0);
/// Counts changes to batteries, adapters, the lid and thermal zones the
/// firmware announced, so a reader can tell when to look again.
static DEVICE_CHANGES: AtomicU32 = AtomicU32::new(0);

fn without_interrupts<R>(body: impl FnOnce() -> R) -> R {
    let flags: u64;
    // SAFETY: reads the flags register through the stack.
    unsafe { core::arch::asm!("pushfq", "pop {}", out(reg) flags, options(preserves_flags)) };
    arch::disable_interrupts();
    let result = body();
    if flags & (1 << 9) != 0 {
        arch::enable_interrupts();
    }
    result
}

fn acpi_mode(acpi: &AcpiInfo) -> bool {
    let control = acpi.pm1a_control_block as u16;
    // SAFETY: the FADT names this PM1 control port.
    let enabled = || unsafe { arch::inw(control) } & SCI_EN != 0;
    if enabled() {
        return true;
    }
    if acpi.smi_command_port == 0 || acpi.acpi_enable_value == 0 {
        return false;
    }
    // SAFETY: the firmware's own handshake for entering ACPI mode.
    unsafe { arch::outb(acpi.smi_command_port as u16, acpi.acpi_enable_value) };
    for _ in 0..3_000_000 {
        if enabled() {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

fn gpe_location(acpi: &AcpiInfo, number: u8) -> Option<(usize, u32)> {
    let blocks = [
        (acpi.gpe0_block, acpi.gpe0_length as u32 / 2, 0u32),
        (
            acpi.gpe1_block,
            acpi.gpe1_length as u32 / 2,
            acpi.gpe1_base as u32,
        ),
    ];
    for (index, (base, bytes, first)) in blocks.into_iter().enumerate() {
        let number = number as u32;
        if base != 0 && base <= 0xffff && number >= first && number < first + bytes * 8 {
            return Some((index, number - first));
        }
    }
    None
}

fn publish_blocks(acpi: &AcpiInfo) {
    let pm1 = [acpi.pm1a_event_block, acpi.pm1b_event_block];
    for (index, block) in pm1.into_iter().enumerate() {
        let valid = block != 0 && block <= 0xffff && acpi.pm1_event_length >= 4;
        PM1_STATUS[index].store(if valid { block } else { 0 }, Ordering::Relaxed);
        PM1_ENABLE[index].store(
            if valid {
                block + acpi.pm1_event_length as u32 / 2
            } else {
                0
            },
            Ordering::Relaxed,
        );
    }
    let gpe = [
        (acpi.gpe0_block, acpi.gpe0_length, 0u32),
        (acpi.gpe1_block, acpi.gpe1_length, acpi.gpe1_base as u32),
    ];
    for (index, (block, length, first)) in gpe.into_iter().enumerate() {
        let valid = block != 0 && block <= 0xffff && length >= 2;
        GPE_BASE[index].store(if valid { block } else { 0 }, Ordering::Relaxed);
        GPE_BYTES[index].store(if valid { length as u32 / 2 } else { 0 }, Ordering::Relaxed);
        GPE_FIRST[index].store(first, Ordering::Relaxed);
    }
}

/// Switches the platform to ACPI mode, enables the power button and the GPEs
/// the firmware has handlers for, and routes the interrupt to `destination`.
/// Needs the namespace loaded.
pub fn initialize(acpi: &AcpiInfo, destination: u32) -> EventReport {
    let mut report = EventReport::EMPTY;
    if !acpi.fadt_valid || acpi.sci_interrupt == 0 || acpi.sci_interrupt > 255 {
        *REPORT.lock() = report;
        return report;
    }
    report.sci = acpi.sci_interrupt;
    report.acpi_mode = acpi_mode(acpi);
    if !report.acpi_mode {
        *REPORT.lock() = report;
        return report;
    }
    publish_blocks(acpi);
    let fixed_button = acpi.fadt_flags & (1 << 4) == 0;
    let mut handlers = [GpeHandler::default(); MAX_HANDLERS];
    let mut count = acpi_ns::with(|aml| acpi_devices::scan_gpes(aml, &mut handlers));
    if let Some(ec) = acpi_ns::with(acpi_devices::find_ec) {
        acpi_ns::set_embedded_controller(ec.data, ec.command);
        acpi_ns::with(|aml| acpi_devices::announce_ec_region(aml, &ec));
        *EC.lock() = Some(ec);
        report.embedded_controller = true;
        if let Some(number) = ec.gpe
            && count < MAX_HANDLERS
            && !handlers[..count].iter().any(|h| h.number == number)
        {
            handlers[count] = GpeHandler {
                number,
                level: true,
                node: ec.node,
                ec: true,
            };
            count += 1;
        }
    }
    report.handlers = count;
    // SAFETY: ports named by the FADT.
    unsafe {
        let status = PM1_STATUS[0].load(Ordering::Relaxed) as u16;
        let enable = PM1_ENABLE[0].load(Ordering::Relaxed) as u16;
        if status != 0 {
            arch::outw(status, PM1_PWRBTN);
            if fixed_button {
                arch::outw(enable, arch::inw(enable) | PM1_PWRBTN);
                report.power_button = true;
            }
        }
        for block in 0..BLOCKS {
            let base = GPE_BASE[block].load(Ordering::Relaxed) as u16;
            let bytes = GPE_BYTES[block].load(Ordering::Relaxed) as u16;
            for byte in 0..bytes {
                // Everything off, and any stale status cleared.
                arch::outb(base + bytes + byte, 0);
                arch::outb(base + byte, 0xff);
            }
        }
        for handler in &handlers[..count] {
            let Some((block, bit)) = gpe_location(acpi, handler.number) else {
                continue;
            };
            let base = GPE_BASE[block].load(Ordering::Relaxed) as u16;
            let bytes = GPE_BYTES[block].load(Ordering::Relaxed) as u16;
            let (byte, mask) = (bit as u16 / 8, 1u8 << (bit % 8));
            let enable = base + bytes + byte;
            arch::outb(enable, arch::inb(enable) | mask);
            if handler.level {
                LEVEL[handler.number as usize / 64]
                    .fetch_or(1 << (handler.number % 64), Ordering::Relaxed);
            }
            report.enabled_gpes += 1;
        }
    }
    *HANDLERS.lock() = (handlers, count);
    if report.power_button || report.enabled_gpes != 0 {
        report.routed =
            ioapic::route_legacy_irq(acpi, acpi.sci_interrupt as u8, destination, VECTOR);
    }
    *REPORT.lock() = report;
    report
}

pub fn report() -> EventReport {
    *REPORT.lock()
}

/// Called from the interrupt vector. Acknowledges the hardware and records
/// what happened; runs no AML.
pub fn handle_interrupt() {
    INTERRUPTS.fetch_add(1, Ordering::Relaxed);
    let mut handled = false;
    // SAFETY: only ports published at initialisation, before the route existed.
    unsafe {
        for block in 0..BLOCKS {
            let status = PM1_STATUS[block].load(Ordering::Relaxed) as u16;
            if status == 0 {
                continue;
            }
            let enable = PM1_ENABLE[block].load(Ordering::Relaxed) as u16;
            if arch::inw(status) & arch::inw(enable) & PM1_PWRBTN != 0 {
                arch::outw(status, PM1_PWRBTN);
                POWER_BUTTON.store(true, Ordering::Release);
                handled = true;
            }
        }
        for block in 0..BLOCKS {
            let base = GPE_BASE[block].load(Ordering::Relaxed) as u16;
            let bytes = GPE_BYTES[block].load(Ordering::Relaxed) as u16;
            let first = GPE_FIRST[block].load(Ordering::Relaxed);
            for byte in 0..bytes {
                let enable_port = base + bytes + byte;
                let enabled = arch::inb(enable_port);
                let active = arch::inb(base + byte) & enabled;
                if active == 0 {
                    continue;
                }
                handled = true;
                // Masked until the handler has run; edge events are
                // acknowledged now, level events once their handler is done.
                arch::outb(enable_port, enabled & !active);
                let mut edge = 0u8;
                for bit in 0..8u32 {
                    if active & (1 << bit) == 0 {
                        continue;
                    }
                    let number = first + byte as u32 * 8 + bit;
                    let slot = number as usize / 64 % 4;
                    let mask = 1u64 << (number % 64);
                    PENDING[slot].fetch_or(mask, Ordering::AcqRel);
                    if LEVEL[slot].load(Ordering::Relaxed) & mask == 0 {
                        edge |= 1 << bit;
                    }
                }
                if edge != 0 {
                    arch::outb(base + byte, edge);
                }
            }
        }
        if !handled && SPURIOUS.fetch_add(1, Ordering::Relaxed) >= STORM_LIMIT {
            shut_off();
        }
    }
}

/// Stops every source from raising the interrupt (a stuck line).
unsafe fn shut_off() {
    SHUT_OFF.store(true, Ordering::Release);
    unsafe {
        for block in 0..BLOCKS {
            let enable = PM1_ENABLE[block].load(Ordering::Relaxed) as u16;
            if enable != 0 {
                arch::outw(enable, 0);
            }
            let base = GPE_BASE[block].load(Ordering::Relaxed) as u16;
            let bytes = GPE_BYTES[block].load(Ordering::Relaxed) as u16;
            for byte in 0..bytes {
                arch::outb(base + bytes + byte, 0);
            }
        }
    }
}

pub fn interrupts() -> u32 {
    INTERRUPTS.load(Ordering::Relaxed)
}

pub fn events_run() -> u32 {
    EVENTS_RUN.load(Ordering::Relaxed)
}

pub fn line_shut_off() -> bool {
    SHUT_OFF.load(Ordering::Acquire)
}

pub fn device_changes() -> u32 {
    DEVICE_CHANGES.load(Ordering::Relaxed)
}

/// True once per press of the power button.
pub fn take_power_button() -> bool {
    POWER_BUTTON.swap(false, Ordering::AcqRel)
}

fn reenable(number: u32, level: bool) {
    let gpe = [
        (
            GPE_BASE[0].load(Ordering::Relaxed),
            GPE_BYTES[0].load(Ordering::Relaxed),
            GPE_FIRST[0].load(Ordering::Relaxed),
        ),
        (
            GPE_BASE[1].load(Ordering::Relaxed),
            GPE_BYTES[1].load(Ordering::Relaxed),
            GPE_FIRST[1].load(Ordering::Relaxed),
        ),
    ];
    for (base, bytes, first) in gpe {
        if base == 0 || number < first || number >= first + bytes * 8 {
            continue;
        }
        let bit = number - first;
        let (byte, mask) = (bit as u16 / 8, 1u8 << (bit % 8));
        let (status, enable) = (base as u16 + byte, base as u16 + bytes as u16 + byte);
        without_interrupts(|| {
            // SAFETY: a port inside the FADT's GPE block.
            unsafe {
                if level {
                    arch::outb(status, mask);
                }
                arch::outb(enable, arch::inb(enable) | mask);
            }
        });
        return;
    }
}

/// Boot-test handshake: announces that the guest is waiting, and gives the
/// harness up to half a minute to press the (virtual) power button.
#[cfg(feature = "boot-test")]
pub fn power_button_test() {
    let report = report();
    if !report.power_button || !report.routed {
        crate::serial::line("AEROS_POWERBTN pressed=false reason=not-routed");
        return;
    }
    crate::serial::line("AEROS_POWERBTN_WAIT");
    let start = crate::time::monotonic_nanoseconds();
    let mut pressed = false;
    while crate::time::monotonic_nanoseconds().saturating_sub(start) < 30_000_000_000 {
        if take_power_button() {
            pressed = true;
            break;
        }
        arch::wait_for_interrupt();
    }
    crate::serial::format(format_args!(
        "AEROS_POWERBTN pressed={pressed} interrupts={} line_shut_off={}\n",
        interrupts(),
        line_shut_off()
    ));
}

/// Runs the AML handlers of events that have fired since the last call and
/// notes the device changes they announce. Cheap when nothing is pending.
pub fn poll() {
    if PENDING.iter().all(|word| word.load(Ordering::Acquire) == 0) {
        return;
    }
    let (handlers, count) = *HANDLERS.lock();
    for (slot, word) in PENDING.iter().enumerate() {
        let mut pending = word.swap(0, Ordering::AcqRel);
        while pending != 0 {
            let bit = pending.trailing_zeros();
            pending &= pending - 1;
            let number = slot as u32 * 64 + bit;
            let Some(handler) = handlers[..count].iter().find(|h| h.number as u32 == number) else {
                continue;
            };
            let ec = *EC.lock();
            let changes = acpi_ns::with(|aml| {
                match ec {
                    Some(ec) if handler.ec => {
                        // Bounded: a controller that never stops reporting
                        // must not hold this thread.
                        for _ in 0..32 {
                            let Some(code) = acpi_ns::ec_query() else {
                                break;
                            };
                            acpi_devices::run_ec_query(aml, &ec, code);
                        }
                    }
                    _ => {
                        let _ = aml.evaluate_node(handler.node, &[]);
                    }
                }
                let mut changes = 0u32;
                while aml.take_notification().is_some() {
                    changes += 1;
                }
                changes
            });
            DEVICE_CHANGES.fetch_add(changes, Ordering::Relaxed);
            EVENTS_RUN.fetch_add(1, Ordering::Relaxed);
            reenable(number, handler.level);
        }
    }
}
