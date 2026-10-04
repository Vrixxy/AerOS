"""Regenerates kernel/src/syscall_fuzz_probe.rs: a small x86-64 user program
that issues random Linux syscalls with arguments drawn from small integers,
-1, 0, raw random words and pointers into its own 64 KiB buffer (which starts
with a few path strings). It skips syscalls that end or signal tasks, block, or
change its own memory map and limits, so a clean run exits with status 0 and
any kernel panic or hang shows up in the boot-test.
"""

import os
import re
import struct

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SYSCALL_RS = os.path.join(ROOT, "kernel", "src", "syscall.rs")
OUT = os.path.join(ROOT, "kernel", "src", "syscall_fuzz_probe.rs")

ROUNDS = int(os.environ.get("FUZZ_ROUNDS", "40000"))
SEED = int(os.environ.get("FUZZ_SEED", "0x9E3779B97F4A7C15"), 0)

# exit, exit_group, fork family, execve family, wait*, kill*, pause, sleeps,
# signal waiting, polling with timeouts, accept/connect/recv (can block),
# futex, prctl/seccomp/capset/rlimits/chdir/umask/priority, reboot, memory map
# calls (the buffer must stay mapped), alarm/setitimer, write/writev to the
# console (keeps the serial log readable).
SKIPPED = {
    1, 7, 9, 10, 11, 12, 15, 20, 23, 25, 34, 35, 37, 38, 42, 43, 45, 47, 56, 57, 58, 59, 60, 61,
    62, 80, 81, 95, 101, 126, 128, 130, 141, 157, 160, 164, 165, 166, 169, 200, 202, 227, 230, 231,
    232, 234, 247, 270, 271, 281, 288, 302, 317, 322, 435, 441,
}

STRINGS = [
    b"/tmp/fz\0",
    b"/proc/self/status\0",
    b"/sys/devices/system/cpu/online\0",
    b"/proc/meminfo\0",
]


def implemented():
    text = open(SYSCALL_RS, encoding="utf-8").read()
    numbers = {int(number) for _, number in re.findall(r"const LINUX_([A-Z0-9_]+): u64 = (\d+);", text)}
    return sorted(number for number in numbers if number not in SKIPPED and number < 450)


