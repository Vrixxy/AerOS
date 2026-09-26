#![no_std]
#![no_main]

// A real, toolchain-compiled (not hand-assembled) multi-segment ELF binary
// that calls AerOS's bootstrap-ABI fork()/wait4() (`int 0x80`, the only
// fork() this kernel has - there is no real Linux `syscall`-instruction
// fork/clone wired up). Proves `create_process_from_elf` + `fork_process`
// genuinely work together for a real compiled program, not just the hand-
// assembled single-page probes every other fork/wait4 self-test uses.
//
// child: exits(22) immediately.
// parent: wait4()s the child (forwarding its real exit status), writes a
// marker onto ITS OWN (fork-copied) stack to prove it kept running
// correctly post-fork, then exits(11) only if the child's forwarded
// status was genuinely 22 - anything else exits(125).

use core::arch::naked_asm;
use core::panic::PanicInfo;

// Lives in a writable PT_LOAD segment: fork() must give the child a private
// copy, so the child's write below must never be visible to the parent.
#[unsafe(no_mangle)]
static mut SHARED_PROBE: u32 = 5;

#[unsafe(no_mangle)]
#[unsafe(naked)]
unsafe extern "C" fn _start() -> ! {
    naked_asm!(
        "mov eax, 2", // SYS_FORK
        "int 0x80",
        "test eax, eax",
        "jnz 2f",
        // child path (eax == 0):
        "mov dword ptr [rip + {probe}], 99",
        "mov edi, 22",
        "xor eax, eax", // SYS_EXIT
        "int 0x80",
        "2:",
        // parent path: eax still holds the child's task id from fork().
        "mov edi, eax", // wait4 arg0 = child id
        "mov eax, 4",   // SYS_WAIT4
        "int 0x80",     // eax = child's real exit status
        "cmp eax, 22",
        "jne 3f",
        "cmp dword ptr [rip + {probe}], 5",
        "jne 3f",
        "mov dword ptr [rsp - 8], 0xaaaa0001",
        "mov edi, 11",
        "jmp 4f",
        "3:",
        "mov edi, 125",
        "4:",
        "xor eax, eax", // SYS_EXIT
        "int 0x80",
        probe = sym SHARED_PROBE,
    );
}

#[panic_handler]
fn panic(_: &PanicInfo<'_>) -> ! {
    loop {}
}
