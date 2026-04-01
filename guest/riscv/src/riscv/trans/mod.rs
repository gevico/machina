//! RISC-V instruction translation — TCG IR generation.
//!
//! Follows QEMU's gen_xxx helper pattern: repetitive instruction
//! translation logic is factored into gen_arith, gen_arith_imm,
//! gen_shift_imm, gen_shiftw, etc., each parameterised by a
//! `BinOp` function pointer.

mod helpers;

use super::ext::MisaExt;
use super::fpu;
use super::insn_decode::*;
use super::RiscvDisasContext;
use crate::DisasJumpType;
use machina_accel::ir::context::Context;
use machina_accel::ir::tb::{
    EXCP_EBREAK, EXCP_ECALL, EXCP_FENCE_I, EXCP_MRET, EXCP_SFENCE_VMA,
    EXCP_SRET, EXCP_WFI, TB_EXIT_IDX0, TB_EXIT_NOCHAIN,
};
use machina_accel::ir::types::{Cond, MemOp, Type};

/// Bail out (return false) if the MISA letter-extension
/// is absent.
macro_rules! require_ext {
    ($ctx:expr, $ext:expr) => {
        if !$ctx.cfg.misa.contains($ext) {
            return false;
        }
    };
}

/// Bail out (return false) if a Z-extension bool field
/// is false.
macro_rules! require_cfg {
    ($ctx:expr, $field:ident) => {
        if !$ctx.cfg.$field {
            return false;
        }
    };
}

// ── Decode trait implementation ────────────────────────────────

impl Decode<Context> for RiscvDisasContext {
    // ── RV32I: Upper immediate ─────────────────────────

    fn trans_lui(&mut self, ir: &mut Context, a: &ArgsU) -> bool {
        let c = ir.new_const(Type::I64, a.imm as u64);
        self.gen_set_gpr(ir, a.rd, c);
        true
    }

    fn trans_auipc(&mut self, ir: &mut Context, a: &ArgsU) -> bool {
        let v = (self.base.pc_next as i64 + a.imm) as u64;
        let c = ir.new_const(Type::I64, v);
        self.gen_set_gpr(ir, a.rd, c);
        true
    }

    // ── RV32I: Jumps ───────────────────────────────────

    fn trans_jal(&mut self, ir: &mut Context, a: &ArgsJ) -> bool {
        let link = self.base.pc_next + self.cur_insn_len as u64;
        let c = ir.new_const(Type::I64, link);
        self.gen_set_gpr(ir, a.rd, c);
        let target = (self.base.pc_next as i64 + a.imm) as u64;
        let c = ir.new_const(Type::I64, target);
        ir.gen_mov(Type::I64, self.pc, c);
        ir.gen_goto_tb(0);
        ir.gen_exit_tb(TB_EXIT_IDX0);
        self.base.is_jmp = DisasJumpType::NoReturn;
        true
    }

    fn trans_jalr(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        let link = self.base.pc_next + self.cur_insn_len as u64;
        let src = self.gpr_or_zero(ir, a.rs1);
        let imm = ir.new_const(Type::I64, a.imm as u64);
        let tmp = ir.new_temp(Type::I64);
        ir.gen_add(Type::I64, tmp, src, imm);
        // Clear bit 0
        let mask = ir.new_const(Type::I64, !1u64);
        ir.gen_and(Type::I64, tmp, tmp, mask);
        let c = ir.new_const(Type::I64, link);
        self.gen_set_gpr(ir, a.rd, c);
        ir.gen_mov(Type::I64, self.pc, tmp);
        ir.gen_exit_tb(TB_EXIT_NOCHAIN);
        self.base.is_jmp = DisasJumpType::NoReturn;
        true
    }

    // ── RV32I: Branches ────────────────────────────────

    fn trans_beq(&mut self, ir: &mut Context, a: &ArgsB) -> bool {
        self.gen_branch(ir, a, Cond::Eq);
        true
    }
    fn trans_bne(&mut self, ir: &mut Context, a: &ArgsB) -> bool {
        self.gen_branch(ir, a, Cond::Ne);
        true
    }
    fn trans_blt(&mut self, ir: &mut Context, a: &ArgsB) -> bool {
        self.gen_branch(ir, a, Cond::Lt);
        true
    }
    fn trans_bge(&mut self, ir: &mut Context, a: &ArgsB) -> bool {
        self.gen_branch(ir, a, Cond::Ge);
        true
    }
    fn trans_bltu(&mut self, ir: &mut Context, a: &ArgsB) -> bool {
        self.gen_branch(ir, a, Cond::Ltu);
        true
    }
    fn trans_bgeu(&mut self, ir: &mut Context, a: &ArgsB) -> bool {
        self.gen_branch(ir, a, Cond::Geu);
        true
    }

    // ── RV32I: Loads ──────────────────────────────────

    fn trans_lb(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        self.gen_load(ir, a, MemOp::sb())
    }
    fn trans_lh(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        self.gen_load(ir, a, MemOp::sw())
    }
    fn trans_lw(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        self.gen_load(ir, a, MemOp::sl())
    }
    fn trans_lbu(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        self.gen_load(ir, a, MemOp::ub())
    }
    fn trans_lhu(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        self.gen_load(ir, a, MemOp::uw())
    }

    // ── RV32I: Stores ─────────────────────────────────

