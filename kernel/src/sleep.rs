//! ACPI S3, suspend to RAM. The memory keeps its contents and everything else
//! may lose its state: `suspend` parks the other processors, records what the
//! boot processor needs, points the firmware's waking vector at a real-mode
//! trampoline (the one the other processors start from), and writes the sleep
//! type to the PM1 control register. On wake the firmware jumps to the
//! trampoline, which reaches long mode and calls `resume_entry`; that puts the
//! processor, the interrupt controllers and the clocks back and returns into
//! `suspend` as if `save_context` had returned a second time.
//!
//! What this does not do is bring devices back. A virtual machine's devices
//! are reset by a wake, a real machine's lose power, and each driver would
//! need its own resume step; none has one yet, so a sleep ends with the
//! processor, interrupt, timer and ACPI state restored and the drivers as they
//! were left (usable only where the hardware keeps its state).

use core::arch::{asm, naked_asm};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::acpi::AcpiInfo;
use crate::arch::paging::PagingState;
use crate::arch::{self, CpuInfo};
use crate::sync::TicketLock;
use crate::{acpi_events, acpi_ns, ioapic, microcode, rtc, scheduler, serial, smp, time};

const STACK_BYTES: usize = 32 * 1024;
const SLP_EN: u16 = 1 << 13;
const WAK_STS: u16 = 1 << 15;
const SLP_TYP_MASK: u16 = 0x1c00;
const MSR_PAT: u32 = 0x277;
const MSR_MTRR_CAP: u32 = 0xfe;
const MSR_MTRR_DEFAULT: u32 = 0x2ff;
const MTRR_FIXED: [u32; 11] = [
    0x250, 0x258, 0x259, 0x268, 0x269, 0x26a, 0x26b, 0x26c, 0x26d, 0x26e, 0x26f,
];
const MAX_VARIABLE: usize = 16;
/// How long the processor waits for the machine to go to sleep.
const ENTRY_TIMEOUT_NS: u64 = 3_000_000_000;

#[repr(C)]
struct Context {
    rbx: u64,
    rbp: u64,
    r12: u64,
    r13: u64,
    r14: u64,
    r15: u64,
    rsp: u64,
    rip: u64,
}

struct Shared<T>(UnsafeCell<T>);
unsafe impl<T> Sync for Shared<T> {}

#[repr(align(4096))]
struct Stack(UnsafeCell<[u8; STACK_BYTES]>);
unsafe impl Sync for Stack {}

static CONTEXT: Shared<Context> = Shared(UnsafeCell::new(Context {
    rbx: 0,
    rbp: 0,
    r12: 0,
    r13: 0,
    r14: 0,
    r15: 0,
    rsp: 0,
    rip: 0,
}));
static RESUME_STACK: Stack = Stack(UnsafeCell::new([0; STACK_BYTES]));

#[derive(Clone, Copy)]
struct Mtrrs {
    valid: bool,
    default: u64,
    fixed: [u64; MTRR_FIXED.len()],
    variable: [(u64, u64); MAX_VARIABLE],
    count: usize,
    pat: u64,
}

impl Mtrrs {
    const EMPTY: Mtrrs = Mtrrs {
        valid: false,
        default: 0,
        fixed: [0; MTRR_FIXED.len()],
        variable: [(0, 0); MAX_VARIABLE],
        count: 0,
        pat: 0,
    };
}

#[derive(Clone, Copy)]
struct Saved {
    acpi: AcpiInfo,
    cr0: u64,
    cr3: u64,
    cr4: u64,
    ring0_stack: u64,
    monotonic: u64,
    ioapic: ioapic::Table,
    timer_hz: u32,
    mtrrs: Mtrrs,
}

impl Saved {
    const EMPTY: Saved = Saved {
        acpi: AcpiInfo::MISSING,
        cr0: 0,
        cr3: 0,
        cr4: 0,
        ring0_stack: 0,
        monotonic: 0,
        ioapic: ioapic::Table::EMPTY,
        timer_hz: 0,
        mtrrs: Mtrrs::EMPTY,
    };
}

static SAVED: Shared<Saved> = Shared(UnsafeCell::new(Saved::EMPTY));
static RESUMED: AtomicBool = AtomicBool::new(false);
static ACPI: TicketLock<Option<AcpiInfo>> = TicketLock::new(None);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    /// The firmware names no S3 type, the tables are unusable, or the
    /// trampoline page cannot reach the page tables.
    Unsupported,
    /// A program is running on another processor.
    Busy,
    /// A processor did not stop.
    CpuStuck,
    /// The machine did not go to sleep.
    NotEntered,
}

