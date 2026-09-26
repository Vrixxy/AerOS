//! SD Host Controller (SDHCI, PCI class 08:05) driver with an SD/SDHC memory
//! card behind it, polled and PIO-only (data moves through the buffer data
//! port, no DMA).

use crate::pci::PciInventory;
use crate::sync::TicketLock;

const BLOCK_SIZE: usize = 0x04;
const BLOCK_COUNT: usize = 0x06;
const ARGUMENT: usize = 0x08;
const TRANSFER_MODE: usize = 0x0c;
const RESPONSE: usize = 0x10;
const BUFFER_PORT: usize = 0x20;
const PRESENT_STATE: usize = 0x24;
const POWER: usize = 0x29;
const CLOCK: usize = 0x2c;
const TIMEOUT_CONTROL: usize = 0x2e;
const RESET: usize = 0x2f;
const INT_STATUS: usize = 0x30;
const ERROR_STATUS: usize = 0x32;
const INT_ENABLE: usize = 0x34;
const ERROR_ENABLE: usize = 0x36;
const CAPABILITIES: usize = 0x40;

const INT_COMMAND_COMPLETE: u16 = 1 << 0;
const INT_TRANSFER_COMPLETE: u16 = 1 << 1;
const INT_BUFFER_WRITE_READY: u16 = 1 << 4;
const INT_BUFFER_READ_READY: u16 = 1 << 5;
const INT_ERROR: u16 = 1 << 15;

const RESPONSE_NONE: u16 = 0;
const RESPONSE_136: u16 = 1;
const RESPONSE_48: u16 = 2;
const RESPONSE_48_BUSY: u16 = 3;
const CRC_CHECK: u16 = 1 << 3;
const INDEX_CHECK: u16 = 1 << 4;
const DATA_PRESENT: u16 = 1 << 5;

#[derive(Clone, Copy)]
pub struct SdReport {
    pub present: bool,
    pub mmio: u64,
    pub version: u8,
    pub card: bool,
    pub high_capacity: bool,
    pub sectors: u64,
    pub read: bool,
    pub write_probe: bool,
    pub verified: bool,
}

impl SdReport {
    pub const EMPTY: Self = Self {
        present: false,
        mmio: 0,
        version: 0,
        card: false,
        high_capacity: false,
        sectors: 0,
        read: false,
        write_probe: false,
        verified: false,
    };
}

struct State {
    mmio: u64,
    high_capacity: bool,
    sectors: u64,
    ready: bool,
}

static SD: TicketLock<State> = TicketLock::new(State {
    mmio: 0,
    high_capacity: false,
    sectors: 0,
    ready: false,
});

fn read8(base: u64, offset: usize) -> u8 {
    unsafe { core::ptr::read_volatile((base as usize + offset) as *const u8) }
}

fn read16(base: u64, offset: usize) -> u16 {
    unsafe { core::ptr::read_volatile((base as usize + offset) as *const u16) }
}

fn read32(base: u64, offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base as usize + offset) as *const u32) }
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

