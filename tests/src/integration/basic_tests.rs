use machina_backend::code_buffer::CodeBuffer;
use machina_backend::translate::translate_and_execute;
use machina_backend::HostCodeGen;
use machina_backend::X86_64CodeGen;
use machina_core::types::Type;
use machina_core::Context;

use super::{setup_riscv_globals, RiscvCpuState, RiscvCpuStateMem};

/// Test: ADDI x1, x0, 42 -> verify cpu.regs[1] == 42
#[test]
fn test_addi_x1_x0_42() {
    let mut backend = X86_64CodeGen::new();
    let mut buf = CodeBuffer::new(4096).unwrap();

    // Emit prologue + epilogue
    backend.emit_prologue(&mut buf);
    backend.emit_epilogue(&mut buf);

    // Set up context with RISC-V globals
    let mut ctx = Context::new();
    backend.init_context(&mut ctx);
    let (_env, regs, _pc) = setup_riscv_globals(&mut ctx);

    // Generate IR: x1 = x0 + 42
    ctx.gen_insn_start(0x1000);

    // x0 is always 0 in RISC-V, but in our IR it's just a
    // global. We load it and add a constant.
    let imm42 = ctx.new_const(Type::I64, 42);
    let tmp = ctx.new_temp(Type::I64);
    ctx.gen_add(Type::I64, tmp, regs[0], imm42);
    ctx.gen_mov(Type::I64, regs[1], tmp);

    // Exit TB
    ctx.gen_exit_tb(0);

    // Execute
    let mut cpu = RiscvCpuState::new();
    cpu.regs[0] = 0; // x0 = 0

    let exit_val = unsafe {
        translate_and_execute(
            &mut ctx,
            &backend,
            &mut buf,
            &mut cpu as *mut RiscvCpuState as *mut u8,
        )
    };

    assert_eq!(exit_val, 0, "exit_tb should return 0");
    assert_eq!(cpu.regs[1], 42, "x1 should be 42");
}

/// Test: ADD x3, x1, x2 -> verify x3 == x1 + x2
#[test]
fn test_add_x3_x1_x2() {
    let mut backend = X86_64CodeGen::new();
    let mut buf = CodeBuffer::new(4096).unwrap();
    backend.emit_prologue(&mut buf);
    backend.emit_epilogue(&mut buf);

    let mut ctx = Context::new();
    backend.init_context(&mut ctx);
    let (_env, regs, _pc) = setup_riscv_globals(&mut ctx);

    ctx.gen_insn_start(0x1000);
    let tmp = ctx.new_temp(Type::I64);
    ctx.gen_add(Type::I64, tmp, regs[1], regs[2]);
    ctx.gen_mov(Type::I64, regs[3], tmp);
    ctx.gen_exit_tb(0);

    let mut cpu = RiscvCpuState::new();
    cpu.regs[1] = 100;
    cpu.regs[2] = 200;

    let exit_val = unsafe {
        translate_and_execute(
            &mut ctx,
            &backend,
            &mut buf,
            &mut cpu as *mut RiscvCpuState as *mut u8,
        )
    };

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[3], 300, "x3 should be 100 + 200 = 300");
}

#[repr(C)]
struct ShiftCpuState {
    out: u64,
}

#[test]
fn test_shift_out_rcx_count_non_rcx() {
    let mut backend = X86_64CodeGen::new();
    let mut buf = CodeBuffer::new(4096).unwrap();
    backend.emit_prologue(&mut buf);
    backend.emit_epilogue(&mut buf);

    let mut ctx = Context::new();
    backend.init_context(&mut ctx);

    let env = ctx.new_fixed(
        Type::I64,
        machina_backend::x86_64::Reg::Rbp as u8,
        "env",
    );

    let c1 = ctx.new_const(Type::I64, 1);
    let cval = ctx.new_const(Type::I64, 0x10);
    let ccnt = ctx.new_const(Type::I64, 3);

    let t_hold = ctx.new_temp(Type::I64);
    let t_val = ctx.new_temp(Type::I64);
    let t_cnt = ctx.new_temp(Type::I64);
    let t_out = ctx.new_temp(Type::I64);
    let t_dummy = ctx.new_temp(Type::I64);

    ctx.gen_insn_start(0x2000);
    ctx.gen_mov(Type::I64, t_hold, c1);
    ctx.gen_mov(Type::I64, t_val, cval);
    ctx.gen_mov(Type::I64, t_cnt, ccnt);
    ctx.gen_shl(Type::I64, t_out, t_val, t_cnt);
    ctx.gen_add(Type::I64, t_dummy, t_hold, t_cnt);
    ctx.gen_st(Type::I64, t_out, env, 0);
    ctx.gen_exit_tb(0);

    let mut cpu = ShiftCpuState { out: 0 };
    let exit_val = unsafe {
        translate_and_execute(
            &mut ctx,
            &backend,
            &mut buf,
            &mut cpu as *mut ShiftCpuState as *mut u8,
        )
    };

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.out, 0x10u64 << 3);
}

