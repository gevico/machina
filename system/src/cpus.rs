// FullSystemCpu: GuestCpu bridge for full-system emulation.
//
// Owns a RiscvCpu with guest_base set for JIT memory
// access. The machine's IRQ sinks update mip on a shared
// AtomicU64, which the exec loop polls via
// pending_interrupt().

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use machina_core::wfi::WfiWaker;

use machina_accel::ir::context::Context;
use machina_accel::ir::TempIdx;
use machina_accel::GuestCpu;
use machina_guest_riscv::riscv::cpu::RiscvCpu;
use machina_guest_riscv::riscv::csr::PrivLevel;
use machina_guest_riscv::riscv::exception::Exception;
use machina_guest_riscv::riscv::ext::RiscvCfg;
use machina_guest_riscv::riscv::{RiscvDisasContext, RiscvTranslator};
use machina_guest_riscv::{translator_loop, DisasJumpType, TranslatorOps};

const NUM_GPRS: usize = 32;
pub const RAM_BASE: u64 = 0x8000_0000;
const MSTATUS_SIE: u64 = 1 << 1;
const MSTATUS_MIE: u64 = 1 << 50;

/// Compute the byte offset of the TLB Box pointer from
/// the start of RiscvCpu (env pointer). Used by the JIT
/// to emit inline TLB lookups.
pub fn tlb_ptr_offset() -> usize {
    let dummy = RiscvCpu::new();
    let base = &dummy as *const RiscvCpu as usize;
    let field = &dummy.mmu.tlb as *const _ as usize;
    let offset = field - base;
    // Verify: loading from [env + offset] should yield
    // the pointer to the TlbEntry array.
    let env_ptr = &dummy as *const RiscvCpu as *const u8;
    let loaded = unsafe { *(env_ptr.add(offset) as *const usize) };
    let actual = &*dummy.mmu.tlb as *const _ as usize;
    debug_assert_eq!(loaded, actual, "TLB Box ptr offset mismatch");
    offset
}

/// Re-export TLB layout constants for JIT configuration.
pub use machina_guest_riscv::riscv::mmu::tlb_offsets;
pub use machina_guest_riscv::riscv::mmu::TLB_SIZE;

/// Compute the byte offset of `mem_fault_cause` within
/// RiscvCpu. Used by the JIT to check for helper faults.
pub fn fault_cause_offset() -> usize {
    let dummy = RiscvCpu::new();
    let base = &dummy as *const RiscvCpu as usize;
    let field = &dummy.mem_fault_cause as *const u64 as usize;
    field - base
}

/// Byte offset of `fault_pc` within RiscvCpu.
pub fn fault_pc_offset() -> usize {
    let dummy = RiscvCpu::new();
    let base = &dummy as *const RiscvCpu as usize;
    let field = &dummy.fault_pc as *const u64 as usize;
    let off = field - base;
    // Verify no overlap with adjacent fields.
    let fc_off = fault_cause_offset();
    debug_assert!(off.abs_diff(fc_off) >= 8, "fault_pc overlaps fault_cause",);
    off
}

