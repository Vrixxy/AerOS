use core::alloc::Layout;
use core::arch::{asm, naked_asm};
use core::cell::UnsafeCell;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::{arch, heap};

const MAX_TASKS: usize = 32;
const STACK_SIZE: usize = 64 * 1024;
const STACK_ALIGNMENT: usize = 16;
const INTERRUPT_STACK_SIZE: usize = 48 * 1024;
const RSP0_CPU: usize = 0;

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum TaskState {
    Empty,
    Ready,
    Running,
    Exited,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Context {
    rsp: u64,
}

#[derive(Clone, Copy)]
struct Task {
    id: u64,
    state: TaskState,
    context: Context,
    stack_base: usize,
    stack_size: usize,
    fpu_base: usize,
    fpu_size: usize,
    switches: u64,
    user_context: arch::user::UserTaskContext,
    user_entry: u64,
    user_stack: u64,
    user_start: u64,
    user_end: u64,
    interrupt_stack_base: usize,
    interrupt_stack_top: u64,
    cr3: u64,
    process_space: Option<arch::paging::ProcessAddressSpace>,
    fork_snapshot: Option<arch::user::ForkSnapshot>,
    parent_id: u64,
    process_state: crate::syscall::ProcessState,
}

impl Task {
    const EMPTY: Self = Self {
        id: 0,
        state: TaskState::Empty,
        context: Context { rsp: 0 },
        stack_base: 0,
        stack_size: 0,
        fpu_base: 0,
        fpu_size: 0,
        switches: 0,
        user_context: arch::user::UserTaskContext::EMPTY,
        user_entry: 0,
        user_stack: 0,
        user_start: 0,
        user_end: 0,
        interrupt_stack_base: 0,
        interrupt_stack_top: 0,
        cr3: 0,
        process_space: None,
        fork_snapshot: None,
        parent_id: 0,
        process_state: crate::syscall::ProcessState::EMPTY,
    };
}

struct Scheduler {
    tasks: [Task; MAX_TASKS],
    current: usize,
    next_id: u64,
    initialized: bool,
    context_switches: u64,
    fpu_template_base: usize,
    fpu_state_bytes: usize,
    fpu_switches: u64,
}

impl Scheduler {
    const fn empty() -> Self {
        Self {
            tasks: [Task::EMPTY; MAX_TASKS],
            current: 0,
            next_id: 1,
            initialized: false,
            context_switches: 0,
            fpu_template_base: 0,
            fpu_state_bytes: 0,
            fpu_switches: 0,
        }
    }

    fn initialize(&mut self, main_fpu: usize, template_fpu: usize, fpu_state_bytes: usize) {
        self.tasks = [Task::EMPTY; MAX_TASKS];
        self.tasks[0] = Task {
            id: 0,
            state: TaskState::Running,
            context: Context { rsp: 0 },
            stack_base: 0,
            stack_size: 0,
            fpu_base: main_fpu,
            fpu_size: fpu_state_bytes,
            switches: 0,
            user_context: arch::user::UserTaskContext::EMPTY,
            user_entry: 0,
            user_stack: 0,
            user_start: 0,
            user_end: 0,
            interrupt_stack_base: 0,
            interrupt_stack_top: arch::gdt::default_ring0_stack(RSP0_CPU),
            cr3: 0,
            process_space: None,
            fork_snapshot: None,
            parent_id: 0,
            process_state: crate::syscall::ProcessState::EMPTY,
        };
        self.current = 0;
        self.next_id = 1;
        self.initialized = true;
        self.context_switches = 0;
        self.fpu_template_base = template_fpu;
        self.fpu_state_bytes = fpu_state_bytes;
        self.fpu_switches = 0;
        arch::gdt::set_ring0_stack(RSP0_CPU, self.tasks[0].interrupt_stack_top);
    }

    fn next_ready(&self) -> Option<usize> {
        for distance in 1..=MAX_TASKS {
            let index = (self.current + distance) % MAX_TASKS;
            if self.tasks[index].state == TaskState::Ready {
                return Some(index);
            }
        }
        None
    }
}

#[repr(align(64))]
struct SchedulerStorage(UnsafeCell<Scheduler>);

unsafe impl Sync for SchedulerStorage {}

static SCHEDULER: SchedulerStorage = SchedulerStorage(UnsafeCell::new(Scheduler::empty()));
static TASK_A_PHASE: AtomicU32 = AtomicU32::new(0);
static TASK_B_PHASE: AtomicU32 = AtomicU32::new(0);
static FPU_A_VALID: AtomicU32 = AtomicU32::new(0);
static FPU_B_VALID: AtomicU32 = AtomicU32::new(0);
static PREEMPT_A_PHASE: AtomicU32 = AtomicU32::new(0);
static PREEMPT_B_PHASE: AtomicU32 = AtomicU32::new(0);
static PREEMPT_A_WORK: AtomicU32 = AtomicU32::new(0);
static PREEMPT_B_WORK: AtomicU32 = AtomicU32::new(0);
static PREEMPT_FPU_A_VALID: AtomicU32 = AtomicU32::new(0);
static PREEMPT_FPU_B_VALID: AtomicU32 = AtomicU32::new(0);
static PREEMPT_TICKS: AtomicU32 = AtomicU32::new(0);
static PREEMPTION_ENABLED: AtomicU32 = AtomicU32::new(0);
static USER_TASK_EXIT: [AtomicU64; MAX_TASKS] = [const { AtomicU64::new(u64::MAX) }; MAX_TASKS];
static DEFAULT_CR3: AtomicU64 = AtomicU64::new(0);
static ACTIVE_CR3: AtomicU64 = AtomicU64::new(0);

pub fn set_default_cr3(root_physical: u64) {
    DEFAULT_CR3.store(root_physical, Ordering::Release);
    ACTIVE_CR3.store(root_physical, Ordering::Release);
}

fn switch_cr3(target: &Task) {
    let root = if target.cr3 != 0 {
        target.cr3
    } else {
        DEFAULT_CR3.load(Ordering::Acquire)
    };
    if ACTIVE_CR3.load(Ordering::Acquire) != root {
        arch::paging::activate_root(root);
        ACTIVE_CR3.store(root, Ordering::Release);
    }
}

#[derive(Clone, Copy)]
pub struct SchedulerStats {
    pub tasks: usize,
    pub ready: usize,
    pub running: usize,
    pub exited: usize,
    pub context_switches: u64,
    pub stack_bytes: usize,
    pub fpu_tasks: usize,
    pub fpu_bytes: usize,
    pub fpu_switches: u64,
    pub fpu_isolation: bool,
    pub highest_task_id: u64,
}

fn release_scheduler_memory(scheduler: &mut Scheduler) {
    for task in &scheduler.tasks {
        if task.stack_base != 0 {
            let released = NonNull::new(task.stack_base as *mut u8)
                .is_some_and(|pointer| heap::HEAP.deallocate(pointer));
            if !released {
                arch::halt_forever();
            }
        }
        if task.fpu_base != 0 {
            let released = NonNull::new(task.fpu_base as *mut u8)
                .is_some_and(|pointer| heap::HEAP.deallocate(pointer));
            if !released {
                arch::halt_forever();
            }
        }
        if task.interrupt_stack_base != 0 {
            let released = NonNull::new(task.interrupt_stack_base as *mut u8)
                .is_some_and(|pointer| heap::HEAP.deallocate(pointer));
            if !released {
                arch::halt_forever();
            }
        }
        if let Some(space) = task.process_space.as_ref()
            && !arch::paging::destroy_process(space)
        {
            arch::halt_forever();
        }
    }
    if scheduler.fpu_template_base != 0 {
        let released = NonNull::new(scheduler.fpu_template_base as *mut u8)
            .is_some_and(|pointer| heap::HEAP.deallocate(pointer));
        if !released {
            arch::halt_forever();
        }
    }
}

pub fn initialize() {
    arch::disable_interrupts();
    // Every self-test in this chain calls initialize() to start from a
    // clean scheduler, but that only resets the Task array -- it doesn't
    // touch syscall.rs's live Linux-ABI statics (fd table, cwd, signal
    // state), which task 0 (this very call site) will pick back up as its
    // own state on the very next context switch. Reset those too, so one
    // self-test's Linux-ABI syscalls (chdir, open, sigaction, ...) can't
    // leak into the next one's task 0.
    crate::syscall::reset_scheduled_process_state();
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    release_scheduler_memory(scheduler);
    *scheduler = Scheduler::empty();
    let fpu_state_bytes = arch::fpu::context_state_bytes();
    let Ok(fpu_layout) = Layout::from_size_align(fpu_state_bytes, arch::fpu::context_alignment())
    else {
        return;
    };
    let Some(main_fpu) = heap::HEAP.allocate(fpu_layout) else {
        return;
    };
    let Some(template_fpu) = heap::HEAP.allocate(fpu_layout) else {
        heap::HEAP.deallocate(main_fpu);
        return;
    };
    let initialized = unsafe {
        arch::fpu::save_context(main_fpu.as_ptr())
            && arch::fpu::reset_context()
            && arch::fpu::save_context(template_fpu.as_ptr())
            && arch::fpu::restore_context(main_fpu.as_ptr())
    };
    if !initialized {
        heap::HEAP.deallocate(template_fpu);
        heap::HEAP.deallocate(main_fpu);
        return;
    }
    scheduler.initialize(
        main_fpu.as_ptr() as usize,
        template_fpu.as_ptr() as usize,
        fpu_state_bytes,
    );
}

pub fn spawn(entry: extern "C" fn() -> !) -> Option<u64> {
    arch::disable_interrupts();
    let stack_layout = Layout::from_size_align(STACK_SIZE, STACK_ALIGNMENT).ok()?;
    let stack = heap::HEAP.allocate(stack_layout)?;
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    if !scheduler.initialized {
        heap::HEAP.deallocate(stack);
        return None;
    }
    let Some(slot) = scheduler
        .tasks
        .iter()
        .position(|task| task.state == TaskState::Empty)
    else {
        heap::HEAP.deallocate(stack);
        return None;
    };
    let Ok(fpu_layout) =
        Layout::from_size_align(scheduler.fpu_state_bytes, arch::fpu::context_alignment())
    else {
        heap::HEAP.deallocate(stack);
        return None;
    };
    let Some(fpu) = heap::HEAP.allocate(fpu_layout) else {
        heap::HEAP.deallocate(stack);
        return None;
    };
    let Ok(interrupt_layout) = Layout::from_size_align(INTERRUPT_STACK_SIZE, STACK_ALIGNMENT)
    else {
        heap::HEAP.deallocate(fpu);
        heap::HEAP.deallocate(stack);
        return None;
    };
    let Some(interrupt_stack) = heap::HEAP.allocate(interrupt_layout) else {
        heap::HEAP.deallocate(fpu);
        heap::HEAP.deallocate(stack);
        return None;
    };
    let stack_base = stack.as_ptr() as usize;
    let Some(stack_end) = stack_base.checked_add(STACK_SIZE) else {
        heap::HEAP.deallocate(interrupt_stack);
        heap::HEAP.deallocate(fpu);
        heap::HEAP.deallocate(stack);
        return None;
    };
    let Some(fake_return) = stack_end.checked_sub(8) else {
        heap::HEAP.deallocate(interrupt_stack);
        heap::HEAP.deallocate(fpu);
        heap::HEAP.deallocate(stack);
        return None;
    };
    let Some(initial_rsp) = fake_return.checked_sub(72) else {
        heap::HEAP.deallocate(interrupt_stack);
        heap::HEAP.deallocate(fpu);
        heap::HEAP.deallocate(stack);
        return None;
    };
    let interrupt_stack_base = interrupt_stack.as_ptr() as usize;
    let Some(interrupt_stack_top) = interrupt_stack_base.checked_add(INTERRUPT_STACK_SIZE) else {
        heap::HEAP.deallocate(interrupt_stack);
        heap::HEAP.deallocate(fpu);
        heap::HEAP.deallocate(stack);
        return None;
    };
    unsafe {
        core::ptr::copy_nonoverlapping(
            scheduler.fpu_template_base as *const u8,
            fpu.as_ptr(),
            scheduler.fpu_state_bytes,
        );
        core::ptr::write_bytes(initial_rsp as *mut u8, 0, 64);
        core::ptr::write(
            (initial_rsp + 24) as *mut usize,
            entry as *const () as usize,
        );
        core::ptr::write(
            (initial_rsp + 64) as *mut usize,
            task_entry_trampoline as *const () as usize,
        );
        core::ptr::write(
            fake_return as *mut usize,
            task_return_guard as *const () as usize,
        );
    }
    let id = scheduler.next_id;
    scheduler.next_id = scheduler.next_id.wrapping_add(1).max(1);
    scheduler.tasks[slot] = Task {
        id,
        state: TaskState::Ready,
        context: Context {
            rsp: initial_rsp as u64,
        },
        stack_base,
        stack_size: STACK_SIZE,
        fpu_base: fpu.as_ptr() as usize,
        fpu_size: scheduler.fpu_state_bytes,
        switches: 0,
        user_context: arch::user::UserTaskContext::EMPTY,
        user_entry: 0,
        user_stack: 0,
        user_start: 0,
        user_end: 0,
        interrupt_stack_base,
        interrupt_stack_top: interrupt_stack_top as u64,
        cr3: 0,
        process_space: None,
        fork_snapshot: None,
        parent_id: 0,
        process_state: crate::syscall::ProcessState::EMPTY,
    };
    Some(id)
}

pub fn spawn_user(entry: u64, stack: u64, user_start: u64, user_end: u64) -> Option<(u64, usize)> {
    let id = spawn(user_task_trampoline)?;
    arch::disable_interrupts();
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    let slot = scheduler.tasks.iter().position(|task| task.id == id)?;
    scheduler.tasks[slot].user_entry = entry;
    scheduler.tasks[slot].user_stack = stack;
    scheduler.tasks[slot].user_start = user_start;
    scheduler.tasks[slot].user_end = user_end;
    Some((id, slot))
}

pub fn spawn_process(
    state: &arch::paging::PagingState,
    code: &[u8],
) -> Option<(u64, usize, arch::paging::ProcessAddressSpace)> {
    spawn_process_with(state, code, "/bin/program", &[b"/bin/program"])
}

/// `spawn_process` with an explicit program name and argv (a real ELF gets
/// them on its initial stack; raw probes ignore both).
pub fn spawn_process_with(
    state: &arch::paging::PagingState,
    code: &[u8],
    path: &str,
    args: &[&[u8]],
) -> Option<(u64, usize, arch::paging::ProcessAddressSpace)> {
    // On-execute protection: a detected image never gets an address space.
    if crate::antivirus::blocks_exec(code) {
        return None;
    }
    let mut space = arch::paging::create_process(state, code)?;
    if !space.verified {
        arch::paging::destroy_process(&space);
        return None;
    }
    // Every existing self-test probe is a raw machine-code blob that fails
    // ELF parsing (see `create_process`'s own dispatch) and must keep its
    // hand-tuned stack layout untouched; a real ELF image, on the other
    // hand, needs the standard argc/argv/envp/auxv contract set up before
    // it runs, or any std-linked binary's runtime init segfaults reading
    // `AT_RANDOM` off what would otherwise be a zeroed stack.
    if let Ok(image) = crate::elf::ElfImage::parse(code)
        && !crate::process::prepare_scheduled_process_stack_with(
            &mut space,
            &image,
            path,
            args,
            &[],
        )
    {
        arch::paging::destroy_process(&space);
        return None;
    }
    let user_start = arch::paging::process_virtual_base();
    let user_end = arch::paging::process_reserved_end(user_start);
    let Some((id, slot)) = spawn_user(space.entry, space.stack_top, user_start, user_end) else {
        arch::paging::destroy_process(&space);
        return None;
    };
    arch::disable_interrupts();
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    scheduler.tasks[slot].cr3 = space.root_physical;
    scheduler.tasks[slot].process_space = Some(space);
    Some((id, slot, space))
}

pub fn fork_current_user_task(snapshot: &arch::user::ForkSnapshot) -> Option<u64> {
    arch::disable_interrupts();
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    let current = scheduler.current;
    let parent_task_id = scheduler.tasks[current].id;
    let parent_space = scheduler.tasks[current].process_space?;
    let user_start = scheduler.tasks[current].user_start;
    let user_end = scheduler.tasks[current].user_end;
    // The parent is still "current", so its live Linux-ABI process state
    // (fd table, cwd, signal handlers) lives in syscall.rs's shared statics
    // right now, not in `scheduler.tasks[current].process_state` (that copy
    // is only refreshed on a context switch) -- snapshot it fresh here so
    // the child inherits an accurate, independent copy.
    let parent_process_state = crate::syscall::save_process_state();
    let child_space = arch::paging::fork_process(&parent_space)?;
    if !child_space.verified {
        arch::paging::destroy_process(&child_space);
        return None;
    }
    let Some(id) = spawn(forked_task_trampoline) else {
        arch::paging::destroy_process(&child_space);
        return None;
    };
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    let Some(slot) = scheduler.tasks.iter().position(|task| task.id == id) else {
        arch::paging::destroy_process(&child_space);
        return None;
    };
    scheduler.tasks[slot].cr3 = child_space.root_physical;
    scheduler.tasks[slot].process_space = Some(child_space);
    scheduler.tasks[slot].fork_snapshot = Some(*snapshot);
    scheduler.tasks[slot].user_start = user_start;
    scheduler.tasks[slot].user_end = user_end;
    scheduler.tasks[slot].parent_id = parent_task_id;
    scheduler.tasks[slot].process_state = parent_process_state;
    crate::syscall::clear_pending_signals(&mut scheduler.tasks[slot].process_state);
    Some(id)
}

pub fn current_task_id() -> u64 {
    arch::disable_interrupts();
    let scheduler = unsafe { &*SCHEDULER.0.get() };
    scheduler.tasks[scheduler.current].id
}

/// Copy-on-write fault for the task that is running right now.
pub fn handle_cow_fault_current(address: u64) -> bool {
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    if !scheduler.initialized {
        return false;
    }
    let current = scheduler.current;
    let Some(space) = scheduler.tasks[current].process_space.as_mut() else {
        return false;
    };
    arch::paging::process_cow_fault(space, address)
}

/// Sends `signal` to another live task's stored process state.
pub fn signal_other_task(id: u64, signal: u64) -> Option<crate::syscall::RemoteSignal> {
    arch::disable_interrupts();
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    if id == 0 || !(1..=64).contains(&signal) {
        return None;
    }
    let slot = scheduler
        .tasks
        .iter()
        .position(|task| task.id == id && task.state != TaskState::Empty)?;
    if slot == scheduler.current {
        return None;
    }
    Some(crate::syscall::signal_stored_state(
        &mut scheduler.tasks[slot].process_state,
        signal,
    ))
}

pub fn task_exists(id: u64) -> bool {
    arch::disable_interrupts();
    let scheduler = unsafe { &*SCHEDULER.0.get() };
    id != 0
        && scheduler
            .tasks
            .iter()
            .any(|task| task.id == id && task.state != TaskState::Empty)
}

pub fn any_child_id() -> Option<u64> {
    arch::disable_interrupts();
    let scheduler = unsafe { &*SCHEDULER.0.get() };
    let parent = scheduler.tasks[scheduler.current].id;
    scheduler
        .tasks
        .iter()
        .find(|task| {
            task.state != TaskState::Empty && task.parent_id == parent && task.id != parent
        })
        .map(|task| task.id)
}

pub fn current_parent_id() -> u64 {
    arch::disable_interrupts();
    let scheduler = unsafe { &*SCHEDULER.0.get() };
    scheduler.tasks[scheduler.current].parent_id
}

/// Real Linux-ABI `getpid()`/`getppid()` for the current task, mirroring
/// `linux_brk_for_current_task`'s fallback contract: `None` means "we are
/// not actually executing as a scheduler-managed `process_space` task
/// right now" (e.g. the boot-time single-shot `run()`/`run_image()` path,
/// which never calls `scheduler::initialize()` at all), so the caller
/// should fall back to the legacy `process::current_pid()` singleton those
/// callers already rely on instead. Gated on `process_space.is_some()`,
/// not just `scheduler.initialized`, because the bootstrap-ABI-only
/// concurrent-scheduling self-tests (`spawn_user`, no `process_space`)
/// have no Linux-ABI identity to report either.
pub fn current_task_pid_for_linux() -> Option<u64> {
    arch::disable_interrupts();
    let scheduler = unsafe { &*SCHEDULER.0.get() };
    if !scheduler.initialized {
        return None;
    }
    let task = &scheduler.tasks[scheduler.current];
    task.process_space.is_some().then_some(task.id)
}

/// The real Linux-ABI `getppid()` counterpart to
/// `current_task_pid_for_linux` - same fallback contract.
pub fn current_parent_pid_for_linux() -> Option<u64> {
    arch::disable_interrupts();
    let scheduler = unsafe { &*SCHEDULER.0.get() };
    if !scheduler.initialized {
        return None;
    }
    let task = &scheduler.tasks[scheduler.current];
    task.process_space.is_some().then_some(task.parent_id)
}

pub fn kill_task(target_id: u64) -> Result<(), ()> {
    if target_id == 0 {
        return Err(());
    }
    arch::disable_interrupts();
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    let Some(slot) = scheduler.tasks.iter().position(|task| task.id == target_id) else {
        return Err(());
    };
    if slot == scheduler.current || scheduler.tasks[slot].state != TaskState::Ready {
        return Err(());
    }
    scheduler.tasks[slot].state = TaskState::Exited;
    USER_TASK_EXIT[slot].store(137, Ordering::Release);
    Ok(())
}

/// Non-blocking `wait_for_child`: `None` = no such task, `Some(None)` = still
/// running, `Some(Some(code))` = it had exited and has now been reaped.
pub fn poll_child(child_id: u64) -> Option<Option<u64>> {
    let slot = slot_for_id(child_id)?;
    let exit_code = USER_TASK_EXIT[slot].load(Ordering::Acquire);
    if exit_code == u64::MAX {
        return Some(None);
    }
    reap();
    Some(Some(exit_code))
}

pub fn wait_for_child(child_id: u64) -> Option<u64> {
    let slot = slot_for_id(child_id)?;
    loop {
        let exit_code = USER_TASK_EXIT[slot].load(Ordering::Acquire);
        if exit_code != u64::MAX {
            reap();
            return Some(exit_code);
        }
        if !yield_now() {
            return None;
        }
    }
}

/// Real Linux-ABI brk(): `request` is an ABSOLUTE target address (0 = just
/// query the current brk), unlike `grow_current_heap`'s page-count delta.
/// Returns None when the current task has no `process_space` at all (the
/// bootstrap-ABI-only tasks from the concurrent-scheduling milestone, or
/// kernel-only tasks), signalling the caller to fall back to the original
/// singleton `arch::paging::user_brk` path used by the single-shot ELF
/// loader. Shrinking isn't supported (`process_brk` only ever grows) --
/// a request below the current brk just reports the current brk unchanged,
/// the same conservative scoping already used for close-on-exec handles.
pub fn linux_brk_for_current_task(request: u64) -> Option<u64> {
    arch::disable_interrupts();
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    let current = scheduler.current;
    let space = scheduler.tasks[current].process_space.as_mut()?;
    let current_brk = arch::paging::process_brk(space, 0)?;
    if request == 0 || request <= current_brk {
        return Some(current_brk);
    }
    let additional_pages = (request - current_brk).div_ceil(4096) as usize;
    arch::paging::process_brk(space, additional_pages).or(Some(current_brk))
}

/// True if some OTHER live task (not the caller) has an fd table entry
/// `matches` accepts. This is what makes closing a real vfs handle safe
/// across `fork()`: within one process, `same_open_description` already
/// scans that process's own live fd table before actually releasing a
/// handle (a `dup()`'d fd doesn't invalidate its sibling); this extends the
/// exact same idea across processes, since a forked child's fd table
/// entries carry the SAME underlying vfs handle numbers as its parent's,
/// but each process's table only exists in `syscall.rs`'s shared statics
/// while that process is the one actually running - every other task's
/// copy sits in its own `Task::process_state`, snapshotted at the last
/// context switch away from it. `Exited` tasks are skipped: they no
/// longer meaningfully "hold" anything, matching `reap()` resetting them
/// to `Task::EMPTY` (whose fd table has no real vfs handles) anyway.
pub fn any_other_task_shares_fd(matches: impl Fn(&crate::syscall::ProcessFd) -> bool) -> bool {
    arch::disable_interrupts();
    let scheduler = unsafe { &*SCHEDULER.0.get() };
    let current = scheduler.current;
    scheduler.tasks.iter().enumerate().any(|(index, task)| {
        index != current && task.state != TaskState::Exited && {
            let fds = crate::syscall::process_state_fds(&task.process_state);
            fds.iter().any(&matches)
        }
    })
}

/// Real Linux-ABI anonymous mmap() for the current task, mirroring
/// `linux_brk_for_current_task`'s fallback contract: `None` means "this task
/// has no `process_space`, use the original singleton path instead."
pub fn linux_mmap_for_current_task(pages: usize, protection: u8) -> Option<Option<u64>> {
    arch::disable_interrupts();
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    let current = scheduler.current;
    let space = scheduler.tasks[current].process_space.as_mut()?;
    Some(arch::paging::process_mmap(space, pages, protection))
}

/// The file-backed half of the same contract: writes into an mmap region
/// this task already has mapped via `linux_mmap_for_current_task`.
pub fn linux_mmap_write_for_current_task(
    address: u64,
    offset: usize,
    source: &[u8],
) -> Option<bool> {
    arch::disable_interrupts();
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    let current = scheduler.current;
    let space = scheduler.tasks[current].process_space.as_ref()?;
    Some(arch::paging::process_mmap_write(
        space, address, offset, source,
    ))
}

pub fn linux_mprotect_for_current_task(address: u64, pages: usize, protection: u8) -> Option<bool> {
    arch::disable_interrupts();
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    let current = scheduler.current;
    let space = scheduler.tasks[current].process_space.as_mut()?;
    Some(arch::paging::process_mprotect(
        space, address, pages, protection,
    ))
}

pub fn linux_munmap_for_current_task(address: u64, pages: usize) -> Option<bool> {
    arch::disable_interrupts();
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    let current = scheduler.current;
    let space = scheduler.tasks[current].process_space.as_mut()?;
    Some(arch::paging::process_munmap(space, address, pages))
}

pub fn grow_current_heap(additional_pages: u64) -> Option<u64> {
    arch::disable_interrupts();
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    let current = scheduler.current;
    let space = scheduler.tasks[current].process_space.as_mut()?;
    arch::paging::process_brk(space, additional_pages as usize)
}

pub fn exec_current_user_task(program: &[u8]) -> Option<(u64, u64)> {
    arch::disable_interrupts();
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    let current = scheduler.current;
    let space = scheduler.tasks[current].process_space.as_mut()?;
    if !arch::paging::exec_process(space, program) {
        return None;
    }
    crate::syscall::exec_reset_process_state();
    Some((space.entry, space.stack_top))
}

/// Raw probe: Linux mmap of 100 pages (past the old 32-page limit), touches
/// the last page, exits 55 on success / 99 on failure.
pub const BIG_MMAP_PROBE: [u8; 69] = [
    0xb8, 0x09, 0x00, 0x00, 0x00, 0x31, 0xff, 0xbe, 0x00, 0x40, 0x06, 0x00, 0xba, 0x03, 0x00, 0x00,
    0x00, 0x41, 0xba, 0x22, 0x00, 0x00, 0x00, 0x49, 0xc7, 0xc0, 0xff, 0xff, 0xff, 0xff, 0x45, 0x31,
    0xc9, 0x0f, 0x05, 0x48, 0x3d, 0x00, 0xf0, 0xff, 0xff, 0x73, 0x11, 0xc7, 0x80, 0x00, 0x30, 0x06,
    0x00, 0x01, 0x00, 0x00, 0x00, 0xbf, 0x37, 0x00, 0x00, 0x00, 0xeb, 0x05, 0xbf, 0x63, 0x00, 0x00,
    0x00, 0x31, 0xc0, 0xcd, 0x80,
];

/// Linux-ABI probe: fork(); child exit(22); parent wait4()s and exits 11
/// only if the status word is 22<<8.
pub const LINUX_FORK_WAIT_PROBE: [u8; 72] = [
    0xb8, 0x39, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x85, 0xc0, 0x75, 0x0c, 0xb8, 0x3c, 0x00, 0x00, 0x00,
    0xbf, 0x16, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x89, 0xc7, 0x48, 0x8d, 0x74, 0x24, 0xf0, 0x31, 0xd2,
    0x45, 0x31, 0xd2, 0xb8, 0x3d, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x8b, 0x44, 0x24, 0xf0, 0x3d, 0x00,
    0x16, 0x00, 0x00, 0x75, 0x07, 0xbf, 0x0b, 0x00, 0x00, 0x00, 0xeb, 0x05, 0xbf, 0x7d, 0x00, 0x00,
    0x00, 0xb8, 0x3c, 0x00, 0x00, 0x00, 0x0f, 0x05,
];

/// Linux-ABI probe: fork(); child spins; parent kill(child, SIGKILL) then
/// wait4()s and exits 11 only if kill returned 0 and the status is 9 (killed by SIGKILL).
pub const LINUX_KILL_PROBE: [u8; 82] = [
    0xb8, 0x39, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x85, 0xc0, 0x75, 0x02, 0xeb, 0xfe, 0x89, 0xc3, 0x89,
    0xdf, 0xbe, 0x09, 0x00, 0x00, 0x00, 0xb8, 0x3e, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x85, 0xc0, 0x75,
    0x25, 0x89, 0xdf, 0x48, 0x8d, 0x74, 0x24, 0xf0, 0x31, 0xd2, 0x45, 0x31, 0xd2, 0xb8, 0x3d, 0x00,
    0x00, 0x00, 0x0f, 0x05, 0x8b, 0x44, 0x24, 0xf0, 0x3d, 0x09, 0x00, 0x00, 0x00, 0x75, 0x07, 0xbf,
    0x0b, 0x00, 0x00, 0x00, 0xeb, 0x05, 0xbf, 0x7d, 0x00, 0x00, 0x00, 0xb8, 0x3c, 0x00, 0x00, 0x00,
    0x0f, 0x05,
];

/// Linux-ABI probe: execve("/bin/aeros-init"); exit 99 if that fails.
pub const LINUX_EXECVE_PROBE: [u8; 46] = [
    0xb8, 0x3b, 0x00, 0x00, 0x00, 0x48, 0x8d, 0x3d, 0x12, 0x00, 0x00, 0x00, 0x31, 0xf6, 0x31, 0xd2,
    0x0f, 0x05, 0xbf, 0x63, 0x00, 0x00, 0x00, 0xb8, 0x3c, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x2f, 0x62,
    0x69, 0x6e, 0x2f, 0x61, 0x65, 0x72, 0x6f, 0x73, 0x2d, 0x69, 0x6e, 0x69, 0x74, 0x00,
];

/// Raw probe: SYS_EXEC_PATH("/bin/aeros-init"); exits 99 if exec fails.
pub const EXEC_PATH_PROBE: [u8; 41] = [
    0xb8, 0x0b, 0x00, 0x00, 0x00, 0xbf, 0x0f, 0x00, 0x00, 0x00, 0x48, 0xbe, 0x2f, 0x62, 0x69, 0x6e,
    0x2f, 0x61, 0x65, 0x72, 0x48, 0xba, 0x6f, 0x73, 0x2d, 0x69, 0x6e, 0x69, 0x74, 0x00, 0xcd, 0x80,
    0xbf, 0x63, 0x00, 0x00, 0x00, 0x31, 0xc0, 0xcd, 0x80,
];

/// execve by path: like `exec_current_user_task`, but a real ELF also gets
/// the argv/envp/auxv stack a compiled program expects.
pub fn exec_current_user_task_path(program: &[u8], path: &str) -> Option<(u64, u64)> {
    exec_current_user_task_path_with(program, path, &[path.as_bytes()], &[])
}

/// `execve` with an explicit argv/envp (already copied out of the old image).
pub fn exec_current_user_task_path_with(
    program: &[u8],
    path: &str,
    args: &[&[u8]],
    env: &[&[u8]],
) -> Option<(u64, u64)> {
    arch::disable_interrupts();
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    let current = scheduler.current;
    let space = scheduler.tasks[current].process_space.as_mut()?;
    if !arch::paging::exec_process(space, program) {
        return None;
    }
    if let Ok(image) = crate::elf::ElfImage::parse(program)
        && !crate::process::prepare_scheduled_process_stack_with(space, &image, path, args, env)
    {
        return None;
    }
    crate::syscall::exec_reset_process_state();
    Some((space.entry, space.stack_top))
}

extern "C" fn forked_task_trampoline() -> ! {
    let (snapshot, user_start, user_end) = unsafe {
        let scheduler = &*SCHEDULER.0.get();
        let task = &scheduler.tasks[scheduler.current];
        (task.fork_snapshot, task.user_start, task.user_end)
    };
    let Some(snapshot) = snapshot else {
        arch::halt_forever();
    };
    let exit_code = arch::user::resume_forked_child(&snapshot, user_start, user_end);
    let current = unsafe { (*SCHEDULER.0.get()).current };
    USER_TASK_EXIT[current].store(exit_code, Ordering::Release);
    crate::syscall::close_all_process_fds();
    exit_current()
}

pub fn yield_now() -> bool {
    arch::disable_interrupts();
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    if !scheduler.initialized {
        return false;
    }
    let Some(next) = scheduler.next_ready() else {
        return false;
    };
    let current = scheduler.current;
    if scheduler.tasks[current].state == TaskState::Running {
        scheduler.tasks[current].state = TaskState::Ready;
    }
    scheduler.tasks[next].state = TaskState::Running;
    scheduler.tasks[next].switches = scheduler.tasks[next].switches.saturating_add(1);
    scheduler.current = next;
    scheduler.context_switches = scheduler.context_switches.saturating_add(1);
    let old_rsp = &mut scheduler.tasks[current].context.rsp as *mut u64;
    let new_rsp = scheduler.tasks[next].context.rsp;
    let old_fpu = scheduler.tasks[current].fpu_base as *mut u8;
    let new_fpu = scheduler.tasks[next].fpu_base as *const u8;
    scheduler.fpu_switches = scheduler.fpu_switches.saturating_add(1);
    scheduler.tasks[current].user_context = arch::user::save_task_context();
    arch::user::restore_task_context(&scheduler.tasks[next].user_context);
    scheduler.tasks[current].process_state = crate::syscall::save_process_state();
    crate::syscall::restore_process_state(&scheduler.tasks[next].process_state);
    arch::gdt::set_ring0_stack(RSP0_CPU, scheduler.tasks[next].interrupt_stack_top);
    arch::syscall_entry::set_syscall_stack(
        RSP0_CPU,
        scheduler.tasks[next]
            .process_space
            .is_some()
            .then_some(scheduler.tasks[next].interrupt_stack_top),
    );
    switch_cr3(&scheduler.tasks[next]);
    unsafe {
        if !arch::fpu::save_context(old_fpu) || !arch::fpu::restore_context(new_fpu) {
            arch::halt_forever();
        }
        switch_context(old_rsp, new_rsp);
    }
    true
}

pub fn exit_current() -> ! {
    arch::disable_interrupts();
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    let current = scheduler.current;
    scheduler.tasks[current].state = TaskState::Exited;
    let Some(next) = scheduler.next_ready() else {
        arch::halt_forever();
    };
    scheduler.tasks[next].state = TaskState::Running;
    scheduler.tasks[next].switches = scheduler.tasks[next].switches.saturating_add(1);
    scheduler.current = next;
    scheduler.context_switches = scheduler.context_switches.saturating_add(1);
    let abandoned_rsp = &mut scheduler.tasks[current].context.rsp as *mut u64;
    let new_rsp = scheduler.tasks[next].context.rsp;
    let abandoned_fpu = scheduler.tasks[current].fpu_base as *mut u8;
    let new_fpu = scheduler.tasks[next].fpu_base as *const u8;
    scheduler.fpu_switches = scheduler.fpu_switches.saturating_add(1);
    arch::user::restore_task_context(&scheduler.tasks[next].user_context);
    crate::syscall::restore_process_state(&scheduler.tasks[next].process_state);
    arch::gdt::set_ring0_stack(RSP0_CPU, scheduler.tasks[next].interrupt_stack_top);
    arch::syscall_entry::set_syscall_stack(
        RSP0_CPU,
        scheduler.tasks[next]
            .process_space
            .is_some()
            .then_some(scheduler.tasks[next].interrupt_stack_top),
    );
    switch_cr3(&scheduler.tasks[next]);
    unsafe {
        if !arch::fpu::save_context(abandoned_fpu) || !arch::fpu::restore_context(new_fpu) {
            arch::halt_forever();
        }
        switch_context(abandoned_rsp, new_rsp);
    }
    arch::halt_forever()
}

pub fn reap() -> usize {
    arch::disable_interrupts();
    let scheduler = unsafe { &mut *SCHEDULER.0.get() };
    let mut reaped = 0;
    for task in &mut scheduler.tasks {
        if task.state != TaskState::Exited || task.stack_base == 0 {
            continue;
        }
        let stack_released = NonNull::new(task.stack_base as *mut u8)
            .is_some_and(|pointer| heap::HEAP.deallocate(pointer));
        let fpu_released = NonNull::new(task.fpu_base as *mut u8)
            .is_some_and(|pointer| heap::HEAP.deallocate(pointer));
        let interrupt_stack_released = task.interrupt_stack_base == 0
            || NonNull::new(task.interrupt_stack_base as *mut u8)
                .is_some_and(|pointer| heap::HEAP.deallocate(pointer));
        let process_released = task
            .process_space
            .as_ref()
            .is_none_or(arch::paging::destroy_process);
        if !stack_released || !fpu_released || !interrupt_stack_released || !process_released {
            arch::halt_forever();
        }
        *task = Task::EMPTY;
        reaped += 1;
    }
    reaped
}

pub fn stats() -> SchedulerStats {
    arch::disable_interrupts();
    let scheduler = unsafe { &*SCHEDULER.0.get() };
    let mut stats = SchedulerStats {
        tasks: 0,
        ready: 0,
        running: 0,
        exited: 0,
        context_switches: scheduler.context_switches,
        stack_bytes: 0,
        fpu_tasks: 0,
        fpu_bytes: 0,
        fpu_switches: scheduler.fpu_switches,
        fpu_isolation: FPU_A_VALID.load(Ordering::Acquire) == 1
            && FPU_B_VALID.load(Ordering::Acquire) == 1,
        highest_task_id: 0,
    };
    for task in &scheduler.tasks {
        match task.state {
            TaskState::Empty => {}
            TaskState::Ready => {
                stats.tasks += 1;
                stats.ready += 1;
            }
            TaskState::Running => {
                stats.tasks += 1;
                stats.running += 1;
            }
            TaskState::Exited => {
                stats.tasks += 1;
                stats.exited += 1;
            }
        }
        if task.state != TaskState::Empty {
            stats.stack_bytes = stats.stack_bytes.saturating_add(task.stack_size);
            if task.fpu_base != 0 {
                stats.fpu_tasks += 1;
                stats.fpu_bytes = stats.fpu_bytes.saturating_add(task.fpu_size);
            }
            stats.highest_task_id = stats.highest_task_id.max(task.id);
        }
    }
    stats
}

pub fn self_test() -> bool {
    initialize();
    TASK_A_PHASE.store(0, Ordering::Release);
    TASK_B_PHASE.store(0, Ordering::Release);
    FPU_A_VALID.store(0, Ordering::Release);
    FPU_B_VALID.store(0, Ordering::Release);
    if spawn(self_test_task_a).is_none() || spawn(self_test_task_b).is_none() {
        return false;
    }
    for _ in 0..8 {
        if stats().tasks <= 1 {
            break;
        }
        yield_now();
    }
    let completed =
        TASK_A_PHASE.load(Ordering::Acquire) == 2 && TASK_B_PHASE.load(Ordering::Acquire) == 2;
    let fpu_isolated =
        FPU_A_VALID.load(Ordering::Acquire) == 1 && FPU_B_VALID.load(Ordering::Acquire) == 1;
    let reaped = reap();
    let final_stats = stats();
    completed
        && fpu_isolated
        && reaped == 2
        && final_stats.tasks == 1
        && final_stats.running == 1
        && final_stats.context_switches >= 6
}

pub fn preemption_self_test() -> bool {
    initialize();
    PREEMPT_A_PHASE.store(0, Ordering::Release);
    PREEMPT_B_PHASE.store(0, Ordering::Release);
    PREEMPT_A_WORK.store(0, Ordering::Release);
    PREEMPT_B_WORK.store(0, Ordering::Release);
    PREEMPT_FPU_A_VALID.store(0, Ordering::Release);
    PREEMPT_FPU_B_VALID.store(0, Ordering::Release);
    PREEMPT_TICKS.store(0, Ordering::Release);
    if spawn(preempt_task_a).is_none() || spawn(preempt_task_b).is_none() {
        return false;
    }
    PREEMPTION_ENABLED.store(1, Ordering::Release);
    if !arch::apic::start_timer(200) {
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return false;
    }
    while PREEMPT_A_PHASE.load(Ordering::Acquire) != 2
        || PREEMPT_B_PHASE.load(Ordering::Acquire) != 2
    {
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    }
    arch::apic::stop_timer();
    PREEMPTION_ENABLED.store(0, Ordering::Release);
    let ticks = PREEMPT_TICKS.load(Ordering::Acquire);
    let work_a = PREEMPT_A_WORK.load(Ordering::Acquire);
    let work_b = PREEMPT_B_WORK.load(Ordering::Acquire);
    let fpu_isolated = preemption_fpu_isolation();
    let before_reap = stats();
    let reaped = reap();
    let after_reap = stats();
    ticks >= 8
        && work_a != 0
        && work_b != 0
        && fpu_isolated
        && before_reap.context_switches >= 8
        && reaped == 2
        && after_reap.tasks == 1
}

pub fn on_timer_tick() {
    if PREEMPTION_ENABLED.load(Ordering::Acquire) == 0 {
        return;
    }
    PREEMPT_TICKS.fetch_add(1, Ordering::AcqRel);
    yield_now();
}

pub fn preemption_ticks() -> u32 {
    PREEMPT_TICKS.load(Ordering::Acquire)
}

pub fn preemption_work() -> (u32, u32) {
    (
        PREEMPT_A_WORK.load(Ordering::Acquire),
        PREEMPT_B_WORK.load(Ordering::Acquire),
    )
}

pub fn preemption_fpu_isolation() -> bool {
    PREEMPT_FPU_A_VALID.load(Ordering::Acquire) == 1
        && PREEMPT_FPU_B_VALID.load(Ordering::Acquire) == 1
}

fn build_spin_probe(iterations: u32, exit_code: u32) -> [u8; 25] {
    let mut code = [0u8; 25];
    code[0] = 0x31;
    code[1] = 0xc9;
    code[2] = 0xba;
    code[3..7].copy_from_slice(&iterations.to_le_bytes());
    code[7] = 0xff;
    code[8] = 0xc1;
    code[9] = 0x39;
    code[10] = 0xd1;
    code[11] = 0x7c;
    code[12] = 0xfa;
    code[13] = 0xb8;
    code[18] = 0xbf;
    code[19..23].copy_from_slice(&exit_code.to_le_bytes());
    code[23] = 0xcd;
    code[24] = 0x80;
    code
}

extern "C" fn user_task_trampoline() -> ! {
    let task = unsafe {
        let scheduler = &*SCHEDULER.0.get();
        scheduler.tasks[scheduler.current]
    };
    let exit_code = arch::user::run_scheduled_task(
        task.user_entry,
        task.user_stack,
        task.user_start,
        task.user_end,
    );
    let current = unsafe { (*SCHEDULER.0.get()).current };
    USER_TASK_EXIT[current].store(exit_code, Ordering::Release);
    crate::syscall::close_all_process_fds();
    exit_current()
}

#[derive(Clone, Copy)]
pub struct ConcurrentUserResult {
    pub exit_a: u64,
    pub exit_b: u64,
    pub switches: u64,
    pub reaped: usize,
    pub verified: bool,
}

fn failed_concurrent_user_result() -> ConcurrentUserResult {
    ConcurrentUserResult {
        exit_a: 0,
        exit_b: 0,
        switches: 0,
        reaped: 0,
        verified: false,
    }
}

pub fn concurrent_user_self_test(
    state: &crate::arch::paging::PagingState,
    frames: &mut crate::memory::FrameAllocator,
) -> ConcurrentUserResult {
    initialize();
    for slot in USER_TASK_EXIT.iter() {
        slot.store(u64::MAX, Ordering::Release);
    }
    let program_a = build_spin_probe(150_000_000, 77);
    let program_b = build_spin_probe(100_000_000, 88);
    let Some(mapping_a) = crate::arch::paging::map_probe_code(state, frames, &program_a) else {
        return failed_concurrent_user_result();
    };
    let Some(mapping_b) = crate::arch::paging::map_probe_code(state, frames, &program_b) else {
        return failed_concurrent_user_result();
    };
    if !mapping_a.verified || !mapping_b.verified {
        return failed_concurrent_user_result();
    }
    let user_end_a = mapping_a.entry + 3 * 4096;
    let user_end_b = mapping_b.entry + 3 * 4096;
    let Some((_, slot_a)) = spawn_user(
        mapping_a.entry,
        mapping_a.stack_top,
        mapping_a.entry,
        user_end_a,
    ) else {
        return failed_concurrent_user_result();
    };
    let Some((_, slot_b)) = spawn_user(
        mapping_b.entry,
        mapping_b.stack_top,
        mapping_b.entry,
        user_end_b,
    ) else {
        return failed_concurrent_user_result();
    };
    PREEMPTION_ENABLED.store(1, Ordering::Release);
    if !arch::apic::start_timer(1000) {
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_concurrent_user_result();
    }
    while USER_TASK_EXIT[slot_a].load(Ordering::Acquire) == u64::MAX
        || USER_TASK_EXIT[slot_b].load(Ordering::Acquire) == u64::MAX
    {
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    }
    arch::apic::stop_timer();
    PREEMPTION_ENABLED.store(0, Ordering::Release);
    let switches = stats().context_switches;
    let exit_a = USER_TASK_EXIT[slot_a].load(Ordering::Acquire);
    let exit_b = USER_TASK_EXIT[slot_b].load(Ordering::Acquire);
    let reaped = reap();
    let after_reap = stats();
    let probe_reap_a = crate::arch::paging::destroy_user_probe(state, frames, &mapping_a);
    let probe_reap_b = crate::arch::paging::destroy_user_probe(state, frames, &mapping_b);
    ConcurrentUserResult {
        exit_a,
        exit_b,
        switches,
        reaped,
        verified: exit_a == 77
            && exit_b == 88
            && switches >= 4
            && reaped == 2
            && after_reap.tasks == 1
            && probe_reap_a.verified
            && probe_reap_b.verified,
    }
}

fn slot_for_id(id: u64) -> Option<usize> {
    arch::disable_interrupts();
    let scheduler = unsafe { &*SCHEDULER.0.get() };
    scheduler.tasks.iter().position(|task| task.id == id)
}

fn fork_probe() -> [u8; 52] {
    [
        0xb8, 0x02, 0x00, 0x00, 0x00, // mov eax, 2 (SYS_FORK)
        0xcd, 0x80, // int 0x80
        0x83, 0xf8, 0x00, // cmp eax, 0
        0x74, 0x14, // je child_path (+20)
        0xc7, 0x44, 0x24, 0xf8, 0xaa, 0xaa, 0xaa, 0xaa, // mov dword ptr [rsp-8], 0xaaaaaaaa
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT)
        0xbf, 0x0b, 0x00, 0x00, 0x00, // mov edi, 11
        0xcd, 0x80, // int 0x80
        // child_path:
        0xc7, 0x44, 0x24, 0xf8, 0xbb, 0xbb, 0xbb, 0xbb, // mov dword ptr [rsp-8], 0xbbbbbbbb
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT)
        0xbf, 0x16, 0x00, 0x00, 0x00, // mov edi, 22
        0xcd, 0x80, // int 0x80
    ]
}