/// Test: combine AND/XOR/OR/ADD in one TB (AND, XOR, OR, ADD).
#[test]
fn test_alu_mix_and_or_xor_add() {
    let mut backend = X86_64CodeGen::new();
    let mut buf = CodeBuffer::new(4096).unwrap();
    backend.emit_prologue(&mut buf);
    backend.emit_epilogue(&mut buf);

    let mut ctx = Context::new();
    backend.init_context(&mut ctx);
    let (_env, regs, _pc) = setup_riscv_globals(&mut ctx);

    ctx.gen_insn_start(0x3000);
    let t_and = ctx.new_temp(Type::I64);
    let t_xor = ctx.new_temp(Type::I64);
    let t_or = ctx.new_temp(Type::I64);
    let t_add = ctx.new_temp(Type::I64);

    ctx.gen_and(Type::I64, t_and, regs[1], regs[2]);
    ctx.gen_xor(Type::I64, t_xor, regs[3], regs[4]);
    ctx.gen_or(Type::I64, t_or, t_and, t_xor);
    ctx.gen_add(Type::I64, t_add, t_or, t_and);
    ctx.gen_mov(Type::I64, regs[5], t_add);
    ctx.gen_exit_tb(0);

    let mut cpu = RiscvCpuState::new();
    cpu.regs[1] = 0x0F0F;
    cpu.regs[2] = 0xFF00;
    cpu.regs[3] = 0x1234;
    cpu.regs[4] = 0x00FF;

    let expected_and = cpu.regs[1] & cpu.regs[2];
    let expected_xor = cpu.regs[3] ^ cpu.regs[4];
    let expected_or = expected_and | expected_xor;
    let expected_add = expected_or.wrapping_add(expected_and);

    let exit_val = unsafe {
        translate_and_execute(
            &mut ctx,
            &backend,
            &mut buf,
            &mut cpu as *mut RiscvCpuState as *mut u8,
        )
    };

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[5], expected_add);
}

/// Test: MUL/ADD/NEG/NOT chain in one TB.
#[test]
fn test_mul_add_neg_not_chain() {
    let mut backend = X86_64CodeGen::new();
    let mut buf = CodeBuffer::new(4096).unwrap();
    backend.emit_prologue(&mut buf);
    backend.emit_epilogue(&mut buf);

    let mut ctx = Context::new();
    backend.init_context(&mut ctx);
    let (_env, regs, _pc) = setup_riscv_globals(&mut ctx);

    let t_mul = ctx.new_temp(Type::I64);
    let t_add = ctx.new_temp(Type::I64);
    let t_neg = ctx.new_temp(Type::I64);
    let t_not = ctx.new_temp(Type::I64);

    ctx.gen_insn_start(0x3050);
    ctx.gen_mul(Type::I64, t_mul, regs[1], regs[2]);
    ctx.gen_add(Type::I64, t_add, t_mul, regs[3]);
    ctx.gen_neg(Type::I64, t_neg, t_add);
    ctx.gen_not(Type::I64, t_not, t_neg);
    ctx.gen_mov(Type::I64, regs[6], t_neg);
    ctx.gen_mov(Type::I64, regs[7], t_not);
    ctx.gen_exit_tb(0);

    let mut cpu = RiscvCpuState::new();
    cpu.regs[1] = 6;
    cpu.regs[2] = 7;
    cpu.regs[3] = 5;

    let expected_mul = cpu.regs[1].wrapping_mul(cpu.regs[2]);
    let expected_add = expected_mul.wrapping_add(cpu.regs[3]);
    let expected_neg = 0u64.wrapping_sub(expected_add);
    let expected_not = !expected_neg;

    let exit_val = unsafe {
        translate_and_execute(
            &mut ctx,
            &backend,
            &mut buf,
            &mut cpu as *mut RiscvCpuState as *mut u8,
        )
    };

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[6], expected_neg);
    assert_eq!(cpu.regs[7], expected_not);
}

