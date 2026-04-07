//! RISC-V CPU state.

use std::mem::offset_of;
use std::sync::atomic::{AtomicBool, AtomicU32};

use super::csr::{CsrFile, PrivLevel};

/// Number of general-purpose registers (x0-x31).
pub const NUM_GPRS: usize = 32;
/// Number of floating-point registers (f0-f31).
pub const NUM_FPRS: usize = 32;

/// RISC-V CPU architectural state (RV64).
///
/// Layout must be `#[repr(C)]` so that TCG global temps can
/// reference fields at fixed offsets from the env pointer.
/// The hot-path fields (gpr, fpr, pc, etc.) are kept at the
/// top for stable offsets; the CSR file and privilege level
/// live below and are accessed via helper methods.
#[repr(C)]
pub struct RiscvCpu {
    /// General-purpose registers x0-x31.
    /// x0 is hardwired to zero (enforced by the frontend,
    /// not by this struct).
    pub gpr: [u64; NUM_GPRS],
    /// Floating-point registers f0-f31 (raw bits).
    pub fpr: [u64; NUM_FPRS],
    /// Program counter.
    pub pc: u64,
    /// Guest memory base pointer (host address).
    /// Used by generated code to translate guest addresses.
    pub guest_base: u64,
    /// LR reservation address (-1 = no reservation).
    pub load_res: u64,
    /// LR loaded value (for SC comparison).
    pub load_val: u64,
    /// Floating-point accrued exception flags (fflags).
    pub fflags: u64,
    /// Floating-point rounding mode (frm).
    pub frm: u64,
    /// User status register (ustatus).
    pub ustatus: u64,
    /// User interrupt-enable register (uie).
    pub uie: u64,
    /// User trap vector base address (utvec).
    pub utvec: u64,
    /// User scratch register (uscratch).
    pub uscratch: u64,
    /// User exception program counter (uepc).
    pub uepc: u64,
    /// User exception cause (ucause).
    pub ucause: u64,
    /// User trap value (utval).
    pub utval: u64,
    /// User interrupt pending (uip).
    pub uip: u64,

    /// Pending interrupt request bitmap (full-system).
    pub interrupt_request: AtomicU32,
    /// Whether the CPU is halted (WFI, full-system).
    pub halted: AtomicBool,

    /// Current privilege level.
    pub priv_level: PrivLevel,
    /// Full CSR register file (M/S/U).
    pub csr: CsrFile,

    /// Runtime PMP state, synced from CSR on pmpcfg/
    /// pmpaddr writes.
    pub pmp: super::pmp::Pmp,
    /// Runtime MMU state, synced from CSR on satp write.
    pub mmu: super::mmu::Mmu,

    /// Pending memory fault from JIT helper. Non-zero cause
    /// means a fault occurred; the exec loop must call
    /// handle_exception after the current TB completes.
    pub mem_fault_cause: u64,
    pub mem_fault_tval: u64,

    /// Pointer to the machine's AddressSpace for MMIO
    /// dispatch from JIT helpers. Cast from *const
    /// AddressSpace. Zero means not initialized.
    pub as_ptr: u64,
    /// End of the RAM window (ram_base + ram_size).
    /// JIT helpers use this to decide RAM vs MMIO.
    pub ram_end: u64,
    /// Pointer to TbStore's code-page bitmap (AtomicU8
    /// array). Store helpers check this to detect writes
    /// to pages containing translated code.  Zero means
    /// not initialized (no code-page tracking).
    pub code_pages_ptr: u64,
    /// Length of the code-page bitmap in bytes.
    pub code_pages_len: u64,

    /// Set by handle_priv_csr when satp is written.
    /// The exec loop checks this flag and performs
    /// TB invalidation when set.
    pub tb_flush_pending: bool,

    /// Physical PC from the last gen_code() translation.
    /// Used by the exec loop to record phys_pc in TB.
    pub last_phys_pc: u64,

    /// PC of the guest memory instruction currently being
    /// executed by JIT code. Written by the translator
    /// before each qemu_ld/qemu_st so that helper-latched
    /// faults have the correct mepc.
    pub fault_pc: u64,

    /// Pointer to the exec loop's jmp_buf for longjmp.
    /// Set by the exec loop before TB execution. Helpers
    /// call longjmp through this pointer when they need
    /// to abort TB execution (e.g. illegal CSR access).
    /// Zero means longjmp is not available.
    pub jmp_env: u64,

    /// Physical pages written since last fence.i. Used
    /// for page-granularity TB invalidation.
    pub dirty_pages: Vec<u64>,

    /// Raw pointer to `GdbState` for watchpoint checks.
    /// Set by `set_gdb_state()`. Zero if no GDB session.
    pub gdb_state_ptr: u64,
    /// Watchpoint hit recorded by JIT helpers.
    /// Layout: (addr << 8) | (1 for write, 2 for read,
    /// 3 for access). Zero if none.
    pub watchpoint_hit: u64,
}

// Field offsets (bytes) from the start of RiscvCpu.
// Used by `Context::new_global()` to bind IR temps.

