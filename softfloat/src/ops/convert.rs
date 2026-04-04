// SPDX-License-Identifier: MIT
// IEEE 754 floating-point conversions (float-to-float,
// float-to-int, int-to-float).

use crate::env::{ExcFlags, FloatEnv, RoundMode};
use crate::parts::{round_pack, unpack, FloatClass, FloatParts};
use crate::types::{
    BFloat16, BitOps, Float128, Float16, Float32, Float64, FloatFormat,
    FloatX80,
};

const INT_BIT: u32 = 126;

// ---------------------------------------------------------------
// Float-to-float conversion
// ---------------------------------------------------------------

/// Convert from format F to format G (widen or narrow).
pub fn convert<F: FloatFormat, G: FloatFormat>(a: F, env: &mut FloatEnv) -> G {
    let mut parts = unpack::<F>(a);
    round_pack::<G>(&mut parts, env)
}

// ---------------------------------------------------------------
// Float-to-integer (internal helper)
// ---------------------------------------------------------------

/// Extract the magnitude as u128, with rounding applied,
/// plus inexact flag. Returns (magnitude, inexact).
fn float_to_uint128<F: FloatFormat>(
    a: F,
    env: &mut FloatEnv,
) -> (FloatParts, u128, bool) {
    let parts = unpack::<F>(a);
    if !matches!(parts.cls, FloatClass::Normal) {
        return (parts, 0, false);
    }

    let shift = INT_BIT as i32 - parts.exp;
    let rm = env.round_mode();

    let (int_val, inexact) = if shift >= 128 {
        (0u128, parts.frac != 0)
    } else if shift > 0 {
        let shift = shift as u32;
        let remainder = parts.frac & ((1u128 << shift) - 1);
        let truncated = parts.frac >> shift;
        let inexact = remainder != 0;
        let half = 1u128 << (shift - 1);
        let up =
            should_round_up_int(remainder, half, truncated, rm, parts.sign);
        (if up { truncated + 1 } else { truncated }, inexact)
    } else if shift == 0 {
        (parts.frac, false)
    } else {
        let lshift = (-shift) as u32;
        if lshift >= 128 {
            return (parts, u128::MAX, false);
        }
        (parts.frac << lshift, false)
    };

    (parts, int_val, inexact)
}

// ---------------------------------------------------------------
// Float-to-integer conversions
// ---------------------------------------------------------------

pub fn to_i32<F: FloatFormat>(a: F, env: &mut FloatEnv) -> i32 {
    let parts = unpack::<F>(a);

    if parts.is_nan() {
        env.raise(ExcFlags::INVALID);
        return i32::MAX;
    }
    if parts.cls == FloatClass::Inf {
        env.raise(ExcFlags::INVALID);
        return if parts.sign { i32::MIN } else { i32::MAX };
    }
    if parts.cls == FloatClass::Zero {
        return 0;
    }

    let (_, mag, inexact) = float_to_uint128::<F>(a, env);

    if parts.sign {
        if mag > 0x8000_0000 {
            env.raise(ExcFlags::INVALID);
            return i32::MIN;
        }
        if inexact {
            env.raise(ExcFlags::INEXACT);
        }
        -(mag as i32)
    } else {
        if mag > 0x7FFF_FFFF {
            env.raise(ExcFlags::INVALID);
            return i32::MAX;
        }
        if inexact {
            env.raise(ExcFlags::INEXACT);
        }
        mag as i32
    }
}

pub fn to_u32<F: FloatFormat>(a: F, env: &mut FloatEnv) -> u32 {
    let parts = unpack::<F>(a);

    if parts.is_nan() {
        env.raise(ExcFlags::INVALID);
        return u32::MAX;
    }
    if parts.cls == FloatClass::Inf {
        env.raise(ExcFlags::INVALID);
        return if parts.sign { 0 } else { u32::MAX };
    }
    if parts.cls == FloatClass::Zero {
        return 0;
    }

    if parts.sign {
        // Negative: truncate toward zero. If the
        // magnitude truncates to 0, result is 0 with
        // INEXACT (value was not zero but rounds to 0).
        // If magnitude > 0, the negative integer is not
        // representable as unsigned → INVALID.
        let (_, mag, inexact) = float_to_uint128::<F>(a, env);
        if mag > 0 {
            env.raise(ExcFlags::INVALID);
        } else if inexact {
            env.raise(ExcFlags::INEXACT);
        }
        return 0;
    }

    let (_, mag, inexact) = float_to_uint128::<F>(a, env);
    if mag > u32::MAX as u128 {
        env.raise(ExcFlags::INVALID);
        return u32::MAX;
    }
    if inexact {
        env.raise(ExcFlags::INEXACT);
    }
    mag as u32
}

