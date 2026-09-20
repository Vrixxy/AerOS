use crate::ahci;
use crate::fat;
use crate::memory::FrameAllocator;

const SECTOR: u64 = 512;
const ESP_TYPE_GUID: [u8; 16] = [
    0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11, 0xba, 0x4b, 0x00, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b,
];
const MIN_TARGET_SECTORS: u64 = 131_072;
const MAX_IMAGE_BYTES: u64 = 48 * 1024 * 1024;
const RESERVED: u64 = 32;
const NUM_FATS: u64 = 2;
const SPC: u64 = 8;
const PART_ALIGN: u64 = 2048;

const MARKER: &[u8] = b"AerOS installed by the native installer.\r\nboot: EFI/BOOT/BOOTX64.EFI\r\n";

#[derive(Clone, Copy)]
pub struct InstallReport {
    pub attempted: bool,
    pub target_disk: usize,
    pub target_sectors: u64,
    pub target_blank: bool,
    pub source_bytes: u64,
    pub gpt: bool,
    pub formatted: bool,
    pub kernel_written: bool,
    pub marker_written: bool,
    pub readback_ok: bool,
    pub verified: bool,
}

impl InstallReport {
    const EMPTY: Self = Self {
        attempted: false,
        target_disk: 0,
        target_sectors: 0,
        target_blank: false,
        source_bytes: 0,
        gpt: false,
        formatted: false,
        kernel_written: false,
        marker_written: false,
        readback_ok: false,
        verified: false,
    };
}

struct Layout {
    disk: usize,
    part_lba: u64,
    part_sectors: u64,
    fat_size: u64,
    data_lba: u64,
    clusters: u64,
    next_free: u32,
}

impl Layout {
    fn cluster_lba(&self, cluster: u32) -> u64 {
        self.data_lba + (cluster as u64 - 2) * SPC
    }

    fn alloc(&mut self, count: u32) -> Option<u32> {
        let first = self.next_free;
        if (first as u64 + count as u64) >= self.clusters + 2 {
            return None;
        }
        self.next_free += count;
        Some(first)
    }

    fn write_chain(&self, first: u32, count: u32) -> bool {
        let mut cluster = first;
        let last = first + count - 1;
        while cluster <= last {
            let byte = cluster as u64 * 4;
            let sector = byte / SECTOR;
            let sector_first = (sector * SECTOR / 4) as u32;
            let sector_last = sector_first + 127;
            let mut buffer = [0u8; 512];
            if !ahci::read_disk_sector(self.disk, self.part_lba + RESERVED + sector, &mut buffer) {
                return false;
            }
            let lo = cluster.max(sector_first);
            let hi = last.min(sector_last);
            for entry in lo..=hi {
                let offset = (entry as usize * 4) % 512;
                let value = if entry < last { entry + 1 } else { 0x0fff_ffff };
                buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            }
            for fat in 0..NUM_FATS {
                let lba = self.part_lba + RESERVED + fat * self.fat_size + sector;
                if !ahci::write_disk_sector(self.disk, lba, &buffer) {
                    return false;
                }
            }
            cluster = hi + 1;
        }
        true
    }

    fn write_cluster_data(&self, first: u32, source_phys: u64, bytes: u64) -> bool {
        let mut done = 0u64;
        let base = self.cluster_lba(first);
        let total = bytes.div_ceil(SECTOR);
        while done < total {
            let batch = (total - done).min(8192) as u32;
            if !ahci::write_disk(self.disk, base + done, batch, source_phys + done * SECTOR) {
                return false;
            }
            done += batch as u64;
        }
        true
    }

