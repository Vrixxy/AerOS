use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch;
use crate::sync::TicketLock;

const COM1: u16 = 0x3f8;
const LOG_BYTES: usize = 16 * 1024;
static READY: AtomicBool = AtomicBool::new(false);
static LOG: TicketLock<LogRing> = TicketLock::new(LogRing::new());

/// Fixed-size ring holding the most recent serial output, read back by `dmesg`.
struct LogRing {
    data: [u8; LOG_BYTES],
    next: usize,
    stored: usize,
}

impl LogRing {
    const fn new() -> Self {
        Self {
            data: [0; LOG_BYTES],
            next: 0,
            stored: 0,
        }
    }

    fn push(&mut self, value: u8) {
        self.data[self.next] = value;
        self.next = (self.next + 1) % LOG_BYTES;
        self.stored = (self.stored + 1).min(LOG_BYTES);
    }

    /// Copies the newest `output.len()` bytes (fewer if less is stored),
    /// oldest first, and returns how many were written.
    fn tail(&self, output: &mut [u8]) -> usize {
        let count = output.len().min(self.stored);
        let start = (self.next + LOG_BYTES - count) % LOG_BYTES;
        for (offset, slot) in output[..count].iter_mut().enumerate() {
            *slot = self.data[(start + offset) % LOG_BYTES];
        }
        count
    }
}

fn record(value: u8) {
    if value == b'\r' {
        return;
    }
    if let Some(mut log) = LOG.try_lock() {
        log.push(value);
    }
}

/// The most recent kernel log bytes, oldest first.
pub fn log_tail(output: &mut [u8]) -> usize {
    LOG.lock().tail(output)
}

pub(crate) fn log_self_test() -> bool {
    let mut ring = LogRing::new();
    for index in 0..(LOG_BYTES + 100) {
        ring.push((index % 251) as u8);
    }
    let mut window = [0u8; 8];
    let count = ring.tail(&mut window);
    let expected_start = LOG_BYTES + 100 - 8;
    let wrap_ok = count == 8
        && window
            .iter()
            .enumerate()
            .all(|(offset, value)| *value == ((expected_start + offset) % 251) as u8);

    line("AEROS_LOG_PROBE");
    let mut recent = [0u8; 256];
    let recent_count = log_tail(&mut recent);
    let live_ok = recent[..recent_count]
        .windows(b"AEROS_LOG_PROBE\n".len())
        .any(|candidate| candidate == b"AEROS_LOG_PROBE\n");
    wrap_ok && live_ok
}

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
    record(value);
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

/// Marks the start of every serial line with the milliseconds since boot, to
/// find slow stages (`--features serial-stamps`).
#[cfg(feature = "serial-stamps")]
fn stamp(value: u8) {
    static AT_LINE_START: AtomicBool = AtomicBool::new(true);
    if value == b'\r' {
        return;
    }
    if value == b'\n' {
        AT_LINE_START.store(true, Ordering::Relaxed);
        return;
    }
    if AT_LINE_START.swap(false, Ordering::Relaxed) {
        let ms = crate::time::monotonic_nanoseconds() / 1_000_000;
        let mut digits = [0u8; 20];
        let mut count = 0;
        let mut rest = ms;
        loop {
            digits[count] = b'0' + (rest % 10) as u8;
            count += 1;
            rest /= 10;
            if rest == 0 {
                break;
            }
        }
        byte(b'[');
        while count > 0 {
            count -= 1;
            byte(digits[count]);
        }
        byte(b']');
        byte(b' ');
    }
}

pub fn text(value: &str) {
    for byte_value in value.bytes() {
        #[cfg(feature = "serial-stamps")]
        stamp(byte_value);
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
