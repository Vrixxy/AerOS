use core::arch::{asm, x86_64::__cpuid_count};
use core::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

use super::CpuInfo;

const CR0_MP: u64 = 1 << 1;
const CR0_EM: u64 = 1 << 2;
const CR0_TS: u64 = 1 << 3;
const CR0_NE: u64 = 1 << 5;
const CR4_OSFXSR: u64 = 1 << 9;
const CR4_OSXMMEXCPT: u64 = 1 << 10;
const CR4_OSXSAVE: u64 = 1 << 18;
const MODE_DISABLED: u8 = 0;
const MODE_FXSAVE: u8 = 1;
const MODE_XSAVE: u8 = 2;
const CONTEXT_ALIGNMENT: usize = 64;

static CONTEXT_MODE: AtomicU8 = AtomicU8::new(MODE_DISABLED);
static CONTEXT_BYTES: AtomicU32 = AtomicU32::new(0);
static CONTEXT_MASK: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
pub struct FpuReport {
    pub fxsave: bool,
    pub sse: bool,
    pub xsave: bool,
    pub avx: bool,
    pub xcr0: u64,
    pub state_bytes: u32,
    pub simd: bool,
    pub verified: bool,
}

pub fn initialize(cpu: &CpuInfo) -> FpuReport {
    CONTEXT_MODE.store(MODE_DISABLED, Ordering::Release);
    CONTEXT_BYTES.store(0, Ordering::Release);
    CONTEXT_MASK.store(0, Ordering::Release);
    if !cpu.fxsave || !cpu.sse {
        return empty_report(cpu);
    }
    let mut cr0: u64;
    let mut cr4: u64;
    unsafe {
        asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
        cr0 = (cr0 | CR0_MP | CR0_NE) & !(CR0_EM | CR0_TS);
        asm!("mov cr0, {}", in(reg) cr0, options(nostack, preserves_flags));
        asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
        cr4 |= CR4_OSFXSR | CR4_OSXMMEXCPT;
        if cpu.xsave {
            cr4 |= CR4_OSXSAVE;
        }
        asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
        asm!("fninit", options(nomem, nostack));
    }
    let xcr0 = if cpu.xsave {
        let requested = 3 | if cpu.avx { 4 } else { 0 };
        unsafe {
            write_xcr0(requested);
        }
        read_xcr0()
    } else {
        0
    };
    let state_bytes = if cpu.xsave {
        __cpuid_count(0x0d, 0).ebx
    } else {
        512
    };
    let simd = simd_self_test();
    let control_valid = cr0 & (CR0_EM | CR0_TS) == 0
        && cr0 & (CR0_MP | CR0_NE) == CR0_MP | CR0_NE
        && cr4 & (CR4_OSFXSR | CR4_OSXMMEXCPT) == CR4_OSFXSR | CR4_OSXMMEXCPT;
    let xsave_valid = !cpu.xsave
        || cr4 & CR4_OSXSAVE != 0
            && xcr0 & 3 == 3
            && (!cpu.avx || xcr0 & 4 != 0)
            && state_bytes >= 512;
    let verified = control_valid && xsave_valid && simd;
    if verified {
        CONTEXT_BYTES.store(state_bytes, Ordering::Release);
        CONTEXT_MASK.store(if cpu.xsave { xcr0 } else { 0 }, Ordering::Release);
        CONTEXT_MODE.store(
            if cpu.xsave { MODE_XSAVE } else { MODE_FXSAVE },
            Ordering::Release,
        );
    }
    FpuReport {
        fxsave: cpu.fxsave,
        sse: cpu.sse,
        xsave: cpu.xsave,
        avx: cpu.avx,
        xcr0,
        state_bytes,
        simd,
        verified,
    }
}

pub fn context_state_bytes() -> usize {
    CONTEXT_BYTES.load(Ordering::Acquire) as usize
}

pub const fn context_alignment() -> usize {
    CONTEXT_ALIGNMENT
}

pub fn avx_context_enabled() -> bool {
    CONTEXT_MASK.load(Ordering::Acquire) & 4 != 0
}

