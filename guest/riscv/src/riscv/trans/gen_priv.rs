//! Privileged gen helpers: CSR read/write, priv exit.

use super::super::cpu::{
    FFLAGS_OFFSET, FRM_OFFSET, UCAUSE_OFFSET, UEPC_OFFSET, UIE_OFFSET,
    UIP_OFFSET, USCRATCH_OFFSET, USTATUS_OFFSET, UTVAL_OFFSET, UTVEC_OFFSET,
};
use super::super::fpu;
use super::super::RiscvDisasContext;
use machina_accel::ir::context::Context;
use machina_accel::ir::types::Type;
use machina_accel::ir::TempIdx;

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

impl RiscvDisasContext {
    /// Emit a TB exit for privileged CSR access.
    /// PC is synced to the current instruction so the
    /// exec loop can re-decode and execute at runtime.
    pub(super) fn gen_priv_csr_exit(&mut self, ir: &mut Context) {
        use machina_accel::ir::tb::EXCP_PRIV_CSR;
        let cur_pc = self.base.pc_next;
        let pc = ir.new_const(Type::I64, cur_pc);
        ir.gen_mov(Type::I64, self.pc, pc);
        ir.gen_exit_tb(EXCP_PRIV_CSR);
        self.base.is_jmp = crate::DisasJumpType::NoReturn;
    }

    /// Generate a helper call for the full CSR operation.
    /// The helper does read-modify-write and returns old
    /// value. On illegal access, it raises exception via
    /// longjmp (never returns).
    pub(super) fn gen_csr_helper(
        &self,
        ir: &mut Context,
        csr: i64,
        rs1_val: TempIdx,
        funct3: u32,
        rd: i64,
    ) {
        // Sync PC so raise_exception has correct mepc.
        let cur_pc = self.base.pc_next;
        let pc = ir.new_const(Type::I64, cur_pc);
        ir.gen_mov(Type::I64, self.pc, pc);

        let csr_arg = ir.new_const(Type::I64, csr as u64);
        let f3_arg = ir.new_const(Type::I64, funct3 as u64);
        let old = self.gen_helper_call(
            ir,
            self.csr_helper as usize,
            &[self.env, csr_arg, rs1_val, f3_arg],
        );
        self.gen_set_gpr(ir, rd, old);

        // Advance PC past the CSR instruction (helper
        // synced PC to cur_pc for exception handling,
        // but on normal return we need next_pc).
        let next_pc = cur_pc + self.cur_insn_len as u64;
        let next = ir.new_const(Type::I64, next_pc);
        ir.gen_mov(Type::I64, self.pc, next);
    }

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
            CSR_CYCLE | CSR_TIME | CSR_INSTRET => None,
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
}
