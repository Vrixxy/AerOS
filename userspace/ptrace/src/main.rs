use std::arch::{asm, naked_asm};
use std::sync::atomic::{AtomicU64, Ordering};

const SYS_GETPID: usize = 39;
const SYS_FORK: usize = 57;
const SYS_EXECVE: usize = 59;
const SYS_WAIT4: usize = 61;
const SYS_KILL: usize = 62;
const SYS_GETPPID: usize = 110;
const SYS_PTRACE: usize = 101;
const SYS_SCHED_YIELD: usize = 24;
const SYS_EXIT_GROUP: usize = 231;

const TRACEME: usize = 0;
const PEEKDATA: usize = 2;
const PEEKUSER: usize = 3;
const POKETEXT: usize = 4;
const POKEDATA: usize = 5;
const POKEUSER: usize = 6;
const CONT: usize = 7;
const KILL: usize = 8;
const SINGLESTEP: usize = 9;
const GETREGS: usize = 12;
const SETREGS: usize = 13;
const GETFPREGS: usize = 14;
const SETFPREGS: usize = 15;
const ATTACH: usize = 16;
const DETACH: usize = 17;
const SYSCALL: usize = 24;
const SETOPTIONS: usize = 0x4200;
const GETSIGINFO: usize = 0x4202;

const OPTIONS: usize = 1 | 0x10 | 0x10_0000;
const RAX: usize = 10;
const R12: usize = 3;
const RIP: usize = 16;
const ORIG_RAX: usize = 15;
const EFLAGS: usize = 18;

static FLAG: AtomicU64 = AtomicU64::new(0x1111);

fn syscall(number: usize, args: [usize; 4]) -> isize {
    let result: isize;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") number as isize => result,
            in("rdi") args[0],
            in("rsi") args[1],
            in("rdx") args[2],
            in("r10") args[3],
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack)
        );
    }
    result
}

fn ptrace(request: usize, pid: usize, address: usize, data: usize) -> isize {
    syscall(SYS_PTRACE, [request, pid, address, data])
}

fn wait(pid: usize) -> (isize, i32) {
    let mut status = 0i32;
    let result = syscall(SYS_WAIT4, [pid, &mut status as *mut i32 as usize, 0, 0]);
    (result, status)
}

fn stopped(pid: usize, signal: i32) -> bool {
    let (result, status) = wait(pid);
    let ok = result == pid as isize && status == (signal << 8) | 0x7f;
    if !ok {
        println!("AEROS_PTRACE_WAIT result={result} status={status:#x} wanted={signal}");
    }
    ok
}

fn regs(pid: usize) -> [u64; 27] {
    let mut regs = [0u64; 27];
    ptrace(GETREGS, pid, 0, regs.as_mut_ptr() as usize);
    regs
}

fn set_regs(pid: usize, regs: &[u64; 27]) -> isize {
    ptrace(SETREGS, pid, 0, regs.as_ptr() as usize)
}

fn peek(pid: usize, address: usize) -> Option<u64> {
    let mut word = 0u64;
    (ptrace(PEEKDATA, pid, address, &mut word as *mut u64 as usize) == 0).then_some(word)
}

fn step(name: &str, ok: bool, code: i32) {
    println!("AEROS_PTRACE_STEP {name} {}", if ok { "ok" } else { "FAILED" });
    if !ok {
        std::process::exit(code);
    }
}

fn fork() -> usize {
    syscall(SYS_FORK, [0; 4]) as usize
}

fn leave(code: i32) -> ! {
    syscall(SYS_EXIT_GROUP, [code as usize, 0, 0, 0]);
    loop {}
}

#[unsafe(naked)]
unsafe extern "C" fn patch_me() -> u64 {
    naked_asm!("mov eax, 1", "ret")
}

