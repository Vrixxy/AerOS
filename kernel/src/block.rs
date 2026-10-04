//! The block layer between the filesystems and the disk drivers: one `Disk`
//! name for every controller, a write-through page cache that gives its
//! memory back under pressure, per-disk counters, and an I/O scheduler for
//! batches of requests (elevator ordering, merging, barriers).

#[cfg(feature = "boot-test")]
use core::sync::atomic::{AtomicBool, Ordering};

use crate::memory::PageBuffer;
use crate::sync::TicketLock;
use crate::{ahci, nvme, sdhci, virtio_blk, xhci};

pub const SECTOR: usize = 512;
pub const MAX_RUN: usize = 8;
const PAGE_SECTORS: u64 = MAX_RUN as u64;
const SETS: usize = 64;
const WAYS: usize = 4;
pub const CACHE_PAGES: usize = SETS * WAYS;
const DISKS: usize = 9;
pub const BATCH_MAX: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Disk {
    Ahci(usize),
    Nvme,
    Usb,
    Sd,
    Virtio,
    /// The block layer's RAM disk, used by the power-loss test.
    #[cfg(feature = "boot-test")]
    Ram,
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
            #[cfg(feature = "boot-test")]
            Disk::Ram => crate::blockdev::RAM_DISK_SECTORS,
        }
    }

    fn index(self) -> usize {
        match self {
            Disk::Ahci(index) => index.min(3),
            Disk::Nvme => 4,
            Disk::Usb => 5,
            Disk::Sd => 6,
            Disk::Virtio => 7,
            #[cfg(feature = "boot-test")]
            Disk::Ram => 8,
        }
    }

    pub fn name(index: usize) -> &'static str {
        [
            "sda", "sdb", "sdc", "sdd", "nvme0n1", "usb0", "mmcblk0", "vda", "ram0",
        ]
        .get(index)
        .copied()
        .unwrap_or("?")
    }

    /// Reads 1..=8 sectors into `buffer`, bypassing the cache.
    pub fn read_run(self, lba: u64, count: usize, buffer: &mut [u8]) -> bool {
        account(self, lba, count, false);
        match self {
            Disk::Ahci(index) => bounce(index, lba, count, Some(buffer), None),
            Disk::Nvme => nvme::read(lba, count as u32, buffer),
            Disk::Usb => xhci::storage_read(lba, count, buffer),
            Disk::Sd => sdhci::read(lba, count, buffer),
            Disk::Virtio => virtio_blk::read(lba, count, buffer),
            #[cfg(feature = "boot-test")]
            Disk::Ram => crate::blockdev::ram_read(lba, &mut buffer[..count * SECTOR]),
        }
    }

    /// Writes 1..=8 sectors straight to the device and drops any cached copy.
    pub fn write_run(self, lba: u64, count: usize, buffer: &[u8]) -> bool {
        account(self, lba, count, true);
        let ok = match self {
            Disk::Ahci(index) => bounce(index, lba, count, None, Some(buffer)),
            Disk::Nvme => nvme::write(lba, count as u32, buffer),
            Disk::Usb => xhci::storage_write(lba, count, buffer),
            Disk::Sd => sdhci::write(lba, count, buffer),
            Disk::Virtio => virtio_blk::write(lba, count, buffer),
            #[cfg(feature = "boot-test")]
            Disk::Ram => crate::blockdev::ram_write(lba, &buffer[..count * SECTOR]),
        };
        if ok && cacheable(self) {
            cache_store(self, lba, count, buffer);
        } else {
            cache_invalidate(self, lba, count as u64);
        }
        ok
    }
}

/// AHCI DMA needs a 2-byte-aligned buffer; callers' buffers (stack arrays,
/// cache slots) have no alignment guarantee, so transfers bounce through this.
#[repr(align(4096))]
struct Bounce([u8; 4096]);

static BOUNCE: TicketLock<Bounce> = TicketLock::new(Bounce([0; 4096]));

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

// ------------------------------------------------------------------ counters

#[derive(Clone, Copy, Default)]
pub struct DiskStats {
    pub reads: u64,
    pub read_sectors: u64,
    pub writes: u64,
    pub write_sectors: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub merges: u64,
    pub seek_sectors: u64,
    pub head: u64,
}

