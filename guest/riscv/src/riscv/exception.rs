//! RISC-V exception and interrupt model.
//!
//! Implements synchronous exception delivery, asynchronous interrupt
//! handling, and privilege-mode return (mret / sret).  Follows the
//! RISC-V Privileged Specification v1.12.

use super::cpu::RiscvCpu;
use super::csr::PrivLevel;

// ── Exception codes ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum Exception {
    InstructionMisaligned = 0,
    InstructionAccessFault = 1,
    IllegalInstruction = 2,
    Breakpoint = 3,
    LoadMisaligned = 4,
    LoadAccessFault = 5,
    StoreMisaligned = 6,
    StoreAccessFault = 7,
    EcallFromU = 8,
    EcallFromS = 9,
    EcallFromM = 11,
    InstructionPageFault = 12,
    LoadPageFault = 13,
    StorePageFault = 15,
}

// ── Interrupt codes ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum Interrupt {
    SupervisorSoftware = 1,
    MachineSoftware = 3,
    SupervisorTimer = 5,
    MachineTimer = 7,
    SupervisorExternal = 9,
    MachineExternal = 11,
}

/// Interrupt cause has bit 63 set.
const INTERRUPT_BIT: u64 = 1 << 63;

// ── mstatus field masks ─────────────────────────────────────────

const MSTATUS_SIE: u64 = 1 << 1;
const MSTATUS_MIE: u64 = 1 << 3;
const MSTATUS_SPIE: u64 = 1 << 5;
const MSTATUS_MPIE: u64 = 1 << 7;
const MSTATUS_SPP: u64 = 1 << 8;
const MSTATUS_MPP_MASK: u64 = 0x3 << 11;
const MSTATUS_MPP_SHIFT: u32 = 11;

// ── Interrupt priority table ────────────────────────────────────

/// Interrupts checked in descending priority order:
/// MEI > MSI > MTI > SEI > SSI > STI.
const INT_PRIORITY: [Interrupt; 6] = [
    Interrupt::MachineExternal,
    Interrupt::MachineSoftware,
    Interrupt::MachineTimer,
    Interrupt::SupervisorExternal,
    Interrupt::SupervisorSoftware,
    Interrupt::SupervisorTimer,
];

// ── tvec helpers ────────────────────────────────────────────────

/// Compute trap entry address from a *tvec register value.
///
/// mode 0 = Direct:   all traps go to BASE.
/// mode 1 = Vectored: interrupts go to BASE + 4 * cause.
fn tvec_addr(tvec: u64, cause: u64, is_interrupt: bool) -> u64 {
    let base = tvec & !0x3;
    let mode = tvec & 0x3;
    if mode == 1 && is_interrupt {
        base.wrapping_add(4 * (cause & !INTERRUPT_BIT))
    } else {
        base
    }
}

// ── Implementation on RiscvCpu ──────────────────────────────────

impl RiscvCpu {
    /// Common trap-entry logic shared by synchronous exceptions
    /// and asynchronous interrupts.
    ///
    /// `cause` is the raw xcause value (bit 63 set for
    /// interrupts).  `tval` is the trap-specific value.
    fn do_trap(&mut self, cause: u64, tval: u64) {
        let is_interrupt = cause & INTERRUPT_BIT != 0;
        let code = cause & !INTERRUPT_BIT;
        let cur_priv = self.priv_level as u64;

        // Determine delegation via medeleg / mideleg.
        let deleg = if is_interrupt {
            self.csr.mideleg
        } else {
            self.csr.medeleg
        };
        let delegated =
            (deleg >> code) & 1 != 0 && self.priv_level < PrivLevel::Machine;

        if delegated {
            // Trap to S-mode.
            self.csr.sepc = self.pc;
            self.csr.scause = cause;
            self.csr.stval = tval;

            // SPP = current priv (1 bit).
            self.csr.mstatus = (self.csr.mstatus & !MSTATUS_SPP)
                | if cur_priv >= PrivLevel::Supervisor as u64 {
                    MSTATUS_SPP
                } else {
                    0
                };

            // SPIE = SIE; SIE = 0.
            if self.csr.mstatus & MSTATUS_SIE != 0 {
                self.csr.mstatus |= MSTATUS_SPIE;
            } else {
                self.csr.mstatus &= !MSTATUS_SPIE;
            }
            self.csr.mstatus &= !MSTATUS_SIE;

            self.pc = tvec_addr(self.csr.stvec, cause, is_interrupt);
            self.priv_level = PrivLevel::Supervisor;
        } else {
            // Trap to M-mode.
            self.csr.mepc = self.pc;
            self.csr.mcause = cause;
            self.csr.mtval = tval;

            // MPP = current priv (2 bits).
            self.csr.mstatus = (self.csr.mstatus & !MSTATUS_MPP_MASK)
                | ((cur_priv & 0x3) << MSTATUS_MPP_SHIFT);

            // MPIE = MIE; MIE = 0.
            if self.csr.mstatus & MSTATUS_MIE != 0 {
                self.csr.mstatus |= MSTATUS_MPIE;
            } else {
                self.csr.mstatus &= !MSTATUS_MPIE;
            }
            self.csr.mstatus &= !MSTATUS_MIE;

            self.pc = tvec_addr(self.csr.mtvec, cause, is_interrupt);
            self.priv_level = PrivLevel::Machine;
        }
        self.mmu.flush();
    }

