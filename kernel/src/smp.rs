use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::acpi::{AcpiInfo, MAX_PROCESSORS};
use crate::arch;
use crate::arch::paging::PagingState;

const PAGE_SIZE: usize = 4096;
const STACK_SIZE: usize = 32 * 1024;
const PROTECTED_OFFSET: usize = 0x40;
const LONG_OFFSET: usize = 0x100;
const GDT_OFFSET: usize = 0x200;
const GDT_POINTER_OFFSET: usize = 0x228;

#[repr(align(4096))]
struct ApStacks(UnsafeCell<[[u8; STACK_SIZE]; MAX_PROCESSORS]>);

#[repr(align(64))]
struct CpuCounter(AtomicU64);

unsafe impl Sync for ApStacks {}

static AP_STACKS: ApStacks = ApStacks(UnsafeCell::new([[0; STACK_SIZE]; MAX_PROCESSORS]));
static ONLINE: AtomicU64 = AtomicU64::new(0);
static FAILED: AtomicU64 = AtomicU64::new(0);
static IDLE: AtomicU64 = AtomicU64::new(0);
static WORK: [CpuCounter; MAX_PROCESSORS] =
    [const { CpuCounter(AtomicU64::new(0)) }; MAX_PROCESSORS];
static OBSERVED_IDS: [AtomicU32; MAX_PROCESSORS] =
    [const { AtomicU32::new(u32::MAX) }; MAX_PROCESSORS];
static TRAMPOLINE: AtomicU64 = AtomicU64::new(0);
static QUEUE_DISPATCHES: AtomicU64 = AtomicU64::new(0);
static QUEUE_COMPLETIONS: AtomicU64 = AtomicU64::new(0);
static QUEUE_RESULTS: [AtomicU64; MAX_PROCESSORS] = [const { AtomicU64::new(0) }; MAX_PROCESSORS];

#[repr(align(64))]
struct CpuJob {
    function: AtomicU64,
    argument: AtomicU64,
    state: AtomicU32,
}

static JOBS: [CpuJob; MAX_PROCESSORS] = [const {
    CpuJob {
        function: AtomicU64::new(0),
        argument: AtomicU64::new(0),
        state: AtomicU32::new(0),
    }
}; MAX_PROCESSORS];

#[derive(Clone, Copy)]
pub struct SmpReport {
    pub discovered: u8,
    pub applications_started: u8,
    pub online: u8,
    pub bsp_id: u32,
    pub last_ap_id: u32,
    pub work: u64,
    pub trampoline: u64,
    pub stage: u8,
    pub gdt_stage: u8,
    pub idle: u8,
    pub ipi_acks: u64,
    pub queue_dispatches: u64,
    pub queue_completions: u64,
    pub work_queue: bool,
    pub verified: bool,
}

