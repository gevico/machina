// SPDX-License-Identifier: MIT
// machina-softfloat: Pure software IEEE 754 floating-point library.

#![no_std]

pub mod env;
pub mod ops;
pub mod parts;
pub mod types;

pub use env::{ExcFlags, FloatEnv, RoundMode, Tininess};
pub use parts::{FloatClass, FloatParts};
pub use types::{
    BFloat16, BitOps, Float128, Float16, Float32, Float64, FloatFormat,
    FloatX80,
};
