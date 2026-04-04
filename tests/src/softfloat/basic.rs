// Basic arithmetic correctness tests for softfloat.

use machina_softfloat::env::{FloatEnv, RoundMode};
use machina_softfloat::types::{Float32, Float64};

fn env() -> FloatEnv {
    FloatEnv::new(RoundMode::NearEven)
}

// ── Float32 addition ────────────────────────────────

#[test]
fn f32_add_simple() {
    let mut e = env();
    let a = Float32::from_f32(1.0);
    let b = Float32::from_f32(2.0);
    let c = a.add(b, &mut e);
    assert_eq!(c.to_f32(), 3.0);
    assert!(e.flags().is_empty());
}

#[test]
fn f32_add_negative() {
    let mut e = env();
    let a = Float32::from_f32(1.0);
    let b = Float32::from_f32(-3.0);
    let c = a.add(b, &mut e);
    assert_eq!(c.to_f32(), -2.0);
    assert!(e.flags().is_empty());
}

#[test]
fn f32_add_zero_plus_zero() {
    let mut e = env();
    let z = Float32::from_f32(0.0);
    let c = z.add(z, &mut e);
    assert_eq!(c.to_bits(), 0x0000_0000); // +0
    assert!(e.flags().is_empty());
}

// ── Float32 subtraction ─────────────────────────────

#[test]
fn f32_sub_simple() {
    let mut e = env();
    let a = Float32::from_f32(5.0);
    let b = Float32::from_f32(3.0);
    let c = a.sub(b, &mut e);
    assert_eq!(c.to_f32(), 2.0);
}

#[test]
fn f32_sub_equal() {
    let mut e = env();
    let a = Float32::from_f32(42.0);
    let c = a.sub(a, &mut e);
    assert_eq!(c.to_bits(), 0x0000_0000); // +0
}

// ── Float32 multiplication ──────────────────────────

#[test]
fn f32_mul_simple() {
    let mut e = env();
    let a = Float32::from_f32(3.0);
    let b = Float32::from_f32(4.0);
    let c = a.mul(b, &mut e);
    assert_eq!(c.to_f32(), 12.0);
    assert!(e.flags().is_empty());
}

#[test]
fn f32_mul_by_zero() {
    let mut e = env();
    let a = Float32::from_f32(42.0);
    let z = Float32::from_f32(0.0);
    let c = a.mul(z, &mut e);
    assert_eq!(c.to_bits(), 0x0000_0000); // +0
    assert!(e.flags().is_empty());
}

#[test]
fn f32_mul_negative() {
    let mut e = env();
    let a = Float32::from_f32(-2.0);
    let b = Float32::from_f32(3.0);
    let c = a.mul(b, &mut e);
    assert_eq!(c.to_f32(), -6.0);
}

// ── Float32 division ────────────────────────────────

#[test]
fn f32_div_simple() {
    let mut e = env();
    let a = Float32::from_f32(10.0);
    let b = Float32::from_f32(2.0);
    let c = a.div(b, &mut e);
    assert_eq!(c.to_f32(), 5.0);
    assert!(e.flags().is_empty());
}

#[test]
fn f32_div_one() {
    let mut e = env();
    let a = Float32::from_f32(7.0);
    let b = Float32::from_f32(1.0);
    let c = a.div(b, &mut e);
    assert_eq!(c.to_f32(), 7.0);
}

// ── Float32 comparison ──────────────────────────────

#[test]
fn f32_compare_eq() {
    let mut e = env();
    let a = Float32::from_f32(1.0);
    let b = Float32::from_f32(1.0);
    assert!(a.eq(b, &mut e));
}

#[test]
fn f32_compare_lt() {
    let mut e = env();
    let a = Float32::from_f32(1.0);
    let b = Float32::from_f32(2.0);
    assert!(a.lt(b, &mut e));
    assert!(a.le(b, &mut e));
    assert!(!b.lt(a, &mut e));
}

// ── Float64 basic ───────────────────────────────────

#[test]
fn f64_add_simple() {
    let mut e = env();
    let a = Float64::from_f64(1.0);
    let b = Float64::from_f64(2.0);
    let c = a.add(b, &mut e);
    assert_eq!(c.to_f64(), 3.0);
    assert!(e.flags().is_empty());
}

#[test]
fn f64_mul_simple() {
    let mut e = env();
    let a = Float64::from_f64(3.0);
    let b = Float64::from_f64(4.0);
    let c = a.mul(b, &mut e);
    assert_eq!(c.to_f64(), 12.0);
}

#[test]
fn f64_div_simple() {
    let mut e = env();
    let a = Float64::from_f64(10.0);
    let b = Float64::from_f64(4.0);
    let c = a.div(b, &mut e);
    assert_eq!(c.to_f64(), 2.5);
}

// ── Float32 sqrt ────────────────────────────────────

#[test]
fn f32_sqrt_perfect() {
    let mut e = env();
    let a = Float32::from_f32(4.0);
    let c = a.sqrt(&mut e);
    assert_eq!(c.to_f32(), 2.0);
    assert!(e.flags().is_empty());
}

#[test]
fn f32_sqrt_one() {
    let mut e = env();
    let a = Float32::from_f32(1.0);
    let c = a.sqrt(&mut e);
    assert_eq!(c.to_f32(), 1.0);
}

// ── Float32 min/max ─────────────────────────────────

#[test]
fn f32_min_max() {
    let mut e = env();
    let a = Float32::from_f32(1.0);
    let b = Float32::from_f32(2.0);
    assert_eq!(a.min(b, &mut e).to_f32(), 1.0);
    assert_eq!(a.max(b, &mut e).to_f32(), 2.0);
}
