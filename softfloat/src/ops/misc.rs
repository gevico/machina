// SPDX-License-Identifier: MIT
// Miscellaneous floating-point operations: abs, neg, scalbn,
// classify.

use crate::env::FloatEnv;
use crate::parts::{round_pack, unpack, FloatClass};
use crate::types::{
    BFloat16, BitOps, Float128, Float16, Float32, Float64, FloatFormat,
    FloatX80,
};

/// Absolute value: clear the sign bit.
pub fn abs<F: FloatFormat>(a: F) -> F {
    let bits = a.to_bits().to_u128();
    let frac_total = if F::HAS_EXPLICIT_INT {
        F::FRAC_BITS + 1
    } else {
        F::FRAC_BITS
    };
    let sign_pos = frac_total + F::EXP_BITS;
    let mask = !(1u128 << sign_pos);
    let result = bits & mask;
    F::from_bits(<F::Bits as BitOps>::from_u128(result))
}

/// Negate: flip the sign bit.
pub fn neg<F: FloatFormat>(a: F) -> F {
    let bits = a.to_bits().to_u128();
    let frac_total = if F::HAS_EXPLICIT_INT {
        F::FRAC_BITS + 1
    } else {
        F::FRAC_BITS
    };
    let sign_pos = frac_total + F::EXP_BITS;
    let result = bits ^ (1u128 << sign_pos);
    F::from_bits(<F::Bits as BitOps>::from_u128(result))
}

/// Scale by power of 2: a * 2^n.
/// Does not raise INEXACT (unlike multiplication by 2^n).
pub fn scalbn<F: FloatFormat>(a: F, n: i32, env: &mut FloatEnv) -> F {
    let mut parts = unpack::<F>(a);

    match parts.cls {
        FloatClass::Normal => {
            parts.exp += n;
            round_pack::<F>(&mut parts, env)
        }
        FloatClass::QNaN | FloatClass::SNaN => {
            let mut r = crate::parts::nan_propagate_one(&parts, env);
            round_pack::<F>(&mut r, env)
        }
        _ => {
            // Zero and Inf are unchanged.
            round_pack::<F>(&mut parts, env)
        }
    }
}

/// IEEE 754 classification.
pub fn classify<F: FloatFormat>(a: F) -> FloatClass {
    let parts = unpack::<F>(a);
    parts.cls
}

/// Check if the value is a signaling NaN.
pub fn is_signaling_nan<F: FloatFormat>(a: F) -> bool {
    let parts = unpack::<F>(a);
    parts.cls == FloatClass::SNaN
}

/// Check if the value is a quiet NaN.
pub fn is_quiet_nan<F: FloatFormat>(a: F) -> bool {
    let parts = unpack::<F>(a);
    parts.cls == FloatClass::QNaN
}

/// Check if the value is subnormal (unpacks to Normal with
/// biased_exp == 0 in the original encoding, but our unpack
/// normalizes subnormals; detect via original exponent range).
pub fn is_subnormal<F: FloatFormat>(a: F) -> bool {
    let bits = a.to_bits().to_u128();
    let frac_total = if F::HAS_EXPLICIT_INT {
        F::FRAC_BITS + 1
    } else {
        F::FRAC_BITS
    };
    let exp_mask = (1u128 << F::EXP_BITS) - 1;
    let raw_exp = (bits >> frac_total) & exp_mask;
    let frac_mask = (1u128 << frac_total) - 1;
    let raw_frac = bits & frac_mask;

    raw_exp == 0 && raw_frac != 0
}

// ---------------------------------------------------------------
// Convenience methods
// ---------------------------------------------------------------

macro_rules! impl_misc {
    ($ty:ty) => {
        impl $ty {
            pub fn abs(self) -> Self {
                abs::<Self>(self)
            }
            #[allow(clippy::should_implement_trait)]
            pub fn neg(self) -> Self {
                neg::<Self>(self)
            }
            pub fn scalbn(self, n: i32, env: &mut FloatEnv) -> Self {
                scalbn::<Self>(self, n, env)
            }
            pub fn classify(self) -> FloatClass {
                classify::<Self>(self)
            }
        }
    };
}

impl_misc!(Float16);
impl_misc!(BFloat16);
impl_misc!(Float32);
impl_misc!(Float64);
impl_misc!(Float128);
impl_misc!(FloatX80);