fn wait(mut condition: impl FnMut() -> bool) -> bool {
    let start = crate::time::monotonic_nanoseconds();
    while crate::time::monotonic_nanoseconds().saturating_sub(start) < 500_000_000 {
        if condition() {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

fn delay_ms(milliseconds: u64) {
    let start = crate::time::monotonic_nanoseconds();
    while crate::time::monotonic_nanoseconds().saturating_sub(start) < milliseconds * 1_000_000 {
        core::hint::spin_loop();
    }
}

/// Sets SDCLK to the fastest divisor of the base clock not above `target_khz`.
fn set_clock(mmio: u64, target_khz: u32) -> bool {
    let capabilities = read32(mmio, CAPABILITIES);
    let version = read16(mmio, 0xfe) & 0xff;
    let base_mhz = if version >= 2 {
        (capabilities >> 8) & 0xff
    } else {
        (capabilities >> 8) & 0x3f
    };
    let base_khz = if base_mhz == 0 {
        100_000
    } else {
        base_mhz * 1000
    };
    write16(mmio, CLOCK, 0);
    // v3 uses a 10-bit divisor N (SDCLK = base / (2N)); v2 a power of two.
    let mut divisor = 1u32;
    while base_khz / (2 * divisor) > target_khz && divisor < 1023 {
        divisor += 1;
    }
    // Spec 2.00 controllers only divide by powers of two.
    let divisor = if version < 2 {
        divisor.next_power_of_two().min(128)
    } else {
        divisor
    };
    let divisor = if base_khz <= target_khz { 0 } else { divisor };
    let encoded = ((divisor & 0xff) << 8) | ((divisor >> 8) & 3) << 6;
    write16(mmio, CLOCK, encoded as u16 | 1);
    if !wait(|| read16(mmio, CLOCK) & 2 != 0) {
        return false;
    }
    write16(mmio, CLOCK, encoded as u16 | 1 | 4);
    true
}

/// Sends one command and returns its four response words.
fn command(mmio: u64, index: u32, argument: u32, flags: u16, data: bool) -> Option<[u32; 4]> {
    let inhibit = if data { 0x3 } else { 0x1 };
    if !wait(|| read32(mmio, PRESENT_STATE) & inhibit == 0) {
        return None;
    }
    write16(mmio, INT_STATUS, 0xffff);
    write16(mmio, ERROR_STATUS, 0xffff);
    write32(mmio, ARGUMENT, argument);
    let command = (index as u16) << 8 | flags;
    write16(mmio, 0x0e, command);
    let mut status = 0u16;
    if !wait(|| {
        status = read16(mmio, INT_STATUS);
        status & (INT_COMMAND_COMPLETE | INT_ERROR) != 0
    }) || status & INT_ERROR != 0
    {
        write16(mmio, INT_STATUS, 0xffff);
        write16(mmio, ERROR_STATUS, 0xffff);
        // Reset the command line after an error.
        write8(mmio, RESET, 2);
        wait(|| read8(mmio, RESET) & 2 == 0);
        return None;
    }
    write16(mmio, INT_STATUS, INT_COMMAND_COMPLETE);
    let mut response = [0u32; 4];
    for (slot, word) in response.iter_mut().enumerate() {
        *word = read32(mmio, RESPONSE + slot * 4);
    }
    if flags & 3 == RESPONSE_48_BUSY {
        wait(|| read16(mmio, INT_STATUS) & INT_TRANSFER_COMPLETE != 0);
        write16(mmio, INT_STATUS, INT_TRANSFER_COMPLETE);
    }
    Some(response)
}

fn app_command(mmio: u64, rca: u32, index: u32, argument: u32, flags: u16) -> Option<[u32; 4]> {
    command(
        mmio,
        55,
        rca << 16,
        RESPONSE_48 | CRC_CHECK | INDEX_CHECK,
        false,
    )?;
    command(mmio, index, argument, flags, false)
}

pub fn initialize(pci: &PciInventory) -> SdReport {
    let Some(device) = pci
        .devices()
        .iter()
        .copied()
        .find(|device| device.class == 0x08 && device.subclass == 0x05)
    else {
        return SdReport::EMPTY;
    };
    if device.bars[0] & 1 != 0
        || device.bars[0] & 0xffff_ff00 == 0
        || !pci.enable_memory_bus_master(device)
    {
        return SdReport::EMPTY;
    }
    let mut mmio = (device.bars[0] & 0xffff_ff00) as u64;
    if device.bars[0] & 0x6 == 0x4 {
        mmio |= (device.bars[1] as u64) << 32;
    }
    let mut report = SdReport {
        present: true,
        mmio,
        version: (read16(mmio, 0xfe) & 0xff) as u8 + 1,
        ..SdReport::EMPTY
    };
    // Reset the whole controller.
    write8(mmio, RESET, 1);
    if !wait(|| read8(mmio, RESET) & 1 == 0) {
        return report;
    }
    write16(mmio, INT_ENABLE, !(1u16 << 6 | 1 << 7 | 1 << 8));
    write16(mmio, ERROR_ENABLE, 0xffff);
    write16(mmio, 0x38, 0); // no interrupt signals: polled
    let capabilities = read32(mmio, CAPABILITIES);
    // 3.3 V (or 3.0/1.8 V when that is all the controller offers).
    let power = if capabilities & (1 << 24) != 0 {
        0x0e
    } else if capabilities & (1 << 25) != 0 {
        0x0c
    } else {
        0x0a
    };
    write8(mmio, POWER, power | 1);
    write8(mmio, TIMEOUT_CONTROL, 0x0e);
    if !set_clock(mmio, 400) {
        return report;
    }
    delay_ms(10);
    // Card identification.
    if command(mmio, 0, 0, RESPONSE_NONE, false).is_none() {
        return report;
    }
    delay_ms(2);
    let v2 = command(mmio, 8, 0x1aa, RESPONSE_48 | CRC_CHECK | INDEX_CHECK, false)
        .is_some_and(|response| response[0] & 0xfff == 0x1aa);
    let mut ocr = 0u32;
    let mut ready = false;
    for _ in 0..200 {
        let argument = 0x00ff_8000 | if v2 { 1 << 30 } else { 0 };
        let Some(response) = app_command(mmio, 0, 41, argument, RESPONSE_48) else {
            return report;
        };
        ocr = response[0];
        if ocr & (1 << 31) != 0 {
            ready = true;
            break;
        }
        delay_ms(5);
    }
    if !ready {
        return report;
    }
    report.card = true;
    report.high_capacity = ocr & (1 << 30) != 0;
    if command(mmio, 2, 0, RESPONSE_136 | CRC_CHECK, false).is_none() {
        return report;
    }
    let Some(rca_response) = command(mmio, 3, 0, RESPONSE_48 | CRC_CHECK | INDEX_CHECK, false)
    else {
        return report;
    };
    let rca = rca_response[0] >> 16;
    // Capacity from the CSD.
    let Some(csd) = command(mmio, 9, rca << 16, RESPONSE_136 | CRC_CHECK, false) else {
        return report;
    };
    report.sectors = if (csd[3] >> 22) & 3 == 1 {
        (((csd[1] >> 8) & 0x3f_ffff) as u64 + 1) * 1024
    } else {
        let c_size = ((csd[1] >> 22) & 0x3ff) | ((csd[2] & 3) << 10);
        let multiplier = (csd[1] >> 7) & 7;
        let block_length = 1u64 << ((csd[2] >> 8) & 0xf);
        ((c_size as u64 + 1) << (multiplier + 2)) * block_length / 512
    };
    if command(
        mmio,
        7,
        rca << 16,
        RESPONSE_48_BUSY | CRC_CHECK | INDEX_CHECK,
        false,
    )
    .is_none()
    {
        return report;
    }
    if !report.high_capacity
        && command(mmio, 16, 512, RESPONSE_48 | CRC_CHECK | INDEX_CHECK, false).is_none()
    {
        return report;
    }
    // Full speed for data transfers.
    let _ = set_clock(mmio, 25_000);
    *SD.lock() = State {
        mmio,
        high_capacity: report.high_capacity,
        sectors: report.sectors,
        ready: true,
    };

    // Self-test: sector 0 carries the signature; 8-sector write/read-back at sector 8.
    let mut sector = [0u8; 512];
    report.read = read(0, 1, &mut sector) && sector[..14] == *b"AEROS-SD-TEST!";
    let mut pattern = [0u8; 4096];
    for (index, byte) in pattern.iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(29).wrapping_add(5);
    }
    let mut readback = [0u8; 4096];
    report.write_probe = report.sectors > 16
        && write(8, 8, &pattern)
        && read(8, 8, &mut readback)
        && readback == pattern;
    report.verified = report.read && report.write_probe;
    report
}

fn transfer_one(
    mmio: u64,
    high_capacity: bool,
    lba: u64,
    buffer: &mut [u8; 512],
    write: bool,
) -> bool {
    let address = if high_capacity { lba } else { lba * 512 } as u32;
    if !wait(|| read32(mmio, PRESENT_STATE) & 3 == 0) {
        return false;
    }
    write16(mmio, INT_STATUS, 0xffff);
    write16(mmio, ERROR_STATUS, 0xffff);
    write16(mmio, BLOCK_SIZE, 512);
    write16(mmio, BLOCK_COUNT, 1);
    write32(mmio, ARGUMENT, address);
    // Transfer mode: single block, read direction for reads. Written together
    // with the command as one 32-bit access.
    let mode: u32 = if write { 0 } else { 1 << 4 };
    let index = if write { 24u32 } else { 17 };
    let command =
        ((index << 8) as u16 | RESPONSE_48 | CRC_CHECK | INDEX_CHECK | DATA_PRESENT) as u32;
    write32(mmio, TRANSFER_MODE, mode | command << 16);
    let ready_bit = if write {
        INT_BUFFER_WRITE_READY
    } else {
        INT_BUFFER_READ_READY
    };
    let mut status = 0u16;
    if !wait(|| {
        status = read16(mmio, INT_STATUS);
        status & (ready_bit | INT_ERROR) != 0
    }) || status & INT_ERROR != 0
    {
        write8(mmio, RESET, 6);
        wait(|| read8(mmio, RESET) & 6 == 0);
        return false;
    }
    write16(mmio, INT_STATUS, ready_bit | INT_COMMAND_COMPLETE);
    for word in 0..128 {
        if write {
            let value = u32::from_le_bytes([
                buffer[word * 4],
                buffer[word * 4 + 1],
                buffer[word * 4 + 2],
                buffer[word * 4 + 3],
            ]);
            write32(mmio, BUFFER_PORT, value);
        } else {
            let value = read32(mmio, BUFFER_PORT).to_le_bytes();
            buffer[word * 4..word * 4 + 4].copy_from_slice(&value);
        }
    }
    let done = wait(|| {
        status = read16(mmio, INT_STATUS);
        status & (INT_TRANSFER_COMPLETE | INT_ERROR) != 0
    }) && status & INT_ERROR == 0;
    write16(mmio, INT_STATUS, 0xffff);
    done
}

/// Sectors on the SD card (0 = none).
pub fn sectors() -> u64 {
    let state = SD.lock();
    if state.ready { state.sectors } else { 0 }
}

/// Reads `count` 512-byte sectors starting at `lba`.
pub fn read(lba: u64, count: usize, destination: &mut [u8]) -> bool {
    let (mmio, high_capacity, ready, sectors) = {
        let state = SD.lock();
        (state.mmio, state.high_capacity, state.ready, state.sectors)
    };
    if !ready || destination.len() < count * 512 || lba + count as u64 > sectors {
        return false;
    }
    let _guard = SD.lock();
    let mut sector = [0u8; 512];
    for index in 0..count {
        if !transfer_one(mmio, high_capacity, lba + index as u64, &mut sector, false) {
            return false;
        }
        destination[index * 512..index * 512 + 512].copy_from_slice(&sector);
    }
    true
}

/// Writes `count` 512-byte sectors starting at `lba`.
pub fn write(lba: u64, count: usize, source: &[u8]) -> bool {
    let (mmio, high_capacity, ready, sectors) = {
        let state = SD.lock();
        (state.mmio, state.high_capacity, state.ready, state.sectors)
    };
    if !ready || source.len() < count * 512 || lba + count as u64 > sectors {
        return false;
    }
    let _guard = SD.lock();
    let mut sector = [0u8; 512];
    for index in 0..count {
        sector.copy_from_slice(&source[index * 512..index * 512 + 512]);
        if !transfer_one(mmio, high_capacity, lba + index as u64, &mut sector, true) {
            return false;
        }
    }
    true
}
