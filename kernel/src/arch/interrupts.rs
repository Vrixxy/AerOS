use core::arch::{asm, naked_asm};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::{arch, serial};

use super::{gdt::KERNEL_CODE_SELECTOR, user};

const IDT_ENTRIES: usize = 256;
static BREAKPOINT_HIT: AtomicBool = AtomicBool::new(false);
static TIMER_TICKS: AtomicU64 = AtomicU64::new(0);
static LOCAL_TIMER_TICKS: AtomicU64 = AtomicU64::new(0);
static IPI_ACKS: AtomicU64 = AtomicU64::new(0);
static IOAPIC_TIMER_TICKS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
#[repr(C)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    attributes: u8,
    offset_middle: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const MISSING: Self = Self {
        offset_low: 0,
        selector: 0,
        ist: 0,
        attributes: 0,
        offset_middle: 0,
        offset_high: 0,
        reserved: 0,
    };

    fn interrupt(handler: u64, ist: u8, privilege: u8) -> Self {
        Self {
            offset_low: handler as u16,
            selector: KERNEL_CODE_SELECTOR,
            ist: ist & 0x07,
            attributes: 0x8e | ((privilege & 0x03) << 5),
            offset_middle: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            reserved: 0,
        }
    }
}

#[repr(C, packed)]
struct DescriptorTablePointer {
    limit: u16,
    base: u64,
}

#[repr(align(16))]
struct IdtStorage(UnsafeCell<[IdtEntry; IDT_ENTRIES]>);

unsafe impl Sync for IdtStorage {}

static IDT: IdtStorage = IdtStorage(UnsafeCell::new([IdtEntry::MISSING; IDT_ENTRIES]));

#[repr(C)]
pub struct InterruptFrame {
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    r11: u64,
    r10: u64,
    r9: u64,
    r8: u64,
    rdi: u64,
    rsi: u64,
    rbp: u64,
    rdx: u64,
    rcx: u64,
    rbx: u64,
    rax: u64,
    vector: u64,
    error: u64,
    rip: u64,
    cs: u64,
    rflags: u64,
}

impl InterruptFrame {
    pub fn fork_snapshot(&self) -> Option<user::ForkSnapshot> {
        if self.cs & 3 != 3 {
            return None;
        }
        let base = self as *const InterruptFrame as *const u64;
        let rsp = unsafe { core::ptr::read(base.add(20)) };
        Some(user::ForkSnapshot {
            r15: self.r15,
            r14: self.r14,
            r13: self.r13,
            r12: self.r12,
            r11: self.r11,
            r10: self.r10,
            r9: self.r9,
            r8: self.r8,
            rdi: self.rdi,
            rsi: self.rsi,
            rbp: self.rbp,
            rdx: self.rdx,
            rbx: self.rbx,
            rcx: self.rcx,
            rip: self.rip,
            rflags: self.rflags,
            rsp,
        })
    }

    pub fn set_return(&mut self, rip: u64, rsp: u64) {
        self.rip = rip;
        let base = self as *mut InterruptFrame as *mut u64;
        unsafe {
            core::ptr::write(base.add(20), rsp);
        }
    }
}

macro_rules! vector_without_error {
    ($name:ident, $vector:literal) => {
        #[unsafe(naked)]
        unsafe extern "C" fn $name() -> ! {
            naked_asm!(
                "push 0",
                "push {vector}",
                "jmp {common}",
                vector = const $vector,
                common = sym interrupt_common,
            )
        }
    };
}

macro_rules! vector_with_error {
    ($name:ident, $vector:literal) => {
        #[unsafe(naked)]
        unsafe extern "C" fn $name() -> ! {
            naked_asm!(
                "push {vector}",
                "jmp {common}",
                vector = const $vector,
                common = sym interrupt_common,
            )
        }
    };
}

