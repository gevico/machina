use machina_core::types::Type;

use super::{run_riscv_tb, RiscvCpuState, RiscvCpuStateMem};

// ==========================================================
// Additional IR TB cases
// ==========================================================

riscv_bin_case!(test_add_case_small, gen_add, 1u64, 2u64, 3u64);
riscv_bin_case!(
    test_add_case_wrap,
    gen_add,
    0xFFFF_FFFF_FFFF_FFFFu64,
    1u64,
    0u64
);
riscv_bin_case!(
    test_add_case_large,
    gen_add,
    0x1234_5678_9ABC_DEF0u64,
    0x1111_1111_1111_1111u64,
    0x2345_6789_ABCD_F001u64
);
riscv_bin_case!(
    test_add_case_carry,
    gen_add,
    0xFFFF_FFFF_FFFF_F000u64,
    0x1000u64,
    0u64
);
riscv_bin_case!(
    test_add_case_mixed,
    gen_add,
    0x8000_0000_0000_0000u64,
    0x7FFF_FFFF_FFFF_FFFFu64,
    0xFFFF_FFFF_FFFF_FFFFu64
);

riscv_bin_case!(test_sub_case_small, gen_sub, 10u64, 3u64, 7u64);
riscv_bin_case!(
    test_sub_case_wrap,
    gen_sub,
    0u64,
    1u64,
    0xFFFF_FFFF_FFFF_FFFFu64
);
riscv_bin_case!(
    test_sub_case_large,
    gen_sub,
    0x1234_0000_0000_0000u64,
    0x22u64,
    0x1233_FFFF_FFFF_FFDEu64
);
riscv_bin_case!(
    test_sub_case_neg,
    gen_sub,
    0x8000_0000_0000_0000u64,
    1u64,
    0x7FFF_FFFF_FFFF_FFFFu64
);
riscv_bin_case!(
    test_sub_case_equal,
    gen_sub,
    0xDEAD_BEEF_DEAD_BEEFu64,
    0xDEAD_BEEF_DEAD_BEEFu64,
    0u64
);

riscv_bin_case!(
    test_and_case_basic,
    gen_and,
    0xF0F0u64,
    0x0FF0u64,
    0x00F0u64
);
riscv_bin_case!(test_and_case_zero, gen_and, 0x1234u64, 0u64, 0u64);
riscv_bin_case!(
    test_and_case_high,
    gen_and,
    0xFFFF_0000_0000_FFFFu64,
    0x0F0F_F0F0_00FF_FF00u64,
    0x0F0F_0000_0000_FF00u64
);

riscv_bin_case!(test_or_case_basic, gen_or, 0xF0u64, 0x0Fu64, 0xFFu64);
riscv_bin_case!(
    test_or_case_zero,
    gen_or,
    0u64,
    0x1234_5678u64,
    0x1234_5678u64
);
riscv_bin_case!(
    test_or_case_high,
    gen_or,
    0x8000_0000_0000_0000u64,
    0x1u64,
    0x8000_0000_0000_0001u64
);

riscv_bin_case!(
    test_xor_case_small,
    gen_xor,
    0xFF00u64,
    0x00FFu64,
    0xFFFFu64
);
riscv_bin_case!(test_xor_case_self, gen_xor, 0x1234u64, 0x1234u64, 0u64);
riscv_bin_case!(test_xor_case_alt, gen_xor, 0xAAAAu64, 0x5555u64, 0xFFFFu64);
riscv_bin_case!(
    test_xor_case_large,
    gen_xor,
    0xFFFF_0000_0000_FFFFu64,
    0x0000_FFFF_0000_FFFFu64,
    0xFFFF_FFFF_0000_0000u64
);
riscv_bin_case!(
    test_xor_case_sign,
    gen_xor,
    0x8000_0000_0000_0000u64,
    0xFFFF_FFFF_FFFF_FFFFu64,
    0x7FFF_FFFF_FFFF_FFFFu64
);

