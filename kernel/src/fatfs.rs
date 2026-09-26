//! A general FAT16/FAT32 filesystem: formatting, subdirectories, long file
//! names (VFAT), timestamps, files up to 4 GiB, read/write/truncate/rename/
//! delete. It sits on any disk the kernel has a driver for (`Disk`) and is
//! write-through (every change is on the disk when the call returns).

use crate::{ahci, nvme, sdhci, virtio_blk, xhci};

pub const NAME_MAX: usize = 255;
const SECTOR: usize = 512;
const CACHE_SLOTS: usize = 16;
const ENTRY: usize = 32;

const ATTR_READ_ONLY: u8 = 0x01;
const ATTR_VOLUME: u8 = 0x08;
const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_ARCHIVE: u8 = 0x20;
const ATTR_LFN: u8 = 0x0f;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FsError {
    Io,
    NotFound,
    Exists,
    NotDirectory,
    IsDirectory,
    NotEmpty,
    NoSpace,
    ReadOnly,
    InvalidName,
    Corrupt,
    Unsupported,
    TooLarge,
}

/// AHCI DMA needs a 2-byte-aligned buffer; callers' buffers (stack arrays,
/// cache slots) have no alignment guarantee, so transfers bounce through this.
#[repr(align(4096))]
struct Bounce([u8; 4096]);

static BOUNCE: crate::sync::TicketLock<Bounce> = crate::sync::TicketLock::new(Bounce([0; 4096]));

fn bounce(
    disk: usize,
    lba: u64,
    count: usize,
    read: Option<&mut [u8]>,
    write: Option<&[u8]>,
) -> bool {
    let mut area = BOUNCE.lock();
    let address = area.0.as_mut_ptr() as u64;
    let bytes = count * SECTOR;
    if let Some(source) = write {
        area.0[..bytes].copy_from_slice(&source[..bytes]);
        return ahci::write_disk(disk, lba, count as u32, address);
    }
    if !ahci::read_disk(disk, lba, count as u32, address) {
        return false;
    }
    if let Some(destination) = read {
        destination[..bytes].copy_from_slice(&area.0[..bytes]);
    }
    true
}

/// A block device the filesystem can live on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Disk {
    Ahci(usize),
    Nvme,
    Usb,
    Sd,
    Virtio,
}

impl Disk {
    pub fn sectors(self) -> u64 {
        match self {
            Disk::Ahci(index) => ahci::disk_sectors(index),
            Disk::Nvme => {
                if nvme::sector_bytes() == 512 {
                    nvme::sectors()
                } else {
                    0
                }
            }
            Disk::Usb => xhci::storage_sectors(),
            Disk::Sd => sdhci::sectors(),
            Disk::Virtio => virtio_blk::sectors(),
        }
    }

    /// Reads 1..=8 sectors into `buffer`.
    fn read_run(self, lba: u64, count: usize, buffer: &mut [u8]) -> bool {
        match self {
            Disk::Ahci(index) => bounce(index, lba, count, Some(buffer), None),
            Disk::Nvme => nvme::read(lba, count as u32, buffer),
            Disk::Usb => xhci::storage_read(lba, count, buffer),
            Disk::Sd => sdhci::read(lba, count, buffer),
            Disk::Virtio => virtio_blk::read(lba, count, buffer),
        }
    }

    fn write_run(self, lba: u64, count: usize, buffer: &[u8]) -> bool {
        match self {
            Disk::Ahci(index) => bounce(index, lba, count, None, Some(buffer)),
            Disk::Nvme => nvme::write(lba, count as u32, buffer),
            Disk::Usb => xhci::storage_write(lba, count, buffer),
            Disk::Sd => sdhci::write(lba, count, buffer),
            Disk::Virtio => virtio_blk::write(lba, count, buffer),
        }
    }
}

/// Reads one sector of `disk` (used to sniff a disk before mounting it).
pub fn read_sector_raw(disk: Disk, lba: u64, out: &mut [u8; SECTOR]) -> bool {
    disk.read_run(lba, 1, out)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FatType {
    Fat16,
    Fat32,
}

/// One mounted volume.
pub struct Fs {
    disk: Disk,
    start: u64,
    fat_type: FatType,
    sectors_per_cluster: u32,
    reserved: u32,
    fat_count: u32,
    fat_sectors: u32,
    label: [u8; 11],
    root_cluster: u32,
    /// First sector of the fixed FAT16 root directory (relative to `start`).
    root_start: u32,
    root_sectors: u32,
    data_start: u32,
    clusters: u32,
    free_hint: u32,
    cache_lba: [u64; CACHE_SLOTS],
    cache: [[u8; SECTOR]; CACHE_SLOTS],
}

/// What the filesystem knows about one file or directory (also the handle
/// state the VFS keeps for an open file).
#[derive(Clone, Copy)]
pub struct Node {
    /// Directory holding the entry (0 = the FAT16 root directory).
    pub dir: u32,
    /// Index of the short entry inside its directory.
    pub index: u32,
    /// Index of the first entry of the entry set (long name entries first).
    pub first_index: u32,
    pub first_cluster: u32,
    pub size: u32,
    pub attributes: u8,
    /// FAT date/time of the last modification.
    pub date: u16,
    pub time: u16,
    /// Position cache for sequential access: chain index -> cluster.
    cursor_index: u32,
    cursor_cluster: u32,
}

impl Node {
    pub const EMPTY: Node = Node {
        dir: 0,
        index: 0,
        first_index: 0,
        first_cluster: 0,
        size: 0,
        attributes: 0,
        date: 0,
        time: 0,
        cursor_index: 0,
        cursor_cluster: 0,
    };

    pub fn is_directory(&self) -> bool {
        self.attributes & ATTR_DIRECTORY != 0
    }

    pub fn read_only(&self) -> bool {
        self.attributes & ATTR_READ_ONLY != 0
    }

    /// The directory-cluster number to use when listing/looking inside this node.
    pub fn as_dir(&self) -> u32 {
        self.first_cluster
    }
}

/// One directory listing entry.
#[derive(Clone, Copy)]
pub struct Listed {
    pub node: Node,
    pub name: [u8; NAME_MAX],
    pub name_len: usize,
}

impl Listed {
    pub fn name(&self) -> &[u8] {
        &self.name[..self.name_len]
    }
}

pub struct FsInfo {
    pub bytes_per_cluster: u32,
    pub clusters: u32,
    pub free_clusters: u32,
    pub fat32: bool,
}

fn le16(bytes: &[u8], at: usize) -> u32 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]]) as u32
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn put16(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 2].copy_from_slice(&(value as u16).to_le_bytes());
}

fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

/// Current time as FAT (date, time).
fn fat_now() -> (u16, u16) {
    let seconds = crate::rtc::unix_seconds();
    let days = (seconds / 86_400) as i64;
    let of_day = seconds % 86_400;
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    if year < 1980 {
        return (0x0021, 0);
    }
    let date = (((year - 1980) as u32) << 9 | (month as u32) << 5 | day as u32) as u16;
    let time = ((of_day / 3600) as u32) << 11
        | (((of_day / 60) % 60) as u32) << 5
        | ((of_day % 60) / 2) as u32;
    (date, time as u16)
}

/// FAT (date, time) as Unix seconds (UTC); 0 for an unset timestamp.
pub fn fat_to_unix(date: u16, time: u16) -> u64 {
    let year = 1980 + (date >> 9) as i64;
    let month = ((date >> 5) & 15) as i64;
    let day = (date & 31) as i64;
    if date == 0 || !(1..=12).contains(&month) || day == 0 {
        return 0;
    }
    // Days from civil (Howard Hinnant).
    let shifted_year = if month <= 2 { year - 1 } else { year };
    let era = shifted_year.div_euclid(400);
    let year_of_era = shifted_year.rem_euclid(400);
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    let seconds =
        (time >> 11) as i64 * 3600 + ((time >> 5) & 63) as i64 * 60 + (time & 31) as i64 * 2;
    (days * 86_400 + seconds).max(0) as u64
}

