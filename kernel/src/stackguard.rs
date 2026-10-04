//! Support for the compiler's stack protector. Functions with buffers or
//! address-taken locals keep a copy of `__security_cookie` (mixed with the
//! stack pointer) below their return address and compare it before
//! returning; on a mismatch they call `__security_check_cookie`, which stops
//! the machine instead of returning into attacker-chosen code.

use core::arch::{naked_asm, x86_64};

use crate::{arch, serial};

#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
pub static mut __security_cookie: usize = 0x2b99_2ddf_a232;

#[cfg(feature = "boot-test")]
const DEFAULT_COOKIE: usize = 0x2b99_2ddf_a232;

#[cfg(feature = "boot-test")]
static EXPECT_FAILURE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
#[cfg(feature = "boot-test")]
static FAILURES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Draws a fresh cookie. Must run before any protected frame that will
/// return later is entered; `kernel_entry` does it first and never returns.
pub fn randomize() {
    let mut value = 0u64;
    // SAFETY: CPUID and RDRAND/RDTSC are unprivileged instructions.
    unsafe {
        if x86_64::__cpuid(1).ecx & (1 << 30) != 0 {
            for _ in 0..8 {
                let mut word = 0u64;
                if x86_64::_rdrand64_step(&mut word) == 1 {
                    value = word;
                    break;
                }
            }
        }
        value ^= x86_64::_rdtsc().rotate_left(17);
    }
    // The low byte stays zero so a string copy cannot run through the cookie.
    let cookie = (value as usize & !0xff) | 0x100;
    // SAFETY: single-threaded at this point of start-up.
    unsafe { core::ptr::write_volatile(&raw mut __security_cookie, cookie) };
}

extern "C" fn smashed() {
    #[cfg(feature = "boot-test")]
    if EXPECT_FAILURE.load(core::sync::atomic::Ordering::SeqCst) {
        FAILURES.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        return;
    }
    serial::line("AEROS_STACK_SMASH");
    arch::halt_forever();
}

#[unsafe(no_mangle)]
#[unsafe(naked)]
#[allow(non_snake_case)]
pub unsafe extern "C" fn __security_check_cookie() {
    naked_asm!(
        "cmp rcx, qword ptr [rip + __security_cookie]",
        "jne 2f",
        "ret",
        "2:",
        "push rax",
        "push rcx",
        "push rdx",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "sub rsp, 40",
        "call {smashed}",
        "add rsp, 40",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rdx",
        "pop rcx",
        "pop rax",
        "ret",
        smashed = sym smashed,
    )
}

#[cfg(feature = "boot-test")]
pub struct TestReport {
    pub cookie_random: bool,
    pub instrumented: bool,
    pub intact_passes: bool,
    pub smash_detected: bool,
}

/// Counts direct calls to `__security_check_cookie` in the loaded image.
#[cfg(feature = "boot-test")]
fn count_call_sites() -> usize {
    let target: usize;
    // SAFETY: loads the address of a symbol of this image.
    unsafe {
        core::arch::asm!(
            "lea {}, [rip + __security_check_cookie]",
            out(reg) target,
            options(nomem, nostack)
        );
    }
    let mut base = target & !0xfff;
    // SAFETY: walks back through this image's own mapped pages to its header.
    unsafe {
        while core::ptr::read_volatile(base as *const u16) != 0x5a4d {
            base -= 0x1000;
        }
        let header = base + core::ptr::read_volatile((base + 0x3c) as *const u32) as usize;
        let size = core::ptr::read_volatile((header + 0x50) as *const u32) as usize;
        let mut count = 0;
        for at in base..base + size.saturating_sub(5) {
            if core::ptr::read_volatile(at as *const u8) == 0xe8 {
                let relative = core::ptr::read_unaligned((at + 1) as *const i32) as isize;
                if (at as isize + 5).wrapping_add(relative) as usize == target {
                    count += 1;
                }
            }
        }
        count
    }
}

/// Whether the code at `address` loads `__security_cookie` (a RIP-relative `mov`).
#[cfg(feature = "boot-test")]
fn loads_cookie(address: usize, length: usize) -> bool {
    let cookie = &raw const __security_cookie as usize;
    // SAFETY: reads the first bytes of a function of this image.
    let code = unsafe { core::slice::from_raw_parts(address as *const u8, length) };
    (0..length.saturating_sub(7)).any(|at| {
        matches!(code[at], 0x48 | 0x4c) && code[at + 1] == 0x8b && code[at + 2] & 0xc7 == 0x05 && {
            let displacement =
                i32::from_le_bytes([code[at + 3], code[at + 4], code[at + 5], code[at + 6]]);
            (address + at + 7).wrapping_add_signed(displacement as isize) == cookie
        }
    })
}

/// A function with a buffer whose address escapes: the compiler must give it a cookie.
#[cfg(feature = "boot-test")]
#[inline(never)]
fn guarded(seed: u8) -> u8 {
    let mut buffer = [seed; 64];
    let slot = core::hint::black_box(&mut buffer);
    slot[3] = slot[3].wrapping_add(1);
    slot[seed as usize % 64]
}

/// Checks the cookie was randomised, that compiled code really uses it
/// (a buffer-holding function loads the cookie and the image calls the
/// check), and that the check passes the right value and catches a wrong one.
#[cfg(feature = "boot-test")]
pub fn self_test() -> TestReport {
    use core::sync::atomic::Ordering;
    guarded(7);
    let instrumented = loads_cookie(guarded as *const () as usize, 256) && count_call_sites() > 0;
    // SAFETY: a plain read of the cookie.
    let cookie = unsafe { core::ptr::read_volatile(&raw const __security_cookie) };
    let call = |value: usize| {
        // SAFETY: the check preserves every register it is not given.
        unsafe {
            core::arch::asm!("call __security_check_cookie", in("rcx") value, clobber_abi("C"));
        }
    };
    EXPECT_FAILURE.store(true, Ordering::SeqCst);
    let before = FAILURES.load(Ordering::SeqCst);
    call(cookie);
    let intact_passes = FAILURES.load(Ordering::SeqCst) == before;
    call(cookie ^ 0x1000);
    let smash_detected = FAILURES.load(Ordering::SeqCst) == before + 1;
    EXPECT_FAILURE.store(false, Ordering::SeqCst);
    TestReport {
        cookie_random: cookie != DEFAULT_COOKIE && cookie != 0,
        instrumented,
        intact_passes,
        smash_detected,
    }
}
