use core::arch::asm;
use core::arch::x86_64::__cpuid_count;

use crate::memory::FrameAllocator;

const EFER_MSR: u32 = 0xc000_0080;
const EFER_SVME: u64 = 1 << 12;
const VM_CR_MSR: u32 = 0xc001_0114;
const VM_HSAVE_PA_MSR: u32 = 0xc001_0117;
const VM_CR_SVMDIS: u64 = 1 << 4;
const VM_CR_LOCK: u64 = 1 << 3;
const FS_BASE_MSR: u32 = 0xc000_0100;
const GS_BASE_MSR: u32 = 0xc000_0101;
const KERNEL_GS_BASE_MSR: u32 = 0xc000_0102;

const PAGE_SIZE: u64 = 4096;
const PAGE_2M: u64 = 0x20_0000;
const GUEST_RAM_PAGES: u64 = 8192;
const NPT_PAGES: u64 = 3;
const IOPM_PAGES: u64 = 3;
const MSRPM_PAGES: u64 = 2;
const GUEST_ENTRY: u64 = 0x2000;
const GUEST_STACK: u64 = 0x1000;
const LONG_ENTRY: u64 = 0x2_0000;
const LONG_STACK: u64 = 0x8000;
const LONG_PML4: u64 = 0x1000;
const LONG_PDPT: u64 = 0x4000;
const LONG_PD: u64 = 0x5000;
const CR0_LONG: u64 = 0x8000_0031;
const CR4_PAE: u64 = 1 << 5;
const EFER_LME: u64 = 1 << 8;
const EFER_LMA: u64 = 1 << 10;
const GUEST_MARKER_ADDR: u64 = 0x9000;
const GUEST_MARKER: u32 = 0xae05_2d05;
const GUEST_HIGH_ADDR: u64 = 0x0140_0000;
const GUEST_HIGH_VALUE: u32 = 0xcafe_f00d;
const GUEST_HIGH_MARKER_ADDR: u64 = 0x9004;
const COM1: u16 = 0x3f8;
const MAX_EXITS: u32 = 4096;
const GUEST_LOG_MAX: usize = 64;

const EXIT_IOIO: u64 = 0x07b;
const EXIT_HLT: u64 = 0x078;
#[cfg(feature = "linux-guest")]
const EXIT_INTR: u64 = 0x060;
#[cfg(feature = "linux-guest")]
const EXIT_IRET: u64 = 0x074;
const EXIT_CPUID: u64 = 0x072;
const EXIT_MSR: u64 = 0x07c;
const EXIT_VMMCALL: u64 = 0x081;
#[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
const EXIT_PAUSE: u64 = 0x077;
#[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
const EXIT_SHUTDOWN: u64 = 0x07f;
const EXIT_NPF: u64 = 0x400;

#[cfg(feature = "linux-guest")]
const INTERCEPT_INTR: u32 = 1 << 0;
const INTERCEPT_HLT: u32 = 1 << 24;
const INTERCEPT_IOIO_PROT: u32 = 1 << 27;
const INTERCEPT_CPUID: u32 = 1 << 18;
const INTERCEPT_MSR_PROT: u32 = 1 << 28;
#[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
const INTERCEPT_PAUSE: u32 = 1 << 23;
const INTERCEPT2_VMRUN: u32 = 1;
const INTERCEPT2_VMMCALL: u32 = 1 << 1;

#[derive(Clone, Copy, PartialEq, Eq)]
enum GuestMode {
    Real,
    Long,
}

#[derive(Clone, Copy)]
pub struct VmReport {
    pub svm_supported: bool,
    pub npt_supported: bool,
    pub svm_enabled: bool,
    pub locked_off: bool,
    pub guest_ram_bytes: u64,
    pub exits: u32,
    pub io_writes: u32,
    pub cpuid_exits: u32,
    pub msr_exits: u32,
    pub last_exit_code: u64,
    pub halted: bool,
    pub console_len: usize,
    pub console_ok: bool,
    pub guest_marker: u32,
    pub high_marker: u32,
    pub verified: bool,
}

impl VmReport {
    const fn early(svm: bool, npt: bool, enabled: bool, locked: bool) -> Self {
        Self {
            svm_supported: svm,
            npt_supported: npt,
            svm_enabled: enabled,
            locked_off: locked,
            guest_ram_bytes: 0,
            exits: 0,
            io_writes: 0,
            cpuid_exits: 0,
            msr_exits: 0,
            last_exit_code: 0,
            halted: false,
            console_len: 0,
            console_ok: false,
            guest_marker: 0,
            high_marker: 0,
            verified: false,
        }
    }
}

fn svm_status() -> (bool, bool) {
    let extended = __cpuid_count(0x8000_0000, 0);
    let svm_supported =
        extended.eax >= 0x8000_0001 && __cpuid_count(0x8000_0001, 0).ecx & (1 << 2) != 0;
    let npt_supported =
        svm_supported && extended.eax >= 0x8000_000a && __cpuid_count(0x8000_000a, 0).edx & 1 != 0;
    (svm_supported, npt_supported)
}

fn prepare_svm() -> bool {
    let (svm_supported, npt_supported) = svm_status();
    if !svm_supported || !npt_supported {
        return false;
    }
    let vm_cr = read_msr(VM_CR_MSR);
    if vm_cr & VM_CR_SVMDIS != 0 && vm_cr & VM_CR_LOCK != 0 {
        return false;
    }
    if vm_cr & VM_CR_SVMDIS != 0 {
        unsafe { write_msr(VM_CR_MSR, vm_cr & !VM_CR_SVMDIS) };
    }
    unsafe { write_msr(EFER_MSR, read_msr(EFER_MSR) | EFER_SVME) };
    read_msr(EFER_MSR) & EFER_SVME != 0
}

pub fn self_test(frames: &mut FrameAllocator) -> VmReport {
    let (svm_supported, npt_supported) = svm_status();
    if !svm_supported || !npt_supported {
        return VmReport::early(svm_supported, npt_supported, false, false);
    }
    let vm_cr = read_msr(VM_CR_MSR);
    if vm_cr & VM_CR_SVMDIS != 0 && vm_cr & VM_CR_LOCK != 0 {
        return VmReport::early(true, npt_supported, false, true);
    }
    if !prepare_svm() {
        return VmReport::early(true, npt_supported, false, false);
    }

    let real = run_mode(frames, GuestMode::Real, npt_supported);
    let long = run_mode(frames, GuestMode::Long, npt_supported);
    crate::serial::format(format_args!(
        "AEROS_VM64 guest_ram_bytes={} exits={} io_writes={} cpuid_exits={} msr_exits={} last_exit_code={:#x} halted={} console_len={} console_ok={} high_marker={:#x} guest_marker={:#x} verified={}\n",
        long.guest_ram_bytes,
        long.exits,
        long.io_writes,
        long.cpuid_exits,
        long.msr_exits,
        long.last_exit_code,
        long.halted,
        long.console_len,
        long.console_ok,
        long.high_marker,
        long.guest_marker,
        long.verified,
    ));
    run_linux(frames);
    real
}

fn run_mode(frames: &mut FrameAllocator, mode: GuestMode, npt_supported: bool) -> VmReport {
    let Some(mut vm) = Vm::new(frames, mode) else {
        return VmReport::early(true, npt_supported, true, false);
    };
    vm.load_guest();

    let host_fs = read_msr(FS_BASE_MSR);
    let host_gs = read_msr(GS_BASE_MSR);
    let host_kernel_gs = read_msr(KERNEL_GS_BASE_MSR);
    unsafe { write_msr(VM_HSAVE_PA_MSR, vm.hsave) };
    match mode {
        GuestMode::Real => crate::serial::line("AEROS_VM_LAUNCH guest=aeros-smoke"),
        GuestMode::Long => crate::serial::line("AEROS_VM_LAUNCH guest=aeros-longmode"),
    }
    vm.run();
    let tag = match mode {
        GuestMode::Real => "AEROS_VM_CONSOLE",
        GuestMode::Long => "AEROS_VM64_CONSOLE",
    };
    crate::serial::format(format_args!("{} bytes={} text=", tag, vm.console_len));
    for &byte in &vm.console[..vm.console_len] {
        if byte == b'\n' {
            crate::serial::text("\\n");
        } else {
            crate::serial::byte(byte);
        }
    }
    crate::serial::text("\n");
    unsafe {
        write_msr(FS_BASE_MSR, host_fs);
        write_msr(GS_BASE_MSR, host_gs);
        write_msr(KERNEL_GS_BASE_MSR, host_kernel_gs);
    }

    let marker = unsafe {
        core::ptr::read_volatile((vm.guest_ram + GUEST_MARKER_ADDR) as usize as *const u32)
    };
    let high_marker = unsafe {
        core::ptr::read_volatile((vm.guest_ram + GUEST_HIGH_MARKER_ADDR) as usize as *const u32)
    };
    let console_ok =
        vm.console_len == GUEST_MESSAGE.len() && vm.console[..vm.console_len] == GUEST_MESSAGE[..];
    let verified = vm.halted
        && console_ok
        && marker == GUEST_MARKER
        && high_marker == GUEST_HIGH_VALUE
        && vm.io_writes as usize >= GUEST_MESSAGE.len()
        && vm.cpuid_exits >= 1
        && vm.msr_exits >= 1;

    VmReport {
        svm_supported: true,
        npt_supported,
        svm_enabled: true,
        locked_off: false,
        guest_ram_bytes: GUEST_RAM_PAGES * PAGE_SIZE,
        exits: vm.exits,
        io_writes: vm.io_writes,
        cpuid_exits: vm.cpuid_exits,
        msr_exits: vm.msr_exits,
        last_exit_code: vm.last_exit_code,
        halted: vm.halted,
        console_len: vm.console_len,
        console_ok,
        guest_marker: marker,
        high_marker,
        verified,
    }
}

const LINUX_LOAD: u64 = 0x10_0000;
const LINUX_BOOT_PARAMS: u64 = 0x7000;
const LINUX_CMDLINE: u64 = 0x1_0000;
const LINUX_STACK: u64 = 0xf000;
const LINUX_MESSAGE: &[u8] = b"LINUX\n";
const LINUX_CMDLINE_TEXT: &[u8] = b"console=ttyS0,115200 earlyprintk=serial,ttyS0,115200\0";

static LINUX_PROBE: [u8; 2048] = make_linux_probe();

const fn make_linux_probe() -> [u8; 2048] {
    let mut img = [0u8; 2048];
    img[0x1f1] = 1;
    img[0x1fe] = 0x55;
    img[0x1ff] = 0xaa;
    img[0x202] = b'H';
    img[0x203] = b'd';
    img[0x204] = b'r';
    img[0x205] = b'S';
    img[0x206] = 0x0f;
    img[0x207] = 0x02;
    img[0x236] = 0x01;
    let payload: &[u8] = &[
        0xba, 0xf8, 0x03, 0x00, 0x00, 0xb0, b'L', 0xee, 0xb0, b'I', 0xee, 0xb0, b'N', 0xee, 0xb0,
        b'U', 0xee, 0xb0, b'X', 0xee, 0xb0, b'\n', 0xee, 0x48, 0xc7, 0xc0, 0x00, 0x90, 0x00, 0x00,
        0xc7, 0x00, 0x05, 0x2d, 0x05, 0xae, 0x0f, 0xb7, 0x86, 0xfe, 0x01, 0x00, 0x00, 0x48, 0xc7,
        0xc3, 0x04, 0x90, 0x00, 0x00, 0x89, 0x03, 0xf4,
    ];
    let start = 1024 + 0x200;
    let mut i = 0;
    while i < payload.len() {
        img[start + i] = payload[i];
        i += 1;
    }
    img
}

fn linux_image() -> (&'static [u8], &'static str) {
    match crate::vfs::file("/boot/vmlinuz") {
        Ok(view) if view.data.len() >= 0x268 => (view.data, "/boot/vmlinuz"),
        _ => (&LINUX_PROBE, "embedded-probe"),
    }
}

fn run_linux(frames: &mut FrameAllocator) {
    let (image, source) = linux_image();
    let Some(mut vm) = Vm::new(frames, GuestMode::Long) else {
        crate::serial::line("AEROS_VM_LINUX source=none loaded=false verified=false");
        return;
    };
    let Some(pm_offset) = vm.load_linux(image) else {
        crate::serial::format(format_args!(
            "AEROS_VM_LINUX source={} bytes={} loaded=false verified=false\n",
            source,
            image.len()
        ));
        return;
    };

    let host_fs = read_msr(FS_BASE_MSR);
    let host_gs = read_msr(GS_BASE_MSR);
    let host_kernel_gs = read_msr(KERNEL_GS_BASE_MSR);
    unsafe { write_msr(VM_HSAVE_PA_MSR, vm.hsave) };
    crate::serial::line("AEROS_VM_LAUNCH guest=linux-boot-protocol");
    vm.run();
    unsafe {
        write_msr(FS_BASE_MSR, host_fs);
        write_msr(GS_BASE_MSR, host_gs);
        write_msr(KERNEL_GS_BASE_MSR, host_kernel_gs);
    }

    let marker = unsafe {
        core::ptr::read_volatile((vm.guest_ram + GUEST_MARKER_ADDR) as usize as *const u32)
    };
    let boot_flag = unsafe {
        core::ptr::read_volatile((vm.guest_ram + GUEST_HIGH_MARKER_ADDR) as usize as *const u32)
    };
    let console_ok =
        vm.console_len == LINUX_MESSAGE.len() && vm.console[..vm.console_len] == LINUX_MESSAGE[..];
    let is_probe = source == "embedded-probe";
    let verified =
        is_probe && vm.halted && console_ok && marker == GUEST_MARKER && boot_flag == 0xaa55;
    crate::serial::format(format_args!(
        "AEROS_VM_LINUX source={} bytes={} pm_offset={} loaded=true exits={} halted={} console_len={} console_ok={} boot_flag={:#x} marker={:#x} verified={}\n",
        source,
        image.len(),
        pm_offset,
        vm.exits,
        vm.halted,
        vm.console_len,
        console_ok,
        boot_flag,
        marker,
        verified,
    ));
}

const GUEST_MESSAGE: &[u8] = b"AER-GUEST\n";
const COM1_END: u16 = COM1 + 7;

#[cfg(feature = "linux-guest")]
pub fn boot_linux(frames: &mut FrameAllocator) {
    linux::boot(frames);
}

/// One top-level X window of the Linux guest, as published by the guest-side
/// `aeros-agent` (tools/aeros-agent.c) through a mailbox in the shared
/// framebuffer region. Positions/sizes are in guest-screen pixels.
#[derive(Clone, Copy)]
#[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
pub struct GuestWindow {
    pub id: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub flags: u32,
    pub title: [u8; 64],
}

#[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
impl GuestWindow {
    pub const EMPTY: Self = Self {
        id: 0,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        flags: 0,
        title: [0; 64],
    };
}

pub const GUEST_MAX_WINDOWS: usize = 16;

#[derive(Clone, Copy)]
pub struct GuestWindows {
    pub count: usize,
    pub screen_width: u32,
    pub screen_height: u32,
    /// Bumps whenever the agent republished the table.
    pub generation: u32,
    pub windows: [GuestWindow; GUEST_MAX_WINDOWS],
}
/// What the desktop's Linux window needs from the hypervisor. Without the
/// `linux-guest` feature the guest doesn't exist and these are inert.
#[cfg(feature = "linux-guest")]
pub fn linux_ready() -> bool {
    linux::is_ready()
}

#[cfg(feature = "linux-guest")]
pub fn linux_pump(budget_tsc: u64) -> bool {
    let start = crate::time::monotonic_nanoseconds();
    let alive = linux::pump(budget_tsc);
    crate::sysmon::add_guest(crate::time::monotonic_nanoseconds().saturating_sub(start));
    alive
}

/// Bytes of RAM the Linux guest owns.
#[cfg(feature = "linux-guest")]
#[allow(dead_code)]
pub fn linux_memory_bytes() -> u64 {
    GUEST_RAM_PAGES * PAGE_SIZE
}

#[cfg(feature = "linux-guest")]
pub fn linux_feed_scancode(code: u8) {
    linux::feed_scancode(code);
}

#[cfg(feature = "linux-guest")]
pub fn linux_mouse_epoch() -> u32 {
    linux::mouse_epoch()
}

#[cfg(feature = "linux-guest")]
pub fn linux_mouse_wheel(buttons: u8, wheel: i32) {
    linux::mouse_wheel(buttons, wheel);
}

#[cfg(feature = "linux-guest")]
pub fn linux_mouse_feed(dx: i32, dy: i32, buttons: u8) {
    linux::mouse_feed(dx, dy, buttons);
}

#[cfg(feature = "linux-guest")]
pub fn linux_framebuffer() -> Option<(*const u32, usize, usize, usize)> {
    linux::framebuffer()
}

#[cfg(feature = "linux-guest")]
pub fn linux_windows() -> Option<GuestWindows> {
    linux::windows()
}

/// Posts a command to the guest agent: 1 close(id), 2 focus(id), 3 spawn(text).
#[cfg(feature = "linux-guest")]
pub fn linux_command(op: u32, args: [u32; 4], text: &[u8]) {
    linux::command(op, args, text);
}

/// One installed Linux application, as the guest agent found it in the
/// .desktop files (NUL-terminated strings).
#[derive(Clone, Copy)]
pub struct LinuxApp {
    pub name: [u8; 48],
    pub exec: [u8; 112],
    pub category: [u8; 24],
}

impl LinuxApp {
    pub const EMPTY: Self = Self {
        name: [0; 48],
        exec: [0; 112],
        category: [0; 24],
    };

    fn text(bytes: &[u8]) -> &str {
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        core::str::from_utf8(&bytes[..end]).unwrap_or("")
    }

    pub fn name_str(&self) -> &str {
        Self::text(&self.name)
    }

    pub fn exec_str(&self) -> &str {
        Self::text(&self.exec)
    }

    pub fn category_str(&self) -> &str {
        Self::text(&self.category)
    }
}

pub const LINUX_MAX_APPS: usize = 96;

/// Asks the guest to fetch `url` and render it as text `cols` characters wide.
#[cfg(feature = "linux-guest")]
pub fn linux_web_request(url: &[u8], cols: u32) -> bool {
    linux::web_request(url, cols)
}

/// The fetched page: `(status, length)` when the guest published something
/// new since `seen` (status 0 done, 1 failed, 2 still fetching); the text is
/// copied into `out`.
#[cfg(feature = "linux-guest")]
pub fn linux_web_take(seen: &mut u32, out: &mut [u8]) -> Option<(u32, usize)> {
    linux::web_take(seen, out)
}

/// The guest's installed applications, when the list changed since `seen`:
/// `(count, install_state)` (0 idle, 1 installing, 2 done, 3 failed).
#[cfg(feature = "linux-guest")]
pub fn linux_apps(seen: &mut u32, out: &mut [LinuxApp; LINUX_MAX_APPS]) -> Option<(usize, u32)> {
    linux::apps_take(seen, out)
}

/// Whether the guest's agent (web fetch, app list, ...) is up.
#[cfg(feature = "linux-guest")]
pub fn linux_agent_ready() -> bool {
    linux::agent_ready()
}

#[cfg(not(feature = "linux-guest"))]
pub fn linux_web_request(_url: &[u8], _cols: u32) -> bool {
    false
}

#[cfg(not(feature = "linux-guest"))]
pub fn linux_web_take(_seen: &mut u32, _out: &mut [u8]) -> Option<(u32, usize)> {
    None
}

#[cfg(not(feature = "linux-guest"))]
pub fn linux_apps(_seen: &mut u32, _out: &mut [LinuxApp; LINUX_MAX_APPS]) -> Option<(usize, u32)> {
    None
}

#[cfg(not(feature = "linux-guest"))]
pub fn linux_agent_ready() -> bool {
    false
}

/// Puts `text` on the guest's X clipboard (the agent takes it over).
#[cfg(feature = "linux-guest")]
pub fn linux_clipboard_set(text: &[u8]) {
    linux::clipboard_set(text);
}

/// Copies the guest's newest clipboard text into `out` when it changed since
/// `seen` (updated); returns the length.
#[cfg(feature = "linux-guest")]
pub fn linux_clipboard_take(seen: &mut u32, out: &mut [u8]) -> Option<usize> {
    linux::clipboard_take(seen, out)
}

#[cfg(not(feature = "linux-guest"))]
pub fn linux_clipboard_set(_text: &[u8]) {}

#[cfg(not(feature = "linux-guest"))]
pub fn linux_clipboard_take(_seen: &mut u32, _out: &mut [u8]) -> Option<usize> {
    None
}

#[cfg(not(feature = "linux-guest"))]
pub fn linux_windows() -> Option<GuestWindows> {
    None
}

#[cfg(not(feature = "linux-guest"))]
pub fn linux_command(_op: u32, _args: [u32; 4], _text: &[u8]) {}
#[cfg(not(feature = "linux-guest"))]
#[allow(dead_code)]
pub fn linux_memory_bytes() -> u64 {
    0
}

#[cfg(not(feature = "linux-guest"))]
pub fn linux_ready() -> bool {
    false
}

