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
const EXIT_CPUID: u64 = 0x072;
const EXIT_MSR: u64 = 0x07c;
const EXIT_VMMCALL: u64 = 0x081;
#[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
const EXIT_PAUSE: u64 = 0x077;
#[cfg_attr(not(feature = "linux-guest"), allow(dead_code))]
const EXIT_SHUTDOWN: u64 = 0x07f;
const EXIT_NPF: u64 = 0x400;

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

#[cfg(feature = "linux-guest")]
mod linux {
    use super::*;
    use crate::fat;

    const MIB: u64 = 1024 * 1024;
    const RAM_CANDIDATES: [u64; 5] = [320, 288, 256, 224, 192];

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

    const CMDLINE_TEXT: &[u8] =
        b"console=ttyS0,115200 earlyprintk=serial,ttyS0,115200 keep_bootcon nokaslr no5lvl nolapic nosmp acpi=off no_timer_check nmi_watchdog=0 reboot=t panic=-1 debug ignore_loglevel root=/dev/vda ro rootwait\0";

    const EFER_NXE: u64 = 1 << 11;
    const CR0_LINUX: u64 = 0x8001_0031;
    const MAX_EXITS_LINUX: u64 = 4_000_000_000;
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
    const VIRTIO_QUEUE_SIZE: u16 = 32;
    const VIRTIO_BLK_T_IN: u32 = 0;
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
    }

    pub fn boot(frames: &mut FrameAllocator) {
        if !super::prepare_svm() {
            crate::serial::line("AEROS_VM_LINUX_KERNEL stage=svm-unavailable");
            return;
        }
        let Some(vmlinuz_bytes) = fat::root_file_size(b"VMLINUZ    ") else {
            crate::serial::line("AEROS_VM_LINUX_KERNEL stage=image-absent file=VMLINUZ");
            return;
        };
        let initrd_bytes = fat::root_file_size(b"INITRD     ").unwrap_or(0);
        if !(0x1000..=64 * MIB).contains(&vmlinuz_bytes) {
            crate::serial::line("AEROS_VM_LINUX_KERNEL stage=image-implausible");
            return;
        }
        let Some(rootfs_bytes) = fat::root_file_size(ROOTFS_NAME) else {
            crate::serial::line("AEROS_VM_LINUX_KERNEL stage=image-absent file=ROOTFS");
            return;
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
            return;
        }

        let Some(npt) = frames.allocate_contiguous(6, 1).map(|f| f.address()) else {
            crate::serial::line("AEROS_VM_LINUX_KERNEL stage=npt-alloc");
            return;
        };
        let Some(scratch) = frames.allocate_contiguous(512, 512).map(|f| f.address()) else {
            crate::serial::line("AEROS_VM_LINUX_KERNEL stage=scratch-alloc");
            return;
        };
        let Some(hsave) = frames.allocate_contiguous(1, 1).map(|f| f.address()) else {
            return;
        };
        let Some(host_vmcb) = frames.allocate_contiguous(1, 1).map(|f| f.address()) else {
            return;
        };
        let Some(vmcb) = frames.allocate_contiguous(1, 1).map(|f| f.address()) else {
            return;
        };
        let Some(iopm) = frames.allocate_contiguous(3, 1).map(|f| f.address()) else {
            return;
        };
        let Some(msrpm) = frames.allocate_contiguous(2, 1).map(|f| f.address()) else {
            return;
        };

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
            return;
        }

        let boot_flag = read_u16(ram, staging + 0x1fe);
        let hdrs = read_u32(ram, staging + 0x202);
        if boot_flag != 0xaa55 || hdrs != u32::from_le_bytes(*b"HdrS") {
            crate::serial::line("AEROS_VM_LINUX_KERNEL stage=bad-bzimage-header");
            return;
        }
        let setup_sects = {
            let raw = read_u8(ram, staging + 0x1f1);
            if raw == 0 { 4u64 } else { raw as u64 }
        };
        let pm_offset = (setup_sects + 1) * 512;
        if pm_offset >= vmlinuz_bytes {
            crate::serial::line("AEROS_VM_LINUX_KERNEL stage=bad-setup-sects");
            return;
        }
        let pm_bytes = vmlinuz_bytes - pm_offset;
        copy_guest(ram, staging + pm_offset, KERNEL_LOAD, pm_bytes);

        if initrd_bytes > 0
            && fat::load_root_file(b"INITRD     ", 0, ram + INITRD_LOAD, initrd_bytes)
                != Some(initrd_bytes)
        {
            crate::serial::line("AEROS_VM_LINUX_KERNEL stage=initrd-read");
            return;
        }

        build_gdt(ram);
        build_page_tables(ram);
        build_zero_page(ram, staging, chosen, initrd_bytes);
        for (index, byte) in CMDLINE_TEXT.iter().enumerate() {
            unsafe { write_u8(ram, CMDLINE + index as u64, *byte) };
        }
        let npt_pd = build_linux_npt(npt, ram, chosen, scratch);
        build_vmcb(vmcb, npt, iopm, msrpm);

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
        };
        machine.gpr[3] = ZERO_PAGE;

        let host_fs = read_msr(FS_BASE_MSR);
        let host_gs = read_msr(GS_BASE_MSR);
        let host_kernel_gs = read_msr(KERNEL_GS_BASE_MSR);
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
        crate::serial::line("---- linux serial ----");
        machine.run();
        crate::serial::line("");
        crate::serial::line("---- linux serial end ----");
        unsafe {
            write_msr(FS_BASE_MSR, host_fs);
            write_msr(GS_BASE_MSR, host_gs);
            write_msr(KERNEL_GS_BASE_MSR, host_kernel_gs);
        }
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
            put(0x0010_0000, ram_bytes - 0x0010_0000, 1);
            write_u8(ram, ZERO_PAGE + 0x1e8, count as u8);
        }
    }

    fn build_vmcb(vmcb: u64, npt: u64, iopm: u64, msrpm: u64) {
        unsafe {
            write_u32(vmcb, 0x008, 1 << 8);
            write_u32(
                vmcb,
                0x00c,
                INTERCEPT_HLT
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
        fn run(&mut self) {
            let start = rdtsc();
            self.last_tick_tsc = start;
            let mut heartbeat = start;
            while self.exits < MAX_EXITS_LINUX {
                self.maybe_tick();
                let ctx = self.gpr.as_mut_ptr();
                unsafe { vmentry(self.vmcb, self.host_vmcb, ctx) };
                self.exits += 1;
                let now = rdtsc();
                if self.tick_pending && now.wrapping_sub(self.last_tick_tsc) > 20 * TICK_TSC {
                    self.tick_pending = false;
                    self.last_tick_tsc = now;
                }
                if now.wrapping_sub(heartbeat) > HEARTBEAT_TSC {
                    heartbeat = now;
                    let rflags = unsafe { read_u64(self.vmcb, 0x570) };
                    let int_state = unsafe { read_u64(self.vmcb, 0x068) };
                    let vintr = unsafe { read_u64(self.vmcb, 0x060) };
                    crate::serial::format(format_args!(
                        "\n[aeros-vm exits={} rip={:#x} exit={:#x} port={:#x} ticks={} serial={} blk_requests={} blk_errors={} avail={} used={} irq_pending={} status={:#x} rflags_if={} int_shadow={} vintr={:#x} pic0={:#x} pic1={:#x} pic_icw={}]\n",
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
                    ));
                }
                if now.wrapping_sub(start) > MAX_RUN_TSC {
                    return;
                }
                let exit = unsafe { read_u64(self.vmcb, 0x070) };
                self.last_exit = exit;
                self.last_rip = unsafe { read_u64(self.vmcb, 0x578) };
                match exit {
                    EXIT_IOIO => self.handle_io(),
                    EXIT_CPUID => self.handle_cpuid(),
                    EXIT_MSR => self.handle_msr(),
                    EXIT_PAUSE => self.advance(2),
                    EXIT_SHUTDOWN | 0x048 => {
                        self.reset = true;
                        return;
                    }
                    EXIT_HLT => {
                        let rflags = unsafe { read_u64(self.vmcb, 0x570) };
                        if rflags & (1 << 9) == 0 {
                            return;
                        }
                        self.advance(1);
                        self.inject_timer();
                    }
                    EXIT_VMMCALL => {
                        unsafe { write_u64(self.vmcb, 0x5f8, 2) };
                        self.advance(3);
                    }
                    EXIT_NPF => {
                        let gpa = unsafe { read_u64(self.vmcb, 0x080) };
                        self.npf_gpa = gpa;
                        if !self.map_fault(gpa) {
                            return;
                        }
                    }
                    _ => return,
                }
                if self.reset {
                    return;
                }
            }
        }

        fn maybe_tick(&mut self) {
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
            if !self.tick_pending && rdtsc().wrapping_sub(self.last_tick_tsc) >= TICK_TSC {
                let rflags = unsafe { read_u64(self.vmcb, 0x570) };
                let int_state = unsafe { read_u64(self.vmcb, 0x068) };
                if rflags & (1 << 9) != 0 && int_state & 1 == 0 {
                    self.inject_timer();
                    self.tick_pending = true;
                }
            }
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
            let mut vintr = unsafe { read_u64(self.vmcb, 0x060) };
            vintr &= !0x0000_00ff_001f_0100;
            vintr |= 1 << 8;
            vintr |= 0xf << 16;
            vintr |= 1 << 20;
            vintr |= (self.pic_base.wrapping_add(irq_line) as u64) << 32;
            unsafe { write_u64(self.vmcb, 0x060, vintr) };
        }

        fn handle_cpuid(&mut self) {
            self.cpuid_exits += 1;
            let leaf = unsafe { read_u64(self.vmcb, 0x5f8) } as u32;
            let subleaf = self.gpr[1] as u32;
            let result = __cpuid_count(leaf, subleaf);
            let (mut a, mut b, mut c, mut d) = (result.eax, result.ebx, result.ecx, result.edx);
            match leaf {
                0x0 if a < 0x16 => a = 0x16,
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
                write_u32(base, 4, 1_788_000_000);
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
                0x3f8 if self.serial_bytes < MAX_SERIAL => {
                    crate::serial::byte(value as u8);
                    self.serial_bytes += 1;
                }
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
                        self.pit_wr_hi = false;
                        if self.pit_latch == 0 {
                            self.pit_latch = 0xffff;
                        }
                    }
                }
                0x61 => self.port61 = value as u8,
                0x64 if value & 0xfe == 0xfe => self.reset = true,
                0x70 => self.cmos_index = value as u8 & 0x7f,
                0x71 => {}
                0xcf9 if value & 0x04 != 0 => self.reset = true,
                0x80 | 0xed | 0xeb => {}
                0xcf8 => self.pci_addr = value,
                0xcfc..=0xcff => self.pci_cfg_write(port, value, size),
                p if self.virtio_port_in_range(p) => {
                    self.virtio_reg_write(p as u32 - self.virtio_io_base(), value, size)
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
                0x3f8 => 0,
                0x3f9 => 0,
                0x3fa => 0x01,
                0x3fb => 0x03,
                0x3fc => 0x03,
                0x3fd => 0x60,
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
                0x64 => 0xff,
                0x71 => self.cmos_read(),
                0xcfc..=0xcff => self.pci_cfg_read(port, size),
                p if self.virtio_port_in_range(p) => {
                    self.virtio_reg_read(p as u32 - self.virtio_io_base(), size)
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
            if bus != 0 || function != 0 || device > 1 {
                return None;
            }
            Some((device as u8, (self.pci_addr & 0xfc) as u16))
        }

        fn pci_cfg_write(&mut self, port: u16, value: u32, size: u8) {
            let Some((device, reg)) = self.pci_cfg_target() else {
                return;
            };
            if device != 1 {
                return; // the host-bridge stub is entirely read-only
            }
            let lane = port - 0xcfc;
            for i in 0..size as u16 {
                self.pci_cfg_write_byte(reg + lane + i, (value >> (8 * i)) as u8);
            }
        }

        fn pci_cfg_read(&mut self, port: u16, size: u8) -> u32 {
            let Some((device, reg)) = self.pci_cfg_target() else {
                return 0xffff_ffff;
            };
            let lane = port - 0xcfc;
            let mut result = 0u32;
            for i in 0..size as u16 {
                let byte = if device == 0 {
                    Self::host_bridge_read_byte(reg + lane + i)
                } else {
                    self.pci_cfg_read_byte(reg + lane + i)
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
                        self.virtio_last_avail = 0;
                        self.virtio_used_idx = 0;
                        self.virtio_irq_pending = false;
                    }
                }
                _ => {}
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

            let avail_idx = self.gread_u16(avail_addr + 2);
            let mut processed = 0u64;
            while self.virtio_last_avail != avail_idx && processed < num {
                processed += 1;
                let ring_slot = (self.virtio_last_avail % VIRTIO_QUEUE_SIZE) as u64;
                let desc_head = self.gread_u16(avail_addr + 4 + 2 * ring_slot);
                let written = self.virtio_handle_request(desc_addr, desc_head as u64, num);
                let used_slot = (self.virtio_used_idx % VIRTIO_QUEUE_SIZE) as u64;
                let elem = used_addr + 4 + 8 * used_slot;
                self.gwrite_u32(elem, desc_head as u32);
                self.gwrite_u32(elem + 4, written);
                self.virtio_used_idx = self.virtio_used_idx.wrapping_add(1);
                self.gwrite_u16(used_addr + 2, self.virtio_used_idx);
                self.virtio_last_avail = self.virtio_last_avail.wrapping_add(1);
            }
            if processed > 0 {
                self.virtio_isr |= 1;
                self.virtio_irq_pending = true;
            }
        }

        /// Follows the descriptor chain for one request: a 16-byte
        /// `virtio_blk_outhdr` (type, reserved, sector), one data buffer,
        /// then a 1-byte device-writable status descriptor. Only
        /// VIRTIO_BLK_T_IN (read) is implemented - the backing ROOTFS is
        /// mounted `ro`, so write requests are never expected.
        fn virtio_handle_request(&mut self, desc_table: u64, head: u64, num: u64) -> u32 {
            self.blk_requests += 1;
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
                    self.blk_errors += 1;
                    return 0;
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
                        self.blk_errors += 1;
                        return 0;
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

            let ok = req_type == VIRTIO_BLK_T_IN
                && data_write
                && data_len > 0
                && self.blk_read_sectors(sector, data_addr, data_len);
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
            fat::load_root_file(ROOTFS_NAME, skip, dest_phys, len as u64) == Some(len as u64)
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
