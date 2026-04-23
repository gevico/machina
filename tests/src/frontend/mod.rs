//! Frontend translation tests — encode real RISC-V instructions,
//! run them through the full frontend→backend pipeline, and verify
//! the resulting CPU state.

#[cfg(unix)]
mod difftest;
mod riscv_zba;
mod riscv_zbb;
mod riscv_zbc;
mod riscv_zbs;

use machina_accel::code_buffer::CodeBuffer;
use machina_accel::ir::tb::{
    EXCP_EBREAK, EXCP_ECALL, EXCP_MRET, EXCP_SFENCE_VMA, EXCP_SRET, EXCP_UNDEF,
    EXCP_WFI,
};
use machina_accel::ir::Context;
use machina_accel::translate::translate_and_execute;
use machina_accel::HostCodeGen;
use machina_accel::X86_64CodeGen;
use machina_guest_riscv::riscv::cpu::RiscvCpu;
use machina_guest_riscv::riscv::ext::{MisaExt, RiscvCfg};
use machina_guest_riscv::riscv::{RiscvDisasContext, RiscvTranslator};
use machina_guest_riscv::translator_loop;

// ── Instruction encoding helpers ──────────────────────────────

fn rv_r(f7: u32, rs2: u32, rs1: u32, f3: u32, rd: u32, op: u32) -> u32 {
    (f7 << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op
}

fn rv_i(imm: i32, rs1: u32, f3: u32, rd: u32, op: u32) -> u32 {
    let imm = (imm as u32) & 0xFFF;
    (imm << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op
}

fn rv_u(imm: i32, rd: u32, op: u32) -> u32 {
    ((imm as u32) & 0xFFFF_F000) | (rd << 7) | op
}

fn rv_b(imm: i32, rs2: u32, rs1: u32, f3: u32) -> u32 {
    let i = imm as u32;
    let b12 = (i >> 12) & 1;
    let b11 = (i >> 11) & 1;
    let b10_5 = (i >> 5) & 0x3F;
    let b4_1 = (i >> 1) & 0xF;
    (b12 << 31)
        | (b10_5 << 25)
        | (rs2 << 20)
        | (rs1 << 15)
        | (f3 << 12)
        | (b4_1 << 8)
        | (b11 << 7)
        | 0b1100011
}

fn rv_j(imm: i32, rd: u32) -> u32 {
    let i = imm as u32;
    let b20 = (i >> 20) & 1;
    let b10_1 = (i >> 1) & 0x3FF;
    let b11 = (i >> 11) & 1;
    let b19_12 = (i >> 12) & 0xFF;
    (b20 << 31)
        | (b10_1 << 21)
        | (b11 << 20)
        | (b19_12 << 12)
        | (rd << 7)
        | 0b1101111
}

// ── Specific instruction encoders ─────────────────────────────

const OP_LUI: u32 = 0b0110111;
const OP_AUIPC: u32 = 0b0010111;
const OP_IMM: u32 = 0b0010011;
const OP_REG: u32 = 0b0110011;
const OP_IMM32: u32 = 0b0011011;
const OP_REG32: u32 = 0b0111011;

fn lui(rd: u32, imm: i32) -> u32 {
    rv_u(imm, rd, OP_LUI)
}
fn auipc(rd: u32, imm: i32) -> u32 {
    rv_u(imm, rd, OP_AUIPC)
}
fn jal(rd: u32, imm: i32) -> u32 {
    rv_j(imm, rd)
}
fn jalr(rd: u32, rs1: u32, imm: i32) -> u32 {
    rv_i(imm, rs1, 0b000, rd, 0b1100111)
}
fn beq(rs1: u32, rs2: u32, imm: i32) -> u32 {
    rv_b(imm, rs2, rs1, 0b000)
}
fn bne(rs1: u32, rs2: u32, imm: i32) -> u32 {
    rv_b(imm, rs2, rs1, 0b001)
}
fn blt(rs1: u32, rs2: u32, imm: i32) -> u32 {
    rv_b(imm, rs2, rs1, 0b100)
}
fn bge(rs1: u32, rs2: u32, imm: i32) -> u32 {
    rv_b(imm, rs2, rs1, 0b101)
}
fn bltu(rs1: u32, rs2: u32, imm: i32) -> u32 {
    rv_b(imm, rs2, rs1, 0b110)
}
fn bgeu(rs1: u32, rs2: u32, imm: i32) -> u32 {
    rv_b(imm, rs2, rs1, 0b111)
}
fn addi(rd: u32, rs1: u32, imm: i32) -> u32 {
    rv_i(imm, rs1, 0b000, rd, OP_IMM)
}
fn slti(rd: u32, rs1: u32, imm: i32) -> u32 {
    rv_i(imm, rs1, 0b010, rd, OP_IMM)
}
fn sltiu(rd: u32, rs1: u32, imm: i32) -> u32 {
    rv_i(imm, rs1, 0b011, rd, OP_IMM)
}
fn xori(rd: u32, rs1: u32, imm: i32) -> u32 {
    rv_i(imm, rs1, 0b100, rd, OP_IMM)
}
fn ori(rd: u32, rs1: u32, imm: i32) -> u32 {
    rv_i(imm, rs1, 0b110, rd, OP_IMM)
}
fn andi(rd: u32, rs1: u32, imm: i32) -> u32 {
    rv_i(imm, rs1, 0b111, rd, OP_IMM)
}
fn slli(rd: u32, rs1: u32, sh: u32) -> u32 {
    rv_r(0b0000000, sh, rs1, 0b001, rd, OP_IMM)
}
fn srli(rd: u32, rs1: u32, sh: u32) -> u32 {
    rv_r(0b0000000, sh, rs1, 0b101, rd, OP_IMM)
}
fn srai(rd: u32, rs1: u32, sh: u32) -> u32 {
    rv_r(0b0100000, sh, rs1, 0b101, rd, OP_IMM)
}
fn add(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0, rs2, rs1, 0b000, rd, OP_REG)
}
fn sub(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0b0100000, rs2, rs1, 0b000, rd, OP_REG)
}
fn sll(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0, rs2, rs1, 0b001, rd, OP_REG)
}
fn slt(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0, rs2, rs1, 0b010, rd, OP_REG)
}
fn sltu(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0, rs2, rs1, 0b011, rd, OP_REG)
}
fn xor(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0, rs2, rs1, 0b100, rd, OP_REG)
}
fn srl(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0, rs2, rs1, 0b101, rd, OP_REG)
}
fn sra(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0b0100000, rs2, rs1, 0b101, rd, OP_REG)
}
fn or(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0, rs2, rs1, 0b110, rd, OP_REG)
}
fn and(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0, rs2, rs1, 0b111, rd, OP_REG)
}
fn fence() -> u32 {
    0x0ff0_000f
}
fn ecall() -> u32 {
    0x0000_0073
}
fn ebreak() -> u32 {
    0x0010_0073
}
fn mret() -> u32 {
    0x3020_0073
}
fn sret() -> u32 {
    0x1020_0073
}
fn wfi() -> u32 {
    0x1050_0073
}
fn sfence_vma(rs1: u32, rs2: u32) -> u32 {
    rv_r(0b0001001, rs2, rs1, 0b000, 0, 0b1110011)
}
// RV64I W-suffix
fn addiw(rd: u32, rs1: u32, imm: i32) -> u32 {
    rv_i(imm, rs1, 0b000, rd, OP_IMM32)
}
fn slliw(rd: u32, rs1: u32, sh: u32) -> u32 {
    rv_r(0, sh, rs1, 0b001, rd, OP_IMM32)
}
fn srliw(rd: u32, rs1: u32, sh: u32) -> u32 {
    rv_r(0, sh, rs1, 0b101, rd, OP_IMM32)
}
fn sraiw(rd: u32, rs1: u32, sh: u32) -> u32 {
    rv_r(0b0100000, sh, rs1, 0b101, rd, OP_IMM32)
}
fn addw(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0, rs2, rs1, 0b000, rd, OP_REG32)
}
fn subw(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0b0100000, rs2, rs1, 0b000, rd, OP_REG32)
}
fn sllw(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0, rs2, rs1, 0b001, rd, OP_REG32)
}
fn srlw(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0, rs2, rs1, 0b101, rd, OP_REG32)
}
fn sraw(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0b0100000, rs2, rs1, 0b101, rd, OP_REG32)
}