#[derive(Clone, Copy, Debug)]
pub struct Resumed {
    /// Processors that came back, the boot processor included.
    pub processors: usize,
    pub expected: usize,
}

/// Remembers the firmware's tables for later sleeps.
pub fn configure(acpi: &AcpiInfo) {
    *ACPI.lock() = Some(*acpi);
}

fn facs_valid(acpi: &AcpiInfo) -> bool {
    let address = acpi.facs_address;
    if address == 0 {
        return false;
    }
    let facs = address as usize as *const u8;
    // SAFETY: the FADT names this table; it is only read.
    arch::paging::with_kernel_tables(|| unsafe {
        core::ptr::read_volatile(facs.cast::<[u8; 4]>()) == *b"FACS"
            && core::ptr::read_volatile(facs.add(4).cast::<u32>()) >= 32
    })
}

fn paging_root() -> u64 {
    arch::paging::boot_state().map_or(0, |state: PagingState| state.root_physical)
}

/// Why a sleep cannot be attempted, or `Ok` when it can.
pub fn check() -> Result<(), Error> {
    let Some(acpi) = *ACPI.lock() else {
        return Err(Error::Unsupported);
    };
    let trampoline = smp::trampoline_page();
    let usable = acpi.fadt_valid
        && facs_valid(&acpi)
        && acpi_ns::report().loaded
        && acpi_ns::sleep_type(3).is_some()
        && trampoline != 0
        && trampoline < 0x10_0000
        && paging_root() != 0
        && paging_root() <= u64::from(u32::MAX);
    if usable {
        Ok(())
    } else {
        Err(Error::Unsupported)
    }
}

#[unsafe(naked)]
unsafe extern "sysv64" fn save_context(context: *mut Context) -> u64 {
    naked_asm!(
        "mov [rdi], rbx",
        "mov [rdi + 8], rbp",
        "mov [rdi + 16], r12",
        "mov [rdi + 24], r13",
        "mov [rdi + 32], r14",
        "mov [rdi + 40], r15",
        "lea rax, [rsp + 8]",
        "mov [rdi + 48], rax",
        "mov rax, [rsp]",
        "mov [rdi + 56], rax",
        "xor eax, eax",
        "ret",
    )
}

#[unsafe(naked)]
unsafe extern "sysv64" fn restore_context(context: *const Context) -> ! {
    naked_asm!(
        "mov rbx, [rdi]",
        "mov rbp, [rdi + 8]",
        "mov r12, [rdi + 16]",
        "mov r13, [rdi + 24]",
        "mov r14, [rdi + 32]",
        "mov r15, [rdi + 40]",
        "mov rsp, [rdi + 48]",
        "mov eax, 1",
        "jmp [rdi + 56]",
    )
}

fn read_msr(register: u32) -> u64 {
    let (low, high): (u32, u32);
    unsafe {
        asm!("rdmsr", in("ecx") register, out("eax") low, out("edx") high, options(nomem, nostack, preserves_flags));
    }
    (u64::from(high) << 32) | u64::from(low)
}

unsafe fn write_msr(register: u32, value: u64) {
    unsafe {
        asm!("wrmsr", in("ecx") register, in("eax") value as u32, in("edx") (value >> 32) as u32, options(nomem, nostack, preserves_flags));
    }
}

fn control_registers() -> (u64, u64, u64) {
    let (cr0, cr3, cr4): (u64, u64, u64);
    unsafe {
        asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
        asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
        asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
    }
    (cr0, cr3, cr4)
}

fn has_mtrr() -> bool {
    let edx = core::arch::x86_64::__cpuid(1).edx;
    edx & (1 << 12) != 0
}

/// The memory-type registers. Read only on real hardware: they matter there
/// (a processor that has lost power starts with caching off) and an emulator
/// would not need them.
fn save_mtrrs() -> Mtrrs {
    if microcode::virtualised() || !has_mtrr() {
        return Mtrrs::EMPTY;
    }
    let capabilities = read_msr(MSR_MTRR_CAP);
    let count = ((capabilities & 0xff) as usize).min(MAX_VARIABLE);
    let mut saved = Mtrrs {
        valid: true,
        default: read_msr(MSR_MTRR_DEFAULT),
        count,
        pat: read_msr(MSR_PAT),
        ..Mtrrs::EMPTY
    };
    if capabilities & (1 << 8) != 0 {
        for (slot, register) in saved.fixed.iter_mut().zip(MTRR_FIXED) {
            *slot = read_msr(register);
        }
    }
    for index in 0..count {
        saved.variable[index] = (
            read_msr(0x200 + 2 * index as u32),
            read_msr(0x201 + 2 * index as u32),
        );
    }
    saved
}