#[unsafe(naked)]
unsafe extern "C" fn interrupt_common() -> ! {
    naked_asm!(
        "push rax",
        "push rbx",
        "push rcx",
        "push rdx",
        "push rbp",
        "push rsi",
        "push rdi",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "cld",
        "mov rbx, rsp",
        "mov rcx, rsp",
        "and rsp, -16",
        "sub rsp, 32",
        "call {dispatch}",
        "test rax, rax",
        "jnz {user_return}",
        "mov rsp, rbx",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rdi",
        "pop rsi",
        "pop rbp",
        "pop rdx",
        "pop rcx",
        "pop rbx",
        "pop rax",
        "add rsp, 16",
        "iretq",
        dispatch = sym aeros_interrupt_dispatch,
        user_return = sym user::return_from_user,
    )
}

vector_without_error!(vector_0, 0);
vector_without_error!(vector_1, 1);
vector_without_error!(vector_2, 2);
vector_without_error!(vector_3, 3);
vector_without_error!(vector_4, 4);
vector_without_error!(vector_5, 5);
vector_without_error!(vector_6, 6);
vector_without_error!(vector_7, 7);
vector_with_error!(vector_8, 8);
vector_without_error!(vector_9, 9);
vector_with_error!(vector_10, 10);
vector_with_error!(vector_11, 11);
vector_with_error!(vector_12, 12);
vector_with_error!(vector_13, 13);
vector_with_error!(vector_14, 14);
vector_without_error!(vector_15, 15);
vector_without_error!(vector_16, 16);
vector_with_error!(vector_17, 17);
vector_without_error!(vector_18, 18);
vector_without_error!(vector_19, 19);
vector_without_error!(vector_20, 20);
vector_with_error!(vector_21, 21);
vector_without_error!(vector_22, 22);
vector_without_error!(vector_23, 23);
vector_without_error!(vector_24, 24);
vector_without_error!(vector_25, 25);
vector_without_error!(vector_26, 26);
vector_without_error!(vector_27, 27);
vector_without_error!(vector_28, 28);
vector_with_error!(vector_29, 29);
vector_with_error!(vector_30, 30);
vector_without_error!(vector_31, 31);
vector_without_error!(vector_32, 32);
vector_without_error!(vector_33, 33);
vector_without_error!(vector_34, 34);
vector_without_error!(vector_35, 35);
vector_without_error!(vector_36, 36);
vector_without_error!(vector_37, 37);
vector_without_error!(vector_38, 38);
vector_without_error!(vector_39, 39);
vector_without_error!(vector_40, 40);
vector_without_error!(vector_41, 41);
vector_without_error!(vector_42, 42);
vector_without_error!(vector_43, 43);
vector_without_error!(vector_44, 44);
vector_without_error!(vector_45, 45);
vector_without_error!(vector_46, 46);
vector_without_error!(vector_47, 47);
vector_without_error!(vector_48, 48);
vector_without_error!(vector_49, 49);
vector_without_error!(vector_50, 50);
vector_without_error!(vector_51, 51);
vector_without_error!(vector_52, 52);
vector_without_error!(vector_53, 53);
vector_without_error!(vector_128, 128);
vector_without_error!(vector_default, 255);