// RV32M
const OP_M_FUNCT7: u32 = 0b0000001;
fn mul(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(OP_M_FUNCT7, rs2, rs1, 0b000, rd, OP_REG)
}
fn div_rv(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(OP_M_FUNCT7, rs2, rs1, 0b100, rd, OP_REG)
}
fn mulw(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(OP_M_FUNCT7, rs2, rs1, 0b000, rd, OP_REG32)
}

// RV32A
const OP_AMO: u32 = 0b0101111;
fn lr_w(rd: u32, rs1: u32) -> u32 {
    rv_r(0b00010 << 2, 0, rs1, 0b010, rd, OP_AMO)
}
fn amoswap_w(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0b00001 << 2, rs2, rs1, 0b010, rd, OP_AMO)
}

// Zicsr
const OP_SYSTEM: u32 = 0b1110011;
fn csrrw(rd: u32, rs1: u32, csr: u32) -> u32 {
    (csr << 20) | (rs1 << 15) | (0b001 << 12) | (rd << 7) | OP_SYSTEM
}

// ── Test runner ───────────────────────────────────────────────

/// Translate one RISC-V instruction at PC=0 and execute it.
fn run_rv(cpu: &mut RiscvCpu, insn: u32) -> usize {
    run_rv_insns(cpu, &[insn])
}

/// Translate a sequence of RISC-V instructions starting at
/// PC=0 and execute the resulting TB.
fn run_rv_insns(cpu: &mut RiscvCpu, insns: &[u32]) -> usize {
    run_rv_insns_with_cfg(cpu, insns, RiscvCfg::default())
}

/// Like `run_rv_insns` but with a custom extension config.
fn run_rv_insns_with_cfg(
    cpu: &mut RiscvCpu,
    insns: &[u32],
    cfg: RiscvCfg,
) -> usize {
    if cfg.misa.contains(MisaExt::F) || cfg.misa.contains(MisaExt::D) {
        cpu.csr.mstatus |= 0x3 << 13;
    }
    let code: Vec<u8> = insns.iter().flat_map(|i| i.to_le_bytes()).collect();
    let guest_base = code.as_ptr();

    let mut backend = X86_64CodeGen::new();
    let mut buf = CodeBuffer::new(4096).unwrap();
    backend.emit_prologue(&mut buf);
    backend.emit_epilogue(&mut buf);

    let mut ctx = Context::new();
    backend.init_context(&mut ctx);

    let mut disas = RiscvDisasContext::new(0, guest_base, cfg);
    disas.base.max_insns = insns.len() as u32;
    translator_loop::<RiscvTranslator>(&mut disas, &mut ctx);

    unsafe {
        translate_and_execute(
            &mut ctx,
            &backend,
            &mut buf,
            cpu as *mut RiscvCpu as *mut u8,
        )
    }
}

/// Like `run_rv` but with a custom extension config.
fn run_rv_with_cfg(cpu: &mut RiscvCpu, insn: u32, cfg: RiscvCfg) -> usize {
    run_rv_insns_with_cfg(cpu, &[insn], cfg)
}

// ── RV32I: Upper immediate ────────────────────────────────────

#[test]
fn test_lui() {
    let mut cpu = RiscvCpu::new();
    run_rv(&mut cpu, lui(1, 0x12345_000u32 as i32));
    assert_eq!(cpu.gpr[1], 0x12345000);
}

#[test]
fn test_lui_negative() {
    let mut cpu = RiscvCpu::new();
    run_rv(&mut cpu, lui(1, 0xFFFFF_000u32 as i32));
    assert_eq!(cpu.gpr[1], 0xFFFF_FFFF_FFFF_F000);
}

#[test]
fn test_auipc() {
    let mut cpu = RiscvCpu::new();
    // PC=0, auipc x1, 0x2000 → x1 = 0 + 0x2000000
    run_rv(&mut cpu, auipc(1, 0x2000_000u32 as i32));
    assert_eq!(cpu.gpr[1], 0x0200_0000);
}

// ── RV32I: Jumps ──────────────────────────────────────────────

#[test]
fn test_jal() {
    let mut cpu = RiscvCpu::new();
    // jal x1, 8 → x1 = PC+4 = 4, PC = 0+8 = 8
    run_rv(&mut cpu, jal(1, 8));
    assert_eq!(cpu.gpr[1], 4);
    assert_eq!(cpu.pc, 8);
}

#[test]
fn test_jalr() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[2] = 0x1000;
    // jalr x1, x2, 4 → x1 = PC+4 = 4, PC = (0x1000+4)&~1 = 0x1004
    run_rv(&mut cpu, jalr(1, 2, 4));
    assert_eq!(cpu.gpr[1], 4);
    assert_eq!(cpu.pc, 0x1004);
}

// ── RV32I: Branches ───────────────────────────────────────────

#[test]
fn test_beq_taken() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 42;
    cpu.gpr[2] = 42;
    run_rv(&mut cpu, beq(1, 2, 16));
    assert_eq!(cpu.pc, 16); // taken
}

#[test]
fn test_beq_not_taken() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 42;
    cpu.gpr[2] = 43;
    run_rv(&mut cpu, beq(1, 2, 16));
    assert_eq!(cpu.pc, 4); // not taken
}

#[test]
fn test_bne_taken() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 1;
    cpu.gpr[2] = 2;
    run_rv(&mut cpu, bne(1, 2, 20));
    assert_eq!(cpu.pc, 20);
}

#[test]
fn test_bne_not_taken() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 5;
    cpu.gpr[2] = 5;
    run_rv(&mut cpu, bne(1, 2, 20));
    assert_eq!(cpu.pc, 4);
}

#[test]
fn test_blt_taken() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = (-1i64) as u64; // -1 < 0
    cpu.gpr[2] = 0;
    run_rv(&mut cpu, blt(1, 2, 12));
    assert_eq!(cpu.pc, 12);
}

#[test]
fn test_blt_not_taken() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 5;
    cpu.gpr[2] = 3;
    run_rv(&mut cpu, blt(1, 2, 12));
    assert_eq!(cpu.pc, 4);
}

#[test]
fn test_bge_taken() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 10;
    cpu.gpr[2] = 10;
    run_rv(&mut cpu, bge(1, 2, 8));
    assert_eq!(cpu.pc, 8);
}

#[test]
fn test_bge_not_taken() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = (-5i64) as u64;
    cpu.gpr[2] = 0;
    run_rv(&mut cpu, bge(1, 2, 8));
    assert_eq!(cpu.pc, 4);
}

#[test]
fn test_bltu_taken() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 3;
    cpu.gpr[2] = 100;
    run_rv(&mut cpu, bltu(1, 2, 16));
    assert_eq!(cpu.pc, 16);
}

#[test]
fn test_bltu_not_taken() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = (-1i64) as u64; // max unsigned
    cpu.gpr[2] = 5;
    run_rv(&mut cpu, bltu(1, 2, 16));
    assert_eq!(cpu.pc, 4);
}

#[test]
fn test_bgeu_taken() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = (-1i64) as u64;
    cpu.gpr[2] = 5;
    run_rv(&mut cpu, bgeu(1, 2, 24));
    assert_eq!(cpu.pc, 24);
}

#[test]
fn test_bgeu_not_taken() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 3;
    cpu.gpr[2] = 100;
    run_rv(&mut cpu, bgeu(1, 2, 24));
    assert_eq!(cpu.pc, 4);
}

// ── RV32I: ALU immediate ──────────────────────────────────────

#[test]
fn test_addi() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 100;
    run_rv(&mut cpu, addi(3, 1, 42));
    assert_eq!(cpu.gpr[3], 142);
}

#[test]
fn test_addi_negative() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 100;
    run_rv(&mut cpu, addi(3, 1, -10));
    assert_eq!(cpu.gpr[3], 90);
}

