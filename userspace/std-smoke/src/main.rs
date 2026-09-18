use std::arch::asm;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const AT_FDCWD: usize = (-100isize) as usize;
const AT_REMOVEDIR: usize = 0x200;

unsafe fn syscall3(number: usize, first: usize, second: usize, third: usize) -> isize {
    let result: isize;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") number as isize => result,
            in("rdi") first,
            in("rsi") second,
            in("rdx") third,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack)
        );
    }
    result
}

unsafe fn syscall5(
    number: usize,
    first: usize,
    second: usize,
    third: usize,
    fourth: usize,
    fifth: usize,
) -> isize {
    let result: isize;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") number as isize => result,
            in("rdi") first,
            in("rsi") second,
            in("rdx") third,
            in("r10") fourth,
            in("r8") fifth,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack)
        );
    }
    result
}

fn main() {
    let release = std::fs::read_to_string("/etc/aeros-release").unwrap_or_default();
    let directory = std::env::current_dir().unwrap_or_default();
    if release != "AerOS 0.1.0 x86_64\n" || directory != Path::new("/") {
        std::process::exit(126);
    }
    if std::fs::create_dir("/tmp/std-smoke").is_err()
        || std::fs::write("/tmp/std-smoke/source", b"standard-library").is_err()
        || std::fs::rename("/tmp/std-smoke/source", "/tmp/std-smoke/result").is_err()
    {
        std::process::exit(125);
    }
    let contents = std::fs::read("/tmp/std-smoke/result").unwrap_or_default();
    if contents != b"standard-library"
        || std::fs::remove_file("/tmp/std-smoke/result").is_err()
        || std::fs::remove_dir("/tmp/std-smoke").is_err()
    {
        std::process::exit(124);
    }
    let directory_path = b"tmp/at-smoke\0";
    let source_path = b"tmp/at-smoke/source\0";
    let target_path = b"tmp/at-smoke/target\0";
    let created = unsafe { syscall3(258, AT_FDCWD, directory_path.as_ptr() as usize, 0o755) };
    if created != 0
        || std::fs::write("/tmp/at-smoke/source", b"modern-source").is_err()
        || std::fs::write("/tmp/at-smoke/target", b"old-target").is_err()
    {
        std::process::exit(123);
    }
    let not_replaced = unsafe {
        syscall5(
            316,
            AT_FDCWD,
            source_path.as_ptr() as usize,
            AT_FDCWD,
            target_path.as_ptr() as usize,
            1,
        )
    };
    let renamed = unsafe {
        syscall5(
            316,
            AT_FDCWD,
            source_path.as_ptr() as usize,
            AT_FDCWD,
            target_path.as_ptr() as usize,
            0,
        )
    };
    let modern_contents = std::fs::read("/tmp/at-smoke/target").unwrap_or_default();
    let removed_file = unsafe { syscall3(263, AT_FDCWD, target_path.as_ptr() as usize, 0) };
    let removed_directory = unsafe {
        syscall3(
            263,
            AT_FDCWD,
            directory_path.as_ptr() as usize,
            AT_REMOVEDIR,
        )
    };
    if not_replaced != -17
        || renamed != 0
        || modern_contents != b"modern-source"
        || removed_file != 0
        || removed_directory != 0
    {
        std::process::exit(122);
    }
    let file_path = "/tmp/file-api";
    let mut file = match std::fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(file_path)
    {
        Ok(file) => file,
        Err(_) => std::process::exit(121),
    };
    if file.write_all(b"truncate-and-sync").is_err()
        || file.set_len(8).is_err()
        || file.sync_data().is_err()
        || file.sync_all().is_err()
        || std::fs::set_permissions(file_path, std::fs::Permissions::from_mode(0o600)).is_err()
    {
        std::process::exit(120);
    }
    drop(file);
    let metadata = match std::fs::metadata(file_path) {
        Ok(metadata) => metadata,
        Err(_) => std::process::exit(119),
    };
    if metadata.len() != 8
        || metadata.permissions().mode() & 0o777 != 0o600
        || std::fs::read(file_path).unwrap_or_default() != b"truncate"
        || std::fs::remove_file(file_path).is_err()
    {
        std::process::exit(118);
    }
    println!("AerOS standard Rust userspace online");
    std::process::exit(75);
}