/// Byte offset of `gpr[i]`: `i * 8`.
pub const fn gpr_offset(i: usize) -> i64 {
    (i * 8) as i64
}

/// Byte offset of `fpr[i]`: `NUM_GPRS*8 + i*8`.
pub const fn fpr_offset(i: usize) -> i64 {
    ((NUM_GPRS + i) * 8) as i64
}

/// Byte offset of the `pc` field.
pub const PC_OFFSET: i64 = ((NUM_GPRS + NUM_FPRS) * 8) as i64; // 512

/// Byte offset of the `guest_base` field.
pub const GUEST_BASE_OFFSET: i64 = PC_OFFSET + 8; // 520

/// Byte offset of the `load_res` field.
pub const LOAD_RES_OFFSET: i64 = GUEST_BASE_OFFSET + 8; // 528

/// Byte offset of the `load_val` field.
pub const LOAD_VAL_OFFSET: i64 = LOAD_RES_OFFSET + 8; // 536

/// Byte offset of `fflags`.
pub const FFLAGS_OFFSET: i64 = LOAD_VAL_OFFSET + 8; // 544
/// Byte offset of `frm`.
pub const FRM_OFFSET: i64 = FFLAGS_OFFSET + 8; // 552
/// Byte offset of `ustatus`.
pub const USTATUS_OFFSET: i64 = FRM_OFFSET + 8; // 560
/// Byte offset of `uie`.
pub const UIE_OFFSET: i64 = USTATUS_OFFSET + 8; // 568
/// Byte offset of `utvec`.
pub const UTVEC_OFFSET: i64 = UIE_OFFSET + 8; // 576
/// Byte offset of `uscratch`.
pub const USCRATCH_OFFSET: i64 = UTVEC_OFFSET + 8; // 584
/// Byte offset of `uepc`.
pub const UEPC_OFFSET: i64 = USCRATCH_OFFSET + 8; // 592
/// Byte offset of `ucause`.
pub const UCAUSE_OFFSET: i64 = UEPC_OFFSET + 8; // 600
/// Byte offset of `utval`.
pub const UTVAL_OFFSET: i64 = UCAUSE_OFFSET + 8; // 608
/// Byte offset of `uip`.
pub const UIP_OFFSET: i64 = UTVAL_OFFSET + 8; // 616

/// Byte offset of `csr.mstatus`.
pub const MSTATUS_OFFSET: i64 =
    (offset_of!(RiscvCpu, csr) + offset_of!(CsrFile, mstatus)) as i64;

/// USTATUS FS bits mask.
pub const USTATUS_FS_MASK: u64 = 0x0000_6000;
/// USTATUS FS = Dirty.
pub const USTATUS_FS_DIRTY: u64 = 0x0000_6000;

impl RiscvCpu {
    pub fn new() -> Self {
        Self {
            gpr: [0u64; NUM_GPRS],
            fpr: [0u64; NUM_FPRS],
            pc: 0,
            guest_base: 0,
            load_res: u64::MAX,
            load_val: 0,
            fflags: 0,
            frm: 0,
            ustatus: USTATUS_FS_DIRTY,
            uie: 0,
            utvec: 0,
            uscratch: 0,
            uepc: 0,
            ucause: 0,
            utval: 0,
            uip: 0,
            interrupt_request: AtomicU32::new(0),
            halted: AtomicBool::new(false),
            priv_level: PrivLevel::Machine,
            csr: CsrFile::new(),
            pmp: super::pmp::Pmp::new(),
            mmu: super::mmu::Mmu::new(),
            mem_fault_cause: 0,
            mem_fault_tval: 0,
            as_ptr: 0,
            ram_end: 0,
            code_pages_ptr: 0,
            code_pages_len: 0,
            tb_flush_pending: false,
            last_phys_pc: 0,
            fault_pc: 0,
            jmp_env: 0,
            dirty_pages: Vec::new(),
            gdb_state_ptr: 0,
            watchpoint_hit: 0,
        }
    }

    /// Set the current privilege level.
    pub fn set_priv(&mut self, p: PrivLevel) {
        self.priv_level = p;
    }

    /// Read a CSR, using the current privilege level.
    /// Panics on illegal access.
    pub fn csr_read(&self, addr: u16) -> u64 {
        self.csr
            .read(addr, self.priv_level)
            .expect("illegal CSR read")
    }

    /// Write a CSR, using the current privilege level.
    /// Panics on illegal access.
    pub fn csr_write(&mut self, addr: u16, val: u64) {
        self.csr
            .write(addr, val, self.priv_level)
            .expect("illegal CSR write");
    }

    /// Try to read a CSR with privilege check.
    pub fn try_csr_read(&self, addr: u16) -> Result<u64, u64> {
        self.csr.read(addr, self.priv_level)
    }

    /// Try to write a CSR with privilege check.
    pub fn try_csr_write(&mut self, addr: u16, val: u64) -> Result<(), u64> {
        self.csr.write(addr, val, self.priv_level)
    }
}

impl Default for RiscvCpu {
    fn default() -> Self {
        Self::new()
    }
}
