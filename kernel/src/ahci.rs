use core::hint::spin_loop;

use crate::memory::FrameAllocator;
use crate::pci::PciInventory;
use crate::sync::TicketLock;

const PAGE_SIZE: u64 = 4096;
const PORT_BASE: usize = 0x100;
const PORT_STRIDE: usize = 0x80;
const SATA_SIGNATURE: u32 = 0x0000_0101;
const PORT_IS: usize = 0x10;
const PORT_CMD: usize = 0x18;
const PORT_TFD: usize = 0x20;
const PORT_SIG: usize = 0x24;
const PORT_SSTS: usize = 0x28;
const PORT_SERR: usize = 0x30;
const PORT_SACT: usize = 0x34;
const PORT_CI: usize = 0x38;
const PORT_CLB: usize = 0x00;
const PORT_CLBU: usize = 0x04;
const PORT_FB: usize = 0x08;
const PORT_FBU: usize = 0x0c;
const COMMAND_TIMEOUT: usize = 20_000_000;
const MAX_DISKS: usize = 6;
const MAX_TRANSFER_SECTORS: u32 = 8192;

const CMD_READ_DMA_EXT: u8 = 0x25;
const CMD_READ_DMA: u8 = 0xc8;
const CMD_WRITE_DMA_EXT: u8 = 0x35;
const CMD_WRITE_DMA: u8 = 0xca;
const CMD_IDENTIFY: u8 = 0xec;
const CMD_FLUSH_EXT: u8 = 0xea;
const CMD_FLUSH: u8 = 0xe7;

#[derive(Clone, Copy)]
pub struct AhciReport {
    pub present: bool,
    pub abar: u64,
    pub version: u32,
    pub implemented_ports: u32,
    pub active_ports: u32,
    pub sata_devices: u32,
    pub command_slots: u32,
    pub dma64: bool,
    pub identify: bool,
    pub read: bool,
    pub write_probe: bool,
    pub disks: u32,
    pub sectors: u64,
    pub sector_bytes: u32,
    pub boot_crc32: u32,
    pub model: [u8; 40],
    pub model_length: usize,
    pub verified: bool,
}

#[derive(Clone, Copy)]
struct Disk {
    port: u64,
    command_list: u64,
    command_table: u64,
    data: u64,
    sectors: u64,
    lba48: bool,
    ready: bool,
}

impl Disk {
    const EMPTY: Self = Self {
        port: 0,
        command_list: 0,
        command_table: 0,
        data: 0,
        sectors: 0,
        lba48: false,
        ready: false,
    };
}

struct AhciState {
    disks: [Disk; MAX_DISKS],
    count: usize,
    boot: usize,
}

impl AhciState {
    const EMPTY: Self = Self {
        disks: [Disk::EMPTY; MAX_DISKS],
        count: 0,
        boot: 0,
    };
}

static CONTROLLER: TicketLock<AhciState> = TicketLock::new(AhciState::EMPTY);
/// The command list/table of a port is shared, so only one command may be in
/// flight: the Linux guest's disk reads run on another CPU (see svm.rs) while
/// this CPU can still touch the disk (file saves), and they must not overlap.
static IO_LOCK: TicketLock<()> = TicketLock::new(());

impl AhciReport {
    const EMPTY: Self = Self {
        present: false,
        abar: 0,
        version: 0,
        implemented_ports: 0,
        active_ports: 0,
        sata_devices: 0,
        command_slots: 0,
        dma64: false,
        identify: false,
        read: false,
        write_probe: false,
        disks: 0,
        sectors: 0,
        sector_bytes: 0,
        boot_crc32: 0,
        model: [0; 40],
        model_length: 0,
        verified: false,
    };

    pub fn model(&self) -> &str {
        core::str::from_utf8(&self.model[..self.model_length]).unwrap_or("unknown")
    }
}