#[derive(Clone, Copy)]
pub struct ForkSelfTestResult {
    pub parent_exit: u64,
    pub child_exit: u64,
    pub parent_stack_value: u32,
    pub child_stack_value: u32,
    pub reaped: usize,
    pub verified: bool,
}

fn failed_fork_result() -> ForkSelfTestResult {
    ForkSelfTestResult {
        parent_exit: 0,
        child_exit: 0,
        parent_stack_value: 0,
        child_stack_value: 0,
        reaped: 0,
        verified: false,
    }
}

pub fn fork_self_test(state: &crate::arch::paging::PagingState) -> ForkSelfTestResult {
    initialize();
    for slot in USER_TASK_EXIT.iter() {
        slot.store(u64::MAX, Ordering::Release);
    }
    let code = fork_probe();
    let Some((parent_id, parent_slot, parent_space)) = spawn_process(state, &code) else {
        return failed_fork_result();
    };
    if !parent_space.verified || !parent_space.owns_code {
        return failed_fork_result();
    }
    let child_id = parent_id + 1;
    PREEMPTION_ENABLED.store(1, Ordering::Release);
    if !arch::apic::start_timer(1000) {
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_fork_result();
    }
    let parent_exit = loop {
        let value = USER_TASK_EXIT[parent_slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    let Some(child_slot) = slot_for_id(child_id) else {
        arch::apic::stop_timer();
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_fork_result();
    };
    let child_exit = loop {
        let value = USER_TASK_EXIT[child_slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    arch::apic::stop_timer();
    PREEMPTION_ENABLED.store(0, Ordering::Release);
    let child_space = unsafe { (*SCHEDULER.0.get()).tasks[child_slot].process_space };
    let Some(child_space) = child_space else {
        return failed_fork_result();
    };
    const STACK_PROBE_OFFSET: u64 = crate::arch::paging::PROCESS_STACK_BYTES - 24;
    let parent_value = unsafe {
        core::ptr::read_volatile(
            (parent_space.stack_physical + STACK_PROBE_OFFSET) as usize as *const u32,
        )
    };
    let child_value = unsafe {
        core::ptr::read_volatile(
            (child_space.stack_physical + STACK_PROBE_OFFSET) as usize as *const u32,
        )
    };
    let shared_refcount_before_reap =
        arch::paging::code_page_refcount(parent_space.code_physical[0]);
    let reaped = reap();
    let shared_refcount_after_reap =
        arch::paging::code_page_refcount(parent_space.code_physical[0]);
    let after = stats();
    ForkSelfTestResult {
        parent_exit,
        child_exit,
        parent_stack_value: parent_value,
        child_stack_value: child_value,
        reaped,
        verified: parent_exit == 11
            && child_exit == 22
            && parent_value == 0xaaaa_aaaa
            && child_value == 0xbbbb_bbbb
            && parent_space.owns_code
            && !child_space.owns_code
            && parent_space.code_physical[0] == child_space.code_physical[0]
            && parent_space.root_physical != child_space.root_physical
            // proves the shared code page is genuinely refcounted (2 while
            // both address spaces are still alive, 0 -- actually freed --
            // once both have been reaped), not just aliased with no bookkeeping.
            && shared_refcount_before_reap == 2
            && shared_refcount_after_reap == 0
            && reaped == 2
            && after.tasks == 1,
    }
}

fn build_fork_exec_probe() -> [u8; 116] {
    [
        0xb8, 0x02, 0x00, 0x00, 0x00, // mov eax, 2 (SYS_FORK)
        0xcd, 0x80, // int 0x80
        0x83, 0xf8, 0x00, // cmp eax, 0
        0x74, 0x14, // je child_path (+20)
        0xc7, 0x44, 0x24, 0xf8, 0xaa, 0xaa, 0xaa, 0xaa, // mov dword ptr [rsp-8], 0xaaaaaaaa
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT)
        0xbf, 0x0b, 0x00, 0x00, 0x00, // mov edi, 11
        0xcd, 0x80, // int 0x80
        // child_path: grow this child's own heap by 1 page BEFORE execve()'ing,
        // to later prove execve() resets heap state instead of carrying it over
        0xb8, 0x08, 0x00, 0x00, 0x00, // mov eax, 8 (SYS_SBRK)
        0xbf, 0x01, 0x00, 0x00, 0x00, // mov edi, 1
        0xcd, 0x80, // int 0x80 (result ignored)
        // pack the 32-byte exec target program into rsi/rdx/r10/r8, then execve() it
        0x48, 0xbe, 0xb8, 0x08, 0x00, 0x00, 0x00, 0xbf, 0x01, 0x00, // movabs rsi, imm64
        0x48, 0xba, 0x00, 0x00, 0xcd, 0x80, 0xc7, 0x44, 0x24, 0xf8, // movabs rdx, imm64
        0x49, 0xba, 0xcc, 0xcc, 0xcc, 0xcc, 0xb8, 0x00, 0x00, 0x00, // movabs r10, imm64
        0x49, 0xb8, 0x00, 0xbf, 0x21, 0x00, 0x00, 0x00, 0xcd, 0x80, // movabs r8, imm64
        0xb8, 0x03, 0x00, 0x00, 0x00, // mov eax, 3 (SYS_EXECVE)
        0xbf, 0x20, 0x00, 0x00, 0x00, // mov edi, 32 (program length)
        0xcd, 0x80, // int 0x80
        // only reached if execve() failed and control fell back to the old code
        0xc7, 0x44, 0x24, 0xf8, 0xbb, 0xbb, 0xbb, 0xbb, // mov dword ptr [rsp-8], 0xbbbbbbbb
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT)
        0xbf, 0x16, 0x00, 0x00, 0x00, // mov edi, 22
        0xcd, 0x80, // int 0x80
    ]
}

// The exec target program packed into rsi/rdx/r10/r8 above, spelled out for
// reference (32 bytes): grow the (freshly exec'd, should-be-empty) heap by
// one more page, write a marker to the stack, exit(33).
//   mov eax, 8; mov edi, 1; int 0x80
//   mov dword ptr [rsp-8], 0xcccccccc
//   mov eax, 0; mov edi, 33; int 0x80

#[derive(Clone, Copy)]
pub struct ExecSelfTestResult {
    pub parent_exit: u64,
    pub child_exit: u64,
    pub parent_stack_value: u32,
    pub child_stack_value: u32,
    pub reaped: usize,
    pub verified: bool,
}

fn failed_exec_result() -> ExecSelfTestResult {
    ExecSelfTestResult {
        parent_exit: 0,
        child_exit: 0,
        parent_stack_value: 0,
        child_stack_value: 0,
        reaped: 0,
        verified: false,
    }
}

pub fn exec_self_test(state: &crate::arch::paging::PagingState) -> ExecSelfTestResult {
    initialize();
    for slot in USER_TASK_EXIT.iter() {
        slot.store(u64::MAX, Ordering::Release);
    }
    let code = build_fork_exec_probe();
    let Some((parent_id, parent_slot, parent_space)) = spawn_process(state, &code) else {
        return failed_exec_result();
    };
    if !parent_space.verified {
        return failed_exec_result();
    }
    let child_id = parent_id + 1;
    PREEMPTION_ENABLED.store(1, Ordering::Release);
    if !arch::apic::start_timer(1000) {
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_exec_result();
    }
    let parent_exit = loop {
        let value = USER_TASK_EXIT[parent_slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    let Some(child_slot) = slot_for_id(child_id) else {
        arch::apic::stop_timer();
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_exec_result();
    };
    let child_exit = loop {
        let value = USER_TASK_EXIT[child_slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    arch::apic::stop_timer();
    PREEMPTION_ENABLED.store(0, Ordering::Release);
    let child_space = unsafe { (*SCHEDULER.0.get()).tasks[child_slot].process_space };
    let Some(child_space) = child_space else {
        return failed_exec_result();
    };
    const STACK_PROBE_OFFSET: u64 = crate::arch::paging::PROCESS_STACK_BYTES - 24;
    let parent_value = unsafe {
        core::ptr::read_volatile(
            (parent_space.stack_physical + STACK_PROBE_OFFSET) as usize as *const u32,
        )
    };
    let child_value = unsafe {
        core::ptr::read_volatile(
            (child_space.stack_physical + STACK_PROBE_OFFSET) as usize as *const u32,
        )
    };
    // The child's own execve() already released its reference to the
    // originally-shared page before either task was reaped, so the shared
    // page should show only the parent's reference here, and the child's
    // freshly exec'd page should show exactly its own.
    let shared_refcount_before_reap =
        arch::paging::code_page_refcount(parent_space.code_physical[0]);
    let exec_refcount_before_reap = arch::paging::code_page_refcount(child_space.code_physical[0]);
    let reaped = reap();
    let shared_refcount_after_reap =
        arch::paging::code_page_refcount(parent_space.code_physical[0]);
    let exec_refcount_after_reap = arch::paging::code_page_refcount(child_space.code_physical[0]);
    let after = stats();
    ExecSelfTestResult {
        parent_exit,
        child_exit,
        parent_stack_value: parent_value,
        child_stack_value: child_value,
        reaped,
        verified: parent_exit == 11
            && child_exit == 33
            && parent_value == 0xaaaa_aaaa
            && child_value == 0xcccc_cccc
            && child_space.code_physical[0] != parent_space.code_physical[0]
            // the child grew its heap by 1 page before execve(), and the
            // post-exec program grew it by 1 page again -- heap_mapped==1
            // (not 2) proves execve() actually reset the old heap instead
            // of letting the new program silently inherit it
            && child_space.heap_mapped == 1
            && shared_refcount_before_reap == 1
            && exec_refcount_before_reap == 1
            && shared_refcount_after_reap == 0
            && exec_refcount_after_reap == 0
            && reaped == 2
            && after.tasks == 1,
    }
}

fn build_fork_chain_probe() -> [u8; 60] {
    [
        0xb8, 0x02, 0x00, 0x00, 0x00, // mov eax, 2 (SYS_FORK) -- A forks B
        0xcd, 0x80, // int 0x80
        0x83, 0xf8, 0x00, // cmp eax, 0
        0x74, 0x0c, // je b_path (+12)
        // A (grandparent):
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT)
        0xbf, 0x01, 0x00, 0x00, 0x00, // mov edi, 1
        0xcd, 0x80, // int 0x80
        // b_path:
        0xb8, 0x02, 0x00, 0x00, 0x00, // mov eax, 2 (SYS_FORK) -- B forks C
        0xcd, 0x80, // int 0x80
        0x83, 0xf8, 0x00, // cmp eax, 0
        0x74, 0x0c, // je c_path (+12)
        // B (parent, child of A):
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT)
        0xbf, 0x02, 0x00, 0x00, 0x00, // mov edi, 2
        0xcd, 0x80, // int 0x80
        // c_path: C (grandchild of A):
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT)
        0xbf, 0x03, 0x00, 0x00, 0x00, // mov edi, 3
        0xcd, 0x80, // int 0x80
    ]
}

#[derive(Clone, Copy)]
pub struct ForkChainSelfTestResult {
    pub exit_a: u64,
    pub exit_b: u64,
    pub exit_c: u64,
    pub reaped: usize,
    pub verified: bool,
}

fn failed_fork_chain_result() -> ForkChainSelfTestResult {
    ForkChainSelfTestResult {
        exit_a: 0,
        exit_b: 0,
        exit_c: 0,
        reaped: 0,
        verified: false,
    }
}

pub fn fork_chain_self_test(state: &crate::arch::paging::PagingState) -> ForkChainSelfTestResult {
    initialize();
    for slot in USER_TASK_EXIT.iter() {
        slot.store(u64::MAX, Ordering::Release);
    }
    let code = build_fork_chain_probe();
    let Some((a_id, a_slot, a_space)) = spawn_process(state, &code) else {
        return failed_fork_chain_result();
    };
    if !a_space.verified {
        return failed_fork_chain_result();
    }
    // fork() ids come from one global monotonic counter regardless of which
    // task calls it, so this 3-task chain is fully deterministic: A is
    // whatever spawn_process handed out, B is A's fork (the first fork call
    // anywhere in this test), C is B's fork (the second).
    let b_id = a_id + 1;
    let c_id = a_id + 2;
    PREEMPTION_ENABLED.store(1, Ordering::Release);
    if !arch::apic::start_timer(1000) {
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_fork_chain_result();
    }
    // A, B and C are independent tasks: A can exit before B has even started
    // running, let alone forked C, so waiting for A's exit first and only
    // then looking up B/C's slots would race. Poll for A's exit and B's
    // existence together, then for B's exit and C's existence together.
    let (exit_a, b_slot) = loop {
        let a_value = USER_TASK_EXIT[a_slot].load(Ordering::Acquire);
        let a_done = (a_value != u64::MAX).then_some(a_value);
        let b_slot = slot_for_id(b_id);
        if let (Some(exit_a), Some(b_slot)) = (a_done, b_slot) {
            break (exit_a, b_slot);
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    let (exit_b, c_slot) = loop {
        let b_value = USER_TASK_EXIT[b_slot].load(Ordering::Acquire);
        let b_done = (b_value != u64::MAX).then_some(b_value);
        let c_slot = slot_for_id(c_id);
        if let (Some(exit_b), Some(c_slot)) = (b_done, c_slot) {
            break (exit_b, c_slot);
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    let exit_c = loop {
        let value = USER_TASK_EXIT[c_slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    arch::apic::stop_timer();
    PREEMPTION_ENABLED.store(0, Ordering::Release);
    let b_space = unsafe { (*SCHEDULER.0.get()).tasks[b_slot].process_space };
    let c_space = unsafe { (*SCHEDULER.0.get()).tasks[c_slot].process_space };
    let (Some(b_space), Some(c_space)) = (b_space, c_space) else {
        return failed_fork_chain_result();
    };
    // Every generation shares the exact same original code page (none of
    // them execve()), so the refcount should read 3 while all three are
    // still alive but unreaped -- proving fork()'s refcounting chains
    // correctly through a grandchild, not just a single parent/child pair.
    let shared_refcount_before_reap = arch::paging::code_page_refcount(a_space.code_physical[0]);
    let reaped = reap();
    let shared_refcount_after_reap = arch::paging::code_page_refcount(a_space.code_physical[0]);
    let after = stats();
    ForkChainSelfTestResult {
        exit_a,
        exit_b,
        exit_c,
        reaped,
        verified: exit_a == 1
            && exit_b == 2
            && exit_c == 3
            && a_space.code_physical[0] == b_space.code_physical[0]
            && b_space.code_physical[0] == c_space.code_physical[0]
            && a_space.root_physical != b_space.root_physical
            && b_space.root_physical != c_space.root_physical
            && a_space.root_physical != c_space.root_physical
            && shared_refcount_before_reap == 3
            && shared_refcount_after_reap == 0
            && reaped == 3
            && after.tasks == 1,
    }
}

fn build_fork_wait_probe() -> [u8; 54] {
    [
        0xb8, 0x02, 0x00, 0x00, 0x00, // mov eax, 2 (SYS_FORK)
        0xcd, 0x80, // int 0x80
        0x83, 0xf8, 0x00, // cmp eax, 0
        0x74, 0x16, // je child_path (+22)
        // parent path: eax = child task id from fork()
        0x89, 0xc7, // mov edi, eax  (wait4 arg0 = child pid)
        0xb8, 0x04, 0x00, 0x00, 0x00, // mov eax, 4 (SYS_WAIT4)
        0xcd, 0x80, // int 0x80  -> eax = child's real exit status
        0x89, 0xc7, // mov edi, eax
        0x89, 0x7c, 0x24, 0xf8, // mov dword ptr [rsp-8], edi
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT)
        0xcd, 0x80, // int 0x80 (parent exits with the child's own status)
        // child_path:
        0xc7, 0x44, 0x24, 0xf8, 0xdd, 0xdd, 0xdd, 0xdd, // mov dword ptr [rsp-8], 0xdddddddd
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT)
        0xbf, 0x2c, 0x00, 0x00, 0x00, // mov edi, 44
        0xcd, 0x80, // int 0x80
    ]
}

#[derive(Clone, Copy)]
pub struct WaitSelfTestResult {
    pub parent_exit: u64,
    pub reaped: usize,
    pub verified: bool,
}

fn failed_wait_result() -> WaitSelfTestResult {
    WaitSelfTestResult {
        parent_exit: 0,
        reaped: 0,
        verified: false,
    }
}

pub fn wait_self_test(state: &crate::arch::paging::PagingState) -> WaitSelfTestResult {
    initialize();
    for slot in USER_TASK_EXIT.iter() {
        slot.store(u64::MAX, Ordering::Release);
    }
    let code = build_fork_wait_probe();
    let Some((_parent_id, parent_slot, parent_space)) = spawn_process(state, &code) else {
        return failed_wait_result();
    };
    if !parent_space.verified {
        return failed_wait_result();
    }
    PREEMPTION_ENABLED.store(1, Ordering::Release);
    if !arch::apic::start_timer(1000) {
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_wait_result();
    }
    // The parent's own code blocks in a real SYS_WAIT4 syscall until the child
    // exits, then re-exits with the child's own status (0x2C == 44) copied
    // straight through -- a value that appears nowhere in the parent's own
    // bytes, only the child's, so this can only be 44 if wait4() genuinely
    // forwarded the real child exit status. The kernel also reaps the child
    // synchronously inside that same wait4() call, before the parent's own
    // exit is even visible here.
    let parent_exit = loop {
        let value = USER_TASK_EXIT[parent_slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    arch::apic::stop_timer();
    PREEMPTION_ENABLED.store(0, Ordering::Release);
    let reaped = reap();
    let after = stats();
    WaitSelfTestResult {
        parent_exit,
        reaped,
        verified: parent_exit == 44 && reaped == 1 && after.tasks == 1,
    }
}

fn build_fork_kill_probe() -> [u8; 65] {
    [
        0xb8, 0x02, 0x00, 0x00, 0x00, // mov eax, 2 (SYS_FORK)
        0xcd, 0x80, // int 0x80
        0x83, 0xf8, 0x00, // cmp eax, 0
        0x74, 0x21, // je child_path (+33)
        // parent path: eax = child task id from fork()
        0x89, 0xc3, // mov ebx, eax  (keep child pid safe across syscalls)
        0x89, 0xdf, // mov edi, ebx  (kill arg0 = child pid)
        0xb8, 0x05, 0x00, 0x00, 0x00, // mov eax, 5 (SYS_KILL)
        0xcd, 0x80, // int 0x80 (result ignored)
        0x89, 0xdf, // mov edi, ebx  (wait4 arg0 = child pid)
        0xb8, 0x04, 0x00, 0x00, 0x00, // mov eax, 4 (SYS_WAIT4)
        0xcd, 0x80, // int 0x80 -> eax = child's real status (the kill's
        // synthetic 137, if the child genuinely never got to run)
        0x89, 0xc7, // mov edi, eax
        0x89, 0x7c, 0x24, 0xf8, // mov dword ptr [rsp-8], edi
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT)
        0xcd, 0x80, // int 0x80
        // child_path: should never execute if kill() genuinely prevented
        // this task from ever being scheduled
        0xc7, 0x44, 0x24, 0xf8, 0xee, 0xee, 0xee, 0xee, // mov dword ptr [rsp-8], 0xeeeeeeee
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0
        0xbf, 0x63, 0x00, 0x00, 0x00, // mov edi, 99
        0xcd, 0x80, // int 0x80
    ]
}

#[derive(Clone, Copy)]
pub struct KillSelfTestResult {
    pub parent_exit: u64,
    pub reaped: usize,
    pub verified: bool,
}

fn failed_kill_result() -> KillSelfTestResult {
    KillSelfTestResult {
        parent_exit: 0,
        reaped: 0,
        verified: false,
    }
}

pub fn kill_self_test(state: &crate::arch::paging::PagingState) -> KillSelfTestResult {
    initialize();
    for slot in USER_TASK_EXIT.iter() {
        slot.store(u64::MAX, Ordering::Release);
    }
    let code = build_fork_kill_probe();
    let Some((_parent_id, parent_slot, parent_space)) = spawn_process(state, &code) else {
        return failed_kill_result();
    };
    if !parent_space.verified {
        return failed_kill_result();
    }
    PREEMPTION_ENABLED.store(1, Ordering::Release);
    if !arch::apic::start_timer(1000) {
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_kill_result();
    }
    // The parent forks, then immediately (before yielding even once) kills
    // the child by task id and wait4()s it. If SYS_KILL genuinely prevents
    // the child from ever being scheduled, the child's own bytes (which
    // would write 0xeeeeeeee and exit(99)) never execute, and wait4() hands
    // back the synthetic kill status (137) instead -- which the parent then
    // re-exits with, so parent_exit == 137 is only possible if the whole
    // chain worked.
    let parent_exit = loop {
        let value = USER_TASK_EXIT[parent_slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    arch::apic::stop_timer();
    PREEMPTION_ENABLED.store(0, Ordering::Release);
    let reaped = reap();
    let after = stats();
    KillSelfTestResult {
        parent_exit,
        reaped,
        verified: parent_exit == 137 && reaped == 1 && after.tasks == 1,
    }
}

fn build_getpid_probe() -> [u8; 51] {
    [
        0xb8, 0x06, 0x00, 0x00, 0x00, // mov eax, 6 (SYS_GETPID)
        0xcd, 0x80, // int 0x80
        0x89, 0x44, 0x24, 0xf8, // mov dword ptr [rsp-8], eax  (parent's own pid)
        0xb8, 0x02, 0x00, 0x00, 0x00, // mov eax, 2 (SYS_FORK)
        0xcd, 0x80, // int 0x80
        0x83, 0xf8, 0x00, // cmp eax, 0
        0x74, 0x0c, // je child_path (+12)
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT)
        0xbf, 0x37, 0x00, 0x00, 0x00, // mov edi, 55
        0xcd, 0x80, // int 0x80
        // child_path:
        0xb8, 0x07, 0x00, 0x00, 0x00, // mov eax, 7 (SYS_GETPPID)
        0xcd, 0x80, // int 0x80
        0x89, 0xc7, // mov edi, eax  (exit code = this child's own getppid())
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT)
        0xcd, 0x80, // int 0x80
    ]
}

#[derive(Clone, Copy)]
pub struct GetPidSelfTestResult {
    pub parent_pid_value: u32,
    pub child_exit: u64,
    pub reaped: usize,
    pub verified: bool,
}

fn failed_getpid_result() -> GetPidSelfTestResult {
    GetPidSelfTestResult {
        parent_pid_value: 0,
        child_exit: 0,
        reaped: 0,
        verified: false,
    }
}

pub fn getpid_self_test(state: &crate::arch::paging::PagingState) -> GetPidSelfTestResult {
    initialize();
    for slot in USER_TASK_EXIT.iter() {
        slot.store(u64::MAX, Ordering::Release);
    }
    let code = build_getpid_probe();
    let Some((parent_id, parent_slot, parent_space)) = spawn_process(state, &code) else {
        return failed_getpid_result();
    };
    if !parent_space.verified {
        return failed_getpid_result();
    }
    let child_id = parent_id + 1;
    PREEMPTION_ENABLED.store(1, Ordering::Release);
    if !arch::apic::start_timer(1000) {
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_getpid_result();
    }
    let parent_exit = loop {
        let value = USER_TASK_EXIT[parent_slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    let Some(child_slot) = slot_for_id(child_id) else {
        arch::apic::stop_timer();
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_getpid_result();
    };
    let child_exit = loop {
        let value = USER_TASK_EXIT[child_slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    arch::apic::stop_timer();
    PREEMPTION_ENABLED.store(0, Ordering::Release);
    // parent_exit (55) just proves the parent's own path ran to completion;
    // the real proof is parent_pid_value (what the parent's own SYS_GETPID
    // returned, read back from its stack) matching the id spawn_process
    // handed out, and child_exit (the child's own SYS_GETPPID result)
    // matching that same parent id -- two independent tasks, two syscalls,
    // cross-checked against a value the kernel alone assigned.
    const STACK_PROBE_OFFSET: u64 = crate::arch::paging::PROCESS_STACK_BYTES - 24;
    let parent_pid_value = unsafe {
        core::ptr::read_volatile(
            (parent_space.stack_physical + STACK_PROBE_OFFSET) as usize as *const u32,
        )
    };
    let reaped = reap();
    let after = stats();
    GetPidSelfTestResult {
        parent_pid_value,
        child_exit,
        reaped,
        verified: parent_exit == 55
            && parent_pid_value == parent_id as u32
            && child_exit == parent_id
            && reaped == 2
            && after.tasks == 1,
    }
}

fn build_heap_probe() -> [u8; 74] {
    [
        0xb8, 0x08, 0x00, 0x00, 0x00, // mov eax, 8 (SYS_SBRK)
        0xbf, 0x02, 0x00, 0x00, 0x00, // mov edi, 2 (grow heap by 2 pages)
        0xcd, 0x80, // int 0x80 -> rax = new brk (heap end), full 64 bits
        0xc7, 0x40, 0xf8, 0xaa, 0xaa, 0xaa, 0xaa, // mov dword ptr [rax-8], 0xaaaaaaaa
        0xb8, 0x02, 0x00, 0x00, 0x00, // mov eax, 2 (SYS_FORK)
        0xcd, 0x80, // int 0x80
        0x83, 0xf8, 0x00, // cmp eax, 0
        0x74, 0x0c, // je child_path (+12)
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT)
        0xbf, 0x0b, 0x00, 0x00, 0x00, // mov edi, 11
        0xcd, 0x80, // int 0x80
        // child_path: re-query its own (forked, independently-copied) brk
        0xb8, 0x08, 0x00, 0x00, 0x00, // mov eax, 8 (SYS_SBRK)
        0xbf, 0x00, 0x00, 0x00, 0x00, // mov edi, 0 (query current brk, no growth)
        0xcd, 0x80, // int 0x80 -> rax = child's own current brk
        0xc7, 0x40, 0xf8, 0xbb, 0xbb, 0xbb, 0xbb, // mov dword ptr [rax-8], 0xbbbbbbbb
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT)
        0xbf, 0x16, 0x00, 0x00, 0x00, // mov edi, 22
        0xcd, 0x80, // int 0x80
    ]
}

#[derive(Clone, Copy)]
pub struct HeapSelfTestResult {
    pub parent_exit: u64,
    pub child_exit: u64,
    pub parent_heap_value: u32,
    pub child_heap_value: u32,
    pub reaped: usize,
    pub verified: bool,
}

fn failed_heap_result() -> HeapSelfTestResult {
    HeapSelfTestResult {
        parent_exit: 0,
        child_exit: 0,
        parent_heap_value: 0,
        child_heap_value: 0,
        reaped: 0,
        verified: false,
    }
}

pub fn heap_self_test(state: &crate::arch::paging::PagingState) -> HeapSelfTestResult {
    initialize();
    for slot in USER_TASK_EXIT.iter() {
        slot.store(u64::MAX, Ordering::Release);
    }
    let code = build_heap_probe();
    let Some((parent_id, parent_slot, parent_space)) = spawn_process(state, &code) else {
        return failed_heap_result();
    };
    if !parent_space.verified {
        return failed_heap_result();
    }
    let child_id = parent_id + 1;
    PREEMPTION_ENABLED.store(1, Ordering::Release);
    if !arch::apic::start_timer(1000) {
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_heap_result();
    }
    let parent_exit = loop {
        let value = USER_TASK_EXIT[parent_slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    let Some(child_slot) = slot_for_id(child_id) else {
        arch::apic::stop_timer();
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_heap_result();
    };
    let child_exit = loop {
        let value = USER_TASK_EXIT[child_slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    arch::apic::stop_timer();
    PREEMPTION_ENABLED.store(0, Ordering::Release);
    let child_space = unsafe { (*SCHEDULER.0.get()).tasks[child_slot].process_space };
    let Some(child_space) = child_space else {
        return failed_heap_result();
    };
    // `parent_space` (from `spawn_process`, above) is a snapshot taken before
    // the parent ever ran -- its heap fields are still all zero. Re-read the
    // parent's *live* process_space now that it has actually grown its heap.
    let live_parent_space = unsafe { (*SCHEDULER.0.get()).tasks[parent_slot].process_space };
    let Some(live_parent_space) = live_parent_space else {
        return failed_heap_result();
    };
    // The parent grows its heap by 2 real pages via SYS_SBRK, writes a marker
    // into the second one, then forks. The child re-queries its own (forked,
    // independently-copied) brk and overwrites the SAME logical slot with a
    // different marker. Reading both processes' *physical* second heap page
    // directly proves the fork copied the page rather than sharing it: the
    // parent's copy must still read 0xaaaaaaaa after the child writes
    // 0xbbbbbbbb into what is, from either program's point of view, "the
    // same address".
    const HEAP_PAGE_OFFSET: u64 = 4096 - 8;
    let parent_heap_value = unsafe {
        core::ptr::read_volatile(
            (live_parent_space.heap_physical[1] + HEAP_PAGE_OFFSET) as usize as *const u32,
        )
    };
    let child_heap_value = unsafe {
        core::ptr::read_volatile(
            (child_space.heap_physical[1] + HEAP_PAGE_OFFSET) as usize as *const u32,
        )
    };
    let reaped = reap();
    let after = stats();
    HeapSelfTestResult {
        parent_exit,
        child_exit,
        parent_heap_value,
        child_heap_value,
        reaped,
        verified: parent_exit == 11
            && child_exit == 22
            && parent_heap_value == 0xaaaa_aaaa
            && child_heap_value == 0xbbbb_bbbb
            && live_parent_space.heap_physical[1] != child_space.heap_physical[1]
            && reaped == 2
            && after.tasks == 1,
    }
}

fn build_write_probe() -> [u8; 54] {
    // Packs the ASCII string "AEROS_SYS_WRITE_OK" (18 bytes, padded with
    // trailing zeros to fill out the last register) into rsi/rdx/r10 and
    // writes it straight to the console via SYS_WRITE, then exits(1). The
    // literal string is grepped for directly in the serial log by
    // tools/test.ps1 -- independent, external proof that user-mode code
    // produced real output through the syscall, not just an exit code.
    [
        0x48, 0xbe, 0x41, 0x45, 0x52, 0x4f, 0x53, 0x5f, 0x53, 0x59, // movabs rsi, imm64
        0x48, 0xba, 0x53, 0x5f, 0x57, 0x52, 0x49, 0x54, 0x45, 0x5f, // movabs rdx, imm64
        0x49, 0xba, 0x4f, 0x4b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // movabs r10, imm64
        0xb8, 0x09, 0x00, 0x00, 0x00, // mov eax, 9 (SYS_WRITE)
        0xbf, 0x12, 0x00, 0x00, 0x00, // mov edi, 18 (length)
        0xcd, 0x80, // int 0x80
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT)
        0xbf, 0x01, 0x00, 0x00, 0x00, // mov edi, 1
        0xcd, 0x80, // int 0x80
    ]
}

#[derive(Clone, Copy)]
pub struct WriteSelfTestResult {
    pub exit_code: u64,
    pub verified: bool,
}

pub fn write_syscall_self_test(state: &crate::arch::paging::PagingState) -> WriteSelfTestResult {
    initialize();
    for slot in USER_TASK_EXIT.iter() {
        slot.store(u64::MAX, Ordering::Release);
    }
    let code = build_write_probe();
    let Some((_id, slot, space)) = spawn_process(state, &code) else {
        return WriteSelfTestResult {
            exit_code: 0,
            verified: false,
        };
    };
    if !space.verified {
        return WriteSelfTestResult {
            exit_code: 0,
            verified: false,
        };
    }
    PREEMPTION_ENABLED.store(1, Ordering::Release);
    if !arch::apic::start_timer(1000) {
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return WriteSelfTestResult {
            exit_code: 0,
            verified: false,
        };
    }
    let exit_code = loop {
        let value = USER_TASK_EXIT[slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    arch::apic::stop_timer();
    PREEMPTION_ENABLED.store(0, Ordering::Release);
    let reaped = reap();
    let after = stats();
    WriteSelfTestResult {
        exit_code,
        verified: exit_code == 1 && reaped == 1 && after.tasks == 1,
    }
}

fn build_getrandom_probe() -> [u8; 36] {
    [
        0xb8, 0x0a, 0x00, 0x00, 0x00, // mov eax, 10 (SYS_GETRANDOM)
        0xcd, 0x80, // int 0x80 -> rax = random value #1
        0x48, 0x89, 0x44, 0x24, 0xf8, // mov qword ptr [rsp-8], rax
        0xb8, 0x0a, 0x00, 0x00, 0x00, // mov eax, 10 (SYS_GETRANDOM)
        0xcd, 0x80, // int 0x80 -> rax = random value #2
        0x48, 0x89, 0x44, 0x24, 0xf0, // mov qword ptr [rsp-16], rax
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT)
        0xbf, 0x01, 0x00, 0x00, 0x00, // mov edi, 1
        0xcd, 0x80, // int 0x80
    ]
}

#[derive(Clone, Copy)]
pub struct GetRandomSelfTestResult {
    pub exit_code: u64,
    pub value_a: u64,
    pub value_b: u64,
    pub verified: bool,
}

fn failed_getrandom_result() -> GetRandomSelfTestResult {
    GetRandomSelfTestResult {
        exit_code: 0,
        value_a: 0,
        value_b: 0,
        verified: false,
    }
}

pub fn getrandom_self_test(state: &crate::arch::paging::PagingState) -> GetRandomSelfTestResult {
    initialize();
    for slot in USER_TASK_EXIT.iter() {
        slot.store(u64::MAX, Ordering::Release);
    }
    let code = build_getrandom_probe();
    let Some((_id, slot, space)) = spawn_process(state, &code) else {
        return failed_getrandom_result();
    };
    if !space.verified {
        return failed_getrandom_result();
    }
    PREEMPTION_ENABLED.store(1, Ordering::Release);
    if !arch::apic::start_timer(1000) {
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_getrandom_result();
    }
    let exit_code = loop {
        let value = USER_TASK_EXIT[slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    arch::apic::stop_timer();
    PREEMPTION_ENABLED.store(0, Ordering::Release);
    // Two back-to-back SYS_GETRANDOM calls, both stored to the stack and read
    // back from known physical offsets (same [rsp-8]/[rsp-16] convention used
    // throughout this test suite). Neither being zero and the two genuinely
    // differing is a simple, real proof this reaches the actual entropy
    // subsystem rather than a stub returning a fixed value.
    const VALUE_A_OFFSET: u64 = crate::arch::paging::PROCESS_STACK_BYTES - 24;
    const VALUE_B_OFFSET: u64 = crate::arch::paging::PROCESS_STACK_BYTES - 32;
    let value_a = unsafe {
        core::ptr::read_volatile((space.stack_physical + VALUE_A_OFFSET) as usize as *const u64)
    };
    let value_b = unsafe {
        core::ptr::read_volatile((space.stack_physical + VALUE_B_OFFSET) as usize as *const u64)
    };
    let reaped = reap();
    let after = stats();
    GetRandomSelfTestResult {
        exit_code,
        value_a,
        value_b,
        verified: exit_code == 1
            && value_a != 0
            && value_b != 0
            && value_a != value_b
            && reaped == 1
            && after.tasks == 1,
    }
}

fn build_close_then_write_probe() -> [u8; 40] {
    // Real Linux ABI, via the raw `syscall` instruction (not the bootstrap
    // ABI's `int 0x80`) -- both entry paths are simultaneously available to
    // any ring3 code on this kernel, regardless of which scheduler task it
    // is. close(1) (LINUX_CLOSE=3), then write(1, NULL, 1) (LINUX_WRITE=1),
    // which should fail with -EBADF now that fd 1 is closed, since the
    // buffer is never even read once the fd lookup itself fails.
    [
        0xb8, 0x03, 0x00, 0x00, 0x00, // mov eax, 3 (LINUX_CLOSE)
        0xbf, 0x01, 0x00, 0x00, 0x00, // mov edi, 1
        0x0f, 0x05, // syscall
        0xb8, 0x01, 0x00, 0x00, 0x00, // mov eax, 1 (LINUX_WRITE)
        0xbf, 0x01, 0x00, 0x00, 0x00, // mov edi, 1
        0x31, 0xf6, // xor esi, esi
        0xba, 0x01, 0x00, 0x00, 0x00, // mov edx, 1
        0x0f, 0x05, // syscall
        0x89, 0xc7, // mov edi, eax (exit code = write()'s return value)
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT, bootstrap ABI)
        0xcd, 0x80, // int 0x80
    ]
}

fn build_write_only_probe() -> [u8; 33] {
    // A completely independent process that never touches fd 1 -- if the
    // Linux-ABI fd table were still a global singleton, running this AFTER
    // build_close_then_write_probe() above would see fd 1 already closed
    // and fail exactly the same way. With per-task fd tables, this process
    // has its own fresh fd 1 and write(1, &self, 1) should succeed (return
    // 1), regardless of what any earlier task did to ITS OWN fd 1.
    [
        0xb8, 0x01, 0x00, 0x00, 0x00, // mov eax, 1 (LINUX_WRITE)
        0xbf, 0x01, 0x00, 0x00, 0x00, // mov edi, 1
        0x48, 0x8d, 0x35, 0x00, 0x00, 0x00, 0x00, // lea rsi, [rip+0] (any readable byte)
        0xba, 0x01, 0x00, 0x00, 0x00, // mov edx, 1
        0x0f, 0x05, // syscall
        0x89, 0xc7, // mov edi, eax (exit code = write()'s return value)
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT)
        0xcd, 0x80, // int 0x80
    ]
}

#[derive(Clone, Copy)]
pub struct LinuxFdIsolationResult {
    pub exit_a: u64,
    pub exit_b: u64,
    pub verified: bool,
}

fn failed_linux_fd_isolation_result() -> LinuxFdIsolationResult {
    LinuxFdIsolationResult {
        exit_a: 0,
        exit_b: 0,
        verified: false,
    }
}

pub fn linux_fd_isolation_self_test(
    state: &crate::arch::paging::PagingState,
) -> LinuxFdIsolationResult {
    initialize();
    for slot in USER_TASK_EXIT.iter() {
        slot.store(u64::MAX, Ordering::Release);
    }
    let code_a = build_close_then_write_probe();
    let Some((_a_id, a_slot, a_space)) = spawn_process(state, &code_a) else {
        return failed_linux_fd_isolation_result();
    };
    if !a_space.verified {
        return failed_linux_fd_isolation_result();
    }
    PREEMPTION_ENABLED.store(1, Ordering::Release);
    if !arch::apic::start_timer(1000) {
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_linux_fd_isolation_result();
    }
    let exit_a = loop {
        let value = USER_TASK_EXIT[a_slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    arch::apic::stop_timer();
    PREEMPTION_ENABLED.store(0, Ordering::Release);
    if reap() != 1 {
        return failed_linux_fd_isolation_result();
    }
    // USER_TASK_EXIT is indexed by scheduler slot, not by task identity, and
    // reap() never clears it -- B is very likely to reuse A's just-freed
    // slot, so without this reset B's own wait loop below would read A's
    // stale exit code immediately, before B ever runs at all.
    for slot in USER_TASK_EXIT.iter() {
        slot.store(u64::MAX, Ordering::Release);
    }
    // Process A has fully exited and been reaped before B is even spawned,
    // so any ordering-dependent scheduling can't be blamed for the result --
    // this is purely about whether B's Linux-ABI fd table starts fresh.
    let code_b = build_write_only_probe();
    let Some((_b_id, b_slot, b_space)) = spawn_process(state, &code_b) else {
        return failed_linux_fd_isolation_result();
    };
    if !b_space.verified {
        return failed_linux_fd_isolation_result();
    }
    PREEMPTION_ENABLED.store(1, Ordering::Release);
    if !arch::apic::start_timer(1000) {
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_linux_fd_isolation_result();
    }
    let exit_b = loop {
        let value = USER_TASK_EXIT[b_slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    arch::apic::stop_timer();
    PREEMPTION_ENABLED.store(0, Ordering::Release);
    let reaped = reap();
    let after = stats();
    LinuxFdIsolationResult {
        exit_a,
        exit_b,
        // -EBADF (9) truncated through `mov edi, eax` (32-bit) then
        // zero-extended into the 64-bit exit code.
        verified: exit_a == 0xffff_fff7 && exit_b == 1 && reaped == 1 && after.tasks == 1,
    }
}

fn build_fork_fd_probe() -> [u8; 85] {
    // fork(), then: parent writes to fd 1 (should succeed, return 1); child
    // closes ITS OWN fd 1 and then tries to write to it (should fail with
    // -EBADF). If fork() shared the Linux-ABI fd table instead of copying
    // it, the child's close() would also close the parent's fd 1.
    [
        0xb8, 0x02, 0x00, 0x00, 0x00, // mov eax, 2 (SYS_FORK)
        0xcd, 0x80, // int 0x80
        0x83, 0xf8, 0x00, // cmp eax, 0
        0x74, 0x21, // je child_path (+33)
        // parent path:
        0xb8, 0x01, 0x00, 0x00, 0x00, // mov eax, 1 (LINUX_WRITE)
        0xbf, 0x01, 0x00, 0x00, 0x00, // mov edi, 1
        0x48, 0x8d, 0x35, 0x00, 0x00, 0x00, 0x00, // lea rsi, [rip+0]
        0xba, 0x01, 0x00, 0x00, 0x00, // mov edx, 1
        0x0f, 0x05, // syscall
        0x89, 0xc7, // mov edi, eax
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT)
        0xcd, 0x80, // int 0x80
        // child_path:
        0xb8, 0x03, 0x00, 0x00, 0x00, // mov eax, 3 (LINUX_CLOSE)
        0xbf, 0x01, 0x00, 0x00, 0x00, // mov edi, 1
        0x0f, 0x05, // syscall
        0xb8, 0x01, 0x00, 0x00, 0x00, // mov eax, 1 (LINUX_WRITE)
        0xbf, 0x01, 0x00, 0x00, 0x00, // mov edi, 1
        0x31, 0xf6, // xor esi, esi
        0xba, 0x01, 0x00, 0x00, 0x00, // mov edx, 1
        0x0f, 0x05, // syscall
        0x89, 0xc7, // mov edi, eax
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT)
        0xcd, 0x80, // int 0x80
    ]
}

#[derive(Clone, Copy)]
pub struct LinuxForkFdResult {
    pub parent_exit: u64,
    pub child_exit: u64,
    pub reaped: usize,
    pub verified: bool,
}

fn failed_linux_fork_fd_result() -> LinuxForkFdResult {
    LinuxForkFdResult {
        parent_exit: 0,
        child_exit: 0,
        reaped: 0,
        verified: false,
    }
}

pub fn linux_fork_fd_self_test(state: &crate::arch::paging::PagingState) -> LinuxForkFdResult {
    initialize();
    for slot in USER_TASK_EXIT.iter() {
        slot.store(u64::MAX, Ordering::Release);
    }
    let code = build_fork_fd_probe();
    let Some((parent_id, parent_slot, parent_space)) = spawn_process(state, &code) else {
        return failed_linux_fork_fd_result();
    };
    if !parent_space.verified {
        return failed_linux_fork_fd_result();
    }
    let child_id = parent_id + 1;
    PREEMPTION_ENABLED.store(1, Ordering::Release);
    if !arch::apic::start_timer(1000) {
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_linux_fork_fd_result();
    }
    let parent_exit = loop {
        let value = USER_TASK_EXIT[parent_slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    let Some(child_slot) = slot_for_id(child_id) else {
        arch::apic::stop_timer();
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_linux_fork_fd_result();
    };
    let child_exit = loop {
        let value = USER_TASK_EXIT[child_slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    arch::apic::stop_timer();
    PREEMPTION_ENABLED.store(0, Ordering::Release);
    let reaped = reap();
    let after = stats();
    LinuxForkFdResult {
        parent_exit,
        child_exit,
        reaped,
        verified: parent_exit == 1 && child_exit == 0xffff_fff7 && reaped == 2 && after.tasks == 1,
    }
}

fn build_fork_brk_probe() -> [u8; 108] {
    // fork(), then both parent and child independently query their current
    // brk via the REAL Linux syscall (LINUX_BRK=12, request=0), grow it by
    // exactly one page, and write a distinct marker into the new page --
    // proving real syscall-convention brk() now works against the per-
    // process heap built earlier this session, with fork() giving each
    // side its own independent copy (not a shared page).
    [
        0xb8, 0x02, 0x00, 0x00, 0x00, // mov eax, 2 (SYS_FORK)
        0xcd, 0x80, // int 0x80
        0x83, 0xf8, 0x00, // cmp eax, 0
        0x74, 0x30, // je child_path (+48)
        // parent path:
        0xb8, 0x0c, 0x00, 0x00, 0x00, // mov eax, 12 (LINUX_BRK)
        0x31, 0xff, // xor edi, edi (request=0, query)
        0x0f, 0x05, // syscall -> rax = current brk
        0x49, 0x89, 0xc4, // mov r12, rax
        0x49, 0x81, 0xc4, 0x00, 0x10, 0x00, 0x00, // add r12, 4096
        0xb8, 0x0c, 0x00, 0x00, 0x00, // mov eax, 12 (LINUX_BRK)
        0x4c, 0x89, 0xe7, // mov rdi, r12 (request = new target)
        0x0f, 0x05, // syscall -> rax = new brk
        0xc7, 0x40, 0xf8, 0xaa, 0xaa, 0xaa, 0xaa, // mov dword ptr [rax-8], 0xaaaaaaaa
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT)
        0xbf, 0x0b, 0x00, 0x00, 0x00, // mov edi, 11
        0xcd, 0x80, // int 0x80
        // child_path:
        0xb8, 0x0c, 0x00, 0x00, 0x00, // mov eax, 12 (LINUX_BRK)
        0x31, 0xff, // xor edi, edi
        0x0f, 0x05, // syscall
        0x49, 0x89, 0xc4, // mov r12, rax
        0x49, 0x81, 0xc4, 0x00, 0x10, 0x00, 0x00, // add r12, 4096
        0xb8, 0x0c, 0x00, 0x00, 0x00, // mov eax, 12 (LINUX_BRK)
        0x4c, 0x89, 0xe7, // mov rdi, r12
        0x0f, 0x05, // syscall
        0xc7, 0x40, 0xf8, 0xbb, 0xbb, 0xbb, 0xbb, // mov dword ptr [rax-8], 0xbbbbbbbb
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT)
        0xbf, 0x16, 0x00, 0x00, 0x00, // mov edi, 22
        0xcd, 0x80, // int 0x80
    ]
}

#[derive(Clone, Copy)]
pub struct LinuxBrkForkResult {
    pub parent_exit: u64,
    pub child_exit: u64,
    pub parent_value: u32,
    pub child_value: u32,
    pub reaped: usize,
    pub verified: bool,
}

fn failed_linux_brk_fork_result() -> LinuxBrkForkResult {
    LinuxBrkForkResult {
        parent_exit: 0,
        child_exit: 0,
        parent_value: 0,
        child_value: 0,
        reaped: 0,
        verified: false,
    }
}

pub fn linux_brk_fork_self_test(state: &crate::arch::paging::PagingState) -> LinuxBrkForkResult {
    initialize();
    for slot in USER_TASK_EXIT.iter() {
        slot.store(u64::MAX, Ordering::Release);
    }
    let code = build_fork_brk_probe();
    let Some((parent_id, parent_slot, parent_space)) = spawn_process(state, &code) else {
        return failed_linux_brk_fork_result();
    };
    if !parent_space.verified {
        return failed_linux_brk_fork_result();
    }
    let child_id = parent_id + 1;
    PREEMPTION_ENABLED.store(1, Ordering::Release);
    if !arch::apic::start_timer(1000) {
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_linux_brk_fork_result();
    }
    let parent_exit = loop {
        let value = USER_TASK_EXIT[parent_slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    let Some(child_slot) = slot_for_id(child_id) else {
        arch::apic::stop_timer();
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_linux_brk_fork_result();
    };
    let child_exit = loop {
        let value = USER_TASK_EXIT[child_slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    arch::apic::stop_timer();
    PREEMPTION_ENABLED.store(0, Ordering::Release);
    // Both `parent_space` and `child_space` (from spawn_process/task lookup)
    // must be read LIVE, after the heap actually grew -- a pre-run snapshot
    // would still show zero heap pages (this exact bug was caught and fixed
    // earlier this session in the plain sbrk() self-test).
    let live_parent_space = unsafe { (*SCHEDULER.0.get()).tasks[parent_slot].process_space };
    let child_space = unsafe { (*SCHEDULER.0.get()).tasks[child_slot].process_space };
    let (Some(live_parent_space), Some(child_space)) = (live_parent_space, child_space) else {
        return failed_linux_brk_fork_result();
    };
    const MARKER_OFFSET: u64 = 4096 - 8;
    let parent_value = unsafe {
        core::ptr::read_volatile(
            (live_parent_space.heap_physical[0] + MARKER_OFFSET) as usize as *const u32,
        )
    };
    let child_value = unsafe {
        core::ptr::read_volatile(
            (child_space.heap_physical[0] + MARKER_OFFSET) as usize as *const u32,
        )
    };
    let reaped = reap();
    let after = stats();
    LinuxBrkForkResult {
        parent_exit,
        child_exit,
        parent_value,
        child_value,
        reaped,
        verified: parent_exit == 11
            && child_exit == 22
            && parent_value == 0xaaaa_aaaa
            && child_value == 0xbbbb_bbbb
            && live_parent_space.heap_physical[0] != child_space.heap_physical[0]
            && reaped == 2
            && after.tasks == 1,
    }
}

fn build_fork_mmap_probe() -> [u8; 271] {
    // fork(), then parent and child independently exercise the REAL Linux
    // mmap(2)/mprotect(2)/munmap(2) syscalls against their own per-process
    // mmap region:
    //   - parent: mmap's two pages (proving distinct slots), munmap's the
    //     first, then mmap's again (proving the freed slot is reused with a
    //     fresh mapping) -- verified by reading the marker each mmap wrote.
    //   - child: mmap's a page, writes a marker, mprotect's it to
    //     PROT_READ, then attempts to write again -- this must fault
    //     (proving mprotect actually changed the hardware page-table
    //     protection, not just a bookkeeping field), terminating the
    //     process with SIGSEGV (exit code 128+11=139) before it ever
    //     reaches its own fallback exit(99).
    [
        0xb8, 0x02, 0x00, 0x00, 0x00, // mov eax, 2 (SYS_FORK)
        0xcd, 0x80, // int 0x80
        0x83, 0xf8, 0x00, // cmp eax, 0
        0x0f, 0x84, 0xaa, 0x00, 0x00, 0x00, // je child_path (+170)
        // parent path:
        // mmap #1 (index 0) -> rbx
        0xb8, 0x09, 0x00, 0x00, 0x00, // mov eax, 9 (LINUX_MMAP)
        0x31, 0xff, // xor edi, edi (addr=0)
        0xbe, 0x00, 0x10, 0x00, 0x00, // mov esi, 4096 (length)
        0xba, 0x03, 0x00, 0x00, 0x00, // mov edx, 3 (PROT_READ|PROT_WRITE)
        0x41, 0xba, 0x22, 0x00, 0x00, 0x00, // mov r10d, 0x22 (MAP_PRIVATE|MAP_ANONYMOUS)
        0x49, 0xc7, 0xc0, 0xff, 0xff, 0xff, 0xff, // mov r8, -1 (fd, sign-extended)
        0x41, 0xb9, 0x00, 0x00, 0x00, 0x00, // mov r9d, 0 (offset)
        0x0f, 0x05, // syscall -> rax = addr1
        0x48, 0x89, 0xc3, // mov rbx, rax
        0xc7, 0x03, 0x01, 0x00, 0xaa, 0xaa, // mov dword ptr [rbx], 0xaaaa0001
        // mmap #2 (index 1) -> r14
        0xb8, 0x09, 0x00, 0x00, 0x00, // mov eax, 9
        0x31, 0xff, // xor edi, edi
        0xbe, 0x00, 0x10, 0x00, 0x00, // mov esi, 4096
        0xba, 0x03, 0x00, 0x00, 0x00, // mov edx, 3
        0x41, 0xba, 0x22, 0x00, 0x00, 0x00, // mov r10d, 0x22
        0x49, 0xc7, 0xc0, 0xff, 0xff, 0xff, 0xff, // mov r8, -1
        0x41, 0xb9, 0x00, 0x00, 0x00, 0x00, // mov r9d, 0
        0x0f, 0x05, // syscall -> rax = addr2
        0x49, 0x89, 0xc6, // mov r14, rax
        0x41, 0xc7, 0x06, 0x02, 0x00, 0xaa, 0xaa, // mov dword ptr [r14], 0xaaaa0002
        // munmap(addr1, 4096) -- frees index 0
        0xb8, 0x0b, 0x00, 0x00, 0x00, // mov eax, 11 (LINUX_MUNMAP)
        0x48, 0x89, 0xdf, // mov rdi, rbx
        0xbe, 0x00, 0x10, 0x00, 0x00, // mov esi, 4096
        0x0f, 0x05, // syscall
        // mmap #3 -- should reuse freed index 0 -> r15
        0xb8, 0x09, 0x00, 0x00, 0x00, // mov eax, 9
        0x31, 0xff, // xor edi, edi
        0xbe, 0x00, 0x10, 0x00, 0x00, // mov esi, 4096
        0xba, 0x03, 0x00, 0x00, 0x00, // mov edx, 3
        0x41, 0xba, 0x22, 0x00, 0x00, 0x00, // mov r10d, 0x22
        0x49, 0xc7, 0xc0, 0xff, 0xff, 0xff, 0xff, // mov r8, -1
        0x41, 0xb9, 0x00, 0x00, 0x00, 0x00, // mov r9d, 0
        0x0f, 0x05, // syscall -> rax = addr3
        0x49, 0x89, 0xc7, // mov r15, rax
        0x41, 0xc7, 0x07, 0x03, 0x00, 0xaa, 0xaa, // mov dword ptr [r15], 0xaaaa0003
        // exit(51)
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0 (SYS_EXIT, bootstrap ABI)
        0xbf, 0x33, 0x00, 0x00, 0x00, // mov edi, 51
        0xcd, 0x80, // int 0x80
        // child_path:
        // mmap RW page -> rbx
        0xb8, 0x09, 0x00, 0x00, 0x00, // mov eax, 9
        0x31, 0xff, // xor edi, edi
        0xbe, 0x00, 0x10, 0x00, 0x00, // mov esi, 4096
        0xba, 0x03, 0x00, 0x00, 0x00, // mov edx, 3
        0x41, 0xba, 0x22, 0x00, 0x00, 0x00, // mov r10d, 0x22
        0x49, 0xc7, 0xc0, 0xff, 0xff, 0xff, 0xff, // mov r8, -1
        0x41, 0xb9, 0x00, 0x00, 0x00, 0x00, // mov r9d, 0
        0x0f, 0x05, // syscall -> rax = addr
        0x48, 0x89, 0xc3, // mov rbx, rax
        0xc7, 0x03, 0x01, 0x00, 0xbb, 0xbb, // mov dword ptr [rbx], 0xbbbb0001
        // mprotect(addr, 4096, PROT_READ)
        0xb8, 0x0a, 0x00, 0x00, 0x00, // mov eax, 10 (LINUX_MPROTECT)
        0x48, 0x89, 0xdf, // mov rdi, rbx
        0xbe, 0x00, 0x10, 0x00, 0x00, // mov esi, 4096
        0xba, 0x01, 0x00, 0x00, 0x00, // mov edx, 1 (PROT_READ)
        0x0f, 0x05, // syscall
        // illegal write -- must fault (SIGSEGV, exit 139)
        0xc7, 0x03, 0x02, 0x00, 0xbb, 0xbb, // mov dword ptr [rbx], 0xbbbb0002
        // unreachable if mprotect worked -- exit(99) as an obvious failure marker
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax, 0
        0xbf, 0x63, 0x00, 0x00, 0x00, // mov edi, 99
        0xcd, 0x80, // int 0x80
    ]
}

#[derive(Clone, Copy)]
pub struct LinuxMmapForkResult {
    pub parent_exit: u64,
    pub child_exit: u64,
    pub parent_marker_second: u32,
    pub parent_marker_reused: u32,
    pub child_marker: u32,
    pub reaped: usize,
    pub verified: bool,
}

fn failed_linux_mmap_fork_result() -> LinuxMmapForkResult {
    LinuxMmapForkResult {
        parent_exit: 0,
        child_exit: 0,
        parent_marker_second: 0,
        parent_marker_reused: 0,
        child_marker: 0,
        reaped: 0,
        verified: false,
    }
}

pub fn linux_mmap_fork_self_test(state: &crate::arch::paging::PagingState) -> LinuxMmapForkResult {
    initialize();
    for slot in USER_TASK_EXIT.iter() {
        slot.store(u64::MAX, Ordering::Release);
    }
    let code = build_fork_mmap_probe();
    let Some((parent_id, parent_slot, parent_space)) = spawn_process(state, &code) else {
        return failed_linux_mmap_fork_result();
    };
    if !parent_space.verified {
        return failed_linux_mmap_fork_result();
    }
    let child_id = parent_id + 1;
    PREEMPTION_ENABLED.store(1, Ordering::Release);
    if !arch::apic::start_timer(1000) {
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_linux_mmap_fork_result();
    }
    let parent_exit = loop {
        let value = USER_TASK_EXIT[parent_slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    let Some(child_slot) = slot_for_id(child_id) else {
        arch::apic::stop_timer();
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_linux_mmap_fork_result();
    };
    let child_exit = loop {
        let value = USER_TASK_EXIT[child_slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    arch::apic::stop_timer();
    PREEMPTION_ENABLED.store(0, Ordering::Release);
    // Read the live per-process mmap-page contents BEFORE reap() runs
    // destroy_process and frees them, same rule the brk/fork test follows.
    let live_parent_space = unsafe { (*SCHEDULER.0.get()).tasks[parent_slot].process_space };
    let child_space = unsafe { (*SCHEDULER.0.get()).tasks[child_slot].process_space };
    let (Some(live_parent_space), Some(child_space)) = (live_parent_space, child_space) else {
        return failed_linux_mmap_fork_result();
    };
    let parent_marker_reused = unsafe {
        core::ptr::read_volatile(live_parent_space.mmap_physical[0] as usize as *const u32)
    };
    let parent_marker_second = unsafe {
        core::ptr::read_volatile(live_parent_space.mmap_physical[1] as usize as *const u32)
    };
    let child_marker =
        unsafe { core::ptr::read_volatile(child_space.mmap_physical[0] as usize as *const u32) };
    let reaped = reap();
    let after = stats();
    LinuxMmapForkResult {
        parent_exit,
        child_exit,
        parent_marker_second,
        parent_marker_reused,
        child_marker,
        reaped,
        verified: parent_exit == 51
            && child_exit == 139
            && parent_marker_reused == 0xaaaa_0003
            && parent_marker_second == 0xaaaa_0002
            && child_marker == 0xbbbb_0001
            && reaped == 2
            && after.tasks == 1,
    }
}

fn build_fork_close_refcount_probe() -> [u8; 226] {
    // Opens a REAL vfs-backed file (not a standard fd), forks, and proves
    // the cross-process handle-refcounting machinery in `syscall.rs`
    // (`remove_process_fd`'s `scheduler::any_other_task_shares_fd` check,
    // `close_all_process_fds`) actually protects a live vfs handle end to
    // end: the child closes ONLY its own copy of fd 3 and exits (its real
    // close() result is folded into the parent's exit code below, via
    // wait4()'s return value); the parent - having wait4()'d the child, so
    // ordering is deterministic, not a race - writes to the SAME fd 3
    // again, which only succeeds if the real underlying vfs handle
    // survived the child's close(). The parent then closes its own (now
    // the LAST) reference and writes once more, which must now fail with
    // -EBADF, proving the handle really was torn down once nothing
    // referenced it anymore, not leaked open forever.
    [
        0xb8, 0x02, 0x00, 0x00, 0x00, // mov eax,2 (LINUX_OPEN)
        0x48, 0x8d, 0x3d, 0xc8, 0x00, 0x00, 0x00, // lea rdi,[rip+PATH]
        0xbe, 0x41, 0x02, 0x00, 0x00, // mov esi,0x241 (O_WRONLY|O_CREAT|O_TRUNC)
        0xba, 0xa4, 0x01, 0x00, 0x00, // mov edx,0x1A4 (mode 0644)
        0x0f, 0x05, // syscall -> fd 3 (fresh process: 0/1/2 are standard)
        0xb8, 0x01, 0x00, 0x00, 0x00, // mov eax,1 (LINUX_WRITE)
        0xbf, 0x03, 0x00, 0x00, 0x00, // mov edi,3
        0x48, 0x8d, 0x35, 0xb8, 0x00, 0x00, 0x00, // lea rsi,[rip+DUMMY]
        0xba, 0x01, 0x00, 0x00, 0x00, // mov edx,1
        0x0f, 0x05, // syscall
        0xb8, 0x02, 0x00, 0x00, 0x00, // mov eax,2 (SYS_FORK)
        0xcd, 0x80, // int 0x80
        0x83, 0xf8, 0x00, // cmp eax,0
        0x75, 0x15, // jne L_parent
        // L_child (fall-through when eax==0): close only its own fd-table
        // copy, then exit with close()'s own result (0 on success) - the
        // parent's wait4() below receives this as its real return value.
        0xb8, 0x03, 0x00, 0x00, 0x00, // mov eax,3 (LINUX_CLOSE)
        0xbf, 0x03, 0x00, 0x00, 0x00, // mov edi,3
        0x0f, 0x05, // syscall
        0x89, 0xc7, // mov edi,eax
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax,0 (SYS_EXIT)
        0xcd, 0x80, // int 0x80
        // L_parent: eax = child task id from fork()
        0x89, 0xc7, // mov edi,eax (wait4 arg0 = child id)
        0xb8, 0x04, 0x00, 0x00, 0x00, // mov eax,4 (SYS_WAIT4)
        0xcd, 0x80, // int 0x80 (block+reap child; eax=child's close() result)
        // phase 0: the child's own close() of ITS fd-table copy must have
        // succeeded (proves the machinery didn't just refuse to close at
        // all - it has to actually work for the non-shared case too).
        0x83, 0xf8, 0x00, // cmp eax,0
        0x75, 0x54, // jne L_phase0_failed
        // phase 1: fd 3 must still be usable after the child's close().
        0xb8, 0x01, 0x00, 0x00, 0x00, // mov eax,1 (LINUX_WRITE)
        0xbf, 0x03, 0x00, 0x00, 0x00, // mov edi,3
        0x48, 0x8d, 0x35, 0x71, 0x00, 0x00, 0x00, // lea rsi,[rip+DUMMY]
        0xba, 0x01, 0x00, 0x00, 0x00, // mov edx,1
        0x0f, 0x05, // syscall
        0x83, 0xf8, 0x01, // cmp eax,1
        0x75, 0x3e, // jne L_phase1_failed
        // phase 2: close the parent's own (now last) reference.
        0xb8, 0x03, 0x00, 0x00, 0x00, // mov eax,3 (LINUX_CLOSE)
        0xbf, 0x03, 0x00, 0x00, 0x00, // mov edi,3
        0x0f, 0x05, // syscall
        0x83, 0xf8, 0x00, // cmp eax,0
        0x75, 0x34, // jne L_phase2_failed
        // phase 3: now fd 3 must be genuinely gone.
        0xb8, 0x01, 0x00, 0x00, 0x00, // mov eax,1 (LINUX_WRITE)
        0xbf, 0x03, 0x00, 0x00, 0x00, // mov edi,3
        0x48, 0x8d, 0x35, 0x43, 0x00, 0x00, 0x00, // lea rsi,[rip+DUMMY]
        0xba, 0x01, 0x00, 0x00, 0x00, // mov edx,1
        0x0f, 0x05, // syscall
        0x3d, 0xf7, 0xff, 0xff, 0xff, // cmp eax,-9 (EBADF)
        0x75, 0x1c, // jne L_phase3_failed
        0xbf, 0xc8, 0x00, 0x00, 0x00, // mov edi,200 (every phase passed)
        0xeb, 0x1a, // jmp L_exit
        0xbf, 0x6e, 0x00, 0x00, 0x00, // L_phase0_failed: mov edi,110
        0xeb, 0x13, // jmp L_exit
        0xbf, 0x6f, 0x00, 0x00, 0x00, // L_phase1_failed: mov edi,111
        0xeb, 0x0c, // jmp L_exit
        0xbf, 0x70, 0x00, 0x00, 0x00, // L_phase2_failed: mov edi,112
        0xeb, 0x05, // jmp L_exit
        0xbf, 0x71, 0x00, 0x00, 0x00, // L_phase3_failed: mov edi,113
        // L_exit (falls through from L_phase3_failed too):
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax,0 (SYS_EXIT)
        0xcd, 0x80, // int 0x80 (parent exits)
        // PATH: "/tmp/FDSHARE" followed by a trailing NUL terminator
        0x2f, 0x74, 0x6d, 0x70, 0x2f, 0x46, 0x44, 0x53, 0x48, 0x41, 0x52, 0x45, 0x00,
        // DUMMY 'X' (write source; content is never checked)
        0x58,
    ]
}

#[derive(Clone, Copy)]
pub struct ForkCloseRefcountResult {
    /// Composite verdict from the hand-assembled parent program: 200 means
    /// every phase passed; 110/111/112/113 pinpoint which one didn't (see
    /// `build_fork_close_refcount_probe`'s L_phase*_failed labels).
    pub parent_exit: u64,
    pub reaped: usize,
    pub verified: bool,
}

fn failed_fork_close_refcount_result() -> ForkCloseRefcountResult {
    ForkCloseRefcountResult {
        parent_exit: 0,
        reaped: 0,
        verified: false,
    }
}

pub fn fork_close_refcount_self_test(
    state: &crate::arch::paging::PagingState,
) -> ForkCloseRefcountResult {
    initialize();
    for slot in USER_TASK_EXIT.iter() {
        slot.store(u64::MAX, Ordering::Release);
    }
    let code = build_fork_close_refcount_probe();
    let Some((_parent_id, parent_slot, parent_space)) = spawn_process(state, &code) else {
        return failed_fork_close_refcount_result();
    };
    if !parent_space.verified {
        return failed_fork_close_refcount_result();
    }
    PREEMPTION_ENABLED.store(1, Ordering::Release);
    if !arch::apic::start_timer(1000) {
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_fork_close_refcount_result();
    }
    // The parent's own wait4() blocks until the child (which closes its OWN
    // fd-table copy of the shared handle and exits) is done and reaped, so
    // by the time this loop sees the parent's own exit, both the child's
    // close() and every phase of the parent's post-close probing already
    // ran, in that guaranteed order - no race.
    let parent_exit = loop {
        let value = USER_TASK_EXIT[parent_slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    arch::apic::stop_timer();
    PREEMPTION_ENABLED.store(0, Ordering::Release);
    // wait4() already reaped the child; this reaps just the parent.
    let reaped = reap();
    let after = stats();
    ForkCloseRefcountResult {
        parent_exit,
        reaped,
        // 200 only happens if phase0 (child's own close() succeeded),
        // phase1 (parent's write survived the child's close()), phase2
        // (parent's own close() succeeded) and phase3 (a write after that
        // now genuinely fails with -EBADF) all passed, in that order -
        // `reaped==1` confirms wait4() already reaped the child by itself.
        verified: parent_exit == 200 && reaped == 1 && after.tasks == 1,
    }
}

fn build_file_mmap_probe() -> [u8; 162] {
    // Writes a real file, then maps it PROT_READ/MAP_PRIVATE through this
    // task's own `process_space` (the `linux_mmap_for_current_task` bridge
    // in scheduler.rs, wired into `syscall.rs`'s `linux_mmap` alongside the
    // anonymous case) - `linux_mmap_fork_self_test` above only ever
    // exercises MAP_ANONYMOUS, so this is the first proof the file-backed
    // half (`mmap_fill_from_file`/`process_mmap_write`) actually streams
    // real file bytes into the mapping, not just zeroed pages. The guest
    // only sanity-checks that mmap's return value isn't a negative errno
    // (`test eax,eax; js`) - the real proof is the host reading the
    // mapped page's physical content directly afterward.
    [
        0xb8, 0x02, 0x00, 0x00, 0x00, // mov eax,2 (LINUX_OPEN)
        0x48, 0x8d, 0x3d, 0x88, 0x00, 0x00, 0x00, // lea rdi,[rip+PATH]
        0xbe, 0x41, 0x02, 0x00, 0x00, // mov esi,0x241 (O_WRONLY|O_CREAT|O_TRUNC)
        0xba, 0xa4, 0x01, 0x00, 0x00, // mov edx,0x1A4 (mode 0644)
        0x0f, 0x05, // syscall -> fd 3
        0xb8, 0x01, 0x00, 0x00, 0x00, // mov eax,1 (LINUX_WRITE)
        0xbf, 0x03, 0x00, 0x00, 0x00, // mov edi,3
        0x48, 0x8d, 0x35, 0x67, 0x00, 0x00, 0x00, // lea rsi,[rip+DATA]
        0xba, 0x04, 0x00, 0x00, 0x00, // mov edx,4
        0x0f, 0x05, // syscall
        0xb8, 0x03, 0x00, 0x00, 0x00, // mov eax,3 (LINUX_CLOSE)
        0xbf, 0x03, 0x00, 0x00, 0x00, // mov edi,3
        0x0f, 0x05, // syscall
        0xb8, 0x02, 0x00, 0x00, 0x00, // mov eax,2 (LINUX_OPEN)
        0x48, 0x8d, 0x3d, 0x4c, 0x00, 0x00, 0x00, // lea rdi,[rip+PATH]
        0xbe, 0x00, 0x00, 0x00, 0x00, // mov esi,0 (O_RDONLY)
        0xba, 0x00, 0x00, 0x00, 0x00, // mov edx,0 (mode ignored)
        0x0f, 0x05, // syscall -> fd 3 (slot reused)
        0xb8, 0x09, 0x00, 0x00, 0x00, // mov eax,9 (LINUX_MMAP)
        0x31, 0xff, // xor edi,edi (addr=0)
        0xbe, 0x00, 0x10, 0x00, 0x00, // mov esi,0x1000 (length=4096)
        0xba, 0x01, 0x00, 0x00, 0x00, // mov edx,1 (PROT_READ)
        0x41, 0xba, 0x02, 0x00, 0x00, 0x00, // mov r10d,2 (MAP_PRIVATE, file-backed)
        0x41, 0xb8, 0x03, 0x00, 0x00, 0x00, // mov r8d,3 (fd)
        0x41, 0xb9, 0x00, 0x00, 0x00, 0x00, // mov r9d,0 (offset)
        0x0f, 0x05, // syscall -> rax=mapped address or -errno
        0x85, 0xc0, // test eax,eax
        0x78, 0x07, // js L_failed
        0xbf, 0x37, 0x00, 0x00, 0x00, // mov edi,55 (success)
        0xeb, 0x05, // jmp L_exit
        0xbf, 0x42, 0x00, 0x00, 0x00, // L_failed: mov edi,66 (failure)
        // L_exit:
        0xb8, 0x00, 0x00, 0x00, 0x00, // mov eax,0 (SYS_EXIT)
        0xcd, 0x80, // int 0x80
        0x41, 0x42, 0x43, 0x44, // DATA "ABCD"
        0x2f, 0x74, 0x6d, 0x70, 0x2f, 0x4d, 0x4d, 0x41, 0x50, 0x46, 0x49, 0x4c, 0x45,
        0x00, // PATH "/tmp/MMAPFILE" + NUL terminator
    ]
}

#[derive(Clone, Copy)]
pub struct FileMmapSelfTestResult {
    pub exit_code: u64,
    /// The mapped page's first 4 bytes, read directly from physical
    /// memory before the process is reaped - the real proof, independent
    /// of whatever the guest program itself believed about its own mmap.
    pub mapped_bytes: [u8; 4],
    pub reaped: usize,
    pub verified: bool,
}

fn failed_file_mmap_result() -> FileMmapSelfTestResult {
    FileMmapSelfTestResult {
        exit_code: 0,
        mapped_bytes: [0; 4],
        reaped: 0,
        verified: false,
    }
}

pub fn file_mmap_self_test(state: &crate::arch::paging::PagingState) -> FileMmapSelfTestResult {
    initialize();
    for slot in USER_TASK_EXIT.iter() {
        slot.store(u64::MAX, Ordering::Release);
    }
    let code = build_file_mmap_probe();
    let Some((_id, slot, space)) = spawn_process(state, &code) else {
        return failed_file_mmap_result();
    };
    if !space.verified {
        return failed_file_mmap_result();
    }
    PREEMPTION_ENABLED.store(1, Ordering::Release);
    if !arch::apic::start_timer(1000) {
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_file_mmap_result();
    }
    let exit_code = loop {
        let value = USER_TASK_EXIT[slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    arch::apic::stop_timer();
    PREEMPTION_ENABLED.store(0, Ordering::Release);
    // Read the mapped page's live physical content BEFORE reap() runs
    // destroy_process and frees it - same rule the anonymous mmap/brk
    // fork tests already follow.
    let live_space = unsafe { (*SCHEDULER.0.get()).tasks[slot].process_space };
    let Some(live_space) = live_space else {
        return failed_file_mmap_result();
    };
    let mut mapped_bytes = [0u8; 4];
    if live_space.mmap_physical[0] != 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(
                live_space.mmap_physical[0] as usize as *const u8,
                mapped_bytes.as_mut_ptr(),
                4,
            );
        }
    }
    let reaped = reap();
    let after = stats();
    FileMmapSelfTestResult {
        exit_code,
        mapped_bytes,
        reaped,
        verified: exit_code == 55 && mapped_bytes == *b"ABCD" && reaped == 1 && after.tasks == 1,
    }
}

#[derive(Clone, Copy)]
pub struct RealElfSelfTestResult {
    pub exit_code: u64,
    pub reaped: usize,
    pub verified: bool,
}

fn failed_real_elf_result() -> RealElfSelfTestResult {
    RealElfSelfTestResult {
        exit_code: 0,
        reaped: 0,
        verified: false,
    }
}

/// Every self-test above this line feeds `spawn_process` a tiny hand-
/// assembled machine-code blob (`create_process`'s `create_process_raw`
/// fallback, one page, no segments). This is the first one to feed it a
/// REAL, already-compiled, multi-segment ELF binary instead - by default
/// `/bin/aeros-init` (rust-static-pie, 3 real segments with distinct
/// executable/writable pages, the same file the OLD single-shot
/// `arch::user::run_image` path already runs earlier in `main.rs`'s boot
/// sequence) - so this proves the fork/exec-capable per-process-CR3
/// loader (`create_process_from_elf`, `process::prepare_scheduled_process
/// _stack`'s argv/envp/auxv setup) can carry a real ELF binary through the
/// scheduler end to end, not just the boot-time synchronous path.
/// `expected_exit` should be whatever exit code that same binary produces
/// through the OLD path, so a match here is proof this loader path is not
/// just "didn't crash" but ran the SAME real program to the SAME
/// conclusion.
/// (`/bin/aeros-std-smoke` also runs here now: the real blocker was mmap/brk
/// addressing from `space.entry` instead of the process base, not stack size.)
pub fn real_elf_self_test(
    state: &crate::arch::paging::PagingState,
    code: &[u8],
    expected_exit: u64,
) -> RealElfSelfTestResult {
    initialize();
    for slot in USER_TASK_EXIT.iter() {
        slot.store(u64::MAX, Ordering::Release);
    }
    let Some((_id, slot, space)) = spawn_process(state, code) else {
        return failed_real_elf_result();
    };
    if !space.verified {
        return failed_real_elf_result();
    }
    PREEMPTION_ENABLED.store(1, Ordering::Release);
    if !arch::apic::start_timer(1000) {
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_real_elf_result();
    }
    // A real std-linked binary does meaningfully more work than the
    // hand-assembled probes above (filesystem syscalls, libc/runtime
    // init), so give it a generous number of ticks to finish rather than
    // reusing a tighter bound only ever validated against tiny probes.
    let mut ticks = 0u32;
    let exit_code = loop {
        let value = USER_TASK_EXIT[slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        ticks += 1;
        if ticks > 20_000 {
            break u64::MAX;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    arch::apic::stop_timer();
    PREEMPTION_ENABLED.store(0, Ordering::Release);
    let reaped = reap();
    let after = stats();
    RealElfSelfTestResult {
        exit_code,
        reaped,
        verified: exit_code == expected_exit && reaped == 1 && after.tasks == 1,
    }
}

#[derive(Clone, Copy)]
pub struct RealElfForkSelfTestResult {
    pub parent_exit: u64,
    pub marker: u32,
    pub reaped: usize,
    pub verified: bool,
}

fn failed_real_elf_fork_result() -> RealElfForkSelfTestResult {
    RealElfForkSelfTestResult {
        parent_exit: 0,
        marker: 0,
        reaped: 0,
        verified: false,
    }
}

/// `real_elf_self_test` proves a real compiled ELF can RUN under the
/// scheduler; this proves one can genuinely FORK there too - spawns
/// `/bin/aeros-fork-probe` (a tiny real toolchain-built binary, not hand
/// assembled, whose whole job is `fork()`/`wait4()`/write-a-marker; see
/// userspace/fork-probe/src/main.rs), which internally forks, has the
/// child exit(22), has the parent wait4() (forwarding the child's real
/// exit status) and only then write a marker onto ITS OWN fork-copied
/// stack before exiting(11). The host does NOT try to read the child's
/// memory (by the time the parent's own exit is visible here, wait4()
/// has already reaped the child internally and freed its pages, the same
/// timing `fork_close_refcount_self_test` above already had to work
/// around) - `parent_exit == 11` alone already proves wait4() forwarded
/// the real child status, since that is the only path to that exit code.
/// The marker is read back from a HOST-COMPUTED physical address
/// (`stack_physical + (stack_top - process_stack_base()) - 8`) rather
/// than a hardcoded offset like the raw-blob probes use, because a real
/// ELF's `stack_top` comes from `prepare_scheduled_process_stack`'s
/// dynamic argv/auxv layout, not a fixed one.
pub fn real_elf_fork_self_test(
    state: &crate::arch::paging::PagingState,
    code: &[u8],
) -> RealElfForkSelfTestResult {
    initialize();
    for slot in USER_TASK_EXIT.iter() {
        slot.store(u64::MAX, Ordering::Release);
    }
    let Some((_id, slot, space)) = spawn_process(state, code) else {
        return failed_real_elf_fork_result();
    };
    if !space.verified {
        return failed_real_elf_fork_result();
    }
    PREEMPTION_ENABLED.store(1, Ordering::Release);
    if !arch::apic::start_timer(1000) {
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed_real_elf_fork_result();
    }
    let parent_exit = loop {
        let value = USER_TASK_EXIT[slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    arch::apic::stop_timer();
    PREEMPTION_ENABLED.store(0, Ordering::Release);
    // Read the marker off the PARENT's own (still live) stack before the
    // outer reap() below frees it - same rule every other fork test here
    // follows for reading a physical page back.
    let live_space = unsafe { (*SCHEDULER.0.get()).tasks[slot].process_space };
    let marker = live_space
        .filter(|space| space.stack_top >= arch::paging::process_stack_base())
        .map(|space| {
            let byte_offset = space.stack_top - arch::paging::process_stack_base();
            let address = space.stack_physical + byte_offset - 8;
            unsafe { core::ptr::read_volatile(address as usize as *const u32) }
        })
        .unwrap_or(0);
    // wait4() already reaped the child; this reaps just the parent.
    let reaped = reap();
    let after = stats();
    RealElfForkSelfTestResult {
        parent_exit,
        marker,
        reaped,
        verified: parent_exit == 11 && marker == 0xaaaa_0001 && reaped == 1 && after.tasks == 1,
    }
}

/// Linux-ABI probe: grows the heap one page, writes 0xAAAA0001, forks; the
/// child overwrites it with 0xBBBB0001; the parent (after wait4) must still
/// read 0xAAAA0001 (copy-on-write) and exits 11 (else 125).
pub const LINUX_COW_PROBE: [u8; 107] = [
    0xb8, 0x0c, 0x00, 0x00, 0x00, 0x31, 0xff, 0x0f, 0x05, 0x48, 0x89, 0xc3, 0x48, 0x8d, 0xbb, 0x00,
    0x10, 0x00, 0x00, 0xb8, 0x0c, 0x00, 0x00, 0x00, 0x0f, 0x05, 0xc7, 0x03, 0x01, 0x00, 0xaa, 0xaa,
    0xb8, 0x39, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x85, 0xc0, 0x75, 0x12, 0xc7, 0x03, 0x01, 0x00, 0xbb,
    0xbb, 0xb8, 0x3c, 0x00, 0x00, 0x00, 0xbf, 0x16, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x89, 0xc7, 0x48,
    0x8d, 0x74, 0x24, 0xf0, 0x31, 0xd2, 0x45, 0x31, 0xd2, 0xb8, 0x3d, 0x00, 0x00, 0x00, 0x0f, 0x05,
    0x81, 0x3b, 0x01, 0x00, 0xaa, 0xaa, 0x75, 0x07, 0xbf, 0x0b, 0x00, 0x00, 0x00, 0xeb, 0x05, 0xbf,
    0x7d, 0x00, 0x00, 0x00, 0xb8, 0x3c, 0x00, 0x00, 0x00, 0x0f, 0x05,
];

/// Like `LINUX_COW_PROBE`, but the child's overwrite is done by the KERNEL
/// (getrandom() into the shared heap page), so the ring-0 write must also
/// take the copy-on-write fault instead of corrupting the parent's copy.
pub const LINUX_COW_KERNEL_PROBE: [u8; 138] = [
    0xb8, 0x0c, 0x00, 0x00, 0x00, 0x31, 0xff, 0x0f, 0x05, 0x48, 0x89, 0xc3, 0x48, 0x8d, 0xbb, 0x00,
    0x10, 0x00, 0x00, 0xb8, 0x0c, 0x00, 0x00, 0x00, 0x0f, 0x05, 0xc7, 0x03, 0x01, 0x00, 0xaa, 0xaa,
    0xb8, 0x39, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x85, 0xc0, 0x75, 0x27, 0x48, 0x89, 0xdf, 0xbe, 0x08,
    0x00, 0x00, 0x00, 0x31, 0xd2, 0xb8, 0x3e, 0x01, 0x00, 0x00, 0x0f, 0x05, 0xbf, 0x16, 0x00, 0x00,
    0x00, 0x83, 0xf8, 0x08, 0x74, 0x05, 0xbf, 0x21, 0x00, 0x00, 0x00, 0xb8, 0x3c, 0x00, 0x00, 0x00,
    0x0f, 0x05, 0x89, 0xc7, 0x48, 0x8d, 0x74, 0x24, 0xf0, 0x31, 0xd2, 0x45, 0x31, 0xd2, 0xb8, 0x3d,
    0x00, 0x00, 0x00, 0x0f, 0x05, 0x81, 0x7c, 0x24, 0xf0, 0x00, 0x16, 0x00, 0x00, 0x75, 0x0f, 0x81,
    0x3b, 0x01, 0x00, 0xaa, 0xaa, 0x75, 0x07, 0xbf, 0x0b, 0x00, 0x00, 0x00, 0xeb, 0x05, 0xbf, 0x7d,
    0x00, 0x00, 0x00, 0xb8, 0x3c, 0x00, 0x00, 0x00, 0x0f, 0x05,
];

/// Linux-ABI probe: installs a SIGUSR1 handler (with sa_restorer), then
/// kill(getpid(), SIGUSR1); the handler stores 0x5151 in the heap page, so the
/// process exits 11 only if the handler really ran and rt_sigreturn resumed it.
pub const LINUX_SIGNAL_PROBE: [u8; 155] = [
    0xb8, 0x0c, 0x00, 0x00, 0x00, 0x31, 0xff, 0x0f, 0x05, 0x48, 0x89, 0xc3, 0x48, 0x8d, 0xbb, 0x00,
    0x10, 0x00, 0x00, 0xb8, 0x0c, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x48, 0x8d, 0x05, 0x6c, 0x00, 0x00,
    0x00, 0x48, 0x89, 0x44, 0x24, 0xc0, 0x48, 0xc7, 0x44, 0x24, 0xc8, 0x00, 0x00, 0x00, 0x04, 0x48,
    0x8d, 0x05, 0x5e, 0x00, 0x00, 0x00, 0x48, 0x89, 0x44, 0x24, 0xd0, 0x48, 0xc7, 0x44, 0x24, 0xd8,
    0x00, 0x00, 0x00, 0x00, 0xbf, 0x0a, 0x00, 0x00, 0x00, 0x48, 0x8d, 0x74, 0x24, 0xc0, 0x31, 0xd2,
    0x41, 0xba, 0x08, 0x00, 0x00, 0x00, 0xb8, 0x0d, 0x00, 0x00, 0x00, 0x0f, 0x05, 0xb8, 0x27, 0x00,
    0x00, 0x00, 0x0f, 0x05, 0x89, 0xc7, 0xbe, 0x0a, 0x00, 0x00, 0x00, 0xb8, 0x3e, 0x00, 0x00, 0x00,
    0x0f, 0x05, 0x81, 0x3b, 0x51, 0x51, 0x00, 0x00, 0x75, 0x07, 0xbf, 0x0b, 0x00, 0x00, 0x00, 0xeb,
    0x05, 0xbf, 0x7d, 0x00, 0x00, 0x00, 0xb8, 0x3c, 0x00, 0x00, 0x00, 0x0f, 0x05, 0xc7, 0x03, 0x51,
    0x51, 0x00, 0x00, 0xc3, 0xb8, 0x0f, 0x00, 0x00, 0x00, 0x0f, 0x05,
];

/// Linux-ABI probe: installs a SIGALRM handler, arms setitimer(ITIMER_REAL,
/// 20ms) and spins in user mode with no syscalls; only the timer firing and the
/// tick-return delivery can run the handler (which stores 0x5151); exits 11.
pub const LINUX_ITIMER_PROBE: [u8; 194] = [
    0xb8, 0x0c, 0x00, 0x00, 0x00, 0x31, 0xff, 0x0f, 0x05, 0x48, 0x89, 0xc3, 0x48, 0x8d, 0xbb, 0x00,
    0x10, 0x00, 0x00, 0xb8, 0x0c, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x48, 0x8d, 0x05, 0x93, 0x00, 0x00,
    0x00, 0x48, 0x89, 0x44, 0x24, 0xc0, 0x48, 0xc7, 0x44, 0x24, 0xc8, 0x00, 0x00, 0x00, 0x04, 0x48,
    0x8d, 0x05, 0x85, 0x00, 0x00, 0x00, 0x48, 0x89, 0x44, 0x24, 0xd0, 0x48, 0xc7, 0x44, 0x24, 0xd8,
    0x00, 0x00, 0x00, 0x00, 0xbf, 0x0e, 0x00, 0x00, 0x00, 0x48, 0x8d, 0x74, 0x24, 0xc0, 0x31, 0xd2,
    0x41, 0xba, 0x08, 0x00, 0x00, 0x00, 0xb8, 0x0d, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x48, 0xc7, 0x44,
    0x24, 0xa0, 0x00, 0x00, 0x00, 0x00, 0x48, 0xc7, 0x44, 0x24, 0xa8, 0x00, 0x00, 0x00, 0x00, 0x48,
    0xc7, 0x44, 0x24, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x48, 0xc7, 0x44, 0x24, 0xb8, 0x20, 0x4e, 0x00,
    0x00, 0x31, 0xff, 0x48, 0x8d, 0x74, 0x24, 0xa0, 0x31, 0xd2, 0xb8, 0x26, 0x00, 0x00, 0x00, 0x0f,
    0x05, 0x81, 0x3b, 0x51, 0x51, 0x00, 0x00, 0x75, 0xf8, 0x81, 0x3b, 0x51, 0x51, 0x00, 0x00, 0x75,
    0x07, 0xbf, 0x0b, 0x00, 0x00, 0x00, 0xeb, 0x05, 0xbf, 0x7d, 0x00, 0x00, 0x00, 0xb8, 0x3c, 0x00,
    0x00, 0x00, 0x0f, 0x05, 0xc7, 0x03, 0x51, 0x51, 0x00, 0x00, 0xc3, 0xb8, 0x0f, 0x00, 0x00, 0x00,
    0x0f, 0x05,
];

/// Linux-ABI probe: like `LINUX_ITIMER_PROBE`, but instead of spinning it
/// calls nanosleep(2s): the 20ms SIGALRM must cut the sleep short with
/// EINTR after its handler ran (stores 0x5151); exits 11.
pub const LINUX_NANOSLEEP_PROBE: [u8; 224] = [
    0xb8, 0x0c, 0x00, 0x00, 0x00, 0x31, 0xff, 0x0f, 0x05, 0x48, 0x89, 0xc3, 0x48, 0x8d, 0xbb, 0x00,
    0x10, 0x00, 0x00, 0xb8, 0x0c, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x48, 0x8d, 0x05, 0xb1, 0x00, 0x00,
    0x00, 0x48, 0x89, 0x44, 0x24, 0xc0, 0x48, 0xc7, 0x44, 0x24, 0xc8, 0x00, 0x00, 0x00, 0x04, 0x48,
    0x8d, 0x05, 0xa3, 0x00, 0x00, 0x00, 0x48, 0x89, 0x44, 0x24, 0xd0, 0x48, 0xc7, 0x44, 0x24, 0xd8,
    0x00, 0x00, 0x00, 0x00, 0xbf, 0x0e, 0x00, 0x00, 0x00, 0x48, 0x8d, 0x74, 0x24, 0xc0, 0x31, 0xd2,
    0x41, 0xba, 0x08, 0x00, 0x00, 0x00, 0xb8, 0x0d, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x48, 0xc7, 0x44,
    0x24, 0xa0, 0x00, 0x00, 0x00, 0x00, 0x48, 0xc7, 0x44, 0x24, 0xa8, 0x00, 0x00, 0x00, 0x00, 0x48,
    0xc7, 0x44, 0x24, 0xb0, 0x00, 0x00, 0x00, 0x00, 0x48, 0xc7, 0x44, 0x24, 0xb8, 0x20, 0x4e, 0x00,
    0x00, 0x31, 0xff, 0x48, 0x8d, 0x74, 0x24, 0xa0, 0x31, 0xd2, 0xb8, 0x26, 0x00, 0x00, 0x00, 0x0f,
    0x05, 0x48, 0xc7, 0x44, 0x24, 0x80, 0x02, 0x00, 0x00, 0x00, 0x48, 0xc7, 0x44, 0x24, 0x88, 0x00,
    0x00, 0x00, 0x00, 0x48, 0x8d, 0x7c, 0x24, 0x80, 0x31, 0xf6, 0xb8, 0x23, 0x00, 0x00, 0x00, 0x0f,
    0x05, 0x48, 0x83, 0xf8, 0xfc, 0x75, 0x0f, 0x81, 0x3b, 0x51, 0x51, 0x00, 0x00, 0x75, 0x07, 0xbf,
    0x0b, 0x00, 0x00, 0x00, 0xeb, 0x05, 0xbf, 0x7d, 0x00, 0x00, 0x00, 0xb8, 0x3c, 0x00, 0x00, 0x00,
    0x0f, 0x05, 0xc7, 0x03, 0x51, 0x51, 0x00, 0x00, 0xc3, 0xb8, 0x0f, 0x00, 0x00, 0x00, 0x0f, 0x05,
];

/// Like `LINUX_SIGNAL_PROBE`, but SIGUSR1 is blocked first: the handler must
/// NOT run while blocked, and must run when the signal is unblocked.
pub const LINUX_SIGNAL_MASK_PROBE: [u8; 216] = [
    0xb8, 0x0c, 0x00, 0x00, 0x00, 0x31, 0xff, 0x0f, 0x05, 0x48, 0x89, 0xc3, 0x48, 0x8d, 0xbb, 0x00,
    0x10, 0x00, 0x00, 0xb8, 0x0c, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x48, 0x8d, 0x05, 0xa9, 0x00, 0x00,
    0x00, 0x48, 0x89, 0x44, 0x24, 0xc0, 0x48, 0xc7, 0x44, 0x24, 0xc8, 0x00, 0x00, 0x00, 0x04, 0x48,
    0x8d, 0x05, 0x9b, 0x00, 0x00, 0x00, 0x48, 0x89, 0x44, 0x24, 0xd0, 0x48, 0xc7, 0x44, 0x24, 0xd8,
    0x00, 0x00, 0x00, 0x00, 0xbf, 0x0a, 0x00, 0x00, 0x00, 0x48, 0x8d, 0x74, 0x24, 0xc0, 0x31, 0xd2,
    0x41, 0xba, 0x08, 0x00, 0x00, 0x00, 0xb8, 0x0d, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x48, 0xc7, 0x44,
    0x24, 0xe0, 0x00, 0x02, 0x00, 0x00, 0x31, 0xff, 0x48, 0x8d, 0x74, 0x24, 0xe0, 0x31, 0xd2, 0x41,
    0xba, 0x08, 0x00, 0x00, 0x00, 0xb8, 0x0e, 0x00, 0x00, 0x00, 0x0f, 0x05, 0xb8, 0x27, 0x00, 0x00,
    0x00, 0x0f, 0x05, 0x89, 0xc7, 0xbe, 0x0a, 0x00, 0x00, 0x00, 0xb8, 0x3e, 0x00, 0x00, 0x00, 0x0f,
    0x05, 0x83, 0x3b, 0x00, 0x75, 0x28, 0xbf, 0x01, 0x00, 0x00, 0x00, 0x48, 0x8d, 0x74, 0x24, 0xe0,
    0x31, 0xd2, 0x41, 0xba, 0x08, 0x00, 0x00, 0x00, 0xb8, 0x0e, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x81,
    0x3b, 0x51, 0x51, 0x00, 0x00, 0x75, 0x07, 0xbf, 0x0b, 0x00, 0x00, 0x00, 0xeb, 0x05, 0xbf, 0x7d,
    0x00, 0x00, 0x00, 0xb8, 0x3c, 0x00, 0x00, 0x00, 0x0f, 0x05, 0xc7, 0x03, 0x51, 0x51, 0x00, 0x00,
    0xc3, 0xb8, 0x0f, 0x00, 0x00, 0x00, 0x0f, 0x05,
];

/// Linux-ABI probe: the child installs a SIGUSR1 handler and spins in
/// sched_yield until the handler stored 0x5151; the parent yields, then
/// kill(child, SIGUSR1) and wait4: the child must exit 22, not die of the signal.
pub const LINUX_SIGNAL_CHILD_PROBE: [u8; 227] = [
    0xb8, 0x0c, 0x00, 0x00, 0x00, 0x31, 0xff, 0x0f, 0x05, 0x48, 0x89, 0xc3, 0x48, 0x8d, 0xbb, 0x00,
    0x10, 0x00, 0x00, 0xb8, 0x0c, 0x00, 0x00, 0x00, 0x0f, 0x05, 0xb8, 0x39, 0x00, 0x00, 0x00, 0x0f,
    0x05, 0x85, 0xc0, 0x75, 0x5e, 0x48, 0x8d, 0x05, 0xa9, 0x00, 0x00, 0x00, 0x48, 0x89, 0x44, 0x24,
    0xc0, 0x48, 0xc7, 0x44, 0x24, 0xc8, 0x00, 0x00, 0x00, 0x04, 0x48, 0x8d, 0x05, 0x9b, 0x00, 0x00,
    0x00, 0x48, 0x89, 0x44, 0x24, 0xd0, 0x48, 0xc7, 0x44, 0x24, 0xd8, 0x00, 0x00, 0x00, 0x00, 0xbf,
    0x0a, 0x00, 0x00, 0x00, 0x48, 0x8d, 0x74, 0x24, 0xc0, 0x31, 0xd2, 0x41, 0xba, 0x08, 0x00, 0x00,
    0x00, 0xb8, 0x0d, 0x00, 0x00, 0x00, 0x0f, 0x05, 0xb8, 0x18, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x81,
    0x3b, 0x51, 0x51, 0x00, 0x00, 0x75, 0xf1, 0xb8, 0x3c, 0x00, 0x00, 0x00, 0xbf, 0x16, 0x00, 0x00,
    0x00, 0x0f, 0x05, 0x89, 0xc5, 0xb9, 0x20, 0x00, 0x00, 0x00, 0x51, 0xb8, 0x18, 0x00, 0x00, 0x00,
    0x0f, 0x05, 0x59, 0xff, 0xc9, 0x75, 0xf3, 0x89, 0xef, 0xbe, 0x0a, 0x00, 0x00, 0x00, 0xb8, 0x3e,
    0x00, 0x00, 0x00, 0x0f, 0x05, 0x89, 0xef, 0x48, 0x8d, 0x74, 0x24, 0xf0, 0x31, 0xd2, 0x45, 0x31,
    0xd2, 0xb8, 0x3d, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x81, 0x7c, 0x24, 0xf0, 0x00, 0x16, 0x00, 0x00,
    0x75, 0x07, 0xbf, 0x0b, 0x00, 0x00, 0x00, 0xeb, 0x05, 0xbf, 0x7d, 0x00, 0x00, 0x00, 0xb8, 0x3c,
    0x00, 0x00, 0x00, 0x0f, 0x05, 0xc7, 0x03, 0x51, 0x51, 0x00, 0x00, 0xc3, 0xb8, 0x0f, 0x00, 0x00,
    0x00, 0x0f, 0x05,
];

/// Like `LINUX_SIGNAL_CHILD_PROBE`, but the child spins in USER mode (no
/// syscalls at all) with sentinels in rcx/r11 until its handler stored 0x5151:
/// the handler can only run via timer-return delivery, and the child exits 22
/// only if `rt_sigreturn` restored rcx, r11 and the flags exactly (else 99).
pub const LINUX_SIGNAL_SPIN_PROBE: [u8; 259] = [
    0xb8, 0x0c, 0x00, 0x00, 0x00, 0x31, 0xff, 0x0f, 0x05, 0x48, 0x89, 0xc3, 0x48, 0x8d, 0xbb, 0x00,
    0x10, 0x00, 0x00, 0xb8, 0x0c, 0x00, 0x00, 0x00, 0x0f, 0x05, 0xb8, 0x39, 0x00, 0x00, 0x00, 0x0f,
    0x05, 0x85, 0xc0, 0x75, 0x7e, 0x48, 0x8d, 0x05, 0xc9, 0x00, 0x00, 0x00, 0x48, 0x89, 0x44, 0x24,
    0xc0, 0x48, 0xc7, 0x44, 0x24, 0xc8, 0x00, 0x00, 0x00, 0x04, 0x48, 0x8d, 0x05, 0xbb, 0x00, 0x00,
    0x00, 0x48, 0x89, 0x44, 0x24, 0xd0, 0x48, 0xc7, 0x44, 0x24, 0xd8, 0x00, 0x00, 0x00, 0x00, 0xbf,
    0x0a, 0x00, 0x00, 0x00, 0x48, 0x8d, 0x74, 0x24, 0xc0, 0x31, 0xd2, 0x41, 0xba, 0x08, 0x00, 0x00,
    0x00, 0xb8, 0x0d, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x48, 0xc7, 0xc1, 0x44, 0x33, 0x22, 0x11, 0x49,
    0xc7, 0xc3, 0x88, 0x77, 0x66, 0x05, 0x81, 0x3b, 0x51, 0x51, 0x00, 0x00, 0x75, 0xf8, 0x48, 0x81,
    0xf9, 0x44, 0x33, 0x22, 0x11, 0x75, 0x10, 0x49, 0x81, 0xfb, 0x88, 0x77, 0x66, 0x05, 0x75, 0x07,
    0xbf, 0x16, 0x00, 0x00, 0x00, 0xeb, 0x05, 0xbf, 0x63, 0x00, 0x00, 0x00, 0xb8, 0x3c, 0x00, 0x00,
    0x00, 0x0f, 0x05, 0x89, 0xc5, 0xb9, 0x20, 0x00, 0x00, 0x00, 0x51, 0xb8, 0x18, 0x00, 0x00, 0x00,
    0x0f, 0x05, 0x59, 0xff, 0xc9, 0x75, 0xf3, 0x89, 0xef, 0xbe, 0x0a, 0x00, 0x00, 0x00, 0xb8, 0x3e,
    0x00, 0x00, 0x00, 0x0f, 0x05, 0x89, 0xef, 0x48, 0x8d, 0x74, 0x24, 0xf0, 0x31, 0xd2, 0x45, 0x31,
    0xd2, 0xb8, 0x3d, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x81, 0x7c, 0x24, 0xf0, 0x00, 0x16, 0x00, 0x00,
    0x75, 0x07, 0xbf, 0x0b, 0x00, 0x00, 0x00, 0xeb, 0x05, 0xbf, 0x7d, 0x00, 0x00, 0x00, 0xb8, 0x3c,
    0x00, 0x00, 0x00, 0x0f, 0x05, 0xc7, 0x03, 0x51, 0x51, 0x00, 0x00, 0xc3, 0xb8, 0x0f, 0x00, 0x00,
    0x00, 0x0f, 0x05,
];

/// Linux-ABI probe: pipe(); the forked child yields once and writes one byte;
/// the parent's blocking read() has to wait (yielding inside the syscall) for it,
/// then wait4()s the child. Exits 11 on success.
pub const LINUX_PIPE_FORK_PROBE: [u8; 149] = [
    0x48, 0x8d, 0x7c, 0x24, 0xf0, 0xb8, 0x16, 0x00, 0x00, 0x00, 0x0f, 0x05, 0xb8, 0x39, 0x00, 0x00,
    0x00, 0x0f, 0x05, 0x85, 0xc0, 0x75, 0x2d, 0xb8, 0x18, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x8b, 0x7c,
    0x24, 0xf4, 0xc6, 0x44, 0x24, 0xe0, 0x58, 0x48, 0x8d, 0x74, 0x24, 0xe0, 0xba, 0x01, 0x00, 0x00,
    0x00, 0xb8, 0x01, 0x00, 0x00, 0x00, 0x0f, 0x05, 0xb8, 0x3c, 0x00, 0x00, 0x00, 0xbf, 0x16, 0x00,
    0x00, 0x00, 0x0f, 0x05, 0x89, 0xc5, 0x8b, 0x7c, 0x24, 0xf0, 0x48, 0x8d, 0x74, 0x24, 0xd0, 0xba,
    0x01, 0x00, 0x00, 0x00, 0x31, 0xc0, 0x0f, 0x05, 0x48, 0x83, 0xf8, 0x01, 0x75, 0x2b, 0x80, 0x7c,
    0x24, 0xd0, 0x58, 0x75, 0x24, 0x89, 0xef, 0x48, 0x8d, 0x74, 0x24, 0xc0, 0x31, 0xd2, 0x45, 0x31,
    0xd2, 0xb8, 0x3d, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x81, 0x7c, 0x24, 0xc0, 0x00, 0x16, 0x00, 0x00,
    0x75, 0x07, 0xbf, 0x0b, 0x00, 0x00, 0x00, 0xeb, 0x05, 0xbf, 0x7d, 0x00, 0x00, 0x00, 0xb8, 0x3c,
    0x00, 0x00, 0x00, 0x0f, 0x05,
];

/// Linux-ABI probe: wait4(WNOHANG) on a running child must return 0; after
/// kill(child, SIGKILL) a blocking wait4 must return its pid with status 9 (killed by SIGKILL).
pub const LINUX_WNOHANG_PROBE: [u8; 115] = [
    0xb8, 0x39, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x85, 0xc0, 0x75, 0x09, 0xb8, 0x18, 0x00, 0x00, 0x00,
    0x0f, 0x05, 0xeb, 0xf7, 0x89, 0xc5, 0x89, 0xef, 0x48, 0x8d, 0x74, 0x24, 0xf0, 0xba, 0x01, 0x00,
    0x00, 0x00, 0x45, 0x31, 0xd2, 0xb8, 0x3d, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x48, 0x85, 0xc0, 0x75,
    0x36, 0x89, 0xef, 0xbe, 0x09, 0x00, 0x00, 0x00, 0xb8, 0x3e, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x89,
    0xef, 0x48, 0x8d, 0x74, 0x24, 0xf0, 0x31, 0xd2, 0x45, 0x31, 0xd2, 0xb8, 0x3d, 0x00, 0x00, 0x00,
    0x0f, 0x05, 0x39, 0xe8, 0x75, 0x11, 0x81, 0x7c, 0x24, 0xf0, 0x09, 0x00, 0x00, 0x00, 0x75, 0x07,
    0xbf, 0x0b, 0x00, 0x00, 0x00, 0xeb, 0x05, 0xbf, 0x7d, 0x00, 0x00, 0x00, 0xb8, 0x3c, 0x00, 0x00,
    0x00, 0x0f, 0x05,
];

/// Linux-ABI probe: execve("/bin/aeros-init", ["init", "hello"], NULL).
pub const LINUX_EXECVE_ARGS_PROBE: [u8; 93] = [
    0x48, 0x8d, 0x05, 0x4b, 0x00, 0x00, 0x00, 0x48, 0x89, 0x44, 0x24, 0xe0, 0x48, 0x8d, 0x05, 0x44,
    0x00, 0x00, 0x00, 0x48, 0x89, 0x44, 0x24, 0xe8, 0x48, 0xc7, 0x44, 0x24, 0xf0, 0x00, 0x00, 0x00,
    0x00, 0x48, 0x8d, 0x74, 0x24, 0xe0, 0x48, 0x8d, 0x3d, 0x15, 0x00, 0x00, 0x00, 0x31, 0xd2, 0xb8,
    0x3b, 0x00, 0x00, 0x00, 0x0f, 0x05, 0xbf, 0x63, 0x00, 0x00, 0x00, 0xb8, 0x3c, 0x00, 0x00, 0x00,
    0x0f, 0x05, 0x2f, 0x62, 0x69, 0x6e, 0x2f, 0x61, 0x65, 0x72, 0x6f, 0x73, 0x2d, 0x69, 0x6e, 0x69,
    0x74, 0x00, 0x69, 0x6e, 0x69, 0x74, 0x00, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x00,
];

#[derive(Clone, Copy)]
pub struct ExecArgsSelfTestResult {
    pub exit_code: u64,
    pub argc: u64,
    pub arg1_matches: bool,
    pub reaped: usize,
    pub verified: bool,
}

/// Runs `LINUX_EXECVE_ARGS_PROBE` (aeros-init exits 121 when argc != 1, which
/// proves the new image saw the two-entry argv) and reads the new image's argc/argv back
/// off its stack before it is reaped.
pub fn linux_execve_args_self_test(
    state: &crate::arch::paging::PagingState,
    code: &[u8],
) -> ExecArgsSelfTestResult {
    let failed = ExecArgsSelfTestResult {
        exit_code: u64::MAX,
        argc: 0,
        arg1_matches: false,
        reaped: 0,
        verified: false,
    };
    initialize();
    for slot in USER_TASK_EXIT.iter() {
        slot.store(u64::MAX, Ordering::Release);
    }
    let Some((_id, slot, space)) = spawn_process(state, code) else {
        return failed;
    };
    if !space.verified {
        return failed;
    }
    PREEMPTION_ENABLED.store(1, Ordering::Release);
    if !arch::apic::start_timer(1000) {
        PREEMPTION_ENABLED.store(0, Ordering::Release);
        return failed;
    }
    let mut waited = 0u32;
    let exit_code = loop {
        let value = USER_TASK_EXIT[slot].load(Ordering::Acquire);
        if value != u64::MAX {
            break value;
        }
        waited += 1;
        if waited > 20_000 {
            break u64::MAX;
        }
        unsafe {
            asm!("sti; hlt", options(nomem, nostack));
        }
    };
    arch::apic::stop_timer();
    PREEMPTION_ENABLED.store(0, Ordering::Release);
    let live_space = unsafe { (*SCHEDULER.0.get()).tasks[slot].process_space };
    let base = arch::paging::process_stack_base();
    let (argc, arg1_matches) = live_space
        .filter(|space| space.stack_top >= base)
        .map(|space| {
            let top = space.stack_physical + (space.stack_top - base);
            // SAFETY: the stack page is still owned by this task until reap().
            let read = |offset: u64| unsafe {
                core::ptr::read_volatile((top + offset) as usize as *const u64)
            };
            let argc = read(0);
            let arg1 = read(16);
            let matches = arg1 >= base && {
                let physical = space.stack_physical + (arg1 - base);
                let mut text = [0u8; 6];
                for (index, byte) in text.iter_mut().enumerate() {
                    *byte = unsafe {
                        core::ptr::read_volatile((physical + index as u64) as usize as *const u8)
                    };
                }
                &text == b"hello\0"
            };
            (argc, matches)
        })
        .unwrap_or((0, false));
    let reaped = reap();
    ExecArgsSelfTestResult {
        exit_code,
        argc,
        arg1_matches,
        reaped,
        verified: exit_code == 121 && argc == 2 && arg1_matches && reaped == 1,
    }
}

extern "C" fn self_test_task_a() -> ! {
    TASK_A_PHASE.store(1, Ordering::Release);
    let expected = [0x1122_3344_5566_7788u64, 0x99aa_bbcc_ddee_ff00];
    write_xmm15(&expected);
    yield_now();
    let observed = read_xmm15();
    FPU_A_VALID.store((observed == expected) as u32, Ordering::Release);
    TASK_A_PHASE.store(2, Ordering::Release);
    exit_current()
}

extern "C" fn self_test_task_b() -> ! {
    TASK_B_PHASE.store(1, Ordering::Release);
    let expected = [0x0f1e_2d3c_4b5a_6978u64, 0x8796_a5b4_c3d2_e1f0];
    write_xmm15(&expected);
    yield_now();
    let observed = read_xmm15();
    FPU_B_VALID.store((observed == expected) as u32, Ordering::Release);
    TASK_B_PHASE.store(2, Ordering::Release);
    exit_current()
}

extern "C" fn task_return_guard() -> ! {
    exit_current()
}

extern "C" fn preempt_task_a() -> ! {
    PREEMPT_A_PHASE.store(1, Ordering::Release);
    let expected = [
        0x0123_4567_89ab_cdefu64,
        0xfedc_ba98_7654_3210,
        0x1357_9bdf_2468_ace0,
        0x0eca_8642_fdb9_7531,
    ];
    write_vector14(&expected);
    write_x87_control(0x077f);
    while PREEMPT_TICKS.load(Ordering::Acquire) < 8 {
        PREEMPT_A_WORK.fetch_add(1, Ordering::Relaxed);
        core::hint::spin_loop();
    }
    let vector_valid = vector14_matches(read_vector14(), expected);
    let control_valid = read_x87_control() == 0x077f;
    PREEMPT_FPU_A_VALID.store((vector_valid && control_valid) as u32, Ordering::Release);
    PREEMPT_A_PHASE.store(2, Ordering::Release);
    exit_current()
}

extern "C" fn preempt_task_b() -> ! {
    PREEMPT_B_PHASE.store(1, Ordering::Release);
    let expected = [
        0x55aa_55aa_aa55_aa55u64,
        0xa55a_a55a_5aa5_5aa5,
        0x1122_3344_7788_99aa,
        0xbbcc_ddee_1020_3040,
    ];
    write_vector14(&expected);
    write_x87_control(0x0b7f);
    while PREEMPT_TICKS.load(Ordering::Acquire) < 8 {
        PREEMPT_B_WORK.fetch_add(1, Ordering::Relaxed);
        core::hint::spin_loop();
    }
    let vector_valid = vector14_matches(read_vector14(), expected);
    let control_valid = read_x87_control() == 0x0b7f;
    PREEMPT_FPU_B_VALID.store((vector_valid && control_valid) as u32, Ordering::Release);
    PREEMPT_B_PHASE.store(2, Ordering::Release);
    exit_current()
}

fn write_xmm15(value: &[u64; 2]) {
    unsafe {
        asm!("movdqu xmm15, [{}]", in(reg) value.as_ptr(), out("xmm15") _, options(nostack));
    }
}

fn read_xmm15() -> [u64; 2] {
    let mut value = [0u64; 2];
    unsafe {
        asm!("movdqu [{}], xmm15", in(reg) value.as_mut_ptr(), options(nostack));
    }
    value
}

fn write_vector14(value: &[u64; 4]) {
    unsafe {
        if arch::fpu::avx_context_enabled() {
            asm!("vmovdqu ymm14, [{}]", in(reg) value.as_ptr(), out("ymm14") _, options(nostack));
        } else {
            asm!("movdqu xmm14, [{}]", in(reg) value.as_ptr(), out("xmm14") _, options(nostack));
        }
    }
}

fn read_vector14() -> [u64; 4] {
    let mut value = [0u64; 4];
    unsafe {
        if arch::fpu::avx_context_enabled() {
            asm!("vmovdqu [{}], ymm14", in(reg) value.as_mut_ptr(), options(nostack));
        } else {
            asm!("movdqu [{}], xmm14", in(reg) value.as_mut_ptr(), options(nostack));
        }
    }
    value
}

fn vector14_matches(observed: [u64; 4], expected: [u64; 4]) -> bool {
    observed[0] == expected[0]
        && observed[1] == expected[1]
        && (!arch::fpu::avx_context_enabled()
            || observed[2] == expected[2] && observed[3] == expected[3])
}

fn write_x87_control(value: u16) {
    unsafe {
        asm!("fldcw [{}]", in(reg) &value, options(nostack));
    }
}

fn read_x87_control() -> u16 {
    let mut value = 0u16;
    unsafe {
        asm!("fnstcw [{}]", in(reg) &mut value, options(nostack));
    }
    value
}

#[unsafe(naked)]
unsafe extern "C" fn task_entry_trampoline() -> ! {
    naked_asm!(
        "sti",
        "sub rsp, 40",
        "call r12",
        "add rsp, 40",
        "call {guard}",
        "ud2",
        guard = sym task_return_guard,
    )
}

#[unsafe(naked)]
unsafe extern "C" fn switch_context(old_rsp: *mut u64, new_rsp: u64) {
    naked_asm!(
        "push rbp",
        "push rbx",
        "push rsi",
        "push rdi",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov [rcx], rsp",
        "mov rsp, rdx",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rdi",
        "pop rsi",
        "pop rbx",
        "pop rbp",
        "ret",
    )
}