/// Writes the registers back the way the SDM asks: caching off, the
/// registers disabled while they change, then everything on again.
fn restore_mtrrs(saved: &Mtrrs) {
    if !saved.valid {
        return;
    }
    let capabilities = read_msr(MSR_MTRR_CAP);
    unsafe {
        let (cr0, _, _) = control_registers();
        asm!("mov cr0, {}", in(reg) (cr0 | (1 << 30)) & !(1 << 29), options(nostack, preserves_flags));
        asm!("wbinvd", options(nostack, preserves_flags));
        write_msr(MSR_MTRR_DEFAULT, 0);
        if capabilities & (1 << 8) != 0 {
            for (value, register) in saved.fixed.iter().zip(MTRR_FIXED) {
                write_msr(register, *value);
            }
        }
        for (index, (base, mask)) in saved.variable[..saved.count].iter().enumerate() {
            write_msr(0x200 + 2 * index as u32, *base);
            write_msr(0x201 + 2 * index as u32, *mask);
        }
        write_msr(MSR_PAT, saved.pat);
        write_msr(MSR_MTRR_DEFAULT, saved.default);
        asm!("wbinvd", options(nostack, preserves_flags));
        asm!("mov cr0, {}", in(reg) cr0, options(nostack, preserves_flags));
    }
}

/// Called by each application processor as it starts: after a sleep it takes
/// the boot processor's memory-type registers.
pub fn prepare_application_processor() {
    if RESUMED.load(Ordering::Acquire) {
        // SAFETY: written once before the sleep, read-only afterwards.
        restore_mtrrs(unsafe { &(*SAVED.0.get()).mtrrs });
    }
}

fn stage(value: u8) {
    let page = smp::trampoline_page();
    if page != 0 && page < 0x10_0000 {
        // SAFETY: the trampoline page is reserved for this.
        unsafe { core::ptr::write_volatile((page + 0x300) as usize as *mut u8, value) };
    }
}

/// Where the firmware's resume path lands, in long mode, on the resume stack.
extern "sysv64" fn resume_entry() -> ! {
    serial::line("AEROS_S3_RESUME_ENTRY");
    stage(10);
    let saved = unsafe { *SAVED.0.get() };
    let acpi = saved.acpi;
    let cpu = CpuInfo::detect();
    unsafe {
        asm!("mov cr4, {}", in(reg) saved.cr4, options(nostack, preserves_flags));
        asm!("mov cr0, {}", in(reg) saved.cr0, options(nostack, preserves_flags));
    }
    arch::gdt::init_for_cpu(0);
    arch::interrupts::load();
    arch::gdt::set_ring0_stack(0, saved.ring0_stack);
    stage(11);
    let fpu = arch::fpu::initialize(&cpu);
    crate::kernel_lock::init_cpu_id(0, cpu.rdtscp);
    let syscall = arch::syscall_entry::init_for_cpu(0, &cpu);
    arch::user::enable_protections_here();
    restore_mtrrs(&saved.mtrrs);
    unsafe { asm!("mov cr3, {}", in(reg) saved.cr3, options(nostack, preserves_flags)) };
    stage(12);
    serial::init();
    let apic = arch::apic::resume_boot_cpu();
    let routed = ioapic::restore(&acpi, &saved.ioapic);
    unsafe {
        arch::outb(0x21, 0xff);
        arch::outb(0xa1, 0xff);
    }
    time::resume(saved.monotonic);
    let clock = rtc::initialize().verified;
    let events = acpi_events::initialize(&acpi, arch::apic::current_id());
    stage(13);
    serial::format(format_args!(
        "AEROS_S3_RESTORED fpu={} syscall={} apic={} ioapic={} clock={} sci={}\n",
        fpu.verified,
        syscall.verified,
        apic,
        routed,
        clock,
        events.acpi_mode && events.routed
    ));
    RESUMED.store(true, Ordering::Release);
    unsafe { restore_context(CONTEXT.0.get()) }
}