fn checksum(short: &[u8; 11]) -> u8 {
    let mut sum = 0u8;
    for byte in short {
        sum = sum.rotate_right(1).wrapping_add(*byte);
    }
    sum
}

/// Upper-cases an ASCII letter; other bytes are unchanged.
fn fold(byte: u8) -> u8 {
    byte.to_ascii_uppercase()
}

fn names_equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| fold(*x) == fold(*y))
}

/// Decodes UTF-8 into UCS-2 (BMP only). Returns the number of units.
fn to_ucs2(name: &[u8], out: &mut [u16; NAME_MAX]) -> Result<usize, FsError> {
    let text = core::str::from_utf8(name).map_err(|_| FsError::InvalidName)?;
    let mut count = 0;
    for character in text.chars() {
        let value = character as u32;
        if value > 0xffff || count == NAME_MAX {
            return Err(FsError::InvalidName);
        }
        out[count] = value as u16;
        count += 1;
    }
    Ok(count)
}

/// Encodes UCS-2 as UTF-8; returns the byte length.
fn from_ucs2(units: &[u16], out: &mut [u8; NAME_MAX]) -> usize {
    let mut length = 0;
    for unit in units {
        let character = char::from_u32(*unit as u32).unwrap_or('?');
        let mut buffer = [0u8; 4];
        let encoded = character.encode_utf8(&mut buffer);
        if length + encoded.len() > NAME_MAX {
            break;
        }
        out[length..length + encoded.len()].copy_from_slice(encoded.as_bytes());
        length += encoded.len();
    }
    length
}

fn valid_name(name: &[u8]) -> bool {
    if name.is_empty() || name.len() > NAME_MAX || name == b"." || name == b".." {
        return false;
    }
    if name.last() == Some(&b' ') || name.last() == Some(&b'.') {
        return false;
    }
    !name
        .iter()
        .any(|byte| *byte < 0x20 || b"\"*/:<>?\\|".contains(byte))
}

impl Fs {
    // ---------------------------------------------------------------- disk

    fn absolute(&self, relative: u32) -> u64 {
        self.start + relative as u64
    }

    fn read_sector(&mut self, relative: u32, out: &mut [u8; SECTOR]) -> Result<(), FsError> {
        let lba = self.absolute(relative);
        let slot = (lba as usize) % CACHE_SLOTS;
        if self.cache_lba[slot] != lba {
            let mut sector = [0u8; SECTOR];
            if !self.disk.read_run(lba, 1, &mut sector) {
                return Err(FsError::Io);
            }
            self.cache[slot] = sector;
            self.cache_lba[slot] = lba;
        }
        *out = self.cache[slot];
        Ok(())
    }

    fn write_sector(&mut self, relative: u32, data: &[u8; SECTOR]) -> Result<(), FsError> {
        let lba = self.absolute(relative);
        if !self.disk.write_run(lba, 1, data) {
            return Err(FsError::Io);
        }
        let slot = (lba as usize) % CACHE_SLOTS;
        self.cache[slot] = *data;
        self.cache_lba[slot] = lba;
        Ok(())
    }

    /// Reads whole sectors straight into `out` (multiples of 512 bytes).
    fn read_run(&mut self, relative: u32, out: &mut [u8]) -> Result<(), FsError> {
        let mut done = 0usize;
        let total = out.len() / SECTOR;
        while done < total {
            let count = (total - done).min(8);
            let lba = self.absolute(relative + done as u32);
            if !self
                .disk
                .read_run(lba, count, &mut out[done * SECTOR..(done + count) * SECTOR])
            {
                return Err(FsError::Io);
            }
            done += count;
        }
        Ok(())
    }

    fn write_run(&mut self, relative: u32, data: &[u8]) -> Result<(), FsError> {
        let mut done = 0usize;
        let total = data.len() / SECTOR;
        while done < total {
            let count = (total - done).min(8);
            let lba = self.absolute(relative + done as u32);
            if !self
                .disk
                .write_run(lba, count, &data[done * SECTOR..(done + count) * SECTOR])
            {
                return Err(FsError::Io);
            }
            // Keep the cache coherent with what was just written.
            for index in 0..count {
                let cached = (lba as usize + index) % CACHE_SLOTS;
                if self.cache_lba[cached] == lba + index as u64 {
                    self.cache_lba[cached] = u64::MAX;
                }
            }
            done += count;
        }
        Ok(())
    }

    // ------------------------------------------------------------------ FAT

    fn cluster_lba(&self, cluster: u32) -> u32 {
        self.data_start + (cluster - 2) * self.sectors_per_cluster
    }

    fn cluster_bytes(&self) -> u32 {
        self.sectors_per_cluster * SECTOR as u32
    }

    fn fat_get(&mut self, cluster: u32) -> Result<u32, FsError> {
        if cluster < 2 || cluster >= self.clusters + 2 {
            return Err(FsError::Corrupt);
        }
        let per_entry = if self.fat_type == FatType::Fat16 {
            2
        } else {
            4
        };
        let offset = cluster * per_entry;
        let mut sector = [0u8; SECTOR];
        self.read_sector(self.reserved + offset / SECTOR as u32, &mut sector)?;
        let at = (offset % SECTOR as u32) as usize;
        Ok(match self.fat_type {
            FatType::Fat16 => le16(&sector, at),
            FatType::Fat32 => le32(&sector, at) & 0x0fff_ffff,
        })
    }

    fn fat_set(&mut self, cluster: u32, value: u32) -> Result<(), FsError> {
        let per_entry = if self.fat_type == FatType::Fat16 {
            2
        } else {
            4
        };
        let offset = cluster * per_entry;
        let at = (offset % SECTOR as u32) as usize;
        for copy in 0..self.fat_count {
            let sector_number = self.reserved + copy * self.fat_sectors + offset / SECTOR as u32;
            let mut sector = [0u8; SECTOR];
            self.read_sector(sector_number, &mut sector)?;
            match self.fat_type {
                FatType::Fat16 => put16(&mut sector, at, value),
                FatType::Fat32 => {
                    let keep = le32(&sector, at) & 0xf000_0000;
                    put32(&mut sector, at, keep | (value & 0x0fff_ffff));
                }
            }
            self.write_sector(sector_number, &sector)?;
        }
        Ok(())
    }

    fn is_end(&self, value: u32) -> bool {
        match self.fat_type {
            FatType::Fat16 => value >= 0xfff8,
            FatType::Fat32 => value >= 0x0fff_fff8,
        }
    }

    fn end_marker(&self) -> u32 {
        match self.fat_type {
            FatType::Fat16 => 0xffff,
            FatType::Fat32 => 0x0fff_ffff,
        }
    }

    /// Next cluster of a chain, or `None` at its end.
    fn next_cluster(&mut self, cluster: u32) -> Result<Option<u32>, FsError> {
        let value = self.fat_get(cluster)?;
        if self.is_end(value) || value < 2 {
            return Ok(None);
        }
        if value >= self.clusters + 2 {
            return Err(FsError::Corrupt);
        }
        Ok(Some(value))
    }

    /// Allocates one free cluster, marks it as a chain end and returns it.
    fn alloc_cluster(&mut self) -> Result<u32, FsError> {
        let total = self.clusters;
        let mut candidate = self.free_hint.max(2);
        for _ in 0..total {
            if candidate >= total + 2 {
                candidate = 2;
            }
            if self.fat_get(candidate)? == 0 {
                self.fat_set(candidate, self.end_marker())?;
                self.free_hint = candidate + 1;
                return Ok(candidate);
            }
            candidate += 1;
        }
        Err(FsError::NoSpace)
    }

