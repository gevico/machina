// SPDX-License-Identifier: MIT
// Decomposed floating-point representation and pack/unpack logic.
//
// Internal convention:
//   frac is a u128 with the integer bit at position 126.
//   For normal numbers: bit 126 = 1 (the implicit/explicit
//   leading one), bits [125 .. 126-FRAC_BITS] hold the
//   explicit fraction, lower bits are zero after unpack
//   (used as guard/round/sticky during operations).
//
// The exponent `exp` is *unbiased*: for a normal number with
// biased exponent `e`, exp = e - BIAS.  Subnormals are
// normalized on unpack so exp = 1 - BIAS - shift_amount.

use crate::env::{ExcFlags, FloatEnv, RoundMode, Tininess};
use crate::types::{BitOps, FloatFormat};

/// Integer bit position inside frac (u128).
const INT_BIT: u32 = 126;

// ---------------------------------------------------------------
// FloatClass
// ---------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatClass {
    Normal,
    Zero,
    Inf,
    QNaN,
    SNaN,
}

// ---------------------------------------------------------------
// FloatParts
// ---------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct FloatParts {
    pub sign: bool,
    pub exp: i32,
    pub frac: u128,
    pub cls: FloatClass,
}

impl FloatParts {
    pub fn is_nan(&self) -> bool {
        matches!(self.cls, FloatClass::QNaN | FloatClass::SNaN)
    }

    pub fn is_inf(&self) -> bool {
        self.cls == FloatClass::Inf
    }

    /// Canonical quiet NaN for format F.
    pub fn default_nan<F: FloatFormat>() -> Self {
        // Canonical NaN: positive, quiet, MSB of fraction set.
        Self {
            sign: false,
            exp: 0,
            frac: 1u128 << (INT_BIT - 1), // quiet bit
            cls: FloatClass::QNaN,
        }
    }
}

// ---------------------------------------------------------------
// Unpack
// ---------------------------------------------------------------

pub fn unpack<F: FloatFormat>(val: F) -> FloatParts {
    let bits = val.to_bits().to_u128();
    let total_bits = 1
        + F::EXP_BITS
        + F::FRAC_BITS
        + if F::HAS_EXPLICIT_INT { 1 } else { 0 };
    let _ = total_bits; // informational

    // Fraction mask (includes explicit integer bit for x80)
    let frac_total = if F::HAS_EXPLICIT_INT {
        F::FRAC_BITS + 1
    } else {
        F::FRAC_BITS
    };
    let frac_mask = (1u128 << frac_total) - 1;
    let raw_frac = bits & frac_mask;

    let exp_mask = (1u128 << F::EXP_BITS) - 1;
    let raw_exp = ((bits >> frac_total) & exp_mask) as u32;

    let sign_shift = frac_total + F::EXP_BITS;
    let sign = ((bits >> sign_shift) & 1) != 0;

    let max_exp = (1u32 << F::EXP_BITS) - 1;

    if raw_exp == max_exp {
        // Infinity or NaN
        let frac_for_nan = if F::HAS_EXPLICIT_INT {
            // For x80, ignore the integer bit for
            // inf/nan classification of the fraction part.
            raw_frac & ((1u128 << F::FRAC_BITS) - 1)
        } else {
            raw_frac
        };

        if frac_for_nan == 0 {
            // Check for x80 pseudo-infinity (integer bit
            // clear): treat as NaN.
            if F::HAS_EXPLICIT_INT && (raw_frac >> F::FRAC_BITS) & 1 == 0 {
                return FloatParts {
                    sign,
                    exp: 0,
                    frac: 1u128 << (INT_BIT - 1),
                    cls: FloatClass::QNaN,
                };
            }
            return FloatParts {
                sign,
                exp: 0,
                frac: 0,
                cls: FloatClass::Inf,
            };
        }

        // NaN: quiet bit is the MSB of the fraction field.
        let quiet_bit = F::FRAC_BITS - 1;
        let is_quiet = (raw_frac >> quiet_bit) & 1 != 0;

        // Shift fraction into internal position.
        // For NaN payload we store the raw fraction
        // (without integer bit for x80) left-aligned
        // below INT_BIT.
        let payload = if F::HAS_EXPLICIT_INT {
            raw_frac & ((1u128 << F::FRAC_BITS) - 1)
        } else {
            raw_frac
        };
        let frac = payload << (INT_BIT - F::FRAC_BITS);

        let cls = if is_quiet {
            FloatClass::QNaN
        } else {
            FloatClass::SNaN
        };
        return FloatParts {
            sign,
            exp: 0,
            frac,
            cls,
        };
    }

    if raw_exp == 0 {
        if raw_frac == 0 {
            return FloatParts {
                sign,
                exp: 0,
                frac: 0,
                cls: FloatClass::Zero,
            };
        }
        // Subnormal: normalize
        return unpack_subnormal::<F>(sign, raw_frac);
    }

    // Normal
    let exp = raw_exp as i32 - F::BIAS;
    let frac = if F::HAS_EXPLICIT_INT {
        // raw_frac includes the explicit integer bit at
        // position FRAC_BITS. Shift so that the integer bit
        // lands at INT_BIT.
        raw_frac << (INT_BIT - F::FRAC_BITS)
    } else {
        (1u128 << INT_BIT) | (raw_frac << (INT_BIT - F::FRAC_BITS))
    };

    FloatParts {
        sign,
        exp,
        frac,
        cls: FloatClass::Normal,
    }
}