#[test]
fn test_slti_true() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = (-5i64) as u64;
    run_rv(&mut cpu, slti(3, 1, 0));
    assert_eq!(cpu.gpr[3], 1);
}

#[test]
fn test_slti_false() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 10;
    run_rv(&mut cpu, slti(3, 1, 5));
    assert_eq!(cpu.gpr[3], 0);
}

#[test]
fn test_sltiu_true() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 3;
    run_rv(&mut cpu, sltiu(3, 1, 10));
    assert_eq!(cpu.gpr[3], 1);
}

#[test]
fn test_sltiu_false() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 100;
    run_rv(&mut cpu, sltiu(3, 1, 10));
    assert_eq!(cpu.gpr[3], 0);
}

#[test]
fn test_xori() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0xFF;
    run_rv(&mut cpu, xori(3, 1, 0x0F));
    assert_eq!(cpu.gpr[3], 0xF0);
}

#[test]
fn test_ori() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0xF0;
    run_rv(&mut cpu, ori(3, 1, 0x0F));
    assert_eq!(cpu.gpr[3], 0xFF);
}

#[test]
fn test_andi() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0xFF;
    run_rv(&mut cpu, andi(3, 1, 0x0F));
    assert_eq!(cpu.gpr[3], 0x0F);
}

// ── RV32I: Shift immediate ────────────────────────────────────

#[test]
fn test_slli() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 1;
    run_rv(&mut cpu, slli(3, 1, 4));
    assert_eq!(cpu.gpr[3], 16);
}

#[test]
fn test_srli() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0x80;
    run_rv(&mut cpu, srli(3, 1, 4));
    assert_eq!(cpu.gpr[3], 0x08);
}

#[test]
fn test_srai() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = (-16i64) as u64;
    run_rv(&mut cpu, srai(3, 1, 2));
    assert_eq!(cpu.gpr[3], (-4i64) as u64);
}

// ── RV32I: R-type ALU ─────────────────────────────────────────

#[test]
fn test_add() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 100;
    cpu.gpr[2] = 200;
    run_rv(&mut cpu, add(3, 1, 2));
    assert_eq!(cpu.gpr[3], 300);
}

#[test]
fn test_sub() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 200;
    cpu.gpr[2] = 50;
    run_rv(&mut cpu, sub(3, 1, 2));
    assert_eq!(cpu.gpr[3], 150);
}

#[test]
fn test_sll() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 1;
    cpu.gpr[2] = 8;
    run_rv(&mut cpu, sll(3, 1, 2));
    assert_eq!(cpu.gpr[3], 256);
}

#[test]
fn test_slt_true() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = (-1i64) as u64;
    cpu.gpr[2] = 0;
    run_rv(&mut cpu, slt(3, 1, 2));
    assert_eq!(cpu.gpr[3], 1);
}

#[test]
fn test_slt_false() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 5;
    cpu.gpr[2] = 3;
    run_rv(&mut cpu, slt(3, 1, 2));
    assert_eq!(cpu.gpr[3], 0);
}

#[test]
fn test_sltu_true() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 3;
    cpu.gpr[2] = 100;
    run_rv(&mut cpu, sltu(3, 1, 2));
    assert_eq!(cpu.gpr[3], 1);
}

#[test]
fn test_sltu_false() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = (-1i64) as u64;
    cpu.gpr[2] = 5;
    run_rv(&mut cpu, sltu(3, 1, 2));
    assert_eq!(cpu.gpr[3], 0);
}

#[test]
fn test_xor() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0xFF00;
    cpu.gpr[2] = 0x0FF0;
    run_rv(&mut cpu, xor(3, 1, 2));
    assert_eq!(cpu.gpr[3], 0xF0F0);
}

#[test]
fn test_srl() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0x100;
    cpu.gpr[2] = 4;
    run_rv(&mut cpu, srl(3, 1, 2));
    assert_eq!(cpu.gpr[3], 0x10);
}

#[test]
fn test_sra() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = (-64i64) as u64;
    cpu.gpr[2] = 3;
    run_rv(&mut cpu, sra(3, 1, 2));
    assert_eq!(cpu.gpr[3], (-8i64) as u64);
}

#[test]
fn test_or() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0xF0;
    cpu.gpr[2] = 0x0F;
    run_rv(&mut cpu, or(3, 1, 2));
    assert_eq!(cpu.gpr[3], 0xFF);
}

#[test]
fn test_and() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0xFF;
    cpu.gpr[2] = 0x0F;
    run_rv(&mut cpu, and(3, 1, 2));
    assert_eq!(cpu.gpr[3], 0x0F);
}

// ── RV32I: Fence / System ─────────────────────────────────────

#[test]
fn test_fence_nop() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 42;
    // fence is a NOP; the TB falls through to tb_stop
    run_rv(&mut cpu, fence());
    assert_eq!(cpu.gpr[1], 42); // unchanged
}

#[test]
fn test_ecall_exit() {
    let mut cpu = RiscvCpu::new();
    let exit = run_rv(&mut cpu, ecall());
    assert_eq!(exit, EXCP_ECALL as usize);
    assert_eq!(cpu.pc, 0); // PC synced to insn PC
}

#[test]
fn test_ebreak_exit() {
    let mut cpu = RiscvCpu::new();
    let exit = run_rv(&mut cpu, ebreak());
    assert_eq!(exit, EXCP_EBREAK as usize);
    assert_eq!(cpu.pc, 0);
}

// ── Privileged instructions ──────────────────────────────────

#[test]
fn test_mret_exit() {
    let mut cpu = RiscvCpu::new();
    let exit = run_rv(&mut cpu, mret());
    assert_eq!(exit, EXCP_MRET as usize);
    // PC points past the mret instruction (next-insn PC).
    assert_eq!(cpu.pc, 4);
}

#[test]
fn test_sret_exit() {
    let mut cpu = RiscvCpu::new();
    let exit = run_rv(&mut cpu, sret());
    assert_eq!(exit, EXCP_SRET as usize);
    assert_eq!(cpu.pc, 4);
}

#[test]
fn test_wfi_exit() {
    let mut cpu = RiscvCpu::new();
    let exit = run_rv(&mut cpu, wfi());
    assert_eq!(exit, EXCP_WFI as usize);
    assert_eq!(cpu.pc, 4);
}

#[test]
fn test_sfence_vma_exit() {
    let mut cpu = RiscvCpu::new();
    let exit = run_rv(&mut cpu, sfence_vma(0, 0));
    assert_eq!(exit, EXCP_SFENCE_VMA as usize);
    assert_eq!(cpu.pc, 4);
}

#[test]
fn test_sfence_vma_with_rs1() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[5] = 0x1000;
    let exit = run_rv(&mut cpu, sfence_vma(5, 0));
    assert_eq!(exit, EXCP_SFENCE_VMA as usize);
    assert_eq!(cpu.pc, 4);
    // GPR unchanged (sfence_vma doesn't modify registers).
    assert_eq!(cpu.gpr[5], 0x1000);
}

// ── RV64I: W-suffix ALU ───────────────────────────────────────

#[test]
fn test_addiw() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 100;
    run_rv(&mut cpu, addiw(3, 1, 50));
    assert_eq!(cpu.gpr[3], 150);
}

#[test]
fn test_addiw_sign_extend() {
    let mut cpu = RiscvCpu::new();
    // 0x7FFF_FFFF + 1 = 0x8000_0000 → sext = 0xFFFF_FFFF_8000_0000
    cpu.gpr[1] = 0x7FFF_FFFF;
    run_rv(&mut cpu, addiw(3, 1, 1));
    assert_eq!(cpu.gpr[3], 0xFFFF_FFFF_8000_0000u64);
}

#[test]
fn test_slliw() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 1;
    run_rv(&mut cpu, slliw(3, 1, 31));
    // 1 << 31 = 0x8000_0000 → sext = 0xFFFF_FFFF_8000_0000
    assert_eq!(cpu.gpr[3], 0xFFFF_FFFF_8000_0000u64);
}

