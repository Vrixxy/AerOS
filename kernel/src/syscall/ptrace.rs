//! ptrace: one process watching and steering another. A traced task stops in
//! its own context (a signal about to be delivered, a system call entry or
//! exit, an exec, a breakpoint or single-step trap, a fault), keeps the CPU
//! free for others until its tracer resumes it, and takes over whatever
//! register changes the tracer made. The tracer sees the stops through
//! `wait4` and works on the stopped task with `ptrace` requests.

use super::*;
use crate::scheduler::{self, Resume};

const TRACEME: u64 = 0;
const PEEKTEXT: u64 = 1;
const PEEKDATA: u64 = 2;
const PEEKUSER: u64 = 3;
const POKETEXT: u64 = 4;
const POKEDATA: u64 = 5;
const POKEUSER: u64 = 6;
const CONT: u64 = 7;
const KILL: u64 = 8;
const SINGLESTEP: u64 = 9;
const GETREGS: u64 = 12;
const SETREGS: u64 = 13;
const GETFPREGS: u64 = 14;
const SETFPREGS: u64 = 15;
const ATTACH: u64 = 16;
const DETACH: u64 = 17;
const SYSCALL: u64 = 24;
const SETOPTIONS: u64 = 0x4200;
const GETEVENTMSG: u64 = 0x4201;
const GETSIGINFO: u64 = 0x4202;

const O_TRACESYSGOOD: u32 = 1;
const O_TRACEEXEC: u32 = 0x10;
const O_EXITKILL: u32 = 0x10_0000;

const SIGTRAP: u64 = 5;
const SIGSTOP: u64 = 19;
const EVENT_EXEC: u32 = 4;
const TRAP_FLAG: u64 = 1 << 8;
const USER_FLAGS: u64 = 0x0000_0cd5;

const REGISTERS: usize = 27;
const R15: usize = 0;
const R14: usize = 1;
const R13: usize = 2;
const R12: usize = 3;
const RBP: usize = 4;
const RBX: usize = 5;
const R11: usize = 6;
const R10: usize = 7;
const R9: usize = 8;
const R8: usize = 9;
const RAX: usize = 10;
const RCX: usize = 11;
const RDX: usize = 12;
const RSI: usize = 13;
const RDI: usize = 14;
const ORIG_RAX: usize = 15;
const RIP: usize = 16;
const CS: usize = 17;
const EFLAGS: usize = 18;
const RSP: usize = 19;
const SS: usize = 20;
const FS_BASE: usize = 21;
const REGISTER_BYTES: usize = REGISTERS * 8;
const FPU_BYTES: usize = 512;

const KIND_SIGNAL: u8 = 0;
const KIND_ENTRY: u8 = 1;
const KIND_EXIT: u8 = 2;