#[cfg(not(feature = "linux-guest"))]
pub fn linux_pump(_budget_tsc: u64) -> bool {
    false
}

#[cfg(not(feature = "linux-guest"))]
pub fn linux_feed_scancode(_code: u8) {}

#[cfg(not(feature = "linux-guest"))]
pub fn linux_mouse_epoch() -> u32 {
    0
}

#[cfg(not(feature = "linux-guest"))]
pub fn linux_mouse_wheel(_buttons: u8, _wheel: i32) {}

#[cfg(not(feature = "linux-guest"))]
pub fn linux_mouse_feed(_dx: i32, _dy: i32, _buttons: u8) {}

#[cfg(not(feature = "linux-guest"))]
pub fn linux_framebuffer() -> Option<(*const u32, usize, usize, usize)> {
    None
}

#[cfg(feature = "linux-guest")]
mod linux {
    use super::*;
    use crate::fat;

    const MIB: u64 = 1024 * 1024;
    const RAM_CANDIDATES: [u64; 9] = [1024, 768, 512, 384, 320, 288, 256, 224, 192];

    const PML4: u64 = 0x1000;
    const PDPT: u64 = 0x2000;
    const PD_BASE: u64 = 0x3000;
    const ZERO_PAGE: u64 = 0x8000;
    const GDT: u64 = 0x9000;
    const CMDLINE: u64 = 0xa000;
    const STACK_TOP: u64 = 0x1_0000;
    const KERNEL_LOAD: u64 = 0x100_0000;
    const INITRD_LOAD: u64 = 0x600_0000;
    const STAGING_BACKOFF: u64 = 0x100_0000;

    /// Two virtual CPUs (feature `linux-smp`): the local APIC is on, the
    /// legacy 8259 keeps routing device interrupts (`noapic`), the CPUs come
    /// from the MP table (`acpi=off`) and the TSC is trusted, which skips the
    /// cross-CPU TSC synchronisation test.
    #[cfg(feature = "linux-smp")]
    const CMDLINE_TEXT: &[u8] =
        b"console=ttyS0,115200 earlyprintk=serial,ttyS0,115200 keep_bootcon nokaslr no5lvl noapic acpi=off no_timer_check nmi_watchdog=0 tsc=reliable reboot=t panic=-1 debug ignore_loglevel root=/dev/vda ro rootwait\0";
    #[cfg(not(feature = "linux-smp"))]
    const CMDLINE_TEXT: &[u8] =
        b"console=ttyS0,115200 earlyprintk=serial,ttyS0,115200 keep_bootcon nokaslr no5lvl nolapic nosmp acpi=off no_timer_check nmi_watchdog=0 reboot=t panic=-1 debug ignore_loglevel root=/dev/vda ro rootwait\0";
    const SMP: bool = cfg!(feature = "linux-smp");

    const EFER_NXE: u64 = 1 << 11;
    const CR0_LINUX: u64 = 0x8001_0031;
    const MAX_EXITS_LINUX: u64 = 4_000_000_000;
    #[cfg(feature = "linux-headless")]
    const MAX_RUN_TSC: u64 = 600_000_000_000;
    const TICK_TSC: u64 = 2_000_000;
    const HEARTBEAT_TSC: u64 = 30_000_000_000;
    const MSR_KVM_WALL_CLOCK: u32 = 0x4b56_4d00;
    const MSR_KVM_SYSTEM_TIME: u32 = 0x4b56_4d01;
    const PVCLOCK_MUL: u32 = 1_301_505_241;
    const MAX_SERIAL: u64 = 4 * 1024 * 1024;
    const GUEST_LIMIT: u64 = 0x1_0000_0000;
    const TSC_PER_PIT: u64 = 2766;

    // Legacy (pre-1.0) virtio-blk-pci: a single PCI function at 00:01.0,
    // I/O-port register bank (fits the same EXIT_IOIO trap already used for
    // the UART/PIT/PIC below - no MMIO/NPT device model needed), one
    // request virtqueue, backed by on-demand sector reads from the ROOTFS
    // file on the host ESP (never loaded wholesale into guest RAM).
    const ROOTFS_NAME: &[u8; 11] = b"ROOTFS     ";
    const VIRTIO_IO_BASE: u32 = 0xc000;
    const VIRTIO_IO_SIZE: u32 = 0x20;
    const VIRTIO_IRQ_LINE: u8 = 5;
    const UART_IRQ_LINE: u8 = 4;
    const KBD_IRQ_LINE: u8 = 1;
    const AUX_IRQ_LINE: u8 = 12;
    const KBD_QUEUE_SIZE: usize = 64;

    /// A pointer event waiting to be turned into PS/2 packets. The guest reads
    /// the controller one byte per interrupt, so a burst (the desktop's
    /// slam-to-corner plus move is ~20 packets on a big screen) can't be
    /// queued in one go; events wait here, in order, instead of being cut off
    /// half way - a truncated move used to leave the cursor in the wrong place.
    #[derive(Clone, Copy)]
    struct MouseEvent {
        dx: i32,
        dy: i32,
        buttons: u8,
        wheel: i32,
    }
    const MOUSE_EVENTS: usize = 32;

    // ---- Asynchronous disk reads -------------------------------------------
    // With a raw root disk and a second host CPU, virtio-blk reads are done by
    // that CPU: the request is parsed here, queued in `BLK_RING`, and the
    // worker (`blk_worker`, run from the work IPI on logical CPU 1) performs
    // the AHCI transfer straight into guest memory while the guest keeps
    // running. `blk_poll` completes finished requests in order. A synchronous
    // read blocked every vCPU for its whole duration (a big one for tens of
    // milliseconds), starving the guest's timers under heavy I/O.
    const BLK_SLOTS: usize = VIRTIO_QUEUE_SIZE as usize;

    #[derive(Clone, Copy)]
    struct BlkJob {
        sector: u64,
        dest_phys: u64,
        len: u32,
        /// False for a request that failed validation: not sent to the disk.
        valid: bool,
        /// A write to the data area (else a read).
        write: bool,
    }

    #[derive(Clone, Copy)]
    struct BlkMeta {
        head: u16,
        status_addr: u64,
        data_len: u32,
        write: bool,
    }

    struct BlkRing(core::cell::UnsafeCell<[BlkJob; BLK_SLOTS]>);
    // SAFETY: a slot is written by this CPU before `BLK_PRODUCED` is bumped
    // (release) and only read by the worker after it observes the bump.
    unsafe impl Sync for BlkRing {}

    static BLK_RING: BlkRing = BlkRing(core::cell::UnsafeCell::new(
        [BlkJob {
            sector: 0,
            dest_phys: 0,
            len: 0,
            valid: false,
            write: false,
        }; BLK_SLOTS],
    ));
    /// Byte offset where the writable data area of the root disk begins
    /// (just past the read-only ext4 root); u64::MAX = the disk is read-only.
    static DATA_START: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(u64::MAX);
    static BLK_PRODUCED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    static BLK_DONE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    static BLK_ACTIVE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    static BLK_OK: [core::sync::atomic::AtomicBool; BLK_SLOTS] =
        [const { core::sync::atomic::AtomicBool::new(false) }; BLK_SLOTS];
    static BLK_DISK: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

    /// Runs on the second host CPU: performs queued reads until none are left.
    fn blk_worker(_argument: u64) {
        use core::sync::atomic::Ordering::{Acquire, Release, SeqCst};
        loop {
            loop {
                let done = BLK_DONE.load(Acquire);
                if done == BLK_PRODUCED.load(Acquire) {
                    break;
                }
                let index = done as usize % BLK_SLOTS;
                // SAFETY: published by the release store of BLK_PRODUCED.
                let job = unsafe { (*BLK_RING.0.get())[index] };
                let mut ok = false;
                if job.valid {
                    ok = true;
                    let disk = BLK_DISK.load(Acquire);
                    let total = job.len / 512;
                    let mut sent = 0u32;
                    while sent < total {
                        let batch = (total - sent).min(8192);
                        let lba = job.sector + sent as u64;
                        let phys = job.dest_phys + sent as u64 * 512;
                        let done = if job.write {
                            crate::ahci::write_disk(disk, lba, batch, phys)
                        } else {
                            crate::ahci::read_disk(disk, lba, batch, phys)
                        };
                        if !done {
                            ok = false;
                            break;
                        }
                        sent += batch;
                    }
                }
                BLK_OK[index].store(ok, Release);
                BLK_DONE.store(done.wrapping_add(1), Release);
            }
            // Going idle: a request queued right now must not be missed.
            BLK_ACTIVE.store(false, SeqCst);
            if BLK_DONE.load(SeqCst) == BLK_PRODUCED.load(SeqCst) {
                return;
            }
            if BLK_ACTIVE
                .compare_exchange(false, true, SeqCst, SeqCst)
                .is_err()
            {
                return; // the submitter restarted a worker
            }
        }
    }
    // The guest's framebuffer: a fixed 4 MiB window inside its RAM,
    // reserved in the e820 map and advertised through screen_info as a VESA
    // linear framebuffer, so the kernel's built-in vesafb/simplefb + fbcon
    // draw a real console there and the host just reads those bytes.
    const FB_BASE: u64 = 0x0900_0000;
    const FB_RESERVED: u64 = 0xC0_0000;
    const FB_WIDTH: u64 = 2048;
    const FB_HEIGHT: u64 = 1152;
    const FB_PITCH: u64 = FB_WIDTH * 4;
    const UART_RX_SIZE: usize = 64;
    const VIRTIO_QUEUE_SIZE: u16 = 32;
    const NET_IO_BASE: u32 = 0xc040;
    const NET_IRQ_LINE: u8 = 10;
    const NET_QUEUE_SIZE: u16 = 64;
    const NET_HDR_LEN: usize = 10;
    // VIRTIO_NET_F_MAC | VIRTIO_NET_F_STATUS. No checksum/GSO offload is
    // offered, so the guest builds complete frames the NIC can send as-is.
    const NET_HOST_FEATURES: u32 = (1 << 5) | (1 << 16);
    const VIRTIO_BLK_T_IN: u32 = 0;
    const VIRTIO_BLK_T_OUT: u32 = 1;
    const VIRTIO_BLK_S_OK: u8 = 0;
    const VIRTIO_BLK_S_IOERR: u8 = 1;

    struct Machine {
        vmcb: u64,
        host_vmcb: u64,
        npt_pd: [u64; 4],
        ram: u64,
        ram_bytes: u64,
        scratch: u64,
        gpr: [u64; 14],
        cpuid_exits: u64,
        npf_maps: u64,
        pic: [u8; 2],
        pic_icw: u8,
        pic_base: u8,
        tick_pending: bool,
        reset: bool,
        pit_latch: u16,
        pit_start: u64,
        pit_wr_hi: bool,
        pit_rd_hi: bool,
        /// Channel 0 mode the guest last programmed (2/3 = periodic tick,
        /// 0 = one-shot / shut down).
        pit_mode: u8,
        port61: u8,
        cmos_index: u8,
        pci_addr: u32,
        exits: u64,
        serial_bytes: u64,
        io_reads: u64,
        io_writes: u64,
        msr_exits: u64,
        ticks: u64,
        last_tick_tsc: u64,
        last_exit: u64,
        last_rip: u64,
        last_port: u16,
        npf_gpa: u64,
        rootfs_bytes: u64,
        /// AHCI disk index backing the virtio-blk device, or usize::MAX when
        /// the root is the ROOTFS file on the boot volume.
        rootfs_disk: usize,
        pci_command: u16,
        bar0_raw: u32,
        virtio_guest_features: u32,
        virtio_queue_pfn: u32,
        virtio_queue_select: u16,
        virtio_status: u8,
        virtio_isr: u8,
        virtio_last_avail: u16,
        virtio_used_idx: u16,
        virtio_irq_pending: bool,
        blk_requests: u64,
        blk_errors: u64,
        // 8250 UART state beyond the byte sink: Linux's tty layer only
        // drains its transmit buffer from the THRE interrupt (IER bit 1),
        // and reads typed input from an RX interrupt (IER bit 0), so a
        // console that merely swallows THR writes shows kernel printk (which
        // polls) but nothing a userspace shell writes.
        uart_ier: u8,
        uart_lcr: u8,
        uart_thre_pending: bool,
        uart_irq_injected: bool,
        uart_rx: [u8; UART_RX_SIZE],
        uart_rx_len: usize,
        kbd_shift: bool,
        kbd_ctrl: bool,
        kbd_extended: bool,
        last_kbd_tsc: u64,
        /// Headless runs (boot-linux.ps1) read the host PS/2 controller
        /// themselves and type into the serial console; in desktop mode the
        /// Linux window forwards scancodes via `feed_scancode` instead.
        host_input: bool,
        dead: bool,
        /// Two-vCPU mode (feature `linux-smp`). `vmcb`/`gpr` above always
        /// belong to vCPU `cur`; the other vCPU's live in `vcpus`.
        smp: bool,
        cur: usize,
        slice_exits: u32,
        vcpus: [Vcpu; NUM_VCPUS],
        // Emulated i8042 + AT keyboard so the guest gets a real tty0 / fbcon
        // keyboard. Scancodes are the host controller's translated set 1,
        // which is what the guest expects while the controller's
        // translation bit (0x40 of the config byte) is left on.
        i8042_cfg: u8,
        i8042_cmd: u8,
        // Output buffer entries: low byte = data, bit 8 = came from the
        // auxiliary (mouse) port.
        kbd_out: [u16; KBD_QUEUE_SIZE],
        kbd_out_len: usize,
        mouse_events: [MouseEvent; MOUSE_EVENTS],
        mouse_event_len: usize,
        blk_meta: [BlkMeta; BLK_SLOTS],
        /// Requests already completed towards the guest (cursor into the
        /// monotonic BLK_PRODUCED/BLK_DONE counters).
        blk_finalized: u32,
        kbd_irq_injected: bool,
        kbd_dev_arg: u8,
        kbd_scanning: bool,
        mouse_reporting: bool,
        /// Bumped every time the guest (re)enables mouse reporting; the host
        /// resynchronises the pointer when it changes.
        mouse_epoch: u32,
        /// PS/2 device id: 0 = plain 3-byte mouse, 3 = IntelliMouse (wheel, 4-byte
        /// packets) once the guest has sent the 200/100/80 sample-rate sequence.
        mouse_id: u8,
        mouse_rates: [u8; 3],
        heartbeat_tsc: u64,
        mouse_packets: u64,
        mouse_arg: u8,
        aux_irq_injected: bool,
        // Legacy virtio-net at 00:02.0 (queue 0 = receive, 1 = transmit),
        // bridged onto the host's e1000: the guest transmits and receives
        // raw Ethernet frames through the same NIC AerOS itself uses, under
        // the NIC's own MAC (the hardware only accepts that MAC + broadcast).
        net_bar_raw: u32,
        net_pci_command: u16,
        net_status: u8,
        net_isr: u8,
        net_features: u32,
        net_queue_select: u16,
        net_queue_pfn: [u32; 2],
        net_last_avail: [u16; 2],
        net_used_idx: [u16; 2],
        net_irq_pending: bool,
        net_mac: [u8; 6],
        net_rx_frames: u64,
        net_tx_frames: u64,
        net_tx_dropped: u64,
        last_net_poll_tsc: u64,
    }

    /// The one guest VM. It lives here between `pump()` slices so the
    /// desktop loop can run it a few milliseconds at a time.
    static mut GUEST: Option<Machine> = None;

    fn host_interrupts_enabled() -> bool {
        let flags: u64;
        unsafe {
            core::arch::asm!("pushfq; pop {}", out(reg) flags, options(nomem, preserves_flags));
        }
        flags & (1 << 9) != 0
    }