riscv_bin_case!(test_mul_case_small, gen_mul, 6u64, 7u64, 42u64);
riscv_bin_case!(test_mul_case_zero, gen_mul, 0u64, 0x1234u64, 0u64);
riscv_bin_case!(
    test_mul_case_wrap,
    gen_mul,
    0xFFFF_FFFF_FFFF_FFFFu64,
    2u64,
    0xFFFF_FFFF_FFFF_FFFEu64
);
riscv_bin_case!(
    test_mul_case_large,
    gen_mul,
    0x1000_0000u64,
    0x1000u64,
    0x100_0000_0000u64
);
riscv_bin_case!(
    test_mul_case_mixed,
    gen_mul,
    0x1_0000_0000u64,
    3u64,
    0x3_0000_0000u64
);

riscv_shift_case!(test_shl_case_1, gen_shl, 0x1u64, 4u64, 0x10u64);
riscv_shift_case!(
    test_shl_case_2,
    gen_shl,
    0x1u64,
    63u64,
    0x8000_0000_0000_0000u64
);
riscv_shift_case!(
    test_shl_case_3,
    gen_shl,
    0x8000_0000_0000_0000u64,
    1u64,
    0u64
);
riscv_shift_case!(test_shl_case_4, gen_shl, 0x1234u64, 0u64, 0x1234u64);

riscv_shift_case!(test_shr_case_1, gen_shr, 0x10u64, 4u64, 0x1u64);
riscv_shift_case!(
    test_shr_case_2,
    gen_shr,
    0x8000_0000_0000_0000u64,
    63u64,
    0x1u64
);
riscv_shift_case!(
    test_shr_case_3,
    gen_shr,
    0xFFFF_0000_0000_0000u64,
    16u64,
    0x0000_FFFF_0000_0000u64
);

riscv_shift_case!(
    test_sar_case_1,
    gen_sar,
    0xFFFF_FFFF_FFFF_F000u64,
    4u64,
    0xFFFF_FFFF_FFFF_FF00u64
);
riscv_shift_case!(
    test_sar_case_2,
    gen_sar,
    0x7FFF_FFFF_FFFF_FFFFu64,
    63u64,
    0u64
);
riscv_shift_case!(
    test_sar_case_3,
    gen_sar,
    0x8000_0000_0000_0000u64,
    63u64,
    0xFFFF_FFFF_FFFF_FFFFu64
);

riscv_setcond_case!(
    test_setcond_eq_case,
    machina_core::Cond::Eq,
    5u64,
    5u64,
    1u64
);
riscv_setcond_case!(
    test_setcond_ne_case,
    machina_core::Cond::Ne,
    5u64,
    6u64,
    1u64
);
riscv_setcond_case!(
    test_setcond_lt_case,
    machina_core::Cond::Lt,
    0xFFFF_FFFF_FFFF_FFFFu64,
    1u64,
    1u64
);
riscv_setcond_case!(
    test_setcond_ge_case,
    machina_core::Cond::Ge,
    5u64,
    0xFFFF_FFFF_FFFF_FFFFu64,
    1u64
);
riscv_setcond_case!(
    test_setcond_le_case,
    machina_core::Cond::Le,
    0xFFFF_FFFF_FFFF_FFFEu64,
    0xFFFF_FFFF_FFFF_FFFFu64,
    1u64
);
riscv_setcond_case!(
    test_setcond_gt_case,
    machina_core::Cond::Gt,
    2u64,
    1u64,
    1u64
);
riscv_setcond_case!(
    test_setcond_ltu_case,
    machina_core::Cond::Ltu,
    0xFFFF_FFFF_FFFF_FFFFu64,
    1u64,
    0u64
);
riscv_setcond_case!(
    test_setcond_geu_case,
    machina_core::Cond::Geu,
    0xFFFF_FFFF_FFFF_FFFFu64,
    1u64,
    1u64
);
riscv_setcond_case!(
    test_setcond_leu_case,
    machina_core::Cond::Leu,
    1u64,
    0xFFFF_FFFF_FFFF_FFFFu64,
    1u64
);
riscv_setcond_case!(
    test_setcond_gtu_case,
    machina_core::Cond::Gtu,
    2u64,
    3u64,
    0u64
);

