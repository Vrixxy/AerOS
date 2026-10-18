//! Software volumes: several disks presented to the block layer as one
//! (`Disk::Volume`). Linear joins them end to end, stripe interleaves chunks
//! for speed, mirror keeps a copy on every member and carries on with one
//! member gone. Each member keeps a header in its last sector (what volume it
//! belongs to, its place in it, which members have failed), so a volume is
//! put back together at boot by `assemble` and a member that missed writes
//! while it was away is noticed and left out.

use crate::block::{self, Disk, SECTOR};
use crate::sync::TicketLock;

pub const MAX_VOLUMES: usize = 2;
pub const MAX_MEMBERS: usize = 4;
/// Largest transfer one call carries (the block layer's run length).
const RUN: usize = block::MAX_RUN;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Linear,
    Stripe,
    Mirror,
}

impl Level {
    pub fn name(self) -> &'static str {
        match self {
            Level::Linear => "linear",
            Level::Stripe => "stripe",
            Level::Mirror => "mirror",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "linear" => Some(Level::Linear),
            "stripe" | "raid0" => Some(Level::Stripe),
            "mirror" | "raid1" => Some(Level::Mirror),
            _ => None,
        }
    }
}

/// What a volume is built from.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Member {
    Disk(Disk),
    /// An in-memory disk of the boot test.
    #[cfg(feature = "boot-test")]
    Memory(u8),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    /// The level needs a different number of members.
    Members,
    /// A member is missing, empty or too small.
    Disk,
    /// A member already belongs to a volume, is the boot disk or is mounted.
    Busy,
    /// No free volume slot, or no such volume.
    Slot,
}

#[derive(Clone, Copy)]
struct Volume {
    level: Level,
    chunk: u64,
    members: [Option<Member>; MAX_MEMBERS],
    failed: [bool; MAX_MEMBERS],
    count: usize,
    /// Sectors each member contributes (stripe and mirror; for a stripe a
    /// whole number of chunks).
    per_member: u64,
    sectors: u64,
    uuid: [u8; 16],
    /// Counts membership changes; a member with a lower count is stale.
    events: u64,
}

static VOLUMES: TicketLock<[Option<Volume>; MAX_VOLUMES]> = TicketLock::new([None; MAX_VOLUMES]);

#[cfg(feature = "boot-test")]
const MEMORY_SECTORS: usize = 256;
/// What a memory member offers as data (the last sector is the header).
#[cfg(feature = "boot-test")]
const USABLE: u64 = MEMORY_SECTORS as u64 - 1;
#[cfg(feature = "boot-test")]
static MEMORY: TicketLock<[[u8; MEMORY_SECTORS * SECTOR]; 4]> =
    TicketLock::new([[0; MEMORY_SECTORS * SECTOR]; 4]);
#[cfg(feature = "boot-test")]
static MEMORY_BROKEN: [core::sync::atomic::AtomicBool; 4] =
    [const { core::sync::atomic::AtomicBool::new(false) }; 4];

const MAGIC: &[u8; 8] = b"AEROSRD1";

/// Sectors of a member that carry data: the last one holds the header.
fn usable(member: Member) -> u64 {
    member_sectors(member).saturating_sub(1)
}

struct Header {
    uuid: [u8; 16],
    level: Level,
    count: usize,
    slot: usize,
    chunk: u64,
    events: u64,
    failed: u8,
}

fn encode_header(volume: &Volume, slot: usize) -> [u8; SECTOR] {
    let mut sector = [0u8; SECTOR];
    sector[..8].copy_from_slice(MAGIC);
    sector[8..24].copy_from_slice(&volume.uuid);
    sector[24] = match volume.level {
        Level::Linear => 0,
        Level::Stripe => 1,
        Level::Mirror => 2,
    };
    sector[25] = volume.count as u8;
    sector[26] = slot as u8;
    sector[28..32].copy_from_slice(&(volume.chunk as u32).to_le_bytes());
    sector[32..40].copy_from_slice(&volume.events.to_le_bytes());
    let failed = volume.failed[..volume.count]
        .iter()
        .enumerate()
        .fold(0u8, |bits, (index, failed)| {
            bits | (u8::from(*failed) << index)
        });
    sector[48] = failed;
    let check = crate::aerfs::crc32(&sector[..56]);
    sector[56..60].copy_from_slice(&check.to_le_bytes());
    sector
}