    fn write_directory(&self, cluster: u32, entries: &[[u8; 32]]) -> bool {
        let mut block = [0u8; (SPC * SECTOR) as usize];
        for (index, entry) in entries.iter().enumerate() {
            let offset = index * 32;
            if offset + 32 > block.len() {
                return false;
            }
            block[offset..offset + 32].copy_from_slice(entry);
        }
        for sector in 0..SPC {
            let mut buffer = [0u8; 512];
            let start = (sector * SECTOR) as usize;
            buffer.copy_from_slice(&block[start..start + 512]);
            if !ahci::write_disk_sector(self.disk, self.cluster_lba(cluster) + sector, &buffer) {
                return false;
            }
        }
        true
    }
}

pub fn install(frames: &mut FrameAllocator) -> InstallReport {
    let mut report = InstallReport::EMPTY;
    let disk_count = ahci::disk_count();
    if disk_count < 2 {
        return report;
    }
    let target = disk_count - 1;
    if target == ahci::boot_disk() {
        return report;
    }
    let target_sectors = ahci::disk_sectors(target);
    report.attempted = true;
    report.target_disk = target;
    report.target_sectors = target_sectors;
    if target_sectors < MIN_TARGET_SECTORS {
        return report;
    }

    let mut head = [0u8; 512];
    let mut tail = [0u8; 512];
    // Sector 2 matters: an ext4 filesystem (e.g. the Linux guest's root disk)
    // starts with 1024 zero bytes, so sectors 0-1 alone would make it look
    // blank; its superblock sits in sector 2.
    let mut superblock = [0u8; 512];
    let blank = ahci::read_disk_sector(target, 0, &mut head)
        && ahci::read_disk_sector(target, 1, &mut tail)
        && ahci::read_disk_sector(target, 2, &mut superblock)
        && head.iter().all(|byte| *byte == 0)
        && tail.iter().all(|byte| *byte == 0)
        && superblock.iter().all(|byte| *byte == 0);
    report.target_blank = blank;
    if !blank {
        return report;
    }

    let Some((source_cluster, source_bytes)) = fat::boot_kernel() else {
        return report;
    };
    report.source_bytes = source_bytes;
    if source_bytes == 0 || source_bytes > MAX_IMAGE_BYTES {
        return report;
    }

    let image_pages = (source_bytes.div_ceil(4096) + 2).max(4);
    let Some(scratch) = frames
        .allocate_contiguous(image_pages, 1)
        .map(|frame| frame.address())
    else {
        return report;
    };
    zero_phys(scratch, image_pages * 4096);
    if fat::stream_clusters(source_cluster, source_bytes, scratch, source_bytes)
        != Some(source_bytes)
    {
        return report;
    }

    let part_lba = PART_ALIGN;
    let part_end = target_sectors - 34;
    if part_end <= part_lba + 2048 {
        return report;
    }
    let part_sectors = part_end - part_lba;

    if !write_gpt(target, target_sectors, part_lba, part_end) {
        return report;
    }
    report.gpt = true;

    let fat_size = (part_sectors - RESERVED).div_ceil(128 * SPC + NUM_FATS);
    let data_lba = part_lba + RESERVED + NUM_FATS * fat_size;
    if data_lba + SPC * 4 >= part_lba + part_sectors {
        return report;
    }
    let clusters = (part_lba + part_sectors - data_lba) / SPC;
    if clusters < 65_525 {
        return report;
    }

    let mut layout = Layout {
        disk: target,
        part_lba,
        part_sectors,
        fat_size,
        data_lba,
        clusters,
        next_free: 3,
    };

    if !format_fat32(&layout) {
        return report;
    }
    report.formatted = true;

    let kernel_clusters = source_bytes.div_ceil(SPC * SECTOR).max(1) as u32;
    let marker_clusters = (MARKER.len() as u64).div_ceil(SPC * SECTOR).max(1) as u32;
    let (Some(efi_dir), Some(boot_dir), Some(kernel_first), Some(aeros_dir), Some(marker_first)) = (
        layout.alloc(1),
        layout.alloc(1),
        layout.alloc(kernel_clusters),
        layout.alloc(1),
        layout.alloc(marker_clusters),
    ) else {
        return report;
    };

    if !layout.write_chain(efi_dir, 1)
        || !layout.write_chain(boot_dir, 1)
        || !layout.write_chain(aeros_dir, 1)
    {
        return report;
    }

    if !layout.write_cluster_data(kernel_first, scratch, source_bytes)
        || !layout.write_chain(kernel_first, kernel_clusters)
    {
        return report;
    }
    report.kernel_written = true;

    let mut kernel_head = [0u8; 512];
    unsafe {
        core::ptr::copy_nonoverlapping(
            scratch as usize as *const u8,
            kernel_head.as_mut_ptr(),
            512,
        );
    }

    zero_phys(scratch, (marker_clusters as u64) * SPC * SECTOR);
    write_phys(scratch, MARKER);
    if !layout.write_cluster_data(marker_first, scratch, MARKER.len() as u64)
        || !layout.write_chain(marker_first, marker_clusters)
    {
        return report;
    }
    report.marker_written = true;

    let root_entries = [
        dir_entry(b"EFI        ", 0x10, efi_dir, 0),
        dir_entry(b"AEROS      ", 0x10, aeros_dir, 0),
    ];
    let efi_entries = [
        dir_entry(b".          ", 0x10, efi_dir, 0),
        dir_entry(b"..         ", 0x10, 0, 0),
        dir_entry(b"BOOT       ", 0x10, boot_dir, 0),
    ];
    let boot_entries = [
        dir_entry(b".          ", 0x10, boot_dir, 0),
        dir_entry(b"..         ", 0x10, efi_dir, 0),
        dir_entry(b"BOOTX64 EFI", 0x20, kernel_first, source_bytes as u32),
    ];
    let aeros_entries = [
        dir_entry(b".          ", 0x10, aeros_dir, 0),
        dir_entry(b"..         ", 0x10, 0, 0),
        dir_entry(b"INSTALL TXT", 0x20, marker_first, MARKER.len() as u32),
    ];
    if !layout.write_directory(2, &root_entries)
        || !layout.write_directory(efi_dir, &efi_entries)
        || !layout.write_directory(boot_dir, &boot_entries)
        || !layout.write_directory(aeros_dir, &aeros_entries)
    {
        return report;
    }

    ahci::flush_disk(target);
    report.readback_ok = verify(&layout, kernel_first, &kernel_head);
    report.verified = report.gpt
        && report.formatted
        && report.kernel_written
        && report.marker_written
        && report.readback_ok;
    report
}