    fn trans_sb(&mut self, ir: &mut Context, a: &ArgsS) -> bool {
        self.gen_store(ir, a, MemOp::ub())
    }
    fn trans_sh(&mut self, ir: &mut Context, a: &ArgsS) -> bool {
        self.gen_store(ir, a, MemOp::uw())
    }
    fn trans_sw(&mut self, ir: &mut Context, a: &ArgsS) -> bool {
        self.gen_store(ir, a, MemOp::ul())
    }

    // ── RV32I: ALU immediate ───────────────────────────

    fn trans_addi(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        self.gen_arith_imm(ir, a, Context::gen_add)
    }
    fn trans_slti(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        self.gen_setcond_imm(ir, a, Cond::Lt)
    }
    fn trans_sltiu(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        self.gen_setcond_imm(ir, a, Cond::Ltu)
    }
    fn trans_xori(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        self.gen_arith_imm(ir, a, Context::gen_xor)
    }
    fn trans_ori(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        self.gen_arith_imm(ir, a, Context::gen_or)
    }
    fn trans_andi(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        self.gen_arith_imm(ir, a, Context::gen_and)
    }

    // ── RV32I: Shift immediate ─────────────────────────

    fn trans_slli(&mut self, ir: &mut Context, a: &ArgsShift) -> bool {
        self.gen_shift_imm(ir, a, Context::gen_shl)
    }
    fn trans_srli(&mut self, ir: &mut Context, a: &ArgsShift) -> bool {
        self.gen_shift_imm(ir, a, Context::gen_shr)
    }
    fn trans_srai(&mut self, ir: &mut Context, a: &ArgsShift) -> bool {
        self.gen_shift_imm(ir, a, Context::gen_sar)
    }

    // ── RV32I: R-type ALU ──────────────────────────────

    fn trans_add(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        self.gen_arith(ir, a, Context::gen_add)
    }
    fn trans_sub(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        self.gen_arith(ir, a, Context::gen_sub)
    }
    fn trans_sll(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        self.gen_arith(ir, a, Context::gen_shl)
    }
    fn trans_slt(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        self.gen_setcond_rr(ir, a, Cond::Lt)
    }
    fn trans_sltu(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        self.gen_setcond_rr(ir, a, Cond::Ltu)
    }
    fn trans_xor(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        self.gen_arith(ir, a, Context::gen_xor)
    }
    fn trans_srl(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        self.gen_arith(ir, a, Context::gen_shr)
    }
    fn trans_sra(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        self.gen_arith(ir, a, Context::gen_sar)
    }
    fn trans_or(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        self.gen_arith(ir, a, Context::gen_or)
    }
    fn trans_and(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        self.gen_arith(ir, a, Context::gen_and)
    }

    // ── RV32I: Fence / System ──────────────────────────

    fn trans_fence(&mut self, _ir: &mut Context, _a: &ArgsAutoFence) -> bool {
        true // NOP
    }

    fn trans_fence_i(&mut self, ir: &mut Context, _a: &ArgsEmpty) -> bool {
        // Exit TB so the exec loop can invalidate TBs
        // by physical page (fence.i semantics).
        let next = self.base.pc_next + self.cur_insn_len as u64;
        let pc = ir.new_const(Type::I64, next);
        ir.gen_mov(Type::I64, self.pc, pc);
        ir.gen_exit_tb(EXCP_FENCE_I);
        self.base.is_jmp = DisasJumpType::NoReturn;
        true
    }

    fn trans_ecall(&mut self, ir: &mut Context, _a: &ArgsEmpty) -> bool {
        let pc = ir.new_const(Type::I64, self.base.pc_next);
        ir.gen_mov(Type::I64, self.pc, pc);
        ir.gen_exit_tb(EXCP_ECALL);
        self.base.is_jmp = DisasJumpType::NoReturn;
        true
    }

    fn trans_ebreak(&mut self, ir: &mut Context, _a: &ArgsEmpty) -> bool {
        let pc = ir.new_const(Type::I64, self.base.pc_next);
        ir.gen_mov(Type::I64, self.pc, pc);
        ir.gen_exit_tb(EXCP_EBREAK);
        self.base.is_jmp = DisasJumpType::NoReturn;
        true
    }

    // ── Privileged: trap return / system ──────────────────

    fn trans_mret(&mut self, ir: &mut Context, _a: &ArgsEmpty) -> bool {
        // Write next-insn PC so exec loop doesn't re-execute
        // this instruction after mret changes privilege.
        let next = self.base.pc_next + self.cur_insn_len as u64;
        let pc = ir.new_const(Type::I64, next);
        ir.gen_mov(Type::I64, self.pc, pc);
        ir.gen_exit_tb(EXCP_MRET);
        self.base.is_jmp = DisasJumpType::NoReturn;
        true
    }

    fn trans_sret(&mut self, ir: &mut Context, _a: &ArgsEmpty) -> bool {
        let next = self.base.pc_next + self.cur_insn_len as u64;
        let pc = ir.new_const(Type::I64, next);
        ir.gen_mov(Type::I64, self.pc, pc);
        ir.gen_exit_tb(EXCP_SRET);
        self.base.is_jmp = DisasJumpType::NoReturn;
        true
    }

    fn trans_wfi(&mut self, ir: &mut Context, _a: &ArgsEmpty) -> bool {
        // Write next-insn PC so resume after WFI starts at
        // the following instruction.
        let next = self.base.pc_next + self.cur_insn_len as u64;
        let pc = ir.new_const(Type::I64, next);
        ir.gen_mov(Type::I64, self.pc, pc);
        ir.gen_exit_tb(EXCP_WFI);
        self.base.is_jmp = DisasJumpType::NoReturn;
        true
    }

