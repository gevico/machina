use machina_backend::code_buffer::CodeBuffer;
use machina_backend::translate::translate_and_execute;
use machina_backend::HostCodeGen;
use machina_backend::X86_64CodeGen;
use machina_core::types::Type;
use machina_core::{Context, Op, Opcode};

use super::{
    run_riscv_tb, setup_riscv_globals, split_i128, split_u128, RiscvCpuState,
    RiscvCpuStateMem,
};

#[test]
fn test_exec_alu_shift_cond_mov() {
    let mut cpu = RiscvCpuState::new();
    let a = 0x1234_5678_9ABC_DEF0u64;
    let b = 0x0F0F_0F0F_0F0F_0F0Fu64;
    let sar_val = 0x8000_0000_0000_0000u64;
    let sc_a = 0xFFFF_FFFF_FFFF_FFFFu64;
    let sc_b = 1u64;
    let shift = 5u64;

    cpu.regs[1] = a;
    cpu.regs[2] = b;
    cpu.regs[3] = sar_val;
    cpu.regs[4] = sc_a;
    cpu.regs[5] = sc_b;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let t_mov = ctx.new_temp(Type::I64);
        let t_add = ctx.new_temp(Type::I64);
        let t_sub = ctx.new_temp(Type::I64);
        let t_mul = ctx.new_temp(Type::I64);
        let t_and = ctx.new_temp(Type::I64);
        let t_or = ctx.new_temp(Type::I64);
        let t_xor = ctx.new_temp(Type::I64);
        let t_neg = ctx.new_temp(Type::I64);
        let t_not = ctx.new_temp(Type::I64);
        let t_shl = ctx.new_temp(Type::I64);
        let t_shr = ctx.new_temp(Type::I64);
        let t_sar = ctx.new_temp(Type::I64);
        let t_sc = ctx.new_temp(Type::I64);
        let c_shift = ctx.new_const(Type::I64, shift);

        ctx.gen_insn_start(0x5000);
        ctx.gen_mov(Type::I64, t_mov, regs[1]);
        ctx.gen_mov(Type::I64, regs[10], t_mov);

        ctx.gen_add(Type::I64, t_add, regs[1], regs[2]);
        ctx.gen_mov(Type::I64, regs[11], t_add);

        ctx.gen_sub(Type::I64, t_sub, regs[1], regs[2]);
        ctx.gen_mov(Type::I64, regs[12], t_sub);

        ctx.gen_mul(Type::I64, t_mul, regs[1], regs[2]);
        ctx.gen_mov(Type::I64, regs[13], t_mul);

        ctx.gen_and(Type::I64, t_and, regs[1], regs[2]);
        ctx.gen_mov(Type::I64, regs[14], t_and);

        ctx.gen_or(Type::I64, t_or, regs[1], regs[2]);
        ctx.gen_mov(Type::I64, regs[15], t_or);

        ctx.gen_xor(Type::I64, t_xor, regs[1], regs[2]);
        ctx.gen_mov(Type::I64, regs[16], t_xor);

        ctx.gen_neg(Type::I64, t_neg, regs[1]);
        ctx.gen_mov(Type::I64, regs[17], t_neg);

        ctx.gen_not(Type::I64, t_not, regs[1]);
        ctx.gen_mov(Type::I64, regs[18], t_not);

        ctx.gen_shl(Type::I64, t_shl, regs[1], c_shift);
        ctx.gen_mov(Type::I64, regs[19], t_shl);

        ctx.gen_shr(Type::I64, t_shr, regs[1], c_shift);
        ctx.gen_mov(Type::I64, regs[20], t_shr);

        ctx.gen_sar(Type::I64, t_sar, regs[3], c_shift);
        ctx.gen_mov(Type::I64, regs[21], t_sar);

        ctx.gen_setcond(
            Type::I64,
            t_sc,
            regs[4],
            regs[5],
            machina_core::Cond::Lt,
        );
        ctx.gen_mov(Type::I64, regs[22], t_sc);

        ctx.gen_exit_tb(0);
    });

    let sh = (shift & 63) as u32;
    let expected_shl = a.wrapping_shl(sh);
    let expected_shr = a >> sh;
    let expected_sar = ((sar_val as i64) >> sh) as u64;
    let expected_sc = if (sc_a as i64) < (sc_b as i64) { 1 } else { 0 };

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[10], a);
    assert_eq!(cpu.regs[11], a.wrapping_add(b));
    assert_eq!(cpu.regs[12], a.wrapping_sub(b));
    assert_eq!(cpu.regs[13], a.wrapping_mul(b));
    assert_eq!(cpu.regs[14], a & b);
    assert_eq!(cpu.regs[15], a | b);
    assert_eq!(cpu.regs[16], a ^ b);
    assert_eq!(cpu.regs[17], 0u64.wrapping_sub(a));
    assert_eq!(cpu.regs[18], !a);
    assert_eq!(cpu.regs[19], expected_shl);
    assert_eq!(cpu.regs[20], expected_shr);
    assert_eq!(cpu.regs[21], expected_sar);
    assert_eq!(cpu.regs[22], expected_sc);
}

