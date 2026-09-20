use crate::ahci;
use crate::partition::{Partition, PartitionReport};
use crate::sync::TicketLock;

const SECTOR_BYTES: u64 = 512;
const MAX_CHAIN: usize = 4096;

static MOUNTED: TicketLock<Option<Volume>> = TicketLock::new(None);

/// The last FAT sector `next_cluster` read. Chain walks are sequential, so
/// one cached sector turns 128 (FAT32) consecutive lookups into one AHCI
/// read. `write_fat_entry` invalidates it.
static FAT_CACHE: TicketLock<Option<(u64, [u8; 512])>> = TicketLock::new(None);

/// Where the last `load_root_file` chain walk stopped: (first cluster of the
/// file, cluster index reached, cluster at that index). A later read at the
/// same or a further offset resumes here instead of re-walking the whole
/// chain from the file start - which is what made every random virtio-blk
/// read into a large ROOTFS cost tens of thousands of FAT lookups.
static CHAIN_HINT: TicketLock<Option<(u32, u64, u32)>> = TicketLock::new(None);

#[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
pub fn root_file_size(name: &[u8; 11]) -> Option<u64> {
    let volume = (*MOUNTED.lock())?;
    let entry = find_root(&volume, name)?;
    if entry.directory || entry.cluster < 2 {
        return None;
    }
    Some(entry.bytes as u64)
}

#[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
pub fn load_root_file(name: &[u8; 11], skip: u64, dest_phys: u64, capacity: u64) -> Option<u64> {
    if !skip.is_multiple_of(SECTOR_BYTES) || !dest_phys.is_multiple_of(SECTOR_BYTES) {
        return None;
    }
    let volume = (*MOUNTED.lock())?;
    let entry = find_root(&volume, name)?;
    if entry.directory || entry.cluster < 2 {
        return None;
    }
    let size = entry.bytes as u64;
    if skip >= size {
        return Some(0);
    }
    let spc = volume.sectors_per_cluster as u64;
    let cluster_sectors = spc;
    let cluster_bytes = spc * SECTOR_BYTES;
    let want = (size - skip).min(capacity);

    let target_index = skip / cluster_bytes;
    let mut cluster = entry.cluster;
    let mut skip_clusters = target_index;
    if let Some((first, index, hinted)) = *CHAIN_HINT.lock()
        && first == entry.cluster
        && index <= target_index
    {
        cluster = hinted;
        skip_clusters = target_index - index;
    }
    while skip_clusters > 0 {
        cluster = match next_cluster(&volume, cluster)? {
            Some(next) if next >= 2 && (next as u64) < volume.clusters + 2 => next,
            _ => return None,
        };
        skip_clusters -= 1;
    }
    *CHAIN_HINT.lock() = Some((entry.cluster, target_index, cluster));
    let mut sector_skip = (skip % cluster_bytes) / SECTOR_BYTES;

    let mut written = 0u64;
    let mut guard = 0usize;
    while written < want {
        guard += 1;
        if guard > MAX_CHAIN * 64 {
            return None;
        }
        let run_start = cluster;
        let mut run = 1u64;
        let mut ended = false;
        // Only walk as far as this request needs; a 4 KiB read must not chase
        // 512 clusters of FAT entries.
        let max_run = (sector_skip * SECTOR_BYTES + (want - written))
            .div_ceil(cluster_bytes)
            .clamp(1, 512);
        loop {
            match next_cluster(&volume, cluster)? {
                Some(next)
                    if next == cluster + 1
                        && (next as u64) < volume.clusters + 2
                        && run < max_run =>
                {
                    cluster = next;
                    run += 1;
                }
                Some(next) if next >= 2 && (next as u64) < volume.clusters + 2 => {
                    cluster = next;
                    break;
                }
                _ => {
                    ended = true;
                    break;
                }
            }
        }
        let first_lba = cluster_lba(&volume, run_start) + sector_skip;
        let run_sectors = run * cluster_sectors - sector_skip;
        let run_bytes = (run_sectors * SECTOR_BYTES).min(want - written);
        let need_sectors = run_bytes.div_ceil(SECTOR_BYTES);
        let mut done = 0u64;
        while done < need_sectors {
            let batch = (need_sectors - done).min(8192) as u32;
            if !ahci::read_into(
                first_lba + done,
                batch,
                dest_phys + written + done * SECTOR_BYTES,
            ) {
                return None;
            }
            done += batch as u64;
        }
        written += run_bytes;
        sector_skip = 0;
        if ended && written < want {
            return None;
        }
    }
    Some(want)
}

pub fn boot_kernel() -> Option<(u32, u64)> {
    let volume = (*MOUNTED.lock())?;
    let efi = find_root(&volume, b"EFI        ")?;
    if !efi.directory {
        return None;
    }
    let boot = find_cluster_directory(&volume, efi.cluster, b"BOOT       ")?;
    if !boot.directory {
        return None;
    }
    let file = find_cluster_directory(&volume, boot.cluster, b"BOOTX64 EFI")?;
    if file.directory || file.cluster < 2 || file.bytes == 0 {
        return None;
    }
    Some((file.cluster, file.bytes as u64))
}

#[derive(Clone, Copy)]
pub struct FatFileEntry {
    pub name: [u8; 11],
    pub bytes: u32,
}