    fn trans_sfence_vma(&mut self, ir: &mut Context, _a: &ArgsR) -> bool {
        // Always perform a full TLB flush (correct but
        // conservative).  Exit TB with EXCP_SFENCE_VMA so the
        // execution loop calls cpu.tlb_flush().
        // No chaining: address translation may change.
        //
        // TODO: page-specific flush optimisation is deferred.
        // When rs1 != x0 the spec allows flushing only the
        // single page identified by rs1, which requires
        // either (a) storing the rs1 value in a CPU-state
        // scratch field before the exit and reading it in the
        // exec loop, or (b) introducing a distinct exit code
        // per flush granularity.  Until then the full-flush
        // fallback is architecturally correct.
        let next = self.base.pc_next + self.cur_insn_len as u64;
        let pc = ir.new_const(Type::I64, next);
        ir.gen_mov(Type::I64, self.pc, pc);
        ir.gen_exit_tb(EXCP_SFENCE_VMA);
        self.base.is_jmp = DisasJumpType::NoReturn;
        true
    }

    // ── RV64I: Loads / Stores (need guest memory) ──────

    fn trans_lwu(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        self.gen_load(ir, a, MemOp::ul())
    }
    fn trans_ld(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        self.gen_load(ir, a, MemOp::uq())
    }
    fn trans_sd(&mut self, ir: &mut Context, a: &ArgsS) -> bool {
        self.gen_store(ir, a, MemOp::uq())
    }

    // ── RV64I: W-suffix ALU ────────────────────────────

    fn trans_addiw(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        self.gen_arith_imm_w(ir, a, Context::gen_add)
    }
    fn trans_slliw(&mut self, ir: &mut Context, a: &ArgsShift) -> bool {
        self.gen_shift_imm_w(ir, a, Context::gen_shl)
    }
    fn trans_srliw(&mut self, ir: &mut Context, a: &ArgsShift) -> bool {
        self.gen_shift_imm_w(ir, a, Context::gen_shr)
    }
    fn trans_sraiw(&mut self, ir: &mut Context, a: &ArgsShift) -> bool {
        self.gen_shift_imm_w(ir, a, Context::gen_sar)
    }
    fn trans_addw(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        self.gen_arith_w(ir, a, Context::gen_add)
    }
    fn trans_subw(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        self.gen_arith_w(ir, a, Context::gen_sub)
    }
    fn trans_sllw(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        self.gen_shiftw(ir, a, Context::gen_shl)
    }
    fn trans_srlw(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        self.gen_shiftw(ir, a, Context::gen_shr)
    }
    fn trans_sraw(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        self.gen_shiftw(ir, a, Context::gen_sar)
    }

    // ── RV32M: Multiply / Divide ────────────────────────

