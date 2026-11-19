"""Checks that every x86-64 Linux system call number (0-334, 424-467) has a
handler written for it: an arm of the dispatchers in kernel/src/syscall.rs
or an entry of kernel/src/syscall/extra.rs. Prints the numbers that would
fall through to the "unknown" path and exits 1 if there are any.

    py tools/syscall-coverage.py
"""
import glob
import os
import re
import sys

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "kernel", "src")


def numbers(text):
    found = set()
    for first, last in re.findall(r"(\d+)(?:\.\.=(\d+))?", re.sub(r"//[^\n]*", "", text)):
        start = int(first)
        found.update(range(start, int(last) + 1) if last else [start])
    return found


def main():
    constants = {}
    for path in glob.glob(os.path.join(ROOT, "*.rs")) + glob.glob(os.path.join(ROOT, "syscall", "*.rs")):
        for name, value in re.findall(r"const (LINUX_[A-Z0-9_]+): u64 = (\d+)", open(path, encoding="utf-8").read()):
            constants[name] = int(value)
    source = open(os.path.join(ROOT, "syscall.rs"), encoding="utf-8").read()
    names_table = source.index("pub(crate) fn linux_syscall_name")
    dispatch = source[:names_table]
    handled = set()
    for arm in re.finditer(r"((?:LINUX_[A-Z0-9_]+\s*\|\s*)*LINUX_[A-Z0-9_]+)\s*(?:=>|if\b)", dispatch):
        for name in re.findall(r"LINUX_[A-Z0-9_]+", arm.group(1)):
            if name in constants:
                handled.add(constants[name])
    extra = open(os.path.join(ROOT, "syscall", "extra.rs"), encoding="utf-8").read()
    for table in ("ABSENT", "PRIVILEGED"):
        handled |= numbers(re.search(table + r": &\[u64\] = &\[(.*?)\];", extra, re.S).group(1))
    body = extra[extra.index("pub(super) fn dispatch"):]
    for arm in re.findall(r"^\s{8}([\d .=|]+)=>", body, re.M):
        handled |= numbers(arm)
    table = set(range(0, 335)) | set(range(424, 468))
    missing = sorted(table - handled)
    print(f"handled {len(table) - len(missing)} of {len(table)}")
    if missing:
        print("missing:", missing)
        sys.exit(1)


main()
