//! RVM gen helpers: signed/unsigned division and remainder.

use super::super::insn_decode::*;
use super::super::RiscvDisasContext;
use super::helpers::{
    helper_divs64, helper_divw64, helper_rems64, helper_remw64,
};
use machina_accel::ir::context::Context;
use machina_accel::ir::types::{Cond, Type};

impl RiscvDisasContext {
    /// Signed division with RISC-V special-case handling
    /// via helper call.
    pub(super) fn gen_div_rem(
        &self,
        ir: &mut Context,
        a: &ArgsR,
        want_rem: bool,
    ) -> bool {
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let s2 = self.gpr_or_zero(ir, a.rs2);
        let r = ir.new_temp(Type::I64);
        let helper = if want_rem {
            helper_rems64 as *const () as u64
        } else {
            helper_divs64 as *const () as u64
        };
        ir.gen_call(r, helper, &[s1, s2]);
        self.gen_set_gpr(ir, a.rd, r);
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

    /// 32-bit signed division (W-suffix) via helper.
    pub(super) fn gen_div_rem_w(
        &self,
        ir: &mut Context,
        a: &ArgsR,
        want_rem: bool,
    ) -> bool {
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let s2 = self.gpr_or_zero(ir, a.rs2);
        let r = ir.new_temp(Type::I64);
        let helper = if want_rem {
            helper_remw64 as *const () as u64
        } else {
            helper_divw64 as *const () as u64
        };
        ir.gen_call(r, helper, &[s1, s2]);
        self.gen_set_gpr(ir, a.rd, r);
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
}
