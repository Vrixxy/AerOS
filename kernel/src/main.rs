#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]
#![allow(clippy::chunks_exact_to_as_chunks)]

mod ac97;
mod acpi;
mod acpi_ns;
pub mod aerui;
mod ahci;
mod aml;
mod antivirus;
mod arch;
mod audio;
mod audit;
mod auth;
mod bench;
mod block;
mod blockdev;
pub mod button;
mod capability;
mod clipboard;
mod compat;
mod crash;
mod datafs;
mod deflate;
mod desktop;
mod e1000;
mod ed25519;
mod elf;
mod fat;
#[cfg(feature = "boot-test")]
mod fat_crash;
mod fatfs;
#[cfg(feature = "boot-test")]
mod fatfs_crash;
mod firewall;
mod font;
mod framebuffer;
#[cfg(feature = "boot-test")]
mod fuzz;
mod hda;
mod heap;
mod image;
mod inflate;
mod initramfs;
mod installer;
mod ioapic;
mod iommu;
mod ip;
mod ipv6;
mod jpeg;
mod keyboard;
mod keymap;
mod loading;
mod logo;
mod measure;
mod memory;
mod mounts;
mod mouse;
mod net;
mod nic;
mod notify;
mod ntp;
mod nvme;
mod oom;
mod partition;
mod pci;
mod pkg;
#[cfg(feature = "boot-test")]
mod pkg_vectors;
mod png;
mod power;
mod process;
mod procfs;
mod random;
mod rtc;
mod rtl8139;
mod rtl8168;
mod scheduler;
mod screenshot;
mod sdhci;
mod seccomp;
mod serial;
mod services;
mod settings;
mod sfx;
mod shell;
mod slab;
mod smp;
mod smpsched;
mod sockopt;
mod stackguard;
mod store;
mod svm;
mod swap;
mod sync;
mod syscall;
#[cfg(feature = "boot-test")]
mod syscall_fuzz_probe;
mod sysmon;
mod tcp;
mod tcpnet;
mod time;
mod timezone;
mod trace;
mod truetype;
mod udp;
mod uefi;
mod ui;
mod update;
mod vfs;
mod virtio;
mod virtio_blk;
mod virtio_gpu;
mod virtio_input;
mod virtio_modern;
mod virtio_net;
mod web;
mod xhci;

use core::panic::PanicInfo;

use arch::CpuInfo;
use framebuffer::FrameBuffer;
use memory::FrameAllocator;
use sync::TicketLock;
use uefi::{Handle, Status, SystemTable};

const VERSION: &str = env!("CARGO_PKG_VERSION");
static BOOT_PHASE: TicketLock<u8> = TicketLock::new(0);

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    serial::format(format_args!("AEROS_PANIC {info}\n"));
    arch::halt_forever()
}

/// The bootstrap processor's kernel stack. `efi_main` runs the whole boot
/// sequence in one very large function, and the firmware's own stack (sized
/// for firmware, and mapped read-only just below it on some machines)
/// overflowed on it depending on the CPU count.
const BOOT_STACK_SIZE: usize = 4 * 1024 * 1024;

#[repr(align(4096))]
#[allow(dead_code)]
struct BootStack(core::cell::UnsafeCell<[u8; BOOT_STACK_SIZE]>);

unsafe impl Sync for BootStack {}

static BOOT_STACK: BootStack = BootStack(core::cell::UnsafeCell::new([0; BOOT_STACK_SIZE]));

/// Turns the IOMMU on, and leaves it on only if the boot disk reads back the
/// same sector through it.
fn enable_iommu() -> bool {
    if !iommu::present() {
        return false;
    }
    let mut before = [0u8; 512];
    let readable = blockdev::read_sector(0, &mut before);
    if !iommu::enable() {
        return false;
    }
    let mut after = [0u8; 512];
    if readable && !(blockdev::read_sector(0, &mut after) && after == before) {
        iommu::disable();
        return false;
    }
    true
}

/// UEFI entry point: moves to `BOOT_STACK` and continues in `kernel_entry`
/// (both take the same two arguments in the Microsoft x64 convention).
#[unsafe(no_mangle)]
#[unsafe(naked)]
extern "efiapi" fn efi_main(_image: Handle, _table: *mut SystemTable) -> Status {
    core::arch::naked_asm!(
        "lea rax, [rip + {stack}]",
        "add rax, {size}",
        "and rax, -16",
        "mov rsp, rax",
        "sub rsp, 32",
        "call {entry}",
        "ud2",
        stack = sym BOOT_STACK,
        size = const BOOT_STACK_SIZE,
        entry = sym kernel_entry,
    )
}

