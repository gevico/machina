//! RISC-V Control and Status Registers (CSRs).
//!
//! Implements the full CSR address space for M/S/U privilege levels,
//! including privilege checks, WARL masking, and S-mode aliasing
//! into M-mode registers (sstatus→mstatus, sip→mip, sie→mie).

// ── CSR address constants ───────────────────────────────────────

// Machine-level CSRs
pub const CSR_MSTATUS: u16 = 0x300;
pub const CSR_MISA: u16 = 0x301;
pub const CSR_MEDELEG: u16 = 0x302;
pub const CSR_MIDELEG: u16 = 0x303;
pub const CSR_MIE: u16 = 0x304;
pub const CSR_MTVEC: u16 = 0x305;
pub const CSR_MCOUNTEREN: u16 = 0x306;
pub const CSR_MSCRATCH: u16 = 0x340;
pub const CSR_MEPC: u16 = 0x341;
pub const CSR_MCAUSE: u16 = 0x342;
pub const CSR_MTVAL: u16 = 0x343;
pub const CSR_MIP: u16 = 0x344;
pub const CSR_MENVCFG: u16 = 0x30A;
pub const CSR_MCOUNTINHIBIT: u16 = 0x320;
pub const CSR_MHARTID: u16 = 0xF14;
pub const CSR_MVENDORID: u16 = 0xF11;
pub const CSR_MARCHID: u16 = 0xF12;
pub const CSR_MIMPID: u16 = 0xF13;

// PMP CSRs
pub const CSR_PMPCFG0: u16 = 0x3A0;
pub const CSR_PMPCFG2: u16 = 0x3A2;
pub const CSR_PMPADDR0: u16 = 0x3B0;

// Supervisor-level CSRs
pub const CSR_SSTATUS: u16 = 0x100;
pub const CSR_SIE: u16 = 0x104;
pub const CSR_STVEC: u16 = 0x105;
pub const CSR_SCOUNTEREN: u16 = 0x106;
pub const CSR_SSCRATCH: u16 = 0x140;
pub const CSR_SEPC: u16 = 0x141;
pub const CSR_SCAUSE: u16 = 0x142;
pub const CSR_STVAL: u16 = 0x143;
pub const CSR_SIP: u16 = 0x144;
pub const CSR_SATP: u16 = 0x180;

// Machine counter CSRs (read-write)
pub const CSR_MCYCLE: u16 = 0xB00;
pub const CSR_MINSTRET: u16 = 0xB02;

// Counter CSRs (read-only)
pub const CSR_CYCLE: u16 = 0xC00;
pub const CSR_TIME: u16 = 0xC01;
pub const CSR_INSTRET: u16 = 0xC02;

// Debug/Trace trigger CSRs (stubs)
pub const CSR_TSELECT: u16 = 0x7A0;
pub const CSR_TDATA1: u16 = 0x7A1;
pub const CSR_TDATA2: u16 = 0x7A2;
pub const CSR_TDATA3: u16 = 0x7A3;
pub const CSR_TCONTROL: u16 = 0x7A5;

// Floating-point CSRs
pub const CSR_FFLAGS: u16 = 0x001;
pub const CSR_FRM: u16 = 0x002;
pub const CSR_FCSR: u16 = 0x003;

// ── MSTATUS bit definitions ────────────────────────────────────

const MSTATUS_SIE: u64 = 1 << 1;
const MSTATUS_MIE: u64 = 1 << 3;
const MSTATUS_SPIE: u64 = 1 << 5;
const MSTATUS_MPIE: u64 = 1 << 7;
const MSTATUS_SPP: u64 = 1 << 8;
const MSTATUS_MPP: u64 = 3 << 11;
const MSTATUS_FS: u64 = 3 << 13;
const MSTATUS_XS: u64 = 3 << 15;
const MSTATUS_MPRV: u64 = 1 << 17;
const MSTATUS_SUM: u64 = 1 << 18;
const MSTATUS_MXR: u64 = 1 << 19;
const MSTATUS_TVM: u64 = 1 << 20;
const MSTATUS_TW: u64 = 1 << 21;
const MSTATUS_TSR: u64 = 1 << 22;
const MSTATUS_UXL: u64 = 3 << 32;
const MSTATUS_SD: u64 = 1 << 63;