fn verify(layout: &Layout, kernel_first: u32, kernel_head: &[u8; 512]) -> bool {
    let mut boot = [0u8; 512];
    let bpb_ok = ahci::read_disk_sector(layout.disk, layout.part_lba, &mut boot)
        && boot[510] == 0x55
        && boot[511] == 0xaa
        && boot[82..90] == *b"FAT32   ";

    let mut root = [0u8; 512];
    let root_ok = ahci::read_disk_sector(layout.disk, layout.cluster_lba(2), &mut root)
        && root[0..11] == *b"EFI        ";

    let mut header = [0u8; 512];
    let kernel_ok =
        ahci::read_disk_sector(layout.disk, layout.cluster_lba(kernel_first), &mut header)
            && header[0] == b'M'
            && header[1] == b'Z'
            && header == *kernel_head;

    bpb_ok && root_ok && kernel_ok
}

fn format_fat32(layout: &Layout) -> bool {
    let mut boot = [0u8; 512];
    boot[0] = 0xeb;
    boot[1] = 0x58;
    boot[2] = 0x90;
    boot[3..11].copy_from_slice(b"MSWIN4.1");
    put16(&mut boot, 11, 512);
    boot[13] = SPC as u8;
    put16(&mut boot, 14, RESERVED as u16);
    boot[16] = NUM_FATS as u8;
    put16(&mut boot, 17, 0);
    put16(&mut boot, 19, 0);
    boot[21] = 0xf8;
    put16(&mut boot, 22, 0);
    put16(&mut boot, 24, 32);
    put16(&mut boot, 26, 8);
    put32(&mut boot, 28, layout.part_lba as u32);
    put32(&mut boot, 32, layout.part_sectors as u32);
    put32(&mut boot, 36, layout.fat_size as u32);
    put16(&mut boot, 40, 0);
    put16(&mut boot, 42, 0);
    put32(&mut boot, 44, 2);
    put16(&mut boot, 48, 1);
    put16(&mut boot, 50, 6);
    boot[64] = 0x80;
    boot[66] = 0x29;
    put32(&mut boot, 67, 0xae05_0501);
    boot[71..82].copy_from_slice(b"AEROS      ");
    boot[82..90].copy_from_slice(b"FAT32   ");
    boot[510] = 0x55;
    boot[511] = 0xaa;

    let mut fsinfo = [0u8; 512];
    put32(&mut fsinfo, 0, 0x4161_5252);
    put32(&mut fsinfo, 484, 0x6141_7272);
    put32(&mut fsinfo, 488, 0xffff_ffff);
    put32(&mut fsinfo, 492, layout.next_free.max(2));
    fsinfo[510] = 0x55;
    fsinfo[511] = 0xaa;

    if !ahci::write_disk_sector(layout.disk, layout.part_lba, &boot)
        || !ahci::write_disk_sector(layout.disk, layout.part_lba + 1, &fsinfo)
        || !ahci::write_disk_sector(layout.disk, layout.part_lba + 6, &boot)
        || !ahci::write_disk_sector(layout.disk, layout.part_lba + 7, &fsinfo)
    {
        return false;
    }

    let zero = [0u8; 512];
    for fat in 0..NUM_FATS {
        let base = layout.part_lba + RESERVED + fat * layout.fat_size;
        for sector in 0..layout.fat_size {
            if !ahci::write_disk_sector(layout.disk, base + sector, &zero) {
                return false;
            }
        }
        let mut head = [0u8; 512];
        put32(&mut head, 0, 0x0fff_fff8);
        put32(&mut head, 4, 0x0fff_ffff);
        put32(&mut head, 8, 0x0fff_ffff);
        if !ahci::write_disk_sector(layout.disk, base, &head) {
            return false;
        }
    }

    let empty = [0u8; 512];
    for sector in 0..SPC {
        if !ahci::write_disk_sector(layout.disk, layout.cluster_lba(2) + sector, &empty) {
            return false;
        }
    }
    true
}

