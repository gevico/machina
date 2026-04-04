// SPDX-License-Identifier: MIT
// IEEE 754 floating-point addition and subtraction.

use crate::env::{FloatEnv, RoundMode};
use crate::parts::{nan_propagate, round_pack, unpack, FloatClass, FloatParts};
use crate::types::{
    BFloat16, Float128, Float16, Float32, Float64, FloatFormat, FloatX80,
};

/// Floating-point addition: a + b.
pub fn add<F: FloatFormat>(a: F, b: F, env: &mut FloatEnv) -> F {
    let pa = unpack::<F>(a);
    let pb = unpack::<F>(b);
    add_parts::<F>(&pa, &pb, env)
}

/// Floating-point subtraction: a - b.
pub fn sub<F: FloatFormat>(a: F, b: F, env: &mut FloatEnv) -> F {
    let pa = unpack::<F>(a);
    let mut pb = unpack::<F>(b);
    pb.sign = !pb.sign;
    add_parts::<F>(&pa, &pb, env)
}

fn add_parts<F: FloatFormat>(
    a: &FloatParts,
    b: &FloatParts,
    env: &mut FloatEnv,
) -> F {
    // NaN propagation
    if a.is_nan() || b.is_nan() {
        let r = nan_propagate(a, b, env);
        let mut r = r;
        return round_pack::<F>(&mut r, env);
    }

    // Inf handling
    if a.cls == FloatClass::Inf {
        if b.cls == FloatClass::Inf {
            if a.sign != b.sign {
                // Inf + (-Inf) = NaN, INVALID
                return crate::parts::return_nan::<F>(env);
            }
            // Same-sign infinities.
            let mut r = *a;
            return round_pack::<F>(&mut r, env);
        }
        let mut r = *a;
        return round_pack::<F>(&mut r, env);
    }
    if b.cls == FloatClass::Inf {
        let mut r = *b;
        return round_pack::<F>(&mut r, env);
    }

    // Zero handling
    if a.cls == FloatClass::Zero {
        if b.cls == FloatClass::Zero {
            // 0 + 0: sign follows IEEE rules.
            let sign = if a.sign == b.sign {
                a.sign
            } else {
                env.round_mode() == RoundMode::Down
            };
            let mut r = FloatParts {
                sign,
                exp: 0,
                frac: 0,
                cls: FloatClass::Zero,
            };
            return round_pack::<F>(&mut r, env);
        }
        let mut r = *b;
        return round_pack::<F>(&mut r, env);
    }
    if b.cls == FloatClass::Zero {
        let mut r = *a;
        return round_pack::<F>(&mut r, env);
    }

    // Both are Normal.
    if a.sign == b.sign {
        add_normal::<F>(a, b, env)
    } else {
        sub_normal::<F>(a, b, env)
    }
}

/// Add two same-sign normals.
fn add_normal<F: FloatFormat>(
    a: &FloatParts,
    b: &FloatParts,
    env: &mut FloatEnv,
) -> F {
    let (mut big, mut small) = if a.exp >= b.exp { (*a, *b) } else { (*b, *a) };

    let exp_diff = (big.exp - small.exp) as u32;

    // Align the smaller operand's fraction.
    if exp_diff > 0 {
        if exp_diff >= 128 {
            // small is entirely in the sticky region.
            small.frac = if small.frac != 0 { 1 } else { 0 };
        } else {
            let sticky = if small.frac & ((1u128 << exp_diff) - 1) != 0 {
                1u128
            } else {
                0
            };
            small.frac = (small.frac >> exp_diff) | sticky;
        }
    }

    big.frac = big.frac.wrapping_add(small.frac);
    // The addition may overflow the integer bit position;
    // round_pack handles normalization.
    let mut result = FloatParts {
        sign: big.sign,
        exp: big.exp,
        frac: big.frac,
        cls: FloatClass::Normal,
    };
    round_pack::<F>(&mut result, env)
}

/// Subtract two normals with different signs
/// (effective subtraction).
fn sub_normal<F: FloatFormat>(
    a: &FloatParts,
    b: &FloatParts,
    env: &mut FloatEnv,
) -> F {
    // Determine which has larger magnitude.
    let (big, small, result_sign) = if a.exp > b.exp {
        (*a, *b, a.sign)
    } else if a.exp < b.exp {
        (*b, *a, b.sign)
    } else if a.frac > b.frac {
        (*a, *b, a.sign)
    } else if a.frac < b.frac {
        (*b, *a, b.sign)
    } else {
        // Exact cancellation: result is +0 (or -0
        // in round-down mode).
        let sign = env.round_mode() == RoundMode::Down;
        let mut r = FloatParts {
            sign,
            exp: 0,
            frac: 0,
            cls: FloatClass::Zero,
        };
        return round_pack::<F>(&mut r, env);
    };

    let exp_diff = (big.exp - small.exp) as u32;

    let mut small_frac = small.frac;
    if exp_diff > 0 {
        if exp_diff >= 128 {
            small_frac = if small_frac != 0 { 1 } else { 0 };
        } else {
            let sticky = if small_frac & ((1u128 << exp_diff) - 1) != 0 {
                1u128
            } else {
                0
            };
            small_frac = (small_frac >> exp_diff) | sticky;
        }
    }

    let frac = big.frac.wrapping_sub(small_frac);

    if frac == 0 {
        let sign = env.round_mode() == RoundMode::Down;
        let mut r = FloatParts {
            sign,
            exp: 0,
            frac: 0,
            cls: FloatClass::Zero,
        };
        return round_pack::<F>(&mut r, env);
    }

    let mut result = FloatParts {
        sign: result_sign,
        exp: big.exp,
        frac,
        cls: FloatClass::Normal,
    };
    round_pack::<F>(&mut result, env)
}

// ---------------------------------------------------------------
// Convenience methods
// ---------------------------------------------------------------

macro_rules! impl_add_sub {
    ($ty:ty) => {
        impl $ty {
            pub fn add(self, other: Self, env: &mut FloatEnv) -> Self {
                add::<Self>(self, other, env)
            }
            pub fn sub(self, other: Self, env: &mut FloatEnv) -> Self {
                sub::<Self>(self, other, env)
            }
        }
    };
}

impl_add_sub!(Float16);
impl_add_sub!(BFloat16);
impl_add_sub!(Float32);
impl_add_sub!(Float64);
impl_add_sub!(Float128);
impl_add_sub!(FloatX80);
