//! Helper methods for RISC-V instruction translation.

use super::super::cpu::{
    fpr_offset, FFLAGS_OFFSET, FRM_OFFSET, UCAUSE_OFFSET, UEPC_OFFSET,
    UIE_OFFSET, UIP_OFFSET, USCRATCH_OFFSET, USTATUS_FS_DIRTY, USTATUS_FS_MASK,
    USTATUS_OFFSET, UTVAL_OFFSET, UTVEC_OFFSET,
};
use super::super::fpu;
use super::super::insn_decode::*;
use super::super::RiscvDisasContext;
use crate::DisasJumpType;
use machina_core::context::Context;
use machina_core::tb::{TB_EXIT_IDX0, TB_EXIT_IDX1};
use machina_core::types::{Cond, MemOp, Type};
use machina_core::TempIdx;

/// Binary IR operation: `fn(ir, ty, dst, lhs, rhs) -> dst`.
pub(super) type BinOp =
    fn(&mut Context, Type, TempIdx, TempIdx, TempIdx) -> TempIdx;

// Memory barrier constants (QEMU TCG_MO_* / TCG_BAR_*).
const TCG_MO_ALL: u32 = 0x0F;
const TCG_BAR_LDAQ: u32 = 0x10;
const TCG_BAR_STRL: u32 = 0x20;

// CSR numbers (user-level).
const CSR_USTATUS: i64 = 0x000;
const CSR_FFLAGS: i64 = 0x001;
const CSR_FRM: i64 = 0x002;
const CSR_FCSR: i64 = 0x003;
const CSR_UIE: i64 = 0x004;
const CSR_UTVEC: i64 = 0x005;
const CSR_USCRATCH: i64 = 0x040;
const CSR_UEPC: i64 = 0x041;
const CSR_UCAUSE: i64 = 0x042;
const CSR_UTVAL: i64 = 0x043;
const CSR_UIP: i64 = 0x044;
const CSR_CYCLE: i64 = 0xC00;
const CSR_TIME: i64 = 0xC01;
const CSR_INSTRET: i64 = 0xC02;

// ── Helpers ────────────────────────────────────────────────────

impl RiscvDisasContext {
    // -- GPR access ----------------------------------------

    /// Read GPR `idx`; x0 yields a constant zero.
    pub(super) fn gpr_or_zero(&self, ir: &mut Context, idx: i64) -> TempIdx {
        if idx == 0 {
            ir.new_const(Type::I64, 0)
        } else {
            self.gpr[idx as usize]
        }
    }

    /// Write `val` into GPR `rd`; writes to x0 discarded.
    pub(super) fn gen_set_gpr(&self, ir: &mut Context, rd: i64, val: TempIdx) {
        if rd != 0 {
            ir.gen_mov(Type::I64, self.gpr[rd as usize], val);
        }
    }

    /// Sign-extend low 32 bits into a 64-bit GPR.
    pub(super) fn gen_set_gpr_sx32(
        &self,
        ir: &mut Context,
        rd: i64,
        val: TempIdx,
    ) {
        if rd != 0 {
            ir.gen_ext_i32_i64(self.gpr[rd as usize], val);
        }
    }

    // -- FPR access ----------------------------------------

    pub(super) fn fpr_load(&self, ir: &mut Context, idx: i64) -> TempIdx {
        let t = ir.new_temp(Type::I64);
        ir.gen_ld(Type::I64, t, self.env, fpr_offset(idx as usize));
        t
    }

    pub(super) fn fpr_store(&self, ir: &mut Context, idx: i64, val: TempIdx) {
        ir.gen_st(Type::I64, val, self.env, fpr_offset(idx as usize));
    }

    // -- FP state helpers -----------------------------------

    pub(super) fn gen_fp_check(&self, ir: &mut Context) {
        let status = ir.new_temp(Type::I64);
        ir.gen_ld(Type::I64, status, self.env, USTATUS_OFFSET);
        let mask = ir.new_const(Type::I64, USTATUS_FS_MASK);
        let fs = ir.new_temp(Type::I64);
        ir.gen_and(Type::I64, fs, status, mask);
        let zero = ir.new_const(Type::I64, 0);
        let ok = ir.new_label();
        ir.gen_brcond(Type::I64, fs, zero, Cond::Ne, ok);
        let pc = ir.new_const(Type::I64, self.base.pc_next);
        ir.gen_mov(Type::I64, self.pc, pc);
        ir.gen_exit_tb(machina_core::tb::EXCP_UNDEF);
        ir.gen_set_label(ok);
    }