/// The registers of the task that is about to stop, in whichever form its
/// entry into the kernel left them.
pub(super) enum Context<'a> {
    Syscall(&'a mut LinuxSyscallFrame),
    Interrupt(&'a mut user::UserRegs),
}

impl Context<'_> {
    fn capture(&self, orig_rax: u64, entry: bool) -> [u64; REGISTERS] {
        let mut regs = [0u64; REGISTERS];
        regs[CS] = 0x23;
        regs[SS] = 0x1b;
        regs[ORIG_RAX] = orig_rax;
        regs[FS_BASE] = user::read_fs_base();
        match self {
            Context::Syscall(frame) => {
                regs[R15] = frame.r15;
                regs[R14] = frame.r14;
                regs[R13] = frame.r13;
                regs[R12] = frame.r12;
                regs[RBP] = frame.rbp;
                regs[RBX] = frame.rbx;
                regs[R11] = frame.user_rflags;
                regs[R10] = frame.argument3;
                regs[R9] = frame.argument5;
                regs[R8] = frame.argument4;
                regs[RAX] = if entry { error(38) } else { frame.number };
                regs[RCX] = frame.user_rip;
                regs[RDX] = frame.argument2;
                regs[RSI] = frame.argument1;
                regs[RDI] = frame.argument0;
                regs[RIP] = frame.user_rip;
                regs[EFLAGS] = frame.user_rflags;
                regs[RSP] = user_stack_pointer();
            }
            Context::Interrupt(user_regs) => {
                regs[R15] = user_regs.r15;
                regs[R14] = user_regs.r14;
                regs[R13] = user_regs.r13;
                regs[R12] = user_regs.r12;
                regs[RBP] = user_regs.rbp;
                regs[RBX] = user_regs.rbx;
                regs[R11] = user_regs.r11;
                regs[R10] = user_regs.r10;
                regs[R9] = user_regs.r9;
                regs[R8] = user_regs.r8;
                regs[RAX] = user_regs.rax;
                regs[RCX] = user_regs.rcx;
                regs[RDX] = user_regs.rdx;
                regs[RSI] = user_regs.rsi;
                regs[RDI] = user_regs.rdi;
                regs[RIP] = user_regs.rip;
                regs[EFLAGS] = user_regs.rflags;
                regs[RSP] = user_regs.rsp;
            }
        }
        regs
    }

    /// Puts the tracer's registers back. Returns whether the system call it
    /// stopped at entry is to be skipped (a negative number was left in `orig_rax`).
    fn apply(&mut self, regs: &[u64; REGISTERS], kind: u8) -> bool {
        let flags = (regs[EFLAGS] & USER_FLAGS) | 0x202;
        let mut skip = false;
        match self {
            Context::Syscall(frame) => {
                frame.argument0 = regs[RDI];
                frame.argument1 = regs[RSI];
                frame.argument2 = regs[RDX];
                frame.argument3 = regs[R10];
                frame.argument4 = regs[R8];
                frame.argument5 = regs[R9];
                frame.r15 = regs[R15];
                frame.r14 = regs[R14];
                frame.r13 = regs[R13];
                frame.r12 = regs[R12];
                frame.rbp = regs[RBP];
                frame.rbx = regs[RBX];
                frame.user_rflags = flags | (frame.user_rflags & TRAP_FLAG);
                if user::range_accessible(regs[RIP], 1, false) {
                    frame.user_rip = regs[RIP];
                }
                if kind == KIND_ENTRY {
                    if (regs[ORIG_RAX] as i64) < 0 {
                        skip = true;
                        frame.number = regs[RAX];
                    } else {
                        frame.number = regs[ORIG_RAX];
                    }
                } else {
                    frame.number = regs[RAX];
                }
                set_user_stack_pointer(regs[RSP]);
            }
            Context::Interrupt(user_regs) => {
                user_regs.r15 = regs[R15];
                user_regs.r14 = regs[R14];
                user_regs.r13 = regs[R13];
                user_regs.r12 = regs[R12];
                user_regs.rbp = regs[RBP];
                user_regs.rbx = regs[RBX];
                user_regs.r11 = regs[R11];
                user_regs.r10 = regs[R10];
                user_regs.r9 = regs[R9];
                user_regs.r8 = regs[R8];
                user_regs.rax = regs[RAX];
                user_regs.rcx = regs[RCX];
                user_regs.rdx = regs[RDX];
                user_regs.rsi = regs[RSI];
                user_regs.rdi = regs[RDI];
                user_regs.rflags = flags | (user_regs.rflags & TRAP_FLAG);
                if user::range_accessible(regs[RIP], 1, false) {
                    user_regs.rip = regs[RIP];
                }
                user_regs.rsp = regs[RSP];
            }
        }
        if regs[FS_BASE] != user::read_fs_base() && regs[FS_BASE] <= 0x0000_7fff_ffff_ffff {
            user::write_fs_base(regs[FS_BASE]);
        }
        skip
    }

    fn set_step(&mut self, step: bool) {
        let flags = match self {
            Context::Syscall(frame) => &mut frame.user_rflags,
            Context::Interrupt(regs) => &mut regs.rflags,
        };
        *flags = (*flags & !TRAP_FLAG) | if step { TRAP_FLAG } else { 0 };
    }

    fn in_syscall(&self) -> bool {
        matches!(self, Context::Syscall(_))
    }
}

pub(super) struct Resumed {
    pub signal: u64,
    skip: bool,
}

/// Stops the running task for its tracer and waits to be resumed. Fails when
/// the task has to die instead (its tracer went away with `PTRACE_O_EXITKILL`).
fn stop(
    context: &mut Context,
    kind: u8,
    signal: u64,
    status: u32,
    orig_rax: u64,
) -> Result<Resumed, ()> {
    let regs = context.capture(orig_rax, kind == KIND_ENTRY);
    scheduler::trace_current(|trace| {
        trace.stopped = true;
        trace.reported = false;
        trace.status = status;
        trace.kind = kind;
        trace.stop_signal = signal;
        trace.regs = regs;
        trace.dirty = false;
        trace.resume = Resume::Wait;
        trace.signal = 0;
    });
    loop {
        let tracer = scheduler::trace_current(|trace| trace.stopped.then_some(trace.tracer));
        let Some(tracer) = tracer else {
            break;
        };
        if tracer == 0 || !scheduler::task_is_live(tracer) {
            scheduler::trace_current(|trace| {
                if trace.options & O_EXITKILL != 0 {
                    trace.resume = Resume::Kill;
                } else if trace.resume == Resume::Wait {
                    trace.resume = Resume::Continue;
                }
                trace.tracer = 0;
                trace.syscall_stops = false;
                trace.stopped = false;
            });
            break;
        }
        if context.in_syscall() {
            yield_in_syscall();
        } else {
            scheduler::yield_now();
        }
    }
    let (resume, injected, dirty, regs) = scheduler::trace_current(|trace| {
        trace.stopped = false;
        (trace.resume, trace.signal, trace.dirty, trace.regs)
    });
    if resume == Resume::Kill {
        return Err(());
    }
    let skip = dirty && context.apply(&regs, kind);
    context.set_step(resume == Resume::Step);
    Ok(Resumed {
        signal: injected,
        skip,
    })
}