fn decode_header(sector: &[u8; SECTOR]) -> Option<Header> {
    if &sector[..8] != MAGIC {
        return None;
    }
    let check = u32::from_le_bytes([sector[56], sector[57], sector[58], sector[59]]);
    if check != crate::aerfs::crc32(&sector[..56]) {
        return None;
    }
    let level = match sector[24] {
        0 => Level::Linear,
        1 => Level::Stripe,
        2 => Level::Mirror,
        _ => return None,
    };
    let count = sector[25] as usize;
    let slot = sector[26] as usize;
    if count == 0 || count > MAX_MEMBERS || slot >= count {
        return None;
    }
    let mut uuid = [0u8; 16];
    uuid.copy_from_slice(&sector[8..24]);
    Some(Header {
        uuid,
        level,
        count,
        slot,
        chunk: u32::from_le_bytes([sector[28], sector[29], sector[30], sector[31]]) as u64,
        events: u64::from_le_bytes([
            sector[32], sector[33], sector[34], sector[35], sector[36], sector[37], sector[38],
            sector[39],
        ]),
        failed: sector[48],
    })
}

fn read_header(member: Member) -> Option<Header> {
    let last = member_sectors(member).checked_sub(1)?;
    let mut sector = [0u8; SECTOR];
    if !member_read(member, last, 1, &mut sector) {
        return None;
    }
    decode_header(&sector)
}

/// Writes the header to every member still in service.
fn persist(index: usize) {
    let Some(volume) = VOLUMES.lock().get(index).copied().flatten() else {
        return;
    };
    for slot in 0..volume.count {
        if volume.failed[slot] {
            continue;
        }
        let Some(member) = volume.members[slot] else {
            continue;
        };
        let Some(last) = member_sectors(member).checked_sub(1) else {
            continue;
        };
        let header = encode_header(&volume, slot);
        let _ = member_write(member, last, 1, &header);
    }
}

fn member_sectors(member: Member) -> u64 {
    match member {
        Member::Disk(disk) => disk.sectors(),
        #[cfg(feature = "boot-test")]
        Member::Memory(_) => MEMORY_SECTORS as u64,
    }
}

fn member_read(member: Member, lba: u64, count: usize, buffer: &mut [u8]) -> bool {
    match member {
        Member::Disk(disk) => block::read(disk, lba, count, buffer),
        #[cfg(feature = "boot-test")]
        Member::Memory(index) => {
            use core::sync::atomic::Ordering;
            let slot = index as usize & 3;
            if MEMORY_BROKEN[slot].load(Ordering::Relaxed)
                || lba + count as u64 > MEMORY_SECTORS as u64
            {
                return false;
            }
            let start = lba as usize * SECTOR;
            buffer[..count * SECTOR]
                .copy_from_slice(&MEMORY.lock()[slot][start..start + count * SECTOR]);
            true
        }
    }
}

fn member_write(member: Member, lba: u64, count: usize, data: &[u8]) -> bool {
    match member {
        Member::Disk(disk) => block::write(disk, lba, count, data),
        #[cfg(feature = "boot-test")]
        Member::Memory(index) => {
            use core::sync::atomic::Ordering;
            let slot = index as usize & 3;
            if MEMORY_BROKEN[slot].load(Ordering::Relaxed)
                || lba + count as u64 > MEMORY_SECTORS as u64
            {
                return false;
            }
            let start = lba as usize * SECTOR;
            MEMORY.lock()[slot][start..start + count * SECTOR]
                .copy_from_slice(&data[..count * SECTOR]);
            true
        }
    }
}