impl FatFileEntry {
    pub const EMPTY: Self = Self {
        name: [0; 11],
        bytes: 0,
    };
}

/// Lists ordinary (non-directory) files sitting directly in the root
/// directory that have a non-blank 8.3 extension - this deliberately
/// excludes the EFI/ directory and the extensionless boot payloads
/// (INITRD, VMLINUZ) the build places in root, surfacing only files a
/// caller such as the VFS's own persistent-storage mount created via
/// [`write_root_file`].
pub fn list_root_files(out: &mut [FatFileEntry]) -> usize {
    let Some(volume) = *MOUNTED.lock() else {
        return 0;
    };
    match volume.fat_bits {
        32 if volume.root_cluster >= 2 => {
            list_directory_chain(&volume, volume.root_cluster, out, false)
        }
        16 => list_fixed_root(&volume, out, false),
        _ => 0,
    }
}

/// Lists subdirectories sitting directly in root that [`create_root_directory`]
/// could have created - excludes the build-placed `EFI` directory by name
/// (a fixed, known sentinel; a user directory named exactly `EFI` under
/// `/data` would collide with this and not be rediscovered after reboot,
/// a disclosed, narrow limitation rather than a silent one).
pub fn list_root_directories(out: &mut [FatFileEntry]) -> usize {
    let Some(volume) = *MOUNTED.lock() else {
        return 0;
    };
    match volume.fat_bits {
        32 if volume.root_cluster >= 2 => {
            list_directory_chain(&volume, volume.root_cluster, out, true)
        }
        16 => list_fixed_root(&volume, out, true),
        _ => 0,
    }
}

fn eligible_entry(entry: &[u8], want_directory: bool) -> bool {
    if entry[0] == 0xe5 || entry[11] == 0x0f || entry[11] & 0x08 != 0 || entry[0] == b'.' {
        return false;
    }
    if (entry[11] & 0x10 != 0) != want_directory {
        return false;
    }
    if want_directory {
        entry[..11] != *b"EFI        "
    } else {
        entry[8..11] != *b"   "
    }
}

fn list_directory_chain(
    volume: &Volume,
    first_cluster: u32,
    out: &mut [FatFileEntry],
    want_directory: bool,
) -> usize {
    let mut count = 0;
    let mut cluster = first_cluster;
    let mut sector = [0u8; 512];
    'outer: for _ in 0..MAX_CHAIN {
        let base = cluster_lba(volume, cluster);
        for s in 0..volume.sectors_per_cluster as u64 {
            if !ahci::read_sector(base + s, &mut sector) {
                break 'outer;
            }
            for entry in sector.chunks_exact(32) {
                if entry[0] == 0 {
                    break 'outer;
                }
                if !eligible_entry(entry, want_directory) {
                    continue;
                }
                if count >= out.len() {
                    break 'outer;
                }
                let mut name = [0u8; 11];
                name.copy_from_slice(&entry[..11]);
                out[count] = FatFileEntry {
                    name,
                    bytes: read_u32(entry, 28),
                };
                count += 1;
            }
        }
        match next_cluster(volume, cluster) {
            Some(Some(next)) if next >= 2 && (next as u64) < volume.clusters + 2 => cluster = next,
            _ => break,
        }
    }
    count
}

fn list_fixed_root(volume: &Volume, out: &mut [FatFileEntry], want_directory: bool) -> usize {
    let root_sectors = (volume.root_entries as u64 * 32).div_ceil(SECTOR_BYTES);
    let mut count = 0;
    let mut sector = [0u8; 512];
    for offset in 0..root_sectors {
        if !ahci::read_sector(volume.first_root + offset, &mut sector) {
            break;
        }
        for entry in sector.chunks_exact(32) {
            if entry[0] == 0 {
                return count;
            }
            if !eligible_entry(entry, want_directory) {
                continue;
            }
            if count >= out.len() {
                return count;
            }
            let mut name = [0u8; 11];
            name.copy_from_slice(&entry[..11]);
            out[count] = FatFileEntry {
                name,
                bytes: read_u32(entry, 28),
            };
            count += 1;
        }
    }
    count
}

/// Deletes a file previously created with [`write_root_file`]: frees its
/// data-cluster chain and marks its directory entry as reusable (0xE5).
/// Only ever touches an entry that exactly matches `name`, so it cannot
/// reach the EFI/BOOT tree.
pub fn delete_root_file(name: &[u8; 11]) -> bool {
    delete_root_entry(name, false)
}

/// Deletes a subdirectory previously created with
/// [`create_root_directory`]. The VFS layer already refuses to remove a
/// directory that still has children in its own (in-memory) node table
/// before this is ever called, and this design never persists content
/// *inside* a `/data` subdirectory - so the FAT-level cluster being
/// removed only ever contains the `.`/`..` entries `create_root_directory`
/// wrote, nothing else can be there to lose.
pub fn delete_root_directory(name: &[u8; 11]) -> bool {
    delete_root_entry(name, true)
}

