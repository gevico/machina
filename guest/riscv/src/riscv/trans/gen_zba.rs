//! Zba (Address Computation) gen helpers.

use super::super::insn_decode::*;
use super::super::RiscvDisasContext;
use machina_accel::ir::context::Context;
use machina_accel::ir::types::Type;

impl RiscvDisasContext {
    // sh{1,2,3}add: rd = (rs1 << N) + rs2

    fn gen_shadd(&self, ir: &mut Context, a: &ArgsR, shift: u64) -> bool {
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let s2 = self.gpr_or_zero(ir, a.rs2);
        let sh = ir.new_const(Type::I64, shift);
        let t = ir.new_temp(Type::I64);
        ir.gen_shl(Type::I64, t, s1, sh);
        let d = ir.new_temp(Type::I64);
        ir.gen_add(Type::I64, d, t, s2);
        self.gen_set_gpr(ir, a.rd, d);
        true
    }

    pub(super) fn gen_sh1add(&self, ir: &mut Context, a: &ArgsR) -> bool {
        self.gen_shadd(ir, a, 1)
    }

    pub(super) fn gen_sh2add(&self, ir: &mut Context, a: &ArgsR) -> bool {
        self.gen_shadd(ir, a, 2)
    }

    pub(super) fn gen_sh3add(&self, ir: &mut Context, a: &ArgsR) -> bool {
        self.gen_shadd(ir, a, 3)
    }

    // sh{1,2,3}add.uw: rd = (zext32(rs1) << N) + rs2

    fn gen_shadd_uw(&self, ir: &mut Context, a: &ArgsR, shift: u64) -> bool {
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let s2 = self.gpr_or_zero(ir, a.rs2);
        let lo = ir.new_temp(Type::I32);
        ir.gen_extrl_i64_i32(lo, s1);
        let zext = ir.new_temp(Type::I64);
        ir.gen_ext_u32_i64(zext, lo);
        let sh = ir.new_const(Type::I64, shift);
        let t = ir.new_temp(Type::I64);
        ir.gen_shl(Type::I64, t, zext, sh);
        let d = ir.new_temp(Type::I64);
        ir.gen_add(Type::I64, d, t, s2);
        self.gen_set_gpr(ir, a.rd, d);
        true
    }

    pub(super) fn gen_sh1add_uw(&self, ir: &mut Context, a: &ArgsR) -> bool {
        self.gen_shadd_uw(ir, a, 1)
    }

    pub(super) fn gen_sh2add_uw(&self, ir: &mut Context, a: &ArgsR) -> bool {
        self.gen_shadd_uw(ir, a, 2)
    }

    pub(super) fn gen_sh3add_uw(&self, ir: &mut Context, a: &ArgsR) -> bool {
        self.gen_shadd_uw(ir, a, 3)
    }

    // add.uw: rd = zext32(rs1) + rs2

    pub(super) fn gen_add_uw(&self, ir: &mut Context, a: &ArgsR) -> bool {
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let s2 = self.gpr_or_zero(ir, a.rs2);
        let lo = ir.new_temp(Type::I32);
        ir.gen_extrl_i64_i32(lo, s1);
        let zext = ir.new_temp(Type::I64);
        ir.gen_ext_u32_i64(zext, lo);
        let d = ir.new_temp(Type::I64);
        ir.gen_add(Type::I64, d, zext, s2);
        self.gen_set_gpr(ir, a.rd, d);
        true
    }

    // slli.uw: rd = zext32(rs1) << shamt

    pub(super) fn gen_slli_uw(&self, ir: &mut Context, a: &ArgsShift) -> bool {
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let lo = ir.new_temp(Type::I32);
        ir.gen_extrl_i64_i32(lo, s1);
        let zext = ir.new_temp(Type::I64);
        ir.gen_ext_u32_i64(zext, lo);
        let sh = ir.new_const(Type::I64, a.shamt as u64);
        let d = ir.new_temp(Type::I64);
        ir.gen_shl(Type::I64, d, zext, sh);
        self.gen_set_gpr(ir, a.rd, d);
        true
    }
}
