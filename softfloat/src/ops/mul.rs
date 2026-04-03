// SPDX-License-Identifier: MIT
// IEEE 754 floating-point multiplication.

use crate::env::FloatEnv;
use crate::parts::{
    nan_propagate, return_nan, round_pack, unpack, FloatClass, FloatParts,
};
use crate::types::{
    BFloat16, Float128, Float16, Float32, Float64, FloatFormat, FloatX80,
};

/// Floating-point multiplication: a * b.
pub fn mul<F: FloatFormat>(a: F, b: F, env: &mut FloatEnv) -> F {
    let pa = unpack::<F>(a);
    let pb = unpack::<F>(b);
    mul_parts::<F>(&pa, &pb, env)
}

fn mul_parts<F: FloatFormat>(
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

    // Inf * 0 = NaN (INVALID)
    if a.cls == FloatClass::Inf {
        if b.cls == FloatClass::Zero {
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
    if b.cls == FloatClass::Inf {
        if a.cls == FloatClass::Zero {
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

    // Zero handling
    if a.cls == FloatClass::Zero || b.cls == FloatClass::Zero {
        let mut r = FloatParts {
            sign: result_sign,
            exp: 0,
            frac: 0,
            cls: FloatClass::Zero,
        };
        return round_pack::<F>(&mut r, env);
    }

    // Both normal: multiply fractions and add exponents.
    let exp = a.exp + b.exp;

    // u128 * u128 -> upper 128 bits.
    // Our fracs have the integer bit at position 126.
    // Product's integer bits would be at position 252
    // in a 256-bit result. We need the upper 128 bits.
    let (hi, lo) = mul_u128(a.frac, b.frac);

    // The product has the integer-bit pair at bit
    // 252 of the 256-bit result = bit 124 of `hi`.
    // We need to left-align so the integer bit is at 126.
    // Shift left by 2, but carry in bits from lo.
    let frac = (hi << 2) | (lo >> 126);
    let sticky = if lo & ((1u128 << 126) - 1) != 0 { 1 } else { 0 };
    let frac = frac | sticky;

    let mut result = FloatParts {
        sign: result_sign,
        exp,
        frac,
        cls: FloatClass::Normal,
    };
    round_pack::<F>(&mut result, env)
}

/// Multiply two u128 values, return (hi, lo) of the 256-bit
/// result. Uses 64-bit half decomposition.
fn mul_u128(a: u128, b: u128) -> (u128, u128) {
    let a_lo = a as u64 as u128;
    let a_hi = (a >> 64) as u64 as u128;
    let b_lo = b as u64 as u128;
    let b_hi = (b >> 64) as u64 as u128;

    let ll = a_lo * b_lo;
    let lh = a_lo * b_hi;
    let hl = a_hi * b_lo;
    let hh = a_hi * b_hi;

    // Accumulate the middle terms.
    let mid = (ll >> 64)
        + (lh & 0xFFFF_FFFF_FFFF_FFFF)
        + (hl & 0xFFFF_FFFF_FFFF_FFFF);

    let lo =
        (ll & 0xFFFF_FFFF_FFFF_FFFF) | ((mid & 0xFFFF_FFFF_FFFF_FFFF) << 64);
    let hi = hh + (lh >> 64) + (hl >> 64) + (mid >> 64);

    (hi, lo)
}

// ---------------------------------------------------------------
// Convenience methods
// ---------------------------------------------------------------

macro_rules! impl_mul {
    ($ty:ty) => {
        impl $ty {
            pub fn mul(self, other: Self, env: &mut FloatEnv) -> Self {
                mul::<Self>(self, other, env)
            }
        }
    };
}

impl_mul!(Float16);
impl_mul!(BFloat16);
impl_mul!(Float32);
impl_mul!(Float64);
impl_mul!(Float128);
impl_mul!(FloatX80);
