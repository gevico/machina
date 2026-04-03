// SPDX-License-Identifier: MIT
// IEEE 754 floating-point minimum and maximum.

use crate::env::{ExcFlags, FloatEnv};
use crate::parts::{nan_propagate, round_pack, unpack, FloatClass};
use crate::types::{
    BFloat16, Float128, Float16, Float32, Float64, FloatFormat, FloatX80,
};

/// IEEE 754-2008 minimum: returns the smaller of a and b.
/// If either operand is SNaN, signals INVALID.
/// If both are NaN, returns a quiet NaN.
/// min(+0, -0) = -0.
pub fn min<F: FloatFormat>(a: F, b: F, env: &mut FloatEnv) -> F {
    let pa = unpack::<F>(a);
    let pb = unpack::<F>(b);

    // Signal INVALID for SNaN.
    if pa.cls == FloatClass::SNaN || pb.cls == FloatClass::SNaN {
        env.raise(ExcFlags::INVALID);
    }

    if pa.is_nan() && pb.is_nan() {
        let mut r = nan_propagate(&pa, &pb, env);
        return round_pack::<F>(&mut r, env);
    }

    if pa.is_nan() {
        return b;
    }
    if pb.is_nan() {
        return a;
    }

    // Both zeros: prefer -0.
    if pa.cls == FloatClass::Zero && pb.cls == FloatClass::Zero {
        return if pb.sign { b } else { a };
    }

    // Use comparison to pick smaller.
    let a_lt_b = if pa.sign != pb.sign {
        pa.sign // negative is smaller
    } else if pa.sign {
        // Both negative: larger magnitude is smaller.
        if pa.exp != pb.exp {
            pa.exp > pb.exp
        } else {
            pa.frac > pb.frac
        }
    } else {
        // Both positive: smaller magnitude is smaller.
        if pa.exp != pb.exp {
            pa.exp < pb.exp
        } else {
            pa.frac < pb.frac
        }
    };

    if a_lt_b {
        a
    } else {
        b
    }
}

/// IEEE 754-2008 maximum: returns the larger of a and b.
/// If either operand is SNaN, signals INVALID.
/// If both are NaN, returns a quiet NaN.
/// max(+0, -0) = +0.
pub fn max<F: FloatFormat>(a: F, b: F, env: &mut FloatEnv) -> F {
    let pa = unpack::<F>(a);
    let pb = unpack::<F>(b);

    if pa.cls == FloatClass::SNaN || pb.cls == FloatClass::SNaN {
        env.raise(ExcFlags::INVALID);
    }

    if pa.is_nan() && pb.is_nan() {
        let mut r = nan_propagate(&pa, &pb, env);
        return round_pack::<F>(&mut r, env);
    }

    if pa.is_nan() {
        return b;
    }
    if pb.is_nan() {
        return a;
    }

    // Both zeros: prefer +0.
    if pa.cls == FloatClass::Zero && pb.cls == FloatClass::Zero {
        return if pa.sign { b } else { a };
    }

    let a_gt_b = if pa.sign != pb.sign {
        pb.sign // positive is larger
    } else if pa.sign {
        // Both negative: smaller magnitude is larger.
        if pa.exp != pb.exp {
            pa.exp < pb.exp
        } else {
            pa.frac < pb.frac
        }
    } else {
        // Both positive: larger magnitude is larger.
        if pa.exp != pb.exp {
            pa.exp > pb.exp
        } else {
            pa.frac > pb.frac
        }
    };

    if a_gt_b {
        a
    } else {
        b
    }
}

// ---------------------------------------------------------------
// Convenience methods
// ---------------------------------------------------------------

macro_rules! impl_minmax {
    ($ty:ty) => {
        impl $ty {
            pub fn min(self, other: Self, env: &mut FloatEnv) -> Self {
                min::<Self>(self, other, env)
            }
            pub fn max(self, other: Self, env: &mut FloatEnv) -> Self {
                max::<Self>(self, other, env)
            }
        }
    };
}

impl_minmax!(Float16);
impl_minmax!(BFloat16);
impl_minmax!(Float32);
impl_minmax!(Float64);
impl_minmax!(Float128);
impl_minmax!(FloatX80);
