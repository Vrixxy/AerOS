"""Losslessly converts a baseline JPEG into a progressive one (like jpegtran
-progressive), so the kernel's progressive decoder can be tested against the
baseline decode of the very same coefficients.

Usage: python tools/make-progressive.py in.jpg out.jpg
Only what the test needs: 8-bit, 1 or 3 components, no restart intervals."""
import struct
import sys

ZIGZAG = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27,
    20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58,
    59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
]

data = open(sys.argv[1], "rb").read()

# ---------------------------------------------------------------- parse baseline
quant = {}
huff = {}  # (class, id) -> (bits, values)
frame = None
pos = 2
scan_data = None
while pos < len(data):
    assert data[pos] == 0xFF
    marker = data[pos + 1]
    if marker == 0xD9:
        break
    length = struct.unpack(">H", data[pos + 2:pos + 4])[0]
    body = data[pos + 4:pos + 2 + length]
    if marker == 0xDB:
        i = 0
        while i < len(body):
            precision, table_id = body[i] >> 4, body[i] & 15
            size = 64 * (2 if precision else 1)
            quant[table_id] = (precision, body[i + 1:i + 1 + size])
            i += 1 + size
    elif marker == 0xC4:
        i = 0
        while i < len(body):
            cls, table_id = body[i] >> 4, body[i] & 15
            bits = list(body[i + 1:i + 17])
            count = sum(bits)
            huff[(cls, table_id)] = (bits, list(body[i + 17:i + 17 + count]))
            i += 17 + count
    elif marker == 0xC0:
        precision, height, width, count = struct.unpack(">BHHB", body[:6])
        comps = []
        for k in range(count):
            cid, hv, tq = body[6 + 3 * k:9 + 3 * k]
            comps.append({"id": cid, "h": hv >> 4, "v": hv & 15, "tq": tq})
        frame = (precision, height, width, comps)
    elif marker == 0xC2:
        sys.exit("already progressive")
    elif marker == 0xDD:
        if struct.unpack(">H", body[:2])[0]:
            sys.exit("restart intervals not supported by this tool")
    elif marker == 0xDA:
        count = body[0]
        scan_tables = {}
        for k in range(count):
            scan_tables[body[1 + 2 * k]] = (body[2 + 2 * k] >> 4, body[2 + 2 * k] & 15)
        scan_data = data[pos + 2 + length:]
        break
    pos += 2 + length

