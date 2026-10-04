//! Slab allocator for small kernel objects: seven size classes from 16 to
//! 1024 bytes, each served from 4 KiB pages taken from the kernel heap. A
//! page holds one class, its header and a free list; a page that becomes empty
//! goes back to the heap (one spare per class is kept), and `reclaim` returns
//! the spares under memory pressure. Freed objects are zeroed and a second
//! free of the same object is detected and refused.

use core::alloc::Layout;
use core::ptr::NonNull;

use crate::heap::HEAP;
use crate::sync::TicketLock;

const PAGE: usize = 4096;
const HEADER: usize = 64;
const CLASSES: usize = 7;
const SIZES: [usize; CLASSES] = [16, 32, 64, 128, 256, 512, 1024];
pub const MAX_SIZE: usize = 1024;
pub const MAX_ALIGN: usize = 64;
const MAGIC: u64 = 0x51ab_c0de_7a3f_9e15;
const POISON: u64 = 0xd1e5_f4ee_b10c_ba5e;

#[repr(C)]
struct Slab {
    magic: u64,
    class: usize,
    free: *mut u8,
    used: usize,
    capacity: usize,
    previous: *mut Slab,
    next: *mut Slab,
}

struct Cache {
    partial: *mut Slab,
    slabs: usize,
    in_use: usize,
    allocations: u64,
    pages_taken: u64,
    pages_returned: u64,
}

struct Slabs {
    caches: [Cache; CLASSES],
}

unsafe impl Send for Slabs {}

static SLABS: TicketLock<Slabs> = TicketLock::new(Slabs {
    caches: [const {
        Cache {
            partial: core::ptr::null_mut(),
            slabs: 0,
            in_use: 0,
            allocations: 0,
            pages_taken: 0,
            pages_returned: 0,
        }
    }; CLASSES],
});

fn class_of(layout: Layout) -> Option<usize> {
    if layout.size() > MAX_SIZE || layout.align() > MAX_ALIGN {
        return None;
    }
    let wanted = layout.size().max(layout.align()).max(1);
    SIZES.iter().position(|&size| size >= wanted)
}

impl Slabs {
    fn new_slab(&mut self, class: usize) -> Option<*mut Slab> {
        let page = HEAP.allocate(Layout::from_size_align(PAGE, PAGE).ok()?)?;
        let size = SIZES[class];
        let capacity = (PAGE - HEADER) / size;
        let slab = page.as_ptr().cast::<Slab>();
        let mut head: *mut u8 = core::ptr::null_mut();
        for index in (0..capacity).rev() {
            let object = unsafe { page.as_ptr().add(HEADER + index * size) };
            unsafe { object.cast::<*mut u8>().write(head) };
            head = object;
        }
        unsafe {
            slab.write(Slab {
                magic: MAGIC,
                class,
                free: head,
                used: 0,
                capacity,
                previous: core::ptr::null_mut(),
                next: core::ptr::null_mut(),
            });
        }
        let cache = &mut self.caches[class];
        cache.slabs += 1;
        cache.pages_taken += 1;
        Some(slab)
    }

    fn link(&mut self, class: usize, slab: *mut Slab) {
        let cache = &mut self.caches[class];
        unsafe {
            (*slab).previous = core::ptr::null_mut();
            (*slab).next = cache.partial;
            if !cache.partial.is_null() {
                (*cache.partial).previous = slab;
            }
        }
        cache.partial = slab;
    }

    fn unlink(&mut self, class: usize, slab: *mut Slab) {
        let cache = &mut self.caches[class];
        unsafe {
            let previous = (*slab).previous;
            let next = (*slab).next;
            if previous.is_null() {
                cache.partial = next;
            } else {
                (*previous).next = next;
            }
            if !next.is_null() {
                (*next).previous = previous;
            }
            (*slab).previous = core::ptr::null_mut();
            (*slab).next = core::ptr::null_mut();
        }
    }