riscv_branch_case!(
    test_bne_taken_extra,
    machina_core::Cond::Ne,
    10u64,
    11u64,
    1u64,
    2u64,
    1u64
);
riscv_branch_case!(
    test_bne_not_taken_extra,
    machina_core::Cond::Ne,
    10u64,
    10u64,
    1u64,
    2u64,
    2u64
);
riscv_branch_case!(
    test_blt_taken_extra,
    machina_core::Cond::Lt,
    0xFFFF_FFFF_FFFF_FFFEu64,
    1u64,
    3u64,
    4u64,
    3u64
);
riscv_branch_case!(
    test_bge_not_taken_extra,
    machina_core::Cond::Ge,
    1u64,
    2u64,
    3u64,
    4u64,
    4u64
);
riscv_branch_case!(
    test_bltu_taken_extra,
    machina_core::Cond::Ltu,
    1u64,
    2u64,
    5u64,
    6u64,
    5u64
);
riscv_branch_case!(
    test_bgeu_taken_extra,
    machina_core::Cond::Geu,
    0xFFFF_FFFF_FFFF_FFFFu64,
    1u64,
    7u64,
    8u64,
    7u64
);

riscv_mem_case!(test_mem_case_0, 0i64, 0x1111_1111_1111_1111u64);
riscv_mem_case!(test_mem_case_8, 8i64, 0x2222_2222_2222_2222u64);
riscv_mem_case!(test_mem_case_16, 16i64, 0x3333_3333_3333_3333u64);
riscv_mem_case!(test_mem_case_24, 24i64, 0x4444_4444_4444_4444u64);
riscv_mem_case!(test_mem_case_32, 32i64, 0x5555_5555_5555_5555u64);
riscv_mem_case!(test_mem_case_40, 40i64, 0x6666_6666_6666_6666u64);

#[test]
fn test_complex_addi_andi_slli() {
    let mut cpu = RiscvCpuState::new();
    cpu.regs[1] = 0x1234u64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let t_add = ctx.new_temp(Type::I64);
        let t_and = ctx.new_temp(Type::I64);
        let t_shl = ctx.new_temp(Type::I64);
        let imm_add = ctx.new_const(Type::I64, 0x100u64);
        let mask = ctx.new_const(Type::I64, 0xFFu64);
        let shamt = ctx.new_const(Type::I64, 4u64);

        ctx.gen_insn_start(0x5000);
        ctx.gen_add(Type::I64, t_add, regs[1], imm_add);
        ctx.gen_and(Type::I64, t_and, t_add, mask);
        ctx.gen_shl(Type::I64, t_shl, t_and, shamt);
        ctx.gen_mov(Type::I64, regs[5], t_shl);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[5], 0x340u64);
}

#[test]
fn test_complex_mul_add_xor() {
    let mut cpu = RiscvCpuState::new();
    cpu.regs[1] = 7u64;
    cpu.regs[2] = 9u64;
    cpu.regs[3] = 5u64;
    cpu.regs[4] = 0xFFu64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let t_mul = ctx.new_temp(Type::I64);
        let t_add = ctx.new_temp(Type::I64);
        let t_xor = ctx.new_temp(Type::I64);

        ctx.gen_insn_start(0x5010);
        ctx.gen_mul(Type::I64, t_mul, regs[1], regs[2]);
        ctx.gen_add(Type::I64, t_add, t_mul, regs[3]);
        ctx.gen_xor(Type::I64, t_xor, t_add, regs[4]);
        ctx.gen_mov(Type::I64, regs[6], t_xor);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[6], (7u64 * 9u64 + 5u64) ^ 0xFFu64);
}

