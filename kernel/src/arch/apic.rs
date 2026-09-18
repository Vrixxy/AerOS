use core::arch::asm;
use core::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

use crate::arch;

const APIC_BASE_MSR: u32 = 0x1b;
const APIC_GLOBAL_ENABLE: u64 = 1 << 11;
const APIC_X2_ENABLE: u64 = 1 << 10;
const APIC_ADDRESS_MASK: u64 = 0x000f_ffff_ffff_f000;
const ID: u32 = 0x20;
const VERSION: u32 = 0x30;
const EOI: u32 = 0xb0;
const SPURIOUS: u32 = 0xf0;
const LVT_TIMER: u32 = 0x320;
const INITIAL_COUNT: u32 = 0x380;
const CURRENT_COUNT: u32 = 0x390;
const DIVIDE: u32 = 0x3e0;
const ICR_LOW: u32 = 0x300;
const ICR_HIGH: u32 = 0x310;
const TIMER_VECTOR: u32 = 48;

static MODE: AtomicU8 = AtomicU8::new(0);
static BASE: AtomicU64 = AtomicU64::new(0);
static COUNTS_PER_100HZ: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy)]
pub struct ApicReport {
    pub enabled: bool,
    pub x2apic: bool,
    pub id: u32,
    pub version: u8,
    pub max_lvt: u8,
    pub counts_per_100hz: u32,
    pub test_ticks: u64,
    pub verified: bool,
}

pub fn initialize(acpi_address: u64) -> ApicReport {
    if acpi_address == 0 {
        return empty_report();
    }
    let mut base_msr = read_msr(APIC_BASE_MSR);
    if base_msr & APIC_GLOBAL_ENABLE == 0 {
        base_msr |= APIC_GLOBAL_ENABLE;
        unsafe {
            write_msr(APIC_BASE_MSR, base_msr);
        }
    }
    let enabled = read_msr(APIC_BASE_MSR) & APIC_GLOBAL_ENABLE != 0;
    let x2apic = base_msr & APIC_X2_ENABLE != 0;
    let address = if x2apic {
        base_msr & APIC_ADDRESS_MASK
    } else {
        acpi_address
    };
    BASE.store(address, Ordering::Release);
    MODE.store(if x2apic { 2 } else { 1 }, Ordering::Release);
    let spurious = read(SPURIOUS);
    write(SPURIOUS, spurious | (1 << 8) | 0xff);
    let id = if x2apic { read(ID) } else { read(ID) >> 24 };
    let version_register = read(VERSION);
    let version = version_register as u8;
    let max_lvt = ((version_register >> 16) & 0xff) as u8;
    write(DIVIDE, 3);
    write(LVT_TIMER, (1 << 16) | TIMER_VECTOR);
    write(INITIAL_COUNT, u32::MAX);
    let reference_ticks = crate::arch::interrupts::timer_self_test(100, 5).unwrap_or(0);
    let elapsed = u32::MAX.wrapping_sub(read(CURRENT_COUNT));
    write(INITIAL_COUNT, 0);
    let counts = (elapsed as u64).checked_div(reference_ticks).unwrap_or(0) as u32;
    COUNTS_PER_100HZ.store(counts, Ordering::Release);
    crate::arch::interrupts::reset_local_timer_ticks();
    if counts != 0 {
        write(DIVIDE, 3);
        write(LVT_TIMER, (1 << 17) | TIMER_VECTOR);
        write(INITIAL_COUNT, counts);
        unsafe {
            asm!("sti", options(nomem, nostack, preserves_flags));
        }
        while crate::arch::interrupts::local_timer_ticks() < 4 {
            unsafe {
                asm!("hlt", options(nomem, nostack));
            }
        }
        arch::disable_interrupts();
        write(LVT_TIMER, (1 << 16) | TIMER_VECTOR);
        write(INITIAL_COUNT, 0);
    }
    let test_ticks = crate::arch::interrupts::local_timer_ticks();
    let verified = enabled
        && version != 0
        && max_lvt >= 4
        && counts != 0
        && test_ticks >= 4
        && read(SPURIOUS) & (1 << 8) != 0;
    ApicReport {
        enabled,
        x2apic,
        id,
        version,
        max_lvt,
        counts_per_100hz: counts,
        test_ticks,
        verified,
    }
}

