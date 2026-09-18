use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch;

const COM1: u16 = 0x3f8;
static READY: AtomicBool = AtomicBool::new(false);

pub fn init() {
    unsafe {
        arch::outb(COM1 + 1, 0x00);
        arch::outb(COM1 + 3, 0x80);
        arch::outb(COM1, 0x03);
        arch::outb(COM1 + 1, 0x00);
        arch::outb(COM1 + 3, 0x03);
        arch::outb(COM1 + 2, 0xc7);
        arch::outb(COM1 + 4, 0x0b);
    }
    READY.store(true, Ordering::Release);
}

pub fn byte(value: u8) {
    if !READY.load(Ordering::Acquire) {
        return;
    }
    unsafe {
        let mut spins = 0usize;
        while arch::inb(COM1 + 5) & 0x20 == 0 && spins < 1_000_000 {
            core::hint::spin_loop();
            spins += 1;
        }
        arch::outb(COM1, value);
    }
}

pub fn text(value: &str) {
    for byte_value in value.bytes() {
        if byte_value == b'\n' {
            byte(b'\r');
        }
        byte(byte_value);
    }
}

pub fn line(value: &str) {
    text(value);
    text("\n");
}

pub fn format(arguments: fmt::Arguments<'_>) {
    let _ = SerialWriter.write_fmt(arguments);
}

struct SerialWriter;

impl Write for SerialWriter {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        text(value);
        Ok(())
    }
}