    fn guest() -> Option<&'static mut Machine> {
        unsafe { (*core::ptr::addr_of_mut!(GUEST)).as_mut() }
    }

    /// Headless mode (boot-linux.ps1): build the VM and run it to completion
    /// (or the run-time cap) before the desktop starts, logging to serial.
    #[cfg(feature = "linux-headless")]
    pub fn boot(frames: &mut FrameAllocator) {
        if !prepare(frames) {
            return;
        }
        crate::serial::line("---- linux serial ----");
        let alive = pump_raw(MAX_RUN_TSC, true);
        let _ = alive;
        crate::serial::line("");
        crate::serial::line("---- linux serial end ----");
        if let Some(machine) = guest() {
            crate::serial::format(format_args!(
                "AEROS_VM_LINUX_KERNEL stage=done reset={} exits={} serial_bytes={} io_reads={} io_writes={} cpuid_exits={} msr_exits={} npf_maps={} ticks={} pic_base={:#x} last_exit={:#x} last_rip={:#x} blk_requests={} blk_errors={}\n",
                machine.reset,
                machine.exits,
                machine.serial_bytes,
                machine.io_reads,
                machine.io_writes,
                machine.cpuid_exits,
                machine.msr_exits,
                machine.npf_maps,
                machine.ticks,
                machine.pic_base,
                machine.last_exit,
                machine.last_rip,
                machine.blk_requests,
                machine.blk_errors,
            ));
        }
    }

    /// Desktop mode: allocate and load the VM but do not run it; the Linux
    /// window drives it through `pump`.
    #[cfg(not(feature = "linux-headless"))]
    pub fn boot(frames: &mut FrameAllocator) {
        let _ = prepare(frames);
    }

    /// Runs the guest for about `budget_tsc` cycles. Returns false once the
    /// guest has powered off, reset or hit an unhandled exit.
    pub fn pump(budget_tsc: u64) -> bool {
        pump_raw(budget_tsc, false)
    }

    /// Where the host's own FPU state is parked while the guest runs.
    #[repr(align(64))]
    struct FpuArea(#[allow(dead_code)] [u8; 4096]);
    static mut HOST_FPU: FpuArea = FpuArea([0; 4096]);

    fn pump_raw(budget_tsc: u64, host_input: bool) -> bool {
        let Some(machine) = guest() else {
            return false;
        };
        if machine.dead {
            return false;
        }
        machine.host_input = host_input;
        let host_fs = read_msr(FS_BASE_MSR);
        let host_gs = read_msr(GS_BASE_MSR);
        let host_kernel_gs = read_msr(KERNEL_GS_BASE_MSR);
        // The guest's FPU/SSE/AVX state lives in its own save area between
        // slices, so it can't be clobbered by (or clobber) host code that
        // runs in between; the host's registers are put back afterwards.
        let host_area = core::ptr::addr_of_mut!(HOST_FPU) as *mut u8;
        unsafe {
            crate::arch::fpu::save_context(host_area);
            crate::arch::fpu::restore_context(machine.vcpus[machine.cur].fpu as *const u8);
        }
        let alive = machine.run_for(budget_tsc);
        unsafe {
            crate::arch::fpu::save_context(machine.vcpus[machine.cur].fpu as *mut u8);
            crate::arch::fpu::restore_context(host_area);
        }
        unsafe {
            write_msr(FS_BASE_MSR, host_fs);
            write_msr(GS_BASE_MSR, host_gs);
            write_msr(KERNEL_GS_BASE_MSR, host_kernel_gs);
        }
        if !alive {
            machine.dead = true;
        }
        alive
    }

    // Above the 2048x1152x4 pixels (0x90_0000) at the top of the 12 MiB region.
    const TABLE_OFFSET: u64 = 0xA0_0000;
    const COMMAND_OFFSET: u64 = 0xA0_1000;
    const TABLE_MAGIC: u32 = 0x5457_4541;
    const COMMAND_MAGIC: u32 = 0x4357_4541;
    const COMMAND_SLOTS: u32 = 16;
    /// op, four arguments and 128 bytes of text.
    const COMMAND_STRIDE: u64 = 20 + 128;

    /// Reads the agent's window table (a seqlock: odd sequence = being
    /// written), retrying a few times if a publish races the copy.
    pub fn windows() -> Option<GuestWindows> {
        let machine = guest()?;
        let base = machine.ram + FB_BASE + TABLE_OFFSET;
        let word = |offset: u64| unsafe { core::ptr::read_volatile((base + offset) as *const u32) };
        if word(0) != TABLE_MAGIC {
            return None;
        }
        for _ in 0..4 {
            let before = word(4);
            if before & 1 != 0 {
                continue;
            }
            let count = (word(8) as usize).min(GUEST_MAX_WINDOWS);
            let mut result = GuestWindows {
                count,
                screen_width: word(12),
                screen_height: word(16),
                generation: before,
                windows: [GuestWindow::EMPTY; GUEST_MAX_WINDOWS],
            };
            for (index, slot) in result.windows.iter_mut().take(count).enumerate() {
                let at = 32 + index as u64 * 88;
                slot.id = word(at);
                slot.x = word(at + 4) as i32;
                slot.y = word(at + 8) as i32;
                slot.width = word(at + 12);
                slot.height = word(at + 16);
                slot.flags = word(at + 20);
                for (i, byte) in slot.title.iter_mut().enumerate() {
                    *byte = unsafe {
                        core::ptr::read_volatile((base + at + 24 + i as u64) as *const u8)
                    };
                }
                slot.title[63] = 0;
            }
            if word(4) == before {
                return Some(result);
            }
        }
        None
    }

    /// Queues a command for the agent in the 16-slot ring: slot fields first,
    /// then the counter bump that publishes it. A full ring drops the new
    /// command (the agent drains it every ~60 ms).
    pub fn command(op: u32, args: [u32; 4], text: &[u8]) {
        let Some(machine) = guest() else {
            return;
        };
        let base = machine.ram + FB_BASE + COMMAND_OFFSET;
        let word = |offset: u64| unsafe { core::ptr::read_volatile((base + offset) as *const u32) };
        if word(0) != COMMAND_MAGIC {
            return; // the agent hasn't started yet
        }
        let (queued, acked) = (word(4), word(8));
        if queued.wrapping_sub(acked) >= COMMAND_SLOTS {
            return;
        }
        let slot = base + 16 + (queued % COMMAND_SLOTS) as u64 * COMMAND_STRIDE;
        let put = |offset: u64, value: u32| unsafe {
            core::ptr::write_volatile((slot + offset) as *mut u32, value)
        };
        put(0, op);
        for (index, arg) in args.iter().enumerate() {
            put(4 + 4 * index as u64, *arg);
        }
        for i in 0..128usize {
            let byte = text.get(i).copied().unwrap_or(0);
            unsafe { core::ptr::write_volatile((slot + 20 + i as u64) as *mut u8, byte) };
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        unsafe { core::ptr::write_volatile((base + 4) as *mut u32, queued.wrapping_add(1)) };
    }

    const WEB_REQ_OFFSET: u64 = 0xA3_0000;
    const WEB_RES_OFFSET: u64 = 0xA4_0000;
    const APPS_OFFSET: u64 = 0xA5_0000;
    const WEB_REQ_MAGIC: u32 = 0x5157_4541;
    const WEB_RES_MAGIC: u32 = 0x5357_4541;
    const APPS_MAGIC: u32 = 0x5041_4541;
    const WEB_MAX: usize = 64_000;

    /// The agent has created its mailboxes (it is running).
    pub fn agent_ready() -> bool {
        let Some(machine) = guest() else {
            return false;
        };
        let base = machine.ram + FB_BASE + WEB_REQ_OFFSET;
        unsafe { core::ptr::read_volatile(base as *const u32) == WEB_REQ_MAGIC }
    }

    pub fn web_request(url: &[u8], cols: u32) -> bool {
        let Some(machine) = guest() else {
            return false;
        };
        let base = machine.ram + FB_BASE + WEB_REQ_OFFSET;
        if unsafe { core::ptr::read_volatile(base as *const u32) } != WEB_REQ_MAGIC {
            return false;
        }
        let mut padded = [0u8; 512];
        let len = url.len().min(511);
        padded[..len].copy_from_slice(&url[..len]);
        for (i, byte) in padded.iter().enumerate() {
            unsafe { core::ptr::write_volatile((base + 16 + i as u64) as *mut u8, *byte) };
        }
        unsafe { core::ptr::write_volatile((base + 12) as *mut u32, cols) };
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        let seq = unsafe { core::ptr::read_volatile((base + 4) as *const u32) };
        unsafe { core::ptr::write_volatile((base + 4) as *mut u32, seq.wrapping_add(1)) };
        true
    }

    pub fn web_take(seen: &mut u32, out: &mut [u8]) -> Option<(u32, usize)> {
        let machine = guest()?;
        let base = machine.ram + FB_BASE + WEB_RES_OFFSET;
        let word = |offset: u64| unsafe { core::ptr::read_volatile((base + offset) as *const u32) };
        if word(0) != WEB_RES_MAGIC {
            return None;
        }
        let before = word(4);
        if before & 1 != 0 || before == *seen {
            return None;
        }
        let status = word(8);
        let len = (word(12) as usize).min(WEB_MAX).min(out.len());
        for (i, slot) in out[..len].iter_mut().enumerate() {
            *slot = unsafe { core::ptr::read_volatile((base + 16 + i as u64) as *const u8) };
        }
        if word(4) != before {
            return None;
        }
        *seen = before;
        Some((status, len))
    }

    pub fn apps_take(
        seen: &mut u32,
        out: &mut [super::LinuxApp; super::LINUX_MAX_APPS],
    ) -> Option<(usize, u32)> {
        let machine = guest()?;
        let base = machine.ram + FB_BASE + APPS_OFFSET;
        let word = |offset: u64| unsafe { core::ptr::read_volatile((base + offset) as *const u32) };
        if word(0) != APPS_MAGIC {
            return None;
        }
        let before = word(4);
        if before & 1 != 0 || before == *seen {
            return None;
        }
        let count = (word(8) as usize).min(super::LINUX_MAX_APPS);
        let state = word(12);
        for (index, app) in out.iter_mut().take(count).enumerate() {
            let at = base + 16 + index as u64 * 184;
            for (i, byte) in app.name.iter_mut().enumerate() {
                *byte = unsafe { core::ptr::read_volatile((at + i as u64) as *const u8) };
            }
            for (i, byte) in app.exec.iter_mut().enumerate() {
                *byte = unsafe { core::ptr::read_volatile((at + 48 + i as u64) as *const u8) };
            }
            for (i, byte) in app.category.iter_mut().enumerate() {
                *byte = unsafe { core::ptr::read_volatile((at + 160 + i as u64) as *const u8) };
            }
        }
        if word(4) != before {
            return None;
        }
        *seen = before;
        Some((count, state))
    }

    const CLIP_IN_OFFSET: u64 = 0xA1_0000;
    const CLIP_OUT_OFFSET: u64 = 0xA2_0000;
    const CLIP_IN_MAGIC: u32 = 0x4943_4541;
    const CLIP_OUT_MAGIC: u32 = 0x4f43_4541;
    const CLIP_MAX: usize = 60_000;

    /// Host -> guest clipboard: text, then the sequence bump the agent
    /// notices.
    pub fn clipboard_set(text: &[u8]) {
        let Some(machine) = guest() else {
            return;
        };
        let base = machine.ram + FB_BASE + CLIP_IN_OFFSET;
        if unsafe { core::ptr::read_volatile(base as *const u32) } != CLIP_IN_MAGIC {
            return;
        }
        let len = text.len().min(CLIP_MAX);
        for (i, byte) in text[..len].iter().enumerate() {
            unsafe { core::ptr::write_volatile((base + 16 + i as u64) as *mut u8, *byte) };
        }
        unsafe { core::ptr::write_volatile((base + 12) as *mut u32, len as u32) };
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        let seq = unsafe { core::ptr::read_volatile((base + 4) as *const u32) };
        unsafe { core::ptr::write_volatile((base + 4) as *mut u32, seq.wrapping_add(1)) };
    }

    /// Guest -> host clipboard (a seqlock: odd = being written).
    pub fn clipboard_take(seen: &mut u32, out: &mut [u8]) -> Option<usize> {
        let machine = guest()?;
        let base = machine.ram + FB_BASE + CLIP_OUT_OFFSET;
        let word = |offset: u64| unsafe { core::ptr::read_volatile((base + offset) as *const u32) };
        if word(0) != CLIP_OUT_MAGIC {
            return None;
        }
        let before = word(4);
        if before & 1 != 0 || before == *seen {
            return None;
        }
        let len = (word(8) as usize).min(CLIP_MAX).min(out.len());
        for (i, slot) in out[..len].iter_mut().enumerate() {
            *slot = unsafe { core::ptr::read_volatile((base + 16 + i as u64) as *const u8) };
        }
        if word(4) != before {
            return None; // raced a publish; try again next frame
        }
        *seen = before;
        (len > 0).then_some(len)
    }
    pub fn is_ready() -> bool {
        guest().is_some_and(|machine| !machine.dead)
    }

    pub fn feed_scancode(code: u8) {
        if let Some(machine) = guest() {
            machine.kbd_feed(code);
        }
    }

    pub fn mouse_epoch() -> u32 {
        guest().map_or(0, |machine| machine.mouse_epoch)
    }

    pub fn mouse_feed(dx: i32, dy: i32, buttons: u8) {
        if let Some(machine) = guest() {
            machine.mouse_feed(dx, dy, buttons, 0);
        }
    }

    pub fn mouse_wheel(buttons: u8, wheel: i32) {
        if let Some(machine) = guest() {
            machine.mouse_feed(0, 0, buttons, wheel);
        }
    }

    /// (host pointer, width, height, stride in pixels) of the guest's linear
    /// framebuffer, in XRGB8888.
    pub fn framebuffer() -> Option<(*const u32, usize, usize, usize)> {
        let machine = guest()?;
        Some((
            (machine.ram + FB_BASE) as *const u32,
            FB_WIDTH as usize,
            FB_HEIGHT as usize,
            (FB_PITCH / 4) as usize,
        ))
    }

    /// A disk other than the boot disk holding an ext4 filesystem labelled
    /// "aeros-root" (superblock magic 0xEF53 at byte 1080, label at 1144).
    /// Nothing else is ever used as the guest's root, so an unrelated data
    /// disk can't be picked up by accident.
    fn find_root_disk() -> Option<(usize, u64)> {
        for disk in 0..crate::ahci::disk_count() {
            if disk == crate::ahci::boot_disk() {
                continue;
            }
            let sectors = crate::ahci::disk_sectors(disk);
            if sectors < 131_072 {
                continue;
            }
            let mut block = [0u8; 512];
            // Superblock = bytes 1024..; magic at +56, volume name at +120.
            let read_ok = crate::ahci::read_disk_sector(disk, 2, &mut block);
            crate::serial::format(format_args!(
                "AEROS_VM_LINUX_ROOTDISK disk={} sectors={} read={} magic={:02x}{:02x} name={:?}\n",
                disk,
                sectors,
                read_ok,
                block[57],
                block[56],
                core::str::from_utf8(&block[120..130]).unwrap_or("?")
            ));
            if !read_ok {
                continue;
            }
            if block[56] == 0x53 && block[57] == 0xef && block[120..130] == *b"aeros-root" {
                // The ext4 root's own size (blocks * block size); anything
                // after it on the disk is a writable data area.
                let blocks = u32::from_le_bytes([block[4], block[5], block[6], block[7]]) as u64;
                let log = u32::from_le_bytes([block[24], block[25], block[26], block[27]]) & 7;
                let root_bytes = blocks * (1024u64 << log);
                if root_bytes + 1024 * 1024 <= sectors * 512 {
                    DATA_START.store(root_bytes, core::sync::atomic::Ordering::Release);
                    crate::serial::format(format_args!(
                        "AEROS_VM_LINUX_DATA start_bytes={} data_bytes={}\n",
                        root_bytes,
                        sectors * 512 - root_bytes
                    ));
                }
                return Some((disk, sectors * 512));
            }
        }
        None
    }
    fn prepare(frames: &mut FrameAllocator) -> bool {
        if !super::prepare_svm() {
            crate::serial::line("AEROS_VM_LINUX_KERNEL stage=svm-unavailable");
            return false;
        }
        let Some(vmlinuz_bytes) = fat::root_file_size(b"VMLINUZ    ") else {
            crate::serial::line("AEROS_VM_LINUX_KERNEL stage=image-absent file=VMLINUZ");
            return false;
        };
        let initrd_bytes = fat::root_file_size(b"INITRD     ").unwrap_or(0);
        if !(0x1000..=64 * MIB).contains(&vmlinuz_bytes) {
            crate::serial::line("AEROS_VM_LINUX_KERNEL stage=image-implausible");
            return false;
        }
        // The root filesystem comes from a dedicated disk when one labelled
        // "aeros-root" is attached (no size limit - a full desktop image won't
        // fit the FAT boot volume), else from the ROOTFS file on the boot
        // volume.
        let (rootfs_disk, rootfs_bytes) = if let Some((disk, bytes)) = find_root_disk() {
            (disk, bytes)
        } else if let Some(bytes) = fat::root_file_size(ROOTFS_NAME) {
            (usize::MAX, bytes)
        } else {
            crate::serial::line("AEROS_VM_LINUX_KERNEL stage=image-absent file=ROOTFS");
            return false;
        };

        let staging_needed = align_up(vmlinuz_bytes, PAGE_2M);
        let mut chosen = 0u64;
        let mut ram = 0u64;
        for candidate in RAM_CANDIDATES {
            let bytes = candidate * MIB;
            let staging = bytes - STAGING_BACKOFF;
            if INITRD_LOAD + align_up(initrd_bytes, PAGE_2M) > staging
                || staging + staging_needed > bytes
            {
                continue;
            }
            if let Some(frame) = frames.allocate_contiguous(bytes / PAGE_SIZE, 512) {
                chosen = bytes;
                ram = frame.address();
                break;
            }
        }
        if chosen == 0 {
            crate::serial::line("AEROS_VM_LINUX_KERNEL stage=no-contiguous-ram");
            return false;
        }

        let Some(npt) = frames.allocate_contiguous(6, 1).map(|f| f.address()) else {
            crate::serial::line("AEROS_VM_LINUX_KERNEL stage=npt-alloc");
            return false;
        };
        let Some(scratch) = frames.allocate_contiguous(512, 512).map(|f| f.address()) else {
            crate::serial::line("AEROS_VM_LINUX_KERNEL stage=scratch-alloc");
            return false;
        };
        let Some(hsave) = frames.allocate_contiguous(1, 1).map(|f| f.address()) else {
            return false;
        };
        let Some(host_vmcb) = frames.allocate_contiguous(1, 1).map(|f| f.address()) else {
            return false;
        };
        let Some(vmcb) = frames.allocate_contiguous(1, 1).map(|f| f.address()) else {
            return false;
        };
        let Some(iopm) = frames.allocate_contiguous(3, 1).map(|f| f.address()) else {
            return false;
        };
        let Some(msrpm) = frames.allocate_contiguous(2, 1).map(|f| f.address()) else {
            return false;
        };
        // The second vCPU's VMCB and both vCPUs' FPU save areas (an XSAVE
        // area needs 64-byte alignment; a page has it).
        let Some(vmcb1) = frames.allocate_contiguous(1, 1).map(|f| f.address()) else {
            return false;
        };
        let Some(fpu0) = frames.allocate_contiguous(1, 1).map(|f| f.address()) else {
            return false;
        };
        let Some(fpu1) = frames.allocate_contiguous(1, 1).map(|f| f.address()) else {
            return false;
        };
        zero_region(vmcb1, PAGE_SIZE);
        zero_region(fpu0, PAGE_SIZE);
        zero_region(fpu1, PAGE_SIZE);

        zero_region(npt, 6 * PAGE_SIZE);
        zero_region(hsave, PAGE_SIZE);
        zero_region(host_vmcb, PAGE_SIZE);
        zero_region(vmcb, PAGE_SIZE);
        fill_region(iopm, 3 * PAGE_SIZE, 0xff);
        zero_region(msrpm, 2 * PAGE_SIZE);
        zero_region(ram, 0x20_0000);
        zero_region(scratch, PAGE_2M);

        let staging = chosen - STAGING_BACKOFF;
        if fat::load_root_file(b"VMLINUZ    ", 0, ram + staging, vmlinuz_bytes)
            != Some(vmlinuz_bytes)
        {
            crate::serial::line("AEROS_VM_LINUX_KERNEL stage=vmlinuz-read");
            return false;
        }

        let boot_flag = read_u16(ram, staging + 0x1fe);
        let hdrs = read_u32(ram, staging + 0x202);
        if boot_flag != 0xaa55 || hdrs != u32::from_le_bytes(*b"HdrS") {
            crate::serial::line("AEROS_VM_LINUX_KERNEL stage=bad-bzimage-header");
            return false;
        }
        let setup_sects = {
            let raw = read_u8(ram, staging + 0x1f1);
            if raw == 0 { 4u64 } else { raw as u64 }
        };
        let pm_offset = (setup_sects + 1) * 512;
        if pm_offset >= vmlinuz_bytes {
            crate::serial::line("AEROS_VM_LINUX_KERNEL stage=bad-setup-sects");
            return false;
        }
        let pm_bytes = vmlinuz_bytes - pm_offset;
        copy_guest(ram, staging + pm_offset, KERNEL_LOAD, pm_bytes);

        if initrd_bytes > 0
            && fat::load_root_file(b"INITRD     ", 0, ram + INITRD_LOAD, initrd_bytes)
                != Some(initrd_bytes)
        {
            crate::serial::line("AEROS_VM_LINUX_KERNEL stage=initrd-read");
            return false;
        }

        build_gdt(ram);
        build_page_tables(ram);
        build_zero_page(ram, staging, chosen, initrd_bytes);
        for (index, byte) in CMDLINE_TEXT.iter().enumerate() {
            unsafe { write_u8(ram, CMDLINE + index as u64, *byte) };
        }
        let npt_pd = build_linux_npt(npt, ram, chosen, scratch);
        build_vmcb(vmcb, npt, iopm, msrpm);
        // vCPU 1 shares the BSP's control area (intercepts, nested page table,
        // permission maps) but needs its own ASID: with one shared ASID the
        // TLB could hand one vCPU the other's translations.
        for offset in (0..0x400u64).step_by(8) {
            unsafe { write_u64(vmcb1, offset, read_u64(vmcb, offset)) };
        }
        unsafe { write_u32(vmcb1, 0x058, 2) };
        // Both vCPUs start from the host's current FPU state.
        unsafe {
            crate::arch::fpu::save_context(fpu0 as *mut u8);
            crate::arch::fpu::save_context(fpu1 as *mut u8);
        }
        if SMP {
            intercept_apic_msrs(msrpm);
            build_mp_table(ram);
        }

        let mut machine = Machine {
            vmcb,
            host_vmcb,
            npt_pd,
            ram,
            ram_bytes: chosen,
            scratch,
            gpr: [0; 14],
            cpuid_exits: 0,
            npf_maps: 0,
            pic: [0xff, 0xff],
            pic_icw: 0,
            pic_base: 0x30,
            tick_pending: false,
            reset: false,
            pit_latch: 0xffff,
            pit_start: rdtsc(),
            pit_wr_hi: false,
            pit_rd_hi: false,
            pit_mode: 2,
            port61: 0,
            cmos_index: 0,
            pci_addr: 0,
            exits: 0,
            serial_bytes: 0,
            io_reads: 0,
            io_writes: 0,
            msr_exits: 0,
            ticks: 0,
            last_tick_tsc: 0,
            last_exit: 0,
            last_rip: 0,
            last_port: 0,
            npf_gpa: 0,
            rootfs_bytes,
            rootfs_disk,
            pci_command: 0,
            bar0_raw: VIRTIO_IO_BASE,
            virtio_guest_features: 0,
            virtio_queue_pfn: 0,
            virtio_queue_select: 0,
            virtio_status: 0,
            virtio_isr: 0,
            virtio_last_avail: 0,
            virtio_used_idx: 0,
            virtio_irq_pending: false,
            blk_requests: 0,
            blk_errors: 0,
            uart_ier: 0,
            uart_lcr: 0x03,
            uart_thre_pending: false,
            uart_irq_injected: false,
            uart_rx: [0; UART_RX_SIZE],
            uart_rx_len: 0,
            kbd_shift: false,
            kbd_ctrl: false,
            kbd_extended: false,
            last_kbd_tsc: 0,
            host_input: false,
            dead: false,
            smp: SMP,
            cur: 0,
            slice_exits: 0,
            vcpus: [Vcpu::new(0, vmcb, fpu0), Vcpu::new(1, vmcb1, fpu1)],
            i8042_cfg: 0x47,
            i8042_cmd: 0,
            kbd_out: [0; KBD_QUEUE_SIZE],
            kbd_out_len: 0,
            mouse_events: [MouseEvent {
                dx: 0,
                dy: 0,
                buttons: 0,
                wheel: 0,
            }; MOUSE_EVENTS],
            mouse_event_len: 0,
            blk_meta: [BlkMeta {
                head: 0,
                status_addr: 0,
                data_len: 0,
                write: false,
            }; BLK_SLOTS],
            blk_finalized: 0,
            kbd_irq_injected: false,
            kbd_dev_arg: 0,
            kbd_scanning: false,
            mouse_reporting: false,
            mouse_epoch: 0,
            mouse_id: 0,
            mouse_rates: [0; 3],
            heartbeat_tsc: 0,
            mouse_packets: 0,
            mouse_arg: 0,
            aux_irq_injected: false,
            net_bar_raw: NET_IO_BASE,
            net_pci_command: 0,
            net_status: 0,
            net_isr: 0,
            net_features: 0,
            net_queue_select: 0,
            net_queue_pfn: [0; 2],
            net_last_avail: [0; 2],
            net_used_idx: [0; 2],
            net_irq_pending: false,
            net_mac: crate::nic::mac_address().unwrap_or([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]),
            net_rx_frames: 0,
            net_tx_frames: 0,
            net_tx_dropped: 0,
            last_net_poll_tsc: 0,
        };
        machine.gpr[3] = ZERO_PAGE;

        unsafe { write_msr(VM_HSAVE_PA_MSR, hsave) };
        crate::serial::format(format_args!(
            "AEROS_VM_LINUX_KERNEL stage=launch ram_mib={} vmlinuz_bytes={} initrd_bytes={} setup_sects={} pm_bytes={} rootfs_bytes={}\n",
            chosen / MIB,
            vmlinuz_bytes,
            initrd_bytes,
            setup_sects,
            pm_bytes,
            rootfs_bytes,
        ));
        // A visible "no signal" colour until the guest's fbdev driver draws.
        for pixel in 0..(FB_PITCH * FB_HEIGHT / 4) {
            unsafe { write_u32(ram, FB_BASE + pixel * 4, 0x0010_1418) };
        }
        unsafe { *core::ptr::addr_of_mut!(GUEST) = Some(machine) };
        true
    }

    include!("svm_apic.rs");

    fn build_linux_npt(npt: u64, ram: u64, ram_bytes: u64, scratch: u64) -> [u64; 4] {
        let flags = 0b111u64;
        let mut pd = [0u64; 4];
        unsafe {
            write_u64(npt, 0, (npt + PAGE_SIZE) | flags);
            for pdpt_index in 0..4u64 {
                let table = npt + (2 + pdpt_index) * PAGE_SIZE;
                pd[pdpt_index as usize] = table;
                write_u64(npt + PAGE_SIZE, pdpt_index * 8, table | flags);
                for pd_index in 0..512u64 {
                    let gpa = pdpt_index * 0x4000_0000 + pd_index * PAGE_2M;
                    let host = if gpa < ram_bytes { ram + gpa } else { scratch };
                    write_u64(table, pd_index * 8, host | flags | (1 << 7));
                }
            }
        }
        pd
    }

    fn build_gdt(ram: u64) {
        unsafe {
            write_u64(ram, GDT, 0);
            write_u64(ram, GDT + 8, 0x00cf_9b00_0000_ffff);
            write_u64(ram, GDT + 16, 0x00af_9b00_0000_ffff);
            write_u64(ram, GDT + 24, 0x00cf_9300_0000_ffff);
            write_u64(ram, GDT + 32, 0x0000_8b00_0000_0067);
            write_u64(ram, GDT + 40, 0);
        }
    }

    fn build_page_tables(ram: u64) {
        let present = 0b11u64;
        unsafe {
            write_u64(ram, PML4, PDPT | present);
            for pdpt_index in 0..4u64 {
                let pd = PD_BASE + pdpt_index * PAGE_SIZE;
                write_u64(ram, PDPT + pdpt_index * 8, pd | present);
                for pd_index in 0..512u64 {
                    let addr = pdpt_index * 0x4000_0000 + pd_index * PAGE_2M;
                    write_u64(ram, pd + pd_index * 8, addr | present | (1 << 7));
                }
            }
        }
    }

    fn build_zero_page(ram: u64, staging: u64, ram_bytes: u64, initrd_bytes: u64) {
        zero_region(ram + ZERO_PAGE, PAGE_SIZE);
        for offset in 0x1f1..0x270u64 {
            let byte = read_u8(ram, staging + offset);
            unsafe { write_u8(ram, ZERO_PAGE + offset, byte) };
        }
        let loadflags = (read_u8(ram, ZERO_PAGE + 0x211) | 0x01) & !0xa0;
        unsafe {
            write_u8(ram, ZERO_PAGE + 0x210, 0xff);
            write_u8(ram, ZERO_PAGE + 0x211, loadflags);
            write_u64(ram, ZERO_PAGE + 0x224, 0);
            write_u32(ram, ZERO_PAGE + 0x228, CMDLINE as u32);
            write_u32(ram, ZERO_PAGE + 0x218, INITRD_LOAD as u32);
            write_u32(ram, ZERO_PAGE + 0x21c, initrd_bytes as u32);
            write_u32(ram, ZERO_PAGE + 0x0c0, 0);
            write_u32(ram, ZERO_PAGE + 0x0c4, 0);
            let ext_mem = ((ram_bytes - MIB) / 1024).min(0xfc00) as u32;
            write_u32(ram, ZERO_PAGE + 0x1e0, ext_mem);
            write_u16(ram, ZERO_PAGE + 0x002, ext_mem.min(0xffff) as u16);

            let table = ZERO_PAGE + 0x2d0;
            let mut count = 0u64;
            let mut put = |addr: u64, size: u64, kind: u32| {
                let base = table + count * 20;
                write_u64(ram, base, addr);
                write_u64(ram, base + 8, size);
                write_u32(ram, base + 16, kind);
                count += 1;
            };
            put(0, 0x0009_fc00, 1);
            put(0x0009_fc00, 0x0006_0400, 2);
            put(0x0010_0000, FB_BASE - 0x0010_0000, 1);
            put(FB_BASE, FB_RESERVED, 2);
            put(FB_BASE + FB_RESERVED, ram_bytes - FB_BASE - FB_RESERVED, 1);
            write_u8(ram, ZERO_PAGE + 0x1e8, count as u8);

            // struct screen_info (boot_params offset 0): a VESA linear
            // framebuffer (orig_video_isVGA = VIDEO_TYPE_VLFB) at FB_BASE.
            write_u8(ram, ZERO_PAGE + 0x0f, 0x23);
            write_u16(ram, ZERO_PAGE + 0x12, FB_WIDTH as u16);
            write_u16(ram, ZERO_PAGE + 0x14, FB_HEIGHT as u16);
            write_u16(ram, ZERO_PAGE + 0x16, 32);
            write_u32(ram, ZERO_PAGE + 0x18, FB_BASE as u32);
            write_u32(ram, ZERO_PAGE + 0x1c, (FB_RESERVED / 0x1_0000) as u32);
            write_u16(ram, ZERO_PAGE + 0x24, FB_PITCH as u16);
            write_u8(ram, ZERO_PAGE + 0x26, 8);
            write_u8(ram, ZERO_PAGE + 0x27, 16);
            write_u8(ram, ZERO_PAGE + 0x28, 8);
            write_u8(ram, ZERO_PAGE + 0x29, 8);
            write_u8(ram, ZERO_PAGE + 0x2a, 8);
            write_u8(ram, ZERO_PAGE + 0x2b, 0);
            write_u8(ram, ZERO_PAGE + 0x2c, 8);
            write_u8(ram, ZERO_PAGE + 0x2d, 24);
        }
    }

    fn build_vmcb(vmcb: u64, npt: u64, iopm: u64, msrpm: u64) {
        unsafe {
            write_u32(vmcb, 0x008, 1 << 8);
            write_u32(
                vmcb,
                0x00c,
                INTERCEPT_INTR
                    | INTERCEPT_HLT
                    | INTERCEPT_IOIO_PROT
                    | INTERCEPT_CPUID
                    | INTERCEPT_PAUSE
                    | INTERCEPT_MSR_PROT,
            );
            write_u32(vmcb, 0x010, INTERCEPT2_VMRUN | INTERCEPT2_VMMCALL);
            write_u64(vmcb, 0x040, iopm);
            write_u64(vmcb, 0x048, msrpm);
            write_u32(vmcb, 0x058, 1);
            write_u8(vmcb, 0x05c, 1);
            // V_INTR_MASKING (bit 24): the guest's EFLAGS.IF only gates the
            // guest's own virtual interrupts, never the host's - together
            // with INTERCEPT_INTR a host APIC/keyboard interrupt exits to
            // the host instead of being delivered through the guest's IDT.
            write_u64(vmcb, 0x060, 1 << 24);
            write_u64(vmcb, 0x090, 1);
            write_u64(vmcb, 0x0b0, npt);

            segment(vmcb, 0x400, 0x18, 0x0093, 0xffff_ffff, 0);
            segment(vmcb, 0x410, 0x10, 0x029b, 0xffff_ffff, 0);
            segment(vmcb, 0x420, 0x18, 0x0093, 0xffff_ffff, 0);
            segment(vmcb, 0x430, 0x18, 0x0093, 0xffff_ffff, 0);
            segment(vmcb, 0x440, 0x18, 0x0093, 0xffff_ffff, 0);
            segment(vmcb, 0x450, 0x18, 0x0093, 0xffff_ffff, 0);
            segment(vmcb, 0x460, 0, 0, 0x2f, GDT);
            segment(vmcb, 0x470, 0, 0, 0xffff, 0);
            segment(vmcb, 0x480, 0, 0, 0, 0);
            segment(vmcb, 0x490, 0x20, 0x008b, 0x67, 0);

            write_u8(vmcb, 0x4cb, 0);
            write_u64(vmcb, 0x4d0, EFER_SVME | EFER_LME | EFER_LMA | EFER_NXE);
            write_u64(vmcb, 0x548, CR4_PAE);
            write_u64(vmcb, 0x550, PML4);
            write_u64(vmcb, 0x558, CR0_LINUX);
            write_u64(vmcb, 0x560, 0x0000_0400);
            write_u64(vmcb, 0x568, 0xffff_0ff0);
            write_u64(vmcb, 0x570, 0x0000_0002);
            write_u64(vmcb, 0x578, KERNEL_LOAD + 0x200);
            write_u64(vmcb, 0x5d8, STACK_TOP);
            write_u64(vmcb, 0x5f8, 0);
            write_u64(vmcb, 0x640, 0);
            write_u64(vmcb, 0x668, 0x0007_0406_0007_0406);
        }
    }

    impl Machine {
        /// Runs the guest for roughly `budget_tsc` cycles. True means the
        /// slice ended normally (budget spent or the guest went idle in a
        /// HLT); false means the guest is finished (power-off, reset, or an
        /// exit this monitor doesn't handle).
        fn run_for(&mut self, budget_tsc: u64) -> bool {
            let start = rdtsc();
            // The single-CPU guest restarts its (fast, fixed) tick clock on
            // every slice. In SMP mode the tick follows the guest's PIT
            // programming (4 ms at HZ=250), which is longer than a desktop
            // slice - resetting it each call would mean it never came due.
            if !self.smp || self.last_tick_tsc == 0 {
                self.last_tick_tsc = start;
            }
            if self.heartbeat_tsc == 0 {
                self.heartbeat_tsc = start;
            }
            let mut handled_at = start;
            while self.exits < MAX_EXITS_LINUX {
                // Host-side stall detector: an exit that took ~0.3 s or more
                // to handle (a blocked device model would look like a guest
                // hang from inside).
                let looped = rdtsc();
                if handled_at != start && looped.wrapping_sub(handled_at) > 900_000_000 {
                    crate::serial::format(format_args!(
                        "AEROS_VM_SLOW cycles={} exit={:#x} port={:#x} cur={} rip={:#x}\n",
                        looped.wrapping_sub(handled_at),
                        self.last_exit,
                        self.last_port,
                        self.cur,
                        self.last_rip
                    ));
                }
                if self.smp && !self.smp_schedule(rdtsc()) {
                    // Every vCPU is halted. Before the guest starts its own
                    // APIC timers the 8259/PIT keeps time (each idle wake-up
                    // is one tick, as in the single-CPU case); afterwards
                    // there is nothing to do until a timer or device fires.
                    let now = rdtsc();
                    if self.pit_ticking()
                        && !self.tick_pending
                        && !self.virq_pending()
                        && now.wrapping_sub(self.last_tick_tsc) >= self.tick_period()
                    {
                        if self.cur != 0 {
                            self.switch_vcpu(0);
                        }
                        self.vcpus[0].halted = false;
                        self.inject_timer();
                        self.tick_pending = true;
                    }
                    if !self.host_input || now.wrapping_sub(start) > budget_tsc {
                        // The desktop's slice is over: hand control back
                        // instead of spinning until the guest's next timer,
                        // which in a tickless idle can be seconds away and
                        // would freeze the host UI meanwhile.
                        return true;
                    }
                    core::hint::spin_loop();
                    continue;
                }
                self.maybe_tick();
                let ctx = self.gpr.as_mut_ptr();
                unsafe { vmentry(self.vmcb, self.host_vmcb, ctx) };
                self.exits += 1;
                self.slice_exits = self.slice_exits.saturating_add(1);
                let now = rdtsc();
                handled_at = now;
                if self.tick_pending
                    && now.wrapping_sub(self.last_tick_tsc) > 20 * self.tick_period()
                {
                    self.tick_pending = false;
                    self.last_tick_tsc = now;
                }
                if now.wrapping_sub(self.heartbeat_tsc) > HEARTBEAT_TSC {
                    self.heartbeat_tsc = now;
                    let rflags = unsafe { read_u64(self.vmcb, 0x570) };
                    let int_state = unsafe { read_u64(self.vmcb, 0x068) };
                    let vintr = unsafe { read_u64(self.vmcb, 0x060) };
                    crate::serial::format(format_args!(
                        "\n[aeros-vm exits={} rip={:#x} exit={:#x} port={:#x} ticks={} serial={} blk_requests={} blk_errors={} avail={} used={} irq_pending={} status={:#x} rflags_if={} int_shadow={} vintr={:#x} pic0={:#x} pic1={:#x} pic_icw={} net_rx={} net_tx={} net_drop={} mouse_reporting={} mouse_epoch={} mouse_packets={} out_len={} cfg={:#x} cur={} ap_started={} ap_halted={} ap_rip={:#x} bsp_halted={} bsp_irr={:?} bsp_isr={:?} bsp_lvt_timer={:#x} bsp_deadline_in={} bsp_sivr={:#x} bsp_tpr={}]\n",
                        self.exits,
                        self.last_rip,
                        self.last_exit,
                        self.last_port,
                        self.ticks,
                        self.serial_bytes,
                        self.blk_requests,
                        self.blk_errors,
                        self.virtio_last_avail,
                        self.virtio_used_idx,
                        self.virtio_irq_pending,
                        self.virtio_status,
                        rflags & (1 << 9) != 0,
                        int_state & 1,
                        vintr,
                        self.pic[0],
                        self.pic[1],
                        self.pic_icw,
                        self.net_rx_frames,
                        self.net_tx_frames,
                        self.net_tx_dropped,
                        self.mouse_reporting,
                        self.mouse_epoch,
                        self.mouse_packets,
                        self.kbd_out_len,
                        self.i8042_cfg,
                        self.cur,
                        self.vcpus[1].started,
                        self.vcpus[1].halted,
                        unsafe { read_u64(self.vcpus[1].vmcb, 0x578) },
                        self.vcpus[0].halted,
                        Apic::highest(&self.vcpus[0].apic.irr),
                        Apic::highest(&self.vcpus[0].apic.isr),
                        self.vcpus[0].apic.lvt[1],
                        (self.vcpus[0].apic.timer_deadline as i64).wrapping_sub(now as i64),
                        self.vcpus[0].apic.sivr,
                        self.vcpus[0].apic.tpr,
                    ));
                }
                if now.wrapping_sub(start) > budget_tsc {
                    return true;
                }
                let exit = unsafe { read_u64(self.vmcb, 0x070) };
                self.last_exit = exit;
                self.last_rip = unsafe { read_u64(self.vmcb, 0x578) };
                match exit {
                    EXIT_IOIO => self.handle_io(),
                    EXIT_CPUID => self.handle_cpuid(),
                    EXIT_MSR => self.handle_msr(),
                    // The IRET that ends an injected NMI's handler (intercepted
                    // only while an NMI is being serviced): NMIs are deliverable
                    // again. It is a trap - the IRET has already completed.
                    EXIT_IRET => self.nmi_done(),
                    EXIT_PAUSE => {
                        self.advance(2);
                        // A spinning vCPU is waiting for the other one.
                        if self.smp {
                            self.slice_exits = SMP_SLICE_EXITS;
                        }
                    }
                    // A host interrupt (APIC timer, keyboard...) forced this
                    // exit; it is delivered to the HOST handler as soon as
                    // host interrupts are enabled. In a headless run they
                    // are off, so open a one-instruction window.
                    EXIT_INTR => {
                        if self.smp {
                            self.slice_exits = SMP_SLICE_EXITS;
                        }
                        if !host_interrupts_enabled() {
                            crate::arch::enable_interrupts();
                            core::hint::spin_loop();
                            crate::arch::disable_interrupts();
                        }
                    }
                    EXIT_SHUTDOWN | 0x048 => {
                        self.reset = true;
                        return false;
                    }
                    EXIT_HLT => {
                        let rflags = unsafe { read_u64(self.vmcb, 0x570) };
                        if rflags & (1 << 9) == 0 {
                            // HLT with interrupts off never wakes: an AP going
                            // offline is fine, the BSP doing it is the end.
                            if self.smp && self.cur != 0 {
                                self.advance(1);
                                self.vcpus[self.cur].halted = true;
                                continue;
                            }
                            return false;
                        }
                        self.advance(1);
                        if self.smp {
                            // Clear the STI interrupt shadow (see below), park
                            // this vCPU until something wakes it, and let the
                            // scheduler pick who runs next.
                            let int_state = unsafe { read_u64(self.vmcb, 0x068) };
                            unsafe { write_u64(self.vmcb, 0x068, int_state & !1) };
                            self.vcpus[self.cur].halted = true;
                            continue;
                        }
                        // The STI that preceded this HLT left an interrupt shadow
                        // in the VMCB; the HLT is done now, so drop it or a
                        // pending device interrupt could never be injected.
                        let int_state = unsafe { read_u64(self.vmcb, 0x068) };
                        unsafe { write_u64(self.vmcb, 0x068, int_state & !1) };
                        // A device interrupt is waiting to be delivered (e.g. the
                        // next byte of a mouse packet): keep running so it goes
                        // in right away - drivers drop packets whose bytes
                        // straggle - and don't spend a timer tick on it.
                        if self.has_pending_irq() {
                            continue;
                        }
                        self.inject_timer();
                        // Idle guest in desktop mode: hand the CPU back to
                        // the desktop until its next tick instead of
                        // spinning through back-to-back HLT exits.
                        if !self.host_input {
                            return true;
                        }
                    }
                    EXIT_VMMCALL => {
                        unsafe { write_u64(self.vmcb, 0x5f8, 2) };
                        self.advance(3);
                    }
                    EXIT_NPF => {
                        let gpa = unsafe { read_u64(self.vmcb, 0x080) };
                        self.npf_gpa = gpa;
                        if !self.map_fault(gpa) {
                            return false;
                        }
                    }
                    _ => return false,
                }
                if self.reset {
                    return false;
                }
            }
            false
        }

        /// A virtual interrupt is already waiting to be taken by the guest
        /// (V_IRQ). Injecting another one now would overwrite it - and for an
        /// APIC vector that has already been marked in service the guest would
        /// then never run its handler or acknowledge it.
        fn virq_pending(&self) -> bool {
            let int_ctl = unsafe { read_u64(self.vmcb, 0x060) };
            int_ctl & (1 << 8) != 0
        }

        fn maybe_tick(&mut self) {
            if self.mouse_event_len > 0 {
                self.mouse_flush();
            }
            self.blk_poll();
            if self.smp {
                if self.virq_pending() {
                    return;
                }
                if self.apic_inject() {
                    return;
                }
                // The legacy devices (8259, PIT, UART, keyboard, virtio)
                // belong to the BSP.
                if self.cur != 0 {
                    return;
                }
            }
            // SVM's V_INTR can only carry one pending vector at a time, so
            // on any exit where an injection window is actually open, only
            // ONE of {virtio completion, timer tick} can be delivered. The
            // periodic timer's own gate (`elapsed >= TICK_TSC`) is true on
            // almost every exit by construction, so if it were checked
            // first it would win every single open window and a pending
            // block completion would starve forever - which is exactly
            // what happened before this was reordered (virtio_irq_pending
            // stayed true for the rest of the guest's run). A block
            // completion is rarer and higher priority than one periodic
            // tick, so it goes first; the timer simply waits for the next
            // window, same as it already tolerates via `tick_pending`.
            if self.virtio_irq_pending {
                let rflags = unsafe { read_u64(self.vmcb, 0x570) };
                let int_state = unsafe { read_u64(self.vmcb, 0x068) };
                if rflags & (1 << 9) != 0 && int_state & 1 == 0 {
                    self.inject_irq(VIRTIO_IRQ_LINE);
                    self.virtio_irq_pending = false;
                    return;
                }
            }
            let now = rdtsc();
            if self.net_status & 4 != 0 && now.wrapping_sub(self.last_net_poll_tsc) >= TICK_TSC {
                self.last_net_poll_tsc = now;
                self.net_poll_rx();
            }
            // The NIC's line is on the slave PIC (IRQ 10 = slave line 2).
            if self.net_irq_pending && self.pic[1] & (1 << 2) == 0 {
                let rflags = unsafe { read_u64(self.vmcb, 0x570) };
                let int_state = unsafe { read_u64(self.vmcb, 0x068) };
                if rflags & (1 << 9) != 0 && int_state & 1 == 0 {
                    self.inject_irq(NET_IRQ_LINE);
                    self.net_irq_pending = false;
                    return;
                }
            }
            if self.host_input {
                self.poll_keyboard();
            }
            if self.kbd_wants_irq() && !self.kbd_irq_injected {
                let rflags = unsafe { read_u64(self.vmcb, 0x570) };
                let int_state = unsafe { read_u64(self.vmcb, 0x068) };
                if rflags & (1 << 9) != 0 && int_state & 1 == 0 {
                    self.inject_irq(KBD_IRQ_LINE);
                    self.kbd_irq_injected = true;
                    return;
                }
            }
            if self.aux_wants_irq() && !self.aux_irq_injected {
                let rflags = unsafe { read_u64(self.vmcb, 0x570) };
                let int_state = unsafe { read_u64(self.vmcb, 0x068) };
                if rflags & (1 << 9) != 0 && int_state & 1 == 0 {
                    self.inject_irq(AUX_IRQ_LINE);
                    self.aux_irq_injected = true;
                    return;
                }
            }
            if self.uart_wants_irq() && !self.uart_irq_injected {
                let rflags = unsafe { read_u64(self.vmcb, 0x570) };
                let int_state = unsafe { read_u64(self.vmcb, 0x068) };
                if rflags & (1 << 9) != 0 && int_state & 1 == 0 {
                    self.inject_irq(UART_IRQ_LINE);
                    self.uart_irq_injected = true;
                    return;
                }
            }
            if !self.tick_pending
                && rdtsc().wrapping_sub(self.last_tick_tsc) >= self.tick_period()
                && self.pit_ticking()
            {
                let rflags = unsafe { read_u64(self.vmcb, 0x570) };
                let int_state = unsafe { read_u64(self.vmcb, 0x068) };
                if rflags & (1 << 9) != 0 && int_state & 1 == 0 {
                    self.inject_timer();
                    self.tick_pending = true;
                }
            }
        }

        fn has_pending_irq(&self) -> bool {
            (self.kbd_wants_irq() && !self.kbd_irq_injected)
                || (self.aux_wants_irq() && !self.aux_irq_injected)
                || (self.uart_wants_irq() && !self.uart_irq_injected)
                || self.net_irq_pending
                || self.virtio_irq_pending
        }

        fn irq_unmasked(&self, line: u8) -> bool {
            self.pic[0] & (1 << line) == 0
        }

        fn kbd_push(&mut self, byte: u8) {
            self.out_push(byte as u16);
        }

        fn aux_push(&mut self, byte: u8) {
            self.out_push(byte as u16 | 0x100);
        }

        fn out_push(&mut self, entry: u16) {
            if self.kbd_out_len < KBD_QUEUE_SIZE {
                self.kbd_out[self.kbd_out_len] = entry;
                self.kbd_out_len += 1;
            }
        }

        fn drop_pending_aux(&mut self) {
            let mut kept = 0;
            for index in 0..self.kbd_out_len {
                let entry = self.kbd_out[index];
                if entry & 0x100 == 0 {
                    self.kbd_out[kept] = entry;
                    kept += 1;
                }
            }
            self.kbd_out_len = kept;
        }

        fn kbd_pop(&mut self) -> u8 {
            if self.kbd_out_len == 0 {
                return 0;
            }
            let entry = self.kbd_out[0];
            self.kbd_out.copy_within(1..self.kbd_out_len, 0);
            self.kbd_out_len -= 1;
            entry as u8
        }

        /// Status register: bit0 output buffer full, bit2 system flag, bit5
        /// set while the byte waiting came from the auxiliary port.
        fn i8042_status(&self) -> u32 {
            let aux = self.kbd_out_len > 0 && self.kbd_out[0] & 0x100 != 0;
            0x04 | (self.kbd_out_len > 0) as u32 | ((aux as u32) << 5)
        }

        fn kbd_wants_irq(&self) -> bool {
            self.kbd_out_len > 0
                && self.kbd_out[0] & 0x100 == 0
                && self.i8042_cfg & 1 != 0
                && self.irq_unmasked(KBD_IRQ_LINE)
        }

        fn aux_wants_irq(&self) -> bool {
            self.kbd_out_len > 0
                && self.kbd_out[0] & 0x100 != 0
                && self.i8042_cfg & 2 != 0
                && self.pic[1] & (1 << (AUX_IRQ_LINE - 8)) == 0
        }

        /// A host key event (translated set-1 scancode) for the guest's
        /// AT keyboard; ignored until the guest driver has enabled scanning.
        fn kbd_feed(&mut self, code: u8) {
            if self.kbd_scanning && self.i8042_cfg & 0x10 == 0 {
                self.kbd_push(code);
            }
        }

        /// Queues PS/2 mouse packets for a host pointer event: `dx`/`dy` in
        /// screen orientation (y grows downward), `buttons` bit0 left, bit1
        /// right, bit2 middle. Large motions are split into several packets
        /// (each axis is one signed byte); dropped if the guest hasn't
        /// enabled data reporting or the output buffer is full.
        fn mouse_feed(&mut self, dx: i32, dy: i32, buttons: u8, wheel: i32) {
            if !self.mouse_reporting || self.i8042_cfg & 0x20 != 0 {
                self.mouse_event_len = 0;
                return;
            }
            let event = MouseEvent {
                dx,
                dy,
                buttons,
                wheel,
            };
            if self.mouse_event_len == MOUSE_EVENTS {
                // Backlog full: fold into the newest event (motion adds up,
                // the latest button state wins).
                let last = &mut self.mouse_events[MOUSE_EVENTS - 1];
                last.dx += dx;
                last.dy += dy;
                last.wheel += wheel;
                last.buttons = buttons;
            } else {
                self.mouse_events[self.mouse_event_len] = event;
                self.mouse_event_len += 1;
            }
            self.mouse_flush();
        }

        /// Emits packets for the oldest waiting events while the controller's
        /// output buffer has room (with headroom kept for command ACKs).
        fn mouse_flush(&mut self) {
            let packet_len = if self.mouse_id == 3 { 4 } else { 3 };
            while self.mouse_event_len > 0 {
                if self.kbd_out_len + packet_len > KBD_QUEUE_SIZE - 16 {
                    return;
                }
                let event = &mut self.mouse_events[0];
                let step_x = event.dx.clamp(-255, 255);
                let step_y = (-event.dy).clamp(-255, 255); // PS/2 y grows upward
                event.dx -= step_x;
                event.dy += step_y;
                let flags = 0x08
                    | (event.buttons & 7)
                    | (((step_x < 0) as u8) << 4)
                    | (((step_y < 0) as u8) << 5);
                let wheel = core::mem::take(&mut event.wheel);
                let finished = event.dx == 0 && event.dy == 0;
                self.mouse_packets += 1;
                self.aux_push(flags);
                self.aux_push(step_x as u8);
                self.aux_push(step_y as u8);
                if packet_len == 4 {
                    // IntelliMouse wheel byte (signed; positive = scroll down).
                    self.aux_push(wheel.clamp(-8, 7) as i8 as u8);
                }
                if finished {
                    self.mouse_events.copy_within(1..self.mouse_event_len, 0);
                    self.mouse_event_len -= 1;
                }
            }
        }

        fn i8042_command(&mut self, command: u8) {
            match command {
                0x20 => self.kbd_push(self.i8042_cfg),
                0x60 | 0xd1 | 0xd3 | 0xd4 => self.i8042_cmd = command,
                0xaa => self.kbd_push(0x55),
                0xab => self.kbd_push(0x00),
                // Auxiliary interface test: pass.
                0xa9 => self.kbd_push(0x00),
                0xa7 => self.i8042_cfg |= 0x20,
                0xa8 => self.i8042_cfg &= !0x20,
                0xad => self.i8042_cfg |= 0x10,
                0xae => self.i8042_cfg &= !0x10,
                0xc0 => self.kbd_push(0x00),
                _ => {}
            }
        }

        fn i8042_data(&mut self, value: u8) {
            match core::mem::take(&mut self.i8042_cmd) {
                0x60 => self.i8042_cfg = value,
                // Output-port write: A20 etc, nothing to model.
                0xd1 => {}
                // Aux loopback: the byte comes straight back as aux data.
                0xd3 => self.aux_push(value),
                0xd4 => self.mouse_device_write(value),
                _ => self.kbd_device_write(value),
            }
        }

        /// The AT keyboard behind port 0x60: just enough of its command set
        /// (reset, identify, LEDs, scancode set, typematic, enable/disable
        /// scanning) for Linux's atkbd driver to bind and enable it.
        fn kbd_device_write(&mut self, value: u8) {
            match core::mem::take(&mut self.kbd_dev_arg) {
                0xf0 => {
                    self.kbd_push(0xfa);
                    if value == 0 {
                        self.kbd_push(0x02);
                    }
                    return;
                }
                0xed | 0xf3 => {
                    self.kbd_push(0xfa);
                    return;
                }
                _ => {}
            }
            match value {
                0xff => {
                    self.kbd_push(0xfa);
                    self.kbd_push(0xaa);
                }
                0xf2 => {
                    self.kbd_push(0xfa);
                    self.kbd_push(0xab);
                    self.kbd_push(0x83);
                }
                0xf4 => {
                    self.kbd_scanning = true;
                    self.kbd_push(0xfa);
                }
                0xf5 => {
                    self.kbd_scanning = false;
                    self.kbd_push(0xfa);
                }
                0xee => self.kbd_push(0xee),
                0xed | 0xf0 | 0xf3 => {
                    self.kbd_dev_arg = value;
                    self.kbd_push(0xfa);
                }
                _ => self.kbd_push(0xfa),
            }
        }

        /// A plain 3-byte PS/2 mouse behind the auxiliary port (reset,
        /// identify, status request, sample rate/resolution, enable
        /// reporting) - enough for Linux's psmouse to bind as a generic
        /// PS/2 mouse. Answers go to the output buffer tagged as aux data.
        fn mouse_device_write(&mut self, value: u8) {
            // A command supersedes any motion data still waiting to be read;
            // left in place it would bury (or, with a full buffer, drop) the
            // reply the driver is waiting for.
            self.drop_pending_aux();
            let pending = core::mem::take(&mut self.mouse_arg);
            if pending != 0 {
                if pending == 0xf3 {
                    // Sample-rate argument: 200, 100, 80 in a row is the
                    // IntelliMouse handshake that turns the wheel on.
                    self.mouse_rates = [self.mouse_rates[1], self.mouse_rates[2], value];
                    if self.mouse_rates == [200, 100, 80] {
                        self.mouse_id = 3;
                    }
                }
                self.aux_push(0xfa); // argument byte of 0xe8 / 0xf3
                return;
            }
            match value {
                0xff => {
                    self.mouse_reporting = false;
                    self.mouse_id = 0;
                    self.mouse_rates = [0; 3];
                    self.aux_push(0xfa);
                    self.aux_push(0xaa);
                    self.aux_push(0x00);
                }
                0xf2 => {
                    self.aux_push(0xfa);
                    self.aux_push(self.mouse_id);
                }
                0xe9 => {
                    self.aux_push(0xfa);
                    self.aux_push((self.mouse_reporting as u8) << 5);
                    self.aux_push(0x02);
                    self.aux_push(0x64);
                }
                0xf4 => {
                    self.mouse_reporting = true;
                    self.mouse_epoch = self.mouse_epoch.wrapping_add(1);
                    self.aux_push(0xfa);
                }
                0xf5 => {
                    self.mouse_reporting = false;
                    self.aux_push(0xfa);
                }
                0xe8 | 0xf3 => {
                    self.mouse_arg = value;
                    self.aux_push(0xfa);
                }
                _ => self.aux_push(0xfa),
            }
        }
        fn uart_wants_irq(&self) -> bool {
            self.irq_unmasked(UART_IRQ_LINE)
                && ((self.uart_ier & 1 != 0 && self.uart_rx_len > 0)
                    || (self.uart_ier & 2 != 0 && self.uart_thre_pending))
        }

        fn uart_pop_rx(&mut self) -> u8 {
            if self.uart_rx_len == 0 {
                return 0;
            }
            let byte = self.uart_rx[0];
            self.uart_rx.copy_within(1..self.uart_rx_len, 0);
            self.uart_rx_len -= 1;
            byte
        }

        fn uart_push_rx(&mut self, byte: u8) {
            if self.uart_rx_len < UART_RX_SIZE {
                self.uart_rx[self.uart_rx_len] = byte;
                self.uart_rx_len += 1;
            }
        }

        /// Forwards the host PS/2 keyboard into the guest's serial console:
        /// the guest has no keyboard controller of its own, but its console
        /// is ttyS0, so typed characters arrive as UART receive data. Polled
        /// at most once per timer period so the port reads don't tax every
        /// VM exit.
        fn poll_keyboard(&mut self) {
            let now = rdtsc();
            if now.wrapping_sub(self.last_kbd_tsc) < TICK_TSC {
                return;
            }
            self.last_kbd_tsc = now;
            crate::keyboard::handle_interrupt();
            while let Some(code) = crate::keyboard::pop_scancode() {
                if code == 0xe0 {
                    self.kbd_extended = true;
                    continue;
                }
                let extended = core::mem::take(&mut self.kbd_extended);
                let released = code & 0x80 != 0;
                let make = code & 0x7f;
                match make {
                    0x2a | 0x36 if !extended => self.kbd_shift = !released,
                    0x1d => self.kbd_ctrl = !released,
                    _ if released || extended => {}
                    0x1c => self.uart_push_rx(b'\r'),
                    0x0e => self.uart_push_rx(0x7f),
                    0x0f => self.uart_push_rx(b'\t'),
                    0x01 => self.uart_push_rx(0x1b),
                    0x39 => self.uart_push_rx(b' '),
                    _ => {
                        if let Some(character) =
                            crate::shell::scancode_character(make, self.kbd_shift, false)
                        {
                            let byte = if self.kbd_ctrl && character.is_ascii_alphabetic() {
                                character.to_ascii_lowercase() & 0x1f
                            } else {
                                character
                            };
                            self.uart_push_rx(byte);
                        }
                    }
                }
            }
        }

        /// Period of the legacy timer tick. In SMP mode it follows what the
        /// guest programmed into the PIT (jiffies must agree with the APIC
        /// timer the guest calibrates against them); the single-CPU guest
        /// keeps its fixed fast tick.
        fn tick_period(&self) -> u64 {
            if !self.smp {
                return TICK_TSC;
            }
            let latch = if self.pit_latch == 0 {
                0x1_0000u64
            } else {
                self.pit_latch as u64
            };
            (latch * TSC_PER_PIT).max(2 * TICK_TSC)
        }

        /// Whether the legacy timer tick should still run. The single-CPU guest
        /// always gets it; in SMP mode it stops when the guest shuts channel 0
        /// down (it verifies its APIC timer against PIT-driven jiffies first,
        /// so it must keep ticking until then).
        fn pit_ticking(&self) -> bool {
            !self.smp || matches!(self.pit_mode, 2 | 3 | 6 | 7)
        }

        fn inject_timer(&mut self) {
            self.ticks += 1;
            self.inject_irq(0);
        }

        /// Injects a legacy 8259-routed interrupt: `irq_line` is the offset
        /// from the master PIC's programmed base vector (ICW2), the same
        /// scheme `inject_timer` used implicitly for IRQ0 before this was
        /// generalized to also carry virtio-blk's completion IRQ.
        fn inject_irq(&mut self, irq_line: u8) {
            self.inject_vector(self.pic_base.wrapping_add(irq_line));
        }

        /// Raises virtual interrupt `vector` in the current vCPU (V_IRQ).
        fn inject_vector(&mut self, vector: u8) {
            let mut vintr = unsafe { read_u64(self.vmcb, 0x060) };
            vintr &= !0x0000_00ff_001f_0100;
            vintr |= 1 << 8;
            vintr |= 0xf << 16;
            vintr |= 1 << 20;
            vintr |= (vector as u64) << 32;
            unsafe { write_u64(self.vmcb, 0x060, vintr) };
        }

        fn handle_cpuid(&mut self) {
            self.cpuid_exits += 1;
            let leaf = unsafe { read_u64(self.vmcb, 0x5f8) } as u32;
            let subleaf = self.gpr[1] as u32;
            let result = __cpuid_count(leaf, subleaf);
            let (mut a, mut b, mut c, mut d) = (result.eax, result.ebx, result.ecx, result.edx);
            let vcpu_id = self.cur as u32;
            match leaf {
                0x0 if a < 0x16 => a = 0x16,
                // Two single-thread cores: each vCPU reports its APIC id and
                // the logical-processor count, x2APIC stays available, the
                // TSC-deadline timer is hidden (the guest uses the APIC timer
                // this monitor emulates) and HTT is off.
                0x1 if self.smp => {
                    b = (b & 0x0000_ffff) | (vcpu_id << 24) | ((NUM_VCPUS as u32) << 16);
                    c &= !(1 << 24);
                    d &= !(1 << 28);
                }
                0xb if self.smp => {
                    let (level_a, level_b, level_c) = match subleaf {
                        0 => (0, 1, 0x100),
                        1 => (1, NUM_VCPUS as u32, 0x201),
                        other => (0, 0, other & 0xff),
                    };
                    a = level_a;
                    b = level_b;
                    c = level_c;
                    d = vcpu_id;
                }
                0x8000_0001 if self.smp => c |= 1 << 1,
                0x8000_0008 if self.smp => c = (c & !0xf0ff) | (1 << 12) | (NUM_VCPUS as u32 - 1),
                0x8000_001e if self.smp => {
                    a = vcpu_id;
                    b = vcpu_id;
                    c = 0;
                    d = 0;
                }
                0x6 => a &= !(1 << 2),
                0x15 => {
                    a = 0;
                    b = 0;
                    c = 0;
                    d = 0;
                }
                0x16 => {
                    a = 3300;
                    b = 3300;
                    c = 100;
                    d = 0;
                }
                0x4000_0000 => {
                    a = 0x4000_0010;
                    b = 0x4b4d_564b;
                    c = 0x564b_4d56;
                    d = 0x0000_004d;
                }
                0x4000_0001 => {
                    a = (1 << 0) | (1 << 3) | (1 << 24);
                    b = 0;
                    c = 0;
                    d = 0;
                }
                0x4000_0010 => {
                    a = 3_300_000;
                    b = 1_000_000;
                    c = 0;
                    d = 0;
                }
                0x4000_0002..=0x4000_00ff => {
                    a = 0;
                    b = 0;
                    c = 0;
                    d = 0;
                }
                _ => {}
            }
            unsafe { write_u64(self.vmcb, 0x5f8, a as u64) };
            self.gpr[0] = b as u64;
            self.gpr[1] = c as u64;
            self.gpr[2] = d as u64;
            self.advance(2);
        }

        fn handle_msr(&mut self) {
            self.msr_exits += 1;
            let info1 = unsafe { read_u64(self.vmcb, 0x078) };
            let msr = self.gpr[1] as u32;
            let rax = unsafe { read_u64(self.vmcb, 0x5f8) };
            if self.smp && is_apic_msr(msr) {
                if info1 & 1 != 0 {
                    let value = (self.gpr[2] << 32) | (rax & 0xffff_ffff);
                    self.apic_msr_write(msr, value);
                } else {
                    let value = self.apic_msr_read(msr);
                    unsafe { write_u64(self.vmcb, 0x5f8, value & 0xffff_ffff) };
                    self.gpr[2] = value >> 32;
                }
                self.advance(2);
                return;
            }
            if info1 & 1 != 0 {
                let value = (self.gpr[2] << 32) | (rax & 0xffff_ffff);
                match msr {
                    MSR_KVM_SYSTEM_TIME => self.pvclock_enable(value),
                    MSR_KVM_WALL_CLOCK => self.wallclock_write(value),
                    _ => {}
                }
            } else {
                unsafe { write_u64(self.vmcb, 0x5f8, 0) };
                self.gpr[2] = 0;
            }
            self.advance(2);
        }

        fn pvclock_enable(&mut self, value: u64) {
            if value & 1 == 0 {
                return;
            }
            let gpa = value & !0x3;
            if gpa + 32 > self.ram_bytes {
                return;
            }
            let base = self.ram + gpa;
            unsafe {
                write_u32(base, 0, 0);
                write_u32(base, 4, 0);
                write_u64(base, 8, rdtsc());
                write_u64(base, 16, 0);
                write_u32(base, 24, PVCLOCK_MUL);
                write_u8(base, 28, 0);
                write_u8(base, 29, 1);
                write_u8(base, 30, 0);
                write_u8(base, 31, 0);
                write_u32(base, 0, 2);
            }
        }

        fn wallclock_write(&mut self, value: u64) {
            let gpa = value & !0x3;
            if gpa + 12 > self.ram_bytes {
                return;
            }
            let base = self.ram + gpa;
            unsafe {
                write_u32(base, 0, 0);
                // Wall-clock seconds at guest boot: the host's real time, or TLS
                // certificates/OCSP responses look "from the future".
                write_u32(base, 4, crate::rtc::unix_seconds() as u32);
                write_u32(base, 8, 0);
                write_u32(base, 0, 2);
            }
        }

        fn advance(&self, insn_len: u64) {
            let rip = unsafe { read_u64(self.vmcb, 0x578) };
            let n_rip = unsafe { read_u64(self.vmcb, 0x0c8) };
            let next = if n_rip > rip { n_rip } else { rip + insn_len };
            unsafe { write_u64(self.vmcb, 0x578, next) };
        }

        fn map_fault(&mut self, gpa: u64) -> bool {
            if gpa >= GUEST_LIMIT || self.npf_maps > 4096 {
                return false;
            }
            let pdpt_index = (gpa >> 30) & 3;
            let pd_index = (gpa >> 21) & 0x1ff;
            let host = if gpa < self.ram_bytes {
                self.ram + (gpa & !(PAGE_2M - 1))
            } else {
                self.scratch
            };
            unsafe {
                write_u64(
                    self.npt_pd[pdpt_index as usize],
                    pd_index * 8,
                    host | 0b111 | (1 << 7),
                );
                write_u64(self.vmcb, 0x058, 1);
            }
            self.npf_maps += 1;
            true
        }

        fn handle_io(&mut self) {
            let info1 = unsafe { read_u64(self.vmcb, 0x078) };
            let next_rip = unsafe { read_u64(self.vmcb, 0x080) };
            let is_in = info1 & 1 != 0;
            let port = ((info1 >> 16) & 0xffff) as u16;
            self.last_port = port;
            let size = if info1 & (1 << 4) != 0 {
                1
            } else if info1 & (1 << 5) != 0 {
                2
            } else {
                4
            };
            let rax = unsafe { read_u64(self.vmcb, 0x5f8) };
            if is_in {
                self.io_reads += 1;
                let value = self.pio_in(port, size);
                let mask = match size {
                    1 => 0xffu64,
                    2 => 0xffff,
                    _ => 0xffff_ffff,
                };
                unsafe { write_u64(self.vmcb, 0x5f8, (rax & !mask) | (value as u64 & mask)) };
            } else {
                self.io_writes += 1;
                self.pio_out(port, rax as u32, size);
            }
            unsafe { write_u64(self.vmcb, 0x578, next_rip) };
        }

        fn pio_out(&mut self, port: u16, value: u32, size: u8) {
            match port {
                0x3f8 | 0x3f9 if self.uart_lcr & 0x80 != 0 => {}
                0x3f8 => {
                    if self.serial_bytes < MAX_SERIAL {
                        crate::serial::byte(value as u8);
                        self.serial_bytes += 1;
                    }
                    if self.uart_ier & 2 != 0 {
                        self.uart_thre_pending = true;
                    }
                }
                0x3f9 => {
                    let newly_enabled = value & 2 != 0 && self.uart_ier & 2 == 0;
                    self.uart_ier = value as u8 & 0x0f;
                    if newly_enabled {
                        self.uart_thre_pending = true;
                    }
                    self.uart_irq_injected = false;
                }
                0x3fb => self.uart_lcr = value as u8,
                0x20 if value & 0x10 != 0 => self.pic_icw = 1,
                0x20 if value & 0x20 != 0 && self.tick_pending => {
                    self.tick_pending = false;
                    self.last_tick_tsc = rdtsc();
                }
                0x20 | 0xa0 => {}
                0x21 => match self.pic_icw {
                    1 => {
                        self.pic_base = (value as u8) & 0xf8;
                        self.pic_icw = 2;
                    }
                    2 => self.pic_icw = 3,
                    3 => self.pic_icw = 0,
                    _ => self.pic[0] = value as u8,
                },
                0xa1 => self.pic[1] = value as u8,
                0x40 | 0x42 => {
                    if self.pit_wr_hi {
                        self.pit_latch = (self.pit_latch & 0x00ff) | ((value as u16) << 8);
                        self.pit_start = rdtsc();
                        self.pit_rd_hi = false;
                    } else {
                        self.pit_latch = (self.pit_latch & 0xff00) | (value as u16 & 0xff);
                    }
                    self.pit_wr_hi = !self.pit_wr_hi;
                }
                0x43 => {
                    let access = (value >> 4) & 3;
                    if access == 0 {
                        self.pit_rd_hi = false;
                    } else {
                        if value >> 6 == 0 {
                            self.pit_mode = ((value >> 1) & 7) as u8;
                        }
                        self.pit_wr_hi = false;
                        if self.pit_latch == 0 {
                            self.pit_latch = 0xffff;
                        }
                    }
                }
                0x61 => self.port61 = value as u8,
                0x64 if value & 0xfe == 0xfe => self.reset = true,
                0x64 => self.i8042_command(value as u8),
                0x60 => self.i8042_data(value as u8),
                0x70 => self.cmos_index = value as u8 & 0x7f,
                0x71 => {}
                0xcf9 if value & 0x04 != 0 => self.reset = true,
                0x80 | 0xed | 0xeb => {}
                0xcf8 => self.pci_addr = value,
                0xcfc..=0xcff => self.pci_cfg_write(port, value, size),
                p if self.virtio_port_in_range(p) => {
                    self.virtio_reg_write(p as u32 - self.virtio_io_base(), value, size)
                }
                p if self.net_port_in_range(p) => {
                    self.net_reg_write(p as u32 - self.net_io_base(), value, size)
                }
                _ => {}
            }
        }

        fn pio_in(&mut self, port: u16, size: u8) -> u32 {
            match port {
                // Linux's own probe for which PCI config mechanism is
                // present (`pci_check_type1`) writes 0x8000_0000 here and
                // requires reading the same value back before it will even
                // attempt to scan a bus - without this the whole PCI
                // subsystem disables itself before device enumeration ever
                // starts.
                0xcf8 => self.pci_addr,
                0x3f8 | 0x3f9 if self.uart_lcr & 0x80 != 0 => 0,
                0x3f8 => self.uart_pop_rx() as u32,
                0x3f9 => self.uart_ier as u32,
                0x3fa => {
                    // Reading IIR is what acknowledges a THRE interrupt; an
                    // RX interrupt stays asserted until the byte is read.
                    self.uart_irq_injected = false;
                    if self.uart_ier & 1 != 0 && self.uart_rx_len > 0 {
                        0x04
                    } else if self.uart_ier & 2 != 0 && self.uart_thre_pending {
                        self.uart_thre_pending = false;
                        0x02
                    } else {
                        0x01
                    }
                }
                0x3fb => self.uart_lcr as u32,
                0x3fc => 0x03,
                0x3fd => 0x60 | (self.uart_rx_len > 0) as u32,
                0x3fe => 0xb0,
                0x3ff => 0,
                0x21 => self.pic[0] as u32,
                0xa1 => self.pic[1] as u32,
                0x40 | 0x42 => {
                    let counter = self.pit_counter();
                    let hi = self.pit_rd_hi;
                    self.pit_rd_hi = !hi;
                    if hi {
                        (counter >> 8) as u32
                    } else {
                        (counter & 0xff) as u32
                    }
                }
                0x61 => {
                    let mut value = (self.port61 as u32) & 0x0f;
                    value |= (rdtsc() >> 13) as u32 & 0x10;
                    if self.pit_expired() {
                        value |= 0x20;
                    }
                    value
                }
                // i8042 status: bit0 output buffer full, bit2 system flag.
                0x64 => self.i8042_status(),
                0x60 => {
                    self.kbd_irq_injected = false;
                    self.aux_irq_injected = false;
                    self.kbd_pop() as u32
                }
                0x71 => self.cmos_read(),
                0xcfc..=0xcff => self.pci_cfg_read(port, size),
                p if self.virtio_port_in_range(p) => {
                    self.virtio_reg_read(p as u32 - self.virtio_io_base(), size)
                }
                p if self.net_port_in_range(p) => {
                    self.net_reg_read(p as u32 - self.net_io_base(), size)
                }
                0x92 => 0x02,
                _ => 0xff,
            }
        }

        /// The one PCI function this hypervisor exposes to the guest: a
        /// legacy (pre-1.0) virtio-blk device at 00:01.0, I/O-mapped so it
        /// reuses the EXIT_IOIO trap above instead of needing a real MMIO/
        /// NPT-backed device model. `bar0_raw` mirrors real BAR hardware:
        /// masking whatever was last written by the region size naturally
        /// reproduces both normal operation and the standard size-probe
        /// protocol (writing all-1s and reading back the size mask), with
        /// no separate "probing" state needed.
        fn effective_bar0(&self) -> u32 {
            (self.bar0_raw & !(VIRTIO_IO_SIZE - 1)) | 1
        }

        fn virtio_io_base(&self) -> u32 {
            self.effective_bar0() & !1
        }

        fn virtio_port_in_range(&self, port: u16) -> bool {
            let base = self.virtio_io_base();
            let port = port as u32;
            port >= base && port < base + VIRTIO_IO_SIZE
        }

        /// `None` when CONFIG_ADDRESS targets nothing this hypervisor
        /// models, else `(function, register)` - function 0 is a stub
        /// PCI-to-host bridge at 00:00.0 (present purely so older-style PCI
        /// sanity checks that scan bus 0 for a bridge/VGA class code before
        /// trusting Type-1 config access find one), function 1 is the real
        /// virtio-blk device at 00:01.0.
        fn pci_cfg_target(&self) -> Option<(u8, u16)> {
            if self.pci_addr & 0x8000_0000 == 0 {
                return None;
            }
            let bus = (self.pci_addr >> 16) & 0xff;
            let device = (self.pci_addr >> 11) & 0x1f;
            let function = (self.pci_addr >> 8) & 0x7;
            if bus != 0 || function != 0 || device > 2 {
                return None;
            }
            Some((device as u8, (self.pci_addr & 0xfc) as u16))
        }

        fn pci_cfg_write(&mut self, port: u16, value: u32, size: u8) {
            let Some((device, reg)) = self.pci_cfg_target() else {
                return;
            };
            if device == 0 {
                return; // the host-bridge stub is entirely read-only
            }
            let lane = port - 0xcfc;
            for i in 0..size as u16 {
                let byte = (value >> (8 * i)) as u8;
                if device == 1 {
                    self.pci_cfg_write_byte(reg + lane + i, byte);
                } else {
                    self.net_pci_write_byte(reg + lane + i, byte);
                }
            }
        }

        fn pci_cfg_read(&mut self, port: u16, size: u8) -> u32 {
            let Some((device, reg)) = self.pci_cfg_target() else {
                return 0xffff_ffff;
            };
            let lane = port - 0xcfc;
            let mut result = 0u32;
            for i in 0..size as u16 {
                let byte = match device {
                    0 => Self::host_bridge_read_byte(reg + lane + i),
                    1 => self.pci_cfg_read_byte(reg + lane + i),
                    _ => self.net_pci_read_byte(reg + lane + i),
                };
                result |= (byte as u32) << (8 * i);
            }
            result
        }

        /// Class 0x0600 (bridge/host) at revision 0, no BARs, no capabilities.
        fn host_bridge_read_byte(offset: u16) -> u8 {
            match offset {
                0 => 0x86,
                1 => 0x80, // vendor id 0x8086
                10 => 0x00,
                11 => 0x06, // class = bridge, subclass = host
                14 => 0x00, // header type: single-function
                _ => 0,
            }
        }

        /// Type-0 PCI config header for vendor 0x1af4 (virtio), device
        /// 0x1001 (legacy convention for "block"); Linux's legacy
        /// virtio-pci driver actually keys the device type off the
        /// Subsystem ID (offset 0x2e), set to 2 = VIRTIO_ID_BLOCK below.
        fn pci_cfg_read_byte(&self, offset: u16) -> u8 {
            match offset {
                0 => 0xf4,
                1 => 0x1a,
                2 => 0x01,
                3 => 0x10,
                4 => self.pci_command as u8,
                5 => (self.pci_command >> 8) as u8,
                10 => 0x80, // subclass: other mass storage controller
                11 => 0x01, // class: mass storage controller
                16..=19 => (self.effective_bar0() >> (8 * (offset - 16))) as u8,
                44 => 0xf4,
                45 => 0x1a,
                46 => 0x02, // subsystem id = VIRTIO_ID_BLOCK
                60 => VIRTIO_IRQ_LINE,
                61 => 0x01, // interrupt pin = INTA#
                _ => 0,
            }
        }

        fn pci_cfg_write_byte(&mut self, offset: u16, value: u8) {
            match offset {
                4 => self.pci_command = (self.pci_command & 0xff00) | value as u16,
                5 => self.pci_command = (self.pci_command & 0x00ff) | ((value as u16) << 8),
                16 => self.bar0_raw = (self.bar0_raw & 0xffff_ff00) | value as u32,
                17 => self.bar0_raw = (self.bar0_raw & 0xffff_00ff) | ((value as u32) << 8),
                18 => self.bar0_raw = (self.bar0_raw & 0xff00_ffff) | ((value as u32) << 16),
                19 => self.bar0_raw = (self.bar0_raw & 0x00ff_ffff) | ((value as u32) << 24),
                _ => {}
            }
        }

        fn virtio_reg_write(&mut self, offset: u32, value: u32, size: u8) {
            let offset = offset as u16;
            for i in 0..size as u16 {
                self.virtio_reg_write_byte(offset + i, (value >> (8 * i)) as u8);
            }
        }

        fn virtio_reg_read(&mut self, offset: u32, size: u8) -> u32 {
            let offset = offset as u16;
            let mut result = 0u32;
            for i in 0..size as u16 {
                result |= (self.virtio_reg_read_byte(offset + i) as u32) << (8 * i);
            }
            result
        }

        /// Legacy virtio-pci register layout (no MSI-X capability, so the
        /// device-specific config starts right at offset 20): features,
        /// queue address/size/select/notify, status, ISR, then the
        /// virtio_blk_config capacity field. Host feature bits are all
        /// zero - the guest driver runs entirely on legacy defaults (one
        /// queue, no indirect descriptors, no optional config fields).
        fn virtio_reg_read_byte(&mut self, offset: u16) -> u8 {
            match offset {
                0..=3 => 0,
                4..=7 => (self.virtio_guest_features >> (8 * (offset - 4))) as u8,
                8..=11 => (self.virtio_queue_pfn >> (8 * (offset - 8))) as u8,
                12 => VIRTIO_QUEUE_SIZE as u8,
                13 => (VIRTIO_QUEUE_SIZE >> 8) as u8,
                14 => self.virtio_queue_select as u8,
                15 => (self.virtio_queue_select >> 8) as u8,
                18 => self.virtio_status,
                19 => {
                    let isr = self.virtio_isr;
                    self.virtio_isr = 0;
                    isr
                }
                20..=27 => {
                    let sectors = self.rootfs_bytes / 512;
                    (sectors >> (8 * (offset - 20))) as u8
                }
                _ => 0,
            }
        }

        fn virtio_reg_write_byte(&mut self, offset: u16, value: u8) {
            match offset {
                4 => {
                    self.virtio_guest_features =
                        (self.virtio_guest_features & 0xffff_ff00) | value as u32
                }
                5 => {
                    self.virtio_guest_features =
                        (self.virtio_guest_features & 0xffff_00ff) | ((value as u32) << 8)
                }
                6 => {
                    self.virtio_guest_features =
                        (self.virtio_guest_features & 0xff00_ffff) | ((value as u32) << 16)
                }
                7 => {
                    self.virtio_guest_features =
                        (self.virtio_guest_features & 0x00ff_ffff) | ((value as u32) << 24)
                }
                8 => self.virtio_queue_pfn = (self.virtio_queue_pfn & 0xffff_ff00) | value as u32,
                9 => {
                    self.virtio_queue_pfn =
                        (self.virtio_queue_pfn & 0xffff_00ff) | ((value as u32) << 8)
                }
                10 => {
                    self.virtio_queue_pfn =
                        (self.virtio_queue_pfn & 0xff00_ffff) | ((value as u32) << 16)
                }
                11 => {
                    self.virtio_queue_pfn =
                        (self.virtio_queue_pfn & 0x00ff_ffff) | ((value as u32) << 24)
                }
                14 => self.virtio_queue_select = (self.virtio_queue_select & 0xff00) | value as u16,
                15 => {
                    self.virtio_queue_select =
                        (self.virtio_queue_select & 0x00ff) | ((value as u16) << 8)
                }
                16 | 17 => self.virtio_process_queue(),
                18 => {
                    self.virtio_status = value;
                    if value == 0 {
                        self.virtio_queue_pfn = 0;
                        self.virtio_guest_features = 0;
                        self.virtio_isr = 0;
                        self.blk_drain();
                        self.virtio_last_avail = 0;
                        self.virtio_used_idx = 0;
                        self.virtio_irq_pending = false;
                    }
                }
                _ => {}
            }
        }

        fn net_io_base(&self) -> u32 {
            self.net_bar_raw & !(VIRTIO_IO_SIZE - 1) & !1
        }

        fn net_port_in_range(&self, port: u16) -> bool {
            let base = self.net_io_base();
            let port = port as u32;
            port >= base && port < base + VIRTIO_IO_SIZE
        }

        /// PCI header for the virtio-net function: vendor 0x1af4, legacy
        /// device 0x1000, subsystem id 1 (= VIRTIO_ID_NET), class network.
        fn net_pci_read_byte(&self, offset: u16) -> u8 {
            match offset {
                0 => 0xf4,
                1 => 0x1a,
                2 => 0x00,
                3 => 0x10,
                4 => self.net_pci_command as u8,
                5 => (self.net_pci_command >> 8) as u8,
                10 => 0x00, // subclass: ethernet
                11 => 0x02, // class: network controller
                16..=19 => {
                    (((self.net_bar_raw & !(VIRTIO_IO_SIZE - 1)) | 1) >> (8 * (offset - 16))) as u8
                }
                44 => 0xf4,
                45 => 0x1a,
                46 => 0x01, // subsystem id = VIRTIO_ID_NET
                60 => NET_IRQ_LINE,
                61 => 0x01,
                _ => 0,
            }
        }

        fn net_pci_write_byte(&mut self, offset: u16, value: u8) {
            match offset {
                4 => self.net_pci_command = (self.net_pci_command & 0xff00) | value as u16,
                5 => self.net_pci_command = (self.net_pci_command & 0x00ff) | ((value as u16) << 8),
                16..=19 => {
                    let shift = 8 * (offset - 16) as u32;
                    self.net_bar_raw =
                        (self.net_bar_raw & !(0xff << shift)) | ((value as u32) << shift);
                }
                _ => {}
            }
        }

        fn net_reg_write(&mut self, offset: u32, value: u32, size: u8) {
            for i in 0..size as u16 {
                self.net_reg_write_byte(offset as u16 + i, (value >> (8 * i)) as u8);
            }
        }

        fn net_reg_read(&mut self, offset: u32, size: u8) -> u32 {
            let mut result = 0u32;
            for i in 0..size as u16 {
                result |= (self.net_reg_read_byte(offset as u16 + i) as u32) << (8 * i);
            }
            result
        }

        fn net_reg_read_byte(&mut self, offset: u16) -> u8 {
            let queue = self.net_queue_select as usize;
            match offset {
                0..=3 => (NET_HOST_FEATURES >> (8 * offset)) as u8,
                4..=7 => (self.net_features >> (8 * (offset - 4))) as u8,
                8..=11 => {
                    let pfn = self.net_queue_pfn.get(queue).copied().unwrap_or(0);
                    (pfn >> (8 * (offset - 8))) as u8
                }
                12 if queue < 2 => NET_QUEUE_SIZE as u8,
                13 if queue < 2 => (NET_QUEUE_SIZE >> 8) as u8,
                14 => self.net_queue_select as u8,
                15 => (self.net_queue_select >> 8) as u8,
                18 => self.net_status,
                19 => {
                    let isr = self.net_isr;
                    self.net_isr = 0;
                    isr
                }
                20..=25 => self.net_mac[(offset - 20) as usize],
                26 => 1, // virtio_net_config.status: VIRTIO_NET_S_LINK_UP
                _ => 0,
            }
        }

        fn net_reg_write_byte(&mut self, offset: u16, value: u8) {
            let queue = self.net_queue_select as usize;
            match offset {
                4..=7 => {
                    let shift = 8 * (offset - 4) as u32;
                    self.net_features =
                        (self.net_features & !(0xff << shift)) | ((value as u32) << shift);
                }
                8..=11 if queue < 2 => {
                    let shift = 8 * (offset - 8) as u32;
                    self.net_queue_pfn[queue] =
                        (self.net_queue_pfn[queue] & !(0xff << shift)) | ((value as u32) << shift);
                }
                14 => self.net_queue_select = (self.net_queue_select & 0xff00) | value as u16,
                15 => {
                    self.net_queue_select = (self.net_queue_select & 0x00ff) | ((value as u16) << 8)
                }
                16 => match value {
                    0 => self.net_poll_rx(),
                    1 => self.net_process_tx(),
                    _ => {}
                },
                18 => {
                    self.net_status = value;
                    if value == 0 {
                        self.net_features = 0;
                        self.net_queue_pfn = [0; 2];
                        self.net_last_avail = [0; 2];
                        self.net_used_idx = [0; 2];
                        self.net_isr = 0;
                        self.net_irq_pending = false;
                    }
                }
                _ => {}
            }
        }

        /// (descriptor table, avail ring, used ring) guest addresses for
        /// net queue `queue`, once the driver has programmed it.
        fn net_ring(&self, queue: usize) -> Option<(u64, u64, u64)> {
            let pfn = *self.net_queue_pfn.get(queue)?;
            if pfn == 0 {
                return None;
            }
            let num = NET_QUEUE_SIZE as u64;
            let desc = (pfn as u64) << 12;
            let avail = desc + 16 * num;
            let used = align_up(avail + 2 * (3 + num), PAGE_SIZE);
            (used + 6 + 8 * num <= self.ram_bytes).then_some((desc, avail, used))
        }

        fn net_complete(&mut self, queue: usize, used: u64, head: u64, written: u32) {
            let slot = (self.net_used_idx[queue] % NET_QUEUE_SIZE) as u64;
            let elem = used + 4 + 8 * slot;
            self.gwrite_u32(elem, head as u32);
            self.gwrite_u32(elem + 4, written);
            self.net_used_idx[queue] = self.net_used_idx[queue].wrapping_add(1);
            self.gwrite_u16(used + 2, self.net_used_idx[queue]);
            self.net_last_avail[queue] = self.net_last_avail[queue].wrapping_add(1);
        }

        /// Guest -> wire: gathers each posted chain (10-byte virtio_net_hdr
        /// first, then the frame) and hands the frame to the host NIC.
        fn net_process_tx(&mut self) {
            let Some((desc, avail, used)) = self.net_ring(1) else {
                return;
            };
            let num = NET_QUEUE_SIZE as u64;
            let avail_idx = self.gread_u16(avail + 2);
            let mut processed = 0u64;
            while self.net_last_avail[1] != avail_idx && processed < num {
                processed += 1;
                let slot = (self.net_last_avail[1] % NET_QUEUE_SIZE) as u64;
                let head = self.gread_u16(avail + 4 + 2 * slot) as u64;
                let mut frame = [0u8; 1600];
                let mut len = 0usize;
                let mut skipped = 0usize;
                let mut cur = head;
                let mut guard = 0u64;
                loop {
                    guard += 1;
                    if guard > num + 4 || cur >= num {
                        break;
                    }
                    let entry = desc + 16 * cur;
                    let addr = self.gread_u64(entry);
                    let dlen = self.gread_u32(entry + 8) as u64;
                    let flags = self.gread_u16(entry + 12);
                    let next = self.gread_u16(entry + 14);
                    if addr.saturating_add(dlen) <= self.ram_bytes {
                        let mut offset = 0usize;
                        let mut remaining = dlen as usize;
                        if skipped < NET_HDR_LEN {
                            let skip = (NET_HDR_LEN - skipped).min(remaining);
                            skipped += skip;
                            offset = skip;
                            remaining -= skip;
                        }
                        let copy = remaining.min(frame.len() - len);
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                (self.ram + addr + offset as u64) as *const u8,
                                frame.as_mut_ptr().add(len),
                                copy,
                            );
                        }
                        len += copy;
                    }
                    if flags & 1 == 0 {
                        break;
                    }
                    cur = next as u64;
                }
                if len >= 14 && crate::nic::transmit(&frame[..len]) {
                    self.net_tx_frames += 1;
                } else {
                    self.net_tx_dropped += 1;
                }
                self.net_complete(1, used, head, 0);
            }
            if processed > 0 {
                self.net_isr |= 1;
                self.net_irq_pending = true;
            }
        }

        /// Wire -> guest: while the guest has receive buffers posted, pulls
        /// frames the NIC has accepted and copies them (behind a zeroed
        /// virtio_net_hdr) into those buffers.
        fn net_poll_rx(&mut self) {
            if self.net_status & 4 == 0 {
                return;
            }
            let Some((desc, avail, used)) = self.net_ring(0) else {
                return;
            };
            let num = NET_QUEUE_SIZE as u64;
            let mut delivered = false;
            for _ in 0..8 {
                if self.net_last_avail[0] == self.gread_u16(avail + 2) {
                    break; // no receive buffer posted: leave the frame in the NIC
                }
                let mut frame = [0u8; 1600];
                let Some(len) = crate::nic::try_receive(&mut frame) else {
                    break;
                };
                let slot = (self.net_last_avail[0] % NET_QUEUE_SIZE) as u64;
                let head = self.gread_u16(avail + 4 + 2 * slot) as u64;
                let total = NET_HDR_LEN + len;
                let mut written = 0usize;
                let mut cur = head;
                let mut guard = 0u64;
                loop {
                    guard += 1;
                    if guard > num + 4 || cur >= num || written >= total {
                        break;
                    }
                    let entry = desc + 16 * cur;
                    let addr = self.gread_u64(entry);
                    let dlen = self.gread_u32(entry + 8) as usize;
                    let flags = self.gread_u16(entry + 12);
                    let next = self.gread_u16(entry + 14);
                    let count = dlen.min(total - written);
                    if addr.saturating_add(count as u64) <= self.ram_bytes {
                        for i in 0..count {
                            let index = written + i;
                            let byte = if index < NET_HDR_LEN {
                                0
                            } else {
                                frame[index - NET_HDR_LEN]
                            };
                            self.gwrite_u8(addr + i as u64, byte);
                        }
                        written += count;
                    }
                    if flags & 1 == 0 {
                        break;
                    }
                    cur = next as u64;
                }
                self.net_complete(0, used, head, written as u32);
                self.net_rx_frames += 1;
                delivered = true;
            }
            if delivered {
                self.net_isr |= 1;
                self.net_irq_pending = true;
            }
        }

        fn gread_u16(&self, gpa: u64) -> u16 {
            read_u16(self.ram, gpa)
        }

        fn gread_u32(&self, gpa: u64) -> u32 {
            read_u32(self.ram, gpa)
        }

        fn gread_u64(&self, gpa: u64) -> u64 {
            unsafe { read_u64(self.ram, gpa) }
        }

        fn gwrite_u8(&mut self, gpa: u64, value: u8) {
            unsafe { write_u8(self.ram, gpa, value) };
        }

        fn gwrite_u16(&mut self, gpa: u64, value: u16) {
            unsafe { write_u16(self.ram, gpa, value) };
        }

        fn gwrite_u32(&mut self, gpa: u64, value: u32) {
            unsafe { write_u32(self.ram, gpa, value) };
        }

        /// Walks the avail ring for however many new requests are pending
        /// and services each synchronously (this whole hypervisor is
        /// single-threaded around `vmentry`, so there is no concurrent
        /// guest execution to race against while a request is handled).
        fn virtio_process_queue(&mut self) {
            if self.virtio_queue_pfn == 0 || self.virtio_queue_select != 0 {
                return;
            }
            let num = VIRTIO_QUEUE_SIZE as u64;
            let desc_addr = (self.virtio_queue_pfn as u64) << 12;
            let avail_addr = desc_addr + 16 * num;
            let used_addr = align_up(avail_addr + 2 * (3 + num), PAGE_SIZE);
            if desc_addr >= self.ram_bytes || used_addr + 6 + 8 * num > self.ram_bytes {
                return;
            }
            let asynchronous = self.rootfs_disk != usize::MAX && crate::smp::online_mask() & 2 != 0;
            if asynchronous {
                BLK_DISK.store(self.rootfs_disk, core::sync::atomic::Ordering::Release);
            }

            let avail_idx = self.gread_u16(avail_addr + 2);
            let mut processed = 0u64;
            while self.virtio_last_avail != avail_idx && processed < num {
                let ring_slot = (self.virtio_last_avail % VIRTIO_QUEUE_SIZE) as u64;
                let desc_head = self.gread_u16(avail_addr + 4 + 2 * ring_slot);
                if asynchronous {
                    if !self.blk_submit(desc_addr, desc_head, num) {
                        break; // ring full (can't happen: same size as the queue)
                    }
                    self.virtio_last_avail = self.virtio_last_avail.wrapping_add(1);
                    continue;
                }
                processed += 1;
                let written = self.virtio_handle_request(desc_addr, desc_head as u64, num);
                self.virtio_complete(desc_head, written);
                self.virtio_last_avail = self.virtio_last_avail.wrapping_add(1);
            }
            if processed > 0 {
                self.virtio_isr |= 1;
                self.virtio_irq_pending = true;
            }
        }

        /// Publishes one finished request in the used ring.
        fn virtio_complete(&mut self, head: u16, written: u32) {
            let num = VIRTIO_QUEUE_SIZE as u64;
            let desc_addr = (self.virtio_queue_pfn as u64) << 12;
            let avail_addr = desc_addr + 16 * num;
            let used_addr = align_up(avail_addr + 2 * (3 + num), PAGE_SIZE);
            let used_slot = (self.virtio_used_idx % VIRTIO_QUEUE_SIZE) as u64;
            let elem = used_addr + 4 + 8 * used_slot;
            self.gwrite_u32(elem, head as u32);
            self.gwrite_u32(elem + 4, written);
            self.virtio_used_idx = self.virtio_used_idx.wrapping_add(1);
            self.gwrite_u16(used_addr + 2, self.virtio_used_idx);
        }

        /// Follows one request's descriptor chain: a 16-byte
        /// `virtio_blk_outhdr` (type, reserved, sector), one data buffer, then
        /// a 1-byte device-writable status descriptor. None when the chain is
        /// malformed.
        fn virtio_parse_request(
            &mut self,
            desc_table: u64,
            head: u64,
            num: u64,
        ) -> Option<(u32, u64, u64, u32, bool, u64)> {
            let mut cur = head;
            let mut first = true;
            let mut data_addr = 0u64;
            let mut data_len = 0u32;
            let mut data_write = false;
            let mut status_addr = 0u64;
            let mut req_type = u32::MAX;
            let mut sector = 0u64;
            let mut guard = 0u64;
            loop {
                guard += 1;
                if guard > num + 4 || cur >= num {
                    return None;
                }
                let entry = desc_table + 16 * cur;
                let addr = self.gread_u64(entry);
                let len = self.gread_u32(entry + 8);
                let flags = self.gread_u16(entry + 12);
                let next = self.gread_u16(entry + 14);
                let has_next = flags & 1 != 0;
                let is_write = flags & 2 != 0;
                if first {
                    first = false;
                    if len < 16 {
                        return None;
                    }
                    req_type = self.gread_u32(addr);
                    sector = self.gread_u64(addr + 8);
                } else if has_next {
                    data_addr = addr;
                    data_len = len;
                    data_write = is_write;
                } else {
                    status_addr = addr;
                }
                if !has_next {
                    break;
                }
                cur = next as u64;
            }
            Some((
                req_type,
                sector,
                data_addr,
                data_len,
                data_write,
                status_addr,
            ))
        }

        /// Only VIRTIO_BLK_T_IN (read) is implemented - the backing root is
        /// mounted `ro`, so write requests are never expected.
        fn virtio_handle_request(&mut self, desc_table: u64, head: u64, num: u64) -> u32 {
            self.blk_requests += 1;
            let Some((req_type, sector, data_addr, data_len, data_write, status_addr)) =
                self.virtio_parse_request(desc_table, head, num)
            else {
                self.blk_errors += 1;
                return 0;
            };
            let ok = if req_type == VIRTIO_BLK_T_OUT {
                !data_write && data_len > 0 && self.blk_write_sectors(sector, data_addr, data_len)
            } else {
                req_type == VIRTIO_BLK_T_IN
                    && data_write
                    && data_len > 0
                    && self.blk_read_sectors(sector, data_addr, data_len)
            };
            if !ok {
                self.blk_errors += 1;
            }
            if status_addr != 0 {
                self.gwrite_u8(
                    status_addr,
                    if ok {
                        VIRTIO_BLK_S_OK
                    } else {
                        VIRTIO_BLK_S_IOERR
                    },
                );
            }
            if ok { data_len + 1 } else { 1 }
        }

        /// Queues a request for the worker CPU.
        fn blk_submit(&mut self, desc_table: u64, head: u16, num: u64) -> bool {
            use core::sync::atomic::Ordering::{Relaxed, Release};
            let produced = BLK_PRODUCED.load(Relaxed);
            if produced.wrapping_sub(self.blk_finalized) >= BLK_SLOTS as u32 {
                return false;
            }
            if produced == 0 {
                crate::serial::line("AEROS_VM_BLK async=true worker_cpu=1");
            }
            self.blk_requests += 1;
            let parsed = self.virtio_parse_request(desc_table, head as u64, num);
            let (job, meta) = match parsed {
                Some((req_type, sector, data_addr, data_len, data_write, status_addr)) => {
                    let dest_phys = self.ram + data_addr;
                    let in_range = data_len != 0
                        && (data_len as u64).is_multiple_of(512)
                        && sector
                            .checked_mul(512)
                            .and_then(|start| start.checked_add(data_len as u64))
                            .is_some_and(|end| end <= self.rootfs_bytes)
                        && data_addr
                            .checked_add(data_len as u64)
                            .is_some_and(|end| end <= self.ram_bytes)
                        && dest_phys.is_multiple_of(512);
                    let write = req_type == VIRTIO_BLK_T_OUT;
                    let allowed = if write {
                        !data_write && sector.saturating_mul(512) >= DATA_START.load(Relaxed)
                    } else {
                        req_type == VIRTIO_BLK_T_IN && data_write
                    };
                    (
                        BlkJob {
                            sector,
                            dest_phys,
                            len: data_len,
                            valid: allowed && in_range,
                            write,
                        },
                        BlkMeta {
                            head,
                            status_addr,
                            data_len,
                            write,
                        },
                    )
                }
                None => (
                    BlkJob {
                        sector: 0,
                        dest_phys: 0,
                        len: 0,
                        valid: false,
                        write: false,
                    },
                    BlkMeta {
                        head,
                        status_addr: 0,
                        data_len: 0,
                        write: false,
                    },
                ),
            };
            let index = produced as usize % BLK_SLOTS;
            // SAFETY: the worker only reads slots below BLK_PRODUCED.
            unsafe { (*BLK_RING.0.get())[index] = job };
            self.blk_meta[index] = meta;
            BLK_PRODUCED.store(produced.wrapping_add(1), Release);
            self.blk_kick();
            true
        }

        /// Makes sure the worker CPU is running when requests are waiting.
        fn blk_kick(&mut self) {
            use core::sync::atomic::Ordering::SeqCst;
            if BLK_ACTIVE.load(SeqCst) {
                return; // the running worker will see the new request
            }
            crate::smp::reap_job(1);
            if BLK_ACTIVE
                .compare_exchange(false, true, SeqCst, SeqCst)
                .is_ok()
                && !crate::smp::start_job(1, blk_worker, 0)
            {
                // The CPU is still winding down its previous job; the next
                // poll tries again.
                BLK_ACTIVE.store(false, SeqCst);
            }
        }

        /// Completes finished asynchronous reads towards the guest, in order,
        /// and restarts the worker if a request is waiting.
        fn blk_poll(&mut self) {
            use core::sync::atomic::Ordering::{Acquire, Relaxed, SeqCst};
            let done = BLK_DONE.load(Acquire);
            if BLK_PRODUCED.load(Relaxed) != done && !BLK_ACTIVE.load(SeqCst) {
                self.blk_kick();
            }
            let mut completed = false;
            while self.blk_finalized != done {
                let index = self.blk_finalized as usize % BLK_SLOTS;
                let meta = self.blk_meta[index];
                let ok = BLK_OK[index].load(Acquire);
                if !ok {
                    self.blk_errors += 1;
                }
                if meta.status_addr != 0 {
                    self.gwrite_u8(
                        meta.status_addr,
                        if ok {
                            VIRTIO_BLK_S_OK
                        } else {
                            VIRTIO_BLK_S_IOERR
                        },
                    );
                }
                let written = if ok {
                    if meta.write { 1 } else { meta.data_len + 1 }
                } else if meta.status_addr != 0 {
                    1
                } else {
                    0
                };
                self.virtio_complete(meta.head, written);
                self.blk_finalized = self.blk_finalized.wrapping_add(1);
                completed = true;
            }
            if completed {
                self.virtio_isr |= 1;
                self.virtio_irq_pending = true;
            }
        }

        /// Waits for every queued read to finish and discards the results (the
        /// guest reset the device).
        fn blk_drain(&mut self) {
            use core::sync::atomic::Ordering::SeqCst;
            let produced = BLK_PRODUCED.load(SeqCst);
            let mut spins = 0u64;
            while BLK_DONE.load(SeqCst) != produced && spins < 4_000_000_000 {
                self.blk_kick();
                core::hint::spin_loop();
                spins += 1;
            }
            self.blk_finalized = produced;
        }

        /// Services one read directly from the ROOTFS file on the host ESP
        /// via the same FAT+AHCI path used to load VMLINUZ/INITRD - the
        /// backing image is never staged wholesale into guest RAM.
        fn blk_read_sectors(&mut self, sector: u64, dest_gpa: u64, len: u32) -> bool {
            if len == 0 || !(len as u64).is_multiple_of(512) {
                return false;
            }
            let Some(skip) = sector.checked_mul(512) else {
                return false;
            };
            let Some(end) = skip.checked_add(len as u64) else {
                return false;
            };
            let Some(gpa_end) = dest_gpa.checked_add(len as u64) else {
                return false;
            };
            if end > self.rootfs_bytes || gpa_end > self.ram_bytes {
                return false;
            }
            let dest_phys = self.ram + dest_gpa;
            if !dest_phys.is_multiple_of(512) {
                return false;
            }
            if self.rootfs_disk != usize::MAX {
                let mut done = 0u32;
                let total = len / 512;
                while done < total {
                    let batch = (total - done).min(8192);
                    if !crate::ahci::read_disk(
                        self.rootfs_disk,
                        sector + done as u64,
                        batch,
                        dest_phys + done as u64 * 512,
                    ) {
                        return false;
                    }
                    done += batch;
                }
                return true;
            }
            fat::load_root_file(ROOTFS_NAME, skip, dest_phys, len as u64) == Some(len as u64)
        }

        /// A write into the disk's data area (never the read-only root).
        fn blk_write_sectors(&mut self, sector: u64, src_gpa: u64, len: u32) -> bool {
            use core::sync::atomic::Ordering::Relaxed;
            if self.rootfs_disk == usize::MAX
                || len == 0
                || !(len as u64).is_multiple_of(512)
                || sector.saturating_mul(512) < DATA_START.load(Relaxed)
            {
                return false;
            }
            let Some(end) = sector
                .checked_mul(512)
                .and_then(|start| start.checked_add(len as u64))
            else {
                return false;
            };
            let Some(gpa_end) = src_gpa.checked_add(len as u64) else {
                return false;
            };
            if end > self.rootfs_bytes || gpa_end > self.ram_bytes {
                return false;
            }
            let src_phys = self.ram + src_gpa;
            if !src_phys.is_multiple_of(512) {
                return false;
            }
            let total = len / 512;
            let mut done = 0u32;
            while done < total {
                let batch = (total - done).min(8192);
                if !crate::ahci::write_disk(
                    self.rootfs_disk,
                    sector + done as u64,
                    batch,
                    src_phys + done as u64 * 512,
                ) {
                    return false;
                }
                done += batch;
            }
            true
        }

        fn pit_ticks(&self) -> u64 {
            rdtsc().wrapping_sub(self.pit_start) / TSC_PER_PIT
        }

        fn pit_counter(&self) -> u16 {
            let latch = if self.pit_latch == 0 {
                0x1_0000u64
            } else {
                self.pit_latch as u64
            };
            let remaining = latch - 1 - (self.pit_ticks() % latch);
            remaining as u16
        }

        fn pit_expired(&self) -> bool {
            let latch = if self.pit_latch == 0 {
                0x1_0000u64
            } else {
                self.pit_latch as u64
            };
            self.pit_ticks() >= latch
        }

        fn cmos_read(&self) -> u32 {
            let value = match self.cmos_index {
                0x00 => 0x00,
                0x02 => 0x00,
                0x04 => 0x09,
                0x06 => 0x01,
                0x07 => 0x06,
                0x08 => 0x09,
                0x09 => 0x26,
                0x0a => 0x26,
                0x0b => 0x02,
                0x0c => 0x00,
                0x0d => 0x80,
                0x32 => 0x20,
                0x50 => 0x20,
                _ => 0x00,
            };
            value as u32
        }
    }

    fn copy_guest(ram: u64, src_off: u64, dst_off: u64, len: u64) {
        let mut offset = 0u64;
        while offset + 8 <= len {
            let word = unsafe { read_u64(ram, src_off + offset) };
            unsafe { write_u64(ram, dst_off + offset, word) };
            offset += 8;
        }
        while offset < len {
            let byte = read_u8(ram, src_off + offset);
            unsafe { write_u8(ram, dst_off + offset, byte) };
            offset += 1;
        }
    }

    fn align_up(value: u64, align: u64) -> u64 {
        (value + align - 1) & !(align - 1)
    }

    fn rdtsc() -> u64 {
        let lo: u32;
        let hi: u32;
        unsafe {
            core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
        }
        (hi as u64) << 32 | lo as u64
    }
}

