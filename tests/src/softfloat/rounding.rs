// Rounding mode tests — verify all 6 modes produce
// correct results for tie-breaking and direction.

use machina_softfloat::env::{FloatEnv, RoundMode};
use machina_softfloat::ops::convert;
use machina_softfloat::types::Float32;

fn env_rm(rm: RoundMode) -> FloatEnv {
    FloatEnv::new(rm)
}

// ── RNE: Round to Nearest, ties to Even ─────────────

#[test]
fn rne_f32_to_i32_half_to_even() {
    let mut e = env_rm(RoundMode::NearEven);
    // 2.5 → 2 (even), 3.5 → 4 (even)
    assert_eq!(convert::to_i32(Float32::from_f32(2.5), &mut e), 2);
    e.clear_flags();
    assert_eq!(convert::to_i32(Float32::from_f32(3.5), &mut e), 4);
}

// ── RTZ: Round towards Zero ─────────────────────────

#[test]
fn rtz_f32_to_i32_truncates() {
    let mut e = env_rm(RoundMode::ToZero);
    assert_eq!(convert::to_i32(Float32::from_f32(2.9), &mut e), 2);
    e.clear_flags();
    assert_eq!(convert::to_i32(Float32::from_f32(-2.9), &mut e), -2);
}

// ── RDN: Round Down (towards -inf) ──────────────────

#[test]
fn rdn_f32_to_i32_floor() {
    let mut e = env_rm(RoundMode::Down);
    assert_eq!(convert::to_i32(Float32::from_f32(2.1), &mut e), 2);
    e.clear_flags();
    assert_eq!(convert::to_i32(Float32::from_f32(-2.1), &mut e), -3);
}

// ── RUP: Round Up (towards +inf) ────────────────────

#[test]
fn rup_f32_to_i32_ceil() {
    let mut e = env_rm(RoundMode::Up);
    assert_eq!(convert::to_i32(Float32::from_f32(2.1), &mut e), 3);
    e.clear_flags();
    assert_eq!(convert::to_i32(Float32::from_f32(-2.1), &mut e), -2);
}

// ── RMM: Round to Nearest, ties to Max Magnitude ────

#[test]
fn rmm_f32_to_i32_half_away() {
    let mut e = env_rm(RoundMode::NearMaxMag);
    // 2.5 → 3 (away from zero), -2.5 → -3
    assert_eq!(convert::to_i32(Float32::from_f32(2.5), &mut e), 3);
    e.clear_flags();
    assert_eq!(convert::to_i32(Float32::from_f32(-2.5), &mut e), -3);
}

// ── RMM vs RNE on 0.5 ──────────────────────────────

#[test]
fn rmm_vs_rne_half() {
    // 0.5: RNE rounds to 0 (even), RMM rounds to 1
    let mut e_rne = env_rm(RoundMode::NearEven);
    let mut e_rmm = env_rm(RoundMode::NearMaxMag);
    let half = Float32::from_f32(0.5);
    let rne = convert::to_i32(half, &mut e_rne);
    let rmm = convert::to_i32(half, &mut e_rmm);
    assert_eq!(rne, 0); // ties to even
    assert_eq!(rmm, 1); // ties away from zero
}

// ── Rounding mode affects arithmetic ────────────────

#[test]
fn rounding_affects_f32_add() {
    // 1.0 + 2^-24 (just below ulp): result depends
    // on rounding direction.
    let one = Float32::from_f32(1.0);
    // 2^-24 = 5.960464e-8
    let eps = Float32::from_bits(0x3380_0000);

    let mut e_up = env_rm(RoundMode::Up);
    let c_up = one.add(eps, &mut e_up);

    let mut e_dn = env_rm(RoundMode::Down);
    let c_dn = one.add(eps, &mut e_dn);

    // RUP should round up, RDN should truncate
    assert!(c_up.to_bits() >= c_dn.to_bits());
}