    pub(super) fn gen_set_fs_dirty(&self, ir: &mut Context) {
        let status = ir.new_temp(Type::I64);
        ir.gen_ld(Type::I64, status, self.env, USTATUS_OFFSET);
        let clear = ir.new_const(Type::I64, !USTATUS_FS_MASK);
        let cleared = ir.new_temp(Type::I64);
        ir.gen_and(Type::I64, cleared, status, clear);
        let dirty = ir.new_const(Type::I64, USTATUS_FS_DIRTY);
        let new_status = ir.new_temp(Type::I64);
        ir.gen_or(Type::I64, new_status, cleared, dirty);
        ir.gen_st(Type::I64, new_status, self.env, USTATUS_OFFSET);
    }

    pub(super) fn gen_helper_call(
        &self,
        ir: &mut Context,
        helper: usize,
        args: &[TempIdx],
    ) -> TempIdx {
        let dst = ir.new_temp(Type::I64);
        ir.gen_call(dst, helper as u64, args);
        dst
    }

    pub(super) fn gen_fp_load(
        &self,
        ir: &mut Context,
        a: &ArgsI,
        memop: MemOp,
        is_single: bool,
    ) -> bool {
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let base = self.gpr_or_zero(ir, a.rs1);
        let addr = if a.imm != 0 {
            let imm = ir.new_const(Type::I64, a.imm as u64);
            let t = ir.new_temp(Type::I64);
            ir.gen_add(Type::I64, t, base, imm)
        } else {
            base
        };
        let val = ir.new_temp(Type::I64);
        ir.gen_qemu_ld(Type::I64, val, addr, memop.bits() as u32);
        if is_single {
            let mask = ir.new_const(Type::I64, 0xffff_ffff_0000_0000u64);
            let boxed = ir.new_temp(Type::I64);
            ir.gen_or(Type::I64, boxed, val, mask);
            self.fpr_store(ir, a.rd, boxed);
        } else {
            self.fpr_store(ir, a.rd, val);
        }
        true
    }

    pub(super) fn gen_fp_store(
        &self,
        ir: &mut Context,
        a: &ArgsS,
        memop: MemOp,
        is_single: bool,
    ) -> bool {
        self.gen_fp_check(ir);
        let base = self.gpr_or_zero(ir, a.rs1);
        let addr = if a.imm != 0 {
            let imm = ir.new_const(Type::I64, a.imm as u64);
            let t = ir.new_temp(Type::I64);
            ir.gen_add(Type::I64, t, base, imm)
        } else {
            base
        };
        let val = self.fpr_load(ir, a.rs2);
        let store_val = if is_single {
            let lo32 = ir.new_temp(Type::I32);
            ir.gen_extrl_i64_i32(lo32, val);
            let lo64 = ir.new_temp(Type::I64);
            ir.gen_ext_i32_i64(lo64, lo32);
            lo64
        } else {
            val
        };
        ir.gen_qemu_st(Type::I64, store_val, addr, memop.bits() as u32);
        true
    }

    // -- CSR helpers ----------------------------------------

