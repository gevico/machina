// IEEE 754 edge case tests: NaN, Inf, Zero, Subnormal.

use machina_softfloat::env::{ExcFlags, FloatEnv, RoundMode};
use machina_softfloat::types::Float32;

fn env() -> FloatEnv {
    FloatEnv::new(RoundMode::NearEven)
}

const POS_INF: u32 = 0x7f80_0000;
const NEG_INF: u32 = 0xff80_0000;
const QNAN: u32 = 0x7fc0_0000; // canonical QNaN
const SNAN: u32 = 0x7f80_0001; // SNaN

// ── NaN propagation ─────────────────────────────────

#[test]
fn f32_add_nan_propagates() {
    let mut e = env();
    let nan = Float32::from_bits(QNAN);
    let one = Float32::from_f32(1.0);
    let c = nan.add(one, &mut e);
    assert!(c.is_nan());
}

#[test]
fn f32_add_snan_signals_invalid() {
    let mut e = env();
    let snan = Float32::from_bits(SNAN);
    let one = Float32::from_f32(1.0);
    let c = snan.add(one, &mut e);
    assert!(c.is_nan());
    assert!(e.flags().contains(ExcFlags::INVALID));
}

#[test]
fn f32_default_nan_mode() {
    let mut e = env();
    e.set_default_nan(true);
    let nan = Float32::from_bits(0x7f80_1234); // non-canonical QNaN
    let one = Float32::from_f32(1.0);
    let c = nan.add(one, &mut e);
    // In default-NaN mode, result is canonical QNaN
    assert_eq!(c.to_bits(), QNAN);
}

// ── Infinity arithmetic ─────────────────────────────

#[test]
fn f32_inf_plus_finite() {
    let mut e = env();
    let inf = Float32::from_bits(POS_INF);
    let one = Float32::from_f32(1.0);
    let c = inf.add(one, &mut e);
    assert_eq!(c.to_bits(), POS_INF);
    assert!(e.flags().is_empty());
}

#[test]
fn f32_inf_minus_inf_is_nan() {
    let mut e = env();
    let inf = Float32::from_bits(POS_INF);
    let c = inf.sub(inf, &mut e);
    assert!(c.is_nan());
    assert!(e.flags().contains(ExcFlags::INVALID));
}

#[test]
fn f32_inf_mul_zero_is_nan() {
    let mut e = env();
    let inf = Float32::from_bits(POS_INF);
    let zero = Float32::from_f32(0.0);
    let c = inf.mul(zero, &mut e);
    assert!(c.is_nan());
    assert!(e.flags().contains(ExcFlags::INVALID));
}

#[test]
fn f32_inf_div_inf_is_nan() {
    let mut e = env();
    let inf = Float32::from_bits(POS_INF);
    let c = inf.div(inf, &mut e);
    assert!(c.is_nan());
    assert!(e.flags().contains(ExcFlags::INVALID));
}

#[test]
fn f32_zero_div_zero_is_nan() {
    let mut e = env();
    let z = Float32::from_f32(0.0);
    let c = z.div(z, &mut e);
    assert!(c.is_nan());
    assert!(e.flags().contains(ExcFlags::INVALID));
}

#[test]
fn f32_finite_div_zero_is_inf() {
    let mut e = env();
    let a = Float32::from_f32(1.0);
    let z = Float32::from_f32(0.0);
    let c = a.div(z, &mut e);
    assert_eq!(c.to_bits(), POS_INF);
    assert!(e.flags().contains(ExcFlags::DIVBYZERO));
}

#[test]
fn f32_neg_div_zero_is_neg_inf() {
    let mut e = env();
    let a = Float32::from_f32(-1.0);
    let z = Float32::from_f32(0.0);
    let c = a.div(z, &mut e);
    assert_eq!(c.to_bits(), NEG_INF);
    assert!(e.flags().contains(ExcFlags::DIVBYZERO));
}

// ── Zero arithmetic ─────────────────────────────────

#[test]
fn f32_neg_zero_plus_pos_zero() {
    let mut e = env();
    let nz = Float32::from_bits(0x8000_0000); // -0
    let pz = Float32::from_f32(0.0); // +0
    let c = nz.add(pz, &mut e);
    // IEEE 754: -0 + (+0) = +0 in RNE mode
    assert_eq!(c.to_bits(), 0x0000_0000);
}

#[test]
fn f32_neg_zero_sub_neg_zero() {
    let mut e = env();
    let nz = Float32::from_bits(0x8000_0000);
    let c = nz.sub(nz, &mut e);
    // IEEE 754: (-0) - (-0) = +0 in RNE mode
    assert_eq!(c.to_bits(), 0x0000_0000);
}

// ── Subnormal ───────────────────────────────────────

#[test]
fn f32_smallest_subnormal() {
    let mut e = env();
    // smallest positive subnormal: 0x00000001
    let a = Float32::from_bits(0x0000_0001);
    let b = Float32::from_bits(0x0000_0001);
    let c = a.add(b, &mut e);
    // 2 * smallest subnormal = 0x00000002
    assert_eq!(c.to_bits(), 0x0000_0002);
}

