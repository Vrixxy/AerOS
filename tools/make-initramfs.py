"""Builds a minimal Linux initramfs (newc cpio) around one static program,
which becomes /init. Usage: make-initramfs.py <program> <output.cpio>"""

import struct
import sys


def entry(name, mode, data=b"", rdev=(0, 0)):
    encoded = name.encode() + b"\0"
    header = "070701" + "".join(
        f"{value:08X}"
        for value in (
            0, mode, 0, 0, 1, 0, len(data), 0, 0, rdev[0], rdev[1], len(encoded), 0,
        )
    )
    blob = header.encode() + encoded
    blob += b"\0" * (-len(blob) % 4)
    blob += data + b"\0" * (-len(data) % 4)
    return blob


def main():
    program = open(sys.argv[1], "rb").read()
    archive = b""
    archive += entry("dev", 0o040755)
    archive += entry("dev/console", 0o020600, rdev=(5, 1))
    archive += entry("dev/null", 0o020666, rdev=(1, 3))
    archive += entry("init", 0o100755, program)
    archive += entry("TRAILER!!!", 0)
    open(sys.argv[2], "wb").write(archive)


main()