#[derive(Clone, Copy, Default)]
struct Uart {
    dll: u8,
    dlm: u8,
    ier: u8,
    fcr: u8,
    lcr: u8,
    mcr: u8,
    scr: u8,
    rx: u8,
    rx_ready: bool,
}

impl Uart {
    fn dlab(&self) -> bool {
        self.lcr & 0x80 != 0
    }

    fn loopback(&self) -> bool {
        self.mcr & 0x10 != 0
    }

    fn read(&mut self, reg: u16) -> u8 {
        match reg {
            0 if self.dlab() => self.dll,
            0 => {
                self.rx_ready = false;
                core::mem::take(&mut self.rx)
            }
            1 if self.dlab() => self.dlm,
            1 => self.ier,
            2 => 0x01 | if self.fcr & 1 != 0 { 0xc0 } else { 0 },
            3 => self.lcr,
            4 => self.mcr,
            5 => 0x60 | if self.rx_ready { 1 } else { 0 },
            6 if self.loopback() => {
                ((self.mcr & 0x02) << 3)
                    | ((self.mcr & 0x01) << 5)
                    | ((self.mcr & 0x04) << 4)
                    | ((self.mcr & 0x08) << 4)
            }
            6 => 0xb0,
            7 => self.scr,
            _ => 0xff,
        }
    }
}

