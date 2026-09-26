"""Builds a corpus of small PNG files covering every colour type, bit depth,
interlacing and filter, together with the RGBA the decoder must produce.

Writes assets/test/png-corpus.bin: repeated { u32 length, u32 crc32 of the
expected RGBA bytes, PNG bytes } (little endian). The kernel's boot test
decodes each file and compares the checksum."""
import random
import struct
import sys
import zlib

random.seed(2026)
WIDTH, HEIGHT = 37, 29


def chunk(kind, body):
    return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)


def paeth(a, b, c):
    p = a + b - c
    pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    return b if pb <= pc else c


def filter_row(kind, row, previous, bpp):
    out = bytearray(len(row))
    for i, value in enumerate(row):
        left = row[i - bpp] if i >= bpp else 0
        up = previous[i]
        up_left = previous[i - bpp] if i >= bpp else 0
        if kind == 0:
            predicted = 0
        elif kind == 1:
            predicted = left
        elif kind == 2:
            predicted = up
        elif kind == 3:
            predicted = (left + up) // 2
        else:
            predicted = paeth(left, up, up_left)
        out[i] = (value - predicted) & 0xFF
    return bytes(out)


ADAM7 = [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)]


def pack_samples(samples, depth):
    """Pack a list of sample values (each `depth` bits) into bytes."""
    if depth == 16:
        return b"".join(struct.pack(">H", s) for s in samples)
    if depth == 8:
        return bytes(samples)
    out = bytearray()
    bits = 0
    count = 0
    for s in samples:
        bits = (bits << depth) | s
        count += depth
        if count == 8:
            out.append(bits)
            bits = 0
            count = 0
    if count:
        out.append(bits << (8 - count))
    return bytes(out)


def make_case(color, depth, interlace, transparency):
    channels = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[color]
    maximum = (1 << depth) - 1
    palette = [(random.randrange(256), random.randrange(256), random.randrange(256)) for _ in range(1 << min(depth, 8))] if color == 3 else None
    palette_alpha = [random.choice([0, 64, 128, 255]) for _ in palette] if color == 3 and transparency else None
    # Random pixels; for tRNS colour keys make sure the key colour appears.
    key = None
    if transparency and color == 0:
        key = random.randrange(maximum + 1)
    if transparency and color == 2:
        key = tuple(random.randrange(maximum + 1) for _ in range(3))
    pixels = []
    for y in range(HEIGHT):
        for x in range(WIDTH):
            if color == 3:
                sample = [random.randrange(len(palette))]
            else:
                sample = [random.randrange(maximum + 1) for _ in range(channels)]
                if key is not None and random.random() < 0.15:
                    sample = list(key) if color == 2 else [key]
            pixels.append(sample)

    def to8(v):
        if depth == 16:
            return v >> 8
        if depth == 8:
            return v
        return v * 255 // maximum

    expected = bytearray()
    for sample in pixels:
        if color == 0:
            g = to8(sample[0])
            a = 0 if key is not None and sample[0] == key else 255
            expected += bytes([g, g, g, a])
        elif color == 2:
            a = 0 if key is not None and tuple(sample) == key else 255
            expected += bytes([to8(sample[0]), to8(sample[1]), to8(sample[2]), a])
        elif color == 3:
            r, g, b = palette[sample[0]]
            a = palette_alpha[sample[0]] if palette_alpha else 255
            expected += bytes([r, g, b, a])
        elif color == 4:
            g = to8(sample[0])
            expected += bytes([g, g, g, to8(sample[1])])
        else:
            expected += bytes([to8(s) for s in sample])

    bits_per_pixel = channels * depth
    bpp = max(1, bits_per_pixel // 8)
    raw = bytearray()
    passes = ADAM7 if interlace else [(0, 0, 1, 1)]
    for x0, y0, dx, dy in passes:
        xs = list(range(x0, WIDTH, dx))
        ys = list(range(y0, HEIGHT, dy))
        if not xs or not ys:
            continue
        previous = bytes(-(-len(xs) * bits_per_pixel // 8))
        for y in ys:
            samples = []
            for x in xs:
                samples += pixels[y * WIDTH + x]
            row = pack_samples(samples, depth)
            kind = random.randrange(5)
            raw += bytes([kind]) + filter_row(kind, row, previous, bpp)
            previous = row
    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", WIDTH, HEIGHT, depth, color, 0, 0, 1 if interlace else 0))
    if palette:
        png += chunk(b"PLTE", b"".join(bytes(c) for c in palette))
        if palette_alpha:
            png += chunk(b"tRNS", bytes(palette_alpha))
    if key is not None:
        if color == 0:
            png += chunk(b"tRNS", struct.pack(">H", key))
        else:
            png += chunk(b"tRNS", struct.pack(">HHH", *key))
    compressed = zlib.compress(bytes(raw), 6)
    # Split the compressed stream over several IDAT chunks.
    third = max(1, len(compressed) // 3)
    for i in range(0, len(compressed), third):
        png += chunk(b"IDAT", compressed[i:i + third])
    png += chunk(b"IEND", b"")
    return png, bytes(expected)


cases = []
for color, depths in [(0, [1, 2, 4, 8, 16]), (2, [8, 16]), (3, [1, 2, 4, 8]), (4, [8, 16]), (6, [8, 16])]:
    for depth in depths:
        for interlace in (False, True):
            cases.append((color, depth, interlace, False))
            if color in (0, 2, 3):
                cases.append((color, depth, interlace, True))

out = bytearray()
for case in cases:
    png, expected = make_case(*case)
    out += struct.pack("<II", len(png), zlib.crc32(expected) & 0xFFFFFFFF) + png
open(sys.argv[1], "wb").write(out)
print("wrote", sys.argv[1], len(out), "bytes,", len(cases), "cases")
