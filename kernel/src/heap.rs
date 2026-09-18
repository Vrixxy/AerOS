use core::alloc::Layout;
use core::ptr::NonNull;

use crate::sync::TicketLock;

const HEADER_CHECK: usize = 0xa3e0_7c51_96d2_4bf8;

#[repr(C)]
struct FreeNode {
    size: usize,
    next: *mut FreeNode,
}

#[repr(C)]
struct AllocationHeader {
    block_start: usize,
    block_size: usize,
    check: usize,
}

struct HeapState {
    head: *mut FreeNode,
    base: usize,
    end: usize,
    total: usize,
    free: usize,
    active: usize,
}

unsafe impl Send for HeapState {}

impl HeapState {
    const fn empty() -> Self {
        Self {
            head: core::ptr::null_mut(),
            base: 0,
            end: 0,
            total: 0,
            free: 0,
            active: 0,
        }
    }

    unsafe fn initialize(&mut self, base: usize, size: usize) -> bool {
        let start = align_up(base, core::mem::align_of::<FreeNode>());
        let Some(raw_end) = base.checked_add(size) else {
            return false;
        };
        let end = align_down(raw_end, core::mem::align_of::<FreeNode>());
        if end <= start || end - start < core::mem::size_of::<FreeNode>() {
            return false;
        }
        self.head = core::ptr::null_mut();
        self.base = start;
        self.end = end;
        self.total = end - start;
        self.free = end - start;
        self.active = 0;
        unsafe {
            self.insert_region(start, end - start);
        }
        true
    }

    unsafe fn allocate(&mut self, layout: Layout) -> Option<NonNull<u8>> {
        let requested = layout.size().max(1);
        let alignment = layout
            .align()
            .max(core::mem::align_of::<AllocationHeader>());
        let header_size = core::mem::size_of::<AllocationHeader>();
        let node_size = core::mem::size_of::<FreeNode>();
        let mut previous: *mut FreeNode = core::ptr::null_mut();
        let mut current = self.head;
        while !current.is_null() {
            let region_start = current as usize;
            let region_size = unsafe { (*current).size };
            let region_end = region_start.checked_add(region_size)?;
            let user = align_up(region_start.checked_add(header_size)?, alignment);
            let header_address = user.checked_sub(header_size)?;
            let requested_end = user.checked_add(requested)?;
            if requested_end > region_end {
                previous = current;
                current = unsafe { (*current).next };
                continue;
            }
            let prefix = header_address - region_start;
            let block_start = if prefix >= node_size {
                header_address
            } else {
                region_start
            };
            let suffix = region_end - requested_end;
            let block_end = if suffix >= node_size {
                requested_end
            } else {
                region_end
            };
            let next = unsafe { (*current).next };
            if previous.is_null() {
                self.head = next;
            } else {
                unsafe {
                    (*previous).next = next;
                }
            }
            if block_start > region_start {
                unsafe {
                    self.insert_region(region_start, block_start - region_start);
                }
            }
            if block_end < region_end {
                unsafe {
                    self.insert_region(block_end, region_end - block_end);
                }
            }
            let block_size = block_end - block_start;
            let header = header_address as *mut AllocationHeader;
            unsafe {
                core::ptr::write(
                    header,
                    AllocationHeader {
                        block_start,
                        block_size,
                        check: user ^ block_start ^ block_size ^ HEADER_CHECK,
                    },
                );
            }
            self.free = self.free.saturating_sub(block_size);
            self.active = self.active.saturating_add(1);
            return NonNull::new(user as *mut u8);
        }
        None
    }

    unsafe fn deallocate(&mut self, pointer: NonNull<u8>) -> bool {
        let user = pointer.as_ptr() as usize;
        let header_size = core::mem::size_of::<AllocationHeader>();
        if user < self.base.saturating_add(header_size) || user >= self.end {
            return false;
        }
        let header_address = user - header_size;
        let header = unsafe { &mut *(header_address as *mut AllocationHeader) };
        let expected = user ^ header.block_start ^ header.block_size ^ HEADER_CHECK;
        if header.check != expected
            || header.block_start < self.base
            || header.block_size < header_size
            || header
                .block_start
                .checked_add(header.block_size)
                .is_none_or(|end| end > self.end)
        {
            return false;
        }
        let block_start = header.block_start;
        let block_size = header.block_size;
        header.check = 0;
        if unsafe { self.overlaps_free(block_start, block_size) } {
            return false;
        }
        unsafe {
            self.insert_region(block_start, block_size);
        }
        self.free = self.free.saturating_add(block_size);
        self.active = self.active.saturating_sub(1);
        true
    }

