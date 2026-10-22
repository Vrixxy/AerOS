use crate::sync::TicketLock;

const PAGE_SIZE: u64 = 4096;
/// Memory below this is never handed out. Besides the legacy area, it is the
/// part of the identity map a fixed-address (`ET_EXEC`) program's window
/// replaces in its own page tables (see `paging::LOW_WINDOW_END`), so nothing
/// the kernel allocates may live there.
const LOW_MEMORY_LIMIT: u64 = crate::arch::paging::LOW_WINDOW_END;
const MAX_MEMORY_REGIONS: usize = 256;
const MAX_FREE_RANGES: usize = 512;

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MemoryKind {
    Usable,
    BootReclaimable,
    Runtime,
    AcpiReclaimable,
    AcpiNvs,
    Mmio,
    Persistent,
    Unusable,
    Reserved,
}

#[derive(Clone, Copy)]
pub struct MemoryRegion {
    pub start: u64,
    pub pages: u64,
    pub kind: MemoryKind,
}

impl MemoryRegion {
    const EMPTY: Self = Self {
        start: 0,
        pages: 0,
        kind: MemoryKind::Reserved,
    };

    fn end(self) -> Option<u64> {
        self.pages
            .checked_mul(PAGE_SIZE)
            .and_then(|size| self.start.checked_add(size))
    }
}

#[derive(Clone, Copy)]
pub struct BootMemoryMap {
    regions: [MemoryRegion; MAX_MEMORY_REGIONS],
    count: usize,
    usable_pages: u64,
    reclaimable_pages: u64,
    dropped: usize,
}

impl BootMemoryMap {
    pub const fn empty() -> Self {
        Self {
            regions: [MemoryRegion::EMPTY; MAX_MEMORY_REGIONS],
            count: 0,
            usable_pages: 0,
            reclaimable_pages: 0,
            dropped: 0,
        }
    }

    pub fn push_uefi(&mut self, raw_kind: u32, start: u64, pages: u64) {
        if pages == 0 || start & (PAGE_SIZE - 1) != 0 {
            self.dropped += 1;
            return;
        }
        let kind = match raw_kind {
            1..=4 => MemoryKind::BootReclaimable,
            5 | 6 => MemoryKind::Runtime,
            7 => MemoryKind::Usable,
            8 => MemoryKind::Unusable,
            9 => MemoryKind::AcpiReclaimable,
            10 => MemoryKind::AcpiNvs,
            11 | 12 => MemoryKind::Mmio,
            14 => MemoryKind::Persistent,
            _ => MemoryKind::Reserved,
        };
        if kind == MemoryKind::Usable {
            self.usable_pages = self.usable_pages.saturating_add(pages);
        }
        if kind == MemoryKind::BootReclaimable {
            self.reclaimable_pages = self.reclaimable_pages.saturating_add(pages);
        }
        if self.count > 0 {
            let previous = &mut self.regions[self.count - 1];
            if previous.kind == kind && previous.end() == Some(start) {
                previous.pages = previous.pages.saturating_add(pages);
                return;
            }
        }
        if self.count == MAX_MEMORY_REGIONS {
            self.dropped += 1;
            return;
        }
        self.regions[self.count] = MemoryRegion { start, pages, kind };
        self.count += 1;
    }

    pub fn regions(&self) -> &[MemoryRegion] {
        &self.regions[..self.count]
    }

    pub fn region_count(&self) -> usize {
        self.count
    }

    pub fn usable_pages(&self) -> u64 {
        self.usable_pages
    }

    pub fn reclaimable_pages(&self) -> u64 {
        self.reclaimable_pages
    }