#[test]
fn test_complex_slt_branch_select() {
    let mut cpu = RiscvCpuState::new();
    cpu.regs[1] = 0xFFFF_FFFF_FFFF_FFFEu64;
    cpu.regs[2] = 1u64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let label_taken = ctx.new_label();
        let label_end = ctx.new_label();
        let t_cond = ctx.new_temp(Type::I64);
        let c_yes = ctx.new_const(Type::I64, 0x11u64);
        let c_no = ctx.new_const(Type::I64, 0x22u64);
        let t_yes = ctx.new_temp(Type::I64);
        let t_no = ctx.new_temp(Type::I64);

        ctx.gen_insn_start(0x5020);
        ctx.gen_setcond(
            Type::I64,
            t_cond,
            regs[1],
            regs[2],
            machina_core::Cond::Lt,
        );
        ctx.gen_brcond(
            Type::I64,
            t_cond,
            regs[0],
            machina_core::Cond::Ne,
            label_taken,
        );

        ctx.gen_mov(Type::I64, t_no, c_no);
        ctx.gen_mov(Type::I64, regs[7], t_no);
        ctx.gen_br(label_end);

        ctx.gen_set_label(label_taken);
        ctx.gen_mov(Type::I64, t_yes, c_yes);
        ctx.gen_mov(Type::I64, regs[7], t_yes);

        ctx.gen_set_label(label_end);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[7], 0x11u64);
}

#[test]
fn test_complex_auipc_addi() {
    let mut cpu = RiscvCpuState::new();
    cpu.pc = 0x2000u64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, pc| {
        let imm20 = 0xABCDEu64;
        let imm = ctx.new_const(Type::I64, imm20 << 12);
        let addi = ctx.new_const(Type::I64, 0x123u64);
        let t_auipc = ctx.new_temp(Type::I64);
        let t_add = ctx.new_temp(Type::I64);

        ctx.gen_insn_start(0x5030);
        ctx.gen_add(Type::I64, t_auipc, pc, imm);
        ctx.gen_add(Type::I64, t_add, t_auipc, addi);
        ctx.gen_mov(Type::I64, regs[8], t_add);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(
        cpu.regs[8],
        cpu.pc.wrapping_add(0xABCDEu64 << 12).wrapping_add(0x123u64)
    );
}

#[test]
fn test_complex_bitfield_extract() {
    let mut cpu = RiscvCpuState::new();
    cpu.regs[1] = 0xF0F0_F0F0_1234_5678u64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let t_shr = ctx.new_temp(Type::I64);
        let t_and = ctx.new_temp(Type::I64);
        let shamt = ctx.new_const(Type::I64, 12u64);
        let mask = ctx.new_const(Type::I64, 0xFFFFu64);

        ctx.gen_insn_start(0x5040);
        ctx.gen_shr(Type::I64, t_shr, regs[1], shamt);
        ctx.gen_and(Type::I64, t_and, t_shr, mask);
        ctx.gen_mov(Type::I64, regs[9], t_and);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[9], 0x2345u64);
}