/// Builds a volume. `chunk` is the stripe unit in sectors (a power of two
/// that divides eight or is a multiple of it; ignored for the other levels).
pub fn create(level: Level, chunk: u64, members: &[Member]) -> Result<u8, Error> {
    let count = members.len();
    let wrong = match level {
        Level::Linear => count == 0 || count > MAX_MEMBERS,
        Level::Stripe | Level::Mirror => !(2..=MAX_MEMBERS).contains(&count),
    };
    if wrong {
        return Err(Error::Members);
    }
    let chunk = if level == Level::Stripe {
        if chunk == 0 || !chunk.is_power_of_two() || chunk > 256 {
            return Err(Error::Members);
        }
        chunk
    } else {
        RUN as u64
    };
    let mut guard = VOLUMES.lock();
    let slot = guard.iter().position(Option::is_none).ok_or(Error::Slot)?;
    let mut volume = Volume {
        level,
        chunk,
        members: [None; MAX_MEMBERS],
        failed: [false; MAX_MEMBERS],
        count,
        per_member: 0,
        sectors: 0,
        uuid: [0; 16],
        events: 1,
    };
    let mut smallest = u64::MAX;
    let mut total = 0u64;
    for (index, member) in members.iter().enumerate() {
        let sectors = usable(*member);
        if sectors == 0 {
            return Err(Error::Disk);
        }
        if members[..index].contains(member) {
            return Err(Error::Busy);
        }
        if guard
            .iter()
            .flatten()
            .any(|other| other.members.iter().flatten().any(|used| used == member))
        {
            return Err(Error::Busy);
        }
        if let Member::Disk(disk) = member
            && disk_in_use(*disk)
        {
            return Err(Error::Busy);
        }
        smallest = smallest.min(sectors);
        total += sectors;
        volume.members[index] = Some(*member);
    }
    match level {
        Level::Linear => volume.sectors = total,
        Level::Stripe => {
            volume.per_member = smallest / chunk * chunk;
            if volume.per_member == 0 {
                return Err(Error::Disk);
            }
            volume.sectors = volume.per_member * count as u64;
        }
        Level::Mirror => {
            volume.per_member = smallest;
            volume.sectors = smallest;
        }
    }
    crate::random::fill(&mut volume.uuid);
    guard[slot] = Some(volume);
    drop(guard);
    persist(slot);
    Ok(slot as u8)
}

/// Puts back together the volume described by the headers on `members`
/// (given in any order). Members that are missing or out of date are marked
/// failed, which only a mirror can run with.
pub fn assemble_from(members: &[Member]) -> Result<u8, Error> {
    let mut found: [Option<(Member, Header)>; MAX_MEMBERS] = [const { None }; MAX_MEMBERS];
    let mut first: Option<Header> = None;
    for member in members {
        let Some(header) = read_header(*member) else {
            continue;
        };
        match &first {
            None => first = Some(Header { ..header }),
            Some(other) if other.uuid != header.uuid => continue,
            Some(_) => {}
        }
        let slot = header.slot;
        found[slot] = Some((*member, header));
    }
    let Some(template) = first else {
        return Err(Error::Disk);
    };
    let count = template.count;
    let newest = found
        .iter()
        .flatten()
        .map(|(_, header)| header.events)
        .max()
        .unwrap_or(0);
    let mut volume = Volume {
        level: template.level,
        chunk: template.chunk.max(1),
        members: [None; MAX_MEMBERS],
        failed: [false; MAX_MEMBERS],
        count,
        per_member: 0,
        sectors: 0,
        uuid: template.uuid,
        events: newest,
    };
    let newest_failed = found
        .iter()
        .flatten()
        .find(|(_, header)| header.events == newest)
        .map_or(0, |(_, header)| header.failed);
    let mut smallest = u64::MAX;
    let mut total = 0u64;
    let mut present = 0usize;
    for (slot, entry) in found.iter().enumerate().take(count) {
        match entry {
            Some((member, header)) => {
                volume.members[slot] = Some(*member);
                volume.failed[slot] = header.events < newest || newest_failed & (1 << slot) != 0;
                if !volume.failed[slot] {
                    present += 1;
                }
                let sectors = usable(*member);
                smallest = smallest.min(sectors);
                total += sectors;
            }
            None => volume.failed[slot] = true,
        }
    }
    if volume.level != Level::Mirror && present != count {
        return Err(Error::Disk);
    }
    if present == 0 {
        return Err(Error::Disk);
    }
    match volume.level {
        Level::Linear => volume.sectors = total,
        Level::Stripe => {
            volume.per_member = smallest / volume.chunk * volume.chunk;
            volume.sectors = volume.per_member * count as u64;
        }
        Level::Mirror => {
            volume.per_member = smallest;
            volume.sectors = smallest;
        }
    }
    let mut guard = VOLUMES.lock();
    if guard
        .iter()
        .flatten()
        .any(|other| other.uuid == volume.uuid)
    {
        return Err(Error::Busy);
    }
    let slot = guard.iter().position(Option::is_none).ok_or(Error::Slot)?;
    guard[slot] = Some(volume);
    Ok(slot as u8)
}