fn signal_status(signal: u64) -> u32 {
    ((signal as u32) << 8) | 0x7f
}

fn syscall_status() -> u32 {
    let sysgood = scheduler::trace_current(|trace| trace.options & O_TRACESYSGOOD != 0);
    signal_status(SIGTRAP | if sysgood { 0x80 } else { 0 })
}

/// A signal is about to be delivered to a traced task: the tracer decides
/// whether it still is (and which one). `Ok(0)` means it was suppressed.
pub(super) fn signal_stop(context: &mut Context, signal: u64) -> Result<u64, ()> {
    stop(
        context,
        KIND_SIGNAL,
        signal,
        signal_status(signal),
        u64::MAX,
    )
    .map(|resumed| resumed.signal)
}

fn die() -> u64 {
    EXITS.fetch_add(1, Ordering::Relaxed);
    user::set_exit_code(128 + 9);
    1
}

/// The system call path of a traced task: stops at entry and exit when the
/// tracer asked for them, and after a successful `execve`.
pub(super) fn traced_syscall(frame: &mut LinuxSyscallFrame) -> u64 {
    let entered = frame.number;
    if scheduler::trace_current(|trace| trace.syscall_stops) {
        let status = syscall_status();
        match stop(
            &mut Context::Syscall(frame),
            KIND_ENTRY,
            SIGTRAP,
            status,
            entered,
        ) {
            Err(()) => return die(),
            Ok(resumed) if resumed.skip => return finish_syscall(frame),
            Ok(_) => {}
        }
    }
    let number = frame.number;
    let outcome = linux_syscall(frame);
    if outcome != 0 || number == LINUX_RT_SIGRETURN {
        return outcome;
    }
    if number == LINUX_EXECVE && frame.number == 0 && scheduler::is_traced() {
        let event = scheduler::trace_current(|trace| trace.options & O_TRACEEXEC != 0);
        let status = if event {
            (EVENT_EXEC << 16) | signal_status(SIGTRAP)
        } else {
            signal_status(SIGTRAP)
        };
        if stop(
            &mut Context::Syscall(frame),
            KIND_SIGNAL,
            SIGTRAP,
            status,
            number,
        )
        .is_err()
        {
            return die();
        }
    }
    if scheduler::trace_current(|trace| trace.tracer != 0 && trace.syscall_stops) {
        let status = syscall_status();
        if stop(
            &mut Context::Syscall(frame),
            KIND_EXIT,
            SIGTRAP,
            status,
            number,
        )
        .is_err()
        {
            return die();
        }
    }
    if frame.user_rflags & TRAP_FLAG != 0 && scheduler::is_traced() {
        // Stepping over a system call reports the stop right behind it.
        frame.user_rflags &= !TRAP_FLAG;
        let status = signal_status(SIGTRAP);
        if stop(
            &mut Context::Syscall(frame),
            KIND_SIGNAL,
            SIGTRAP,
            status,
            number,
        )
        .is_err()
        {
            return die();
        }
    }
    0
}

pub enum Trap {
    /// Carry on at the (possibly changed) registers.
    Resume,
    /// The task must exit with this status.
    Exit(u64),
    /// Nothing the tracer did cancels the fault: it ends the task as usual.
    Fault,
}

/// A breakpoint, single-step or fault trap of a traced task in user mode.
pub(super) fn trap(regs: &mut user::UserRegs, signal: u64, fault: bool) -> Trap {
    match signal_stop(&mut Context::Interrupt(regs), signal) {
        Err(()) => Trap::Exit(128 + 9),
        Ok(0) => Trap::Resume,
        Ok(_) if fault => Trap::Fault,
        Ok(_) => Trap::Resume,
    }
}