static STATS: TicketLock<[DiskStats; DISKS]> = TicketLock::new(
    [DiskStats {
        reads: 0,
        read_sectors: 0,
        writes: 0,
        write_sectors: 0,
        cache_hits: 0,
        cache_misses: 0,
        merges: 0,
        seek_sectors: 0,
        head: 0,
    }; DISKS],
);

fn account(disk: Disk, lba: u64, count: usize, write: bool) {
    let mut stats = STATS.lock();
    let entry = &mut stats[disk.index()];
    entry.seek_sectors += lba.abs_diff(entry.head);
    entry.head = lba + count as u64;
    if write {
        entry.writes += 1;
        entry.write_sectors += count as u64;
    } else {
        entry.reads += 1;
        entry.read_sectors += count as u64;
    }
}

pub fn stats(index: usize) -> DiskStats {
    STATS.lock().get(index).copied().unwrap_or_default()
}

fn count_cache(disk: Disk, hit: bool) {
    let mut stats = STATS.lock();
    let entry = &mut stats[disk.index()];
    if hit {
        entry.cache_hits += 1;
    } else {
        entry.cache_misses += 1;
    }
}

fn count_merges(disk: Disk, merged: u64) {
    STATS.lock()[disk.index()].merges += merged;
}

// ---------------------------------------------------------------- page cache

struct Page {
    disk: Disk,
    number: u64,
    data: Option<PageBuffer>,
    stamp: u64,
}

impl Page {
    const EMPTY: Page = Page {
        disk: Disk::Nvme,
        number: 0,
        data: None,
        stamp: 0,
    };
}

struct Cache {
    pages: [[Page; WAYS]; SETS],
    clock: u64,
    resident: usize,
    limit: usize,
    last_miss: [u64; DISKS],
    readaheads: u64,
    reclaimed: u64,
}

static CACHE: TicketLock<Cache> = TicketLock::new(Cache {
    pages: [const { [Page::EMPTY; WAYS] }; SETS],
    clock: 0,
    resident: 0,
    limit: CACHE_PAGES,
    last_miss: [u64::MAX; DISKS],
    readaheads: 0,
    reclaimed: 0,
});

#[cfg(feature = "boot-test")]
static CACHE_RAM: AtomicBool = AtomicBool::new(false);

fn cacheable(disk: Disk) -> bool {
    #[cfg(feature = "boot-test")]
    if disk == Disk::Ram {
        return CACHE_RAM.load(Ordering::Relaxed);
    }
    let _ = disk;
    true
}

fn set_of(disk: Disk, number: u64) -> usize {
    ((number ^ (disk.index() as u64).wrapping_mul(0x9e37)) % SETS as u64) as usize
}

impl Cache {
    fn find(&mut self, disk: Disk, number: u64) -> Option<(usize, usize)> {
        let set = set_of(disk, number);
        (0..WAYS)
            .find(|&way| {
                let page = &self.pages[set][way];
                page.data.is_some() && page.disk == disk && page.number == number
            })
            .map(|way| (set, way))
    }

    fn drop_page(&mut self, set: usize, way: usize) {
        if self.pages[set][way].data.take().is_some() {
            self.resident -= 1;
        }
    }

    fn insert(&mut self, disk: Disk, number: u64, data: PageBuffer) {
        let set = set_of(disk, number);
        if let Some(way) = (0..WAYS).find(|&way| self.pages[set][way].data.is_none()) {
            if self.resident >= self.limit {
                return;
            }
            self.pages[set][way] = Page {
                disk,
                number,
                data: Some(data),
                stamp: self.tick(),
            };
            self.resident += 1;
            return;
        }
        let victim = (0..WAYS)
            .min_by_key(|&way| self.pages[set][way].stamp)
            .unwrap_or(0);
        self.pages[set][victim] = Page {
            disk,
            number,
            data: Some(data),
            stamp: self.tick(),
        };
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }
}

fn page_sectors(disk: Disk, number: u64) -> usize {
    let first = number * PAGE_SECTORS;
    disk.sectors().saturating_sub(first).min(PAGE_SECTORS) as usize
}

fn cache_invalidate(disk: Disk, lba: u64, count: u64) {
    let mut cache = CACHE.lock();
    for number in lba / PAGE_SECTORS..=(lba + count.max(1) - 1) / PAGE_SECTORS {
        if let Some((set, way)) = cache.find(disk, number) {
            cache.drop_page(set, way);
        }
    }
}

