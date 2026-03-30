// FullSystemCpu: GuestCpu bridge for full-system emulation.
//
// Owns a RiscvCpu with guest_base set for JIT memory
// access. The machine's IRQ sinks update mip on a shared
// AtomicU64, which the exec loop polls via
// pending_interrupt().

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use machina_core::wfi::WfiWaker;

use machina_accel::ir::context::Context;
use machina_accel::ir::TempIdx;
use machina_accel::GuestCpu;
use machina_guest_riscv::riscv::cpu::RiscvCpu;
use machina_guest_riscv::riscv::exception::Exception;
use machina_guest_riscv::riscv::ext::RiscvCfg;
use machina_guest_riscv::riscv::{
    RiscvDisasContext, RiscvTranslator,
};
use machina_guest_riscv::{
    translator_loop, DisasJumpType, TranslatorOps,
};

const NUM_GPRS: usize = 32;
const RAM_BASE: u64 = 0x8000_0000;

/// Shared mip register for IRQ delivery from devices.
/// Devices write to this atomically; the exec loop reads
/// it in pending_interrupt() and syncs to the CPU's CSR.
pub type SharedMip = Arc<AtomicU64>;

/// Create a new shared mip register.
pub fn new_shared_mip() -> SharedMip {
    Arc::new(AtomicU64::new(0))
}

/// Full-system CPU wrapper bridging RiscvCpu to the
/// execution loop via the GuestCpu trait.
pub struct FullSystemCpu {
    pub cpu: RiscvCpu,
    ram_ptr: *const u8,
    ram_size: u64,
    shared_mip: SharedMip,
    wfi_waker: Arc<WfiWaker>,
}

// SAFETY: ram_ptr points to mmap'd memory owned by
// Arc<RamBlock> that outlives FullSystemCpu.
unsafe impl Send for FullSystemCpu {}

impl FullSystemCpu {
    /// Create a full-system CPU bridge.
    ///
    /// # Safety
    /// `ram_ptr` must point to valid mmap'd memory of
    /// `ram_size` bytes backing guest RAM at RAM_BASE.
    pub unsafe fn new(
        mut cpu: RiscvCpu,
        ram_ptr: *const u8,
        ram_size: u64,
        shared_mip: SharedMip,
        wfi_waker: Arc<WfiWaker>,
    ) -> Self {
        cpu.guest_base = (ram_ptr as usize)
            .wrapping_sub(RAM_BASE as usize)
            as u64;
        Self {
            cpu,
            ram_ptr,
            ram_size,
            shared_mip,
            wfi_waker,
        }
    }

    /// Get a clone of the shared mip for IRQ sinks.
    pub fn shared_mip(&self) -> SharedMip {
        self.shared_mip.clone()
    }

    /// Get a clone of the WFI waker for IRQ sinks.
    pub fn wfi_waker(&self) -> Arc<WfiWaker> {
        self.wfi_waker.clone()
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

    fn gen_code(
        &mut self,
        ir: &mut Context,
        pc: u64,
        max_insns: u32,
    ) -> u32 {
        let base = (self.ram_ptr as usize)
            .wrapping_sub(RAM_BASE as usize)
            as *const u8;

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
            let mut d =
                RiscvDisasContext::new(pc, base, cfg);
            d.base.max_insns = limit;
            translator_loop::<RiscvTranslator>(
                &mut d, ir,
            );
            d.base.num_insns * 4
        } else {
            let mut d =
                RiscvDisasContext::new(pc, base, cfg);
            d.base.max_insns = limit;
            d.env = TempIdx(0);
            for i in 0..NUM_GPRS {
                d.gpr[i] = TempIdx(1 + i as u32);
            }
            d.pc = TempIdx(1 + NUM_GPRS as u32);
            RiscvTranslator::tb_start(&mut d, ir);
            loop {
                RiscvTranslator::insn_start(&mut d, ir);
                RiscvTranslator::translate_insn(
                    &mut d, ir,
                );
                if d.base.is_jmp != DisasJumpType::Next
                {
                    break;
                }
                if d.base.num_insns >= d.base.max_insns
                {
                    d.base.is_jmp =
                        DisasJumpType::TooMany;
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

    // -- Full-system hooks --

    fn pending_interrupt(&self) -> bool {
        // Combine software mip (CPU-set bits like SSIP)
        // with device mip (PLIC/ACLINT-driven bits).
        // Don't modify csr.mip — just check.
        let dev_mip =
            self.shared_mip.load(Ordering::Relaxed);
        let effective = self.cpu.csr.mip | dev_mip;
        effective & self.cpu.csr.mie != 0
    }

    fn is_halted(&self) -> bool {
        self.cpu
            .halted
            .load(Ordering::Relaxed)
    }

    fn set_halted(&mut self, halted: bool) {
        self.cpu
            .halted
            .store(halted, Ordering::Relaxed);
    }

    fn privilege_level(&self) -> u8 {
        self.cpu.priv_level as u8
    }

    fn handle_interrupt(&mut self) {
        // Merge device bits into csr.mip temporarily
        // for the interrupt handler, then restore the
        // software-only mip. Device bit presence is
        // always re-evaluated from shared_mip on the
        // next pending_interrupt() call.
        let dev_mip =
            self.shared_mip.load(Ordering::Relaxed);
        let saved = self.cpu.csr.mip;
        self.cpu.csr.mip = saved | dev_mip;
        self.cpu.handle_interrupt();
        // Restore software-only bits. The handler may
        // have cleared some bits (e.g., SSIP); preserve
        // those clears but remove device bits.
        self.cpu.csr.mip &= !dev_mip;
    }

    fn handle_exception(
        &mut self,
        excp: u32,
        tval: u64,
    ) {
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

    fn execute_mret(&mut self) {
        self.cpu.execute_mret();
    }

    fn execute_sret(&mut self) {
        self.cpu.execute_sret();
    }

    fn tlb_flush(&mut self) {}

    fn tlb_flush_page(&mut self, _vpn: u64) {}

    fn wait_for_interrupt(&self) -> bool {
        // Block indefinitely on condvar until a device
        // IRQ arrives. The IRQ sink calls wake() after
        // updating SharedMip.
        self.wfi_waker.wait()
    }
}
