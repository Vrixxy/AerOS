"""Independent FAT16/FAT32 reader used to cross-check the kernel's filesystem.

Usage: python tools/check-fat.py <image> [start_sector]
Lists the tree (long names included), checks that every cluster chain is
sound and that no cluster is claimed twice, and verifies the test pattern
files the kernel self-test leaves behind."""
import struct
import sys

image = open(sys.argv[1], "rb").read()
start = int(sys.argv[2]) if len(sys.argv) > 2 else 0
base = start * 512
boot = image[base:base + 512]
assert boot[510:512] == b"\x55\xaa", "no boot signature"
bps, spc, reserved, fats = struct.unpack_from("<HBHB", boot, 11)
root_entries, total16 = struct.unpack_from("<HH", boot, 17)
fat16 = struct.unpack_from("<H", boot, 22)[0]
total32, fat32 = struct.unpack_from("<I", boot, 32)[0], struct.unpack_from("<I", boot, 36)[0]
fat_sectors = fat16 or fat32
total = total16 or total32
root_sectors = (root_entries * 32 + 511) // 512
data_start = reserved + fats * fat_sectors + root_sectors
clusters = (total - data_start) // spc
is32 = clusters >= 65525
root_cluster = struct.unpack_from("<I", boot, 44)[0] if is32 else 0
print(f"FAT{'32' if is32 else '16'} clusters={clusters} spc={spc} fats={fats} fat_sectors={fat_sectors}")


def sector(n):
    return image[base + n * 512: base + (n + 1) * 512]


fat_bytes = b"".join(sector(reserved + i) for i in range(fat_sectors))
for copy in range(1, fats):
    other = b"".join(sector(reserved + copy * fat_sectors + i) for i in range(fat_sectors))
    assert other == fat_bytes, "FAT copies differ"


def fat_get(c):
    if is32:
        return struct.unpack_from("<I", fat_bytes, c * 4)[0] & 0x0FFFFFFF
    return struct.unpack_from("<H", fat_bytes, c * 2)[0]


def end(v):
    return v >= (0x0FFFFFF8 if is32 else 0xFFF8)


claimed = {}


def chain(first, owner):
    out = []
    c = first
    while c >= 2 and not end(c):
        assert 2 <= c < clusters + 2, f"{owner}: cluster {c} out of range"
        assert c not in claimed, f"{owner}: cluster {c} also owned by {claimed[c]}"
        claimed[c] = owner
        out.append(c)
        c = fat_get(c)
        assert len(out) <= clusters, f"{owner}: chain loop"
    return out


def cluster_data(c):
    return b"".join(sector(data_start + (c - 2) * spc + i) for i in range(spc))


def dir_bytes(first):
    if first == 0 and not is32:
        return b"".join(sector(reserved + fats * fat_sectors + i) for i in range(root_sectors))
    return b"".join(cluster_data(c) for c in chain_nocount(first))


def chain_nocount(first):
    out, c = [], first
    while c >= 2 and not end(c):
        out.append(c)
        c = fat_get(c)
    return out


def checksum(short):
    s = 0
    for b in short:
        s = (((s & 1) << 7) | (s >> 1)) + b & 0xFF
    return s


def read_dir(first, path, owner_first=True):
    raw = dir_bytes(first)
    entries = []
    lfn = []
    for i in range(0, len(raw), 32):
        e = raw[i:i + 32]
        if e[0] == 0:
            break
        if e[0] == 0xE5:
            lfn = []
            continue
        if e[11] & 0x3F == 0x0F:
            lfn.append(e)
            continue
        if e[11] & 0x08:
            lfn = []
            continue
        short = e[:11]
        name = None
        if lfn:
            parts = sorted(lfn, key=lambda x: x[0] & 0x1F)
            assert all(p[13] == checksum(short) for p in parts), f"bad LFN checksum near {short!r}"
            chars = b""
            for p in parts:
                chars += p[1:11] + p[14:26] + p[28:32]
            text = chars.decode("utf-16-le")
            name = text.split("\x00")[0]
        if name is None:
            name = (short[:8].decode().rstrip() + ("." + short[8:].decode().rstrip() if short[8:].strip() else ""))
        lfn = []
        cl = struct.unpack_from("<H", e, 26)[0] | ((struct.unpack_from("<H", e, 20)[0] << 16) if is32 else 0)
        size = struct.unpack_from("<I", e, 28)[0]
        attr = e[11]
        if name in (".", ".."):
            continue
        entries.append((name, attr, cl, size))
    return entries


def pattern(seed, i):
    return ((i * 2654435761 + seed) & 0xFFFFFFFF) >> 16 & 0xFF


def walk(first, path):
    for name, attr, cl, size in read_dir(first, path):
        full = path + "/" + name
        if attr & 0x10:
            print("DIR ", full)
            chain(cl, full)
            walk(cl, full)
        else:
            print(f"FILE {full} ({size} bytes)")
            ch = chain(cl, full) if cl else []
            assert len(ch) * spc * 512 >= size, f"{full}: chain too short"
            if size:
                data = b"".join(cluster_data(c) for c in ch)[:size]
                if "remember me" in name:
                    assert all(data[i] == pattern(5, i) for i in range(size)), "pattern mismatch"
                    print("     pattern OK")


walk(root_cluster, "")
free = sum(1 for c in range(2, clusters + 2) if fat_get(c) == 0)
used = sum(1 for c in range(2, clusters + 2) if fat_get(c) != 0)
assert used == len(claimed), f"FAT marks {used} clusters used, tree owns {len(claimed)}"
print(f"OK: {len(claimed)} clusters in use, {free} free")