#[repr(C, align(16))]
struct Vm {
    gpr: [u64; 14],
    vmcb: u64,
    host_vmcb: u64,
    hsave: u64,
    guest_ram: u64,
    mode: GuestMode,
    exits: u32,
    io_writes: u32,
    cpuid_exits: u32,
    msr_exits: u32,
    last_exit_code: u64,
    halted: bool,
    uart: Uart,
    console: [u8; GUEST_LOG_MAX],
    console_len: usize,
}

impl Vm {
    fn new(frames: &mut FrameAllocator, mode: GuestMode) -> Option<Self> {
        let hsave = frames.allocate_contiguous(1, 1)?.address();
        let host_vmcb = frames.allocate_contiguous(1, 1)?.address();
        let vmcb = frames.allocate_contiguous(1, 1)?.address();
        let npt = frames.allocate_contiguous(NPT_PAGES, 1)?.address();
        let iopm = frames.allocate_contiguous(IOPM_PAGES, 1)?.address();
        let msrpm = frames.allocate_contiguous(MSRPM_PAGES, 1)?.address();
        let guest_ram = frames.allocate_contiguous(GUEST_RAM_PAGES, 512)?.address();

        zero_region(hsave, PAGE_SIZE);
        zero_region(host_vmcb, PAGE_SIZE);
        zero_region(vmcb, PAGE_SIZE);
        zero_region(npt, NPT_PAGES * PAGE_SIZE);
        zero_region(guest_ram, GUEST_RAM_PAGES * PAGE_SIZE);
        fill_region(iopm, IOPM_PAGES * PAGE_SIZE, 0xff);
        fill_region(msrpm, MSRPM_PAGES * PAGE_SIZE, 0xff);

        build_npt(npt, guest_ram, GUEST_RAM_PAGES * PAGE_SIZE);
        if mode == GuestMode::Long {
            build_guest_paging(guest_ram, GUEST_RAM_PAGES * PAGE_SIZE);
        }
        build_vmcb(vmcb, npt, iopm, msrpm, mode);

        Some(Self {
            gpr: [0; 14],
            vmcb,
            host_vmcb,
            hsave,
            guest_ram,
            mode,
            exits: 0,
            io_writes: 0,
            cpuid_exits: 0,
            msr_exits: 0,
            last_exit_code: 0,
            halted: false,
            uart: Uart::default(),
            console: [0; GUEST_LOG_MAX],
            console_len: 0,
        })
    }

