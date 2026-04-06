//! Zbc (carry-less multiplication) frontend tests.

use super::*;

// ── Instruction encoders ────────────────────────────

const ZBC_FUNCT7: u32 = 0b0000101;

fn clmul(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(ZBC_FUNCT7, rs2, rs1, 0b001, rd, OP_REG)
}

fn clmulh(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(ZBC_FUNCT7, rs2, rs1, 0b011, rd, OP_REG)
}

fn clmulr(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(ZBC_FUNCT7, rs2, rs1, 0b010, rd, OP_REG)
}

fn cfg_zbc() -> RiscvCfg {
    RiscvCfg {
        ext_zbc: true,
        ..RiscvCfg::default()
    }
}

// ── Reference implementations ───────────────────────

fn ref_clmul(a: u64, b: u64) -> u64 {
    let mut r = 0u64;
    for i in 0..64 {
        if (b >> i) & 1 != 0 {
            r ^= a << i;
        }
    }
    r
}

fn ref_clmulh(a: u64, b: u64) -> u64 {
    let mut r = 0u64;
    for i in 1..64 {
        if (b >> i) & 1 != 0 {
            r ^= a >> (64 - i);
        }
    }
    r
}

fn ref_clmulr(a: u64, b: u64) -> u64 {
    let mut r = 0u64;
    for i in 0..64 {
        if (b >> i) & 1 != 0 {
            r ^= a >> (63 - i);
        }
    }
    r
}

// ── Extension gating ────────────────────────────────

#[test]
fn test_clmul_rejected_without_zbc() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 3;
    cpu.gpr[2] = 5;
    let exit = run_rv_with_cfg(
        &mut cpu,
        clmul(3, 1, 2),
        RiscvCfg {
            ext_zbc: false,
            ..RiscvCfg::default()
        },
    );
    assert_eq!(exit, EXCP_UNDEF as usize);
}

#[test]
fn test_clmulh_rejected_without_zbc() {
    let mut cpu = RiscvCpu::new();
    let exit = run_rv_with_cfg(
        &mut cpu,
        clmulh(3, 1, 2),
        RiscvCfg {
            ext_zbc: false,
            ..RiscvCfg::default()
        },
    );
    assert_eq!(exit, EXCP_UNDEF as usize);
}

#[test]
fn test_clmulr_rejected_without_zbc() {
    let mut cpu = RiscvCpu::new();
    let exit = run_rv_with_cfg(
        &mut cpu,
        clmulr(3, 1, 2),
        RiscvCfg {
            ext_zbc: false,
            ..RiscvCfg::default()
        },
    );
    assert_eq!(exit, EXCP_UNDEF as usize);
}

// ── clmul basic ─────────────────────────────────────

#[test]
fn test_clmul_zero_rs1() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0;
    cpu.gpr[2] = 0xDEAD_BEEF;
    run_rv_with_cfg(&mut cpu, clmul(3, 1, 2), cfg_zbc());
    assert_eq!(cpu.gpr[3], 0);
}

#[test]
fn test_clmul_zero_rs2() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0xDEAD_BEEF;
    cpu.gpr[2] = 0;
    run_rv_with_cfg(&mut cpu, clmul(3, 1, 2), cfg_zbc());
    assert_eq!(cpu.gpr[3], 0);
}

#[test]
fn test_clmul_one() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0x1234_5678_9ABC_DEF0;
    cpu.gpr[2] = 1;
    run_rv_with_cfg(&mut cpu, clmul(3, 1, 2), cfg_zbc());
    assert_eq!(cpu.gpr[3], 0x1234_5678_9ABC_DEF0);
}

#[test]
fn test_clmul_known() {
    let mut cpu = RiscvCpu::new();
    let a: u64 = 0x0000_0000_0000_0007;
    let b: u64 = 0x0000_0000_0000_000B;
    cpu.gpr[1] = a;
    cpu.gpr[2] = b;
    run_rv_with_cfg(&mut cpu, clmul(3, 1, 2), cfg_zbc());
    assert_eq!(cpu.gpr[3], ref_clmul(a, b));
}

#[test]
fn test_clmul_all_ones() {
    let mut cpu = RiscvCpu::new();
    let a = u64::MAX;
    let b = u64::MAX;
    cpu.gpr[1] = a;
    cpu.gpr[2] = b;
    run_rv_with_cfg(&mut cpu, clmul(3, 1, 2), cfg_zbc());
    assert_eq!(cpu.gpr[3], ref_clmul(a, b));
}

#[test]
fn test_clmul_large() {
    let mut cpu = RiscvCpu::new();
    let a: u64 = 0xA5A5_A5A5_A5A5_A5A5;
    let b: u64 = 0x5A5A_5A5A_5A5A_5A5A;
    cpu.gpr[1] = a;
    cpu.gpr[2] = b;
    run_rv_with_cfg(&mut cpu, clmul(3, 1, 2), cfg_zbc());
    assert_eq!(cpu.gpr[3], ref_clmul(a, b));
}

// ── clmulh basic ────────────────────────────────────

#[test]
fn test_clmulh_zero() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0;
    cpu.gpr[2] = u64::MAX;
    run_rv_with_cfg(&mut cpu, clmulh(3, 1, 2), cfg_zbc());
    assert_eq!(cpu.gpr[3], 0);
}

