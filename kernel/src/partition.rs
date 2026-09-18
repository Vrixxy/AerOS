use crate::ahci;

const MAX_PARTITIONS: usize = 16;
const MAX_GPT_ENTRIES: u32 = 128;
const MAX_GPT_ENTRY_SIZE: u32 = 512;

#[derive(Clone, Copy)]
pub struct Partition {
    pub first_lba: u64,
    pub sectors: u64,
    pub kind: u8,
    pub bootable: bool,
}

impl Partition {
    const EMPTY: Self = Self {
        first_lba: 0,
        sectors: 0,
        kind: 0,
        bootable: false,
    };

    pub fn end_lba(&self) -> u64 {
        self.first_lba + self.sectors
    }

    pub fn fat_candidate(&self) -> bool {
        matches!(self.kind, 0x01 | 0x04 | 0x06 | 0x0b | 0x0c | 0x0e | 0xef)
    }
}

#[derive(Clone, Copy)]
pub struct PartitionReport {
    pub mbr: bool,
    pub gpt: bool,
    pub protective: bool,
    pub partitions: usize,
    pub fat_candidates: usize,
    pub bootable: usize,
    pub first_lba: u64,
    pub covered_sectors: u64,
    pub verified: bool,
    entries: [Partition; MAX_PARTITIONS],
}

impl PartitionReport {
    const EMPTY: Self = Self {
        mbr: false,
        gpt: false,
        protective: false,
        partitions: 0,
        fat_candidates: 0,
        bootable: 0,
        first_lba: 0,
        covered_sectors: 0,
        verified: false,
        entries: [Partition::EMPTY; MAX_PARTITIONS],
    };

    pub fn entries(&self) -> &[Partition] {
        &self.entries[..self.partitions]
    }
}

pub fn inspect(disk_sectors: u64) -> PartitionReport {
    let mut sector = [0u8; 512];
    if disk_sectors < 2 || !ahci::read_sector(0, &mut sector) {
        return PartitionReport::EMPTY;
    }
    let mbr_valid = sector[510] == 0x55 && sector[511] == 0xaa;
    if !mbr_valid {
        return PartitionReport::EMPTY;
    }
    let protective = (0..4).any(|index| sector[446 + index * 16 + 4] == 0xee);
    if protective {
        let mut report = inspect_gpt(disk_sectors);
        report.mbr = true;
        report.protective = true;
        return report;
    }
    let mut report = PartitionReport {
        mbr: true,
        ..PartitionReport::EMPTY
    };
    for index in 0..4 {
        let offset = 446 + index * 16;
        let status = sector[offset];
        let kind = sector[offset + 4];
        if kind == 0 {
            continue;
        }
        if status != 0 && status != 0x80 {
            return report;
        }
        let first_lba = read_u32(&sector, offset + 8) as u64;
        let sectors = read_u32(&sector, offset + 12) as u64;
        if first_lba == 0
            || sectors == 0
            || first_lba
                .checked_add(sectors)
                .is_none_or(|end| end > disk_sectors)
            || !push(
                &mut report,
                Partition {
                    first_lba,
                    sectors,
                    kind,
                    bootable: status == 0x80,
                },
            )
        {
            return report;
        }
    }
    report.verified = report.partitions != 0 && validate(&report, disk_sectors);
    report
}