    pub(super) fn gen_csr_read(
        &self,
        ir: &mut Context,
        csr: i64,
    ) -> Option<TempIdx> {
        match csr {
            CSR_FFLAGS => {
                let v = ir.new_temp(Type::I64);
                ir.gen_ld(Type::I64, v, self.env, FFLAGS_OFFSET);
                let mask = ir.new_const(Type::I64, fpu::FFLAGS_MASK);
                let out = ir.new_temp(Type::I64);
                ir.gen_and(Type::I64, out, v, mask);
                Some(out)
            }
            CSR_FRM => {
                let v = ir.new_temp(Type::I64);
                ir.gen_ld(Type::I64, v, self.env, FRM_OFFSET);
                let mask = ir.new_const(Type::I64, fpu::FRM_MASK);
                let out = ir.new_temp(Type::I64);
                ir.gen_and(Type::I64, out, v, mask);
                Some(out)
            }
            CSR_FCSR => {
                let fflags = ir.new_temp(Type::I64);
                ir.gen_ld(Type::I64, fflags, self.env, FFLAGS_OFFSET);
                let fmask = ir.new_const(Type::I64, fpu::FFLAGS_MASK);
                ir.gen_and(Type::I64, fflags, fflags, fmask);
                let frm = ir.new_temp(Type::I64);
                ir.gen_ld(Type::I64, frm, self.env, FRM_OFFSET);
                let rmask = ir.new_const(Type::I64, fpu::FRM_MASK);
                ir.gen_and(Type::I64, frm, frm, rmask);
                let shift = ir.new_const(Type::I64, 5);
                let frm_shift = ir.new_temp(Type::I64);
                ir.gen_shl(Type::I64, frm_shift, frm, shift);
                let out = ir.new_temp(Type::I64);
                ir.gen_or(Type::I64, out, fflags, frm_shift);
                Some(out)
            }
            CSR_USTATUS => {
                let v = ir.new_temp(Type::I64);
                ir.gen_ld(Type::I64, v, self.env, USTATUS_OFFSET);
                Some(v)
            }
            CSR_UIE => {
                let v = ir.new_temp(Type::I64);
                ir.gen_ld(Type::I64, v, self.env, UIE_OFFSET);
                Some(v)
            }
            CSR_UTVEC => {
                let v = ir.new_temp(Type::I64);
                ir.gen_ld(Type::I64, v, self.env, UTVEC_OFFSET);
                Some(v)
            }
            CSR_USCRATCH => {
                let v = ir.new_temp(Type::I64);
                ir.gen_ld(Type::I64, v, self.env, USCRATCH_OFFSET);
                Some(v)
            }
            CSR_UEPC => {
                let v = ir.new_temp(Type::I64);
                ir.gen_ld(Type::I64, v, self.env, UEPC_OFFSET);
                Some(v)
            }
            CSR_UCAUSE => {
                let v = ir.new_temp(Type::I64);
                ir.gen_ld(Type::I64, v, self.env, UCAUSE_OFFSET);
                Some(v)
            }
            CSR_UTVAL => {
                let v = ir.new_temp(Type::I64);
                ir.gen_ld(Type::I64, v, self.env, UTVAL_OFFSET);
                Some(v)
            }
            CSR_UIP => {
                let v = ir.new_temp(Type::I64);
                ir.gen_ld(Type::I64, v, self.env, UIP_OFFSET);
                Some(v)
            }
            CSR_CYCLE | CSR_TIME | CSR_INSTRET => {
                let v = ir.new_const(Type::I64, 0);
                Some(v)
            }
            _ => None,
        }
    }

    pub(super) fn gen_csr_write(
        &self,
        ir: &mut Context,
        csr: i64,
        val: TempIdx,
    ) -> bool {
        match csr {
            CSR_FFLAGS => {
                let mask = ir.new_const(Type::I64, fpu::FFLAGS_MASK);
                let v = ir.new_temp(Type::I64);
                ir.gen_and(Type::I64, v, val, mask);
                ir.gen_st(Type::I64, v, self.env, FFLAGS_OFFSET);
                self.gen_set_fs_dirty(ir);
                true
            }
            CSR_FRM => {
                let mask = ir.new_const(Type::I64, fpu::FRM_MASK);
                let v = ir.new_temp(Type::I64);
                ir.gen_and(Type::I64, v, val, mask);
                ir.gen_st(Type::I64, v, self.env, FRM_OFFSET);
                self.gen_set_fs_dirty(ir);
                true
            }
            CSR_FCSR => {
                let fmask = ir.new_const(Type::I64, fpu::FFLAGS_MASK);
                let fflags = ir.new_temp(Type::I64);
                ir.gen_and(Type::I64, fflags, val, fmask);
                ir.gen_st(Type::I64, fflags, self.env, FFLAGS_OFFSET);
                let shift = ir.new_const(Type::I64, 5);
                let frm = ir.new_temp(Type::I64);
                ir.gen_shr(Type::I64, frm, val, shift);
                let rmask = ir.new_const(Type::I64, fpu::FRM_MASK);
                ir.gen_and(Type::I64, frm, frm, rmask);
                ir.gen_st(Type::I64, frm, self.env, FRM_OFFSET);
                self.gen_set_fs_dirty(ir);
                true
            }
            CSR_USTATUS => {
                ir.gen_st(Type::I64, val, self.env, USTATUS_OFFSET);
                true
            }
            CSR_UIE => {
                ir.gen_st(Type::I64, val, self.env, UIE_OFFSET);
                true
            }
            CSR_UTVEC => {
                ir.gen_st(Type::I64, val, self.env, UTVEC_OFFSET);
                true
            }
            CSR_USCRATCH => {
                ir.gen_st(Type::I64, val, self.env, USCRATCH_OFFSET);
                true
            }
            CSR_UEPC => {
                ir.gen_st(Type::I64, val, self.env, UEPC_OFFSET);
                true
            }
            CSR_UCAUSE => {
                ir.gen_st(Type::I64, val, self.env, UCAUSE_OFFSET);
                true
            }
            CSR_UTVAL => {
                ir.gen_st(Type::I64, val, self.env, UTVAL_OFFSET);
                true
            }
            CSR_UIP => {
                ir.gen_st(Type::I64, val, self.env, UIP_OFFSET);
                true
            }
            CSR_CYCLE | CSR_TIME | CSR_INSTRET => false,
            _ => false,
        }
    }