/// Writes the sleep type into the PM1 control registers, the second write
/// carrying the enable bit, after clearing a stale wake status.
unsafe fn enter(acpi: &AcpiInfo, types: (u8, u8)) {
    unsafe {
        asm!("wbinvd", options(nostack, preserves_flags));
        for block in [acpi.pm1a_event_block, acpi.pm1b_event_block] {
            if block != 0 && block <= 0xffff {
                arch::outw(block as u16, WAK_STS);
            }
        }
        let blocks = [
            (acpi.pm1a_control_block, types.0),
            (acpi.pm1b_control_block, types.1),
        ];
        for step in 0..2 {
            for (block, typ) in blocks {
                if block == 0 || block > 0xffff {
                    continue;
                }
                let port = block as u16;
                let value = arch::inw(port) & !(SLP_TYP_MASK | SLP_EN) | (u16::from(typ) << 10);
                arch::outw(port, if step == 0 { value } else { value | SLP_EN });
            }
        }
    }
}

/// Saves the boot processor's context, goes to sleep and, on the second
/// return, is the processor waking up. Returns true after a wake.
#[inline(never)]
fn sleep_here(acpi: &AcpiInfo, types: (u8, u8)) -> bool {
    // SAFETY: only this thread touches the context, with interrupts off.
    if unsafe { save_context(CONTEXT.0.get()) } == 0 {
        unsafe { enter(acpi, types) };
        let start = time::monotonic_nanoseconds();
        while time::monotonic_nanoseconds().wrapping_sub(start) < ENTRY_TIMEOUT_NS {
            core::hint::spin_loop();
        }
        return false;
    }
    true
}

fn tables_for_wake(acpi: &AcpiInfo, trampoline: u64, root: u64, nx: bool) -> bool {
    let facs = acpi.facs_address as usize as *mut u8;
    let stack = RESUME_STACK.0.get().cast::<u8>() as u64 + STACK_BYTES as u64;
    if !smp::build_trampoline_for(
        trampoline,
        root,
        stack & !15,
        nx,
        resume_entry as *const () as usize as u64,
    ) {
        return false;
    }
    // SAFETY: the FACS was checked; the real-mode vector is a 32-bit field and
    // the 64-bit one stays zero.
    unsafe {
        core::ptr::write_volatile(facs.add(12).cast::<u32>(), trampoline as u32);
        core::ptr::write_volatile(facs.add(24).cast::<u64>(), 0);
        if core::ptr::read_volatile(facs.add(4).cast::<u32>()) >= 40 {
            core::ptr::write_volatile(facs.add(36).cast::<u32>(), 0);
        }
    }
    true
}

/// Puts the machine to sleep and returns after it has woken.
/// How each driver came back (a device that is not there counts as fine).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct DriverResume {
    pub sata: bool,
    pub nvme: bool,
    pub virtio_disk: bool,
    pub sd: bool,
    pub usb: bool,
    pub ethernet: bool,
    pub virtio_net: bool,
    pub audio: bool,
    pub gpu: bool,
    pub touch: bool,
    pub keyboard: bool,
    pub mouse: bool,
}

impl DriverResume {
    #[cfg_attr(not(feature = "boot-test"), allow(dead_code))]
    pub fn all_ok(&self) -> bool {
        self.sata
            && self.nvme
            && self.virtio_disk
            && self.sd
            && self.usb
            && self.ethernet
            && self.virtio_net
            && self.audio
            && self.gpu
            && self.touch
    }
}