#[test]
fn test_complex_load_add_store() {
    let mut cpu = RiscvCpuStateMem::new();
    cpu.mem[0..8].copy_from_slice(&0x10u64.to_le_bytes());

    let exit_val = run_riscv_tb(&mut cpu, |ctx, env, regs, _pc| {
        let t_load = ctx.new_temp(Type::I64);
        let t_add = ctx.new_temp(Type::I64);
        let c_add = ctx.new_const(Type::I64, 0x20u64);
        let mem_offset = std::mem::offset_of!(RiscvCpuStateMem, mem) as i64;

        ctx.gen_insn_start(0x5050);
        ctx.gen_ld(Type::I64, t_load, env, mem_offset);
        ctx.gen_add(Type::I64, t_add, t_load, c_add);
        ctx.gen_st(Type::I64, t_add, env, mem_offset + 8);
        ctx.gen_mov(Type::I64, regs[10], t_add);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[10], 0x30u64);
    let stored = u64::from_le_bytes(cpu.mem[8..16].try_into().unwrap());
    assert_eq!(stored, 0x30u64);
}

#[test]
fn test_complex_shift_or() {
    let mut cpu = RiscvCpuState::new();
    cpu.regs[1] = 0x1u64;
    cpu.regs[2] = 8u64;
    cpu.regs[3] = 0xFF00u64;
    cpu.regs[4] = 4u64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let t_shl = ctx.new_temp(Type::I64);
        let t_shr = ctx.new_temp(Type::I64);
        let t_or = ctx.new_temp(Type::I64);

        ctx.gen_insn_start(0x5060);
        ctx.gen_shl(Type::I64, t_shl, regs[1], regs[2]);
        ctx.gen_shr(Type::I64, t_shr, regs[3], regs[4]);
        ctx.gen_or(Type::I64, t_or, t_shl, t_shr);
        ctx.gen_mov(Type::I64, regs[11], t_or);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[11], (0x1u64 << 8) | (0xFF00u64 >> 4));
}

#[test]
fn test_complex_xor_sub_and() {
    let mut cpu = RiscvCpuState::new();
    cpu.regs[1] = 0xAAAAu64;
    cpu.regs[2] = 0x5555u64;
    cpu.regs[3] = 0xFF00u64;
    cpu.regs[4] = 0x0F0Fu64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let t_xor = ctx.new_temp(Type::I64);
        let t_and = ctx.new_temp(Type::I64);
        let t_sub = ctx.new_temp(Type::I64);

        ctx.gen_insn_start(0x5070);
        ctx.gen_xor(Type::I64, t_xor, regs[1], regs[2]);
        ctx.gen_and(Type::I64, t_and, regs[3], regs[4]);
        ctx.gen_sub(Type::I64, t_sub, t_xor, t_and);
        ctx.gen_mov(Type::I64, regs[12], t_sub);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(
        cpu.regs[12],
        (0xAAAAu64 ^ 0x5555u64).wrapping_sub(0xFF00u64 & 0x0F0Fu64)
    );
}

#[test]
fn test_complex_branch_fallthrough() {
    let mut cpu = RiscvCpuState::new();
    cpu.regs[1] = 0x10u64;
    cpu.regs[2] = 0x10u64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let label_taken = ctx.new_label();
        let label_end = ctx.new_label();
        let c1 = ctx.new_const(Type::I64, 1u64);
        let c2 = ctx.new_const(Type::I64, 2u64);
        let t1 = ctx.new_temp(Type::I64);
        let t2 = ctx.new_temp(Type::I64);

        ctx.gen_insn_start(0x5080);
        ctx.gen_brcond(
            Type::I64,
            regs[1],
            regs[2],
            machina_core::Cond::Ne,
            label_taken,
        );
        ctx.gen_mov(Type::I64, t1, c1);
        ctx.gen_mov(Type::I64, regs[13], t1);
        ctx.gen_br(label_end);

        ctx.gen_set_label(label_taken);
        ctx.gen_mov(Type::I64, t2, c2);
        ctx.gen_mov(Type::I64, regs[13], t2);

        ctx.gen_set_label(label_end);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[13], 1u64);
}

#[test]
fn test_complex_pc_relative_mask() {
    let mut cpu = RiscvCpuState::new();
    cpu.pc = 0x8000u64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, pc| {
        let imm = ctx.new_const(Type::I64, 0x100u64);
        let mask = ctx.new_const(Type::I64, 0xFFFu64);
        let t_add = ctx.new_temp(Type::I64);
        let t_and = ctx.new_temp(Type::I64);

        ctx.gen_insn_start(0x5090);
        ctx.gen_add(Type::I64, t_add, pc, imm);
        ctx.gen_and(Type::I64, t_and, t_add, mask);
        ctx.gen_mov(Type::I64, regs[14], t_and);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[14], (cpu.pc + 0x100u64) & 0xFFFu64);
}

