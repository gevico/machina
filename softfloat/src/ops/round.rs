// SPDX-License-Identifier: MIT
// IEEE 754 round-to-integer (in floating-point format).

use crate::env::{ExcFlags, FloatEnv, RoundMode};
use crate::parts::{
    nan_propagate_one, round_pack, unpack, FloatClass, FloatParts,
};
use crate::types::{
    BFloat16, Float128, Float16, Float32, Float64, FloatFormat, FloatX80,
};

const INT_BIT: u32 = 126;

/// Round a floating-point number to an integer value,
/// returning the result in the same float format.
pub fn round_to_int<F: FloatFormat>(a: F, env: &mut FloatEnv) -> F {
    let pa = unpack::<F>(a);

    if pa.is_nan() {
        let mut r = nan_propagate_one(&pa, env);
        return round_pack::<F>(&mut r, env);
    }

    if pa.cls == FloatClass::Inf || pa.cls == FloatClass::Zero {
        let mut r = pa;
        return round_pack::<F>(&mut r, env);
    }

    // If exponent is large enough that all fraction bits
    // are integer bits, the number is already integral.
    if pa.exp >= F::FRAC_BITS as i32 {
        let mut r = pa;
        return round_pack::<F>(&mut r, env);
    }

    // If exponent is very negative, the magnitude is < 1.
    if pa.exp < 0 {
        let rm = env.round_mode();
        let inexact = pa.frac != 0;
        if inexact {
            env.raise(ExcFlags::INEXACT);
        }

        let round_up = match rm {
            RoundMode::NearEven => {
                // Round to even: 0.5 exactly rounds to 0
                // (even), anything above 0.5 rounds to 1.
                pa.exp == -1 && pa.frac > (1u128 << INT_BIT)
            }
            RoundMode::NearMaxMag => pa.exp == -1,
            RoundMode::ToZero => false,
            RoundMode::Up => !pa.sign && inexact,
            RoundMode::Down => pa.sign && inexact,
            RoundMode::Odd => true,
        };

        if round_up {
            // Return +-1.0
            let mut r = FloatParts {
                sign: pa.sign,
                exp: 0,
                frac: 1u128 << INT_BIT,
                cls: FloatClass::Normal,
            };
            return round_pack::<F>(&mut r, env);
        }

        // Return +-0
        let mut r = FloatParts {
            sign: pa.sign,
            exp: 0,
            frac: 0,
            cls: FloatClass::Zero,
        };
        return round_pack::<F>(&mut r, env);
    }

    // 0 <= exp < FRAC_BITS: some fractional bits exist.
    // Mask off the fractional bits and round.
    let frac_shift = INT_BIT - pa.exp as u32;
    let frac_mask = (1u128 << frac_shift) - 1;
    let remainder = pa.frac & frac_mask;

    if remainder == 0 {
        // Already an integer.
        let mut r = pa;
        return round_pack::<F>(&mut r, env);
    }

    env.raise(ExcFlags::INEXACT);

    let rm = env.round_mode();
    let half = 1u128 << (frac_shift - 1);
    let truncated = pa.frac & !frac_mask;
    let lsb_set = (pa.frac >> frac_shift) & 1 != 0;
    let increment = 1u128 << frac_shift;

    let round_up = match rm {
        RoundMode::NearEven => {
            if remainder > half {
                true
            } else if remainder == half {
                lsb_set
            } else {
                false
            }
        }
        RoundMode::NearMaxMag => remainder >= half,
        RoundMode::ToZero => false,
        RoundMode::Up => !pa.sign,
        RoundMode::Down => pa.sign,
        RoundMode::Odd => {
            // Set LSB to 1 if inexact.
            !lsb_set
        }
    };

    let frac = if round_up {
        truncated.wrapping_add(increment)
    } else {
        truncated
    };

    let mut r = FloatParts {
        sign: pa.sign,
        exp: pa.exp,
        frac,
        cls: FloatClass::Normal,
    };
    round_pack::<F>(&mut r, env)
}

// ---------------------------------------------------------------
// Convenience methods
// ---------------------------------------------------------------

macro_rules! impl_round {
    ($ty:ty) => {
        impl $ty {
            pub fn round_to_int(self, env: &mut FloatEnv) -> Self {
                round_to_int::<Self>(self, env)
            }
        }
    };
}

impl_round!(Float16);
impl_round!(BFloat16);
impl_round!(Float32);
impl_round!(Float64);
impl_round!(Float128);
impl_round!(FloatX80);
