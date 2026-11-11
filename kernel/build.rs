//! With the `linux-drivers` feature: builds kernel/linux (the Linux driver
//! adapter and drivers, in C) with Zig's clang into one position-independent
//! ELF module that kernel/src/linux.rs embeds and loads.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Unmodified Linux DRM files (MIT-licensed; GPL ones must not be listed:
/// AerOS's license does not allow them), taken from FreeBSD's drm-kmod
/// checkout in build/gpu/drm-kmod (or AEROS_DRM_KMOD).
const DRM_SOURCES: &[&str] = &[
    "drivers/gpu/drm/drm_mm.c",
    "drivers/gpu/drm/drm_print.c",
    "drivers/gpu/buddy.c",
    "drivers/gpu/drm/drm_buddy.c",
    "drivers/dma-buf/dma-fence.c",
    "drivers/dma-buf/dma-fence-array.c",
    "drivers/dma-buf/dma-fence-chain.c",
    "drivers/dma-buf/dma-resv.c",
    "drivers/gpu/drm/scheduler/sched_main.c",
    "drivers/gpu/drm/scheduler/sched_entity.c",
    "drivers/gpu/drm/scheduler/sched_fence.c",
    "drivers/gpu/drm/drm_vma_manager.c",
    "drivers/gpu/drm/drm_cache.c",
    "drivers/gpu/drm/ttm/ttm_module.c",
    "drivers/gpu/drm/ttm/ttm_device.c",
    "drivers/gpu/drm/ttm/ttm_resource.c",
    "drivers/gpu/drm/ttm/ttm_range_manager.c",
    "drivers/gpu/drm/ttm/ttm_sys_manager.c",
    "drivers/gpu/drm/ttm/ttm_pool.c",
    "drivers/gpu/drm/ttm/ttm_tt.c",
    "drivers/gpu/drm/ttm/ttm_bo.c",
    "drivers/gpu/drm/ttm/ttm_bo_util.c",
    "drivers/gpu/drm/ttm/ttm_execbuf_util.c",
    "drivers/gpu/drm/drm_managed.c",
    "drivers/gpu/drm/drm_auth.c",
    "drivers/gpu/drm/drm_file.c",
    "drivers/gpu/drm/drm_gem.c",
    "drivers/gpu/drm/drm_ioctl.c",
    "drivers/gpu/drm/drm_syncobj.c",
    "drivers/gpu/drm/drm_drv.c",
    "drivers/gpu/drm/drm_mode_object.c",
    "drivers/gpu/drm/drm_property.c",
    "drivers/gpu/drm/drm_blend.c",
    "drivers/gpu/drm/drm_color_mgmt.c",
    "drivers/gpu/drm/drm_fourcc.c",
    "drivers/gpu/drm/drm_modes.c",
    "drivers/gpu/drm/drm_rect.c",
    "drivers/gpu/drm/drm_displayid.c",
    "drivers/gpu/drm/drm_eld.c",
    "drivers/gpu/drm/drm_edid.c",
    "drivers/gpu/drm/drm_modeset_lock.c",
    "drivers/gpu/drm/drm_mode_config.c",
    "drivers/gpu/drm/drm_framebuffer.c",
    "drivers/gpu/drm/drm_crtc.c",
    "drivers/gpu/drm/drm_plane.c",
    "drivers/gpu/drm/drm_encoder.c",
    "drivers/gpu/drm/drm_connector.c",
    "drivers/gpu/drm/drm_bridge.c",
    "drivers/gpu/drm/drm_vblank.c",
    "drivers/gpu/drm/drm_vblank_work.c",
    "drivers/gpu/drm/drm_atomic.c",
    "drivers/gpu/drm/drm_atomic_uapi.c",
    "drivers/gpu/drm/drm_atomic_state_helper.c",
    "drivers/gpu/drm/drm_atomic_helper.c",
    "drivers/gpu/drm/drm_crtc_helper.c",
    "drivers/gpu/drm/drm_probe_helper.c",
    "drivers/gpu/drm/drm_plane_helper.c",
    "drivers/gpu/drm/drm_damage_helper.c",
    "drivers/gpu/drm/drm_self_refresh_helper.c",
    "drivers/gpu/drm/drm_gem_framebuffer_helper.c",
    "drivers/gpu/drm/drm_gem_atomic_helper.c",
    "drivers/gpu/drm/drm_modeset_helper.c",
    "drivers/gpu/drm/drm_dumb_buffers.c",
    "drivers/gpu/drm/drm_lease.c",
    "drivers/gpu/drm/drm_prime.c",
    "drivers/gpu/drm/drm_client.c",
    "drivers/gpu/drm/drm_client_modeset.c",
    "drivers/video/hdmi.c",
    "drivers/gpu/drm/ttm/ttm_bo_vm.c",
    "drivers/gpu/drm/drm_suballoc.c",
    "drivers/gpu/drm/drm_exec.c",
    "drivers/gpu/drm/display/drm_display_helper_mod.c",
    "drivers/gpu/drm/display/drm_dp_helper.c",
    "drivers/gpu/drm/display/drm_dp_mst_topology.c",
];