    unsafe fn insert_region(&mut self, start: usize, size: usize) {
        let node = start as *mut FreeNode;
        unsafe {
            core::ptr::write(
                node,
                FreeNode {
                    size,
                    next: core::ptr::null_mut(),
                },
            );
        }
        if self.head.is_null() || start < self.head as usize {
            unsafe {
                (*node).next = self.head;
            }
            self.head = node;
        } else {
            let mut current = self.head;
            unsafe {
                while !(*current).next.is_null() && ((*current).next as usize) < start {
                    current = (*current).next;
                }
                (*node).next = (*current).next;
                (*current).next = node;
            }
        }
        unsafe {
            self.coalesce();
        }
    }

    unsafe fn coalesce(&mut self) {
        let mut current = self.head;
        while !current.is_null() {
            let next = unsafe { (*current).next };
            if next.is_null() {
                break;
            }
            let current_end = (current as usize).saturating_add(unsafe { (*current).size });
            if current_end == next as usize {
                unsafe {
                    (*current).size = (*current).size.saturating_add((*next).size);
                    (*current).next = (*next).next;
                }
            } else {
                current = next;
            }
        }
    }

    unsafe fn overlaps_free(&self, start: usize, size: usize) -> bool {
        let Some(end) = start.checked_add(size) else {
            return true;
        };
        let mut current = self.head;
        while !current.is_null() {
            let free_start = current as usize;
            let free_end = free_start.saturating_add(unsafe { (*current).size });
            if start < free_end && end > free_start {
                return true;
            }
            current = unsafe { (*current).next };
        }
        false
    }
}

#[derive(Clone, Copy)]
pub struct HeapStats {
    pub total_bytes: usize,
    pub free_bytes: usize,
    pub active_allocations: usize,
}

pub struct KernelHeap {
    state: TicketLock<HeapState>,
}

impl KernelHeap {
    pub const fn new() -> Self {
        Self {
            state: TicketLock::new(HeapState::empty()),
        }
    }

    pub unsafe fn initialize(&self, base: usize, size: usize) -> bool {
        unsafe { self.state.lock().initialize(base, size) }
    }

    pub fn allocate(&self, layout: Layout) -> Option<NonNull<u8>> {
        unsafe { self.state.lock().allocate(layout) }
    }

    pub fn deallocate(&self, pointer: NonNull<u8>) -> bool {
        unsafe { self.state.lock().deallocate(pointer) }
    }

    pub fn stats(&self) -> HeapStats {
        let state = self.state.lock();
        HeapStats {
            total_bytes: state.total,
            free_bytes: state.free,
            active_allocations: state.active,
        }
    }

    pub fn self_test(&self) -> bool {
        let before = self.stats();
        let Some(first) = self.allocate(Layout::from_size_align(37, 8).unwrap()) else {
            return false;
        };
        let Some(second) = self.allocate(Layout::from_size_align(4096, 4096).unwrap()) else {
            return false;
        };
        let Some(third) = self.allocate(Layout::from_size_align(113, 64).unwrap()) else {
            return false;
        };
        if first.as_ptr() as usize & 7 != 0
            || second.as_ptr() as usize & 4095 != 0
            || third.as_ptr() as usize & 63 != 0
        {
            return false;
        }
        unsafe {
            core::ptr::write_bytes(first.as_ptr(), 0xa3, 37);
            core::ptr::write_bytes(second.as_ptr(), 0x5c, 4096);
            core::ptr::write_bytes(third.as_ptr(), 0x71, 113);
        }
        let released = self.deallocate(second) && self.deallocate(first) && self.deallocate(third);
        let after = self.stats();
        released
            && after.total_bytes == before.total_bytes
            && after.free_bytes == before.free_bytes
            && after.active_allocations == 0
    }
}

pub static HEAP: KernelHeap = KernelHeap::new();

fn align_up(value: usize, alignment: usize) -> usize {
    value.saturating_add(alignment - 1) & !(alignment - 1)
}

fn align_down(value: usize, alignment: usize) -> usize {
    value & !(alignment - 1)
}
