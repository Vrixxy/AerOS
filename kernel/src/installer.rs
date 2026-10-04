use crate::ahci;
use crate::fat;
use crate::memory::{FrameAllocator, PageBuffer};

const SECTOR: u64 = 512;
const ESP_TYPE_GUID: [u8; 16] = [
    0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11, 0xba, 0x4b, 0x00, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b,
];
const MIN_TARGET_SECTORS: u64 = 131_072;
/// The smallest partition that can hold a FAT32 volume with 65,525 clusters.
pub const MIN_PARTITION_SECTORS: u64 = 590_000;
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
    /// The disk already had a valid GPT and the system went into its free space.
    pub existing_table: bool,
    /// Partitions that were there before and still are, byte for byte.
    pub preserved: u32,
    /// Why nothing was written, when the installer declined the disk.
    pub refusal: Option<Refusal>,
    /// The step the installation had reached when it stopped.
    pub stage: &'static str,
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
        existing_table: false,
        preserved: 0,
        refusal: None,
        stage: "start",
        verified: false,
    };
}

struct Layout {
    target: Target,
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
            if !self
                .target
                .read(self.part_lba + RESERVED + sector, &mut buffer)
            {
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
                if !self.target.write(lba, &buffer) {
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
            if !self
                .target
                .write_phys(base + done, batch, source_phys + done * SECTOR)
            {
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
            if !self
                .target
                .write(self.cluster_lba(cluster) + sector, &buffer)
            {
                return false;
            }
        }
        true
    }
}

/// A disk the installer can write to: a SATA disk, or (in the boot-test) the
/// block layer's RAM disk.
#[derive(Clone, Copy)]
pub enum Target {
    Ahci(usize),
    #[cfg(feature = "boot-test")]
    Ram,
}

impl Target {
    fn sectors(self) -> u64 {
        match self {
            Target::Ahci(disk) => ahci::disk_sectors(disk),
            #[cfg(feature = "boot-test")]
            Target::Ram => crate::blockdev::RAM_DISK_SECTORS,
        }
    }

    fn read(self, lba: u64, buffer: &mut [u8; 512]) -> bool {
        match self {
            Target::Ahci(disk) => ahci::read_disk_sector(disk, lba, buffer),
            #[cfg(feature = "boot-test")]
            Target::Ram => crate::blockdev::ram_read(lba, buffer),
        }
    }

    fn write(self, lba: u64, buffer: &[u8; 512]) -> bool {
        match self {
            Target::Ahci(disk) => ahci::write_disk_sector(disk, lba, buffer),
            #[cfg(feature = "boot-test")]
            Target::Ram => crate::blockdev::ram_write(lba, buffer),
        }
    }

    fn write_phys(self, lba: u64, sectors: u32, physical: u64) -> bool {
        match self {
            Target::Ahci(disk) => ahci::write_disk(disk, lba, sectors, physical),
            #[cfg(feature = "boot-test")]
            Target::Ram => {
                let mut sector = [0u8; 512];
                (0..sectors as u64).all(|index| {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            (physical + index * SECTOR) as usize as *const u8,
                            sector.as_mut_ptr(),
                            512,
                        );
                    }
                    crate::blockdev::ram_write(lba + index, &sector)
                })
            }
        }
    }

    fn flush(self) {
        match self {
            Target::Ahci(disk) => {
                ahci::flush_disk(disk);
                crate::block::invalidate_disk(crate::block::Disk::Ahci(disk));
            }
            #[cfg(feature = "boot-test")]
            Target::Ram => {}
        }
    }
}

// ------------------------------------------------------------- GPT in place

const MAX_ENTRIES: usize = 128;
const ENTRY_BYTES: usize = 128;
const ARRAY_BYTES: usize = MAX_ENTRIES * ENTRY_BYTES;
const ARRAY_SECTORS: u64 = (ARRAY_BYTES / 512) as u64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refusal {
    Unreadable,
    NoTable,
    Damaged,
    Unsupported,
    Full,
    NoFreeSpace,
    NotAllowed,
}