fn cache_store(disk: Disk, lba: u64, count: usize, data: &[u8]) {
    let mut cache = CACHE.lock();
    for index in 0..count {
        let sector = lba + index as u64;
        let number = sector / PAGE_SECTORS;
        if let Some((set, way)) = cache.find(disk, number) {
            let stamp = cache.tick();
            let page = &mut cache.pages[set][way];
            page.stamp = stamp;
            let offset = (sector % PAGE_SECTORS) as usize * SECTOR;
            if let Some(buffer) = page.data.as_mut() {
                buffer.as_mut_slice()[offset..offset + SECTOR]
                    .copy_from_slice(&data[index * SECTOR..(index + 1) * SECTOR]);
            }
        }
    }
}

fn pressure_allows_caching() -> bool {
    crate::oom::pressure() == crate::oom::Pressure::Normal
}

/// Reads sectors through the cache; `buffer` holds `count * 512` bytes.
pub fn read(disk: Disk, lba: u64, count: usize, buffer: &mut [u8]) -> bool {
    if !cacheable(disk) {
        let mut done = 0;
        while done < count {
            let run = (count - done).min(MAX_RUN);
            if !disk.read_run(
                lba + done as u64,
                run,
                &mut buffer[done * SECTOR..(done + run) * SECTOR],
            ) {
                return false;
            }
            done += run;
        }
        return true;
    }
    let mut done = 0usize;
    while done < count {
        let sector = lba + done as u64;
        let number = sector / PAGE_SECTORS;
        let within = (sector % PAGE_SECTORS) as usize;
        let take = (count - done).min(MAX_RUN - within);
        if !read_page_part(
            disk,
            number,
            within,
            take,
            &mut buffer[done * SECTOR..(done + take) * SECTOR],
        ) {
            return false;
        }
        done += take;
    }
    true
}

fn read_page_part(disk: Disk, number: u64, within: usize, take: usize, out: &mut [u8]) -> bool {
    {
        let mut cache = CACHE.lock();
        if let Some((set, way)) = cache.find(disk, number) {
            let stamp = cache.tick();
            let page = &mut cache.pages[set][way];
            page.stamp = stamp;
            if let Some(buffer) = page.data.as_ref() {
                out.copy_from_slice(&buffer.as_slice()[within * SECTOR..(within + take) * SECTOR]);
            }
            drop(cache);
            count_cache(disk, true);
            return true;
        }
    }
    count_cache(disk, false);
    let sectors = page_sectors(disk, number);
    let first = number * PAGE_SECTORS;
    if sectors == MAX_RUN
        && pressure_allows_caching()
        && let Some(mut page) = PageBuffer::new(MAX_RUN * SECTOR)
    {
        if !disk.read_run(first, MAX_RUN, page.as_mut_slice()) {
            return false;
        }
        out.copy_from_slice(&page.as_slice()[within * SECTOR..(within + take) * SECTOR]);
        let sequential = {
            let mut cache = CACHE.lock();
            let previous = cache.last_miss[disk.index()];
            cache.last_miss[disk.index()] = number;
            cache.insert(disk, number, page);
            previous.wrapping_add(1) == number
        };
        if sequential {
            read_ahead(disk, number + 1);
        }
        return true;
    }
    let mut scratch = [0u8; MAX_RUN * SECTOR];
    if !disk.read_run(first + within as u64, take, &mut scratch[..take * SECTOR]) {
        return false;
    }
    out.copy_from_slice(&scratch[..take * SECTOR]);
    true
}

fn read_ahead(disk: Disk, number: u64) {
    if page_sectors(disk, number) != MAX_RUN || !pressure_allows_caching() {
        return;
    }
    if CACHE.lock().find(disk, number).is_some() {
        return;
    }
    let Some(mut page) = PageBuffer::new(MAX_RUN * SECTOR) else {
        return;
    };
    if disk.read_run(number * PAGE_SECTORS, MAX_RUN, page.as_mut_slice()) {
        let mut cache = CACHE.lock();
        cache.insert(disk, number, page);
        cache.readaheads += 1;
    }
}