const CFLAGS: &[&str] = &[
    "-target",
    "x86_64-freestanding-none",
    "-O2",
    "-std=gnu11",
    "-D__KERNEL__",
    "-D__FreeBSD_version=9999999",
    "-D__linux__",
    "-DKBUILD_MODNAME=\"aeros\"",
    "-include",
    "linux/types.h",
    "-ffreestanding",
    "-fno-builtin",
    "-fno-stack-protector",
    "-fno-sanitize=all",
    "-mno-red-zone",
    "-mgeneral-regs-only",
    "-fPIE",
    "-fno-strict-aliasing",
    "-fwrapv",
    "-fno-delete-null-pointer-checks",
    "-fno-asynchronous-unwind-tables",
    "-fno-unwind-tables",
    "-Wall",
    "-Wno-unused-function",
    "-Werror",
];

/// Warnings the Linux build itself turns off; only applied to upstream files.
const UPSTREAM_CFLAGS: &[&str] = &[
    "-Wno-unused-but-set-variable",
    "-Wno-unused-variable",
    "-Wno-sign-compare",
    "-Wno-pointer-sign",
    "-Wno-missing-braces",
    "-Wno-address-of-packed-member",
    "-Wno-gnu-variable-sized-type-not-at-end",
    "-Wno-visibility",
];

/// amdgpu (kernel/linux/amdgpu.list): built without its display core (DC),
/// ACPI, and the pre-2014 SI/CIK families.
const AMD_CFLAGS: &[&str] = &[
    "-UKBUILD_MODNAME",
    "-DKBUILD_MODNAME=\"amdgpu\"",
    "-DCONFIG_X86",
    "-DCONFIG_X86_64",
    "-DCONFIG_64BIT",
    "-Wno-#warnings",
    "-Wno-format-security",
];

const AMD_INCLUDES: &[&str] = &[
    "acp/include",
    "amdgpu",
    "amdkfd",
    "display",
    "display/amdgpu_dm",
    "display/dc",
    "display/dc/inc",
    "display/dc/inc/hw",
    "display/dmub/inc",
    "display/include",
    "display/modules/hdcp",
    "display/modules/inc",
    "display/modules/info_packet",
    "display/modules/power",
    "include",
    "include/asic_reg",
    "pm",
    "pm/inc",
    "pm/legacy-dpm",
    "pm/powerplay",
    "pm/powerplay/inc",
    "pm/powerplay/hwmgr",
    "pm/powerplay/smumgr",
    "pm/swsmu",
    "pm/swsmu/inc",
    "pm/swsmu/inc/pmfw_if",
    "pm/swsmu/smu11",
    "pm/swsmu/smu12",
    "pm/swsmu/smu13",
    "pm/swsmu/smu14",
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=AEROS_ZIG");
    if env::var_os("CARGO_FEATURE_LINUX_DRIVERS").is_none() {
        return;
    }
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let source = manifest.join("linux");
    let zig = env::var_os("AEROS_ZIG")
        .map(PathBuf::from)
        .or_else(|| find_zig(&manifest.join("../build/gpu")))
        .expect("linux-drivers needs Zig: set AEROS_ZIG or unpack it under build/gpu");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let objects_dir = out.join("linux");
    std::fs::create_dir_all(&objects_dir).unwrap();
    let mut sources = Vec::new();
    collect(&source.join("lib"), &mut sources);
    track(&source.join("include"));
    println!("cargo:rerun-if-changed={}", source.join("lib").display());
    println!(
        "cargo:rerun-if-changed={}",
        source.join("drivers").display()
    );
    println!("cargo:rerun-if-env-changed=AEROS_DRM_KMOD");
    let drm = env::var_os("AEROS_DRM_KMOD")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("../build/gpu/drm-kmod"));
    let include_dirs = [
        source.join("include"),
        drm.join("include"),
        drm.join("include/uapi"),
        drm.join("linuxkpi/bsd/include"),
        drm.join("drivers/gpu"),
    ];
    for relative in DRM_SOURCES {
        let file = drm.join(relative);
        assert!(
            file.is_file(),
            "linux-drivers needs {} (drm-kmod checkout)",
            file.display()
        );
        sources.push(file);
    }
    let amd_list = source.join("amdgpu.list");
    println!("cargo:rerun-if-changed={}", amd_list.display());
    let amd_sources: Vec<PathBuf> = std::fs::read_to_string(&amd_list)
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| drm.join(line.trim()))
        .collect();
    sources.extend(amd_sources.iter().cloned());
    // Initcalls run in link order: library code before the drivers using it.
    collect(&source.join("drivers"), &mut sources);
    let amd = drm.join("drivers/gpu/drm/amd");
    let mut jobs = Vec::new();
    for file in &sources {
        println!("cargo:rerun-if-changed={}", file.display());
        let relative = file
            .strip_prefix(&source)
            .or_else(|_| file.strip_prefix(&drm))
            .unwrap()
            .to_string_lossy();
        let object = objects_dir.join(relative.replace(['/', '\\'], "_") + ".o");
        let mut arguments: Vec<std::ffi::OsString> = vec!["cc".into()];
        arguments.extend(CFLAGS.iter().map(Into::into));
        if !file.starts_with(&source) {
            arguments.extend(UPSTREAM_CFLAGS.iter().map(Into::into));
        }
        if file.starts_with(drm.join("drivers/dma-buf")) || file.ends_with("drm_managed.c") {
            arguments.extend(["-include", "aeros/freebsd.h"].map(Into::into));
        }
        for directory in &include_dirs {
            arguments.push("-I".into());
            arguments.push(directory.into());
        }
        if file.starts_with(&amd) {
            arguments.extend(AMD_CFLAGS.iter().map(Into::into));
            for directory in ["include/drm", "drivers/gpu/drm/scheduler"] {
                arguments.push("-I".into());
                arguments.push(drm.join(directory).into());
            }
            for directory in AMD_INCLUDES {
                arguments.push("-I".into());
                arguments.push(amd.join(directory).into());
            }
        }
        jobs.push((file.clone(), object, arguments));
    }
    compile_all(&zig, &jobs);
    // Hundreds of objects overflow a Windows command line: list them in a file.
    let response = out.join("objects.rsp");
    let listing: String = jobs
        .iter()
        .map(|(_, object, _)| format!("\"{}\"\n", object.display().to_string().replace('\\', "/")))
        .collect();
    std::fs::write(&response, listing).unwrap();
    run(Command::new(&zig)
        .args([
            "ld.lld",
            "-pie",
            "--no-dynamic-linker",
            "-e",
            "aeros_linux_entry",
        ])
        .args([
            "--gc-sections",
            "--error-limit=0",
            "--strip-debug",
            "-z",
            "nostart-stop-gc",
            "-z",
            "max-page-size=4096",
        ])
        .arg("-o")
        .arg(out.join("aeros-linux.elf"))
        .arg(format!("@{}", response.display())));
}

