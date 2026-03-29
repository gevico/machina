// TCG IR optimizer — single-pass constant folding, copy propagation,
// algebraic simplification. Runs before liveness analysis.
//
// Reference: ~/qemu/tcg/optimize.c

use machina_core::op::OpIdx;
use machina_core::opcode::{OpFlags, Opcode};
use machina_core::temp::TempIdx;
use machina_core::types::{Cond, Type};
use machina_core::Context;

/// Per-temp optimization info tracked during the pass.
#[derive(Clone, Copy, Default)]
struct TempInfo {
    is_const: bool,
    val: u64,
    /// Canonical copy source (None = no known copy).
    copy_of: Option<TempIdx>,
}

/// Truncation mask for a given IR type.
fn type_mask(ty: Type) -> u64 {
    match ty {
        Type::I32 => 0xFFFF_FFFF,
        _ => u64::MAX,
    }
}

/// Evaluate a comparison condition on two constant operands.
fn eval_cond(a: u64, b: u64, cond: Cond, ty: Type) -> bool {
    let mask = type_mask(ty);
    let a = a & mask;
    let b = b & mask;
    match cond {
        Cond::Always => true,
        Cond::Never => false,
        Cond::Eq => a == b,
        Cond::Ne => a != b,
        Cond::Lt => (a as i64) < (b as i64),
        Cond::Ge => (a as i64) >= (b as i64),
        Cond::Le => (a as i64) <= (b as i64),
        Cond::Gt => (a as i64) > (b as i64),
        Cond::Ltu => a < b,
        Cond::Geu => a >= b,
        Cond::Leu => a <= b,
        Cond::Gtu => a > b,
        Cond::TstEq => (a & b) == 0,
        Cond::TstNe => (a & b) != 0,
    }
}

/// Decode a carg-encoded Cond value.
fn cond_from_carg(t: TempIdx) -> Cond {
    match t.0 {
        0 => Cond::Never,
        1 => Cond::Always,
        8 => Cond::Eq,
        9 => Cond::Ne,
        10 => Cond::Lt,
        11 => Cond::Ge,
        12 => Cond::Le,
        13 => Cond::Gt,
        14 => Cond::Ltu,
        15 => Cond::Geu,
        16 => Cond::Leu,
        17 => Cond::Gtu,
        18 => Cond::TstEq,
        19 => Cond::TstNe,
        _ => Cond::Never,
    }
}