/// Writes sectors through to the device, keeping cached copies current.
pub fn write(disk: Disk, lba: u64, count: usize, data: &[u8]) -> bool {
    let mut done = 0;
    while done < count {
        let run = (count - done).min(MAX_RUN);
        if !disk.write_run(
            lba + done as u64,
            run,
            &data[done * SECTOR..(done + run) * SECTOR],
        ) {
            return false;
        }
        done += run;
    }
    true
}

/// Forgets cached sectors another path wrote behind the cache's back.
#[cfg(feature = "linux-guest")]
pub fn invalidate(disk: Disk, lba: u64, count: u64) {
    cache_invalidate(disk, lba, count);
}

pub fn resident_pages() -> usize {
    CACHE.lock().resident
}

/// Cached pages, pages loaded ahead of a sequential reader, pages given back.
pub fn cache_counters() -> (usize, u64, u64) {
    let cache = CACHE.lock();
    (cache.resident, cache.readaheads, cache.reclaimed)
}

pub fn invalidate_disk(disk: Disk) {
    let mut cache = CACHE.lock();
    for set in 0..SETS {
        for way in 0..WAYS {
            if cache.pages[set][way].disk == disk {
                cache.drop_page(set, way);
            }
        }
    }
}

/// Gives cached pages back to the allocator, least recently used first.
/// Returns how many were released.
pub fn reclaim(target: usize) -> usize {
    let Some(mut cache) = CACHE.try_lock() else {
        return 0;
    };
    let mut released = 0;
    while released < target && cache.resident > 0 {
        let mut oldest: Option<(usize, usize, u64)> = None;
        for set in 0..SETS {
            for way in 0..WAYS {
                let page = &cache.pages[set][way];
                if page.data.is_some() && oldest.is_none_or(|(_, _, stamp)| page.stamp < stamp) {
                    oldest = Some((set, way, page.stamp));
                }
            }
        }
        let Some((set, way, _)) = oldest else { break };
        cache.drop_page(set, way);
        released += 1;
    }
    cache.reclaimed += released as u64;
    released
}

// ----------------------------------------------------------------- scheduler

#[cfg_attr(not(feature = "boot-test"), allow(dead_code))]
pub enum IoData<'a> {
    Read(&'a mut [u8]),
    Write(&'a [u8]),
}

pub struct Io<'a> {
    pub disk: Disk,
    pub lba: u64,
    pub sectors: usize,
    pub data: IoData<'a>,
    /// Everything submitted before this request completes before it starts.
    pub barrier: bool,
    pub ok: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "boot-test"), allow(dead_code))]
pub enum Schedule {
    Fifo,
    Elevator,
}

#[repr(align(4096))]
struct Staging([u8; MAX_RUN * SECTOR]);

static STAGING: TicketLock<Staging> = TicketLock::new(Staging([0; MAX_RUN * SECTOR]));

#[cfg(feature = "boot-test")]
static DISPATCH_LOG: TicketLock<([(u64, bool); 128], usize)> =
    TicketLock::new(([(0, false); 128], 0));

fn is_write(io: &Io) -> bool {
    matches!(io.data, IoData::Write(_))
}

fn conflicts(a: &Io, b: &Io) -> bool {
    a.disk == b.disk
        && (is_write(a) || is_write(b))
        && a.lba < b.lba + b.sectors as u64
        && b.lba < a.lba + a.sectors as u64
}

/// Runs a batch of requests (at most [`BATCH_MAX`]) and returns how many
/// succeeded. With [`Schedule::Elevator`] each barrier-delimited group is
/// issued in one sweep upward from the current head position, then wrapped
/// (C-LOOK), and neighbouring requests of the same kind are merged into runs
/// of up to eight sectors. A group in which a write overlaps another request
/// keeps its submission order.
pub fn submit(batch: &mut [Io], schedule: Schedule) -> usize {
    let total = batch.len().min(BATCH_MAX);
    let mut start = 0;
    while start < total {
        let mut end = start + 1;
        while end < total && !batch[end].barrier {
            end += 1;
        }
        run_group(&mut batch[start..end], schedule);
        start = end;
    }
    batch[..total].iter().filter(|io| io.ok).count()
}