#[test]
fn test_exec_mem_and_ext() {
    let mut cpu = RiscvCpuStateMem::new();
    cpu.mem[0] = 0x80;
    cpu.mem[2..4].copy_from_slice(&0x1234u16.to_le_bytes());
    cpu.mem[4..6].copy_from_slice(&0x8000u16.to_le_bytes());
    cpu.mem[8..12].copy_from_slice(&0x1234_5678u32.to_le_bytes());
    cpu.mem[12..16].copy_from_slice(&0x8000_0000u32.to_le_bytes());
    cpu.mem[16..24].copy_from_slice(&0x0123_4567_89AB_CDEFu64.to_le_bytes());

    let exit_val = run_riscv_tb(&mut cpu, |ctx, env, regs, _pc| {
        let mem_offset = std::mem::offset_of!(RiscvCpuStateMem, mem) as i64;
        let t_ld8u = ctx.new_temp(Type::I64);
        let t_ld8s = ctx.new_temp(Type::I64);
        let t_ld16u = ctx.new_temp(Type::I64);
        let t_ld16s = ctx.new_temp(Type::I64);
        let t_ld32u = ctx.new_temp(Type::I64);
        let t_ld32s = ctx.new_temp(Type::I64);
        let t_ld = ctx.new_temp(Type::I64);

        let c_st64 = ctx.new_const(Type::I64, 0xDEAD_BEEF_DEAD_BEEFu64);
        let c_st32 = ctx.new_const(Type::I64, 0xAABB_CCDDu64);
        let c_st16 = ctx.new_const(Type::I64, 0xEEFFu64);
        let c_st8 = ctx.new_const(Type::I64, 0x11u64);

        let c_i32_neg = ctx.new_const(Type::I32, 0xFFFF_FF80u64);
        let c_u32 = ctx.new_const(Type::I32, 0xFFFF_FFFFu64);
        let c_i64 = ctx.new_const(Type::I64, 0x1234_5678_9ABC_DEF0u64);
        let t_ext_s = ctx.new_temp(Type::I64);
        let t_ext_u = ctx.new_temp(Type::I64);
        let t_extrl = ctx.new_temp(Type::I32);

        ctx.gen_insn_start(0x5100);

        ctx.gen_ld8u(Type::I64, t_ld8u, env, mem_offset + 0);
        ctx.gen_mov(Type::I64, regs[10], t_ld8u);
        ctx.gen_ld8s(Type::I64, t_ld8s, env, mem_offset + 0);
        ctx.gen_mov(Type::I64, regs[11], t_ld8s);

        ctx.gen_ld16u(Type::I64, t_ld16u, env, mem_offset + 2);
        ctx.gen_mov(Type::I64, regs[12], t_ld16u);
        ctx.gen_ld16s(Type::I64, t_ld16s, env, mem_offset + 4);
        ctx.gen_mov(Type::I64, regs[13], t_ld16s);

        ctx.gen_ld32u(Type::I64, t_ld32u, env, mem_offset + 8);
        ctx.gen_mov(Type::I64, regs[14], t_ld32u);
        ctx.gen_ld32s(Type::I64, t_ld32s, env, mem_offset + 12);
        ctx.gen_mov(Type::I64, regs[15], t_ld32s);

        ctx.gen_ld(Type::I64, t_ld, env, mem_offset + 16);
        ctx.gen_mov(Type::I64, regs[16], t_ld);

        ctx.gen_st(Type::I64, c_st64, env, mem_offset + 32);
        ctx.gen_st32(Type::I64, c_st32, env, mem_offset + 40);
        ctx.gen_st16(Type::I64, c_st16, env, mem_offset + 44);
        ctx.gen_st8(Type::I64, c_st8, env, mem_offset + 46);

        ctx.gen_ext_i32_i64(t_ext_s, c_i32_neg);
        ctx.gen_mov(Type::I64, regs[20], t_ext_s);
        ctx.gen_ext_u32_i64(t_ext_u, c_u32);
        ctx.gen_mov(Type::I64, regs[21], t_ext_u);

        ctx.gen_extrl_i64_i32(t_extrl, c_i64);
        ctx.gen_st32(Type::I32, t_extrl, env, mem_offset + 48);

        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[10], 0x80);
    assert_eq!(cpu.regs[11], 0xFFFF_FFFF_FFFF_FF80u64);
    assert_eq!(cpu.regs[12], 0x1234);
    assert_eq!(cpu.regs[13], 0xFFFF_FFFF_FFFF_8000u64);
    assert_eq!(cpu.regs[14], 0x1234_5678);
    assert_eq!(cpu.regs[15], 0xFFFF_FFFF_8000_0000u64);
    assert_eq!(cpu.regs[16], 0x0123_4567_89AB_CDEFu64);
    assert_eq!(cpu.regs[20], 0xFFFF_FFFF_FFFF_FF80u64);
    assert_eq!(cpu.regs[21], 0x0000_0000_FFFF_FFFFu64);

    let mem = &cpu.mem;
    assert_eq!(
        u64::from_le_bytes(mem[32..40].try_into().unwrap()),
        0xDEAD_BEEF_DEAD_BEEFu64
    );
    assert_eq!(
        u32::from_le_bytes(mem[40..44].try_into().unwrap()),
        0xAABB_CCDDu32
    );
    assert_eq!(
        u16::from_le_bytes(mem[44..46].try_into().unwrap()),
        0xEEFFu16
    );
    assert_eq!(mem[46], 0x11u8);
    assert_eq!(
        u32::from_le_bytes(mem[48..52].try_into().unwrap()),
        0x9ABC_DEF0u32
    );
}

#[test]
fn test_exec_control_flow_ops() {
    let mut cpu = RiscvCpuState::new();
    cpu.regs[1] = 1;
    cpu.regs[2] = 2;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let c1 = ctx.new_const(Type::I64, 1);
        let c2 = ctx.new_const(Type::I64, 2);
        let label_br = ctx.new_label();
        let label_taken = ctx.new_label();
        let label_end = ctx.new_label();

        ctx.gen_insn_start(0x5200);
        let nop = Op::with_args(ctx.next_op_idx(), Opcode::Nop, Type::I64, &[]);
        ctx.emit_op(nop);

        ctx.gen_br(label_br);
        ctx.gen_mov(Type::I64, regs[10], c2);
        ctx.gen_set_label(label_br);
        ctx.gen_mov(Type::I64, regs[10], c1);

        ctx.gen_brcond(
            Type::I64,
            regs[1],
            regs[2],
            machina_core::Cond::Lt,
            label_taken,
        );
        ctx.gen_mov(Type::I64, regs[11], c2);
        ctx.gen_br(label_end);
        ctx.gen_set_label(label_taken);
        ctx.gen_mov(Type::I64, regs[11], c1);
        ctx.gen_set_label(label_end);

        ctx.gen_goto_tb(0);

        ctx.gen_exit_tb(0x1234);
    });

    assert_eq!(exit_val, 0x1234);
    assert_eq!(cpu.regs[10], 1);
    assert_eq!(cpu.regs[11], 1);
}