/// A GUID partition table read from a disk, valid in both copies.
struct Table {
    sectors: u64,
    header: [u8; 512],
    array: [u8; ARRAY_BYTES],
    entries: usize,
    first_usable: u64,
    last_usable: u64,
}

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn read_u64(bytes: &[u8], at: usize) -> u64 {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(raw)
}

fn header_valid(header: &[u8; 512], here: u64, other: u64, sectors: u64) -> bool {
    if header[..8] != *b"EFI PART" || read_u32(header, 8) != 0x0001_0000 {
        return false;
    }
    let size = read_u32(header, 12) as usize;
    if !(92..=512).contains(&size) {
        return false;
    }
    let mut copy = *header;
    copy[16..20].fill(0);
    read_u32(header, 16) == crc32(&copy[..size])
        && read_u64(header, 24) == here
        && read_u64(header, 32) == other
        && read_u64(header, 48) < sectors
        && read_u64(header, 40) <= read_u64(header, 48)
}

fn read_array(target: Target, header: &[u8; 512]) -> Result<[u8; ARRAY_BYTES], Refusal> {
    let entries = read_u32(header, 80) as usize;
    if entries == 0 || entries > MAX_ENTRIES || read_u32(header, 84) as usize != ENTRY_BYTES {
        return Err(Refusal::Unsupported);
    }
    let first = read_u64(header, 72);
    let mut array = [0u8; ARRAY_BYTES];
    let used = (entries * ENTRY_BYTES).div_ceil(512) as u64;
    for sector in 0..used {
        let mut buffer = [0u8; 512];
        if !target.read(first + sector, &mut buffer) {
            return Err(Refusal::Unreadable);
        }
        let at = sector as usize * 512;
        array[at..at + 512].copy_from_slice(&buffer);
    }
    if crc32(&array[..entries * ENTRY_BYTES]) != read_u32(header, 88) {
        return Err(Refusal::Damaged);
    }
    Ok(array)
}

fn read_table(target: Target) -> Result<Table, Refusal> {
    let sectors = target.sectors();
    if sectors < 100 {
        return Err(Refusal::Unreadable);
    }
    let mut mbr = [0u8; 512];
    let mut header = [0u8; 512];
    if !target.read(0, &mut mbr) || !target.read(1, &mut header) {
        return Err(Refusal::Unreadable);
    }
    if header[..8] != *b"EFI PART" {
        return Err(if mbr[510] == 0x55 && mbr[511] == 0xaa {
            Refusal::Unsupported
        } else {
            Refusal::NoTable
        });
    }
    let backup_lba = sectors - 1;
    if !header_valid(&header, 1, backup_lba, sectors) {
        return Err(Refusal::Damaged);
    }
    let mut backup = [0u8; 512];
    if !target.read(backup_lba, &mut backup) {
        return Err(Refusal::Unreadable);
    }
    if !header_valid(&backup, backup_lba, 1, sectors) {
        return Err(Refusal::Damaged);
    }
    let array = read_array(target, &header)?;
    let backup_array = read_array(target, &backup)?;
    let entries = read_u32(&header, 80) as usize;
    if read_u32(&backup, 80) as usize != entries
        || array[..entries * ENTRY_BYTES] != backup_array[..entries * ENTRY_BYTES]
        || read_u64(&header, 40) != read_u64(&backup, 40)
        || read_u64(&header, 48) != read_u64(&backup, 48)
    {
        return Err(Refusal::Damaged);
    }
    let table = Table {
        sectors,
        header,
        array,
        entries,
        first_usable: read_u64(&header, 40),
        last_usable: read_u64(&header, 48),
    };
    table.valid_layout()?;
    Ok(table)
}

impl Table {
    fn entry(&self, index: usize) -> &[u8] {
        &self.array[index * ENTRY_BYTES..(index + 1) * ENTRY_BYTES]
    }

    fn used(&self, index: usize) -> bool {
        self.entry(index)[..16].iter().any(|byte| *byte != 0)
    }

    fn extent(&self, index: usize) -> (u64, u64) {
        (
            read_u64(self.entry(index), 32),
            read_u64(self.entry(index), 40),
        )
    }