#[test]
fn test_srliw() {
    let mut cpu = RiscvCpu::new();
    // Only low 32 bits matter: 0xFFFF_FFFF → >> 16 = 0x0000_FFFF
    cpu.gpr[1] = 0xFFFF_FFFF_FFFF_FFFFu64;
    run_rv(&mut cpu, srliw(3, 1, 16));
    assert_eq!(cpu.gpr[3], 0x0000_FFFF);
}

#[test]
fn test_sraiw() {
    let mut cpu = RiscvCpu::new();
    // Low 32 bits = 0x8000_0000 (negative in i32)
    cpu.gpr[1] = 0x0000_0000_8000_0000u64;
    run_rv(&mut cpu, sraiw(3, 1, 4));
    // i32: 0x8000_0000 >> 4 = 0xF800_0000 → sext
    assert_eq!(cpu.gpr[3], 0xFFFF_FFFF_F800_0000u64);
}

#[test]
fn test_addw() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0x7FFF_FFFF;
    cpu.gpr[2] = 1;
    run_rv(&mut cpu, addw(3, 1, 2));
    assert_eq!(cpu.gpr[3], 0xFFFF_FFFF_8000_0000u64);
}

#[test]
fn test_subw() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0;
    cpu.gpr[2] = 1;
    run_rv(&mut cpu, subw(3, 1, 2));
    // 0 - 1 = 0xFFFF_FFFF → sext = 0xFFFF_FFFF_FFFF_FFFF
    assert_eq!(cpu.gpr[3], 0xFFFF_FFFF_FFFF_FFFFu64);
}

#[test]
fn test_sllw() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0xFF;
    cpu.gpr[2] = 24;
    run_rv(&mut cpu, sllw(3, 1, 2));
    // 0xFF << 24 = 0xFF00_0000 → sext = 0xFFFF_FFFF_FF00_0000
    assert_eq!(cpu.gpr[3], 0xFFFF_FFFF_FF00_0000u64);
}

#[test]
fn test_srlw() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0xFFFF_FFFF_FFFF_FFFFu64;
    cpu.gpr[2] = 16;
    run_rv(&mut cpu, srlw(3, 1, 2));
    // low32 = 0xFFFF_FFFF >> 16 = 0x0000_FFFF → sext positive
    assert_eq!(cpu.gpr[3], 0x0000_FFFF);
}

#[test]
fn test_sraw() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0x8000_0000u64; // negative in i32
    cpu.gpr[2] = 4;
    run_rv(&mut cpu, sraw(3, 1, 2));
    assert_eq!(cpu.gpr[3], 0xFFFF_FFFF_F800_0000u64);
}

// ── x0 hardwired zero ─────────────────────────────────────────

#[test]
fn test_x0_write_ignored() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 42;
    // addi x0, x1, 100 → should NOT change x0
    run_rv(&mut cpu, addi(0, 1, 100));
    assert_eq!(cpu.gpr[0], 0);
}

#[test]
fn test_x0_read_zero() {
    let mut cpu = RiscvCpu::new();
    // addi x3, x0, 77 → x3 = 0 + 77 = 77
    run_rv(&mut cpu, addi(3, 0, 77));
    assert_eq!(cpu.gpr[3], 77);
}

// ── Multi-instruction sequences ───────────────────────────────

#[test]
fn test_addi_addi_sequence() {
    let mut cpu = RiscvCpu::new();
    // addi x1, x0, 10; addi x2, x1, 20
    run_rv_insns(&mut cpu, &[addi(1, 0, 10), addi(2, 1, 20)]);
    assert_eq!(cpu.gpr[1], 10);
    assert_eq!(cpu.gpr[2], 30);
}

#[test]
fn test_lui_addi_combo() {
    let mut cpu = RiscvCpu::new();
    // lui x1, 0x12345000; addi x1, x1, 0x678
    run_rv_insns(
        &mut cpu,
        &[lui(1, 0x12345_000u32 as i32), addi(1, 1, 0x678)],
    );
    assert_eq!(cpu.gpr[1], 0x12345678);
}

// ── RVC encoding helpers ─────────────────────────────────────

#[allow(dead_code)]
fn rv_ci(f3: u32, imm5: u32, rd: u32, imm4_0: u32, op: u32) -> u16 {
    ((f3 & 0x7) << 13
        | (imm5 & 1) << 12
        | (rd & 0x1f) << 7
        | (imm4_0 & 0x1f) << 2
        | (op & 0x3)) as u16
}

#[allow(dead_code)]
fn rv_cr(f4: u32, rd: u32, rs2: u32, op: u32) -> u16 {
    ((f4 & 0xf) << 12 | (rd & 0x1f) << 7 | (rs2 & 0x1f) << 2 | (op & 0x3))
        as u16
}

#[allow(dead_code)]
fn rv_css(f3: u32, imm: u32, rs2: u32, op: u32) -> u16 {
    ((f3 & 0x7) << 13 | (imm & 0x3f) << 7 | (rs2 & 0x1f) << 2 | (op & 0x3))
        as u16
}

#[allow(dead_code)]
fn rv_ciw(f3: u32, imm: u32, rdp: u32, op: u32) -> u16 {
    ((f3 & 0x7) << 13 | (imm & 0xff) << 5 | (rdp & 0x7) << 2 | (op & 0x3))
        as u16
}

#[allow(dead_code)]
fn rv_cl(
    f3: u32,
    imm_hi: u32,
    rs1p: u32,
    imm_lo: u32,
    rdp: u32,
    op: u32,
) -> u16 {
    ((f3 & 0x7) << 13
        | (imm_hi & 0x7) << 10
        | (rs1p & 0x7) << 7
        | (imm_lo & 0x3) << 5
        | (rdp & 0x7) << 2
        | (op & 0x3)) as u16
}

#[allow(dead_code)]
fn rv_cs(
    f3: u32,
    imm_hi: u32,
    rs1p: u32,
    imm_lo: u32,
    rs2p: u32,
    op: u32,
) -> u16 {
    rv_cl(f3, imm_hi, rs1p, imm_lo, rs2p, op)
}

#[allow(dead_code)]
fn rv_cb(f3: u32, off_hi: u32, rs1p: u32, off_lo: u32, op: u32) -> u16 {
    ((f3 & 0x7) << 13
        | (off_hi & 0x7) << 10
        | (rs1p & 0x7) << 7
        | (off_lo & 0x1f) << 2
        | (op & 0x3)) as u16
}

#[allow(dead_code)]
fn rv_cj(f3: u32, target: u32, op: u32) -> u16 {
    ((f3 & 0x7) << 13 | (target & 0x7ff) << 2 | (op & 0x3)) as u16
}

// Specific RVC instruction encoders

/// C.LI rd, imm → addi rd, x0, sext(imm)
fn c_li(rd: u32, imm: i32) -> u16 {
    let imm = imm as u32;
    rv_ci(0b010, (imm >> 5) & 1, rd, imm & 0x1f, 0b01)
}

/// C.ADDI rd, nzimm → addi rd, rd, sext(nzimm)
fn c_addi(rd: u32, nzimm: i32) -> u16 {
    let nzimm = nzimm as u32;
    rv_ci(0b000, (nzimm >> 5) & 1, rd, nzimm & 0x1f, 0b01)
}

/// C.LUI rd, nzimm → lui rd, sext(nzimm<<12)
fn c_lui(rd: u32, nzimm: i32) -> u16 {
    // nzimm is the value >> 12
    let nzimm = nzimm as u32;
    rv_ci(0b011, (nzimm >> 5) & 1, rd, nzimm & 0x1f, 0b01)
}

/// C.MV rd, rs2 → add rd, x0, rs2
fn c_mv(rd: u32, rs2: u32) -> u16 {
    rv_cr(0b1000, rd, rs2, 0b10)
}

/// C.ADD rd, rs2 → add rd, rd, rs2
fn c_add(rd: u32, rs2: u32) -> u16 {
    rv_cr(0b1001, rd, rs2, 0b10)
}

