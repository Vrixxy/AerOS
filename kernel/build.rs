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
];

const CFLAGS: &[&str] = &[
    "-target",
    "x86_64-freestanding-none",
    "-O2",
    "-std=gnu11",
    "-D__KERNEL__",
    "-D__FreeBSD_version=9999999",
    "-D__linux__",
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
    // Initcalls run in link order: library code before the drivers using it.
    collect(&source.join("drivers"), &mut sources);
    let mut objects = Vec::new();
    for file in &sources {
        println!("cargo:rerun-if-changed={}", file.display());
        let relative = file
            .strip_prefix(&source)
            .or_else(|_| file.strip_prefix(&drm))
            .unwrap()
            .to_string_lossy();
        let object = objects_dir.join(relative.replace(['/', '\\'], "_") + ".o");
        let mut command = Command::new(&zig);
        command.arg("cc").args(CFLAGS);
        if !file.starts_with(&source) {
            command.args(UPSTREAM_CFLAGS);
        }
        if file.starts_with(drm.join("drivers/dma-buf")) || file.ends_with("drm_managed.c") {
            command.args(["-include", "aeros/freebsd.h"]);
        }
        for directory in &include_dirs {
            command.arg("-I").arg(directory);
        }
        run(command.arg("-c").arg(file).arg("-o").arg(&object));
        objects.push(object);
    }
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
            "-z",
            "nostart-stop-gc",
            "-z",
            "max-page-size=4096",
        ])
        .arg("-o")
        .arg(out.join("aeros-linux.elf"))
        .args(&objects));
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
