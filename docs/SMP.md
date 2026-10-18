# User programs on several processors

AerOS runs user programs on every processor. The kernel itself is not yet parallel: it runs under one big lock, the way Linux 2.0 did, so user mode runs in parallel and kernel mode takes turns.

## How it works

- **The big kernel lock** (`kernel_lock.rs`) is a ticket lock. A processor takes it whenever it enters the kernel from user mode (a `syscall`, an interrupt or a fault, wrapped in `aeros_linux_syscall_dispatch` and `aeros_interrupt_dispatch`) and gives it up when it returns to user mode. The assembly that starts a task in user mode (`enter_user`, `enter_user_from_snapshot`) releases it itself, right after it has stored the task's return point, so nothing else can use that slot in between.
- **The live task.** The kernel keeps the Linux process state of one task (descriptor table, signal handlers, directory, limits) in statics, and the scheduler's `current` names the task that state belongs to. Whoever holds the lock is that task, so a processor that enters the kernel first makes the task it was running the live one (`Scheduler::rebind`, which does what a context switch does for those statics without switching stacks). The cost is a state copy of about half a microsecond.
- **Waiting gives the lock up.** A task that waits in the kernel (`yield_in_syscall`, `wait4`, a blocked pipe or futex) does so by yielding. With nothing else to switch to on its processor it lets the other processors into the kernel for a moment (`relax_kernel`) and takes its place back, so a waiter never keeps out the task it waits for. The bootstrap processor's idle loop does the same around `hlt`.
- **Dispatch.** An application processor with nothing to do looks for a ready task in its idle loop (`idle_step`), switches to it, and comes back to the idle loop when the last task it runs ends. New tasks wake sleeping processors with the work interrupt, and each application processor runs its own 250 Hz tick that preempts the task it caught in user mode. Per-processor stacks, task state segments and `syscall` stacks were already in place; the scheduler now sets them for the processor it works on.
- **Who runs where.** User tasks run on the application processors; the bootstrap processor keeps the desktop going. (With the `linux-guest` feature, where an application processor may be busy with the guest, the bootstrap processor runs tasks too.) With one processor everything is as before.
- **Kills and signals** for a task that is running on another processor mark it (`kill_pending`) or store the signal and send that processor the work interrupt, so it enters the kernel and acts on it.

## The rules that keep it correct

- The threads of one process run on several processors at once (different processes always did). They share one address space, so a change that removes or downgrades a mapping - `munmap`, `mprotect`, a copy-on-write break, `fork` (which makes the parent's pages read-only), `exec`, a swap-out - first calls `stop_siblings`: every other processor that has the address space loaded and is running user code is sent the work interrupt and held in the kernel on the kernel lock until the change is made, and every processor that has it loaded reloads it before it next leaves the kernel (`FLUSH_PENDING`). Nobody can use a stale translation or a freed frame. `IN_USER` is cleared before a processor waits for the lock, so the waiting initiator never waits for a processor that cannot answer.
- `Scheduler::current` is never a slot that may be freed: a task that ends with nothing to run hands its live state to the boot task's slot before its processor goes idle.
- Interrupts that catch a processor in kernel mode do not take the lock and do not preempt (only the tick that caught user mode does).

## Limits

- Kernel mode is serialised. A system-call-heavy workload gains nothing from more processors, and the desktop's own drawing holds the lock between its `relax_kernel` points, so a program's system calls wait for them.
- There is no processor-affinity interface, and the address-space changes above stop the other threads of the process for their duration (no per-page shootdown).
- A task blocked on a pipe or a socket still polls (the primitives were never wait queues; futexes are), so a blocked task keeps its processor busy.
- Tested with two to four processors in QEMU only.

## Tests

`AEROS_SMP_USER` runs two compute-bound processes (they must end on different processors with their run times overlapping by at least 40 per cent) and two processes that make 25,000 system calls each (every call enters the kernel under the lock). `AEROS_SMP_PROGRAMS` runs the real programs (`aeros-init`, the std smoke test, the threads test, the ptrace test, a pipe-and-fork probe, the benchmark) the way the shell runs them, with the application processors only and the bootstrap processor asleep between ticks, and each must end with its one-processor status. `lscpu` shows the scheduler's state and the lock's counters.

## Rules found by running threads in parallel

- A task that ends because `exit_group` (or an execve from a threaded process) asked it to is told with `kill_pending` and a work interrupt; the waiting `execve` lets the kernel lock go until they have ended. Such a task's status (137) must not replace the status `exit_group` recorded.
- An interrupt that catches a task in user mode and then finds it killed returns before its handler runs, so it must still acknowledge the interrupt (EOI), or that vector, and every timer below it, stays blocked on that processor.
- A task that yields while another processor queues for the kernel lock lets that processor in first (`has_waiters`); otherwise the boot task and a sleeping thread can pass the processor between them forever.

`AEROS_SMP_THREADS` runs `userspace/parallel` with the boot processor taking tasks too: two threads must be seen on different processors at the same moment, a thread that keeps writing a page while the process forks must not change the child's copy, a page that is unmapped and mapped again at the same address must be seen new at once by a thread that had read the old one, and the process must be able to exit while two of its threads still spin on other processors.

## Rules found by running a debugger across processors

- A task that waits in a ptrace stop keeps its processor, so its floating-point registers stay in the hardware: it saves them once on entering the stop and reloads them, and flushes its TLB, when it resumes (the debugger may have changed its registers, and may have given it a private page through the kernel's own mapping).
- A kill aimed at a task that is already waiting in the kernel is seen by the wait loop (`take_kill_pending`) and, for every other task, just before it returns to user mode.