    // -- R-type helpers ------------------------------------

    // -- Guest memory helpers --------------------------------

    /// Guest load: rd = *(addr), addr = rs1 + imm.
    pub(super) fn gen_load(
        &self,
        ir: &mut Context,
        a: &ArgsI,
        memop: MemOp,
    ) -> bool {
        let base = self.gpr_or_zero(ir, a.rs1);
        let addr = if a.imm != 0 {
            let imm = ir.new_const(Type::I64, a.imm as u64);
            let t = ir.new_temp(Type::I64);
            ir.gen_add(Type::I64, t, base, imm)
        } else {
            base
        };
        let dst = ir.new_temp(Type::I64);
        ir.gen_qemu_ld(Type::I64, dst, addr, memop.bits() as u32);
        self.gen_set_gpr(ir, a.rd, dst);
        true
    }

    /// Guest store: *(addr) = rs2, addr = rs1 + imm.
    pub(super) fn gen_store(
        &self,
        ir: &mut Context,
        a: &ArgsS,
        memop: MemOp,
    ) -> bool {
        let base = self.gpr_or_zero(ir, a.rs1);
        let addr = if a.imm != 0 {
            let imm = ir.new_const(Type::I64, a.imm as u64);
            let t = ir.new_temp(Type::I64);
            ir.gen_add(Type::I64, t, base, imm)
        } else {
            base
        };
        let val = self.gpr_or_zero(ir, a.rs2);
        ir.gen_qemu_st(Type::I64, val, addr, memop.bits() as u32);
        true
    }

    // -- R-type ALU helpers ----------------------------------

    /// R-type ALU: `rd = op(rs1, rs2)`.
    pub(super) fn gen_arith(
        &self,
        ir: &mut Context,
        a: &ArgsR,
        op: BinOp,
    ) -> bool {
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let s2 = self.gpr_or_zero(ir, a.rs2);
        let d = ir.new_temp(Type::I64);
        op(ir, Type::I64, d, s1, s2);
        self.gen_set_gpr(ir, a.rd, d);
        true
    }

    /// R-type setcond: `rd = (rs1 cond rs2) ? 1 : 0`.
    pub(super) fn gen_setcond_rr(
        &self,
        ir: &mut Context,
        a: &ArgsR,
        cond: Cond,
    ) -> bool {
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let s2 = self.gpr_or_zero(ir, a.rs2);
        let d = ir.new_temp(Type::I64);
        ir.gen_setcond(Type::I64, d, s1, s2, cond);
        self.gen_set_gpr(ir, a.rd, d);
        true
    }

    // -- I-type helpers ------------------------------------

    /// I-type ALU: `rd = op(rs1, sext(imm))`.
    pub(super) fn gen_arith_imm(
        &self,
        ir: &mut Context,
        a: &ArgsI,
        op: BinOp,
    ) -> bool {
        let src = self.gpr_or_zero(ir, a.rs1);
        let imm = ir.new_const(Type::I64, a.imm as u64);
        let d = ir.new_temp(Type::I64);
        op(ir, Type::I64, d, src, imm);
        self.gen_set_gpr(ir, a.rd, d);
        true
    }

