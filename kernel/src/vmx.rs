//! Intel VT-x backend: the same smoke guests as the AMD-V path in `svm.rs`
//! (a real-mode and a 64-bit program that print on COM1, touch memory above
//! 16 MiB, run CPUID and RDMSR and halt, plus the Linux boot-protocol probe),
//! on VMX with EPT.
//!
//! STATUS: written from the Intel SDM and checked on the host by
//! `tools/vmx-host` (field encodings, control adjustment, the EPT tables),
//! but it has not run on an Intel processor: the machine this was built on
//! is AMD and QEMU cannot expose VMX to a guest here. Treat the first run on
//! Intel hardware as the real test. The Linux desktop window and the guest
//! devices in `svm.rs` are still AMD-V only.

use core::arch::asm;
use core::arch::x86_64::__cpuid_count;

use crate::memory::FrameAllocator;
use crate::svm::{
    COM1, COM1_END, CR0_LONG, CR4_PAE, EFER_LMA, EFER_LME, GUEST_ENTRY, GUEST_HIGH_ADDR,
    GUEST_HIGH_MARKER_ADDR, GUEST_HIGH_VALUE, GUEST_LOG_MAX, GUEST_MARKER, GUEST_MARKER_ADDR,
    GUEST_MESSAGE, GUEST_RAM_PAGES, GUEST_STACK, LINUX_BOOT_PARAMS, LINUX_LOAD, LINUX_MESSAGE,
    LINUX_PROBE, LINUX_STACK, LONG_ENTRY, LONG_PML4, LONG_STACK, MAX_EXITS, PAGE_SIZE,
    build_guest_paging, load_linux_memory, long_guest_code, read_msr, real_guest_code, write_msr,
    zero_region,
};
use crate::vmx_logic::{
    CR4_VMXE, EPT_MEMORY_TYPE_WRITE_BACK, FEATURE_CONTROL_LOCK, FEATURE_CONTROL_VMXON_OUTSIDE_SMX,
    PAGE_2M, UNUSABLE_SEGMENT, adjust_controls, advance_rip, apply_fixed_bits, build_ept,
    control_msr, decode_io, entry_control, ept_pointer, exit_control, exit_reason, field,
    merge_register, msr, pin, proc, secondary,
};

const IA32_EFER: u32 = 0xc000_0080;
const IA32_SYSENTER_CS: u32 = 0x174;
const IA32_SYSENTER_ESP: u32 = 0x175;
const IA32_SYSENTER_EIP: u32 = 0x176;
const CR0_PE: u64 = 1;
const CR0_PG: u64 = 1 << 31;
const GUEST_RAM_BYTES: u64 = GUEST_RAM_PAGES * PAGE_SIZE;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Real,
    Long,
}

#[derive(Clone, Copy)]
pub struct VmxReport {
    pub supported: bool,
    pub locked_off: bool,
    pub enabled: bool,
    pub ept: bool,
    pub unrestricted_guest: bool,
    pub real_mode_ok: bool,
    pub long_mode_ok: bool,
    pub linux_probe_ok: bool,
    pub exits: u32,
    pub instruction_error: u32,
    pub last_exit: u32,
    pub qualification: u64,
    pub verified: bool,
}

impl VmxReport {
    const fn none() -> Self {
        Self {
            supported: false,
            locked_off: false,
            enabled: false,
            ept: false,
            unrestricted_guest: false,
            real_mode_ok: false,
            long_mode_ok: false,
            linux_probe_ok: false,
            exits: 0,
            instruction_error: 0,
            last_exit: 0,
            qualification: 0,
            verified: false,
        }
    }

    /// Nothing to test on this processor (no VMX): reported as a skip, not a
    /// failure.
    pub fn skipped(&self) -> bool {
        !self.supported
    }
}

// ------------------------------------------------------------ instructions