fn traced_child() {
    ptrace(TRACEME, 0, 0, 0);
    let pid = syscall(SYS_GETPID, [0; 4]) as usize;
    syscall(SYS_KILL, [pid, 10, 0, 0]);
    if syscall(SYS_GETPID, [0; 4]) != 1234 {
        leave(10);
    }
    let mut r12 = 0x1234u64;
    unsafe {
        asm!("int3", "nop", "nop", "nop", inout("r12") r12, options(nostack));
    }
    if r12 != 0x5678 {
        leave(11);
    }
    if FLAG.load(Ordering::SeqCst) != 0x2222 {
        leave(12);
    }
    if unsafe { patch_me() } != 7 {
        leave(13);
    }
    let pattern = 0x1122_3344_5566_7788u64;
    let back: u64;
    unsafe {
        asm!(
            "movq xmm0, {a}",
            "int3",
            "movq {b}, xmm0",
            a = in(reg) pattern,
            b = out(reg) back,
            out("xmm0") _,
            options(nostack)
        );
    }
    if back != 0x8877_6655_4433_2211 {
        leave(14);
    }
    if syscall(SYS_GETPPID, [0; 4]) != -77 {
        leave(15);
    }
    let path = b"/bin/aeros-ptrace\0";
    let argument = b"exec\0";
    let argv = [path.as_ptr() as usize, argument.as_ptr() as usize, 0];
    syscall(SYS_EXECVE, [path.as_ptr() as usize, argv.as_ptr() as usize, 0, 0]);
    leave(16);
}

fn trace_one() {
    let child = fork();
    if child == 0 {
        traced_child();
    }
    step("signal-stop", stopped(child, 10), 21);
    let state = regs(child);
    let mut info = [0u8; 128];
    ptrace(GETSIGINFO, child, 0, info.as_mut_ptr() as usize);
    step(
        "registers",
        state[RIP] != 0 && state[19] != 0 && state[ORIG_RAX] == u64::MAX && state[17] == 0x23,
        22,
    );
    step("siginfo", u32::from_le_bytes([info[0], info[1], info[2], info[3]]) == 10, 23);
    step("bad-option", ptrace(SETOPTIONS, child, 0, 2) == -22, 24);
    step("options", ptrace(SETOPTIONS, child, 0, OPTIONS) == 0, 25);
    step("suppress-signal", ptrace(SYSCALL, child, 0, 0) == 0, 26);
    step("kill-exit-stop", stopped(child, 0x85), 27);
    step("kill-number", regs(child)[ORIG_RAX] == SYS_KILL as u64, 28);
    ptrace(SYSCALL, child, 0, 0);
    step("entry-stop", stopped(child, 0x85), 29);
    let entry = regs(child);
    step("entry-registers", entry[ORIG_RAX] == SYS_GETPID as u64 && entry[RAX] as i64 == -38, 30);
    ptrace(SYSCALL, child, 0, 0);
    step("exit-stop", stopped(child, 0x85), 31);
    let exit = regs(child);
    step("exit-registers", exit[ORIG_RAX] == SYS_GETPID as u64 && exit[RAX] == child as u64, 32);
    step("poke-user", ptrace(POKEUSER, child, RAX * 8, 1234) == 0, 33);
    step("peek-user", {
        let mut word = 0u64;
        ptrace(PEEKUSER, child, RAX * 8, &mut word as *mut u64 as usize) == 0 && word == 1234
    }, 34);
    ptrace(CONT, child, 0, 0);

    step("breakpoint-stop", stopped(child, 5), 40);
    let at = regs(child);
    let code = peek(child, at[RIP] as usize - 1).unwrap_or(0);
    step("peek-code", code & 0xff == 0xcc, 41);
    let flag = &FLAG as *const AtomicU64 as usize;
    step("peek-data", peek(child, flag) == Some(0x1111), 42);
    step("poke-data", ptrace(POKEDATA, child, flag, 0x2222) == 0, 43);
    step(
        "poke-is-private",
        FLAG.load(Ordering::SeqCst) == 0x1111 && peek(child, flag) == Some(0x2222),
        44,
    );
    let function = patch_me as *const () as usize;
    let word = peek(child, function).unwrap_or(0);
    step("poke-code", ptrace(POKETEXT, child, function, ((word & !0xff00) | 0x0700) as usize) == 0, 45);
    step("code-private", unsafe { patch_me() } == 1, 46);
    let mut changed = regs(child);
    step("r12-before", changed[R12] == 0x1234, 47);
    changed[R12] = 0x5678;
    step("set-regs", set_regs(child, &changed) == 0, 48);
    ptrace(SINGLESTEP, child, 0, 0);
    step("step-1", stopped(child, 5) && regs(child)[RIP] == at[RIP] + 1, 49);
    ptrace(SINGLESTEP, child, 0, 0);
    step("step-2", stopped(child, 5) && regs(child)[RIP] == at[RIP] + 2, 50);
    step("flags", regs(child)[EFLAGS] & 0x200 != 0, 51);
    ptrace(CONT, child, 0, 0);

    step("fpu-stop", stopped(child, 5), 52);
    let mut area = [0u8; 512];
    step("get-fpregs", ptrace(GETFPREGS, child, 0, area.as_mut_ptr() as usize) == 0, 53);
    let xmm0 = u64::from_le_bytes(area[160..168].try_into().unwrap());
    step("xmm0", xmm0 == 0x1122_3344_5566_7788, 54);
    area[160..168].copy_from_slice(&0x8877_6655_4433_2211u64.to_le_bytes());
    step("set-fpregs", ptrace(SETFPREGS, child, 0, area.as_ptr() as usize) == 0, 55);
    ptrace(SYSCALL, child, 0, 0);

    step("skip-entry", stopped(child, 0x85), 56);
    let mut skipped = regs(child);
    step("skip-number", skipped[ORIG_RAX] == SYS_GETPPID as u64, 57);
    skipped[ORIG_RAX] = u64::MAX;
    skipped[RAX] = (-77i64) as u64;
    set_regs(child, &skipped);
    ptrace(CONT, child, 0, 0);

    let (result, status) = wait(child);
    step("exec-event", result == child as isize && status == (4 << 16) | (5 << 8) | 0x7f, 58);
    ptrace(CONT, child, 0, 0);
    let (result, status) = wait(child);
    step("exec-exit", result == child as isize && status == 5 << 8, 59);
}

