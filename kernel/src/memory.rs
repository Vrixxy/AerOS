use crate::sync::TicketLock;

const PAGE_SIZE: u64 = 4096;
const LOW_MEMORY_LIMIT: u64 = 0x10_0000;
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
}

impl FrameAllocator {
    pub fn from_range(start: u64, pages: u64) -> Option<Self> {
        if pages == 0 || start & (PAGE_SIZE - 1) != 0 {
            return None;
        }
        let end = start.checked_add(pages.checked_mul(PAGE_SIZE)?)?;
        let range = Range { start, end };
        let mut allocator = Self {
            managed: [Range::EMPTY; MAX_MEMORY_REGIONS],
            managed_count: 1,
            free: [Range::EMPTY; MAX_FREE_RANGES],
            free_count: 1,
            free_pages: pages,
            allocated_pages: 0,
        };
        allocator.managed[0] = range;
        allocator.free[0] = range;
        Some(allocator)
    }

    pub fn from_map(map: &BootMemoryMap) -> Self {
        let mut allocator = Self {
            managed: [Range::EMPTY; MAX_MEMORY_REGIONS],
            managed_count: 0,
            free: [Range::EMPTY; MAX_FREE_RANGES],
            free_count: 0,
            free_pages: 0,
            allocated_pages: 0,
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
        if !self.release(first) {
            return false;
        }
        let reused = self.allocate();
        let first_stage = reused == Some(first) && self.release(second) && self.release(first);
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

static GLOBAL_FRAMES: TicketLock<Option<FrameAllocator>> = TicketLock::new(None);

pub fn install_global(allocator: FrameAllocator) {
    *GLOBAL_FRAMES.lock() = Some(allocator);
}

pub fn with_frames<R>(f: impl FnOnce(&mut FrameAllocator) -> R) -> Option<R> {
    GLOBAL_FRAMES.lock().as_mut().map(f)
}

pub fn allocate_global() -> Option<PhysFrame> {
    with_frames(FrameAllocator::allocate).flatten()
}

pub fn release_global_address(address: u64) -> bool {
    with_frames(|frames| frames.release_contiguous(address, 1)).unwrap_or(false)
}

pub fn allocate_global_contiguous(pages: u64, alignment_pages: u64) -> Option<PhysFrame> {
    with_frames(|frames| frames.allocate_contiguous(pages, alignment_pages)).flatten()
}

pub fn release_global_contiguous(address: u64, pages: u64) -> bool {
    with_frames(|frames| frames.release_contiguous(address, pages)).unwrap_or(false)
}

pub fn global_stats() -> Option<AllocatorStats> {
    with_frames(|frames| frames.stats())
}