extern "efiapi" fn kernel_entry(image: Handle, table: *mut SystemTable) -> Status {
    stackguard::randomize();
    *BOOT_PHASE.lock() = 1;
    serial::init();
    serial::format(format_args!("AEROS_BOOT version={VERSION}\n"));

    let fonts = font::FontCatalog::load();
    if let Some(mut phase) = BOOT_PHASE.try_lock() {
        *phase = 2;
    }
    serial::format(format_args!(
        "AEROS_FONTS plus_jakarta={} roboto_mono={}\n",
        fonts.ui_ready(),
        fonts.mono_ready()
    ));

    let boot = match unsafe { uefi::take_control(image, table) } {
        Ok(state) => state,
        Err(error) => {
            serial::format(format_args!(
                "AEROS_BOOT_ERROR stage={} status={:#x}\n",
                error.stage.as_str(),
                error.status
            ));
            return error.status;
        }
    };
    *BOOT_PHASE.lock() = 3;

    arch::disable_interrupts();
    let descriptors = arch::gdt::init();
    arch::interrupts::init();
    let interrupts_valid = arch::interrupts::self_test();
    let timer_ticks = arch::interrupts::timer_self_test(100, 4).unwrap_or(0);
    serial::format(format_args!(
        "AEROS_ARCH gdt={} idt={} kernel_cs={:#x} kernel_ds={:#x} user_cs={:#x} user_ds={:#x} tss={:#x}\n",
        descriptors.loaded,
        interrupts_valid,
        descriptors.kernel_code,
        descriptors.kernel_data,
        descriptors.user_code,
        descriptors.user_data,
        descriptors.task
    ));
    serial::format(format_args!(
        "AEROS_TIMER source=pit frequency_hz=100 ticks={} valid={}\n",
        timer_ticks,
        timer_ticks >= 4
    ));
    if !descriptors.loaded || !interrupts_valid || timer_ticks < 4 {
        serial::line("AEROS_ARCH_FAILURE");
        arch::halt_forever();
    }
    let cpu = CpuInfo::detect();
    let fpu = arch::fpu::initialize(&cpu);
    if !fpu.verified {
        serial::line("AEROS_FPU_FAILURE");
        arch::halt_forever();
    }
    let syscall_entry = arch::syscall_entry::init(&cpu);
    if !syscall_entry.verified {
        serial::line("AEROS_SYSCALL_ENTRY_FAILURE");
        arch::halt_forever();
    }
    let acpi = unsafe { acpi::inspect(boot.rsdp) };
    let time = time::initialize(acpi.hpet_address);
    if !acpi.hpet_valid || !time.verified {
        serial::format(format_args!(
            "AEROS_HPET_DISCOVERY table={:#x} address={:#x} space={} width={} acpi_valid={} timer_valid={}\n",
            acpi.hpet_table,
            acpi.hpet_address,
            acpi.hpet_address_space,
            acpi.hpet_bit_width,
            acpi.hpet_valid,
            time.verified
        ));
        serial::line("AEROS_HPET_FAILURE");
        arch::halt_forever();
    }
    #[cfg(feature = "boot-test")]
    {
        let test = stackguard::self_test();
        let verified =
            test.cookie_random && test.instrumented && test.intact_passes && test.smash_detected;
        serial::format(format_args!(
            "AEROS_STACK_PROTECTOR cookie_random={} instrumented={} intact_passes={} smash_detected={} verified={}
",
            test.cookie_random,
            test.instrumented,
            test.intact_passes,
            test.smash_detected,
            verified
        ));
        if !verified {
            serial::line("AEROS_STACK_PROTECTOR_INVARIANT_FAILURE");
            arch::halt_forever();
        }
    }
    let namespace = acpi_ns::initialize(&acpi);
    let rtc = rtc::initialize();
    if !rtc.verified {
        serial::line("AEROS_RTC_FAILURE");
        arch::halt_forever();
    }
    let entropy = random::initialize(&cpu);
    if !entropy.verified {
        serial::line("AEROS_ENTROPY_FAILURE");
        arch::halt_forever();
    }
    let apic = arch::apic::initialize(acpi.local_apic_address);
    if !apic.verified {
        serial::line("AEROS_APIC_FAILURE");
        arch::halt_forever();
    }
    let ioapic = ioapic::initialize(&acpi, apic.id);
    if !ioapic.verified {
        serial::line("AEROS_IOAPIC_FAILURE");
        arch::halt_forever();
    }
    let mut frames = FrameAllocator::from_map(&boot.memory);
    frames.track();
    sysmon::set_total_ram((boot.memory.usable_pages() + boot.memory.reclaimable_pages()) * 4096);
    let allocator_check = frames.self_test();
    let paging = match arch::paging::initialize(&mut frames, &cpu) {
        Some(state) => state,
        None => {
            serial::line("AEROS_PAGING_FAILURE");
            arch::halt_forever();
        }
    };
    scheduler::set_default_cr3(paging.root_physical);
    arch::paging::set_boot_state(&paging);
    let demand_paging_ready = arch::paging::demand_init(&paging, &mut frames);
    const DEMAND_POOL_PAGES: u64 = 4096;
    let demand_pool_installed = frames
        .allocate_contiguous(DEMAND_POOL_PAGES, 1)
        .and_then(|block| FrameAllocator::from_range(block.address(), DEMAND_POOL_PAGES))
        .map(memory::install_global)
        .is_some();
    let demand_paging =
        demand_paging_ready && demand_pool_installed && arch::paging::demand_self_test();
    let demand_stats = arch::paging::demand_stats();
    serial::format(format_args!(
        "AEROS_DEMAND_PAGING ready={} pool_pages={} reserved={} committed={} faults={} verified={}\n",
        demand_paging_ready,
        DEMAND_POOL_PAGES,
        demand_stats.reserved_pages,
        demand_stats.committed_pages,
        demand_stats.faults_handled,
        demand_paging
    ));
    if !demand_paging {
        serial::line("AEROS_DEMAND_PAGING_FAILURE");
        arch::halt_forever();
    }
    let smp = smp::initialize(&acpi, &paging, boot.ap_trampoline);
    let syscall_cpu_mask = arch::syscall_entry::ready_mask();
    let syscall_per_cpu = syscall_cpu_mask == smp::online_mask();
    if !smp.verified || !syscall_per_cpu {
        serial::format(format_args!(
            "AEROS_SMP_DIAGNOSTIC discovered={} applications={} online={} bsp_id={} last_ap_id={} trampoline={:#x} stage={} gdt_stage={} work={} idle={} ipi_acks={} queue_dispatches={} queue_completions={} work_queue={} verified={}\n",
            smp.discovered,
            smp.applications_started,
            smp.online,
            smp.bsp_id,
            smp.last_ap_id,
            smp.trampoline,
            smp.stage,
            smp.gdt_stage,
            smp.work,
            smp.idle,
            smp.ipi_acks,
            smp.queue_dispatches,
            smp.queue_completions,
            smp.work_queue,
            smp.verified
        ));
        serial::line("AEROS_SMP_FAILURE");
        arch::halt_forever();
    }
    // 4096 pages = 16 MiB - up from the original 1 MiB, now that `map_heap`
    // builds a real multi-page-table region instead of squeezing into the
    // handful of spare slots in the paging probe's single leaf table (see
    // its doc comment). Concretely unblocks `spawn()`'s 64 KB-per-task
    // kernel-stack allocations (`heap::HEAP`) well past the ~9-task ceiling
    // the old 1 MiB heap hit in `AEROS_TASK_EXHAUSTION` earlier this session.
    let heap_mapping = match arch::paging::map_heap(&paging, &mut frames, 4096) {
        Some(mapping) => mapping,
        None => {
            serial::line("AEROS_HEAP_MAPPING_FAILURE");
            arch::halt_forever();
        }
    };
    let heap_initialized =
        unsafe { heap::HEAP.initialize(heap_mapping.virtual_base as usize, heap_mapping.size) };
    let heap_valid = heap_initialized && heap::HEAP.self_test();
    #[cfg(feature = "boot-test")]
    {
        let verified = slab::self_test();
        serial::format(format_args!("AEROS_SLAB verified={}\n", verified));
        if !verified {
            serial::line("AEROS_SLAB_INVARIANT_FAILURE");
            arch::halt_forever();
        }
    }
    let heap_stats = heap::HEAP.stats();
    let pci = pci::PciInventory::scan();
    let pci_summary = pci.summary();
    let iommu_report = iommu::initialize(&acpi, &pci, &mut frames);
    let ahci = ahci::initialize(&pci, &mut frames);
    let nvme = nvme::initialize(&pci, &mut frames);
    let audio = ac97::initialize(&pci, &mut frames);
    let hda_audio = hda::initialize(&pci, &mut frames);
    let usb = xhci::initialize(&pci, &mut frames);
    let sd = sdhci::initialize(&pci);
    let virtio_disk = virtio_blk::initialize(&pci, &mut frames);
    let virtio_nic = virtio_net::initialize(&pci, &mut frames);
    let virtio_touch = virtio_input::initialize(&pci, &mut frames);
    let virtio_gpu = virtio_gpu::initialize(&pci, &mut frames);
    let partitions = partition::inspect(if ahci.sectors != 0 {
        ahci.sectors
    } else {
        blockdev::boot_sectors()
    });
    if ahci.sectors != 0 {
        for partition in partitions.entries().iter().filter(|part| part.kind == 0x82) {
            if swap::configure_partition(
                block::Disk::Ahci(ahci::boot_disk()),
                partition.first_lba,
                partition.sectors,
            ) != 0
            {
                break;
            }
        }
    }
    let fat = fat::inspect(&partitions);
    let install = installer::install(installer::unattended_allowed(), &mut frames);
    let mut network = e1000::initialize(&pci, &mut frames);
    let rtl8139_nic = rtl8139::initialize(&pci, &mut frames);
    let rtl8168_nic = rtl8168::initialize(&pci, &mut frames);
    nic::select_primary(network.verified, &rtl8139_nic, &rtl8168_nic);
    let iommu_active = enable_iommu();
    #[cfg(feature = "boot-test")]
    {
        let test = iommu::self_test(&pci);
        let status = iommu::status();
        let verified = !iommu_report.present || (iommu_active && test.verified());
        serial::format(format_args!(
            "AEROS_IOMMU present={} base={:#x} devices={} unity={} enabled={} mapped_pages={} edu={} mapped={} blocked_write={} blocked_read={} revoked={} faults={} other_events={} last_fault={:#x}/{:#x} verified={}\n",
            iommu_report.present,
            iommu_report.base,
            iommu_report.devices,
            iommu_report.unity_ranges,
            iommu_active,
            status.mapped_pages,
            test.edu,
            test.mapped,
            test.blocked_write,
            test.blocked_read,
            test.revoked,
            status.faults,
            status.other_events,
            status.last_fault.raw[0],
            status.last_fault.raw[1],
            verified
        ));
        if !verified {
            serial::line("AEROS_IOMMU_INVARIANT_FAILURE");
            arch::halt_forever();
        }
    }
    serial::format(format_args!(
        "AEROS_IOMMU_UNIT present={} base={:#x} devices={} unity={} enabled={}\n",
        iommu_report.present,
        iommu_report.base,
        iommu_report.devices,
        iommu_report.unity_ranges,
        iommu_active
    ));
    if !network.verified {
        // No Intel NIC: the stack runs on the first Realtek one that reached
        // the default gateway.
        if rtl8139_nic.verified && rtl8139_nic.gateway == [10, 0, 2, 2] {
            network = e1000::NetworkReport::from_nic(&rtl8139_nic, "rtl8139");
        } else if rtl8168_nic.verified && rtl8168_nic.gateway == [10, 0, 2, 2] {
            network = e1000::NetworkReport::from_nic(&rtl8168_nic, "rtl8168");
        }
    }
    let internet = net::self_test(&network);
    #[cfg(feature = "boot-test")]
    {
        // The host side of this connects in over a QEMU hostfwd rule
        // (tools/test.ps1) and exchanges real bytes - the only way to prove
        // a genuine passive-open TCP server, not just the existing client.
        serial::line("AEROS_TCP_LISTENING");
        let server = net::tcp_server_self_test(17_654, 12_000_000_000);
        serial::format(format_args!(
            "AEROS_TCP_SERVER syn_received={} handshake_completed={} echoed_bytes={} closed_cleanly={} verified={}\n",
            server.syn_received,
            server.handshake_completed,
            server.echoed_bytes,
            server.closed_cleanly,
            server.verified
        ));
        if !server.verified {
            serial::line("AEROS_TCP_SERVER_INVARIANT_FAILURE");
            arch::halt_forever();
        }
        serial::line("AEROS_TCP_NET_LISTENING");
        let (accepted, echoed, closed) = syscall::tcp_net_echo_self_test(17_655, 20_000_000_000);
        serial::format(format_args!(
            "AEROS_TCP_NET accepted={} echoed_bytes={} closed={} verified={}
",
            accepted,
            echoed,
            closed,
            accepted && echoed > 0 && closed
        ));
        if !(accepted && echoed > 0 && closed) {
            serial::line("AEROS_TCP_NET_INVARIANT_FAILURE");
            arch::halt_forever();
        }
        tcpnet::reset();
        let mut response = [0u8; 8192];
        let http = net::http_get_port(
            ip::v4([10, 0, 2, 2]),
            18_080,
            "10.0.2.2",
            "/big",
            &mut response,
        );
        let body_start = response[..http.bytes]
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map_or(0, |position| position + 4);
        let body = &response[body_start..http.bytes];
        let body_ok = body.len() == 6_000
            && body
                .iter()
                .enumerate()
                .all(|(index, byte)| *byte == b'a' + (index % 26) as u8);
        serial::format(format_args!(
            "AEROS_HTTP_CLIENT connected={} status={} bytes={} body_ok={} verified={}\n",
            http.connected,
            http.status,
            http.bytes,
            body_ok,
            http.connected && http.status == 200 && body_ok
        ));
        if !(http.connected && http.status == 200 && body_ok) {
            serial::line("AEROS_HTTP_CLIENT_INVARIANT_FAILURE");
            arch::halt_forever();
        }
        tcpnet::reset();
    }
    let firewall_report = firewall::self_test();
    serial::format(format_args!(
        "AEROS_FIREWALL empty_passes={} port_match_blocks={} port_mismatch_passes={} protocol_mismatch_passes={} any_port_blocks_all={} cleared_passes_again={} dropped_counted={} verified={}\n",
        firewall_report.empty_passes,
        firewall_report.port_match_blocks,
        firewall_report.port_mismatch_passes,
        firewall_report.protocol_mismatch_passes,
        firewall_report.any_port_blocks_all,
        firewall_report.cleared_passes_again,
        firewall_report.dropped_counted,
        firewall_report.verified
    ));
    if !firewall_report.verified {
        serial::line("AEROS_FIREWALL_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let initramfs_entries = initramfs::entries();
    let vfs_stats = vfs::initialize(&initramfs_entries);
    let home = datafs::initialize();
    #[cfg(not(feature = "boot-test"))]
    settings::load();
    #[cfg(feature = "boot-test")]
    {
        serial::format(format_args!(
            "AEROS_HOME present={} formatted={} fat32={} clusters={} free_clusters={} verified={}\n",
            home.present,
            home.formatted,
            home.fat32,
            home.clusters,
            home.free_clusters,
            home.verified
        ));
        if home.present {
            let result = datafs::self_test();
            serial::format(format_args!(
                "AEROS_HOME_VFS tree={} file_io={} listing={} rename_paths={} cross_mount={} read_only={} remount={} verified={}\n",
                result.tree,
                result.file_io,
                result.listing,
                result.rename_paths,
                result.cross_mount,
                result.read_only,
                result.remount,
                result.verified
            ));
            if !home.verified || !result.verified {
                serial::line("AEROS_HOME_INVARIANT_FAILURE");
                arch::halt_forever();
            }
            let mounts = mounts::self_test();
            serial::format(format_args!(
                "AEROS_MOUNTS bound={} same_file={} listing={} rename={} protected={} refused={} unbind={} persists={} verified={}\n",
                mounts.bound,
                mounts.same_file,
                mounts.listing,
                mounts.rename_inside,
                mounts.protected,
                mounts.refused,
                mounts.unbind,
                mounts.persists,
                mounts.verified
            ));
            if !mounts.verified {
                serial::line("AEROS_MOUNTS_INVARIANT_FAILURE");
                arch::halt_forever();
            }
        }
    }
    #[cfg(not(feature = "boot-test"))]
    let _ = &home;
    let init_file = match vfs::file("/bin/init") {
        Ok(file) => file,
        Err(_) => {
            serial::line("AEROS_INIT_NOT_FOUND");
            arch::halt_forever();
        }
    };
    let init_image = match elf::ElfImage::parse(init_file.data) {
        Ok(image) => image,
        Err(_) => {
            serial::line("AEROS_ELF_VALIDATION_FAILURE");
            arch::halt_forever();
        }
    };
    let elf_valid = init_image.self_test();
    let compatibility = compat::inspect(init_file.data);
    if !compatibility.verified {
        serial::line("AEROS_COMPATIBILITY_FAILURE");
        arch::halt_forever();
    }
    let svm = svm::self_test(&mut frames);
    serial::format(format_args!(
        "AEROS_SVM svm_supported={} npt_supported={} svm_enabled={} locked_off={} guest_ram_bytes={} exits={} io_writes={} cpuid_exits={} msr_exits={} last_exit_code={:#x} halted={} console_len={} console_ok={} high_marker={:#x} guest_marker={:#x} verified={}\n",
        svm.svm_supported,
        svm.npt_supported,
        svm.svm_enabled,
        svm.locked_off,
        svm.guest_ram_bytes,
        svm.exits,
        svm.io_writes,
        svm.cpuid_exits,
        svm.msr_exits,
        svm.last_exit_code,
        svm.halted,
        svm.console_len,
        svm.console_ok,
        svm.high_marker,
        svm.guest_marker,
        svm.verified
    ));
    #[cfg(feature = "linux-guest")]
    svm::boot_linux(&mut frames);
    process::initialize();
    let process_frames_before = frames.stats();
    let user_mapping = match arch::paging::map_user_probe(&paging, &mut frames) {
        Some(mapping) => mapping,
        None => {
            serial::line("AEROS_USER_MAPPING_FAILURE");
            arch::halt_forever();
        }
    };
    let user_probe = arch::user::run_probe(&user_mapping, &cpu);
    let user_reap = arch::paging::destroy_user_probe(&paging, &mut frames, &user_mapping);
    let mut init_mapping = match arch::paging::map_user_image(&paging, &mut frames, &init_image) {
        Some(mapping) => mapping,
        None => {
            serial::line("AEROS_PROCESS_MAPPING_FAILURE");
            arch::halt_forever();
        }
    };
    let process_stack = process::prepare_linux_stack(&mut init_mapping, &init_image, "/bin/init");
    if !process_stack.verified {
        serial::line("AEROS_PROCESS_STACK_FAILURE");
        arch::halt_forever();
    }
    let init_token = match process::spawn("/bin/init", 0) {
        Some(token) if process::activate(&token) => token,
        _ => {
            serial::line("AEROS_PROCESS_TABLE_FAILURE");
            arch::halt_forever();
        }
    };
    let init_process = arch::user::run_image(&init_mapping, &cpu, 73);
    let init_exited = process::mark_exit(&init_token, init_process.exit_code);
    let init_reap = arch::paging::destroy_user_image(&paging, &mut frames, &init_mapping);
    let init_lifecycle = init_exited && process::reap(&init_token);
    let compiled_file = match vfs::file("/bin/aeros-init") {
        Ok(file) => file,
        Err(_) => {
            serial::line("AEROS_COMPILED_INIT_NOT_FOUND");
            arch::halt_forever();
        }
    };
    let compiled_image = match elf::ElfImage::parse(compiled_file.data) {
        Ok(image) => image,
        Err(_) => {
            serial::line("AEROS_COMPILED_ELF_FAILURE");
            arch::halt_forever();
        }
    };
    let mut compiled_mapping =
        match arch::paging::map_user_image(&paging, &mut frames, &compiled_image) {
            Some(mapping) => mapping,
            None => {
                serial::line("AEROS_COMPILED_MAPPING_FAILURE");
                arch::halt_forever();
            }
        };
    let compiled_stack =
        process::prepare_linux_stack(&mut compiled_mapping, &compiled_image, "/bin/aeros-init");
    if !compiled_stack.verified {
        serial::line("AEROS_COMPILED_STACK_FAILURE");
        arch::halt_forever();
    }
    let compiled_token = match process::spawn("/bin/aeros-init", 0) {
        Some(token) if process::activate(&token) => token,
        _ => {
            serial::line("AEROS_COMPILED_PROCESS_FAILURE");
            arch::halt_forever();
        }
    };
    let compiled_process = arch::user::run_image(&compiled_mapping, &cpu, 74);
    let compiled_exited = process::mark_exit(&compiled_token, compiled_process.exit_code);
    let compiled_reap = arch::paging::destroy_user_image(&paging, &mut frames, &compiled_mapping);
    let compiled_lifecycle = compiled_exited && process::reap(&compiled_token);
    let std_file = match vfs::file("/bin/aeros-std-smoke") {
        Ok(file) => file,
        Err(_) => {
            serial::line("AEROS_STD_SMOKE_NOT_FOUND");
            arch::halt_forever();
        }
    };
    let std_image = match elf::ElfImage::parse(std_file.data) {
        Ok(image) => image,
        Err(_) => {
            serial::line("AEROS_STD_ELF_FAILURE");
            arch::halt_forever();
        }
    };
    let mut std_mapping = match arch::paging::map_user_image(&paging, &mut frames, &std_image) {
        Some(mapping) => mapping,
        None => {
            serial::line("AEROS_STD_MAPPING_FAILURE");
            arch::halt_forever();
        }
    };
    let std_stack =
        process::prepare_linux_stack(&mut std_mapping, &std_image, "/bin/aeros-std-smoke");
    if !std_stack.verified {
        serial::line("AEROS_STD_STACK_FAILURE");
        arch::halt_forever();
    }
    let std_token = match process::spawn("/bin/aeros-std-smoke", 0) {
        Some(token) if process::activate(&token) => token,
        _ => {
            serial::line("AEROS_STD_PROCESS_FAILURE");
            arch::halt_forever();
        }
    };
    let std_process = arch::user::run_image(&std_mapping, &cpu, 75);
    let std_exited = process::mark_exit(&std_token, std_process.exit_code);
    let std_reap = arch::paging::destroy_user_image(&paging, &mut frames, &std_mapping);
    let std_lifecycle = std_exited && process::reap(&std_token);
    let fault_file = match vfs::file("/bin/fault-probe") {
        Ok(file) => file,
        Err(_) => {
            serial::line("AEROS_FAULT_PROBE_NOT_FOUND");
            arch::halt_forever();
        }
    };
    let fault_image = match elf::ElfImage::parse(fault_file.data) {
        Ok(image) => image,
        Err(_) => {
            serial::line("AEROS_FAULT_ELF_FAILURE");
            arch::halt_forever();
        }
    };
    let fault_mapping = match arch::paging::map_user_image(&paging, &mut frames, &fault_image) {
        Some(mapping) => mapping,
        None => {
            serial::line("AEROS_FAULT_MAPPING_FAILURE");
            arch::halt_forever();
        }
    };
    let fault_token = match process::spawn("/bin/fault-probe", 0) {
        Some(token) if process::activate(&token) => token,
        _ => {
            serial::line("AEROS_FAULT_PROCESS_TABLE_FAILURE");
            arch::halt_forever();
        }
    };
    let fault_process = arch::user::run_image(&fault_mapping, &cpu, 132);
    let fault_exited = process::mark_exit(&fault_token, fault_process.exit_code);
    let fault_reap = arch::paging::destroy_user_image(&paging, &mut frames, &fault_mapping);
    let fault_lifecycle = fault_exited && process::reap(&fault_token);
    let fault_stats = arch::user::fault_stats();
    let syscall_stats = syscall::stats();
    let runtime_vfs = vfs::stats();
    let tmpfs_valid = runtime_vfs.verified
        && runtime_vfs.nodes == runtime_vfs.directories + runtime_vfs.files
        && runtime_vfs.directories == 11
        && runtime_vfs.files == initramfs::entries().len() + 1
        && runtime_vfs.mutable_files == 1
        && runtime_vfs.mutable_bytes == 6
        && runtime_vfs.open_handles == 0;
    let process_table = process::stats();
    let process_generations = process::distinct_generation(&init_token, &compiled_token)
        && process::distinct_generation(&init_token, &std_token)
        && process::distinct_generation(&compiled_token, &fault_token)
        && process::distinct_generation(&compiled_token, &std_token)
        && process::distinct_generation(&std_token, &fault_token)
        && process::distinct_generation(&init_token, &fault_token);
    let process_frames_after = frames.stats();
    let process_released_pages = user_reap
        .released_pages
        .saturating_add(init_reap.released_pages)
        .saturating_add(compiled_reap.released_pages)
        .saturating_add(std_reap.released_pages)
        .saturating_add(fault_reap.released_pages);
    let process_reap_valid = user_reap.verified
        && init_reap.verified
        && compiled_reap.verified
        && std_reap.verified
        && fault_reap.verified
        && process_frames_after.free_pages == process_frames_before.free_pages
        && process_frames_after.allocated_pages == process_frames_before.allocated_pages
        && init_lifecycle
        && compiled_lifecycle
        && std_lifecycle
        && fault_lifecycle
        && process_generations
        && process_table.verified
        && process_table.spawned == 4
        && process_table.reaped == 4;
    let scheduler_valid = scheduler::self_test();
    let preemption_valid = scheduler_valid && scheduler::preemption_self_test();
    let pipe_blocking_valid = scheduler_valid && syscall::pipe_blocking_self_test();
    let scheduler_stats = scheduler::stats();
    let preemption_ticks = scheduler::preemption_ticks();
    let (preempt_work_a, preempt_work_b) = scheduler::preemption_work();
    let preempt_fpu_isolation = scheduler::preemption_fpu_isolation();
    let keyboard = keyboard::initialize(&acpi, apic.id);
    let mouse = mouse::initialize(&acpi, apic.id);
    let shell_info = shell::SystemInfo {
        version: VERSION,
        cpu: &cpu,
        memory: &boot.memory,
        allocator: frames.stats(),
        pci: &pci,
        storage: ahci,
        fat,
        network,
        internet,
        cpu_count: smp.online as usize,
    };
    let commands = shell::self_test(shell_info);
    let ui_core = aerui::self_test();

    serial::format(format_args!(
        "AEROS_CPU vendor={} max_leaf={:#x} nx={} syscall={} page1g={} smep={} smap={} x2apic={} invariant_tsc={} rdrand={} rdseed={}\n",
        cpu.vendor(),
        cpu.max_basic_leaf,
        cpu.nx,
        cpu.syscall,
        cpu.one_gib_pages,
        cpu.smep,
        cpu.smap,
        cpu.x2apic,
        cpu.invariant_tsc,
        cpu.rdrand,
        cpu.rdseed
    ));
    serial::format(format_args!(
        "AEROS_COMPAT guest=debian13-amd64 vmx={} svm={} npt={} nested={} hardware_acceleration={} software_emulation={} static={} dynamic={} script={} malformed={} readonly_base={} per_app_overlay={} host_files_default_deny={} devices_default_deny={} bridge_protocol={} verified={}\n",
        compatibility.vmx,
        compatibility.svm,
        compatibility.npt,
        compatibility.under_hypervisor,
        compatibility.hardware_acceleration,
        compatibility.software_emulation,
        compatibility.static_route.name(),
        compatibility.dynamic_route.name(),
        compatibility.script_route.name(),
        compatibility.malformed_route.name(),
        compatibility.read_only_base,
        compatibility.per_app_overlay,
        compatibility.host_files_default_deny,
        compatibility.devices_default_deny,
        compatibility.bridge_protocol,
        compatibility.verified
    ));
    serial::format(format_args!(
        "AEROS_FPU fxsave={} sse={} xsave={} avx={} xcr0={:#x} state_bytes={} simd={} per_cpu={} verified={}\n",
        fpu.fxsave,
        fpu.sse,
        fpu.xsave,
        fpu.avx,
        fpu.xcr0,
        fpu.state_bytes,
        fpu.simd,
        smp.online == smp.discovered,
        fpu.verified && smp.verified
    ));
    serial::format(format_args!(
        "AEROS_SYSCALL_ENTRY supported={} sce={} target={} flags_masked={} cpu_mask={:#x} per_cpu={} swapgs=true verified={}\n",
        syscall_entry.supported,
        syscall_entry.sce,
        syscall_entry.target_valid,
        syscall_entry.flags_masked,
        syscall_cpu_mask,
        syscall_per_cpu,
        syscall_entry.verified && syscall_per_cpu
    ));
    serial::format(format_args!(
        "AEROS_APIC enabled={} x2apic={} id={} version={:#x} max_lvt={} counts_per_100hz={} test_ticks={} verified={}\n",
        apic.enabled,
        apic.x2apic,
        apic.id,
        apic.version,
        apic.max_lvt,
        apic.counts_per_100hz,
        apic.test_ticks,
        apic.verified
    ));
    serial::format(format_args!(
        "AEROS_IOAPIC present={} address={:#x} id={} version={:#x} redirections={} gsi_base={} timer_gsi={} vector={} active_low={} level={} ticks={} pic_masked=true verified={}\n",
        ioapic.present,
        ioapic.address,
        ioapic.id,
        ioapic.version,
        ioapic.redirections,
        ioapic.gsi_base,
        ioapic.timer_gsi,
        ioapic.timer_vector,
        ioapic.active_low,
        ioapic.level_triggered,
        ioapic.ticks,
        ioapic.verified
    ));
    serial::format(format_args!(
        "AEROS_SMP discovered={} applications={} online={} bsp_id={} last_ap_id={} trampoline={:#x} stage={} gdt_stage={} work={} idle={} ipi_acks={} queue_dispatches={} queue_completions={} work_queue={} isolated_stacks=true verified={}\n",
        smp.discovered,
        smp.applications_started,
        smp.online,
        smp.bsp_id,
        smp.last_ap_id,
        smp.trampoline,
        smp.stage,
        smp.gdt_stage,
        smp.work,
        smp.idle,
        smp.ipi_acks,
        smp.queue_dispatches,
        smp.queue_completions,
        smp.work_queue,
        smp.verified
    ));
    serial::format(format_args!(
        "AEROS_HPET present={} base={:#x} period_fs={} counter64={} timers={} start={} end={} elapsed_ns={} verified={}\n",
        time.present,
        time.base,
        time.period_fs,
        time.counter_64,
        time.timers,
        time.start,
        time.end,
        time.nanoseconds,
        time.verified
    ));
    serial::format(format_args!(
        "AEROS_RTC year={} month={} day={} hour={} minute={} second={} unix_seconds={} verified={}\n",
        rtc.year,
        rtc.month,
        rtc.day,
        rtc.hour,
        rtc.minute,
        rtc.second,
        rtc.unix_seconds,
        rtc.verified
    ));
    let chacha20_verified = random::chacha20_self_test();
    let aslr_verified = arch::paging::aslr_self_test(&paging, &mut frames);
    let process_aslr_verified = arch::paging::scheduled_process_aslr_self_test(&paging);
    serial::format(format_args!(
        "AEROS_ENTROPY rdrand={} rdseed={} hardware_words={} sample_a_nonzero={} sample_b_nonzero={} distinct={} chacha20={} aslr={} process_aslr={} verified={}\n",
        entropy.rdrand,
        entropy.rdseed,
        entropy.hardware_words,
        entropy.sample_a != 0,
        entropy.sample_b != 0,
        entropy.sample_a != entropy.sample_b,
        chacha20_verified,
        aslr_verified,
        process_aslr_verified,
        entropy.verified && chacha20_verified && aslr_verified && process_aslr_verified
    ));
    if !chacha20_verified {
        serial::line("AEROS_ENTROPY_CHACHA20_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    if !aslr_verified {
        serial::line("AEROS_ENTROPY_ASLR_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    if !process_aslr_verified {
        serial::line("AEROS_ENTROPY_PROCESS_ASLR_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    serial::format(format_args!(
        "AEROS_MEMORY regions={} usable_pages={} reclaimable_pages={} dropped={} free_pages={} allocator_test={}\n",
        boot.memory.region_count(),
        boot.memory.usable_pages(),
        boot.memory.reclaimable_pages(),
        boot.memory.dropped_regions(),
        frames.stats().free_pages,
        allocator_check
    ));
    serial::format(format_args!(
        "AEROS_PAGING root={:#x} previous={:#x} probe_virtual={:#x} probe_physical={:#x} nx={} wp={} verified={}\n",
        paging.root_physical,
        paging.previous_root,
        paging.probe_virtual,
        paging.probe_physical,
        paging.nx_enabled,
        paging.write_protect_enabled,
        paging.verified
    ));
    if !paging.verified || !paging.write_protect_enabled || (cpu.nx && !paging.nx_enabled) {
        serial::line("AEROS_PAGING_INVARIANT_FAILURE");
        arch::halt_forever();
    }
    serial::format(format_args!(
        "AEROS_HEAP virtual={:#x} physical={:#x} bytes={} free={} active={} verified={}\n",
        heap_mapping.virtual_base,
        heap_mapping.physical_base,
        heap_stats.total_bytes,
        heap_stats.free_bytes,
        heap_stats.active_allocations,
        heap_valid
    ));
    if !heap_valid {
        serial::line("AEROS_HEAP_INVARIANT_FAILURE");
        arch::halt_forever();
    }
    let clipboard_valid = clipboard::self_test();
    serial::format(format_args!("AEROS_CLIPBOARD verified={clipboard_valid}\n"));
    if !clipboard_valid {
        serial::line("AEROS_CLIPBOARD_INVARIANT_FAILURE");
        arch::halt_forever();
    }
    serial::format(format_args!(
        "AEROS_VFS nodes={} directories={} files={} bytes={} handles={} mutable_files={} mutable_bytes={} readonly_root=true verified={}\n",
        vfs_stats.nodes,
        vfs_stats.directories,
        vfs_stats.files,
        vfs_stats.bytes,
        vfs_stats.open_handles,
        vfs_stats.mutable_files,
        vfs_stats.mutable_bytes,
        vfs_stats.verified
    ));
    if !vfs_stats.verified {
        serial::line("AEROS_VFS_INVARIANT_FAILURE");
        arch::halt_forever();
    }
    serial::format(format_args!(
        "AEROS_ELF type=pie machine=x86_64 segments={} file_bytes={} memory_bytes={} entry={:#x} wx=false verified={}\n",
        init_image.segments().len(),
        init_file.data.len(),
        init_image.memory_bytes(),
        init_image.entry(),
        elf_valid
    ));
    if !elf_valid {
        serial::line("AEROS_ELF_INVARIANT_FAILURE");
        arch::halt_forever();
    }
    serial::format(format_args!(
        "AEROS_USER entry={:#x} stack={:#x} code_physical={:#x} stack_physical={:#x} exit={} smep={} smap={} mapped={} verified={}\n",
        user_mapping.entry,
        user_mapping.stack_top,
        user_mapping.code_physical,
        user_mapping.stack_physical,
        user_probe.exit_code,
        user_probe.smep_enabled,
        user_probe.smap_enabled,
        user_mapping.verified,
        user_probe.verified
    ));
    serial::format(format_args!(
        "AEROS_PROCESS path=/bin/init load_bias={:#x} entry={:#x} stack={:#x} guard={:#x} pages={} executable={} writable={} stack_pages={} exit={} verified={}\n",
        init_mapping.load_bias,
        init_mapping.entry,
        init_mapping.stack_top,
        init_mapping.guard_page,
        init_mapping.mapped_pages,
        init_mapping.executable_pages,
        init_mapping.writable_pages,
        init_mapping.stack_pages,
        init_process.exit_code,
        init_process.verified
    ));
    serial::format(format_args!(
        "AEROS_PROCESS_STACK abi=linux pointer={:#x} argc={} aux_entries={} bytes={} aligned={} random={} phdr={:#x} verified={}\n",
        process_stack.stack_pointer,
        process_stack.argc,
        process_stack.aux_entries,
        process_stack.bytes_used,
        process_stack.aligned,
        process_stack.random_nonzero,
        init_mapping.load_bias + 64,
        process_stack.verified
    ));
    serial::format(format_args!(
        "AEROS_COMPILED_USERSPACE path=/bin/aeros-init format=rust-static-pie segments={} file_bytes={} pages={} executable={} writable={} stack_aligned={} exit={} verified={}\n",
        compiled_image.segments().len(),
        compiled_file.data.len(),
        compiled_mapping.mapped_pages,
        compiled_mapping.executable_pages,
        compiled_mapping.writable_pages,
        compiled_stack.aligned,
        compiled_process.exit_code,
        compiled_image.self_test()
            && compiled_stack.verified
            && compiled_process.verified
            && compiled_lifecycle
            && compiled_reap.verified
    ));
    serial::format(format_args!(
        "AEROS_STD_USERSPACE path=/bin/aeros-std-smoke runtime=rust-std-musl segments={} file_bytes={} pages={} executable={} writable={} stack_aligned={} exit={} verified={}\n",
        std_image.segments().len(),
        std_file.data.len(),
        std_mapping.mapped_pages,
        std_mapping.executable_pages,
        std_mapping.writable_pages,
        std_stack.aligned,
        std_process.exit_code,
        std_image.self_test()
            && std_stack.verified
            && std_process.verified
            && std_lifecycle
            && std_reap.verified
    ));
    serial::format(format_args!(
        "AEROS_ISOLATION path=/bin/fault-probe vector={} error={:#x} address={:#x} faults={} exit={} kernel_survived=true verified={}\n",
        fault_stats.last_vector,
        fault_stats.last_error,
        fault_stats.last_address,
        fault_stats.faults,
        fault_process.exit_code,
        fault_process.verified
            && fault_image.self_test()
            && fault_stats.faults == 1
            && fault_stats.last_vector == 6
    ));
    serial::format(format_args!(
        "AEROS_REAP mappings=5 released_pages={} free_before={} free_after={} address_spaces_removed={} scrubbed={} verified={}\n",
        process_released_pages,
        process_frames_before.free_pages,
        process_frames_after.free_pages,
        user_reap.address_space_removed
            && init_reap.address_space_removed
            && compiled_reap.address_space_removed
            && std_reap.address_space_removed
            && fault_reap.address_space_removed,
        user_reap.scrubbed
            && init_reap.scrubbed
            && compiled_reap.scrubbed
            && std_reap.scrubbed
            && fault_reap.scrubbed,
        process_reap_valid
    ));
    serial::format(format_args!(
        "AEROS_PROCESSES spawned={} reaped={} highest_pid={} ready={} running={} zombies={} generations={} init_exit={} compiled_exit={} std_exit={} fault_exit={} verified={}\n",
        process_table.spawned,
        process_table.reaped,
        process_table.highest_pid,
        process_table.ready,
        process_table.running,
        process_table.zombies,
        process_generations,
        init_process.exit_code,
        compiled_process.exit_code,
        std_process.exit_code,
        fault_process.exit_code,
        process_reap_valid
    ));
    serial::format(format_args!(
        "AEROS_TMPFS path=/tmp nodes={} directories={} files={} mutable_files={} mutable_bytes={} handles={} verified={}\n",
        runtime_vfs.nodes,
        runtime_vfs.directories,
        runtime_vfs.files,
        runtime_vfs.mutable_files,
        runtime_vfs.mutable_bytes,
        runtime_vfs.open_handles,
        tmpfs_valid
    ));
    serial::format(format_args!(
        "AEROS_SYSCALL abi={:#x} calls={} bootstrap={} linux={} exits={} unknown={} opens={} reads={} writes={} closes={} io_bytes={} clocks={} random_calls={} random_bytes={} compat_calls={} memory_calls={} mmaps={} file_mmaps={} mprotects={} munmaps={} metadata={} seeks={} paths={} resources={} rseq={} futex={} fd_calls={} dup_calls={} last_fd={} signals={} runtime={} directories={} sockets={} datagrams={} network_bytes={} vectored={} positional={} access={} statx={} wall_clock={} sleeps={} chdir={} relative_paths={} polls={} creates={} renames={} removes={} syncs={} truncates={} chmods={} verified={}\n",
        syscall::ABI_VERSION,
        syscall_stats.calls,
        syscall_stats.bootstrap_calls,
        syscall_stats.linux_calls,
        syscall_stats.exits,
        syscall_stats.unknown,
        syscall_stats.opens,
        syscall_stats.reads,
        syscall_stats.writes,
        syscall_stats.closes,
        syscall_stats.io_bytes,
        syscall_stats.clock_calls,
        syscall_stats.random_calls,
        syscall_stats.random_bytes,
        syscall_stats.compat_calls,
        syscall_stats.memory_calls,
        syscall_stats.mmaps,
        syscall_stats.file_mmaps,
        syscall_stats.mprotects,
        syscall_stats.munmaps,
        syscall_stats.metadata_calls,
        syscall_stats.seek_calls,
        syscall_stats.path_calls,
        syscall_stats.resource_calls,
        syscall_stats.rseq_calls,
        syscall_stats.futex_calls,
        syscall_stats.fd_calls,
        syscall_stats.dup_calls,
        syscall_stats.last_open_fd,
        syscall_stats.signal_calls,
        syscall_stats.runtime_calls,
        syscall_stats.directory_calls,
        syscall_stats.socket_calls,
        syscall_stats.datagrams,
        syscall_stats.network_bytes,
        syscall_stats.vectored_calls,
        syscall_stats.positional_calls,
        syscall_stats.access_calls,
        syscall_stats.statx_calls,
        syscall_stats.wall_clock_calls,
        syscall_stats.sleep_calls,
        syscall_stats.chdir_calls,
        syscall_stats.relative_path_calls,
        syscall_stats.poll_calls,
        syscall_stats.create_calls,
        syscall_stats.rename_calls,
        syscall_stats.remove_calls,
        syscall_stats.sync_calls,
        syscall_stats.truncate_calls,
        syscall_stats.chmod_calls,
        syscall_stats.calls == 161
            && syscall_stats.bootstrap_calls == 2
            && syscall_stats.linux_calls == 159
            && syscall_stats.exits == 4
            && syscall_stats.unknown == 0
            && syscall_stats.opens == 13
            && syscall_stats.reads == 12
            && syscall_stats.writes == 10
            && syscall_stats.closes == 17
            && syscall_stats.io_bytes == 264
            && syscall_stats.clock_calls == 1
            && syscall_stats.random_calls == 1
            && syscall_stats.random_bytes == 32
            && syscall_stats.compat_calls == 73
            && syscall_stats.memory_calls == 15
            && syscall_stats.mmaps == 5
            && syscall_stats.file_mmaps == 1
            && syscall_stats.mprotects == 2
            && syscall_stats.munmaps == 4
            && syscall_stats.metadata_calls == 9
            && syscall_stats.seek_calls == 2
            && syscall_stats.path_calls == 12
            && syscall_stats.resource_calls == 1
            && syscall_stats.rseq_calls == 1
            && syscall_stats.futex_calls == 1
            && syscall_stats.fd_calls == 10
            && syscall_stats.dup_calls == 4
            && syscall_stats.last_open_fd == 3
            && syscall_stats.signal_calls == 12
            && syscall_stats.runtime_calls == 8
            && syscall_stats.directory_calls == 1
            && syscall_stats.socket_calls == 4
            && syscall_stats.datagrams == 2
            && syscall_stats.network_bytes > 27
            && syscall_stats.vectored_calls == 2
            && syscall_stats.positional_calls == 1
            && syscall_stats.access_calls == 1
            && syscall_stats.statx_calls == 1
            && syscall_stats.wall_clock_calls == 2
            && syscall_stats.sleep_calls == 2
            && syscall_stats.chdir_calls == 1
            && syscall_stats.relative_path_calls == 5
            && syscall_stats.poll_calls == 1
            && syscall_stats.create_calls == 2
            && syscall_stats.rename_calls == 2
            && syscall_stats.remove_calls == 5
            && syscall_stats.sync_calls == 2
            && syscall_stats.truncate_calls == 1
            && syscall_stats.chmod_calls == 1
    ));
    if !user_probe.verified
        || !init_process.verified
        || !process_stack.verified
        || !compiled_image.self_test()
        || !compiled_stack.verified
        || !compiled_process.verified
        || !std_image.self_test()
        || !std_stack.verified
        || !std_process.verified
        || !fault_process.verified
        || !process_reap_valid
        || !tmpfs_valid
        || !fault_image.self_test()
        || fault_stats.faults != 1
        || fault_stats.last_vector != 6
        || syscall_stats.calls != 161
        || syscall_stats.bootstrap_calls != 2
        || syscall_stats.linux_calls != 159
        || syscall_stats.exits != 4
        || syscall_stats.unknown != 0
        || syscall_stats.opens != 13
        || syscall_stats.reads != 12
        || syscall_stats.writes != 10
        || syscall_stats.closes != 17
        || syscall_stats.io_bytes != 264
        || syscall_stats.clock_calls != 1
        || syscall_stats.random_calls != 1
        || syscall_stats.random_bytes != 32
        || syscall_stats.compat_calls != 73
        || syscall_stats.memory_calls != 15
        || syscall_stats.mmaps != 5
        || syscall_stats.file_mmaps != 1
        || syscall_stats.mprotects != 2
        || syscall_stats.munmaps != 4
        || syscall_stats.metadata_calls != 9
        || syscall_stats.seek_calls != 2
        || syscall_stats.path_calls != 12
        || syscall_stats.resource_calls != 1
        || syscall_stats.rseq_calls != 1
        || syscall_stats.futex_calls != 1
        || syscall_stats.fd_calls != 10
        || syscall_stats.dup_calls != 4
        || syscall_stats.last_open_fd != 3
        || syscall_stats.signal_calls != 12
        || syscall_stats.runtime_calls != 8
        || syscall_stats.directory_calls != 1
        || syscall_stats.socket_calls != 4
        || syscall_stats.datagrams != 2
        || syscall_stats.network_bytes <= 27
        || syscall_stats.vectored_calls != 2
        || syscall_stats.positional_calls != 1
        || syscall_stats.access_calls != 1
        || syscall_stats.statx_calls != 1
        || syscall_stats.wall_clock_calls != 2
        || syscall_stats.sleep_calls != 2
        || syscall_stats.chdir_calls != 1
        || syscall_stats.relative_path_calls != 5
        || syscall_stats.poll_calls != 1
        || syscall_stats.create_calls != 2
        || syscall_stats.rename_calls != 2
        || syscall_stats.remove_calls != 5
        || syscall_stats.sync_calls != 2
        || syscall_stats.truncate_calls != 1
        || syscall_stats.chmod_calls != 1
    {
        serial::line("AEROS_USER_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    serial::format(format_args!(
        "AEROS_PCI devices={} dropped={} storage={} network={} display={} usb={} bridges={} virtio={} capabilities={} verified={}\n",
        pci_summary.devices,
        pci_summary.dropped,
        pci_summary.storage,
        pci_summary.network,
        pci_summary.display,
        pci_summary.usb,
        pci_summary.bridges,
        pci_summary.virtio,
        pci_summary.capabilities,
        pci_summary.verified
    ));
    pci.log_devices();
    if !pci_summary.verified {
        serial::line("AEROS_PCI_INVARIANT_FAILURE");
        arch::halt_forever();
    }
    serial::format(format_args!(
        "AEROS_AHCI present={} abar={:#x} version={:#x} implemented={} active={} sata={} disks={} slots={} dma64={} identify={} read={} write_probe={} sectors={} sector_bytes={} boot_crc32={:08x} model={} verified={}\n",
        ahci.present,
        ahci.abar,
        ahci.version,
        ahci.implemented_ports,
        ahci.active_ports,
        ahci.sata_devices,
        ahci.disks,
        ahci.command_slots,
        ahci.dma64,
        ahci.identify,
        ahci.read,
        ahci.write_probe,
        ahci.sectors,
        ahci.sector_bytes,
        ahci.boot_crc32,
        ahci.model(),
        ahci.verified
    ));
    serial::format(format_args!(
        "AEROS_NVME present={} base={:#x} version={:#x} max_queue={} identify={} io_queue={} read={} write_probe={} sectors={} sector_bytes={} model={} verified={}\n",
        nvme.present,
        nvme.base,
        nvme.version,
        nvme.queue_entries_max,
        nvme.identify,
        nvme.io_queue,
        nvme.read,
        nvme.write_probe,
        nvme.sectors,
        nvme.sector_bytes,
        core::str::from_utf8(&nvme.model[..nvme.model_length]).unwrap_or("?"),
        nvme.verified
    ));
    #[cfg(feature = "boot-test")]
    if nvme.present && !nvme.verified {
        serial::line("AEROS_NVME_INVARIANT_FAILURE");
        arch::halt_forever();
    }
    serial::format(format_args!(
        "AEROS_AC97 present={} nam={:#x} nabm={:#x} codec_ready={} buffers_played={} verified={}
",
        audio.present,
        audio.nam,
        audio.nabm,
        audio.codec_ready,
        audio.buffers_played,
        audio.verified
    ));
    #[cfg(feature = "boot-test")]
    if audio.present && !audio.verified {
        serial::line("AEROS_AC97_INVARIANT_FAILURE");
        arch::halt_forever();
    }
    serial::format(format_args!(
        "AEROS_HDA present={} base={:#x} codecs={} dac={} pin={} path_found={} bytes_played={} verified={}
",
        hda_audio.present, hda_audio.base, hda_audio.codecs, hda_audio.dac, hda_audio.pin, hda_audio.path_found, hda_audio.bytes_played, hda_audio.verified
    ));
    #[cfg(feature = "boot-test")]
    if hda_audio.present && !hda_audio.verified {
        serial::line("AEROS_HDA_INVARIANT_FAILURE");
        arch::halt_forever();
    }
    serial::format(format_args!(
        "AEROS_XHCI present={} base={:#x} version={:#x} ports={} slots={} devices={} keyboards={} mice={} tablets={} hubs={} disks={} disk_sectors={} disk_read={} disk_write={} verified={}\n",
        usb.present,
        usb.base,
        usb.version,
        usb.ports,
        usb.slots,
        usb.devices,
        usb.keyboards,
        usb.mice,
        usb.tablets,
        usb.hubs,
        usb.disks,
        usb.disk_sectors,
        usb.disk_read,
        usb.disk_write,
        usb.verified
    ));
    #[cfg(feature = "boot-test")]
    if usb.present && !usb.verified {
        serial::line("AEROS_XHCI_INVARIANT_FAILURE");
        arch::halt_forever();
    }
    serial::format(format_args!(
        "AEROS_SDHCI present={} mmio={:#x} spec={} card={} sdhc={} sectors={} read={} write_probe={} verified={}\n",
        sd.present,
        sd.mmio,
        sd.version,
        sd.card,
        sd.high_capacity,
        sd.sectors,
        sd.read,
        sd.write_probe,
        sd.verified
    ));
    #[cfg(feature = "boot-test")]
    if sd.present && sd.card && !sd.verified {
        serial::line("AEROS_SDHCI_INVARIANT_FAILURE");
        arch::halt_forever();
    }
    // Filesystem engine self-test on a scratch region of the USB (or SD) test disk.
    #[cfg(feature = "boot-test")]
    {
        let scratch = if usb.disks > 0 {
            Some((fatfs::Disk::Usb, xhci::storage_sectors()))
        } else if sd.card {
            Some((fatfs::Disk::Sd, sdhci::sectors()))
        } else {
            None
        };
        if let Some((disk, sectors)) = scratch {
            let result = fatfs::self_test(disk, 64, sectors - 64);
            serial::format(format_args!(
                "AEROS_FATFS formatted={} fat32={} directories={} long_names={} big_file={} rename_move={} truncate={} delete_frees={} remount={} verified={}\n",
                result.formatted,
                result.fat32,
                result.directories,
                result.long_names,
                result.big_file,
                result.rename_move,
                result.truncate,
                result.delete_frees,
                result.remount,
                result.verified
            ));
            if !result.verified {
                serial::line("AEROS_FATFS_INVARIANT_FAILURE");
                arch::halt_forever();
            }
        }
    }
    #[cfg(feature = "boot-test")]
    if usb.disks > 0 {
        let media = datafs::media_self_test();
        serial::format(format_args!(
            "AEROS_MEDIA_VFS mounted={} listed={} data={} write={} verified={}\n",
            media.mounted, media.listed, media.data, media.write, media.verified
        ));
        if !media.verified {
            serial::line("AEROS_MEDIA_INVARIANT_FAILURE");
            arch::halt_forever();
        }
    }
    #[cfg(feature = "boot-test")]
    {
        let snapshot = sysmon::snapshot();
        let sane = snapshot.cpu_count >= 1
            && snapshot.memory_total > 0
            && snapshot.memory_used + snapshot.memory_free == snapshot.memory_total
            && snapshot.cpu_permille <= 1000
            && snapshot.heap_used <= snapshot.heap_total
            && snapshot.process_count >= 1
            && snapshot.processes[0].name() == "AerOS Desktop";
        serial::format(format_args!(
            "AEROS_SYSMON cpus={} memory_mib={} used_mib={} heap_kib={} processes={} verified={}\n",
            snapshot.cpu_count,
            snapshot.memory_total / 1024 / 1024,
            snapshot.memory_used / 1024 / 1024,
            snapshot.heap_total / 1024,
            snapshot.process_count,
            sane
        ));
        if !sane {
            serial::line("AEROS_SYSMON_INVARIANT_FAILURE");
            arch::halt_forever();
        }
    }
    #[cfg(feature = "boot-test")]
    {
        let zones = timezone::self_test();
        serial::format(format_args!(
            "AEROS_TIMEZONE zones={} verified={}\n",
            timezone::ZONES.len(),
            zones.verified
        ));
        if !zones.verified {
            serial::line("AEROS_TIMEZONE_INVARIANT_FAILURE");
            arch::halt_forever();
        }
        // Network time: needs the internet, so an unreachable server isn't a failure.
        let before = rtc::unix_seconds();
        let sync = ntp::sync();
        let drift = if sync.synced {
            sync.unix_seconds as i64 - before as i64
        } else {
            0
        };
        serial::format(format_args!(
            "AEROS_NTP result={} drift_seconds={}\n",
            if sync.synced { "ok" } else { "unavailable" },
            drift
        ));
        if sync.synced && drift.abs() > 30 {
            serial::line("AEROS_NTP_INVARIANT_FAILURE");
            arch::halt_forever();
        }
    }
    serial::format(format_args!(
        "AEROS_VIRTIO_BLK present={} io={:#x} sectors={} read={} write_probe={} verified={}
",
        virtio_disk.present,
        virtio_disk.io,
        virtio_disk.sectors,
        virtio_disk.read,
        virtio_disk.write_probe,
        virtio_disk.verified
    ));
    serial::format(format_args!(
        "AEROS_VIRTIO_NET present={} io={:#x} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} arp_reply={} verified={}
",
        virtio_nic.present,
        virtio_nic.io,
        virtio_nic.mac[0],
        virtio_nic.mac[1],
        virtio_nic.mac[2],
        virtio_nic.mac[3],
        virtio_nic.mac[4],
        virtio_nic.mac[5],
        virtio_nic.arp_reply,
        virtio_nic.verified
    ));
    serial::format(format_args!(
        "AEROS_VIRTIO_INPUT present={} queue_ready={} abs_x_span={} abs_y_span={} verified={}\n",
        virtio_touch.present,
        virtio_touch.queue_ready,
        virtio_touch.abs_x_span,
        virtio_touch.abs_y_span,
        virtio_touch.verified
    ));
    serial::format(format_args!(
        "AEROS_VIRTIO_GPU present={} queue_ready={} display_width={} display_height={} resource_created={} backing_attached={} transfer_ok={} flush_ok={} verified={}\n",
        virtio_gpu.present,
        virtio_gpu.queue_ready,
        virtio_gpu.display_width,
        virtio_gpu.display_height,
        virtio_gpu.resource_created,
        virtio_gpu.backing_attached,
        virtio_gpu.transfer_ok,
        virtio_gpu.flush_ok,
        virtio_gpu.verified
    ));
    #[cfg(feature = "boot-test")]
    if (virtio_disk.present && !virtio_disk.verified)
        || (virtio_nic.present && !virtio_nic.verified)
        || !virtio_touch.verified
        || !virtio_gpu.verified
    {
        serial::line("AEROS_VIRTIO_INVARIANT_FAILURE");
        arch::halt_forever();
    }
    serial::format(format_args!(
        "AEROS_INSTALL attempted={} target_disk={} target_sectors={} target_blank={} existing_table={} preserved={} refusal={:?} stage={} source_bytes={} gpt={} formatted={} kernel_written={} marker_written={} readback_ok={} verified={}\n",
        install.attempted,
        install.target_disk,
        install.target_sectors,
        install.target_blank,
        install.existing_table,
        install.preserved,
        install.refusal,
        install.stage,
        install.source_bytes,
        install.gpt,
        install.formatted,
        install.kernel_written,
        install.marker_written,
        install.readback_ok,
        install.verified,
    ));
    #[cfg(feature = "boot-test")]
    {
        let verified = installer::self_test();
        serial::format(format_args!("AEROS_INSTALLER verified={}\n", verified));
        if !verified {
            serial::line("AEROS_INSTALLER_INVARIANT_FAILURE");
            arch::halt_forever();
        }
    }
    if !ahci.verified {
        serial::line("AEROS_AHCI_INVARIANT_FAILURE");
        arch::halt_forever();
    }
    serial::format(format_args!(
        "AEROS_PARTITIONS mbr={} gpt={} protective={} count={} fat={} bootable={} first_lba={} covered_sectors={} verified={}\n",
        partitions.mbr,
        partitions.gpt,
        partitions.protective,
        partitions.partitions,
        partitions.fat_candidates,
        partitions.bootable,
        partitions.first_lba,
        partitions.covered_sectors,
        partitions.verified
    ));
    if !partitions.verified {
        serial::line("AEROS_PARTITION_INVARIANT_FAILURE");
        arch::halt_forever();
    }
    serial::format(format_args!(
        "AEROS_FAT mounted={} bits={} partition_lba={} volume_sectors={} sectors_per_cluster={} clusters={} fat_sectors={} root_entries={} efi={} boot={} bootx64={} bootx64_bytes={} pe={} verified={}\n",
        fat.mounted,
        fat.fat_bits,
        fat.partition_lba,
        fat.volume_sectors,
        fat.sectors_per_cluster,
        fat.clusters,
        fat.fat_sectors,
        fat.root_entries,
        fat.efi_directory,
        fat.boot_directory,
        fat.boot_file,
        fat.boot_file_bytes,
        fat.pe_image,
        fat.verified
    ));
    if !fat.verified {
        serial::line("AEROS_FAT_INVARIANT_FAILURE");
        arch::halt_forever();
    }
    let fat_write_payload = b"AerOS persistent storage self-test payload\n";
    let fat_write_supported = fat.fat_bits == 16 || fat.fat_bits == 32;
    let fat_write_created =
        fat_write_supported && fat::write_root_file(b"AEROSFS TXT", fat_write_payload);
    let mut fat_read_buffer = [0u8; 128];
    let fat_read_len = if fat_write_created {
        fat::read_root_file(b"AEROSFS TXT", &mut fat_read_buffer).unwrap_or(0)
    } else {
        0
    };
    let fat_roundtrip = fat_read_len == fat_write_payload.len()
        && fat_read_buffer[..fat_read_len] == *fat_write_payload;
    let fat_write_verified = !fat_write_supported || (fat_write_created && fat_roundtrip);
    serial::format(format_args!(
        "AEROS_FAT_WRITE supported={} created={} bytes={} roundtrip={} verified={}\n",
        fat_write_supported,
        fat_write_created,
        fat_write_payload.len(),
        fat_roundtrip,
        fat_write_verified
    ));
    if !fat_write_verified {
        serial::line("AEROS_FAT_WRITE_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let persist_payload = b"vfs persistence self-test\n";
    let persist_created = vfs::open_file("/data/PERSIST.TXT", true, true, false, 0o644, true).ok();
    let persist_written = match persist_created {
        Some(descriptor) => {
            let written =
                vfs::write(descriptor, persist_payload, false) == Ok(persist_payload.len());
            let _ = vfs::close(descriptor);
            written
        }
        None => false,
    };
    let mut persist_read_buffer = [0u8; 64];
    let persist_readback = if persist_written {
        match vfs::open_file("/data/PERSIST.TXT", false, false, false, 0, false) {
            Ok(descriptor) => {
                let read_len = vfs::read(descriptor, &mut persist_read_buffer).unwrap_or(0);
                let _ = vfs::close(descriptor);
                read_len == persist_payload.len()
                    && persist_read_buffer[..read_len] == *persist_payload
            }
            Err(_) => false,
        }
    } else {
        false
    };
    let mut persist_disk_buffer = [0u8; 64];
    let persist_disk_len =
        fat::read_root_file(b"PERSIST TXT", &mut persist_disk_buffer).unwrap_or(0);
    let persist_on_disk = persist_disk_len == persist_payload.len()
        && persist_disk_buffer[..persist_disk_len] == *persist_payload;
    let persist_verified = persist_written && persist_readback && persist_on_disk;
    serial::format(format_args!(
        "AEROS_VFS_PERSIST created={} written={} readback={} on_disk={} verified={}\n",
        persist_created.is_some(),
        persist_written,
        persist_readback,
        persist_on_disk,
        persist_verified
    ));
    if !persist_verified {
        serial::line("AEROS_VFS_PERSIST_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    // Proves cross-reboot rediscovery, not just same-boot create+read: if a
    // prior boot already created and left this directory, load_persisted_
    // files() (run once, early, during vfs::initialize()) will have already
    // recreated it as a VFS node by the time this runs - so success here
    // *without* this boot creating it first is the actual evidence.
    let dir_rediscovered = vfs::open_directory("/data/TESTDIR").is_ok();
    let dir_create_result = if dir_rediscovered {
        Ok(())
    } else {
        vfs::create_directory("/data/TESTDIR", 0o755)
    };
    let dir_present = dir_create_result.is_ok() || dir_rediscovered;
    let mut dir_listing = [fat::FatFileEntry::EMPTY; 4];
    let dir_listing_count = fat::list_root_directories(&mut dir_listing);
    let dir_on_disk = dir_listing[..dir_listing_count]
        .iter()
        .any(|entry| entry.name == *b"TESTDIR    ");
    let dir_verified = dir_present && dir_on_disk;
    serial::format(format_args!(
        "AEROS_VFS_PERSIST_DIR rediscovered={} present={} on_disk={} verified={}\n",
        dir_rediscovered, dir_present, dir_on_disk, dir_verified
    ));
    if !dir_verified {
        serial::line("AEROS_VFS_PERSIST_DIR_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let pwrite_created = vfs::open_file("/tmp/PWRITE_TEST", true, true, false, 0o644, true).ok();
    let pwrite_verified = match pwrite_created {
        Some(descriptor) => {
            let write_ok = vfs::write_at(descriptor, 5, b"XYZ") == Ok(3);
            let mut buffer = [0u8; 8];
            let read_len = vfs::read_at(descriptor, 0, &mut buffer).unwrap_or(0);
            let _ = vfs::close(descriptor);
            write_ok && read_len == buffer.len() && buffer == *b"\0\0\0\0\0XYZ"
        }
        None => false,
    };
    serial::format(format_args!(
        "AEROS_PWRITE created={} verified={}\n",
        pwrite_created.is_some(),
        pwrite_verified
    ));
    if !pwrite_verified {
        serial::line("AEROS_PWRITE_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let pipe_verified = syscall::pipe_self_test();
    serial::format(format_args!("AEROS_PIPE verified={}\n", pipe_verified));
    if !pipe_verified {
        serial::line("AEROS_PIPE_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    #[cfg(feature = "boot-test")]
    {
        let tcp = tcp::self_test();
        serial::format(format_args!(
            "AEROS_TCP_ENGINE clean={} lossy={} burst_loss={} slow_reader={} refused={} unreachable={} hostile={} sack_receiver={} sack_recovery={} retransmits={} fast_retransmits={} peak_cwnd={} verified={}
",
            tcp.clean,
            tcp.lossy,
            tcp.burst_loss,
            tcp.slow_reader,
            tcp.refused,
            tcp.unreachable,
            tcp.hostile,
            tcp.sack_receiver,
            tcp.sack_recovery,
            tcp.retransmits,
            tcp.fast_retransmits,
            tcp.peak_cwnd,
            tcp.verified
        ));
        if !tcp.verified {
            serial::line("AEROS_TCP_ENGINE_INVARIANT_FAILURE");
            arch::halt_forever();
        }
    }
    #[cfg(feature = "boot-test")]
    {
        let pkg_verified = pkg::self_test();
        serial::format(format_args!(
            "AEROS_PKG verified={}
",
            pkg_verified
        ));
        if !pkg_verified {
            serial::line("AEROS_PKG_INVARIANT_FAILURE");
            arch::halt_forever();
        }
    }
    #[cfg(feature = "boot-test")]
    {
        let table_verified = udp::self_test();
        let socket_verified = syscall::udp_socket_self_test();
        serial::format(format_args!(
            "AEROS_UDP table={} sockets={} verified={}\n",
            table_verified,
            socket_verified,
            table_verified && socket_verified
        ));
        if !(table_verified && socket_verified) {
            serial::line("AEROS_UDP_INVARIANT_FAILURE");
            arch::halt_forever();
        }
    }
    let maps_verified = arch::paging::regions_self_test(&paging);
    serial::format(format_args!("AEROS_PROC_MAPS verified={}\n", maps_verified));
    if !maps_verified {
        serial::line("AEROS_PROC_MAPS_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    #[cfg(feature = "boot-test")]
    {
        let report = block::self_test();
        serial::format(format_args!(
            "AEROS_BLOCK fifo_seek={} elevator_seek={} fifo_commands={} elevator_commands={} merged={} barrier={} hazard={} intact={} cache_hit={} write_through={} failed_write={} eviction={} readahead={} reclaimed={} verified={}\n",
            report.fifo_seek,
            report.elevator_seek,
            report.fifo_commands,
            report.elevator_commands,
            report.merged,
            report.barrier_order,
            report.hazard_order,
            report.data_intact,
            report.cache_hit,
            report.write_through,
            report.failed_write,
            report.eviction,
            report.readahead,
            report.reclaimed,
            report.verified
        ));
        if !report.verified {
            serial::line("AEROS_BLOCK_INVARIANT_FAILURE");
            arch::halt_forever();
        }
    }
    #[cfg(feature = "boot-test")]
    {
        let report = smpsched::self_test();
        serial::format(format_args!(
            "AEROS_SMP_SCHED cpus={} placement={} stealing={} priority_order={} affinity={} refused={} balanced={} work_stolen={} steals={} verified={}\n",
            report.cpus,
            report.placement,
            report.stealing,
            report.priority_order,
            report.affinity,
            report.refused,
            report.balanced,
            report.work_stolen,
            smpsched::steals(),
            report.verified
        ));
        if !report.verified {
            serial::line("AEROS_SMP_SCHED_INVARIANT_FAILURE");
            arch::halt_forever();
        }
    }
    let procfs_verified = procfs::self_test();
    serial::format(format_args!("AEROS_PROCFS verified={}\n", procfs_verified));
    if !procfs_verified {
        serial::line("AEROS_PROCFS_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let tcp_socket_verified = syscall::tcp_socket_self_test();
    serial::format(format_args!(
        "AEROS_TCP_SOCKET verified={}
",
        tcp_socket_verified
    ));
    if !tcp_socket_verified {
        serial::line("AEROS_TCP_SOCKET_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    #[cfg(feature = "boot-test")]
    {
        let verified = syscall::socket_options_self_test();
        serial::format(format_args!("AEROS_SOCKOPT verified={}\n", verified));
        if !verified {
            serial::line("AEROS_SOCKOPT_INVARIANT_FAILURE");
            arch::halt_forever();
        }
    }
    let misc_syscalls_verified = syscall::misc_syscalls_self_test();
    serial::format(format_args!(
        "AEROS_SYSCALL_MISC verified={}\n",
        misc_syscalls_verified
    ));
    if !misc_syscalls_verified {
        serial::line("AEROS_SYSCALL_MISC_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let epoll_verified = syscall::epoll_self_test();
    serial::format(format_args!("AEROS_EPOLL verified={}\n", epoll_verified));
    if !epoll_verified {
        serial::line("AEROS_EPOLL_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let unix_socketpair_verified = syscall::unix_socketpair_self_test();
    serial::format(format_args!(
        "AEROS_UNIX_SOCKETPAIR verified={}\n",
        unix_socketpair_verified
    ));
    if !unix_socketpair_verified {
        serial::line("AEROS_UNIX_SOCKETPAIR_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let unix_domain_socket_verified = syscall::unix_domain_socket_self_test();
    serial::format(format_args!(
        "AEROS_UNIX_DOMAIN_SOCKET verified={}\n",
        unix_domain_socket_verified
    ));
    if !unix_domain_socket_verified {
        serial::line("AEROS_UNIX_DOMAIN_SOCKET_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let rlimit_nofile_verified = syscall::rlimit_nofile_self_test();
    serial::format(format_args!(
        "AEROS_RLIMIT_NOFILE verified={}\n",
        rlimit_nofile_verified
    ));
    if !rlimit_nofile_verified {
        serial::line("AEROS_RLIMIT_NOFILE_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    #[cfg(feature = "boot-test")]
    {
        let crash = fat_crash::run();
        serial::format(format_args!(
            "AEROS_FAT_CRASH cases={} elapsed_ms={} dangling={} cross_linked={} short={} torn={} lost={} verified={}\n",
            crash.cases,
            crash.elapsed_ms,
            crash.dangling,
            crash.cross_linked,
            crash.short,
            crash.torn,
            crash.lost,
            crash.verified
        ));
        if !crash.verified {
            serial::line("AEROS_FAT_CRASH_INVARIANT_FAILURE");
            arch::halt_forever();
        }
        let home_crash = fatfs_crash::run();
        serial::format(format_args!(
            "AEROS_FATFS_CRASH cases={} structural={} torn={} leaks_repaired={} repair_failures={} verified={}\n",
            home_crash.cases,
            home_crash.structural,
            home_crash.torn,
            home_crash.leaks_repaired,
            home_crash.repair_failures,
            home_crash.verified
        ));
        if !home_crash.verified {
            serial::line("AEROS_FATFS_CRASH_INVARIANT_FAILURE");
            arch::halt_forever();
        }
        let fuzz = fuzz::run();
        serial::format(format_args!(
            "AEROS_FUZZ iterations={} elapsed_ms={} verified={}\n",
            fuzz.iterations, fuzz.elapsed_ms, fuzz.verified
        ));
    }
    let oom_verified = oom::self_test();
    serial::format(format_args!(
        "AEROS_OOM pressure={} verified={}\n",
        oom::pressure().label(),
        oom_verified
    ));
    if !oom_verified {
        serial::line("AEROS_OOM_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let services_verified = services::self_test();
    serial::format(format_args!(
        "AEROS_SERVICES verified={}\n",
        services_verified
    ));
    if !services_verified {
        serial::line("AEROS_SERVICES_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let kernel_log_verified = serial::log_self_test();
    serial::format(format_args!(
        "AEROS_KERNEL_LOG verified={}\n",
        kernel_log_verified
    ));
    if !kernel_log_verified {
        serial::line("AEROS_KERNEL_LOG_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let ed25519_started = time::monotonic_nanoseconds();
    let ed25519_verified = ed25519::self_test();
    serial::format(format_args!(
        "AEROS_ED25519 vectors=3 elapsed_ms={} verified={}\n",
        (time::monotonic_nanoseconds() - ed25519_started) / 1_000_000,
        ed25519_verified
    ));
    if !ed25519_verified {
        serial::line("AEROS_ED25519_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let statfs_verified = syscall::statfs_self_test();
    serial::format(format_args!("AEROS_STATFS verified={}\n", statfs_verified));
    if !statfs_verified {
        serial::line("AEROS_STATFS_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let select_verified = syscall::select_self_test();
    serial::format(format_args!("AEROS_SELECT verified={}\n", select_verified));
    if !select_verified {
        serial::line("AEROS_SELECT_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let socket_api_verified = syscall::socket_helpers_self_test();
    serial::format(format_args!(
        "AEROS_SOCKET_API verified={}\n",
        socket_api_verified
    ));
    if !socket_api_verified {
        serial::line("AEROS_SOCKET_API_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let symlink_syscall_verified = syscall::symlink_syscall_self_test();
    serial::format(format_args!(
        "AEROS_SYMLINK_SYSCALL verified={}\n",
        symlink_syscall_verified
    ));
    if !symlink_syscall_verified {
        serial::line("AEROS_SYMLINK_SYSCALL_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let process_group_verified = syscall::process_group_self_test();
    serial::format(format_args!(
        "AEROS_PROCESS_GROUP verified={}\n",
        process_group_verified
    ));
    if !process_group_verified {
        serial::line("AEROS_PROCESS_GROUP_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let sysinfo_verified = syscall::sysinfo_self_test();
    serial::format(format_args!(
        "AEROS_SYSINFO verified={}\n",
        sysinfo_verified
    ));
    if !sysinfo_verified {
        serial::line("AEROS_SYSINFO_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    serial::format(format_args!(
        "AEROS_NETWORK driver={} present={} mmio={:#x} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} link={} speed_mbps={} duplex={} tx={} rx={} rx_bytes={} arp_gateway={} verified={}\n",
        network.driver,
        network.present,
        network.mmio,
        network.mac[0],
        network.mac[1],
        network.mac[2],
        network.mac[3],
        network.mac[4],
        network.mac[5],
        network.link,
        network.speed_mbps,
        network.full_duplex,
        network.tx,
        network.rx,
        network.rx_bytes,
        network.arp_reply,
        network.verified
    ));
    if !network.verified {
        serial::line("AEROS_NETWORK_DEGRADED");
    }
    for (name, nic_report) in [("RTL8139", &rtl8139_nic), ("RTL8168", &rtl8168_nic)] {
        serial::format(format_args!(
            "AEROS_{} present={} mmio={:#x} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} link={} tx={} arp_reply={} gateway={}.{}.{}.{} verified={}\n",
            name,
            nic_report.present,
            nic_report.mmio,
            nic_report.mac[0],
            nic_report.mac[1],
            nic_report.mac[2],
            nic_report.mac[3],
            nic_report.mac[4],
            nic_report.mac[5],
            nic_report.link,
            nic_report.tx,
            nic_report.arp_reply,
            nic_report.gateway[0],
            nic_report.gateway[1],
            nic_report.gateway[2],
            nic_report.gateway[3],
            nic_report.verified
        ));
        #[cfg(feature = "boot-test")]
        if nic_report.present && !nic_report.verified {
            serial::format(format_args!("AEROS_{}_INVARIANT_FAILURE\n", name));
            arch::halt_forever();
        }
    }
    serial::format(format_args!(
        "AEROS_DHCP discover={} request={} ack={} address={}.{}.{}.{} gateway={}.{}.{}.{} dns={}.{}.{}.{} lease_seconds={} verified={}\n",
        internet.dhcp_discover,
        internet.dhcp_request,
        internet.dhcp_ack,
        internet.local_ip[0],
        internet.local_ip[1],
        internet.local_ip[2],
        internet.local_ip[3],
        internet.gateway_ip[0],
        internet.gateway_ip[1],
        internet.gateway_ip[2],
        internet.gateway_ip[3],
        internet.dns_ip[0],
        internet.dns_ip[1],
        internet.dns_ip[2],
        internet.dns_ip[3],
        internet.lease_seconds,
        internet.dhcp_verified
    ));
    serial::format(format_args!(
        "AEROS_IPV4 local={}.{}.{}.{} gateway={}.{}.{}.{} tx={} rx={} reply_bytes={} ip_checksum={} icmp_checksum={} echo_reply={} verified={}\n",
        internet.local_ip[0],
        internet.local_ip[1],
        internet.local_ip[2],
        internet.local_ip[3],
        internet.gateway_ip[0],
        internet.gateway_ip[1],
        internet.gateway_ip[2],
        internet.gateway_ip[3],
        internet.ipv4_tx,
        internet.ipv4_rx,
        internet.reply_bytes,
        internet.header_checksum,
        internet.icmp_checksum,
        internet.echo_reply,
        internet.verified
    ));
    serial::format(format_args!(
        "AEROS_UDP_DNS server={}.{}.{}.{} tx={} rx={} udp_checksum={} response={} answers={} address={}.{}.{}.{} verified={}\n",
        internet.dns_ip[0],
        internet.dns_ip[1],
        internet.dns_ip[2],
        internet.dns_ip[3],
        internet.udp_tx,
        internet.udp_rx,
        internet.udp_checksum,
        internet.dns_response,
        internet.dns_answers,
        internet.dns_address[0],
        internet.dns_address[1],
        internet.dns_address[2],
        internet.dns_address[3],
        internet.dns_verified
    ));
    if !internet.dhcp_verified || !internet.verified {
        serial::line("AEROS_IPV4_DEGRADED");
    }
    if !internet.dns_verified {
        serial::line("AEROS_DNS_DEGRADED");
    }
    let ipv6 = net::ipv6_self_test(&network);
    serial::format(format_args!(
        "AEROS_IPV6 link_local={:x?} router_solicited={} router_advertised={} router_ip={:x?} neighbor_solicited={} neighbor_resolved={} echo_tx={} echo_rx={} prefix_found={} global={:x?} global_echo_rx={} verified={}\n",
        ipv6.link_local,
        ipv6.router_solicited,
        ipv6.router_advertised,
        ipv6.router_ip,
        ipv6.neighbor_solicited,
        ipv6.neighbor_resolved,
        ipv6.echo_tx,
        ipv6.echo_rx,
        ipv6.prefix_found,
        ipv6.global_address,
        ipv6.global_echo_rx,
        ipv6.verified
    ));
    if !ipv6.verified {
        // Real exchanges over a real network: on a machine without IPv6 the
        // boot goes on and the harness is what insists on success.
        serial::line("AEROS_IPV6_DEGRADED");
    }
    #[cfg(feature = "boot-test")]
    {
        let offline = ip::self_test() && ipv6::offline_self_test();
        let sockets = syscall::inet6_socket_self_test();
        serial::format(format_args!(
            "AEROS_IPV6_OFFLINE addresses_and_frames={} sockets={} verified={}\n",
            offline,
            sockets,
            offline && sockets
        ));
        if !(offline && sockets) {
            serial::line("AEROS_IPV6_OFFLINE_INVARIANT_FAILURE");
            arch::halt_forever();
        }
        tcpnet::reset();
        if ipv6.verified {
            let mut gateway = ipv6.global_address;
            gateway[8..].fill(0);
            gateway[15] = 2;
            let mut response = [0u8; 8192];
            let http = net::http_get_port(gateway, 18_081, "[fec0::2]", "/big6", &mut response);
            let body_start = response[..http.bytes]
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map_or(0, |position| position + 4);
            let body = &response[body_start..http.bytes];
            let body_ok = body.len() == 6_000
                && body
                    .iter()
                    .enumerate()
                    .all(|(index, byte)| *byte == b'a' + (index % 26) as u8);
            serial::format(format_args!(
                "AEROS_HTTP6 connected={} status={} bytes={} body_ok={} verified={}\n",
                http.connected,
                http.status,
                http.bytes,
                body_ok,
                http.connected && http.status == 200 && body_ok
            ));
            if !(http.connected && http.status == 200 && body_ok) {
                serial::line("AEROS_HTTP6_INVARIANT_FAILURE");
                arch::halt_forever();
            }
            tcpnet::reset();
            let aaaa = net::resolve6("localhost");
            serial::format(format_args!(
                "AEROS_DNS6 found={} address={:x?}\n",
                aaaa.is_some(),
                aaaa.unwrap_or([0; 16])
            ));
        }
    }
    serial::format(format_args!(
        "AEROS_SCHEDULER tasks={} ready={} running={} exited={} switches={} stack_bytes={} fpu_tasks={} fpu_bytes={} fpu_switches={} fpu_isolation={} highest_id={} verified={}\n",
        scheduler_stats.tasks,
        scheduler_stats.ready,
        scheduler_stats.running,
        scheduler_stats.exited,
        scheduler_stats.context_switches,
        scheduler_stats.stack_bytes,
        scheduler_stats.fpu_tasks,
        scheduler_stats.fpu_bytes,
        scheduler_stats.fpu_switches,
        scheduler_stats.fpu_isolation,
        scheduler_stats.highest_task_id,
        scheduler_valid && scheduler_stats.fpu_isolation
    ));
    if !scheduler_valid || !scheduler_stats.fpu_isolation {
        serial::line("AEROS_SCHEDULER_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    serial::format(format_args!(
        "AEROS_PREEMPT source=lapic ticks={} work_a={} work_b={} fpu_isolation={} verified={}\n",
        preemption_ticks,
        preempt_work_a,
        preempt_work_b,
        preempt_fpu_isolation,
        preemption_valid && preempt_fpu_isolation
    ));
    if !preemption_valid || !preempt_fpu_isolation {
        serial::line("AEROS_PREEMPT_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    serial::format(format_args!(
        "AEROS_PIPE_BLOCKING verified={}\n",
        pipe_blocking_valid
    ));
    if !pipe_blocking_valid {
        serial::line("AEROS_PIPE_BLOCKING_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let concurrent_user = scheduler::concurrent_user_self_test(&paging, &mut frames);
    serial::format(format_args!(
        "AEROS_CONCURRENT_USER exit_a={} exit_b={} switches={} reaped={} verified={}\n",
        concurrent_user.exit_a,
        concurrent_user.exit_b,
        concurrent_user.switches,
        concurrent_user.reaped,
        concurrent_user.verified
    ));
    if !concurrent_user.verified {
        serial::line("AEROS_CONCURRENT_USER_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let task_exhaustion = scheduler::task_exhaustion_self_test(&paging);
    serial::format(format_args!(
        "AEROS_TASK_EXHAUSTION spawned={} exhausted_cleanly={} reaped={} recovered={} verified={}\n",
        task_exhaustion.spawned_before_full,
        task_exhaustion.exhausted_cleanly,
        task_exhaustion.reaped,
        task_exhaustion.recovered,
        task_exhaustion.verified
    ));
    if !task_exhaustion.verified {
        serial::line("AEROS_TASK_EXHAUSTION_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let fork_result = scheduler::fork_self_test(&paging);
    serial::format(format_args!(
        "AEROS_FORK parent_exit={} child_exit={} parent_stack={:#x} child_stack={:#x} reaped={} verified={}\n",
        fork_result.parent_exit,
        fork_result.child_exit,
        fork_result.parent_stack_value,
        fork_result.child_stack_value,
        fork_result.reaped,
        fork_result.verified
    ));
    if !fork_result.verified {
        serial::line("AEROS_FORK_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let stack_growth_result = scheduler::stack_growth_self_test(&paging);
    serial::format(format_args!(
        "AEROS_STACK_GROWTH grown_exit={} grown_pages={} overflow_exit={} verified={}\n",
        stack_growth_result.grown_exit,
        stack_growth_result.grown_pages,
        stack_growth_result.overflow_exit,
        stack_growth_result.verified
    ));
    if !stack_growth_result.verified {
        serial::line("AEROS_STACK_GROWTH_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let fork_chain_result = scheduler::fork_chain_self_test(&paging);
    serial::format(format_args!(
        "AEROS_FORK_CHAIN exit_a={} exit_b={} exit_c={} reaped={} verified={}\n",
        fork_chain_result.exit_a,
        fork_chain_result.exit_b,
        fork_chain_result.exit_c,
        fork_chain_result.reaped,
        fork_chain_result.verified
    ));
    if !fork_chain_result.verified {
        serial::line("AEROS_FORK_CHAIN_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let exec_result = scheduler::exec_self_test(&paging);
    serial::format(format_args!(
        "AEROS_EXECVE parent_exit={} child_exit={} parent_stack={:#x} child_stack={:#x} reaped={} verified={}\n",
        exec_result.parent_exit,
        exec_result.child_exit,
        exec_result.parent_stack_value,
        exec_result.child_stack_value,
        exec_result.reaped,
        exec_result.verified
    ));
    if !exec_result.verified {
        serial::line("AEROS_EXECVE_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let wait_result = scheduler::wait_self_test(&paging);
    serial::format(format_args!(
        "AEROS_WAIT4 parent_exit={} reaped={} verified={}\n",
        wait_result.parent_exit, wait_result.reaped, wait_result.verified
    ));
    if !wait_result.verified {
        serial::line("AEROS_WAIT4_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let kill_result = scheduler::kill_self_test(&paging);
    serial::format(format_args!(
        "AEROS_KILL parent_exit={} reaped={} verified={}\n",
        kill_result.parent_exit, kill_result.reaped, kill_result.verified
    ));
    if !kill_result.verified {
        serial::line("AEROS_KILL_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let getpid_result = scheduler::getpid_self_test(&paging);
    serial::format(format_args!(
        "AEROS_GETPID parent_pid={} child_getppid={} reaped={} verified={}\n",
        getpid_result.parent_pid_value,
        getpid_result.child_exit,
        getpid_result.reaped,
        getpid_result.verified
    ));
    if !getpid_result.verified {
        serial::line("AEROS_GETPID_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let heap_result = scheduler::heap_self_test(&paging);
    serial::format(format_args!(
        "AEROS_HEAP_SBRK parent_exit={} child_exit={} parent_heap={:#x} child_heap={:#x} reaped={} verified={}\n",
        heap_result.parent_exit,
        heap_result.child_exit,
        heap_result.parent_heap_value,
        heap_result.child_heap_value,
        heap_result.reaped,
        heap_result.verified
    ));
    if !heap_result.verified {
        serial::line("AEROS_HEAP_SBRK_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let write_result = scheduler::write_syscall_self_test(&paging);
    serial::format(format_args!(
        "AEROS_WRITE_SYSCALL exit={} verified={}\n",
        write_result.exit_code, write_result.verified
    ));
    if !write_result.verified {
        serial::line("AEROS_WRITE_SYSCALL_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let getrandom_result = scheduler::getrandom_self_test(&paging);
    serial::format(format_args!(
        "AEROS_GETRANDOM_SYSCALL exit={} distinct={} verified={}\n",
        getrandom_result.exit_code,
        getrandom_result.value_a != getrandom_result.value_b,
        getrandom_result.verified
    ));
    if !getrandom_result.verified {
        serial::line("AEROS_GETRANDOM_SYSCALL_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let fd_isolation_result = scheduler::linux_fd_isolation_self_test(&paging);
    serial::format(format_args!(
        "AEROS_LINUX_FD_ISOLATION exit_a={:#x} exit_b={} verified={}\n",
        fd_isolation_result.exit_a, fd_isolation_result.exit_b, fd_isolation_result.verified
    ));
    if !fd_isolation_result.verified {
        serial::line("AEROS_LINUX_FD_ISOLATION_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let fork_fd_result = scheduler::linux_fork_fd_self_test(&paging);
    serial::format(format_args!(
        "AEROS_LINUX_FORK_FD parent_exit={} child_exit={:#x} reaped={} verified={}\n",
        fork_fd_result.parent_exit,
        fork_fd_result.child_exit,
        fork_fd_result.reaped,
        fork_fd_result.verified
    ));
    if !fork_fd_result.verified {
        serial::line("AEROS_LINUX_FORK_FD_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let brk_fork_result = scheduler::linux_brk_fork_self_test(&paging);
    serial::format(format_args!(
        "AEROS_LINUX_BRK_FORK parent_exit={} child_exit={} parent_value={:#x} child_value={:#x} reaped={} verified={}\n",
        brk_fork_result.parent_exit,
        brk_fork_result.child_exit,
        brk_fork_result.parent_value,
        brk_fork_result.child_value,
        brk_fork_result.reaped,
        brk_fork_result.verified
    ));
    if !brk_fork_result.verified {
        serial::line("AEROS_LINUX_BRK_FORK_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let mmap_fork_result = scheduler::linux_mmap_fork_self_test(&paging);
    serial::format(format_args!(
        "AEROS_LINUX_MMAP_FORK parent_exit={} child_exit={} parent_marker_second={:#x} parent_marker_reused={:#x} child_marker={:#x} reaped={} verified={}\n",
        mmap_fork_result.parent_exit,
        mmap_fork_result.child_exit,
        mmap_fork_result.parent_marker_second,
        mmap_fork_result.parent_marker_reused,
        mmap_fork_result.child_marker,
        mmap_fork_result.reaped,
        mmap_fork_result.verified
    ));
    if !mmap_fork_result.verified {
        serial::line("AEROS_LINUX_MMAP_FORK_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let fd_refcount_result = scheduler::fork_close_refcount_self_test(&paging);
    serial::format(format_args!(
        "AEROS_LINUX_FD_REFCOUNT parent_exit={} reaped={} verified={}\n",
        fd_refcount_result.parent_exit, fd_refcount_result.reaped, fd_refcount_result.verified
    ));
    if !fd_refcount_result.verified {
        serial::line("AEROS_LINUX_FD_REFCOUNT_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let file_mmap_result = scheduler::file_mmap_self_test(&paging);
    serial::format(format_args!(
        "AEROS_LINUX_FILE_MMAP exit={} bytes={:?} reaped={} verified={}\n",
        file_mmap_result.exit_code,
        file_mmap_result.mapped_bytes,
        file_mmap_result.reaped,
        file_mmap_result.verified
    ));
    if !file_mmap_result.verified {
        serial::line("AEROS_LINUX_FILE_MMAP_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    // Every scheduler self-test up to this point only ever spawns a tiny
    // hand-assembled probe; this is the first one to run a REAL,
    // already-loaded multi-segment ELF (the same /bin/aeros-init the
    // boot-time single-shot path above already ran: rust-static-pie, 3
    // real segments, distinct executable/writable pages) through the
    // fork/exec-capable per-process scheduler path instead.
    let real_elf_result = scheduler::real_elf_self_test(&paging, compiled_file.data, 74);
    serial::format(format_args!(
        "AEROS_SCHEDULED_REAL_ELF exit={} reaped={} verified={}\n",
        real_elf_result.exit_code, real_elf_result.reaped, real_elf_result.verified
    ));
    if !real_elf_result.verified {
        serial::line("AEROS_SCHEDULED_REAL_ELF_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    // The big std-linked binary needs a multi-page stack (argv/auxv plus
    // musl start-up); it segfaulted under the scheduler with one page.
    let std_elf_result = scheduler::real_elf_self_test(&paging, std_file.data, 75);
    serial::format(format_args!(
        "AEROS_SCHEDULED_STD_ELF exit={} reaped={} verified={}\n",
        std_elf_result.exit_code, std_elf_result.reaped, std_elf_result.verified
    ));
    if !std_elf_result.verified {
        serial::line("AEROS_SCHEDULED_STD_ELF_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let threads_result = match vfs::file("/bin/aeros-threads") {
        Ok(file) => scheduler::threaded_elf_self_test(&paging, file.data, 76),
        Err(_) => scheduler::threaded_elf_self_test(&paging, &[], 76),
    };
    serial::format(format_args!(
        "AEROS_THREADS exit={} reaped={} verified={}\n",
        threads_result.exit_code, threads_result.reaped, threads_result.verified
    ));
    if !threads_result.verified {
        serial::line("AEROS_THREADS_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let ptrace_result = match vfs::file("/bin/aeros-ptrace") {
        Ok(file) => scheduler::threaded_elf_self_test(&paging, file.data, 77),
        Err(_) => scheduler::threaded_elf_self_test(&paging, &[], 77),
    };
    serial::format(format_args!(
        "AEROS_PTRACE exit={} reaped={} verified={}\n",
        ptrace_result.exit_code, ptrace_result.reaped, ptrace_result.verified
    ));
    if !ptrace_result.verified {
        serial::line("AEROS_PTRACE_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    #[cfg(feature = "boot-test")]
    {
        let before = swap::stats();
        let was_active = swap::active();
        let slots = swap::configure_partition(block::Disk::Virtio, 4096, 12288);
        let mut evicted = 0;
        let result = match vfs::file("/bin/aeros-swap") {
            Ok(file) => scheduler::threaded_elf_self_test_with(&paging, file.data, 88, || {
                evicted += scheduler::swap_out_pages(8)
            }),
            Err(_) => scheduler::threaded_elf_self_test(&paging, &[], 88),
        };
        let stats = swap::stats();
        swap::configure(block::Disk::Virtio, 0, 0);
        let verified = slots >= 1000
            && result.verified
            && evicted > 0
            && stats.written > before.written
            && stats.read > before.read
            && stats.in_use == 0
            && stats.failures == 0;
        serial::format(format_args!(
            "AEROS_SWAP slots={slots} exit={} reaped={} evicted={evicted} written={} read={} in_use={} failures={} verified={verified}\n",
            result.exit_code,
            result.reaped,
            stats.written - before.written,
            stats.read - before.read,
            stats.in_use,
            stats.failures
        ));
        let _ = was_active;
        if !verified {
            serial::line("AEROS_SWAP_INVARIANT_FAILURE");
            arch::halt_forever();
        }
    }
    #[cfg(feature = "boot-test")]
    {
        let result = match vfs::file("/bin/aeros-bench") {
            Ok(file) => scheduler::threaded_elf_self_test(&paging, file.data, 0),
            Err(_) => scheduler::threaded_elf_self_test(&paging, &[], 0),
        };
        serial::format(format_args!(
            "AEROS_BENCH_RESULT exit={} reaped={} verified={}
",
            result.exit_code, result.reaped, result.verified
        ));
        if !result.verified {
            serial::line("AEROS_BENCH_RESULT_INVARIANT_FAILURE");
            arch::halt_forever();
        }
    }
    let big_mmap_result = scheduler::real_elf_self_test(&paging, &scheduler::BIG_MMAP_PROBE, 55);
    serial::format(format_args!(
        "AEROS_BIG_MMAP exit={} reaped={} verified={}\n",
        big_mmap_result.exit_code, big_mmap_result.reaped, big_mmap_result.verified
    ));
    if !big_mmap_result.verified {
        serial::line("AEROS_BIG_MMAP_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let linux_fw_result =
        scheduler::real_elf_self_test(&paging, &scheduler::LINUX_FORK_WAIT_PROBE, 11);
    serial::format(format_args!(
        "AEROS_LINUX_FORK_WAIT exit={} reaped={} verified={}\n",
        linux_fw_result.exit_code, linux_fw_result.reaped, linux_fw_result.verified
    ));
    if !linux_fw_result.verified {
        serial::line("AEROS_LINUX_FORK_WAIT_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let linux_exec_result =
        scheduler::real_elf_self_test(&paging, &scheduler::LINUX_EXECVE_PROBE, 74);
    serial::format(format_args!(
        "AEROS_LINUX_EXECVE exit={} reaped={} verified={}\n",
        linux_exec_result.exit_code, linux_exec_result.reaped, linux_exec_result.verified
    ));
    if !linux_exec_result.verified {
        serial::line("AEROS_LINUX_EXECVE_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let linux_kill_result =
        scheduler::real_elf_self_test(&paging, &scheduler::LINUX_KILL_PROBE, 11);
    serial::format(format_args!(
        "AEROS_LINUX_KILL exit={} reaped={} verified={}\n",
        linux_kill_result.exit_code, linux_kill_result.reaped, linux_kill_result.verified
    ));
    if !linux_kill_result.verified {
        serial::line("AEROS_LINUX_KILL_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let seccomp_result =
        scheduler::real_elf_self_test(&paging, &scheduler::SECCOMP_STRICT_PROBE, 137);
    serial::format(format_args!(
        "AEROS_SECCOMP_STRICT exit={} reaped={} verified={}\n",
        seccomp_result.exit_code, seccomp_result.reaped, seccomp_result.verified
    ));
    if !seccomp_result.verified {
        serial::line("AEROS_SECCOMP_STRICT_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let seccomp_filter_result =
        scheduler::real_elf_self_test(&paging, &scheduler::SECCOMP_FILTER_PROBE, 21);
    serial::format(format_args!(
        "AEROS_SECCOMP_FILTER exit={} reaped={} bpf={} verified={}\n",
        seccomp_filter_result.exit_code,
        seccomp_filter_result.reaped,
        seccomp::self_test(),
        seccomp_filter_result.verified && seccomp::self_test()
    ));
    if !(seccomp_filter_result.verified && seccomp::self_test()) {
        serial::line("AEROS_SECCOMP_FILTER_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let capability_result =
        scheduler::real_elf_self_test(&paging, &scheduler::CAPABILITY_PROBE, 22);
    let capability_pure = capability::self_test() && syscall::capability_enforcement_self_test();
    serial::format(format_args!(
        "AEROS_CAPABILITY exit={} reaped={} pure={} verified={}\n",
        capability_result.exit_code,
        capability_result.reaped,
        capability_pure,
        capability_result.verified && capability_pure
    ));
    if !(capability_result.verified && capability_pure) {
        serial::line("AEROS_CAPABILITY_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    #[cfg(feature = "boot-test")]
    {
        let started = time::monotonic_nanoseconds();
        let fuzz_probe =
            scheduler::real_elf_self_test(&paging, &syscall_fuzz_probe::SYSCALL_FUZZ_PROBE, 0);
        serial::format(format_args!(
            "AEROS_SYSCALL_FUZZ exit={} reaped={} elapsed_ms={} verified={}
",
            fuzz_probe.exit_code,
            fuzz_probe.reaped,
            (time::monotonic_nanoseconds() - started) / 1_000_000,
            fuzz_probe.verified
        ));
        if !fuzz_probe.verified {
            serial::line("AEROS_SYSCALL_FUZZ_INVARIANT_FAILURE");
            arch::halt_forever();
        }
    }
    let exec_args_result =
        scheduler::linux_execve_args_self_test(&paging, &scheduler::LINUX_EXECVE_ARGS_PROBE);
    serial::format(format_args!(
        "AEROS_LINUX_EXECVE_ARGS exit={} argc={} arg1={} reaped={} verified={}\n",
        exec_args_result.exit_code,
        exec_args_result.argc,
        exec_args_result.arg1_matches,
        exec_args_result.reaped,
        exec_args_result.verified
    ));
    if !exec_args_result.verified {
        serial::line("AEROS_LINUX_EXECVE_ARGS_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let cow_before = arch::paging::cow_stats();
    let cow_probe = scheduler::real_elf_self_test(&paging, &scheduler::LINUX_COW_PROBE, 11);
    let cow_after = arch::paging::cow_stats();
    let cow_probe_ok =
        cow_probe.verified && cow_after.0 > cow_before.0 && cow_after.1 > cow_before.1;
    serial::format(format_args!(
        "AEROS_COW_FORK exit={} faults_delta={} copies_delta={} verified={}\n",
        cow_probe.exit_code,
        cow_after.0 - cow_before.0,
        cow_after.1 - cow_before.1,
        cow_probe_ok
    ));
    if !cow_probe_ok {
        serial::line("AEROS_COW_FORK_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let cow_k_before = arch::paging::cow_stats();
    let cow_k_probe =
        scheduler::real_elf_self_test(&paging, &scheduler::LINUX_COW_KERNEL_PROBE, 11);
    let cow_k_after = arch::paging::cow_stats();
    let cow_k_ok = cow_k_probe.verified && cow_k_after.0 > cow_k_before.0;
    serial::format(format_args!(
        "AEROS_COW_KERNEL exit={} faults_delta={} verified={}\n",
        cow_k_probe.exit_code,
        cow_k_after.0 - cow_k_before.0,
        cow_k_ok
    ));
    if !cow_k_ok {
        serial::line("AEROS_COW_KERNEL_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let signal_result = scheduler::real_elf_self_test(&paging, &scheduler::LINUX_SIGNAL_PROBE, 11);
    serial::format(format_args!(
        "AEROS_LINUX_SIGNAL exit={} reaped={} verified={}\n",
        signal_result.exit_code, signal_result.reaped, signal_result.verified
    ));
    if !signal_result.verified {
        serial::line("AEROS_LINUX_SIGNAL_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let signal_mask_result =
        scheduler::real_elf_self_test(&paging, &scheduler::LINUX_SIGNAL_MASK_PROBE, 11);
    serial::format(format_args!(
        "AEROS_LINUX_SIGNAL_MASK exit={} reaped={} verified={}\n",
        signal_mask_result.exit_code, signal_mask_result.reaped, signal_mask_result.verified
    ));
    if !signal_mask_result.verified {
        serial::line("AEROS_LINUX_SIGNAL_MASK_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let signal_child_result =
        scheduler::real_elf_self_test(&paging, &scheduler::LINUX_SIGNAL_CHILD_PROBE, 11);
    serial::format(format_args!(
        "AEROS_LINUX_SIGNAL_CHILD exit={} reaped={} verified={}\n",
        signal_child_result.exit_code, signal_child_result.reaped, signal_child_result.verified
    ));
    if !signal_child_result.verified {
        serial::line("AEROS_LINUX_SIGNAL_CHILD_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let async_before = syscall::async_signal_count();
    let signal_spin_result =
        scheduler::real_elf_self_test(&paging, &scheduler::LINUX_SIGNAL_SPIN_PROBE, 11);
    let async_signals = syscall::async_signal_count() - async_before;
    serial::format(format_args!(
        "AEROS_LINUX_SIGNAL_ASYNC exit={} reaped={} async={} verified={}
",
        signal_spin_result.exit_code,
        signal_spin_result.reaped,
        async_signals,
        signal_spin_result.verified && async_signals > 0
    ));
    if !signal_spin_result.verified || async_signals == 0 {
        serial::line("AEROS_LINUX_SIGNAL_ASYNC_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let itimer_before = syscall::async_signal_count();
    let itimer_result = scheduler::real_elf_self_test(&paging, &scheduler::LINUX_ITIMER_PROBE, 11);
    let itimer_signals = syscall::async_signal_count() - itimer_before;
    serial::format(format_args!(
        "AEROS_LINUX_ITIMER exit={} reaped={} async={} verified={}\n",
        itimer_result.exit_code,
        itimer_result.reaped,
        itimer_signals,
        itimer_result.verified && itimer_signals > 0
    ));
    if !itimer_result.verified || itimer_signals == 0 {
        serial::line("AEROS_LINUX_ITIMER_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let nanosleep_result =
        scheduler::real_elf_self_test(&paging, &scheduler::LINUX_NANOSLEEP_PROBE, 11);
    serial::format(format_args!(
        "AEROS_LINUX_NANOSLEEP exit={} reaped={} verified={}\n",
        nanosleep_result.exit_code, nanosleep_result.reaped, nanosleep_result.verified
    ));
    if !nanosleep_result.verified {
        serial::line("AEROS_LINUX_NANOSLEEP_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let pipe_fork_result =
        scheduler::real_elf_self_test(&paging, &scheduler::LINUX_PIPE_FORK_PROBE, 11);
    serial::format(format_args!(
        "AEROS_LINUX_PIPE_FORK exit={} reaped={} verified={}\n",
        pipe_fork_result.exit_code, pipe_fork_result.reaped, pipe_fork_result.verified
    ));
    if !pipe_fork_result.verified {
        serial::line("AEROS_LINUX_PIPE_FORK_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let wnohang_result =
        scheduler::real_elf_self_test(&paging, &scheduler::LINUX_WNOHANG_PROBE, 11);
    serial::format(format_args!(
        "AEROS_LINUX_WNOHANG exit={} reaped={} verified={}\n",
        wnohang_result.exit_code, wnohang_result.reaped, wnohang_result.verified
    ));
    if !wnohang_result.verified {
        serial::line("AEROS_LINUX_WNOHANG_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    // fork() shares pages copy-on-write: the fork tests above must have
    // triggered write faults that each produced a private copy.
    let (cow_faults, cow_copies) = arch::paging::cow_stats();
    serial::format(format_args!(
        "AEROS_COW faults={} copies={} verified={}\n",
        cow_faults,
        cow_copies,
        cow_faults >= 1 && cow_copies >= 1
    ));
    if !(cow_faults >= 1 && cow_copies >= 1) {
        serial::line("AEROS_COW_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    // execve by VFS path: a raw probe replaces itself with the real
    // /bin/aeros-init ELF (exit 74 proves the new image ran with a valid
    // argv/auxv stack).
    let exec_path_result = scheduler::real_elf_self_test(&paging, &scheduler::EXEC_PATH_PROBE, 74);
    serial::format(format_args!(
        "AEROS_EXEC_PATH exit={} reaped={} verified={}\n",
        exec_path_result.exit_code, exec_path_result.reaped, exec_path_result.verified
    ));
    if !exec_path_result.verified {
        serial::line("AEROS_EXEC_PATH_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    // A real compiled ELF that calls fork() itself (unlike /bin/aeros-init
    // or /bin/aeros-std-smoke, which never fork), proving
    // create_process_from_elf + fork_process genuinely work together for
    // a toolchain-built multi-segment binary, not just hand-assembled
    // single-page probes.
    let fork_probe_file = match vfs::file("/bin/aeros-fork-probe") {
        Ok(file) => file,
        Err(_) => {
            serial::line("AEROS_FORK_PROBE_NOT_FOUND");
            arch::halt_forever();
        }
    };
    let real_elf_fork_result = scheduler::real_elf_fork_self_test(&paging, fork_probe_file.data);
    serial::format(format_args!(
        "AEROS_SCHEDULED_REAL_ELF_FORK parent_exit={} marker={:#x} reaped={} verified={}\n",
        real_elf_fork_result.parent_exit,
        real_elf_fork_result.marker,
        real_elf_fork_result.reaped,
        real_elf_fork_result.verified
    ));
    if !real_elf_fork_result.verified {
        serial::line("AEROS_SCHEDULED_REAL_ELF_FORK_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    // `run`/execve can also load a program straight from a persistent
    // volume (`/home`), not just the embedded `/bin/*` binaries: the same
    // real fork-capable ELF above, planted on `/home` and read back through
    // `vfs::with_home_file` instead of a `'static` slice. Skipped (treated
    // as passing) when no home volume is mounted.
    let (home_exec_planted, home_exec_ran) = if datafs::route("/home").is_some() {
        let planted = vfs::open_file("/home/EXECTEST", true, true, true, 0o755, true)
            .ok()
            .is_some_and(|descriptor| {
                let ok = vfs::write(descriptor, fork_probe_file.data, false)
                    == Ok(fork_probe_file.data.len());
                let _ = vfs::close(descriptor);
                ok
            });
        let ran = planted
            && vfs::with_home_file("/home/EXECTEST", |data, _mode| {
                scheduler::spawn_process_with(&paging, data, "/home/EXECTEST", &[b"/home/EXECTEST"])
            })
            .ok()
            .flatten()
            .is_some_and(|(id, _slot, _space)| scheduler::wait_for_child(id).is_some());
        let _ = vfs::remove("/home/EXECTEST", false);
        (planted, ran)
    } else {
        (true, true)
    };
    let home_exec_verified = home_exec_planted && home_exec_ran;
    serial::format(format_args!(
        "AEROS_HOME_EXEC planted={} ran={} verified={}\n",
        home_exec_planted, home_exec_ran, home_exec_verified
    ));
    if !home_exec_verified {
        serial::line("AEROS_HOME_EXEC_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    // A separate, pre-existing gap noticed while adding /home exec support
    // above: the real Linux `execve` path had no antivirus gate at all
    // (unlike spawning a brand-new process, which already refused a
    // detected image) - an already-running process could `execve()` into
    // malware. `exec_current_user_task_path` refuses a detected image
    // before it ever touches scheduler state, so this is safe to call here
    // without a real running task to exec from.
    let execve_blocked =
        scheduler::exec_current_user_task_path(&antivirus::eicar(), "/tmp/x").is_none();
    serial::format(format_args!(
        "AEROS_EXECVE_AV_GATE blocked={}\n",
        execve_blocked
    ));
    if !execve_blocked {
        serial::line("AEROS_EXECVE_AV_GATE_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    serial::format(format_args!(
        "AEROS_COMMANDS count={} shell=aersh elevation=ear unique={} parser={} privilege={} filesystem={} reauth={} redirection={} startup={} symlinks={} background_jobs={} firewall_command={} dmesg_command={} service_command={} text_tools={} priority_command={} crashes_command={} bench_command={} sigcheck_command={} pipelines={} strace_command={} fsck_command={} verified={}\n",
        commands.commands,
        commands.unique,
        commands.parser,
        commands.privilege,
        commands.filesystem,
        commands.reauth,
        commands.redirection,
        commands.startup,
        commands.symlinks,
        commands.background_jobs,
        commands.firewall_command,
        commands.dmesg_command,
        commands.service_command,
        commands.text_tools,
        commands.priority_command,
        commands.crashes_command,
        commands.bench_command,
        commands.sigcheck_command,
        commands.pipelines,
        commands.strace_command,
        commands.fsck_command,
        commands.verified
    ));
    if !commands.verified {
        serial::line("AEROS_COMMAND_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    serial::format(format_args!(
        "AEROS_UI_CORE geometry={} scaling={} interaction={} frost={} max_frost_pixels={} verified={}\n",
        ui_core.geometry,
        ui_core.scaling,
        ui_core.interaction,
        ui_core.frost,
        aerui::MAX_FROST_PIXELS,
        ui_core.verified
    ));
    if !ui_core.verified {
        serial::line("AEROS_UI_CORE_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    serial::format(format_args!(
        "AEROS_KEYBOARD controller={} routed={} vector=52 queued={} dropped={} verified={}\n",
        keyboard.controller,
        keyboard.routed,
        keyboard.queue_bytes,
        keyboard.dropped,
        keyboard.verified
    ));
    serial::format(format_args!(
        "AEROS_MOUSE present={} routed={} vector=53 reporting={} absolute={} verified={}\n",
        mouse.present, mouse.routed, mouse.reporting, mouse.absolute, mouse.verified
    ));
    serial::format(format_args!(
        "AEROS_PLATFORM framebuffer={}x{} stride={} rsdp={:#x} acpi_revision={} acpi_root={:#x} acpi_valid={}\n",
        boot.framebuffer.width,
        boot.framebuffer.height,
        boot.framebuffer.stride,
        boot.rsdp,
        acpi.revision,
        acpi.root_table,
        acpi.valid
    ));
    serial::format(format_args!(
        "AEROS_ACPI tables={} madt={:#x} madt_valid={} hpet={:#x} hpet_valid={} lapic={:#x} processors={} enabled={} ioapics={} overrides={}\n",
        acpi.table_count,
        acpi.madt_address,
        acpi.madt_valid,
        acpi.hpet_address,
        acpi.hpet_valid,
        acpi.local_apic_address,
        acpi.processor_count,
        acpi.enabled_processor_count,
        acpi.io_apic_count,
        acpi.interrupt_override_count
    ));
    if !acpi.valid || !acpi.madt_valid || !smp.verified {
        serial::line("AEROS_ACPI_INVARIANT_FAILURE");
        arch::halt_forever();
    }
    let power_report = power::inspect(&acpi);
    serial::format(format_args!(
        "AEROS_AML loaded={} tables={} nodes={} devices={} methods={} load_errors={} apic_mode={} s5_interpreted={}\n",
        namespace.loaded,
        namespace.tables,
        namespace.nodes,
        namespace.devices,
        namespace.methods,
        namespace.load_errors,
        namespace.apic_mode,
        power_report.s5_interpreted
    ));
    #[cfg(feature = "boot-test")]
    {
        let test = acpi_ns::self_test();
        let scan_matches = test.s5 == Some((power_report.slp_typ_a, power_report.slp_typ_b));
        let verified = namespace.loaded
            && namespace.load_errors == 0
            && namespace.apic_mode
            && test.pci_root
            && test.crs_ok
            && test.prt_entries >= 16
            && test.prt_links
            && test.com1_ok
            && test.link_crs_ok
            && test.sta_errors == 0
            && test.sta_ok > 0
            && scan_matches
            && power_report.s5_interpreted;
        serial::format(format_args!(
            "AEROS_AML_QUERIES s5={:?} matches_scan={} pci_root={} crs_ok={} prt_entries={} prt_links={} com1_ok={} link_crs_ok={} sta_ok={} sta_errors={} verified={}\n",
            test.s5,
            scan_matches,
            test.pci_root,
            test.crs_ok,
            test.prt_entries,
            test.prt_links,
            test.com1_ok,
            test.link_crs_ok,
            test.sta_ok,
            test.sta_errors,
            verified
        ));
        if !verified {
            serial::line("AEROS_AML_INVARIANT_FAILURE");
            arch::halt_forever();
        }
    }
    serial::format(format_args!(
        "AEROS_POWER fadt={} dsdt={} s5_found={} slp_typ_a={} slp_typ_b={} pm1a_cnt={:#x} pm1b_cnt={:#x} ready={}\n",
        power_report.fadt_present,
        power_report.dsdt_present,
        power_report.s5_found,
        power_report.slp_typ_a,
        power_report.slp_typ_b,
        power_report.pm1a_control_block,
        power_report.pm1b_control_block,
        power_report.ready
    ));
    power::set(power_report);
    let image_measured = measure::measure_boot();
    let measured_status = measure::compare(measure::REFERENCE_PATH);
    serial::format(format_args!(
        "AEROS_MEASURE entries=2 image_measured={image_measured} status={} register={}\n",
        measured_status.label(),
        measure::hex_text(&measure::register()).as_str()
    ));
    if measured_status == measure::Status::Changed {
        audit::record(
            "MEASURE",
            format_args!("ALERT: the boot image differs from the sealed one"),
        );
    }
    #[cfg(feature = "boot-test")]
    {
        let test = measure::self_test(image_measured);
        let verified = test.entries == 2
            && test.image_measured
            && test.register_chain
            && test.file_hash
            && test.seal_matches
            && test.tamper_detected
            && test.no_reference
            && measured_status != measure::Status::Changed;
        serial::format(format_args!(
            "AEROS_MEASURE_TEST entries={} image_measured={} register_chain={} file_hash={} seal_matches={} tamper_detected={} no_reference={} verified={verified}\n",
            test.entries,
            test.image_measured,
            test.register_chain,
            test.file_hash,
            test.seal_matches,
            test.tamper_detected,
            test.no_reference
        ));
        let update = update::self_test();
        let verified = update.applied
            && update.previous_kept
            && update.bad_signature_rejected
            && update.tampered_rejected
            && update.untrusted_rejected
            && update.rollback
            && update.torn == 0
            && update.crash_cases >= 10;
        serial::format(format_args!(
            "AEROS_UPDATE applied={} previous_kept={} bad_signature_rejected={} tampered_rejected={} untrusted_rejected={} rollback={} crash_cases={} torn={} verified={verified}\n",
            update.applied,
            update.previous_kept,
            update.bad_signature_rejected,
            update.tampered_rejected,
            update.untrusted_rejected,
            update.rollback,
            update.crash_cases,
            update.torn
        ));
        if !verified {
            serial::line("AEROS_UPDATE_INVARIANT_FAILURE");
            arch::halt_forever();
        }
    }

    // AerOS Shield: load any signature updates left on the data volume
    // (absent on a fresh install; never fails the boot either way), then
    // primitives, a real scan -> quarantine -> restore round trip through
    // the VFS, and a scan of the whole filesystem.
    if let Some(loaded) = antivirus::load_default_signatures() {
        serial::format(format_args!(
            "AEROS_AV_SIGNATURES loaded={} skipped={} path={}\n",
            loaded.added,
            loaded.skipped,
            antivirus::DEFAULT_SIGNATURE_PATH
        ));
    }
    let av_self = antivirus::self_test();
    // The manual flow below needs the realtime hooks out of the way.
    antivirus::set_realtime(false);
    let av_flow = {
        let test = antivirus::eicar();
        let created = vfs::open_file("/tmp/AVTEST", true, true, false, 0o644, true).ok();
        let wrote = created.is_some_and(|descriptor| {
            let ok = vfs::write(descriptor, &test, false) == Ok(test.len());
            let _ = vfs::close(descriptor);
            ok
        });
        let mut report = antivirus::Report::new();
        antivirus::scan_path("/tmp/AVTEST", true, &mut report);
        let found = report.threats == 1;
        let moved = report.findings[0].is_some_and(|finding| finding.quarantined.is_some())
            && vfs::metadata("/tmp/AVTEST").is_err();
        let id = report.findings[0].and_then(|finding| finding.quarantined);
        let restored = id.is_some_and(|id| antivirus::restore(id).is_ok())
            && vfs::metadata("/tmp/AVTEST").is_ok();
        let _ = vfs::remove("/tmp/AVTEST", false);
        wrote && found && moved && restored
    };
    antivirus::set_realtime(true);
    // Realtime: writing a known-bad file and closing it gets it quarantined
    // on the spot, with no scan asked for.
    let av_realtime = {
        let created = vfs::open_file("/tmp/RT_TEST", true, true, false, 0o644, true).ok();
        let events_before = antivirus::event_seq();
        let wrote = created.is_some_and(|descriptor| {
            let ok = vfs::write(descriptor, &antivirus::eicar(), false) == Ok(68);
            let _ = vfs::close(descriptor);
            ok
        });
        let gone = vfs::metadata("/tmp/RT_TEST").is_err();
        let logged = antivirus::event_seq() > events_before;
        let mut found_id = None;
        antivirus::quarantine_list(|entry| {
            if entry.original.as_str() == "/tmp/RT_TEST" {
                found_id = Some(entry.id);
            }
        });
        let leftover = found_id.is_some();
        if let Some(id) = found_id {
            let _ = antivirus::delete(id);
        }
        wrote && gone && logged && leftover
    };
    // The same realtime check, but through a real mounted /home volume
    // (datafs), which used to bypass every antivirus hook entirely - a
    // descriptor whose top bit is set (see `vfs::is_mounted_descriptor`)
    // returned early out of `vfs::open_file`/`write_at`/`close` before any
    // of the scanning code was reached. Skipped when no home volume is
    // mounted (e.g. real hardware with no second disk).
    let av_home_realtime = if datafs::route("/home").is_some() {
        let created = vfs::open_file("/home/RT_HOME_TEST", true, true, false, 0o644, true).ok();
        let events_before = antivirus::event_seq();
        let wrote = created.is_some_and(|descriptor| {
            let ok = vfs::write(descriptor, &antivirus::eicar(), false) == Ok(68);
            let _ = vfs::close(descriptor);
            ok
        });
        let gone = vfs::metadata("/home/RT_HOME_TEST").is_err();
        let logged = antivirus::event_seq() > events_before;
        let mut found_id = None;
        antivirus::quarantine_list(|entry| {
            if entry.original.as_str() == "/home/RT_HOME_TEST" {
                found_id = Some(entry.id);
            }
        });
        let leftover = found_id.is_some();
        if let Some(id) = found_id {
            let _ = antivirus::delete(id);
        }
        wrote && gone && logged && leftover
    } else {
        true
    };
    // `av delete` is supposed to make a quarantined file's bytes actually
    // gone, not just unlink the directory entry while the content sits
    // recoverable in its old disk sectors - checks that directly by
    // shredding a known non-zero payload and reading it back.
    let av_shred = {
        let path = "/tmp/AV_SHRED_TEST";
        // tmpfs (where /tmp lives) caps a single file at MUTABLE_FILE_BYTES
        // (4096) - this is the largest payload that fits, and it exactly
        // fills `shred`'s own 4096-byte zero buffer, so the write-in-place
        // path is exercised right up to that boundary.
        let payload = [0xa5u8; 4096];
        let created = vfs::open_file(path, true, true, false, 0o644, true).ok();
        let wrote = created.is_some_and(|descriptor| {
            let ok = vfs::write(descriptor, &payload, false) == Ok(payload.len());
            let _ = vfs::close(descriptor);
            ok
        });
        antivirus::shred(path);
        let zeroed = vfs::open_file_raw(path).is_ok_and(|descriptor| {
            let mut buffer = [0xffu8; 4096];
            let mut total = 0usize;
            while total < buffer.len() {
                match vfs::read(descriptor, &mut buffer[total..]) {
                    Ok(0) => break,
                    Ok(count) => total += count,
                    Err(_) => break,
                }
            }
            let _ = vfs::close(descriptor);
            total == buffer.len() && buffer.iter().all(|&byte| byte == 0)
        });
        let _ = vfs::remove(path, false);
        wrote && zeroed
    };
    let av_quarantine_encrypted = antivirus::quarantine_encryption_self_test();
    let mut av_report = antivirus::Report::new();
    antivirus::scan_path("/", false, &mut av_report);
    serial::format(format_args!(
        "AEROS_AV signatures={} self_test={} quarantine_flow={} quarantine_encrypted={} realtime={} home_realtime={} shred={} scanned_files={} threats={} unreadable={} verified={}\n",
        antivirus::signature_count(),
        av_self,
        av_flow,
        av_quarantine_encrypted,
        av_realtime,
        av_home_realtime,
        av_shred,
        av_report.files,
        av_report.threats,
        av_report.errors,
        av_self
            && av_flow
            && av_quarantine_encrypted
            && av_realtime
            && av_home_realtime
            && av_shred
            && av_report.threats == 0
    ));
    for finding in av_report.findings.iter().flatten() {
        serial::format(format_args!(
            "AEROS_AV_FINDING class={} name={} path={}\n",
            finding.detection.class.label(),
            finding.detection.name,
            finding.path.as_str()
        ));
    }
    if !(av_self
        && av_flow
        && av_quarantine_encrypted
        && av_realtime
        && av_home_realtime
        && av_shred)
    {
        serial::line("AEROS_AV_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }

    let audit_verified = audit::self_test();
    serial::format(format_args!("AEROS_AUDIT verified={}\n", audit_verified));
    if !audit_verified {
        serial::line("AEROS_AUDIT_INVARIANT_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }

    let mut framebuffer = unsafe { FrameBuffer::new(boot.framebuffer) };
    let ui_render = aerui::render_self_test(&mut framebuffer);
    serial::format(format_args!(
        "AEROS_UI_RENDER pixels={} captured={} changed={} corner_clipped={} verified={}\n",
        ui_render.pixels,
        ui_render.captured,
        ui_render.changed,
        ui_render.corner_clipped,
        ui_render.verified
    ));
    if !ui_render.verified {
        serial::line("AEROS_UI_RENDER_FAILURE");
        #[cfg(feature = "boot-test")]
        arch::halt_forever();
    }
    let desktop_render = desktop::render_self_test(&mut framebuffer, &fonts);
    serial::format(format_args!(
        "AEROS_DESKTOP wallpaper={} dock={} app_switcher={} quick_settings={} window={} button={} input={} verified={}\n",
        desktop_render.wallpaper,
        desktop_render.dock,
        desktop_render.app_switcher,
        desktop_render.quick_settings,
        desktop_render.window,
        desktop_render.button,
        desktop_render.input,
        desktop_render.verified
    ));
    if !desktop_render.verified {
        serial::line("AEROS_DESKTOP_FAILURE");
        arch::halt_forever();
    }
    let window_animation = desktop::window_animation_self_test();
    serial::format(format_args!(
        "AEROS_WINDOW_ANIMATION mirror=true monotonic=true centered=true verified={}\n",
        window_animation
    ));
    if !window_animation {
        serial::line("AEROS_WINDOW_ANIMATION_FAILURE");
        arch::halt_forever();
    }
    let text_processing_valid = desktop::text_processing_self_test();
    serial::format(format_args!(
        "AEROS_TEXT_WRAP verified={}\n",
        text_processing_valid
    ));
    if !text_processing_valid {
        serial::line("AEROS_TEXT_WRAP_FAILURE");
        arch::halt_forever();
    }
    let health = ui::BootHealth {
        cpu: &cpu,
        memory: &boot.memory,
        allocator: frames.stats(),
        acpi_valid: acpi.valid,
        topology_valid: acpi.madt_valid,
        allocator_valid: allocator_check,
        architecture_valid: descriptors.loaded && interrupts_valid && smp.verified,
        timer_valid: timer_ticks >= 4 && apic.verified && ioapic.verified && time.verified,
        paging_valid: paging.verified && paging.write_protect_enabled,
        heap_valid,
        user_valid: user_probe.verified
            && init_process.verified
            && compiled_process.verified
            && std_process.verified
            && fault_process.verified
            && fault_stats.faults == 1
            && process_reap_valid,
        vfs_valid: vfs_stats.verified
            && tmpfs_valid
            && elf_valid
            && compiled_image.self_test()
            && std_image.self_test(),
        pci_valid: pci_summary.verified,
        storage_valid: ahci.verified && partitions.verified && fat.verified,
        network_valid: network.verified
            && internet.dhcp_verified
            && internet.verified
            && internet.dns_verified,
        scheduler_valid: scheduler_valid && preemption_valid,
    };
    ui::draw_boot_complete(&mut framebuffer, &fonts, health);

    // Harmless to do even in a `test.ps1` boot-test run (which exits via
    // `debug_exit` below before `desktop::run` is ever reached, so nothing
    // further checks the result there) - keeping this call unconditional
    // avoids needing a `#[cfg]` split just to dodge a dead-code warning on
    // the `bind`/`SET_SCANOUT` path in that build.
    virtio_gpu::bind(&framebuffer, &mut frames);

    memory::install_primary(frames);
    #[cfg(feature = "boot-test")]
    image::self_test();
    #[cfg(feature = "boot-test")]
    keymap::self_test();
    #[cfg(feature = "boot-test")]
    font::truetype_self_test();
    #[cfg(feature = "boot-test")]
    screenshot::self_test(&framebuffer);
    #[cfg(feature = "boot-test")]
    logo::self_test();
    #[cfg(feature = "boot-test")]
    {
        let started = time::monotonic_nanoseconds();
        let lines = shell::fuzz_commands(shell_info, 2_500);
        serial::format(format_args!(
            "AEROS_SHELL_FUZZ lines={} elapsed_ms={} verified={}\n",
            lines,
            (time::monotonic_nanoseconds() - started) / 1_000_000,
            lines == 2_500
        ));
    }
    serial::line("AEROS_READY");

    #[cfg(feature = "boot-test")]
    unsafe {
        arch::debug_exit(0x10);
    }

    desktop::run(&mut framebuffer, &fonts, shell_info)
}