#[test]
fn test_exec_rotate_and_bitfield_ops() {
    let mut cpu = RiscvCpuState::new();
    let a = 0x0123_4567_89AB_CDEFu64;
    let shift = 8u64;
    let sext_val = 0x0000_0000_0000_8001u64;
    let dep_a = 0x1122_3344_5566_7788u64;
    let dep_b = 0xAAu64;
    let dep_b16 = 0xBEEF_u64;
    let ex2_al = 0x1122_3344_5566_7788u64;
    let ex2_ah = 0x99AA_BBCC_DDEE_FF00u64;
    let ex2_shift = 8u32;
    let ex32_val = 0xFFFF_FFFF_1234_5678u64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let c_a = ctx.new_const(Type::I64, a);
        let c_shift = ctx.new_const(Type::I64, shift);
        let c_sext = ctx.new_const(Type::I64, sext_val);
        let c_dep_a = ctx.new_const(Type::I64, dep_a);
        let c_dep_b = ctx.new_const(Type::I64, dep_b);
        let c_dep_b16 = ctx.new_const(Type::I64, dep_b16);
        let c_ex2_al = ctx.new_const(Type::I64, ex2_al);
        let c_ex2_ah = ctx.new_const(Type::I64, ex2_ah);
        let c_ex32 = ctx.new_const(Type::I64, ex32_val);

        let t_rotl = ctx.new_temp(Type::I64);
        let t_rotr = ctx.new_temp(Type::I64);
        let t_extract8 = ctx.new_temp(Type::I64);
        let t_extract32 = ctx.new_temp(Type::I64);
        let t_sextract16 = ctx.new_temp(Type::I64);
        let t_deposit8 = ctx.new_temp(Type::I64);
        let t_deposit16 = ctx.new_temp(Type::I64);
        let t_extract2 = ctx.new_temp(Type::I64);

        ctx.gen_insn_start(0x5300);
        ctx.gen_rotl(Type::I64, t_rotl, c_a, c_shift);
        ctx.gen_rotr(Type::I64, t_rotr, c_a, c_shift);
        ctx.gen_extract(Type::I64, t_extract8, c_a, 0, 8);
        ctx.gen_extract(Type::I64, t_extract32, c_ex32, 0, 32);
        ctx.gen_sextract(Type::I64, t_sextract16, c_sext, 0, 16);
        ctx.gen_deposit(Type::I64, t_deposit8, c_dep_a, c_dep_b, 0, 8);
        ctx.gen_deposit(Type::I64, t_deposit16, c_dep_a, c_dep_b16, 0, 16);
        ctx.gen_extract2(Type::I64, t_extract2, c_ex2_al, c_ex2_ah, ex2_shift);

        ctx.gen_mov(Type::I64, regs[10], t_rotl);
        ctx.gen_mov(Type::I64, regs[11], t_rotr);
        ctx.gen_mov(Type::I64, regs[12], t_extract8);
        ctx.gen_mov(Type::I64, regs[13], t_extract32);
        ctx.gen_mov(Type::I64, regs[14], t_sextract16);
        ctx.gen_mov(Type::I64, regs[15], t_deposit8);
        ctx.gen_mov(Type::I64, regs[16], t_deposit16);
        ctx.gen_mov(Type::I64, regs[17], t_extract2);
        ctx.gen_exit_tb(0);
    });

    let expected_rotl = a.rotate_left(shift as u32);
    let expected_rotr = a.rotate_right(shift as u32);
    let expected_extract8 = a & 0xFF;
    let expected_extract32 = ex32_val & 0xFFFF_FFFF;
    let expected_sextract16 = (sext_val as i16) as i64 as u64;
    let expected_deposit8 = (dep_a & !0xFF) | (dep_b & 0xFF);
    let expected_deposit16 = (dep_a & !0xFFFF) | (dep_b16 & 0xFFFF);
    let expected_extract2 =
        (ex2_al >> ex2_shift) | (ex2_ah << (64 - ex2_shift));

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[10], expected_rotl);
    assert_eq!(cpu.regs[11], expected_rotr);
    assert_eq!(cpu.regs[12], expected_extract8);
    assert_eq!(cpu.regs[13], expected_extract32);
    assert_eq!(cpu.regs[14], expected_sextract16);
    assert_eq!(cpu.regs[15], expected_deposit8);
    assert_eq!(cpu.regs[16], expected_deposit16);
    assert_eq!(cpu.regs[17], expected_extract2);
}