fn run_group(group: &mut [Io], schedule: Schedule) {
    let count = group.len();
    let hazard = (0..count).any(|a| (a + 1..count).any(|b| conflicts(&group[a], &group[b])));
    let ordered = schedule == Schedule::Elevator && !hazard;
    let mut order = [0u8; BATCH_MAX];
    let mut pending = 0;
    for (index, io) in group.iter_mut().enumerate() {
        if ordered && !is_write(io) && cacheable(io.disk) && serve_cached(io) {
            continue;
        }
        order[pending] = index as u8;
        pending += 1;
    }
    if ordered {
        order[..pending].sort_unstable_by_key(|&index| {
            let io = &group[index as usize];
            (io.disk.index(), io.lba)
        });
        let mut from = 0;
        while from < pending {
            let disk = group[order[from] as usize].disk;
            let mut to = from;
            while to < pending && group[order[to] as usize].disk == disk {
                to += 1;
            }
            let head = STATS.lock()[disk.index()].head;
            let split = order[from..to]
                .iter()
                .position(|&index| group[index as usize].lba >= head)
                .unwrap_or(0);
            order[from..to].rotate_left(split);
            from = to;
        }
    }
    let mut position = 0;
    while position < pending {
        let first = order[position] as usize;
        let mut length = group[first].sectors;
        let mut last = position;
        if ordered && length < MAX_RUN {
            while last + 1 < pending {
                let next = &group[order[last + 1] as usize];
                let current = &group[order[last] as usize];
                if next.disk == group[first].disk
                    && is_write(next) == is_write(&group[first])
                    && next.lba == current.lba + current.sectors as u64
                    && length + next.sectors <= MAX_RUN
                {
                    length += next.sectors;
                    last += 1;
                } else {
                    break;
                }
            }
        }
        if last == position {
            run_single(&mut group[first], !ordered);
        } else {
            run_merged(group, &order[position..=last], length);
            count_merges(group[first].disk, (last - position) as u64);
        }
        position = last + 1;
    }
}

fn valid(io: &Io) -> bool {
    let bytes = io.sectors * SECTOR;
    io.sectors > 0
        && io.sectors <= MAX_RUN
        && match &io.data {
            IoData::Read(buffer) => buffer.len() >= bytes,
            IoData::Write(buffer) => buffer.len() >= bytes,
        }
}

/// Completes a read from cached pages if every sector is resident.
fn serve_cached(io: &mut Io) -> bool {
    if !valid(io) {
        return false;
    }
    let IoData::Read(buffer) = &mut io.data else {
        return false;
    };
    let mut cache = CACHE.lock();
    for index in 0..io.sectors {
        let sector = io.lba + index as u64;
        let Some((set, way)) = cache.find(io.disk, sector / PAGE_SECTORS) else {
            return false;
        };
        let stamp = cache.tick();
        let page = &mut cache.pages[set][way];
        page.stamp = stamp;
        let offset = (sector % PAGE_SECTORS) as usize * SECTOR;
        if let Some(data) = page.data.as_ref() {
            buffer[index * SECTOR..(index + 1) * SECTOR]
                .copy_from_slice(&data.as_slice()[offset..offset + SECTOR]);
        }
    }
    drop(cache);
    for _ in 0..io.sectors {
        count_cache(io.disk, true);
    }
    io.ok = true;
    true
}

/// Keeps a freshly read run as a cache page when it is exactly one page.
fn cache_fill(disk: Disk, lba: u64, count: usize, data: &[u8]) {
    if count != MAX_RUN
        || !lba.is_multiple_of(PAGE_SECTORS)
        || !cacheable(disk)
        || !pressure_allows_caching()
        || page_sectors(disk, lba / PAGE_SECTORS) != MAX_RUN
    {
        return;
    }
    let number = lba / PAGE_SECTORS;
    if CACHE.lock().find(disk, number).is_some() {
        return;
    }
    if let Some(mut page) = PageBuffer::new(MAX_RUN * SECTOR) {
        page.as_mut_slice()
            .copy_from_slice(&data[..MAX_RUN * SECTOR]);
        CACHE.lock().insert(disk, number, page);
    }
}

