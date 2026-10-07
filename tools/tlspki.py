"""Throwaway certificate authorities made with OpenSSL, for the TLS tests.

Everything made here is test material: the keys are never used for anything
else, and the certificates are valid from 2024 to 2049 so that a clock that is
a little off cannot make a test fail."""

import os
import subprocess

NOT_BEFORE = "20240101000000Z"
NOT_AFTER = "20490101000000Z"

KEY_ARGS = {
    "rsa": ["-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048"],
    "p256": ["-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:prime256v1"],
    "p384": ["-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:secp384r1"],
    "ed25519": ["-algorithm", "ED25519"],
}

INTERMEDIATE = "basicConstraints=critical,CA:TRUE,pathlen:0\nkeyUsage=critical,keyCertSign,cRLSign\n"
NOT_A_CA = "basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\n"
LEAF = (
    "basicConstraints=CA:FALSE\nkeyUsage=critical,digitalSignature\n"
    "extendedKeyUsage=serverAuth\nsubjectAltName=DNS:aeros.test,DNS:*.wild.test,IP:10.0.2.2\n"
)


def run(args):
    return subprocess.run(args, check=True, capture_output=True).stdout


def make_key(directory, name, kind):
    path = os.path.join(directory, name + ".key")
    run(["openssl", "genpkey", *KEY_ARGS[kind], "-out", path])
    return path


def digest_args(kind, digest):
    return [] if kind == "ed25519" else ["-" + digest]


def make_root(directory, name, kind, digest):
    key = make_key(directory, name, kind)
    cert = os.path.join(directory, name + ".crt")
    run([
        "openssl", "req", "-x509", "-new", "-key", key, "-subj", "/CN=AerOS Test Root " + name,
        "-not_before", NOT_BEFORE, "-not_after", NOT_AFTER, *digest_args(kind, digest),
        "-addext", "basicConstraints=critical,CA:TRUE",
        "-addext", "keyUsage=critical,keyCertSign,cRLSign", "-out", cert,
    ])
    return key, cert


def issue(directory, name, kind, issuer_key, issuer_kind, issuer_cert, digest, extensions):
    key = make_key(directory, name, kind)
    request = os.path.join(directory, name + ".csr")
    run(["openssl", "req", "-new", "-key", key, "-subj", "/CN=" + name, "-out", request])
    config = os.path.join(directory, name + ".cnf")
    open(config, "w").write(extensions)
    cert = os.path.join(directory, name + ".crt")
    run([
        "openssl", "x509", "-req", "-in", request, "-CA", issuer_cert, "-CAkey", issuer_key,
        "-CAcreateserial", "-not_before", NOT_BEFORE, "-not_after", NOT_AFTER,
        *digest_args(issuer_kind, digest), "-extfile", config, "-out", cert,
    ])
    return key, cert


def der(cert):
    return run(["openssl", "x509", "-in", cert, "-outform", "DER"])


def chain(directory, label, root_kind, root_digest, middle, leaf_kind, leaf_digest, middle_ext=INTERMEDIATE):
    """Root, optional intermediate (kind, digest) and leaf. Returns a dict of
    DER and PEM material."""
    root_key, root_cert = make_root(directory, label + "-root", root_kind, root_digest)
    issuer_key, issuer_kind, issuer_cert = root_key, root_kind, root_cert
    middle_der = b""
    leaf_sign_digest = root_digest
    if middle:
        kind, digest = middle
        key, cert = issue(directory, label + "-mid", kind, root_key, root_kind, root_cert, root_digest, middle_ext)
        middle_der = der(cert)
        issuer_key, issuer_kind, issuer_cert = key, kind, cert
        leaf_sign_digest = digest
    leaf_key, leaf_cert = issue(
        directory, label + "-leaf", leaf_kind, issuer_key, issuer_kind, issuer_cert, leaf_sign_digest, LEAF
    )
    return {
        "name": label,
        "root": der(root_cert),
        "middle": middle_der,
        "leaf": der(leaf_cert),
        "intermediate_pem": open(issuer_cert).read() if middle else "",
        "leaf_pem": open(leaf_cert).read(),
        "leaf_key_pem": open(leaf_key).read(),
    }