fn delete_root_entry(name: &[u8; 11], expect_directory: bool) -> bool {
    let Some(volume) = *MOUNTED.lock() else {
        return false;
    };
    let located = match volume.fat_bits {
        32 if volume.root_cluster >= 2 => {
            find_in_sectors_located(&volume, volume.root_cluster, name)
        }
        16 => {
            let root_sectors = (volume.root_entries as u64 * 32).div_ceil(SECTOR_BYTES);
            find_in_sectors_located_fixed(volume.first_root, root_sectors, name)
        }
        _ => None,
    };
    let Some((entry, lba, offset)) = located else {
        return false;
    };
    if entry.directory != expect_directory {
        return false;
    }
    if entry.cluster >= 2 && !free_chain(&volume, entry.cluster) {
        return false;
    }
    let mut sector = [0u8; 512];
    if !ahci::read_sector(lba, &mut sector) {
        return false;
    }
    sector[offset] = 0xe5;
    ahci::write_disk_sector(ahci::boot_disk(), lba, &sector)
}

pub fn stream_clusters(
    first_cluster: u32,
    size: u64,
    dest_phys: u64,
    capacity: u64,
) -> Option<u64> {
    let volume = (*MOUNTED.lock())?;
    stream_entry(&volume, first_cluster, size, dest_phys, capacity)
}

fn stream_entry(
    volume: &Volume,
    first_cluster: u32,
    size: u64,
    dest_phys: u64,
    capacity: u64,
) -> Option<u64> {
    if !dest_phys.is_multiple_of(SECTOR_BYTES) {
        return None;
    }
    let want = size.min(capacity);
    if want == 0 {
        return Some(0);
    }
    if first_cluster < 2 || (first_cluster as u64) >= volume.clusters + 2 {
        return None;
    }
    let cluster_sectors = volume.sectors_per_cluster as u64;
    let mut cluster = first_cluster;
    let mut written = 0u64;
    let mut guard = 0usize;
    while written < want {
        guard += 1;
        if guard > MAX_CHAIN * 64 {
            return None;
        }
        let run_start = cluster;
        let mut run = 1u64;
        let mut ended = false;
        loop {
            match next_cluster(volume, cluster)? {
                Some(next)
                    if next == cluster + 1 && (next as u64) < volume.clusters + 2 && run < 512 =>
                {
                    cluster = next;
                    run += 1;
                }
                Some(next) if next >= 2 && (next as u64) < volume.clusters + 2 => {
                    cluster = next;
                    break;
                }
                _ => {
                    ended = true;
                    break;
                }
            }
        }
        let first_lba = cluster_lba(volume, run_start);
        let run_bytes = (run * cluster_sectors * SECTOR_BYTES).min(want - written);
        let need = run_bytes.div_ceil(SECTOR_BYTES);
        let mut done = 0u64;
        while done < need {
            let batch = (need - done).min(8192) as u32;
            if !ahci::read_into(
                first_lba + done,
                batch,
                dest_phys + written + done * SECTOR_BYTES,
            ) {
                return None;
            }
            done += batch as u64;
        }
        written += run_bytes;
        if ended && written < want {
            return None;
        }
    }
    Some(want)
}

#[derive(Clone, Copy)]
pub struct FatReport {
    pub mounted: bool,
    pub fat_bits: u8,
    pub partition_lba: u64,
    pub volume_sectors: u64,
    pub sectors_per_cluster: u8,
    pub clusters: u64,
    pub fat_sectors: u64,
    pub root_entries: u16,
    pub efi_directory: bool,
    pub boot_directory: bool,
    pub boot_file: bool,
    pub boot_file_bytes: u32,
    pub pe_image: bool,
    pub verified: bool,
}

impl FatReport {
    const EMPTY: Self = Self {
        mounted: false,
        fat_bits: 0,
        partition_lba: 0,
        volume_sectors: 0,
        sectors_per_cluster: 0,
        clusters: 0,
        fat_sectors: 0,
        root_entries: 0,
        efi_directory: false,
        boot_directory: false,
        boot_file: false,
        boot_file_bytes: 0,
        pe_image: false,
        verified: false,
    };
}

#[derive(Clone, Copy)]
struct Volume {
    start: u64,
    sectors: u64,
    sectors_per_cluster: u8,
    first_fat: u64,
    first_root: u64,
    first_data: u64,
    root_entries: u16,
    root_cluster: u32,
    fat_sectors: u64,
    fat_bits: u8,
    clusters: u64,
    fats: u64,
}

#[derive(Clone, Copy)]
struct DirectoryEntry {
    cluster: u32,
    bytes: u32,
    directory: bool,
}

pub fn inspect(partitions: &PartitionReport) -> FatReport {
    for partition in partitions.entries() {
        if !partition.fat_candidate() {
            continue;
        }
        let Some(volume) = read_volume(*partition) else {
            continue;
        };
        *MOUNTED.lock() = Some(volume);
        let Some(efi) = find_root(&volume, b"EFI        ") else {
            return base_report(volume);
        };
        let mut report = base_report(volume);
        report.efi_directory = efi.directory;
        if !efi.directory {
            return report;
        }
        let Some(boot) = find_cluster_directory(&volume, efi.cluster, b"BOOT       ") else {
            return report;
        };
        report.boot_directory = boot.directory;
        if !boot.directory {
            return report;
        }
        let Some(file) = find_cluster_directory(&volume, boot.cluster, b"BOOTX64 EFI") else {
            return report;
        };
        report.boot_file = !file.directory && file.bytes != 0;
        report.boot_file_bytes = file.bytes;
        if report.boot_file && file.cluster >= 2 {
            let mut sector = [0u8; 512];
            let lba = cluster_lba(&volume, file.cluster);
            report.pe_image = ahci::read_sector(lba, &mut sector) && sector[..2] == *b"MZ";
        }
        report.verified = report.mounted
            && report.efi_directory
            && report.boot_directory
            && report.boot_file
            && report.pe_image;
        return report;
    }
    FatReport::EMPTY
}