/// UXL=2 (64-bit) and SXL=2 (64-bit) for RV64.
const MSTATUS_UXL_64: u64 = 2 << 32;
const MSTATUS_SXL_64: u64 = 2 << 34;

/// Writable mask for mstatus (WARL).
const MSTATUS_WRITE_MASK: u64 = MSTATUS_SIE
    | MSTATUS_MIE
    | MSTATUS_SPIE
    | MSTATUS_MPIE
    | MSTATUS_SPP
    | MSTATUS_MPP
    | MSTATUS_FS
    | MSTATUS_MPRV
    | MSTATUS_SUM
    | MSTATUS_MXR
    | MSTATUS_TVM
    | MSTATUS_TW
    | MSTATUS_TSR;

/// S-mode view mask for sstatus (subset of mstatus visible to S).
const SSTATUS_MASK: u64 = MSTATUS_SIE
    | MSTATUS_SPIE
    | MSTATUS_SPP
    | MSTATUS_FS
    | MSTATUS_XS
    | MSTATUS_SUM
    | MSTATUS_MXR
    | MSTATUS_UXL
    | MSTATUS_SD;

/// S-mode writable bits within sstatus.
const SSTATUS_WRITE_MASK: u64 = MSTATUS_SIE
    | MSTATUS_SPIE
    | MSTATUS_SPP
    | MSTATUS_FS
    | MSTATUS_SUM
    | MSTATUS_MXR;

// ── Interrupt delegation masks ──────────────────────────────────

/// S-mode visible interrupt bits (SSIP, STIP, SEIP).
const SIP_MASK: u64 = (1 << 1) | (1 << 5) | (1 << 9);

// ── FP CSR masks ────────────────────────────────────────────────

const FFLAGS_MASK: u64 = 0x1F;
const FRM_MASK: u64 = 0x07;

// ── MISA value for RV64IMAFDC ───────────────────────────────────

/// MXL = 2 (64-bit) in bits [63:62].
const MXL_64: u64 = 2 << 62;

fn misa_rv64imafdcsu() -> u64 {
    MXL_64
        | (1 << 0)  // A
        | (1 << 2)  // C
        | (1 << 3)  // D
        | (1 << 5)  // F
        | (1 << 8)  // I
        | (1 << 12) // M
        | (1 << 18) // S
        | (1 << 20) // U
}

// ── MEDELEG writable mask ───────────────────────────────────────

/// Delegable synchronous exceptions (all standard ones except
/// those that only occur in M-mode).
const MEDELEG_MASK: u64 = (1 << 0)   // Insn addr misaligned
    | (1 << 1)   // Insn access fault
    | (1 << 2)   // Illegal insn
    | (1 << 3)   // Breakpoint
    | (1 << 4)   // Load addr misaligned
    | (1 << 5)   // Load access fault
    | (1 << 6)   // Store addr misaligned
    | (1 << 7)   // Store access fault
    | (1 << 8)   // Ecall from U
    | (1 << 9)   // Ecall from S
    | (1 << 12)  // Insn page fault
    | (1 << 13)  // Load page fault
    | (1 << 15); // Store page fault

// ── Illegal-instruction cause ───────────────────────────────────

const CAUSE_ILLEGAL_INSN: u64 = 2;

// ── Privilege level ─────────────────────────────────────────────

/// RISC-V privilege level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum PrivLevel {
    User = 0,
    Supervisor = 1,
    Machine = 3,
}

impl PrivLevel {
    /// Minimum privilege required to access a CSR, derived from
    /// bits [9:8] of the CSR address.
    fn required(addr: u16) -> Self {
        match (addr >> 8) & 0x3 {
            0 => PrivLevel::User,
            1 => PrivLevel::Supervisor,
            // 2 is "Hypervisor" — treat as Machine for now
            _ => PrivLevel::Machine,
        }
    }
}