pub fn initialize(acpi: &AcpiInfo, paging: &PagingState, trampoline: u64) -> SmpReport {
    ONLINE.store(1, Ordering::Release);
    TRAMPOLINE.store(trampoline, Ordering::Release);
    FAILED.store(0, Ordering::Release);
    IDLE.store(0, Ordering::Release);
    QUEUE_DISPATCHES.store(0, Ordering::Release);
    QUEUE_COMPLETIONS.store(0, Ordering::Release);
    arch::interrupts::reset_ipi_acks();
    for slot in &WORK {
        slot.0.store(0, Ordering::Release);
    }
    for slot in &OBSERVED_IDS {
        slot.store(u32::MAX, Ordering::Release);
    }
    for slot in &QUEUE_RESULTS {
        slot.store(0, Ordering::Release);
    }
    for job in &JOBS {
        job.function.store(0, Ordering::Release);
        job.argument.store(0, Ordering::Release);
        job.state.store(0, Ordering::Release);
    }
    let bsp_id = arch::apic::current_id();
    OBSERVED_IDS[0].store(bsp_id, Ordering::Release);
    let discovered = acpi.stored_processor_count;
    if trampoline == 0
        || trampoline >= 0x10_0000
        || trampoline & 0xfff != 0
        || paging.root_physical > u32::MAX as u64
        || discovered == 0
    {
        return failed_report(discovered, bsp_id, trampoline);
    }
    let mut applications_started = 0u8;
    let mut logical = 1usize;
    for apic_id in &acpi.enabled_apic_ids[..discovered as usize] {
        if *apic_id == bsp_id {
            continue;
        }
        if logical >= MAX_PROCESSORS {
            break;
        }
        let stack = unsafe { (*AP_STACKS.0.get())[logical].as_ptr().add(STACK_SIZE) as u64 & !15 };
        if !build_trampoline(
            trampoline,
            paging.root_physical,
            stack,
            logical as u32,
            *apic_id,
            paging.nx_enabled,
        ) {
            FAILED.fetch_or(1 << logical, Ordering::AcqRel);
            break;
        }
        core::sync::atomic::fence(Ordering::SeqCst);
        if !arch::apic::start_processor(*apic_id, trampoline) {
            FAILED.fetch_or(1 << logical, Ordering::AcqRel);
            break;
        }
        applications_started += 1;
        if !wait_for(
            || ONLINE.load(Ordering::Acquire) & (1 << logical) != 0,
            500_000_000,
        ) {
            FAILED.fetch_or(1 << logical, Ordering::AcqRel);
            break;
        }
        logical += 1;
    }
    let _ = wait_for(
        || IDLE.load(Ordering::Acquire).count_ones() as usize == logical.saturating_sub(1),
        2_000_000_000,
    );
    for apic_id in &acpi.enabled_apic_ids[..discovered as usize] {
        if *apic_id != bsp_id && !arch::apic::send_fixed(*apic_id, 49) {
            FAILED.fetch_or(1 << 63, Ordering::AcqRel);
        }
    }
    let _ = wait_for(
        || arch::interrupts::ipi_acks() == applications_started as u64,
        500_000_000,
    );
    let online = ONLINE.load(Ordering::Acquire).count_ones() as u8;
    let work = WORK[1..logical]
        .iter()
        .map(|counter| counter.0.load(Ordering::Acquire))
        .sum();
    let last_ap_id = if logical > 1 {
        OBSERVED_IDS[logical - 1].load(Ordering::Acquire)
    } else {
        bsp_id
    };
    let stage = unsafe { core::ptr::read_volatile((trampoline + 0x300) as usize as *const u8) };
    let idle = IDLE.load(Ordering::Acquire).count_ones() as u8;
    let ipi_acks = arch::interrupts::ipi_acks();
    let work_queue = work_queue_self_test(logical);
    let queue_dispatches = QUEUE_DISPATCHES.load(Ordering::Acquire);
    let queue_completions = QUEUE_COMPLETIONS.load(Ordering::Acquire);
    let verified = acpi.enabled_processor_count == discovered as u16
        && discovered as usize <= MAX_PROCESSORS
        && applications_started.saturating_add(1) == discovered
        && online == discovered
        && FAILED.load(Ordering::Acquire) == 0
        && (discovered == 1
            || work != 0
                && idle == applications_started
                && ipi_acks == applications_started as u64)
        && work_queue;
    SmpReport {
        discovered,
        applications_started,
        online,
        bsp_id,
        last_ap_id,
        work,
        trampoline,
        stage,
        gdt_stage: arch::gdt::stage(1),
        idle,
        ipi_acks,
        queue_dispatches,
        queue_completions,
        work_queue,
        verified,
    }
}

pub fn online_mask() -> u64 {
    ONLINE.load(Ordering::Acquire)
}