/// C.SUB rd', rs2' → sub rd'+8, rd'+8, rs2'+8
fn c_sub(rdp: u32, rs2p: u32) -> u16 {
    // 100 0 11 rd' 00 rs2' 01
    ((0b100 << 13)
        | (0 << 12)
        | (0b11 << 10)
        | ((rdp & 0x7) << 7)
        | (0b00 << 5)
        | ((rs2p & 0x7) << 2)
        | 0b01) as u16
}

/// C.SLLI rd, shamt → slli rd, rd, shamt
fn c_slli(rd: u32, shamt: u32) -> u16 {
    rv_ci(0b000, (shamt >> 5) & 1, rd, shamt & 0x1f, 0b10)
}

/// C.ADDI4SPN rd', nzuimm → addi rd'+8, x2, nzuimm
/// nzuimm encoding: bits[5:4|9:6|2|3] in imm[12:5]
fn c_addi4spn(rdp: u32, nzuimm: u32) -> u16 {
    // Encode nzuimm into the CIW format bits
    let b5_4 = (nzuimm >> 4) & 0x3;
    let b9_6 = (nzuimm >> 6) & 0xf;
    let b2 = (nzuimm >> 2) & 0x1;
    let b3 = (nzuimm >> 3) & 0x1;
    let imm8 = (b5_4 << 6) | (b9_6 << 2) | (b3 << 1) | b2;
    rv_ciw(0b000, imm8, rdp, 0b00)
}

/// C.ADDIW rd, imm → addiw rd, rd, sext(imm)
fn c_addiw(rd: u32, imm: i32) -> u16 {
    let imm = imm as u32;
    rv_ci(0b001, (imm >> 5) & 1, rd, imm & 0x1f, 0b01)
}

/// C.J offset → jal x0, offset
fn c_j(offset: i32) -> u16 {
    // CJ target encoding: [11|4|9:8|10|6|7|3:1|5]
    let o = offset as u32;
    let b11 = (o >> 11) & 1;
    let b4 = (o >> 4) & 1;
    let b9_8 = (o >> 8) & 0x3;
    let b10 = (o >> 10) & 1;
    let b6 = (o >> 6) & 1;
    let b7 = (o >> 7) & 1;
    let b3_1 = (o >> 1) & 0x7;
    let b5 = (o >> 5) & 1;
    let target = (b11 << 10)
        | (b4 << 9)
        | (b9_8 << 7)
        | (b10 << 6)
        | (b6 << 5)
        | (b7 << 4)
        | (b3_1 << 1)
        | b5;
    rv_cj(0b101, target, 0b01)
}

/// C.BEQZ rs1', offset → beq rs1'+8, x0, offset
fn c_beqz(rs1p: u32, offset: i32) -> u16 {
    let o = offset as u32;
    let b8 = (o >> 8) & 1;
    let b4_3 = (o >> 3) & 0x3;
    let off_hi = (b8 << 2) | b4_3;
    let b7_6 = (o >> 6) & 0x3;
    let b2_1 = (o >> 1) & 0x3;
    let b5 = (o >> 5) & 1;
    let off_lo = (b7_6 << 3) | (b2_1 << 1) | b5;
    rv_cb(0b110, off_hi, rs1p, off_lo, 0b01)
}

/// C.BNEZ rs1', offset → bne rs1'+8, x0, offset
fn c_bnez(rs1p: u32, offset: i32) -> u16 {
    let o = offset as u32;
    let b8 = (o >> 8) & 1;
    let b4_3 = (o >> 3) & 0x3;
    let off_hi = (b8 << 2) | b4_3;
    let b7_6 = (o >> 6) & 0x3;
    let b2_1 = (o >> 1) & 0x3;
    let b5 = (o >> 5) & 1;
    let off_lo = (b7_6 << 3) | (b2_1 << 1) | b5;
    rv_cb(0b111, off_hi, rs1p, off_lo, 0b01)
}

/// C.EBREAK → ebreak
fn c_ebreak() -> u16 {
    rv_cr(0b1001, 0, 0, 0b10)
}

// ── RV32F/RV64F instruction encoders ────────────────────────

const OP_FP: u32 = 0b1010011;
const OP_FMADD: u32 = 0b1000011;
const OP_FMSUB: u32 = 0b1000111;
const OP_FNMSUB: u32 = 0b1001011;
const OP_FNMADD: u32 = 0b1001111;

/// R4-type FP encoding (FMA family).
fn rv_r4(
    rs3: u32,
    fmt: u32,
    rs2: u32,
    rs1: u32,
    rm: u32,
    rd: u32,
    op: u32,
) -> u32 {
    (rs3 << 27)
        | (fmt << 25)
        | (rs2 << 20)
        | (rs1 << 15)
        | (rm << 12)
        | (rd << 7)
        | op
}

fn fmadd_s(rd: u32, rs1: u32, rs2: u32, rs3: u32, rm: u32) -> u32 {
    rv_r4(rs3, 0b00, rs2, rs1, rm, rd, OP_FMADD)
}
fn fmsub_s(rd: u32, rs1: u32, rs2: u32, rs3: u32, rm: u32) -> u32 {
    rv_r4(rs3, 0b00, rs2, rs1, rm, rd, OP_FMSUB)
}
fn fnmsub_s(rd: u32, rs1: u32, rs2: u32, rs3: u32, rm: u32) -> u32 {
    rv_r4(rs3, 0b00, rs2, rs1, rm, rd, OP_FNMSUB)
}
fn fnmadd_s(rd: u32, rs1: u32, rs2: u32, rs3: u32, rm: u32) -> u32 {
    rv_r4(rs3, 0b00, rs2, rs1, rm, rd, OP_FNMADD)
}

fn fadd_s(rd: u32, rs1: u32, rs2: u32, rm: u32) -> u32 {
    rv_r(0b0000000, rs2, rs1, rm, rd, OP_FP)
}
fn fsub_s(rd: u32, rs1: u32, rs2: u32, rm: u32) -> u32 {
    rv_r(0b0000100, rs2, rs1, rm, rd, OP_FP)
}
fn fmul_s(rd: u32, rs1: u32, rs2: u32, rm: u32) -> u32 {
    rv_r(0b0001000, rs2, rs1, rm, rd, OP_FP)
}

fn feq_s(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0b1010000, rs2, rs1, 0b010, rd, OP_FP)
}
fn flt_s(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0b1010000, rs2, rs1, 0b001, rd, OP_FP)
}
fn fle_s(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0b1010000, rs2, rs1, 0b000, rd, OP_FP)
}

/// FMV.X.W rd, rs1 — move f[rs1] low 32 bits to x[rd]
fn fmv_x_w(rd: u32, rs1: u32) -> u32 {
    rv_r(0b1110000, 0, rs1, 0b000, rd, OP_FP)
}

/// FMV.W.X rd, rs1 — move x[rs1] to f[rd] with NaN-boxing
fn fmv_w_x(rd: u32, rs1: u32) -> u32 {
    rv_r(0b1111000, 0, rs1, 0b000, rd, OP_FP)
}

/// FCVT.S.W rd, rs1, rm — convert signed i32 to f32
fn fcvt_s_w(rd: u32, rs1: u32, rm: u32) -> u32 {
    rv_r(0b1101000, 0, rs1, rm, rd, OP_FP)
}

// ── Byte-level test runner ───────────────────────────────────

/// Count instructions in a raw byte stream (mixed 16/32-bit).
fn count_insns(code: &[u8]) -> u32 {
    let mut count = 0u32;
    let mut off = 0;
    while off + 1 < code.len() {
        let half = u16::from_le_bytes([code[off], code[off + 1]]);
        if half & 0x3 != 0x3 {
            off += 2;
        } else {
            off += 4;
        }
        count += 1;
    }
    count
}