    fn load_guest(&mut self) {
        match self.mode {
            GuestMode::Real => self.load_real_guest(),
            GuestMode::Long => self.load_long_guest(),
        }
    }

    fn load_linux(&mut self, image: &[u8]) -> Option<usize> {
        if image.len() < 0x268 || image[0x1fe] != 0x55 || image[0x1ff] != 0xaa {
            return None;
        }
        if image[0x202..0x206] != *b"HdrS" {
            return None;
        }
        let setup_sects = if image[0x1f1] == 0 {
            4usize
        } else {
            image[0x1f1] as usize
        };
        let pm_offset = (setup_sects + 1) * 512;
        if pm_offset >= image.len() {
            return None;
        }
        let ram = self.guest_ram;
        for (index, byte) in image[pm_offset..].iter().enumerate() {
            unsafe { write_u8(ram, LINUX_LOAD + index as u64, *byte) };
        }
        zero_region(ram + LINUX_BOOT_PARAMS, PAGE_SIZE);
        for (index, byte) in image.iter().enumerate().take(0x268).skip(0x1f1) {
            unsafe { write_u8(ram, LINUX_BOOT_PARAMS + index as u64, *byte) };
        }
        unsafe {
            write_u8(ram, LINUX_BOOT_PARAMS + 0x210, 0xff);
            write_u8(ram, LINUX_BOOT_PARAMS + 0x211, image[0x211] | 0x81);
            write_u32(ram, LINUX_BOOT_PARAMS + 0x228, LINUX_CMDLINE as u32);
            write_u8(ram, LINUX_BOOT_PARAMS + 0x1e8, 1);
            let entry = ram + LINUX_BOOT_PARAMS + 0x2d0;
            write_u64(entry, 0, 0);
            write_u64(entry, 8, GUEST_RAM_PAGES * PAGE_SIZE);
            write_u32(entry, 16, 1);
        }
        for (index, byte) in LINUX_CMDLINE_TEXT.iter().enumerate() {
            unsafe { write_u8(ram, LINUX_CMDLINE + index as u64, *byte) };
        }
        build_guest_paging(ram, GUEST_RAM_PAGES * PAGE_SIZE);
        unsafe {
            write_u64(self.vmcb, 0x578, LINUX_LOAD + 0x200);
            write_u64(self.vmcb, 0x5d8, LINUX_STACK);
        }
        self.gpr[3] = LINUX_BOOT_PARAMS;
        Some(pm_offset)
    }