fn unpack_subnormal<F: FloatFormat>(sign: bool, raw_frac: u128) -> FloatParts {
    // Minimum exponent for the format (exponent of subnormals
    // if they had biased_exp == 1).
    let exp_min = 1 - F::BIAS;

    // Place the fraction bits left-aligned just below INT_BIT.
    let shift_base = INT_BIT - F::FRAC_BITS;
    let mut frac = raw_frac << shift_base;

    // Normalize: shift left until bit INT_BIT is set.
    let lz = frac.leading_zeros();
    // frac is u128, INT_BIT = 126, so we need
    // (127 - INT_BIT) = 1 leading zero when bit 126 is set.
    let shift = lz - (127 - INT_BIT);
    frac <<= shift;
    let exp = exp_min - shift as i32;

    FloatParts {
        sign,
        exp,
        frac,
        cls: FloatClass::Normal,
    }
}

// ---------------------------------------------------------------
// Pack
// ---------------------------------------------------------------

/// Round, handle overflow/underflow, and pack into target format.
pub fn round_pack<F: FloatFormat>(
    parts: &mut FloatParts,
    env: &mut FloatEnv,
) -> F {
    match parts.cls {
        FloatClass::Zero => return pack_zero::<F>(parts.sign),
        FloatClass::Inf => return pack_inf::<F>(parts.sign),
        FloatClass::QNaN | FloatClass::SNaN => {
            return pack_nan::<F>(parts, env);
        }
        FloatClass::Normal => {}
    }

    // Normalize: ensure integer bit is at INT_BIT.
    if parts.frac == 0 {
        return pack_zero::<F>(parts.sign);
    }
    let lz = parts.frac.leading_zeros();
    let target_lz = 127 - INT_BIT; // = 1
    if lz > target_lz {
        let shift = lz - target_lz;
        parts.frac <<= shift;
        parts.exp -= shift as i32;
    } else if lz < target_lz {
        let shift = target_lz - lz;
        // Shift right, preserving sticky bits.
        let sticky = if parts.frac & ((1u128 << shift) - 1) != 0 {
            1u128
        } else {
            0
        };
        parts.frac = (parts.frac >> shift) | sticky;
        parts.exp += shift as i32;
    }

    pack::<F>(parts, env)
}