    /// I-type setcond: `rd = (rs1 cond imm) ? 1 : 0`.
    pub(super) fn gen_setcond_imm(
        &self,
        ir: &mut Context,
        a: &ArgsI,
        cond: Cond,
    ) -> bool {
        let src = self.gpr_or_zero(ir, a.rs1);
        let imm = ir.new_const(Type::I64, a.imm as u64);
        let d = ir.new_temp(Type::I64);
        ir.gen_setcond(Type::I64, d, src, imm, cond);
        self.gen_set_gpr(ir, a.rd, d);
        true
    }

    // -- Shift helpers -------------------------------------

    /// Shift immediate: `rd = op(rs1, shamt)`.
    pub(super) fn gen_shift_imm(
        &self,
        ir: &mut Context,
        a: &ArgsShift,
        op: BinOp,
    ) -> bool {
        let src = self.gpr_or_zero(ir, a.rs1);
        let sh = ir.new_const(Type::I64, a.shamt as u64);
        let d = ir.new_temp(Type::I64);
        op(ir, Type::I64, d, src, sh);
        self.gen_set_gpr(ir, a.rd, d);
        true
    }

    // -- W-suffix helpers (RV64) ---------------------------

    /// R-type W: `rd = sext32(op(rs1, rs2))`.
    pub(super) fn gen_arith_w(
        &self,
        ir: &mut Context,
        a: &ArgsR,
        op: BinOp,
    ) -> bool {
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let s2 = self.gpr_or_zero(ir, a.rs2);
        let d = ir.new_temp(Type::I64);
        op(ir, Type::I64, d, s1, s2);
        self.gen_set_gpr_sx32(ir, a.rd, d);
        true
    }

    /// I-type W: `rd = sext32(op(rs1, imm))`.
    pub(super) fn gen_arith_imm_w(
        &self,
        ir: &mut Context,
        a: &ArgsI,
        op: BinOp,
    ) -> bool {
        let src = self.gpr_or_zero(ir, a.rs1);
        let imm = ir.new_const(Type::I64, a.imm as u64);
        let d = ir.new_temp(Type::I64);
        op(ir, Type::I64, d, src, imm);
        self.gen_set_gpr_sx32(ir, a.rd, d);
        true
    }

    /// R-type shift W: truncate to I32, shift, sext.
    pub(super) fn gen_shiftw(
        &self,
        ir: &mut Context,
        a: &ArgsR,
        op: BinOp,
    ) -> bool {
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let s2 = self.gpr_or_zero(ir, a.rs2);
        let a32 = ir.new_temp(Type::I32);
        ir.gen_extrl_i64_i32(a32, s1);
        let b32 = ir.new_temp(Type::I32);
        ir.gen_extrl_i64_i32(b32, s2);
        let d32 = ir.new_temp(Type::I32);
        op(ir, Type::I32, d32, a32, b32);
        self.gen_set_gpr_sx32(ir, a.rd, d32);
        true
    }

    /// Shift immediate W: truncate to I32, shift, sext.
    pub(super) fn gen_shift_imm_w(
        &self,
        ir: &mut Context,
        a: &ArgsShift,
        op: BinOp,
    ) -> bool {
        let src = self.gpr_or_zero(ir, a.rs1);
        let s32 = ir.new_temp(Type::I32);
        ir.gen_extrl_i64_i32(s32, src);
        let sh = ir.new_const(Type::I32, a.shamt as u64);
        let d32 = ir.new_temp(Type::I32);
        op(ir, Type::I32, d32, s32, sh);
        self.gen_set_gpr_sx32(ir, a.rd, d32);
        true
    }

    // -- M-extension helpers (mul/div/rem) -----------------