    fn trans_mul(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::M);
        self.gen_arith(ir, a, Context::gen_mul)
    }

    fn trans_mulh(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::M);
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let s2 = self.gpr_or_zero(ir, a.rs2);
        let lo = ir.new_temp(Type::I64);
        let hi = ir.new_temp(Type::I64);
        ir.gen_muls2(Type::I64, lo, hi, s1, s2);
        self.gen_set_gpr(ir, a.rd, hi);
        true
    }

    fn trans_mulhsu(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::M);
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let s2 = self.gpr_or_zero(ir, a.rs2);
        let lo = ir.new_temp(Type::I64);
        let hi = ir.new_temp(Type::I64);
        ir.gen_mulu2(Type::I64, lo, hi, s1, s2);
        // Correction: high -= (s1 >> 63) & s2
        let c63 = ir.new_const(Type::I64, 63);
        let sign = ir.new_temp(Type::I64);
        ir.gen_sar(Type::I64, sign, s1, c63);
        let adj = ir.new_temp(Type::I64);
        ir.gen_and(Type::I64, adj, sign, s2);
        ir.gen_sub(Type::I64, hi, hi, adj);
        self.gen_set_gpr(ir, a.rd, hi);
        true
    }

    fn trans_mulhu(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::M);
        let s1 = self.gpr_or_zero(ir, a.rs1);
        let s2 = self.gpr_or_zero(ir, a.rs2);
        let lo = ir.new_temp(Type::I64);
        let hi = ir.new_temp(Type::I64);
        ir.gen_mulu2(Type::I64, lo, hi, s1, s2);
        self.gen_set_gpr(ir, a.rd, hi);
        true
    }

    fn trans_div(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::M);
        self.gen_div_rem(ir, a, false)
    }

    fn trans_divu(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::M);
        self.gen_divu_remu(ir, a, false)
    }

    fn trans_rem(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::M);
        self.gen_div_rem(ir, a, true)
    }

    fn trans_remu(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::M);
        self.gen_divu_remu(ir, a, true)
    }

    // ── RV64M: W-suffix Mul / Div ─────────────────────

    fn trans_mulw(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::M);
        self.gen_arith_w(ir, a, Context::gen_mul)
    }

    fn trans_divw(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::M);
        self.gen_div_rem_w(ir, a, false)
    }

    fn trans_divuw(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::M);
        self.gen_divu_remu_w(ir, a, false)
    }

    fn trans_remw(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::M);
        self.gen_div_rem_w(ir, a, true)
    }

    fn trans_remuw(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::M);
        self.gen_divu_remu_w(ir, a, true)
    }

    // ── RV32A: Atomic ─────────────────────────────────────

    fn trans_lr_w(&mut self, ir: &mut Context, a: &ArgsAtomic) -> bool {
        require_ext!(self, MisaExt::A);
        self.gen_lr(ir, a, MemOp::sl())
    }
    fn trans_sc_w(&mut self, ir: &mut Context, a: &ArgsAtomic) -> bool {
        require_ext!(self, MisaExt::A);
        self.gen_sc(ir, a, MemOp::ul())
    }
    fn trans_amoswap_w(&mut self, ir: &mut Context, a: &ArgsAtomic) -> bool {
        require_ext!(self, MisaExt::A);
        self.gen_amo_swap(ir, a, MemOp::sl())
    }
    fn trans_amoadd_w(&mut self, ir: &mut Context, a: &ArgsAtomic) -> bool {
        require_ext!(self, MisaExt::A);
        self.gen_amo(ir, a, Context::gen_add, MemOp::sl())
    }
    fn trans_amoxor_w(&mut self, ir: &mut Context, a: &ArgsAtomic) -> bool {
        require_ext!(self, MisaExt::A);
        self.gen_amo(ir, a, Context::gen_xor, MemOp::sl())
    }
    fn trans_amoand_w(&mut self, ir: &mut Context, a: &ArgsAtomic) -> bool {
        require_ext!(self, MisaExt::A);
        self.gen_amo(ir, a, Context::gen_and, MemOp::sl())
    }
    fn trans_amoor_w(&mut self, ir: &mut Context, a: &ArgsAtomic) -> bool {
        require_ext!(self, MisaExt::A);
        self.gen_amo(ir, a, Context::gen_or, MemOp::sl())
    }
    fn trans_amomin_w(&mut self, ir: &mut Context, a: &ArgsAtomic) -> bool {
        require_ext!(self, MisaExt::A);
        self.gen_amo_minmax(ir, a, Cond::Lt, MemOp::sl())
    }
    fn trans_amomax_w(&mut self, ir: &mut Context, a: &ArgsAtomic) -> bool {
        require_ext!(self, MisaExt::A);
        self.gen_amo_minmax(ir, a, Cond::Gt, MemOp::sl())
    }
    fn trans_amominu_w(&mut self, ir: &mut Context, a: &ArgsAtomic) -> bool {
        require_ext!(self, MisaExt::A);
        self.gen_amo_minmax(ir, a, Cond::Ltu, MemOp::sl())
    }
    fn trans_amomaxu_w(&mut self, ir: &mut Context, a: &ArgsAtomic) -> bool {
        require_ext!(self, MisaExt::A);
        self.gen_amo_minmax(ir, a, Cond::Gtu, MemOp::sl())
    }

    // ── RV64A: Atomic ─────────────────────────────────────

    fn trans_lr_d(&mut self, ir: &mut Context, a: &ArgsAtomic) -> bool {
        require_ext!(self, MisaExt::A);
        self.gen_lr(ir, a, MemOp::uq())
    }
    fn trans_sc_d(&mut self, ir: &mut Context, a: &ArgsAtomic) -> bool {
        require_ext!(self, MisaExt::A);
        self.gen_sc(ir, a, MemOp::uq())
    }
    fn trans_amoswap_d(&mut self, ir: &mut Context, a: &ArgsAtomic) -> bool {
        require_ext!(self, MisaExt::A);
        self.gen_amo_swap(ir, a, MemOp::uq())
    }
    fn trans_amoadd_d(&mut self, ir: &mut Context, a: &ArgsAtomic) -> bool {
        require_ext!(self, MisaExt::A);
        self.gen_amo(ir, a, Context::gen_add, MemOp::uq())
    }
    fn trans_amoxor_d(&mut self, ir: &mut Context, a: &ArgsAtomic) -> bool {
        require_ext!(self, MisaExt::A);
        self.gen_amo(ir, a, Context::gen_xor, MemOp::uq())
    }
    fn trans_amoand_d(&mut self, ir: &mut Context, a: &ArgsAtomic) -> bool {
        require_ext!(self, MisaExt::A);
        self.gen_amo(ir, a, Context::gen_and, MemOp::uq())
    }
    fn trans_amoor_d(&mut self, ir: &mut Context, a: &ArgsAtomic) -> bool {
        require_ext!(self, MisaExt::A);
        self.gen_amo(ir, a, Context::gen_or, MemOp::uq())
    }
    fn trans_amomin_d(&mut self, ir: &mut Context, a: &ArgsAtomic) -> bool {
        require_ext!(self, MisaExt::A);
        self.gen_amo_minmax(ir, a, Cond::Lt, MemOp::uq())
    }
    fn trans_amomax_d(&mut self, ir: &mut Context, a: &ArgsAtomic) -> bool {
        require_ext!(self, MisaExt::A);
        self.gen_amo_minmax(ir, a, Cond::Gt, MemOp::uq())
    }
    fn trans_amominu_d(&mut self, ir: &mut Context, a: &ArgsAtomic) -> bool {
        require_ext!(self, MisaExt::A);
        self.gen_amo_minmax(ir, a, Cond::Ltu, MemOp::uq())
    }
    fn trans_amomaxu_d(&mut self, ir: &mut Context, a: &ArgsAtomic) -> bool {
        require_ext!(self, MisaExt::A);
        self.gen_amo_minmax(ir, a, Cond::Gtu, MemOp::uq())
    }

    // ── Zicsr: CSR access ─────────────────────────────

    fn trans_csrrw(&mut self, ir: &mut Context, a: &ArgsCsr) -> bool {
        require_cfg!(self, ext_zicsr);
        let old = match self.gen_csr_read(ir, a.csr) {
            Some(v) => v,
            None => {
                self.gen_priv_csr_exit(ir);
                return true;
            }
        };
        let rs1 = self.gpr_or_zero(ir, a.rs1);
        if !self.gen_csr_write(ir, a.csr, rs1) {
            self.gen_priv_csr_exit(ir);
            return true;
        }
        self.gen_set_gpr(ir, a.rd, old);
        true
    }

    fn trans_csrrs(&mut self, ir: &mut Context, a: &ArgsCsr) -> bool {
        require_cfg!(self, ext_zicsr);
        let old = match self.gen_csr_read(ir, a.csr) {
            Some(v) => v,
            None => {
                self.gen_priv_csr_exit(ir);
                return true;
            }
        };
        if a.rs1 != 0 {
            let rs1 = self.gpr_or_zero(ir, a.rs1);
            let new = ir.new_temp(Type::I64);
            ir.gen_or(Type::I64, new, old, rs1);
            if !self.gen_csr_write(ir, a.csr, new) {
                self.gen_priv_csr_exit(ir);
                return true;
            }
        }
        self.gen_set_gpr(ir, a.rd, old);
        true
    }

    fn trans_csrrc(&mut self, ir: &mut Context, a: &ArgsCsr) -> bool {
        require_cfg!(self, ext_zicsr);
        let old = match self.gen_csr_read(ir, a.csr) {
            Some(v) => v,
            None => {
                self.gen_priv_csr_exit(ir);
                return true;
            }
        };
        if a.rs1 != 0 {
            let rs1 = self.gpr_or_zero(ir, a.rs1);
            let inv = ir.new_temp(Type::I64);
            ir.gen_not(Type::I64, inv, rs1);
            let new = ir.new_temp(Type::I64);
            ir.gen_and(Type::I64, new, old, inv);
            if !self.gen_csr_write(ir, a.csr, new) {
                self.gen_priv_csr_exit(ir);
                return true;
            }
        }
        self.gen_set_gpr(ir, a.rd, old);
        true
    }

    fn trans_csrrwi(&mut self, ir: &mut Context, a: &ArgsCsr) -> bool {
        require_cfg!(self, ext_zicsr);
        let old = match self.gen_csr_read(ir, a.csr) {
            Some(v) => v,
            None => {
                self.gen_priv_csr_exit(ir);
                return true;
            }
        };
        let zimm = ir.new_const(Type::I64, a.rs1 as u64);
        if !self.gen_csr_write(ir, a.csr, zimm) {
            self.gen_priv_csr_exit(ir);
            return true;
        }
        self.gen_set_gpr(ir, a.rd, old);
        true
    }

    fn trans_csrrsi(&mut self, ir: &mut Context, a: &ArgsCsr) -> bool {
        require_cfg!(self, ext_zicsr);
        let old = match self.gen_csr_read(ir, a.csr) {
            Some(v) => v,
            None => {
                self.gen_priv_csr_exit(ir);
                return true;
            }
        };
        if a.rs1 != 0 {
            let zimm = ir.new_const(Type::I64, a.rs1 as u64);
            let new = ir.new_temp(Type::I64);
            ir.gen_or(Type::I64, new, old, zimm);
            if !self.gen_csr_write(ir, a.csr, new) {
                self.gen_priv_csr_exit(ir);
                return true;
            }
        }
        self.gen_set_gpr(ir, a.rd, old);
        true
    }

    fn trans_csrrci(&mut self, ir: &mut Context, a: &ArgsCsr) -> bool {
        require_cfg!(self, ext_zicsr);
        let old = match self.gen_csr_read(ir, a.csr) {
            Some(v) => v,
            None => {
                self.gen_priv_csr_exit(ir);
                return true;
            }
        };
        if a.rs1 != 0 {
            let zimm = ir.new_const(Type::I64, a.rs1 as u64);
            let inv = ir.new_temp(Type::I64);
            ir.gen_not(Type::I64, inv, zimm);
            let new = ir.new_temp(Type::I64);
            ir.gen_and(Type::I64, new, old, inv);
            if !self.gen_csr_write(ir, a.csr, new) {
                self.gen_priv_csr_exit(ir);
                return true;
            }
        }
        self.gen_set_gpr(ir, a.rd, old);
        true
    }

    // ── RV32F/RV64F: FP Loads/Stores ──────────────────

    fn trans_flw(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_load(ir, a, MemOp::ul(), true)
    }
    fn trans_fsw(&mut self, ir: &mut Context, a: &ArgsS) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_store(ir, a, MemOp::ul(), true)
    }
    fn trans_fld(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_load(ir, a, MemOp::uq(), false)
    }
    fn trans_fsd(&mut self, ir: &mut Context, a: &ArgsS) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_store(ir, a, MemOp::uq(), false)
    }

    // ── RV32F: FMA ────────────────────────────────────

    fn trans_fmadd_s(&mut self, ir: &mut Context, a: &ArgsR4Rm) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let rs3 = self.fpr_load(ir, a.rs3);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fmadd_s as *const ()) as usize,
            &[self.env, rs1, rs2, rs3, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fmsub_s(&mut self, ir: &mut Context, a: &ArgsR4Rm) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let rs3 = self.fpr_load(ir, a.rs3);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fmsub_s as *const ()) as usize,
            &[self.env, rs1, rs2, rs3, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fnmsub_s(&mut self, ir: &mut Context, a: &ArgsR4Rm) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let rs3 = self.fpr_load(ir, a.rs3);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fnmsub_s as *const ()) as usize,
            &[self.env, rs1, rs2, rs3, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fnmadd_s(&mut self, ir: &mut Context, a: &ArgsR4Rm) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let rs3 = self.fpr_load(ir, a.rs3);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fnmadd_s as *const ()) as usize,
            &[self.env, rs1, rs2, rs3, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }

    // ── RV32F: Arithmetic ─────────────────────────────

    fn trans_fadd_s(&mut self, ir: &mut Context, a: &ArgsRRm) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fadd_s as *const ()) as usize,
            &[self.env, rs1, rs2, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fsub_s(&mut self, ir: &mut Context, a: &ArgsRRm) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fsub_s as *const ()) as usize,
            &[self.env, rs1, rs2, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fmul_s(&mut self, ir: &mut Context, a: &ArgsRRm) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fmul_s as *const ()) as usize,
            &[self.env, rs1, rs2, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fdiv_s(&mut self, ir: &mut Context, a: &ArgsRRm) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fdiv_s as *const ()) as usize,
            &[self.env, rs1, rs2, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fsqrt_s(&mut self, ir: &mut Context, a: &ArgsR2Rm) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fsqrt_s as *const ()) as usize,
            &[self.env, rs1, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }

    fn trans_fsgnj_s(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fsgnj_s as *const ()) as usize,
            &[self.env, rs1, rs2],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fsgnjn_s(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fsgnjn_s as *const ()) as usize,
            &[self.env, rs1, rs2],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fsgnjx_s(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fsgnjx_s as *const ()) as usize,
            &[self.env, rs1, rs2],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fmin_s(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fmin_s as *const ()) as usize,
            &[self.env, rs1, rs2],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fmax_s(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fmax_s as *const ()) as usize,
            &[self.env, rs1, rs2],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }

    fn trans_feq_s(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_feq_s as *const ()) as usize,
            &[self.env, rs1, rs2],
        );
        self.gen_set_gpr(ir, a.rd, res);
        true
    }
    fn trans_flt_s(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_flt_s as *const ()) as usize,
            &[self.env, rs1, rs2],
        );
        self.gen_set_gpr(ir, a.rd, res);
        true
    }
    fn trans_fle_s(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fle_s as *const ()) as usize,
            &[self.env, rs1, rs2],
        );
        self.gen_set_gpr(ir, a.rd, res);
        true
    }

    fn trans_fclass_s(&mut self, ir: &mut Context, a: &ArgsR2) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fclass_s as *const ()) as usize,
            &[self.env, rs1],
        );
        self.gen_set_gpr(ir, a.rd, res);
        true
    }

    // ── RV32F: Conversions ─────────────────────────────

    fn trans_fcvt_w_s(&mut self, ir: &mut Context, a: &ArgsR2Rm) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fcvt_w_s as *const ()) as usize,
            &[self.env, rs1, rm],
        );
        self.gen_set_gpr(ir, a.rd, res);
        true
    }
    fn trans_fcvt_wu_s(&mut self, ir: &mut Context, a: &ArgsR2Rm) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fcvt_wu_s as *const ()) as usize,
            &[self.env, rs1, rm],
        );
        self.gen_set_gpr(ir, a.rd, res);
        true
    }
    fn trans_fcvt_s_w(&mut self, ir: &mut Context, a: &ArgsR2Rm) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.gpr_or_zero(ir, a.rs1);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fcvt_s_w as *const ()) as usize,
            &[self.env, rs1, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fcvt_s_wu(&mut self, ir: &mut Context, a: &ArgsR2Rm) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.gpr_or_zero(ir, a.rs1);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fcvt_s_wu as *const ()) as usize,
            &[self.env, rs1, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }

    fn trans_fmv_x_w(&mut self, ir: &mut Context, a: &ArgsR2) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        let val = self.fpr_load(ir, a.rs1);
        let lo32 = ir.new_temp(Type::I32);
        ir.gen_extrl_i64_i32(lo32, val);
        self.gen_set_gpr_sx32(ir, a.rd, lo32);
        true
    }
    fn trans_fmv_w_x(&mut self, ir: &mut Context, a: &ArgsR2) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let src = self.gpr_or_zero(ir, a.rs1);
        let lo32 = ir.new_temp(Type::I32);
        ir.gen_extrl_i64_i32(lo32, src);
        let lo64 = ir.new_temp(Type::I64);
        ir.gen_ext_u32_i64(lo64, lo32);
        let mask = ir.new_const(Type::I64, 0xffff_ffff_0000_0000u64);
        let boxed = ir.new_temp(Type::I64);
        ir.gen_or(Type::I64, boxed, lo64, mask);
        self.fpr_store(ir, a.rd, boxed);
        true
    }

    // ── RV64F additions ───────────────────────────────

    fn trans_fcvt_l_s(&mut self, ir: &mut Context, a: &ArgsR2Rm) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fcvt_l_s as *const ()) as usize,
            &[self.env, rs1, rm],
        );
        self.gen_set_gpr(ir, a.rd, res);
        true
    }
    fn trans_fcvt_lu_s(&mut self, ir: &mut Context, a: &ArgsR2Rm) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fcvt_lu_s as *const ()) as usize,
            &[self.env, rs1, rm],
        );
        self.gen_set_gpr(ir, a.rd, res);
        true
    }
    fn trans_fcvt_s_l(&mut self, ir: &mut Context, a: &ArgsR2Rm) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.gpr_or_zero(ir, a.rs1);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fcvt_s_l as *const ()) as usize,
            &[self.env, rs1, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fcvt_s_lu(&mut self, ir: &mut Context, a: &ArgsR2Rm) -> bool {
        require_ext!(self, MisaExt::F);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.gpr_or_zero(ir, a.rs1);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fcvt_s_lu as *const ()) as usize,
            &[self.env, rs1, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }

    // ── RV32D/RV64D: FMA ──────────────────────────────

    fn trans_fmadd_d(&mut self, ir: &mut Context, a: &ArgsR4Rm) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let rs3 = self.fpr_load(ir, a.rs3);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fmadd_d as *const ()) as usize,
            &[self.env, rs1, rs2, rs3, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fmsub_d(&mut self, ir: &mut Context, a: &ArgsR4Rm) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let rs3 = self.fpr_load(ir, a.rs3);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fmsub_d as *const ()) as usize,
            &[self.env, rs1, rs2, rs3, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fnmsub_d(&mut self, ir: &mut Context, a: &ArgsR4Rm) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let rs3 = self.fpr_load(ir, a.rs3);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fnmsub_d as *const ()) as usize,
            &[self.env, rs1, rs2, rs3, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fnmadd_d(&mut self, ir: &mut Context, a: &ArgsR4Rm) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let rs3 = self.fpr_load(ir, a.rs3);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fnmadd_d as *const ()) as usize,
            &[self.env, rs1, rs2, rs3, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }

    // ── RV32D: Arithmetic ─────────────────────────────

    fn trans_fadd_d(&mut self, ir: &mut Context, a: &ArgsRRm) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fadd_d as *const ()) as usize,
            &[self.env, rs1, rs2, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fsub_d(&mut self, ir: &mut Context, a: &ArgsRRm) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fsub_d as *const ()) as usize,
            &[self.env, rs1, rs2, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fmul_d(&mut self, ir: &mut Context, a: &ArgsRRm) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fmul_d as *const ()) as usize,
            &[self.env, rs1, rs2, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fdiv_d(&mut self, ir: &mut Context, a: &ArgsRRm) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fdiv_d as *const ()) as usize,
            &[self.env, rs1, rs2, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fsqrt_d(&mut self, ir: &mut Context, a: &ArgsR2Rm) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fsqrt_d as *const ()) as usize,
            &[self.env, rs1, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }

    fn trans_fsgnj_d(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fsgnj_d as *const ()) as usize,
            &[self.env, rs1, rs2],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fsgnjn_d(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fsgnjn_d as *const ()) as usize,
            &[self.env, rs1, rs2],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fsgnjx_d(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fsgnjx_d as *const ()) as usize,
            &[self.env, rs1, rs2],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fmin_d(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fmin_d as *const ()) as usize,
            &[self.env, rs1, rs2],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fmax_d(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fmax_d as *const ()) as usize,
            &[self.env, rs1, rs2],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }

    fn trans_feq_d(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_feq_d as *const ()) as usize,
            &[self.env, rs1, rs2],
        );
        self.gen_set_gpr(ir, a.rd, res);
        true
    }
    fn trans_flt_d(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_flt_d as *const ()) as usize,
            &[self.env, rs1, rs2],
        );
        self.gen_set_gpr(ir, a.rd, res);
        true
    }
    fn trans_fle_d(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rs2 = self.fpr_load(ir, a.rs2);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fle_d as *const ()) as usize,
            &[self.env, rs1, rs2],
        );
        self.gen_set_gpr(ir, a.rd, res);
        true
    }

    fn trans_fclass_d(&mut self, ir: &mut Context, a: &ArgsR2) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fclass_d as *const ()) as usize,
            &[self.env, rs1],
        );
        self.gen_set_gpr(ir, a.rd, res);
        true
    }

    // ── RV32D: Conversions ─────────────────────────────

    fn trans_fcvt_s_d(&mut self, ir: &mut Context, a: &ArgsR2Rm) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fcvt_s_d as *const ()) as usize,
            &[self.env, rs1, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fcvt_d_s(&mut self, ir: &mut Context, a: &ArgsR2Rm) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fcvt_d_s as *const ()) as usize,
            &[self.env, rs1, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fcvt_w_d(&mut self, ir: &mut Context, a: &ArgsR2Rm) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fcvt_w_d as *const ()) as usize,
            &[self.env, rs1, rm],
        );
        self.gen_set_gpr(ir, a.rd, res);
        true
    }
    fn trans_fcvt_wu_d(&mut self, ir: &mut Context, a: &ArgsR2Rm) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fcvt_wu_d as *const ()) as usize,
            &[self.env, rs1, rm],
        );
        self.gen_set_gpr(ir, a.rd, res);
        true
    }
    fn trans_fcvt_d_w(&mut self, ir: &mut Context, a: &ArgsR2Rm) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.gpr_or_zero(ir, a.rs1);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fcvt_d_w as *const ()) as usize,
            &[self.env, rs1, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fcvt_d_wu(&mut self, ir: &mut Context, a: &ArgsR2Rm) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.gpr_or_zero(ir, a.rs1);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fcvt_d_wu as *const ()) as usize,
            &[self.env, rs1, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }

    // ── RV64D additions ───────────────────────────────

    fn trans_fcvt_l_d(&mut self, ir: &mut Context, a: &ArgsR2Rm) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fcvt_l_d as *const ()) as usize,
            &[self.env, rs1, rm],
        );
        self.gen_set_gpr(ir, a.rd, res);
        true
    }
    fn trans_fcvt_lu_d(&mut self, ir: &mut Context, a: &ArgsR2Rm) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        let rs1 = self.fpr_load(ir, a.rs1);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fcvt_lu_d as *const ()) as usize,
            &[self.env, rs1, rm],
        );
        self.gen_set_gpr(ir, a.rd, res);
        true
    }
    fn trans_fcvt_d_l(&mut self, ir: &mut Context, a: &ArgsR2Rm) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.gpr_or_zero(ir, a.rs1);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fcvt_d_l as *const ()) as usize,
            &[self.env, rs1, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }
    fn trans_fcvt_d_lu(&mut self, ir: &mut Context, a: &ArgsR2Rm) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let rs1 = self.gpr_or_zero(ir, a.rs1);
        let rm = ir.new_const(Type::I64, a.rm as u64);
        let res = self.gen_helper_call(
            ir,
            (fpu::helper_fcvt_d_lu as *const ()) as usize,
            &[self.env, rs1, rm],
        );
        self.fpr_store(ir, a.rd, res);
        true
    }

    fn trans_fmv_x_d(&mut self, ir: &mut Context, a: &ArgsR2) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        let val = self.fpr_load(ir, a.rs1);
        self.gen_set_gpr(ir, a.rd, val);
        true
    }
    fn trans_fmv_d_x(&mut self, ir: &mut Context, a: &ArgsR2) -> bool {
        require_ext!(self, MisaExt::D);
        self.gen_fp_check(ir);
        self.gen_set_fs_dirty(ir);
        let src = self.gpr_or_zero(ir, a.rs1);
        self.fpr_store(ir, a.rd, src);
        true
    }
}