/// Main optimizer entry point.
pub fn optimize(ctx: &mut Context) {
    let n_temps = ctx.nb_temps() as usize;
    let mut info: Vec<TempInfo> = vec![TempInfo::default(); n_temps];

    // Seed const info from existing const temps.
    for (i, ti) in info.iter_mut().enumerate().take(n_temps) {
        let t = ctx.temp(TempIdx(i as u32));
        if t.is_const() {
            ti.is_const = true;
            ti.val = t.val;
        }
    }

    let num_ops = ctx.num_ops();
    for oi in 0..num_ops {
        let op_idx = OpIdx(oi as u32);

        // Read op fields into locals to avoid borrow conflicts.
        let opc = ctx.op(op_idx).opc;
        let op_type = ctx.op(op_idx).op_type;
        let args = ctx.op(op_idx).args;
        let def = opc.def();

        // --- BB boundary: reset all temp info ---
        if matches!(
            opc,
            Opcode::SetLabel
                | Opcode::Br
                | Opcode::ExitTb
                | Opcode::GotoTb
                | Opcode::GotoPtr
                | Opcode::Call
        ) {
            invalidate_outputs(&mut info, def, &args, ctx);
            reset_copies(&mut info);
            continue;
        }

        // Skip ops we don't optimize, but still invalidate
        // their outputs so stale info doesn't leak.
        if def.flags.contains(OpFlags::SIDE_EFFECTS)
            || def.flags.contains(OpFlags::VECTOR)
            || opc == Opcode::Nop
            || opc == Opcode::InsnStart
            || opc == Opcode::Discard
        {
            invalidate_outputs(&mut info, def, &args, ctx);
            continue;
        }

        // --- Copy propagation on inputs ---
        let iarg_start = def.nb_oargs as usize;
        let iarg_end = iarg_start + def.nb_iargs as usize;
        for (slot, &tidx) in args[iarg_start..iarg_end].iter().enumerate() {
            if let Some(src) = resolve_copy(&info, tidx) {
                ctx.op_mut(op_idx).args[iarg_start + slot] = src;
            }
        }

        // Re-read args after copy propagation.
        let args = ctx.op(op_idx).args;

        // --- Per-opcode optimization ---
        match opc {
            Opcode::Mov => {
                fold_mov(ctx, &mut info, op_idx, args, op_type);
            }
            Opcode::Neg | Opcode::Not => {
                fold_unary(ctx, &mut info, op_idx, opc, args, op_type);
            }
            Opcode::ExtI32I64
            | Opcode::ExtUI32I64
            | Opcode::ExtrlI64I32
            | Opcode::ExtrhI64I32 => {
                fold_ext(ctx, &mut info, op_idx, opc, args);
            }
            Opcode::Add
            | Opcode::Sub
            | Opcode::Mul
            | Opcode::And
            | Opcode::Or
            | Opcode::Xor
            | Opcode::AndC
            | Opcode::Shl
            | Opcode::Shr
            | Opcode::Sar
            | Opcode::RotL
            | Opcode::RotR => {
                fold_binary(ctx, &mut info, op_idx, opc, args, op_type);
            }
            Opcode::BrCond => {
                fold_brcond(ctx, &info, op_idx, args, op_type);
            }
            _ => {
                invalidate_outputs(&mut info, def, &args, ctx);
            }
        }
    }
}

// ---- Helper functions ----

/// Follow copy chain to canonical source.
fn resolve_copy(info: &[TempInfo], tidx: TempIdx) -> Option<TempIdx> {
    let i = tidx.0 as usize;
    if i < info.len() {
        info[i].copy_of
    } else {
        None
    }
}

/// Reset all copy relationships (at BB boundaries).
fn reset_copies(info: &mut [TempInfo]) {
    for ti in info.iter_mut() {
        ti.copy_of = None;
    }
}

/// Invalidate output temp info for ops we don't optimize.
fn invalidate_outputs(
    info: &mut [TempInfo],
    def: &machina_core::OpDef,
    args: &[TempIdx; machina_core::MAX_OP_ARGS],
    ctx: &Context,
) {
    for &tidx in args.iter().take(def.nb_oargs as usize) {
        let idx = tidx.0 as usize;
        if idx < info.len() && !ctx.temp(tidx).is_const() {
            info[idx].is_const = false;
            info[idx].copy_of = None;
            // Clear stale copy references to this temp.
            for ti in info.iter_mut() {
                if ti.copy_of == Some(tidx) {
                    ti.copy_of = None;
                }
            }
        }
    }
}

/// Get temp info, returning default for out-of-range indices.
fn ti(info: &[TempInfo], tidx: TempIdx) -> TempInfo {
    let i = tidx.0 as usize;
    if i < info.len() {
        info[i]
    } else {
        TempInfo::default()
    }
}

/// Record that `dst` is now a known constant.
fn set_const(info: &mut Vec<TempInfo>, dst: TempIdx, val: u64) {
    let i = dst.0 as usize;
    ensure_info(info, i);
    info[i].is_const = true;
    info[i].val = val;
    info[i].copy_of = None;
}

/// Record that `dst` is a copy of `src`.
fn set_copy(info: &mut Vec<TempInfo>, dst: TempIdx, src: TempIdx) {
    let i = dst.0 as usize;
    ensure_info(info, i);
    let si = ti(info, src);
    if si.is_const {
        info[i].is_const = true;
        info[i].val = si.val;
        info[i].copy_of = None;
    } else {
        info[i].is_const = false;
        info[i].copy_of = Some(src);
    }
}

