use core::arch::asm;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU8, Ordering};

pub const KERNEL_CODE_SELECTOR: u16 = 0x08;
pub const KERNEL_DATA_SELECTOR: u16 = 0x10;
pub const USER_DATA_SELECTOR: u16 = 0x1b;
pub const USER_CODE_SELECTOR: u16 = 0x23;
pub const TSS_SELECTOR: u16 = 0x28;

const GDT_ENTRIES: usize = 7;
const TSS_SIZE: usize = 104;
const RING0_STACK_SIZE: usize = 32 * 1024;
const DOUBLE_FAULT_STACK_SIZE: usize = 32 * 1024;
const MAX_CPUS: usize = 8;

#[repr(C, packed)]
struct DescriptorTablePointer {
    limit: u16,
    base: u64,
}

#[repr(align(16))]
struct GdtStorage(UnsafeCell<[u64; GDT_ENTRIES]>);

#[repr(align(16))]
struct TssStorage(UnsafeCell<[u8; TSS_SIZE]>);

#[repr(align(4096))]
struct StackStorage<const N: usize>(UnsafeCell<[u8; N]>);

unsafe impl Sync for GdtStorage {}
unsafe impl Sync for TssStorage {}
unsafe impl<const N: usize> Sync for StackStorage<N> {}

static GDTS: [GdtStorage; MAX_CPUS] =
    [const { GdtStorage(UnsafeCell::new([0; GDT_ENTRIES])) }; MAX_CPUS];
static TSSES: [TssStorage; MAX_CPUS] =
    [const { TssStorage(UnsafeCell::new([0; TSS_SIZE])) }; MAX_CPUS];
static RING0_STACKS: [StackStorage<RING0_STACK_SIZE>; MAX_CPUS] =
    [const { StackStorage(UnsafeCell::new([0; RING0_STACK_SIZE])) }; MAX_CPUS];
static DOUBLE_FAULT_STACKS: [StackStorage<DOUBLE_FAULT_STACK_SIZE>; MAX_CPUS] =
    [const { StackStorage(UnsafeCell::new([0; DOUBLE_FAULT_STACK_SIZE])) }; MAX_CPUS];
static STAGES: [AtomicU8; MAX_CPUS] = [const { AtomicU8::new(0) }; MAX_CPUS];

pub struct GdtState {
    pub loaded: bool,
    pub kernel_code: u16,
    pub kernel_data: u16,
    pub user_code: u16,
    pub user_data: u16,
    pub task: u16,
}

pub fn init() -> GdtState {
    init_for_cpu(0)
}

pub fn init_for_cpu(cpu: usize) -> GdtState {
    if cpu >= MAX_CPUS {
        return GdtState {
            loaded: false,
            kernel_code: KERNEL_CODE_SELECTOR,
            kernel_data: KERNEL_DATA_SELECTOR,
            user_code: USER_CODE_SELECTOR,
            user_data: USER_DATA_SELECTOR,
            task: TSS_SELECTOR,
        };
    }
    STAGES[cpu].store(1, Ordering::Release);
    unsafe {
        let ring0_top = RING0_STACKS[cpu].0.get().cast::<u8>().add(RING0_STACK_SIZE) as u64;
        let double_fault_top = DOUBLE_FAULT_STACKS[cpu]
            .0
            .get()
            .cast::<u8>()
            .add(DOUBLE_FAULT_STACK_SIZE) as u64;
        write_tss_u64(cpu, 4, ring0_top);
        write_tss_u64(cpu, 36, double_fault_top);
        write_tss_u16(cpu, 102, TSS_SIZE as u16);

        let tss_base = TSSES[cpu].0.get().cast::<u8>() as u64;
        let tss_limit = (TSS_SIZE - 1) as u64;
        let tss_low = (tss_limit & 0xffff)
            | (tss_base & 0xffff) << 16
            | ((tss_base >> 16) & 0xff) << 32
            | 0x89u64 << 40
            | ((tss_limit >> 16) & 0x0f) << 48
            | ((tss_base >> 24) & 0xff) << 56;
        let tss_high = tss_base >> 32;
        let entries = &mut *GDTS[cpu].0.get();
        entries[0] = 0;
        entries[1] = 0x00af_9a00_0000_ffff;
        entries[2] = 0x00cf_9200_0000_ffff;
        entries[3] = 0x00cf_f200_0000_ffff;
        entries[4] = 0x00af_fa00_0000_ffff;
        entries[5] = tss_low;
        entries[6] = tss_high;
        STAGES[cpu].store(2, Ordering::Release);

        let pointer = DescriptorTablePointer {
            limit: (core::mem::size_of::<[u64; GDT_ENTRIES]>() - 1) as u16,
            base: entries.as_ptr() as u64,
        };
        asm!("lgdt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));
        STAGES[cpu].store(3, Ordering::Release);
        asm!(
            "push {code}",
            "lea rax, [rip + 2f]",
            "push rax",
            "retfq",
            "2:",
            "mov ax, {data:x}",
            "mov ds, ax",
            "mov es, ax",
            "mov ss, ax",
            "xor eax, eax",
            "mov fs, ax",
            "mov gs, ax",
            code = const KERNEL_CODE_SELECTOR,
            data = in(reg) KERNEL_DATA_SELECTOR,
            out("rax") _,
            options(preserves_flags)
        );
        STAGES[cpu].store(4, Ordering::Release);
        asm!("ltr {selector:x}", selector = in(reg) TSS_SELECTOR, options(nostack, preserves_flags));
        STAGES[cpu].store(5, Ordering::Release);

        let code: u16;
        let task: u16;
        asm!("mov {value:x}, cs", value = out(reg) code, options(nomem, nostack, preserves_flags));
        asm!("str {value:x}", value = out(reg) task, options(nomem, nostack, preserves_flags));
        STAGES[cpu].store(6, Ordering::Release);
        GdtState {
            loaded: code == KERNEL_CODE_SELECTOR && task == TSS_SELECTOR,
            kernel_code: KERNEL_CODE_SELECTOR,
            kernel_data: KERNEL_DATA_SELECTOR,
            user_code: USER_CODE_SELECTOR,
            user_data: USER_DATA_SELECTOR,
            task: TSS_SELECTOR,
        }
    }
}

pub fn set_ring0_stack(cpu: usize, top: u64) {
    if cpu >= MAX_CPUS {
        return;
    }
    unsafe {
        write_tss_u64(cpu, 4, top);
    }
}

pub fn default_ring0_stack(cpu: usize) -> u64 {
    if cpu >= MAX_CPUS {
        return 0;
    }
    unsafe { RING0_STACKS[cpu].0.get().cast::<u8>().add(RING0_STACK_SIZE) as u64 }
}

pub fn stage(cpu: usize) -> u8 {
    STAGES
        .get(cpu)
        .map(|stage| stage.load(Ordering::Acquire))
        .unwrap_or(0)
}

unsafe fn write_tss_u64(cpu: usize, offset: usize, value: u64) {
    unsafe {
        core::ptr::write_unaligned(
            TSSES[cpu].0.get().cast::<u8>().add(offset).cast::<u64>(),
            value,
        );
    }
}

unsafe fn write_tss_u16(cpu: usize, offset: usize, value: u16) {
    unsafe {
        core::ptr::write_unaligned(
            TSSES[cpu].0.get().cast::<u8>().add(offset).cast::<u16>(),
            value,
        );
    }
}
