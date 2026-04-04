//! RVI gen helpers: load/store, ALU, branch.

use super::super::insn_decode::*;
use super::super::RiscvDisasContext;
use super::gen_common::BinOp;
use crate::DisasJumpType;
use machina_accel::ir::context::Context;
use machina_accel::ir::tb::{TB_EXIT_IDX0, TB_EXIT_IDX1};
use machina_accel::ir::types::{Cond, MemOp, Type};

impl RiscvDisasContext {
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
        self.sync_pc(ir);
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
        self.sync_pc(ir);
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

        // Not taken: PC = next insn, chain slot 0.
        let next_pc = self.base.pc_next + self.cur_insn_len as u64;
        let c = ir.new_const(Type::I64, next_pc);
        ir.gen_mov(Type::I64, self.pc, c);
        ir.gen_goto_tb(0);
        ir.gen_exit_tb(TB_EXIT_IDX0);

        // Taken: PC = branch target, chain slot 1.
        ir.gen_set_label(taken);
        let target = (self.base.pc_next as i64 + a.imm) as u64;
        let c = ir.new_const(Type::I64, target);
        ir.gen_mov(Type::I64, self.pc, c);
        ir.gen_goto_tb(1);
        ir.gen_exit_tb(TB_EXIT_IDX1);

        self.base.is_jmp = DisasJumpType::NoReturn;
    }
}
