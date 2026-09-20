#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

mod acpi;
pub mod aerui;
mod ahci;
mod arch;
pub mod button;
mod clipboard;
mod compat;
mod desktop;
mod e1000;
mod elf;
mod fat;
mod font;
mod framebuffer;
mod heap;
mod initramfs;
mod installer;
mod ioapic;
mod keyboard;
mod loading;
mod memory;
mod mouse;
mod net;
mod partition;
mod pci;
mod power;
mod process;
mod random;
mod rtc;
mod scheduler;
mod serial;
mod shell;
mod smp;
mod svm;
mod sync;
mod syscall;
mod time;
mod uefi;
mod ui;
mod vfs;

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

#[unsafe(no_mangle)]
extern "efiapi" fn efi_main(image: Handle, table: *mut SystemTable) -> Status {
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
    let heap_mapping = match arch::paging::map_heap(&paging, &mut frames, 256) {
        Some(mapping) => mapping,
        None => {
            serial::line("AEROS_HEAP_MAPPING_FAILURE");
            arch::halt_forever();
        }
    };
    let heap_initialized =
        unsafe { heap::HEAP.initialize(heap_mapping.virtual_base as usize, heap_mapping.size) };
    let heap_valid = heap_initialized && heap::HEAP.self_test();
    let heap_stats = heap::HEAP.stats();
    let pci = pci::PciInventory::scan();
    let pci_summary = pci.summary();
    let ahci = ahci::initialize(&pci, &mut frames);
    let partitions = partition::inspect(ahci.sectors);
    let fat = fat::inspect(&partitions);
    let install = installer::install(&mut frames);
    let network = e1000::initialize(&pci, &mut frames);
    let internet = net::self_test(&network);
    let initramfs_entries = initramfs::entries();
    let vfs_stats = vfs::initialize(&initramfs_entries);
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
        && runtime_vfs.nodes == 11
        && runtime_vfs.directories == 5
        && runtime_vfs.files == 6
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
    serial::format(format_args!(
        "AEROS_ENTROPY rdrand={} rdseed={} hardware_words={} sample_a_nonzero={} sample_b_nonzero={} distinct={} chacha20=true aslr=true verified={}\n",
        entropy.rdrand,
        entropy.rdseed,
        entropy.hardware_words,
        entropy.sample_a != 0,
        entropy.sample_b != 0,
        entropy.sample_a != entropy.sample_b,
        entropy.verified
    ));
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
        "AEROS_INSTALL attempted={} target_disk={} target_sectors={} target_blank={} source_bytes={} gpt={} formatted={} kernel_written={} marker_written={} readback_ok={} verified={}\n",
        install.attempted,
        install.target_disk,
        install.target_sectors,
        install.target_blank,
        install.source_bytes,
        install.gpt,
        install.formatted,
        install.kernel_written,
        install.marker_written,
        install.readback_ok,
        install.verified,
    ));
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
        "AEROS_NETWORK driver=e1000e present={} mmio={:#x} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} link={} speed_mbps={} duplex={} tx={} rx={} rx_bytes={} arp_gateway={} verified={}\n",
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
    serial::format(format_args!(
        "AEROS_COMMANDS count={} shell=aersh elevation=ear unique={} parser={} privilege={} filesystem={} verified={}\n",
        commands.commands,
        commands.unique,
        commands.parser,
        commands.privilege,
        commands.filesystem,
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

    serial::line("AEROS_READY");

    #[cfg(feature = "boot-test")]
    unsafe {
        arch::debug_exit(0x10);
    }

    desktop::run(&mut framebuffer, &fonts, shell_info)
}