fn base_report(volume: Volume) -> FatReport {
    FatReport {
        mounted: true,
        fat_bits: volume.fat_bits,
        partition_lba: volume.start,
        volume_sectors: volume.sectors,
        sectors_per_cluster: volume.sectors_per_cluster,
        clusters: volume.clusters,
        fat_sectors: volume.fat_sectors,
        root_entries: volume.root_entries,
        ..FatReport::EMPTY
    }
}

fn read_volume(partition: Partition) -> Option<Volume> {
    let mut sector = [0u8; 512];
    if !ahci::read_sector(partition.first_lba, &mut sector)
        || sector[510] != 0x55
        || sector[511] != 0xaa
        || !matches!(sector[0], 0xeb | 0xe9)
    {
        return None;
    }
    let bytes_per_sector = read_u16(&sector, 11) as u64;
    let sectors_per_cluster = sector[13];
    let reserved = read_u16(&sector, 14) as u64;
    let fats = sector[16] as u64;
    let root_entries = read_u16(&sector, 17);
    let total16 = read_u16(&sector, 19) as u64;
    let total32 = read_u32(&sector, 32) as u64;
    let fat16 = read_u16(&sector, 22) as u64;
    let fat32 = read_u32(&sector, 36) as u64;
    let total = if total16 != 0 { total16 } else { total32 };
    let fat_sectors = if fat16 != 0 { fat16 } else { fat32 };
    if bytes_per_sector != SECTOR_BYTES
        || !sectors_per_cluster.is_power_of_two()
        || sectors_per_cluster > 128
        || reserved == 0
        || !(1..=2).contains(&fats)
        || total == 0
        || total > partition.sectors
        || fat_sectors == 0
    {
        return None;
    }
    let root_sectors = (root_entries as u64 * 32).div_ceil(SECTOR_BYTES);
    let metadata = reserved
        .checked_add(fats.checked_mul(fat_sectors)?)?
        .checked_add(root_sectors)?;
    let data_sectors = total.checked_sub(metadata)?;
    let clusters = data_sectors / sectors_per_cluster as u64;
    let fat_bits = if clusters < 4085 {
        12
    } else if clusters < 65525 {
        16
    } else {
        32
    };
    if fat_bits == 12 || fat_bits == 32 && root_entries != 0 || fat_bits == 16 && root_entries == 0
    {
        return None;
    }
    let first_fat = partition.first_lba.checked_add(reserved)?;
    let first_root = first_fat.checked_add(fats.checked_mul(fat_sectors)?)?;
    let first_data = first_root.checked_add(root_sectors)?;
    let root_cluster = if fat_bits == 32 {
        read_u32(&sector, 44) & 0x0fff_ffff
    } else {
        0
    };
    if fat_bits == 32 && root_cluster < 2 {
        return None;
    }
    Some(Volume {
        start: partition.first_lba,
        sectors: total,
        sectors_per_cluster,
        first_fat,
        first_root,
        first_data,
        root_entries,
        root_cluster,
        fat_sectors,
        fat_bits,
        clusters,
        fats,
    })
}

fn find_root(volume: &Volume, name: &[u8; 11]) -> Option<DirectoryEntry> {
    if volume.fat_bits == 32 {
        find_cluster_directory(volume, volume.root_cluster, name)
    } else {
        let sectors = (volume.root_entries as u64 * 32).div_ceil(SECTOR_BYTES);
        find_in_sectors(volume.first_root, sectors, name)
    }
}

fn find_cluster_directory(
    volume: &Volume,
    first_cluster: u32,
    name: &[u8; 11],
) -> Option<DirectoryEntry> {
    if first_cluster < 2 || first_cluster as u64 >= volume.clusters + 2 {
        return None;
    }
    let mut cluster = first_cluster;
    for _ in 0..MAX_CHAIN {
        if let Some(entry) = find_in_sectors(
            cluster_lba(volume, cluster),
            volume.sectors_per_cluster as u64,
            name,
        ) {
            return Some(entry);
        }
        match next_cluster(volume, cluster)? {
            Some(next) if next >= 2 && (next as u64) < volume.clusters + 2 => cluster = next,
            Some(_) => return None,
            None => return None,
        }
    }
    None
}

fn find_in_sectors(first_lba: u64, count: u64, name: &[u8; 11]) -> Option<DirectoryEntry> {
    let mut sector = [0u8; 512];
    for offset in 0..count {
        if !ahci::read_sector(first_lba + offset, &mut sector) {
            return None;
        }
        for entry in sector.chunks_exact(32) {
            if entry[0] == 0 {
                return None;
            }
            if entry[0] == 0xe5 || entry[11] == 0x0f || entry[11] & 0x08 != 0 {
                continue;
            }
            if entry[..11] == *name {
                return Some(DirectoryEntry {
                    cluster: ((read_u16(entry, 20) as u32) << 16) | read_u16(entry, 26) as u32,
                    bytes: read_u32(entry, 28),
                    directory: entry[11] & 0x10 != 0,
                });
            }
        }
    }
    None
}