    /// Entries that lie inside the usable range, are not backwards and do not
    /// overlap one another.
    fn valid_layout(&self) -> Result<(), Refusal> {
        let mut extents = [(0u64, 0u64); MAX_ENTRIES];
        let mut count = 0;
        for index in (0..self.entries).filter(|&index| self.used(index)) {
            let (first, last) = self.extent(index);
            if first > last || first < self.first_usable || last > self.last_usable {
                return Err(Refusal::Damaged);
            }
            extents[count] = (first, last);
            count += 1;
        }
        extents[..count].sort_unstable();
        if extents[..count]
            .windows(2)
            .any(|pair| pair[1].0 <= pair[0].1)
        {
            return Err(Refusal::Damaged);
        }
        Ok(())
    }

    /// The free gaps between partitions as (first, last) sector pairs, the
    /// start rounded up to the partition alignment; returns how many.
    fn gaps(&self, out: &mut [(u64, u64); MAX_ENTRIES + 1]) -> usize {
        let mut used = [(0u64, 0u64); MAX_ENTRIES];
        let mut count = 0;
        for index in (0..self.entries).filter(|&index| self.used(index)) {
            used[count] = self.extent(index);
            count += 1;
        }
        used[..count].sort_unstable();
        let mut gaps = 0;
        let mut cursor = self.first_usable;
        for &(first, last) in &used[..count] {
            if first > cursor {
                out[gaps] = (cursor, first - 1);
                gaps += 1;
            }
            cursor = cursor.max(last + 1);
        }
        if cursor <= self.last_usable {
            out[gaps] = (cursor, self.last_usable);
            gaps += 1;
        }
        for gap in &mut out[..gaps] {
            gap.0 = gap.0.div_ceil(PART_ALIGN) * PART_ALIGN;
        }
        let mut kept = 0;
        for index in 0..gaps {
            if out[index].0 <= out[index].1 {
                out[kept] = out[index];
                kept += 1;
            }
        }
        kept
    }

    /// The largest free gap that holds at least `minimum` sectors.
    fn largest_gap(&self, minimum: u64) -> Result<(u64, u64), Refusal> {
        let mut gaps = [(0u64, 0u64); MAX_ENTRIES + 1];
        let count = self.gaps(&mut gaps);
        gaps[..count]
            .iter()
            .copied()
            .filter(|&(first, last)| last - first + 1 >= minimum)
            .max_by_key(|&(first, last)| last - first)
            .ok_or(Refusal::NoFreeSpace)
    }

    /// Adds a partition in the first empty slot and returns the slot.
    fn insert(
        &mut self,
        first: u64,
        last: u64,
        kind: &[u8; 16],
        name: &str,
    ) -> Result<usize, Refusal> {
        let slot = (0..self.entries)
            .find(|&index| !self.used(index))
            .ok_or(Refusal::Full)?;
        let mut guid = [0u8; 16];
        crate::random::fill(&mut guid);
        let entry = &mut self.array[slot * ENTRY_BYTES..(slot + 1) * ENTRY_BYTES];
        entry.fill(0);
        entry[..16].copy_from_slice(kind);
        entry[16..32].copy_from_slice(&guid);
        put64(entry, 32, first);
        put64(entry, 40, last);
        for (index, unit) in name.encode_utf16().take(36).enumerate() {
            entry[56 + index * 2..58 + index * 2].copy_from_slice(&unit.to_le_bytes());
        }
        Ok(slot)
    }

    /// Writes the backup copy first and the primary last, each array before
    /// its header, so a power cut leaves at least one complete valid table.
    fn write(&mut self, target: Target) -> bool {
        let bytes = self.entries * ENTRY_BYTES;
        let array_crc = crc32(&self.array[..bytes]);
        let backup_lba = self.sectors - 1;
        let mut backup = self.header;
        let primary_array = read_u64(&self.header, 72);
        let backup_array = self.sectors - 1 - ARRAY_SECTORS;
        put64(&mut backup, 24, backup_lba);
        put64(&mut backup, 32, 1);
        put64(&mut backup, 72, backup_array);
        for (lba, header, array_lba) in [
            (backup_lba, &mut backup, backup_array),
            (1, &mut self.header, primary_array),
        ] {
            put32(header, 88, array_crc);
            header[16..20].fill(0);
            let size = read_u32(header, 12) as usize;
            let crc = crc32(&header[..size]);
            put32(header, 16, crc);
            let used = bytes.div_ceil(512) as u64;
            for sector in 0..used {
                let mut buffer = [0u8; 512];
                let at = sector as usize * 512;
                buffer.copy_from_slice(&self.array[at..at + 512]);
                if !target.write(array_lba + sector, &buffer) {
                    return false;
                }
            }
            if !target.write(lba, header) {
                return false;
            }
        }
        true
    }
}