/// What `wait4` finds about task `id`, if `group` traces it.
pub(super) enum Event {
    Stopped(u32),
    Exited(u64),
}

pub(super) fn poll(group: u64, id: u64) -> Option<Event> {
    let stopped = scheduler::trace_of(id, |trace| {
        if trace.tracer != group {
            return None;
        }
        if trace.stopped && !trace.reported {
            trace.reported = true;
            return Some(trace.status);
        }
        None
    })
    .flatten();
    if let Some(status) = stopped {
        return Some(Event::Stopped(status));
    }
    let traced = scheduler::trace_of(id, |trace| trace.tracer == group) == Some(true);
    if traced && scheduler::parent_of(id) != Some(group) {
        let code = scheduler::exit_status_of(id)?;
        scheduler::trace_of(id, |trace| *trace = scheduler::Trace::NONE);
        return Some(Event::Exited(code));
    }
    None
}

/// The tasks `wait4(-1)` has to look at: children and tracees.
pub(super) fn candidates(group: u64, out: &mut [u64]) -> usize {
    let mut count = scheduler::child_ids(group, out);
    scheduler::for_each_tracee(group, |id, _, _| {
        if count < out.len() && !out[..count].contains(&id) {
            out[count] = id;
            count += 1;
        }
    });
    count
}

fn read_memory(tid: u64, address: u64, out: &mut [u8]) -> bool {
    let mut done = 0;
    while done < out.len() {
        let at = address.wrapping_add(done as u64);
        let amount = ((4096 - at % 4096) as usize).min(out.len() - done);
        let Some(physical) = scheduler::with_task_space(tid, |space| {
            crate::arch::paging::process_swap_in(space, at);
            crate::arch::paging::process_translate(space, at)
        })
        .flatten() else {
            return false;
        };
        // SAFETY: the physical page belongs to the traced process and memory is
        // identity-mapped for the kernel.
        unsafe {
            core::ptr::copy_nonoverlapping(
                physical as usize as *const u8,
                out[done..].as_mut_ptr(),
                amount,
            );
        }
        done += amount;
    }
    true
}

fn write_memory(tid: u64, address: u64, data: &[u8]) -> bool {
    let mut done = 0;
    while done < data.len() {
        let at = address.wrapping_add(done as u64);
        let amount = ((4096 - at % 4096) as usize).min(data.len() - done);
        let Some(physical) = scheduler::with_task_space(tid, |space| {
            crate::arch::paging::process_swap_in(space, at);
            if !crate::arch::paging::process_make_private(space, at) {
                return None;
            }
            crate::arch::paging::process_translate(space, at)
        })
        .flatten() else {
            return false;
        };
        // SAFETY: as in `read_memory`; the page is the traced process's own copy now.
        unsafe {
            core::ptr::copy_nonoverlapping(
                data[done..].as_ptr(),
                physical as usize as *mut u8,
                amount,
            );
        }
        done += amount;
    }
    true
}

fn traceme() -> u64 {
    let parent = scheduler::parent_group();
    if parent == 0 {
        return error(1);
    }
    scheduler::trace_current(|trace| {
        if trace.tracer != 0 {
            return error(1);
        }
        *trace = scheduler::Trace::NONE;
        trace.tracer = parent;
        0
    })
}

fn attach(pid: u64) -> u64 {
    let me = scheduler::current_group();
    if pid == 0 || pid == me {
        return error(1);
    }
    if scheduler::group_of_tid(pid) != Some(pid) {
        return error(3);
    }
    if scheduler::parent_of(pid) != Some(me) && !has_capability(crate::capability::CAP_SYS_PTRACE) {
        return error(1);
    }
    let attached = scheduler::trace_of(pid, |trace| {
        if trace.tracer != 0 {
            return false;
        }
        *trace = scheduler::Trace::NONE;
        trace.tracer = me;
        true
    });
    if attached != Some(true) {
        return error(1);
    }
    match scheduler::signal_other_task(pid, SIGSTOP) {
        Some(_) => 0,
        None => {
            scheduler::trace_of(pid, |trace| *trace = scheduler::Trace::NONE);
            error(3)
        }
    }
}

fn read_user(address: u64, out: &mut [u8]) -> bool {
    user::copy_from_user(address, out)
}

fn words(regs: &[u64; REGISTERS]) -> [u8; REGISTER_BYTES] {
    let mut bytes = [0u8; REGISTER_BYTES];
    for (index, word) in regs.iter().enumerate() {
        bytes[index * 8..index * 8 + 8].copy_from_slice(&word.to_le_bytes());
    }
    bytes
}