/// Test: SLT/SLTU using SetCond for signed and unsigned compares.
#[test]
fn test_slt_sltu() {
    let mut backend = X86_64CodeGen::new();
    let mut buf = CodeBuffer::new(4096).unwrap();
    backend.emit_prologue(&mut buf);
    backend.emit_epilogue(&mut buf);

    let mut ctx = Context::new();
    backend.init_context(&mut ctx);
    let (_env, regs, _pc) = setup_riscv_globals(&mut ctx);

    let a = ctx.new_const(Type::I64, 0xFFFF_FFFF_FFFF_FFFF);
    let b = ctx.new_const(Type::I64, 1);
    let t_slt = ctx.new_temp(Type::I64);
    let t_sltu = ctx.new_temp(Type::I64);

    ctx.gen_insn_start(0x3200);
    ctx.gen_setcond(Type::I64, t_slt, a, b, machina_core::Cond::Lt);
    ctx.gen_setcond(Type::I64, t_sltu, a, b, machina_core::Cond::Ltu);
    ctx.gen_mov(Type::I64, regs[8], t_slt);
    ctx.gen_mov(Type::I64, regs[9], t_sltu);
    ctx.gen_exit_tb(0);

    let mut cpu = RiscvCpuState::new();

    let exit_val = unsafe {
        translate_and_execute(
            &mut ctx,
            &backend,
            &mut buf,
            &mut cpu as *mut RiscvCpuState as *mut u8,
        )
    };

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[8], 1u64);
    assert_eq!(cpu.regs[9], 0u64);
}

/// Test: AUIPC/LUI style sequences using pc + imm and imm << 12.
#[test]
fn test_auipc_lui() {
    let mut backend = X86_64CodeGen::new();
    let mut buf = CodeBuffer::new(4096).unwrap();
    backend.emit_prologue(&mut buf);
    backend.emit_epilogue(&mut buf);

    let mut ctx = Context::new();
    backend.init_context(&mut ctx);
    let (_env, regs, pc) = setup_riscv_globals(&mut ctx);

    let imm = ctx.new_const(Type::I64, 0xABCDE << 12);
    let t_auipc = ctx.new_temp(Type::I64);
    let t_lui = ctx.new_temp(Type::I64);

    ctx.gen_insn_start(0x3300);
    ctx.gen_add(Type::I64, t_auipc, pc, imm);
    ctx.gen_mov(Type::I64, regs[10], t_auipc);
    ctx.gen_mov(Type::I64, t_lui, imm);
    ctx.gen_mov(Type::I64, regs[11], t_lui);
    ctx.gen_exit_tb(0);

    let mut cpu = RiscvCpuState::new();
    cpu.pc = 0x8000_0000;

    let exit_val = unsafe {
        translate_and_execute(
            &mut ctx,
            &backend,
            &mut buf,
            &mut cpu as *mut RiscvCpuState as *mut u8,
        )
    };

    let expected_auipc = 0x8000_0000u64.wrapping_add(0xABCDE << 12);

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[10], expected_auipc);
    assert_eq!(cpu.regs[11], 0xABCDE << 12);
}

