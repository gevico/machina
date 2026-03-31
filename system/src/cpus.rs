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
use machina_guest_riscv::riscv::{RiscvDisasContext, RiscvTranslator};
use machina_guest_riscv::{translator_loop, DisasJumpType, TranslatorOps};

const NUM_GPRS: usize = 32;
const RAM_BASE: u64 = 0x8000_0000;

/// Shared mip register for IRQ delivery from devices.
pub type SharedMip = Arc<AtomicU64>;

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
    /// `as_ref` must point to an AddressSpace that
    /// outlives FullSystemCpu.
    pub unsafe fn new(
        mut cpu: RiscvCpu,
        ram_ptr: *const u8,
        ram_size: u64,
        shared_mip: SharedMip,
        wfi_waker: Arc<WfiWaker>,
        as_ptr: *const machina_memory::address_space::AddressSpace,
    ) -> Self {
        cpu.guest_base =
            (ram_ptr as usize).wrapping_sub(RAM_BASE as usize) as u64;
        cpu.as_ptr = as_ptr as u64;
        cpu.ram_end = RAM_BASE + ram_size;
        Self {
            cpu,
            ram_ptr,
            ram_size,
            shared_mip,
            wfi_waker,
        }
    }

    pub fn shared_mip(&self) -> SharedMip {
        self.shared_mip.clone()
    }

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

    fn gen_code(&mut self, ir: &mut Context, pc: u64, max_insns: u32) -> u32 {
        let base = (self.ram_ptr as usize).wrapping_sub(RAM_BASE as usize)
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
            let mut d = RiscvDisasContext::new(pc, base, cfg);
            d.base.max_insns = limit;
            translator_loop::<RiscvTranslator>(&mut d, ir);
            d.base.num_insns * 4
        } else {
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

    // -- Full-system hooks --

    fn pending_interrupt(&self) -> bool {
        let dev_mip = self.shared_mip.load(Ordering::Relaxed);
        let effective = self.cpu.csr.mip | dev_mip;
        effective & self.cpu.csr.mie != 0
    }

    fn is_halted(&self) -> bool {
        self.cpu.halted.load(Ordering::Relaxed)
    }

    fn set_halted(&mut self, halted: bool) {
        self.cpu.halted.store(halted, Ordering::Relaxed);
    }

    fn privilege_level(&self) -> u8 {
        self.cpu.priv_level as u8
    }

    fn handle_interrupt(&mut self) {
        let dev_mip = self.shared_mip.load(Ordering::Relaxed);
        let saved = self.cpu.csr.mip;
        self.cpu.csr.mip = saved | dev_mip;
        self.cpu.handle_interrupt();
        self.cpu.csr.mip &= !dev_mip;
    }

    fn handle_exception(&mut self, excp: u32, tval: u64) {
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
        self.wfi_waker.wait()
    }

    fn handle_priv_csr(&mut self) -> bool {
        // Read the 32-bit instruction at current PC.
        let pc = self.cpu.pc;
        let pc_off = pc.wrapping_sub(RAM_BASE);
        if pc_off >= self.ram_size {
            return false;
        }
        let insn = unsafe {
            let ptr = self.ram_ptr.add(pc_off as usize);
            *(ptr as *const u32)
        };
        // Decode CSR instruction fields:
        //   [31:20] = csr, [19:15] = rs1, [14:12] = funct3,
        //   [11:7] = rd, [6:0] = opcode
        let opcode = insn & 0x7F;
        if opcode != 0x73 {
            return false; // not SYSTEM opcode
        }
        let funct3 = (insn >> 12) & 0x7;
        if funct3 == 0 {
            return false; // ECALL/EBREAK, not CSR
        }
        let rd = ((insn >> 7) & 0x1F) as usize;
        let rs1_idx = ((insn >> 15) & 0x1F) as usize;
        let csr_addr = (insn >> 20) as u16;

        let priv_level = self.cpu.priv_level;
        let rs1_val = if funct3 >= 5 {
            // Immediate forms: rs1 field is the zimm.
            rs1_idx as u64
        } else {
            if rs1_idx == 0 { 0 } else { self.cpu.gpr[rs1_idx] }
        };

        let old = match self
            .cpu
            .csr
            .read(csr_addr, priv_level)
        {
            Ok(v) => v,
            Err(_) => return false,
        };

        // Compute new value based on funct3.
        let new_val = match funct3 {
            1 | 5 => rs1_val,            // CSRRW / CSRRWI
            2 | 6 => old | rs1_val,      // CSRRS / CSRRSI
            3 | 7 => old & !rs1_val,     // CSRRC / CSRRCI
            _ => return false,
        };

        // Write only if rs1 != 0 for RS/RC variants,
        // always for RW.
        let do_write = match funct3 {
            1 | 5 => true,
            2 | 3 | 6 | 7 => rs1_idx != 0,
            _ => false,
        };

        if do_write {
            if self
                .cpu
                .csr
                .write(csr_addr, new_val, priv_level)
                .is_err()
            {
                return false;
            }
        }

        if rd != 0 {
            self.cpu.gpr[rd] = old;
        }

        // Advance PC past the CSR instruction (4 bytes).
        self.cpu.pc += 4;
        true
    }
}