fn run_single(io: &mut Io, through_cache: bool) {
    if !valid(io) {
        io.ok = false;
        return;
    }
    #[cfg(feature = "boot-test")]
    log_dispatch(io.lba, is_write(io));
    io.ok = match &mut io.data {
        IoData::Read(buffer) if through_cache => read(io.disk, io.lba, io.sectors, buffer),
        IoData::Read(buffer) => {
            let ok = io.disk.read_run(io.lba, io.sectors, buffer);
            if ok {
                cache_fill(io.disk, io.lba, io.sectors, buffer);
            }
            ok
        }
        IoData::Write(buffer) => io.disk.write_run(io.lba, io.sectors, buffer),
    };
}

fn run_merged(group: &mut [Io], members: &[u8], length: usize) {
    if members.iter().any(|&index| !valid(&group[index as usize])) {
        for &index in members {
            run_single(&mut group[index as usize], false);
        }
        return;
    }
    let first = &group[members[0] as usize];
    let (disk, lba, write) = (first.disk, first.lba, is_write(first));
    #[cfg(feature = "boot-test")]
    log_dispatch(lba, write);
    let mut staging = STAGING.lock();
    let mut offset = 0;
    if write {
        for &index in members {
            let io = &group[index as usize];
            if let IoData::Write(source) = &io.data {
                let bytes = io.sectors * SECTOR;
                staging.0[offset..offset + bytes].copy_from_slice(&source[..bytes]);
                offset += bytes;
            }
        }
    }
    let ok = if write {
        disk.write_run(lba, length, &staging.0)
    } else {
        disk.read_run(lba, length, &mut staging.0)
    };
    if ok && !write {
        cache_fill(disk, lba, length, &staging.0);
    }
    offset = 0;
    for &index in members {
        let io = &mut group[index as usize];
        let bytes = io.sectors * SECTOR;
        if ok && let IoData::Read(destination) = &mut io.data {
            destination[..bytes].copy_from_slice(&staging.0[offset..offset + bytes]);
        }
        io.ok = ok;
        offset += bytes;
    }
}

/// Reads a list of (first sector, sector count) extents that lie one after
/// another in `out`, as one scheduled batch per 64 page-aligned pieces.
pub fn read_scatter(disk: Disk, extents: &[(u64, usize)], out: &mut [u8]) -> bool {
    let mut rest: &mut [u8] = out;
    let mut cursor = 0usize;
    let mut pieces: [(u64, usize); BATCH_MAX] = [(0, 0); BATCH_MAX];
    let mut queued = 0usize;
    let mut extent = 0usize;
    let mut sector_in_extent = 0usize;
    loop {
        while queued < BATCH_MAX && extent < extents.len() {
            let (first, total) = extents[extent];
            let lba = first + sector_in_extent as u64;
            let take = (total - sector_in_extent).min((PAGE_SECTORS - lba % PAGE_SECTORS) as usize);
            pieces[queued] = (lba, take);
            queued += 1;
            sector_in_extent += take;
            if sector_in_extent == total {
                extent += 1;
                sector_in_extent = 0;
            }
        }
        if queued == 0 {
            return true;
        }
        let mut slices: [Option<&mut [u8]>; BATCH_MAX] = [const { None }; BATCH_MAX];
        for (slot, &(_, sectors)) in slices.iter_mut().zip(&pieces[..queued]) {
            let taken = core::mem::take(&mut rest);
            if taken.len() < sectors * SECTOR {
                return false;
            }
            let (head, tail) = taken.split_at_mut(sectors * SECTOR);
            *slot = Some(head);
            rest = tail;
            cursor += sectors;
        }
        let mut batch: [Io; BATCH_MAX] = core::array::from_fn(|index| Io {
            disk,
            lba: pieces[index].0,
            sectors: if index < queued { pieces[index].1 } else { 0 },
            data: IoData::Read(slices[index].take().unwrap_or_default()),
            barrier: false,
            ok: false,
        });
        if submit(&mut batch[..queued], Schedule::Elevator) != queued {
            return false;
        }
        queued = 0;
        let _ = cursor;
    }
}

#[cfg(feature = "boot-test")]
fn log_dispatch(lba: u64, write: bool) {
    let mut log = DISPATCH_LOG.lock();
    let position = log.1;
    if position < log.0.len() {
        log.0[position] = (lba, write);
        log.1 += 1;
    }
}

// --------------------------------------------------------------------- tests