pub fn init() {
    unsafe {
        arch::outb(0x21, 0xff);
        arch::outb(0xa1, 0xff);
        let entries = &mut *IDT.0.get();
        let default = IdtEntry::interrupt(vector_default as *const () as usize as u64, 0, 0);
        for entry in entries.iter_mut() {
            *entry = default;
        }
        set(entries, 0, vector_0, 0, 0);
        set(entries, 1, vector_1, 0, 0);
        set(entries, 2, vector_2, 0, 0);
        set(entries, 3, vector_3, 0, 3);
        set(entries, 4, vector_4, 0, 3);
        set(entries, 5, vector_5, 0, 0);
        set(entries, 6, vector_6, 0, 0);
        set(entries, 7, vector_7, 0, 0);
        set(entries, 8, vector_8, 1, 0);
        set(entries, 9, vector_9, 0, 0);
        set(entries, 10, vector_10, 0, 0);
        set(entries, 11, vector_11, 0, 0);
        set(entries, 12, vector_12, 0, 0);
        set(entries, 13, vector_13, 0, 0);
        set(entries, 14, vector_14, 0, 0);
        set(entries, 15, vector_15, 0, 0);
        set(entries, 16, vector_16, 0, 0);
        set(entries, 17, vector_17, 0, 0);
        set(entries, 18, vector_18, 0, 0);
        set(entries, 19, vector_19, 0, 0);
        set(entries, 20, vector_20, 0, 0);
        set(entries, 21, vector_21, 0, 0);
        set(entries, 22, vector_22, 0, 0);
        set(entries, 23, vector_23, 0, 0);
        set(entries, 24, vector_24, 0, 0);
        set(entries, 25, vector_25, 0, 0);
        set(entries, 26, vector_26, 0, 0);
        set(entries, 27, vector_27, 0, 0);
        set(entries, 28, vector_28, 0, 0);
        set(entries, 29, vector_29, 0, 0);
        set(entries, 30, vector_30, 0, 0);
        set(entries, 31, vector_31, 0, 0);
        set(entries, 32, vector_32, 0, 0);
        set(entries, 33, vector_33, 0, 0);
        set(entries, 34, vector_34, 0, 0);
        set(entries, 35, vector_35, 0, 0);
        set(entries, 36, vector_36, 0, 0);
        set(entries, 37, vector_37, 0, 0);
        set(entries, 38, vector_38, 0, 0);
        set(entries, 39, vector_39, 0, 0);
        set(entries, 40, vector_40, 0, 0);
        set(entries, 41, vector_41, 0, 0);
        set(entries, 42, vector_42, 0, 0);
        set(entries, 43, vector_43, 0, 0);
        set(entries, 44, vector_44, 0, 0);
        set(entries, 45, vector_45, 0, 0);
        set(entries, 46, vector_46, 0, 0);
        set(entries, 47, vector_47, 0, 0);
        set(entries, 48, vector_48, 0, 0);
        set(entries, 49, vector_49, 0, 0);
        set(entries, 50, vector_50, 0, 0);
        set(entries, 51, vector_51, 0, 0);
        set(entries, 52, vector_52, 0, 0);
        set(entries, 53, vector_53, 0, 0);
        set(entries, 128, vector_128, 0, 3);
    }
    load();
}