pub fn to_i64<F: FloatFormat>(a: F, env: &mut FloatEnv) -> i64 {
    let parts = unpack::<F>(a);

    if parts.is_nan() {
        env.raise(ExcFlags::INVALID);
        return i64::MAX;
    }
    if parts.cls == FloatClass::Inf {
        env.raise(ExcFlags::INVALID);
        return if parts.sign { i64::MIN } else { i64::MAX };
    }
    if parts.cls == FloatClass::Zero {
        return 0;
    }

    let (_, mag, inexact) = float_to_uint128::<F>(a, env);

    if parts.sign {
        if mag > 0x8000_0000_0000_0000 {
            env.raise(ExcFlags::INVALID);
            return i64::MIN;
        }
        if inexact {
            env.raise(ExcFlags::INEXACT);
        }
        -(mag as i64)
    } else {
        if mag > 0x7FFF_FFFF_FFFF_FFFF {
            env.raise(ExcFlags::INVALID);
            return i64::MAX;
        }
        if inexact {
            env.raise(ExcFlags::INEXACT);
        }
        mag as i64
    }
}

pub fn to_u64<F: FloatFormat>(a: F, env: &mut FloatEnv) -> u64 {
    let parts = unpack::<F>(a);

    if parts.is_nan() {
        env.raise(ExcFlags::INVALID);
        return u64::MAX;
    }
    if parts.cls == FloatClass::Inf {
        env.raise(ExcFlags::INVALID);
        return if parts.sign { 0 } else { u64::MAX };
    }
    if parts.cls == FloatClass::Zero {
        return 0;
    }

    if parts.sign {
        let (_, mag, inexact) = float_to_uint128::<F>(a, env);
        if mag > 0 {
            env.raise(ExcFlags::INVALID);
        } else if inexact {
            env.raise(ExcFlags::INEXACT);
        }
        return 0;
    }

    let (_, mag, inexact) = float_to_uint128::<F>(a, env);
    if mag > u64::MAX as u128 {
        env.raise(ExcFlags::INVALID);
        return u64::MAX;
    }
    if inexact {
        env.raise(ExcFlags::INEXACT);
    }
    mag as u64
}

fn should_round_up_int(
    remainder: u128,
    half: u128,
    truncated: u128,
    rm: RoundMode,
    sign: bool,
) -> bool {
    match rm {
        RoundMode::NearEven => {
            if remainder > half {
                true
            } else if remainder == half {
                truncated & 1 != 0
            } else {
                false
            }
        }
        RoundMode::NearMaxMag => remainder >= half,
        RoundMode::ToZero => false,
        RoundMode::Down => sign && remainder != 0,
        RoundMode::Up => !sign && remainder != 0,
        RoundMode::Odd => {
            if remainder != 0 {
                truncated & 1 == 0
            } else {
                false
            }
        }
    }
}

// ---------------------------------------------------------------
// Integer-to-float conversions
// ---------------------------------------------------------------

pub fn from_i32<F: FloatFormat>(a: i32, env: &mut FloatEnv) -> F {
    if a == 0 {
        return F::from_bits(<F::Bits as BitOps>::from_u128(0));
    }
    let sign = a < 0;
    let mag = if sign {
        (a as i64).unsigned_abs()
    } else {
        a as u64
    };
    from_uint_impl::<F>(mag as u128, sign, env)
}

pub fn from_u32<F: FloatFormat>(a: u32, env: &mut FloatEnv) -> F {
    if a == 0 {
        return F::from_bits(<F::Bits as BitOps>::from_u128(0));
    }
    from_uint_impl::<F>(a as u128, false, env)
}

pub fn from_i64<F: FloatFormat>(a: i64, env: &mut FloatEnv) -> F {
    if a == 0 {
        return F::from_bits(<F::Bits as BitOps>::from_u128(0));
    }
    let sign = a < 0;
    let mag = a.unsigned_abs();
    from_uint_impl::<F>(mag as u128, sign, env)
}

pub fn from_u64<F: FloatFormat>(a: u64, env: &mut FloatEnv) -> F {
    if a == 0 {
        return F::from_bits(<F::Bits as BitOps>::from_u128(0));
    }
    from_uint_impl::<F>(a as u128, false, env)
}