fn failed_report(discovered: u8, bsp_id: u32, trampoline: u64) -> SmpReport {
    SmpReport {
        discovered,
        applications_started: 0,
        online: 1,
        bsp_id,
        last_ap_id: bsp_id,
        work: 0,
        trampoline,
        stage: if trampoline != 0 && trampoline < 0x10_0000 {
            unsafe { core::ptr::read_volatile((trampoline + 0x300) as usize as *const u8) }
        } else {
            0
        },
        gdt_stage: arch::gdt::stage(1),
        idle: 0,
        ipi_acks: 0,
        queue_dispatches: 0,
        queue_completions: 0,
        work_queue: false,
        verified: false,
    }
}

fn work_queue_self_test(logical_count: usize) -> bool {
    if logical_count <= 1 {
        return true;
    }
    for logical in 1..logical_count {
        if !submit(logical, queue_test_job, logical as u64) {
            return false;
        }
    }
    for (logical, result) in QUEUE_RESULTS.iter().enumerate().take(logical_count).skip(1) {
        if !wait_job(logical) || result.load(Ordering::Acquire) != (logical as u64 + 1) * 1000 {
            return false;
        }
    }
    QUEUE_DISPATCHES.load(Ordering::Acquire) == (logical_count - 1) as u64
        && QUEUE_COMPLETIONS.load(Ordering::Acquire) == (logical_count - 1) as u64
}

fn submit(logical: usize, function: fn(u64), argument: u64) -> bool {
    if logical == 0
        || logical >= MAX_PROCESSORS
        || ONLINE.load(Ordering::Acquire) & (1 << logical) == 0
        || JOBS[logical]
            .state
            .compare_exchange(0, 3, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return false;
    }
    JOBS[logical]
        .function
        .store(function as usize as u64, Ordering::Release);
    JOBS[logical].argument.store(argument, Ordering::Release);
    JOBS[logical].state.store(1, Ordering::Release);
    let apic_id = OBSERVED_IDS[logical].load(Ordering::Acquire);
    if apic_id == u32::MAX || !arch::apic::send_fixed(apic_id, 51) {
        JOBS[logical].state.store(0, Ordering::Release);
        return false;
    }
    QUEUE_DISPATCHES.fetch_add(1, Ordering::AcqRel);
    true
}

/// Starts `function(argument)` on logical CPU `logical` if it is idle. The job
/// runs to completion from the work IPI handler on that CPU.
#[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
pub fn start_job(logical: usize, function: fn(u64), argument: u64) -> bool {
    submit(logical, function, argument)
}