// ── CSR file ────────────────────────────────────────────────────

/// Full RISC-V CSR register file for M/S/U privilege levels.
/// Number of PMP entries supported.
pub const PMP_COUNT: usize = 16;

pub struct CsrFile {
    // Machine-level
    pub mstatus: u64,
    pub misa: u64,
    pub medeleg: u64,
    pub mideleg: u64,
    pub mie: u64,
    pub mtvec: u64,
    pub mcounteren: u64,
    pub mscratch: u64,
    pub mepc: u64,
    pub mcause: u64,
    pub mtval: u64,
    pub mip: u64,
    pub menvcfg: u64,
    pub mcountinhibit: u64,

    // PMP
    pub pmpcfg: [u64; 4],
    pub pmpaddr: [u64; PMP_COUNT],

    // Supervisor-level (non-aliased)
    pub satp: u64,
    pub sscratch: u64,
    pub sepc: u64,
    pub scause: u64,
    pub stval: u64,
    pub stvec: u64,
    pub scounteren: u64,

    // Counters
    pub cycle: u64,
    pub instret: u64,

    // Floating-point
    pub fflags: u64,
    pub frm: u64,

    // Machine info (read-only)
    pub hart_id: u64,
}

impl CsrFile {
    pub fn new() -> Self {
        Self {
            mstatus: 0,
            misa: misa_rv64imafdcsu(),
            medeleg: 0,
            mideleg: 0,
            mie: 0,
            mtvec: 0,
            mcounteren: 0,
            mscratch: 0,
            mepc: 0,
            mcause: 0,
            mtval: 0,
            mip: 0,
            menvcfg: 0,
            mcountinhibit: 0,
            pmpcfg: [0u64; 4],
            pmpaddr: [0u64; PMP_COUNT],
            satp: 0,
            sscratch: 0,
            sepc: 0,
            scause: 0,
            stval: 0,
            stvec: 0,
            scounteren: 0,
            cycle: 0,
            instret: 0,
            fflags: 0,
            frm: 0,
            hart_id: 0,
        }
    }

