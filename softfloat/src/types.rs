// SPDX-License-Identifier: MIT
// IEEE 754 floating-point type definitions.

use core::fmt;

// ---------------------------------------------------------------
// Bit-manipulation helper trait for unsigned integer backing types
// ---------------------------------------------------------------

pub trait BitOps:
    Copy + Clone + PartialEq + Eq + core::hash::Hash + fmt::Debug + Sized + 'static
{
    const ZERO: Self;
    const ONE: Self;
    const MAX: Self;
    const BITS: u32;
    fn shl(self, n: u32) -> Self;
    fn shr(self, n: u32) -> Self;
    fn bitand(self, other: Self) -> Self;
    fn bitor(self, other: Self) -> Self;
    fn bitxor(self, other: Self) -> Self;
    fn not(self) -> Self;
    fn wrapping_sub(self, other: Self) -> Self;
    fn wrapping_add(self, other: Self) -> Self;
    fn to_u128(self) -> u128;
    fn from_u128(v: u128) -> Self;
    fn is_zero(self) -> bool;
    fn leading_zeros(self) -> u32;
}

macro_rules! impl_bitops {
    ($ty:ty) => {
        impl BitOps for $ty {
            const ZERO: Self = 0;
            const ONE: Self = 1;
            const MAX: Self = <$ty>::MAX;
            const BITS: u32 = <$ty>::BITS;
            #[inline]
            fn shl(self, n: u32) -> Self {
                self << n
            }
            #[inline]
            fn shr(self, n: u32) -> Self {
                self >> n
            }
            #[inline]
            fn bitand(self, o: Self) -> Self {
                self & o
            }
            #[inline]
            fn bitor(self, o: Self) -> Self {
                self | o
            }
            #[inline]
            fn bitxor(self, o: Self) -> Self {
                self ^ o
            }
            #[inline]
            fn not(self) -> Self {
                !self
            }
            #[inline]
            fn wrapping_sub(self, o: Self) -> Self {
                <$ty>::wrapping_sub(self, o)
            }
            #[inline]
            fn wrapping_add(self, o: Self) -> Self {
                <$ty>::wrapping_add(self, o)
            }
            #[inline]
            fn to_u128(self) -> u128 {
                self as u128
            }
            #[inline]
            fn from_u128(v: u128) -> Self {
                v as Self
            }
            #[inline]
            fn is_zero(self) -> bool {
                self == 0
            }
            #[inline]
            fn leading_zeros(self) -> u32 {
                <$ty>::leading_zeros(self)
            }
        }
    };
}

impl_bitops!(u16);
impl_bitops!(u32);
impl_bitops!(u64);
impl_bitops!(u128);

// ---------------------------------------------------------------
// FloatFormat trait
// ---------------------------------------------------------------

pub trait FloatFormat: Copy + Clone + PartialEq + Eq {
    type Bits: BitOps;
    const EXP_BITS: u32;
    const FRAC_BITS: u32;
    const BIAS: i32;
    const HAS_EXPLICIT_INT: bool;
    fn to_bits(self) -> Self::Bits;
    fn from_bits(bits: Self::Bits) -> Self;
}

// ---------------------------------------------------------------
// Float16 -- IEEE 754 half precision: 1+5+10
// ---------------------------------------------------------------

#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Float16(pub(crate) u16);

impl Float16 {
    pub const fn from_bits(u: u16) -> Self {
        Self(u)
    }
    pub const fn to_bits(self) -> u16 {
        self.0
    }

    pub fn is_nan(self) -> bool {
        let exp = (self.0 >> 10) & 0x1F;
        let frac = self.0 & 0x3FF;
        exp == 0x1F && frac != 0
    }
    pub fn is_inf(self) -> bool {
        let exp = (self.0 >> 10) & 0x1F;
        let frac = self.0 & 0x3FF;
        exp == 0x1F && frac == 0
    }
    pub fn is_zero(self) -> bool {
        self.0 & 0x7FFF == 0
    }
    pub fn is_neg(self) -> bool {
        self.0 & 0x8000 != 0
    }
}

impl fmt::Debug for Float16 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Float16(0x{:04X})", self.0)
    }
}

impl FloatFormat for Float16 {
    type Bits = u16;
    const EXP_BITS: u32 = 5;
    const FRAC_BITS: u32 = 10;
    const BIAS: i32 = 15;
    const HAS_EXPLICIT_INT: bool = false;
    fn to_bits(self) -> u16 {
        self.0
    }
    fn from_bits(bits: u16) -> Self {
        Self(bits)
    }
}

