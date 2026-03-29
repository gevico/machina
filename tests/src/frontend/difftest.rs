//! Differential testing: compare tcg-rs RISC-V instruction
//! simulation against QEMU (qemu-riscv64 user-mode).
//!
//! For each test case we:
//! 1. Run the instruction through tcg-rs full pipeline
//! 2. Generate RISC-V assembly, cross-compile, run under
//!    qemu-riscv64, and parse the register dump
//! 3. Compare the specified output registers

use std::io::Write;
use std::process::Command;

use machina_backend::code_buffer::CodeBuffer;
use machina_backend::translate::translate_and_execute;
use machina_backend::HostCodeGen;
use machina_backend::X86_64CodeGen;
use machina_core::Context;
use machina_frontend::riscv::cpu::RiscvCpu;
use machina_frontend::riscv::ext::RiscvCfg;
use machina_frontend::riscv::{RiscvDisasContext, RiscvTranslator};
use machina_frontend::translator_loop;

// ── Instruction encoders (reused from mod.rs) ──────────────

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

const OP_LUI: u32 = 0b0110111;
const OP_IMM: u32 = 0b0010011;
const OP_REG: u32 = 0b0110011;
const OP_IMM32: u32 = 0b0011011;
const OP_REG32: u32 = 0b0111011;

