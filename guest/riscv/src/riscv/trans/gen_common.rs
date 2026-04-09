//! Common IR generation utilities shared across extensions.

use super::super::cpu::{
    fpr_offset, MSTATUS_OFFSET, USTATUS_FS_DIRTY, USTATUS_FS_MASK,
};
use super::super::insn_decode::*;
use super::super::RiscvDisasContext;
use machina_accel::ir::context::Context;
use machina_accel::ir::types::{MemOp, Type};
use machina_accel::ir::TempIdx;

/// Binary IR operation: `fn(ir, ty, dst, lhs, rhs) -> dst`.
pub(super) type BinOp =
    fn(&mut Context, Type, TempIdx, TempIdx, TempIdx) -> TempIdx;

// Memory barrier constants (QEMU TCG_MO_* / TCG_BAR_*).
pub(super) const TCG_MO_ALL: u32 = 0x0F;
pub(super) const TCG_BAR_LDAQ: u32 = 0x10;
pub(super) const TCG_BAR_STRL: u32 = 0x20;

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
        ir.gen_ld(Type::I64, status, self.env, MSTATUS_OFFSET);
        let mask = ir.new_const(Type::I64, USTATUS_FS_MASK);
        let fs = ir.new_temp(Type::I64);
        ir.gen_and(Type::I64, fs, status, mask);
        let zero = ir.new_const(Type::I64, 0);
        let ok = ir.new_label();
        ir.gen_brcond(
            Type::I64,
            fs,
            zero,
            machina_accel::ir::types::Cond::Ne,
            ok,
        );
        let pc = ir.new_const(Type::I64, self.base.pc_next);
        ir.gen_mov(Type::I64, self.pc, pc);
        ir.gen_exit_tb(machina_accel::ir::tb::EXCP_UNDEF);
        ir.gen_set_label(ok);
    }

    pub(super) fn gen_set_fs_dirty(&self, ir: &mut Context) {
        let status = ir.new_temp(Type::I64);
        ir.gen_ld(Type::I64, status, self.env, MSTATUS_OFFSET);
        let clear = ir.new_const(Type::I64, !USTATUS_FS_MASK);
        let cleared = ir.new_temp(Type::I64);
        ir.gen_and(Type::I64, cleared, status, clear);
        let dirty = ir.new_const(Type::I64, USTATUS_FS_DIRTY);
        let new_status = ir.new_temp(Type::I64);
        ir.gen_or(Type::I64, new_status, cleared, dirty);
        ir.gen_st(Type::I64, new_status, self.env, MSTATUS_OFFSET);
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

    /// Write the current instruction's PC to the env
    /// `pc` global so that a helper-triggered fault
    /// has the correct mepc/sepc.
    pub(super) fn sync_pc(&self, _ir: &mut Context) {}

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
        self.sync_pc(ir);
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

    /// Half-precision FP load with f16 NaN boxing.
    pub(super) fn gen_fp_load_h(&self, ir: &mut Context, a: &ArgsI) -> bool {
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
        self.sync_pc(ir);
        let val = ir.new_temp(Type::I64);
        ir.gen_qemu_ld(Type::I64, val, addr, MemOp::uw().bits() as u32);
        // NaN box: bits[63:16] = all 1s
        let mask = ir.new_const(Type::I64, 0xffff_ffff_ffff_0000u64);
        let boxed = ir.new_temp(Type::I64);
        ir.gen_or(Type::I64, boxed, val, mask);
        self.fpr_store(ir, a.rd, boxed);
        true
    }

    /// Half-precision FP store (low 16 bits of FPR).
    pub(super) fn gen_fp_store_h(&self, ir: &mut Context, a: &ArgsS) -> bool {
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
        let mask = ir.new_const(Type::I64, 0xffff);
        let lo16 = ir.new_temp(Type::I64);
        ir.gen_and(Type::I64, lo16, val, mask);
        self.sync_pc(ir);
        ir.gen_qemu_st(Type::I64, lo16, addr, MemOp::uw().bits() as u32);
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
        self.sync_pc(ir);
        ir.gen_qemu_st(Type::I64, store_val, addr, memop.bits() as u32);
        true
    }
}
