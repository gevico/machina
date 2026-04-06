//! Zbs (Single-Bit Operations) extension tests.

use machina_accel::code_buffer::CodeBuffer;
use machina_accel::ir::tb::EXCP_UNDEF;
use machina_accel::ir::Context;
use machina_accel::translate::translate_and_execute;
use machina_accel::HostCodeGen;
use machina_accel::X86_64CodeGen;
use machina_guest_riscv::riscv::cpu::RiscvCpu;
use machina_guest_riscv::riscv::ext::RiscvCfg;
use machina_guest_riscv::riscv::{RiscvDisasContext, RiscvTranslator};
use machina_guest_riscv::translator_loop;

// ── Encoding helpers ─────────────────────────────────────────

const OP_REG: u32 = 0b0110011;
const OP_IMM: u32 = 0b0010011;

fn rv_r(f7: u32, rs2: u32, rs1: u32, f3: u32, rd: u32, op: u32) -> u32 {
    (f7 << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op
}

// R-type encoders
fn bclr(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0b0100100, rs2, rs1, 0b001, rd, OP_REG)
}
fn bext(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0b0100100, rs2, rs1, 0b101, rd, OP_REG)
}
fn binv(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0b0110100, rs2, rs1, 0b001, rd, OP_REG)
}
fn bset(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0b0010100, rs2, rs1, 0b001, rd, OP_REG)
}

// Immediate encoders (funct6 + 6-bit shamt)
fn bclri(rd: u32, rs1: u32, shamt: u32) -> u32 {
    let f6 = 0b010010u32;
    (f6 << 26)
        | (shamt << 20)
        | (rs1 << 15)
        | (0b001 << 12)
        | (rd << 7)
        | OP_IMM
}
fn bexti(rd: u32, rs1: u32, shamt: u32) -> u32 {
    let f6 = 0b010010u32;
    (f6 << 26)
        | (shamt << 20)
        | (rs1 << 15)
        | (0b101 << 12)
        | (rd << 7)
        | OP_IMM
}
fn binvi(rd: u32, rs1: u32, shamt: u32) -> u32 {
    let f6 = 0b011010u32;
    (f6 << 26)
        | (shamt << 20)
        | (rs1 << 15)
        | (0b001 << 12)
        | (rd << 7)
        | OP_IMM
}
fn bseti(rd: u32, rs1: u32, shamt: u32) -> u32 {
    let f6 = 0b001010u32;
    (f6 << 26)
        | (shamt << 20)
        | (rs1 << 15)
        | (0b001 << 12)
        | (rd << 7)
        | OP_IMM
}

// ── Test runner ──────────────────────────────────────────────

fn cfg_zbs() -> RiscvCfg {
    RiscvCfg {
        ext_zbs: true,
        ..RiscvCfg::default()
    }
}

fn run_zbs(cpu: &mut RiscvCpu, insn: u32) -> usize {
    run_zbs_cfg(cpu, insn, cfg_zbs())
}

fn run_zbs_cfg(cpu: &mut RiscvCpu, insn: u32, cfg: RiscvCfg) -> usize {
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

// ── bclr tests ──────────────────────────────────────────────

#[test]
fn test_bclr_bit0() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0xFF;
    cpu.gpr[2] = 0; // clear bit 0
    run_zbs(&mut cpu, bclr(3, 1, 2));
    assert_eq!(cpu.gpr[3], 0xFE);
}

#[test]
fn test_bclr_bit63() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = u64::MAX;
    cpu.gpr[2] = 63; // clear bit 63
    run_zbs(&mut cpu, bclr(3, 1, 2));
    assert_eq!(cpu.gpr[3], u64::MAX >> 1);
}

#[test]
fn test_bclr_already_clear() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0;
    cpu.gpr[2] = 5;
    run_zbs(&mut cpu, bclr(3, 1, 2));
    assert_eq!(cpu.gpr[3], 0);
}

// ── bclri tests ─────────────────────────────────────────────

#[test]
fn test_bclri_bit4() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0xFF;
    run_zbs(&mut cpu, bclri(3, 1, 4));
    assert_eq!(cpu.gpr[3], 0xEF);
}

#[test]
fn test_bclri_bit63() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = u64::MAX;
    run_zbs(&mut cpu, bclri(3, 1, 63));
    assert_eq!(cpu.gpr[3], u64::MAX >> 1);
}

// ── bext tests ──────────────────────────────────────────────

#[test]
fn test_bext_set() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0b1010;
    cpu.gpr[2] = 1; // extract bit 1
    run_zbs(&mut cpu, bext(3, 1, 2));
    assert_eq!(cpu.gpr[3], 1);
}

#[test]
fn test_bext_clear() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0b1010;
    cpu.gpr[2] = 0; // extract bit 0
    run_zbs(&mut cpu, bext(3, 1, 2));
    assert_eq!(cpu.gpr[3], 0);
}

#[test]
fn test_bext_bit63() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 1u64 << 63;
    cpu.gpr[2] = 63;
    run_zbs(&mut cpu, bext(3, 1, 2));
    assert_eq!(cpu.gpr[3], 1);
}

#[test]
fn test_bext_bit0() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0xFFFF_FFFF_FFFF_FFFE;
    cpu.gpr[2] = 0;
    run_zbs(&mut cpu, bext(3, 1, 2));
    assert_eq!(cpu.gpr[3], 0);
}