// ── Decode16 trait implementation (RVC) ───────────────────────
//
// Most compressed instructions map directly to their 32-bit
// equivalents, so we delegate to the Decode impl.

impl Decode16<Context> for RiscvDisasContext {
    fn trans_illegal(&mut self, _ir: &mut Context, _a: &ArgsEmpty) -> bool {
        false
    }

    fn trans_c64_illegal(&mut self, _ir: &mut Context, _a: &ArgsEmpty) -> bool {
        false
    }

    fn trans_addi(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        <Self as Decode<Context>>::trans_addi(self, ir, a)
    }

    fn trans_lw(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        <Self as Decode<Context>>::trans_lw(self, ir, a)
    }

    fn trans_ld(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        <Self as Decode<Context>>::trans_ld(self, ir, a)
    }

    fn trans_c_fld(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        <Self as Decode<Context>>::trans_fld(self, ir, a)
    }

    fn trans_c_flw(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        <Self as Decode<Context>>::trans_flw(self, ir, a)
    }

    fn trans_sw(&mut self, ir: &mut Context, a: &ArgsS) -> bool {
        <Self as Decode<Context>>::trans_sw(self, ir, a)
    }

    fn trans_sd(&mut self, ir: &mut Context, a: &ArgsS) -> bool {
        <Self as Decode<Context>>::trans_sd(self, ir, a)
    }