pub fn initialize(pci: &PciInventory, frames: &mut FrameAllocator) -> AhciReport {
    let Some(controller) = pci.find_class(0x01, 0x06, 0x01) else {
        return AhciReport::EMPTY;
    };
    let bar = controller.bars[5];
    if bar & 1 != 0 || bar & 0xffff_fff0 == 0 || !pci.enable_memory_bus_master(controller) {
        return AhciReport::EMPTY;
    }
    let abar = (bar & 0xffff_fff0) as u64;
    let capability = unsafe { read_register(abar, 0x00) };
    let version = unsafe { read_register(abar, 0x10) };
    let ports = unsafe { read_register(abar, 0x0c) };
    let command_slots = ((capability >> 8) & 0x1f) + 1;
    let dma64 = capability & (1 << 31) != 0;
    let max_ports = (capability & 0x1f) + 1;
    let valid_mask = if max_ports == 32 {
        u32::MAX
    } else {
        (1u32 << max_ports) - 1
    };
    if ports & !valid_mask != 0 {
        return AhciReport::EMPTY;
    }
    unsafe {
        let control = read_register(abar, 0x04);
        write_register(abar, 0x04, control | (1 << 31));
    }

    let mut report = AhciReport {
        present: true,
        abar,
        version,
        implemented_ports: ports.count_ones(),
        command_slots,
        dma64,
        ..AhciReport::EMPTY
    };

    let mut state = AhciState::EMPTY;
    for port in 0..32usize {
        if state.count >= MAX_DISKS || ports & (1 << port) == 0 {
            continue;
        }
        let base = port_base(abar, port);
        let status = unsafe { read_register(base, PORT_SSTS) };
        if status & 0x0f != 3 || status >> 8 & 0x0f != 1 {
            continue;
        }
        report.active_ports += 1;
        if unsafe { read_register(base, PORT_SIG) } != SATA_SIGNATURE {
            continue;
        }
        report.sata_devices += 1;

        let Some(dma) = frames.allocate_contiguous(4, 1) else {
            continue;
        };
        let dma_base = dma.address();
        if !dma64
            && dma_base
                .checked_add(PAGE_SIZE * 4)
                .is_none_or(|end| end > u32::MAX as u64)
        {
            continue;
        }
        unsafe {
            core::ptr::write_bytes(dma_base as usize as *mut u8, 0, (PAGE_SIZE * 4) as usize);
        }
        let command_list = dma_base;
        let received_fis = dma_base + PAGE_SIZE;
        let command_table = dma_base + PAGE_SIZE * 2;
        let data = dma_base + PAGE_SIZE * 3;
        if !configure_port(base, command_list, received_fis) {
            continue;
        }
        if !issue_command(
            base,
            command_list,
            command_table,
            data,
            CMD_IDENTIFY,
            false,
            0,
        ) {
            continue;
        }
        let words = unsafe { core::slice::from_raw_parts(data as usize as *const u16, 256) };
        let lba48 = words[83] & (1 << 10) != 0;
        let sectors = if lba48 {
            words[100] as u64
                | (words[101] as u64) << 16
                | (words[102] as u64) << 32
                | (words[103] as u64) << 48
        } else {
            words[60] as u64 | (words[61] as u64) << 16
        };
        if sectors == 0 {
            continue;
        }

        if state.count == 0 {
            let identify = unsafe { core::slice::from_raw_parts(data as usize as *const u8, 512) };
            for index in 0..20 {
                report.model[index * 2] = identify[(27 + index) * 2 + 1];
                report.model[index * 2 + 1] = identify[(27 + index) * 2];
            }
            report.model_length = report
                .model
                .iter()
                .rposition(|byte| *byte != b' ' && *byte != 0)
                .map_or(0, |index| index + 1);
            report.sectors = sectors;
            report.sector_bytes = if words[106] & 0xd000 == 0x5000 {
                (words[117] as u32 | (words[118] as u32) << 16).saturating_mul(2)
            } else {
                512
            };
        }

        state.disks[state.count] = Disk {
            port: base,
            command_list,
            command_table,
            data,
            sectors,
            lba48,
            ready: true,
        };
        state.count += 1;
    }

    if state.count == 0 {
        return report;
    }
    report.disks = state.count as u32;

    let boot = &state.disks[0];
    unsafe {
        core::ptr::write_bytes(boot.data as usize as *mut u8, 0, 512);
    }
    report.read = issue_command(
        boot.port,
        boot.command_list,
        boot.command_table,
        boot.data,
        if boot.lba48 {
            CMD_READ_DMA_EXT
        } else {
            CMD_READ_DMA
        },
        boot.lba48,
        0,
    );
    if report.read {
        let sector = unsafe { core::slice::from_raw_parts(boot.data as usize as *const u8, 512) };
        report.boot_crc32 = crc32(sector);
        report.read = sector[510] == 0x55 && sector[511] == 0xaa;
    }
    report.identify = true;

    if state.count > 1 {
        report.write_probe = write_probe(&state.disks[state.count - 1]);
    } else {
        report.write_probe = true;
    }

    report.verified = report.present
        && report.active_ports != 0
        && report.sata_devices != 0
        && report.command_slots != 0
        && report.identify
        && report.read
        && report.sectors != 0
        && report.sector_bytes >= 512
        && report.model_length != 0;

    if report.verified {
        *CONTROLLER.lock() = state;
    }
    report
}