fn write_gpt(disk: usize, disk_sectors: u64, part_first: u64, part_end: u64) -> bool {
    let mut mbr = [0u8; 512];
    mbr[446] = 0x00;
    mbr[447] = 0x00;
    mbr[448] = 0x02;
    mbr[449] = 0x00;
    mbr[450] = 0xee;
    mbr[451] = 0xff;
    mbr[452] = 0xff;
    mbr[453] = 0xff;
    put32(&mut mbr, 454, 1);
    put32(&mut mbr, 458, (disk_sectors - 1).min(0xffff_ffff) as u32);
    mbr[510] = 0x55;
    mbr[511] = 0xaa;
    if !ahci::write_disk_sector(disk, 0, &mbr) {
        return false;
    }

    let mut disk_guid = [0u8; 16];
    let mut part_guid = [0u8; 16];
    crate::random::fill(&mut disk_guid);
    crate::random::fill(&mut part_guid);

    let mut entry = [0u8; 128];
    entry[0..16].copy_from_slice(&ESP_TYPE_GUID);
    entry[16..32].copy_from_slice(&part_guid);
    put64(&mut entry, 32, part_first);
    put64(&mut entry, 40, part_end - 1);
    for (index, unit) in "EFI System".encode_utf16().enumerate() {
        let offset = 56 + index * 2;
        if offset + 2 > 128 {
            break;
        }
        entry[offset..offset + 2].copy_from_slice(&unit.to_le_bytes());
    }

    let mut array_crc = 0xffff_ffffu32;
    array_crc = crc32_update(array_crc, &entry);
    let zeros = [0u8; 256];
    let mut remaining = 128usize * 128 - 128;
    while remaining > 0 {
        let take = remaining.min(zeros.len());
        array_crc = crc32_update(array_crc, &zeros[..take]);
        remaining -= take;
    }
    array_crc = !array_crc;

    let backup_header_lba = disk_sectors - 1;
    let backup_array_lba = disk_sectors - 33;

    let mut entry_sector = [0u8; 512];
    entry_sector[0..128].copy_from_slice(&entry);
    let zero_sector = [0u8; 512];

    for (header_lba, current, backup, array_lba) in [
        (1u64, 1u64, backup_header_lba, 2u64),
        (backup_header_lba, backup_header_lba, 1u64, backup_array_lba),
    ] {
        for sector in 0..32u64 {
            let data = if sector == 0 {
                &entry_sector
            } else {
                &zero_sector
            };
            if !ahci::write_disk_sector(disk, array_lba + sector, data) {
                return false;
            }
        }
        let mut header = [0u8; 512];
        header[0..8].copy_from_slice(b"EFI PART");
        put32(&mut header, 8, 0x0001_0000);
        put32(&mut header, 12, 92);
        put64(&mut header, 24, current);
        put64(&mut header, 32, backup);
        put64(&mut header, 40, 34);
        put64(&mut header, 48, disk_sectors - 34);
        header[56..72].copy_from_slice(&disk_guid);
        put64(&mut header, 72, array_lba);
        put32(&mut header, 80, 128);
        put32(&mut header, 84, 128);
        put32(&mut header, 88, array_crc);
        let header_crc = crc32(&header[0..92]);
        put32(&mut header, 16, header_crc);
        if !ahci::write_disk_sector(disk, header_lba, &header) {
            return false;
        }
    }
    true
}

