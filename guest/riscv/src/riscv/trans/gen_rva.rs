//! RVA gen helpers: atomic load-reserved / store-conditional,
//! AMO read-modify-write, swap, min/max.

use super::super::insn_decode::*;
use super::super::RiscvDisasContext;
use super::gen_common::{BinOp, TCG_BAR_LDAQ, TCG_BAR_STRL, TCG_MO_ALL};
use super::helpers::helper_sc;
use machina_accel::ir::context::Context;
use machina_accel::ir::types::{Cond, MemOp, Type};

impl RiscvDisasContext {
    /// LR: load-reserved.
    pub(super) fn gen_lr(
        &self,
        ir: &mut Context,
        a: &ArgsAtomic,
        memop: MemOp,
    ) -> bool {
        ir.contains_atomic = true;
        let addr = self.gpr_or_zero(ir, a.rs1);
        if a.rl != 0 {
            ir.gen_mb(TCG_MO_ALL | TCG_BAR_STRL);
        }
        self.sync_pc(ir);
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

    /// SC: store-conditional via helper.
    ///
    /// Uses a helper function to atomically check
    /// reservation and conditionally store. Returns
    /// 0 on success, 1 on failure.
    pub(super) fn gen_sc(
        &self,
        ir: &mut Context,
        a: &ArgsAtomic,
        memop: MemOp,
    ) -> bool {
        ir.contains_atomic = true;
        if a.rl != 0 {
            ir.gen_mb(TCG_MO_ALL | TCG_BAR_STRL);
        }
        let addr = self.gpr_or_zero(ir, a.rs1);
        let src2 = self.gpr_or_zero(ir, a.rs2);
        self.sync_pc(ir);
        let is_word = ir.new_const(Type::I64, memop.size_bytes() as u64);
        let r = ir.new_temp(Type::I64);
        ir.gen_call(
            r,
            helper_sc as *const () as u64,
            &[self.env, addr, src2, is_word],
        );
        if a.aq != 0 {
            ir.gen_mb(TCG_MO_ALL | TCG_BAR_LDAQ);
        }
        self.gen_set_gpr(ir, a.rd, r);
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
        ir.contains_atomic = true;
        let addr = self.gpr_or_zero(ir, a.rs1);
        if a.rl != 0 {
            ir.gen_mb(TCG_MO_ALL | TCG_BAR_STRL);
        }
        self.sync_pc(ir);
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
        ir.contains_atomic = true;
        let addr = self.gpr_or_zero(ir, a.rs1);
        if a.rl != 0 {
            ir.gen_mb(TCG_MO_ALL | TCG_BAR_STRL);
        }
        self.sync_pc(ir);
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
        ir.contains_atomic = true;
        let addr = self.gpr_or_zero(ir, a.rs1);
        if a.rl != 0 {
            ir.gen_mb(TCG_MO_ALL | TCG_BAR_STRL);
        }
        self.sync_pc(ir);
        let old = ir.new_temp(Type::I64);
        ir.gen_qemu_ld(Type::I64, old, addr, memop.bits() as u32);
        let src2 = self.gpr_or_zero(ir, a.rs2);

        // For 32-bit AMO, truncate src2 to 32 bits and
        // extend to match the loaded value's width.
        let is_32 = memop.size_bytes() == 4;
        let cmp_src2 = if is_32 {
            let t = ir.new_temp(Type::I64);
            let t32 = ir.new_temp(Type::I32);
            ir.gen_extrl_i64_i32(t32, src2);
            // Signed cond: sign-extend; unsigned: zero.
            if cond == Cond::Lt || cond == Cond::Gt {
                ir.gen_ext_i32_i64(t, t32);
            } else {
                ir.gen_ext_u32_i64(t, t32);
            }
            t
        } else {
            src2
        };

        // For unsigned 32-bit cmp, also zero-extend old
        // (which was sign-extended by the load).
        let cmp_old = if is_32 && (cond == Cond::Ltu || cond == Cond::Gtu) {
            let t = ir.new_temp(Type::I64);
            let t32 = ir.new_temp(Type::I32);
            ir.gen_extrl_i64_i32(t32, old);
            ir.gen_ext_u32_i64(t, t32);
            t
        } else {
            old
        };

        let new = ir.new_temp(Type::I64);
        ir.gen_movcond(Type::I64, new, cmp_old, cmp_src2, old, src2, cond);
        ir.gen_qemu_st(Type::I64, new, addr, memop.bits() as u32);
        if a.aq != 0 {
            ir.gen_mb(TCG_MO_ALL | TCG_BAR_LDAQ);
        }
        self.gen_set_gpr(ir, a.rd, old);
        true
    }
}