pub(super) fn ptrace(request: u64, pid: u64, address: u64, data: u64) -> u64 {
    match request {
        TRACEME => return traceme(),
        ATTACH => return attach(pid),
        _ => {}
    }
    let me = scheduler::current_group();
    let state = scheduler::trace_of(pid, |trace| {
        (trace.tracer == me && trace.stopped).then_some((trace.regs, trace.stop_signal))
    })
    .flatten();
    let Some((regs, stop_signal)) = state else {
        return error(3);
    };
    let resume = |how: Resume, syscall_stops: bool| {
        scheduler::trace_of(pid, |trace| {
            trace.resume = how;
            trace.syscall_stops = syscall_stops;
            trace.signal = if (1..=64).contains(&data) { data } else { 0 };
            trace.stopped = false;
        });
        0
    };
    match request {
        PEEKTEXT | PEEKDATA => {
            let mut word = [0u8; 8];
            if !read_memory(pid, address, &mut word) {
                return error(5);
            }
            if user::copy_to_user(data, &word) {
                0
            } else {
                error(14)
            }
        }
        POKETEXT | POKEDATA => {
            if write_memory(pid, address, &data.to_le_bytes()) {
                0
            } else {
                error(5)
            }
        }
        PEEKUSER => {
            if !address.is_multiple_of(8) || address as usize >= REGISTER_BYTES {
                return error(5);
            }
            let word = regs[address as usize / 8];
            if user::copy_to_user(data, &word.to_le_bytes()) {
                0
            } else {
                error(14)
            }
        }
        POKEUSER => {
            if !address.is_multiple_of(8) || address as usize >= REGISTER_BYTES {
                return error(5);
            }
            scheduler::trace_of(pid, |trace| {
                trace.regs[address as usize / 8] = data;
                trace.dirty = true;
            });
            0
        }
        GETREGS => {
            if user::copy_to_user(data, &words(&regs)) {
                0
            } else {
                error(14)
            }
        }
        SETREGS => {
            let mut bytes = [0u8; REGISTER_BYTES];
            if !read_user(data, &mut bytes) {
                return error(14);
            }
            scheduler::trace_of(pid, |trace| {
                for (index, word) in trace.regs.iter_mut().enumerate() {
                    *word = u64::from_le_bytes(
                        bytes[index * 8..index * 8 + 8].try_into().unwrap_or([0; 8]),
                    );
                }
                trace.dirty = true;
            });
            0
        }
        GETFPREGS | SETFPREGS => {
            let Some((base, size)) = scheduler::fpu_area_of(pid) else {
                return error(3);
            };
            if base == 0 || size < FPU_BYTES {
                return error(5);
            }
            // SAFETY: the saved floating-point area of a switched-out task has
            // at least the 512 bytes of the legacy FXSAVE image.
            let area = unsafe { core::slice::from_raw_parts_mut(base as *mut u8, FPU_BYTES) };
            if request == GETFPREGS {
                if user::copy_to_user(data, area) {
                    0
                } else {
                    error(14)
                }
            } else if read_user(data, area) {
                0
            } else {
                error(14)
            }
        }
        CONT => resume(Resume::Continue, false),
        SYSCALL => resume(Resume::Syscall, true),
        SINGLESTEP => resume(Resume::Step, false),
        KILL => match scheduler::kill_task(pid) {
            Ok(()) => 0,
            Err(()) => error(3),
        },
        DETACH => {
            scheduler::trace_of(pid, |trace| {
                let signal = if (1..=64).contains(&data) { data } else { 0 };
                *trace = scheduler::Trace {
                    resume: Resume::Continue,
                    signal,
                    regs: trace.regs,
                    dirty: trace.dirty,
                    ..scheduler::Trace::NONE
                };
            });
            0
        }
        SETOPTIONS => {
            let allowed = (O_TRACESYSGOOD | O_TRACEEXEC | O_EXITKILL) as u64;
            if data & !allowed != 0 {
                return error(22);
            }
            scheduler::trace_of(pid, |trace| trace.options = data as u32);
            0
        }
        GETEVENTMSG => {
            if user::copy_to_user(data, &0u64.to_le_bytes()) {
                0
            } else {
                error(14)
            }
        }
        GETSIGINFO => {
            let mut info = [0u8; 128];
            info[..4].copy_from_slice(&(stop_signal as u32).to_le_bytes());
            let code: u32 = if stop_signal == SIGTRAP { 0x80 } else { 0 };
            info[8..12].copy_from_slice(&code.to_le_bytes());
            if user::copy_to_user(data, &info) {
                0
            } else {
                error(14)
            }
        }
        _ => error(5),
    }
}