fn read_cr0() -> u64 {
    let value: u64;
    unsafe { asm!("mov {}, cr0", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

fn read_cr3() -> u64 {
    let value: u64;
    unsafe { asm!("mov {}, cr3", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

fn read_cr4() -> u64 {
    let value: u64;
    unsafe { asm!("mov {}, cr4", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

unsafe fn write_cr0(value: u64) {
    unsafe { asm!("mov cr0, {}", in(reg) value, options(nostack)) };
}

unsafe fn write_cr4(value: u64) {
    unsafe { asm!("mov cr4, {}", in(reg) value, options(nostack)) };
}

macro_rules! read_selector {
    ($name:literal) => {{
        let value: u64;
        unsafe {
            asm!(concat!("mov {0:x}, ", $name), out(reg) value, options(nomem, nostack, preserves_flags))
        };
        (value & 0xffff) as u16
    }};
}

#[repr(C, packed)]
struct TableRegister {
    limit: u16,
    base: u64,
}

fn store_gdtr() -> u64 {
    let mut table = TableRegister { limit: 0, base: 0 };
    unsafe { asm!("sgdt [{}]", in(reg) &mut table, options(nostack, preserves_flags)) };
    table.base
}

fn store_idtr() -> u64 {
    let mut table = TableRegister { limit: 0, base: 0 };
    unsafe { asm!("sidt [{}]", in(reg) &mut table, options(nostack, preserves_flags)) };
    table.base
}

fn store_tr() -> u16 {
    let value: u64;
    unsafe { asm!("str {0:x}", out(reg) value, options(nomem, nostack, preserves_flags)) };
    (value & 0xffff) as u16
}

/// Base address of a system-segment (TSS) descriptor in the current GDT.
fn tss_base(gdt: u64, selector: u16) -> u64 {
    let at = gdt + u64::from(selector & !7);
    let low = unsafe { core::ptr::read_unaligned(at as *const u64) };
    let high = unsafe { core::ptr::read_unaligned((at + 8) as *const u64) };
    ((low >> 16) & 0xff_ffff) | (((low >> 56) & 0xff) << 24) | ((high & 0xffff_ffff) << 32)
}

unsafe fn vmxon(region: u64) -> bool {
    let failed: u8;
    unsafe {
        asm!("vmxon qword ptr [{0}]", "setna {1}", in(reg) &region, out(reg_byte) failed, options(nostack));
    }
    failed == 0
}

unsafe fn vmxoff() {
    unsafe { asm!("vmxoff", options(nostack)) };
}

unsafe fn vmclear(region: u64) -> bool {
    let failed: u8;
    unsafe {
        asm!("vmclear qword ptr [{0}]", "setna {1}", in(reg) &region, out(reg_byte) failed, options(nostack));
    }
    failed == 0
}

unsafe fn vmptrld(region: u64) -> bool {
    let failed: u8;
    unsafe {
        asm!("vmptrld qword ptr [{0}]", "setna {1}", in(reg) &region, out(reg_byte) failed, options(nostack));
    }
    failed == 0
}

unsafe fn vmwrite(encoding: u32, value: u64) -> bool {
    let failed: u8;
    unsafe {
        asm!("vmwrite {0}, {1}", "setna {2}", in(reg) u64::from(encoding), in(reg) value, out(reg_byte) failed, options(nostack));
    }
    failed == 0
}

unsafe fn vmread(encoding: u32) -> u64 {
    let value: u64;
    unsafe {
        asm!("vmread {0}, {1}", out(reg) value, in(reg) u64::from(encoding), options(nostack));
    }
    value
}

/// Enters the guest (launch the first time, resume afterwards) with the
/// general registers in `regs` (rax, rbx, rcx, rdx, rsi, rdi, rbp, r8-r15,
/// then a non-zero word once launched) and returns after the next VM exit
/// with the guest's registers stored back: 0 for an exit, 1 if the entry
/// itself failed (`VM_INSTRUCTION_ERROR` says why).
///
/// The exit lands on the label below with `rsp` as written to HOST_RSP, which
/// is the stack slot holding `regs`; everything the guest does not preserve
/// is saved first.
#[inline(never)]
unsafe fn vmx_enter(regs: *mut u64) -> u64 {
    let status: u64;
    unsafe {
        asm!(
            "push rbp",
            "push rbx",
            "push r12",
            "push r13",
            "push r14",
            "push r15",
            "push rdi",
            "mov eax, 0x6c14",
            "vmwrite rax, rsp",
            "lea rax, [rip + 5f]",
            "mov ecx, 0x6c16",
            "vmwrite rcx, rax",
            "cmp qword ptr [rdi + 120], 0",
            "mov rbx, [rdi + 8]",
            "mov rcx, [rdi + 16]",
            "mov rdx, [rdi + 24]",
            "mov rsi, [rdi + 32]",
            "mov rbp, [rdi + 48]",
            "mov r8,  [rdi + 56]",
            "mov r9,  [rdi + 64]",
            "mov r10, [rdi + 72]",
            "mov r11, [rdi + 80]",
            "mov r12, [rdi + 88]",
            "mov r13, [rdi + 96]",
            "mov r14, [rdi + 104]",
            "mov r15, [rdi + 112]",
            "mov rax, [rdi + 0]",
            "mov rdi, [rdi + 40]",
            "je 2f",
            "vmresume",
            "jmp 3f",
            "2:",
            "vmlaunch",
            "3:",
            "mov eax, 1",
            "jmp 6f",
            "5:",
            "xchg rdi, [rsp]",
            "mov [rdi + 0], rax",
            "mov [rdi + 8], rbx",
            "mov [rdi + 16], rcx",
            "mov [rdi + 24], rdx",
            "mov [rdi + 32], rsi",
            "mov [rdi + 48], rbp",
            "mov [rdi + 56], r8",
            "mov [rdi + 64], r9",
            "mov [rdi + 72], r10",
            "mov [rdi + 80], r11",
            "mov [rdi + 88], r12",
            "mov [rdi + 96], r13",
            "mov [rdi + 104], r14",
            "mov [rdi + 112], r15",
            "pop rax",
            "mov [rdi + 40], rax",
            "xor eax, eax",
            "jmp 7f",
            "6:",
            "add rsp, 8",
            "7:",
            "pop r15",
            "pop r14",
            "pop r13",
            "pop r12",
            "pop rbx",
            "pop rbp",
            inout("rdi") regs => _,
            out("rax") status,
            out("rcx") _,
            out("rdx") _,
            out("rsi") _,
            out("r8") _,
            out("r9") _,
            out("r10") _,
            out("r11") _,
        );
    }
    status
}

// -------------------------------------------------------------- capability

#[derive(Clone, Copy)]
struct Controls {
    pin: u32,
    proc: u32,
    secondary: u32,
    exit: u32,
    entry_real: u32,
    entry_long: u32,
    ept: bool,
    unrestricted: bool,
    cr0_fixed0: u64,
    cr0_fixed1: u64,
    cr4_fixed0: u64,
    cr4_fixed1: u64,
    revision: u32,
}

/// What the processor allows, or `None` when something the backend needs
/// (EPT with 2 MiB pages, the basic controls, loading EFER) is missing.
fn probe_controls() -> Option<Controls> {
    let basic = read_msr(msr::VMX_BASIC);
    let pin_cap = read_msr(control_msr(basic, msr::PINBASED, msr::TRUE_PINBASED));
    let proc_cap = read_msr(control_msr(basic, msr::PROCBASED, msr::TRUE_PROCBASED));
    let exit_cap = read_msr(control_msr(basic, msr::EXIT, msr::TRUE_EXIT));
    let entry_cap = read_msr(control_msr(basic, msr::ENTRY, msr::TRUE_ENTRY));
    let wanted_proc = proc::HLT_EXITING | proc::UNCONDITIONAL_IO_EXITING | proc::SECONDARY_CONTROLS;
    let secondary_cap = if (proc_cap >> 32) as u32 & proc::SECONDARY_CONTROLS != 0 {
        read_msr(msr::PROCBASED2)
    } else {
        return None;
    };
    let ept_cap = read_msr(msr::EPT_VPID_CAP);
    // 4-level walks, write-back memory, 2 MiB pages.
    let ept = ept_cap & (1 << 6) != 0 && ept_cap & (1 << 14) != 0 && ept_cap & (1 << 16) != 0;
    if !ept || (secondary_cap >> 32) as u32 & secondary::ENABLE_EPT == 0 {
        return None;
    }
    let unrestricted = (secondary_cap >> 32) as u32 & secondary::UNRESTRICTED_GUEST != 0;
    let mut wanted_secondary = secondary::ENABLE_EPT;
    if unrestricted {
        wanted_secondary |= secondary::UNRESTRICTED_GUEST;
    }
    if (secondary_cap >> 32) as u32 & secondary::ENABLE_RDTSCP != 0 {
        wanted_secondary |= secondary::ENABLE_RDTSCP;
    }
    let wanted_exit =
        exit_control::HOST_ADDRESS_SPACE_SIZE | exit_control::LOAD_EFER | exit_control::SAVE_EFER;
    Some(Controls {
        pin: adjust_controls(pin::EXTERNAL_INTERRUPT_EXITING, pin_cap)?,
        proc: adjust_controls(wanted_proc, proc_cap)?,
        secondary: adjust_controls(wanted_secondary, secondary_cap)?,
        exit: adjust_controls(wanted_exit, exit_cap)?,
        entry_real: adjust_controls(entry_control::LOAD_EFER, entry_cap)?,
        entry_long: adjust_controls(
            entry_control::LOAD_EFER | entry_control::IA32E_MODE_GUEST,
            entry_cap,
        )?,
        ept,
        unrestricted,
        cr0_fixed0: read_msr(msr::CR0_FIXED0),
        cr0_fixed1: read_msr(msr::CR0_FIXED1),
        cr4_fixed0: read_msr(msr::CR4_FIXED0),
        cr4_fixed1: read_msr(msr::CR4_FIXED1),
        revision: (basic & 0x7fff_ffff) as u32,
    })
}

struct Enabled {
    saved_cr0: u64,
    saved_cr4: u64,
}

enum EnableError {
    LockedOff,
    Failed,
}

/// Turns VMX on: the feature-control MSR, CR0/CR4 fixed bits, CR4.VMXE and
/// VMXON on a fresh region.
fn enable(controls: &Controls, frames: &mut FrameAllocator) -> Result<Enabled, EnableError> {
    let feature = read_msr(msr::FEATURE_CONTROL);
    if feature & FEATURE_CONTROL_LOCK != 0 {
        if feature & FEATURE_CONTROL_VMXON_OUTSIDE_SMX == 0 {
            return Err(EnableError::LockedOff);
        }
    } else {
        unsafe {
            write_msr(
                msr::FEATURE_CONTROL,
                feature | FEATURE_CONTROL_LOCK | FEATURE_CONTROL_VMXON_OUTSIDE_SMX,
            );
        }
    }
    let region = frames
        .allocate_contiguous(1, 1)
        .ok_or(EnableError::Failed)?
        .address();
    zero_region(region, PAGE_SIZE);
    unsafe { core::ptr::write_volatile(region as usize as *mut u32, controls.revision) };

    let saved_cr0 = read_cr0();
    let saved_cr4 = read_cr4();
    let cr0 = apply_fixed_bits(saved_cr0, controls.cr0_fixed0, controls.cr0_fixed1);
    let cr4 = apply_fixed_bits(
        saved_cr4 | CR4_VMXE,
        controls.cr4_fixed0,
        controls.cr4_fixed1,
    );
    unsafe {
        if cr0 != saved_cr0 {
            write_cr0(cr0);
        }
        write_cr4(cr4);
        if !vmxon(region) {
            write_cr4(saved_cr4);
            write_cr0(saved_cr0);
            return Err(EnableError::Failed);
        }
    }
    Ok(Enabled {
        saved_cr0,
        saved_cr4,
    })
}

fn disable(enabled: Enabled) {
    unsafe {
        vmxoff();
        write_cr4(enabled.saved_cr4);
        write_cr0(enabled.saved_cr0);
    }
}

// ---------------------------------------------------------------------- VM

struct Vm {
    regs: [u64; 16],
    vmcs: u64,
    guest_ram: u64,
    mode: Mode,
    exits: u32,
    io_writes: u32,
    cpuid_exits: u32,
    msr_exits: u32,
    last_exit: u32,
    halted: bool,
    console: [u8; GUEST_LOG_MAX],
    console_len: usize,
    entry_error: u32,
    qualification: u64,
}

/// Stops writing to the VMCS at the first failure.
struct Writer {
    ok: bool,
}

impl Writer {
    fn put(&mut self, encoding: u32, value: u64) {
        if self.ok {
            self.ok = unsafe { vmwrite(encoding, value) };
        }
    }
}

impl Vm {
    fn new(controls: &Controls, frames: &mut FrameAllocator, mode: Mode) -> Option<Self> {
        let vmcs = frames.allocate_contiguous(1, 1)?.address();
        let tables = frames.allocate_contiguous(3, 1)?.address();
        let guest_ram = frames.allocate_contiguous(GUEST_RAM_PAGES, 512)?.address();
        zero_region(vmcs, PAGE_SIZE);
        zero_region(tables, 3 * PAGE_SIZE);
        zero_region(guest_ram, GUEST_RAM_BYTES);
        unsafe { core::ptr::write_volatile(vmcs as usize as *mut u32, controls.revision) };
        if !build_ept(
            tables,
            tables + PAGE_SIZE,
            tables + 2 * PAGE_SIZE,
            guest_ram,
            GUEST_RAM_BYTES,
            |at, value| unsafe { core::ptr::write_volatile(at as usize as *mut u64, value) },
        ) {
            return None;
        }
        let mut vm = Self {
            regs: [0; 16],
            vmcs,
            guest_ram,
            mode,
            exits: 0,
            io_writes: 0,
            cpuid_exits: 0,
            msr_exits: 0,
            last_exit: 0,
            halted: false,
            console: [0; GUEST_LOG_MAX],
            console_len: 0,
            entry_error: 0,
            qualification: 0,
        };
        if mode == Mode::Long {
            build_guest_paging(guest_ram, GUEST_RAM_BYTES);
        }
        vm.program(controls, ept_pointer(tables))?;
        Some(vm)
    }

    fn load(&mut self, code: &[u8], at: u64) {
        for (offset, byte) in code.iter().enumerate() {
            unsafe {
                core::ptr::write_volatile(
                    (self.guest_ram + at + offset as u64) as usize as *mut u8,
                    *byte,
                );
            }
        }
    }

    /// Writes every VMCS field for a fresh guest. The VMCS is current when
    /// this runs.
    fn program(&mut self, controls: &Controls, eptp: u64) -> Option<()> {
        unsafe {
            if !vmclear(self.vmcs) || !vmptrld(self.vmcs) {
                return None;
            }
        }
        let mut w = Writer { ok: true };

        w.put(field::PIN_BASED_CONTROLS, u64::from(controls.pin));
        w.put(field::PROC_BASED_CONTROLS, u64::from(controls.proc));
        w.put(field::SECONDARY_CONTROLS, u64::from(controls.secondary));
        w.put(field::EXIT_CONTROLS, u64::from(controls.exit));
        let entry = if self.mode == Mode::Long {
            controls.entry_long
        } else {
            controls.entry_real
        };
        w.put(field::ENTRY_CONTROLS, u64::from(entry));
        w.put(field::EXCEPTION_BITMAP, 0);
        w.put(field::PAGE_FAULT_ERROR_MASK, 0);
        w.put(field::PAGE_FAULT_ERROR_MATCH, 0);
        w.put(field::CR3_TARGET_COUNT, 0);
        w.put(field::EXIT_MSR_STORE_COUNT, 0);
        w.put(field::EXIT_MSR_LOAD_COUNT, 0);
        w.put(field::ENTRY_MSR_LOAD_COUNT, 0);
        w.put(field::ENTRY_INTERRUPTION_INFO, 0);
        w.put(field::EPT_POINTER, eptp);
        w.put(field::CR0_GUEST_HOST_MASK, 0);
        w.put(field::CR0_READ_SHADOW, 0);
        w.put(field::CR4_GUEST_HOST_MASK, CR4_VMXE);
        w.put(field::CR4_READ_SHADOW, 0);

        self.host_state(&mut w);
        self.guest_state(&mut w, controls);
        w.ok.then_some(())
    }

    fn host_state(&self, w: &mut Writer) {
        let gdt = store_gdtr();
        let tr = store_tr();
        w.put(field::HOST_CR0, read_cr0());
        w.put(field::HOST_CR3, read_cr3());
        w.put(field::HOST_CR4, read_cr4());
        w.put(
            field::HOST_ES_SELECTOR,
            u64::from(read_selector!("es") & !7),
        );
        w.put(
            field::HOST_CS_SELECTOR,
            u64::from(read_selector!("cs") & !7),
        );
        w.put(
            field::HOST_SS_SELECTOR,
            u64::from(read_selector!("ss") & !7),
        );
        w.put(
            field::HOST_DS_SELECTOR,
            u64::from(read_selector!("ds") & !7),
        );
        w.put(field::HOST_FS_SELECTOR, 0);
        w.put(field::HOST_GS_SELECTOR, 0);
        w.put(field::HOST_TR_SELECTOR, u64::from(tr & !7));
        w.put(field::HOST_FS_BASE, read_msr(0xc000_0100));
        w.put(field::HOST_GS_BASE, read_msr(0xc000_0101));
        w.put(field::HOST_TR_BASE, tss_base(gdt, tr));
        w.put(field::HOST_GDTR_BASE, gdt);
        w.put(field::HOST_IDTR_BASE, store_idtr());
        w.put(field::HOST_SYSENTER_CS, read_msr(IA32_SYSENTER_CS));
        w.put(field::HOST_SYSENTER_ESP, read_msr(IA32_SYSENTER_ESP));
        w.put(field::HOST_SYSENTER_EIP, read_msr(IA32_SYSENTER_EIP));
        w.put(field::HOST_EFER, read_msr(IA32_EFER));
    }

    fn guest_state(&mut self, w: &mut Writer, controls: &Controls) {
        let real = self.mode == Mode::Real;
        let (cr0_target, cr4_target, efer, cr3, rip, rsp) = if real {
            (0x6000_0010u64, 0u64, 0u64, 0u64, GUEST_ENTRY, GUEST_STACK)
        } else {
            (
                CR0_LONG,
                CR4_PAE,
                EFER_LME | EFER_LMA,
                LONG_PML4,
                LONG_ENTRY,
                LONG_STACK,
            )
        };
        // With unrestricted guest the guest may run with PE and PG clear.
        let relaxed = if controls.unrestricted {
            !(CR0_PE | CR0_PG)
        } else {
            !0
        };
        let cr0 = apply_fixed_bits(
            cr0_target,
            controls.cr0_fixed0 & relaxed,
            controls.cr0_fixed1,
        );
        let cr4 = apply_fixed_bits(
            cr4_target | CR4_VMXE,
            controls.cr4_fixed0,
            controls.cr4_fixed1,
        );
        w.put(field::GUEST_CR0, cr0);
        w.put(field::GUEST_CR3, cr3);
        w.put(field::GUEST_CR4, cr4);
        w.put(field::GUEST_DR7, 0x400);
        w.put(field::GUEST_RSP, rsp);
        w.put(field::GUEST_RIP, rip);
        w.put(field::GUEST_RFLAGS, 2);
        w.put(field::GUEST_EFER, efer);

        // (selector, limit, access rights, base) per segment register.
        let (code, data) = if real {
            ((0xffffu64, 0x9bu64), (0xffff_ffffu64, 0x8093u64))
        } else {
            ((0xffff_ffff, 0xa09b), (0xffff_ffff, 0xc093))
        };
        w.put(field::GUEST_CS_SELECTOR, 0);
        w.put(field::GUEST_CS_LIMIT, code.0);
        w.put(field::GUEST_CS_ACCESS, code.1);
        w.put(field::GUEST_CS_BASE, 0);
        for (selector, limit, access, base) in [
            (
                field::GUEST_ES_SELECTOR,
                field::GUEST_ES_LIMIT,
                field::GUEST_ES_ACCESS,
                field::GUEST_ES_BASE,
            ),
            (
                field::GUEST_SS_SELECTOR,
                field::GUEST_SS_LIMIT,
                field::GUEST_SS_ACCESS,
                field::GUEST_SS_BASE,
            ),
            (
                field::GUEST_DS_SELECTOR,
                field::GUEST_DS_LIMIT,
                field::GUEST_DS_ACCESS,
                field::GUEST_DS_BASE,
            ),
            (
                field::GUEST_FS_SELECTOR,
                field::GUEST_FS_LIMIT,
                field::GUEST_FS_ACCESS,
                field::GUEST_FS_BASE,
            ),
            (
                field::GUEST_GS_SELECTOR,
                field::GUEST_GS_LIMIT,
                field::GUEST_GS_ACCESS,
                field::GUEST_GS_BASE,
            ),
        ] {
            w.put(selector, 0);
            w.put(limit, data.0);
            w.put(access, data.1);
            w.put(base, 0);
        }
        w.put(field::GUEST_LDTR_SELECTOR, 0);
        w.put(field::GUEST_LDTR_LIMIT, 0);
        w.put(field::GUEST_LDTR_ACCESS, u64::from(UNUSABLE_SEGMENT));
        w.put(field::GUEST_LDTR_BASE, 0);
        w.put(field::GUEST_TR_SELECTOR, 0);
        w.put(field::GUEST_TR_LIMIT, 0x67);
        w.put(field::GUEST_TR_ACCESS, 0x8b);
        w.put(field::GUEST_TR_BASE, 0);
        w.put(field::GUEST_GDTR_BASE, 0);
        w.put(field::GUEST_GDTR_LIMIT, 0xffff);
        w.put(field::GUEST_IDTR_BASE, 0);
        w.put(field::GUEST_IDTR_LIMIT, 0xffff);

        w.put(field::GUEST_INTERRUPTIBILITY, 0);
        w.put(field::GUEST_ACTIVITY_STATE, 0);
        w.put(field::GUEST_PENDING_DEBUG, 0);
        w.put(field::GUEST_DEBUGCTL, 0);
        w.put(field::GUEST_SYSENTER_CS, 0);
        w.put(field::GUEST_SYSENTER_ESP, 0);
        w.put(field::GUEST_SYSENTER_EIP, 0);
        w.put(field::GUEST_VMCS_LINK_POINTER, u64::MAX);
    }

    fn run(&mut self) {
        let flags: u64;
        unsafe { asm!("pushfq", "pop {}", out(reg) flags) };
        // Interrupt exits are re-taken by the host with a brief `sti`.
        unsafe { asm!("cli", options(nomem, nostack)) };
        self.run_loop();
        if flags & (1 << 9) != 0 {
            unsafe { asm!("sti", options(nomem, nostack)) };
        }
    }

    fn run_loop(&mut self) {
        while self.exits < MAX_EXITS {
            let status = unsafe { vmx_enter(self.regs.as_mut_ptr()) };
            if status != 0 {
                self.entry_error = unsafe { vmread(field::VM_INSTRUCTION_ERROR) } as u32;
                return;
            }
            self.regs[15] = 1;
            self.exits += 1;
            let raw = unsafe { vmread(field::EXIT_REASON) } as u32;
            self.last_exit = raw;
            if raw & exit_reason::ENTRY_FAILURE != 0 {
                self.qualification = unsafe { vmread(field::EXIT_QUALIFICATION) };
                return;
            }
            match raw & 0xffff {
                exit_reason::HLT | exit_reason::VMCALL => {
                    self.halted = true;
                    return;
                }
                exit_reason::CPUID => {
                    self.cpuid_exits += 1;
                    self.handle_cpuid();
                }
                exit_reason::IO_INSTRUCTION => {
                    if !self.handle_io() {
                        return;
                    }
                }
                exit_reason::RDMSR => {
                    self.msr_exits += 1;
                    self.regs[0] = 0;
                    self.regs[3] = 0;
                    self.skip_instruction();
                }
                exit_reason::WRMSR => {
                    self.msr_exits += 1;
                    self.skip_instruction();
                }
                exit_reason::EXTERNAL_INTERRUPT => unsafe {
                    asm!("sti", "nop", "nop", "cli", options(nomem, nostack));
                },
                exit_reason::EPT_VIOLATION => {
                    let physical = unsafe { vmread(field::GUEST_PHYSICAL_ADDRESS) };
                    crate::serial::format(format_args!(
                        "AEROS_VMX_EPT guest_physical={physical:#x}\n"
                    ));
                    return;
                }
                _ => return,
            }
        }
    }

    fn skip_instruction(&mut self) {
        unsafe {
            let length = vmread(field::EXIT_INSTRUCTION_LENGTH) as u32;
            let rip = vmread(field::GUEST_RIP);
            let _ = vmwrite(field::GUEST_RIP, advance_rip(rip, length));
        }
    }

    fn handle_cpuid(&mut self) {
        let leaf = self.regs[0] as u32;
        let sub_leaf = self.regs[2] as u32;
        let mut result = __cpuid_count(leaf, sub_leaf);
        if leaf == 1 {
            // The guest is not offered VMX.
            result.ecx &= !(1 << 5);
        }
        self.regs[0] = u64::from(result.eax);
        self.regs[1] = u64::from(result.ebx);
        self.regs[2] = u64::from(result.ecx);
        self.regs[3] = u64::from(result.edx);
        self.skip_instruction();
    }

    fn handle_io(&mut self) -> bool {
        let io = decode_io(unsafe { vmread(field::EXIT_QUALIFICATION) });
        if io.string {
            return false;
        }
        if io.is_in {
            let value = u64::from(self.pio_in(io.port));
            self.regs[0] = merge_register(self.regs[0], value, io.size);
        } else {
            self.pio_out(io.port, self.regs[0] as u8);
        }
        self.skip_instruction();
        true
    }

    fn pio_in(&self, port: u16) -> u8 {
        match port {
            // Line status: transmitter empty.
            p if p == COM1 + 5 => 0x60,
            COM1..=COM1_END => 0,
            _ => 0xff,
        }
    }

    fn pio_out(&mut self, port: u16, value: u8) {
        if port == COM1 {
            self.io_writes += 1;
            if self.console_len < GUEST_LOG_MAX {
                self.console[self.console_len] = value;
                self.console_len += 1;
            }
        }
    }

    fn read_guest_u32(&self, address: u64) -> u32 {
        unsafe { core::ptr::read_volatile((self.guest_ram + address) as usize as *const u32) }
    }

    fn console_is(&self, expected: &[u8]) -> bool {
        self.console_len == expected.len() && self.console[..self.console_len] == *expected
    }

    fn finish(self) {
        unsafe {
            let _ = vmclear(self.vmcs);
        }
    }
}

fn run_guest(controls: &Controls, frames: &mut FrameAllocator, mode: Mode) -> Option<Vm> {
    let mut vm = Vm::new(controls, frames, mode)?;
    let (code, written, at) = match mode {
        Mode::Real => {
            let (code, written) = real_guest_code();
            (code, written, GUEST_ENTRY)
        }
        Mode::Long => {
            let (code, written) = long_guest_code();
            (code, written, LONG_ENTRY)
        }
    };
    vm.load(&code[..written], at);
    crate::serial::line(match mode {
        Mode::Real => "AEROS_VMX_LAUNCH guest=aeros-smoke",
        Mode::Long => "AEROS_VMX_LAUNCH guest=aeros-longmode",
    });
    vm.run();
    Some(vm)
}

fn smoke_ok(vm: &Vm) -> bool {
    vm.halted
        && vm.console_is(GUEST_MESSAGE)
        && vm.read_guest_u32(GUEST_MARKER_ADDR) == GUEST_MARKER
        && vm.read_guest_u32(GUEST_HIGH_MARKER_ADDR) == GUEST_HIGH_VALUE
        && vm.read_guest_u32(GUEST_HIGH_ADDR) == GUEST_HIGH_VALUE
        && vm.io_writes as usize >= GUEST_MESSAGE.len()
        && vm.cpuid_exits >= 1
        && vm.msr_exits >= 1
}

fn run_linux_probe(controls: &Controls, frames: &mut FrameAllocator) -> Option<(bool, u32)> {
    let mut vm = Vm::new(controls, frames, Mode::Long)?;
    let pm_offset = load_linux_memory(vm.guest_ram, &LINUX_PROBE)?;
    let _ = pm_offset;
    unsafe {
        let _ = vmwrite(field::GUEST_RIP, LINUX_LOAD + 0x200);
        let _ = vmwrite(field::GUEST_RSP, LINUX_STACK);
    }
    vm.regs[4] = LINUX_BOOT_PARAMS;
    crate::serial::line("AEROS_VMX_LAUNCH guest=linux-boot-protocol");
    vm.run();
    let ok = vm.halted
        && vm.console_is(LINUX_MESSAGE)
        && vm.read_guest_u32(GUEST_MARKER_ADDR) == GUEST_MARKER
        && vm.read_guest_u32(GUEST_HIGH_MARKER_ADDR) == 0xaa55;
    let exits = vm.exits;
    vm.finish();
    Some((ok, exits))
}

pub fn self_test(frames: &mut FrameAllocator) -> VmxReport {
    let mut report = VmxReport::none();
    if __cpuid_count(1, 0).ecx & (1 << 5) == 0 {
        return report;
    }
    report.supported = true;
    let Some(controls) = probe_controls() else {
        return report;
    };
    report.ept = controls.ept;
    report.unrestricted_guest = controls.unrestricted;
    let enabled = match enable(&controls, frames) {
        Ok(enabled) => enabled,
        Err(EnableError::LockedOff) => {
            report.locked_off = true;
            return report;
        }
        Err(EnableError::Failed) => return report,
    };
    report.enabled = true;

    if controls.unrestricted {
        if let Some(vm) = run_guest(&controls, frames, Mode::Real) {
            report.real_mode_ok = smoke_ok(&vm);
            report.exits += vm.exits;
            report.instruction_error |= vm.entry_error;
            if !report.real_mode_ok {
                report.last_exit = vm.last_exit;
                report.qualification = vm.qualification;
            }
            vm.finish();
        }
    } else {
        // Real mode needs unrestricted guest; nothing to test without it.
        report.real_mode_ok = true;
    }
    if let Some(vm) = run_guest(&controls, frames, Mode::Long) {
        report.long_mode_ok = smoke_ok(&vm);
        report.exits += vm.exits;
        report.instruction_error |= vm.entry_error;
        if !report.long_mode_ok {
            report.last_exit = vm.last_exit;
            report.qualification = vm.qualification;
        }
        vm.finish();
    }
    if let Some((ok, exits)) = run_linux_probe(&controls, frames) {
        report.linux_probe_ok = ok;
        report.exits += exits;
    }
    disable(enabled);

    report.verified = report.real_mode_ok && report.long_mode_ok && report.linux_probe_ok;
    report
}

const _: () = assert!(EPT_MEMORY_TYPE_WRITE_BACK == 0x30 && PAGE_2M == 0x20_0000);
const _: () = assert!(GUEST_RAM_BYTES.is_multiple_of(PAGE_2M));
