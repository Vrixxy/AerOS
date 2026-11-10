"""Packs Linux firmware files into FIRMWARE.PAK for the boot partition.

The Linux driver adapter (kernel/linux) reads it for request_firmware(), so
the long Linux names need no long-file-name support on FAT.

    python tools/firmware-pak.py <linux-firmware dir> <out FIRMWARE.PAK> amdgpu/renoir_*.bin ...

Format: "AEFW", u32 count, then per file a 56-byte NUL-padded name, u32
offset and u32 size (offsets from the start of the pack), then the data.
"""
import glob
import os
import struct
import sys


def main():
    if len(sys.argv) < 4:
        sys.exit(__doc__)
    root, output, patterns = sys.argv[1], sys.argv[2], sys.argv[3:]
    names = []
    for pattern in patterns:
        matches = sorted(glob.glob(os.path.join(root, pattern)))
        if not matches:
            sys.exit(f"nothing matches {pattern}")
        names += [os.path.relpath(path, root).replace(os.sep, "/") for path in matches]
    blobs = [open(os.path.join(root, name), "rb").read() for name in names]
    offset = 8 + 64 * len(names)
    header = b"AEFW" + struct.pack("<I", len(names))
    for name, blob in zip(names, blobs):
        encoded = name.encode()
        if len(encoded) > 55:
            sys.exit(f"name too long: {name}")
        header += encoded.ljust(56, b"\0") + struct.pack("<II", offset, len(blob))
        offset += len(blob)
    with open(output, "wb") as pak:
        pak.write(header)
        for blob in blobs:
            pak.write(blob)
    print(f"{output}: {len(names)} files, {offset} bytes")


if __name__ == "__main__":
    main()