// ── Sqrt edge cases ─────────────────────────────────

#[test]
fn f32_sqrt_zero() {
    let mut e = env();
    let z = Float32::from_f32(0.0);
    let c = z.sqrt(&mut e);
    assert_eq!(c.to_bits(), 0x0000_0000);
}

#[test]
fn f32_sqrt_neg_zero() {
    let mut e = env();
    let nz = Float32::from_bits(0x8000_0000);
    let c = nz.sqrt(&mut e);
    // sqrt(-0) = -0
    assert_eq!(c.to_bits(), 0x8000_0000);
}

#[test]
fn f32_sqrt_negative_is_nan() {
    let mut e = env();
    let a = Float32::from_f32(-1.0);
    let c = a.sqrt(&mut e);
    assert!(c.is_nan());
    assert!(e.flags().contains(ExcFlags::INVALID));
}

#[test]
fn f32_sqrt_inf() {
    let mut e = env();
    let inf = Float32::from_bits(POS_INF);
    let c = inf.sqrt(&mut e);
    assert_eq!(c.to_bits(), POS_INF);
}

// ── Compare edge cases ──────────────────────────────

#[test]
fn f32_nan_not_equal_to_self() {
    let mut e = env();
    let nan = Float32::from_bits(QNAN);
    assert!(!nan.eq(nan, &mut e));
}

#[test]
fn f32_pos_zero_eq_neg_zero() {
    let mut e = env();
    let pz = Float32::from_f32(0.0);
    let nz = Float32::from_bits(0x8000_0000);
    assert!(pz.eq(nz, &mut e));
}

#[test]
fn f32_nan_compare_signals_invalid() {
    let mut e = env();
    let nan = Float32::from_bits(QNAN);
    let one = Float32::from_f32(1.0);
    // lt/le with NaN should signal INVALID
    assert!(!nan.lt(one, &mut e));
    assert!(e.flags().contains(ExcFlags::INVALID));
}

// ---------------------------------------------------------------------------
// Float32 min/max edge cases (IEEE 754 representative tests)
// ---------------------------------------------------------------------------

#[test]
fn f32_minmax_zero_boundary() {
    let mut e = env();
    let p0 = Float32::from_f32(0.0);
    let n0 = Float32::from_bits(0x8000_0000); // -0.0

    // min(+0, -0) should be -0
    let mn = p0.min(n0, &mut e);
    assert_eq!(mn.to_bits(), 0x8000_0000);

    // max(+0, -0) should be +0
    let mx = p0.max(n0, &mut e);
    assert_eq!(mx.to_bits(), 0x0000_0000);
}

#[test]
fn f32_minmax_qnan_behaviour() {
    let mut e = env();
    let qnan = Float32::from_bits(QNAN);
    let finite = Float32::from_f32(3.14);

    // min(qNaN, finite) -> finite; no INVALID for qNaN
    let mn = qnan.min(finite, &mut e);
    assert_eq!(mn.to_f32(), 3.14);
    assert!(e.flags().is_empty());

    // max(qNaN, finite) -> finite; no INVALID for qNaN
    let mx = qnan.max(finite, &mut e);
    assert_eq!(mx.to_f32(), 3.14);
    assert!(e.flags().is_empty());
}

#[test]
fn f32_minmax_snan_behaviour() {
    let mut e = env();
    let snan = Float32::from_bits(SNAN);
    let finite = Float32::from_f32(3.14);

    // min(sNaN, finite) -> finite; INVALID is signalled
    let mn = snan.min(finite, &mut e);
    assert_eq!(mn.to_f32(), 3.14);
    assert!(e.flags().contains(ExcFlags::INVALID));

    // max(sNaN, finite) -> finite; INVALID is signalled
    let mx = snan.max(finite, &mut e);
    assert_eq!(mx.to_f32(), 3.14);
    assert!(e.flags().contains(ExcFlags::INVALID));
}

#[test]
fn f32_minmax_signed_ordering() {
    let mut e = env();
    let a = Float32::from_f32(-2.0);
    let b = Float32::from_f32(-1.0);
    // min(-2, -1) -> -2
    assert_eq!(a.min(b, &mut e).to_f32(), -2.0);
    // max(-2, -1) -> -1
    assert_eq!(a.max(b, &mut e).to_f32(), -1.0);

    // cross-sign: ensure correctness with a negative and a positive
    // Use -1.0 to verify cross-sign behavior
    let c = Float32::from_f32(1.0);
    let d = Float32::from_f32(-1.0);
    // min(-1.0, +1.0) -> -1.0
    assert_eq!(d.min(c, &mut e).to_f32(), -1.0);
    // max(-1.0, +1.0) -> +1.0
    assert_eq!(d.max(c, &mut e).to_f32(), 1.0);
}

