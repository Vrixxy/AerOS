use core::arch::{asm, naked_asm};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::CpuInfo;

use super::user;

const EFER: u32 = 0xc000_0080;
const STAR: u32 = 0xc000_0081;
const LSTAR: u32 = 0xc000_0082;
const FMASK: u32 = 0xc000_0084;
const EFER_SCE: u64 = 1;
const KERNEL_CODE: u64 = 0x08;
const USER_STAR_BASE: u64 = 0x13;
const MASKED_FLAGS: u64 = (1 << 8) | (1 << 9) | (1 << 10) | (1 << 18);
const SYSCALL_STACK_SIZE: usize = 32 * 1024;
const MAX_CPUS: usize = 8;
const FS_BASE: u32 = 0xc000_0100;
const GS_BASE: u32 = 0xc000_0101;
const KERNEL_GS_BASE: u32 = 0xc000_0102;

#[repr(align(16))]
struct SyscallStack(UnsafeCell<[u8; SYSCALL_STACK_SIZE]>);

#[repr(C, align(64))]
struct CpuLocal {
    kernel_rsp: UnsafeCell<u64>,
    user_rsp: UnsafeCell<u64>,
    logical: UnsafeCell<u64>,
}

unsafe impl Sync for SyscallStack {}
unsafe impl Sync for CpuLocal {}

static SYSCALL_STACKS: [SyscallStack; MAX_CPUS] =
    [const { SyscallStack(UnsafeCell::new([0; SYSCALL_STACK_SIZE])) }; MAX_CPUS];
static CPU_LOCALS: [CpuLocal; MAX_CPUS] = [const {
    CpuLocal {
        kernel_rsp: UnsafeCell::new(0),
        user_rsp: UnsafeCell::new(0),
        logical: UnsafeCell::new(0),
    }
}; MAX_CPUS];
static READY: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
pub struct SyscallEntryState {
    pub supported: bool,
    pub sce: bool,
    pub target_valid: bool,
    pub flags_masked: bool,
    pub verified: bool,
}

pub fn init(cpu: &CpuInfo) -> SyscallEntryState {
    init_for_cpu(0, cpu)
}

pub fn init_for_cpu(logical: usize, cpu: &CpuInfo) -> SyscallEntryState {
    if !cpu.syscall {
        return SyscallEntryState {
            supported: false,
            sce: false,
            target_valid: false,
            flags_masked: false,
            verified: false,
        };
    }
    if logical >= MAX_CPUS {
        return SyscallEntryState {
            supported: true,
            sce: false,
            target_valid: false,
            flags_masked: false,
            verified: false,
        };
    }
    let stack_top = unsafe {
        (*SYSCALL_STACKS[logical].0.get())
            .as_ptr()
            .add(SYSCALL_STACK_SIZE) as u64
    } & !15;
    unsafe {
        *CPU_LOCALS[logical].kernel_rsp.get() = stack_top;
        *CPU_LOCALS[logical].user_rsp.get() = 0;
        *CPU_LOCALS[logical].logical.get() = logical as u64;
    }
    let star = (USER_STAR_BASE << 48) | (KERNEL_CODE << 32);
    unsafe {
        write_msr(GS_BASE, 0);
        write_msr(
            KERNEL_GS_BASE,
            &CPU_LOCALS[logical] as *const CpuLocal as u64,
        );
        write_msr(EFER, read_msr(EFER) | EFER_SCE);
        write_msr(STAR, star);
        write_msr(LSTAR, syscall_entry as *const () as usize as u64);
        write_msr(FMASK, MASKED_FLAGS);
    }
    let efer = read_msr(EFER);
    let observed_star = read_msr(STAR);
    let observed_target = read_msr(LSTAR);
    let observed_mask = read_msr(FMASK);
    let sce = efer & EFER_SCE != 0;
    let target_valid =
        observed_star == star && observed_target == syscall_entry as *const () as usize as u64;
    let flags_masked = observed_mask & MASKED_FLAGS == MASKED_FLAGS;
    let gs_valid = read_msr(GS_BASE) == 0
        && read_msr(KERNEL_GS_BASE) == &CPU_LOCALS[logical] as *const CpuLocal as u64;
    let verified = sce && target_valid && flags_masked && gs_valid;
    if verified {
        READY.fetch_or(1 << logical, Ordering::AcqRel);
    }
    SyscallEntryState {
        supported: true,
        sce,
        target_valid,
        flags_masked,
        verified,
    }
}

pub fn ready_mask() -> u64 {
    READY.load(Ordering::Acquire)
}

pub fn set_user_gs_base(address: u64) {
    unsafe {
        write_msr(KERNEL_GS_BASE, address);
    }
}

pub fn user_gs_base() -> u64 {
    read_msr(KERNEL_GS_BASE)
}

pub fn reset_user_bases() {
    unsafe {
        write_msr(FS_BASE, 0);
        write_msr(GS_BASE, 0);
    }
}

#[unsafe(naked)]
unsafe extern "C" fn syscall_entry() -> ! {
    naked_asm!(
        "swapgs",
        "mov qword ptr gs:[8], rsp",
        "mov rsp, qword ptr gs:[0]",
        "push r11",
        "push rcx",
        "push rbx",
        "push rbp",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "push r9",
        "push r8",
        "push r10",
        "push rdx",
        "push rsi",
        "push rdi",
        "push rax",
        "mov rbx, rsp",
        "mov rcx, rsp",
        "and rsp, -16",
        "sub rsp, 32",
        "call {dispatch}",
        "test rax, rax",
        "jnz {process_exit}",
        "mov rsp, rbx",
        "pop rax",
        "pop rdi",
        "pop rsi",
        "pop rdx",
        "pop r10",
        "pop r8",
        "pop r9",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbp",
        "pop rbx",
        "pop rcx",
        "pop r11",
        "mov rsp, qword ptr gs:[8]",
        "swapgs",
        "sysretq",
        dispatch = sym crate::syscall::aeros_linux_syscall_dispatch,
        process_exit = sym syscall_process_exit,
    )
}

#[unsafe(naked)]
unsafe extern "C" fn syscall_process_exit() -> ! {
    naked_asm!("swapgs", "jmp {}", sym user::return_from_user)
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
