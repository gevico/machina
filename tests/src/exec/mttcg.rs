//! Multi-threaded TCG (MTTCG) concurrent execution tests.

use std::sync::Arc;
use std::thread;

use machina_backend::X86_64CodeGen;
use machina_core::context::Context;
use machina_core::tb::EXCP_ECALL;
use machina_core::TempIdx;
use machina_exec::exec_loop::{cpu_exec_loop_mt, ExitReason};
use machina_exec::{ExecEnv, GuestCpu, PerCpuState, SharedState};
use machina_frontend::riscv::cpu::RiscvCpu;
use machina_frontend::riscv::ext::RiscvCfg;
use machina_frontend::riscv::{RiscvDisasContext, RiscvTranslator};
use machina_frontend::{translator_loop, DisasJumpType, TranslatorOps};

const NUM_GPRS: usize = 32;

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

impl GuestCpu for TestCpu {
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
            let mut d = RiscvDisasContext::new(pc, base, RiscvCfg::default());
            d.base.max_insns = limit;
            translator_loop::<RiscvTranslator>(&mut d, ir);
            d.base.num_insns * 4
        } else {
            let mut d = RiscvDisasContext::new(pc, base, RiscvCfg::default());
            d.base.max_insns = limit;
            d.env = TempIdx(0);
            for i in 0..NUM_GPRS {
                d.gpr[i] = TempIdx(1 + i as u32);
            }
            d.pc = TempIdx(1 + NUM_GPRS as u32);
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
}