// ---------------------------------------------------------------
// BFloat16 -- Brain float: 1+8+7
// ---------------------------------------------------------------

#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct BFloat16(pub(crate) u16);

impl BFloat16 {
    pub const fn from_bits(u: u16) -> Self {
        Self(u)
    }
    pub const fn to_bits(self) -> u16 {
        self.0
    }

    pub fn is_nan(self) -> bool {
        let exp = (self.0 >> 7) & 0xFF;
        let frac = self.0 & 0x7F;
        exp == 0xFF && frac != 0
    }
    pub fn is_inf(self) -> bool {
        let exp = (self.0 >> 7) & 0xFF;
        let frac = self.0 & 0x7F;
        exp == 0xFF && frac == 0
    }
    pub fn is_zero(self) -> bool {
        self.0 & 0x7FFF == 0
    }
    pub fn is_neg(self) -> bool {
        self.0 & 0x8000 != 0
    }
}

impl fmt::Debug for BFloat16 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BFloat16(0x{:04X})", self.0)
    }
}

impl FloatFormat for BFloat16 {
    type Bits = u16;
    const EXP_BITS: u32 = 8;
    const FRAC_BITS: u32 = 7;
    const BIAS: i32 = 127;
    const HAS_EXPLICIT_INT: bool = false;
    fn to_bits(self) -> u16 {
        self.0
    }
    fn from_bits(bits: u16) -> Self {
        Self(bits)
    }
}

// ---------------------------------------------------------------
// Float32 -- IEEE 754 single precision: 1+8+23
// ---------------------------------------------------------------

#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Float32(pub(crate) u32);

impl Float32 {
    pub const fn from_bits(u: u32) -> Self {
        Self(u)
    }
    pub const fn to_bits(self) -> u32 {
        self.0
    }

    pub fn is_nan(self) -> bool {
        let exp = (self.0 >> 23) & 0xFF;
        let frac = self.0 & 0x7F_FFFF;
        exp == 0xFF && frac != 0
    }
    pub fn is_inf(self) -> bool {
        let exp = (self.0 >> 23) & 0xFF;
        let frac = self.0 & 0x7F_FFFF;
        exp == 0xFF && frac == 0
    }
    pub fn is_zero(self) -> bool {
        self.0 & 0x7FFF_FFFF == 0
    }
    pub fn is_neg(self) -> bool {
        self.0 & 0x8000_0000 != 0
    }
    /// Construct from a native `f32` (bit reinterpret).
    pub fn from_f32(v: f32) -> Self {
        Self(v.to_bits())
    }
    /// Convert to native `f32` (bit reinterpret).
    pub fn to_f32(self) -> f32 {
        f32::from_bits(self.0)
    }
}

impl fmt::Debug for Float32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Float32(0x{:08X})", self.0)
    }
}

impl FloatFormat for Float32 {
    type Bits = u32;
    const EXP_BITS: u32 = 8;
    const FRAC_BITS: u32 = 23;
    const BIAS: i32 = 127;
    const HAS_EXPLICIT_INT: bool = false;
    fn to_bits(self) -> u32 {
        self.0
    }
    fn from_bits(bits: u32) -> Self {
        Self(bits)
    }
}

// ---------------------------------------------------------------
// Float64 -- IEEE 754 double precision: 1+11+52
// ---------------------------------------------------------------

#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Float64(pub(crate) u64);

impl Float64 {
    pub const fn from_bits(u: u64) -> Self {
        Self(u)
    }
    pub const fn to_bits(self) -> u64 {
        self.0
    }

    pub fn is_nan(self) -> bool {
        let exp = (self.0 >> 52) & 0x7FF;
        let frac = self.0 & 0xF_FFFF_FFFF_FFFF;
        exp == 0x7FF && frac != 0
    }
    pub fn is_inf(self) -> bool {
        let exp = (self.0 >> 52) & 0x7FF;
        let frac = self.0 & 0xF_FFFF_FFFF_FFFF;
        exp == 0x7FF && frac == 0
    }
    pub fn is_zero(self) -> bool {
        self.0 & 0x7FFF_FFFF_FFFF_FFFF == 0
    }
    pub fn is_neg(self) -> bool {
        self.0 & 0x8000_0000_0000_0000 != 0
    }
    pub fn from_f64(v: f64) -> Self {
        Self(v.to_bits())
    }
    pub fn to_f64(self) -> f64 {
        f64::from_bits(self.0)
    }
}

impl fmt::Debug for Float64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Float64(0x{:016X})", self.0)
    }
}

