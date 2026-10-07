//! ECDSA verification on P-256 and P-384 (FIPS 186-4), in Jacobian
//! coordinates over Montgomery field arithmetic. Public data only.

use core::cmp::Ordering;

use super::bignum::{Big, Mont, from_hex};
use super::der;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Curve {
    P256,
    P384,
}

struct Params<const N: usize> {
    p: Big<N>,
    b: Big<N>,
    gx: Big<N>,
    gy: Big<N>,
    n: Big<N>,
    bytes: usize,
}

const P256: Params<4> = Params {
    p: from_hex("ffffffff00000001000000000000000000000000ffffffffffffffffffffffff"),
    b: from_hex("5ac635d8aa3a93e7b3ebbd55769886bc651d06b0cc53b0f63bce3c3e27d2604b"),
    gx: from_hex("6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296"),
    gy: from_hex("4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5"),
    n: from_hex("ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551"),
    bytes: 32,
};

const P384: Params<6> = Params {
    p: from_hex(
        "fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffeffffffff0000000000000000ffffffff",
    ),
    b: from_hex(
        "b3312fa7e23ee7e4988e056be3f82d19181d9c6efe8141120314088f5013875ac656398d8a2ed19d2a85c8edd3ec2aef",
    ),
    gx: from_hex(
        "aa87ca22be8b05378eb1c71ef320ad746e1d3b628ba79b9859f741e082542a385502f25dbf55296c3a545e3872760ab7",
    ),
    gy: from_hex(
        "3617de4a96262c6f5d9e98bf9292dc29f8f41dbd289a147ce9da3113b5f0b8c00a60b1ce1d7e819d7a431d7c90ea0e5f",
    ),
    n: from_hex(
        "ffffffffffffffffffffffffffffffffffffffffffffffffc7634d81f4372ddf581a0db248b0a77aecec196accc52973",
    ),
    bytes: 48,
};

#[derive(Clone, Copy)]
struct Point<const N: usize> {
    x: Big<N>,
    y: Big<N>,
    z: Big<N>,
}

impl<const N: usize> Point<N> {
    const INFINITY: Self = Self {
        x: Big::ZERO,
        y: Big::ZERO,
        z: Big::ZERO,
    };

    fn is_infinity(&self) -> bool {
        self.z.is_zero()
    }
}

fn double<const N: usize>(f: &Mont<N>, p: &Point<N>) -> Point<N> {
    if p.is_infinity() || p.y.is_zero() {
        return Point::INFINITY;
    }
    let delta = f.mul(&p.z, &p.z);
    let gamma = f.mul(&p.y, &p.y);
    let beta = f.mul(&p.x, &gamma);
    let t = f.mul(&f.sub(&p.x, &delta), &f.add(&p.x, &delta));
    let alpha = f.add(&f.add(&t, &t), &t);
    let beta2 = f.add(&beta, &beta);
    let beta4 = f.add(&beta2, &beta2);
    let beta8 = f.add(&beta4, &beta4);
    let x3 = f.sub(&f.mul(&alpha, &alpha), &beta8);
    let yz = f.add(&p.y, &p.z);
    let z3 = f.sub(&f.sub(&f.mul(&yz, &yz), &gamma), &delta);
    let gamma_sq = f.mul(&gamma, &gamma);
    let g2 = f.add(&gamma_sq, &gamma_sq);
    let g4 = f.add(&g2, &g2);
    let g8 = f.add(&g4, &g4);
    let y3 = f.sub(&f.mul(&alpha, &f.sub(&beta4, &x3)), &g8);
    Point {
        x: x3,
        y: y3,
        z: z3,
    }
}

fn add<const N: usize>(f: &Mont<N>, p: &Point<N>, q: &Point<N>) -> Point<N> {
    if p.is_infinity() {
        return *q;
    }
    if q.is_infinity() {
        return *p;
    }
    let z1z1 = f.mul(&p.z, &p.z);
    let z2z2 = f.mul(&q.z, &q.z);
    let u1 = f.mul(&p.x, &z2z2);
    let u2 = f.mul(&q.x, &z1z1);
    let s1 = f.mul(&f.mul(&p.y, &q.z), &z2z2);
    let s2 = f.mul(&f.mul(&q.y, &p.z), &z1z1);
    let h = f.sub(&u2, &u1);
    let r = f.sub(&s2, &s1);
    if h.is_zero() {
        return if r.is_zero() {
            double(f, p)
        } else {
            Point::INFINITY
        };
    }
    let r = f.add(&r, &r);
    let h2 = f.add(&h, &h);
    let i = f.mul(&h2, &h2);
    let j = f.mul(&h, &i);
    let v = f.mul(&u1, &i);
    let v2 = f.add(&v, &v);
    let x3 = f.sub(&f.sub(&f.mul(&r, &r), &j), &v2);
    let s1j = f.mul(&s1, &j);
    let y3 = f.sub(&f.mul(&r, &f.sub(&v, &x3)), &f.add(&s1j, &s1j));
    let zs = f.add(&p.z, &q.z);
    let z3 = f.mul(&f.sub(&f.sub(&f.mul(&zs, &zs), &z1z1), &z2z2), &h);
    Point {
        x: x3,
        y: y3,
        z: z3,
    }
}