/// Translate and execute a raw byte stream of RISC-V
/// instructions (supports mixed 16/32-bit).
fn run_rv_bytes(cpu: &mut RiscvCpu, code: &[u8]) -> usize {
    let guest_base = code.as_ptr();

    let mut backend = X86_64CodeGen::new();
    let mut buf = CodeBuffer::new(4096).unwrap();
    backend.emit_prologue(&mut buf);
    backend.emit_epilogue(&mut buf);

    let mut ctx = Context::new();
    backend.init_context(&mut ctx);

    let n = count_insns(code);
    let mut disas = RiscvDisasContext::new(0, guest_base, RiscvCfg::default());
    disas.base.max_insns = n;
    translator_loop::<RiscvTranslator>(&mut disas, &mut ctx);

    unsafe {
        translate_and_execute(
            &mut ctx,
            &backend,
            &mut buf,
            cpu as *mut RiscvCpu as *mut u8,
        )
    }
}

/// Helper: run a single 16-bit instruction.
fn run_rvc(cpu: &mut RiscvCpu, insn: u16) -> usize {
    let code = insn.to_le_bytes();
    run_rv_bytes(cpu, &code)
}

/// Like `run_rvc` but with a custom extension config.
fn run_rvc_with_cfg(cpu: &mut RiscvCpu, insn: u16, cfg: RiscvCfg) -> usize {
    let code = insn.to_le_bytes();
    let guest_base = code.as_ptr();

    let mut backend = X86_64CodeGen::new();
    let mut buf = CodeBuffer::new(4096).unwrap();
    backend.emit_prologue(&mut buf);
    backend.emit_epilogue(&mut buf);

    let mut ctx = Context::new();
    backend.init_context(&mut ctx);

    let mut disas = RiscvDisasContext::new(0, guest_base, cfg);
    disas.base.max_insns = 1;
    translator_loop::<RiscvTranslator>(&mut disas, &mut ctx);

    unsafe {
        translate_and_execute(
            &mut ctx,
            &backend,
            &mut buf,
            cpu as *mut RiscvCpu as *mut u8,
        )
    }
}

// ── RVC execution tests ──────────────────────────────────────

#[test]
fn test_c_li() {
    let mut cpu = RiscvCpu::new();
    run_rvc(&mut cpu, c_li(1, 15));
    assert_eq!(cpu.gpr[1], 15);
}

#[test]
fn test_c_addi() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 100;
    run_rvc(&mut cpu, c_addi(1, 5));
    assert_eq!(cpu.gpr[1], 105);
}

#[test]
fn test_c_lui() {
    let mut cpu = RiscvCpu::new();
    // C.LUI x3, 2 → x3 = 2 << 12 = 0x2000
    run_rvc(&mut cpu, c_lui(3, 2));
    assert_eq!(cpu.gpr[3], 0x2000);
}

#[test]
fn test_c_mv() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[2] = 0xDEAD;
    run_rvc(&mut cpu, c_mv(1, 2));
    assert_eq!(cpu.gpr[1], 0xDEAD);
}

#[test]
fn test_c_add() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 100;
    cpu.gpr[2] = 200;
    run_rvc(&mut cpu, c_add(1, 2));
    assert_eq!(cpu.gpr[1], 300);
}

#[test]
fn test_c_sub() {
    let mut cpu = RiscvCpu::new();
    // rd' = 0 → x8, rs2' = 1 → x9
    cpu.gpr[8] = 100;
    cpu.gpr[9] = 30;
    run_rvc(&mut cpu, c_sub(0, 1));
    assert_eq!(cpu.gpr[8], 70);
}

#[test]
fn test_c_slli() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 1;
    run_rvc(&mut cpu, c_slli(1, 8));
    assert_eq!(cpu.gpr[1], 256);
}

#[test]
fn test_c_addi4spn() {
    let mut cpu = RiscvCpu::new();
    // x2 (sp) = 0x1000, nzuimm = 16
    cpu.gpr[2] = 0x1000;
    run_rvc(&mut cpu, c_addi4spn(0, 16)); // rd' = 0 → x8
    assert_eq!(cpu.gpr[8], 0x1010);
}

#[test]
fn test_c_addiw() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0x7FFF_FFFF;
    run_rvc(&mut cpu, c_addiw(1, 1));
    // 0x7FFF_FFFF + 1 = 0x8000_0000 → sext32
    assert_eq!(cpu.gpr[1], 0xFFFF_FFFF_8000_0000u64);
}

#[test]
fn test_c_j() {
    let mut cpu = RiscvCpu::new();
    // C.J +8 → PC = 0 + 8 = 8
    run_rvc(&mut cpu, c_j(8));
    assert_eq!(cpu.pc, 8);
}

#[test]
fn test_c_beqz_taken() {
    let mut cpu = RiscvCpu::new();
    // rs1' = 0 → x8, x8 = 0 → branch taken
    cpu.gpr[8] = 0;
    run_rvc(&mut cpu, c_beqz(0, 8));
    assert_eq!(cpu.pc, 8);
}

#[test]
fn test_c_beqz_not_taken() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[8] = 1;
    run_rvc(&mut cpu, c_beqz(0, 8));
    assert_eq!(cpu.pc, 2); // PC + 2 (16-bit insn)
}

#[test]
fn test_c_bnez_taken() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[8] = 1;
    run_rvc(&mut cpu, c_bnez(0, 8));
    assert_eq!(cpu.pc, 8);
}

#[test]
fn test_c_bnez_not_taken() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[8] = 0;
    run_rvc(&mut cpu, c_bnez(0, 8));
    assert_eq!(cpu.pc, 2);
}

#[test]
fn test_c_ebreak() {
    let mut cpu = RiscvCpu::new();
    let exit = run_rvc(&mut cpu, c_ebreak());
    assert_eq!(exit, EXCP_EBREAK as usize);
}

// ── Mixed 32/16-bit sequence ─────────────────────────────────

#[test]
fn test_mixed_32_16() {
    let mut cpu = RiscvCpu::new();
    // addi x1, x0, 10 (32-bit) + C.ADDI x1, 5 (16-bit)
    let insn32 = addi(1, 0, 10);
    let insn16 = c_addi(1, 5);
    let mut code = Vec::new();
    code.extend_from_slice(&insn32.to_le_bytes());
    code.extend_from_slice(&insn16.to_le_bytes());
    run_rv_bytes(&mut cpu, &code);
    assert_eq!(cpu.gpr[1], 15);
}

// ── NaN-boxing helper ───────────────────────────────────────

/// NaN-box a 32-bit float value for FPR storage.
fn nanbox(bits: u32) -> u64 {
    0xffff_ffff_0000_0000u64 | (bits as u64)
}

// ── RV32F: FADD.S (exercises Call regalloc path) ────────────

#[test]
fn test_fadd_s() {
    let mut cpu = RiscvCpu::new();
    // f1 = 1.0f, f2 = 2.0f
    cpu.fpr[1] = nanbox(0x3f80_0000); // 1.0f
    cpu.fpr[2] = nanbox(0x4000_0000); // 2.0f
                                      // FADD.S f3, f1, f2, rm=0 (RNE)
    run_rv(&mut cpu, fadd_s(3, 1, 2, 0));
    assert_eq!(cpu.fpr[3], nanbox(0x4040_0000)); // 3.0f
}

#[test]
fn test_fsub_s() {
    let mut cpu = RiscvCpu::new();
    cpu.fpr[1] = nanbox(0x4040_0000); // 3.0f
    cpu.fpr[2] = nanbox(0x3f80_0000); // 1.0f
    run_rv(&mut cpu, fsub_s(3, 1, 2, 0));
    assert_eq!(cpu.fpr[3], nanbox(0x4000_0000)); // 2.0f
}

#[test]
fn test_fmul_s() {
    let mut cpu = RiscvCpu::new();
    cpu.fpr[1] = nanbox(0x4000_0000); // 2.0f
    cpu.fpr[2] = nanbox(0x4040_0000); // 3.0f
    run_rv(&mut cpu, fmul_s(3, 1, 2, 0));
    assert_eq!(cpu.fpr[3], nanbox(0x40c0_0000)); // 6.0f
}