#[cfg(feature = "boot-test")]
#[derive(Clone, Copy, Default)]
pub struct Report {
    pub fifo_seek: u64,
    pub elevator_seek: u64,
    pub fifo_commands: u64,
    pub elevator_commands: u64,
    pub merged: u64,
    pub barrier_order: bool,
    pub hazard_order: bool,
    pub data_intact: bool,
    pub cache_hit: bool,
    pub write_through: bool,
    pub eviction: bool,
    pub failed_write: bool,
    pub readahead: bool,
    pub reclaimed: bool,
    pub pressure_gate: bool,
    pub verified: bool,
}

#[cfg(feature = "boot-test")]
fn pattern(lba: u64) -> u8 {
    (lba.wrapping_mul(2654435761) >> 7) as u8 ^ 0x5a
}

#[cfg(feature = "boot-test")]
fn fill_ram(first: u64, count: u64) {
    for lba in first..first + count {
        let sector = [pattern(lba); SECTOR];
        crate::blockdev::ram_write(lba, &sector);
    }
}

#[cfg(feature = "boot-test")]
fn run_reads(lbas: &[u64], schedule: Schedule) -> (u64, u64, bool, u64) {
    let mut buffers = [[0u8; SECTOR]; 48];
    let before = stats(Disk::Ram.index());
    let mut ok = true;
    {
        let mut slots = buffers.each_mut().into_iter();
        let mut ios: [Io; 48] = core::array::from_fn(|index| {
            let slot = slots.next().map_or(&mut [][..], |slot| &mut slot[..]);
            Io {
                disk: Disk::Ram,
                lba: lbas.get(index).copied().unwrap_or(0),
                sectors: usize::from(index < lbas.len()),
                data: IoData::Read(slot),
                barrier: false,
                ok: false,
            }
        });
        submit(&mut ios[..lbas.len()], schedule);
        ok &= ios[..lbas.len()].iter().all(|io| io.ok);
    }
    let after = stats(Disk::Ram.index());
    for (index, &lba) in lbas.iter().enumerate() {
        ok &= buffers[index] == [pattern(lba); SECTOR];
    }
    (
        after.seek_sectors - before.seek_sectors,
        after.reads - before.reads,
        ok,
        after.merges - before.merges,
    )
}

#[cfg(feature = "boot-test")]
fn ram_hits() -> u64 {
    stats(Disk::Ram.index()).cache_hits
}