    /// Signed division with RISC-V special-case handling.
    /// div-by-zero -> -1 (quot) / dividend (rem).
    /// MIN / -1 -> MIN (quot) / 0 (rem).
    pub(super) fn gen_div_rem(
        &self,
        ir: &mut Context,
        a: &ArgsR,
        want_rem: bool,
    ) -> bool {
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let s2 = self.gpr_or_zero(ir, a.rs2);
        let zero = ir.new_const(Type::I64, 0);
        let one = ir.new_const(Type::I64, 1);
        let neg1 = ir.new_const(Type::I64, u64::MAX);

        // Replace divisor=0 with 1 to avoid trap
        let safe = ir.new_temp(Type::I64);
        ir.gen_movcond(Type::I64, safe, s2, zero, one, s2, Cond::Eq);
        // Replace divisor=-1 with 1 to avoid overflow
        ir.gen_movcond(Type::I64, safe, safe, neg1, one, safe, Cond::Eq);

        let ah = ir.new_temp(Type::I64);
        let c63 = ir.new_const(Type::I64, 63);
        ir.gen_sar(Type::I64, ah, s1, c63);

        let quot = ir.new_temp(Type::I64);
        let rem = ir.new_temp(Type::I64);
        ir.gen_divs2(Type::I64, quot, rem, s1, ah, safe);

        if want_rem {
            // 0 -> s1, -1 -> 0, else -> rem
            let r = ir.new_temp(Type::I64);
            ir.gen_movcond(Type::I64, r, s2, zero, s1, rem, Cond::Eq);
            ir.gen_movcond(Type::I64, r, s2, neg1, zero, r, Cond::Eq);
            self.gen_set_gpr(ir, a.rd, r);
        } else {
            // 0 -> -1, -1 -> neg(s1), else -> quot
            let neg_s1 = ir.new_temp(Type::I64);
            ir.gen_neg(Type::I64, neg_s1, s1);
            let r = ir.new_temp(Type::I64);
            ir.gen_movcond(Type::I64, r, s2, zero, neg1, quot, Cond::Eq);
            ir.gen_movcond(Type::I64, r, s2, neg1, neg_s1, r, Cond::Eq);
            self.gen_set_gpr(ir, a.rd, r);
        }
        true
    }

    /// Unsigned division with RISC-V special-case handling.
    /// div-by-zero -> MAX (quot) / dividend (rem).
    pub(super) fn gen_divu_remu(
        &self,
        ir: &mut Context,
        a: &ArgsR,
        want_rem: bool,
    ) -> bool {
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let s2 = self.gpr_or_zero(ir, a.rs2);
        let zero = ir.new_const(Type::I64, 0);
        let one = ir.new_const(Type::I64, 1);

        let safe = ir.new_temp(Type::I64);
        ir.gen_movcond(Type::I64, safe, s2, zero, one, s2, Cond::Eq);

        let quot = ir.new_temp(Type::I64);
        let rem = ir.new_temp(Type::I64);
        ir.gen_divu2(Type::I64, quot, rem, s1, zero, safe);

        if want_rem {
            let r = ir.new_temp(Type::I64);
            ir.gen_movcond(Type::I64, r, s2, zero, s1, rem, Cond::Eq);
            self.gen_set_gpr(ir, a.rd, r);
        } else {
            let neg1 = ir.new_const(Type::I64, u64::MAX);
            let r = ir.new_temp(Type::I64);
            ir.gen_movcond(Type::I64, r, s2, zero, neg1, quot, Cond::Eq);
            self.gen_set_gpr(ir, a.rd, r);
        }
        true
    }

    /// 32-bit signed division (W-suffix).
    pub(super) fn gen_div_rem_w(
        &self,
        ir: &mut Context,
        a: &ArgsR,
        want_rem: bool,
    ) -> bool {
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let s2 = self.gpr_or_zero(ir, a.rs2);
        let a32 = ir.new_temp(Type::I32);
        ir.gen_extrl_i64_i32(a32, s1);
        let b32 = ir.new_temp(Type::I32);
        ir.gen_extrl_i64_i32(b32, s2);

        let zero = ir.new_const(Type::I32, 0);
        let one = ir.new_const(Type::I32, 1);
        let neg1 = ir.new_const(Type::I32, u32::MAX as u64);

        let safe = ir.new_temp(Type::I32);
        ir.gen_movcond(Type::I32, safe, b32, zero, one, b32, Cond::Eq);
        ir.gen_movcond(Type::I32, safe, safe, neg1, one, safe, Cond::Eq);

        let ah = ir.new_temp(Type::I32);
        let c31 = ir.new_const(Type::I32, 31);
        ir.gen_sar(Type::I32, ah, a32, c31);

        let quot = ir.new_temp(Type::I32);
        let rem = ir.new_temp(Type::I32);
        ir.gen_divs2(Type::I32, quot, rem, a32, ah, safe);

        if want_rem {
            let r = ir.new_temp(Type::I32);
            ir.gen_movcond(Type::I32, r, b32, zero, a32, rem, Cond::Eq);
            ir.gen_movcond(Type::I32, r, b32, neg1, zero, r, Cond::Eq);
            self.gen_set_gpr_sx32(ir, a.rd, r);
        } else {
            let neg_a = ir.new_temp(Type::I32);
            ir.gen_neg(Type::I32, neg_a, a32);
            let r = ir.new_temp(Type::I32);
            ir.gen_movcond(Type::I32, r, b32, zero, neg1, quot, Cond::Eq);
            ir.gen_movcond(Type::I32, r, b32, neg1, neg_a, r, Cond::Eq);
            self.gen_set_gpr_sx32(ir, a.rd, r);
        }
        true
    }