#[test]
fn test_exec_andc() {
    if !std::is_x86_feature_detected!("bmi1") {
        return;
    }
    let mut cpu = RiscvCpuState::new();
    let a = 0xFF00_FF00_FF00_FF00u64;
    let b = 0x0F0F_0F0F_0F0F_0F0Fu64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let c_a = ctx.new_const(Type::I64, a);
        let c_b = ctx.new_const(Type::I64, b);
        let t_andc = ctx.new_temp(Type::I64);

        ctx.gen_insn_start(0x5310);
        ctx.gen_andc(Type::I64, t_andc, c_a, c_b);
        ctx.gen_mov(Type::I64, regs[10], t_andc);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[10], a & !b);
}

#[test]
fn test_exec_bswap_ops() {
    let mut cpu = RiscvCpuState::new();
    let v16 = 0xA1B2u64;
    let v32 = 0x8000_00FFu64;
    let v64 = 0x0102_0304_0506_0708u64;
    let flags_os = 4u32;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let c_v16 = ctx.new_const(Type::I64, v16);
        let c_v32 = ctx.new_const(Type::I64, v32);
        let c_v64 = ctx.new_const(Type::I64, v64);
        let t_bswap16 = ctx.new_temp(Type::I64);
        let t_bswap32 = ctx.new_temp(Type::I64);
        let t_bswap64 = ctx.new_temp(Type::I64);

        ctx.gen_insn_start(0x5320);
        ctx.gen_bswap16(Type::I64, t_bswap16, c_v16, 0);
        ctx.gen_bswap32(Type::I64, t_bswap32, c_v32, flags_os);
        ctx.gen_bswap64(Type::I64, t_bswap64, c_v64, 0);
        ctx.gen_mov(Type::I64, regs[10], t_bswap16);
        ctx.gen_mov(Type::I64, regs[11], t_bswap32);
        ctx.gen_mov(Type::I64, regs[12], t_bswap64);
        ctx.gen_exit_tb(0);
    });

    let expected_bswap16 = 0xB2A1u64;
    let expected_bswap32 = 0xFFFF_FFFF_FF00_0080u64;
    let expected_bswap64 = 0x0807_0605_0403_0201u64;

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[10], expected_bswap16);
    assert_eq!(cpu.regs[11], expected_bswap32);
    assert_eq!(cpu.regs[12], expected_bswap64);
}

