use machina_backend::code_buffer::CodeBuffer;
use machina_backend::translate::translate_and_execute;
use machina_backend::HostCodeGen;
use machina_backend::X86_64CodeGen;
use machina_core::types::Type;
use machina_core::{Context, TempIdx};

/// Minimal RISC-V CPU state for testing.
#[repr(C)]
pub(super) struct RiscvCpuState {
    pub regs: [u64; 32], // x0-x31, offset 0..256
    pub pc: u64,         // offset 256
}

impl RiscvCpuState {
    pub fn new() -> Self {
        Self {
            regs: [0; 32],
            pc: 0,
        }
    }
}

/// RISC-V CPU state with a small memory window
/// for load/store tests.
#[repr(C)]
pub(super) struct RiscvCpuStateMem {
    pub regs: [u64; 32],
    pub pc: u64,
    pub mem: [u8; 64],
}

impl RiscvCpuStateMem {
    pub fn new() -> Self {
        Self {
            regs: [0; 32],
            pc: 0,
            mem: [0; 64],
        }
    }
}

/// Register globals for RISC-V x0-x31 and pc.
/// Returns (env_temp, reg_temps[0..32], pc_temp).
pub(super) fn setup_riscv_globals(
    ctx: &mut Context,
) -> (TempIdx, [TempIdx; 32], TempIdx) {
    // env pointer is a fixed temp in RBP
    let env = ctx.new_fixed(
        Type::I64,
        machina_backend::x86_64::Reg::Rbp as u8,
        "env",
    );

    // x0-x31 as globals backed by RiscvCpuState.regs
    let mut reg_temps = [TempIdx(0); 32];
    for i in 0..32u32 {
        let offset = (i as i64) * 8;
        let name: &'static str = match i {
            0 => "x0",
            1 => "x1",
            2 => "x2",
            3 => "x3",
            4 => "x4",
            5 => "x5",
            _ => "xN",
        };
        reg_temps[i as usize] = ctx.new_global(Type::I64, env, offset, name);
    }

    // pc at offset 256
    let pc = ctx.new_global(Type::I64, env, 256, "pc");

    (env, reg_temps, pc)
}

pub(super) fn run_riscv_tb<S, F>(cpu: &mut S, build: F) -> usize
where
    F: FnOnce(&mut Context, TempIdx, [TempIdx; 32], TempIdx),
{
    let mut backend = X86_64CodeGen::new();
    let mut buf = CodeBuffer::new(4096).unwrap();
    backend.emit_prologue(&mut buf);
    backend.emit_epilogue(&mut buf);

    let mut ctx = Context::new();
    backend.init_context(&mut ctx);
    let (env, regs, pc) = setup_riscv_globals(&mut ctx);

    build(&mut ctx, env, regs, pc);

    unsafe {
        translate_and_execute(
            &mut ctx,
            &backend,
            &mut buf,
            cpu as *mut S as *mut u8,
        )
    }
}

pub(super) fn split_u128(val: u128) -> (u64, u64) {
    (val as u64, (val >> 64) as u64)
}

pub(super) fn split_i128(val: i128) -> (u64, u64) {
    split_u128(val as u128)
}

macro_rules! riscv_bin_case {
    ($name:ident, $op:ident, $lhs:expr, $rhs:expr, $expect:expr) => {
        #[test]
        fn $name() {
            let mut cpu = RiscvCpuState::new();
            cpu.regs[1] = $lhs;
            cpu.regs[2] = $rhs;

            let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
                let tmp = ctx.new_temp(Type::I64);
                ctx.gen_insn_start(0x4000);
                ctx.$op(Type::I64, tmp, regs[1], regs[2]);
                ctx.gen_mov(Type::I64, regs[3], tmp);
                ctx.gen_exit_tb(0);
            });

            assert_eq!(exit_val, 0);
            assert_eq!(cpu.regs[3], $expect);
        }
    };
}