fn multiply<const N: usize>(f: &Mont<N>, point: &Point<N>, scalar: &Big<N>) -> Point<N> {
    let mut result = Point::INFINITY;
    for index in (0..scalar.bit_len()).rev() {
        result = double(f, &result);
        if scalar.bit(index) {
            result = add(f, &result, point);
        }
    }
    result
}

fn verify_generic<const N: usize>(
    curve: &Params<N>,
    key: &[u8],
    digest: &[u8],
    r: &[u8],
    s: &[u8],
) -> bool {
    if key.len() != 1 + 2 * curve.bytes || key[0] != 4 {
        return false;
    }
    let (Some(qx), Some(qy)) = (
        Big::<N>::from_be_bytes(&key[1..1 + curve.bytes]),
        Big::<N>::from_be_bytes(&key[1 + curve.bytes..]),
    ) else {
        return false;
    };
    let (Some(r), Some(s)) = (Big::<N>::from_be_bytes(r), Big::<N>::from_be_bytes(s)) else {
        return false;
    };
    let (Some(field), Some(order)) = (Mont::new(curve.p), Mont::new(curve.n)) else {
        return false;
    };
    if qx.compare(&curve.p) != Ordering::Less
        || qy.compare(&curve.p) != Ordering::Less
        || r.is_zero()
        || s.is_zero()
        || r.compare(&curve.n) != Ordering::Less
        || s.compare(&curve.n) != Ordering::Less
    {
        return false;
    }
    let qx = field.to_mont(&qx);
    let qy = field.to_mont(&qy);
    let three_x = {
        let double_x = field.add(&qx, &qx);
        field.add(&double_x, &qx)
    };
    let right = field.add(
        &field.sub(&field.mul(&field.mul(&qx, &qx), &qx), &three_x),
        &field.to_mont(&curve.b),
    );
    if field.mul(&qy, &qy) != right {
        return false;
    }

    let take = digest.len().min(curve.bytes);
    let Some(mut z) = Big::<N>::from_be_bytes(&digest[..take]) else {
        return false;
    };
    if z.compare(&curve.n) != Ordering::Less {
        z.sub_in_place(&curve.n);
    }
    let mut exponent = curve.n;
    exponent.sub_in_place(&Big::from_u64(2));
    let w = order.pow(&order.to_mont(&s), &exponent);
    let u1 = order.to_plain(&order.mul(&order.to_mont(&z), &w));
    let u2 = order.to_plain(&order.mul(&order.to_mont(&r), &w));

    let one = field.one();
    let generator = Point {
        x: field.to_mont(&curve.gx),
        y: field.to_mont(&curve.gy),
        z: one,
    };
    let public = Point {
        x: qx,
        y: qy,
        z: one,
    };
    let sum = add(
        &field,
        &multiply(&field, &generator, &u1),
        &multiply(&field, &public, &u2),
    );
    if sum.is_infinity() {
        return false;
    }
    let mut p_minus_2 = curve.p;
    p_minus_2.sub_in_place(&Big::from_u64(2));
    let z_inverse = field.pow(&sum.z, &p_minus_2);
    let mut x = field.to_plain(&field.mul(&sum.x, &field.mul(&z_inverse, &z_inverse)));
    if x.compare(&curve.n) != Ordering::Less {
        x.sub_in_place(&curve.n);
    }
    x == r
}

/// `key` is an uncompressed point, `digest` the hash of the signed data and
/// `signature` a DER ECDSA-Sig-Value.
pub fn verify(curve: Curve, key: &[u8], digest: &[u8], signature: &[u8]) -> bool {
    let Some((r, s)) = der::ecdsa_signature(signature) else {
        return false;
    };
    match curve {
        Curve::P256 => verify_generic(&P256, key, digest, r, s),
        Curve::P384 => verify_generic(&P384, key, digest, r, s),
    }
}