#[test]
fn test_exec_clz_ctz_ctpop() {
    if !std::is_x86_feature_detected!("lzcnt")
        || !std::is_x86_feature_detected!("bmi1")
        || !std::is_x86_feature_detected!("popcnt")
    {
        return;
    }

    let mut cpu = RiscvCpuState::new();
    let val_clz = 0x0010_0000_0000_0000u64;
    let val_ctz = 0x0000_0000_0000_0100u64;
    let val_pop = 0xF0F0_F00F_0001u64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let c_clz = ctx.new_const(Type::I64, val_clz);
        let c_ctz = ctx.new_const(Type::I64, val_ctz);
        let c_pop = ctx.new_const(Type::I64, val_pop);
        let c_fallback = ctx.new_const(Type::I64, 0x1234);
        let t_clz = ctx.new_temp(Type::I64);
        let t_ctz = ctx.new_temp(Type::I64);
        let t_pop = ctx.new_temp(Type::I64);

        ctx.gen_insn_start(0x5330);
        ctx.gen_clz(Type::I64, t_clz, c_clz, c_fallback);
        ctx.gen_ctz(Type::I64, t_ctz, c_ctz, c_fallback);
        ctx.gen_ctpop(Type::I64, t_pop, c_pop);
        ctx.gen_mov(Type::I64, regs[10], t_clz);
        ctx.gen_mov(Type::I64, regs[11], t_ctz);
        ctx.gen_mov(Type::I64, regs[12], t_pop);
        ctx.gen_exit_tb(0);
    });

    let expected_clz = val_clz.leading_zeros() as u64;
    let expected_ctz = val_ctz.trailing_zeros() as u64;
    let expected_pop = val_pop.count_ones() as u64;

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[10], expected_clz);
    assert_eq!(cpu.regs[11], expected_ctz);
    assert_eq!(cpu.regs[12], expected_pop);
}

#[test]
fn test_exec_muls2() {
    let mut cpu = RiscvCpuState::new();
    let a_s: i64 = -3;
    let b_s: i64 = 5;

    let prod_s = (a_s as i128) * (b_s as i128);
    let (muls_lo, muls_hi) = split_i128(prod_s);

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let c_a_s = ctx.new_const(Type::I64, a_s as u64);
        let c_b_s = ctx.new_const(Type::I64, b_s as u64);
        let t_muls_lo = ctx.new_temp(Type::I64);
        let t_muls_hi = ctx.new_temp(Type::I64);

        ctx.gen_insn_start(0x5340);
        ctx.gen_muls2(Type::I64, t_muls_lo, t_muls_hi, c_a_s, c_b_s);
        ctx.gen_mov(Type::I64, regs[10], t_muls_lo);
        ctx.gen_mov(Type::I64, regs[11], t_muls_hi);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[10], muls_lo);
    assert_eq!(cpu.regs[11], muls_hi);
}

#[test]
fn test_exec_mulu2() {
    let mut cpu = RiscvCpuState::new();
    let a_u: u64 = 0x1_0000_0000;
    let b_u: u64 = 3;

    let prod_u = (a_u as u128) * (b_u as u128);
    let (mulu_lo, mulu_hi) = split_u128(prod_u);

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let c_a_u = ctx.new_const(Type::I64, a_u);
        let c_b_u = ctx.new_const(Type::I64, b_u);
        let t_mulu_lo = ctx.new_temp(Type::I64);
        let t_mulu_hi = ctx.new_temp(Type::I64);

        ctx.gen_insn_start(0x5341);
        ctx.gen_mulu2(Type::I64, t_mulu_lo, t_mulu_hi, c_a_u, c_b_u);
        ctx.gen_mov(Type::I64, regs[10], t_mulu_lo);
        ctx.gen_mov(Type::I64, regs[11], t_mulu_hi);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[10], mulu_lo);
    assert_eq!(cpu.regs[11], mulu_hi);
}