precision, height, width, comps = frame
assert precision == 8
max_h = max(c["h"] for c in comps)
max_v = max(c["v"] for c in comps)
mcus_x = -(-width // (8 * max_h))
mcus_y = -(-height // (8 * max_v))
for c in comps:
    c["blocks_w"] = mcus_x * c["h"]
    c["blocks_h"] = mcus_y * c["v"]
    comp_w = -(-(width * c["h"]) // max_h)
    comp_h = -(-(height * c["v"]) // max_v)
    c["real_w"] = -(-comp_w // 8)
    c["real_h"] = -(-comp_h // 8)
    c["blocks"] = [[0] * 64 for _ in range(c["blocks_w"] * c["blocks_h"])]

# Huffman decoding tables.
def build_decoder(bits, values):
    table = {}
    code = 0
    index = 0
    for length in range(1, 17):
        for _ in range(bits[length - 1]):
            table[(length, code)] = values[index]
            code += 1
            index += 1
        code <<= 1
    return table

decoders = {key: build_decoder(*value) for key, value in huff.items()}

# Unstuff the entropy-coded data up to the next marker.
raw = bytearray()
i = 0
while i < len(scan_data):
    b = scan_data[i]
    if b == 0xFF:
        nxt = scan_data[i + 1]
        if nxt == 0:
            raw.append(0xFF)
            i += 2
            continue
        break
    raw.append(b)
    i += 1
bitpos = 0

def read_bit():
    global bitpos
    byte = raw[bitpos >> 3] if (bitpos >> 3) < len(raw) else 0
    bit = (byte >> (7 - (bitpos & 7))) & 1
    bitpos += 1
    return bit

def read_bits(n):
    value = 0
    for _ in range(n):
        value = (value << 1) | read_bit()
    return value

def decode_symbol(table):
    code = 0
    for length in range(1, 17):
        code = (code << 1) | read_bit()
        if (length, code) in table:
            return table[(length, code)]
    raise ValueError("bad huffman code")

def extend(value, n):
    return value - (1 << n) + 1 if n and value < (1 << (n - 1)) else value

predictors = [0] * len(comps)
for my in range(mcus_y):
    for mx in range(mcus_x):
        for ci, c in enumerate(comps):
            dc_id, ac_id = scan_tables[c["id"]]
            for by in range(c["v"]):
                for bx in range(c["h"]):
                    block = c["blocks"][(my * c["v"] + by) * c["blocks_w"] + mx * c["h"] + bx]
                    n = decode_symbol(decoders[(0, dc_id)])
                    predictors[ci] += extend(read_bits(n), n) if n else 0
                    block[0] = predictors[ci]
                    k = 1
                    while k < 64:
                        rs = decode_symbol(decoders[(1, ac_id)])
                        r, s = rs >> 4, rs & 15
                        if s == 0:
                            if r == 15:
                                k += 16
                                continue
                            break
                        k += r
                        block[ZIGZAG[k]] = extend(read_bits(s), s)
                        k += 1

# ---------------------------------------------------------------- progressive encoder
def optimal_table(freq):
    freq = list(freq) + [1]
    codesize = [0] * 257
    others = [-1] * 257
    while True:
        c1, v = -1, 10**18
        for i in range(257):
            if freq[i] and freq[i] <= v:
                v, c1 = freq[i], i
        c2, v = -1, 10**18
        for i in range(257):
            if freq[i] and freq[i] <= v and i != c1:
                v, c2 = freq[i], i
        if c2 < 0:
            break
        freq[c1] += freq[c2]
        freq[c2] = 0
        codesize[c1] += 1
        while others[c1] >= 0:
            c1 = others[c1]
            codesize[c1] += 1
        others[c1] = c2
        codesize[c2] += 1
        while others[c2] >= 0:
            c2 = others[c2]
            codesize[c2] += 1
    bits = [0] * 33
    for i in range(257):
        if codesize[i]:
            bits[codesize[i]] += 1
    for i in range(32, 16, -1):
        while bits[i] > 0:
            j = i - 2
            while bits[j] == 0:
                j -= 1
            bits[i] -= 2
            bits[i - 1] += 1
            bits[j + 1] += 2
            bits[j] -= 1
    i = 16
    while bits[i] == 0:
        i -= 1
    bits[i] -= 1
    values = []
    for size in range(1, 33):
        for sym in range(256):
            if codesize[sym] == size:
                values.append(sym)
    return bits[1:17], values


def code_table(bits, values):
    codes = {}
    code = 0
    index = 0
    for length in range(1, 17):
        for _ in range(bits[length - 1]):
            codes[values[index]] = (code, length)
            code += 1
            index += 1
        code <<= 1
    return codes


class Scan:
    """Runs a scan twice: once to count symbols, once to write the bits."""

    def __init__(self, component_indexes, ss, se, ah, al):
        self.comps = component_indexes
        self.ss, self.se, self.ah, self.al = ss, se, ah, al

    def run(self, emit_symbol, emit_bits):
        ss, se, ah, al = self.ss, self.se, self.ah, self.al
        state = {"eobrun": 0, "buffer": [], "last_dc": {ci: 0 for ci in self.comps}}

        def flush_eobrun():
            if state["eobrun"] > 0:
                n = state["eobrun"].bit_length() - 1
                emit_symbol(n << 4)
                if n:
                    emit_bits(state["eobrun"] & ((1 << n) - 1), n)
                state["eobrun"] = 0
                for b in state["buffer"]:
                    emit_bits(b, 1)
                state["buffer"] = []

        def do_block(ci, block):
            if ss == 0:
                if ah == 0:
                    temp = block[0] >> al
                    diff = temp - state["last_dc"][ci]
                    state["last_dc"][ci] = temp
                    n = abs(diff).bit_length()
                    emit_symbol(n)
                    if n:
                        emit_bits(diff if diff >= 0 else diff - 1 & ((1 << n) - 1), n)
                else:
                    emit_bits((block[0] >> al) & 1, 1)
                return
            if ah == 0:
                r = 0
                for k in range(ss, se + 1):
                    coefficient = block[ZIGZAG[k]]
                    if coefficient == 0:
                        r += 1
                        continue
                    if coefficient < 0:
                        temp = (-coefficient) >> al
                        temp2 = ~temp
                    else:
                        temp = coefficient >> al
                        temp2 = temp
                    if temp == 0:
                        r += 1
                        continue
                    flush_eobrun()
                    while r > 15:
                        emit_symbol(0xF0)
                        r -= 16
                    n = temp.bit_length()
                    emit_symbol((r << 4) + n)
                    emit_bits(temp2 & ((1 << n) - 1), n)
                    r = 0
                if r > 0:
                    state["eobrun"] += 1
                    if state["eobrun"] == 0x7FFF:
                        flush_eobrun()
                return
            # AC refinement
            absvalues = [0] * 64
            last_new = 0
            for k in range(ss, se + 1):
                temp = abs(block[ZIGZAG[k]]) >> al
                absvalues[k] = temp
                if temp == 1:
                    last_new = k
            r = 0
            pending = []
            for k in range(ss, se + 1):
                temp = absvalues[k]
                if temp == 0:
                    r += 1
                    continue
                while r > 15 and k <= last_new:
                    flush_eobrun()
                    emit_symbol(0xF0)
                    r -= 16
                    for b in pending:
                        emit_bits(b, 1)
                    pending = []
                if temp > 1:
                    pending.append(temp & 1)
                    continue
                flush_eobrun()
                emit_symbol((r << 4) + 1)
                emit_bits(0 if block[ZIGZAG[k]] < 0 else 1, 1)
                for b in pending:
                    emit_bits(b, 1)
                pending = []
                r = 0
            if r > 0 or pending:
                state["eobrun"] += 1
                state["buffer"].extend(pending)
                if state["eobrun"] == 0x7FFF or len(state["buffer"]) > 900:
                    flush_eobrun()

        if len(self.comps) > 1:
            for my in range(mcus_y):
                for mx in range(mcus_x):
                    for ci in self.comps:
                        c = comps[ci]
                        for by in range(c["v"]):
                            for bx in range(c["h"]):
                                do_block(ci, c["blocks"][(my * c["v"] + by) * c["blocks_w"] + mx * c["h"] + bx])
        else:
            ci = self.comps[0]
            c = comps[ci]
            for by in range(c["real_h"]):
                for bx in range(c["real_w"]):
                    do_block(ci, c["blocks"][by * c["blocks_w"] + bx])
        flush_eobrun()


def build_scan(scan_spec, out):
    scan = Scan(*scan_spec)
    is_dc = scan.ss == 0
    freq = [0] * 256
    if is_dc and scan.ah != 0:
        table = None  # refinement of DC: raw bits only
    else:
        def count(symbol):
            freq[symbol] += 1
        scan.run(count, lambda v, n: None)
        bits, values = optimal_table(freq)
        table = code_table(bits, values)
        cls = 0 if is_dc else 1
        payload = bytes([(cls << 4) | 0]) + bytes(bits) + bytes(values)
        out += b"\xFF\xC4" + struct.pack(">H", 2 + len(payload)) + payload
    # SOS header
    sos = bytes([len(scan.comps)])
    for ci in scan.comps:
        sos += bytes([comps[ci]["id"], 0x00])
    sos += bytes([scan.ss, scan.se, (scan.ah << 4) | scan.al])
    out += b"\xFF\xDA" + struct.pack(">H", 2 + len(sos)) + sos
    # entropy-coded data
    bitbuf = []

    def emit_symbol(symbol):
        code, length = table[symbol]
        for i in range(length - 1, -1, -1):
            bitbuf.append((code >> i) & 1)

    def emit_bits(value, n):
        for i in range(n - 1, -1, -1):
            bitbuf.append((value >> i) & 1)

    scan.run(emit_symbol, emit_bits)
    while len(bitbuf) % 8:
        bitbuf.append(1)
    for i in range(0, len(bitbuf), 8):
        byte = 0
        for b in bitbuf[i:i + 8]:
            byte = (byte << 1) | b
        out.append(byte)
        if byte == 0xFF:
            out.append(0)


result = bytearray(b"\xFF\xD8")
# Copy the quantization tables.
for table_id, (prec, bytes_) in sorted(quant.items()):
    result += b"\xFF\xDB" + struct.pack(">H", 2 + 1 + len(bytes_)) + bytes([(prec << 4) | table_id]) + bytes(bytes_)
# Frame header (progressive).
sof = struct.pack(">BHHB", 8, height, width, len(comps))
for c in comps:
    sof += bytes([c["id"], (c["h"] << 4) | c["v"], c["tq"]])
result += b"\xFF\xC2" + struct.pack(">H", 2 + len(sof)) + sof

if len(comps) == 3:
    specs = [
        ([0, 1, 2], 0, 0, 0, 1),
        ([0], 1, 5, 0, 2),
        ([2], 1, 63, 0, 1),
        ([1], 1, 63, 0, 1),
        ([0], 6, 63, 0, 2),
        ([0], 1, 63, 2, 1),
        ([0, 1, 2], 0, 0, 1, 0),
        ([2], 1, 63, 1, 0),
        ([1], 1, 63, 1, 0),
        ([0], 1, 63, 1, 0),
    ]
else:
    specs = [([0], 0, 0, 0, 1), ([0], 1, 5, 0, 2), ([0], 6, 63, 0, 2), ([0], 1, 63, 2, 1), ([0], 0, 0, 1, 0), ([0], 1, 63, 1, 0)]
for spec in specs:
    build_scan(spec, result)
result += b"\xFF\xD9"
open(sys.argv[2], "wb").write(result)
print("wrote", sys.argv[2], len(result), "bytes,", len(specs), "scans")