#[test]
fn test_neg_basic() {
    let mut cpu = RiscvCpuState::new();
    cpu.regs[1] = 0x1234u64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let t_neg = ctx.new_temp(Type::I64);
        ctx.gen_insn_start(0x5100);
        ctx.gen_neg(Type::I64, t_neg, regs[1]);
        ctx.gen_mov(Type::I64, regs[15], t_neg);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[15], 0u64.wrapping_sub(0x1234u64));
}

#[test]
fn test_not_basic() {
    let mut cpu = RiscvCpuState::new();
    cpu.regs[1] = 0x00FF_00FF_00FF_00FFu64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let t_not = ctx.new_temp(Type::I64);
        ctx.gen_insn_start(0x5110);
        ctx.gen_not(Type::I64, t_not, regs[1]);
        ctx.gen_mov(Type::I64, regs[16], t_not);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[16], !0x00FF_00FF_00FF_00FFu64);
}

#[test]
fn test_mov_chain() {
    let mut cpu = RiscvCpuState::new();
    cpu.regs[1] = 0xA5A5_5AA5_A5A5_5AA5u64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        ctx.gen_insn_start(0x5120);
        ctx.gen_mov(Type::I64, regs[2], regs[1]);
        ctx.gen_mov(Type::I64, regs[3], regs[2]);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[2], cpu.regs[1]);
    assert_eq!(cpu.regs[3], cpu.regs[1]);
}

#[test]
fn test_brcond_on_temp_eq() {
    let mut cpu = RiscvCpuState::new();
    cpu.regs[1] = 10u64;
    cpu.regs[2] = 20u64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let label_eq = ctx.new_label();
        let label_end = ctx.new_label();
        let t_add = ctx.new_temp(Type::I64);
        let c30 = ctx.new_const(Type::I64, 30u64);
        let c1 = ctx.new_const(Type::I64, 1u64);
        let c0 = ctx.new_const(Type::I64, 0u64);
        let t_out = ctx.new_temp(Type::I64);

        ctx.gen_insn_start(0x5130);
        ctx.gen_add(Type::I64, t_add, regs[1], regs[2]);
        ctx.gen_brcond(Type::I64, t_add, c30, machina_core::Cond::Eq, label_eq);
        ctx.gen_mov(Type::I64, t_out, c0);
        ctx.gen_mov(Type::I64, regs[4], t_out);
        ctx.gen_br(label_end);

        ctx.gen_set_label(label_eq);
        ctx.gen_mov(Type::I64, t_out, c1);
        ctx.gen_mov(Type::I64, regs[4], t_out);
        ctx.gen_set_label(label_end);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[4], 1u64);
}

#[test]
fn test_countdown_loop_sum() {
    let mut cpu = RiscvCpuState::new();
    cpu.regs[1] = 4u64;
    cpu.regs[2] = 0u64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let label_loop = ctx.new_label();
        let c1 = ctx.new_const(Type::I64, 1u64);
        let c0 = ctx.new_const(Type::I64, 0u64);
        let t_sum = ctx.new_temp(Type::I64);
        let t_cnt = ctx.new_temp(Type::I64);

        ctx.gen_insn_start(0x5140);
        ctx.gen_set_label(label_loop);
        ctx.gen_add(Type::I64, t_sum, regs[2], regs[1]);
        ctx.gen_mov(Type::I64, regs[2], t_sum);
        ctx.gen_sub(Type::I64, t_cnt, regs[1], c1);
        ctx.gen_mov(Type::I64, regs[1], t_cnt);
        ctx.gen_brcond(
            Type::I64,
            regs[1],
            c0,
            machina_core::Cond::Ne,
            label_loop,
        );
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[2], 10u64);
    assert_eq!(cpu.regs[1], 0u64);
}

