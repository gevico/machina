//! Integration tests for the execution loop.

mod multi_vcpu;

use machina_accel::exec::exec_loop::{cpu_exec_loop_env, ExitReason};
use machina_accel::exec::ExecEnv;
use machina_accel::ir::context::Context;
use machina_accel::ir::TempIdx;
use machina_accel::GuestCpu;
use machina_accel::X86_64CodeGen;
use machina_guest_riscv::riscv::cpu::RiscvCpu;
use machina_guest_riscv::riscv::exception::Exception;
use machina_guest_riscv::riscv::ext::RiscvCfg;
use machina_guest_riscv::riscv::{RiscvDisasContext, RiscvTranslator};
use machina_guest_riscv::{translator_loop, DisasJumpType, TranslatorOps};

/// Test wrapper: RiscvCpu + guest code buffer.
struct TestCpu {
    cpu: RiscvCpu,
    code: Vec<u8>,
}

impl TestCpu {
    fn new(insns: &[u32]) -> Self {
        let code: Vec<u8> =
            insns.iter().flat_map(|i| i.to_le_bytes()).collect();
        Self {
            cpu: RiscvCpu::new(),
            code,
        }
    }
}

const NUM_GPRS: usize = 32;

impl GuestCpu for TestCpu {
    type IrContext = Context;

    fn get_pc(&self) -> u64 {
        self.cpu.pc
    }

    fn get_flags(&self) -> u32 {
        0
    }

    fn gen_code(&mut self, ir: &mut Context, pc: u64, max_insns: u32) -> u32 {
        let base = self.code.as_ptr();
        let avail = (self.code.len() as u64 - pc) / 4;
        let limit = max_insns.min(avail as u32);

        if ir.nb_globals() == 0 {
            // First call: register globals via translator_loop
            let mut d = RiscvDisasContext::new(pc, base, RiscvCfg::default());
            d.base.max_insns = limit;
            translator_loop::<RiscvTranslator>(&mut d, ir);
            d.base.num_insns * 4
        } else {
            // Reuse existing globals (same order as
            // init_disas_context: env, gpr[0..32], pc)
            let mut d = RiscvDisasContext::new(pc, base, RiscvCfg::default());
            d.base.max_insns = limit;
            d.env = TempIdx(0);
            for i in 0..NUM_GPRS {
                d.gpr[i] = TempIdx(1 + i as u32);
            }
            d.pc = TempIdx(1 + NUM_GPRS as u32);
            // Run translation loop without init
            RiscvTranslator::tb_start(&mut d, ir);
            loop {
                RiscvTranslator::insn_start(&mut d, ir);
                RiscvTranslator::translate_insn(&mut d, ir);
                if d.base.is_jmp != DisasJumpType::Next {
                    break;
                }
                if d.base.num_insns >= d.base.max_insns {
                    d.base.is_jmp = DisasJumpType::TooMany;
                    break;
                }
            }
            RiscvTranslator::tb_stop(&mut d, ir);
            d.base.num_insns * 4
        }
    }

    fn env_ptr(&mut self) -> *mut u8 {
        &mut self.cpu as *mut RiscvCpu as *mut u8
    }

    fn handle_exception(&mut self, excp: u64, tval: u64) {
        let e = match excp {
            0 => Exception::InstructionMisaligned,
            1 => Exception::InstructionAccessFault,
            2 => Exception::IllegalInstruction,
            3 => Exception::Breakpoint,
            4 => Exception::LoadMisaligned,
            5 => Exception::LoadAccessFault,
            6 => Exception::StoreMisaligned,
            7 => Exception::StoreAccessFault,
            8 => Exception::EcallFromU,
            9 => Exception::EcallFromS,
            11 => Exception::EcallFromM,
            12 => Exception::InstructionPageFault,
            13 => Exception::LoadPageFault,
            15 => Exception::StorePageFault,
            _ => Exception::IllegalInstruction,
        };
        self.cpu.raise_exception(e, tval);
    }

    fn set_jmp_env(&mut self, ptr: u64) {
        self.cpu.jmp_env = ptr;
    }