/// Starts every driver again over the memory it already owns: the hardware
/// reset, its rings and registers programmed from scratch. After a real sleep
/// the devices have lost their state; the boot test calls it on a running
/// system, which resets and restarts them the same way.
pub fn resume_drivers() -> DriverResume {
    let sata = crate::ahci::resume();
    let nvme = crate::nvme::resume();
    let virtio_disk = crate::virtio_blk::resume();
    let sd = crate::sdhci::resume();
    let usb = crate::xhci::resume();
    let ethernet = crate::e1000::resume() & crate::rtl8139::resume() & crate::rtl8168::resume();
    let virtio_net = crate::virtio_net::resume();
    let audio = crate::hda::reinitialize() & crate::ac97::reinitialize();
    let gpu = crate::virtio_gpu::resume();
    let touch = crate::virtio_input::resume();
    // No PS/2 controller is not a failure either.
    let keyboard = crate::keyboard::resume();
    let mouse = crate::mouse::resume();
    let report = DriverResume {
        sata,
        nvme,
        virtio_disk,
        sd,
        usb,
        ethernet,
        virtio_net,
        audio,
        gpu,
        touch,
        keyboard,
        mouse,
    };
    serial::format(format_args!(
        "AEROS_DRIVER_RESUME sata={} nvme={} virtio_blk={} sd={} usb={} ethernet={} virtio_net={} audio={} gpu={} touch={} keyboard={} mouse={}\n",
        report.sata,
        report.nvme,
        report.virtio_disk,
        report.sd,
        report.usb,
        report.ethernet,
        report.virtio_net,
        report.audio,
        report.gpu,
        report.touch,
        report.keyboard,
        report.mouse
    ));
    report
}

pub fn suspend() -> Result<Resumed, Error> {
    check()?;
    let acpi = (*ACPI.lock()).ok_or(Error::Unsupported)?;
    let types = acpi_ns::sleep_type(3).ok_or(Error::Unsupported)?;
    if scheduler::cpus_busy() {
        return Err(Error::Busy);
    }
    let expected = smp::online_mask().count_ones() as usize;
    if !smp::park_all() {
        smp::restart_all();
        return Err(Error::CpuStuck);
    }
    let flags: u64;
    unsafe { asm!("pushfq", "pop {}", out(reg) flags, options(preserves_flags)) };
    arch::disable_interrupts();
    let (cr0, cr3, cr4) = control_registers();
    let root = paging_root();
    let trampoline = smp::trampoline_page();
    let nx = arch::paging::boot_state().is_some_and(|state| state.nx_enabled);
    let saved = Saved {
        acpi,
        cr0,
        cr3,
        cr4,
        ring0_stack: arch::gdt::ring0_stack(0),
        monotonic: time::monotonic_nanoseconds(),
        ioapic: ioapic::save(&acpi),
        timer_hz: arch::apic::timer_frequency(),
        mtrrs: save_mtrrs(),
    };
    arch::apic::save_state();
    crate::pci::save_config();
    // SAFETY: nothing else runs: the other processors are parked and
    // interrupts are off.
    unsafe { *SAVED.0.get() = saved };
    RESUMED.store(false, Ordering::Release);
    let woke = arch::paging::with_kernel_tables(|| tables_for_wake(&acpi, trampoline, root, nx))
        && sleep_here(&acpi, types);
    if !woke {
        if flags & (1 << 9) != 0 {
            arch::enable_interrupts();
        }
        smp::restart_all();
        return Err(Error::NotEntered);
    }
    if saved.timer_hz != 0 {
        arch::apic::start_timer(saved.timer_hz);
    }
    if flags & (1 << 9) == 0 {
        arch::disable_interrupts();
    }
    let back = 1 + smp::restart_all();
    crate::pci::restore_config();
    resume_drivers();
    Ok(Resumed {
        processors: back,
        expected,
    })
}

#[cfg(feature = "s3-early")]
#[derive(Clone, Copy, Default)]
pub struct Report {
    pub supported: bool,
    pub slept: bool,
    pub processors: usize,
    pub expected: usize,
    pub clock: bool,
    pub timer: bool,
    pub ioapic: bool,
    pub job: bool,
    pub verified: bool,
}

#[cfg(feature = "s3-early")]
fn probe_job(_: u64) {
    PROBE.store(true, Ordering::Release);
}

#[cfg(feature = "s3-early")]
static PROBE: AtomicBool = AtomicBool::new(false);