/// Frees the CPU's job slot once its last job has finished (true if it had).
#[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
pub fn reap_job(logical: usize) -> bool {
    let Some(job) = JOBS.get(logical) else {
        return false;
    };
    if job
        .state
        .compare_exchange(2, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    job.function.store(0, Ordering::Release);
    job.argument.store(0, Ordering::Release);
    true
}

fn wait_job(logical: usize) -> bool {
    if !wait_for(
        || JOBS[logical].state.load(Ordering::Acquire) == 2,
        1_000_000_000,
    ) {
        return false;
    }
    JOBS[logical].function.store(0, Ordering::Release);
    JOBS[logical].argument.store(0, Ordering::Release);
    JOBS[logical].state.store(0, Ordering::Release);
    true
}

fn queue_test_job(logical: u64) {
    if let Some(result) = QUEUE_RESULTS.get(logical as usize) {
        result.store((logical + 1) * 1000, Ordering::Release);
    }
}

pub fn handle_work_ipi() {
    let apic_id = arch::apic::current_id();
    let Some(logical) = OBSERVED_IDS
        .iter()
        .position(|observed| observed.load(Ordering::Acquire) == apic_id)
    else {
        return;
    };
    if JOBS[logical]
        .state
        .compare_exchange(1, 3, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    let function = JOBS[logical].function.load(Ordering::Acquire);
    let argument = JOBS[logical].argument.load(Ordering::Acquire);
    if function == 0 {
        JOBS[logical].state.store(0, Ordering::Release);
        return;
    }
    let function: fn(u64) = unsafe { core::mem::transmute(function as usize) };
    function(argument);
    QUEUE_COMPLETIONS.fetch_add(1, Ordering::AcqRel);
    JOBS[logical].state.store(2, Ordering::Release);
}

fn wait_for(mut condition: impl FnMut() -> bool, timeout: u64) -> bool {
    let start = crate::time::monotonic_nanoseconds();
    loop {
        if condition() {
            return true;
        }
        if crate::time::monotonic_nanoseconds().wrapping_sub(start) >= timeout {
            return false;
        }
        core::hint::spin_loop();
    }
}

fn build_trampoline(
    physical: u64,
    root: u64,
    stack: u64,
    logical: u32,
    apic_id: u32,
    nx: bool,
) -> bool {
    let Ok(protected) = u32::try_from(physical + PROTECTED_OFFSET as u64) else {
        return false;
    };
    let Ok(long) = u32::try_from(physical + LONG_OFFSET as u64) else {
        return false;
    };
    let Ok(root) = u32::try_from(root) else {
        return false;
    };
    let Ok(gdt) = u32::try_from(physical + GDT_OFFSET as u64) else {
        return false;
    };
    let mut page = [0u8; PAGE_SIZE];
    let mut cursor = 0usize;
    if !emit(
        &mut page,
        &mut cursor,
        &[
            0xfa, 0xfc, 0x0e, 0x1f, 0xc6, 0x06, 0x00, 0x03, 0x01, 0x0f, 0x01, 0x16,
        ],
    ) || !emit_u16(&mut page, &mut cursor, GDT_POINTER_OFFSET as u16)
        || !emit(
            &mut page,
            &mut cursor,
            &[
                0x66, 0x0f, 0x20, 0xc0, 0x66, 0x83, 0xc8, 0x01, 0x66, 0x0f, 0x22, 0xc0, 0x66, 0xea,
            ],
        )
        || !emit_u32(&mut page, &mut cursor, protected)
        || !emit_u16(&mut page, &mut cursor, 0x08)
    {
        return false;
    }
    cursor = PROTECTED_OFFSET;
    if !emit(
        &mut page,
        &mut cursor,
        &[
            0x66, 0xb8, 0x10, 0x00, 0x8e, 0xd8, 0x8e, 0xc0, 0x8e, 0xd0, 0x0f, 0x20, 0xe0, 0x83,
            0xc8, 0x20, 0x0f, 0x22, 0xe0, 0xb8,
        ],
    ) || !emit_u32(&mut page, &mut cursor, root)
        || !emit(
            &mut page,
            &mut cursor,
            &[
                0x0f, 0x22, 0xd8, 0xb9, 0x80, 0x00, 0x00, 0xc0, 0x0f, 0x32, 0x0d,
            ],
        )
        || !emit_u32(&mut page, &mut cursor, if nx { 0x900 } else { 0x100 })
        || !emit(
            &mut page,
            &mut cursor,
            &[
                0x0f, 0x30, 0x0f, 0x20, 0xc0, 0x0d, 0x00, 0x00, 0x01, 0x80, 0x0f, 0x22, 0xc0, 0xea,
            ],
        )
        || !emit_u32(&mut page, &mut cursor, long)
        || !emit_u16(&mut page, &mut cursor, 0x18)
    {
        return false;
    }
    cursor = LONG_OFFSET;
    if !emit(
        &mut page,
        &mut cursor,
        &[
            0x66, 0xb8, 0x20, 0x00, 0x8e, 0xd8, 0x8e, 0xc0, 0x8e, 0xd0, 0x48, 0xbc,
        ],
    ) || !emit_u64(&mut page, &mut cursor, stack)
        || !emit(&mut page, &mut cursor, &[0x31, 0xed, 0xbf])
        || !emit_u32(&mut page, &mut cursor, logical)
        || !emit(&mut page, &mut cursor, &[0xbe])
        || !emit_u32(&mut page, &mut cursor, apic_id)
        || !emit(&mut page, &mut cursor, &[0x48, 0xb8])
        || !emit_u64(
            &mut page,
            &mut cursor,
            aeros_ap_entry as *const () as usize as u64,
        )
        || !emit(&mut page, &mut cursor, &[0xff, 0xd0, 0x0f, 0x0b])
    {
        return false;
    }
    put_u64(&mut page, GDT_OFFSET + 8, 0x00cf_9a00_0000_ffff);
    put_u64(&mut page, GDT_OFFSET + 16, 0x00cf_9200_0000_ffff);
    put_u64(&mut page, GDT_OFFSET + 24, 0x00af_9a00_0000_ffff);
    put_u64(&mut page, GDT_OFFSET + 32, 0x00cf_9200_0000_ffff);
    put_u16(&mut page, GDT_POINTER_OFFSET, 39);
    put_u32(&mut page, GDT_POINTER_OFFSET + 2, gdt);
    unsafe {
        core::ptr::copy_nonoverlapping(page.as_ptr(), physical as usize as *mut u8, PAGE_SIZE);
    }
    true
}

fn emit(destination: &mut [u8; PAGE_SIZE], cursor: &mut usize, source: &[u8]) -> bool {
    let Some(end) = cursor.checked_add(source.len()) else {
        return false;
    };
    if end > destination.len() {
        return false;
    }
    destination[*cursor..end].copy_from_slice(source);
    *cursor = end;
    true
}

fn emit_u16(destination: &mut [u8; PAGE_SIZE], cursor: &mut usize, value: u16) -> bool {
    emit(destination, cursor, &value.to_le_bytes())
}

fn emit_u32(destination: &mut [u8; PAGE_SIZE], cursor: &mut usize, value: u32) -> bool {
    emit(destination, cursor, &value.to_le_bytes())
}

fn emit_u64(destination: &mut [u8; PAGE_SIZE], cursor: &mut usize, value: u64) -> bool {
    emit(destination, cursor, &value.to_le_bytes())
}

fn put_u16(destination: &mut [u8; PAGE_SIZE], offset: usize, value: u16) {
    destination[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(destination: &mut [u8; PAGE_SIZE], offset: usize, value: u32) {
    destination[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(destination: &mut [u8; PAGE_SIZE], offset: usize, value: u64) {
    destination[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

#[unsafe(no_mangle)]
extern "sysv64" fn aeros_ap_entry(logical: u32, expected_apic_id: u32) -> ! {
    arch::disable_interrupts();
    let trampoline = TRAMPOLINE.load(Ordering::Acquire);
    if trampoline != 0 {
        unsafe {
            core::ptr::write_volatile((trampoline + 0x300) as usize as *mut u8, 7);
        }
    }
    let logical = logical as usize;
    if logical >= MAX_PROCESSORS {
        loop {
            core::hint::spin_loop();
        }
    }
    let descriptors = arch::gdt::init_for_cpu(logical);
    arch::interrupts::load();
    let apic_id = arch::apic::initialize_secondary();
    let cpu = arch::CpuInfo::detect();
    let fpu = arch::fpu::initialize(&cpu);
    let syscall = arch::syscall_entry::init_for_cpu(logical, &cpu);
    OBSERVED_IDS[logical].store(apic_id, Ordering::Release);
    if !descriptors.loaded || !fpu.verified || !syscall.verified || apic_id != expected_apic_id {
        FAILED.fetch_or(1 << logical, Ordering::AcqRel);
        loop {
            core::hint::spin_loop();
        }
    }
    ONLINE.fetch_or(1 << logical, Ordering::AcqRel);
    for _ in 0..100_000 {
        WORK[logical].0.fetch_add(1, Ordering::Relaxed);
        core::hint::spin_loop();
    }
    IDLE.fetch_or(1 << (logical - 1), Ordering::AcqRel);
    loop {
        unsafe {
            core::arch::asm!("sti; hlt", options(nomem, nostack));
        }
    }
}
