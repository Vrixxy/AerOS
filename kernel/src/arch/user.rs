use core::arch::{asm, naked_asm};
use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::CpuInfo;

use super::gdt::{KERNEL_DATA_SELECTOR, USER_CODE_SELECTOR, USER_DATA_SELECTOR};
use super::paging::{UserImageMapping, UserMapping};

const CR4_SMEP: u64 = 1 << 20;
const CR4_SMAP: u64 = 1 << 21;
const FS_BASE_MSR: u32 = 0xc000_0100;

static USER_RETURN_RSP: AtomicU64 = AtomicU64::new(0);
static USER_EXIT_CODE: AtomicU64 = AtomicU64::new(0);
static ACTIVE_USER_START: AtomicU64 = AtomicU64::new(0);
static ACTIVE_USER_END: AtomicU64 = AtomicU64::new(0);
static SMAP_ACTIVE: AtomicU64 = AtomicU64::new(0);
static USER_FAULTS: AtomicU64 = AtomicU64::new(0);
static LAST_FAULT_VECTOR: AtomicU64 = AtomicU64::new(0);
static LAST_FAULT_ERROR: AtomicU64 = AtomicU64::new(0);
static LAST_FAULT_ADDRESS: AtomicU64 = AtomicU64::new(0);

pub struct UserProbeResult {
    pub exit_code: u64,
    pub smep_enabled: bool,
    pub smap_enabled: bool,
    pub verified: bool,
}

#[derive(Clone, Copy)]
pub struct UserFaultStats {
    pub faults: u64,
    pub last_vector: u64,
    pub last_error: u64,
    pub last_address: u64,
}

#[derive(Clone, Copy)]
pub struct UserTaskContext {
    return_rsp: u64,
    exit_code: u64,
    active_start: u64,
    active_end: u64,
    smap_active: u64,
}

impl UserTaskContext {
    pub const EMPTY: Self = Self {
        return_rsp: 0,
        exit_code: 0,
        active_start: 0,
        active_end: 0,
        smap_active: 0,
    };
}

pub fn save_task_context() -> UserTaskContext {
    UserTaskContext {
        return_rsp: USER_RETURN_RSP.load(Ordering::Acquire),
        exit_code: USER_EXIT_CODE.load(Ordering::Acquire),
        active_start: ACTIVE_USER_START.load(Ordering::Acquire),
        active_end: ACTIVE_USER_END.load(Ordering::Acquire),
        smap_active: SMAP_ACTIVE.load(Ordering::Acquire),
    }
}

pub fn restore_task_context(context: &UserTaskContext) {
    USER_RETURN_RSP.store(context.return_rsp, Ordering::Release);
    USER_EXIT_CODE.store(context.exit_code, Ordering::Release);
    ACTIVE_USER_START.store(context.active_start, Ordering::Release);
    ACTIVE_USER_END.store(context.active_end, Ordering::Release);
    SMAP_ACTIVE.store(context.smap_active, Ordering::Release);
}

