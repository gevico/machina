// FullSystemCpu: GuestCpu bridge for full-system emulation.
//
// Wraps RiscvCpu + mmap'd RAM pointer, providing the
// GuestCpu trait required by the execution loop.

use machina_accel::ir::context::Context;
use machina_accel::ir::TempIdx;
use machina_accel::GuestCpu;
use machina_guest_riscv::riscv::cpu::RiscvCpu;
use machina_guest_riscv::riscv::ext::RiscvCfg;
use machina_guest_riscv::riscv::{RiscvDisasContext, RiscvTranslator};
use machina_guest_riscv::{translator_loop, DisasJumpType, TranslatorOps};

const NUM_GPRS: usize = 32;
const RAM_BASE: u64 = 0x8000_0000;

/// Full-system CPU wrapper bridging RiscvCpu to the
/// execution loop via the GuestCpu trait.
///
/// The `ram_ptr` field points to the host mmap'd region
/// backing guest RAM at `RAM_BASE`.  Instruction fetch
/// computes `guest_base = ram_ptr - RAM_BASE` so that
/// `guest_base + pc` yields the correct host address.
pub struct FullSystemCpu {
    pub cpu: RiscvCpu,
    ram_ptr: *const u8,
    ram_size: u64,
}

// SAFETY: ram_ptr points to mmap'd memory owned by an
// Arc<RamBlock> that outlives FullSystemCpu.
unsafe impl Send for FullSystemCpu {}

impl FullSystemCpu {
    /// Create a full-system CPU bridge.
    ///
    /// # Safety
    /// `ram_ptr` must point to valid mmap'd memory of
    /// `ram_size` bytes, representing guest RAM at
    /// `RAM_BASE` (0x8000_0000).
    pub unsafe fn new(
        cpu: RiscvCpu,
        ram_ptr: *const u8,
        ram_size: u64,
    ) -> Self {
        Self {
            cpu,
            ram_ptr,
            ram_size,
        }
    }
}

impl GuestCpu for FullSystemCpu {
    type IrContext = Context;

    fn get_pc(&self) -> u64 {
        self.cpu.pc
    }

    fn get_flags(&self) -> u32 {
        0
    }

    fn gen_code(&mut self, ir: &mut Context, pc: u64, max_insns: u32) -> u32 {
        // Compute guest_base so that guest_base + pc
        // yields ram_ptr + (pc - RAM_BASE).
        // Use wrapping arithmetic: the resulting pointer
        // is before the allocation but only dereferenced
        // at valid offsets (same pattern as QEMU
        // guest_base).
        let base = (self.ram_ptr as usize).wrapping_sub(RAM_BASE as usize)
            as *const u8;

        // Bounds check: PC must fall within RAM.
        let pc_offset = pc.wrapping_sub(RAM_BASE);
        if pc_offset >= self.ram_size {
            return 0;
        }
        let avail = (self.ram_size - pc_offset) / 4;
        let limit = max_insns.min(avail as u32);
        if limit == 0 {
            return 0;
        }

        let cfg = RiscvCfg::default();

        if ir.nb_globals() == 0 {
            // First TB: register globals via
            // translator_loop.
            let mut d = RiscvDisasContext::new(pc, base, cfg);
            d.base.max_insns = limit;
            translator_loop::<RiscvTranslator>(&mut d, ir);
            d.base.num_insns * 4
        } else {
            // Reuse existing globals (same order as
            // init_disas_context: env, gpr[0..32], pc).
            let mut d = RiscvDisasContext::new(pc, base, cfg);
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