fn write_probe(disk: &Disk) -> bool {
    if disk.sectors < 2 {
        return false;
    }
    let lba = disk.sectors - 1;
    let mut original = [0u8; 512];
    if !disk_read_sector_state(disk, lba, &mut original) {
        return false;
    }
    let mut probe = original;
    probe[0] ^= 0xa5;
    let mut check = [0u8; 512];
    disk_write_sector_state(disk, lba, &probe)
        && disk_read_sector_state(disk, lba, &mut check)
        && check[0] == probe[0]
        && disk_write_sector_state(disk, lba, &original)
}

pub fn disk_count() -> usize {
    CONTROLLER.lock().count
}

pub fn boot_disk() -> usize {
    CONTROLLER.lock().boot
}

pub fn disk_sectors(disk: usize) -> u64 {
    let state = CONTROLLER.lock();
    state.disks.get(disk).map_or(0, |d| d.sectors)
}

pub fn read_sector(lba: u64, destination: &mut [u8; 512]) -> bool {
    read_disk_sector(boot_disk(), lba, destination)
}

pub fn read_disk_sector(disk: usize, lba: u64, destination: &mut [u8; 512]) -> bool {
    let state = CONTROLLER.lock();
    let Some(d) = state.disks.get(disk).copied() else {
        return false;
    };
    disk_read_sector_state(&d, lba, destination)
}

pub fn write_disk_sector(disk: usize, lba: u64, source: &[u8; 512]) -> bool {
    let state = CONTROLLER.lock();
    let Some(d) = state.disks.get(disk).copied() else {
        return false;
    };
    disk_write_sector_state(&d, lba, source)
}

#[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
pub fn read_into(lba: u64, sectors: u32, dest_phys: u64) -> bool {
    read_disk(boot_disk(), lba, sectors, dest_phys)
}

pub fn read_disk(disk: usize, lba: u64, sectors: u32, dest_phys: u64) -> bool {
    transfer(disk, lba, sectors, dest_phys, false)
}

pub fn write_disk(disk: usize, lba: u64, sectors: u32, source_phys: u64) -> bool {
    transfer(disk, lba, sectors, source_phys, true)
}

pub fn flush_disk(disk: usize) -> bool {
    let state = CONTROLLER.lock();
    let Some(d) = state.disks.get(disk).copied() else {
        return false;
    };
    drop(state);
    issue_command(
        d.port,
        d.command_list,
        d.command_table,
        d.data,
        if d.lba48 { CMD_FLUSH_EXT } else { CMD_FLUSH },
        d.lba48,
        0,
    )
}

fn transfer(disk: usize, lba: u64, sectors: u32, buffer_phys: u64, write: bool) -> bool {
    if sectors == 0 || sectors > MAX_TRANSFER_SECTORS || buffer_phys & 1 != 0 {
        return false;
    }
    let state = CONTROLLER.lock();
    let Some(d) = state.disks.get(disk).copied() else {
        return false;
    };
    drop(state);
    if !d.ready || lba.saturating_add(sectors as u64) > d.sectors {
        return false;
    }
    if !d.lba48 && lba + sectors as u64 > 1 << 28 {
        return false;
    }
    dma_command(&d, lba, sectors, buffer_phys, write)
}

fn disk_read_sector_state(disk: &Disk, lba: u64, destination: &mut [u8; 512]) -> bool {
    if !disk.ready || lba >= disk.sectors {
        return false;
    }
    unsafe {
        core::ptr::write_bytes(disk.data as usize as *mut u8, 0, 512);
    }
    if !dma_command(disk, lba, 1, disk.data, false) {
        return false;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            disk.data as usize as *const u8,
            destination.as_mut_ptr(),
            512,
        );
    }
    true
}

