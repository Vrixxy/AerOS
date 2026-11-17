"""Makes a copy of an AerOS-Setup.exe with some payload files replaced or added.

    python tools/patch-installer.py <in.exe> <out.exe> NAME=local/file ...

The payload (a zip after the setup program, then "AEROSPKG" and the
program's length) is copied record by record without recompressing, so the
4 GB root image is not touched; only the named entries are rewritten.
Useful when tools/build-installer.ps1 can't run (it needs WSL for ROOTFS).
"""
import struct
import sys
import zlib

LOCAL = b'PK\x03\x04'
CENTRAL = b'PK\x01\x02'
END = b'PK\x05\x06'


def read_central(data):
    end = data.rindex(END)
    count, size, offset = struct.unpack_from('<HII', data, end + 10)
    if count == 0xFFFF or offset == 0xFFFFFFFF:
        raise SystemExit('zip64 payloads are not handled')
    entries, at = [], offset
    for _ in range(count):
        assert data[at:at + 4] == CENTRAL
        name_length, extra_length, comment_length = struct.unpack_from('<HHH', data, at + 28)
        local = struct.unpack_from('<I', data, at + 42)[0]
        name = data[at + 46:at + 46 + name_length].decode('utf-8')
        record = data[at:at + 46 + name_length + extra_length + comment_length]
        entries.append((name, local, record))
        at += len(record)
    return entries, offset


def local_length(data, local, record):
    compressed = struct.unpack_from('<I', record, 20)[0]
    name_length, extra_length = struct.unpack_from('<HH', data, local + 26)
    flags = struct.unpack_from('<H', data, local + 6)[0]
    length = 30 + name_length + extra_length + compressed
    if flags & 8:
        length += 16 if data[local + length:local + length + 4] == b'PK\x07\x08' else 12
    return length


def new_entry(name, content):
    packer = zlib.compressobj(9, zlib.DEFLATED, -15)
    packed = packer.compress(content) + packer.flush()
    crc = zlib.crc32(content) & 0xFFFFFFFF
    encoded = name.encode()
    header = struct.pack('<4sHHHHHIIIHH', LOCAL, 20, 0, 8, 0, 0x21, crc, len(packed), len(content), len(encoded), 0)
    central = struct.pack('<4sHHHHHHIIIHHHHHII', CENTRAL, 20, 20, 0, 8, 0, 0x21, crc, len(packed), len(content),
                          len(encoded), 0, 0, 0, 0, 0, 0) + encoded
    return header + encoded + packed, central


def main():
    source, target, pairs = sys.argv[1], sys.argv[2], sys.argv[3:]
    replacements = {}
    for pair in pairs:
        name, path = pair.split('=', 1)
        replacements[name] = open(path, 'rb').read()
    blob = open(source, 'rb').read()
    assert blob[-16:-8] == b'AEROSPKG'
    program_length = struct.unpack('<q', blob[-8:])[0]
    program, data = blob[:program_length], blob[program_length:-16]
    entries, _ = read_central(data)
    out = bytearray()
    central = bytearray()
    count = 0
    for name, local, record in entries:
        if name in replacements:
            continue
        piece = data[local:local + local_length(data, local, record)]
        record = bytearray(record)
        struct.pack_into('<I', record, 42, len(out))
        out += piece
        central += record
        count += 1
    for name, content in replacements.items():
        local_record, central_record = new_entry(name, content)
        central_record = bytearray(central_record)
        struct.pack_into('<I', central_record, 42, len(out))
        out += local_record
        central += central_record
        count += 1
        print(f'{name}: {len(content)} bytes')
    if len(out) + len(central) >= 1 << 32:
        raise SystemExit('payload would need zip64')
    directory_offset = len(out)
    out += central
    out += struct.pack('<4sHHHHIIH', END, 0, 0, count, count, len(central), directory_offset, 0)
    with open(target, 'wb') as file:
        file.write(program)
        file.write(out)
        file.write(b'AEROSPKG' + struct.pack('<q', program_length))
    print(f'{target}: {count} entries')


if __name__ == '__main__':
    main()