fn from_uint_impl<F: FloatFormat>(
    mag: u128,
    sign: bool,
    env: &mut FloatEnv,
) -> F {
    if mag == 0 {
        return F::from_bits(<F::Bits as BitOps>::from_u128(0));
    }

    let msb = 127 - mag.leading_zeros();
    let exp = msb as i32;

    let frac = if msb <= INT_BIT {
        mag << (INT_BIT - msb)
    } else {
        let shift = msb - INT_BIT;
        let sticky = if mag & ((1u128 << shift) - 1) != 0 {
            1u128
        } else {
            0
        };
        (mag >> shift) | sticky
    };

    let mut parts = FloatParts {
        sign,
        exp,
        frac,
        cls: FloatClass::Normal,
    };
    round_pack::<F>(&mut parts, env)
}

// ---------------------------------------------------------------
// Convenience methods for float-to-float conversion
// ---------------------------------------------------------------

macro_rules! impl_convert_from {
    ($dst:ty, $src:ty, $method:ident) => {
        impl $dst {
            pub fn $method(v: $src, env: &mut FloatEnv) -> Self {
                convert::<$src, $dst>(v, env)
            }
        }
    };
}

impl_convert_from!(Float32, Float16, from_f16);
impl_convert_from!(Float32, BFloat16, from_bf16);
impl_convert_from!(Float32, Float64, from_f64);
impl_convert_from!(Float32, Float128, from_f128);
impl_convert_from!(Float32, FloatX80, from_fx80);

impl_convert_from!(Float64, Float16, from_f16);
impl_convert_from!(Float64, BFloat16, from_bf16);
impl_convert_from!(Float64, Float32, from_f32);
impl_convert_from!(Float64, Float128, from_f128);
impl_convert_from!(Float64, FloatX80, from_fx80);

impl_convert_from!(Float16, Float32, from_f32);
impl_convert_from!(Float16, Float64, from_f64);
impl_convert_from!(Float16, BFloat16, from_bf16);
impl_convert_from!(Float16, Float128, from_f128);
impl_convert_from!(Float16, FloatX80, from_fx80);

impl_convert_from!(BFloat16, Float16, from_f16);
impl_convert_from!(BFloat16, Float32, from_f32);
impl_convert_from!(BFloat16, Float64, from_f64);
impl_convert_from!(BFloat16, Float128, from_f128);
impl_convert_from!(BFloat16, FloatX80, from_fx80);

impl_convert_from!(Float128, Float16, from_f16);
impl_convert_from!(Float128, BFloat16, from_bf16);
impl_convert_from!(Float128, Float32, from_f32);
impl_convert_from!(Float128, Float64, from_f64);
impl_convert_from!(Float128, FloatX80, from_fx80);

impl_convert_from!(FloatX80, Float16, from_f16);
impl_convert_from!(FloatX80, BFloat16, from_bf16);
impl_convert_from!(FloatX80, Float32, from_f32);
impl_convert_from!(FloatX80, Float64, from_f64);
impl_convert_from!(FloatX80, Float128, from_f128);

macro_rules! impl_to_int {
    ($ty:ty) => {
        impl $ty {
            pub fn to_i32(self, env: &mut FloatEnv) -> i32 {
                to_i32::<Self>(self, env)
            }
            pub fn to_u32(self, env: &mut FloatEnv) -> u32 {
                to_u32::<Self>(self, env)
            }
            pub fn to_i64(self, env: &mut FloatEnv) -> i64 {
                to_i64::<Self>(self, env)
            }
            pub fn to_u64(self, env: &mut FloatEnv) -> u64 {
                to_u64::<Self>(self, env)
            }
        }
    };
}

impl_to_int!(Float16);
impl_to_int!(BFloat16);
impl_to_int!(Float32);
impl_to_int!(Float64);
impl_to_int!(Float128);
impl_to_int!(FloatX80);

macro_rules! impl_from_int {
    ($ty:ty) => {
        impl $ty {
            pub fn from_i32(v: i32, env: &mut FloatEnv) -> Self {
                from_i32::<Self>(v, env)
            }
            pub fn from_u32(v: u32, env: &mut FloatEnv) -> Self {
                from_u32::<Self>(v, env)
            }
            pub fn from_i64(v: i64, env: &mut FloatEnv) -> Self {
                from_i64::<Self>(v, env)
            }
            pub fn from_u64(v: u64, env: &mut FloatEnv) -> Self {
                from_u64::<Self>(v, env)
            }
        }
    };
}

impl_from_int!(Float16);
impl_from_int!(BFloat16);
impl_from_int!(Float32);
impl_from_int!(Float64);
impl_from_int!(Float128);
impl_from_int!(FloatX80);
