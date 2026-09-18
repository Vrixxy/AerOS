use crate::acpi::{self, AcpiInfo};
use crate::arch;
use crate::sync::TicketLock;

const SLP_EN: u16 = 1 << 13;

static STATE: TicketLock<PowerReport> = TicketLock::new(PowerReport::EMPTY);

#[derive(Clone, Copy)]
pub struct PowerReport {
    pub fadt_present: bool,
    pub dsdt_present: bool,
    pub s5_found: bool,
    pub slp_typ_a: u8,
    pub slp_typ_b: u8,
    pub pm1a_control_block: u32,
    pub pm1b_control_block: u32,
    pub ready: bool,
}

impl PowerReport {
    const EMPTY: Self = Self {
        fadt_present: false,
        dsdt_present: false,
        s5_found: false,
        slp_typ_a: 0,
        slp_typ_b: 0,
        pm1a_control_block: 0,
        pm1b_control_block: 0,
        ready: false,
    };
}

/// Locates the ACPI S5 (soft-off) sleep-type values by scanning the DSDT for
/// the `_S5_` AML package, following the widely-used minimal-AML-parse
/// technique (full AML bytecode interpretation is out of scope; this reads
/// only the handful of bytes that encode the two SLP_TYP constants). This
/// function only inspects tables and never touches hardware state.
pub fn inspect(acpi: &AcpiInfo) -> PowerReport {
    let mut report = PowerReport {
        fadt_present: acpi.fadt_valid,
        pm1a_control_block: acpi.pm1a_control_block,
        pm1b_control_block: acpi.pm1b_control_block,
        ..PowerReport::EMPTY
    };
    if !acpi.fadt_valid || acpi.dsdt_address == 0 {
        return report;
    }
    let base = acpi.dsdt_address as usize as *const u8;
    let Some(length) = (unsafe { acpi::validate_sdt(base, Some(*b"DSDT")) }) else {
        return report;
    };
    report.dsdt_present = true;
    if let Some((typ_a, typ_b)) = unsafe { scan_s5(base, length) } {
        report.s5_found = true;
        report.slp_typ_a = typ_a;
        report.slp_typ_b = typ_b;
    }
    report.ready = report.fadt_present && report.s5_found;
    report
}

/// Stores the boot-time power report so later readers (the shell's
/// `shutdown` command) don't need to re-parse ACPI tables.
pub fn set(report: PowerReport) {
    *STATE.lock() = report;
}

pub fn current() -> PowerReport {
    *STATE.lock()
}

unsafe fn scan_s5(base: *const u8, length: usize) -> Option<(u8, u8)> {
    if length < 9 {
        return None;
    }
    let marker = *b"_S5_";
    let mut offset = 0usize;
    while offset + 4 <= length {
        let matches = (0..4).all(|index| unsafe {
            core::ptr::read_volatile(base.add(offset + index)) == marker[index]
        });
        if matches && let Some(values) = unsafe { parse_s5_package(base, offset + 4, length) } {
            return Some(values);
        }
        offset += 1;
    }
    None
}

unsafe fn parse_s5_package(base: *const u8, mut offset: usize, length: usize) -> Option<(u8, u8)> {
    let byte_at = |index: usize| -> Option<u8> {
        if index >= length {
            None
        } else {
            Some(unsafe { core::ptr::read_volatile(base.add(index)) })
        }
    };
    // Optional NameOp prefix (0x08 '_' 'S' '5' '_') already matched via the
    // marker; some DSDTs place a PackageOp (0x12) directly after the name,
    // others via an intermediate NameString - only the direct case is
    // handled, which covers what QEMU/OVMF and SeaBIOS emit.
    if byte_at(offset)? != 0x12 {
        return None;
    }
    offset += 1;
    let lead = byte_at(offset)?;
    let extra_bytes = (lead >> 6) & 0x3;
    offset += 1 + extra_bytes as usize;
    // NumElements byte.
    offset += 1;
    let read_element = |offset: &mut usize| -> Option<u8> {
        let mut value = byte_at(*offset)?;
        if value == 0x0a {
            // BytePrefix - the actual constant follows.
            *offset += 1;
            value = byte_at(*offset)?;
        } else if value > 0x07 {
            // Not a small inline constant or a recognized encoding.
            return None;
        }
        *offset += 1;
        Some(value)
    };
    let typ_a = read_element(&mut offset)?;
    let typ_b = read_element(&mut offset)?;
    Some((typ_a, typ_b))
}

/// Performs a real ACPI S5 soft power-off by writing SLP_TYP|SLP_EN into the
/// PM1 control block(s). Only call this interactively (e.g. from a shell
/// command) - never from the boot self-test path, since a successful call
/// does not return.
pub fn shutdown(report: &PowerReport) -> ! {
    if report.ready && report.pm1a_control_block != 0 && report.pm1a_control_block <= 0xffff {
        let port_a = report.pm1a_control_block as u16;
        let value_a = SLP_EN | ((report.slp_typ_a as u16) << 10);
        unsafe {
            arch::outw(port_a, value_a);
        }
        if report.pm1b_control_block != 0 && report.pm1b_control_block <= 0xffff {
            let port_b = report.pm1b_control_block as u16;
            let value_b = SLP_EN | ((report.slp_typ_b as u16) << 10);
            unsafe {
                arch::outw(port_b, value_b);
            }
        }
    }
    arch::halt_forever()
}