    /// Read a CSR with privilege check.
    ///
    /// Returns `Err(CAUSE_ILLEGAL_INSN)` if the current privilege
    /// level is insufficient.
    pub fn read(&self, addr: u16, priv_level: PrivLevel) -> Result<u64, u64> {
        if priv_level < PrivLevel::required(addr) {
            return Err(CAUSE_ILLEGAL_INSN);
        }
        match addr {
            // -- M-level --
            CSR_MSTATUS => Ok(self.mstatus
                | self.sd_bit()
                | MSTATUS_UXL_64
                | MSTATUS_SXL_64),
            CSR_MISA => Ok(self.misa),
            CSR_MEDELEG => Ok(self.medeleg),
            CSR_MIDELEG => Ok(self.mideleg),
            CSR_MIE => Ok(self.mie),
            CSR_MTVEC => Ok(self.mtvec),
            CSR_MCOUNTEREN => Ok(self.mcounteren),
            CSR_MSCRATCH => Ok(self.mscratch),
            CSR_MEPC => Ok(self.mepc),
            CSR_MCAUSE => Ok(self.mcause),
            CSR_MTVAL => Ok(self.mtval),
            CSR_MIP => Ok(self.mip),
            CSR_MENVCFG => Ok(self.menvcfg),
            CSR_MCOUNTINHIBIT => Ok(self.mcountinhibit),

            // PMP config (RV64: pmpcfg0 and pmpcfg2 only)
            addr if addr == CSR_PMPCFG0 || addr == CSR_PMPCFG2 => {
                let idx = ((addr - CSR_PMPCFG0) / 2) as usize;
                Ok(self.pmpcfg[idx])
            }
            // pmpcfg1/pmpcfg3 don't exist in RV64.
            addr if addr == CSR_PMPCFG0 + 1 || addr == CSR_PMPCFG0 + 3 => Ok(0),
            // PMP address registers
            addr if addr >= CSR_PMPADDR0
                && addr < CSR_PMPADDR0 + PMP_COUNT as u16 =>
            {
                let idx = (addr - CSR_PMPADDR0) as usize;
                Ok(self.pmpaddr[idx])
            }

            // -- S-level (aliased) --
            CSR_SSTATUS => {
                Ok((self.mstatus | self.sd_bit() | MSTATUS_UXL_64)
                    & SSTATUS_MASK)
            }
            CSR_SIE => Ok(self.mie & self.mideleg & SIP_MASK),
            CSR_STVEC => Ok(self.stvec),
            CSR_SCOUNTEREN => Ok(self.scounteren),
            CSR_SSCRATCH => Ok(self.sscratch),
            CSR_SEPC => Ok(self.sepc),
            CSR_SCAUSE => Ok(self.scause),
            CSR_STVAL => Ok(self.stval),
            CSR_SIP => Ok(self.mip & self.mideleg & SIP_MASK),
            CSR_SATP => Ok(self.satp),

            // -- Machine info (read-only) --
            CSR_MHARTID => Ok(self.hart_id),
            CSR_MVENDORID => Ok(0),
            CSR_MARCHID => Ok(0),
            CSR_MIMPID => Ok(0),

            // -- Counters --
            CSR_CYCLE | CSR_TIME => Ok(self.cycle),
            CSR_INSTRET => Ok(self.instret),
            CSR_MCYCLE => Ok(self.cycle),
            CSR_MINSTRET => Ok(self.instret),

            // -- Debug/Trace trigger stubs --
            CSR_TSELECT | CSR_TDATA1 | CSR_TDATA2 | CSR_TDATA3
            | CSR_TCONTROL => Ok(0),

            // -- FP --
            CSR_FFLAGS => Ok(self.fflags & FFLAGS_MASK),
            CSR_FRM => Ok(self.frm & FRM_MASK),
            CSR_FCSR => {
                Ok((self.fflags & FFLAGS_MASK) | ((self.frm & FRM_MASK) << 5))
            }

            // Machine HPM counters (0xB03-0xB1F) and
            // event selectors (0x323-0x33F): return 0.
            addr if (0xB03..=0xB1F).contains(&addr)
                || (0x323..=0x33F).contains(&addr) =>
            {
                Ok(0)
            }

            _ => Err(CAUSE_ILLEGAL_INSN),
        }
    }