impl FloatFormat for Float64 {
    type Bits = u64;
    const EXP_BITS: u32 = 11;
    const FRAC_BITS: u32 = 52;
    const BIAS: i32 = 1023;
    const HAS_EXPLICIT_INT: bool = false;
    fn to_bits(self) -> u64 {
        self.0
    }
    fn from_bits(bits: u64) -> Self {
        Self(bits)
    }
}

// ---------------------------------------------------------------
// Float128 -- IEEE 754 quad precision: 1+15+112
// ---------------------------------------------------------------

#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Float128(pub(crate) u128);

impl Float128 {
    pub const fn from_bits(u: u128) -> Self {
        Self(u)
    }
    pub const fn to_bits(self) -> u128 {
        self.0
    }

    pub fn is_nan(self) -> bool {
        let exp = (self.0 >> 112) & 0x7FFF;
        let frac = self.0 & 0xFFFF_FFFF_FFFF_FFFF_FFFF_FFFF_FFFF;
        exp == 0x7FFF && frac != 0
    }
    pub fn is_inf(self) -> bool {
        let exp = (self.0 >> 112) & 0x7FFF;
        let frac = self.0 & 0xFFFF_FFFF_FFFF_FFFF_FFFF_FFFF_FFFF;
        exp == 0x7FFF && frac == 0
    }
    pub fn is_zero(self) -> bool {
        self.0 & ((1u128 << 127) - 1) == 0
    }
    pub fn is_neg(self) -> bool {
        self.0 & (1u128 << 127) != 0
    }
}

impl fmt::Debug for Float128 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Float128(0x{:032X})", self.0)
    }
}

impl FloatFormat for Float128 {
    type Bits = u128;
    const EXP_BITS: u32 = 15;
    const FRAC_BITS: u32 = 112;
    const BIAS: i32 = 16383;
    const HAS_EXPLICIT_INT: bool = false;
    fn to_bits(self) -> u128 {
        self.0
    }
    fn from_bits(bits: u128) -> Self {
        Self(bits)
    }
}

// ---------------------------------------------------------------
// FloatX80 -- x87 extended precision: 1+15+64 (explicit int bit)
// ---------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FloatX80 {
    pub lo: u64,
    pub hi: u16,
}

impl FloatX80 {
    pub fn from_bits(u: u128) -> Self {
        Self {
            lo: u as u64,
            hi: (u >> 64) as u16,
        }
    }
    pub fn to_bits(self) -> u128 {
        (self.lo as u128) | ((self.hi as u128) << 64)
    }

    pub fn is_nan(self) -> bool {
        let exp = self.hi & 0x7FFF;
        // Explicit integer bit is bit 63 of lo
        if exp != 0x7FFF {
            return false;
        }
        // Inf has integer bit set and frac==0.
        // NaN has integer bit set and frac!=0, or
        // unnormal/pseudo forms.
        let j = (self.lo >> 63) & 1;
        let frac = self.lo & 0x7FFF_FFFF_FFFF_FFFF;
        if j == 1 && frac != 0 {
            return true;
        }
        // Pseudo-NaN: integer bit clear but exp==max
        if j == 0 {
            return true;
        }
        false
    }
    pub fn is_inf(self) -> bool {
        let exp = self.hi & 0x7FFF;
        if exp != 0x7FFF {
            return false;
        }
        // Integer bit must be set, fraction must be zero
        self.lo == 0x8000_0000_0000_0000
    }
    pub fn is_zero(self) -> bool {
        self.lo == 0 && (self.hi & 0x7FFF) == 0
    }
    pub fn is_neg(self) -> bool {
        self.hi & 0x8000 != 0
    }
}

impl fmt::Debug for FloatX80 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FloatX80(hi=0x{:04X}, lo=0x{:016X})", self.hi, self.lo)
    }
}

impl FloatFormat for FloatX80 {
    // Use u128 for uniformity in pack/unpack
    type Bits = u128;
    const EXP_BITS: u32 = 15;
    // 64 significand bits including explicit integer bit.
    // FRAC_BITS = 63 (fractional part only, the integer
    // bit is handled via HAS_EXPLICIT_INT).
    const FRAC_BITS: u32 = 63;
    const BIAS: i32 = 16383;
    const HAS_EXPLICIT_INT: bool = true;

    fn to_bits(self) -> u128 {
        FloatX80::to_bits(self)
    }
    fn from_bits(bits: u128) -> Self {
        FloatX80::from_bits(bits)
    }
}