#[test]
fn test_exec_divs2() {
    let mut cpu = RiscvCpuState::new();
    let divs_al: i64 = 100;
    let divs_ah: i64 = 0;
    let divs_b: i64 = 7;

    let divs_dividend = ((divs_ah as i128) << 64) | (divs_al as u64 as i128);
    let divs_q = divs_dividend / (divs_b as i128);
    let divs_r = divs_dividend % (divs_b as i128);
    let divs_q_lo = divs_q as u64;
    let divs_r_hi = divs_r as u64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let c_divs_al = ctx.new_const(Type::I64, divs_al as u64);
        let c_divs_ah = ctx.new_const(Type::I64, divs_ah as u64);
        let c_divs_b = ctx.new_const(Type::I64, divs_b as u64);
        let t_divs_lo = ctx.new_temp(Type::I64);
        let t_divs_hi = ctx.new_temp(Type::I64);

        ctx.gen_insn_start(0x5342);
        ctx.gen_divs2(
            Type::I64,
            t_divs_lo,
            t_divs_hi,
            c_divs_al,
            c_divs_ah,
            c_divs_b,
        );
        ctx.gen_mov(Type::I64, regs[10], t_divs_lo);
        ctx.gen_mov(Type::I64, regs[11], t_divs_hi);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[10], divs_q_lo);
    assert_eq!(cpu.regs[11], divs_r_hi);
}

#[test]
fn test_exec_divu2() {
    let mut cpu = RiscvCpuState::new();
    let divu_al: u64 = 0x1_0000_0000;
    let divu_ah: u64 = 0;
    let divu_b: u64 = 3;

    let divu_dividend = ((divu_ah as u128) << 64) | (divu_al as u128);
    let divu_q = divu_dividend / (divu_b as u128);
    let divu_r = divu_dividend % (divu_b as u128);
    let divu_q_lo = divu_q as u64;
    let divu_r_hi = divu_r as u64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let c_divu_al = ctx.new_const(Type::I64, divu_al);
        let c_divu_ah = ctx.new_const(Type::I64, divu_ah);
        let c_divu_b = ctx.new_const(Type::I64, divu_b);
        let t_divu_lo = ctx.new_temp(Type::I64);
        let t_divu_hi = ctx.new_temp(Type::I64);

        ctx.gen_insn_start(0x5343);
        ctx.gen_divu2(
            Type::I64,
            t_divu_lo,
            t_divu_hi,
            c_divu_al,
            c_divu_ah,
            c_divu_b,
        );
        ctx.gen_mov(Type::I64, regs[10], t_divu_lo);
        ctx.gen_mov(Type::I64, regs[11], t_divu_hi);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[10], divu_q_lo);
    assert_eq!(cpu.regs[11], divu_r_hi);
}

