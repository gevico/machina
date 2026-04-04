// SPDX-License-Identifier: MIT
// IEEE 754 floating-point division.

use crate::env::{ExcFlags, FloatEnv};
use crate::parts::{
    nan_propagate, return_nan, round_pack, unpack, FloatClass, FloatParts,
};
use crate::types::{
    BFloat16, Float128, Float16, Float32, Float64, FloatFormat, FloatX80,
};

/// Floating-point division: a / b.
pub fn div<F: FloatFormat>(a: F, b: F, env: &mut FloatEnv) -> F {
    let pa = unpack::<F>(a);
    let pb = unpack::<F>(b);
    div_parts::<F>(&pa, &pb, env)
}

fn div_parts<F: FloatFormat>(
    a: &FloatParts,
    b: &FloatParts,
    env: &mut FloatEnv,
) -> F {
    let result_sign = a.sign ^ b.sign;

    // NaN propagation
    if a.is_nan() || b.is_nan() {
        let mut r = nan_propagate(a, b, env);
        r.sign = result_sign;
        return round_pack::<F>(&mut r, env);
    }

    // Inf / Inf = NaN (INVALID)
    if a.cls == FloatClass::Inf {
        if b.cls == FloatClass::Inf {
            return return_nan::<F>(env);
        }
        let mut r = FloatParts {
            sign: result_sign,
            exp: 0,
            frac: 0,
            cls: FloatClass::Inf,
        };
        return round_pack::<F>(&mut r, env);
    }

    // x / Inf = 0
    if b.cls == FloatClass::Inf {
        let mut r = FloatParts {
            sign: result_sign,
            exp: 0,
            frac: 0,
            cls: FloatClass::Zero,
        };
        return round_pack::<F>(&mut r, env);
    }

    // 0 / 0 = NaN (INVALID)
    if a.cls == FloatClass::Zero {
        if b.cls == FloatClass::Zero {
            return return_nan::<F>(env);
        }
        let mut r = FloatParts {
            sign: result_sign,
            exp: 0,
            frac: 0,
            cls: FloatClass::Zero,
        };
        return round_pack::<F>(&mut r, env);
    }

    // x / 0 = Inf (DIVBYZERO)
    if b.cls == FloatClass::Zero {
        env.raise(ExcFlags::DIVBYZERO);
        let mut r = FloatParts {
            sign: result_sign,
            exp: 0,
            frac: 0,
            cls: FloatClass::Inf,
        };
        return round_pack::<F>(&mut r, env);
    }

    // Both normal: divide fractions, subtract exponents.
    let exp = a.exp - b.exp;

    // We need a.frac / b.frac with enough precision.
    // Both fracs have integer bit at position 126.
    //
    // Strategy: use iterative long division via shifting.
    // We want ~128 bits of quotient. Since both operands
    // are ~127-bit numbers (bit 126 set), the quotient
    // is close to 1.xxx (at most 2.xxx if a.frac >= b.frac
    // before any normalization).
    //
    // Shift the dividend left to get more quotient bits.
    // dividend = a.frac, divisor = b.frac.
    // If a.frac >= b.frac, quotient integer bit is 1 and
    // exp stays. Otherwise, shift dividend left by 1 and
    // decrement exp.

    let (frac, exp) = div_frac(a.frac, b.frac, exp);

    let mut result = FloatParts {
        sign: result_sign,
        exp,
        frac,
        cls: FloatClass::Normal,
    };
    round_pack::<F>(&mut result, env)
}

/// Divide two u128 fractions (integer bit at position 126).
/// Returns (quotient_frac, adjusted_exp).
///
/// Algorithm: ensure dividend >= divisor (adjust exp),
/// then iterative trial subtraction to produce ~128 bits
/// of quotient with integer bit at position 126.
fn div_frac(a_frac: u128, b_frac: u128, mut exp: i32) -> (u128, i32) {
    if b_frac == 0 {
        return (0, exp);
    }

    let mut rem = a_frac;

    // If a_frac < b_frac, shift numerator left by 1
    // and decrement exponent so quotient >= 1.
    if rem < b_frac {
        rem <<= 1;
        exp -= 1;
    }

    // Now rem >= b_frac. The integer part of the
    // quotient is 1. Produce 127 fractional bits
    // via trial subtraction (total: 128 bits with
    // integer bit at position 127).
    let mut q: u128 = 0;
    for _ in 0..128 {
        q <<= 1;
        if rem >= b_frac {
            q |= 1;
            rem -= b_frac;
        }
        rem <<= 1;
    }

    // Sticky bit from remaining remainder.
    if rem != 0 {
        q |= 1;
    }

    // q has integer bit at position 127. Shift right
    // by 1 to place it at position 126 (our convention).
    let sticky = q & 1;
    let frac = (q >> 1) | sticky;

    (frac, exp)
}

// ---------------------------------------------------------------
// Convenience methods
// ---------------------------------------------------------------

macro_rules! impl_div {
    ($ty:ty) => {
        impl $ty {
            pub fn div(self, other: Self, env: &mut FloatEnv) -> Self {
                div::<Self>(self, other, env)
            }
        }
    };
}

impl_div!(Float16);
impl_div!(BFloat16);
impl_div!(Float32);
impl_div!(Float64);
impl_div!(Float128);
impl_div!(FloatX80);