    fn trans_c_fsd(&mut self, ir: &mut Context, a: &ArgsS) -> bool {
        <Self as Decode<Context>>::trans_fsd(self, ir, a)
    }

    fn trans_c_fsw(&mut self, ir: &mut Context, a: &ArgsS) -> bool {
        <Self as Decode<Context>>::trans_fsw(self, ir, a)
    }

    fn trans_lui(&mut self, ir: &mut Context, a: &ArgsU) -> bool {
        <Self as Decode<Context>>::trans_lui(self, ir, a)
    }

    fn trans_srli(&mut self, ir: &mut Context, a: &ArgsShift) -> bool {
        <Self as Decode<Context>>::trans_srli(self, ir, a)
    }

    fn trans_srai(&mut self, ir: &mut Context, a: &ArgsShift) -> bool {
        <Self as Decode<Context>>::trans_srai(self, ir, a)
    }

    fn trans_andi(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        <Self as Decode<Context>>::trans_andi(self, ir, a)
    }

    fn trans_sub(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        <Self as Decode<Context>>::trans_sub(self, ir, a)
    }

    fn trans_xor(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        <Self as Decode<Context>>::trans_xor(self, ir, a)
    }

    fn trans_or(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        <Self as Decode<Context>>::trans_or(self, ir, a)
    }