fn fault_stop() {
    let child = fork();
    if child == 0 {
        ptrace(TRACEME, 0, 0, 0);
        unsafe { std::ptr::write_volatile(std::ptr::null_mut::<u8>(), 1) };
        leave(60);
    }
    step("fault-stop", stopped(child, 11), 61);
    ptrace(CONT, child, 0, 11);
    let (result, status) = wait(child);
    step("fault-ends", result == child as isize && status == 11, 62);
}

fn kill_request() {
    let child = fork();
    if child == 0 {
        ptrace(TRACEME, 0, 0, 0);
        let pid = syscall(SYS_GETPID, [0; 4]) as usize;
        syscall(SYS_KILL, [pid, 10, 0, 0]);
        leave(63);
    }
    step("kill-stop", stopped(child, 10), 64);
    step("kill-request", ptrace(KILL, child, 0, 0) == 0, 65);
    let (result, status) = wait(child);
    step("kill-status", result == child as isize && status == 9, 66);
}

fn attach_detach() {
    let child = fork();
    if child == 0 {
        loop {
            syscall(SYS_GETPID, [0; 4]);
            syscall(SYS_SCHED_YIELD, [0; 4]);
        }
    }
    step("not-a-tracee", ptrace(GETREGS, child, 0, 0) == -3, 20);
    step("attach", ptrace(ATTACH, child, 0, 0) == 0, 70);
    step("attach-twice", ptrace(ATTACH, child, 0, 0) == -1, 71);
    step("attach-stop", stopped(child, 19), 72);
    step("attached-regs", regs(child)[RIP] != 0, 73);
    step("detach", ptrace(DETACH, child, 0, 0) == 0, 74);
    step("detached-refused", ptrace(GETREGS, child, 0, 0) == -3, 75);
    syscall(SYS_KILL, [child, 9, 0, 0]);
    let (result, status) = wait(child);
    step("detached-killed", result == child as isize && status == 9, 76);
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("exec") {
        std::process::exit(5);
    }
    trace_one();
    fault_stop();
    kill_request();
    attach_detach();
    println!("AEROS_PTRACE_DONE");
    std::process::exit(77);
}