pub fn start_timer(frequency: u32) -> bool {
    let reference = COUNTS_PER_100HZ.load(Ordering::Acquire);
    if reference == 0 || !(20..=1000).contains(&frequency) {
        return false;
    }
    let count = ((reference as u64 * 100) / frequency as u64).max(1) as u32;
    write(DIVIDE, 3);
    write(LVT_TIMER, (1 << 17) | TIMER_VECTOR);
    write(INITIAL_COUNT, count);
    unsafe {
        asm!("sti", options(nomem, nostack, preserves_flags));
    }
    true
}

pub fn stop_timer() {
    arch::disable_interrupts();
    write(LVT_TIMER, (1 << 16) | TIMER_VECTOR);
    write(INITIAL_COUNT, 0);
}

pub fn end_interrupt() {
    write(EOI, 0);
}

pub fn current_id() -> u32 {
    if MODE.load(Ordering::Acquire) == 2 {
        read(ID)
    } else {
        read(ID) >> 24
    }
}

pub fn initialize_secondary() -> u32 {
    let mut base = read_msr(APIC_BASE_MSR);
    if base & APIC_GLOBAL_ENABLE == 0 {
        base |= APIC_GLOBAL_ENABLE;
        unsafe {
            write_msr(APIC_BASE_MSR, base);
        }
    }
    let spurious = read(SPURIOUS);
    write(SPURIOUS, spurious | (1 << 8) | 0xff);
    write(LVT_TIMER, (1 << 16) | TIMER_VECTOR);
    write(INITIAL_COUNT, 0);
    current_id()
}

pub fn start_processor(destination: u32, trampoline: u64) -> bool {
    if trampoline == 0 || trampoline >= 0x10_0000 || trampoline & 0xfff != 0 {
        return false;
    }
    if MODE.load(Ordering::Acquire) == 1 && destination > u8::MAX as u32 {
        return false;
    }
    if !send_ipi(destination, 0xc500) {
        return false;
    }
    delay(10_000_000);
    if !send_ipi(destination, 0x8500) {
        return false;
    }
    delay(200_000);
    let startup = 0x600 | (trampoline >> 12) as u32;
    if !send_ipi(destination, startup) {
        return false;
    }
    delay(200_000);
    send_ipi(destination, startup)
}

pub fn send_fixed(destination: u32, vector: u8) -> bool {
    if vector < 32 || vector == 255 {
        return false;
    }
    send_ipi(destination, vector as u32)
}

fn send_ipi(destination: u32, command: u32) -> bool {
    if !wait_delivery() {
        return false;
    }
    match MODE.load(Ordering::Acquire) {
        1 => {
            write(ICR_HIGH, destination << 24);
            write(ICR_LOW, command);
        }
        2 => unsafe {
            write_msr(0x830, ((destination as u64) << 32) | command as u64);
        },
        _ => return false,
    }
    wait_delivery()
}

fn wait_delivery() -> bool {
    for _ in 0..1_000_000 {
        if read(ICR_LOW) & (1 << 12) == 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

fn delay(nanoseconds: u64) {
    let start = crate::time::monotonic_nanoseconds();
    while crate::time::monotonic_nanoseconds().wrapping_sub(start) < nanoseconds {
        core::hint::spin_loop();
    }
}

fn empty_report() -> ApicReport {
    ApicReport {
        enabled: false,
        x2apic: false,
        id: 0,
        version: 0,
        max_lvt: 0,
        counts_per_100hz: 0,
        test_ticks: 0,
        verified: false,
    }
}

fn read(register: u32) -> u32 {
    match MODE.load(Ordering::Acquire) {
        1 => unsafe {
            core::ptr::read_volatile(
                (BASE.load(Ordering::Acquire) + register as u64) as usize as *const u32,
            )
        },
        2 => read_msr(0x800 + register / 16) as u32,
        _ => 0,
    }
}

fn write(register: u32, value: u32) {
    match MODE.load(Ordering::Acquire) {
        1 => unsafe {
            core::ptr::write_volatile(
                (BASE.load(Ordering::Acquire) + register as u64) as usize as *mut u32,
                value,
            );
        },
        2 => unsafe {
            write_msr(0x800 + register / 16, value as u64);
        },
        _ => {}
    }
}

fn read_msr(register: u32) -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") register,
            out("eax") low,
            out("edx") high,
            options(nomem, nostack, preserves_flags)
        );
    }
    low as u64 | (high as u64) << 32
}

unsafe fn write_msr(register: u32, value: u64) {
    unsafe {
        asm!(
            "wrmsr",
            in("ecx") register,
            in("eax") value as u32,
            in("edx") (value >> 32) as u32,
            options(nostack, preserves_flags)
        );
    }
}
