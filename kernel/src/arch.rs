use core::arch::{asm, x86_64::__cpuid};

pub mod apic;
pub mod fpu;
pub mod gdt;
pub mod interrupts;
pub mod paging;
pub mod syscall_entry;
pub mod user;

#[derive(Clone, Copy)]
pub struct CpuInfo {
    vendor: [u8; 12],
    pub max_basic_leaf: u32,
    pub nx: bool,
    pub syscall: bool,
    pub one_gib_pages: bool,
    pub x2apic: bool,
    pub smep: bool,
    pub smap: bool,
    pub invariant_tsc: bool,
    pub rdrand: bool,
    pub rdseed: bool,
    pub fxsave: bool,
    pub sse: bool,
    pub xsave: bool,
    pub avx: bool,
}

impl CpuInfo {
    pub fn detect() -> Self {
        let basic = __cpuid(0);
        let mut vendor = [0u8; 12];
        vendor[0..4].copy_from_slice(&basic.ebx.to_le_bytes());
        vendor[4..8].copy_from_slice(&basic.edx.to_le_bytes());
        vendor[8..12].copy_from_slice(&basic.ecx.to_le_bytes());

        let leaf1 = if basic.eax >= 1 {
            __cpuid(1)
        } else {
            __cpuid(0)
        };
        let leaf7 = if basic.eax >= 7 {
            __cpuid(7)
        } else {
            __cpuid(0)
        };
        let extended = __cpuid(0x8000_0000);
        let leaf_ext1 = if extended.eax >= 0x8000_0001 {
            __cpuid(0x8000_0001)
        } else {
            __cpuid(0)
        };
        let leaf_ext7 = if extended.eax >= 0x8000_0007 {
            __cpuid(0x8000_0007)
        } else {
            __cpuid(0)
        };

        Self {
            vendor,
            max_basic_leaf: basic.eax,
            nx: leaf_ext1.edx & (1 << 20) != 0,
            syscall: leaf_ext1.edx & (1 << 11) != 0,
            one_gib_pages: leaf_ext1.edx & (1 << 26) != 0,
            x2apic: leaf1.ecx & (1 << 21) != 0,
            smep: leaf7.ebx & (1 << 7) != 0,
            smap: leaf7.ebx & (1 << 20) != 0,
            invariant_tsc: leaf_ext7.edx & (1 << 8) != 0,
            rdrand: leaf1.ecx & (1 << 30) != 0,
            rdseed: leaf7.ebx & (1 << 18) != 0,
            fxsave: leaf1.edx & (1 << 24) != 0,
            sse: leaf1.edx & (1 << 25) != 0,
            xsave: leaf1.ecx & (1 << 26) != 0,
            avx: leaf1.ecx & (1 << 28) != 0,
        }
    }

    pub fn vendor(&self) -> &str {
        core::str::from_utf8(&self.vendor).unwrap_or("unknown")
    }
}

pub fn disable_interrupts() {
    unsafe {
        asm!("cli", options(nomem, nostack, preserves_flags));
    }
}

pub fn enable_interrupts() {
    unsafe {
        asm!("sti", options(nomem, nostack, preserves_flags));
    }
}

pub fn wait_for_interrupt() {
    let start = crate::time::monotonic_nanoseconds();
    unsafe {
        asm!("sti; hlt", options(nomem, nostack));
    }
    crate::sysmon::add_idle(crate::time::monotonic_nanoseconds().saturating_sub(start));
}

pub fn reboot() -> ! {
    disable_interrupts();
    for _ in 0..100_000 {
        if unsafe { inb(0x64) } & 2 == 0 {
            unsafe {
                outb(0x64, 0xfe);
            }
            break;
        }
        core::hint::spin_loop();
    }
    halt_forever()
}

pub fn halt_forever() -> ! {
    loop {
        unsafe {
            asm!("cli; hlt", options(nomem, nostack));
        }
    }
}

pub unsafe fn outb(port: u16, value: u8) {
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}

pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    }
    value
}

pub unsafe fn outw(port: u16, value: u16) {
    unsafe {
        asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack, preserves_flags));
    }
}

pub unsafe fn outl(port: u16, value: u32) {
    unsafe {
        asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack, preserves_flags));
    }
}

pub unsafe fn inl(port: u16) -> u32 {
    let value: u32;
    unsafe {
        asm!("in eax, dx", in("dx") port, out("eax") value, options(nomem, nostack, preserves_flags));
    }
    value
}

#[cfg(feature = "boot-test")]
pub unsafe fn debug_exit(value: u32) {
    unsafe { outl(0xf4, value) }
}