fn dir_entry(name: &[u8; 11], attr: u8, cluster: u32, size: u32) -> [u8; 32] {
    let mut entry = [0u8; 32];
    entry[0..11].copy_from_slice(name);
    entry[11] = attr;
    put16(&mut entry, 14, 0);
    put16(&mut entry, 16, 0x5928);
    put16(&mut entry, 18, 0x5928);
    put16(&mut entry, 20, (cluster >> 16) as u16);
    put16(&mut entry, 22, 0);
    put16(&mut entry, 24, 0x5928);
    put16(&mut entry, 26, cluster as u16);
    put32(&mut entry, 28, size);
    entry
}

fn zero_phys(base: u64, bytes: u64) {
    let mut offset = 0u64;
    while offset < bytes {
        unsafe { core::ptr::write_volatile((base + offset) as usize as *mut u64, 0) };
        offset += 8;
    }
}

fn write_phys(base: u64, data: &[u8]) {
    for (index, byte) in data.iter().enumerate() {
        unsafe { core::ptr::write_volatile((base + index as u64) as usize as *mut u8, *byte) };
    }
}

fn put16(buffer: &mut [u8], offset: usize, value: u16) {
    buffer[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(buffer: &mut [u8], offset: usize, value: u32) {
    buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(buffer: &mut [u8], offset: usize, value: u64) {
    buffer[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn crc32(bytes: &[u8]) -> u32 {
    !crc32_update(0xffff_ffff, bytes)
}

fn crc32_update(mut value: u32, bytes: &[u8]) -> u32 {
    for byte in bytes {
        value ^= *byte as u32;
        for _ in 0..8 {
            value = value >> 1 ^ (0xedb8_8320 & 0u32.wrapping_sub(value & 1));
        }
    }
    value
}