    fn allocate(&mut self, class: usize) -> Option<NonNull<u8>> {
        let mut slab = self.caches[class].partial;
        if slab.is_null() {
            slab = self.new_slab(class)?;
            self.link(class, slab);
        }
        let object = unsafe {
            let object = (*slab).free;
            (*slab).free = object.cast::<*mut u8>().read();
            (*slab).used += 1;
            if (*slab).free.is_null() {
                self.unlink(class, slab);
            }
            object
        };
        unsafe { core::ptr::write_bytes(object, 0, SIZES[class]) };
        let cache = &mut self.caches[class];
        cache.in_use += 1;
        cache.allocations += 1;
        NonNull::new(object)
    }

    fn release(&mut self, pointer: NonNull<u8>, class: usize) -> bool {
        let address = pointer.as_ptr() as usize;
        let slab = (address & !(PAGE - 1)) as *mut Slab;
        let size = SIZES[class];
        let offset = address & (PAGE - 1);
        if offset < HEADER || !(offset - HEADER).is_multiple_of(size) {
            return false;
        }
        unsafe {
            if (*slab).magic != MAGIC || (*slab).class != class {
                return false;
            }
            if (offset - HEADER) / size >= (*slab).capacity {
                return false;
            }
            let object = pointer.as_ptr();
            if size >= 16 && object.add(8).cast::<u64>().read_unaligned() == POISON {
                return false;
            }
            core::ptr::write_bytes(object, 0, size);
            object.add(8).cast::<u64>().write_unaligned(POISON);
            let was_full = (*slab).free.is_null();
            object.cast::<*mut u8>().write((*slab).free);
            (*slab).free = object;
            (*slab).used -= 1;
            let cache = &mut self.caches[class];
            cache.in_use -= 1;
            if was_full {
                self.link(class, slab);
            }
            if (*slab).used == 0 && self.caches[class].slabs > 1 && self.spare_count(class) > 1 {
                self.drop_slab(class, slab);
            }
        }
        true
    }

    fn spare_count(&self, class: usize) -> usize {
        let mut count = 0;
        let mut slab = self.caches[class].partial;
        while !slab.is_null() {
            unsafe {
                if (*slab).used == 0 {
                    count += 1;
                }
                slab = (*slab).next;
            }
        }
        count
    }

    fn drop_slab(&mut self, class: usize, slab: *mut Slab) {
        self.unlink(class, slab);
        unsafe { (*slab).magic = 0 };
        if let Some(page) = NonNull::new(slab.cast::<u8>()) {
            HEAP.deallocate(page);
        }
        let cache = &mut self.caches[class];
        cache.slabs -= 1;
        cache.pages_returned += 1;
    }

    fn reclaim(&mut self) -> usize {
        let mut released = 0;
        for class in 0..CLASSES {
            let mut slab = self.caches[class].partial;
            while !slab.is_null() {
                let next = unsafe { (*slab).next };
                if unsafe { (*slab).used } == 0 {
                    self.drop_slab(class, slab);
                    released += 1;
                }
                slab = next;
            }
        }
        released
    }
}

/// Allocates an object of `layout`; layouts above 1 KiB or 64-byte alignment
/// go to the heap.
pub fn allocate(layout: Layout) -> Option<NonNull<u8>> {
    match class_of(layout) {
        Some(class) => SLABS.lock().allocate(class),
        None => HEAP.allocate(layout),
    }
}

/// Frees an object allocated with the same `layout`; false for a pointer that
/// is not a live object of that class (wrong size, foreign pointer, double
/// free).
pub fn deallocate(pointer: NonNull<u8>, layout: Layout) -> bool {
    match class_of(layout) {
        Some(class) => SLABS.lock().release(pointer, class),
        None => HEAP.deallocate(pointer),
    }
}

/// Returns empty pages to the heap.
pub fn reclaim() -> usize {
    SLABS.try_lock().map_or(0, |mut slabs| slabs.reclaim())
}