    /// 32-bit unsigned division (W-suffix).
    pub(super) fn gen_divu_remu_w(
        &self,
        ir: &mut Context,
        a: &ArgsR,
        want_rem: bool,
    ) -> bool {
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let s2 = self.gpr_or_zero(ir, a.rs2);
        let a32 = ir.new_temp(Type::I32);
        ir.gen_extrl_i64_i32(a32, s1);
        let b32 = ir.new_temp(Type::I32);
        ir.gen_extrl_i64_i32(b32, s2);

        let zero = ir.new_const(Type::I32, 0);
        let one = ir.new_const(Type::I32, 1);

        let safe = ir.new_temp(Type::I32);
        ir.gen_movcond(Type::I32, safe, b32, zero, one, b32, Cond::Eq);

        let quot = ir.new_temp(Type::I32);
        let rem = ir.new_temp(Type::I32);
        ir.gen_divu2(Type::I32, quot, rem, a32, zero, safe);

        if want_rem {
            let r = ir.new_temp(Type::I32);
            ir.gen_movcond(Type::I32, r, b32, zero, a32, rem, Cond::Eq);
            self.gen_set_gpr_sx32(ir, a.rd, r);
        } else {
            let max = ir.new_const(Type::I32, u32::MAX as u64);
            let r = ir.new_temp(Type::I32);
            ir.gen_movcond(Type::I32, r, b32, zero, max, quot, Cond::Eq);
            self.gen_set_gpr_sx32(ir, a.rd, r);
        }
        true
    }

    // -- Atomic helpers (A extension) ----------------------

    /// LR: load-reserved.
    pub(super) fn gen_lr(
        &self,
        ir: &mut Context,
        a: &ArgsAtomic,
        memop: MemOp,
    ) -> bool {
        let addr = self.gpr_or_zero(ir, a.rs1);
        if a.rl != 0 {
            ir.gen_mb(TCG_MO_ALL | TCG_BAR_STRL);
        }
        let val = ir.new_temp(Type::I64);
        ir.gen_qemu_ld(Type::I64, val, addr, memop.bits() as u32);
        if a.aq != 0 {
            ir.gen_mb(TCG_MO_ALL | TCG_BAR_LDAQ);
        }
        ir.gen_mov(Type::I64, self.load_res, addr);
        ir.gen_mov(Type::I64, self.load_val, val);
        self.gen_set_gpr(ir, a.rd, val);
        true
    }

    /// SC: store-conditional (single-thread simplified).
    ///
    /// In single-threaded mode, SC always succeeds if there
    /// is a valid reservation (set by a preceding LR).
    /// We skip the address comparison since no other thread
    /// can invalidate the reservation.
    pub(super) fn gen_sc(
        &self,
        ir: &mut Context,
        a: &ArgsAtomic,
        memop: MemOp,
    ) -> bool {
        let addr = self.gpr_or_zero(ir, a.rs1);

        // Always succeed: store and set rd = 0.
        let src2 = self.gpr_or_zero(ir, a.rs2);
        ir.gen_qemu_st(Type::I64, src2, addr, memop.bits() as u32);
        let zero = ir.new_const(Type::I64, 0);
        self.gen_set_gpr(ir, a.rd, zero);

        // Clear reservation.
        let neg1 = ir.new_const(Type::I64, u64::MAX);
        ir.gen_mov(Type::I64, self.load_res, neg1);
        true
    }