#[test]
fn test_exec_carry_borrow_ops() {
    let mut cpu = RiscvCpuState::new();
    let max = u64::MAX;
    let one = 1u64;
    let three = 3u64;
    let five = 5u64;
    let six = 6u64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let c_max = ctx.new_const(Type::I64, max);
        let c_one = ctx.new_const(Type::I64, one);
        let c_three = ctx.new_const(Type::I64, three);
        let c_five = ctx.new_const(Type::I64, five);
        let c_six = ctx.new_const(Type::I64, six);

        let t_addco1 = ctx.new_temp(Type::I64);
        let t_addci1 = ctx.new_temp(Type::I64);
        let t_addco2 = ctx.new_temp(Type::I64);
        let t_addcio = ctx.new_temp(Type::I64);
        let t_addci2 = ctx.new_temp(Type::I64);
        let t_addc1o = ctx.new_temp(Type::I64);

        let t_subbo = ctx.new_temp(Type::I64);
        let t_subbi1 = ctx.new_temp(Type::I64);
        let t_subbio = ctx.new_temp(Type::I64);
        let t_subbi2 = ctx.new_temp(Type::I64);
        let t_subb1o = ctx.new_temp(Type::I64);

        ctx.gen_insn_start(0x5350);
        ctx.gen_addco(Type::I64, t_addco1, c_max, c_one);
        ctx.gen_mov(Type::I64, regs[10], t_addco1);
        ctx.gen_addci(Type::I64, t_addci1, c_five, c_six);
        ctx.gen_mov(Type::I64, regs[11], t_addci1);
        ctx.gen_addco(Type::I64, t_addco2, c_max, c_one);
        ctx.gen_mov(Type::I64, regs[12], t_addco2);
        ctx.gen_addcio(Type::I64, t_addcio, c_max, c_one);
        ctx.gen_mov(Type::I64, regs[13], t_addcio);
        ctx.gen_addci(Type::I64, t_addci2, c_five, c_six);
        ctx.gen_mov(Type::I64, regs[14], t_addci2);
        ctx.gen_addc1o(Type::I64, t_addc1o, c_five, c_six);
        ctx.gen_mov(Type::I64, regs[15], t_addc1o);

        ctx.gen_subbo(Type::I64, t_subbo, c_one, c_three);
        ctx.gen_mov(Type::I64, regs[16], t_subbo);
        ctx.gen_subbi(Type::I64, t_subbi1, c_five, c_three);
        ctx.gen_mov(Type::I64, regs[17], t_subbi1);
        ctx.gen_subbo(Type::I64, t_subbo, c_one, c_three);
        ctx.gen_subbio(Type::I64, t_subbio, c_one, c_three);
        ctx.gen_mov(Type::I64, regs[18], t_subbio);
        ctx.gen_subbi(Type::I64, t_subbi2, c_five, c_three);
        ctx.gen_mov(Type::I64, regs[19], t_subbi2);
        ctx.gen_subb1o(Type::I64, t_subb1o, c_five, c_three);
        ctx.gen_mov(Type::I64, regs[20], t_subb1o);

        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[10], 0);
    assert_eq!(cpu.regs[11], 12);
    assert_eq!(cpu.regs[12], 0);
    assert_eq!(cpu.regs[13], 1);
    assert_eq!(cpu.regs[14], 12);
    assert_eq!(cpu.regs[15], 12);
    assert_eq!(cpu.regs[16], 0xFFFF_FFFF_FFFF_FFFEu64);
    assert_eq!(cpu.regs[17], 1);
    assert_eq!(cpu.regs[18], 0xFFFF_FFFF_FFFF_FFFDu64);
    assert_eq!(cpu.regs[19], 1);
    assert_eq!(cpu.regs[20], 1);
}

#[test]
fn test_exec_negsetcond_movcond() {
    let mut cpu = RiscvCpuState::new();

    let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
        let c5 = ctx.new_const(Type::I64, 5);
        let c6 = ctx.new_const(Type::I64, 6);
        let v1a = ctx.new_const(Type::I64, 0x1111);
        let v2a = ctx.new_const(Type::I64, 0x2222);
        let v1b = ctx.new_const(Type::I64, 0xAAAA);
        let v2b = ctx.new_const(Type::I64, 0xBBBB);

        let t_nsc_true = ctx.new_temp(Type::I64);
        let t_nsc_false = ctx.new_temp(Type::I64);
        let t_mov_true = ctx.new_temp(Type::I64);
        let t_mov_false = ctx.new_temp(Type::I64);

        ctx.gen_insn_start(0x5360);
        ctx.gen_negsetcond(
            Type::I64,
            t_nsc_true,
            c5,
            c5,
            machina_core::Cond::Eq,
        );
        ctx.gen_mov(Type::I64, regs[10], t_nsc_true);
        ctx.gen_negsetcond(
            Type::I64,
            t_nsc_false,
            c5,
            c6,
            machina_core::Cond::Eq,
        );
        ctx.gen_mov(Type::I64, regs[11], t_nsc_false);

        ctx.gen_movcond(
            Type::I64,
            t_mov_true,
            c5,
            c5,
            v1a,
            v2a,
            machina_core::Cond::Eq,
        );
        ctx.gen_mov(Type::I64, regs[12], t_mov_true);
        ctx.gen_movcond(
            Type::I64,
            t_mov_false,
            c5,
            c6,
            v1b,
            v2b,
            machina_core::Cond::Eq,
        );
        ctx.gen_mov(Type::I64, regs[13], t_mov_false);

        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[10], 0u64.wrapping_sub(1));
    assert_eq!(cpu.regs[11], 0);
    assert_eq!(cpu.regs[12], 0x1111);
    assert_eq!(cpu.regs[13], 0xBBBB);
}

#[test]
fn test_exec_extrh_i64_i32() {
    let mut cpu = RiscvCpuStateMem::new();
    let value = 0x1122_3344_5566_7788u64;

    let exit_val = run_riscv_tb(&mut cpu, |ctx, env, _regs, _pc| {
        let mem_offset = std::mem::offset_of!(RiscvCpuStateMem, mem) as i64;
        let c_val = ctx.new_const(Type::I64, value);
        let t_extrh = ctx.new_temp(Type::I32);

        ctx.gen_insn_start(0x5370);
        ctx.gen_extrh_i64_i32(t_extrh, c_val);
        ctx.gen_st32(Type::I32, t_extrh, env, mem_offset);
        ctx.gen_exit_tb(0);
    });

    assert_eq!(exit_val, 0);
    assert_eq!(
        u32::from_le_bytes(cpu.mem[0..4].try_into().unwrap()),
        0x1122_3344u32
    );
}

