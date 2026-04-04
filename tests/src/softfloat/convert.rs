// Float-to-float and float-to-int conversion tests.

use machina_softfloat::env::{ExcFlags, FloatEnv, RoundMode};
use machina_softfloat::ops::convert;
use machina_softfloat::types::{Float16, Float32, Float64};

fn env() -> FloatEnv {
    FloatEnv::new(RoundMode::NearEven)
}

// ── Float32 → Float64 (widen) ───────────────────────

#[test]
fn f32_to_f64_exact() {
    let mut e = env();
    let a = Float32::from_f32(1.5);
    let b: Float64 = convert::convert(a, &mut e);
    assert_eq!(b.to_f64(), 1.5);
    assert!(e.flags().is_empty());
}

#[test]
fn f32_to_f64_inf() {
    let mut e = env();
    let a = Float32::from_bits(0x7f80_0000); // +inf
    let b: Float64 = convert::convert(a, &mut e);
    assert!(b.is_inf());
    assert!(!b.is_neg());
}

#[test]
fn f32_to_f64_nan() {
    let mut e = env();
    let a = Float32::from_bits(0x7fc0_0000); // QNaN
    let b: Float64 = convert::convert(a, &mut e);
    assert!(b.is_nan());
}

// ── Float64 → Float32 (narrow) ──────────────────────

#[test]
fn f64_to_f32_exact() {
    let mut e = env();
    let a = Float64::from_f64(1.5);
    let b: Float32 = convert::convert(a, &mut e);
    assert_eq!(b.to_f32(), 1.5);
    assert!(e.flags().is_empty());
}

#[test]
fn f64_to_f32_inexact() {
    let mut e = env();
    // 1.0000001 (has more precision than f32)
    let a = Float64::from_f64(1.0000001);
    let _b: Float32 = convert::convert(a, &mut e);
    assert!(e.flags().contains(ExcFlags::INEXACT));
}

// ── Float32 → int ───────────────────────────────────

#[test]
fn f32_to_i32_simple() {
    let mut e = env();
    let a = Float32::from_f32(42.0);
    let v = convert::to_i32(a, &mut e);
    assert_eq!(v, 42);
    assert!(e.flags().is_empty());
}

#[test]
fn f32_to_i32_negative() {
    let mut e = env();
    let a = Float32::from_f32(-7.0);
    let v = convert::to_i32(a, &mut e);
    assert_eq!(v, -7);
}

#[test]
fn f32_to_i32_truncates_rne() {
    let mut e = env();
    let a = Float32::from_f32(2.5);
    let v = convert::to_i32(a, &mut e);
    // RNE: 2.5 rounds to 2 (ties to even)
    assert_eq!(v, 2);
}

#[test]
fn f32_to_i32_nan_is_invalid() {
    let mut e = env();
    let nan = Float32::from_bits(0x7fc0_0000);
    let _v = convert::to_i32(nan, &mut e);
    assert!(e.flags().contains(ExcFlags::INVALID));
}

#[test]
fn f32_to_i32_overflow_is_invalid() {
    let mut e = env();
    let big = Float32::from_f32(3.0e10);
    let v = convert::to_i32(big, &mut e);
    // Should clamp to i32::MAX
    assert_eq!(v, i32::MAX);
    assert!(e.flags().contains(ExcFlags::INVALID));
}

#[test]
fn f32_to_u32_simple() {
    let mut e = env();
    let a = Float32::from_f32(100.0);
    let v = convert::to_u32(a, &mut e);
    assert_eq!(v, 100);
}

#[test]
fn f32_to_u32_negative_is_invalid() {
    let mut e = env();
    let a = Float32::from_f32(-1.0);
    let v = convert::to_u32(a, &mut e);
    assert_eq!(v, 0);
    assert!(e.flags().contains(ExcFlags::INVALID));
}

// ── int → Float32 ───────────────────────────────────

#[test]
fn i32_to_f32_simple() {
    let mut e = env();
    let v: Float32 = convert::from_i32(42, &mut e);
    assert_eq!(v.to_f32(), 42.0);
}

#[test]
fn i64_to_f32_large() {
    let mut e = env();
    let v: Float32 = convert::from_i64(1_000_000_000, &mut e);
    assert_eq!(v.to_f32(), 1.0e9);
}

#[test]
fn u64_to_f64_max() {
    let mut e = env();
    let v: Float64 = convert::from_u64(u64::MAX, &mut e);
    // u64::MAX ≈ 1.8446744e19
    assert!(v.to_f64() > 1.8e19);
}

// ── Float16 conversions ─────────────────────────────

#[test]
fn f16_to_f32() {
    let mut e = env();
    // 1.0 in float16 = 0x3C00
    let a = Float16::from_bits(0x3C00);
    let b: Float32 = convert::convert(a, &mut e);
    assert_eq!(b.to_f32(), 1.0);
}

#[test]
fn f32_to_f16() {
    let mut e = env();
    let a = Float32::from_f32(1.0);
    let b: Float16 = convert::convert(a, &mut e);
    assert_eq!(b.to_bits(), 0x3C00);
}