/// Compiles every job on all cores, skipping objects newer than everything
/// their last compile read (from clang's dependency file) and built with the
/// same arguments.
fn compile_all(zig: &Path, jobs: &[(PathBuf, PathBuf, Vec<std::ffi::OsString>)]) {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let next = AtomicUsize::new(0);
    let failures = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some((file, object, arguments)) = jobs.get(index) else {
                        break;
                    };
                    let depfile = object.with_extension("d");
                    let stamp = object.with_extension("args");
                    let key = format!("{arguments:?}");
                    if up_to_date(object, &depfile)
                        && std::fs::read_to_string(&stamp).is_ok_and(|old| old == key)
                    {
                        continue;
                    }
                    let output = Command::new(zig)
                        .args(arguments)
                        .arg("-MD")
                        .arg("-MF")
                        .arg(&depfile)
                        .arg("-c")
                        .arg(file)
                        .arg("-o")
                        .arg(object)
                        .output()
                        .expect("running zig");
                    if output.status.success() {
                        std::fs::write(&stamp, key).unwrap();
                    } else {
                        let _ = std::fs::remove_file(object);
                        failures.lock().unwrap().push(format!(
                            "{}:\n{}",
                            file.display(),
                            String::from_utf8_lossy(&output.stderr)
                        ));
                    }
                }
            });
        }
    });
    let failures = failures.into_inner().unwrap();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn up_to_date(object: &Path, depfile: &Path) -> bool {
    let (Ok(built), Ok(text)) = (
        std::fs::metadata(object).and_then(|m| m.modified()),
        std::fs::read_to_string(depfile),
    ) else {
        return false;
    };
    let text = text.replace("\\\r\n", " ").replace("\\\n", " ");
    let Some((_, dependencies)) = text.split_once(".o:") else {
        return false;
    };
    let mut path = String::new();
    for token in dependencies.split_whitespace() {
        path.push_str(token);
        if let Some(stripped) = path.strip_suffix('\\') {
            path = format!("{stripped} ");
            continue;
        }
        match std::fs::metadata(&path).and_then(|m| m.modified()) {
            Ok(changed) if changed <= built => {}
            _ => return false,
        }
        path.clear();
    }
    true
}

fn run(command: &mut Command) {
    let status = command.status().expect("running zig");
    assert!(status.success(), "{command:?} failed");
}

fn find_zig(directory: &Path) -> Option<PathBuf> {
    std::fs::read_dir(directory)
        .ok()?
        .flatten()
        .map(|entry| entry.path().join("zig.exe"))
        .find(|path| path.is_file())
}

fn collect(directory: &Path, sources: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(&path, sources);
        } else if path.extension().is_some_and(|extension| extension == "c") {
            sources.push(path);
        }
    }
}

fn track(directory: &Path) {
    println!("cargo:rerun-if-changed={}", directory.display());
    if let Ok(entries) = std::fs::read_dir(directory) {
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.is_dir() {
                track(&path);
            } else {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
}
