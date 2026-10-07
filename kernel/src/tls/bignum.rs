//! Fixed-size unsigned integers and Montgomery arithmetic for checking RSA
//! and ECDSA signatures. Everything it handles is public (keys, signatures,
//! hashes), so nothing here is constant-time.

use core::cmp::Ordering;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Big<const N: usize>(pub [u64; N]);

pub const fn from_hex<const N: usize>(text: &str) -> Big<N> {
    let bytes = text.as_bytes();
    let mut limbs = [0u64; N];
    let mut nibble = 0;
    let mut at = bytes.len();
    while at > 0 {
        at -= 1;
        let digit = match bytes[at] {
            c @ b'0'..=b'9' => c - b'0',
            c @ b'a'..=b'f' => c - b'a' + 10,
            _ => panic!("not a hex digit"),
        } as u64;
        limbs[nibble / 16] |= digit << ((nibble % 16) * 4);
        nibble += 1;
    }
    Big(limbs)
}

impl<const N: usize> Big<N> {
    pub const ZERO: Self = Self([0; N]);

    pub const fn from_u64(value: u64) -> Self {
        let mut limbs = [0u64; N];
        limbs[0] = value;
        Self(limbs)
    }

    /// `None` when the value does not fit in `N` limbs.
    pub fn from_be_bytes(bytes: &[u8]) -> Option<Self> {
        let mut limbs = [0u64; N];
        for (index, byte) in bytes.iter().rev().enumerate() {
            if index / 8 >= N {
                if *byte != 0 {
                    return None;
                }
                continue;
            }
            limbs[index / 8] |= u64::from(*byte) << ((index % 8) * 8);
        }
        Some(Self(limbs))
    }

    /// Writes the low `out.len()` bytes, big-endian.
    pub fn write_be(&self, out: &mut [u8]) {
        for (index, slot) in out.iter_mut().rev().enumerate() {
            *slot = if index / 8 < N {
                (self.0[index / 8] >> ((index % 8) * 8)) as u8
            } else {
                0
            };
        }
    }

    pub fn is_zero(&self) -> bool {
        self.0.iter().all(|limb| *limb == 0)
    }

    pub fn bit(&self, index: usize) -> bool {
        (self.0[index / 64] >> (index % 64)) & 1 == 1
    }

    pub fn bit_len(&self) -> usize {
        for index in (0..N).rev() {
            if self.0[index] != 0 {
                return index * 64 + 64 - self.0[index].leading_zeros() as usize;
            }
        }
        0
    }

    pub fn compare(&self, other: &Self) -> Ordering {
        for index in (0..N).rev() {
            if self.0[index] != other.0[index] {
                return self.0[index].cmp(&other.0[index]);
            }
        }
        Ordering::Equal
    }

    /// Adds `other`; returns the carry out.
    pub fn add_in_place(&mut self, other: &Self) -> bool {
        let mut carry = false;
        for index in 0..N {
            let (sum, first) = self.0[index].overflowing_add(other.0[index]);
            let (sum, second) = sum.overflowing_add(u64::from(carry));
            self.0[index] = sum;
            carry = first || second;
        }
        carry
    }

    /// Subtracts `other`; returns the borrow out.
    pub fn sub_in_place(&mut self, other: &Self) -> bool {
        let mut borrow = false;
        for index in 0..N {
            let (diff, first) = self.0[index].overflowing_sub(other.0[index]);
            let (diff, second) = diff.overflowing_sub(u64::from(borrow));
            self.0[index] = diff;
            borrow = first || second;
        }
        borrow
    }
}

/// Arithmetic modulo an odd `n`, with values kept in Montgomery form
/// (`a * 2^(64N) mod n`).
pub struct Mont<const N: usize> {
    pub n: Big<N>,
    n0_inverse: u64,
    r_squared: Big<N>,
}

impl<const N: usize> Mont<N> {
    pub fn new(n: Big<N>) -> Option<Self> {
        if n.0[0] & 1 == 0 || n.bit_len() < 2 {
            return None;
        }
        let mut inverse = n.0[0];
        for _ in 0..6 {
            inverse = inverse.wrapping_mul(2u64.wrapping_sub(n.0[0].wrapping_mul(inverse)));
        }
        let mut context = Self {
            n,
            n0_inverse: inverse.wrapping_neg(),
            r_squared: Big::ZERO,
        };
        let mut value = Big::<N>::from_u64(1);
        for _ in 0..128 * N {
            value = context.add(&value, &value);
        }
        context.r_squared = value;
        Some(context)
    }

    pub fn add(&self, a: &Big<N>, b: &Big<N>) -> Big<N> {
        let mut sum = *a;
        let carry = sum.add_in_place(b);
        if carry || sum.compare(&self.n) != Ordering::Less {
            sum.sub_in_place(&self.n);
        }
        sum
    }

    pub fn sub(&self, a: &Big<N>, b: &Big<N>) -> Big<N> {
        let mut difference = *a;
        if difference.sub_in_place(b) {
            difference.add_in_place(&self.n);
        }
        difference
    }

    pub fn mul(&self, a: &Big<N>, b: &Big<N>) -> Big<N> {
        let mut t = [0u64; 66];
        for i in 0..N {
            let mut carry = 0u64;
            for (slot, limb) in t.iter_mut().zip(a.0.iter()) {
                let value =
                    u128::from(*limb) * u128::from(b.0[i]) + u128::from(*slot) + u128::from(carry);
                *slot = value as u64;
                carry = (value >> 64) as u64;
            }
            let value = u128::from(t[N]) + u128::from(carry);
            t[N] = value as u64;
            t[N + 1] = (value >> 64) as u64;

            let m = t[0].wrapping_mul(self.n0_inverse);
            let value = u128::from(m) * u128::from(self.n.0[0]) + u128::from(t[0]);
            let mut carry = (value >> 64) as u64;
            for j in 1..N {
                let value = u128::from(m) * u128::from(self.n.0[j])
                    + u128::from(t[j])
                    + u128::from(carry);
                t[j - 1] = value as u64;
                carry = (value >> 64) as u64;
            }
            let value = u128::from(t[N]) + u128::from(carry);
            t[N - 1] = value as u64;
            t[N] = t[N + 1] + (value >> 64) as u64;
            t[N + 1] = 0;
        }
        let mut result = Big([0u64; N]);
        result.0.copy_from_slice(&t[..N]);
        if t[N] != 0 || result.compare(&self.n) != Ordering::Less {
            result.sub_in_place(&self.n);
        }
        result
    }

    /// `a` must be below the modulus.
    pub fn to_mont(&self, a: &Big<N>) -> Big<N> {
        self.mul(a, &self.r_squared)
    }

    pub fn to_plain(&self, a: &Big<N>) -> Big<N> {
        self.mul(a, &Big::from_u64(1))
    }

    pub fn one(&self) -> Big<N> {
        self.to_mont(&Big::from_u64(1))
    }

    /// `base` in Montgomery form, `exponent` plain; the result is in
    /// Montgomery form.
    pub fn pow(&self, base: &Big<N>, exponent: &Big<N>) -> Big<N> {
        let mut result = self.one();
        for index in (0..exponent.bit_len()).rev() {
            result = self.mul(&result, &result);
            if exponent.bit(index) {
                result = self.mul(&result, base);
            }
        }
        result
    }
}