/// Creates (or replaces) a file directly in the FAT root directory and
/// writes `data` into it. Supports both FAT32 (the root there is an
/// ordinary cluster chain, so the same allocation/directory-entry code
/// path used for subdirectories applies) and FAT16 (a fixed-size root
/// region, handled separately since it has no cluster chain of its own
/// and can't grow). Never touches the EFI/BOOT tree — this only ever
/// adds/replaces entries whose short name matches `name`.
pub fn write_root_file(name: &[u8; 11], data: &[u8]) -> bool {
    let Some(volume) = *MOUNTED.lock() else {
        return false;
    };
    match volume.fat_bits {
        32 if volume.root_cluster >= 2 => {
            write_file_in_directory(&volume, volume.root_cluster, name, data)
        }
        16 => write_file_in_fixed_root(&volume, name, data),
        _ => false,
    }
}

/// Creates a subdirectory directly in the FAT root: allocates one cluster,
/// writes the conventional `.`/`..` entries into it, then registers the
/// new directory in root. Mirrors [`write_root_file`]'s FAT32/FAT16 split,
/// but only ever creates - there's no replace case for directories, and
/// this only ever adds one level directly under root (matching the VFS's
/// own `/data` mount, which doesn't yet support nested persisted
/// directories either).
pub fn create_root_directory(name: &[u8; 11]) -> bool {
    let Some(volume) = *MOUNTED.lock() else {
        return false;
    };
    match volume.fat_bits {
        32 if volume.root_cluster >= 2 => {
            create_directory_in(&volume, volume.root_cluster, name, volume.root_cluster)
        }
        16 => create_directory_in_fixed_root(&volume, name),
        _ => false,
    }
}

fn create_directory_in(volume: &Volume, dir_cluster: u32, name: &[u8; 11], dotdot: u32) -> bool {
    if find_in_sectors_located(volume, dir_cluster, name).is_some() {
        return false;
    }
    let Some(new_cluster) = allocate_cluster(volume) else {
        return false;
    };
    if !write_dot_entries(volume, new_cluster, dotdot) {
        let _ = write_fat_entry(volume, new_cluster, 0);
        return false;
    }
    let Some((slot_lba, slot_offset)) = find_or_extend_free_slot(volume, dir_cluster, name) else {
        free_chain(volume, new_cluster);
        return false;
    };
    write_directory_entry_attr(slot_lba, slot_offset, name, new_cluster, 0, 0x10)
}

fn create_directory_in_fixed_root(volume: &Volume, name: &[u8; 11]) -> bool {
    let root_sectors = (volume.root_entries as u64 * 32).div_ceil(SECTOR_BYTES);
    if find_in_sectors_located_fixed(volume.first_root, root_sectors, name).is_some() {
        return false;
    }
    let Some(new_cluster) = allocate_cluster(volume) else {
        return false;
    };
    // A directory whose parent is FAT16's fixed root has no cluster number
    // to point ".." at (that root isn't cluster-based) - 0 is the FAT
    // convention for "parent is root".
    if !write_dot_entries(volume, new_cluster, 0) {
        let _ = write_fat_entry(volume, new_cluster, 0);
        return false;
    }
    let Some((slot_lba, slot_offset)) = find_free_slot_fixed(volume.first_root, root_sectors, name)
    else {
        free_chain(volume, new_cluster);
        return false;
    };
    write_directory_entry_attr(slot_lba, slot_offset, name, new_cluster, 0, 0x10)
}

fn write_dot_entries(volume: &Volume, cluster: u32, dotdot_cluster: u32) -> bool {
    if !zero_cluster(volume, cluster) {
        return false;
    }
    let lba = cluster_lba(volume, cluster);
    let mut sector = [0u8; 512];
    if !ahci::read_sector(lba, &mut sector) {
        return false;
    }
    sector[..11].copy_from_slice(b".          ");
    sector[11] = 0x10;
    sector[20] = (cluster >> 16) as u8;
    sector[21] = (cluster >> 24) as u8;
    sector[26] = cluster as u8;
    sector[27] = (cluster >> 8) as u8;
    sector[32..43].copy_from_slice(b"..         ");
    sector[43] = 0x10;
    sector[52] = (dotdot_cluster >> 16) as u8;
    sector[53] = (dotdot_cluster >> 24) as u8;
    sector[58] = dotdot_cluster as u8;
    sector[59] = (dotdot_cluster >> 8) as u8;
    ahci::write_disk_sector(ahci::boot_disk(), lba, &sector)
}