def build():
    code = bytearray()
    fixups = []

    def emit(*parts):
        for part in parts:
            code.extend(part if isinstance(part, (bytes, bytearray)) else bytes([part]))

    def lea(reg_modrm, label):
        # lea reg, [rip + label]; REX.W 8D modrm disp32
        emit(0x48, 0x8D, reg_modrm)
        fixups.append((len(code), label))
        emit(b"\0\0\0\0")

    # base = mmap(0, 0x10000, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0)
    emit(0xB8, struct.pack("<I", 9))
    emit(0x31, 0xFF)
    emit(0xBE, struct.pack("<I", 0x10000))
    emit(0xBA, struct.pack("<I", 3))
    emit(0x41, 0xBA, struct.pack("<I", 0x22))
    emit(0x49, 0xC7, 0xC0, b"\xff\xff\xff\xff")
    emit(0x45, 0x31, 0xC9)
    emit(0x0F, 0x05)
    emit(0x49, 0x89, 0xC7)                        # mov r15, rax
    emit(0x4D, 0x8D, 0xA7, struct.pack("<I", 0xF000))  # lea r12, [r15 + 0xf000]
    emit(0x49, 0xBE, struct.pack("<Q", SEED))     # mov r14, seed
    emit(0x41, 0xBD, struct.pack("<I", ROUNDS))   # mov r13d, rounds

    loop_start = len(code)
    lea(0x35, "strings")                          # lea rsi, [rip + strings]
    emit(0x4C, 0x89, 0xFF)                        # mov rdi, r15
    emit(0xB9, struct.pack("<I", 256))            # mov ecx, 256
    emit(0xF3, 0xA4)                              # rep movsb

    # xorshift64 on r14
    emit(0x4C, 0x89, 0xF0, 0x48, 0xC1, 0xE0, 13, 0x49, 0x31, 0xC6)
    emit(0x4C, 0x89, 0xF0, 0x48, 0xC1, 0xE8, 7, 0x49, 0x31, 0xC6)
    emit(0x4C, 0x89, 0xF0, 0x48, 0xC1, 0xE0, 17, 0x49, 0x31, 0xC6)

    # candidate values at [r12 + 8 * n]
    emit(0x4C, 0x89, 0xF0, 0x48, 0xC1, 0xE8, 8, 0x83, 0xE0, 0x3F, 0x49, 0x89, 0x04, 0x24)          # 0: small
    emit(0x4C, 0x89, 0xF0, 0x48, 0xC1, 0xE8, 8, 0x25, struct.pack("<I", 0x1FF),
         0x48, 0xC1, 0xE0, 3, 0x4C, 0x01, 0xF8, 0x49, 0x89, 0x44, 0x24, 8)                           # 1: pointer
    emit(0x4D, 0x89, 0x7C, 0x24, 16)                                                                # 2: base
    emit(0x4D, 0x89, 0x74, 0x24, 24)                                                                # 3: raw
    emit(0x49, 0xC7, 0x44, 0x24, 32, b"\xff\xff\xff\xff")                                           # 4: -1
    emit(0x49, 0xC7, 0x44, 0x24, 40, b"\0\0\0\0")                                                   # 5: 0
    emit(0x4C, 0x89, 0xF0, 0x48, 0xC1, 0xE8, 20, 0x25, struct.pack("<I", 0xFFF),
         0x49, 0x89, 0x44, 0x24, 48)                                                                # 6: small
    emit(0x4C, 0x89, 0xF0, 0x48, 0xC1, 0xE8, 30, 0x83, 0xE0, 0x03,
         0x48, 0xC1, 0xE0, 6, 0x4C, 0x01, 0xF8, 0x49, 0x89, 0x44, 0x24, 56)                          # 7: string

    # arguments: pick one candidate per register from three bits of r14
    for index, (rex, modrm) in enumerate(
        [(0x49, 0x3C), (0x49, 0x34), (0x49, 0x14), (0x4D, 0x14), (0x4D, 0x04), (0x4D, 0x0C)]
    ):
        emit(0x4C, 0x89, 0xF0, 0x48, 0xC1, 0xE8, 8 + 3 * index, 0x83, 0xE0, 0x07)
        emit(rex, 0x8B, modrm, 0xC4)

    # syscall number from the 256-entry table
    emit(0x4C, 0x89, 0xF0, 0x25, struct.pack("<I", 0xFF))
    lea(0x1D, "table")                            # lea rbx, [rip + table]
    emit(0x0F, 0xB7, 0x04, 0x43)                  # movzx eax, word [rbx + rax * 2]
    emit(0x0F, 0x05)                              # syscall
    emit(0x41, 0xFF, 0xCD)                        # dec r13d
    emit(0x0F, 0x85)
    emit(struct.pack("<i", loop_start - (len(code) + 4)))

    emit(0xB8, struct.pack("<I", 60), 0x31, 0xFF, 0x0F, 0x05)

    numbers = implemented()
    table = [numbers[index % len(numbers)] for index in range(240)]
    table += [301, 333, 355, 379, 401, 419, 433, 449, 303, 311, 345, 361, 391, 409, 427, 447]
    table_bytes = b"".join(struct.pack("<H", value) for value in table)
    assert len(table_bytes) == 512

    strings = bytearray(256)
    for slot, text in enumerate(STRINGS):
        strings[slot * 64 : slot * 64 + len(text)] = text

    labels = {"table": len(code), "strings": len(code) + len(table_bytes)}
    code.extend(table_bytes)
    code.extend(strings)
    for position, label in fixups:
        code[position : position + 4] = struct.pack("<i", labels[label] - (position + 4))
    return bytes(code), len(numbers)


def main():
    code, count = build()
    body = ", ".join(f"0x{byte:02x}" for byte in code)
    with open(OUT, "w", newline="\n") as handle:
        handle.write("//! Generated by tools/make-fuzz-probe.py - do not edit.\n\n")
        handle.write(f"pub const SYSCALL_FUZZ_PROBE: [u8; {len(code)}] = [{body}];\n")
    print(f"wrote {OUT}: {len(code)} bytes, {count} syscalls in the table")


main()