fn disk_write_sector_state(disk: &Disk, lba: u64, source: &[u8; 512]) -> bool {
    if !disk.ready || lba >= disk.sectors {
        return false;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(source.as_ptr(), disk.data as usize as *mut u8, 512);
    }
    dma_command(disk, lba, 1, disk.data, true)
}

fn dma_command(disk: &Disk, lba: u64, sectors: u32, buffer_phys: u64, write: bool) -> bool {
    let _io = IO_LOCK.lock();
    let port = disk.port;
    let command_list = disk.command_list;
    let command_table = disk.command_table;
    let extended = disk.lba48;
    if !wait_clear(port, PORT_TFD, 0x88) {
        return false;
    }
    let occupied = unsafe { read_register(port, PORT_SACT) | read_register(port, PORT_CI) };
    let Some(slot) = (0..32usize).find(|slot| occupied & (1 << slot) == 0) else {
        return false;
    };
    let entry = command_list + slot as u64 * 32;
    let byte_count = sectors * 512;
    let command = match (write, extended) {
        (false, true) => CMD_READ_DMA_EXT,
        (false, false) => CMD_READ_DMA,
        (true, true) => CMD_WRITE_DMA_EXT,
        (true, false) => CMD_WRITE_DMA,
    };
    unsafe {
        core::ptr::write_bytes(entry as usize as *mut u8, 0, 32);
        let flags: u16 = 5 | if write { 1 << 6 } else { 0 };
        core::ptr::write_volatile(entry as usize as *mut u16, flags);
        core::ptr::write_volatile((entry + 2) as usize as *mut u16, 1);
        core::ptr::write_volatile((entry + 8) as usize as *mut u32, command_table as u32);
        core::ptr::write_volatile(
            (entry + 12) as usize as *mut u32,
            (command_table >> 32) as u32,
        );
        core::ptr::write_bytes(command_table as usize as *mut u8, 0, PAGE_SIZE as usize);
        let fis = command_table as usize as *mut u8;
        core::ptr::write_volatile(fis, 0x27);
        core::ptr::write_volatile(fis.add(1), 0x80);
        core::ptr::write_volatile(fis.add(2), command);
        core::ptr::write_volatile(fis.add(4), lba as u8);
        core::ptr::write_volatile(fis.add(5), (lba >> 8) as u8);
        core::ptr::write_volatile(fis.add(6), (lba >> 16) as u8);
        core::ptr::write_volatile(
            fis.add(7),
            0x40 | if extended {
                0
            } else {
                (lba >> 24) as u8 & 0x0f
            },
        );
        if extended {
            core::ptr::write_volatile(fis.add(8), (lba >> 24) as u8);
            core::ptr::write_volatile(fis.add(9), (lba >> 32) as u8);
            core::ptr::write_volatile(fis.add(10), (lba >> 40) as u8);
        }
        core::ptr::write_volatile(fis.add(12), sectors as u8);
        core::ptr::write_volatile(fis.add(13), (sectors >> 8) as u8);
        let descriptor = command_table + 0x80;
        core::ptr::write_volatile(descriptor as usize as *mut u32, buffer_phys as u32);
        core::ptr::write_volatile(
            (descriptor + 4) as usize as *mut u32,
            (buffer_phys >> 32) as u32,
        );
        core::ptr::write_volatile(
            (descriptor + 12) as usize as *mut u32,
            (1 << 31) | (byte_count - 1),
        );
        write_register(port, PORT_IS, u32::MAX);
        write_register(port, PORT_CI, 1 << slot);
    }
    for _ in 0..COMMAND_TIMEOUT {
        if unsafe { read_register(port, PORT_IS) } & (1 << 30) != 0 {
            return false;
        }
        if unsafe { read_register(port, PORT_CI) } & (1 << slot) == 0 {
            return true;
        }
        spin_loop();
    }
    false
}

fn configure_port(port: u64, command_list: u64, received_fis: u64) -> bool {
    unsafe {
        let mut command = read_register(port, PORT_CMD);
        command &= !1;
        write_register(port, PORT_CMD, command);
        if !wait_clear(port, PORT_CMD, 1 << 15) {
            return false;
        }
        command &= !(1 << 4);
        write_register(port, PORT_CMD, command);
        if !wait_clear(port, PORT_CMD, 1 << 14) {
            return false;
        }
        write_register(port, PORT_CLB, command_list as u32);
        write_register(port, PORT_CLBU, (command_list >> 32) as u32);
        write_register(port, PORT_FB, received_fis as u32);
        write_register(port, PORT_FBU, (received_fis >> 32) as u32);
        write_register(port, PORT_SERR, u32::MAX);
        write_register(port, PORT_IS, u32::MAX);
        command = read_register(port, PORT_CMD) | (1 << 4);
        write_register(port, PORT_CMD, command);
        command |= 1;
        write_register(port, PORT_CMD, command);
    }
    true
}