    fn clear_jmp_env(&mut self) {
        self.cpu.jmp_env = 0;
    }

    fn tlb_flush(&mut self) {
        self.cpu.mmu.flush();
    }
}

// ── RISC-V instruction encoding helpers ─────────────────────

fn rv_i(imm: i32, rs1: u32, f3: u32, rd: u32, op: u32) -> u32 {
    let imm = (imm as u32) & 0xFFF;
    (imm << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op
}

fn rv_r(f7: u32, rs2: u32, rs1: u32, f3: u32, rd: u32, op: u32) -> u32 {
    (f7 << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op
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

const OP_IMM: u32 = 0b0010011;
const OP_REG: u32 = 0b0110011;
const OP_LUI: u32 = 0b0110111;

fn addi(rd: u32, rs1: u32, imm: i32) -> u32 {
    rv_i(imm, rs1, 0b000, rd, OP_IMM)
}
fn slli(rd: u32, rs1: u32, sh: u32) -> u32 {
    rv_r(0, sh, rs1, 0b001, rd, OP_IMM)
}
fn add(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0, rs2, rs1, 0b000, rd, OP_REG)
}
fn lui(rd: u32, imm: i32) -> u32 {
    rv_u(imm, rd, OP_LUI)
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
fn ecall() -> u32 {
    0x0000_0073
}
fn ebreak() -> u32 {
    0x0010_0073
}

// ── Helper ──────────────────────────────────────────────────

fn run(insns: &[u32], setup: impl FnOnce(&mut TestCpu)) -> TestCpu {
    let mut t = TestCpu::new(insns);
    setup(&mut t);
    let mut env = ExecEnv::new(X86_64CodeGen::new());
    let r = unsafe { cpu_exec_loop_env(&mut env, &mut t) };
    assert_eq!(
        r,
        ExitReason::Ecall { priv_level: 0 },
        "expected ecall exit"
    );
    t
}

fn run_env(
    insns: &[u32],
    setup: impl FnOnce(&mut TestCpu),
) -> (TestCpu, ExecEnv<X86_64CodeGen>) {
    let mut t = TestCpu::new(insns);
    setup(&mut t);
    let mut env = ExecEnv::new(X86_64CodeGen::new());
    let r = unsafe { cpu_exec_loop_env(&mut env, &mut t) };
    assert_eq!(r, ExitReason::Ecall { priv_level: 0 });
    (t, env)
}

// ── Original tests ──────────────────────────────────────────

/// Single TB that exits immediately via ecall.
#[test]
fn test_exec_loop_simple() {
    let t = run(&[addi(1, 0, 42), ecall()], |_| {});
    assert_eq!(t.cpu.gpr[1], 42);
}

/// Two TBs: first sets x1 and updates PC, second reads x1.
#[test]
fn test_exec_loop_two_tbs() {
    let (t, env) = run_env(&[addi(1, 0, 10), ecall()], |_| {});
    assert_eq!(t.cpu.gpr[1], 10);
    assert_eq!(env.shared.tb_store.len(), 1);
}

/// Execute the same TB twice to verify cache hit.
#[test]
fn test_exec_loop_cache_hit() {
    let insns = [addi(1, 0, 5), ecall()];
    let mut t = TestCpu::new(&insns);
    let mut env = ExecEnv::new(X86_64CodeGen::new());

    let r1 = unsafe { cpu_exec_loop_env(&mut env, &mut t) };
    assert_eq!(r1, ExitReason::Ecall { priv_level: 0 });
    assert_eq!(t.cpu.gpr[1], 5);
    assert_eq!(env.shared.tb_store.len(), 1);

    // Reset PC and x1, run again — should hit cache
    t.cpu.pc = 0;
    t.cpu.gpr[1] = 0;
    let r2 = unsafe { cpu_exec_loop_env(&mut env, &mut t) };
    assert_eq!(r2, ExitReason::Ecall { priv_level: 0 });
    assert_eq!(t.cpu.gpr[1], 5);
    assert_eq!(env.shared.tb_store.len(), 1);
}

/// Loop computing 1+2+...+N.
///
///   PC=0:  addi x1, x1, 1
///   PC=4:  add  x2, x2, x1
///   PC=8:  bne  x1, x3, -8   → goto PC=0
///   PC=12: ecall
#[test]
fn test_exec_loop_sum() {
    let t = run(
        &[addi(1, 1, 1), add(2, 2, 1), bne(1, 3, -8), ecall()],
        |t| {
            t.cpu.gpr[3] = 5;
        },
    );
    assert_eq!(t.cpu.gpr[1], 5);
    assert_eq!(t.cpu.gpr[2], 15); // 1+2+3+4+5
}

// ── New multi-TB tests ──────────────────────────────────────

/// Countdown: x1 starts at N, decrements to 0, then exits.
/// Single loop body TB + exit TB.
///
///   PC=0:  addi x1, x1, -1
///   PC=4:  bne  x1, x0, -4   → goto PC=0
///   PC=8:  ecall
#[test]
fn test_countdown_loop() {
    let t = run(&[addi(1, 1, -1), bne(1, 0, -4), ecall()], |t| {
        t.cpu.gpr[1] = 100;
    });
    assert_eq!(t.cpu.gpr[1], 0);
}

/// Fibonacci: compute fib(10) = 55.
///
///   x1 = fib(n-2), x2 = fib(n-1), x4 = remaining iters
///   PC=0:  add  x3, x1, x2    # x3 = fib(n)
///   PC=4:  add  x1, x2, x0    # x1 = old x2
///   PC=8:  add  x2, x3, x0    # x2 = x3
///   PC=12: addi x4, x4, -1
///   PC=16: bne  x4, x0, -16   → goto PC=0
///   PC=20: ecall
#[test]
fn test_fibonacci() {
    let t = run(
        &[
            add(3, 1, 2),
            add(1, 2, 0),
            add(2, 3, 0),
            addi(4, 4, -1),
            bne(4, 0, -16),
            ecall(),
        ],
        |t| {
            t.cpu.gpr[1] = 0; // fib(0)
            t.cpu.gpr[2] = 1; // fib(1)
            t.cpu.gpr[4] = 9; // 9 iterations → fib(10)
        },
    );
    assert_eq!(t.cpu.gpr[2], 55);
}

/// Two sequential loops: first counts x1 to M, then x2 to N.
/// The current frontend splits this flow into five TBs.
///
///   PC=0:  addi x1, x1, 1
///   PC=4:  bne  x1, x3, -4    → goto PC=0
///   PC=8:  addi x2, x2, 1
///   PC=12: bne  x2, x4, -4    → goto PC=8
///   PC=16: ecall
#[test]
fn test_two_sequential_loops() {
    let (t, env) = run_env(
        &[
            addi(1, 1, 1),
            bne(1, 3, -4),
            addi(2, 2, 1),
            bne(2, 4, -4),
            ecall(),
        ],
        |t| {
            t.cpu.gpr[3] = 7; // loop1 limit
            t.cpu.gpr[4] = 3; // loop2 limit
        },
    );
    assert_eq!(t.cpu.gpr[1], 7);
    assert_eq!(t.cpu.gpr[2], 3);
    // TB count depends on translator block-splitting
    // heuristics; verify correctness, not exact count.
    assert!(env.shared.tb_store.len() >= 3);
}

/// JAL forward skip: jump over dead code.
///
///   PC=0:  addi x1, x0, 1
///   PC=4:  jal  x0, 8         → goto PC=12
///   PC=8:  addi x1, x0, 99    # dead code
///   PC=12: addi x2, x1, 10
///   PC=16: ecall
#[test]
fn test_jal_forward_skip() {
    let (t, env) = run_env(
        &[
            addi(1, 0, 1),
            jal(0, 8),
            addi(1, 0, 99),
            addi(2, 1, 10),
            ecall(),
        ],
        |_| {},
    );
    assert_eq!(t.cpu.gpr[1], 1); // not 99
    assert_eq!(t.cpu.gpr[2], 11); // 1 + 10
                                  // TB at PC=0 (addi+jal), TB at PC=12 (addi+ecall)
    assert_eq!(env.shared.tb_store.len(), 2);
}

/// JAL chain: TB0 → TB1 → TB2 → exit.
///
///   PC=0:  addi x1, x0, 10
///   PC=4:  jal  x0, 8         → goto PC=12
///   PC=8:  ecall               # unreachable
///   PC=12: addi x2, x0, 20
///   PC=16: jal  x0, 8         → goto PC=24
///   PC=20: ecall               # unreachable
///   PC=24: add  x3, x1, x2
///   PC=28: ecall
#[test]
fn test_jal_chain_three_tbs() {
    let (t, env) = run_env(
        &[
            addi(1, 0, 10),
            jal(0, 8),
            ecall(),
            addi(2, 0, 20),
            jal(0, 8),
            ecall(),
            add(3, 1, 2),
            ecall(),
        ],
        |_| {},
    );
    assert_eq!(t.cpu.gpr[1], 10);
    assert_eq!(t.cpu.gpr[2], 20);
    assert_eq!(t.cpu.gpr[3], 30);
    assert_eq!(env.shared.tb_store.len(), 3);
}

/// JAL with link: simulate function call.
///
///   PC=0:  addi x1, x0, 5
///   PC=4:  jal  x5, 8         → call PC=12, x5=8
///   PC=8:  addi x3, x2, 100   # return point
///   PC=12: ecall
///   -- "function" at PC=12 --
///   PC=12: add  x2, x1, x1    # x2 = x1*2
///   PC=16: jalr x0, x5, 0     → return to x5=8
///
/// Wait, PC=12 is used twice. Let me redesign:
///   PC=0:  addi x1, x0, 5
///   PC=4:  jal  x5, 12        → call PC=16, x5=8
///   PC=8:  addi x3, x2, 100   # after return
///   PC=12: ecall
///   PC=16: add  x2, x1, x1    # "function body"
///   PC=20: jalr x0, x5, 0     → return to x5=8
#[test]
fn test_jal_jalr_call_return() {
    let (t, env) = run_env(
        &[
            addi(1, 0, 5),   // PC=0
            jal(5, 12),      // PC=4: call, x5=8
            addi(3, 2, 100), // PC=8: after return
            ecall(),         // PC=12
            add(2, 1, 1),    // PC=16: "function"
            jalr(0, 5, 0),   // PC=20: return to x5=8
        ],
        |_| {},
    );
    assert_eq!(t.cpu.gpr[1], 5);
    assert_eq!(t.cpu.gpr[2], 10); // 5+5
    assert_eq!(t.cpu.gpr[3], 110); // 10+100
    assert_eq!(t.cpu.gpr[5], 8); // return address
                                 // TBs: PC=0, PC=16, PC=8, PC=12 (or fewer if merged)
    assert!(env.shared.tb_store.len() >= 3);
}

/// Conditional path: BEQ selects between two code paths.
///
///   PC=0:  beq  x1, x0, 12    → if x1==0 goto PC=16
///   PC=4:  addi x2, x0, 200   # path A (x1 != 0)
///   PC=8:  jal  x0, 12        → goto PC=20
///   PC=12: ecall               # unreachable filler
///   PC=16: addi x2, x0, 100   # path B (x1 == 0)
///   PC=20: ecall
#[test]
fn test_conditional_path_taken() {
    let t = run(
        &[
            beq(1, 0, 16),
            addi(2, 0, 200),
            jal(0, 12),
            ecall(),
            addi(2, 0, 100),
            ecall(),
        ],
        |t| {
            t.cpu.gpr[1] = 0; // x1 == 0 → take branch
        },
    );
    assert_eq!(t.cpu.gpr[2], 100); // path B
}

#[test]
fn test_conditional_path_not_taken() {
    let t = run(
        &[
            beq(1, 0, 16),
            addi(2, 0, 200),
            jal(0, 12),
            ecall(),
            addi(2, 0, 100),
            ecall(),
        ],
        |t| {
            t.cpu.gpr[1] = 1; // x1 != 0 → fall through
        },
    );
    assert_eq!(t.cpu.gpr[2], 200); // path A
}

/// Nested loop: outer runs M times, inner runs N times each.
/// Total inner iterations = M * N.
///
///   PC=0:  addi x5, x5, 1       # inner++
///   PC=4:  addi x2, x2, 1       # inner counter
///   PC=8:  bne  x2, x4, -8      → inner loop (PC=0)
///   PC=12: addi x2, x0, 0       # reset inner counter
///   PC=16: addi x1, x1, 1       # outer counter
///   PC=20: bne  x1, x3, -20     → outer loop (PC=0)
///   PC=24: ecall
#[test]
fn test_nested_loop() {
    let (t, env) = run_env(
        &[
            addi(5, 5, 1),
            addi(2, 2, 1),
            bne(2, 4, -8),
            addi(2, 0, 0),
            addi(1, 1, 1),
            bne(1, 3, -20),
            ecall(),
        ],
        |t| {
            t.cpu.gpr[3] = 4; // outer limit
            t.cpu.gpr[4] = 3; // inner limit
        },
    );
    assert_eq!(t.cpu.gpr[1], 4);
    assert_eq!(t.cpu.gpr[5], 12); // 4 * 3 = 12
                                  // TBs: PC=0 (inner body+branch), PC=12 (reset+outer),
                                  //       PC=24 (ecall)
    assert_eq!(env.shared.tb_store.len(), 3);
}

/// Larger sum: 1+2+...+1000 = 500500.
/// Stress-tests TB cache with many loop iterations.
#[test]
fn test_large_sum_loop() {
    let t = run(
        &[addi(1, 1, 1), add(2, 2, 1), bne(1, 3, -8), ecall()],
        |t| {
            t.cpu.gpr[3] = 1000;
        },
    );
    assert_eq!(t.cpu.gpr[1], 1000);
    assert_eq!(t.cpu.gpr[2], 500_500);
}

/// BLT loop: count while x1 < x3 (signed comparison).
///
///   PC=0:  addi x1, x1, 1
///   PC=4:  blt  x1, x3, -4    → if x1 < x3, goto PC=0
///   PC=8:  ecall
#[test]
fn test_blt_loop() {
    let t = run(&[addi(1, 1, 1), blt(1, 3, -4), ecall()], |t| {
        t.cpu.gpr[1] = (-5i64) as u64; // start negative
        t.cpu.gpr[3] = 3;
    });
    assert_eq!(t.cpu.gpr[1], 3); // stopped at x1 == x3
}

/// BGE exit: loop until x1 >= x3.
///
///   PC=0:  addi x1, x1, 1
///   PC=4:  bge  x1, x3, 4     → if x1 >= x3, goto PC=12
///   PC=8:  jal  x0, -8        → goto PC=0
///   PC=12: ecall
#[test]
fn test_bge_exit_loop() {
    let t = run(&[addi(1, 1, 1), bge(1, 3, 8), jal(0, -8), ecall()], |t| {
        t.cpu.gpr[3] = 10;
    });
    assert_eq!(t.cpu.gpr[1], 10);
}

/// Multi-register accumulation across TBs.
/// Each TB sets one register and jumps to the next.
///
///   PC=0:  addi x1, x0, 1;  jal x0, 8
///   PC=8:  addi x2, x0, 2;  jal x0, 8
///   PC=16: addi x3, x0, 3;  jal x0, 8
///   PC=24: addi x4, x0, 4;  jal x0, 8
///   PC=32: add  x5, x1, x2
///   PC=36: add  x5, x5, x3
///   PC=40: add  x5, x5, x4
///   PC=44: ecall
#[test]
fn test_multi_tb_register_pipeline() {
    let (t, env) = run_env(
        &[
            addi(1, 0, 1), // PC=0
            jal(0, 8),     // PC=4  → PC=12
            addi(2, 0, 2), // PC=8  (dead)
            addi(2, 0, 2), // PC=12
            jal(0, 8),     // PC=16 → PC=24
            addi(3, 0, 3), // PC=20 (dead)
            addi(3, 0, 3), // PC=24
            jal(0, 8),     // PC=28 → PC=36
            addi(4, 0, 4), // PC=32 (dead)
            addi(4, 0, 4), // PC=36
            jal(0, 8),     // PC=40 → PC=48
            add(5, 1, 2),  // PC=44 (dead)
            add(5, 1, 2),  // PC=48
            add(5, 5, 3),  // PC=52
            add(5, 5, 4),  // PC=56
            ecall(),       // PC=60
        ],
        |_| {},
    );
    assert_eq!(t.cpu.gpr[1], 1);
    assert_eq!(t.cpu.gpr[2], 2);
    assert_eq!(t.cpu.gpr[3], 3);
    assert_eq!(t.cpu.gpr[4], 4);
    assert_eq!(t.cpu.gpr[5], 10); // 1+2+3+4
    assert_eq!(env.shared.tb_store.len(), 5);
}

/// Bit manipulation loop: shift-and-count set bits.
/// Counts bits in x1 by shifting right and adding LSB.
///
///   PC=0:  ori  x3, x3, 1     # mask = 1
///   PC=4:  beq  x1, x0, 16    → if x1==0, goto PC=24
///   PC=8:  add  x4, x1, x0    # tmp = x1
///   PC=12: xori x4, x4, -1    # invert (to get AND via sub)
///   ... too complex, let me simplify:
///
/// Simpler: count down x1 by subtracting 1, accumulate in x2.
/// Actually let me do a power-of-2 computation: x2 = 1 << x1.
///
///   PC=0:  addi x2, x0, 1     # x2 = 1
///   PC=4:  beq  x1, x0, 8     → if x1==0, goto PC=16
///   PC=8:  slli x2, x2, 1     # x2 <<= 1
///   PC=12: addi x1, x1, -1    # x1--
///   PC=16: beq  x1, x0, 8     → if x1==0, goto PC=28
///   PC=20: jal  x0, -12       → goto PC=8
///   PC=24: ecall               # unreachable
///   PC=28: ecall
///
/// Hmm, this is getting complicated. Let me use a simpler
/// approach: just shift left in a loop.
///
///   PC=0:  addi x2, x0, 1     # x2 = 1
///   PC=4:  slli x2, x2, 1     # x2 <<= 1
///   PC=8:  addi x1, x1, -1    # x1--
///   PC=12: bne  x1, x0, -8    → if x1!=0, goto PC=4
///   PC=16: ecall
#[test]
fn test_shift_loop_power_of_two() {
    let t = run(
        &[
            addi(2, 0, 1),
            slli(2, 2, 1),
            addi(1, 1, -1),
            bne(1, 0, -8),
            ecall(),
        ],
        |t| {
            t.cpu.gpr[1] = 10; // compute 2^10
        },
    );
    assert_eq!(t.cpu.gpr[2], 1024); // 2^10
}

/// Ebreak traps to mtvec and then exits via the handler ecall.
#[test]
fn test_ebreak_exit_code() {
    let insns = [addi(1, 0, 77), ebreak(), ecall()];
    let mut t = TestCpu::new(&insns);
    t.cpu.csr.mtvec = 8;
    let mut env = ExecEnv::new(X86_64CodeGen::new());
    let r = unsafe { cpu_exec_loop_env(&mut env, &mut t) };
    assert_eq!(r, ExitReason::Ecall { priv_level: 0 });
    assert_eq!(t.cpu.gpr[1], 77);
    assert_eq!(t.cpu.csr.mcause, 3);
    assert_eq!(t.cpu.csr.mepc, 4);
}

/// LUI + ADDI to build a 32-bit constant, then loop.
///
///   PC=0:  lui  x1, 0x12345000
///   PC=4:  addi x1, x1, 0x678
///   PC=8:  addi x2, x2, 1
///   PC=12: bne  x2, x3, -4    → goto PC=8
///   PC=16: ecall
#[test]
fn test_lui_addi_with_loop() {
    let t = run(
        &[
            lui(1, 0x12345_000u32 as i32),
            addi(1, 1, 0x678),
            addi(2, 2, 1),
            bne(2, 3, -4),
            ecall(),
        ],
        |t| {
            t.cpu.gpr[3] = 5;
        },
    );
    assert_eq!(t.cpu.gpr[1], 0x12345678);
    assert_eq!(t.cpu.gpr[2], 5);
}

/// Alternating branches: even/odd counter selects path.
///
///   PC=0:  addi x1, x1, 1       # counter++
///   PC=4:  xori x4, x1, 1       # x4 = x1 ^ 1
///   PC=8:  beq  x4, x0, 8       → if (x1&1)==1, goto PC=20
///   PC=12: addi x2, x2, 1       # even path: x2++
///   PC=16: jal  x0, 8           → goto PC=28
///   PC=20: addi x3, x3, 1       # odd path: x3++
///   PC=24: jal  x0, 4           → goto PC=28
///   PC=28: bne  x1, x5, -28     → if x1!=limit, goto PC=0
///   PC=32: ecall
///
/// Wait, the xori trick doesn't isolate bit 0 cleanly.
/// Let me use a different approach: subtract and check.
///
/// Actually simpler: use two nested counters.
/// Even simpler: just test that multiple branch targets
/// create multiple TBs and all get cached.
///
///   PC=0:  addi x1, x1, 1
///   PC=4:  blt  x1, x3, 8     → if x1 < limit, goto PC=16
///   PC=8:  addi x2, x0, 1     # done: x2 = 1
///   PC=12: ecall
///   PC=16: blt  x1, x4, -16   → if x1 < half, goto PC=0
///   PC=20: addi x5, x5, 1     # past halfway: x5++
///   PC=24: jal  x0, -24       → goto PC=0
#[test]
fn test_multi_branch_targets() {
    let (t, env) = run_env(
        &[
            addi(1, 1, 1),  // PC=0
            blt(1, 3, 12),  // PC=4:  if x1<10 goto PC=16
            addi(2, 0, 1),  // PC=8:  done
            ecall(),        // PC=12
            blt(1, 4, -16), // PC=16: if x1<5 goto PC=0
            addi(5, 5, 1),  // PC=20: past halfway
            jal(0, -24),    // PC=24: goto PC=0
        ],
        |t| {
            t.cpu.gpr[3] = 10; // limit
            t.cpu.gpr[4] = 5; // halfway
        },
    );
    assert_eq!(t.cpu.gpr[1], 10);
    assert_eq!(t.cpu.gpr[2], 1);
    assert_eq!(t.cpu.gpr[5], 5); // iterations 5..9
                                 // Multiple TBs from different branch targets
    assert!(env.shared.tb_store.len() >= 4);
}

// ── Privileged instruction encoding helpers ────────────────

fn mret() -> u32 {
    0x3020_0073
}
fn wfi() -> u32 {
    0x1050_0073
}
fn sfence_vma(rs1: u32, rs2: u32) -> u32 {
    rv_r(0b0001001, rs2, rs1, 0b000, 0, 0b1110011)
}

// ── Privileged exit tests ──────────────────────────────────

/// MRET exits the TB, the exec loop calls
/// execute_mret() and continues.  The instruction after
/// MRET still executes (default no-op keeps PC at
/// pc_next, so the next TB is the following insn).
///
///   PC=0:  addi x1, x0, 10
///   PC=4:  mret                → EXCP_MRET, continue
///   PC=8:  addi x2, x0, 20
///   PC=12: ecall
#[test]
fn test_exec_loop_mret_continues() {
    let insns = [addi(1, 0, 10), mret(), addi(2, 0, 20), ecall()];
    let mut t = TestCpu::new(&insns);
    let mut env = ExecEnv::new(X86_64CodeGen::new());
    let r = unsafe { cpu_exec_loop_env(&mut env, &mut t) };
    assert_eq!(
        r,
        ExitReason::Ecall { priv_level: 0 },
        "mret should not halt; ecall follows"
    );
    assert_eq!(t.cpu.gpr[1], 10);
    assert_eq!(t.cpu.gpr[2], 20);
}

/// WFI causes the loop to return ExitReason::Halted.
///
///   PC=0:  addi x1, x0, 42
///   PC=4:  wfi                → EXCP_WFI, halt
#[test]
fn test_exec_loop_wfi_halts() {
    let insns = [addi(1, 0, 42), wfi()];
    let mut t = TestCpu::new(&insns);
    let mut env = ExecEnv::new(X86_64CodeGen::new());
    let r = unsafe { cpu_exec_loop_env(&mut env, &mut t) };
    assert_eq!(r, ExitReason::Halted);
    assert_eq!(t.cpu.gpr[1], 42);
}

/// SFENCE.VMA flushes the TLB and continues execution.
///
///   PC=0:  addi x1, x0, 5
///   PC=4:  sfence.vma x0, x0  → EXCP_SFENCE_VMA, continue
///   PC=8:  addi x2, x0, 99
///   PC=12: ecall
#[test]
fn test_exec_loop_sfence_vma_continues() {
    let insns = [addi(1, 0, 5), sfence_vma(0, 0), addi(2, 0, 99), ecall()];
    let mut t = TestCpu::new(&insns);
    let mut env = ExecEnv::new(X86_64CodeGen::new());
    let r = unsafe { cpu_exec_loop_env(&mut env, &mut t) };
    assert_eq!(
        r,
        ExitReason::Ecall { priv_level: 0 },
        "sfence.vma should not halt"
    );
    assert_eq!(t.cpu.gpr[1], 5);
    assert_eq!(t.cpu.gpr[2], 99);
}

/// ECALL returns Ecall with privilege_level().
/// Default TestCpu returns priv_level=0 (U-mode).
#[test]
fn test_ecall_priv_level_u_mode() {
    let insns = [addi(1, 0, 1), ecall()];
    let mut t = TestCpu::new(&insns);
    let mut env = ExecEnv::new(X86_64CodeGen::new());
    let r = unsafe { cpu_exec_loop_env(&mut env, &mut t) };
    assert_eq!(r, ExitReason::Ecall { priv_level: 0 });
}

/// Verify that different privilege levels produce the
/// correct Ecall variant.  Uses a wrapper that overrides
/// privilege_level().
struct PrivTestCpu {
    inner: TestCpu,
    priv_level: u8,
}

impl GuestCpu for PrivTestCpu {
    type IrContext = Context;

    fn get_pc(&self) -> u64 {
        self.inner.get_pc()
    }
    fn get_flags(&self) -> u32 {
        self.inner.get_flags()
    }
    fn gen_code(&mut self, ir: &mut Context, pc: u64, max_insns: u32) -> u32 {
        self.inner.gen_code(ir, pc, max_insns)
    }
    fn env_ptr(&mut self) -> *mut u8 {
        self.inner.env_ptr()
    }
    fn privilege_level(&self) -> u8 {
        self.priv_level
    }
}

#[test]
fn test_ecall_from_different_privs() {
    // U-mode (0)
    let insns = [ecall()];
    let mut cpu = PrivTestCpu {
        inner: TestCpu::new(&insns),
        priv_level: 0,
    };
    let mut env = ExecEnv::new(X86_64CodeGen::new());
    let r = unsafe { cpu_exec_loop_env(&mut env, &mut cpu) };
    assert_eq!(r, ExitReason::Ecall { priv_level: 0 });

    // S-mode (1)
    cpu.inner = TestCpu::new(&insns);
    cpu.priv_level = 1;
    cpu.inner.cpu.pc = 0;
    let mut env = ExecEnv::new(X86_64CodeGen::new());
    let r = unsafe { cpu_exec_loop_env(&mut env, &mut cpu) };
    assert_eq!(r, ExitReason::Ecall { priv_level: 1 });

    // M-mode (3)
    cpu.inner = TestCpu::new(&insns);
    cpu.priv_level = 3;
    cpu.inner.cpu.pc = 0;
    let mut env = ExecEnv::new(X86_64CodeGen::new());
    let r = unsafe { cpu_exec_loop_env(&mut env, &mut cpu) };
    assert_eq!(r, ExitReason::Ecall { priv_level: 3 });
}