// ── RV32F: FMA family (FNMSUB/FNMADD fix) ──────────────────
//
// a=2.0, b=3.0, c=1.0:
//   FMADD:  fma(a,b,c)    =  2*3+1 =  7.0
//   FMSUB:  fma(a,b,-c)   =  2*3-1 =  5.0
//   FNMSUB: fma(-a,b,c)   = -2*3+1 = -5.0
//   FNMADD: fma(-a,b,-c)  = -2*3-1 = -7.0

#[test]
fn test_fmadd_s() {
    let mut cpu = RiscvCpu::new();
    cpu.fpr[1] = nanbox(0x4000_0000); // 2.0f
    cpu.fpr[2] = nanbox(0x4040_0000); // 3.0f
    cpu.fpr[3] = nanbox(0x3f80_0000); // 1.0f
    run_rv(&mut cpu, fmadd_s(4, 1, 2, 3, 0));
    assert_eq!(cpu.fpr[4], nanbox(0x40e0_0000)); // 7.0f
}

#[test]
fn test_fmsub_s() {
    let mut cpu = RiscvCpu::new();
    cpu.fpr[1] = nanbox(0x4000_0000); // 2.0f
    cpu.fpr[2] = nanbox(0x4040_0000); // 3.0f
    cpu.fpr[3] = nanbox(0x3f80_0000); // 1.0f
    run_rv(&mut cpu, fmsub_s(4, 1, 2, 3, 0));
    assert_eq!(cpu.fpr[4], nanbox(0x40a0_0000)); // 5.0f
}

#[test]
fn test_fnmsub_s() {
    let mut cpu = RiscvCpu::new();
    cpu.fpr[1] = nanbox(0x4000_0000); // 2.0f
    cpu.fpr[2] = nanbox(0x4040_0000); // 3.0f
    cpu.fpr[3] = nanbox(0x3f80_0000); // 1.0f
                                      // FNMSUB: fma(-a, b, c) = -2*3 + 1 = -5.0
    run_rv(&mut cpu, fnmsub_s(4, 1, 2, 3, 0));
    assert_eq!(cpu.fpr[4], nanbox(0xc0a0_0000)); // -5.0f
}

#[test]
fn test_fnmadd_s() {
    let mut cpu = RiscvCpu::new();
    cpu.fpr[1] = nanbox(0x4000_0000); // 2.0f
    cpu.fpr[2] = nanbox(0x4040_0000); // 3.0f
    cpu.fpr[3] = nanbox(0x3f80_0000); // 1.0f
                                      // FNMADD: fma(-a, b, -c) = -2*3 - 1 = -7.0
    run_rv(&mut cpu, fnmadd_s(4, 1, 2, 3, 0));
    assert_eq!(cpu.fpr[4], nanbox(0xc0e0_0000)); // -7.0f
}

// ── RV32F: FMV.W.X NaN-boxing ──────────────────────────────

#[test]
fn test_fmv_w_x_nanbox() {
    let mut cpu = RiscvCpu::new();
    // x1 = 0xBF800000 (-1.0f, bit 31 set)
    cpu.gpr[1] = 0xBF80_0000;
    run_rv(&mut cpu, fmv_w_x(1, 1));
    // Must be NaN-boxed: upper 32 bits all-1s
    assert_eq!(cpu.fpr[1], nanbox(0xBF80_0000));
}

#[test]
fn test_fmv_w_x_positive() {
    let mut cpu = RiscvCpu::new();
    // x1 = 0x3F800000 (+1.0f, bit 31 clear)
    cpu.gpr[1] = 0x3F80_0000;
    run_rv(&mut cpu, fmv_w_x(2, 1));
    assert_eq!(cpu.fpr[2], nanbox(0x3F80_0000));
}

#[test]
fn test_fmv_x_w() {
    let mut cpu = RiscvCpu::new();
    cpu.fpr[1] = nanbox(0xBF80_0000); // -1.0f
    run_rv(&mut cpu, fmv_x_w(3, 1));
    // Sign-extended 32→64: 0xBF800000 → 0xFFFFFFFF_BF800000
    assert_eq!(cpu.gpr[3], 0xFFFF_FFFF_BF80_0000u64);
}

// ── RV32F: FEQ/FLT/FLE (read-only, result to GPR) ──────────

#[test]
fn test_feq_s_equal() {
    let mut cpu = RiscvCpu::new();
    cpu.fpr[1] = nanbox(0x3f80_0000); // 1.0f
    cpu.fpr[2] = nanbox(0x3f80_0000); // 1.0f
    run_rv(&mut cpu, feq_s(3, 1, 2));
    assert_eq!(cpu.gpr[3], 1);
}

#[test]
fn test_feq_s_not_equal() {
    let mut cpu = RiscvCpu::new();
    cpu.fpr[1] = nanbox(0x3f80_0000); // 1.0f
    cpu.fpr[2] = nanbox(0x4000_0000); // 2.0f
    run_rv(&mut cpu, feq_s(3, 1, 2));
    assert_eq!(cpu.gpr[3], 0);
}

#[test]
fn test_flt_s_true() {
    let mut cpu = RiscvCpu::new();
    cpu.fpr[1] = nanbox(0x3f80_0000); // 1.0f
    cpu.fpr[2] = nanbox(0x4000_0000); // 2.0f
    run_rv(&mut cpu, flt_s(3, 1, 2));
    assert_eq!(cpu.gpr[3], 1);
}

#[test]
fn test_flt_s_false() {
    let mut cpu = RiscvCpu::new();
    cpu.fpr[1] = nanbox(0x4000_0000); // 2.0f
    cpu.fpr[2] = nanbox(0x3f80_0000); // 1.0f
    run_rv(&mut cpu, flt_s(3, 1, 2));
    assert_eq!(cpu.gpr[3], 0);
}

#[test]
fn test_fle_s_equal() {
    let mut cpu = RiscvCpu::new();
    cpu.fpr[1] = nanbox(0x3f80_0000); // 1.0f
    cpu.fpr[2] = nanbox(0x3f80_0000); // 1.0f
    run_rv(&mut cpu, fle_s(3, 1, 2));
    assert_eq!(cpu.gpr[3], 1);
}

#[test]
fn test_fle_s_less() {
    let mut cpu = RiscvCpu::new();
    cpu.fpr[1] = nanbox(0x3f80_0000); // 1.0f
    cpu.fpr[2] = nanbox(0x4000_0000); // 2.0f
    run_rv(&mut cpu, fle_s(3, 1, 2));
    assert_eq!(cpu.gpr[3], 1);
}

// ── RV32F: FCVT.S.W + FADD.S sequence ──────────────────────
// Exercises multiple Call ops in one TB (regalloc stress).

#[test]
fn test_fcvt_fadd_sequence() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 10;
    cpu.gpr[2] = 20;
    // FCVT.S.W f1, x1, rm=0  → f1 = 10.0f
    // FCVT.S.W f2, x2, rm=0  → f2 = 20.0f
    // FADD.S   f3, f1, f2, rm=0 → f3 = 30.0f
    run_rv_insns(
        &mut cpu,
        &[fcvt_s_w(1, 1, 0), fcvt_s_w(2, 2, 0), fadd_s(3, 1, 2, 0)],
    );
    // 30.0f = 0x41F00000
    assert_eq!(cpu.fpr[3], nanbox(0x41f0_0000));
}

// ── Extension profile tests ─────────────────────────────────

/// Helper: RV64I-only config (no M/A/F/D/C).
fn cfg_rv64i_only() -> RiscvCfg {
    RiscvCfg {
        misa: MisaExt::I,
        ext_zicsr: false,
        ext_zifencei: false,
        ext_zicntr: false,
        ext_ziccid: false,
        ext_ziccif: false,
        ext_zicclsm: false,
        ext_ziccamoa: false,
        ext_zicbom: false,
        ext_zicbop: false,
        ext_zicboz: false,
        ext_zba: false,
        ext_zbb: false,
        ext_zbc: false,
        ext_zbkb: false,
        ext_zbkx: false,
        ext_zbs: false,
        ext_zfh: false,
        ext_zfhmin: false,
        ext_ssvnapot: false,
        ext_svadu: false,
        ext_sstc: false,
    }
}