/// What the installer finds on a disk, for `install` without arguments.
#[derive(Clone, Copy)]
pub struct Inspection {
    pub sectors: u64,
    pub blank: bool,
    pub table: Result<usize, Refusal>,
    pub free_sectors: u64,
}

pub fn inspect(target: Target) -> Inspection {
    let mut inspection = Inspection {
        sectors: target.sectors(),
        blank: is_blank(target),
        table: Err(Refusal::NoTable),
        free_sectors: 0,
    };
    if inspection.blank {
        inspection.free_sectors = inspection.sectors.saturating_sub(PART_ALIGN + 34);
        return inspection;
    }
    match read_table(target) {
        Ok(table) => {
            inspection.table = Ok((0..table.entries)
                .filter(|&index| table.used(index))
                .count());
            inspection.free_sectors = table
                .largest_gap(1)
                .map_or(0, |(first, last)| last - first + 1);
        }
        Err(refusal) => inspection.table = Err(refusal),
    }
    inspection
}

fn is_blank(target: Target) -> bool {
    let mut head = [0u8; 512];
    let mut tail = [0u8; 512];
    // Sector 2 matters: an ext4 filesystem (e.g. the Linux guest's root disk)
    // starts with 1024 zero bytes, so sectors 0-1 alone would make it look
    // blank; its superblock sits in sector 2.
    let mut superblock = [0u8; 512];
    target.read(0, &mut head)
        && target.read(1, &mut tail)
        && target.read(2, &mut superblock)
        && head.iter().all(|byte| *byte == 0)
        && tail.iter().all(|byte| *byte == 0)
        && superblock.iter().all(|byte| *byte == 0)
}

/// The installer's own decision about a disk, applied to a table and an
/// installation request: whether the system goes onto the whole disk or into
/// free space, and where.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Plan {
    WholeDisk,
    FreeSpace { first: u64, last: u64 },
    Refuse(Refusal),
}

/// Whether and where an installation may go. A blank disk is always fine; a
/// disk with a valid table only when `allow_free_space` is given.
pub fn plan(target: Target, allow_free_space: bool, minimum: u64) -> Plan {
    if is_blank(target) {
        return Plan::WholeDisk;
    }
    if !allow_free_space {
        return Plan::Refuse(Refusal::NotAllowed);
    }
    match read_table(target).and_then(|table| table.largest_gap(minimum)) {
        Ok((first, last)) => Plan::FreeSpace { first, last },
        Err(refusal) => Plan::Refuse(refusal),
    }
}

/// Whether the boot disk carries an `INSTALL.CFG` asking for an unattended
/// installation into the free space of a disk that already has a table.
pub fn unattended_allowed() -> bool {
    let mut buffer = [0u8; 32];
    fat::read_root_file(b"INSTALL CFG", &mut buffer)
        .is_some_and(|length| buffer[..length].starts_with(b"free-space"))
}

pub fn install(allow_free_space: bool, frames: &mut FrameAllocator) -> InstallReport {
    let report = InstallReport::EMPTY;
    let disk_count = ahci::disk_count();
    if disk_count < 2 {
        return report;
    }
    let target_disk = disk_count - 1;
    if target_disk == ahci::boot_disk() {
        return report;
    }
    install_to(
        Target::Ahci(target_disk),
        allow_free_space,
        MIN_PARTITION_SECTORS,
        Some(frames),
    )
}