    /// Write a CSR with privilege check and WARL masking.
    ///
    /// Returns `Err(CAUSE_ILLEGAL_INSN)` if the current privilege
    /// level is insufficient or the CSR is read-only.
    pub fn write(
        &mut self,
        addr: u16,
        val: u64,
        priv_level: PrivLevel,
    ) -> Result<(), u64> {
        if priv_level < PrivLevel::required(addr) {
            return Err(CAUSE_ILLEGAL_INSN);
        }
        // Bits [11:10] == 0b11 means read-only.
        if (addr >> 10) & 0x3 == 0x3 {
            return Err(CAUSE_ILLEGAL_INSN);
        }
        match addr {
            // -- M-level --
            CSR_MSTATUS => {
                self.mstatus = (self.mstatus & !MSTATUS_WRITE_MASK)
                    | (val & MSTATUS_WRITE_MASK);
                Ok(())
            }
            CSR_MISA => Ok(()), // read-only in this impl
            CSR_MEDELEG => {
                self.medeleg = val & MEDELEG_MASK;
                Ok(())
            }
            CSR_MIDELEG => {
                self.mideleg = val & SIP_MASK;
                Ok(())
            }
            CSR_MIE => {
                self.mie = val;
                Ok(())
            }
            CSR_MTVEC => {
                self.mtvec = val;
                Ok(())
            }
            CSR_MCOUNTEREN => {
                self.mcounteren = val & 0x7;
                Ok(())
            }
            CSR_MSCRATCH => {
                self.mscratch = val;
                Ok(())
            }
            CSR_MEPC => {
                self.mepc = val & !1u64;
                Ok(())
            }
            CSR_MCAUSE => {
                self.mcause = val;
                Ok(())
            }
            CSR_MTVAL => {
                self.mtval = val;
                Ok(())
            }
            CSR_MIP => {
                self.mip = (self.mip & !SIP_MASK) | (val & SIP_MASK);
                Ok(())
            }
            CSR_MENVCFG => {
                self.menvcfg = val;
                Ok(())
            }
            CSR_MCOUNTINHIBIT => {
                self.mcountinhibit = val & 0x7;
                Ok(())
            }
            // PMP config (RV64: pmpcfg0 and pmpcfg2)
            addr if addr == CSR_PMPCFG0 || addr == CSR_PMPCFG2 => {
                let idx = ((addr - CSR_PMPCFG0) / 2) as usize;
                self.pmpcfg[idx] = val;
                Ok(())
            }
            // pmpcfg1/pmpcfg3: ignored in RV64.
            addr if addr == CSR_PMPCFG0 + 1 || addr == CSR_PMPCFG0 + 3 => {
                Ok(())
            }
            // PMP address registers
            addr if addr >= CSR_PMPADDR0
                && addr < CSR_PMPADDR0 + PMP_COUNT as u16 =>
            {
                let idx = (addr - CSR_PMPADDR0) as usize;
                self.pmpaddr[idx] = val;
                Ok(())
            }

            // -- S-level (aliased) --
            CSR_SSTATUS => {
                self.mstatus = (self.mstatus & !SSTATUS_WRITE_MASK)
                    | (val & SSTATUS_WRITE_MASK);
                Ok(())
            }
            CSR_SIE => {
                let mask = self.mideleg & SIP_MASK;
                self.mie = (self.mie & !mask) | (val & mask);
                Ok(())
            }
            CSR_STVEC => {
                self.stvec = val;
                Ok(())
            }
            CSR_SCOUNTEREN => {
                self.scounteren = val & 0x7;
                Ok(())
            }
            CSR_SSCRATCH => {
                self.sscratch = val;
                Ok(())
            }
            CSR_SEPC => {
                self.sepc = val & !1u64;
                Ok(())
            }
            CSR_SCAUSE => {
                self.scause = val;
                Ok(())
            }
            CSR_STVAL => {
                self.stval = val;
                Ok(())
            }
            CSR_SIP => {
                let mask = self.mideleg & SIP_MASK;
                self.mip = (self.mip & !mask) | (val & mask);
                Ok(())
            }
            CSR_SATP => {
                self.satp = val;
                Ok(())
            }

            // -- FP --
            CSR_FFLAGS => {
                self.fflags = val & FFLAGS_MASK;
                Ok(())
            }
            CSR_FRM => {
                self.frm = val & FRM_MASK;
                Ok(())
            }
            CSR_FCSR => {
                self.fflags = val & FFLAGS_MASK;
                self.frm = (val >> 5) & FRM_MASK;
                Ok(())
            }

            // Machine counters (writable)
            CSR_MCYCLE => {
                self.cycle = val;
                Ok(())
            }
            CSR_MINSTRET => {
                self.instret = val;
                Ok(())
            }

            // Debug/Trace trigger stubs (write-ignore)
            CSR_TSELECT | CSR_TDATA1 | CSR_TDATA2 | CSR_TDATA3
            | CSR_TCONTROL => Ok(()),

            // Machine HPM counters and event selectors:
            // silently ignore writes.
            addr if (0xB03..=0xB1F).contains(&addr)
                || (0x323..=0x33F).contains(&addr) =>
            {
                Ok(())
            }

            _ => Err(CAUSE_ILLEGAL_INSN),
        }
    }

    /// Compute the SD bit from FS/XS dirty state.
    fn sd_bit(&self) -> u64 {
        let fs = self.mstatus & MSTATUS_FS;
        let xs = self.mstatus & MSTATUS_XS;
        if fs == MSTATUS_FS || xs == MSTATUS_XS {
            MSTATUS_SD
        } else {
            0
        }
    }
}

impl Default for CsrFile {
    fn default() -> Self {
        Self::new()
    }
}