#[test]
fn test_ext_mul_rejected_without_m() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[2] = 6;
    cpu.gpr[3] = 7;
    let exit = run_rv_with_cfg(&mut cpu, mul(1, 2, 3), cfg_rv64i_only());
    assert_eq!(exit, EXCP_UNDEF as usize);
}

#[test]
fn test_ext_mul_accepted_with_m() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[2] = 6;
    cpu.gpr[3] = 7;
    let cfg = RiscvCfg {
        misa: MisaExt::I.union(MisaExt::M),
        ..cfg_rv64i_only()
    };
    run_rv_with_cfg(&mut cpu, mul(1, 2, 3), cfg);
    assert_eq!(cpu.gpr[1], 42);
}

#[test]
fn test_ext_div_rejected_without_m() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[2] = 42;
    cpu.gpr[3] = 6;
    let exit = run_rv_with_cfg(&mut cpu, div_rv(1, 2, 3), cfg_rv64i_only());
    assert_eq!(exit, EXCP_UNDEF as usize);
}

#[test]
fn test_ext_mulw_rejected_without_m() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[2] = 3;
    cpu.gpr[3] = 4;
    let exit = run_rv_with_cfg(&mut cpu, mulw(1, 2, 3), cfg_rv64i_only());
    assert_eq!(exit, EXCP_UNDEF as usize);
}

#[test]
fn test_ext_lr_w_rejected_without_a() {
    let mut cpu = RiscvCpu::new();
    let exit = run_rv_with_cfg(&mut cpu, lr_w(1, 2), cfg_rv64i_only());
    assert_eq!(exit, EXCP_UNDEF as usize);
}

#[test]
fn test_ext_amoswap_w_rejected_without_a() {
    let mut cpu = RiscvCpu::new();
    let exit = run_rv_with_cfg(&mut cpu, amoswap_w(1, 2, 3), cfg_rv64i_only());
    assert_eq!(exit, EXCP_UNDEF as usize);
}

#[test]
fn test_ext_csrrw_rejected_without_zicsr() {
    let mut cpu = RiscvCpu::new();
    // CSRRW x1, fflags(0x001), x0
    let exit = run_rv_with_cfg(&mut cpu, csrrw(1, 0, 0x001), cfg_rv64i_only());
    assert_eq!(exit, EXCP_UNDEF as usize);
}

#[test]
fn test_ext_csrrw_accepted_with_zicsr() {
    let mut cpu = RiscvCpu::new();
    let cfg = RiscvCfg {
        ext_zicsr: true,
        ..cfg_rv64i_only()
    };
    // CSRRW x1, fflags(0x001), x0 — reads fflags
    run_rv_with_cfg(&mut cpu, csrrw(1, 0, 0x001), cfg);
    assert_eq!(cpu.gpr[1], 0);
}

#[test]
fn test_ext_fadd_s_rejected_without_f() {
    let mut cpu = RiscvCpu::new();
    let exit = run_rv_with_cfg(&mut cpu, fadd_s(1, 2, 3, 0), cfg_rv64i_only());
    assert_eq!(exit, EXCP_UNDEF as usize);
}

#[test]
fn test_ext_c_insn_rejected_without_c() {
    let mut cpu = RiscvCpu::new();
    let cfg = RiscvCfg {
        misa: MisaExt::I.union(MisaExt::M),
        ..cfg_rv64i_only()
    };
    // C.LI x1, 42 — should fail without C extension
    let exit = run_rvc_with_cfg(&mut cpu, c_li(1, 42), cfg);
    assert_eq!(exit, EXCP_UNDEF as usize);
}

// ── Zicbom / Zicboz — CBO instruction encoders ─────

const OP_MISC_MEM: u32 = 0b0001111;

fn cbo_inval(rs1: u32) -> u32 {
    (0x000 << 20) | (rs1 << 15) | (0b010 << 12) | OP_MISC_MEM
}
fn cbo_clean(rs1: u32) -> u32 {
    (0x001 << 20) | (rs1 << 15) | (0b010 << 12) | OP_MISC_MEM
}
fn cbo_flush(rs1: u32) -> u32 {
    (0x002 << 20) | (rs1 << 15) | (0b010 << 12) | OP_MISC_MEM
}
fn cbo_zero(rs1: u32) -> u32 {
    (0x004 << 20) | (rs1 << 15) | (0b010 << 12) | OP_MISC_MEM
}

#[test]
fn test_cbo_clean_accepted() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[5] = 0x1000;
    let cfg = RiscvCfg {
        ext_zicbom: true,
        ..cfg_rv64i_only()
    };
    run_rv_with_cfg(&mut cpu, cbo_clean(5), cfg);
    assert_eq!(cpu.gpr[5], 0x1000);
}

#[test]
fn test_cbo_flush_accepted() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[5] = 0x2000;
    let cfg = RiscvCfg {
        ext_zicbom: true,
        ..cfg_rv64i_only()
    };
    run_rv_with_cfg(&mut cpu, cbo_flush(5), cfg);
    assert_eq!(cpu.gpr[5], 0x2000);
}

#[test]
fn test_cbo_inval_accepted() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[5] = 0x3000;
    let cfg = RiscvCfg {
        ext_zicbom: true,
        ..cfg_rv64i_only()
    };
    run_rv_with_cfg(&mut cpu, cbo_inval(5), cfg);
    assert_eq!(cpu.gpr[5], 0x3000);
}

#[test]
fn test_cbo_zero_accepted() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[5] = 0x4000;
    let cfg = RiscvCfg {
        ext_zicboz: true,
        ..cfg_rv64i_only()
    };
    run_rv_with_cfg(&mut cpu, cbo_zero(5), cfg);
    assert_eq!(cpu.gpr[5], 0x4000);
}

#[test]
fn test_cbo_clean_rejected_without_zicbom() {
    let mut cpu = RiscvCpu::new();
    let exit = run_rv_with_cfg(&mut cpu, cbo_clean(5), cfg_rv64i_only());
    assert_eq!(exit, EXCP_UNDEF as usize);
}

#[test]
fn test_cbo_flush_rejected_without_zicbom() {
    let mut cpu = RiscvCpu::new();
    let exit = run_rv_with_cfg(&mut cpu, cbo_flush(5), cfg_rv64i_only());
    assert_eq!(exit, EXCP_UNDEF as usize);
}

#[test]
fn test_cbo_inval_rejected_without_zicbom() {
    let mut cpu = RiscvCpu::new();
    let exit = run_rv_with_cfg(&mut cpu, cbo_inval(5), cfg_rv64i_only());
    assert_eq!(exit, EXCP_UNDEF as usize);
}

#[test]
fn test_cbo_zero_rejected_without_zicboz() {
    let mut cpu = RiscvCpu::new();
    let exit = run_rv_with_cfg(&mut cpu, cbo_zero(5), cfg_rv64i_only());
    assert_eq!(exit, EXCP_UNDEF as usize);
}

#[test]
fn test_cbo_clean_no_gpr_change() {
    let mut cpu = RiscvCpu::new();
    for i in 1..32 {
        cpu.gpr[i] = (i as u64) * 0x111;
    }
    let cfg = RiscvCfg {
        ext_zicbom: true,
        ..cfg_rv64i_only()
    };
    run_rv_with_cfg(&mut cpu, cbo_clean(10), cfg);
    for i in 1..32 {
        assert_eq!(
            cpu.gpr[i],
            (i as u64) * 0x111,
            "gpr[{i}] modified by cbo.clean"
        );
    }
}

#[test]
fn test_cbo_zero_no_gpr_change() {
    let mut cpu = RiscvCpu::new();
    for i in 1..32 {
        cpu.gpr[i] = (i as u64) * 0x222;
    }
    let cfg = RiscvCfg {
        ext_zicboz: true,
        ..cfg_rv64i_only()
    };
    run_rv_with_cfg(&mut cpu, cbo_zero(10), cfg);
    for i in 1..32 {
        assert_eq!(
            cpu.gpr[i],
            (i as u64) * 0x222,
            "gpr[{i}] modified by cbo.zero"
        );
    }
}
