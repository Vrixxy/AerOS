//! X25519 (RFC 7748) on the Ed25519 field arithmetic. The ladder swaps with
//! masks, so the secret scalar does not steer a branch or an index.

use crate::ed25519::{Gf, fadd, invert, mul, pack25519, select, square, sub, unpack25519};

const BASE_POINT: [u8; 32] = {
    let mut point = [0u8; 32];
    point[0] = 9;
    point
};

const A24: Gf = [0xdb41, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

pub fn public_key(secret: &[u8; 32]) -> [u8; 32] {
    scalar_mult(secret, &BASE_POINT)
}

/// The shared secret, or `None` for a low-order peer key (all-zero output).
pub fn shared_secret(secret: &[u8; 32], peer: &[u8; 32]) -> Option<[u8; 32]> {
    let out = scalar_mult(secret, peer);
    let mut folded = 0u8;
    for byte in out {
        folded |= byte;
    }
    (folded != 0).then_some(out)
}

pub fn scalar_mult(scalar: &[u8; 32], point: &[u8; 32]) -> [u8; 32] {
    let mut z = *scalar;
    z[31] = (scalar[31] & 127) | 64;
    z[0] &= 248;
    let x = unpack25519(point);
    let mut a: Gf = [0; 16];
    let mut b = x;
    let mut c: Gf = [0; 16];
    let mut d: Gf = [0; 16];
    a[0] = 1;
    d[0] = 1;
    for i in (0..=254).rev() {
        let bit = i64::from((z[i >> 3] >> (i & 7)) & 1);
        select(&mut a, &mut b, bit);
        select(&mut c, &mut d, bit);
        let mut e = fadd(&a, &c);
        a = sub(&a, &c);
        c = fadd(&b, &d);
        b = sub(&b, &d);
        d = square(&e);
        let f = square(&a);
        a = mul(&c, &a);
        c = mul(&b, &e);
        e = fadd(&a, &c);
        a = sub(&a, &c);
        b = square(&a);
        c = sub(&d, &f);
        a = mul(&c, &A24);
        a = fadd(&a, &d);
        c = mul(&c, &a);
        a = mul(&d, &f);
        d = mul(&b, &x);
        b = square(&e);
        select(&mut a, &mut b, bit);
        select(&mut c, &mut d, bit);
    }
    let inverse = invert(&c);
    pack25519(&mul(&a, &inverse))
}