#[test]
fn test_exec_goto_ptr() {
    let mut backend = X86_64CodeGen::new();
    let mut buf = CodeBuffer::new(4096).unwrap();
    backend.emit_prologue(&mut buf);
    backend.emit_epilogue(&mut buf);

    let mut ctx = Context::new();
    backend.init_context(&mut ctx);
    let (env, _regs, _pc) = setup_riscv_globals(&mut ctx);
    let mem_offset = std::mem::offset_of!(RiscvCpuStateMem, mem) as i64;

    let c_mark = ctx.new_const(Type::I64, 0x55);
    let c_after = ctx.new_const(Type::I64, 0xAA);
    let t_ptr = ctx.new_temp(Type::I64);

    ctx.gen_insn_start(0x5380);
    ctx.gen_st(Type::I64, c_mark, env, mem_offset + 8);
    ctx.gen_ld(Type::I64, t_ptr, env, mem_offset);
    ctx.gen_goto_ptr(t_ptr);
    ctx.gen_st(Type::I64, c_after, env, mem_offset + 16);
    ctx.gen_exit_tb(0x9999);

    let mut cpu = RiscvCpuStateMem::new();
    let target = buf.ptr_at(backend.epilogue_return_zero_offset) as u64;
    cpu.mem[0..8].copy_from_slice(&target.to_le_bytes());

    let exit_val = unsafe {
        translate_and_execute(
            &mut ctx,
            &backend,
            &mut buf,
            &mut cpu as *mut RiscvCpuStateMem as *mut u8,
        )
    };

    assert_eq!(exit_val, 0);
    assert_eq!(
        u64::from_le_bytes(cpu.mem[8..16].try_into().unwrap()),
        0x55u64
    );
    assert_eq!(
        u64::from_le_bytes(cpu.mem[16..24].try_into().unwrap()),
        0u64
    );
}

/// Test: compute sum 1..5 using a loop
#[test]
fn test_sum_loop() {
    let mut backend = X86_64CodeGen::new();
    let mut buf = CodeBuffer::new(4096).unwrap();
    backend.emit_prologue(&mut buf);
    backend.emit_epilogue(&mut buf);

    let mut ctx = Context::new();
    backend.init_context(&mut ctx);
    let (_env, regs, _pc) = setup_riscv_globals(&mut ctx);

    // x1 = sum (accumulator), x2 = counter, x3 = limit
    // Loop: sum += counter; counter++;
    //       if counter <= limit goto loop
    let label_loop = ctx.new_label();
    let label_end = ctx.new_label();

    ctx.gen_insn_start(0x1000);

    // Loop header
    ctx.gen_set_label(label_loop);

    // sum += counter: x1 = x1 + x2
    let tmp_sum = ctx.new_temp(Type::I64);
    ctx.gen_add(Type::I64, tmp_sum, regs[1], regs[2]);
    ctx.gen_mov(Type::I64, regs[1], tmp_sum);

    // counter++: x2 = x2 + 1
    let imm1 = ctx.new_const(Type::I64, 1);
    let tmp_cnt = ctx.new_temp(Type::I64);
    ctx.gen_add(Type::I64, tmp_cnt, regs[2], imm1);
    ctx.gen_mov(Type::I64, regs[2], tmp_cnt);

    // if counter <= limit goto loop
    ctx.gen_brcond(
        Type::I64,
        regs[2],
        regs[3],
        machina_core::Cond::Le,
        label_loop,
    );

    ctx.gen_set_label(label_end);
    ctx.gen_exit_tb(0);

    // sum = 0, counter = 1, limit = 5
    // Expected: 1+2+3+4+5 = 15
    let mut cpu = RiscvCpuState::new();
    cpu.regs[1] = 0; // sum
    cpu.regs[2] = 1; // counter
    cpu.regs[3] = 5; // limit

    let exit_val = unsafe {
        translate_and_execute(
            &mut ctx,
            &backend,
            &mut buf,
            &mut cpu as *mut RiscvCpuState as *mut u8,
        )
    };

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[1], 15, "sum of 1..5 should be 15");
    assert_eq!(cpu.regs[2], 6, "counter should be 6 after loop");
}
