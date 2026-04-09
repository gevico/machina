//! Zbkx (Crypto Crossbar Permutation) gen helpers.
//! xperm4/xperm8 use helper calls.

use super::super::insn_decode::*;
use super::super::RiscvDisasContext;
use super::helpers::{helper_xperm4, helper_xperm8};
use machina_accel::ir::context::Context;
use machina_accel::ir::types::Type;

impl RiscvDisasContext {
    pub(super) fn gen_xperm4(&self, ir: &mut Context, a: &ArgsR) -> bool {
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let s2 = self.gpr_or_zero(ir, a.rs2);
        let r = ir.new_temp(Type::I64);
        ir.gen_call(r, helper_xperm4 as *const () as u64, &[s1, s2]);
        self.gen_set_gpr(ir, a.rd, r);
        true
    }

    pub(super) fn gen_xperm8(&self, ir: &mut Context, a: &ArgsR) -> bool {
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let s2 = self.gpr_or_zero(ir, a.rs2);
        let r = ir.new_temp(Type::I64);
        ir.gen_call(r, helper_xperm8 as *const () as u64, &[s1, s2]);
        self.gen_set_gpr(ir, a.rd, r);
        true
    }
}