/// Reads back a file previously written with [`write_root_file`] into a
/// plain kernel-memory buffer. Returns the number of bytes copied.
pub fn read_root_file(name: &[u8; 11], buffer: &mut [u8]) -> Option<usize> {
    let volume = (*MOUNTED.lock())?;
    let entry = find_root(&volume, name)?;
    if entry.directory {
        return None;
    }
    let want = (entry.bytes as usize).min(buffer.len());
    if want == 0 {
        return Some(0);
    }
    if entry.cluster < 2 || (entry.cluster as u64) >= volume.clusters + 2 {
        return None;
    }
    let cluster_bytes = volume.sectors_per_cluster as usize * SECTOR_BYTES as usize;
    let mut cluster = entry.cluster;
    let mut copied = 0usize;
    let mut sector = [0u8; 512];
    let mut guard = 0usize;
    while copied < want {
        guard += 1;
        if guard > MAX_CHAIN {
            return None;
        }
        let mut offset_in_cluster = 0usize;
        while offset_in_cluster < cluster_bytes && copied < want {
            let lba = cluster_lba(&volume, cluster) + (offset_in_cluster as u64 / SECTOR_BYTES);
            if !ahci::read_sector(lba, &mut sector) {
                return None;
            }
            let take = (want - copied).min(SECTOR_BYTES as usize);
            buffer[copied..copied + take].copy_from_slice(&sector[..take]);
            copied += take;
            offset_in_cluster += SECTOR_BYTES as usize;
        }
        if copied >= want {
            break;
        }
        match next_cluster(&volume, cluster)? {
            Some(next) if next >= 2 && (next as u64) < volume.clusters + 2 => cluster = next,
            _ => return None,
        }
    }
    Some(copied)
}

fn write_file_in_directory(
    volume: &Volume,
    dir_cluster: u32,
    name: &[u8; 11],
    data: &[u8],
) -> bool {
    if let Some((existing, _lba, _offset)) = find_in_sectors_located(volume, dir_cluster, name) {
        if existing.directory {
            return false;
        }
        if existing.cluster >= 2 {
            free_chain(volume, existing.cluster);
        }
    }
    let cluster_bytes = volume.sectors_per_cluster as u64 * SECTOR_BYTES;
    let clusters_needed = if data.is_empty() {
        0
    } else {
        (data.len() as u64).div_ceil(cluster_bytes)
    };
    let first_cluster = if clusters_needed > 0 {
        match allocate_chain(volume, clusters_needed, data) {
            Some(cluster) => cluster,
            None => return false,
        }
    } else {
        0
    };
    let Some((slot_lba, slot_offset)) = find_or_extend_free_slot(volume, dir_cluster, name) else {
        if first_cluster >= 2 {
            free_chain(volume, first_cluster);
        }
        return false;
    };
    write_directory_entry(
        slot_lba,
        slot_offset,
        name,
        first_cluster,
        data.len() as u32,
    )
}

fn write_file_in_fixed_root(volume: &Volume, name: &[u8; 11], data: &[u8]) -> bool {
    let root_sectors = (volume.root_entries as u64 * 32).div_ceil(SECTOR_BYTES);
    if let Some((existing, _lba, _offset)) =
        find_in_sectors_located_fixed(volume.first_root, root_sectors, name)
    {
        if existing.directory {
            return false;
        }
        if existing.cluster >= 2 {
            free_chain(volume, existing.cluster);
        }
    }
    let cluster_bytes = volume.sectors_per_cluster as u64 * SECTOR_BYTES;
    let clusters_needed = if data.is_empty() {
        0
    } else {
        (data.len() as u64).div_ceil(cluster_bytes)
    };
    let first_cluster = if clusters_needed > 0 {
        match allocate_chain(volume, clusters_needed, data) {
            Some(cluster) => cluster,
            None => return false,
        }
    } else {
        0
    };
    let Some((slot_lba, slot_offset)) = find_free_slot_fixed(volume.first_root, root_sectors, name)
    else {
        if first_cluster >= 2 {
            free_chain(volume, first_cluster);
        }
        return false;
    };
    write_directory_entry(
        slot_lba,
        slot_offset,
        name,
        first_cluster,
        data.len() as u32,
    )
}

fn find_in_sectors_located_fixed(
    first_lba: u64,
    count: u64,
    name: &[u8; 11],
) -> Option<(DirectoryEntry, u64, usize)> {
    let mut sector = [0u8; 512];
    for offset in 0..count {
        let lba = first_lba + offset;
        if !ahci::read_sector(lba, &mut sector) {
            return None;
        }
        for (slot, entry) in sector.chunks_exact(32).enumerate() {
            if entry[0] == 0 {
                return None;
            }
            if entry[0] == 0xe5 || entry[11] == 0x0f || entry[11] & 0x08 != 0 {
                continue;
            }
            if entry[..11] == *name {
                return Some((
                    DirectoryEntry {
                        cluster: ((read_u16(entry, 20) as u32) << 16) | read_u16(entry, 26) as u32,
                        bytes: read_u32(entry, 28),
                        directory: entry[11] & 0x10 != 0,
                    },
                    lba,
                    slot * 32,
                ));
            }
        }
    }
    None
}

fn find_free_slot_fixed(first_lba: u64, count: u64, name: &[u8; 11]) -> Option<(u64, usize)> {
    let mut sector = [0u8; 512];
    for offset in 0..count {
        let lba = first_lba + offset;
        if !ahci::read_sector(lba, &mut sector) {
            return None;
        }
        for (slot, entry) in sector.chunks_exact(32).enumerate() {
            if entry[0] == 0 || entry[0] == 0xe5 || entry[..11] == *name {
                return Some((lba, slot * 32));
            }
        }
    }
    None
}

