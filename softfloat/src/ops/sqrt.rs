// SPDX-License-Identifier: MIT
// IEEE 754 floating-point square root.

use crate::env::FloatEnv;
use crate::parts::{
    nan_propagate_one, return_nan, round_pack, unpack, FloatClass, FloatParts,
};
use crate::types::{
    BFloat16, Float128, Float16, Float32, Float64, FloatFormat, FloatX80,
};

const INT_BIT: u32 = 126;

/// Floating-point square root.
pub fn sqrt<F: FloatFormat>(a: F, env: &mut FloatEnv) -> F {
    let pa = unpack::<F>(a);

    if pa.is_nan() {
        let mut r = nan_propagate_one(&pa, env);
        return round_pack::<F>(&mut r, env);
    }

    if pa.cls == FloatClass::Inf {
        if pa.sign {
            // sqrt(-Inf) = NaN, INVALID
            return return_nan::<F>(env);
        }
        let mut r = pa;
        return round_pack::<F>(&mut r, env);
    }

    if pa.cls == FloatClass::Zero {
        // sqrt(+-0) = +-0
        let mut r = pa;
        return round_pack::<F>(&mut r, env);
    }

    // sqrt of negative number = NaN, INVALID
    if pa.sign {
        return return_nan::<F>(env);
    }

    // Normal positive number.
    // Compute sqrt using Newton-Raphson on the u128 mantissa.
    //
    // If exp is odd, shift frac right by 1 so that exp
    // becomes even (we need exp/2 to be an integer).
    let mut frac = pa.frac;
    let mut exp = pa.exp;

    if exp & 1 != 0 {
        frac >>= 1;
        exp += 1;
    }

    // Result exponent is exp/2.
    let result_exp = exp >> 1;

    // Compute integer square root of frac.
    // frac has integer bit at position 126 (or 125 after
    // the odd-exp shift). The result should have the
    // integer bit at position 126.
    //
    // sqrt(frac) where frac ~ 2^126 -> result ~ 2^63.
    // We need to scale: result = sqrt(frac) << 63 (approx).
    //
    // Better: compute sqrt(frac << 126) to get a result
    // with ~126 significant bits. But we can't shift a
    // u128 by 126.
    //
    // Alternative: bit-by-bit sqrt algorithm.
    let result_frac = isqrt_u128(frac);

    let mut result = FloatParts {
        sign: false,
        exp: result_exp,
        frac: result_frac,
        cls: FloatClass::Normal,
    };
    round_pack::<F>(&mut result, env)
}

/// Integer square root with extra precision for rounding.
/// Input has the integer bit at position 126.
/// Output has the integer bit at position 126.
///
/// We compute sqrt(frac * 2^126), then the result has the
/// integer bit at position 126 of the output.
///
/// Since frac ~ 2^126, sqrt(frac) ~ 2^63. We need 2^126
/// in the result, so we compute: result = sqrt(frac) << 63.
/// But we need more precision than that for rounding.
///
/// Use the bit-by-bit square root algorithm operating on
/// a 256-bit extended value (frac << 126) to produce 128
/// quotient bits.
fn isqrt_u128(frac: u128) -> u128 {
    // We want to compute floor(sqrt(frac * 2^126)).
    // This gives us a result with integer bit at position
    // 126 (since sqrt(2^126 * 2^126) = 2^126).
    //
    // Actually: frac has integer bit at position 126, so
    // frac ~ 1.xxx * 2^126. Then frac * 2^126 ~ 2^252.
    // sqrt(2^252) = 2^126. Good.
    //
    // We can't represent 2^252 in a u128. So we use the
    // digit-by-digit method with a virtual shift.

    // Simplified approach: use Newton's method with u128.
    // Start with an estimate and iterate.

    if frac == 0 {
        return 0;
    }

    let mut rem: u128 = 0;
    let mut result: u128 = 0;

    // We process 2 bits of the radicand per iteration
    // to produce 1 bit of the result.
    // Total result bits needed: 128 (position 0..127).
    // Total radicand bits: 253 (positions 0..252).
    // We process from the top.

    // The radicand is (frac << 126). Its bits:
    // - bits [252..126] come from frac[126..0]
    // - bits [125..0] are all zero

    for i in (0..=INT_BIT).rev() {
        // We're producing result bit at position
        // (INT_BIT - (INT_BIT - i)) = i... let me
        // restructure.
        // Actually, let's produce bits from position
        // INT_BIT down to 0 (127 iterations).
        let bit_pos = i;

        // Bring in 2 bits of the radicand.
        // Radicand bit positions for iteration producing
        // result bit at position `bit_pos`:
        //   radicand bits at 2*bit_pos+1 and 2*bit_pos.
        let rb_hi = 2 * bit_pos + 1;
        let rb_lo = 2 * bit_pos;

        // Get radicand bits from (frac << 126).
        let get_radicand_bit = |pos: u32| -> u128 {
            if pos >= 253 {
                return 0;
            }
            if pos >= 126 {
                (frac >> (pos - 126)) & 1
            } else {
                0 // lower 126 bits are zero
            }
        };

        rem = (rem << 2)
            | (get_radicand_bit(rb_hi) << 1)
            | get_radicand_bit(rb_lo);

        let trial = (result << 2) | 1;
        if rem >= trial {
            rem -= trial;
            result = (result << 1) | 1;
        } else {
            result <<= 1;
        }
    }

    // Set sticky bit if remainder is non-zero.
    if rem != 0 {
        result |= 1;
    }

    result
}

// ---------------------------------------------------------------
// Convenience methods
// ---------------------------------------------------------------

macro_rules! impl_sqrt {
    ($ty:ty) => {
        impl $ty {
            pub fn sqrt(self, env: &mut FloatEnv) -> Self {
                sqrt::<Self>(self, env)
            }
        }
    };
}

impl_sqrt!(Float16);
impl_sqrt!(BFloat16);
impl_sqrt!(Float32);
impl_sqrt!(Float64);
impl_sqrt!(Float128);
impl_sqrt!(FloatX80);