// ---- JIT memory helpers ----
//
// Called from JIT-generated code when a guest memory
// address falls outside the RAM window. The fast path
// (ram_base <= addr < ram_end) stays inline in the JIT;
// only out-of-window accesses land here.
//
// These helpers read AddressSpace pointer and RAM bounds
// directly from the RiscvCpu struct (via env pointer),
// eliminating any process-global state.

use machina_core::address::GPA;
use machina_memory::address_space::AddressSpace;

/// Byte offset of `as_ptr` within RiscvCpu.
fn as_ptr_offset() -> usize {
    // as_ptr is at: offset_of(RiscvCpu, as_ptr)
    // We compute it by taking the address diff.
    let dummy = RiscvCpu::new();
    let base = &dummy as *const RiscvCpu as usize;
    let field = &dummy.as_ptr as *const u64 as usize;
    field - base
}

/// Byte offset of `ram_end` within RiscvCpu.
fn ram_end_offset() -> usize {
    let dummy = RiscvCpu::new();
    let base = &dummy as *const RiscvCpu as usize;
    let field = &dummy.ram_end as *const u64 as usize;
    field - base
}

#[no_mangle]
pub unsafe extern "C" fn machina_mem_read(
    env: *mut u8,
    addr: u64,
    size: u32,
) -> u64 {
    let end = *(env.add(ram_end_offset()) as *const u64);
    if addr >= RAM_BASE && addr < end {
        let gb_off =
            machina_guest_riscv::riscv::cpu::GUEST_BASE_OFFSET as usize;
        let gb = *(env.add(gb_off) as *const u64);
        let ptr = (gb + addr) as *const u8;
        match size {
            1 => *ptr as u64,
            2 => *(ptr as *const u16) as u64,
            4 => *(ptr as *const u32) as u64,
            8 => *(ptr as *const u64),
            _ => 0,
        }
    } else {
        let asp = *(env.add(as_ptr_offset()) as *const u64);
        if asp != 0 {
            let as_ = &*(asp as *const AddressSpace);
            as_.read(GPA::new(addr), size)
        } else {
            0
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn machina_mem_write(
    env: *mut u8,
    addr: u64,
    val: u64,
    size: u32,
) {
    let end = *(env.add(ram_end_offset()) as *const u64);
    if addr >= RAM_BASE && addr < end {
        let gb_off =
            machina_guest_riscv::riscv::cpu::GUEST_BASE_OFFSET as usize;
        let gb = *(env.add(gb_off) as *const u64);
        let ptr = (gb + addr) as *mut u8;
        match size {
            1 => *ptr = val as u8,
            2 => *(ptr as *mut u16) = val as u16,
            4 => *(ptr as *mut u32) = val as u32,
            8 => *(ptr as *mut u64) = val,
            _ => {}
        }
    } else {
        let asp = *(env.add(as_ptr_offset()) as *const u64);
        if asp != 0 {
            let as_ = &*(asp as *const AddressSpace);
            as_.write(GPA::new(addr), size, val);
        }
    }
}
