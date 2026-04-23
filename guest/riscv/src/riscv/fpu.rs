// RISC-V floating-point helpers using pure software IEEE 754.
//
// All arithmetic, conversion, comparison, and classification
// operations use machina_softfloat instead of host FPU / C fenv.

use machina_softfloat::env::{ExcFlags, FloatEnv, RoundMode};
use machina_softfloat::ops::{compare, convert, minmax};
use machina_softfloat::types::{Float16, Float32, Float64};

use super::cpu::RiscvCpu;

// ---------------------------------------------------------------
// RISC-V fflags / frm constants
// ---------------------------------------------------------------

pub const FFLAGS_NV: u64 = 1 << 4;
pub const FFLAGS_DZ: u64 = 1 << 3;
pub const FFLAGS_OF: u64 = 1 << 2;
pub const FFLAGS_UF: u64 = 1 << 1;
pub const FFLAGS_NX: u64 = 1 << 0;
pub const FFLAGS_MASK: u64 = 0x1f;

pub const FRM_MASK: u64 = 0x7;
pub const FRM_DYN: u64 = 0x7;

const FCSR_RD_SHIFT: u64 = 5;

// ---------------------------------------------------------------
// Softfloat environment helpers
// ---------------------------------------------------------------

/// Convert softfloat ExcFlags to RISC-V fflags bits.
fn flags_to_rv(f: ExcFlags) -> u64 {
    let mut rv = 0u64;
    if f.contains(ExcFlags::INVALID) {
        rv |= FFLAGS_NV;
    }
    if f.contains(ExcFlags::DIVBYZERO) {
        rv |= FFLAGS_DZ;
    }
    if f.contains(ExcFlags::OVERFLOW) {
        rv |= FFLAGS_OF;
    }
    if f.contains(ExcFlags::UNDERFLOW) {
        rv |= FFLAGS_UF;
    }
    if f.contains(ExcFlags::INEXACT) {
        rv |= FFLAGS_NX;
    }
    rv
}

/// Create a FloatEnv from RISC-V rounding mode.
/// If rm == 7 (DYN), read from cpu.frm.
/// Returns None if rm is illegal (5 or 6).
fn make_env(cpu: &RiscvCpu, rm: u64) -> Option<FloatEnv> {
    let rm_val = if (rm & FRM_MASK) == FRM_DYN {
        cpu.frm & FRM_MASK
    } else {
        rm & FRM_MASK
    };
    let mode = match rm_val {
        0 => RoundMode::NearEven,
        1 => RoundMode::ToZero,
        2 => RoundMode::Down,
        3 => RoundMode::Up,
        4 => RoundMode::NearMaxMag,
        _ => return None,
    };
    let mut env = FloatEnv::new(mode);
    env.set_default_nan(true);
    Some(env)
}

/// Create a FloatEnv, falling back to RNE + INVALID on
/// illegal rm.
fn make_env_or_invalid(cpu: &mut RiscvCpu, rm: u64) -> FloatEnv {
    match make_env(cpu, rm) {
        Some(e) => e,
        None => {
            cpu.fflags = (cpu.fflags | FFLAGS_NV) & FFLAGS_MASK;
            let mut e = FloatEnv::new(RoundMode::NearEven);
            e.set_default_nan(true);
            e
        }
    }
}

/// Accumulate softfloat flags into cpu.fflags.
fn commit_flags(cpu: &mut RiscvCpu, fe: &FloatEnv) {
    let rv = flags_to_rv(fe.flags());
    cpu.fflags = (cpu.fflags | rv) & FFLAGS_MASK;
}

/// Comparison / min-max environment: RNE + default-NaN.
fn make_cmp_env() -> FloatEnv {
    let mut env = FloatEnv::new(RoundMode::NearEven);
    env.set_default_nan(true);
    env
}

// ---------------------------------------------------------------
// NaN boxing (f32 in 64-bit FP register)
// ---------------------------------------------------------------

fn nanbox(bits: u32) -> u64 {
    0xffff_ffff_0000_0000u64 | (bits as u64)
}