    fn trans_and(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        <Self as Decode<Context>>::trans_and(self, ir, a)
    }

    fn trans_jal(&mut self, ir: &mut Context, a: &ArgsJ) -> bool {
        <Self as Decode<Context>>::trans_jal(self, ir, a)
    }

    fn trans_beq(&mut self, ir: &mut Context, a: &ArgsB) -> bool {
        <Self as Decode<Context>>::trans_beq(self, ir, a)
    }

    fn trans_bne(&mut self, ir: &mut Context, a: &ArgsB) -> bool {
        <Self as Decode<Context>>::trans_bne(self, ir, a)
    }

    fn trans_addiw(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        <Self as Decode<Context>>::trans_addiw(self, ir, a)
    }

    fn trans_subw(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        <Self as Decode<Context>>::trans_subw(self, ir, a)
    }

    fn trans_addw(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        <Self as Decode<Context>>::trans_addw(self, ir, a)
    }

    fn trans_slli(&mut self, ir: &mut Context, a: &ArgsShift) -> bool {
        <Self as Decode<Context>>::trans_slli(self, ir, a)
    }

    fn trans_jalr(&mut self, ir: &mut Context, a: &ArgsI) -> bool {
        <Self as Decode<Context>>::trans_jalr(self, ir, a)
    }

    fn trans_ebreak(&mut self, ir: &mut Context, a: &ArgsEmpty) -> bool {
        <Self as Decode<Context>>::trans_ebreak(self, ir, a)
    }

    fn trans_add(&mut self, ir: &mut Context, a: &ArgsR) -> bool {
        <Self as Decode<Context>>::trans_add(self, ir, a)
    }
}
