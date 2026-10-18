//! The TPM behind the firmware's TPM2 table, reached through the TIS
//! registers. Nothing is touched unless the firmware lists a TPM.

use crate::acpi::{self, AcpiInfo};
use crate::sync::TicketLock;
use crate::tpm::{self, Bus, Tis};

struct Mmio;

impl Bus for Mmio {
    fn read8(&mut self, offset: u64) -> u8 {
        // SAFETY: the range was checked to be mapped before the bus was made.
        unsafe { core::ptr::read_volatile((tpm::TIS_BASE + offset) as usize as *const u8) }
    }

    fn read32(&mut self, offset: u64) -> u32 {
        // SAFETY: as above.
        unsafe { core::ptr::read_volatile((tpm::TIS_BASE + offset) as usize as *const u32) }
    }

    fn write8(&mut self, offset: u64, value: u8) {
        // SAFETY: as above.
        unsafe { core::ptr::write_volatile((tpm::TIS_BASE + offset) as usize as *mut u8, value) }
    }

    fn write32(&mut self, offset: u64, value: u32) {
        // SAFETY: as above.
        unsafe { core::ptr::write_volatile((tpm::TIS_BASE + offset) as usize as *mut u32, value) }
    }

    fn now_ns(&mut self) -> u64 {
        crate::time::monotonic_nanoseconds()
    }
}

static DEVICE: TicketLock<Option<(Tis<Mmio>, u32)>> = TicketLock::new(None);

/// Looks for a TPM: the firmware must list one and the registers must answer.
pub fn initialize(acpi_info: &AcpiInfo) -> bool {
    let mut listed = false;
    // SAFETY: the root table was validated by `acpi::inspect`.
    unsafe {
        acpi::each_table(acpi_info, |signature, _, _| {
            listed |= signature == *b"TPM2";
        });
    }
    if !listed || !crate::arch::paging::kernel_range_mapped(tpm::TIS_BASE, 0x1000) {
        return false;
    }
    match Tis::open(Mmio) {
        Ok(device) => {
            *DEVICE.lock() = Some(device);
            true
        }
        Err(_) => false,
    }
}

pub fn present() -> Option<u32> {
    DEVICE.lock().as_ref().map(|(_, id)| *id)
}

/// Runs one command; the response lands in `response`.
pub fn run(
    command: &tpm::Command,
    response: &mut [u8; tpm::MAX_RESPONSE],
) -> Result<usize, tpm::Error> {
    match DEVICE.lock().as_mut() {
        Some((device, _)) => device.run(command, response),
        None => Err(tpm::Error::NoDevice),
    }
}