/// Looks at the last sector of every disk that is free to use and assembles
/// the volumes found there; returns how many.
pub fn assemble() -> usize {
    let mut candidates = [Disk::Nvme; 8];
    let mut count = 0usize;
    let mut add = |disk: Disk| {
        if count < candidates.len()
            && disk.sectors() > 1
            && !disk_in_use(disk)
            && !candidates[..count].contains(&disk)
        {
            candidates[count] = disk;
            count += 1;
        }
    };
    for index in 0..crate::ahci::disk_count() {
        add(Disk::Ahci(index));
    }
    for disk in [Disk::Nvme, Disk::Usb, Disk::Sd, Disk::Virtio] {
        add(disk);
    }
    let mut taken = [false; 8];
    let mut assembled = 0;
    for first in 0..count {
        if taken[first] {
            continue;
        }
        let Some(header) = read_header(Member::Disk(candidates[first])) else {
            continue;
        };
        let mut group = [Member::Disk(Disk::Nvme); MAX_MEMBERS];
        let mut size = 0;
        for other in first..count {
            if taken[other] || size == MAX_MEMBERS {
                continue;
            }
            if read_header(Member::Disk(candidates[other])).is_some_and(|h| h.uuid == header.uuid) {
                taken[other] = true;
                group[size] = Member::Disk(candidates[other]);
                size += 1;
            }
        }
        if assemble_from(&group[..size]).is_ok() {
            assembled += 1;
        }
    }
    assembled
}

fn disk_in_use(disk: Disk) -> bool {
    crate::blockdev::boot_disk() == Some(disk) || crate::datafs::disk_mounted(disk)
}

/// Removes a volume (its members are untouched and free again).
pub fn stop(index: usize) -> bool {
    let mut guard = VOLUMES.lock();
    match guard.get_mut(index) {
        Some(slot @ Some(_)) => {
            *slot = None;
            block::invalidate_disk(Disk::Volume(index as u8));
            true
        }
        _ => false,
    }
}

pub fn sectors(index: usize) -> u64 {
    VOLUMES
        .lock()
        .get(index)
        .copied()
        .flatten()
        .map_or(0, |volume| volume.sectors)
}

#[derive(Clone, Copy)]
pub struct Info {
    pub level: Level,
    pub members: usize,
    pub failed: usize,
    pub sectors: u64,
    pub chunk: u64,
}

pub fn info(index: usize) -> Option<Info> {
    let volume = VOLUMES.lock().get(index).copied().flatten()?;
    Some(Info {
        level: volume.level,
        members: volume.count,
        failed: volume.failed[..volume.count].iter().filter(|f| **f).count(),
        sectors: volume.sectors,
        chunk: volume.chunk,
    })
}

/// The disk a member slot is, for listings.
pub fn member_disk(index: usize, slot: usize) -> Option<Disk> {
    let volume = VOLUMES.lock().get(index).copied().flatten()?;
    match volume.members.get(slot).copied().flatten()? {
        Member::Disk(disk) => Some(disk),
        #[cfg(feature = "boot-test")]
        Member::Memory(_) => None,
    }
}

pub fn member_failed(index: usize, slot: usize) -> bool {
    VOLUMES
        .lock()
        .get(index)
        .copied()
        .flatten()
        .is_some_and(|volume| volume.failed.get(slot).copied().unwrap_or(false))
}