    fn free_chain(&mut self, first: u32) -> Result<(), FsError> {
        let mut cluster = first;
        let mut guard = 0u32;
        while cluster >= 2 {
            let next = self.next_cluster(cluster)?;
            self.fat_set(cluster, 0)?;
            if cluster < self.free_hint {
                self.free_hint = cluster;
            }
            guard += 1;
            if guard > self.clusters {
                return Err(FsError::Corrupt);
            }
            match next {
                Some(next) => cluster = next,
                None => break,
            }
        }
        Ok(())
    }

    fn zero_cluster(&mut self, cluster: u32) -> Result<(), FsError> {
        let zeros = [0u8; SECTOR];
        for index in 0..self.sectors_per_cluster {
            self.write_sector(self.cluster_lba(cluster) + index, &zeros)?;
        }
        Ok(())
    }

    // ---------------------------------------------------------- directories

    /// Relative sector number of the `sector_index`-th sector of a directory.
    fn dir_sector(&mut self, dir: u32, sector_index: u32) -> Result<Option<u32>, FsError> {
        if dir == 0 {
            return Ok((sector_index < self.root_sectors).then_some(self.root_start + sector_index));
        }
        let hops = sector_index / self.sectors_per_cluster;
        let mut cluster = dir;
        for _ in 0..hops {
            match self.next_cluster(cluster)? {
                Some(next) => cluster = next,
                None => return Ok(None),
            }
        }
        Ok(Some(
            self.cluster_lba(cluster) + sector_index % self.sectors_per_cluster,
        ))
    }

    fn read_entry(&mut self, dir: u32, index: u32) -> Result<Option<[u8; ENTRY]>, FsError> {
        let per_sector = (SECTOR / ENTRY) as u32;
        let Some(sector) = self.dir_sector(dir, index / per_sector)? else {
            return Ok(None);
        };
        let mut data = [0u8; SECTOR];
        self.read_sector(sector, &mut data)?;
        let at = (index % per_sector) as usize * ENTRY;
        let mut entry = [0u8; ENTRY];
        entry.copy_from_slice(&data[at..at + ENTRY]);
        Ok(Some(entry))
    }

    fn write_entry(&mut self, dir: u32, index: u32, entry: &[u8; ENTRY]) -> Result<(), FsError> {
        let per_sector = (SECTOR / ENTRY) as u32;
        let Some(sector) = self.dir_sector(dir, index / per_sector)? else {
            return Err(FsError::NoSpace);
        };
        let mut data = [0u8; SECTOR];
        self.read_sector(sector, &mut data)?;
        let at = (index % per_sector) as usize * ENTRY;
        data[at..at + ENTRY].copy_from_slice(entry);
        self.write_sector(sector, &data)
    }

    /// Adds a zeroed cluster to a (cluster-based) directory.
    fn extend_dir(&mut self, dir: u32) -> Result<(), FsError> {
        if dir == 0 {
            return Err(FsError::NoSpace);
        }
        let new = self.alloc_cluster()?;
        self.zero_cluster(new)?;
        let mut last = dir;
        while let Some(next) = self.next_cluster(last)? {
            last = next;
        }
        self.fat_set(last, new)
    }

    fn node_from_entry(&self, dir: u32, index: u32, first_index: u32, entry: &[u8; ENTRY]) -> Node {
        let high = if self.fat_type == FatType::Fat32 {
            le16(entry, 20)
        } else {
            0
        };
        Node {
            dir,
            index,
            first_index,
            first_cluster: high << 16 | le16(entry, 26),
            size: le32(entry, 28),
            attributes: entry[11],
            date: le16(entry, 24) as u16,
            time: le16(entry, 22) as u16,
            cursor_index: 0,
            cursor_cluster: 0,
        }
    }

    /// Reads the next entry of a directory listing starting at `*cursor`
    /// (an entry index); skips deleted entries, "." and "..", volume labels.
    pub fn list_next(&mut self, dir: u32, cursor: &mut u32) -> Result<Option<Listed>, FsError> {
        let mut units = [0u16; 260];
        let mut expected = 0u8;
        let mut check = 0u8;
        let mut lfn_first = 0u32;
        let mut seen_mask = 0u32;
        loop {
            let index = *cursor;
            let Some(entry) = self.read_entry(dir, index)? else {
                return Ok(None);
            };
            *cursor += 1;
            match entry[0] {
                0x00 => {
                    *cursor = index;
                    return Ok(None);
                }
                0xe5 => {
                    expected = 0;
                    continue;
                }
                _ => {}
            }
            if entry[11] & 0x3f == ATTR_LFN {
                let order = entry[0];
                let sequence = (order & 0x1f) as usize;
                if order & 0x40 != 0 {
                    units = [0xffff; 260];
                    expected = sequence as u8;
                    check = entry[13];
                    lfn_first = index;
                    seen_mask = 0;
                }
                if sequence == 0 || sequence > 20 || expected == 0 || entry[13] != check {
                    expected = 0;
                    continue;
                }
                let base = (sequence - 1) * 13;
                for (position, offset) in
                    (base..).zip([1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30])
                {
                    units[position] = le16(&entry, offset) as u16;
                }
                seen_mask |= 1 << sequence;
                continue;
            }
            if entry[11] & ATTR_VOLUME != 0 {
                expected = 0;
                continue;
            }
            if entry[0] == b'.' && (entry[1] == b' ' || (entry[1] == b'.' && entry[2] == b' ')) {
                expected = 0;
                continue;
            }
            let mut short = [0u8; 11];
            short.copy_from_slice(&entry[..11]);
            let node;
            let mut name = [0u8; NAME_MAX];
            let name_len;
            let complete = expected != 0
                && seen_mask == (1u32 << (expected as u32 + 1)) - 2
                && checksum(&short) == check;
            if complete {
                let mut length = 0;
                while length < units.len() && units[length] != 0 && units[length] != 0xffff {
                    length += 1;
                }
                name_len = from_ucs2(&units[..length], &mut name);
                node = self.node_from_entry(dir, index, lfn_first, &entry);
            } else {
                // Short name with NT lower-case flags (bit 3 base, bit 4 extension).
                let base_len = short[..8]
                    .iter()
                    .rposition(|b| *b != b' ')
                    .map_or(0, |i| i + 1);
                let ext_len = short[8..]
                    .iter()
                    .rposition(|b| *b != b' ')
                    .map_or(0, |i| i + 1);
                let mut length = 0;
                for (position, byte) in short[..base_len].iter().enumerate() {
                    let _ = position;
                    name[length] = if entry[12] & 0x08 != 0 {
                        byte.to_ascii_lowercase()
                    } else {
                        *byte
                    };
                    length += 1;
                }
                if ext_len > 0 {
                    name[length] = b'.';
                    length += 1;
                    for byte in &short[8..8 + ext_len] {
                        name[length] = if entry[12] & 0x10 != 0 {
                            byte.to_ascii_lowercase()
                        } else {
                            *byte
                        };
                        length += 1;
                    }
                }
                name_len = length;
                node = self.node_from_entry(dir, index, index, &entry);
                // 0x05 stands in for a leading 0xE5 byte.
                if short[0] == 0x05 && name_len > 0 {
                    name[0] = 0xe5;
                }
            }
            return Ok(Some(Listed {
                node,
                name,
                name_len,
            }));
        }
    }

    /// Finds `name` (case-insensitively) inside a directory.
    pub fn find(&mut self, dir: u32, name: &[u8]) -> Result<Node, FsError> {
        let mut cursor = 0;
        while let Some(listed) = self.list_next(dir, &mut cursor)? {
            if names_equal(listed.name(), name) {
                return Ok(listed.node);
            }
        }
        Err(FsError::NotFound)
    }

    /// The root directory as a node.
    pub fn root(&self) -> Node {
        Node {
            dir: 0,
            index: 0,
            first_index: 0,
            first_cluster: if self.fat_type == FatType::Fat32 {
                self.root_cluster
            } else {
                0
            },
            size: 0,
            attributes: ATTR_DIRECTORY,
            date: 0,
            time: 0,
            cursor_index: 0,
            cursor_cluster: 0,
        }
    }

