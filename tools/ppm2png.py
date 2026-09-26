import struct, sys, zlib


def convert(source, target):
    data = open(source, "rb").read()
    parts = data.split(None, 4)
    width, height = int(parts[1]), int(parts[2])
    pixels = parts[4] if len(parts) > 4 else b""
    start = len(data) - width * height * 3
    pixels = data[start:]
    rows = b"".join(b"\x00" + pixels[y * width * 3:(y + 1) * width * 3] for y in range(height))

    def chunk(tag, body):
        return struct.pack(">I", len(body)) + tag + body + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(rows, 3)) + chunk(b"IEND", b"")
    open(target, "wb").write(png)


if __name__ == "__main__":
    for path in sys.argv[1:]:
        convert(path, path[:-4] + ".png")