fn unbox_f32(raw: u64) -> Float32 {
    if (raw >> 32) as u32 != 0xffff_ffff {
        Float32::from_bits(0x7fc0_0000) // canonical NaN
    } else {
        Float32::from_bits(raw as u32)
    }
}

// ---------------------------------------------------------------
// Sign helpers for FMA variants
// ---------------------------------------------------------------

fn neg_f32(v: Float32) -> Float32 {
    Float32::from_bits(v.to_bits() ^ 0x8000_0000)
}

fn neg_f64(v: Float64) -> Float64 {
    Float64::from_bits(v.to_bits() ^ 0x8000_0000_0000_0000)
}

// ---------------------------------------------------------------
// NaN boxing (f16 in 64-bit FP register)
// ---------------------------------------------------------------

fn nanbox_h(bits: u16) -> u64 {
    0xffff_ffff_ffff_0000u64 | (bits as u64)
}

fn unbox_f16(raw: u64) -> Float16 {
    if (raw >> 16) != 0xffff_ffff_ffff {
        Float16::from_bits(0x7e00) // canonical NaN
    } else {
        Float16::from_bits(raw as u16)
    }
}

fn neg_f16(v: Float16) -> Float16 {
    Float16::from_bits(v.to_bits() ^ 0x8000)
}

// ---------------------------------------------------------------
// Classification (shared logic for f16/f32/f64)
// ---------------------------------------------------------------

fn fclass_bits(
    sign: bool,
    exp: u64,
    frac: u64,
    exp_max: u64,
    qnan_bit: u64,
) -> u64 {
    let is_inf = exp == exp_max && frac == 0;
    let is_nan = exp == exp_max && frac != 0;
    let is_zero = exp == 0 && frac == 0;
    let is_sub = exp == 0 && frac != 0;
    let is_norm = exp != 0 && exp != exp_max;
    let is_snan = is_nan && (frac & qnan_bit) == 0;
    let is_qnan = is_nan && !is_snan;
    match () {
        _ if is_inf && sign => 1 << 0,
        _ if is_norm && sign => 1 << 1,
        _ if is_sub && sign => 1 << 2,
        _ if is_zero && sign => 1 << 3,
        _ if is_zero => 1 << 4,
        _ if is_sub => 1 << 5,
        _ if is_norm => 1 << 6,
        _ if is_inf => 1 << 7,
        _ if is_snan => 1 << 8,
        _ if is_qnan => 1 << 9,
        _ => 0,
    }
}

fn fclass_f16(bits: u16) -> u64 {
    fclass_bits(
        (bits >> 15) != 0,
        ((bits >> 10) & 0x1f) as u64,
        (bits & 0x3ff) as u64,
        0x1f,
        1 << 9,
    )
}

fn fclass_f32(bits: u32) -> u64 {
    fclass_bits(
        (bits >> 31) != 0,
        ((bits >> 23) & 0xff) as u64,
        (bits & 0x7f_ffff) as u64,
        0xff,
        1 << 22,
    )
}

fn fclass_f64(bits: u64) -> u64 {
    fclass_bits(
        (bits >> 63) != 0,
        (bits >> 52) & 0x7ff,
        bits & 0x000f_ffff_ffff_ffff,
        0x7ff,
        1 << 51,
    )
}

// ===============================================================
// F extension (single-precision) helpers
// ===============================================================

