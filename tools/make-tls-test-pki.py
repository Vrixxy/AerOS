"""Makes the certificates the boot-test's TLS check talks to.

usage: make-tls-test-pki.py <dir>

Writes into <dir> the PEM files for two `openssl s_server` instances (rsa.*
and ec.*) and the two roots as DER (TLSRSA.DER, TLSEC.DER); the harness puts
those on the virtio test disk for the kernel to read."""

import os
import sys
import tempfile

import tlspki


def main():
    server_dir = sys.argv[1]
    os.makedirs(server_dir, exist_ok=True)
    with tempfile.TemporaryDirectory() as work:
        for prefix, name, args in (
            ("rsa", "rsa-test", ("rsa", "sha256", ("rsa", "sha384"), "rsa", "sha384")),
            ("ec", "ec-test", ("p256", "sha384", ("p384", "sha384"), "p256", "sha256")),
        ):
            material = tlspki.chain(work, name, *args)
            for suffix, text in (
                ("leaf.pem", material["leaf_pem"]),
                ("chain.pem", material["intermediate_pem"]),
                ("key.pem", material["leaf_key_pem"]),
            ):
                open(os.path.join(server_dir, "%s.%s" % (prefix, suffix)), "w", newline="\n").write(text)
            open(os.path.join(server_dir, "TLS%s.DER" % prefix.upper()), "wb").write(material["root"])
    print("test PKI written")


main()