/// Pack an already-normalized FloatParts into format F.
/// The integer bit must be at position INT_BIT.
pub fn pack<F: FloatFormat>(parts: &mut FloatParts, env: &mut FloatEnv) -> F {
    match parts.cls {
        FloatClass::Zero => return pack_zero::<F>(parts.sign),
        FloatClass::Inf => return pack_inf::<F>(parts.sign),
        FloatClass::QNaN | FloatClass::SNaN => {
            return pack_nan::<F>(parts, env);
        }
        FloatClass::Normal => {}
    }

    if parts.frac == 0 {
        return pack_zero::<F>(parts.sign);
    }

    let max_exp = ((1u32 << F::EXP_BITS) - 1) as i32;
    let rm = env.round_mode();

    // Number of extra bits below the fraction field.
    let round_pos = INT_BIT - F::FRAC_BITS;
    let round_mask = (1u128 << round_pos) - 1;
    let half = 1u128 << (round_pos - 1);

    let mut biased_exp = parts.exp + F::BIAS;

    // --- Handle potential underflow (subnormal result) ---
    if biased_exp < 1 {
        // Need to right-shift frac to make biased_exp == 0.
        let shift = (1 - biased_exp) as u32;
        if shift >= 128 {
            // Completely shifted out.
            let nonzero = parts.frac != 0;
            parts.frac = 0;
            if nonzero {
                parts.frac = 1; // sticky
            }
        } else {
            let sticky = if parts.frac & ((1u128 << shift) - 1) != 0 {
                1u128
            } else {
                0
            };
            parts.frac = (parts.frac >> shift) | sticky;
        }
        biased_exp = 0;

        // Detect tininess
        let is_tiny_before = true; // biased_exp < 1
        let remainder = parts.frac & round_mask;
        if remainder != 0 {
            // Check tininess mode
            let is_tiny = match env.tininess() {
                Tininess::BeforeRounding => is_tiny_before,
                Tininess::AfterRounding => {
                    // Tiny if after rounding it's still
                    // subnormal (integer bit not at
                    // INT_BIT).
                    let rounded = apply_rounding(
                        parts.frac, remainder, half, round_mask, rm, parts.sign,
                    );
                    (rounded >> INT_BIT) & 1 == 0
                }
            };
            if is_tiny {
                env.raise(ExcFlags::UNDERFLOW);
            }
        }
    }

    // --- Rounding ---
    let remainder = parts.frac & round_mask;
    let inexact = remainder != 0;

    parts.frac =
        apply_rounding(parts.frac, remainder, half, round_mask, rm, parts.sign);

    // Check if rounding caused the integer bit to overflow
    // (e.g., frac was all-ones in the fraction field).
    if parts.frac >> (INT_BIT + 1) != 0 {
        parts.frac >>= 1;
        biased_exp += 1;
    }

    // --- Overflow check ---
    if biased_exp >= max_exp {
        env.raise(ExcFlags::OVERFLOW | ExcFlags::INEXACT);
        return overflow_result::<F>(parts.sign, rm);
    }

    if inexact {
        env.raise(ExcFlags::INEXACT);
    }

    // --- Assemble the result ---
    let frac_field = if biased_exp == 0 {
        // Subnormal: no implicit integer bit.
        // Extract fraction bits from below INT_BIT.
        (parts.frac >> round_pos) & ((1u128 << F::FRAC_BITS) - 1)
    } else if F::HAS_EXPLICIT_INT {
        // x80: include the integer bit.
        (parts.frac >> (INT_BIT - F::FRAC_BITS))
            & ((1u128 << (F::FRAC_BITS + 1)) - 1)
    } else {
        // Drop the implicit integer bit at INT_BIT.
        (parts.frac >> round_pos) & ((1u128 << F::FRAC_BITS) - 1)
    };

    let frac_total = if F::HAS_EXPLICIT_INT {
        F::FRAC_BITS + 1
    } else {
        F::FRAC_BITS
    };

    let bits = ((parts.sign as u128) << (frac_total + F::EXP_BITS))
        | ((biased_exp as u128) << frac_total)
        | frac_field;

    F::from_bits(<F::Bits as crate::types::BitOps>::from_u128(bits))
}

// ---------------------------------------------------------------
// Rounding helper
// ---------------------------------------------------------------