    fn load_long_guest(&mut self) {
        let mk = GUEST_MARKER;
        let high = GUEST_HIGH_VALUE;
        let hi_addr = GUEST_HIGH_ADDR as u32;
        let mut code = [0u8; 128];
        let written = {
            let mut n = 0usize;
            let mut put = |bytes: &[u8]| {
                for byte in bytes {
                    code[n] = *byte;
                    n += 1;
                }
            };
            put(&[0xba, 0xf8, 0x03, 0x00, 0x00]);
            for byte in GUEST_MESSAGE {
                put(&[0xb0, *byte, 0xee]);
            }
            put(&[0x48, 0xc7, 0xc0, 0x00, 0x90, 0x00, 0x00]);
            put(&[
                0xc7,
                0x00,
                mk as u8,
                (mk >> 8) as u8,
                (mk >> 16) as u8,
                (mk >> 24) as u8,
            ]);
            put(&[
                0x48,
                0xc7,
                0xc3,
                hi_addr as u8,
                (hi_addr >> 8) as u8,
                (hi_addr >> 16) as u8,
                (hi_addr >> 24) as u8,
            ]);
            put(&[
                0xc7,
                0x03,
                high as u8,
                (high >> 8) as u8,
                (high >> 16) as u8,
                (high >> 24) as u8,
            ]);
            put(&[0x8b, 0x0b]);
            put(&[0x48, 0xc7, 0xc0, 0x04, 0x90, 0x00, 0x00]);
            put(&[0x89, 0x08]);
            put(&[0xb8, 0x01, 0x00, 0x00, 0x00]);
            put(&[0x0f, 0xa2]);
            put(&[0xb9, 0x80, 0x00, 0x00, 0xc0]);
            put(&[0x0f, 0x32]);
            put(&[0xf4]);
            n
        };
        for (offset, byte) in code.iter().take(written).enumerate() {
            unsafe {
                core::ptr::write_volatile(
                    (self.guest_ram + LONG_ENTRY + offset as u64) as usize as *mut u8,
                    *byte,
                );
            }
        }
    }