/// Installs the running system's boot image onto `target`: onto a blank disk
/// as its only partition, or, when allowed, into the largest free gap of a
/// disk that already has a valid GPT. The existing partitions are never
/// written: the new partition is formatted and filled first and only then
/// entered in the table, and afterwards every earlier entry is compared byte
/// for byte.
pub fn install_to(
    target: Target,
    allow_free_space: bool,
    minimum: u64,
    frames: Option<&mut FrameAllocator>,
) -> InstallReport {
    let mut report = InstallReport::EMPTY;
    let target_sectors = target.sectors();
    match target {
        Target::Ahci(disk) => report.target_disk = disk,
        #[cfg(feature = "boot-test")]
        Target::Ram => {}
    }
    report.attempted = true;
    report.target_sectors = target_sectors;
    if target_sectors < MIN_TARGET_SECTORS {
        return report;
    }
    report.target_blank = is_blank(target);
    let (part_lba, part_end, mut table) = match plan(target, allow_free_space, minimum) {
        Plan::WholeDisk => (PART_ALIGN, target_sectors - 34, None),
        Plan::FreeSpace { first, last } => {
            let Ok(table) = read_table(target) else {
                return report;
            };
            report.existing_table = true;
            (first, last + 1, Some(table))
        }
        Plan::Refuse(reason) => {
            report.refusal = Some(reason);
            return report;
        }
    };
    let before = table.as_ref().map(|table| (table.array, table.entries));

    report.stage = "read-image";
    let Some((source_cluster, source_bytes)) = fat::boot_kernel() else {
        return report;
    };
    report.source_bytes = source_bytes;
    if source_bytes == 0 || source_bytes > MAX_IMAGE_BYTES {
        return report;
    }

    let image_pages = (source_bytes.div_ceil(4096) + 2).max(4);
    let mut scratch_buffer = None;
    let scratch = match frames {
        Some(frames) => match frames.allocate_contiguous(image_pages, 1) {
            Some(frame) => {
                zero_phys(frame.address(), image_pages * 4096);
                frame.address()
            }
            None => return report,
        },
        None => match PageBuffer::new(image_pages as usize * 4096) {
            Some(buffer) => scratch_buffer.insert(buffer).as_mut_slice().as_mut_ptr() as u64,
            None => return report,
        },
    };
    if fat::stream_clusters(source_cluster, source_bytes, scratch, source_bytes)
        != Some(source_bytes)
    {
        return report;
    }

    report.stage = "layout";
    if part_end <= part_lba + 2048 {
        return report;
    }
    let part_sectors = part_end - part_lba;
    if table.is_none() {
        if !write_gpt(target, target_sectors, part_lba, part_end) {
            return report;
        }
        report.gpt = true;
    }

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
        target,
        part_lba,
        part_sectors,
        fat_size,
        data_lba,
        clusters,
        next_free: 3,
    };

    report.stage = "format";
    if !format_fat32(&layout) {
        return report;
    }
    report.formatted = true;
    report.stage = "files";

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

    report.stage = "table";
    if let Some(table) = table.as_mut() {
        if table
            .insert(part_lba, part_end - 1, &ESP_TYPE_GUID, "EFI System")
            .is_err()
            || !table.write(target)
        {
            return report;
        }
        report.gpt = true;
    }
    report.stage = "verify";
    target.flush();
    report.readback_ok = verify(&layout, kernel_first, &kernel_head);
    if let Some((array, entries)) = before {
        match read_table(target) {
            Ok(after) => {
                let kept = (0..entries)
                    .filter(|&index| {
                        array[index * ENTRY_BYTES..(index + 1) * ENTRY_BYTES]
                            .iter()
                            .any(|byte| *byte != 0)
                            && after.entry(index)
                                == &array[index * ENTRY_BYTES..(index + 1) * ENTRY_BYTES]
                    })
                    .count() as u32;
                let expected = (0..entries)
                    .filter(|&index| {
                        array[index * ENTRY_BYTES..(index + 1) * ENTRY_BYTES]
                            .iter()
                            .any(|byte| *byte != 0)
                    })
                    .count() as u32;
                report.preserved = kept;
                report.readback_ok &= kept == expected;
            }
            Err(_) => report.readback_ok = false,
        }
    }
    report.verified = report.gpt
        && report.formatted
        && report.kernel_written
        && report.marker_written
        && report.readback_ok;
    report
}