fn find_in_sectors_located(
    volume: &Volume,
    dir_cluster: u32,
    name: &[u8; 11],
) -> Option<(DirectoryEntry, u64, usize)> {
    let mut cluster = dir_cluster;
    let mut sector = [0u8; 512];
    for _ in 0..MAX_CHAIN {
        let base = cluster_lba(volume, cluster);
        for s in 0..volume.sectors_per_cluster as u64 {
            if !ahci::read_sector(base + s, &mut sector) {
                return None;
            }
            for (slot, entry) in sector.chunks_exact(32).enumerate() {
                if entry[0] == 0 {
                    return None;
                }
                if entry[0] == 0xe5 || entry[11] == 0x0f || entry[11] & 0x08 != 0 {
                    continue;
                }
                if entry[..11] == *name {
                    return Some((
                        DirectoryEntry {
                            cluster: ((read_u16(entry, 20) as u32) << 16)
                                | read_u16(entry, 26) as u32,
                            bytes: read_u32(entry, 28),
                            directory: entry[11] & 0x10 != 0,
                        },
                        base + s,
                        slot * 32,
                    ));
                }
            }
        }
        match next_cluster(volume, cluster)? {
            Some(next) if next >= 2 && (next as u64) < volume.clusters + 2 => cluster = next,
            _ => return None,
        }
    }
    None
}

fn find_or_extend_free_slot(
    volume: &Volume,
    dir_cluster: u32,
    name: &[u8; 11],
) -> Option<(u64, usize)> {
    let mut cluster = dir_cluster;
    let mut sector = [0u8; 512];
    let mut last_cluster = dir_cluster;
    for _ in 0..MAX_CHAIN {
        last_cluster = cluster;
        let base = cluster_lba(volume, cluster);
        for s in 0..volume.sectors_per_cluster as u64 {
            let lba = base + s;
            if !ahci::read_sector(lba, &mut sector) {
                return None;
            }
            for (slot, entry) in sector.chunks_exact(32).enumerate() {
                if entry[0] == 0 || entry[0] == 0xe5 {
                    return Some((lba, slot * 32));
                }
                if entry[..11] == *name {
                    // Being replaced by the caller's free_chain step; this
                    // slot will be reused directly rather than treated as
                    // occupied.
                    return Some((lba, slot * 32));
                }
            }
        }
        match next_cluster(volume, cluster) {
            Some(Some(next)) if next >= 2 && (next as u64) < volume.clusters + 2 => cluster = next,
            Some(None) => break,
            _ => return None,
        }
    }
    let new_cluster = allocate_cluster(volume)?;
    if !write_fat_entry(volume, last_cluster, new_cluster) {
        return None;
    }
    if !zero_cluster(volume, new_cluster) {
        return None;
    }
    Some((cluster_lba(volume, new_cluster), 0))
}

fn write_directory_entry(
    lba: u64,
    offset: usize,
    name: &[u8; 11],
    cluster: u32,
    size: u32,
) -> bool {
    write_directory_entry_attr(lba, offset, name, cluster, size, 0x20)
}

fn write_directory_entry_attr(
    lba: u64,
    offset: usize,
    name: &[u8; 11],
    cluster: u32,
    size: u32,
    attributes: u8,
) -> bool {
    let mut sector = [0u8; 512];
    if !ahci::read_sector(lba, &mut sector) {
        return false;
    }
    let entry = &mut sector[offset..offset + 32];
    entry.fill(0);
    entry[..11].copy_from_slice(name);
    entry[11] = attributes;
    entry[20] = (cluster >> 16) as u8;
    entry[21] = (cluster >> 24) as u8;
    entry[26] = cluster as u8;
    entry[27] = (cluster >> 8) as u8;
    entry[28..32].copy_from_slice(&size.to_le_bytes());
    ahci::write_disk_sector(ahci::boot_disk(), lba, &sector)
}

/// Allocates and links a `clusters_needed`-long chain, writing `data` into
/// it as it goes. Any failure partway through (the disk fills up, or a
/// sector write fails) frees every cluster this call allocated before
/// returning `None` - without that, a disk-full write would silently leak
/// the clusters it got partway through allocating: they'd stay marked
/// used in the FAT forever, un-freeable by anything since no directory
/// entry ever points at them; ordinary use (writes that succeed) is
/// unaffected either way.
fn allocate_chain(volume: &Volume, clusters_needed: u64, data: &[u8]) -> Option<u32> {
    let mut first: Option<u32> = None;
    let mut previous: Option<u32> = None;
    let cluster_bytes = volume.sectors_per_cluster as u64 * SECTOR_BYTES;
    let mut written = 0u64;
    for _ in 0..clusters_needed {
        let Some(cluster) = allocate_cluster(volume) else {
            if let Some(first_cluster) = first {
                free_chain(volume, first_cluster);
            }
            return None;
        };
        if first.is_none() {
            first = Some(cluster);
        }
        if let Some(prev) = previous
            && !write_fat_entry(volume, prev, cluster)
        {
            // `cluster` was allocated above but the link from `prev` to
            // it never took, so it isn't reachable by walking the chain
            // from `first` - free it individually as well, or it leaks.
            let _ = write_fat_entry(volume, cluster, 0);
            if let Some(first_cluster) = first {
                free_chain(volume, first_cluster);
            }
            return None;
        }
        let chunk_len = (data.len() as u64 - written).min(cluster_bytes) as usize;
        if !write_cluster_data(
            volume,
            cluster,
            &data[written as usize..written as usize + chunk_len],
        ) {
            if let Some(first_cluster) = first {
                free_chain(volume, first_cluster);
            }
            return None;
        }
        written += chunk_len as u64;
        previous = Some(cluster);
    }
    first
}