// ---------------------------------------------------------------
// Arithmetic
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fadd_s(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res = unbox_f32(a).add(unbox_f32(b), &mut fe);
    commit_flags(cpu, &fe);
    nanbox(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fsub_s(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res = unbox_f32(a).sub(unbox_f32(b), &mut fe);
    commit_flags(cpu, &fe);
    nanbox(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fmul_s(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res = unbox_f32(a).mul(unbox_f32(b), &mut fe);
    commit_flags(cpu, &fe);
    nanbox(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fdiv_s(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res = unbox_f32(a).div(unbox_f32(b), &mut fe);
    commit_flags(cpu, &fe);
    nanbox(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fsqrt_s(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res = unbox_f32(a).sqrt(&mut fe);
    commit_flags(cpu, &fe);
    nanbox(res.to_bits())
}

// ---------------------------------------------------------------
// Fused multiply-add variants
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fmadd_s(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    c: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res = unbox_f32(a).fma(unbox_f32(b), unbox_f32(c), &mut fe);
    commit_flags(cpu, &fe);
    nanbox(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fmsub_s(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    c: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let nc = neg_f32(unbox_f32(c));
    let res = unbox_f32(a).fma(unbox_f32(b), nc, &mut fe);
    commit_flags(cpu, &fe);
    nanbox(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fnmsub_s(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    c: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let na = neg_f32(unbox_f32(a));
    let res = na.fma(unbox_f32(b), unbox_f32(c), &mut fe);
    commit_flags(cpu, &fe);
    nanbox(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fnmadd_s(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    c: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let na = neg_f32(unbox_f32(a));
    let nc = neg_f32(unbox_f32(c));
    let res = na.fma(unbox_f32(b), nc, &mut fe);
    commit_flags(cpu, &fe);
    nanbox(res.to_bits())
}

// ---------------------------------------------------------------
// Sign injection (pure bit manipulation)
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fsgnj_s(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
) -> u64 {
    let _ = env;
    let ab = unbox_f32(a).to_bits();
    let bb = unbox_f32(b).to_bits();
    let sign = bb & 0x8000_0000;
    nanbox((ab & 0x7fff_ffff) | sign)
}

#[no_mangle]
pub extern "sysv64" fn helper_fsgnjn_s(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
) -> u64 {
    let _ = env;
    let ab = unbox_f32(a).to_bits();
    let bb = unbox_f32(b).to_bits();
    let sign = (!bb) & 0x8000_0000;
    nanbox((ab & 0x7fff_ffff) | sign)
}

#[no_mangle]
pub extern "sysv64" fn helper_fsgnjx_s(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
) -> u64 {
    let _ = env;
    let ab = unbox_f32(a).to_bits();
    let bb = unbox_f32(b).to_bits();
    let sign = (ab ^ bb) & 0x8000_0000;
    nanbox((ab & 0x7fff_ffff) | sign)
}

// ---------------------------------------------------------------
// Min / Max
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fmin_s(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_cmp_env();
    let res = minmax::min(unbox_f32(a), unbox_f32(b), &mut fe);
    commit_flags(cpu, &fe);
    nanbox(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fmax_s(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_cmp_env();
    let res = minmax::max(unbox_f32(a), unbox_f32(b), &mut fe);
    commit_flags(cpu, &fe);
    nanbox(res.to_bits())
}

// ---------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_feq_s(env: *mut RiscvCpu, a: u64, b: u64) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_cmp_env();
    let res = compare::eq(unbox_f32(a), unbox_f32(b), &mut fe);
    commit_flags(cpu, &fe);
    res as u64
}

#[no_mangle]
pub extern "sysv64" fn helper_flt_s(env: *mut RiscvCpu, a: u64, b: u64) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_cmp_env();
    let res = compare::lt(unbox_f32(a), unbox_f32(b), &mut fe);
    commit_flags(cpu, &fe);
    res as u64
}

#[no_mangle]
pub extern "sysv64" fn helper_fle_s(env: *mut RiscvCpu, a: u64, b: u64) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_cmp_env();
    let res = compare::le(unbox_f32(a), unbox_f32(b), &mut fe);
    commit_flags(cpu, &fe);
    res as u64
}

// ---------------------------------------------------------------
// Classification
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fclass_s(env: *mut RiscvCpu, a: u64) -> u64 {
    let _ = env;
    fclass_f32(unbox_f32(a).to_bits())
}

// ---------------------------------------------------------------
// Float-to-integer conversions (single)
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_w_s(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let val: i32 = convert::to_i32(unbox_f32(a), &mut fe);
    commit_flags(cpu, &fe);
    val as i64 as u64
}

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_wu_s(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let val: u32 = convert::to_u32(unbox_f32(a), &mut fe);
    commit_flags(cpu, &fe);
    val as i32 as i64 as u64
}

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_l_s(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let val: i64 = convert::to_i64(unbox_f32(a), &mut fe);
    commit_flags(cpu, &fe);
    val as u64
}

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_lu_s(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let val: u64 = convert::to_u64(unbox_f32(a), &mut fe);
    commit_flags(cpu, &fe);
    val
}

// ---------------------------------------------------------------
// Integer-to-float conversions (single)
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_s_w(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res: Float32 = convert::from_i32(a as i32, &mut fe);
    commit_flags(cpu, &fe);
    nanbox(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_s_wu(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res: Float32 = convert::from_u32(a as u32, &mut fe);
    commit_flags(cpu, &fe);
    nanbox(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_s_l(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res: Float32 = convert::from_i64(a as i64, &mut fe);
    commit_flags(cpu, &fe);
    nanbox(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_s_lu(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res: Float32 = convert::from_u64(a, &mut fe);
    commit_flags(cpu, &fe);
    nanbox(res.to_bits())
}

// ===============================================================
// D extension (double-precision) helpers
// ===============================================================

// ---------------------------------------------------------------
// Arithmetic
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fadd_d(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let af = Float64::from_bits(a);
    let bf = Float64::from_bits(b);
    let res = af.add(bf, &mut fe);
    commit_flags(cpu, &fe);
    res.to_bits()
}

#[no_mangle]
pub extern "sysv64" fn helper_fsub_d(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let af = Float64::from_bits(a);
    let bf = Float64::from_bits(b);
    let res = af.sub(bf, &mut fe);
    commit_flags(cpu, &fe);
    res.to_bits()
}

#[no_mangle]
pub extern "sysv64" fn helper_fmul_d(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let af = Float64::from_bits(a);
    let bf = Float64::from_bits(b);
    let res = af.mul(bf, &mut fe);
    commit_flags(cpu, &fe);
    res.to_bits()
}

#[no_mangle]
pub extern "sysv64" fn helper_fdiv_d(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let af = Float64::from_bits(a);
    let bf = Float64::from_bits(b);
    let res = af.div(bf, &mut fe);
    commit_flags(cpu, &fe);
    res.to_bits()
}

#[no_mangle]
pub extern "sysv64" fn helper_fsqrt_d(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res = Float64::from_bits(a).sqrt(&mut fe);
    commit_flags(cpu, &fe);
    res.to_bits()
}

// ---------------------------------------------------------------
// Fused multiply-add variants (double)
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fmadd_d(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    c: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let af = Float64::from_bits(a);
    let bf = Float64::from_bits(b);
    let cf = Float64::from_bits(c);
    let res = af.fma(bf, cf, &mut fe);
    commit_flags(cpu, &fe);
    res.to_bits()
}

#[no_mangle]
pub extern "sysv64" fn helper_fmsub_d(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    c: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let af = Float64::from_bits(a);
    let bf = Float64::from_bits(b);
    let nc = neg_f64(Float64::from_bits(c));
    let res = af.fma(bf, nc, &mut fe);
    commit_flags(cpu, &fe);
    res.to_bits()
}

#[no_mangle]
pub extern "sysv64" fn helper_fnmsub_d(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    c: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let na = neg_f64(Float64::from_bits(a));
    let bf = Float64::from_bits(b);
    let cf = Float64::from_bits(c);
    let res = na.fma(bf, cf, &mut fe);
    commit_flags(cpu, &fe);
    res.to_bits()
}

#[no_mangle]
pub extern "sysv64" fn helper_fnmadd_d(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    c: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let na = neg_f64(Float64::from_bits(a));
    let bf = Float64::from_bits(b);
    let nc = neg_f64(Float64::from_bits(c));
    let res = na.fma(bf, nc, &mut fe);
    commit_flags(cpu, &fe);
    res.to_bits()
}

// ---------------------------------------------------------------
// Sign injection (double, pure bit manipulation)
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fsgnj_d(
    _env: *mut RiscvCpu,
    a: u64,
    b: u64,
) -> u64 {
    let sign = b & (1u64 << 63);
    (a & !(1u64 << 63)) | sign
}

#[no_mangle]
pub extern "sysv64" fn helper_fsgnjn_d(
    _env: *mut RiscvCpu,
    a: u64,
    b: u64,
) -> u64 {
    let sign = (!b) & (1u64 << 63);
    (a & !(1u64 << 63)) | sign
}

#[no_mangle]
pub extern "sysv64" fn helper_fsgnjx_d(
    _env: *mut RiscvCpu,
    a: u64,
    b: u64,
) -> u64 {
    let sign = (a ^ b) & (1u64 << 63);
    (a & !(1u64 << 63)) | sign
}

// ---------------------------------------------------------------
// Min / Max (double)
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fmin_d(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_cmp_env();
    let af = Float64::from_bits(a);
    let bf = Float64::from_bits(b);
    let res = minmax::min(af, bf, &mut fe);
    commit_flags(cpu, &fe);
    res.to_bits()
}

#[no_mangle]
pub extern "sysv64" fn helper_fmax_d(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_cmp_env();
    let af = Float64::from_bits(a);
    let bf = Float64::from_bits(b);
    let res = minmax::max(af, bf, &mut fe);
    commit_flags(cpu, &fe);
    res.to_bits()
}

// ---------------------------------------------------------------
// Comparison (double)
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_feq_d(env: *mut RiscvCpu, a: u64, b: u64) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_cmp_env();
    let af = Float64::from_bits(a);
    let bf = Float64::from_bits(b);
    let res = compare::eq(af, bf, &mut fe);
    commit_flags(cpu, &fe);
    res as u64
}

#[no_mangle]
pub extern "sysv64" fn helper_flt_d(env: *mut RiscvCpu, a: u64, b: u64) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_cmp_env();
    let af = Float64::from_bits(a);
    let bf = Float64::from_bits(b);
    let res = compare::lt(af, bf, &mut fe);
    commit_flags(cpu, &fe);
    res as u64
}

#[no_mangle]
pub extern "sysv64" fn helper_fle_d(env: *mut RiscvCpu, a: u64, b: u64) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_cmp_env();
    let af = Float64::from_bits(a);
    let bf = Float64::from_bits(b);
    let res = compare::le(af, bf, &mut fe);
    commit_flags(cpu, &fe);
    res as u64
}

// ---------------------------------------------------------------
// Classification (double)
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fclass_d(env: *mut RiscvCpu, a: u64) -> u64 {
    let _ = env;
    fclass_f64(a)
}

// ---------------------------------------------------------------
// Float-to-integer conversions (double)
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_w_d(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let val: i32 = convert::to_i32(Float64::from_bits(a), &mut fe);
    commit_flags(cpu, &fe);
    val as i64 as u64
}

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_wu_d(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let val: u32 = convert::to_u32(Float64::from_bits(a), &mut fe);
    commit_flags(cpu, &fe);
    val as i32 as i64 as u64
}

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_l_d(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let val: i64 = convert::to_i64(Float64::from_bits(a), &mut fe);
    commit_flags(cpu, &fe);
    val as u64
}

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_lu_d(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let val: u64 = convert::to_u64(Float64::from_bits(a), &mut fe);
    commit_flags(cpu, &fe);
    val
}

// ---------------------------------------------------------------
// Integer-to-float conversions (double)
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_d_w(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res: Float64 = convert::from_i32(a as i32, &mut fe);
    commit_flags(cpu, &fe);
    res.to_bits()
}

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_d_wu(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res: Float64 = convert::from_u32(a as u32, &mut fe);
    commit_flags(cpu, &fe);
    res.to_bits()
}

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_d_l(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res: Float64 = convert::from_i64(a as i64, &mut fe);
    commit_flags(cpu, &fe);
    res.to_bits()
}

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_d_lu(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res: Float64 = convert::from_u64(a, &mut fe);
    commit_flags(cpu, &fe);
    res.to_bits()
}

// ---------------------------------------------------------------
// Cross-format conversions
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_s_d(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res: Float32 = convert::convert(Float64::from_bits(a), &mut fe);
    commit_flags(cpu, &fe);
    nanbox(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_d_s(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res: Float64 = convert::convert(unbox_f32(a), &mut fe);
    commit_flags(cpu, &fe);
    res.to_bits()
}

// ===============================================================
// FCSR helpers (read/write, no FP operations)
// ===============================================================

#[no_mangle]
pub extern "sysv64" fn helper_fcsr_read(env: *mut RiscvCpu) -> u64 {
    let env = unsafe { &mut *env };
    let fflags = env.fflags & FFLAGS_MASK;
    let frm = env.frm & FRM_MASK;
    fflags | (frm << FCSR_RD_SHIFT)
}

#[no_mangle]
pub extern "sysv64" fn helper_fcsr_write(env: *mut RiscvCpu, val: u64) -> u64 {
    let env = unsafe { &mut *env };
    let old =
        (env.fflags & FFLAGS_MASK) | ((env.frm & FRM_MASK) << FCSR_RD_SHIFT);
    env.fflags = val & FFLAGS_MASK;
    env.frm = (val >> FCSR_RD_SHIFT) & FRM_MASK;
    old
}

// ===============================================================
// Zfh (half-precision) helpers
// ===============================================================

// ---------------------------------------------------------------
// Arithmetic
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fadd_h(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res = unbox_f16(a).add(unbox_f16(b), &mut fe);
    commit_flags(cpu, &fe);
    nanbox_h(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fsub_h(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res = unbox_f16(a).sub(unbox_f16(b), &mut fe);
    commit_flags(cpu, &fe);
    nanbox_h(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fmul_h(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res = unbox_f16(a).mul(unbox_f16(b), &mut fe);
    commit_flags(cpu, &fe);
    nanbox_h(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fdiv_h(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res = unbox_f16(a).div(unbox_f16(b), &mut fe);
    commit_flags(cpu, &fe);
    nanbox_h(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fsqrt_h(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res = unbox_f16(a).sqrt(&mut fe);
    commit_flags(cpu, &fe);
    nanbox_h(res.to_bits())
}

// ---------------------------------------------------------------
// Fused multiply-add variants (half)
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fmadd_h(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    c: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res = unbox_f16(a).fma(unbox_f16(b), unbox_f16(c), &mut fe);
    commit_flags(cpu, &fe);
    nanbox_h(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fmsub_h(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    c: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let nc = neg_f16(unbox_f16(c));
    let res = unbox_f16(a).fma(unbox_f16(b), nc, &mut fe);
    commit_flags(cpu, &fe);
    nanbox_h(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fnmsub_h(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    c: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let na = neg_f16(unbox_f16(a));
    let res = na.fma(unbox_f16(b), unbox_f16(c), &mut fe);
    commit_flags(cpu, &fe);
    nanbox_h(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fnmadd_h(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
    c: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let na = neg_f16(unbox_f16(a));
    let nc = neg_f16(unbox_f16(c));
    let res = na.fma(unbox_f16(b), nc, &mut fe);
    commit_flags(cpu, &fe);
    nanbox_h(res.to_bits())
}

// ---------------------------------------------------------------
// Sign injection (half, pure bit manipulation)
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fsgnj_h(
    _env: *mut RiscvCpu,
    a: u64,
    b: u64,
) -> u64 {
    let ab = unbox_f16(a).to_bits();
    let bb = unbox_f16(b).to_bits();
    let sign = bb & 0x8000;
    nanbox_h((ab & 0x7fff) | sign)
}

#[no_mangle]
pub extern "sysv64" fn helper_fsgnjn_h(
    _env: *mut RiscvCpu,
    a: u64,
    b: u64,
) -> u64 {
    let ab = unbox_f16(a).to_bits();
    let bb = unbox_f16(b).to_bits();
    let sign = (!bb) & 0x8000;
    nanbox_h((ab & 0x7fff) | sign)
}

#[no_mangle]
pub extern "sysv64" fn helper_fsgnjx_h(
    _env: *mut RiscvCpu,
    a: u64,
    b: u64,
) -> u64 {
    let ab = unbox_f16(a).to_bits();
    let bb = unbox_f16(b).to_bits();
    let sign = (ab ^ bb) & 0x8000;
    nanbox_h((ab & 0x7fff) | sign)
}

// ---------------------------------------------------------------
// Min / Max (half)
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fmin_h(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = FloatEnv::new(RoundMode::NearEven);
    fe.set_default_nan(true);
    let res = minmax::min(unbox_f16(a), unbox_f16(b), &mut fe);
    commit_flags(cpu, &fe);
    nanbox_h(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fmax_h(
    env: *mut RiscvCpu,
    a: u64,
    b: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = FloatEnv::new(RoundMode::NearEven);
    fe.set_default_nan(true);
    let res = minmax::max(unbox_f16(a), unbox_f16(b), &mut fe);
    commit_flags(cpu, &fe);
    nanbox_h(res.to_bits())
}

// ---------------------------------------------------------------
// Comparison (half)
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_feq_h(env: *mut RiscvCpu, a: u64, b: u64) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = FloatEnv::new(RoundMode::NearEven);
    fe.set_default_nan(true);
    let res = compare::eq(unbox_f16(a), unbox_f16(b), &mut fe);
    commit_flags(cpu, &fe);
    res as u64
}

#[no_mangle]
pub extern "sysv64" fn helper_flt_h(env: *mut RiscvCpu, a: u64, b: u64) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = FloatEnv::new(RoundMode::NearEven);
    fe.set_default_nan(true);
    let res = compare::lt(unbox_f16(a), unbox_f16(b), &mut fe);
    commit_flags(cpu, &fe);
    res as u64
}

#[no_mangle]
pub extern "sysv64" fn helper_fle_h(env: *mut RiscvCpu, a: u64, b: u64) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = FloatEnv::new(RoundMode::NearEven);
    fe.set_default_nan(true);
    let res = compare::le(unbox_f16(a), unbox_f16(b), &mut fe);
    commit_flags(cpu, &fe);
    res as u64
}

// ---------------------------------------------------------------
// Classification (half)
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fclass_h(_env: *mut RiscvCpu, a: u64) -> u64 {
    fclass_f16(unbox_f16(a).to_bits())
}

// ---------------------------------------------------------------
// Float-to-integer conversions (half)
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_w_h(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let val: i32 = convert::to_i32(unbox_f16(a), &mut fe);
    commit_flags(cpu, &fe);
    val as i64 as u64
}

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_wu_h(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let val: u32 = convert::to_u32(unbox_f16(a), &mut fe);
    commit_flags(cpu, &fe);
    val as i32 as i64 as u64
}

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_l_h(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let val: i64 = convert::to_i64(unbox_f16(a), &mut fe);
    commit_flags(cpu, &fe);
    val as u64
}

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_lu_h(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let val: u64 = convert::to_u64(unbox_f16(a), &mut fe);
    commit_flags(cpu, &fe);
    val
}

// ---------------------------------------------------------------
// Integer-to-float conversions (half)
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_h_w(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res: Float16 = convert::from_i32(a as i32, &mut fe);
    commit_flags(cpu, &fe);
    nanbox_h(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_h_wu(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res: Float16 = convert::from_u32(a as u32, &mut fe);
    commit_flags(cpu, &fe);
    nanbox_h(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_h_l(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res: Float16 = convert::from_i64(a as i64, &mut fe);
    commit_flags(cpu, &fe);
    nanbox_h(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_h_lu(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res: Float16 = convert::from_u64(a, &mut fe);
    commit_flags(cpu, &fe);
    nanbox_h(res.to_bits())
}

// ---------------------------------------------------------------
// Cross-format conversions (half <-> single / double)
// ---------------------------------------------------------------

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_s_h(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res: Float32 = convert::convert(unbox_f16(a), &mut fe);
    commit_flags(cpu, &fe);
    nanbox(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_h_s(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res: Float16 = convert::convert(unbox_f32(a), &mut fe);
    commit_flags(cpu, &fe);
    nanbox_h(res.to_bits())
}

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_d_h(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res: Float64 = convert::convert(unbox_f16(a), &mut fe);
    commit_flags(cpu, &fe);
    res.to_bits()
}

#[no_mangle]
pub extern "sysv64" fn helper_fcvt_h_d(
    env: *mut RiscvCpu,
    a: u64,
    rm: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    let mut fe = make_env_or_invalid(cpu, rm);
    let res: Float16 = convert::convert(Float64::from_bits(a), &mut fe);
    commit_flags(cpu, &fe);
    nanbox_h(res.to_bits())
}