    fn load_real_guest(&mut self) {
        let marker_lo = GUEST_MARKER_ADDR as u16;
        let marker_hi = marker_lo + 2;
        let mk = GUEST_MARKER;
        let mut code = [0u8; 128];
        let written = {
            let mut n = 0usize;
            let mut put = |bytes: &[u8]| {
                for byte in bytes {
                    code[n] = *byte;
                    n += 1;
                }
            };
            put(&[0xba, 0xf8, 0x03]);
            for byte in GUEST_MESSAGE {
                put(&[0xb0, *byte, 0xee]);
            }
            put(&[
                0xc7,
                0x06,
                marker_lo as u8,
                (marker_lo >> 8) as u8,
                mk as u8,
                (mk >> 8) as u8,
            ]);
            put(&[
                0xc7,
                0x06,
                marker_hi as u8,
                (marker_hi >> 8) as u8,
                (mk >> 16) as u8,
                (mk >> 24) as u8,
            ]);
            let high = GUEST_HIGH_VALUE;
            let addr = GUEST_HIGH_ADDR as u32;
            let mark = GUEST_HIGH_MARKER_ADDR as u16;
            put(&[
                0x66,
                0xb8,
                high as u8,
                (high >> 8) as u8,
                (high >> 16) as u8,
                (high >> 24) as u8,
            ]);
            put(&[
                0x67,
                0x66,
                0xa3,
                addr as u8,
                (addr >> 8) as u8,
                (addr >> 16) as u8,
                (addr >> 24) as u8,
            ]);
            put(&[
                0x67,
                0x66,
                0xa1,
                addr as u8,
                (addr >> 8) as u8,
                (addr >> 16) as u8,
                (addr >> 24) as u8,
            ]);
            put(&[0x66, 0xa3, mark as u8, (mark >> 8) as u8]);
            put(&[0x66, 0xb8, 0x01, 0x00, 0x00, 0x00]);
            put(&[0x0f, 0xa2]);
            put(&[0x66, 0xb9, 0x80, 0x00, 0x00, 0xc0]);
            put(&[0x0f, 0x32]);
            put(&[0xf4]);
            n
        };
        for (offset, byte) in code.iter().take(written).enumerate() {
            unsafe {
                core::ptr::write_volatile(
                    (self.guest_ram + GUEST_ENTRY + offset as u64) as usize as *mut u8,
                    *byte,
                );
            }
        }
    }

    fn run(&mut self) {
        while self.exits < MAX_EXITS {
            let ctx = self.gpr.as_mut_ptr();
            unsafe { vmentry(self.vmcb, self.host_vmcb, ctx) };
            self.exits += 1;
            let exit_code = unsafe { read_u64(self.vmcb, 0x070) };
            self.last_exit_code = exit_code;
            match exit_code {
                EXIT_HLT | EXIT_VMMCALL => {
                    self.halted = true;
                    return;
                }
                EXIT_IOIO => {
                    if !self.handle_io() {
                        return;
                    }
                }
                EXIT_CPUID => {
                    self.cpuid_exits += 1;
                    self.skip_instruction(2);
                }
                EXIT_MSR => {
                    self.msr_exits += 1;
                    self.skip_instruction(2);
                }
                EXIT_NPF => {
                    let fault = unsafe { read_u64(self.vmcb, 0x080) };
                    crate::serial::format(format_args!(
                        "AEROS_VM_NPF guest_physical={:#x}\n",
                        fault
                    ));
                    return;
                }
                _ => return,
            }
        }
    }

    fn handle_io(&mut self) -> bool {
        let info1 = unsafe { read_u64(self.vmcb, 0x078) };
        let next_rip = unsafe { read_u64(self.vmcb, 0x080) };
        let is_in = info1 & 1 != 0;
        let port = ((info1 >> 16) & 0xffff) as u16;
        let rax = unsafe { read_u64(self.vmcb, 0x5f8) };
        if is_in {
            let value = self.pio_in(port);
            unsafe { write_u64(self.vmcb, 0x5f8, (rax & !0xff) | value as u64) };
        } else {
            self.pio_out(port, rax as u8);
        }
        let guest_rip = next_rip.saturating_sub(self.guest_cs_base());
        unsafe { write_u64(self.vmcb, 0x578, guest_rip) };
        true
    }

    fn pio_in(&mut self, port: u16) -> u8 {
        match port {
            COM1..=COM1_END => self.uart.read(port - COM1),
            _ => 0xff,
        }
    }

    fn pio_out(&mut self, port: u16, value: u8) {
        if let COM1..=COM1_END = port {
            self.uart_write(port - COM1, value);
        }
    }

    fn uart_write(&mut self, reg: u16, value: u8) {
        match reg {
            0 if self.uart.dlab() => self.uart.dll = value,
            0 if self.uart.loopback() => {
                self.uart.rx = value;
                self.uart.rx_ready = true;
            }
            0 => {
                self.io_writes += 1;
                if self.console_len < GUEST_LOG_MAX {
                    self.console[self.console_len] = value;
                    self.console_len += 1;
                }
            }
            1 if self.uart.dlab() => self.uart.dlm = value,
            1 => self.uart.ier = value,
            2 => self.uart.fcr = value,
            3 => self.uart.lcr = value,
            4 => self.uart.mcr = value,
            7 => self.uart.scr = value,
            _ => {}
        }
    }

    fn guest_cs_base(&self) -> u64 {
        unsafe { read_u64(self.vmcb, 0x410 + 8) }
    }

    fn skip_instruction(&mut self, length: u64) {
        let rip = unsafe { read_u64(self.vmcb, 0x578) };
        unsafe { write_u64(self.vmcb, 0x578, rip + length) };
    }
}

fn build_npt(npt: u64, guest_ram: u64, bytes: u64) {
    let pdpt = npt + PAGE_SIZE;
    let pd = pdpt + PAGE_SIZE;
    let flags = 0b111u64;
    let entries = bytes.div_ceil(PAGE_2M).min(512);
    unsafe {
        write_u64(npt, 0, pdpt | flags);
        write_u64(pdpt, 0, pd | flags);
        for index in 0..entries {
            write_u64(
                pd,
                index * 8,
                (guest_ram + index * PAGE_2M) | flags | (1 << 7),
            );
        }
    }
}

fn build_guest_paging(guest_ram: u64, bytes: u64) {
    let flags = 0b11u64;
    let entries = bytes.div_ceil(PAGE_2M).min(512);
    unsafe {
        write_u64(guest_ram + LONG_PML4, 0, LONG_PDPT | flags);
        write_u64(guest_ram + LONG_PDPT, 0, LONG_PD | flags);
        for index in 0..entries {
            write_u64(
                guest_ram + LONG_PD,
                index * 8,
                (index * PAGE_2M) | flags | (1 << 7),
            );
        }
    }
}

fn build_vmcb(vmcb: u64, npt: u64, iopm: u64, msrpm: u64, mode: GuestMode) {
    unsafe {
        write_u32(
            vmcb,
            0x00c,
            INTERCEPT_HLT | INTERCEPT_IOIO_PROT | INTERCEPT_CPUID | INTERCEPT_MSR_PROT,
        );
        write_u32(vmcb, 0x010, INTERCEPT2_VMRUN | INTERCEPT2_VMMCALL);
        write_u64(vmcb, 0x040, iopm);
        write_u64(vmcb, 0x048, msrpm);
        write_u32(vmcb, 0x058, 1);
        write_u8(vmcb, 0x05c, 1);
        write_u64(vmcb, 0x090, 1);
        write_u64(vmcb, 0x0b0, npt);

        let cs_attrib = match mode {
            GuestMode::Real => 0x009b,
            GuestMode::Long => 0x029b,
        };
        segment(vmcb, 0x400, 0, 0x0093, 0xffff_ffff, 0);
        segment(vmcb, 0x410, 0, cs_attrib, 0xffff_ffff, 0);
        segment(vmcb, 0x420, 0, 0x0093, 0xffff_ffff, 0);
        segment(vmcb, 0x430, 0, 0x0093, 0xffff_ffff, 0);
        segment(vmcb, 0x440, 0, 0x0093, 0xffff, 0);
        segment(vmcb, 0x450, 0, 0x0093, 0xffff, 0);
        segment(vmcb, 0x460, 0, 0, 0xffff, 0);
        segment(vmcb, 0x470, 0, 0x0082, 0xffff, 0);
        segment(vmcb, 0x480, 0, 0, 0xffff, 0);
        segment(vmcb, 0x490, 0, 0x008b, 0xffff, 0);

        let (efer, cr0, cr4, cr3, rip, rsp) = match mode {
            GuestMode::Real => (EFER_SVME, 0x6000_0010, 0, 0, GUEST_ENTRY, GUEST_STACK),
            GuestMode::Long => (
                EFER_SVME | EFER_LME | EFER_LMA,
                CR0_LONG,
                CR4_PAE,
                LONG_PML4,
                LONG_ENTRY,
                LONG_STACK,
            ),
        };
        write_u8(vmcb, 0x4cb, 0);
        write_u64(vmcb, 0x4d0, efer);
        write_u64(vmcb, 0x548, cr4);
        write_u64(vmcb, 0x550, cr3);
        write_u64(vmcb, 0x558, cr0);
        write_u64(vmcb, 0x560, 0x0000_0400);
        write_u64(vmcb, 0x568, 0xffff_0ff0);
        write_u64(vmcb, 0x570, 0x0000_0002);
        write_u64(vmcb, 0x578, rip);
        write_u64(vmcb, 0x5d8, rsp);
        write_u64(vmcb, 0x5f8, 0);
        write_u64(vmcb, 0x640, 0);
        write_u64(vmcb, 0x668, 0x0007_0406_0007_0406);
    }
}

unsafe fn segment(vmcb: u64, offset: u64, selector: u16, attrib: u16, limit: u32, base: u64) {
    unsafe {
        write_u16(vmcb, offset, selector);
        write_u16(vmcb, offset + 2, attrib);
        write_u32(vmcb, offset + 4, limit);
        write_u64(vmcb, offset + 8, base);
    }
}

#[inline(never)]
unsafe fn vmentry(vmcb: u64, host_vmcb: u64, ctx: *mut u64) {
    unsafe {
        asm!(
            "push rsi",
            "push rdx",
            "push rdi",
            "push rbx",
            "push rbp",
            "push r12",
            "push r13",
            "push r14",
            "push r15",
            "mov rax, [rsp + 56]",
            "vmsave rax",
            "clgi",
            "mov rax, [rsp + 48]",
            "mov rbx, [rax + 0]",
            "mov rcx, [rax + 8]",
            "mov rdx, [rax + 16]",
            "mov rsi, [rax + 24]",
            "mov rdi, [rax + 32]",
            "mov rbp, [rax + 40]",
            "mov r8,  [rax + 48]",
            "mov r9,  [rax + 56]",
            "mov r10, [rax + 64]",
            "mov r11, [rax + 72]",
            "mov r12, [rax + 80]",
            "mov r13, [rax + 88]",
            "mov r14, [rax + 96]",
            "mov r15, [rax + 104]",
            "mov rax, [rsp + 64]",
            "vmload rax",
            "vmrun rax",
            "vmsave rax",
            "push rax",
            "mov rax, [rsp + 56]",
            "mov [rax + 0], rbx",
            "mov [rax + 8], rcx",
            "mov [rax + 16], rdx",
            "mov [rax + 24], rsi",
            "mov [rax + 32], rdi",
            "mov [rax + 40], rbp",
            "mov [rax + 48], r8",
            "mov [rax + 56], r9",
            "mov [rax + 64], r10",
            "mov [rax + 72], r11",
            "mov [rax + 80], r12",
            "mov [rax + 88], r13",
            "mov [rax + 96], r14",
            "mov [rax + 104], r15",
            "pop rax",
            "mov rax, [rsp + 56]",
            "vmload rax",
            "stgi",
            "pop r15",
            "pop r14",
            "pop r13",
            "pop r12",
            "pop rbp",
            "pop rbx",
            "add rsp, 24",
            inout("rsi") vmcb => _,
            inout("rdx") host_vmcb => _,
            inout("rdi") ctx => _,
            out("rax") _,
            out("rcx") _,
            out("r8") _,
            out("r9") _,
            out("r10") _,
            out("r11") _,
        );
    }
}

fn zero_region(address: u64, bytes: u64) {
    fill_u64(address, bytes, 0);
}

fn fill_region(address: u64, bytes: u64, byte: u8) {
    let word = u64::from_ne_bytes([byte; 8]);
    fill_u64(address, bytes, word);
}

fn fill_u64(address: u64, bytes: u64, value: u64) {
    for offset in (0..bytes).step_by(8) {
        unsafe { core::ptr::write_volatile((address + offset) as usize as *mut u64, value) };
    }
}

unsafe fn write_u8(base: u64, offset: u64, value: u8) {
    unsafe { core::ptr::write_volatile((base + offset) as usize as *mut u8, value) };
}

unsafe fn write_u16(base: u64, offset: u64, value: u16) {
    unsafe { core::ptr::write_volatile((base + offset) as usize as *mut u16, value) };
}

unsafe fn write_u32(base: u64, offset: u64, value: u32) {
    unsafe { core::ptr::write_volatile((base + offset) as usize as *mut u32, value) };
}

unsafe fn write_u64(base: u64, offset: u64, value: u64) {
    unsafe { core::ptr::write_volatile((base + offset) as usize as *mut u64, value) };
}

unsafe fn read_u64(base: u64, offset: u64) -> u64 {
    unsafe { core::ptr::read_volatile((base + offset) as usize as *const u64) }
}

#[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
fn read_u8(base: u64, offset: u64) -> u8 {
    unsafe { core::ptr::read_volatile((base + offset) as usize as *const u8) }
}

#[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
fn read_u16(base: u64, offset: u64) -> u16 {
    unsafe { core::ptr::read_volatile((base + offset) as usize as *const u16) }
}

#[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
fn read_u32(base: u64, offset: u64) -> u32 {
    unsafe { core::ptr::read_volatile((base + offset) as usize as *const u32) }
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
            options(nomem, nostack, preserves_flags)
        );
    }
}