fn issue_command(
    port: u64,
    command_list: u64,
    command_table: u64,
    data: u64,
    command: u8,
    extended: bool,
    lba: u64,
) -> bool {
    let _io = IO_LOCK.lock();
    if !wait_clear(port, PORT_TFD, 0x88) {
        return false;
    }
    let occupied = unsafe { read_register(port, PORT_SACT) | read_register(port, PORT_CI) };
    let Some(slot) = (0..32usize).find(|slot| occupied & (1 << slot) == 0) else {
        return false;
    };
    let header = command_list + slot as u64 * 32;
    unsafe {
        core::ptr::write_bytes(header as usize as *mut u8, 0, 32);
        core::ptr::write_volatile(header as usize as *mut u16, 5);
        core::ptr::write_volatile((header + 2) as usize as *mut u16, 1);
        core::ptr::write_volatile((header + 8) as usize as *mut u32, command_table as u32);
        core::ptr::write_volatile(
            (header + 12) as usize as *mut u32,
            (command_table >> 32) as u32,
        );
        core::ptr::write_bytes(command_table as usize as *mut u8, 0, PAGE_SIZE as usize);
        let fis = command_table as usize as *mut u8;
        core::ptr::write_volatile(fis, 0x27);
        core::ptr::write_volatile(fis.add(1), 0x80);
        core::ptr::write_volatile(fis.add(2), command);
        if command != CMD_IDENTIFY {
            core::ptr::write_volatile(fis.add(4), lba as u8);
            core::ptr::write_volatile(fis.add(5), (lba >> 8) as u8);
            core::ptr::write_volatile(fis.add(6), (lba >> 16) as u8);
            core::ptr::write_volatile(
                fis.add(7),
                0x40 | if extended {
                    0
                } else {
                    (lba >> 24) as u8 & 0x0f
                },
            );
            core::ptr::write_volatile(fis.add(12), 1);
            if extended {
                core::ptr::write_volatile(fis.add(8), (lba >> 24) as u8);
                core::ptr::write_volatile(fis.add(9), (lba >> 32) as u8);
                core::ptr::write_volatile(fis.add(10), (lba >> 40) as u8);
                core::ptr::write_volatile(fis.add(13), 0);
            }
        }
        let descriptor = command_table + 0x80;
        core::ptr::write_volatile(descriptor as usize as *mut u32, data as u32);
        core::ptr::write_volatile((descriptor + 4) as usize as *mut u32, (data >> 32) as u32);
        core::ptr::write_volatile((descriptor + 12) as usize as *mut u32, (1 << 31) | 511);
        write_register(port, PORT_IS, u32::MAX);
        write_register(port, PORT_CI, 1 << slot);
    }
    for _ in 0..COMMAND_TIMEOUT {
        let active = unsafe { read_register(port, PORT_CI) } & (1 << slot) != 0;
        let failed = unsafe { read_register(port, PORT_IS) } & (1 << 30) != 0;
        if failed {
            return false;
        }
        if !active {
            return true;
        }
        spin_loop();
    }
    false
}

fn wait_clear(base: u64, offset: usize, mask: u32) -> bool {
    for _ in 0..COMMAND_TIMEOUT {
        if unsafe { read_register(base, offset) } & mask == 0 {
            return true;
        }
        spin_loop();
    }
    false
}

fn port_base(abar: u64, port: usize) -> u64 {
    abar + PORT_BASE as u64 + port as u64 * PORT_STRIDE as u64
}

unsafe fn read_register(base: u64, offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base as usize + offset) as *const u32) }
}

unsafe fn write_register(base: u64, offset: usize, value: u32) {
    unsafe {
        core::ptr::write_volatile((base as usize + offset) as *mut u32, value);
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut value = u32::MAX;
    for byte in bytes {
        value ^= *byte as u32;
        for _ in 0..8 {
            value = value >> 1 ^ (0xedb8_8320 & 0u32.wrapping_sub(value & 1));
        }
    }
    !value
}