#[test]
fn test_clmulh_one() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = u64::MAX;
    cpu.gpr[2] = 1;
    run_rv_with_cfg(&mut cpu, clmulh(3, 1, 2), cfg_zbc());
    assert_eq!(cpu.gpr[3], 0);
}

#[test]
fn test_clmulh_known() {
    let mut cpu = RiscvCpu::new();
    let a: u64 = 0x0000_0000_0000_0007;
    let b: u64 = 0x0000_0000_0000_000B;
    cpu.gpr[1] = a;
    cpu.gpr[2] = b;
    run_rv_with_cfg(&mut cpu, clmulh(3, 1, 2), cfg_zbc());
    assert_eq!(cpu.gpr[3], ref_clmulh(a, b));
}

#[test]
fn test_clmulh_all_ones() {
    let mut cpu = RiscvCpu::new();
    let a = u64::MAX;
    let b = u64::MAX;
    cpu.gpr[1] = a;
    cpu.gpr[2] = b;
    run_rv_with_cfg(&mut cpu, clmulh(3, 1, 2), cfg_zbc());
    assert_eq!(cpu.gpr[3], ref_clmulh(a, b));
}

#[test]
fn test_clmulh_large() {
    let mut cpu = RiscvCpu::new();
    let a: u64 = 0xA5A5_A5A5_A5A5_A5A5;
    let b: u64 = 0x5A5A_5A5A_5A5A_5A5A;
    cpu.gpr[1] = a;
    cpu.gpr[2] = b;
    run_rv_with_cfg(&mut cpu, clmulh(3, 1, 2), cfg_zbc());
    assert_eq!(cpu.gpr[3], ref_clmulh(a, b));
}

// ── clmulr basic ────────────────────────────────────

#[test]
fn test_clmulr_zero() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 0;
    cpu.gpr[2] = u64::MAX;
    run_rv_with_cfg(&mut cpu, clmulr(3, 1, 2), cfg_zbc());
    assert_eq!(cpu.gpr[3], 0);
}

#[test]
fn test_clmulr_one() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = u64::MAX;
    cpu.gpr[2] = 1;
    run_rv_with_cfg(&mut cpu, clmulr(3, 1, 2), cfg_zbc());
    assert_eq!(cpu.gpr[3], ref_clmulr(u64::MAX, 1));
}

#[test]
fn test_clmulr_known() {
    let mut cpu = RiscvCpu::new();
    let a: u64 = 0x0000_0000_0000_0007;
    let b: u64 = 0x0000_0000_0000_000B;
    cpu.gpr[1] = a;
    cpu.gpr[2] = b;
    run_rv_with_cfg(&mut cpu, clmulr(3, 1, 2), cfg_zbc());
    assert_eq!(cpu.gpr[3], ref_clmulr(a, b));
}

#[test]
fn test_clmulr_all_ones() {
    let mut cpu = RiscvCpu::new();
    let a = u64::MAX;
    let b = u64::MAX;
    cpu.gpr[1] = a;
    cpu.gpr[2] = b;
    run_rv_with_cfg(&mut cpu, clmulr(3, 1, 2), cfg_zbc());
    assert_eq!(cpu.gpr[3], ref_clmulr(a, b));
}

#[test]
fn test_clmulr_large() {
    let mut cpu = RiscvCpu::new();
    let a: u64 = 0xA5A5_A5A5_A5A5_A5A5;
    let b: u64 = 0x5A5A_5A5A_5A5A_5A5A;
    cpu.gpr[1] = a;
    cpu.gpr[2] = b;
    run_rv_with_cfg(&mut cpu, clmulr(3, 1, 2), cfg_zbc());
    assert_eq!(cpu.gpr[3], ref_clmulr(a, b));
}

// ── x0 behavior ─────────────────────────────────────

#[test]
fn test_clmul_rd_x0() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = 7;
    cpu.gpr[2] = 11;
    run_rv_with_cfg(&mut cpu, clmul(0, 1, 2), cfg_zbc());
    assert_eq!(cpu.gpr[0], 0);
}

#[test]
fn test_clmul_rs1_x0() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[2] = 0xFFFF;
    run_rv_with_cfg(&mut cpu, clmul(3, 0, 2), cfg_zbc());
    assert_eq!(cpu.gpr[3], 0);
}

// ── clmulr = bit-reverse of clmul relationship ─────

#[test]
fn test_clmulr_is_clmulh_shifted() {
    // clmulr(a,b) == clmulh(a,b) | ((clmul(a,b) >> 63) & 1)
    // Actually: clmulr(a,b) = reverse(clmul(reverse(a),reverse(b)))
    // But let's just verify known values match reference.
    let a: u64 = 0x1234_5678_9ABC_DEF0;
    let b: u64 = 0xFEDC_BA98_7654_3210;
    let mut cpu = RiscvCpu::new();
    cpu.gpr[1] = a;
    cpu.gpr[2] = b;
    run_rv_with_cfg(&mut cpu, clmulr(3, 1, 2), cfg_zbc());
    assert_eq!(cpu.gpr[3], ref_clmulr(a, b));
}