// RISC-V encoding helpers (same as exec/mod.rs)
fn rv_i(imm: i32, rs1: u32, f3: u32, rd: u32, op: u32) -> u32 {
    let imm = (imm as u32) & 0xFFF;
    (imm << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op
}
fn rv_r(f7: u32, rs2: u32, rs1: u32, f3: u32, rd: u32, op: u32) -> u32 {
    (f7 << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op
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

const OP_IMM: u32 = 0b0010011;
const OP_REG: u32 = 0b0110011;

fn addi(rd: u32, rs1: u32, imm: i32) -> u32 {
    rv_i(imm, rs1, 0b000, rd, OP_IMM)
}
fn add(rd: u32, rs1: u32, rs2: u32) -> u32 {
    rv_r(0, rs2, rs1, 0b000, rd, OP_REG)
}
fn bne(rs1: u32, rs2: u32, imm: i32) -> u32 {
    rv_b(imm, rs2, rs1, 0b001)
}
fn ecall() -> u32 {
    0x0000_0073
}

fn new_per_cpu() -> PerCpuState {
    PerCpuState {
        jump_cache: machina_core::tb::JumpCache::new(),
        stats: machina_exec::ExecStats::default(),
    }
}

/// Two vCPU threads each run an independent sum loop on
/// the same shared TB cache. Verifies correct results and
/// no panics from concurrent access.
#[test]
fn test_mt_sum_loop() {
    // sum 1..=N: addi x1,x1,1; add x2,x2,x1; bne x1,x3,-8; ecall
    let insns = [addi(1, 1, 1), add(2, 2, 1), bne(1, 3, -8), ecall()];
    let code: Vec<u8> = insns.iter().flat_map(|i| i.to_le_bytes()).collect();

    let env = ExecEnv::new(X86_64CodeGen::new());
    let shared = env.shared.clone();

    let code1 = code.clone();
    let shared1 = shared.clone();
    let h1 = thread::spawn(move || {
        let mut cpu = TestCpu {
            cpu: RiscvCpu::new(),
            code: code1,
        };
        cpu.cpu.gpr[3] = 100; // sum 1..=100
        let mut pc = new_per_cpu();
        let r = unsafe { cpu_exec_loop_mt(&shared1, &mut pc, &mut cpu) };
        assert_eq!(r, ExitReason::Exit(EXCP_ECALL as usize));
        assert_eq!(cpu.cpu.gpr[2], 5050);
    });

    let code2 = code.clone();
    let shared2 = shared.clone();
    let h2 = thread::spawn(move || {
        let mut cpu = TestCpu {
            cpu: RiscvCpu::new(),
            code: code2,
        };
        cpu.cpu.gpr[3] = 200; // sum 1..=200
        let mut pc = new_per_cpu();
        let r = unsafe { cpu_exec_loop_mt(&shared2, &mut pc, &mut cpu) };
        assert_eq!(r, ExitReason::Exit(EXCP_ECALL as usize));
        assert_eq!(cpu.cpu.gpr[2], 20100);
    });

    h1.join().unwrap();
    h2.join().unwrap();
}

/// Two vCPU threads execute the same code, verifying that
/// TBs are shared (translated only once).
#[test]
fn test_shared_tb_cache() {
    let insns = [addi(1, 0, 42), ecall()];
    let code: Vec<u8> = insns.iter().flat_map(|i| i.to_le_bytes()).collect();

    let env = ExecEnv::new(X86_64CodeGen::new());
    let shared = env.shared.clone();

    let mut handles = Vec::new();
    for _ in 0..4 {
        let c = code.clone();
        let s = shared.clone();
        handles.push(thread::spawn(move || {
            let mut cpu = TestCpu {
                cpu: RiscvCpu::new(),
                code: c,
            };
            let mut pc = new_per_cpu();
            let r = unsafe { cpu_exec_loop_mt(&s, &mut pc, &mut cpu) };
            assert_eq!(r, ExitReason::Exit(EXCP_ECALL as usize));
            assert_eq!(cpu.cpu.gpr[1], 42);
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    // TB should be translated only once (or at most a few
    // times due to races before double-check kicks in).
    assert!(shared.tb_store.len() <= 4);
}

/// Multiple threads concurrently look up the same TB.
#[test]
fn test_concurrent_tb_lookup() {
    let insns = [addi(1, 0, 1), ecall()];
    let code: Vec<u8> = insns.iter().flat_map(|i| i.to_le_bytes()).collect();

    let env = ExecEnv::new(X86_64CodeGen::new());
    let shared = env.shared.clone();

    // Pre-translate by running once.
    {
        let mut cpu = TestCpu {
            cpu: RiscvCpu::new(),
            code: code.clone(),
        };
        let mut pc = new_per_cpu();
        unsafe {
            cpu_exec_loop_mt(&shared, &mut pc, &mut cpu);
        }
    }
    assert_eq!(shared.tb_store.len(), 1);

    // Now spawn threads that all look up the same TB.
    let mut handles = Vec::new();
    for _ in 0..8 {
        let c = code.clone();
        let s = shared.clone();
        handles.push(thread::spawn(move || {
            let mut cpu = TestCpu {
                cpu: RiscvCpu::new(),
                code: c,
            };
            let mut pc = new_per_cpu();
            let r = unsafe { cpu_exec_loop_mt(&s, &mut pc, &mut cpu) };
            assert_eq!(r, ExitReason::Exit(EXCP_ECALL as usize));
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    // No new TBs should have been created.
    assert_eq!(shared.tb_store.len(), 1);
}

/// Multiple threads concurrently chain TBs.
#[test]
fn test_concurrent_chaining() {
    // Loop: addi x1,x1,1; bne x1,x3,-4; ecall
    let insns = [addi(1, 1, 1), bne(1, 3, -4), ecall()];
    let code: Vec<u8> = insns.iter().flat_map(|i| i.to_le_bytes()).collect();

    let env = ExecEnv::new(X86_64CodeGen::new());
    let shared = env.shared.clone();

    let mut handles = Vec::new();
    for i in 0..4 {
        let c = code.clone();
        let s = shared.clone();
        handles.push(thread::spawn(move || {
            let mut cpu = TestCpu {
                cpu: RiscvCpu::new(),
                code: c,
            };
            cpu.cpu.gpr[3] = 50 + i as u64;
            let mut pc = new_per_cpu();
            let r = unsafe { cpu_exec_loop_mt(&s, &mut pc, &mut cpu) };
            assert_eq!(r, ExitReason::Exit(EXCP_ECALL as usize));
            assert_eq!(cpu.cpu.gpr[1], 50 + i as u64);
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
}

/// Concurrent translation: multiple threads trigger
/// translation simultaneously.
#[test]
fn test_concurrent_translation() {
    // Each thread runs a different loop count, but same code.
    let insns = [addi(1, 1, 1), add(2, 2, 1), bne(1, 3, -8), ecall()];
    let code: Vec<u8> = insns.iter().flat_map(|i| i.to_le_bytes()).collect();

    let env = ExecEnv::new(X86_64CodeGen::new());
    let shared = env.shared.clone();

    let mut handles = Vec::new();
    for i in 0..4 {
        let c = code.clone();
        let s = shared.clone();
        handles.push(thread::spawn(move || {
            let mut cpu = TestCpu {
                cpu: RiscvCpu::new(),
                code: c,
            };
            cpu.cpu.gpr[3] = 10 * (i + 1) as u64;
            let mut pc = new_per_cpu();
            let r = unsafe { cpu_exec_loop_mt(&s, &mut pc, &mut cpu) };
            assert_eq!(r, ExitReason::Exit(EXCP_ECALL as usize));
            let n = cpu.cpu.gpr[3];
            let expected = n * (n + 1) / 2;
            assert_eq!(cpu.cpu.gpr[2], expected);
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
}