fn apply_rounding(
    frac: u128,
    remainder: u128,
    half: u128,
    round_mask: u128,
    rm: RoundMode,
    sign: bool,
) -> u128 {
    let truncated = frac & !round_mask;
    let lsb_set = (frac >> round_mask.count_ones()) & 1 != 0;

    match rm {
        RoundMode::NearEven => {
            if remainder > half {
                truncated.wrapping_add(round_mask + 1)
            } else if remainder == half {
                // Ties to even: round up if LSB is odd.
                if lsb_set {
                    truncated.wrapping_add(round_mask + 1)
                } else {
                    truncated
                }
            } else {
                truncated
            }
        }
        RoundMode::NearMaxMag => {
            // Ties away from zero.
            if remainder >= half {
                truncated.wrapping_add(round_mask + 1)
            } else {
                truncated
            }
        }
        RoundMode::ToZero => truncated,
        RoundMode::Down => {
            // Towards -inf.
            if sign && remainder != 0 {
                truncated.wrapping_add(round_mask + 1)
            } else {
                truncated
            }
        }
        RoundMode::Up => {
            // Towards +inf.
            if !sign && remainder != 0 {
                truncated.wrapping_add(round_mask + 1)
            } else {
                truncated
            }
        }
        RoundMode::Odd => {
            if remainder != 0 {
                // Set the LSB to 1.
                truncated | (round_mask + 1)
            } else {
                truncated
            }
        }
    }
}

// ---------------------------------------------------------------
// Overflow result
// ---------------------------------------------------------------

fn overflow_result<F: FloatFormat>(sign: bool, rm: RoundMode) -> F {
    // Depending on rounding mode, overflow may produce infinity
    // or the largest finite number.
    match rm {
        RoundMode::NearEven | RoundMode::NearMaxMag => pack_inf::<F>(sign),
        RoundMode::ToZero => pack_max_finite::<F>(sign),
        RoundMode::Down => {
            if sign {
                pack_inf::<F>(true)
            } else {
                pack_max_finite::<F>(false)
            }
        }
        RoundMode::Up => {
            if sign {
                pack_max_finite::<F>(true)
            } else {
                pack_inf::<F>(false)
            }
        }
        RoundMode::Odd => pack_max_finite::<F>(sign),
    }
}

// ---------------------------------------------------------------
// Packing helpers for special values
// ---------------------------------------------------------------

fn pack_zero<F: FloatFormat>(sign: bool) -> F {
    let frac_total = if F::HAS_EXPLICIT_INT {
        F::FRAC_BITS + 1
    } else {
        F::FRAC_BITS
    };
    let bits = (sign as u128) << (frac_total + F::EXP_BITS);
    F::from_bits(<F::Bits as crate::types::BitOps>::from_u128(bits))
}

fn pack_inf<F: FloatFormat>(sign: bool) -> F {
    let max_exp = (1u128 << F::EXP_BITS) - 1;
    let frac_total = if F::HAS_EXPLICIT_INT {
        F::FRAC_BITS + 1
    } else {
        F::FRAC_BITS
    };

    let mut bits = ((sign as u128) << (frac_total + F::EXP_BITS))
        | (max_exp << frac_total);

    // x80 infinity: integer bit set, fraction zero.
    if F::HAS_EXPLICIT_INT {
        bits |= 1u128 << F::FRAC_BITS;
    }

    F::from_bits(<F::Bits as crate::types::BitOps>::from_u128(bits))
}

fn pack_max_finite<F: FloatFormat>(sign: bool) -> F {
    let max_exp = (1u128 << F::EXP_BITS) - 2;
    let frac_total = if F::HAS_EXPLICIT_INT {
        F::FRAC_BITS + 1
    } else {
        F::FRAC_BITS
    };
    let frac_all_ones = (1u128 << frac_total) - 1;

    let bits = ((sign as u128) << (frac_total + F::EXP_BITS))
        | (max_exp << frac_total)
        | frac_all_ones;

    F::from_bits(<F::Bits as crate::types::BitOps>::from_u128(bits))
}