#[cfg(feature = "boot-test")]
pub fn self_test() -> Report {
    use core::sync::atomic::Ordering::Relaxed;
    let mut report = Report::default();
    fill_ram(0, 4096);

    let mut scattered = [0u64; 32];
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    for lba in scattered.iter_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *lba = (64 + (state % 3900)) & !7;
        *lba += state % 5;
    }
    let (fifo_seek, fifo_reads, fifo_ok, _) = run_reads(&scattered, Schedule::Fifo);
    let (elevator_seek, elevator_reads, elevator_ok, _) = run_reads(&scattered, Schedule::Elevator);
    let _ = (fifo_reads, elevator_reads);
    report.fifo_seek = fifo_seek;
    report.elevator_seek = elevator_seek;

    let mut contiguous = [0u64; 48];
    for (index, lba) in contiguous.iter_mut().enumerate() {
        *lba = 2000 + ((index * 29) % 48) as u64;
    }
    let (_, fifo_commands, fifo_contiguous_ok, _) = run_reads(&contiguous, Schedule::Fifo);
    let (_, elevator_commands, elevator_contiguous_ok, merges) =
        run_reads(&contiguous, Schedule::Elevator);
    report.fifo_commands = fifo_commands;
    report.elevator_commands = elevator_commands;
    report.merged = merges;
    report.data_intact = fifo_ok && elevator_ok && fifo_contiguous_ok && elevator_contiguous_ok;

    {
        let data = [[1u8; SECTOR], [2; SECTOR], [3; SECTOR], [4; SECTOR]];
        let make = |lba: u64, index: usize, barrier: bool| Io {
            disk: Disk::Ram,
            lba,
            sectors: 1,
            data: IoData::Write(&data[index]),
            barrier,
            ok: false,
        };
        let mut batch = [
            make(90, 0, false),
            make(10, 1, false),
            make(50, 2, true),
            make(30, 3, false),
        ];
        DISPATCH_LOG.lock().1 = 0;
        STATS.lock()[Disk::Ram.index()].head = 0;
        let done = submit(&mut batch, Schedule::Elevator);
        let log = DISPATCH_LOG.lock();
        report.barrier_order = done == 4
            && log.1 == 4
            && [log.0[0].0, log.0[1].0, log.0[2].0, log.0[3].0] == [10, 90, 30, 50];
        drop(log);
        fill_ram(0, 100);
    }

    {
        let new = [[0xeeu8; SECTOR]];
        let mut readback = [0u8; SECTOR];
        let mut batch = [
            Io {
                disk: Disk::Ram,
                lba: 700,
                sectors: 1,
                data: IoData::Write(&new[0]),
                barrier: false,
                ok: false,
            },
            Io {
                disk: Disk::Ram,
                lba: 700,
                sectors: 1,
                data: IoData::Read(&mut readback),
                barrier: false,
                ok: false,
            },
        ];
        let done = submit(&mut batch, Schedule::Elevator);
        report.hazard_order = done == 2 && readback == new[0];
        fill_ram(700, 1);
    }

    CACHE_RAM.store(true, Relaxed);
    reclaim(usize::MAX);
    let free_before = crate::memory::TRACKED_FREE_PAGES.load(Relaxed);
    let hits_before = ram_hits();
    let mut sector = [0u8; SECTOR];
    let first = read(Disk::Ram, 1000, 1, &mut sector) && sector == [pattern(1000); SECTOR];
    let second = read(Disk::Ram, 1000, 1, &mut sector) && sector == [pattern(1000); SECTOR];
    report.cache_hit = first && second && ram_hits() == hits_before + 1;

    let updated = [0x77u8; SECTOR];
    let wrote = write(Disk::Ram, 1000, 1, &updated);
    let hits = ram_hits();
    let cached = read(Disk::Ram, 1000, 1, &mut sector) && sector == updated;
    let mut raw = [0u8; SECTOR];
    let device = crate::blockdev::ram_read(1000, &mut raw) && raw == updated;
    report.write_through = wrote && cached && device && ram_hits() == hits + 1;

    crate::blockdev::inject_power_loss_after(0);
    let lost = write(Disk::Ram, 1001, 1, &[0x11; SECTOR]);
    crate::blockdev::clear_power_loss();
    let mut after = [0u8; SECTOR];
    let _ = read(Disk::Ram, 1001, 1, &mut after);
    report.failed_write = !lost && after == [pattern(1001); SECTOR];

    reclaim(usize::MAX);
    let base = 1024u64;
    for way in 0..=WAYS as u64 {
        let lba = (base + way * SETS as u64 * 2) * PAGE_SECTORS;
        let _ = read(Disk::Ram, lba, 1, &mut sector);
    }
    let evicted_target = base * PAGE_SECTORS;
    let hits = ram_hits();
    let _ = read(Disk::Ram, evicted_target, 1, &mut sector);
    let newest = (base + WAYS as u64 * SETS as u64 * 2) * PAGE_SECTORS;
    let _ = read(Disk::Ram, newest, 1, &mut sector);
    report.eviction = ram_hits() == hits + 1;

    reclaim(usize::MAX);
    let readaheads = cache_counters().1;
    for page in 100..104u64 {
        let _ = read(Disk::Ram, page * PAGE_SECTORS, 1, &mut sector);
    }
    report.readahead = cache_counters().1 > readaheads;

    let resident = resident_pages();
    let freed = reclaim(usize::MAX);
    let free_after = crate::memory::TRACKED_FREE_PAGES.load(Relaxed);
    report.reclaimed = resident > 0
        && freed == resident
        && resident_pages() == 0
        && free_after >= free_before.saturating_sub(1);
    report.pressure_gate = crate::oom::classify(5, 1000) != crate::oom::Pressure::Normal;
    CACHE_RAM.store(false, Relaxed);
    reclaim(usize::MAX);

    report.verified = report.data_intact
        && report.elevator_seek * 2 < report.fifo_seek
        && report.elevator_commands * 4 <= report.fifo_commands
        && report.merged >= 40
        && report.barrier_order
        && report.hazard_order
        && report.cache_hit
        && report.write_through
        && report.failed_write
        && report.eviction
        && report.readahead
        && report.reclaimed
        && report.pressure_gate;
    report
}
