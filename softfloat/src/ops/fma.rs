// SPDX-License-Identifier: MIT
// IEEE 754 fused multiply-add: a*b + c with single rounding.

use crate::env::{ExcFlags, FloatEnv, RoundMode};
use crate::parts::{return_nan, round_pack, unpack, FloatClass, FloatParts};
use crate::types::{
    BFloat16, Float128, Float16, Float32, Float64, FloatFormat, FloatX80,
};

const INT_BIT: u32 = 126;

/// Fused multiply-add: a * b + c with a single rounding step.
pub fn fma<F: FloatFormat>(a: F, b: F, c: F, env: &mut FloatEnv) -> F {
    let pa = unpack::<F>(a);
    let pb = unpack::<F>(b);
    let pc = unpack::<F>(c);
    fma_parts::<F>(&pa, &pb, &pc, env)
}

fn fma_parts<F: FloatFormat>(
    a: &FloatParts,
    b: &FloatParts,
    c: &FloatParts,
    env: &mut FloatEnv,
) -> F {
    let ab_sign = a.sign ^ b.sign;

    // NaN propagation: check all three operands.
    if a.is_nan() || b.is_nan() || c.is_nan() {
        // 3-operand NaN propagation.
        if a.cls == FloatClass::SNaN
            || b.cls == FloatClass::SNaN
            || c.cls == FloatClass::SNaN
        {
            env.raise(ExcFlags::INVALID);
        }
        // Also check for Inf * 0 (INVALID even with NaN c).
        if (a.cls == FloatClass::Inf && b.cls == FloatClass::Zero)
            || (a.cls == FloatClass::Zero && b.cls == FloatClass::Inf)
        {
            env.raise(ExcFlags::INVALID);
        }
        // Pick a NaN to propagate.
        let mut r = if a.is_nan() {
            *a
        } else if b.is_nan() {
            *b
        } else {
            *c
        };
        if r.cls == FloatClass::SNaN {
            r.cls = FloatClass::QNaN;
            r.frac |= 1u128 << (INT_BIT - 1);
        }
        if env.default_nan() {
            r = FloatParts::default_nan::<F>();
        }
        return round_pack::<F>(&mut r, env);
    }

    // Inf * 0 + c = NaN (INVALID)
    if (a.cls == FloatClass::Inf && b.cls == FloatClass::Zero)
        || (a.cls == FloatClass::Zero && b.cls == FloatClass::Inf)
    {
        return return_nan::<F>(env);
    }

    // Inf * x + c
    if a.cls == FloatClass::Inf || b.cls == FloatClass::Inf {
        if c.cls == FloatClass::Inf && c.sign != ab_sign {
            // Inf + (-Inf) = NaN
            return return_nan::<F>(env);
        }
        let mut r = FloatParts {
            sign: ab_sign,
            exp: 0,
            frac: 0,
            cls: FloatClass::Inf,
        };
        return round_pack::<F>(&mut r, env);
    }

    // a*b is finite. If c is Inf, result is c.
    if c.cls == FloatClass::Inf {
        let mut r = *c;
        return round_pack::<F>(&mut r, env);
    }

    // a*b when one of a,b is zero.
    let ab_zero = a.cls == FloatClass::Zero || b.cls == FloatClass::Zero;

    if ab_zero {
        if c.cls == FloatClass::Zero {
            let sign = if ab_sign == c.sign {
                ab_sign
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
        let mut r = *c;
        return round_pack::<F>(&mut r, env);
    }

    // Both a,b are normal. Compute product a*b.
    let p_exp = a.exp + b.exp;

    // u128 * u128 -> (hi, lo) 256-bit product.
    let (p_hi, p_lo) = mul_u128(a.frac, b.frac);

    // Product integer bits are at position 252 of the
    // 256-bit result = bit 124 of p_hi. Shift so integer
    // bit is at INT_BIT (126) of p_hi.
    let p_frac = (p_hi << 2) | (p_lo >> 126);
    let p_sticky = if p_lo & ((1u128 << 126) - 1) != 0 {
        1
    } else {
        0
    };
    let p_frac = p_frac | p_sticky;

    if c.cls == FloatClass::Zero {
        let mut r = FloatParts {
            sign: ab_sign,
            exp: p_exp,
            frac: p_frac,
            cls: FloatClass::Normal,
        };
        return round_pack::<F>(&mut r, env);
    }

    // Add the product to c.
    let product = FloatParts {
        sign: ab_sign,
        exp: p_exp,
        frac: p_frac,
        cls: FloatClass::Normal,
    };

    // Reuse the addition logic.
    add_parts_fma::<F>(&product, c, env)
}

/// Addition for FMA (called with pre-computed product).
fn add_parts_fma<F: FloatFormat>(
    a: &FloatParts,
    b: &FloatParts,
    env: &mut FloatEnv,
) -> F {
    if a.sign == b.sign {
        // Same sign: add magnitudes.
        let (mut big, mut small) =
            if a.exp >= b.exp { (*a, *b) } else { (*b, *a) };
        let exp_diff = (big.exp - small.exp) as u32;
        if exp_diff > 0 {
            if exp_diff >= 128 {
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
        let mut r = FloatParts {
            sign: big.sign,
            exp: big.exp,
            frac: big.frac,
            cls: FloatClass::Normal,
        };
        round_pack::<F>(&mut r, env)
    } else {
        // Different signs: subtract.
        let (big, small, sign) = if a.exp > b.exp {
            (*a, *b, a.sign)
        } else if a.exp < b.exp {
            (*b, *a, b.sign)
        } else if a.frac > b.frac {
            (*a, *b, a.sign)
        } else if a.frac < b.frac {
            (*b, *a, b.sign)
        } else {
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

        let mut r = FloatParts {
            sign,
            exp: big.exp,
            frac,
            cls: FloatClass::Normal,
        };
        round_pack::<F>(&mut r, env)
    }
}

fn mul_u128(a: u128, b: u128) -> (u128, u128) {
    let a_lo = a as u64 as u128;
    let a_hi = (a >> 64) as u64 as u128;
    let b_lo = b as u64 as u128;
    let b_hi = (b >> 64) as u64 as u128;

    let ll = a_lo * b_lo;
    let lh = a_lo * b_hi;
    let hl = a_hi * b_lo;
    let hh = a_hi * b_hi;

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

macro_rules! impl_fma {
    ($ty:ty) => {
        impl $ty {
            pub fn fma(self, b: Self, c: Self, env: &mut FloatEnv) -> Self {
                fma::<Self>(self, b, c, env)
            }
        }
    };
}

impl_fma!(Float16);
impl_fma!(BFloat16);
impl_fma!(Float32);
impl_fma!(Float64);
impl_fma!(Float128);
impl_fma!(FloatX80);