/// Marks a member as failed by hand (administration and tests). A stripe or
/// linear volume has nothing to fall back on, so only a mirror accepts it.
pub fn fail(index: usize, slot: usize) -> bool {
    let mut guard = VOLUMES.lock();
    let Some(Some(volume)) = guard.get_mut(index) else {
        return false;
    };
    if volume.level != Level::Mirror || slot >= volume.count {
        return false;
    }
    let healthy = volume.failed[..volume.count]
        .iter()
        .filter(|f| !**f)
        .count();
    if volume.failed[slot] || healthy < 2 {
        return false;
    }
    volume.failed[slot] = true;
    volume.events += 1;
    drop(guard);
    persist(index);
    true
}

/// Reads `count` sectors from a volume.
pub fn read(index: usize, lba: u64, count: usize, buffer: &mut [u8]) -> bool {
    transfer(index, lba, count, Direction::Read(buffer))
}

pub fn write(index: usize, lba: u64, count: usize, data: &[u8]) -> bool {
    transfer(index, lba, count, Direction::Write(data))
}

enum Direction<'a> {
    Read(&'a mut [u8]),
    Write(&'a [u8]),
}

fn transfer(index: usize, lba: u64, count: usize, mut direction: Direction<'_>) -> bool {
    let Some(volume) = VOLUMES.lock().get(index).copied().flatten() else {
        return false;
    };
    let length = count * SECTOR;
    let available = match &direction {
        Direction::Read(buffer) => buffer.len(),
        Direction::Write(data) => data.len(),
    };
    if count == 0
        || available < length
        || lba
            .checked_add(count as u64)
            .is_none_or(|end| end > volume.sectors)
    {
        return false;
    }
    match volume.level {
        Level::Mirror => mirror_transfer(index, &volume, lba, count, direction),
        Level::Linear | Level::Stripe => {
            let mut done = 0usize;
            while done < count {
                let here = lba + done as u64;
                let (slot, member_lba, run) = locate(&volume, here, count - done);
                let Some(member) = volume.members[slot] else {
                    return false;
                };
                let span = done * SECTOR..(done + run) * SECTOR;
                let ok = match &mut direction {
                    Direction::Read(buffer) => {
                        member_read(member, member_lba, run, &mut buffer[span])
                    }
                    Direction::Write(data) => member_write(member, member_lba, run, &data[span]),
                };
                if !ok {
                    return false;
                }
                done += run;
            }
            true
        }
    }
}

/// Which member holds volume sector `lba`, where, and for how many of the
/// next `want` sectors it stays contiguous there.
fn locate(volume: &Volume, lba: u64, want: usize) -> (usize, u64, usize) {
    match volume.level {
        Level::Stripe => {
            let chunk_number = lba / volume.chunk;
            let within = lba % volume.chunk;
            let slot = (chunk_number % volume.count as u64) as usize;
            let row = chunk_number / volume.count as u64;
            let run = ((volume.chunk - within) as usize).min(want);
            (slot, row * volume.chunk + within, run)
        }
        _ => {
            let mut start = 0u64;
            for slot in 0..volume.count {
                let length = volume.members[slot].map_or(0, usable);
                if lba < start + length {
                    let run = ((start + length - lba) as usize).min(want);
                    return (slot, lba - start, run);
                }
                start += length;
            }
            (0, 0, 0)
        }
    }
}

fn mirror_transfer(
    index: usize,
    volume: &Volume,
    lba: u64,
    count: usize,
    direction: Direction<'_>,
) -> bool {
    match direction {
        Direction::Read(buffer) => {
            for slot in 0..volume.count {
                if member_failed(index, slot) {
                    continue;
                }
                let Some(member) = volume.members[slot] else {
                    continue;
                };
                if member_read(member, lba, count, buffer) {
                    return true;
                }
                mark_failed(index, slot);
            }
            false
        }
        Direction::Write(data) => {
            let mut wrote = false;
            for slot in 0..volume.count {
                if member_failed(index, slot) {
                    continue;
                }
                let Some(member) = volume.members[slot] else {
                    continue;
                };
                if member_write(member, lba, count, data) {
                    wrote = true;
                } else {
                    mark_failed(index, slot);
                }
            }
            wrote
        }
    }
}

fn mark_failed(index: usize, slot: usize) {
    let mut guard = VOLUMES.lock();
    if let Some(Some(volume)) = guard.get_mut(index) {
        // The last working copy stays in service whatever happens to it.
        let healthy = volume.failed[..volume.count]
            .iter()
            .filter(|f| !**f)
            .count();
        if healthy > 1 {
            volume.failed[slot] = true;
            volume.events += 1;
            drop(guard);
            persist(index);
        }
    }
}

/// Puts `member` in the place of a failed one and copies the whole volume
/// onto it from a healthy copy; the slot is in service again afterwards.
pub fn replace(index: usize, slot: usize, member: Member) -> Result<(), Error> {
    let volume = VOLUMES
        .lock()
        .get(index)
        .copied()
        .flatten()
        .ok_or(Error::Slot)?;
    if volume.level != Level::Mirror || slot >= volume.count {
        return Err(Error::Slot);
    }
    if usable(member) < volume.sectors {
        return Err(Error::Disk);
    }
    if let Member::Disk(disk) = member
        && disk_in_use(disk)
    {
        return Err(Error::Busy);
    }
    let source = (0..volume.count)
        .find(|candidate| *candidate != slot && !member_failed(index, *candidate))
        .and_then(|candidate| volume.members[candidate])
        .ok_or(Error::Disk)?;
    let mut buffer = [0u8; RUN * SECTOR];
    let mut lba = 0u64;
    while lba < volume.sectors {
        let run = ((volume.sectors - lba) as usize).min(RUN);
        if !member_read(source, lba, run, &mut buffer) || !member_write(member, lba, run, &buffer) {
            return Err(Error::Disk);
        }
        lba += run as u64;
    }
    {
        let mut guard = VOLUMES.lock();
        if let Some(Some(volume)) = guard.get_mut(index) {
            volume.members[slot] = Some(member);
            volume.failed[slot] = false;
            volume.events += 1;
        }
    }
    persist(index);
    Ok(())
}

#[cfg(feature = "boot-test")]
#[derive(Clone, Copy)]
pub struct Report {
    pub linear: bool,
    pub stripe: bool,
    pub mirror: bool,
    pub degraded: bool,
    pub resync: bool,
    pub block_layer: bool,
    pub refused: bool,
    pub headers: bool,
    pub verified: bool,
}

#[cfg(feature = "boot-test")]
fn pattern(lba: u64, round: u8) -> u8 {
    (lba as u8)
        .wrapping_mul(31)
        .wrapping_add(round)
        .wrapping_add(7)
}

#[cfg(feature = "boot-test")]
fn fill(data: &mut [u8], first: u64, round: u8) {
    for (offset, sector) in data.chunks_mut(SECTOR).enumerate() {
        sector.fill(pattern(first + offset as u64, round));
    }
}

#[cfg(feature = "boot-test")]
fn matches(data: &[u8], first: u64, round: u8) -> bool {
    data.chunks(SECTOR).enumerate().all(|(offset, sector)| {
        sector
            .iter()
            .all(|byte| *byte == pattern(first + offset as u64, round))
    })
}

#[cfg(feature = "boot-test")]
fn member_holds(member: Member, lba: u64, expect: u8) -> bool {
    let mut sector = [0u8; SECTOR];
    member_read(member, lba, 1, &mut sector) && sector.iter().all(|byte| *byte == expect)
}

#[cfg(feature = "boot-test")]
pub fn self_test() -> Report {
    use core::sync::atomic::Ordering;
    for broken in &MEMORY_BROKEN {
        broken.store(false, Ordering::Relaxed);
    }
    for memory in MEMORY.lock().iter_mut() {
        memory.fill(0);
    }
    let mem = |index: u8| Member::Memory(index);
    let mut buffer = [0u8; 40 * SECTOR];
    let mut check = [0u8; 40 * SECTOR];

    // Linear: two members end to end; a write across the join lands in both.
    let linear = match create(Level::Linear, 0, &[mem(0), mem(1)]) {
        Ok(volume) => {
            let volume = volume as usize;
            let first = USABLE - 5;
            fill(&mut buffer[..16 * SECTOR], first, 1);
            let ok = sectors(volume) == 2 * USABLE
                && write(volume, first, 16, &buffer[..16 * SECTOR])
                && read(volume, first, 16, &mut check[..16 * SECTOR])
                && matches(&check[..16 * SECTOR], first, 1)
                && member_holds(mem(0), USABLE - 1, pattern(first + 4, 1))
                && member_holds(mem(1), 0, pattern(first + 5, 1))
                && !write(volume, 2 * USABLE - 1, 2, &buffer[..2 * SECTOR]);
            stop(volume);
            ok
        }
        Err(_) => false,
    };

    // Stripe: three members, four-sector chunks; chunk n lands on member
    // n % 3 at row n / 3.
    let stripe = match create(Level::Stripe, 4, &[mem(0), mem(1), mem(2)]) {
        Ok(volume) => {
            let volume = volume as usize;
            fill(&mut buffer, 0, 2);
            let layout = write(volume, 0, 8, &buffer[..8 * SECTOR])
                && write(volume, 8, 8, &buffer[8 * SECTOR..16 * SECTOR])
                && write(volume, 16, 8, &buffer[16 * SECTOR..24 * SECTOR])
                && write(volume, 24, 8, &buffer[24 * SECTOR..32 * SECTOR])
                && write(volume, 32, 8, &buffer[32 * SECTOR..40 * SECTOR]);
            // Chunk 0 -> member 0 row 0, chunk 1 -> member 1 row 0, chunk 2 ->
            // member 2 row 0, chunk 3 -> member 0 row 1 (sector 4 of it).
            let mapped = member_holds(mem(0), 0, pattern(0, 2))
                && member_holds(mem(1), 0, pattern(4, 2))
                && member_holds(mem(2), 3, pattern(11, 2))
                && member_holds(mem(0), 4, pattern(12, 2));
            // Unaligned read spanning three chunks.
            let spanning = read(volume, 3, 11, &mut check[..11 * SECTOR])
                && matches(&check[..11 * SECTOR], 3, 2);
            let whole = read(volume, 0, 40, &mut check) && matches(&check, 0, 2);
            let size = sectors(volume) == 3 * (USABLE / 4 * 4);
            stop(volume);
            layout && mapped && spanning && whole && size
        }
        Err(_) => false,
    };

    // Mirror: both members hold everything; one failing leaves the volume
    // readable and writable (degraded); replacing it resynchronises it.
    let mut mirror = false;
    let mut degraded = false;
    let mut resync = false;
    if let Ok(volume) = create(Level::Mirror, 0, &[mem(0), mem(1)]) {
        let volume = volume as usize;
        fill(&mut buffer, 0, 3);
        mirror = sectors(volume) == USABLE
            && write(volume, 0, 8, &buffer[..8 * SECTOR])
            && write(volume, 8, 8, &buffer[8 * SECTOR..16 * SECTOR])
            && member_holds(mem(0), 9, pattern(9, 3))
            && member_holds(mem(1), 9, pattern(9, 3))
            && read(volume, 0, 16, &mut check[..16 * SECTOR])
            && matches(&check[..16 * SECTOR], 0, 3);

        // Member 0 dies: reads come from member 1, writes go to member 1 only.
        MEMORY_BROKEN[0].store(true, Ordering::Relaxed);
        let still_reads =
            read(volume, 0, 16, &mut check[..16 * SECTOR]) && matches(&check[..16 * SECTOR], 0, 3);
        fill(&mut buffer, 20, 4);
        let still_writes = write(volume, 20, 8, &buffer[..8 * SECTOR]);
        let marked = info(volume).is_some_and(|info| info.failed == 1);
        let cannot_lose_last = !fail(volume, 1);
        degraded = still_reads && still_writes && marked && cannot_lose_last;

        // A fresh disk takes slot 0 and receives everything.
        MEMORY_BROKEN[0].store(false, Ordering::Relaxed);
        MEMORY.lock()[2].fill(0);
        let replaced = replace(volume, 0, mem(2)).is_ok();
        let copied = member_holds(mem(2), 9, pattern(9, 3))
            && member_holds(mem(2), 23, pattern(23, 4))
            && info(volume).is_some_and(|info| info.failed == 0);
        resync = replaced && copied;
        stop(volume);
    }

    // Through the block layer's names and cache.
    let mut block_layer = false;
    if let Ok(volume) = create(Level::Stripe, 8, &[mem(0), mem(1)]) {
        let disk = Disk::Volume(volume);
        let sector = [0x5au8; SECTOR];
        let mut back = [0u8; SECTOR];
        block_layer = disk.sectors() == 2 * (USABLE / 8 * 8)
            && block::write(disk, 13, 1, &sector)
            && block::read(disk, 13, 1, &mut back)
            && back == sector
            && member_holds(mem(1), 5, 0x5a);
        stop(volume as usize);
    }

    // Refusals: a member in two volumes, wrong member counts, and nothing
    // left behind afterwards.
    let first = create(Level::Mirror, 0, &[mem(0), mem(1)]);
    let refused = first.is_ok()
        && create(Level::Linear, 0, &[mem(1)]) == Err(Error::Busy)
        && create(Level::Mirror, 0, &[mem(3)]) == Err(Error::Members)
        && create(Level::Stripe, 3, &[mem(2), mem(3)]) == Err(Error::Members)
        && create(Level::Mirror, 0, &[mem(2), mem(2)]) == Err(Error::Busy);
    if let Ok(volume) = first {
        stop(volume as usize);
    }
    let clean = VOLUMES.lock().iter().all(Option::is_none);
    let refused = refused && clean;

    // Headers: a stopped volume comes back from its members (given in any
    // order); a member that missed a membership change comes back failed; an
    // unrelated disk is not taken for a member.
    let mut headers = false;
    for blank in [2usize, 3] {
        MEMORY.lock()[blank].fill(0);
    }
    if let Ok(first) = create(Level::Mirror, 0, &[mem(0), mem(1)]) {
        let first = first as usize;
        fill(&mut buffer, 0, 5);
        let wrote = write(first, 0, 8, &buffer[..8 * SECTOR]);
        let uuid = VOLUMES.lock()[first].map(|volume| volume.uuid);
        stop(first);
        let back = assemble_from(&[mem(1), mem(0)]);
        let intact = match back {
            Ok(volume) => {
                let volume = volume as usize;
                read(volume, 0, 8, &mut check[..8 * SECTOR])
                    && matches(&check[..8 * SECTOR], 0, 5)
                    && VOLUMES.lock()[volume].map(|v| v.uuid) == uuid
                    && sectors(volume) == USABLE
                    && info(volume).is_some_and(|info| info.failed == 0)
            }
            Err(_) => false,
        };
        let degraded_back = match back {
            Ok(volume) => {
                let volume = volume as usize;
                let marked = fail(volume, 1);
                stop(volume);
                match assemble_from(&[mem(0), mem(1)]) {
                    Ok(again) => {
                        let again = again as usize;
                        let ok = marked
                            && info(again).is_some_and(|info| info.failed == 1)
                            && member_failed(again, 1)
                            && read(again, 0, 8, &mut check[..8 * SECTOR])
                            && matches(&check[..8 * SECTOR], 0, 5);
                        stop(again);
                        ok
                    }
                    Err(_) => false,
                }
            }
            Err(_) => false,
        };
        let stranger = assemble_from(&[mem(2), mem(3)]) == Err(Error::Disk);
        headers = wrote && intact && degraded_back && stranger;
    }
    stop(0);
    stop(1);

    Report {
        linear,
        stripe,
        mirror,
        degraded,
        resync,
        block_layer,
        refused,
        headers,
        verified: linear
            && stripe
            && mirror
            && degraded
            && resync
            && block_layer
            && refused
            && headers,
    }
}
