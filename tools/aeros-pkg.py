"""Builds and signs AerOS packages on the host (needs Python 3 and OpenSSL).

  aeros-pkg.py keygen <key.pem>                 new Ed25519 key; prints the public key in hex
  aeros-pkg.py pubkey <key.pem>                 prints the public key in hex (for `pkg trust`)
  aeros-pkg.py build <name> <version> <dir> <out.pkg>
                                                packs every file under <dir> (files in bin/ get mode 755)
  aeros-pkg.py sign <key.pem> <file.pkg>        writes <file.pkg>.sig (64 bytes)

Copy both files to the machine and run `pkg install <file.pkg>` after
`pkg trust <public key hex>`.
"""

import hashlib
import os
import re
import subprocess
import sys

MAX_FILES = 16
MAX_FILE = 1 << 20


def public_key(pem):
    der = subprocess.run(
        ["openssl", "pkey", "-in", pem, "-pubout", "-outform", "DER"],
        check=True,
        capture_output=True,
    ).stdout
    return der[-32:].hex()


def build(name, version, directory, output):
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,24}", name):
        sys.exit("name must be 1-24 letters, digits, - or _")
    if not re.fullmatch(r"[A-Za-z0-9._-]{1,16}", version):
        sys.exit("version must be 1-16 letters, digits, . - or _")
    files = []
    for folder, _, names in os.walk(directory):
        for entry in sorted(names):
            full = os.path.join(folder, entry)
            relative = os.path.relpath(full, directory).replace(os.sep, "/")
            files.append((relative, full))
    files.sort()
    if not 0 < len(files) <= MAX_FILES:
        sys.exit(f"a package needs between 1 and {MAX_FILES} files")
    lines = ["AEROSPKG1", f"name={name}", f"version={version}"]
    data = b""
    for relative, full in files:
        if len(relative) > 96 or relative.count("/") >= 8 or ";" in relative:
            sys.exit(f"unsupported path: {relative}")
        content = open(full, "rb").read()
        if len(content) > MAX_FILE:
            sys.exit(f"{relative} is larger than 1 MiB")
        mode = 0o755 if relative.startswith("bin/") else 0o644
        lines.append(f"file={relative};{mode:o};{len(content)};{hashlib.sha256(content).hexdigest()}")
        data += content
    with open(output, "wb") as handle:
        handle.write(("\n".join(lines) + "\ndata\n").encode() + data)
    print(f"{output}: {len(files)} files, {len(data)} bytes of data")


def main():
    arguments = sys.argv[1:]
    if len(arguments) == 2 and arguments[0] == "keygen":
        subprocess.run(["openssl", "genpkey", "-algorithm", "ed25519", "-out", arguments[1]], check=True)
        print(public_key(arguments[1]))
    elif len(arguments) == 2 and arguments[0] == "pubkey":
        print(public_key(arguments[1]))
    elif len(arguments) == 5 and arguments[0] == "build":
        build(*arguments[1:])
    elif len(arguments) == 3 and arguments[0] == "sign":
        subprocess.run(
            ["openssl", "pkeyutl", "-sign", "-rawin", "-inkey", arguments[1],
             "-in", arguments[2], "-out", arguments[2] + ".sig"],
            check=True,
        )
        print(arguments[2] + ".sig")
    else:
        sys.exit(__doc__)


main()