    /// AMO: atomic read-modify-write
    /// (single-thread: ld+op+st).
    pub(super) fn gen_amo(
        &self,
        ir: &mut Context,
        a: &ArgsAtomic,
        op: BinOp,
        memop: MemOp,
    ) -> bool {
        let addr = self.gpr_or_zero(ir, a.rs1);
        if a.rl != 0 {
            ir.gen_mb(TCG_MO_ALL | TCG_BAR_STRL);
        }
        let old = ir.new_temp(Type::I64);
        ir.gen_qemu_ld(Type::I64, old, addr, memop.bits() as u32);
        let src2 = self.gpr_or_zero(ir, a.rs2);
        let new = ir.new_temp(Type::I64);
        op(ir, Type::I64, new, old, src2);
        ir.gen_qemu_st(Type::I64, new, addr, memop.bits() as u32);
        if a.aq != 0 {
            ir.gen_mb(TCG_MO_ALL | TCG_BAR_LDAQ);
        }
        self.gen_set_gpr(ir, a.rd, old);
        true
    }

    /// AMO swap: store rs2, return old value.
    pub(super) fn gen_amo_swap(
        &self,
        ir: &mut Context,
        a: &ArgsAtomic,
        memop: MemOp,
    ) -> bool {
        let addr = self.gpr_or_zero(ir, a.rs1);
        if a.rl != 0 {
            ir.gen_mb(TCG_MO_ALL | TCG_BAR_STRL);
        }
        let old = ir.new_temp(Type::I64);
        ir.gen_qemu_ld(Type::I64, old, addr, memop.bits() as u32);
        let src2 = self.gpr_or_zero(ir, a.rs2);
        ir.gen_qemu_st(Type::I64, src2, addr, memop.bits() as u32);
        if a.aq != 0 {
            ir.gen_mb(TCG_MO_ALL | TCG_BAR_LDAQ);
        }
        self.gen_set_gpr(ir, a.rd, old);
        true
    }

    /// AMO min/max: conditional select via movcond.
    pub(super) fn gen_amo_minmax(
        &self,
        ir: &mut Context,
        a: &ArgsAtomic,
        cond: Cond,
        memop: MemOp,
    ) -> bool {
        let addr = self.gpr_or_zero(ir, a.rs1);
        if a.rl != 0 {
            ir.gen_mb(TCG_MO_ALL | TCG_BAR_STRL);
        }
        let old = ir.new_temp(Type::I64);
        ir.gen_qemu_ld(Type::I64, old, addr, memop.bits() as u32);
        let src2 = self.gpr_or_zero(ir, a.rs2);
        let new = ir.new_temp(Type::I64);
        // new = (old cond src2) ? old : src2
        ir.gen_movcond(Type::I64, new, old, src2, old, src2, cond);
        ir.gen_qemu_st(Type::I64, new, addr, memop.bits() as u32);
        if a.aq != 0 {
            ir.gen_mb(TCG_MO_ALL | TCG_BAR_LDAQ);
        }
        self.gen_set_gpr(ir, a.rd, old);
        true
    }

    // -- Branch helper -------------------------------------

    /// Conditional branch that terminates the TB.
    pub(super) fn gen_branch(
        &mut self,
        ir: &mut Context,
        a: &ArgsB,
        cond: Cond,
    ) {
        let src1 = self.gpr_or_zero(ir, a.rs1);
        let src2 = self.gpr_or_zero(ir, a.rs2);

        let taken = ir.new_label();
        ir.gen_brcond(Type::I64, src1, src2, cond, taken);

        // Not taken: PC = next insn, return chain slot 0.
        let next_pc = self.base.pc_next + self.cur_insn_len as u64;
        let c = ir.new_const(Type::I64, next_pc);
        ir.gen_mov(Type::I64, self.pc, c);
        ir.gen_goto_tb(0);
        ir.gen_exit_tb(TB_EXIT_IDX0);

        // Taken: PC = branch target, return chain slot 1.
        ir.gen_set_label(taken);
        let target = (self.base.pc_next as i64 + a.imm) as u64;
        let c = ir.new_const(Type::I64, target);
        ir.gen_mov(Type::I64, self.pc, c);
        ir.gen_goto_tb(1);
        ir.gen_exit_tb(TB_EXIT_IDX1);

        self.base.is_jmp = DisasJumpType::NoReturn;
    }
}