// ── bexti tests ─────────────────────────────────────────────

#[test]
fn test_bexti_set() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0b10000;
    run_zbs(&mut cpu, bexti(3, 1, 4));
    assert_eq!(cpu.gpr[3], 1);
}

#[test]
fn test_bexti_clear() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0b01111;
    run_zbs(&mut cpu, bexti(3, 1, 4));
    assert_eq!(cpu.gpr[3], 0);
}

#[test]
fn test_bexti_bit63() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = u64::MAX;
    run_zbs(&mut cpu, bexti(3, 1, 63));
    assert_eq!(cpu.gpr[3], 1);
}

// ── binv tests ──────────────────────────────────────────────

#[test]
fn test_binv_set_to_clear() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0xFF;
    cpu.gpr[2] = 0; // invert bit 0: 0xFF → 0xFE
    run_zbs(&mut cpu, binv(3, 1, 2));
    assert_eq!(cpu.gpr[3], 0xFE);
}

#[test]
fn test_binv_clear_to_set() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0;
    cpu.gpr[2] = 5; // invert bit 5: 0 → 0x20
    run_zbs(&mut cpu, binv(3, 1, 2));
    assert_eq!(cpu.gpr[3], 0x20);
}

#[test]
fn test_binv_bit63() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0;
    cpu.gpr[2] = 63;
    run_zbs(&mut cpu, binv(3, 1, 2));
    assert_eq!(cpu.gpr[3], 1u64 << 63);
}

// ── binvi tests ─────────────────────────────────────────────

#[test]
fn test_binvi_toggle() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0b1111;
    run_zbs(&mut cpu, binvi(3, 1, 2));
    // invert bit 2: 0b1111 → 0b1011
    assert_eq!(cpu.gpr[3], 0b1011);
}

#[test]
fn test_binvi_bit63() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = u64::MAX;
    run_zbs(&mut cpu, binvi(3, 1, 63));
    assert_eq!(cpu.gpr[3], u64::MAX ^ (1u64 << 63));
}

// ── bset tests ──────────────────────────────────────────────

#[test]
fn test_bset_bit0() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0;
    cpu.gpr[2] = 0;
    run_zbs(&mut cpu, bset(3, 1, 2));
    assert_eq!(cpu.gpr[3], 1);
}

#[test]
fn test_bset_bit63() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0;
    cpu.gpr[2] = 63;
    run_zbs(&mut cpu, bset(3, 1, 2));
    assert_eq!(cpu.gpr[3], 1u64 << 63);
}

#[test]
fn test_bset_already_set() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = u64::MAX;
    cpu.gpr[2] = 31;
    run_zbs(&mut cpu, bset(3, 1, 2));
    assert_eq!(cpu.gpr[3], u64::MAX);
}

// ── bseti tests ─────────────────────────────────────────────

#[test]
fn test_bseti_bit0() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0;
    run_zbs(&mut cpu, bseti(3, 1, 0));
    assert_eq!(cpu.gpr[3], 1);
}

#[test]
fn test_bseti_bit63() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0;
    run_zbs(&mut cpu, bseti(3, 1, 63));
    assert_eq!(cpu.gpr[3], 1u64 << 63);
}

#[test]
fn test_bseti_or_semantics() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0xAA;
    run_zbs(&mut cpu, bseti(3, 1, 4));
    // 0xAA | (1 << 4) = 0xBA
    assert_eq!(cpu.gpr[3], 0xBA);
}

// ── Extension gate tests ─────────────────────────────────────

#[test]
fn test_bclr_rejected_without_zbs() {
    let mut cpu = RiscvCpu::new();
    let exit = run_zbs_cfg(
        &mut cpu,
        bclr(3, 1, 2),
        RiscvCfg {
            ext_zbs: false,
            ..RiscvCfg::default()
        },
    );
    assert_eq!(exit, EXCP_UNDEF as usize);
}

#[test]
fn test_bext_rejected_without_zbs() {
    let mut cpu = RiscvCpu::new();
    let exit = run_zbs_cfg(
        &mut cpu,
        bext(3, 1, 2),
        RiscvCfg {
            ext_zbs: false,
            ..RiscvCfg::default()
        },
    );
    assert_eq!(exit, EXCP_UNDEF as usize);
}

#[test]
fn test_bseti_rejected_without_zbs() {
    let mut cpu = RiscvCpu::new();
    let exit = run_zbs_cfg(
        &mut cpu,
        bseti(3, 1, 5),
        RiscvCfg {
            ext_zbs: false,
            ..RiscvCfg::default()
        },
    );
    assert_eq!(exit, EXCP_UNDEF as usize);
}

// ── x0 behavior ──────────────────────────────────────────────

#[test]
fn test_bset_rd_x0() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0;
    cpu.gpr[2] = 5;
    run_zbs(&mut cpu, bset(0, 1, 2));
    assert_eq!(cpu.gpr[0], 0); // x0 stays zero
}

#[test]
fn test_bext_rs1_x0() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[2] = 5;
    run_zbs(&mut cpu, bext(3, 0, 2));
    // (0 >> 5) & 1 = 0
    assert_eq!(cpu.gpr[3], 0);
}