fn ensure_info(info: &mut Vec<TempInfo>, idx: usize) {
    if idx >= info.len() {
        info.resize(idx + 1, TempInfo::default());
    }
}

/// Replace op with `mov dst, const_val`.
fn replace_with_const(
    ctx: &mut Context,
    info: &mut Vec<TempInfo>,
    op_idx: OpIdx,
    dst: TempIdx,
    val: u64,
    ty: Type,
) {
    let masked = val & type_mask(ty);
    let c = ctx.new_const(ty, masked);
    // Ensure info covers the new const temp.
    ensure_info(info, c.0 as usize);
    info[c.0 as usize].is_const = true;
    info[c.0 as usize].val = masked;

    let op = ctx.op_mut(op_idx);
    op.opc = Opcode::Mov;
    op.args[0] = dst;
    op.args[1] = c;
    op.nargs = 2;

    set_const(info, dst, masked);
}

/// Replace op with `mov dst, src`.
fn replace_with_mov(
    ctx: &mut Context,
    info: &mut Vec<TempInfo>,
    op_idx: OpIdx,
    dst: TempIdx,
    src: TempIdx,
) {
    let op = ctx.op_mut(op_idx);
    op.opc = Opcode::Mov;
    op.args[0] = dst;
    op.args[1] = src;
    op.nargs = 2;

    // Conservative: just invalidate dst. We don't track
    // the copy relationship here because the source temp
    // may be redefined later in the same EBB, and our
    // invalidation doesn't propagate to derived const info.
    invalidate_one(info, dst);
}

// ---- Per-opcode fold functions ----

/// Mov: record copy/const relationship.
fn fold_mov(
    ctx: &mut Context,
    info: &mut Vec<TempInfo>,
    op_idx: OpIdx,
    args: [TempIdx; machina_core::MAX_OP_ARGS],
    ty: Type,
) {
    let dst = args[0];
    let src = args[1];
    let si = ti(info, src);
    if si.is_const {
        set_const(info, dst, si.val & type_mask(ty));
    } else {
        set_copy(info, dst, src);
    }
    // Keep the mov as-is; liveness/DCE will clean up.
    let _ = (ctx, op_idx);
}

/// Unary ops: Neg, Not.
fn fold_unary(
    ctx: &mut Context,
    info: &mut Vec<TempInfo>,
    op_idx: OpIdx,
    opc: Opcode,
    args: [TempIdx; machina_core::MAX_OP_ARGS],
    ty: Type,
) {
    let dst = args[0];
    let src = args[1];
    let si = ti(info, src);
    if !si.is_const {
        invalidate_one(info, dst);
        return;
    }
    let mask = type_mask(ty);
    let val = match opc {
        Opcode::Neg => (0u64.wrapping_sub(si.val)) & mask,
        Opcode::Not => (!si.val) & mask,
        _ => unreachable!(),
    };
    replace_with_const(ctx, info, op_idx, dst, val, ty);
}

/// Type conversion ops.
fn fold_ext(
    ctx: &mut Context,
    info: &mut Vec<TempInfo>,
    op_idx: OpIdx,
    opc: Opcode,
    args: [TempIdx; machina_core::MAX_OP_ARGS],
) {
    let dst = args[0];
    let src = args[1];
    let si = ti(info, src);
    if !si.is_const {
        invalidate_one(info, dst);
        return;
    }
    let val = match opc {
        Opcode::ExtI32I64 => {
            // sign-extend i32 -> i64
            (si.val as u32 as i32 as i64) as u64
        }
        Opcode::ExtUI32I64 => si.val & 0xFFFF_FFFF,
        Opcode::ExtrlI64I32 => si.val & 0xFFFF_FFFF,
        Opcode::ExtrhI64I32 => (si.val >> 32) & 0xFFFF_FFFF,
        _ => unreachable!(),
    };
    let out_ty = match opc {
        Opcode::ExtI32I64 | Opcode::ExtUI32I64 => Type::I64,
        _ => Type::I32,
    };
    replace_with_const(ctx, info, op_idx, dst, val, out_ty);
}