macro_rules! riscv_shift_case {
    ($name:ident, $op:ident, $val:expr, $shift:expr, $expect:expr) => {
        #[test]
        fn $name() {
            let mut cpu = RiscvCpuState::new();
            cpu.regs[1] = $val;
            cpu.regs[2] = $shift;

            let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
                let tmp = ctx.new_temp(Type::I64);
                ctx.gen_insn_start(0x4100);
                ctx.$op(Type::I64, tmp, regs[1], regs[2]);
                ctx.gen_mov(Type::I64, regs[3], tmp);
                ctx.gen_exit_tb(0);
            });

            assert_eq!(exit_val, 0);
            assert_eq!(cpu.regs[3], $expect);
        }
    };
}

macro_rules! riscv_setcond_case {
    ($name:ident, $cond:expr, $lhs:expr, $rhs:expr, $expect:expr) => {
        #[test]
        fn $name() {
            let mut cpu = RiscvCpuState::new();
            cpu.regs[1] = $lhs;
            cpu.regs[2] = $rhs;

            let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
                let tmp = ctx.new_temp(Type::I64);
                ctx.gen_insn_start(0x4200);
                ctx.gen_setcond(Type::I64, tmp, regs[1], regs[2], $cond);
                ctx.gen_mov(Type::I64, regs[3], tmp);
                ctx.gen_exit_tb(0);
            });

            assert_eq!(exit_val, 0);
            assert_eq!(cpu.regs[3], $expect);
        }
    };
}

macro_rules! riscv_branch_case {
    ($name:ident, $cond:expr, $lhs:expr, $rhs:expr, $taken:expr, $not:expr, $expect:expr) => {
        #[test]
        fn $name() {
            let mut cpu = RiscvCpuState::new();
            cpu.regs[1] = $lhs;
            cpu.regs[2] = $rhs;

            let exit_val = run_riscv_tb(&mut cpu, |ctx, _env, regs, _pc| {
                let label_taken = ctx.new_label();
                let label_end = ctx.new_label();
                let t_taken = ctx.new_temp(Type::I64);
                let t_not = ctx.new_temp(Type::I64);
                let c_taken = ctx.new_const(Type::I64, $taken);
                let c_not = ctx.new_const(Type::I64, $not);

                ctx.gen_insn_start(0x4300);
                ctx.gen_brcond(Type::I64, regs[1], regs[2], $cond, label_taken);
                ctx.gen_mov(Type::I64, t_not, c_not);
                ctx.gen_mov(Type::I64, regs[3], t_not);
                ctx.gen_br(label_end);

                ctx.gen_set_label(label_taken);
                ctx.gen_mov(Type::I64, t_taken, c_taken);
                ctx.gen_mov(Type::I64, regs[3], t_taken);

                ctx.gen_set_label(label_end);
                ctx.gen_exit_tb(0);
            });

            assert_eq!(exit_val, 0);
            assert_eq!(cpu.regs[3], $expect);
        }
    };
}

macro_rules! riscv_mem_case {
    ($name:ident, $offset:expr, $value:expr) => {
        #[test]
        fn $name() {
            let mut cpu = RiscvCpuStateMem::new();
            let exit_val = run_riscv_tb(&mut cpu, |ctx, env, regs, _pc| {
                let t_val = ctx.new_temp(Type::I64);
                let t_load = ctx.new_temp(Type::I64);
                let cval = ctx.new_const(Type::I64, $value);
                let mem_offset = std::mem::offset_of!(RiscvCpuStateMem, mem)
                    as i64
                    + $offset;

                ctx.gen_insn_start(0x4400);
                ctx.gen_mov(Type::I64, t_val, cval);
                ctx.gen_st(Type::I64, t_val, env, mem_offset);
                ctx.gen_ld(Type::I64, t_load, env, mem_offset);
                ctx.gen_mov(Type::I64, regs[4], t_load);
                ctx.gen_exit_tb(0);
            });

            assert_eq!(exit_val, 0);
            assert_eq!(cpu.regs[4], $value);
            let start = $offset as usize;
            let end = start + 8;
            let stored =
                u64::from_le_bytes(cpu.mem[start..end].try_into().unwrap());
            assert_eq!(stored, $value);
        }
    };
}

// Submodule declarations MUST come after macro
// definitions so the macros are visible to child
// modules.
mod advanced_tests;
mod basic_tests;
mod generated_and_complex_tests;
