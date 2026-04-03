// SPDX-License-Identifier: MIT
// Floating-point environment: rounding mode, exception flags, tininess.

use core::ops::{BitOr, BitOrAssign};

// ---------------------------------------------------------------
// Rounding mode
// ---------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum RoundMode {
    #[default]
    NearEven = 0,
    ToZero = 1,
    Down = 2,
    Up = 3,
    NearMaxMag = 4,
    Odd = 5,
}

// ---------------------------------------------------------------
// Exception flags (bitflags-style)
// ---------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExcFlags(pub u8);

impl ExcFlags {
    pub const NONE: Self = Self(0);
    pub const INVALID: Self = Self(1);
    pub const DIVBYZERO: Self = Self(2);
    pub const OVERFLOW: Self = Self(4);
    pub const UNDERFLOW: Self = Self(8);
    pub const INEXACT: Self = Self(16);

    #[inline]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    #[inline]
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

impl BitOr for ExcFlags {
    type Output = Self;
    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for ExcFlags {
    #[inline]
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

// ---------------------------------------------------------------
// Tininess detection mode
// ---------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tininess {
    BeforeRounding,
    #[default]
    AfterRounding,
}

// ---------------------------------------------------------------
// Floating-point environment
// ---------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct FloatEnv {
    round_mode: RoundMode,
    flags: ExcFlags,
    tininess: Tininess,
    default_nan: bool,
}

impl FloatEnv {
    pub fn new(rm: RoundMode) -> Self {
        Self {
            round_mode: rm,
            flags: ExcFlags::NONE,
            tininess: Tininess::AfterRounding,
            default_nan: false,
        }
    }

    #[inline]
    pub fn round_mode(&self) -> RoundMode {
        self.round_mode
    }
    #[inline]
    pub fn set_round_mode(&mut self, rm: RoundMode) {
        self.round_mode = rm;
    }

    #[inline]
    pub fn flags(&self) -> ExcFlags {
        self.flags
    }
    #[inline]
    pub fn set_flags(&mut self, f: ExcFlags) {
        self.flags = f;
    }
    #[inline]
    pub fn raise(&mut self, f: ExcFlags) {
        self.flags |= f;
    }
    #[inline]
    pub fn clear_flags(&mut self) {
        self.flags = ExcFlags::NONE;
    }

    #[inline]
    pub fn tininess(&self) -> Tininess {
        self.tininess
    }
    #[inline]
    pub fn set_tininess(&mut self, t: Tininess) {
        self.tininess = t;
    }

    #[inline]
    pub fn default_nan(&self) -> bool {
        self.default_nan
    }
    #[inline]
    pub fn set_default_nan(&mut self, v: bool) {
        self.default_nan = v;
    }
}

impl Default for FloatEnv {
    fn default() -> Self {
        Self::new(RoundMode::NearEven)
    }
}
