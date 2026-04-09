//! Zbkb (Crypto Bit Manipulation, RV64 subset) gen
//! helpers.  brev8 uses a helper call; pack/packh/packw
//! use inline IR.

use super::super::insn_decode::*;
use super::super::RiscvDisasContext;
use super::helpers::helper_brev8;
use machina_accel::ir::context::Context;
use machina_accel::ir::types::Type;

impl RiscvDisasContext {
    pub(super) fn gen_brev8(&self, ir: &mut Context, a: &ArgsR2) -> bool {
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let r = ir.new_temp(Type::I64);
        ir.gen_call(r, helper_brev8 as *const () as u64, &[s1]);
        self.gen_set_gpr(ir, a.rd, r);
        true
    }

    // pack: rd = rs1[31:0] | (rs2[31:0] << 32)
    pub(super) fn gen_pack(&self, ir: &mut Context, a: &ArgsR) -> bool {
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let s2 = self.gpr_or_zero(ir, a.rs2);
        let lo = ir.new_temp(Type::I64);
        let mask = ir.new_const(Type::I64, 0xffff_ffff);
        ir.gen_and(Type::I64, lo, s1, mask);
        let c32 = ir.new_const(Type::I64, 32);
        let hi = ir.new_temp(Type::I64);
        ir.gen_shl(Type::I64, hi, s2, c32);
        let d = ir.new_temp(Type::I64);
        ir.gen_or(Type::I64, d, lo, hi);
        self.gen_set_gpr(ir, a.rd, d);
        true
    }

    // packh: rd = rs1[7:0] | (rs2[7:0] << 8)
    pub(super) fn gen_packh(&self, ir: &mut Context, a: &ArgsR) -> bool {
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let s2 = self.gpr_or_zero(ir, a.rs2);
        let lo = ir.new_temp(Type::I64);
        let m8 = ir.new_const(Type::I64, 0xff);
        ir.gen_and(Type::I64, lo, s1, m8);
        let hi_byte = ir.new_temp(Type::I64);
        ir.gen_and(Type::I64, hi_byte, s2, m8);
        let c8 = ir.new_const(Type::I64, 8);
        let hi = ir.new_temp(Type::I64);
        ir.gen_shl(Type::I64, hi, hi_byte, c8);
        let d = ir.new_temp(Type::I64);
        ir.gen_or(Type::I64, d, lo, hi);
        self.gen_set_gpr(ir, a.rd, d);
        true
    }

    // packw: 32-bit pack, sign-extended to 64
    // rd = sext32(rs1[15:0] | (rs2[15:0] << 16))
    pub(super) fn gen_packw(&self, ir: &mut Context, a: &ArgsR) -> bool {
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let s2 = self.gpr_or_zero(ir, a.rs2);
        let s1_32 = ir.new_temp(Type::I32);
        ir.gen_extrl_i64_i32(s1_32, s1);
        let s2_32 = ir.new_temp(Type::I32);
        ir.gen_extrl_i64_i32(s2_32, s2);
        let m16 = ir.new_const(Type::I32, 0xffff);
        let lo = ir.new_temp(Type::I32);
        ir.gen_and(Type::I32, lo, s1_32, m16);
        let c16 = ir.new_const(Type::I32, 16);
        let hi = ir.new_temp(Type::I32);
        ir.gen_shl(Type::I32, hi, s2_32, c16);
        let d32 = ir.new_temp(Type::I32);
        ir.gen_or(Type::I32, d32, lo, hi);
        self.gen_set_gpr_sx32(ir, a.rd, d32);
        true
    }
}
