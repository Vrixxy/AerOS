#!/usr/bin/env python3
"""Makes and checks disk images for the installer tests.

  make-gpt-disk.py make   <image> <size-mib> <partition-mib>
      A raw image with a protective MBR, a GPT and one Linux data partition at
      sector 2048 filled with pseudo-random bytes; prints the SHA-256 of the
      partition.
  make-gpt-disk.py verify <image> <partition-sha256>
      Checks that the GPT is valid in both copies, the first partition is
      byte for byte what it was, and a second partition of type EFI System
      holds a FAT32 volume with EFI/BOOT/BOOTX64.EFI.
"""
import hashlib
import struct
import sys
import uuid
import zlib

SECTOR = 512
LINUX = uuid.UUID("0FC63DAF-8483-4772-8E79-3D69D8477DE4").bytes_le
ESP = uuid.UUID("C12A7328-F81F-11D2-BA4B-00A0C93EC93B").bytes_le


def crc(data):
    return zlib.crc32(data) & 0xFFFFFFFF


def make(path, size_mib, part_mib):
    sectors = size_mib * 2048
    first = 2048
    last = first + part_mib * 2048 - 1
    image = bytearray(sectors * SECTOR)
    mbr = bytearray(SECTOR)
    mbr[446 + 4] = 0xEE
    struct.pack_into("<II", mbr, 446 + 8, 1, min(sectors - 1, 0xFFFFFFFF))
    mbr[510:512] = b"\x55\xaa"
    image[:SECTOR] = mbr
    entry = bytearray(128)
    entry[0:16] = LINUX
    entry[16:32] = uuid.UUID("11111111-2222-3333-4444-555555555555").bytes_le
    struct.pack_into("<QQ", entry, 32, first, last)
    entry[56:72] = "Existing".encode("utf-16-le")
    array = bytes(entry) + bytes(127 * 128)
    array_crc = crc(array)
    disk_guid = uuid.UUID("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee").bytes_le

    def header(here, other, array_lba):
        h = bytearray(SECTOR)
        h[0:8] = b"EFI PART"
        struct.pack_into("<IIIII", h, 8, 0x00010000, 92, 0, 0, here)
        struct.pack_into("<Q", h, 32, other)
        struct.pack_into("<QQ", h, 40, 34, sectors - 34)
        h[56:72] = disk_guid
        struct.pack_into("<QIII", h, 72, array_lba, 128, 128, array_crc)
        struct.pack_into("<I", h, 16, crc(bytes(h[:92])))
        return h

    image[SECTOR:2 * SECTOR] = header(1, sectors - 1, 2)
    image[2 * SECTOR:2 * SECTOR + len(array)] = array
    backup_array = sectors - 33
    image[backup_array * SECTOR:backup_array * SECTOR + len(array)] = array
    image[(sectors - 1) * SECTOR:sectors * SECTOR] = header(sectors - 1, 1, backup_array)
    seed = 0x12345678
    block = bytearray()
    state = seed
    for _ in range(4096):
        state = (state * 1103515245 + 12345) & 0xFFFFFFFF
        block.append(state >> 16 & 0xFF)
    body = bytes(block) * ((last - first + 1) * SECTOR // len(block))
    image[first * SECTOR:first * SECTOR + len(body)] = body
    with open(path, "wb") as handle:
        handle.write(image)
    print(hashlib.sha256(image[first * SECTOR:(last + 1) * SECTOR]).hexdigest())


def verify(path, expected):
    with open(path, "rb") as handle:
        data = handle.read()
    sectors = len(data) // SECTOR

    def read_table(lba, other):
        h = data[lba * SECTOR:(lba + 1) * SECTOR]
        assert h[:8] == b"EFI PART", "no GPT signature"
        size = struct.unpack_from("<I", h, 12)[0]
        stored = struct.unpack_from("<I", h, 16)[0]
        copy = bytearray(h[:size])
        copy[16:20] = bytes(4)
        assert crc(bytes(copy)) == stored, "header CRC"
        assert struct.unpack_from("<Q", h, 24)[0] == lba
        assert struct.unpack_from("<Q", h, 32)[0] == other
        array_lba, count, entry_size, array_crc = struct.unpack_from("<QIII", h, 72)
        array = data[array_lba * SECTOR:array_lba * SECTOR + count * entry_size]
        assert crc(array) == array_crc, "array CRC"
        return array, count, entry_size

    primary = read_table(1, sectors - 1)
    backup = read_table(sectors - 1, 1)
    assert primary[0] == backup[0], "table copies differ"
    array, count, size = primary
    used = [array[i * size:(i + 1) * size] for i in range(count) if any(array[i * size:i * size + 16])]
    assert len(used) == 2, f"expected 2 partitions, found {len(used)}"
    first = struct.unpack_from("<QQ", used[0], 32)
    assert used[0][:16] == LINUX and first[0] == 2048, "the first partition moved"
    actual = hashlib.sha256(data[first[0] * SECTOR:(first[1] + 1) * SECTOR]).hexdigest()
    assert actual == expected, "the existing partition changed"
    assert used[1][:16] == ESP, "second partition is not an EFI System partition"
    start, end = struct.unpack_from("<QQ", used[1], 32)
    assert start > first[1] and end < sectors - 33
    boot = data[start * SECTOR:(start + 1) * SECTOR]
    assert boot[82:90] == b"FAT32   " and boot[510:512] == b"\x55\xaa", "no FAT32 volume"
    reserved = struct.unpack_from("<H", boot, 14)[0]
    fats = boot[16]
    fat_size = struct.unpack_from("<I", boot, 36)[0]
    cluster_sectors = boot[13]
    data_start = start + reserved + fats * fat_size
    root = data[data_start * SECTOR:data_start * SECTOR + cluster_sectors * SECTOR]
    assert b"EFI        " in root, "no EFI directory"
    print("ok", start, end)


if __name__ == "__main__":
    if sys.argv[1] == "make":
        make(sys.argv[2], int(sys.argv[3]), int(sys.argv[4]))
    else:
        verify(sys.argv[2], sys.argv[3])
