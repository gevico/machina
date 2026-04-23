//! Zbc gen helpers: carry-less multiplication via
//! helper calls (no direct x86-64 mapping).

use super::super::insn_decode::*;
use super::super::RiscvDisasContext;
use machina_accel::ir::context::Context;
use machina_accel::ir::types::Type;

impl RiscvDisasContext {
    /// Emit a carry-less multiplication helper call.
    pub(super) fn gen_clmul_op(
        &self,
        ir: &mut Context,
        a: &ArgsR,
        helper: extern "sysv64" fn(u64, u64) -> u64,
    ) -> bool {
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let s2 = self.gpr_or_zero(ir, a.rs2);
        let r = ir.new_temp(Type::I64);
        ir.gen_call(r, helper as *const () as u64, &[s1, s2]);
        self.gen_set_gpr(ir, a.rd, r);
        true
    }
}