fn pack_nan<F: FloatFormat>(parts: &FloatParts, env: &mut FloatEnv) -> F {
    if env.default_nan() {
        let dn = FloatParts::default_nan::<F>();
        return encode_nan::<F>(&dn);
    }

    // Quieten SNaN if needed.
    let mut p = *parts;
    if p.cls == FloatClass::SNaN {
        p.cls = FloatClass::QNaN;
        // Set the quiet bit.
        p.frac |= 1u128 << (INT_BIT - 1);
    }
    encode_nan::<F>(&p)
}

fn encode_nan<F: FloatFormat>(parts: &FloatParts) -> F {
    let max_exp = (1u128 << F::EXP_BITS) - 1;
    let frac_total = if F::HAS_EXPLICIT_INT {
        F::FRAC_BITS + 1
    } else {
        F::FRAC_BITS
    };

    // Extract fraction payload from internal position.
    let payload = (parts.frac >> (INT_BIT - F::FRAC_BITS))
        & ((1u128 << F::FRAC_BITS) - 1);

    // Ensure at least the quiet bit is set for QNaN.
    let quiet_bit = 1u128 << (F::FRAC_BITS - 1);
    let payload = if parts.cls == FloatClass::QNaN {
        payload | quiet_bit
    } else {
        payload & !quiet_bit
    };

    // Ensure NaN payload is non-zero.
    let payload = if payload == 0 { quiet_bit } else { payload };

    let mut bits = ((parts.sign as u128) << (frac_total + F::EXP_BITS))
        | (max_exp << frac_total)
        | payload;

    // x80: set the integer bit for NaN.
    if F::HAS_EXPLICIT_INT {
        bits |= 1u128 << F::FRAC_BITS;
    }

    F::from_bits(<F::Bits as crate::types::BitOps>::from_u128(bits))
}

// ---------------------------------------------------------------
// NaN propagation
// ---------------------------------------------------------------

/// IEEE 754 NaN propagation: prefer first SNaN, then first QNaN.
/// Signals INVALID if either operand is SNaN.
pub fn nan_propagate(
    a: &FloatParts,
    b: &FloatParts,
    env: &mut FloatEnv,
) -> FloatParts {
    if a.cls == FloatClass::SNaN || b.cls == FloatClass::SNaN {
        env.raise(ExcFlags::INVALID);
    }

    if env.default_nan() {
        // Use arbitrary format; caller will re-encode.
        // The frac/exp don't matter much -- pack_nan
        // will produce the canonical NaN.
        return FloatParts {
            sign: false,
            exp: 0,
            frac: 1u128 << (INT_BIT - 1),
            cls: FloatClass::QNaN,
        };
    }

    // Prefer SNaN (quietened) over QNaN, prefer `a` over `b`.
    let pick = if a.cls == FloatClass::SNaN {
        a
    } else if b.cls == FloatClass::SNaN {
        b
    } else if a.cls == FloatClass::QNaN {
        a
    } else {
        b
    };

    let mut result = *pick;
    // Quieten SNaN.
    if result.cls == FloatClass::SNaN {
        result.cls = FloatClass::QNaN;
        result.frac |= 1u128 << (INT_BIT - 1);
    }
    result
}

/// Propagate NaN from a single operand (unary ops).
pub fn nan_propagate_one(a: &FloatParts, env: &mut FloatEnv) -> FloatParts {
    if a.cls == FloatClass::SNaN {
        env.raise(ExcFlags::INVALID);
    }

    if env.default_nan() {
        return FloatParts {
            sign: false,
            exp: 0,
            frac: 1u128 << (INT_BIT - 1),
            cls: FloatClass::QNaN,
        };
    }

    let mut result = *a;
    if result.cls == FloatClass::SNaN {
        result.cls = FloatClass::QNaN;
        result.frac |= 1u128 << (INT_BIT - 1);
    }
    result
}

// ---------------------------------------------------------------
// Utility: return invalid-operation default NaN
// ---------------------------------------------------------------

pub fn return_nan<F: FloatFormat>(env: &mut FloatEnv) -> F {
    env.raise(ExcFlags::INVALID);
    let dn = FloatParts::default_nan::<F>();
    encode_nan::<F>(&dn)
}