fn inspect_gpt(disk_sectors: u64) -> PartitionReport {
    let mut report = PartitionReport {
        gpt: true,
        ..PartitionReport::EMPTY
    };
    let mut header = [0u8; 512];
    if !ahci::read_sector(1, &mut header) || header[..8] != *b"EFI PART" {
        return report;
    }
    let header_size = read_u32(&header, 12) as usize;
    let stored_header_crc = read_u32(&header, 16);
    if !(92..=512).contains(&header_size)
        || read_u64(&header, 24) != 1
        || read_u64(&header, 32) >= disk_sectors
        || crc32_zeroed(&header[..header_size], 16, 4) != stored_header_crc
    {
        return report;
    }
    let first_usable = read_u64(&header, 40);
    let last_usable = read_u64(&header, 48);
    let entry_lba = read_u64(&header, 72);
    let entry_count = read_u32(&header, 80);
    let entry_size = read_u32(&header, 84);
    let stored_entries_crc = read_u32(&header, 88);
    if first_usable > last_usable
        || last_usable >= disk_sectors
        || entry_count == 0
        || entry_count > MAX_GPT_ENTRIES
        || !(128..=MAX_GPT_ENTRY_SIZE).contains(&entry_size)
        || entry_size & 7 != 0
    {
        return report;
    }
    let Some(total_bytes) = entry_count.checked_mul(entry_size) else {
        return report;
    };
    let sectors = total_bytes.div_ceil(512);
    if entry_lba
        .checked_add(sectors as u64)
        .is_none_or(|end| end > disk_sectors)
    {
        return report;
    }
    let mut crc = u32::MAX;
    let mut remaining = total_bytes as usize;
    let mut entry_sector = [0u8; 512];
    for sector_index in 0..sectors {
        if !ahci::read_sector(entry_lba + sector_index as u64, &mut entry_sector) {
            return report;
        }
        let count = remaining.min(512);
        crc = crc32_update(crc, &entry_sector[..count]);
        remaining -= count;
    }
    if !crc != stored_entries_crc {
        return report;
    }
    for index in 0..entry_count {
        let byte_offset = index as u64 * entry_size as u64;
        let lba = entry_lba + byte_offset / 512;
        let within = (byte_offset % 512) as usize;
        if within + entry_size as usize > 512 || !ahci::read_sector(lba, &mut entry_sector) {
            return report;
        }
        let entry = &entry_sector[within..within + entry_size as usize];
        if entry[..16].iter().all(|byte| *byte == 0) {
            continue;
        }
        let first_lba = read_u64(entry, 32);
        let last_lba = read_u64(entry, 40);
        if first_lba < first_usable || last_lba > last_usable || first_lba > last_lba {
            return report;
        }
        let kind = if is_efi_system_guid(&entry[..16]) {
            0xef
        } else {
            0xff
        };
        if !push(
            &mut report,
            Partition {
                first_lba,
                sectors: last_lba - first_lba + 1,
                kind,
                bootable: false,
            },
        ) {
            return report;
        }
    }
    report.verified = report.partitions != 0 && validate(&report, disk_sectors);
    report
}

fn push(report: &mut PartitionReport, partition: Partition) -> bool {
    if report.partitions == MAX_PARTITIONS {
        return false;
    }
    if report.partitions == 0 {
        report.first_lba = partition.first_lba;
    } else {
        report.first_lba = report.first_lba.min(partition.first_lba);
    }
    report.fat_candidates += usize::from(partition.fat_candidate());
    report.bootable += usize::from(partition.bootable);
    report.covered_sectors = report.covered_sectors.saturating_add(partition.sectors);
    report.entries[report.partitions] = partition;
    report.partitions += 1;
    true
}

fn validate(report: &PartitionReport, disk_sectors: u64) -> bool {
    for (index, partition) in report.entries().iter().enumerate() {
        if partition.sectors == 0 || partition.end_lba() > disk_sectors {
            return false;
        }
        for other in &report.entries()[index + 1..] {
            if partition.first_lba < other.end_lba() && partition.end_lba() > other.first_lba {
                return false;
            }
        }
    }
    true
}

fn is_efi_system_guid(bytes: &[u8]) -> bool {
    bytes
        == [
            0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11, 0xba, 0x4b, 0x00, 0xa0, 0xc9, 0x3e,
            0xc9, 0x3b,
        ]
}

fn crc32_zeroed(bytes: &[u8], zero_offset: usize, zero_length: usize) -> u32 {
    let mut value = u32::MAX;
    for (index, byte) in bytes.iter().enumerate() {
        value = crc32_byte(
            value,
            if (zero_offset..zero_offset + zero_length).contains(&index) {
                0
            } else {
                *byte
            },
        );
    }
    !value
}

fn crc32_update(mut value: u32, bytes: &[u8]) -> u32 {
    for byte in bytes {
        value = crc32_byte(value, *byte);
    }
    value
}

fn crc32_byte(mut value: u32, byte: u8) -> u32 {
    value ^= byte as u32;
    for _ in 0..8 {
        value = value >> 1 ^ (0xedb8_8320 & 0u32.wrapping_sub(value & 1));
    }
    value
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}
