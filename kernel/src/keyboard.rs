use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use crate::{acpi::AcpiInfo, arch, ioapic};

const DATA: u16 = 0x60;
const STATUS: u16 = 0x64;
const COMMAND: u16 = 0x64;
const QUEUE_SIZE: usize = 128;

static mut QUEUE: [u8; QUEUE_SIZE] = [0; QUEUE_SIZE];
static READ: AtomicUsize = AtomicUsize::new(0);
static WRITE: AtomicUsize = AtomicUsize::new(0);
static DROPPED: AtomicU64 = AtomicU64::new(0);
static READY: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
pub struct KeyboardReport {
    pub controller: bool,
    pub routed: bool,
    pub queue_bytes: usize,
    pub dropped: u64,
    pub verified: bool,
}

pub fn initialize(acpi: &AcpiInfo, destination: u32) -> KeyboardReport {
    READY.store(false, Ordering::Release);
    READ.store(0, Ordering::Release);
    WRITE.store(0, Ordering::Release);
    DROPPED.store(0, Ordering::Release);
    let controller = configure_controller();
    let routed = ioapic::route_legacy_irq(acpi, 1, destination, 52);
    READY.store(controller && routed, Ordering::Release);
    report(controller, routed)
}

pub fn handle_interrupt() {
    for _ in 0..QUEUE_SIZE {
        let status = unsafe { arch::inb(STATUS) };
        if status & 1 == 0 {
            return;
        }
        let byte = unsafe { arch::inb(DATA) };
        if status & 0x20 != 0 {
            crate::mouse::feed_byte(byte);
        } else {
            push_scancode(byte);
        }
    }
}

pub fn push_scancode(byte: u8) {
    let write = WRITE.load(Ordering::Relaxed);
    let next = (write + 1) % QUEUE_SIZE;
    if next == READ.load(Ordering::Acquire) {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    unsafe {
        core::ptr::write_volatile(core::ptr::addr_of_mut!(QUEUE[write]), byte);
    }
    WRITE.store(next, Ordering::Release);
}

pub fn has_pending() -> bool {
    READY.load(Ordering::Acquire) && READ.load(Ordering::Relaxed) != WRITE.load(Ordering::Acquire)
}

pub fn pop_scancode() -> Option<u8> {
    if !READY.load(Ordering::Acquire) {
        return None;
    }
    let read = READ.load(Ordering::Relaxed);
    if read == WRITE.load(Ordering::Acquire) {
        return None;
    }
    let byte = unsafe { core::ptr::read_volatile(core::ptr::addr_of!(QUEUE[read])) };
    READ.store((read + 1) % QUEUE_SIZE, Ordering::Release);
    Some(byte)
}

fn report(controller: bool, routed: bool) -> KeyboardReport {
    let read = READ.load(Ordering::Acquire);
    let write = WRITE.load(Ordering::Acquire);
    KeyboardReport {
        controller,
        routed,
        queue_bytes: if write >= read {
            write - read
        } else {
            QUEUE_SIZE - read + write
        },
        dropped: DROPPED.load(Ordering::Acquire),
        verified: controller && routed,
    }
}

fn configure_controller() -> bool {
    if !wait_write() {
        return false;
    }
    unsafe {
        arch::outb(COMMAND, 0xad);
    }
    while unsafe { arch::inb(STATUS) } & 1 != 0 {
        unsafe {
            arch::inb(DATA);
        }
    }
    if !wait_write() {
        return false;
    }
    unsafe {
        arch::outb(COMMAND, 0x20);
    }
    if !wait_read() {
        return false;
    }
    let configuration = unsafe { arch::inb(DATA) } | 1;
    if !wait_write() {
        return false;
    }
    unsafe {
        arch::outb(COMMAND, 0x60);
    }
    if !wait_write() {
        return false;
    }
    unsafe {
        arch::outb(DATA, configuration);
    }
    if !wait_write() {
        return false;
    }
    unsafe {
        arch::outb(COMMAND, 0xae);
    }
    true
}

fn wait_write() -> bool {
    for _ in 0..100_000 {
        if unsafe { arch::inb(STATUS) } & 2 == 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

fn wait_read() -> bool {
    for _ in 0..100_000 {
        if unsafe { arch::inb(STATUS) } & 1 != 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}