pub unsafe fn save_context(pointer: *mut u8) -> bool {
    if pointer.is_null() || !(pointer as usize).is_multiple_of(CONTEXT_ALIGNMENT) {
        return false;
    }
    match CONTEXT_MODE.load(Ordering::Acquire) {
        MODE_XSAVE => {
            let mask = CONTEXT_MASK.load(Ordering::Acquire);
            unsafe {
                asm!(
                    "xsave64 [{}]",
                    in(reg) pointer,
                    in("eax") mask as u32,
                    in("edx") (mask >> 32) as u32,
                    options(nostack)
                );
            }
            true
        }
        MODE_FXSAVE => {
            unsafe {
                asm!("fxsave64 [{}]", in(reg) pointer, options(nostack));
            }
            true
        }
        _ => false,
    }
}

pub unsafe fn restore_context(pointer: *const u8) -> bool {
    if pointer.is_null() || !(pointer as usize).is_multiple_of(CONTEXT_ALIGNMENT) {
        return false;
    }
    match CONTEXT_MODE.load(Ordering::Acquire) {
        MODE_XSAVE => {
            let mask = CONTEXT_MASK.load(Ordering::Acquire);
            unsafe {
                asm!(
                    "xrstor64 [{}]",
                    in(reg) pointer,
                    in("eax") mask as u32,
                    in("edx") (mask >> 32) as u32,
                    options(nostack)
                );
            }
            true
        }
        MODE_FXSAVE => {
            unsafe {
                asm!("fxrstor64 [{}]", in(reg) pointer, options(nostack));
            }
            true
        }
        _ => false,
    }
}

pub unsafe fn reset_context() -> bool {
    if CONTEXT_MODE.load(Ordering::Acquire) == MODE_DISABLED {
        return false;
    }
    let mxcsr = 0x1f80u32;
    unsafe {
        asm!(
            "fninit",
            "ldmxcsr [{mxcsr}]",
            mxcsr = in(reg) &mxcsr,
            options(nostack)
        );
        if CONTEXT_MASK.load(Ordering::Acquire) & 4 != 0 {
            asm!("vzeroall", options(nomem, nostack));
        } else {
            asm!(
                "pxor xmm0, xmm0",
                "pxor xmm1, xmm1",
                "pxor xmm2, xmm2",
                "pxor xmm3, xmm3",
                "pxor xmm4, xmm4",
                "pxor xmm5, xmm5",
                "pxor xmm6, xmm6",
                "pxor xmm7, xmm7",
                "pxor xmm8, xmm8",
                "pxor xmm9, xmm9",
                "pxor xmm10, xmm10",
                "pxor xmm11, xmm11",
                "pxor xmm12, xmm12",
                "pxor xmm13, xmm13",
                "pxor xmm14, xmm14",
                "pxor xmm15, xmm15",
                options(nomem, nostack)
            );
        }
    }
    true
}

fn simd_self_test() -> bool {
    let input = [1.0f32, 2.0, 4.0, 8.0];
    let mut output = [0.0f32; 4];
    unsafe {
        asm!(
            "movups xmm0, [{input}]",
            "addps xmm0, xmm0",
            "movups [{output}], xmm0",
            input = in(reg) input.as_ptr(),
            output = in(reg) output.as_mut_ptr(),
            out("xmm0") _,
            options(nostack)
        );
    }
    output == [2.0, 4.0, 8.0, 16.0]
}

fn read_xcr0() -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        asm!(
            "xgetbv",
            in("ecx") 0u32,
            out("eax") low,
            out("edx") high,
            options(nomem, nostack, preserves_flags)
        );
    }
    low as u64 | (high as u64) << 32
}

unsafe fn write_xcr0(value: u64) {
    unsafe {
        asm!(
            "xsetbv",
            in("ecx") 0u32,
            in("eax") value as u32,
            in("edx") (value >> 32) as u32,
            options(nomem, nostack, preserves_flags)
        );
    }
}

fn empty_report(cpu: &CpuInfo) -> FpuReport {
    FpuReport {
        fxsave: cpu.fxsave,
        sse: cpu.sse,
        xsave: cpu.xsave,
        avx: cpu.avx,
        xcr0: 0,
        state_bytes: 0,
        simd: false,
        verified: false,
    }
}