    pub fn dropped_regions(&self) -> usize {
        self.dropped
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct PhysFrame(u64);

impl PhysFrame {
    pub fn address(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy)]
struct Range {
    start: u64,
    end: u64,
}

impl Range {
    const EMPTY: Self = Self { start: 0, end: 0 };

    fn contains(self, address: u64) -> bool {
        address >= self.start && address < self.end
    }
}

#[derive(Clone, Copy)]
pub struct AllocatorStats {
    pub free_pages: u64,
    pub allocated_pages: u64,
    pub free_ranges: usize,
    pub managed_regions: usize,
}

pub struct FrameAllocator {
    managed: [Range; MAX_MEMORY_REGIONS],
    managed_count: usize,
    free: [Range; MAX_FREE_RANGES],
    free_count: usize,
    free_pages: u64,
    allocated_pages: u64,
    /// Mirrors its free-page count into `TRACKED_FREE_PAGES` (the main allocator).
    tracked: bool,
}

/// Free pages of the tracked (main) allocator, readable from anywhere.
pub static TRACKED_FREE_PAGES: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
/// Total pages the tracked allocator manages (free plus allocated at `track`).
pub static TRACKED_TOTAL_PAGES: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

impl FrameAllocator {
    /// Makes this allocator the one the system monitor reports on.
    pub fn track(&mut self) {
        self.tracked = true;
        TRACKED_TOTAL_PAGES.store(
            self.free_pages + self.allocated_pages,
            core::sync::atomic::Ordering::Relaxed,
        );
        self.publish();
    }

    fn publish(&self) {
        if self.tracked {
            TRACKED_FREE_PAGES.store(self.free_pages, core::sync::atomic::Ordering::Relaxed);
        }
    }

    /// The (start, end) byte ranges of RAM this allocator manages.
    pub fn managed_ranges(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        self.managed[..self.managed_count]
            .iter()
            .map(|range| (range.start, range.end))
    }

    pub fn from_map(map: &BootMemoryMap) -> Self {
        let mut allocator = Self {
            managed: [Range::EMPTY; MAX_MEMORY_REGIONS],
            managed_count: 0,
            free: [Range::EMPTY; MAX_FREE_RANGES],
            free_count: 0,
            free_pages: 0,
            allocated_pages: 0,
            tracked: false,
        };
        for region in map.regions() {
            if region.kind != MemoryKind::Usable {
                continue;
            }
            let start = align_up(region.start.max(LOW_MEMORY_LIMIT), PAGE_SIZE);
            let Some(end) = region.end().map(|value| align_down(value, PAGE_SIZE)) else {
                continue;
            };
            if start >= end
                || allocator.managed_count == MAX_MEMORY_REGIONS
                || allocator.free_count == MAX_FREE_RANGES
            {
                continue;
            }
            let range = Range { start, end };
            allocator.managed[allocator.managed_count] = range;
            allocator.managed_count += 1;
            allocator.free[allocator.free_count] = range;
            allocator.free_count += 1;
            allocator.free_pages = allocator
                .free_pages
                .saturating_add((end - start) / PAGE_SIZE);
        }
        allocator.sort_free();
        allocator.coalesce_free();
        allocator
    }

    pub fn allocate(&mut self) -> Option<PhysFrame> {
        self.allocate_contiguous(1, 1)
    }

    /// Like `allocate_contiguous`, for memory a device will read or write: the
    /// IOMMU is told to let devices reach it.
    pub fn allocate_dma(&mut self, pages: u64, alignment_pages: u64) -> Option<PhysFrame> {
        let frame = self.allocate_contiguous(pages, alignment_pages)?;
        if crate::iommu::allow(frame.address(), pages) {
            return Some(frame);
        }
        self.release_contiguous(frame.address(), pages);
        None
    }

    pub fn allocate_contiguous(&mut self, pages: u64, alignment_pages: u64) -> Option<PhysFrame> {
        if pages == 0 || !alignment_pages.is_power_of_two() {
            return None;
        }
        let bytes = pages.checked_mul(PAGE_SIZE)?;
        let alignment = alignment_pages.checked_mul(PAGE_SIZE)?;
        for index in 0..self.free_count {
            let range = self.free[index];
            let start = align_up(range.start, alignment);
            let end = start.checked_add(bytes)?;
            if end > range.end {
                continue;
            }
            if start > range.start && end < range.end && self.free_count == MAX_FREE_RANGES {
                continue;
            }
            self.consume_range(index, range, start, end);
            self.free_pages = self.free_pages.saturating_sub(pages);
            self.allocated_pages = self.allocated_pages.saturating_add(pages);
            self.publish();
            return Some(PhysFrame(start));
        }
        None
    }

    pub fn release(&mut self, frame: PhysFrame) -> bool {
        self.release_contiguous(frame.address(), 1)
    }

    pub fn release_contiguous(&mut self, start: u64, pages: u64) -> bool {
        if pages == 0 || start & (PAGE_SIZE - 1) != 0 {
            return false;
        }
        let Some(bytes) = pages.checked_mul(PAGE_SIZE) else {
            return false;
        };
        let Some(end) = start.checked_add(bytes) else {
            return false;
        };
        if !self.managed[..self.managed_count]
            .iter()
            .any(|range| range.contains(start) && end <= range.end)
            || self.free[..self.free_count]
                .iter()
                .any(|range| start < range.end && end > range.start)
        {
            return false;
        }
        let Some(free_pages) = self.free_pages.checked_add(pages) else {
            return false;
        };
        let Some(allocated_pages) = self.allocated_pages.checked_sub(pages) else {
            return false;
        };
        // Every frame handed back here comes from a process address space
        // being torn down (`destroy_process`/exec-rollback in paging.rs) -
        // its code, heap, anonymous-mmap or stack pages can hold anything
        // that process had in memory (decrypted file contents, terminal
        // input, key material). `allocate_contiguous` already zeroes fresh
        // frames before handing them to a *new* owner, but that still
        // leaves a window where a dead process's data sits untouched in
        // "free" physical RAM until something happens to reuse that exact
        // frame. Every caller only releases a frame once it has already
        // unmapped it (the `frame_release`/`frame_share` refcounting in
        // paging.rs guarantees a shared frame only reaches here from its
        // last owner), so nothing can still be reading through this
        // physical address - safe to wipe it right now instead of waiting.
        unsafe {
            core::ptr::write_bytes(start as usize as *mut u8, 0, bytes as usize);
        }
        let left = self.free[..self.free_count]
            .iter()
            .position(|range| range.end == start);
        let right = self.free[..self.free_count]
            .iter()
            .position(|range| range.start == end);
        match (left, right) {
            (Some(left), Some(right)) => {
                self.free[left].end = self.free[right].end;
                self.remove_free(right);
            }
            (Some(left), None) => self.free[left].end = end,
            (None, Some(right)) => self.free[right].start = start,
            (None, None) => {
                if self.free_count == MAX_FREE_RANGES {
                    return false;
                }
                self.free[self.free_count] = Range { start, end };
                self.free_count += 1;
            }
        }
        self.free_pages = free_pages;
        self.allocated_pages = allocated_pages;
        self.publish();
        self.sort_free();
        self.coalesce_free();
        true
    }

    pub fn stats(&self) -> AllocatorStats {
        AllocatorStats {
            free_pages: self.free_pages,
            allocated_pages: self.allocated_pages,
            free_ranges: self.free_count,
            managed_regions: self.managed_count,
        }
    }

    pub fn self_test(&mut self) -> bool {
        let before = self.stats();
        let Some(first) = self.allocate() else {
            return false;
        };
        let Some(second) = self.allocate() else {
            return false;
        };
        if first == second || first.address() & (PAGE_SIZE - 1) != 0 {
            return false;
        }
        unsafe {
            core::ptr::write_bytes(
                first.address() as usize as *mut u8,
                0xa5,
                PAGE_SIZE as usize,
            );
        }
        if !self.release(first) {
            return false;
        }
        // `allocate`/`allocate_contiguous` don't zero on their own (callers
        // in paging.rs do that explicitly for a *new* owner) - reading the
        // physical byte back right here, before anything reallocates this
        // exact frame, is what actually proves `release_contiguous` did.
        let wiped = unsafe { core::ptr::read_volatile(first.address() as usize as *const u8) } == 0;
        let reused = self.allocate();
        let first_stage =
            wiped && reused == Some(first) && self.release(second) && self.release(first);
        let Some(block) = self.allocate_contiguous(4, 4) else {
            return false;
        };
        let second_stage = block.address() & (PAGE_SIZE * 4 - 1) == 0
            && self.release_contiguous(block.address(), 4);
        let after = self.stats();
        first_stage
            && second_stage
            && after.free_pages == before.free_pages
            && after.allocated_pages == before.allocated_pages
    }

    fn consume_range(&mut self, index: usize, range: Range, start: u64, end: u64) {
        match (start > range.start, end < range.end) {
            (false, false) => self.remove_free(index),
            (false, true) => self.free[index].start = end,
            (true, false) => self.free[index].end = start,
            (true, true) => {
                self.free[index].end = start;
                self.insert_free(
                    index + 1,
                    Range {
                        start: end,
                        end: range.end,
                    },
                );
            }
        }
    }

    fn insert_free(&mut self, index: usize, range: Range) {
        for position in (index..self.free_count).rev() {
            self.free[position + 1] = self.free[position];
        }
        self.free[index] = range;
        self.free_count += 1;
    }

    fn remove_free(&mut self, index: usize) {
        for position in index..self.free_count - 1 {
            self.free[position] = self.free[position + 1];
        }
        self.free_count -= 1;
        self.free[self.free_count] = Range::EMPTY;
    }

    fn sort_free(&mut self) {
        for outer in 1..self.free_count {
            let value = self.free[outer];
            let mut inner = outer;
            while inner > 0 && self.free[inner - 1].start > value.start {
                self.free[inner] = self.free[inner - 1];
                inner -= 1;
            }
            self.free[inner] = value;
        }
    }

    fn coalesce_free(&mut self) {
        let mut index = 0;
        while index + 1 < self.free_count {
            if self.free[index].end >= self.free[index + 1].start {
                self.free[index].end = self.free[index].end.max(self.free[index + 1].end);
                self.remove_free(index + 1);
            } else {
                index += 1;
            }
        }
    }
}

fn align_up(value: u64, alignment: u64) -> u64 {
    value.saturating_add(alignment - 1) & !(alignment - 1)
}

fn align_down(value: u64, alignment: u64) -> u64 {
    value & !(alignment - 1)
}

/// The most memory the process pool can hold: 1 GiB.
pub const POOL_MAX_PAGES: usize = 1 << 18;
const POOL_BLOCKS: usize = 16;

/// The pool every process's pages come from: a few blocks of RAM and one
/// bit per page, so a page is always given back however fragmented the pool
/// is (a free list of ranges can run out of entries).
struct PagePool {
    /// (first byte, pages, first bit) of each block.
    blocks: [(u64, usize, usize); POOL_BLOCKS],
    block_count: usize,
    used: [u64; POOL_MAX_PAGES / 64],
    pages: usize,
    allocated: usize,
    /// A word to start looking for a free page from.
    hint: usize,
}

static POOL: TicketLock<PagePool> = TicketLock::new(PagePool {
    blocks: [(0, 0, 0); POOL_BLOCKS],
    block_count: 0,
    used: [0; POOL_MAX_PAGES / 64],
    pages: 0,
    allocated: 0,
    hint: 0,
});

impl PagePool {
    fn is_used(&self, bit: usize) -> bool {
        self.used[bit / 64] & (1 << (bit % 64)) != 0
    }

    fn set(&mut self, bit: usize, used: bool) {
        if used {
            self.used[bit / 64] |= 1 << (bit % 64);
        } else {
            self.used[bit / 64] &= !(1 << (bit % 64));
        }
    }

    fn address(&self, bit: usize) -> Option<u64> {
        self.blocks[..self.block_count]
            .iter()
            .find(|(_, pages, first)| (*first..first + pages).contains(&bit))
            .map(|(start, _, first)| start + (bit - first) as u64 * PAGE_SIZE)
    }

    fn bit(&self, address: u64) -> Option<usize> {
        self.blocks[..self.block_count]
            .iter()
            .find(|(start, pages, _)| {
                address >= *start && address < start + *pages as u64 * PAGE_SIZE
            })
            .map(|(start, _, first)| first + ((address - start) / PAGE_SIZE) as usize)
    }

    fn add(&mut self, start: u64, pages: usize) -> bool {
        let pages = pages.min(POOL_MAX_PAGES - self.pages);
        if pages == 0 || self.block_count == POOL_BLOCKS || start & (PAGE_SIZE - 1) != 0 {
            return false;
        }
        self.blocks[self.block_count] = (start, pages, self.pages);
        self.block_count += 1;
        self.pages += pages;
        true
    }

    fn allocate_one(&mut self) -> Option<u64> {
        let words = self.pages.div_ceil(64);
        for step in 0..words {
            let word = (self.hint + step) % words;
            let free = !self.used[word];
            if free == 0 {
                continue;
            }
            let bit = word * 64 + free.trailing_zeros() as usize;
            if bit >= self.pages {
                continue;
            }
            self.set(bit, true);
            self.allocated += 1;
            self.hint = word;
            return self.address(bit);
        }
        None
    }

    /// `pages` pages in a row, inside one block, starting at a multiple of
    /// `alignment` pages (physically).
    fn allocate_run(&mut self, pages: usize, alignment: u64) -> Option<u64> {
        for block in 0..self.block_count {
            let (start, count, first) = self.blocks[block];
            let mut page = 0;
            while page + pages <= count {
                let address = start + page as u64 * PAGE_SIZE;
                if !(address / PAGE_SIZE).is_multiple_of(alignment) {
                    page += 1;
                    continue;
                }
                match (0..pages).find(|offset| self.is_used(first + page + offset)) {
                    Some(offset) => page += offset + 1,
                    None => {
                        for offset in 0..pages {
                            self.set(first + page + offset, true);
                        }
                        self.allocated += pages;
                        return Some(address);
                    }
                }
            }
        }
        None
    }

    fn release(&mut self, address: u64, pages: usize, wipe: bool) -> bool {
        let Some(first) = self.bit(address) else {
            return false;
        };
        // The whole run must be in one block and allocated.
        if self.bit(address + (pages as u64 - 1) * PAGE_SIZE) != Some(first + pages - 1)
            || (0..pages).any(|offset| !self.is_used(first + offset))
        {
            return false;
        }
        // A dead process's pages can hold anything it had in memory (file
        // contents, input, keys): wiped now rather than when they are reused.
        if wipe {
            unsafe {
                core::ptr::write_bytes(address as usize as *mut u8, 0, pages * PAGE_SIZE as usize)
            };
        }
        for offset in 0..pages {
            self.set(first + offset, false);
        }
        self.allocated -= pages;
        true
    }
}

/// Adds a block of RAM to the process pool; false when the pool is full.
pub fn add_global_block(start: u64, pages: u64) -> bool {
    POOL.lock().add(start, usize::try_from(pages).unwrap_or(0))
}

pub fn allocate_global() -> Option<PhysFrame> {
    POOL.lock().allocate_one().map(PhysFrame)
}

pub fn release_global_address(address: u64) -> bool {
    POOL.lock().release(address, 1, true)
}

pub fn allocate_global_contiguous(pages: u64, alignment_pages: u64) -> Option<PhysFrame> {
    if pages == 0 || !alignment_pages.is_power_of_two() {
        return None;
    }
    let pages = usize::try_from(pages).ok()?;
    if pages == 1 && alignment_pages == 1 {
        return allocate_global();
    }
    POOL.lock()
        .allocate_run(pages, alignment_pages)
        .map(PhysFrame)
}

pub fn release_global_contiguous(address: u64, pages: u64) -> bool {
    match usize::try_from(pages) {
        Ok(pages) if pages != 0 && address & (PAGE_SIZE - 1) == 0 => {
            POOL.lock().release(address, pages, true)
        }
        _ => false,
    }
}

/// `release_global_contiguous` for memory that holds nothing of a process's
/// (its page tables), which is not wiped first.
pub fn release_global_contiguous_unwiped(address: u64, pages: u64) -> bool {
    match usize::try_from(pages) {
        Ok(pages) if pages != 0 && address & (PAGE_SIZE - 1) == 0 => {
            POOL.lock().release(address, pages, false)
        }
        _ => false,
    }
}

pub fn global_stats() -> Option<AllocatorStats> {
    let pool = POOL.lock();
    (pool.pages != 0).then(|| AllocatorStats {
        free_pages: (pool.pages - pool.allocated) as u64,
        allocated_pages: pool.allocated as u64,
        free_ranges: 0,
        managed_regions: pool.block_count,
    })
}

/// The main allocator, installed once boot setup is done, for big buffers
/// (image decoding, screenshots, ...).
static PRIMARY_FRAMES: TicketLock<Option<FrameAllocator>> = TicketLock::new(None);

pub fn install_primary(allocator: FrameAllocator) {
    *PRIMARY_FRAMES.lock() = Some(allocator);
}

/// A run of physical pages (identity-mapped) freed when dropped. Comes from
/// the primary allocator when installed, else from the global demand pool.
pub struct PageBuffer {
    address: u64,
    pages: u64,
    bytes: usize,
    from_primary: bool,
}

impl PageBuffer {
    /// Allocates at least `bytes` zeroed bytes.
    pub fn new(bytes: usize) -> Option<Self> {
        let pages = (bytes as u64).div_ceil(PAGE_SIZE).max(1);
        let primary = PRIMARY_FRAMES
            .lock()
            .as_mut()
            .and_then(|frames| frames.allocate_contiguous(pages, 1));
        let (frame, from_primary) = match primary {
            Some(frame) => (frame, true),
            None => (allocate_global_contiguous(pages, 1)?, false),
        };
        let address = frame.address();
        // SAFETY: the pages were just allocated and are identity-mapped RAM.
        unsafe {
            core::ptr::write_bytes(address as usize as *mut u8, 0, (pages * PAGE_SIZE) as usize)
        };
        Some(Self {
            address,
            pages,
            bytes,
            from_primary,
        })
    }

    pub fn len(&self) -> usize {
        self.bytes
    }

    #[cfg(feature = "boot-test")]
    pub fn physical(&self) -> u64 {
        self.address
    }

    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: owned, allocated pages.
        unsafe { core::slice::from_raw_parts(self.address as usize as *const u8, self.bytes) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: owned, allocated pages.
        unsafe { core::slice::from_raw_parts_mut(self.address as usize as *mut u8, self.bytes) }
    }
}

impl Drop for PageBuffer {
    fn drop(&mut self) {
        if self.from_primary {
            if let Some(frames) = PRIMARY_FRAMES.lock().as_mut() {
                frames.release_contiguous(self.address, self.pages);
            }
        } else {
            release_global_contiguous(self.address, self.pages);
        }
    }
}
