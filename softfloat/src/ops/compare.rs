// SPDX-License-Identifier: MIT
// IEEE 754 floating-point comparison operations.

use core::cmp::Ordering;

use crate::env::{ExcFlags, FloatEnv};
use crate::parts::{unpack, FloatClass};
use crate::types::{
    BFloat16, Float128, Float16, Float32, Float64, FloatFormat, FloatX80,
};

/// Signaling compare: returns None for unordered (NaN).
/// Signals INVALID for any NaN operand.
pub fn compare<F: FloatFormat>(
    a: F,
    b: F,
    env: &mut FloatEnv,
) -> Option<Ordering> {
    let pa = unpack::<F>(a);
    let pb = unpack::<F>(b);

    if pa.is_nan() || pb.is_nan() {
        env.raise(ExcFlags::INVALID);
        return None;
    }

    compare_ordered(&pa, &pb)
}

/// Quiet compare: returns None for unordered (NaN).
/// Only signals INVALID for SNaN, not QNaN.
pub fn compare_quiet<F: FloatFormat>(
    a: F,
    b: F,
    env: &mut FloatEnv,
) -> Option<Ordering> {
    let pa = unpack::<F>(a);
    let pb = unpack::<F>(b);

    if pa.is_nan() || pb.is_nan() {
        if pa.cls == FloatClass::SNaN || pb.cls == FloatClass::SNaN {
            env.raise(ExcFlags::INVALID);
        }
        return None;
    }

    compare_ordered(&pa, &pb)
}

fn compare_ordered(
    a: &crate::parts::FloatParts,
    b: &crate::parts::FloatParts,
) -> Option<Ordering> {
    // Both zeros are equal regardless of sign.
    if a.cls == FloatClass::Zero && b.cls == FloatClass::Zero {
        return Some(Ordering::Equal);
    }

    // Different signs.
    if a.sign != b.sign {
        // Negative < positive.
        return if a.sign {
            Some(Ordering::Less)
        } else {
            Some(Ordering::Greater)
        };
    }

    // Same sign. Compare by (cls, exp, frac).
    let raw_ord = if a.cls == FloatClass::Inf && b.cls == FloatClass::Inf {
        Ordering::Equal
    } else if a.cls == FloatClass::Inf {
        Ordering::Greater
    } else if b.cls == FloatClass::Inf {
        Ordering::Less
    } else if a.cls == FloatClass::Zero {
        // b is non-zero.
        Ordering::Less
    } else if b.cls == FloatClass::Zero {
        Ordering::Greater
    } else {
        // Both normal. Compare exponent first, then frac.
        match a.exp.cmp(&b.exp) {
            Ordering::Equal => a.frac.cmp(&b.frac),
            other => other,
        }
    };

    // If negative, reverse the ordering.
    if a.sign {
        Some(raw_ord.reverse())
    } else {
        Some(raw_ord)
    }
}

/// Quiet equality: NaN != NaN.
pub fn eq<F: FloatFormat>(a: F, b: F, env: &mut FloatEnv) -> bool {
    compare_quiet::<F>(a, b, env) == Some(Ordering::Equal)
}

/// Signaling less-than.
pub fn lt<F: FloatFormat>(a: F, b: F, env: &mut FloatEnv) -> bool {
    compare::<F>(a, b, env) == Some(Ordering::Less)
}

/// Signaling less-than-or-equal.
pub fn le<F: FloatFormat>(a: F, b: F, env: &mut FloatEnv) -> bool {
    matches!(
        compare::<F>(a, b, env),
        Some(Ordering::Less) | Some(Ordering::Equal)
    )
}

// ---------------------------------------------------------------
// Convenience methods
// ---------------------------------------------------------------

macro_rules! impl_compare {
    ($ty:ty) => {
        impl $ty {
            pub fn compare(
                self,
                other: Self,
                env: &mut FloatEnv,
            ) -> Option<Ordering> {
                compare::<Self>(self, other, env)
            }
            pub fn compare_quiet(
                self,
                other: Self,
                env: &mut FloatEnv,
            ) -> Option<Ordering> {
                compare_quiet::<Self>(self, other, env)
            }
            pub fn eq(self, other: Self, env: &mut FloatEnv) -> bool {
                eq::<Self>(self, other, env)
            }
            pub fn lt(self, other: Self, env: &mut FloatEnv) -> bool {
                lt::<Self>(self, other, env)
            }
            pub fn le(self, other: Self, env: &mut FloatEnv) -> bool {
                le::<Self>(self, other, env)
            }
        }
    };
}

impl_compare!(Float16);
impl_compare!(BFloat16);
impl_compare!(Float32);
impl_compare!(Float64);
impl_compare!(Float128);
impl_compare!(FloatX80);