    /// Raise a synchronous exception.
    ///
    /// Saves current PC, cause, and tval into the appropriate
    /// M-mode or S-mode CSRs (based on delegation), updates
    /// mstatus, and redirects PC to the trap vector.
    pub fn raise_exception(&mut self, excp: Exception, tval: u64) {
        self.do_trap(excp as u64, tval);
    }

    /// Check for pending, enabled interrupts and take the
    /// highest-priority one if possible.
    ///
    /// Returns `true` if an interrupt was taken.
    pub fn handle_interrupt(&mut self) -> bool {
        let pending = self.csr.mip & self.csr.mie;
        if pending == 0 {
            return false;
        }

        let cur_priv = self.priv_level as u64;

        for &irq in &INT_PRIORITY {
            let bit = 1u64 << (irq as u64);
            if pending & bit == 0 {
                continue;
            }

            let delegated = (self.csr.mideleg >> (irq as u64)) & 1 != 0;

            if delegated {
                // S-mode interrupt: take if cur_priv < S,
                // or cur_priv == S and SIE is set.
                let s = PrivLevel::Supervisor as u64;
                let can_take = cur_priv < s
                    || (cur_priv == s && self.csr.mstatus & MSTATUS_SIE != 0);
                if !can_take {
                    continue;
                }
            } else {
                // M-mode interrupt: take if cur_priv < M,
                // or cur_priv == M and MIE is set.
                let m = PrivLevel::Machine as u64;
                let can_take = cur_priv < m
                    || (cur_priv == m && self.csr.mstatus & MSTATUS_MIE != 0);
                if !can_take {
                    continue;
                }
            }

            let cause = INTERRUPT_BIT | (irq as u64);
            self.do_trap(cause, 0);
            return true;
        }

        false
    }

    /// Execute MRET: return from M-mode trap handler.
    ///
    /// Restores privilege from mstatus.MPP, restores MIE from
    /// MPIE, sets MPIE=1, MPP=User, and jumps to mepc.
    pub fn execute_mret(&mut self) {
        // Restore priv from MPP.
        let mpp = (self.csr.mstatus & MSTATUS_MPP_MASK) >> MSTATUS_MPP_SHIFT;
        self.priv_level = match mpp {
            0 => PrivLevel::User,
            1 => PrivLevel::Supervisor,
            _ => PrivLevel::Machine,
        };

        // MIE = MPIE; MPIE = 1.
        if self.csr.mstatus & MSTATUS_MPIE != 0 {
            self.csr.mstatus |= MSTATUS_MIE;
        } else {
            self.csr.mstatus &= !MSTATUS_MIE;
        }
        self.csr.mstatus |= MSTATUS_MPIE;

        // MPP = User (lowest supported privilege).
        self.csr.mstatus &= !MSTATUS_MPP_MASK;

        self.pc = self.csr.mepc;
        self.mmu.flush();
    }

    /// Execute SRET: return from S-mode trap handler.
    ///
    /// Returns false if the instruction is illegal
    /// (current privilege < S-mode).
    pub fn execute_sret(&mut self) -> bool {
        if self.priv_level < PrivLevel::Supervisor {
            return false;
        }
        // Restore priv from SPP (1 bit: 0=U, 1=S).
        self.priv_level = if self.csr.mstatus & MSTATUS_SPP != 0 {
            PrivLevel::Supervisor
        } else {
            PrivLevel::User
        };

        // SIE = SPIE; SPIE = 1.
        if self.csr.mstatus & MSTATUS_SPIE != 0 {
            self.csr.mstatus |= MSTATUS_SIE;
        } else {
            self.csr.mstatus &= !MSTATUS_SIE;
        }
        self.csr.mstatus |= MSTATUS_SPIE;

        // SPP = 0.
        self.csr.mstatus &= !MSTATUS_SPP;

        self.pc = self.csr.sepc;
        self.mmu.flush();
        true
    }
}