/// Last translated TB PC for crash diagnosis.
pub static LAST_TB_PC: AtomicU64 = AtomicU64::new(0);

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
    mrom_ptr: *const u8,
    mrom_base: u64,
    mrom_size: u64,
    shared_mip: SharedMip,
    wfi_waker: Arc<WfiWaker>,
    stop_flag: Arc<AtomicBool>,
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
        stop_flag: Arc<AtomicBool>,
    ) -> Self {
        cpu.guest_base =
            (ram_ptr as usize).wrapping_sub(RAM_BASE as usize) as u64;
        cpu.as_ptr = as_ptr as u64;
        cpu.ram_end = RAM_BASE + ram_size;
        Self {
            cpu,
            ram_ptr,
            ram_size,
            mrom_ptr: std::ptr::null(),
            mrom_base: 0,
            mrom_size: 0,
            shared_mip,
            wfi_waker,
            stop_flag,
        }
    }

    /// Read ACLINT mtime register via AddressSpace MMIO.
    fn read_aclint_mtime(&self) -> u64 {
        const ACLINT_MTIME: u64 = 0x0200_BFF8;
        let asp = self.cpu.as_ptr;
        if asp == 0 {
            return 0;
        }
        unsafe {
            let as_ =
                &*(asp as *const AddressSpace);
            as_.read(GPA::new(ACLINT_MTIME), 8)
        }
    }

    /// Resolve a physical address to (host_ptr, base, size)
    /// for instruction fetch.
    fn resolve_fetch_region(
        &self,
        pa: u64,
    ) -> (*const u8, u64, u64) {
        let ram_off = pa.wrapping_sub(RAM_BASE);
        if ram_off < self.ram_size {
            return (self.ram_ptr, RAM_BASE, self.ram_size);
        }
        if !self.mrom_ptr.is_null() {
            let mrom_off = pa.wrapping_sub(self.mrom_base);
            if mrom_off < self.mrom_size {
                return (
                    self.mrom_ptr,
                    self.mrom_base,
                    self.mrom_size,
                );
            }
        }
        (std::ptr::null(), 0, 0)
    }

    /// Register MROM region for instruction fetch.
    pub fn set_mrom(
        &mut self,
        ptr: *const u8,
        base: u64,
        size: u64,
    ) {
        self.mrom_ptr = ptr;
        self.mrom_base = base;
        self.mrom_size = size;
    }

    /// Translate a virtual PC to physical address for
    /// instruction fetch. Returns u64::MAX on fault
    /// (fault cause is latched).
    fn translate_pc(&mut self, vpc: u64) -> u64 {
        let mode = self.cpu.mmu.satp_mode();
        if mode == 0 {
            // BARE mode: VA == PA. PMP check for execute.
            use machina_guest_riscv::riscv::mmu::AccessType;
            match self.cpu.pmp.check_access(
                vpc,
                2, // minimum fetch size
                AccessType::Execute,
                self.cpu.priv_level,
            ) {
                Ok(()) => vpc,
                Err(_) => {
                    self.cpu.mem_fault_cause = 1;
                    self.cpu.mem_fault_tval = vpc;
                    u64::MAX
                }
            }
        } else {
            // Sv39: translate via MMU.
            use machina_guest_riscv::riscv::mmu::AccessType;
            let priv_level = self.cpu.priv_level;
            let mstatus = self.cpu.csr.mstatus;
            let cpu_ptr = &mut self.cpu as *mut RiscvCpu;
            let mem_read =
                |pa: u64| -> u64 { unsafe { read_phys(cpu_ptr, pa) } };
            let mut mem_write =
                |pa: u64, val: u64| unsafe { write_phys(cpu_ptr, pa, val) };
            let pmp = unsafe { &(*cpu_ptr).pmp };
            match self.cpu.mmu.translate_miss(
                vpc,
                AccessType::Execute,
                priv_level,
                mstatus,
                2,
                Some(pmp),
                &mem_read,
                &mut mem_write,
            ) {
                Ok(pa) => pa,
                Err(e) => {
                    self.cpu.mem_fault_cause = match e {
                        Exception::InstructionPageFault => 12,
                        Exception::InstructionAccessFault => 1,
                        _ => 12,
                    };
                    self.cpu.mem_fault_tval = vpc;
                    u64::MAX
                }
            }
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
        let priv_bits = self.cpu.priv_level as u32;
        let satp_mode = (self.cpu.mmu.get_satp() >> 60) as u32 & 0xF;
        priv_bits | (satp_mode << 2)
    }

    fn gen_code(&mut self, ir: &mut Context, pc: u64, max_insns: u32) -> u32 {
        LAST_TB_PC.store(pc, Ordering::Relaxed);

        // Clear fault_pc so fetch faults don't inherit
        // a stale value from a prior data instruction.
        self.cpu.fault_pc = 0;

        // Translate virtual PC to physical PC via MMU.
        let phys_pc = self.translate_pc(pc);
        if phys_pc == u64::MAX {
            // Fetch fault — latched in mem_fault_cause.
            return 0;
        }

        // Store phys_pc for the exec loop to record in TB.
        self.cpu.last_phys_pc = phys_pc;

        // Resolve phys_pc to host pointer and region size.
        let (region_ptr, region_base, region_size) = {
            let ram_off = phys_pc.wrapping_sub(RAM_BASE);
            if ram_off < self.ram_size {
                (self.ram_ptr, RAM_BASE, self.ram_size)
            } else if !self.mrom_ptr.is_null() {
                let mrom_off =
                    phys_pc.wrapping_sub(self.mrom_base);
                if mrom_off < self.mrom_size {
                    (
                        self.mrom_ptr,
                        self.mrom_base,
                        self.mrom_size,
                    )
                } else {
                    self.cpu.mem_fault_cause = 1;
                    self.cpu.mem_fault_tval = pc;
                    return 0;
                }
            } else {
                self.cpu.mem_fault_cause = 1;
                self.cpu.mem_fault_tval = pc;
                return 0;
            }
        };
        let phys_offset = phys_pc.wrapping_sub(region_base);

        // TB must not cross physical page boundary
        // (AC-10). Limit avail to remaining bytes in
        // the current 4K page.
        let page_remain = 4096 - (phys_pc & 0xFFF);
        let avail_bytes =
            page_remain.min(region_size - phys_offset);
        // Allow 2-byte (compressed) instructions.
        let avail = avail_bytes / 2;
        let limit = max_insns.min(avail as u32);
        if limit == 0 {
            return 0;
        }

        // Use phys_pc-based pointer for instruction fetch.
        let base = (region_ptr as usize)
            .wrapping_sub(region_base as usize)
            as *const u8;
        let base = (base as usize)
            .wrapping_add(phys_pc as usize)
            .wrapping_sub(pc as usize) as *const u8;

        // AC-11: Pre-fetch cross-page 32-bit instruction.
        // When the page has an odd number of halfwords
        // (page_remain % 4 == 2), the last 2 bytes might
        // be the first half of a 32-bit instruction.
        let cross_page = if page_remain % 4 == 2 {
            let boundary_pa = phys_pc + page_remain - 2;
            let boundary_off =
                boundary_pa.wrapping_sub(region_base);
            if boundary_off < region_size {
                let lo = unsafe {
                    let p =
                        region_ptr.add(boundary_off as usize);
                    (p as *const u16).read_unaligned()
                };
                let is_32bit = (lo & 0x3) == 0x3
                    && ((lo >> 2) & 0x7) != 0x7;
                if is_32bit {
                    let next_vpc = pc + page_remain;
                    let sfc = self.cpu.mem_fault_cause;
                    let sft = self.cpu.mem_fault_tval;
                    let sfp = self.cpu.fault_pc;
                    let next_phys =
                        self.translate_pc(next_vpc);
                    if next_phys == u64::MAX {
                        0u32
                    } else {
                        self.cpu.mem_fault_cause = sfc;
                        self.cpu.mem_fault_tval = sft;
                        self.cpu.fault_pc = sfp;
                        let (np, nb, ns) =
                            self.resolve_fetch_region(
                                next_phys,
                            );
                        let noff = next_phys
                            .wrapping_sub(nb);
                        if np.is_null() || noff >= ns {
                            self.cpu.mem_fault_cause = 1;
                            self.cpu.mem_fault_tval =
                                pc + page_remain;
                            0u32
                        } else {
                            let hi = unsafe {
                                let p = np
                                    .add(noff as usize);
                                (p as *const u16)
                                    .read_unaligned()
                            };
                            (lo as u32)
                                | ((hi as u32) << 16)
                        }
                    }
                } else {
                    0u32
                }
            } else {
                0u32
            }
        } else {
            0u32
        };

        // If page B fetch failed, a fault is latched
        // (mem_fault_cause != 0). Return immediately
        // to prevent the translator from falling back
        // to raw pointer fetch_insn32().
        if self.cpu.mem_fault_cause != 0 {
            return 0;
        }

        let cfg = RiscvCfg::default();

        // The virtual PC of the boundary instruction
        // (last 2 bytes of page A).
        let xpage_pc = if cross_page != 0 {
            pc + page_remain - 2
        } else {
            0
        };

        if ir.nb_globals() == 0 {
            let mut d = RiscvDisasContext::new(pc, base, cfg);
            d.base.max_insns = limit;
            d.cross_page_insn = cross_page;
            d.cross_page_pc = xpage_pc;
            translator_loop::<RiscvTranslator>(&mut d, ir);
            d.base.num_insns * 4
        } else {
            let mut d = RiscvDisasContext::new(pc, base, cfg);
            d.base.max_insns = limit;
            d.cross_page_insn = cross_page;
            d.cross_page_pc = xpage_pc;
            d.env = TempIdx(0);
            for i in 0..NUM_GPRS {
                d.gpr[i] = TempIdx(1 + i as u32);
            }
            d.pc = TempIdx(1 + NUM_GPRS as u32);
            d.load_res = ir.new_global(
                machina_accel::ir::types::Type::I64,
                d.env,
                machina_guest_riscv::riscv::cpu::LOAD_RES_OFFSET,
                "load_res",
            );
            d.load_val = ir.new_global(
                machina_accel::ir::types::Type::I64,
                d.env,
                machina_guest_riscv::riscv::cpu::LOAD_VAL_OFFSET,
                "load_val",
            );
            d.fault_pc = ir.new_global(
                machina_accel::ir::types::Type::I64,
                d.env,
                fault_pc_offset() as i64,
                "fault_pc",
            );
            d.csr_helper =
                machina_csr_op as *const () as u64;
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
        let pending =
            (self.cpu.csr.mip | dev_mip) & self.cpu.csr.mie;
        if pending == 0 {
            return false;
        }

        let cur_priv = self.cpu.priv_level as u64;
        for irq in [11u64, 3, 7, 9, 1, 5] {
            let bit = 1u64 << irq;
            if pending & bit == 0 {
                continue;
            }

            let delegated =
                (self.cpu.csr.mideleg >> irq) & 1 != 0;
            if delegated {
                let s = PrivLevel::Supervisor as u64;
                return cur_priv < s
                    || (cur_priv == s
                        && self.cpu.csr.mstatus & MSTATUS_SIE
                            != 0);
            }

            let m = PrivLevel::Machine as u64;
            return cur_priv < m
                || (cur_priv == m
                    && self.cpu.csr.mstatus & MSTATUS_MIE
                        != 0);
        }

        false
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

    fn execute_sret(&mut self) -> bool {
        self.cpu.execute_sret()
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

    fn tlb_flush_page(&mut self, vpn: u64) {
        self.cpu.mmu.flush_page(vpn);
    }

    fn should_exit(&self) -> bool {
        !self.stop_flag.load(Ordering::Relaxed)
    }

    fn check_mem_fault(&mut self) -> bool {
        let cause = self.cpu.mem_fault_cause;
        if cause != 0 {
            let tval = self.cpu.mem_fault_tval;
            self.cpu.mem_fault_cause = 0;
            self.cpu.mem_fault_tval = 0;
            // Use fault_pc for precise mepc.
            if self.cpu.fault_pc != 0 {
                self.cpu.pc = self.cpu.fault_pc;
                self.cpu.fault_pc = 0;
            }
            self.handle_exception(cause as u32, tval);
            true
        } else {
            false
        }
    }

    fn take_tb_flush_pending(&mut self) -> bool {
        let pending = self.cpu.tb_flush_pending;
        self.cpu.tb_flush_pending = false;
        pending
    }

    fn last_phys_pc(&self) -> u64 {
        self.cpu.last_phys_pc
    }

    fn take_dirty_pages(&mut self) -> Vec<u64> {
        // Combine helper-tracked dirty pages with
        // TLB-tracked dirty pages (JIT fast-path).
        let mut pages = std::mem::take(&mut self.cpu.dirty_pages);
        let tlb_pages = self.cpu.mmu.take_dirty_tlb_pages();
        for p in tlb_pages {
            if !pages.contains(&p) {
                pages.push(p);
            }
        }
        pages
    }

    fn wait_for_interrupt(&self) -> bool {
        self.wfi_waker.wait()
    }

    fn handle_priv_csr(&mut self) -> bool {
        let pc = self.cpu.pc;
        self.cpu.fault_pc = 0;
        let phys_pc = self.translate_pc(pc);
        if phys_pc == u64::MAX {
            return self.cpu.mem_fault_cause != 0;
        }
        let (rp, rb, rs) =
            self.resolve_fetch_region(phys_pc);
        let off = phys_pc.wrapping_sub(rb);
        if rp.is_null() || off >= rs {
            self.cpu.mem_fault_cause = 1;
            self.cpu.mem_fault_tval = pc;
            return true;
        }
        let insn = unsafe {
            let ptr = rp.add(off as usize);
            std::ptr::read_unaligned(ptr as *const u32)
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
            if rs1_idx == 0 {
                0
            } else {
                self.cpu.gpr[rs1_idx]
            }
        };

        let old = match csr_addr {
            machina_guest_riscv::riscv::csr::CSR_TIME
            | machina_guest_riscv::riscv::csr::CSR_CYCLE => {
                self.read_aclint_mtime()
            }
            machina_guest_riscv::riscv::csr::CSR_INSTRET => {
                self.cpu.csr.instret
            }
            _ => match self.cpu.csr.read(csr_addr, priv_level)
            {
                Ok(v) => v,
                Err(_) => return false,
            },
        };

        // Compute new value based on funct3.
        let new_val = match funct3 {
            1 | 5 => rs1_val,        // CSRRW / CSRRWI
            2 | 6 => old | rs1_val,  // CSRRS / CSRRSI
            3 | 7 => old & !rs1_val, // CSRRC / CSRRCI
            _ => return false,
        };

        // Write only if rs1 != 0 for RS/RC variants,
        // always for RW.
        let do_write = match funct3 {
            1 | 5 => true,
            2 | 3 | 6 | 7 => rs1_idx != 0,
            _ => false,
        };

        if do_write
            && self.cpu.csr.write(csr_addr, new_val, priv_level).is_err()
        {
            return false;
        }

        // Sync runtime state after privileged CSR writes.
        if do_write {
            use machina_guest_riscv::riscv::csr::{
                CSR_PMPADDR0, CSR_PMPCFG0, CSR_SATP, PMP_COUNT,
            };
            let is_pmp = (CSR_PMPCFG0..=CSR_PMPCFG0 + 3).contains(&csr_addr)
                || (CSR_PMPADDR0..CSR_PMPADDR0 + PMP_COUNT as u16)
                    .contains(&csr_addr);
            if is_pmp {
                self.cpu
                    .pmp
                    .sync_from_csr(&self.cpu.csr.pmpcfg, &self.cpu.csr.pmpaddr);
            }
            if csr_addr == CSR_SATP {
                self.cpu.mmu.set_satp(new_val);
                self.cpu.mmu.flush();
                self.cpu.tb_flush_pending = true;
            }
        }

        if rd != 0 {
            self.cpu.gpr[rd] = old;
        }

        self.cpu.pc += 4;
        true
    }
}

// ---- JIT memory helpers ----
//
// The TLB fast path in JIT-generated code checks the TLB
// tag and addend inline. On TLB miss (or MMIO sentinel),
// the slow path helper is called. It translates the
// guest virtual address through the MMU, fills the TLB
// entry with the host addend (or MMIO sentinel), and
// performs the actual memory access.

use machina_core::address::GPA;
use machina_guest_riscv::riscv::mmu::{AccessType, PAGE_MASK, TLB_MMIO_ADDEND};
use machina_memory::address_space::AddressSpace;

/// Translate a guest virtual address to physical,
/// filling the TLB. Returns the PA on success.
/// On fault, latches cause/tval and returns None.
unsafe fn translate_for_helper(
    cpu: &mut RiscvCpu,
    gva: u64,
    access: AccessType,
    size: u32,
) -> Option<u64> {
    let mode = cpu.mmu.satp_mode();
    if mode == 0 {
        // BARE mode (M-mode): VA == PA.
        // PMP check.
        match cpu
            .pmp
            .check_access(gva, size as u64, access, cpu.priv_level)
        {
            Ok(()) => {}
            Err(e) => {
                cpu.mem_fault_cause = match e {
                    Exception::LoadAccessFault => 5,
                    Exception::StoreAccessFault => 7,
                    Exception::InstructionAccessFault => 1,
                    _ => 5,
                };
                cpu.mem_fault_tval = gva;
                return None;
            }
        }
        // Fill TLB with identity mapping.
        let ram_end = cpu.ram_end;
        let addend = if gva >= RAM_BASE && gva < ram_end {
            cpu.guest_base as usize
        } else {
            TLB_MMIO_ADDEND
        };
        cpu.mmu.fill_identity(gva, addend);
        Some(gva)
    } else {
        // Sv39: full MMU translation.
        let priv_level = cpu.priv_level;
        let mstatus = cpu.csr.mstatus;
        let ram_end = cpu.ram_end;
        let guest_base = cpu.guest_base;

        // Use raw pointer to avoid borrow conflicts
        // with the closures capturing cpu.
        let cpu_ptr = cpu as *mut RiscvCpu;
        let mem_read =
            |pa: u64| -> u64 { read_phys(cpu_ptr as *const RiscvCpu, pa) };
        let mut mem_write = |pa: u64, val: u64| {
            write_phys(cpu_ptr, pa, val);
        };

        let pmp = &(*cpu_ptr).pmp;
        match cpu.mmu.translate_miss(
            gva,
            access,
            priv_level,
            mstatus,
            size as u64,
            Some(pmp),
            &mem_read,
            &mut mem_write,
        ) {
            Ok(pa) => {
                let addend = if pa >= RAM_BASE && pa < ram_end {
                    let gva_page = gva & PAGE_MASK;
                    let pa_page = pa & PAGE_MASK;
                    (guest_base as usize)
                        .wrapping_add(pa_page as usize)
                        .wrapping_sub(gva_page as usize)
                } else {
                    TLB_MMIO_ADDEND
                };
                cpu.mmu.fill_addend(gva, addend);
                Some(pa)
            }
            Err(e) => {
                cpu.mem_fault_cause = match e {
                    Exception::LoadPageFault => 13,
                    Exception::StorePageFault => 15,
                    Exception::InstructionPageFault => 12,
                    Exception::LoadAccessFault => 5,
                    Exception::StoreAccessFault => 7,
                    Exception::InstructionAccessFault => 1,
                    _ => 5,
                };
                cpu.mem_fault_tval = gva;
                None
            }
        }
    }
}

/// Read from guest physical memory (RAM or MMIO).
unsafe fn read_phys_sized(cpu: *const RiscvCpu, pa: u64, size: u32) -> u64 {
    let cpu_ref = &*cpu;
    let ram_end = cpu_ref.ram_end;
    if pa >= RAM_BASE && pa < ram_end {
        let gb = cpu_ref.guest_base;
        let ptr = (gb + pa) as *const u8;
        match size {
            1 => *ptr as u64,
            2 => (ptr as *const u16).read_unaligned() as u64,
            4 => (ptr as *const u32).read_unaligned() as u64,
            8 => (ptr as *const u64).read_unaligned(),
            _ => 0,
        }
    } else {
        let asp = cpu_ref.as_ptr;
        if asp != 0 {
            let as_ = &*(asp as *const AddressSpace);
            as_.read(GPA::new(pa), size)
        } else {
            0
        }
    }
}

/// Write to guest physical memory (RAM or MMIO).
unsafe fn write_phys_sized(cpu: *mut RiscvCpu, pa: u64, val: u64, size: u32) {
    let cpu_ref = &*cpu;
    let ram_end = cpu_ref.ram_end;
    if pa >= RAM_BASE && pa < ram_end {
        let gb = cpu_ref.guest_base;
        let ptr = (gb + pa) as *mut u8;
        match size {
            1 => *ptr = val as u8,
            2 => (ptr as *mut u16).write_unaligned(val as u16),
            4 => (ptr as *mut u32).write_unaligned(val as u32),
            8 => (ptr as *mut u64).write_unaligned(val),
            _ => {}
        }
        // Track dirty page for fence.i invalidation.
        let page = pa >> 12;
        let cpu_mut = &mut *cpu;
        if !cpu_mut.dirty_pages.contains(&page) {
            cpu_mut.dirty_pages.push(page);
        }
    } else {
        let asp = cpu_ref.as_ptr;
        if asp != 0 {
            let as_ = &*(asp as *const AddressSpace);
            as_.write(GPA::new(pa), size, val);
        }
    }
}

/// Read 8 bytes from guest physical memory (for page
/// table walks).
unsafe fn read_phys(cpu: *const RiscvCpu, pa: u64) -> u64 {
    read_phys_sized(cpu, pa, 8)
}

/// Write 8 bytes to guest physical memory (for PTE A/D
/// bit updates).
unsafe fn write_phys(cpu: *mut RiscvCpu, pa: u64, val: u64) {
    write_phys_sized(cpu, pa, val, 8);
}

/// Check whether a physical address range is backed by
/// RAM or a mapped MMIO device.
fn is_phys_backed(cpu: &RiscvCpu, pa: u64, size: u32) -> bool {
    if pa >= RAM_BASE
        && pa.checked_add(size as u64)
            .is_some_and(|end| end <= cpu.ram_end)
    {
        return true;
    }
    let asp = cpu.as_ptr;
    if asp == 0 {
        return false;
    }
    let as_ = unsafe { &*(asp as *const AddressSpace) };
    as_.is_mapped(GPA::new(pa), size)
}

/// JIT slow path: guest load (TLB miss or MMIO).
///
/// # Safety
/// `env` must point to a valid `RiscvCpu`.
#[no_mangle]
pub unsafe extern "C" fn machina_mem_read(
    env: *mut u8,
    gva: u64,
    size: u32,
) -> u64 {
    let cpu = &mut *(env as *mut RiscvCpu);
    match translate_for_helper(
        cpu,
        gva,
        AccessType::Read,
        size,
    ) {
        Some(pa) => {
            if !is_phys_backed(cpu, pa, size) {
                cpu.mem_fault_cause = 5;
                cpu.mem_fault_tval = gva;
                return 0;
            }
            read_phys_sized(cpu, pa, size)
        }
        None => 0,
    }
}

/// JIT slow path: guest store (TLB miss or MMIO).
///
/// # Safety
/// `env` must point to a valid `RiscvCpu`.
#[no_mangle]
pub unsafe extern "C" fn machina_mem_write(
    env: *mut u8,
    gva: u64,
    val: u64,
    size: u32,
) {
    let cpu = &mut *(env as *mut RiscvCpu);
    if let Some(pa) = translate_for_helper(
        cpu,
        gva,
        AccessType::Write,
        size,
    ) {
        if !is_phys_backed(cpu, pa, size) {
            cpu.mem_fault_cause = 7;
            cpu.mem_fault_tval = gva;
            return;
        }
        write_phys_sized(cpu, pa, val, size);
    }
}

// ---- longjmp-based TB abort ----

/// Abort the current TB and return to the exec loop via
/// longjmp. The caller must have already delivered the
/// exception via raise_exception() before calling this.
///
/// # Safety
/// `cpu.jmp_env` must point to a valid jmp_buf set by
/// the exec loop's setjmp.
unsafe extern "C" {
    fn siglongjmp(env: *mut u8, val: i32) -> !;
}

unsafe fn cpu_loop_exit(cpu: &RiscvCpu) -> ! {
    let ptr = cpu.jmp_env;
    assert!(ptr != 0, "cpu_loop_exit: no jmp_env");
    siglongjmp(ptr as *mut u8, 1);
}

// ---- CSR helper for JIT ----

/// JIT helper: execute a CSR read-modify-write.
///
/// Called from JIT code via gen_call instead of exiting
/// the TB. On illegal CSR access, delivers the exception
/// via raise_exception + longjmp back to exec loop.
#[no_mangle]
pub unsafe extern "C" fn machina_csr_op(
    env: *mut u8,
    csr: u64,
    rs1_val: u64,
    funct3: u64,
) -> u64 {
    use machina_guest_riscv::riscv::csr::{
        CSR_PMPADDR0, CSR_PMPCFG0, CSR_SATP, PMP_COUNT,
    };
    let cpu = &mut *(env as *mut RiscvCpu);
    let csr_addr = csr as u16;
    let priv_level = cpu.priv_level;

    let old = match csr_addr {
        machina_guest_riscv::riscv::csr::CSR_TIME
        | machina_guest_riscv::riscv::csr::CSR_CYCLE => {
            let asp = cpu.as_ptr;
            if asp != 0 {
                let a = &*(asp as *const AddressSpace);
                a.read(GPA::new(0x0200_BFF8), 8)
            } else {
                0
            }
        }
        machina_guest_riscv::riscv::csr::CSR_INSTRET => {
            cpu.csr.instret
        }
        _ => match cpu.csr.read(csr_addr, priv_level) {
            Ok(v) => v,
            Err(_) => {
                cpu.raise_exception(
                    Exception::IllegalInstruction,
                    0,
                );
                cpu_loop_exit(cpu);
                return 0; // unreachable
            }
        },
    };

    let new_val = match funct3 {
        1 | 5 => rs1_val,
        2 | 6 => old | rs1_val,
        3 | 7 => old & !rs1_val,
        _ => return old,
    };

    let do_write = match funct3 {
        1 | 5 => true,
        _ => rs1_val != 0,
    };

    if do_write {
        if cpu
            .csr
            .write(csr_addr, new_val, priv_level)
            .is_err()
        {
            cpu.raise_exception(
                Exception::IllegalInstruction,
                0,
            );
            cpu_loop_exit(cpu);
            return 0; // unreachable
        }
        let is_pmp =
            (CSR_PMPCFG0..=CSR_PMPCFG0 + 3)
                .contains(&csr_addr)
                || (CSR_PMPADDR0
                    ..CSR_PMPADDR0 + PMP_COUNT as u16)
                    .contains(&csr_addr);
        if is_pmp {
            cpu.pmp.sync_from_csr(
                &cpu.csr.pmpcfg,
                &cpu.csr.pmpaddr,
            );
        }
        if csr_addr == CSR_SATP {
            cpu.mmu.set_satp(new_val);
            cpu.mmu.flush();
            cpu.tb_flush_pending = true;
        }
    }

    old
}