fn verify(layout: &Layout, kernel_first: u32, kernel_head: &[u8; 512]) -> bool {
    let mut boot = [0u8; 512];
    let bpb_ok = layout.target.read(layout.part_lba, &mut boot)
        && boot[510] == 0x55
        && boot[511] == 0xaa
        && boot[82..90] == *b"FAT32   ";

    let mut root = [0u8; 512];
    let root_ok =
        layout.target.read(layout.cluster_lba(2), &mut root) && root[0..11] == *b"EFI        ";

    let mut header = [0u8; 512];
    let kernel_ok = layout
        .target
        .read(layout.cluster_lba(kernel_first), &mut header)
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

    if !layout.target.write(layout.part_lba, &boot)
        || !layout.target.write(layout.part_lba + 1, &fsinfo)
        || !layout.target.write(layout.part_lba + 6, &boot)
        || !layout.target.write(layout.part_lba + 7, &fsinfo)
    {
        return false;
    }

    let zero = [0u8; 512];
    for fat in 0..NUM_FATS {
        let base = layout.part_lba + RESERVED + fat * layout.fat_size;
        for sector in 0..layout.fat_size {
            if !layout.target.write(base + sector, &zero) {
                return false;
            }
        }
        let mut head = [0u8; 512];
        put32(&mut head, 0, 0x0fff_fff8);
        put32(&mut head, 4, 0x0fff_ffff);
        put32(&mut head, 8, 0x0fff_ffff);
        if !layout.target.write(base, &head) {
            return false;
        }
    }

    let empty = [0u8; 512];
    for sector in 0..SPC {
        if !layout.target.write(layout.cluster_lba(2) + sector, &empty) {
            return false;
        }
    }
    true
}

fn write_gpt(target: Target, disk_sectors: u64, part_first: u64, part_end: u64) -> bool {
    write_gpt_entries(
        target,
        disk_sectors,
        MAX_ENTRIES,
        &[(part_first, part_end - 1, ESP_TYPE_GUID)],
    )
}