#[test]
fn test_mem_store_overwrite() {
    let mut cpu = RiscvCpuStateMem::new();
    let exit_val = run_riscv_tb(&mut cpu, |ctx, env, regs, _pc| {
        let v1 = ctx.new_const(Type::I64, 0x1111_2222_3333_4444u64);
        let v2 = ctx.new_const(Type::I64, 0xAAAA_BBBB_CCCC_DDDDu64);
        let t1 = ctx.new_temp(Type::I64);
        let t2 = ctx.new_temp(Type::I64);
        let t_load = ctx.new_temp(Type::I64);
        let mem_offset =
            std::mem::offset_of!(RiscvCpuStateMem, mem) as i64 + 16;

        ctx.gen_insn_start(0x5150);
        ctx.gen_mov(Type::I64, t1, v1);
        ctx.gen_st(Type::I64, t1, env, mem_offset);
        ctx.gen_mov(Type::I64, t2, v2);
        ctx.gen_st(Type::I64, t2, env, mem_offset);
        ctx.gen_ld(Type::I64, t_load, env, mem_offset);
        ctx.gen_mov(Type::I64, regs[1], t_load);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[1], 0xAAAA_BBBB_CCCC_DDDDu64);
    let stored = u64::from_le_bytes(cpu.mem[16..24].try_into().unwrap());
    assert_eq!(stored, 0xAAAA_BBBB_CCCC_DDDDu64);
}

#[test]
fn test_mem_load_add_sum() {
    let mut cpu = RiscvCpuStateMem::new();
    cpu.mem[0..8].copy_from_slice(&0x10u64.to_le_bytes());
    cpu.mem[8..16].copy_from_slice(&0x20u64.to_le_bytes());

    let exit_val = run_riscv_tb(&mut cpu, |ctx, env, regs, _pc| {
        let t0 = ctx.new_temp(Type::I64);
        let t1 = ctx.new_temp(Type::I64);
        let t_sum = ctx.new_temp(Type::I64);
        let mem_offset = std::mem::offset_of!(RiscvCpuStateMem, mem) as i64;

        ctx.gen_insn_start(0x5160);
        ctx.gen_ld(Type::I64, t0, env, mem_offset);
        ctx.gen_ld(Type::I64, t1, env, mem_offset + 8);
        ctx.gen_add(Type::I64, t_sum, t0, t1);
        ctx.gen_mov(Type::I64, regs[2], t_sum);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[2], 0x30u64);
}

#[test]
fn test_shift_count_computed() {
    let mut cpu = RiscvCpuState::new();
    cpu.regs[1] = 0x2u64;
    cpu.regs[2] = 3u64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let t_cnt = ctx.new_temp(Type::I64);
        let t_out = ctx.new_temp(Type::I64);
        let c1 = ctx.new_const(Type::I64, 1u64);

        ctx.gen_insn_start(0x5170);
        ctx.gen_add(Type::I64, t_cnt, regs[2], c1);
        ctx.gen_shl(Type::I64, t_out, regs[1], t_cnt);
        ctx.gen_mov(Type::I64, regs[5], t_out);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[5], 0x2u64 << 4);
}

#[test]
fn test_mul_sub_mix() {
    let mut cpu = RiscvCpuState::new();
    cpu.regs[1] = 9u64;
    cpu.regs[2] = 7u64;
    cpu.regs[3] = 10u64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let t_mul = ctx.new_temp(Type::I64);
        let t_sub = ctx.new_temp(Type::I64);

        ctx.gen_insn_start(0x5180);
        ctx.gen_mul(Type::I64, t_mul, regs[1], regs[2]);
        ctx.gen_sub(Type::I64, t_sub, t_mul, regs[3]);
        ctx.gen_mov(Type::I64, regs[6], t_sub);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[6], (9u64 * 7u64).wrapping_sub(10u64));
}