    /// Resolves an absolute `/a/b/c` path (relative to the volume root).
    pub fn resolve(&mut self, path: &[u8]) -> Result<Node, FsError> {
        let mut node = self.root();
        for component in path.split(|byte| *byte == b'/') {
            if component.is_empty() || component == b"." {
                continue;
            }
            if !node.is_directory() {
                return Err(FsError::NotDirectory);
            }
            node = self.find(node.as_dir(), component)?;
        }
        Ok(node)
    }

    /// Splits `path` into its parent directory node and final component.
    pub fn resolve_parent<'a>(&mut self, path: &'a [u8]) -> Result<(Node, &'a [u8]), FsError> {
        let trimmed = path.strip_suffix(b"/").unwrap_or(path);
        let split = trimmed.iter().rposition(|byte| *byte == b'/');
        let (parent_path, name) = match split {
            Some(position) => (&trimmed[..position], &trimmed[position + 1..]),
            None => (&b""[..], trimmed),
        };
        let parent = self.resolve(parent_path)?;
        if !parent.is_directory() {
            return Err(FsError::NotDirectory);
        }
        Ok((parent, name))
    }

    // ------------------------------------------------------------- creating

    /// Builds the 8.3 short name for `name`; `exact` is true when the long
    /// name equals it (no long-name entries needed).
    fn make_short(&mut self, dir: u32, name: &[u8]) -> Result<([u8; 11], bool), FsError> {
        let dot = name.iter().rposition(|byte| *byte == b'.');
        let (base_raw, ext_raw) = match dot {
            Some(position) if position > 0 => (&name[..position], &name[position + 1..]),
            _ => (name, &b""[..]),
        };
        let clean = |source: &[u8], limit: usize, lossy: &mut bool| {
            let mut out = [b' '; 8];
            let mut length = 0;
            for byte in source {
                if *byte == b' ' || *byte == b'.' {
                    *lossy = true;
                    continue;
                }
                let byte = if *byte >= 0x80 || b"+,;=[]".contains(byte) {
                    *lossy = true;
                    b'_'
                } else {
                    fold(*byte)
                };
                if length == limit {
                    *lossy = true;
                    break;
                }
                out[length] = byte;
                length += 1;
            }
            (out, length)
        };
        let mut lossy = name.iter().any(|byte| byte.is_ascii_lowercase());
        let (base, base_len) = clean(base_raw, 8, &mut lossy);
        let (ext, ext_len) = clean(ext_raw, 3, &mut lossy);
        if base_len == 0 {
            lossy = true;
        }
        let mut short = [b' '; 11];
        short[..base_len].copy_from_slice(&base[..base_len]);
        short[8..8 + ext_len].copy_from_slice(&ext[..ext_len]);
        if !lossy {
            // Exactly representable: still must be unique.
            if !self.short_exists(dir, &short)? {
                return Ok((short, true));
            }
            return Err(FsError::Exists);
        }
        if base_len == 0 {
            short[..1].copy_from_slice(b"_");
        }
        let stem_len = base_len.clamp(1, 6);
        for number in 1..1_000_000u32 {
            let mut digits = [0u8; 8];
            let mut count = 0;
            let mut value = number;
            while value > 0 {
                digits[count] = b'0' + (value % 10) as u8;
                value /= 10;
                count += 1;
            }
            let stem = stem_len.min(7 - count);
            let mut candidate = short;
            for slot in candidate[stem..8].iter_mut() {
                *slot = b' ';
            }
            candidate[stem] = b'~';
            for index in 0..count {
                candidate[stem + 1 + index] = digits[count - 1 - index];
            }
            if !self.short_exists(dir, &candidate)? {
                return Ok((candidate, false));
            }
        }
        Err(FsError::NoSpace)
    }

    fn short_exists(&mut self, dir: u32, short: &[u8; 11]) -> Result<bool, FsError> {
        let mut index = 0;
        loop {
            let Some(entry) = self.read_entry(dir, index)? else {
                return Ok(false);
            };
            if entry[0] == 0 {
                return Ok(false);
            }
            if entry[0] != 0xe5 && entry[11] & 0x3f != ATTR_LFN && entry[..11] == short[..] {
                return Ok(true);
            }
            index += 1;
        }
    }

    /// Finds (growing the directory if needed) `count` consecutive free
    /// entries and returns the index of the first.
    fn find_free_slots(&mut self, dir: u32, count: u32) -> Result<u32, FsError> {
        let mut index = 0u32;
        let mut run_start = 0u32;
        let mut run = 0u32;
        loop {
            match self.read_entry(dir, index)? {
                None => {
                    // Past the end of the directory's clusters: grow it.
                    self.extend_dir(dir)?;
                    continue;
                }
                Some(entry) => {
                    if entry[0] == 0x00 {
                        // Everything from here on is free.
                        let first = if run > 0 { run_start } else { index };
                        let last = first + count - 1;
                        // Make sure the space exists.
                        while self.read_entry(dir, last)?.is_none() {
                            self.extend_dir(dir)?;
                        }
                        return Ok(first);
                    }
                    if entry[0] == 0xe5 {
                        if run == 0 {
                            run_start = index;
                        }
                        run += 1;
                        if run == count {
                            return Ok(run_start);
                        }
                    } else {
                        run = 0;
                    }
                }
            }
            index += 1;
        }
    }

    /// Writes the directory entries (long name + short) for a new node.
    fn add_entry(
        &mut self,
        dir: u32,
        name: &[u8],
        attributes: u8,
        first_cluster: u32,
        size: u32,
    ) -> Result<Node, FsError> {
        if !valid_name(name) {
            return Err(FsError::InvalidName);
        }
        if self.find(dir, name).is_ok() {
            return Err(FsError::Exists);
        }
        let (short, exact) = match self.make_short(dir, name) {
            Ok(result) => result,
            Err(FsError::Exists) => return Err(FsError::Exists),
            Err(error) => return Err(error),
        };
        let mut units = [0u16; NAME_MAX];
        let length = to_ucs2(name, &mut units)?;
        let lfn_entries = if exact { 0 } else { length.div_ceil(13) as u32 };
        let first = self.find_free_slots(dir, lfn_entries + 1)?;
        let sum = checksum(&short);
        for sequence in 0..lfn_entries {
            let number = lfn_entries - sequence; // written last-part first
            let mut entry = [0u8; ENTRY];
            entry[0] = number as u8 | if sequence == 0 { 0x40 } else { 0 };
            entry[11] = ATTR_LFN;
            entry[13] = sum;
            let base = (number as usize - 1) * 13;
            for (position, offset) in [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30]
                .into_iter()
                .enumerate()
            {
                let at = base + position;
                let unit = if at < length {
                    units[at]
                } else if at == length {
                    0
                } else {
                    0xffff
                };
                put16(&mut entry, offset, unit as u32);
            }
            self.write_entry(dir, first + sequence, &entry)?;
        }
        let (date, time) = fat_now();
        let mut entry = [0u8; ENTRY];
        entry[..11].copy_from_slice(&short);
        entry[11] = attributes;
        put16(&mut entry, 14, time as u32);
        put16(&mut entry, 16, date as u32);
        put16(&mut entry, 18, date as u32);
        put16(&mut entry, 20, first_cluster >> 16);
        put16(&mut entry, 22, time as u32);
        put16(&mut entry, 24, date as u32);
        put16(&mut entry, 26, first_cluster & 0xffff);
        put32(&mut entry, 28, size);
        let index = first + lfn_entries;
        self.write_entry(dir, index, &entry)?;
        Ok(self.node_from_entry(dir, index, first, &entry))
    }

    /// Creates an empty file in `dir`.
    pub fn create_file(&mut self, dir: u32, name: &[u8]) -> Result<Node, FsError> {
        self.add_entry(dir, name, ATTR_ARCHIVE, 0, 0)
    }

    /// Creates a directory in `dir`.
    pub fn create_dir(&mut self, dir: u32, name: &[u8]) -> Result<Node, FsError> {
        if !valid_name(name) {
            return Err(FsError::InvalidName);
        }
        if self.find(dir, name).is_ok() {
            return Err(FsError::Exists);
        }
        let cluster = self.alloc_cluster()?;
        if let Err(error) = self.zero_cluster(cluster) {
            let _ = self.free_chain(cluster);
            return Err(error);
        }
        let (date, time) = fat_now();
        let mut dot = [0u8; ENTRY];
        dot[..11].copy_from_slice(b".          ");
        dot[11] = ATTR_DIRECTORY;
        put16(&mut dot, 20, cluster >> 16);
        put16(&mut dot, 22, time as u32);
        put16(&mut dot, 24, date as u32);
        put16(&mut dot, 26, cluster & 0xffff);
        let mut dotdot = dot;
        dotdot[..11].copy_from_slice(b"..         ");
        // ".." names the parent; 0 means the root directory.
        let parent = if dir == self.root_cluster && self.fat_type == FatType::Fat32 {
            0
        } else {
            dir
        };
        put16(&mut dotdot, 20, parent >> 16);
        put16(&mut dotdot, 26, parent & 0xffff);
        self.write_entry(cluster, 0, &dot)?;
        self.write_entry(cluster, 1, &dotdot)?;
        match self.add_entry(dir, name, ATTR_DIRECTORY, cluster, 0) {
            Ok(node) => Ok(node),
            Err(error) => {
                let _ = self.free_chain(cluster);
                Err(error)
            }
        }
    }

    // ---------------------------------------------------------- file access

    /// The cluster holding chain position `target`, optionally allocating.
    fn cluster_at(
        &mut self,
        node: &mut Node,
        target: u32,
        allocate: bool,
    ) -> Result<Option<u32>, FsError> {
        let (mut index, mut cluster) = if node.cursor_cluster != 0 && node.cursor_index <= target {
            (node.cursor_index, node.cursor_cluster)
        } else {
            if node.first_cluster == 0 {
                if !allocate {
                    return Ok(None);
                }
                let first = self.alloc_cluster()?;
                node.first_cluster = first;
                self.update_entry(node)?;
            }
            (0, node.first_cluster)
        };
        while index < target {
            match self.next_cluster(cluster)? {
                Some(next) => cluster = next,
                None => {
                    if !allocate {
                        return Ok(None);
                    }
                    let new = self.alloc_cluster()?;
                    self.fat_set(cluster, new)?;
                    cluster = new;
                }
            }
            index += 1;
        }
        node.cursor_index = index;
        node.cursor_cluster = cluster;
        Ok(Some(cluster))
    }

    /// Writes a node's size/cluster/time back into its directory entry.
    pub fn update_entry(&mut self, node: &Node) -> Result<(), FsError> {
        let Some(mut entry) = self.read_entry(node.dir, node.index)? else {
            return Err(FsError::Corrupt);
        };
        put16(&mut entry, 20, node.first_cluster >> 16);
        put16(&mut entry, 26, node.first_cluster & 0xffff);
        put32(&mut entry, 28, node.size);
        put16(&mut entry, 22, node.time as u32);
        put16(&mut entry, 24, node.date as u32);
        put16(&mut entry, 18, node.date as u32);
        entry[11] = node.attributes;
        self.write_entry(node.dir, node.index, &entry)
    }

    pub fn read_at(
        &mut self,
        node: &mut Node,
        offset: u64,
        out: &mut [u8],
    ) -> Result<usize, FsError> {
        if node.is_directory() {
            return Err(FsError::IsDirectory);
        }
        if offset >= node.size as u64 {
            return Ok(0);
        }
        let want = out.len().min((node.size as u64 - offset) as usize);
        let cluster_bytes = self.cluster_bytes() as u64;
        let mut done = 0usize;
        while done < want {
            let position = offset + done as u64;
            let chain = (position / cluster_bytes) as u32;
            let within = (position % cluster_bytes) as usize;
            let Some(cluster) = self.cluster_at(node, chain, false)? else {
                return Err(FsError::Corrupt);
            };
            let span = (cluster_bytes as usize - within).min(want - done);
            let sector_in_cluster = (within / SECTOR) as u32;
            let base = self.cluster_lba(cluster) + sector_in_cluster;
            let first_offset = within % SECTOR;
            if first_offset == 0 && span >= SECTOR {
                // Whole sectors straight into the caller's buffer.
                let bytes = span / SECTOR * SECTOR;
                self.read_run(base, &mut out[done..done + bytes])?;
                done += bytes;
            } else {
                let mut sector = [0u8; SECTOR];
                self.read_sector(base, &mut sector)?;
                let count = (SECTOR - first_offset).min(span);
                out[done..done + count]
                    .copy_from_slice(&sector[first_offset..first_offset + count]);
                done += count;
            }
        }
        Ok(done)
    }

    pub fn write_at(
        &mut self,
        node: &mut Node,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, FsError> {
        if node.is_directory() {
            return Err(FsError::IsDirectory);
        }
        if data.is_empty() {
            return Ok(0);
        }
        let end = offset + data.len() as u64;
        if end > u32::MAX as u64 {
            return Err(FsError::TooLarge);
        }
        if offset > node.size as u64 {
            // Growing past the end: the gap reads as zeros.
            self.fill_zero(node, node.size as u64, offset)?;
        }
        let cluster_bytes = self.cluster_bytes() as u64;
        let mut done = 0usize;
        while done < data.len() {
            let position = offset + done as u64;
            let chain = (position / cluster_bytes) as u32;
            let within = (position % cluster_bytes) as usize;
            let Some(cluster) = self.cluster_at(node, chain, true)? else {
                return Err(FsError::NoSpace);
            };
            let span = (cluster_bytes as usize - within).min(data.len() - done);
            let sector_in_cluster = (within / SECTOR) as u32;
            let base = self.cluster_lba(cluster) + sector_in_cluster;
            let first_offset = within % SECTOR;
            if first_offset == 0 && span >= SECTOR {
                let bytes = span / SECTOR * SECTOR;
                self.write_run(base, &data[done..done + bytes])?;
                done += bytes;
            } else {
                let mut sector = [0u8; SECTOR];
                let count = (SECTOR - first_offset).min(span);
                // A sector past the old end holds junk: don't read it back.
                let existing = ((position / SECTOR as u64) * SECTOR as u64) < node.size as u64;
                if existing {
                    self.read_sector(base, &mut sector)?;
                }
                sector[first_offset..first_offset + count]
                    .copy_from_slice(&data[done..done + count]);
                self.write_sector(base, &sector)?;
                done += count;
            }
        }
        if end > node.size as u64 {
            node.size = end as u32;
        }
        let (date, time) = fat_now();
        node.date = date;
        node.time = time;
        node.attributes |= ATTR_ARCHIVE;
        self.update_entry(node)?;
        Ok(done)
    }

    fn fill_zero(&mut self, node: &mut Node, from: u64, to: u64) -> Result<(), FsError> {
        let zeros = [0u8; 4096];
        let mut position = from;
        while position < to {
            let count = ((to - position) as usize).min(zeros.len());
            // Bypass the size update's recursion: write raw, then set the size.
            let saved = node.size;
            node.size = position as u32;
            self.write_at(node, position, &zeros[..count])?;
            node.size = saved.max((position + count as u64) as u32);
            position += count as u64;
        }
        Ok(())
    }

    /// Shrinks or grows (zero-filling) a file to `length` bytes.
    pub fn truncate(&mut self, node: &mut Node, length: u64) -> Result<(), FsError> {
        if node.is_directory() {
            return Err(FsError::IsDirectory);
        }
        if length > u32::MAX as u64 {
            return Err(FsError::TooLarge);
        }
        if length > node.size as u64 {
            let old = node.size as u64;
            self.fill_zero(node, old, length)?;
            node.size = length as u32;
        } else {
            let cluster_bytes = self.cluster_bytes() as u64;
            let keep = length.div_ceil(cluster_bytes) as u32;
            if keep == 0 {
                if node.first_cluster != 0 {
                    self.free_chain(node.first_cluster)?;
                }
                node.first_cluster = 0;
            } else if let Some(last) = self.cluster_at(node, keep - 1, false)? {
                if let Some(rest) = self.next_cluster(last)? {
                    self.free_chain(rest)?;
                }
                self.fat_set(last, self.end_marker())?;
            }
            node.size = length as u32;
            node.cursor_index = 0;
            node.cursor_cluster = 0;
        }
        let (date, time) = fat_now();
        node.date = date;
        node.time = time;
        self.update_entry(node)
    }

    pub fn set_read_only(&mut self, node: &mut Node, read_only: bool) -> Result<(), FsError> {
        if read_only {
            node.attributes |= ATTR_READ_ONLY;
        } else {
            node.attributes &= !ATTR_READ_ONLY;
        }
        self.update_entry(node)
    }

    // ---------------------------------------------------------- removing

    /// Whether a directory holds nothing but "." and "..".
    pub fn dir_is_empty(&mut self, dir: u32) -> Result<bool, FsError> {
        let mut cursor = 0;
        Ok(self.list_next(dir, &mut cursor)?.is_none())
    }

    fn delete_entries(&mut self, node: &Node) -> Result<(), FsError> {
        for index in node.first_index..=node.index {
            let Some(mut entry) = self.read_entry(node.dir, index)? else {
                return Err(FsError::Corrupt);
            };
            entry[0] = 0xe5;
            self.write_entry(node.dir, index, &entry)?;
        }
        Ok(())
    }

    /// Deletes a file or an empty directory.
    pub fn remove(&mut self, node: &Node) -> Result<(), FsError> {
        if node.is_directory() && !self.dir_is_empty(node.as_dir())? {
            return Err(FsError::NotEmpty);
        }
        self.delete_entries(node)?;
        if node.first_cluster >= 2 {
            self.free_chain(node.first_cluster)?;
        }
        Ok(())
    }

    /// Moves/renames `node` to `name` inside `new_dir`. An existing file at
    /// the destination is replaced when `replace` is set.
    pub fn rename(
        &mut self,
        node: &Node,
        new_dir: u32,
        name: &[u8],
        replace: bool,
    ) -> Result<Node, FsError> {
        if !valid_name(name) {
            return Err(FsError::InvalidName);
        }
        let mut same = false;
        if let Ok(existing) = self.find(new_dir, name) {
            if existing.dir == node.dir && existing.index == node.index {
                // Only the spelling changes (e.g. the case): drop the old
                // entries first so the new name doesn't collide with them.
                same = true;
                self.delete_entries(node)?;
            } else if !replace || existing.is_directory() {
                return Err(FsError::Exists);
            } else {
                self.remove(&existing)?;
            }
        }
        if node.is_directory() && new_dir != node.dir {
            // A directory can't move into itself or a descendant.
            let mut check = new_dir;
            while check != 0 && check >= 2 {
                if check == node.first_cluster {
                    return Err(FsError::InvalidName);
                }
                let Some(entry) = self.read_entry(check, 1)? else {
                    break;
                };
                let parent = le16(&entry, 20) << 16 | le16(&entry, 26);
                if parent == check {
                    break;
                }
                check = parent;
            }
        }
        let moved = self.add_entry(
            new_dir,
            name,
            node.attributes,
            node.first_cluster,
            node.size,
        )?;
        // Keep the original timestamps.
        let mut kept = moved;
        kept.date = node.date;
        kept.time = node.time;
        self.update_entry(&kept)?;
        if !same {
            self.delete_entries(node)?;
        }
        if node.is_directory() && new_dir != node.dir && node.first_cluster >= 2 {
            let parent = if self.fat_type == FatType::Fat32 && new_dir == self.root_cluster {
                0
            } else {
                new_dir
            };
            if let Some(mut dotdot) = self.read_entry(node.first_cluster, 1)? {
                put16(&mut dotdot, 20, parent >> 16);
                put16(&mut dotdot, 26, parent & 0xffff);
                self.write_entry(node.first_cluster, 1, &dotdot)?;
            }
        }
        Ok(kept)
    }

    // ------------------------------------------------------------- volume

    /// The volume label (space padded, as stored in the boot sector).
    pub fn label(&self) -> &[u8; 11] {
        &self.label
    }

    pub fn info(&mut self) -> Result<FsInfo, FsError> {
        let mut free = 0;
        for cluster in 2..self.clusters + 2 {
            if self.fat_get(cluster)? == 0 {
                free += 1;
            }
        }
        Ok(FsInfo {
            bytes_per_cluster: self.cluster_bytes(),
            clusters: self.clusters,
            free_clusters: free,
            fat32: self.fat_type == FatType::Fat32,
        })
    }

    /// Mounts the FAT volume that starts at `start` (sector) on `disk`.
    pub fn mount(disk: Disk, start: u64) -> Result<Box64, FsError> {
        let mut boot = [0u8; SECTOR];
        if !disk.read_run(start, 1, &mut boot) {
            return Err(FsError::Io);
        }
        if boot[510] != 0x55 || boot[511] != 0xaa {
            return Err(FsError::Unsupported);
        }
        if le16(&boot, 11) != SECTOR as u32 {
            return Err(FsError::Unsupported);
        }
        let sectors_per_cluster = boot[13] as u32;
        let reserved = le16(&boot, 14);
        let fat_count = boot[16] as u32;
        let root_entries = le16(&boot, 17);
        let total16 = le16(&boot, 19);
        let fat16_sectors = le16(&boot, 22);
        let total32 = le32(&boot, 32);
        let fat32_sectors = le32(&boot, 36);
        if sectors_per_cluster == 0
            || !sectors_per_cluster.is_power_of_two()
            || reserved == 0
            || fat_count == 0
            || fat_count > 4
        {
            return Err(FsError::Unsupported);
        }
        let fat_sectors = if fat16_sectors != 0 {
            fat16_sectors
        } else {
            fat32_sectors
        };
        let total_sectors = if total16 != 0 { total16 } else { total32 };
        let root_sectors = (root_entries * ENTRY as u32).div_ceil(SECTOR as u32);
        let root_start = reserved + fat_count * fat_sectors;
        let data_start = root_start + root_sectors;
        if fat_sectors == 0 || total_sectors <= data_start {
            return Err(FsError::Unsupported);
        }
        let clusters = (total_sectors - data_start) / sectors_per_cluster;
        let fat_type = if clusters < 4085 {
            return Err(FsError::Unsupported); // FAT12
        } else if clusters < 65525 {
            FatType::Fat16
        } else {
            FatType::Fat32
        };
        let mut fs = Box64::new(Fs {
            disk,
            start,
            fat_type,
            sectors_per_cluster,
            reserved,
            fat_count,
            fat_sectors,
            label: {
                let at = if fat_type == FatType::Fat32 { 71 } else { 43 };
                let mut label = [b' '; 11];
                label.copy_from_slice(&boot[at..at + 11]);
                label
            },
            root_cluster: if fat_type == FatType::Fat32 {
                le32(&boot, 44)
            } else {
                0
            },
            root_start,
            root_sectors,
            data_start,
            clusters,
            free_hint: 2,
            cache_lba: [u64::MAX; CACHE_SLOTS],
            cache: [[0; SECTOR]; CACHE_SLOTS],
        });
        if fat_type == FatType::Fat32 && fs.get().root_cluster < 2 {
            return Err(FsError::Corrupt);
        }
        Ok(fs)
    }

    /// Writes a fresh FAT16/FAT32 filesystem over `sectors` sectors at `start`.
    pub fn format(disk: Disk, start: u64, sectors: u64, label: &[u8]) -> Result<(), FsError> {
        if sectors < 4200 || sectors > u32::MAX as u64 {
            return Err(FsError::Unsupported);
        }
        let total = sectors as u32;
        // FAT32 for big volumes, FAT16 for small ones.
        let fat32 = sectors >= 1_048_576;
        let sectors_per_cluster: u32 = if fat32 {
            8
        } else {
            let mut spc = 1u32;
            while total / spc >= 65_000 {
                spc *= 2;
            }
            spc
        };
        let reserved: u32 = if fat32 { 32 } else { 1 };
        let root_entries: u32 = if fat32 { 0 } else { 512 };
        let root_sectors = root_entries * ENTRY as u32 / SECTOR as u32;
        let per_entry = if fat32 { 4 } else { 2 };
        // Solve for the FAT size (each FAT copy) iteratively.
        let fat_count = 2u32;
        let mut fat_sectors = 1u32;
        loop {
            let data = total - reserved - fat_count * fat_sectors - root_sectors;
            let clusters = data / sectors_per_cluster;
            let needed = ((clusters + 2) * per_entry).div_ceil(SECTOR as u32);
            if needed <= fat_sectors {
                break;
            }
            fat_sectors = needed;
        }
        let data = total - reserved - fat_count * fat_sectors - root_sectors;
        let clusters = data / sectors_per_cluster;
        if fat32 && clusters < 65525 || !fat32 && !(4085..65525).contains(&clusters) {
            return Err(FsError::Unsupported);
        }
        let mut boot = [0u8; SECTOR];
        boot[0..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
        boot[3..11].copy_from_slice(b"AEROS   ");
        put16(&mut boot, 11, SECTOR as u32);
        boot[13] = sectors_per_cluster as u8;
        put16(&mut boot, 14, reserved);
        boot[16] = fat_count as u8;
        put16(&mut boot, 17, root_entries);
        if !fat32 && total < 65_536 {
            put16(&mut boot, 19, total);
        } else {
            put32(&mut boot, 32, total);
        }
        boot[21] = 0xf8;
        put16(&mut boot, 24, 32); // sectors per track
        put16(&mut boot, 26, 64); // heads
        put32(&mut boot, 28, start as u32); // hidden sectors
        let mut volume_label = [b' '; 11];
        for (slot, byte) in volume_label.iter_mut().zip(label.iter().take(11)) {
            *slot = fold(*byte);
        }
        let serial = crate::rtc::unix_seconds() as u32 ^ 0xae05_1234;
        if fat32 {
            put32(&mut boot, 36, fat_sectors);
            put32(&mut boot, 44, 2); // root cluster
            put16(&mut boot, 48, 1); // FSInfo sector
            put16(&mut boot, 50, 6); // backup boot sector
            boot[64] = 0x80;
            boot[66] = 0x29;
            put32(&mut boot, 67, serial);
            boot[71..82].copy_from_slice(&volume_label);
            boot[82..90].copy_from_slice(b"FAT32   ");
        } else {
            put16(&mut boot, 22, fat_sectors);
            boot[36] = 0x80;
            boot[38] = 0x29;
            put32(&mut boot, 39, serial);
            boot[43..54].copy_from_slice(&volume_label);
            boot[54..62].copy_from_slice(if fat32 { b"FAT32   " } else { b"FAT16   " });
        }
        boot[510] = 0x55;
        boot[511] = 0xaa;
        let write = |relative: u32, data: &[u8; SECTOR]| -> Result<(), FsError> {
            if disk.write_run(start + relative as u64, 1, data) {
                Ok(())
            } else {
                Err(FsError::Io)
            }
        };
        let zeros = [0u8; SECTOR];
        // Clear the reserved area, both FATs and the root directory.
        let metadata = reserved
            + fat_count * fat_sectors
            + root_sectors
            + if fat32 { sectors_per_cluster } else { 0 };
        for sector in 0..metadata {
            write(sector, &zeros)?;
        }
        write(0, &boot)?;
        if fat32 {
            let mut info = [0u8; SECTOR];
            put32(&mut info, 0, 0x4161_5252);
            put32(&mut info, 484, 0x6141_7272);
            put32(&mut info, 488, 0xffff_ffff);
            put32(&mut info, 492, 0xffff_ffff);
            put32(&mut info, 508, 0xaa55_0000);
            write(1, &info)?;
            write(6, &boot)?;
        }
        // FAT entries 0 and 1 (media descriptor / end marker) and the root
        // directory cluster of FAT32.
        let mut first = [0u8; SECTOR];
        if fat32 {
            put32(&mut first, 0, 0x0fff_fff8);
            put32(&mut first, 4, 0x0fff_ffff);
            put32(&mut first, 8, 0x0fff_ffff); // root directory cluster
        } else {
            put16(&mut first, 0, 0xfff8);
            put16(&mut first, 2, 0xffff);
        }
        for copy in 0..fat_count {
            write(reserved + copy * fat_sectors, &first)?;
        }
        Ok(())
    }
}

/// A boxed-by-static value: filesystem state is large (the sector cache), so
/// it lives in a caller-provided static slot instead of on the stack.
pub struct Box64(&'static mut Fs);

static mut SLOTS: [Option<Fs>; 4] = [None, None, None, None];
static SLOT_USED: [core::sync::atomic::AtomicBool; 4] =
    [const { core::sync::atomic::AtomicBool::new(false) }; 4];

impl Box64 {
    fn new(fs: Fs) -> Self {
        for (index, used) in SLOT_USED.iter().enumerate() {
            if !used.swap(true, core::sync::atomic::Ordering::AcqRel) {
                // SAFETY: the slot was free, so no other reference exists.
                let slot = unsafe { &mut *core::ptr::addr_of_mut!(SLOTS[index]) };
                *slot = Some(fs);
                let reference = slot.as_mut().unwrap();
                // SAFETY: slots are static and never move.
                return Self(unsafe { &mut *(reference as *mut Fs) });
            }
        }
        panic!("no filesystem slot free");
    }

    pub fn get(&mut self) -> &mut Fs {
        self.0
    }
}

impl Drop for Box64 {
    fn drop(&mut self) {
        let pointer = self.0 as *mut Fs as usize;
        for index in 0..4 {
            // SAFETY: comparing addresses only.
            let slot = unsafe { &mut *core::ptr::addr_of_mut!(SLOTS[index]) };
            if let Some(fs) = slot.as_mut()
                && fs as *mut Fs as usize == pointer
            {
                *slot = None;
                SLOT_USED[index].store(false, core::sync::atomic::Ordering::Release);
            }
        }
    }
}

impl core::ops::Deref for Box64 {
    type Target = Fs;
    fn deref(&self) -> &Fs {
        self.0
    }
}

impl core::ops::DerefMut for Box64 {
    fn deref_mut(&mut self) -> &mut Fs {
        self.0
    }
}

#[cfg(feature = "boot-test")]
/// Result of the in-kernel filesystem self-test.
pub struct SelfTest {
    pub formatted: bool,
    pub fat32: bool,
    pub directories: bool,
    pub long_names: bool,
    pub big_file: bool,
    pub rename_move: bool,
    pub truncate: bool,
    pub delete_frees: bool,
    pub remount: bool,
    pub verified: bool,
}

#[cfg(feature = "boot-test")]
pub(crate) fn pattern(seed: u32, index: usize) -> u8 {
    (index as u32)
        .wrapping_mul(2_654_435_761)
        .wrapping_add(seed)
        .to_le_bytes()[2]
}

#[cfg(feature = "boot-test")]
fn write_pattern(fs: &mut Fs, node: &mut Node, seed: u32, length: usize) -> bool {
    let mut chunk = [0u8; 1500];
    let mut done = 0;
    while done < length {
        let count = (length - done).min(chunk.len());
        for (offset, byte) in chunk[..count].iter_mut().enumerate() {
            *byte = pattern(seed, done + offset);
        }
        if fs.write_at(node, done as u64, &chunk[..count]) != Ok(count) {
            return false;
        }
        done += count;
    }
    true
}

#[cfg(feature = "boot-test")]
fn check_pattern(fs: &mut Fs, node: &mut Node, seed: u32, length: usize) -> bool {
    if node.size as usize != length {
        return false;
    }
    let mut chunk = [0u8; 1024];
    let mut done = 0;
    while done < length {
        let count = (length - done).min(chunk.len());
        if fs.read_at(node, done as u64, &mut chunk[..count]) != Ok(count) {
            return false;
        }
        if chunk[..count]
            .iter()
            .enumerate()
            .any(|(offset, byte)| *byte != pattern(seed, done + offset))
        {
            return false;
        }
        done += count;
    }
    true
}

#[cfg(feature = "boot-test")]
/// Formats `sectors` sectors at `start` and exercises the whole filesystem:
/// nested directories, long names, multi-cluster files, rename/move, truncate,
/// delete (space must come back), and a fresh remount that reads it all back.
pub fn self_test(disk: Disk, start: u64, sectors: u64) -> SelfTest {
    let mut report = SelfTest {
        formatted: false,
        fat32: false,
        directories: false,
        long_names: false,
        big_file: false,
        rename_move: false,
        truncate: false,
        delete_frees: false,
        remount: false,
        verified: false,
    };
    if Fs::format(disk, start, sectors, b"AEROSTEST").is_err() {
        return report;
    }
    let Ok(mut fs) = Fs::mount(disk, start) else {
        return report;
    };
    report.formatted = true;
    report.fat32 = fs.fat_type == FatType::Fat32;
    let Ok(before) = fs.info() else {
        return report;
    };
    let root = fs.root();

    // Directories, including a nested one.
    let documents = fs.create_dir(root.as_dir(), b"Documents");
    let Ok(documents) = documents else {
        return report;
    };
    let nested = fs.create_dir(documents.as_dir(), b"Project Files 2026");
    let Ok(nested) = nested else {
        return report;
    };
    report.directories = fs.create_dir(root.as_dir(), b"Documents") == Err(FsError::Exists)
        && fs.resolve(b"/Documents/Project Files 2026").is_ok()
        && fs.resolve(b"/documents/PROJECT FILES 2026").is_ok();

    // Long names: created, found case-insensitively, listed with their spelling.
    let long_name: &[u8] = b"A rather long file name, with spaces & symbols (v2).txt";
    let Ok(mut long_file) = fs.create_file(nested.as_dir(), long_name) else {
        return report;
    };
    let wrote = write_pattern(&mut fs, &mut long_file, 7, 3000);
    let mut listed_name = false;
    let mut cursor = 0;
    while let Ok(Some(entry)) = fs.list_next(nested.as_dir(), &mut cursor) {
        if entry.name() == long_name {
            listed_name = true;
        }
    }
    report.long_names = wrote
        && listed_name
        && fs
            .resolve(b"/Documents/Project Files 2026/a RATHER long FILE name, with spaces & symbols (v2).txt")
            .is_ok()
        && fs.create_file(nested.as_dir(), b"A short.txt").is_ok()
        && fs.create_file(nested.as_dir(), b"A shor~1.txt").is_ok()
        && fs.create_file(nested.as_dir(), b"caf\xc3\xa9.txt").is_ok();

    // A big multi-cluster file, read back in odd-sized pieces.
    let Ok(mut big) = fs.create_file(root.as_dir(), b"big.bin") else {
        return report;
    };
    let big_length = 300_000;
    report.big_file = write_pattern(&mut fs, &mut big, 99, big_length)
        && check_pattern(&mut fs, &mut big, 99, big_length);

    // Rename in place, move across directories, replace.
    let moved = fs.rename(&big, documents.as_dir(), b"Moved Big File.bin", false);
    let Ok(mut moved) = moved else {
        return report;
    };
    let case_only = fs.rename(&moved, documents.as_dir(), b"moved big file.BIN", false);
    report.rename_move = case_only.is_ok()
        && fs.resolve(b"/big.bin") == Err(FsError::NotFound)
        && fs.resolve(b"/Documents/MOVED BIG FILE.bin").is_ok();
    if let Ok(renamed) = case_only {
        moved = renamed;
    }
    report.rename_move &= check_pattern(&mut fs, &mut moved, 99, big_length);

    // Truncate: shrink then grow (the new tail reads as zeros).
    let shrink =
        fs.truncate(&mut moved, 1000).is_ok() && check_pattern(&mut fs, &mut moved, 99, 1000);
    let grow = fs.truncate(&mut moved, 5000).is_ok() && moved.size == 5000;
    let mut tail = [0xffu8; 100];
    let zeros = fs.read_at(&mut moved, 4000, &mut tail) == Ok(100) && tail.iter().all(|b| *b == 0);
    report.truncate = shrink && grow && zeros;

    // Deleting gives every cluster back (only the directories remain).
    let remove_moved = fs.remove(&moved).is_ok();
    let Ok(long_node) = fs.resolve(
        b"/Documents/Project Files 2026/a RATHER long FILE name, with spaces & symbols (v2).txt",
    ) else {
        return report;
    };
    let mut cleanup = remove_moved && fs.remove(&long_node).is_ok();
    for name in [
        &b"A short.txt"[..],
        &b"A shor~1.txt"[..],
        "caf\u{e9}.txt".as_bytes(),
    ] {
        match fs.find(nested.as_dir(), name) {
            Ok(node) => cleanup &= fs.remove(&node).is_ok(),
            Err(_) => cleanup = false,
        }
    }
    let not_empty = fs.remove(&documents) == Err(FsError::NotEmpty);
    cleanup &= not_empty && fs.remove(&nested).is_ok() && fs.remove(&documents).is_ok();
    let after = fs.info();
    report.delete_frees = cleanup
        && after.is_ok_and(|info| info.free_clusters == before.free_clusters)
        && fs.resolve(b"/Documents") == Err(FsError::NotFound);

    // Persistence: build a small tree, drop the mount, mount again from the
    // disk alone and read everything back.
    let mut ok = true;
    let Ok(keep_dir) = fs.create_dir(root.as_dir(), b"Persisted Folder") else {
        return report;
    };
    let Ok(mut keep) = fs.create_file(keep_dir.as_dir(), b"remember me please.dat") else {
        return report;
    };
    ok &= write_pattern(&mut fs, &mut keep, 5, 70_000);
    drop(fs);
    let Ok(mut fresh) = Fs::mount(disk, start) else {
        return report;
    };
    ok &= match fresh.resolve(b"/persisted folder/REMEMBER ME PLEASE.dat") {
        Ok(mut node) => check_pattern(&mut fresh, &mut node, 5, 70_000),
        Err(_) => false,
    };
    ok &= fresh.resolve(b"/Documents") == Err(FsError::NotFound);
    report.remount = ok;
    // Leave an MBR pointing at the volume, so the kernel's media scan can
    // find and mount it (the boot test then reads the data back through /media).
    let mut mbr = [0u8; SECTOR];
    mbr[446 + 4] = 0x06;
    mbr[446..446 + 4].copy_from_slice(&[0x00, 0xfe, 0xff, 0xff]);
    mbr[446 + 8..446 + 12].copy_from_slice(&(start as u32).to_le_bytes());
    mbr[446 + 12..446 + 16].copy_from_slice(&(sectors as u32).to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xaa;
    let mbr_written = start > 0 && disk.write_run(0, 1, &mbr);
    report.remount &= mbr_written;
    report.verified = report.formatted
        && report.directories
        && report.long_names
        && report.big_file
        && report.rename_move
        && report.truncate
        && report.delete_frees
        && report.remount;
    report
}

#[cfg(feature = "boot-test")]
impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.dir == other.dir
            && self.index == other.index
            && self.first_cluster == other.first_cluster
    }
}