/// Writes a protective MBR and both copies of a GPT holding `partitions`
/// (first sector, last sector, type), in a table of `entry_count` slots.
fn write_gpt_entries(
    target: Target,
    disk_sectors: u64,
    entry_count: usize,
    partitions: &[(u64, u64, [u8; 16])],
) -> bool {
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
    if !target.write(0, &mbr) {
        return false;
    }

    let mut disk_guid = [0u8; 16];
    crate::random::fill(&mut disk_guid);
    let mut array = [0u8; ARRAY_BYTES];
    for (slot, &(first, last, kind)) in partitions.iter().enumerate().take(entry_count) {
        let mut part_guid = [0u8; 16];
        crate::random::fill(&mut part_guid);
        let entry = &mut array[slot * ENTRY_BYTES..(slot + 1) * ENTRY_BYTES];
        entry[0..16].copy_from_slice(&kind);
        entry[16..32].copy_from_slice(&part_guid);
        put64(entry, 32, first);
        put64(entry, 40, last);
        for (index, unit) in "EFI System".encode_utf16().enumerate() {
            entry[56 + index * 2..58 + index * 2].copy_from_slice(&unit.to_le_bytes());
        }
    }
    let array_crc = crc32(&array[..entry_count * ENTRY_BYTES]);
    let used_sectors = (entry_count * ENTRY_BYTES).div_ceil(512) as u64;
    let backup_header_lba = disk_sectors - 1;
    let backup_array_lba = disk_sectors - 1 - ARRAY_SECTORS;
    for (header_lba, other, array_lba) in [
        (1u64, backup_header_lba, 2u64),
        (backup_header_lba, 1u64, backup_array_lba),
    ] {
        for sector in 0..used_sectors {
            let mut buffer = [0u8; 512];
            let at = sector as usize * 512;
            buffer.copy_from_slice(&array[at..at + 512]);
            if !target.write(array_lba + sector, &buffer) {
                return false;
            }
        }
        let mut header = [0u8; 512];
        header[0..8].copy_from_slice(b"EFI PART");
        put32(&mut header, 8, 0x0001_0000);
        put32(&mut header, 12, 92);
        put64(&mut header, 24, header_lba);
        put64(&mut header, 32, other);
        put64(&mut header, 40, 34);
        put64(&mut header, 48, disk_sectors - 34);
        header[56..72].copy_from_slice(&disk_guid);
        put64(&mut header, 72, array_lba);
        put32(&mut header, 80, entry_count as u32);
        put32(&mut header, 84, ENTRY_BYTES as u32);
        put32(&mut header, 88, array_crc);
        let header_crc = crc32(&header[0..92]);
        put32(&mut header, 16, header_crc);
        if !target.write(header_lba, &header) {
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

#[cfg(feature = "boot-test")]
fn extent_kind() -> [u8; 16] {
    [
        0xaf, 0x3d, 0xc6, 0x0f, 0x83, 0x84, 0x72, 0x47, 0x8e, 0x79, 0x3d, 0x69, 0xd8, 0x47, 0x7d,
        0xe4,
    ]
}

#[cfg(feature = "boot-test")]
fn build(partitions: &[(u64, u64)], entry_count: usize) -> bool {
    let mut extents = [(0u64, 0u64, [0u8; 16]); 8];
    for (slot, &(first, last)) in partitions.iter().enumerate() {
        extents[slot] = (first, last, extent_kind());
    }
    let zero = [0u8; 512];
    (0..crate::blockdev::RAM_DISK_SECTORS)
        .step_by(1)
        .take(80)
        .all(|lba| Target::Ram.write(lba, &zero))
        && (crate::blockdev::RAM_DISK_SECTORS - 80..crate::blockdev::RAM_DISK_SECTORS)
            .all(|lba| Target::Ram.write(lba, &zero))
        && write_gpt_entries(
            Target::Ram,
            crate::blockdev::RAM_DISK_SECTORS,
            entry_count,
            &extents[..partitions.len()],
        )
}

#[cfg(feature = "boot-test")]
fn copy_is_valid(primary: bool) -> bool {
    let target = Target::Ram;
    let sectors = target.sectors();
    let (lba, other) = if primary {
        (1, sectors - 1)
    } else {
        (sectors - 1, 1)
    };
    let mut header = [0u8; 512];
    target.read(lba, &mut header)
        && header_valid(&header, lba, other, sectors)
        && read_array(target, &header).is_ok()
}

/// Planning and editing a GPT in place on a RAM disk: free gaps, the refusal
/// cases, entries that stay byte for byte as they were, and a table that is
/// still valid in at least one copy after a power cut at every write.
#[cfg(feature = "boot-test")]
pub fn self_test() -> bool {
    use crate::blockdev;
    let sectors = blockdev::RAM_DISK_SECTORS;
    let minimum = 1024;

    let whole = (0..80).all(|lba| Target::Ram.write(lba, &[0; 512]))
        && plan(Target::Ram, false, minimum) == Plan::WholeDisk
        && plan(Target::Ram, true, minimum) == Plan::WholeDisk;

    let table_ok = build(&[(2048, 4095), (8192, 10239)], 128);
    let reading = read_table(Target::Ram).is_ok_and(|table| {
        let mut gaps = [(0u64, 0u64); MAX_ENTRIES + 1];
        let count = table.gaps(&mut gaps);
        count == 2 && gaps[0] == (4096, 8191) && gaps[1] == (10240, sectors - 34)
    });
    let planned = plan(Target::Ram, true, minimum)
        == Plan::FreeSpace {
            first: 10240,
            last: sectors - 34,
        }
        && plan(Target::Ram, false, minimum) == Plan::Refuse(Refusal::NotAllowed)
        && plan(Target::Ram, true, 10_000) == Plan::Refuse(Refusal::NoFreeSpace);
    let inspected = {
        let found = inspect(Target::Ram);
        !found.blank && found.table == Ok(2) && found.free_sectors == sectors - 34 - 10240 + 1
    };

    let mut edit = false;
    if let Ok(mut table) = read_table(Target::Ram) {
        let before = table.array;
        let inserted = table.insert(10240, sectors - 35, &ESP_TYPE_GUID, "EFI System");
        let written = inserted.is_ok() && table.write(Target::Ram);
        edit = written
            && inserted == Ok(2)
            && read_table(Target::Ram).is_ok_and(|after| {
                after.entry(0) == &before[..ENTRY_BYTES]
                    && after.entry(1) == &before[ENTRY_BYTES..2 * ENTRY_BYTES]
                    && after.extent(2) == (10240, sectors - 35)
                    && after.entry(2)[..16] == ESP_TYPE_GUID
                    && (3..after.entries).all(|index| !after.used(index))
            })
            && copy_is_valid(true)
            && copy_is_valid(false)
            && plan(Target::Ram, true, 8000) == Plan::Refuse(Refusal::NoFreeSpace);
    }

    let full = build(&[(2048, 4095), (8192, 10239)], 2)
        && read_table(Target::Ram).is_ok_and(|mut table| {
            table.insert(12288, 14335, &ESP_TYPE_GUID, "x") == Err(Refusal::Full)
        });
    let covered = build(&[(34, sectors - 35)], 128)
        && plan(Target::Ram, true, minimum) == Plan::Refuse(Refusal::NoFreeSpace);

    let mut damage = true;
    for corruption in 0..6 {
        damage &= build(&[(2048, 4095)], 128);
        let (lba, offset) = match corruption {
            0 => (2, 40),
            1 => (1, 16),
            2 => (sectors - 1, 17),
            3 => (sectors - 33, 36),
            4 => (1, 88),
            _ => (1, 0),
        };
        let mut sector = [0u8; 512];
        damage &= Target::Ram.read(lba, &mut sector);
        sector[offset] ^= 0x01;
        damage &= Target::Ram.write(lba, &sector);
        damage &= read_table(Target::Ram).is_err()
            && matches!(plan(Target::Ram, true, minimum), Plan::Refuse(_));
    }
    let overlapping = build(&[(2048, 5000), (4000, 6000)], 128)
        && plan(Target::Ram, true, minimum) == Plan::Refuse(Refusal::Damaged);
    let outside = build(&[(2048, sectors)], 128)
        && plan(Target::Ram, true, minimum) == Plan::Refuse(Refusal::Damaged);

    let mbr_only = {
        let mut mbr = [0u8; 512];
        mbr[446 + 4] = 0x83;
        mbr[510] = 0x55;
        mbr[511] = 0xaa;
        (0..80).all(|lba| Target::Ram.write(lba, &[0; 512]))
            && Target::Ram.write(0, &mbr)
            && plan(Target::Ram, true, minimum) == Plan::Refuse(Refusal::Unsupported)
    };
    let raw = {
        let mut data = [0xa5u8; 512];
        data[510] = 0;
        (0..80).all(|lba| Target::Ram.write(lba, &data))
            && plan(Target::Ram, true, minimum) == Plan::Refuse(Refusal::NoTable)
    };

    let mut atomic = build(&[(2048, 4095), (8192, 10239)], 128);
    let mut cuts = 0;
    for limit in 0..80u32 {
        atomic &= build(&[(2048, 4095), (8192, 10239)], 128);
        let Ok(mut table) = read_table(Target::Ram) else {
            atomic = false;
            break;
        };
        let _ = table.insert(10240, sectors - 35, &ESP_TYPE_GUID, "EFI System");
        blockdev::inject_power_loss_after(limit);
        let finished = table.write(Target::Ram);
        blockdev::clear_power_loss();
        atomic &= copy_is_valid(true) || copy_is_valid(false);
        if finished {
            atomic &= read_table(Target::Ram).is_ok_and(|after| after.used(2));
            break;
        }
        cuts += 1;
    }
    atomic &= cuts > 60;
    let _ = (0..80).all(|lba| Target::Ram.write(lba, &[0; 512]));
    let _ = (sectors - 80..sectors).all(|lba| Target::Ram.write(lba, &[0; 512]));

    whole
        && table_ok
        && reading
        && planned
        && inspected
        && edit
        && full
        && covered
        && damage
        && overlapping
        && outside
        && mbr_only
        && raw
        && atomic
}