#[derive(Clone, Copy)]
pub struct ClassStats {
    pub size: usize,
    pub slabs: usize,
    pub in_use: usize,
    pub allocations: u64,
}

pub fn class_stats(class: usize) -> Option<ClassStats> {
    let slabs = SLABS.lock();
    let cache = slabs.caches.get(class)?;
    Some(ClassStats {
        size: SIZES[class],
        slabs: cache.slabs,
        in_use: cache.in_use,
        allocations: cache.allocations,
    })
}

pub fn class_count() -> usize {
    CLASSES
}

#[cfg(feature = "boot-test")]
pub fn self_test() -> bool {
    let heap_before = HEAP.stats();
    let mut pointers: [Option<(NonNull<u8>, Layout)>; 400] = [None; 400];
    let mut state = 0x853c_49e6_748f_ea9bu64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut ok = true;
    for (index, slot) in pointers.iter_mut().enumerate() {
        let size = [8, 16, 24, 48, 100, 200, 400, 832, 1024, 1][next() as usize % 10];
        let align = [1, 8, 16, 64][next() as usize % 4];
        let Ok(layout) = Layout::from_size_align(size, align) else {
            continue;
        };
        let Some(pointer) = allocate(layout) else {
            ok = false;
            continue;
        };
        ok &= (pointer.as_ptr() as usize).is_multiple_of(align);
        unsafe { core::ptr::write_bytes(pointer.as_ptr(), index as u8 | 1, size) };
        *slot = Some((pointer, layout));
    }
    for (index, slot) in pointers.iter().enumerate() {
        if let Some((pointer, layout)) = slot {
            let expected = index as u8 | 1;
            ok &= (0..layout.size())
                .all(|byte| unsafe { pointer.as_ptr().add(byte).read() } == expected);
        }
    }
    let used_peak = (0..CLASSES)
        .filter_map(class_stats)
        .map(|stats| stats.slabs)
        .sum::<usize>();
    for pass in 0..2 {
        for index in 0..pointers.len() {
            let pick = (index * 7 + 3) % pointers.len();
            if pass == 0 && pick % 2 == 1 {
                continue;
            }
            if let Some((pointer, layout)) = pointers[pick].take() {
                ok &= deallocate(pointer, layout);
            }
        }
    }
    for slot in pointers.iter_mut() {
        if let Some((pointer, layout)) = slot.take() {
            ok &= deallocate(pointer, layout);
        }
    }
    let probe = Layout::from_size_align(64, 64).unwrap();
    let (first, second) = (allocate(probe), allocate(probe));
    let distinct = matches!((first, second), (Some(a), Some(b)) if a != b);
    let mut double_free_refused = false;
    let mut wrong_class_refused = false;
    if let (Some(a), Some(b)) = (first, second) {
        double_free_refused = deallocate(a, probe) && !deallocate(a, probe);
        wrong_class_refused = !deallocate(b, Layout::from_size_align(512, 8).unwrap());
        ok &= deallocate(b, probe);
    }
    let foreign = HEAP.allocate(Layout::from_size_align(200, 8).unwrap());
    let foreign_refused = foreign.is_some_and(|pointer| {
        let refused = !deallocate(pointer, Layout::from_size_align(200, 8).unwrap());
        HEAP.deallocate(pointer);
        refused
    });
    let spares = reclaim();
    let heap_after = HEAP.stats();
    let in_use: usize = (0..CLASSES)
        .filter_map(class_stats)
        .map(|stats| stats.in_use)
        .sum();
    ok && used_peak > CLASSES
        && distinct
        && double_free_refused
        && wrong_class_refused
        && foreign_refused
        && spares > 0
        && in_use == 0
        && (0..CLASSES)
            .filter_map(class_stats)
            .all(|stats| stats.slabs == 0)
        && heap_after.free_bytes == heap_before.free_bytes
        && heap_after.active_allocations == heap_before.active_allocations
}