/// Test: store/load via env base, then move back to a register.
#[test]
fn test_load_store_64() {
    let mut backend = X86_64CodeGen::new();
    let mut buf = CodeBuffer::new(4096).unwrap();
    backend.emit_prologue(&mut buf);
    backend.emit_epilogue(&mut buf);

    let mut ctx = Context::new();
    backend.init_context(&mut ctx);
    let (env, regs, _pc) = setup_riscv_globals(&mut ctx);

    let val = ctx.new_const(Type::I64, 0xDEAD_BEEF_CAFE_BABEu64);
    let t_st = ctx.new_temp(Type::I64);
    let t_ld = ctx.new_temp(Type::I64);
    let mem_offset = std::mem::offset_of!(RiscvCpuStateMem, mem) as i64;

    ctx.gen_insn_start(0x3400);
    ctx.gen_mov(Type::I64, t_st, val);
    ctx.gen_st(Type::I64, t_st, env, mem_offset);
    ctx.gen_ld(Type::I64, t_ld, env, mem_offset);
    ctx.gen_mov(Type::I64, regs[1], t_ld);
    ctx.gen_exit_tb(0);

    let mut cpu = RiscvCpuStateMem::new();

    let exit_val = unsafe {
        translate_and_execute(
            &mut ctx,
            &backend,
            &mut buf,
            &mut cpu as *mut RiscvCpuStateMem as *mut u8,
        )
    };

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[1], 0xDEAD_BEEF_CAFE_BABEu64);
    let stored = u64::from_le_bytes(cpu.mem[0..8].try_into().unwrap());
    assert_eq!(stored, 0xDEAD_BEEF_CAFE_BABEu64);
}

/// Test: signed vs unsigned branches with two compare paths.
#[test]
fn test_signed_unsigned_branches() {
    let mut backend = X86_64CodeGen::new();
    let mut buf = CodeBuffer::new(4096).unwrap();
    backend.emit_prologue(&mut buf);
    backend.emit_epilogue(&mut buf);

    let mut ctx = Context::new();
    backend.init_context(&mut ctx);
    let (_env, regs, _pc) = setup_riscv_globals(&mut ctx);

    let label_signed_taken = ctx.new_label();
    let label_after_signed = ctx.new_label();
    let label_unsigned_taken = ctx.new_label();
    let label_end = ctx.new_label();

    let neg1 = ctx.new_const(Type::I64, 0xFFFF_FFFF_FFFF_FFFFu64);
    let one = ctx.new_const(Type::I64, 1);
    let t1 = ctx.new_temp(Type::I64);
    let t0 = ctx.new_temp(Type::I64);
    let c1 = ctx.new_const(Type::I64, 1);
    let c0 = ctx.new_const(Type::I64, 0);

    ctx.gen_insn_start(0x3500);

    // Signed: -1 < 1 (true)
    ctx.gen_brcond(
        Type::I64,
        neg1,
        one,
        machina_core::Cond::Lt,
        label_signed_taken,
    );
    ctx.gen_mov(Type::I64, t0, c0);
    ctx.gen_mov(Type::I64, regs[10], t0);
    ctx.gen_br(label_after_signed);

    ctx.gen_set_label(label_signed_taken);
    ctx.gen_mov(Type::I64, t1, c1);
    ctx.gen_mov(Type::I64, regs[10], t1);

    ctx.gen_set_label(label_after_signed);

    // Unsigned: 0xFFFF... > 1 (true)
    ctx.gen_brcond(
        Type::I64,
        neg1,
        one,
        machina_core::Cond::Gtu,
        label_unsigned_taken,
    );
    ctx.gen_mov(Type::I64, t0, c0);
    ctx.gen_mov(Type::I64, regs[11], t0);
    ctx.gen_br(label_end);

    ctx.gen_set_label(label_unsigned_taken);
    ctx.gen_mov(Type::I64, t1, c1);
    ctx.gen_mov(Type::I64, regs[11], t1);

    ctx.gen_set_label(label_end);
    ctx.gen_exit_tb(0);

    let mut cpu = RiscvCpuState::new();

    let exit_val = unsafe {
        translate_and_execute(
            &mut ctx,
            &backend,
            &mut buf,
            &mut cpu as *mut RiscvCpuState as *mut u8,
        )
    };

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[10], 1);
    assert_eq!(cpu.regs[11], 1);
}

/// Test: SUB x3, x1, x2
#[test]
fn test_sub_x3_x1_x2() {
    let mut backend = X86_64CodeGen::new();
    let mut buf = CodeBuffer::new(4096).unwrap();
    backend.emit_prologue(&mut buf);
    backend.emit_epilogue(&mut buf);

    let mut ctx = Context::new();
    backend.init_context(&mut ctx);
    let (_env, regs, _pc) = setup_riscv_globals(&mut ctx);

    ctx.gen_insn_start(0x1000);
    let tmp = ctx.new_temp(Type::I64);
    ctx.gen_sub(Type::I64, tmp, regs[1], regs[2]);
    ctx.gen_mov(Type::I64, regs[3], tmp);
    ctx.gen_exit_tb(0);

    let mut cpu = RiscvCpuState::new();
    cpu.regs[1] = 500;
    cpu.regs[2] = 200;

    let exit_val = unsafe {
        translate_and_execute(
            &mut ctx,
            &backend,
            &mut buf,
            &mut cpu as *mut RiscvCpuState as *mut u8,
        )
    };

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[3], 300, "x3 should be 500 - 200 = 300");
}