/// Binary arithmetic/logic ops.
fn fold_binary(
    ctx: &mut Context,
    info: &mut Vec<TempInfo>,
    op_idx: OpIdx,
    opc: Opcode,
    args: [TempIdx; machina_core::MAX_OP_ARGS],
    ty: Type,
) {
    let dst = args[0];
    let a_idx = args[1];
    let b_idx = args[2];
    let ai = ti(info, a_idx);
    let bi = ti(info, b_idx);

    // Both constant → fold.
    if ai.is_const && bi.is_const {
        let mask = type_mask(ty);
        let a = ai.val & mask;
        let b = bi.val & mask;
        if let Some(val) = eval_binary(opc, a, b, ty) {
            replace_with_const(ctx, info, op_idx, dst, val, ty);
            return;
        }
    }

    // Algebraic simplification with one constant.
    if try_simplify(ctx, info, op_idx, opc, dst, a_idx, b_idx, &ai, &bi, ty) {
        return;
    }

    // Same-operand identities: x & x → x, x | x → x,
    // x ^ x → 0, x - x → 0.
    if a_idx == b_idx {
        match opc {
            Opcode::And | Opcode::Or => {
                replace_with_mov(ctx, info, op_idx, dst, a_idx);
                return;
            }
            Opcode::Xor | Opcode::Sub => {
                replace_with_const(ctx, info, op_idx, dst, 0, ty);
                return;
            }
            _ => {}
        }
    }

    invalidate_one(info, dst);
}

/// Evaluate a binary op on two constants.
fn eval_binary(opc: Opcode, a: u64, b: u64, ty: Type) -> Option<u64> {
    let mask = type_mask(ty);
    let bits = ty.size_bits();
    let r = match opc {
        Opcode::Add => a.wrapping_add(b),
        Opcode::Sub => a.wrapping_sub(b),
        Opcode::Mul => a.wrapping_mul(b),
        Opcode::And => a & b,
        Opcode::Or => a | b,
        Opcode::Xor => a ^ b,
        Opcode::AndC => a & !b,
        Opcode::Shl => {
            let sh = (b as u32) % bits;
            a.wrapping_shl(sh)
        }
        Opcode::Shr => {
            let sh = (b as u32) % bits;
            (a & mask).wrapping_shr(sh)
        }
        Opcode::Sar => {
            let sh = (b as u32) % bits;
            if ty == Type::I32 {
                ((a as u32 as i32) >> sh) as u64
            } else {
                ((a as i64) >> sh) as u64
            }
        }
        Opcode::RotL => {
            let sh = (b as u32) % bits;
            if ty == Type::I32 {
                let v = a as u32;
                (v.rotate_left(sh)) as u64
            } else {
                a.rotate_left(sh)
            }
        }
        Opcode::RotR => {
            let sh = (b as u32) % bits;
            if ty == Type::I32 {
                let v = a as u32;
                (v.rotate_right(sh)) as u64
            } else {
                a.rotate_right(sh)
            }
        }
        _ => return None,
    };
    Some(r & mask)
}

