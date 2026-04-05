//! Zba (Address Computation) extension tests.

use machina_accel::code_buffer::CodeBuffer;
use machina_accel::ir::tb::EXCP_UNDEF;
use machina_accel::ir::Context;
use machina_accel::translate::translate_and_execute;
use machina_accel::HostCodeGen;
use machina_accel::X86_64CodeGen;
use machina_guest_riscv::riscv::cpu::RiscvCpu;
use machina_guest_riscv::riscv::ext::{MisaExt, RiscvCfg};
use machina_guest_riscv::riscv::{RiscvDisasContext, RiscvTranslator};
use machina_guest_riscv::translator_loop;

// ── Encoding helpers ─────────────────────────────────────────

const OP_REG: u32 = 0b0110011;
const OP_REG32: u32 = 0b0111011;
const OP_IMM32: u32 = 0b0011011;

fn rv_r(f7: u32, rs2: u32, rs1: u32, f3: u32, rd: u32, op: u32) -> u32 {
    (f7 << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op
}

fn sh1add(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0b0010000, rs2, rs1, 0b010, rd, OP_REG)
}

fn sh2add(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0b0010000, rs2, rs1, 0b100, rd, OP_REG)
}

fn sh3add(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0b0010000, rs2, rs1, 0b110, rd, OP_REG)
}

fn add_uw(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0b0000100, rs2, rs1, 0b000, rd, OP_REG32)
}

fn slli_uw(rd: u32, rs1: u32, shamt: u32) -> u32 {
    // funct6=000010, shamt[5:0], rs1, funct3=001, rd, 0011011
    let f6 = 0b000010u32;
    (f6 << 26)
        | (shamt << 20)
        | (rs1 << 15)
        | (0b001 << 12)
        | (rd << 7)
        | OP_IMM32
}

// ── Test runner ──────────────────────────────────────────────

fn cfg_zba() -> RiscvCfg {
    RiscvCfg {
        ext_zba: true,
        ..RiscvCfg::default()
    }
}

fn run_zba(cpu: &mut RiscvCpu, insn: u32) -> usize {
    run_zba_cfg(cpu, insn, cfg_zba())
}

fn run_zba_cfg(cpu: &mut RiscvCpu, insn: u32, cfg: RiscvCfg) -> usize {
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

// ── sh1add tests ─────────────────────────────────────────────

#[test]
fn test_sh1add_basic() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 10;
    cpu.gpr[2] = 100;
    run_zba(&mut cpu, sh1add(3, 1, 2));
    // rd = (10 << 1) + 100 = 120
    assert_eq!(cpu.gpr[3], 120);
}

#[test]
fn test_sh1add_large() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0x4000_0000_0000_0000;
    cpu.gpr[2] = 1;
    run_zba(&mut cpu, sh1add(3, 1, 2));
    // (0x4000... << 1) + 1 = 0x8000...001
    assert_eq!(cpu.gpr[3], 0x8000_0000_0000_0001);
}

// ── sh2add tests ─────────────────────────────────────────────

#[test]
fn test_sh2add_basic() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 5;
    cpu.gpr[2] = 0x1000;
    run_zba(&mut cpu, sh2add(3, 1, 2));
    // rd = (5 << 2) + 0x1000 = 20 + 4096 = 4116
    assert_eq!(cpu.gpr[3], 4116);
}

#[test]
fn test_sh2add_zero() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0;
    cpu.gpr[2] = 42;
    run_zba(&mut cpu, sh2add(3, 1, 2));
    assert_eq!(cpu.gpr[3], 42);
}

// ── sh3add tests ─────────────────────────────────────────────

#[test]
fn test_sh3add_basic() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 3;
    cpu.gpr[2] = 0x2000;
    run_zba(&mut cpu, sh3add(3, 1, 2));
    // rd = (3 << 3) + 0x2000 = 24 + 8192 = 8216
    assert_eq!(cpu.gpr[3], 8216);
}