fn write_cluster_data(volume: &Volume, cluster: u32, chunk: &[u8]) -> bool {
    let mut sector = [0u8; 512];
    let base = cluster_lba(volume, cluster);
    let mut offset = 0usize;
    for s in 0..volume.sectors_per_cluster as u64 {
        sector.fill(0);
        let take = (chunk.len() - offset).min(SECTOR_BYTES as usize);
        if take > 0 {
            sector[..take].copy_from_slice(&chunk[offset..offset + take]);
        }
        if !ahci::write_disk_sector(ahci::boot_disk(), base + s, &sector) {
            return false;
        }
        offset += take;
        if offset >= chunk.len() {
            break;
        }
    }
    true
}

fn zero_cluster(volume: &Volume, cluster: u32) -> bool {
    let sector = [0u8; 512];
    let base = cluster_lba(volume, cluster);
    for s in 0..volume.sectors_per_cluster as u64 {
        if !ahci::write_disk_sector(ahci::boot_disk(), base + s, &sector) {
            return false;
        }
    }
    true
}

fn allocate_cluster(volume: &Volume) -> Option<u32> {
    let entry_bytes: u64 = match volume.fat_bits {
        16 => 2,
        32 => 4,
        _ => return None,
    };
    let eoc: u32 = if volume.fat_bits == 16 {
        0xffff
    } else {
        0x0fff_ffff
    };
    let mut sector = [0u8; 512];
    let max_cluster = volume.clusters as u32 + 2;
    for cluster in 2..max_cluster {
        let byte_offset = cluster as u64 * entry_bytes;
        let lba = volume.first_fat + byte_offset / SECTOR_BYTES;
        if !ahci::read_sector(lba, &mut sector) {
            return None;
        }
        let offset = (byte_offset % SECTOR_BYTES) as usize;
        let value = if volume.fat_bits == 16 {
            read_u16(&sector, offset) as u32
        } else {
            read_u32(&sector, offset) & 0x0fff_ffff
        };
        if value == 0 {
            if write_fat_entry(volume, cluster, eoc) {
                return Some(cluster);
            }
            return None;
        }
    }
    None
}

fn write_fat_entry(volume: &Volume, cluster: u32, value: u32) -> bool {
    let entry_bytes: u64 = match volume.fat_bits {
        16 => 2,
        32 => 4,
        _ => return false,
    };
    let byte_offset = cluster as u64 * entry_bytes;
    let sector_index = byte_offset / SECTOR_BYTES;
    let offset = (byte_offset % SECTOR_BYTES) as usize;
    let mut sector = [0u8; 512];
    *FAT_CACHE.lock() = None;
    *CHAIN_HINT.lock() = None;
    for copy in 0..volume.fats {
        let lba = volume.first_fat + copy * volume.fat_sectors + sector_index;
        if !ahci::read_sector(lba, &mut sector) {
            return false;
        }
        if volume.fat_bits == 16 {
            let stored = value as u16;
            sector[offset..offset + 2].copy_from_slice(&stored.to_le_bytes());
        } else {
            let preserved = read_u32(&sector, offset) & 0xf000_0000;
            let stored = (value & 0x0fff_ffff) | preserved;
            sector[offset..offset + 4].copy_from_slice(&stored.to_le_bytes());
        }
        if !ahci::write_disk_sector(ahci::boot_disk(), lba, &sector) {
            return false;
        }
    }
    true
}

fn free_chain(volume: &Volume, first_cluster: u32) -> bool {
    let mut cluster = first_cluster;
    let mut guard = 0usize;
    loop {
        guard += 1;
        if guard > MAX_CHAIN {
            return false;
        }
        let next = next_cluster(volume, cluster);
        if !write_fat_entry(volume, cluster, 0) {
            return false;
        }
        match next {
            Some(Some(value)) if value >= 2 && (value as u64) < volume.clusters + 2 => {
                cluster = value
            }
            _ => return true,
        }
    }
}

fn next_cluster(volume: &Volume, cluster: u32) -> Option<Option<u32>> {
    let byte_offset = match volume.fat_bits {
        16 => cluster as u64 * 2,
        32 => cluster as u64 * 4,
        _ => return None,
    };
    let lba = volume.first_fat + byte_offset / SECTOR_BYTES;
    let mut sector = [0u8; 512];
    let cached = *FAT_CACHE.lock();
    match cached {
        Some((cached_lba, data)) if cached_lba == lba => sector = data,
        _ => {
            if !ahci::read_sector(lba, &mut sector) {
                return None;
            }
            *FAT_CACHE.lock() = Some((lba, sector));
        }
    }
    let offset = (byte_offset % SECTOR_BYTES) as usize;
    let value = if volume.fat_bits == 16 {
        read_u16(&sector, offset) as u32
    } else {
        read_u32(&sector, offset) & 0x0fff_ffff
    };
    let end = if volume.fat_bits == 16 {
        value >= 0xfff8
    } else {
        value >= 0x0fff_fff8
    };
    if end { Some(None) } else { Some(Some(value)) }
}

fn cluster_lba(volume: &Volume, cluster: u32) -> u64 {
    volume.first_data + (cluster as u64 - 2) * volume.sectors_per_cluster as u64
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}