fn lui(rd: u32, imm: i32) -> u32 {
    rv_u(imm, rd, OP_LUI)
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

// ── Difftest infrastructure ────────────────────────────────

/// A single ALU difftest case: set initial regs, run one
/// instruction, compare output register.
struct AluTest {
    name: &'static str,
    /// RISC-V assembly mnemonic for the test instruction.
    asm: String,
    /// Machine code for tcg-rs.
    insn: u32,
    /// (reg_index, value) pairs to initialize before the test.
    init: Vec<(usize, u64)>,
    /// Register index to check after execution.
    check_reg: usize,
}

/// A branch difftest case: set two source regs, execute a
/// branch, record whether it was taken via a result register.
struct BranchTest {
    name: &'static str,
    /// Branch mnemonic (e.g. "beq").
    mnemonic: &'static str,
    /// Machine code for the branch (tcg-rs uses offset=16).
    insn_fn: fn(u32, u32, i32) -> u32,
    rs1_val: u64,
    rs2_val: u64,
}

/// RV register ABI names for assembly generation.
const REG_NAME: [&str; 32] = [
    "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1",
    "a2", "a3", "a4", "a5", "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7",
    "s8", "s9", "s10", "s11", "t3", "t4", "t5", "t6",
];

/// Generate assembly source for an ALU difftest.
/// Uses x5(t0), x6(t1) as source regs, x7(t2) as dest.
/// x3(gp) is reserved for the save-area pointer.
fn gen_alu_asm(test: &AluTest) -> String {
    let mut asm =
        String::from(".global _start\n_start:\n    la gp, save_area\n");
    // Load initial register values
    for &(reg, val) in &test.init {
        assert_ne!(reg, 3, "x3 reserved for save area");
        asm.push_str(&format!("    li {}, {}\n", REG_NAME[reg], val as i64));
    }
    // Test instruction
    asm.push_str(&format!("    {}\n", test.asm));
    // Save all registers
    for i in 0..32 {
        asm.push_str(&format!("    sd {}, {}(gp)\n", REG_NAME[i], i * 8));
    }
    // write(1, save_area, 256)
    asm.push_str(
        "    li a7, 64\n\
         \x20   li a0, 1\n\
         \x20   mv a1, gp\n\
         \x20   li a2, 256\n\
         \x20   ecall\n\
         \x20   li a7, 93\n\
         \x20   li a0, 0\n\
         \x20   ecall\n\
         .bss\n\
         .align 3\n\
         save_area: .space 256\n",
    );
    asm
}

/// Generate assembly for a branch difftest.
/// Sets x5, x6 as source regs, branches, and records
/// taken=1 / not-taken=0 in x7.
fn gen_branch_asm(test: &BranchTest) -> String {
    let mut asm =
        String::from(".global _start\n_start:\n    la gp, save_area\n");
    asm.push_str(&format!(
        "    li t0, {}\n    li t1, {}\n",
        test.rs1_val as i64, test.rs2_val as i64
    ));
    asm.push_str(&format!(
        "    {} t0, t1, 1f\n\
         \x20   li t2, 0\n\
         \x20   j 2f\n\
         1:  li t2, 1\n\
         2:\n",
        test.mnemonic
    ));
    for i in 0..32 {
        asm.push_str(&format!("    sd {}, {}(gp)\n", REG_NAME[i], i * 8));
    }
    asm.push_str(
        "    li a7, 64\n\
         \x20   li a0, 1\n\
         \x20   mv a1, gp\n\
         \x20   li a2, 256\n\
         \x20   ecall\n\
         \x20   li a7, 93\n\
         \x20   li a0, 0\n\
         \x20   ecall\n\
         .bss\n\
         .align 3\n\
         save_area: .space 256\n",
    );
    asm
}

/// Cross-compile assembly and run under qemu-riscv64.
/// Returns the 32-element register array.
fn run_qemu(asm_src: &str) -> [u64; 32] {
    let dir = std::env::temp_dir();
    let id = std::process::id();
    let tid: u64 = unsafe { std::mem::transmute(std::thread::current().id()) };
    let tag = format!("difftest_{id}_{tid}");
    let s_path = dir.join(format!("{tag}.S"));
    let elf_path = dir.join(format!("{tag}.elf"));

    // Write assembly source
    {
        let mut f = std::fs::File::create(&s_path).unwrap();
        f.write_all(asm_src.as_bytes()).unwrap();
    }

    // Cross-compile
    let cc = Command::new("riscv64-linux-gnu-gcc")
        .args([
            "-nostdlib",
            "-static",
            "-o",
            elf_path.to_str().unwrap(),
            s_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run riscv64-linux-gnu-gcc");
    assert!(
        cc.status.success(),
        "gcc failed: {}",
        String::from_utf8_lossy(&cc.stderr)
    );

    // Run under QEMU
    let qemu = Command::new("qemu-riscv64")
        .arg(elf_path.to_str().unwrap())
        .output()
        .expect("failed to run qemu-riscv64");
    assert!(
        qemu.status.success(),
        "qemu-riscv64 exited with {:?}",
        qemu.status.code()
    );
    assert_eq!(
        qemu.stdout.len(),
        256,
        "expected 256 bytes, got {}",
        qemu.stdout.len()
    );

    // Parse register dump
    let mut regs = [0u64; 32];
    for i in 0..32 {
        let off = i * 8;
        regs[i] =
            u64::from_le_bytes(qemu.stdout[off..off + 8].try_into().unwrap());
    }

    // Cleanup
    let _ = std::fs::remove_file(&s_path);
    let _ = std::fs::remove_file(&elf_path);

    regs
}

/// Run a single instruction through tcg-rs and return the
/// full CPU state.
fn run_tcgrs(init: &[(usize, u64)], insns: &[u32]) -> RiscvCpu {
    let code: Vec<u8> = insns.iter().flat_map(|i| i.to_le_bytes()).collect();
    let guest_base = code.as_ptr();

    let mut backend = X86_64CodeGen::new();
    let mut buf = CodeBuffer::new(4096).unwrap();
    backend.emit_prologue(&mut buf);
    backend.emit_epilogue(&mut buf);

    let mut ctx = Context::new();
    backend.init_context(&mut ctx);

    let mut disas = RiscvDisasContext::new(0, guest_base, RiscvCfg::default());
    disas.base.max_insns = insns.len() as u32;
    translator_loop::<RiscvTranslator>(&mut disas, &mut ctx);

    let mut cpu = RiscvCpu::new();
    for &(reg, val) in init {
        cpu.gpr[reg] = val;
    }

    unsafe {
        translate_and_execute(
            &mut ctx,
            &backend,
            &mut buf,
            &mut cpu as *mut RiscvCpu as *mut u8,
        );
    }
    cpu
}

/// Run an ALU difftest: compare tcg-rs vs QEMU for one
/// instruction.
fn difftest_alu(test: &AluTest) {
    let asm = gen_alu_asm(test);
    let qemu_regs = run_qemu(&asm);
    let cpu = run_tcgrs(&test.init, &[test.insn]);
    let r = test.check_reg;
    assert_eq!(
        cpu.gpr[r], qemu_regs[r],
        "DIFFTEST FAIL [{}]: x{} tcg-rs={:#x} qemu={:#x}",
        test.name, r, cpu.gpr[r], qemu_regs[r]
    );
}

/// Run a branch difftest: compare taken/not-taken result.
/// QEMU side uses assembly with taken/not-taken paths.
/// tcg-rs side runs just the branch and checks the PC.
fn difftest_branch(test: &BranchTest) {
    let asm = gen_branch_asm(test);
    let qemu_regs = run_qemu(&asm);
    let qemu_taken = qemu_regs[7]; // x7 = t2

    // tcg-rs: run just the branch instruction.
    // If taken → PC = 0 + 16 = 16; if not taken → PC = 4.
    let branch_insn = (test.insn_fn)(5, 6, 16);
    let init = vec![(5, test.rs1_val), (6, test.rs2_val)];
    let cpu = run_tcgrs(&init, &[branch_insn]);
    let tcgrs_taken: u64 = if cpu.pc == 16 { 1 } else { 0 };

    assert_eq!(
        tcgrs_taken, qemu_taken,
        "DIFFTEST FAIL [{}]: tcg-rs_taken={} (pc={:#x}) \
         qemu_taken={}",
        test.name, tcgrs_taken, cpu.pc, qemu_taken
    );
}

// ── Edge-case values ───────────────────────────────────────

const V0: u64 = 0;
const V1: u64 = 1;
const VMAX: u64 = 0x7FFF_FFFF_FFFF_FFFF; // i64::MAX
const VMIN: u64 = 0x8000_0000_0000_0000; // i64::MIN
const VNEG1: u64 = 0xFFFF_FFFF_FFFF_FFFF; // -1
const V32MAX: u64 = 0x7FFF_FFFF; // i32::MAX
const V32MIN: u64 = 0xFFFF_FFFF_8000_0000; // i32::MIN sext
const V32FF: u64 = 0xFFFF_FFFF; // u32::MAX
const VPATTERN: u64 = 0xDEAD_BEEF_CAFE_BABE;

// ── R-type ALU difftests ───────────────────────────────────

/// Helper: build an R-type ALU test with two source values.
fn rtype_test(
    name: &'static str,
    mnemonic: &str,
    insn: u32,
    v1: u64,
    v2: u64,
) -> AluTest {
    AluTest {
        name,
        asm: format!("{} t2, t0, t1", mnemonic),
        insn,
        init: vec![(5, v1), (6, v2)],
        check_reg: 7,
    }
}

/// Helper: build an I-type ALU test (reg + imm12).
fn itype_test(
    name: &'static str,
    mnemonic: &str,
    insn: u32,
    v1: u64,
) -> AluTest {
    AluTest {
        name,
        asm: format!("{}", mnemonic),
        insn,
        init: vec![(5, v1)],
        check_reg: 7,
    }
}

#[test]
fn difftest_add() {
    let cases: Vec<(u64, u64)> = vec![
        (V0, V0),
        (V1, VNEG1),
        (VMAX, V1),
        (VMIN, VNEG1),
        (VPATTERN, V32FF),
        (V32MAX, V32MAX),
    ];
    for (a, b) in cases {
        difftest_alu(&rtype_test("add", "add", add(7, 5, 6), a, b));
    }
}

#[test]
fn difftest_sub() {
    let cases: Vec<(u64, u64)> = vec![
        (V0, V0),
        (V0, V1),
        (VMIN, V1),
        (VMAX, VNEG1),
        (VPATTERN, VPATTERN),
    ];
    for (a, b) in cases {
        difftest_alu(&rtype_test("sub", "sub", sub(7, 5, 6), a, b));
    }
}

#[test]
fn difftest_sll() {
    let cases: Vec<(u64, u64)> =
        vec![(V1, 0), (V1, 63), (VNEG1, 32), (VPATTERN, 4), (V32MAX, 1)];
    for (a, b) in cases {
        difftest_alu(&rtype_test("sll", "sll", sll(7, 5, 6), a, b));
    }
}

#[test]
fn difftest_srl() {
    let cases: Vec<(u64, u64)> = vec![
        (VNEG1, 0),
        (VNEG1, 1),
        (VNEG1, 63),
        (VPATTERN, 16),
        (VMIN, 32),
    ];
    for (a, b) in cases {
        difftest_alu(&rtype_test("srl", "srl", srl(7, 5, 6), a, b));
    }
}

#[test]
fn difftest_sra() {
    let cases: Vec<(u64, u64)> = vec![
        (VNEG1, 0),
        (VNEG1, 1),
        (VNEG1, 63),
        (VMIN, 32),
        (VMAX, 32),
        (VPATTERN, 8),
    ];
    for (a, b) in cases {
        difftest_alu(&rtype_test("sra", "sra", sra(7, 5, 6), a, b));
    }
}

#[test]
fn difftest_slt() {
    let cases: Vec<(u64, u64)> = vec![
        (V0, V0),
        (VNEG1, V0),
        (V0, VNEG1),
        (VMIN, VMAX),
        (VMAX, VMIN),
        (V1, V1),
    ];
    for (a, b) in cases {
        difftest_alu(&rtype_test("slt", "slt", slt(7, 5, 6), a, b));
    }
}

#[test]
fn difftest_sltu() {
    let cases: Vec<(u64, u64)> = vec![
        (V0, V0),
        (V0, V1),
        (V1, V0),
        (VNEG1, V0),
        (V0, VNEG1),
        (VMIN, VMAX),
    ];
    for (a, b) in cases {
        difftest_alu(&rtype_test("sltu", "sltu", sltu(7, 5, 6), a, b));
    }
}

#[test]
fn difftest_xor() {
    let cases: Vec<(u64, u64)> = vec![
        (VNEG1, VNEG1),
        (VNEG1, V0),
        (VPATTERN, VNEG1),
        (V32MAX, V32FF),
    ];
    for (a, b) in cases {
        difftest_alu(&rtype_test("xor", "xor", xor(7, 5, 6), a, b));
    }
}

#[test]
fn difftest_or() {
    let cases: Vec<(u64, u64)> =
        vec![(V0, V0), (VPATTERN, V0), (0xF0F0, 0x0F0F), (VMIN, VMAX)];
    for (a, b) in cases {
        difftest_alu(&rtype_test("or", "or", or(7, 5, 6), a, b));
    }
}

#[test]
fn difftest_and() {
    let cases: Vec<(u64, u64)> = vec![
        (VNEG1, VNEG1),
        (VNEG1, V0),
        (VPATTERN, V32FF),
        (0xFF00, 0x0FF0),
    ];
    for (a, b) in cases {
        difftest_alu(&rtype_test("and", "and", and(7, 5, 6), a, b));
    }
}

// ── I-type ALU difftests ───────────────────────────────────

#[test]
fn difftest_addi() {
    let cases: Vec<(u64, i32)> = vec![
        (V0, 0),
        (V0, 2047),
        (V0, -2048),
        (VNEG1, 1),
        (VMAX, 1),
        (VMIN, -1),
        (VPATTERN, -1),
    ];
    for (a, imm) in cases {
        difftest_alu(&itype_test(
            "addi",
            &format!("addi t2, t0, {imm}"),
            addi(7, 5, imm),
            a,
        ));
    }
}

#[test]
fn difftest_slti() {
    let cases: Vec<(u64, i32)> =
        vec![(V0, 0), (V0, 1), (VNEG1, 0), (VMAX, -1), (VMIN, 0)];
    for (a, imm) in cases {
        difftest_alu(&itype_test(
            "slti",
            &format!("slti t2, t0, {imm}"),
            slti(7, 5, imm),
            a,
        ));
    }
}

#[test]
fn difftest_sltiu() {
    let cases: Vec<(u64, i32)> =
        vec![(V0, 0), (V0, 1), (V1, 0), (VNEG1, -1), (VNEG1, 1)];
    for (a, imm) in cases {
        difftest_alu(&itype_test(
            "sltiu",
            &format!("sltiu t2, t0, {imm}"),
            sltiu(7, 5, imm),
            a,
        ));
    }
}

#[test]
fn difftest_xori() {
    let cases: Vec<(u64, i32)> = vec![(VNEG1, -1), (VPATTERN, 0x7FF), (V0, -1)];
    for (a, imm) in cases {
        difftest_alu(&itype_test(
            "xori",
            &format!("xori t2, t0, {imm}"),
            xori(7, 5, imm),
            a,
        ));
    }
}

#[test]
fn difftest_ori() {
    let cases: Vec<(u64, i32)> = vec![(V0, 0), (V0, -1), (VPATTERN, 0x0F)];
    for (a, imm) in cases {
        difftest_alu(&itype_test(
            "ori",
            &format!("ori t2, t0, {imm}"),
            ori(7, 5, imm),
            a,
        ));
    }
}

#[test]
fn difftest_andi() {
    let cases: Vec<(u64, i32)> =
        vec![(VNEG1, -1), (VNEG1, 0), (VPATTERN, 0xFF)];
    for (a, imm) in cases {
        difftest_alu(&itype_test(
            "andi",
            &format!("andi t2, t0, {imm}"),
            andi(7, 5, imm),
            a,
        ));
    }
}

#[test]
fn difftest_slli() {
    let cases: Vec<(u64, u32)> =
        vec![(V1, 0), (V1, 63), (VNEG1, 32), (VPATTERN, 4)];
    for (a, sh) in cases {
        difftest_alu(&itype_test(
            "slli",
            &format!("slli t2, t0, {sh}"),
            slli(7, 5, sh),
            a,
        ));
    }
}

#[test]
fn difftest_srli() {
    let cases: Vec<(u64, u32)> =
        vec![(VNEG1, 0), (VNEG1, 1), (VNEG1, 63), (VMIN, 32)];
    for (a, sh) in cases {
        difftest_alu(&itype_test(
            "srli",
            &format!("srli t2, t0, {sh}"),
            srli(7, 5, sh),
            a,
        ));
    }
}

#[test]
fn difftest_srai() {
    let cases: Vec<(u64, u32)> =
        vec![(VNEG1, 0), (VNEG1, 63), (VMIN, 1), (VMIN, 63), (VMAX, 32)];
    for (a, sh) in cases {
        difftest_alu(&itype_test(
            "srai",
            &format!("srai t2, t0, {sh}"),
            srai(7, 5, sh),
            a,
        ));
    }
}

// ── LUI difftest ───────────────────────────────────────────

#[test]
fn difftest_lui() {
    let cases: Vec<i32> = vec![
        0x12345_000u32 as i32,
        0xFFFFF_000u32 as i32,
        0x00001_000u32 as i32,
        0x80000_000u32 as i32,
        0,
    ];
    for imm in cases {
        let upper = (imm as u32) >> 12;
        difftest_alu(&AluTest {
            name: "lui",
            asm: format!("lui t2, {upper}"),
            insn: lui(7, imm),
            init: vec![],
            check_reg: 7,
        });
    }
}

// ── W-suffix difftests ─────────────────────────────────────

#[test]
fn difftest_addw() {
    let cases: Vec<(u64, u64)> = vec![
        (V32MAX, V1),
        (V0, V0),
        (VNEG1, V1),
        (V32MIN, VNEG1),
        (VPATTERN, V32FF),
    ];
    for (a, b) in cases {
        difftest_alu(&rtype_test("addw", "addw", addw(7, 5, 6), a, b));
    }
}

#[test]
fn difftest_subw() {
    let cases: Vec<(u64, u64)> =
        vec![(V0, V1), (V32MIN, V1), (V1, V1), (VPATTERN, VPATTERN)];
    for (a, b) in cases {
        difftest_alu(&rtype_test("subw", "subw", subw(7, 5, 6), a, b));
    }
}

#[test]
fn difftest_sllw() {
    let cases: Vec<(u64, u64)> =
        vec![(V1, 31), (0xFF, 24), (VNEG1, 0), (V32MAX, 1)];
    for (a, b) in cases {
        difftest_alu(&rtype_test("sllw", "sllw", sllw(7, 5, 6), a, b));
    }
}

#[test]
fn difftest_srlw() {
    let cases: Vec<(u64, u64)> =
        vec![(VNEG1, 16), (V32MIN, 1), (V32FF, 0), (0x8000_0000u64, 31)];
    for (a, b) in cases {
        difftest_alu(&rtype_test("srlw", "srlw", srlw(7, 5, 6), a, b));
    }
}

#[test]
fn difftest_sraw() {
    let cases: Vec<(u64, u64)> =
        vec![(0x8000_0000u64, 4), (V32MIN, 31), (V32MAX, 16), (VNEG1, 0)];
    for (a, b) in cases {
        difftest_alu(&rtype_test("sraw", "sraw", sraw(7, 5, 6), a, b));
    }
}

#[test]
fn difftest_addiw() {
    let cases: Vec<(u64, i32)> =
        vec![(V32MAX, 1), (V0, 0), (V0, -1), (VNEG1, 1), (VPATTERN, 100)];
    for (a, imm) in cases {
        difftest_alu(&itype_test(
            "addiw",
            &format!("addiw t2, t0, {imm}"),
            addiw(7, 5, imm),
            a,
        ));
    }
}

#[test]
fn difftest_slliw() {
    let cases: Vec<(u64, u32)> =
        vec![(V1, 31), (V1, 0), (0xFF, 24), (VNEG1, 16)];
    for (a, sh) in cases {
        difftest_alu(&itype_test(
            "slliw",
            &format!("slliw t2, t0, {sh}"),
            slliw(7, 5, sh),
            a,
        ));
    }
}

#[test]
fn difftest_srliw() {
    let cases: Vec<(u64, u32)> =
        vec![(VNEG1, 16), (0x8000_0000u64, 31), (V32FF, 0)];
    for (a, sh) in cases {
        difftest_alu(&itype_test(
            "srliw",
            &format!("srliw t2, t0, {sh}"),
            srliw(7, 5, sh),
            a,
        ));
    }
}

#[test]
fn difftest_sraiw() {
    let cases: Vec<(u64, u32)> =
        vec![(0x8000_0000u64, 4), (V32MIN, 31), (V32MAX, 16)];
    for (a, sh) in cases {
        difftest_alu(&itype_test(
            "sraiw",
            &format!("sraiw t2, t0, {sh}"),
            sraiw(7, 5, sh),
            a,
        ));
    }
}

// ── Branch difftests ───────────────────────────────────────

#[test]
fn difftest_beq() {
    let cases: Vec<(u64, u64)> =
        vec![(V0, V0), (V0, V1), (VNEG1, VNEG1), (VMAX, VMIN)];
    for (a, b) in cases {
        difftest_branch(&BranchTest {
            name: "beq",
            mnemonic: "beq",
            insn_fn: beq,
            rs1_val: a,
            rs2_val: b,
        });
    }
}

#[test]
fn difftest_bne() {
    let cases: Vec<(u64, u64)> =
        vec![(V0, V0), (V0, V1), (VNEG1, V0), (VMAX, VMAX)];
    for (a, b) in cases {
        difftest_branch(&BranchTest {
            name: "bne",
            mnemonic: "bne",
            insn_fn: bne,
            rs1_val: a,
            rs2_val: b,
        });
    }
}

#[test]
fn difftest_blt() {
    let cases: Vec<(u64, u64)> = vec![
        (VNEG1, V0),
        (V0, VNEG1),
        (VMIN, VMAX),
        (VMAX, VMIN),
        (V0, V0),
    ];
    for (a, b) in cases {
        difftest_branch(&BranchTest {
            name: "blt",
            mnemonic: "blt",
            insn_fn: blt,
            rs1_val: a,
            rs2_val: b,
        });
    }
}

#[test]
fn difftest_bge() {
    let cases: Vec<(u64, u64)> =
        vec![(V0, V0), (V1, V0), (VNEG1, V0), (VMAX, VMIN), (VMIN, VMAX)];
    for (a, b) in cases {
        difftest_branch(&BranchTest {
            name: "bge",
            mnemonic: "bge",
            insn_fn: bge,
            rs1_val: a,
            rs2_val: b,
        });
    }
}

#[test]
fn difftest_bltu() {
    let cases: Vec<(u64, u64)> =
        vec![(V0, V1), (V1, V0), (V0, VNEG1), (VNEG1, V0), (VMIN, VMAX)];
    for (a, b) in cases {
        difftest_branch(&BranchTest {
            name: "bltu",
            mnemonic: "bltu",
            insn_fn: bltu,
            rs1_val: a,
            rs2_val: b,
        });
    }
}

#[test]
fn difftest_bgeu() {
    let cases: Vec<(u64, u64)> = vec![
        (V0, V0),
        (VNEG1, V0),
        (V0, VNEG1),
        (VMAX, VMIN),
        (VNEG1, VNEG1),
    ];
    for (a, b) in cases {
        difftest_branch(&BranchTest {
            name: "bgeu",
            mnemonic: "bgeu",
            insn_fn: bgeu,
            rs1_val: a,
            rs2_val: b,
        });
    }
}
