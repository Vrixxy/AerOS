#![no_std]
#![no_main]

use core::arch::{asm, naked_asm};
use core::mem::MaybeUninit;
use core::panic::PanicInfo;

static MESSAGE: &[u8] = b"AerOS compiled userspace online\n";

#[unsafe(no_mangle)]
#[unsafe(naked)]
unsafe extern "C" fn _start() -> ! {
    naked_asm!("mov rdi, rsp", "jmp {}", sym start);
}

extern "C" fn start(stack: usize) -> ! {
    if stack & 15 != 0 {
        exit(120)
    }
    let argc = unsafe { (stack as *const usize).read() };
    if argc != 1 {
        exit(121)
    }
    let written = unsafe { syscall3(1, 1, MESSAGE.as_ptr() as usize, MESSAGE.len()) };
    if written != MESSAGE.len() as isize {
        exit(122)
    }
    let pid = unsafe { syscall0(39) };
    if pid <= 0 {
        exit(123)
    }
    let mut name = MaybeUninit::<[u8; 390]>::uninit();
    let result = unsafe { syscall1(63, name.as_mut_ptr() as usize) };
    let name = unsafe { name.assume_init() };
    if result != 0 || &name[..6] != b"Linux\0" {
        exit(124)
    }
    let uid = unsafe { syscall0(102) };
    let gid = unsafe { syscall0(104) };
    if uid != 0 || gid != 0 {
        exit(125)
    }
    exit(74)
}

#[panic_handler]
fn panic(_: &PanicInfo<'_>) -> ! {
    exit(127)
}

fn exit(status: usize) -> ! {
    unsafe {
        asm!(
            "syscall",
            in("rax") 60usize,
            in("rdi") status,
            options(noreturn)
        );
    }
}

unsafe fn syscall0(number: usize) -> isize {
    let result: isize;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") number as isize => result,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack)
        );
    }
    result
}

unsafe fn syscall1(number: usize, first: usize) -> isize {
    let result: isize;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") number as isize => result,
            in("rdi") first,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack)
        );
    }
    result
}

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