/// Test: BEQ branch taken
#[test]
fn test_beq_taken() {
    let mut backend = X86_64CodeGen::new();
    let mut buf = CodeBuffer::new(4096).unwrap();
    backend.emit_prologue(&mut buf);
    backend.emit_epilogue(&mut buf);

    let mut ctx = Context::new();
    backend.init_context(&mut ctx);
    let (_env, regs, _pc) = setup_riscv_globals(&mut ctx);

    let label_eq = ctx.new_label();
    let label_end = ctx.new_label();
    let c1 = ctx.new_const(Type::I64, 1);
    let c0 = ctx.new_const(Type::I64, 0);
    let t1 = ctx.new_temp(Type::I64);
    let t0 = ctx.new_temp(Type::I64);

    ctx.gen_insn_start(0x1000);

    ctx.gen_brcond(
        Type::I64,
        regs[1],
        regs[2],
        machina_core::Cond::Eq,
        label_eq,
    );

    // Not taken path: x3 = 0
    ctx.gen_mov(Type::I64, t0, c0);
    ctx.gen_mov(Type::I64, regs[3], t0);
    ctx.gen_br(label_end);

    // Taken path: x3 = 1
    ctx.gen_set_label(label_eq);
    ctx.gen_mov(Type::I64, t1, c1);
    ctx.gen_mov(Type::I64, regs[3], t1);

    ctx.gen_set_label(label_end);
    ctx.gen_exit_tb(0);

    let mut cpu = RiscvCpuState::new();
    cpu.regs[1] = 42;
    cpu.regs[2] = 42;

    let exit_val = unsafe {
        translate_and_execute(
            &mut ctx,
            &backend,
            &mut buf,
            &mut cpu as *mut RiscvCpuState as *mut u8,
        )
    };

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[3], 1, "branch should be taken, x3 = 1");
}

/// Test: BEQ branch not taken
#[test]
fn test_beq_not_taken() {
    let mut backend = X86_64CodeGen::new();
    let mut buf = CodeBuffer::new(4096).unwrap();
    backend.emit_prologue(&mut buf);
    backend.emit_epilogue(&mut buf);

    let mut ctx = Context::new();
    backend.init_context(&mut ctx);
    let (_env, regs, _pc) = setup_riscv_globals(&mut ctx);

    let label_eq = ctx.new_label();
    let label_end = ctx.new_label();
    let c1 = ctx.new_const(Type::I64, 1);
    let c0 = ctx.new_const(Type::I64, 0);
    let t1 = ctx.new_temp(Type::I64);
    let t0 = ctx.new_temp(Type::I64);

    ctx.gen_insn_start(0x1000);

    ctx.gen_brcond(
        Type::I64,
        regs[1],
        regs[2],
        machina_core::Cond::Eq,
        label_eq,
    );

    // Not taken path: x3 = 0
    ctx.gen_mov(Type::I64, t0, c0);
    ctx.gen_mov(Type::I64, regs[3], t0);
    ctx.gen_br(label_end);

    // Taken path: x3 = 1
    ctx.gen_set_label(label_eq);
    ctx.gen_mov(Type::I64, t1, c1);
    ctx.gen_mov(Type::I64, regs[3], t1);

    ctx.gen_set_label(label_end);
    ctx.gen_exit_tb(0);

    let mut cpu = RiscvCpuState::new();
    cpu.regs[1] = 42;
    cpu.regs[2] = 99;

    let exit_val = unsafe {
        translate_and_execute(
            &mut ctx,
            &backend,
            &mut buf,
            &mut cpu as *mut RiscvCpuState as *mut u8,
        )
    };

    assert_eq!(exit_val, 0);
    assert_eq!(cpu.regs[3], 0, "branch should NOT be taken, x3 = 0");
}