/// Sleeps once, with the harness waking the machine, and checks that the
/// processor, the timers and the interrupt controllers work afterwards.
#[cfg(feature = "s3-early")]
pub fn self_test() -> Report {
    let mut report = Report {
        supported: check().is_ok(),
        ..Report::default()
    };
    if !report.supported {
        return report;
    }
    let before = time::monotonic_nanoseconds();
    serial::line("AEROS_S3_SLEEP");
    let Ok(resumed) = suspend() else {
        return report;
    };
    report.slept = true;
    report.processors = resumed.processors;
    report.expected = resumed.expected;
    let after = time::monotonic_nanoseconds();
    report.clock = after >= before && after - before < 120_000_000_000;

    arch::interrupts::reset_local_timer_ticks();
    let started = arch::apic::start_timer(100);
    let begin = time::monotonic_nanoseconds();
    while started
        && arch::interrupts::local_timer_ticks() < 4
        && time::monotonic_nanoseconds().wrapping_sub(begin) < 500_000_000
    {
        arch::wait_for_interrupt();
    }
    report.timer = started && arch::interrupts::local_timer_ticks() >= 4;
    arch::apic::stop_timer();

    if let Some(acpi) = *ACPI.lock() {
        let routed = ioapic::route_legacy_irq(&acpi, 0, arch::apic::current_id(), 50);
        report.ioapic = routed
            && arch::interrupts::ioapic_timer_self_test(100, 4).is_some_and(|ticks| ticks >= 4);
        ioapic::mask_legacy_irq(&acpi, 0);
    }

    PROBE.store(false, Ordering::Release);
    report.job = resumed.expected < 2
        || (smp::start_job(1, probe_job, 0) && {
            let begin = time::monotonic_nanoseconds();
            while !smp::reap_job(1)
                && time::monotonic_nanoseconds().wrapping_sub(begin) < 500_000_000
            {
                core::hint::spin_loop();
            }
            PROBE.load(Ordering::Acquire)
        });
    report.verified = report.supported
        && report.slept
        && report.processors == report.expected
        && report.clock
        && report.timer
        && report.ioapic
        && report.job;
    report
}

/// Boot test: restarts every driver the way a wake from sleep does, then
/// proves each present device works again with real I/O (a disk sector
/// signature, an ARP exchange, a tune, enumerated USB input).
#[cfg(feature = "boot-test")]
pub fn driver_self_test() -> bool {
    let usb_inputs = crate::xhci::input_devices();
    let usb_disk = crate::xhci::storage_sectors() != 0;
    // Whatever the disk holds in its first sector (the volume has been
    // formatted since the USB test ran) must read back the same afterwards.
    let mut usb_before = [0u8; 512];
    let usb_disk = usb_disk && crate::xhci::storage_read(0, 1, &mut usb_before);
    let report = resume_drivers();
    let mut sector = [0u8; 4096];
    let sata = crate::ahci::disk_count() == 0
        || (crate::ahci::read_sector(0, (&mut sector[..512]).try_into().unwrap())
            && sector[510] == 0x55
            && sector[511] == 0xaa);
    let nvme = crate::nvme::sectors() == 0
        || (crate::nvme::read(0, 1, &mut sector[..crate::nvme::sector_bytes() as usize])
            && &sector[..15] == b"AEROS-NVME-TEST");
    let virtio_disk = crate::virtio_blk::sectors() == 0
        || (crate::virtio_blk::read(0, 1, &mut sector[..512])
            && &sector[..16] == b"AEROS-VIRTIO-BLK");
    let sd = crate::sdhci::sectors() == 0
        || (crate::sdhci::read(0, 1, &mut sector[..512]) && &sector[..14] == b"AEROS-SD-TEST!");
    let usb_storage = !usb_disk
        || (crate::xhci::storage_read(0, 1, &mut sector[..512]) && sector[..512] == usb_before[..]);
    let usb_input = crate::xhci::input_devices() == usb_inputs;
    let mut ethernet = true;
    if let Some(mac) = crate::e1000::mac_address() {
        ethernet &=
            crate::nic::arp_probe(mac, crate::e1000::transmit, crate::e1000::try_receive).is_some();
    }
    if let Some(mac) = crate::rtl8139::mac_address() {
        ethernet &=
            crate::nic::arp_probe(mac, crate::rtl8139::transmit, crate::rtl8139::try_receive)
                .is_some();
    }
    if let Some(mac) = crate::rtl8168::mac_address() {
        ethernet &=
            crate::nic::arp_probe(mac, crate::rtl8168::transmit, crate::rtl8168::try_receive)
                .is_some();
    }
    let audio = crate::hda::playback_probe() && crate::ac97::playback_probe();
    let verified = report.all_ok()
        && sata
        && nvme
        && virtio_disk
        && sd
        && usb_storage
        && usb_input
        && ethernet
        && audio;
    serial::format(format_args!(
        "AEROS_DRIVER_RESUME_TEST sata={sata} nvme={nvme} virtio_blk={virtio_disk} sd={sd} usb_storage={usb_storage} usb_input={usb_input} ethernet={ethernet} audio={audio} verified={verified}\n"
    ));
    verified
}