pub fn run_scheduled_task(entry: u64, stack: u64, user_start: u64, user_end: u64) -> u64 {
    let cpu = CpuInfo::detect();
    let (_, smap_enabled) = enable_user_protections(&cpu);
    super::syscall_entry::reset_user_bases();
    ACTIVE_USER_START.store(user_start, Ordering::Release);
    ACTIVE_USER_END.store(user_end, Ordering::Release);
    SMAP_ACTIVE.store(smap_enabled as u64, Ordering::Release);
    USER_EXIT_CODE.store(u64::MAX, Ordering::Release);
    let exit_code = unsafe { enter_user(entry, stack) };
    super::syscall_entry::reset_user_bases();
    ACTIVE_USER_START.store(0, Ordering::Release);
    ACTIVE_USER_END.store(0, Ordering::Release);
    exit_code
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct ForkSnapshot {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rdx: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rip: u64,
    pub rflags: u64,
    pub rsp: u64,
}

pub fn resume_forked_child(snapshot: &ForkSnapshot, user_start: u64, user_end: u64) -> u64 {
    let cpu = CpuInfo::detect();
    let (_, smap_enabled) = enable_user_protections(&cpu);
    super::syscall_entry::reset_user_bases();
    ACTIVE_USER_START.store(user_start, Ordering::Release);
    ACTIVE_USER_END.store(user_end, Ordering::Release);
    SMAP_ACTIVE.store(smap_enabled as u64, Ordering::Release);
    USER_EXIT_CODE.store(u64::MAX, Ordering::Release);
    let exit_code = unsafe { enter_user_from_snapshot(snapshot) };
    super::syscall_entry::reset_user_bases();
    ACTIVE_USER_START.store(0, Ordering::Release);
    ACTIVE_USER_END.store(0, Ordering::Release);
    exit_code
}

pub fn run_probe(mapping: &UserMapping, cpu: &CpuInfo) -> UserProbeResult {
    super::paging::deactivate_user_memory();
    run(
        mapping.entry,
        mapping.stack_top,
        mapping.entry,
        mapping.entry + 3 * 4096,
        mapping.verified,
        cpu,
        42,
    )
}

pub fn run_image(mapping: &UserImageMapping, cpu: &CpuInfo, expected: u64) -> UserProbeResult {
    super::paging::activate_user_memory(mapping);
    run(
        mapping.entry,
        mapping.stack_top,
        mapping.load_bias,
        mapping.load_bias + 512 * 4096,
        mapping.verified,
        cpu,
        expected,
    )
}

fn run(
    entry: u64,
    stack: u64,
    user_start: u64,
    user_end: u64,
    mapping_valid: bool,
    cpu: &CpuInfo,
    expected: u64,
) -> UserProbeResult {
    let (smep_enabled, smap_enabled) = enable_user_protections(cpu);
    super::syscall_entry::reset_user_bases();
    ACTIVE_USER_START.store(user_start, Ordering::Release);
    ACTIVE_USER_END.store(user_end, Ordering::Release);
    SMAP_ACTIVE.store(smap_enabled as u64, Ordering::Release);
    USER_EXIT_CODE.store(u64::MAX, Ordering::Release);
    let exit_code = unsafe { enter_user(entry, stack) };
    crate::syscall::finish_process();
    super::syscall_entry::reset_user_bases();
    ACTIVE_USER_START.store(0, Ordering::Release);
    ACTIVE_USER_END.store(0, Ordering::Release);
    SMAP_ACTIVE.store(0, Ordering::Release);
    UserProbeResult {
        exit_code,
        smep_enabled,
        smap_enabled,
        verified: mapping_valid
            && exit_code == expected
            && (!cpu.smep || smep_enabled)
            && (!cpu.smap || smap_enabled),
    }
}

pub fn set_thread_base(gs: bool, address: u64) -> bool {
    if address != 0 && (address > 0x0000_7fff_ffff_ffff || !range_within_active(address, 1)) {
        return false;
    }
    if gs {
        super::syscall_entry::set_user_gs_base(address);
    } else {
        unsafe {
            write_msr(FS_BASE_MSR, address);
        }
    }
    true
}

pub fn thread_base(gs: bool) -> u64 {
    if gs {
        super::syscall_entry::user_gs_base()
    } else {
        read_msr(FS_BASE_MSR)
    }
}

pub fn range_accessible(address: u64, length: usize, write: bool) -> bool {
    if length == 0 {
        return true;
    }
    let start = ACTIVE_USER_START.load(Ordering::Acquire);
    let end = ACTIVE_USER_END.load(Ordering::Acquire);
    let Some(request_end) = address.checked_add(length as u64) else {
        return false;
    };
    address >= start
        && request_end <= end
        && request_end >= address
        && super::paging::user_range_accessible(address, length, write)
}

fn range_within_active(address: u64, length: usize) -> bool {
    let start = ACTIVE_USER_START.load(Ordering::Acquire);
    let end = ACTIVE_USER_END.load(Ordering::Acquire);
    address
        .checked_add(length as u64)
        .is_some_and(|request_end| address >= start && request_end <= end)
}

pub fn copy_from_user(address: u64, destination: &mut [u8]) -> bool {
    if !range_accessible(address, destination.len(), false) {
        return false;
    }
    set_user_access(true);
    unsafe {
        core::ptr::copy_nonoverlapping(
            address as usize as *const u8,
            destination.as_mut_ptr(),
            destination.len(),
        );
    }
    set_user_access(false);
    true
}

pub fn copy_to_user(address: u64, source: &[u8]) -> bool {
    if !range_accessible(address, source.len(), true) {
        return false;
    }
    set_user_access(true);
    unsafe {
        core::ptr::copy_nonoverlapping(source.as_ptr(), address as usize as *mut u8, source.len());
    }
    set_user_access(false);
    true
}

pub fn copy_string(address: u64, destination: &mut [u8]) -> Option<usize> {
    for index in 0..destination.len() {
        if !copy_from_user(
            address.checked_add(index as u64)?,
            &mut destination[index..=index],
        ) {
            return None;
        }
        if destination[index] == 0 {
            return Some(index);
        }
    }
    None
}

fn set_user_access(enabled: bool) {
    if SMAP_ACTIVE.load(Ordering::Acquire) == 0 {
        return;
    }
    unsafe {
        if enabled {
            asm!("stac", options(nomem, nostack, preserves_flags));
        } else {
            asm!("clac", options(nomem, nostack, preserves_flags));
        }
    }
}

pub fn set_exit_code(code: u64) {
    USER_EXIT_CODE.store(code, Ordering::Release);
}

pub fn terminate_fault(vector: u64, error: u64, address: u64) {
    USER_FAULTS.fetch_add(1, Ordering::Relaxed);
    LAST_FAULT_VECTOR.store(vector, Ordering::Release);
    LAST_FAULT_ERROR.store(error, Ordering::Release);
    LAST_FAULT_ADDRESS.store(address, Ordering::Release);
    let signal = match vector {
        0 => 8,
        1 | 3 | 4 => 5,
        6 => 4,
        17 => 7,
        _ => 11,
    };
    set_exit_code(128 + signal);
}

pub fn fault_stats() -> UserFaultStats {
    UserFaultStats {
        faults: USER_FAULTS.load(Ordering::Acquire),
        last_vector: LAST_FAULT_VECTOR.load(Ordering::Acquire),
        last_error: LAST_FAULT_ERROR.load(Ordering::Acquire),
        last_address: LAST_FAULT_ADDRESS.load(Ordering::Acquire),
    }
}

fn enable_user_protections(cpu: &CpuInfo) -> (bool, bool) {
    let mut value: u64;
    unsafe {
        asm!("mov {}, cr4", out(reg) value, options(nomem, nostack, preserves_flags));
        if cpu.smep {
            value |= CR4_SMEP;
        }
        if cpu.smap {
            value |= CR4_SMAP;
        }
        asm!("mov cr4, {}", in(reg) value, options(nostack, preserves_flags));
        asm!("mov {}, cr4", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    (value & CR4_SMEP != 0, value & CR4_SMAP != 0)
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

#[unsafe(naked)]
unsafe extern "C" fn enter_user(entry: u64, stack: u64) -> u64 {
    naked_asm!(
        "push rbp",
        "push rbx",
        "push rsi",
        "push rdi",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov qword ptr [rip + {return_rsp}], rsp",
        "mov ax, {user_data}",
        "mov ds, ax",
        "mov es, ax",
        "push {user_data}",
        "push rdx",
        "push 0x202",
        "push {user_code}",
        "push rcx",
        "iretq",
        return_rsp = sym USER_RETURN_RSP,
        user_data = const USER_DATA_SELECTOR,
        user_code = const USER_CODE_SELECTOR,
    )
}

#[unsafe(naked)]
unsafe extern "C" fn enter_user_from_snapshot(snapshot: *const ForkSnapshot) -> u64 {
    naked_asm!(
        "push rbp",
        "push rbx",
        "push rsi",
        "push rdi",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov qword ptr [rip + {return_rsp}], rsp",
        "mov ax, {user_data}",
        "mov ds, ax",
        "mov es, ax",
        "push {user_data}",
        "push qword ptr [rcx + 128]",
        "push qword ptr [rcx + 120]",
        "push {user_code}",
        "push qword ptr [rcx + 112]",
        "mov rax, qword ptr [rcx + 104]",
        "mov r15, qword ptr [rcx + 0]",
        "mov r14, qword ptr [rcx + 8]",
        "mov r13, qword ptr [rcx + 16]",
        "mov r12, qword ptr [rcx + 24]",
        "mov r11, qword ptr [rcx + 32]",
        "mov r10, qword ptr [rcx + 40]",
        "mov r9, qword ptr [rcx + 48]",
        "mov r8, qword ptr [rcx + 56]",
        "mov rdi, qword ptr [rcx + 64]",
        "mov rsi, qword ptr [rcx + 72]",
        "mov rbp, qword ptr [rcx + 80]",
        "mov rdx, qword ptr [rcx + 88]",
        "mov rbx, qword ptr [rcx + 96]",
        "mov rcx, rax",
        "xor eax, eax",
        "iretq",
        return_rsp = sym USER_RETURN_RSP,
        user_data = const USER_DATA_SELECTOR,
        user_code = const USER_CODE_SELECTOR,
    )
}

#[unsafe(naked)]
pub(super) unsafe extern "C" fn return_from_user() -> ! {
    naked_asm!(
        "mov rsp, qword ptr [rip + {return_rsp}]",
        "mov ax, {kernel_data}",
        "mov ds, ax",
        "mov es, ax",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rdi",
        "pop rsi",
        "pop rbx",
        "pop rbp",
        "mov rax, qword ptr [rip + {exit_code}]",
        "ret",
        return_rsp = sym USER_RETURN_RSP,
        exit_code = sym USER_EXIT_CODE,
        kernel_data = const KERNEL_DATA_SELECTOR,
    )
}