pub fn load() {
    let pointer = DescriptorTablePointer {
        limit: (core::mem::size_of::<[IdtEntry; IDT_ENTRIES]>() - 1) as u16,
        base: IDT.0.get() as u64,
    };
    unsafe {
        asm!("lidt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));
    }
}

pub fn self_test() -> bool {
    BREAKPOINT_HIT.store(false, Ordering::Release);
    unsafe {
        asm!("int3", options(nomem, nostack));
    }
    BREAKPOINT_HIT.load(Ordering::Acquire)
}

extern "C" fn aeros_interrupt_dispatch(frame: *mut InterruptFrame) -> u64 {
    let frame = unsafe { &mut *frame };
    match frame.vector {
        3 => {
            BREAKPOINT_HIT.store(true, Ordering::Release);
            0
        }
        32 => {
            TIMER_TICKS.fetch_add(1, Ordering::Release);
            end_legacy_interrupt(32);
            crate::scheduler::on_timer_tick();
            0
        }
        33..=47 => {
            end_legacy_interrupt(frame.vector as u8);
            0
        }
        48 => {
            LOCAL_TIMER_TICKS.fetch_add(1, Ordering::Release);
            crate::arch::apic::end_interrupt();
            crate::scheduler::on_timer_tick();
            0
        }
        49 => {
            IPI_ACKS.fetch_add(1, Ordering::Release);
            crate::arch::apic::end_interrupt();
            0
        }
        50 => {
            IOAPIC_TIMER_TICKS.fetch_add(1, Ordering::Release);
            crate::arch::apic::end_interrupt();
            0
        }
        51 => {
            crate::arch::apic::end_interrupt();
            crate::smp::handle_work_ipi();
            0
        }
        52 => {
            crate::keyboard::handle_interrupt();
            crate::arch::apic::end_interrupt();
            0
        }
        53 => {
            crate::mouse::handle_interrupt();
            crate::arch::apic::end_interrupt();
            0
        }
        255 => 0,
        128 => {
            let arguments = [
                frame.rdi, frame.rsi, frame.rdx, frame.r10, frame.r8, frame.r9,
            ];
            match crate::syscall::dispatch(frame.rax, arguments) {
                crate::syscall::BootstrapResult::Return(value) => {
                    frame.rax = value;
                    0
                }
                crate::syscall::BootstrapResult::Exit(code) => {
                    user::set_exit_code(code);
                    1
                }
                crate::syscall::BootstrapResult::Fork => {
                    let child = frame
                        .fork_snapshot()
                        .and_then(|snapshot| crate::scheduler::fork_current_user_task(&snapshot));
                    frame.rax = child.unwrap_or(u64::MAX);
                    0
                }
                crate::syscall::BootstrapResult::Exec { bytes, length } => {
                    match crate::scheduler::exec_current_user_task(&bytes[..length]) {
                        Some((entry, stack_top)) => {
                            frame.set_return(entry, stack_top);
                        }
                        None => {
                            frame.rax = 0u64.wrapping_sub(8);
                        }
                    }
                    0
                }
                crate::syscall::BootstrapResult::Wait { pid } => {
                    frame.rax = match crate::scheduler::wait_for_child(pid) {
                        Some(status) => status,
                        None => 0u64.wrapping_sub(10),
                    };
                    0
                }
                crate::syscall::BootstrapResult::Kill { pid } => {
                    frame.rax = match crate::scheduler::kill_task(pid) {
                        Ok(()) => 0,
                        Err(()) => 0u64.wrapping_sub(3),
                    };
                    0
                }
                crate::syscall::BootstrapResult::CurrentId => {
                    frame.rax = crate::scheduler::current_task_id();
                    0
                }
                crate::syscall::BootstrapResult::ParentId => {
                    frame.rax = crate::scheduler::current_parent_id();
                    0
                }
                crate::syscall::BootstrapResult::Grow { pages } => {
                    frame.rax = match crate::scheduler::grow_current_heap(pages) {
                        Some(brk) => brk,
                        None => 0u64.wrapping_sub(12),
                    };
                    0
                }
                crate::syscall::BootstrapResult::Write { bytes, length } => {
                    for byte in &bytes[..length] {
                        crate::serial::byte(*byte);
                    }
                    frame.rax = length as u64;
                    0
                }
            }
        }
        _ => {
            let fault_address = if frame.vector == 14 { read_cr2() } else { 0 };
            if frame.vector == 14
                && frame.error & 1 == 0
                && super::paging::handle_demand_fault(fault_address)
            {
                return 0;
            }
            if frame.cs & 3 == 3 && frame.vector < 32 {
                user::terminate_fault(frame.vector, frame.error, fault_address);
                return 1;
            }
            serial::format(format_args!(
                "AEROS_EXCEPTION vector={} error={:#x} rip={:#x} cs={:#x} rflags={:#x} cr2={:#x}\n",
                frame.vector, frame.error, frame.rip, frame.cs, frame.rflags, fault_address
            ));
            arch::halt_forever();
        }
    }
}

pub fn timer_self_test(frequency: u32, required_ticks: u64) -> Option<u64> {
    if required_ticks == 0 || !start_legacy_timer(frequency) {
        return None;
    }
    while TIMER_TICKS.load(Ordering::Acquire) < required_ticks {
        unsafe {
            asm!("hlt", options(nomem, nostack));
        }
    }
    stop_legacy_timer();
    Some(TIMER_TICKS.load(Ordering::Acquire))
}

pub fn reset_local_timer_ticks() {
    LOCAL_TIMER_TICKS.store(0, Ordering::Release);
}

pub fn local_timer_ticks() -> u64 {
    LOCAL_TIMER_TICKS.load(Ordering::Acquire)
}

pub fn reset_ipi_acks() {
    IPI_ACKS.store(0, Ordering::Release);
}

pub fn ipi_acks() -> u64 {
    IPI_ACKS.load(Ordering::Acquire)
}

pub fn ioapic_timer_self_test(frequency: u32, required_ticks: u64) -> Option<u64> {
    if required_ticks == 0 || !(19..=1_193_182).contains(&frequency) {
        return None;
    }
    IOAPIC_TIMER_TICKS.store(0, Ordering::Release);
    let divisor = (1_193_182u32 / frequency).clamp(1, u16::MAX as u32) as u16;
    unsafe {
        arch::outb(0x21, 0xff);
        arch::outb(0xa1, 0xff);
        arch::outb(0x43, 0x36);
        arch::outb(0x40, divisor as u8);
        arch::outb(0x40, (divisor >> 8) as u8);
        asm!("sti", options(nomem, nostack, preserves_flags));
    }
    let start = crate::time::monotonic_nanoseconds();
    while IOAPIC_TIMER_TICKS.load(Ordering::Acquire) < required_ticks {
        if crate::time::monotonic_nanoseconds().wrapping_sub(start) > 500_000_000 {
            arch::disable_interrupts();
            return None;
        }
        unsafe {
            asm!("hlt", options(nomem, nostack));
        }
    }
    arch::disable_interrupts();
    Some(IOAPIC_TIMER_TICKS.load(Ordering::Acquire))
}

pub fn start_legacy_timer(frequency: u32) -> bool {
    if !(19..=1_193_182).contains(&frequency) {
        return false;
    }
    let divisor = (1_193_182u32 / frequency).clamp(1, u16::MAX as u32) as u16;
    TIMER_TICKS.store(0, Ordering::Release);
    unsafe {
        remap_legacy_pic();
        arch::outb(0x21, 0xfe);
        arch::outb(0xa1, 0xff);
        arch::outb(0x43, 0x36);
        arch::outb(0x40, divisor as u8);
        arch::outb(0x40, (divisor >> 8) as u8);
        asm!("sti", options(nomem, nostack, preserves_flags));
    }
    true
}

pub fn stop_legacy_timer() {
    unsafe {
        asm!("cli", options(nomem, nostack, preserves_flags));
        arch::outb(0x21, 0xff);
    }
}

unsafe fn set(
    entries: &mut [IdtEntry; IDT_ENTRIES],
    vector: usize,
    handler: unsafe extern "C" fn() -> !,
    ist: u8,
    privilege: u8,
) {
    entries[vector] = IdtEntry::interrupt(handler as *const () as usize as u64, ist, privilege);
}

fn end_legacy_interrupt(vector: u8) {
    unsafe {
        if vector >= 40 {
            arch::outb(0xa0, 0x20);
        }
        arch::outb(0x20, 0x20);
    }
}

unsafe fn remap_legacy_pic() {
    unsafe {
        arch::outb(0x20, 0x11);
        io_wait();
        arch::outb(0xa0, 0x11);
        io_wait();
        arch::outb(0x21, 0x20);
        io_wait();
        arch::outb(0xa1, 0x28);
        io_wait();
        arch::outb(0x21, 0x04);
        io_wait();
        arch::outb(0xa1, 0x02);
        io_wait();
        arch::outb(0x21, 0x01);
        io_wait();
        arch::outb(0xa1, 0x01);
        io_wait();
    }
}

unsafe fn io_wait() {
    unsafe {
        arch::outb(0x80, 0);
    }
}

fn read_cr2() -> u64 {
    let address: u64;
    unsafe {
        asm!("mov {}, cr2", out(reg) address, options(nomem, nostack, preserves_flags));
    }
    address
}