/// Algebraic simplification when one operand is constant.
/// Returns true if the op was simplified.
#[allow(clippy::too_many_arguments)]
fn try_simplify(
    ctx: &mut Context,
    info: &mut Vec<TempInfo>,
    op_idx: OpIdx,
    opc: Opcode,
    dst: TempIdx,
    a_idx: TempIdx,
    b_idx: TempIdx,
    ai: &TempInfo,
    bi: &TempInfo,
    ty: Type,
) -> bool {
    let mask = type_mask(ty);
    let all_ones = mask;

    // b is constant
    if bi.is_const {
        let b = bi.val & mask;
        match opc {
            // x + 0, x - 0, x | 0, x ^ 0, x << 0,
            // x >> 0, x >>> 0 → mov x
            Opcode::Add
            | Opcode::Sub
            | Opcode::Or
            | Opcode::Xor
            | Opcode::Shl
            | Opcode::Shr
            | Opcode::Sar
            | Opcode::RotL
            | Opcode::RotR
                if b == 0 =>
            {
                replace_with_mov(ctx, info, op_idx, dst, a_idx);
                return true;
            }
            // x * 0, x & 0 → mov 0
            Opcode::Mul | Opcode::And if b == 0 => {
                replace_with_const(ctx, info, op_idx, dst, 0, ty);
                return true;
            }
            // x * 1 → mov x
            Opcode::Mul if b == 1 => {
                replace_with_mov(ctx, info, op_idx, dst, a_idx);
                return true;
            }
            // x & -1 → mov x
            Opcode::And if b == all_ones => {
                replace_with_mov(ctx, info, op_idx, dst, a_idx);
                return true;
            }
            // x | -1 → mov -1
            Opcode::Or if b == all_ones => {
                replace_with_const(ctx, info, op_idx, dst, all_ones, ty);
                return true;
            }
            // x & ~0 with AndC → mov 0 (andc x, -1)
            Opcode::AndC if b == all_ones => {
                replace_with_const(ctx, info, op_idx, dst, 0, ty);
                return true;
            }
            _ => {}
        }
    }

    // a is constant
    if ai.is_const {
        let a = ai.val & mask;
        match opc {
            // 0 + x → mov x
            Opcode::Add if a == 0 => {
                replace_with_mov(ctx, info, op_idx, dst, b_idx);
                return true;
            }
            // 0 - x → neg x (strength reduction)
            Opcode::Sub if a == 0 => {
                let op = ctx.op_mut(op_idx);
                op.opc = Opcode::Neg;
                op.args[0] = dst;
                op.args[1] = b_idx;
                op.nargs = 2;
                invalidate_one(info, dst);
                return true;
            }
            // 0 * x → mov 0
            Opcode::Mul if a == 0 => {
                replace_with_const(ctx, info, op_idx, dst, 0, ty);
                return true;
            }
            // 0 & x → mov 0
            Opcode::And if a == 0 => {
                replace_with_const(ctx, info, op_idx, dst, 0, ty);
                return true;
            }
            // -1 | x → mov -1
            Opcode::Or if a == all_ones => {
                replace_with_const(ctx, info, op_idx, dst, all_ones, ty);
                return true;
            }
            _ => {}
        }
    }

    false
}

/// Fold BrCond when both inputs are constant.
fn fold_brcond(
    ctx: &mut Context,
    info: &[TempInfo],
    op_idx: OpIdx,
    args: [TempIdx; machina_core::MAX_OP_ARGS],
    ty: Type,
) {
    let a_idx = args[0]; // iarg 0
    let b_idx = args[1]; // iarg 1
    let cond_carg = args[2]; // carg 0: condition
    let label_carg = args[3]; // carg 1: label id

    let ai = ti(info, a_idx);
    let bi = ti(info, b_idx);
    if !ai.is_const || !bi.is_const {
        return;
    }

    let cond = cond_from_carg(cond_carg);
    if eval_cond(ai.val, bi.val, cond, ty) {
        // Always taken → unconditional branch.
        let op = ctx.op_mut(op_idx);
        op.opc = Opcode::Br;
        op.args[0] = label_carg;
        op.nargs = 1;
    } else {
        // Never taken → nop.
        let op = ctx.op_mut(op_idx);
        op.opc = Opcode::Nop;
        op.nargs = 0;
    }
}

fn invalidate_one(info: &mut Vec<TempInfo>, dst: TempIdx) {
    let i = dst.0 as usize;
    ensure_info(info, i);
    info[i].is_const = false;
    info[i].copy_of = None;
    // Clear any temp that was a copy of dst, since dst
    // is being redefined.
    for ti in info.iter_mut() {
        if ti.copy_of == Some(dst) {
            ti.copy_of = None;
        }
    }
}