#[test]
fn test_sh3add_negative() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = (-1i64) as u64; // all 1s
    cpu.gpr[2] = 0;
    run_zba(&mut cpu, sh3add(3, 1, 2));
    // (-1 << 3) + 0 = -8
    assert_eq!(cpu.gpr[3], (-8i64) as u64);
}

// ── add.uw tests ─────────────────────────────────────────────

#[test]
fn test_add_uw_basic() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0xFFFF_FFFF_FFFF_FFFF;
    cpu.gpr[2] = 1;
    run_zba(&mut cpu, add_uw(3, 1, 2));
    // zext32(0xFFFF...) = 0x0000_0000_FFFF_FFFF, + 1
    assert_eq!(cpu.gpr[3], 0x1_0000_0000);
}

#[test]
fn test_add_uw_zero_ext() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0xDEAD_BEEF_CAFE_BABE;
    cpu.gpr[2] = 0;
    run_zba(&mut cpu, add_uw(3, 1, 2));
    // zext32(0xCAFE_BABE) = 0x0000_0000_CAFE_BABE
    assert_eq!(cpu.gpr[3], 0x0000_0000_CAFE_BABE);
}

#[test]
fn test_add_uw_with_base() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0x8000_0000; // bit 31 set
    cpu.gpr[2] = 0x1000_0000_0000_0000;
    run_zba(&mut cpu, add_uw(3, 1, 2));
    // zext32(0x8000_0000) + 0x1000... = 0x1000_0000_8000_0000
    assert_eq!(cpu.gpr[3], 0x1000_0000_8000_0000);
}

// ── slli.uw tests ────────────────────────────────────────────

#[test]
fn test_slli_uw_basic() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0xFFFF_FFFF_FFFF_FFFF;
    run_zba(&mut cpu, slli_uw(3, 1, 4));
    // zext32(0xFFFF...) << 4 = 0xFFFF_FFFF << 4 = 0xF_FFFF_FFF0
    assert_eq!(cpu.gpr[3], 0x0000_000F_FFFF_FFF0);
}

#[test]
fn test_slli_uw_zero_shift() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0xDEAD_BEEF_CAFE_BABE;
    run_zba(&mut cpu, slli_uw(3, 1, 0));
    // zext32(0xCAFE_BABE) << 0 = 0xCAFE_BABE
    assert_eq!(cpu.gpr[3], 0x0000_0000_CAFE_BABE);
}

#[test]
fn test_slli_uw_large_shift() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 1;
    run_zba(&mut cpu, slli_uw(3, 1, 32));
    // zext32(1) << 32 = 0x1_0000_0000
    assert_eq!(cpu.gpr[3], 0x0000_0001_0000_0000);
}

// ── Extension gate tests ─────────────────────────────────────

#[test]
fn test_sh1add_rejected_without_zba() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 10;
    cpu.gpr[2] = 100;
    let exit = run_zba_cfg(&mut cpu, sh1add(3, 1, 2), RiscvCfg::default());
    assert_eq!(exit, EXCP_UNDEF as usize);
}

#[test]
fn test_add_uw_rejected_without_zba() {
    let mut cpu = RiscvCpu::new();
    let exit = run_zba_cfg(&mut cpu, add_uw(3, 1, 2), RiscvCfg::default());
    assert_eq!(exit, EXCP_UNDEF as usize);
}

#[test]
fn test_slli_uw_rejected_without_zba() {
    let mut cpu = RiscvCpu::new();
    let exit = run_zba_cfg(&mut cpu, slli_uw(3, 1, 4), RiscvCfg::default());
    assert_eq!(exit, EXCP_UNDEF as usize);
}

// ── x0 behavior ──────────────────────────────────────────────

#[test]
fn test_sh1add_rd_x0() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 10;
    cpu.gpr[2] = 100;
    run_zba(&mut cpu, sh1add(0, 1, 2));
    assert_eq!(cpu.gpr[0], 0); // x0 stays zero
}

#[test]
fn test_sh2add_rs1_x0() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[2] = 42;
    run_zba(&mut cpu, sh2add(3, 0, 2));
    // (0 << 2) + 42 = 42
    assert_eq!(cpu.gpr[3], 42);
}
