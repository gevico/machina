//! RISC-V Physical Memory Protection (PMP).
//!
//! Implements the 16-entry PMP unit per the RISC-V Privileged
//! Specification v1.12.  Supports OFF, TOR, NA4, and NAPOT
//! address-matching modes with R/W/X permission checks and
//! the L (locked) bit.

use super::csr::PrivLevel;
use super::exception::Exception;
use super::mmu::AccessType;

// ── PMP configuration bit masks ────────────────────────────────

const PMP_R: u8 = 1 << 0;
const PMP_W: u8 = 1 << 1;
const PMP_X: u8 = 1 << 2;
const PMP_A_SHIFT: u32 = 3;
const PMP_A_MASK: u8 = 0x3 << PMP_A_SHIFT;
const PMP_L: u8 = 1 << 7;

/// Number of PMP entries.
const PMP_COUNT: usize = 16;

// ── Address-matching mode ──────────────────────────────────────

/// PMP address-matching mode (A field, bits [4:3] of pmpcfg).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PmpAddrMatch {
    Off = 0,
    Tor = 1,
    Na4 = 2,
    Napot = 3,
}

impl PmpAddrMatch {
    pub fn from_cfg(cfg: u8) -> Self {
        match (cfg & PMP_A_MASK) >> PMP_A_SHIFT {
            0 => Self::Off,
            1 => Self::Tor,
            2 => Self::Na4,
            3 => Self::Napot,
            _ => unreachable!(),
        }
    }
}

// ── PMP entry ──────────────────────────────────────────────────

/// A single PMP entry (pmpcfg + pmpaddr pair).
#[derive(Debug, Clone, Copy, Default)]
pub struct PmpEntry {
    /// Configuration byte: R(0) | W(1) | X(2) | A(4:3) | L(7).
    pub cfg: u8,
    /// Address register value (physical address >> 2).
    pub addr: u64,
}

// ── PMP unit ───────────────────────────────────────────────────

/// 16-entry PMP unit.
pub struct Pmp {
    entries: [PmpEntry; PMP_COUNT],
}

impl Pmp {
    /// Create a PMP unit with all entries disabled.
    pub fn new() -> Self {
        Self {
            entries: [PmpEntry::default(); PMP_COUNT],
        }
    }

    /// Set the pmpcfg byte for entry `idx`.
    pub fn set_cfg(&mut self, idx: usize, cfg: u8) {
        assert!(idx < PMP_COUNT);
        self.entries[idx].cfg = cfg;
    }

    /// Set the pmpaddr value for entry `idx`.
    pub fn set_addr(&mut self, idx: usize, addr: u64) {
        assert!(idx < PMP_COUNT);
        self.entries[idx].addr = addr;
    }

    /// Read the pmpcfg byte for entry `idx`.
    pub fn get_cfg(&self, idx: usize) -> u8 {
        assert!(idx < PMP_COUNT);
        self.entries[idx].cfg
    }

    /// Read the pmpaddr value for entry `idx`.
    pub fn get_addr(&self, idx: usize) -> u64 {
        assert!(idx < PMP_COUNT);
        self.entries[idx].addr
    }

    /// Sync PMP entries from the CSR file's pmpcfg/pmpaddr
    /// arrays. Called after any PMP CSR write.
    pub fn sync_from_csr(
        &mut self,
        pmpcfg: &[u64; 4],
        pmpaddr: &[u64; super::csr::PMP_COUNT],
    ) {
        for (i, (entry, &addr)) in
            self.entries.iter_mut().zip(pmpaddr.iter()).enumerate()
        {
            let cfg_idx = i / 8;
            let cfg_shift = (i % 8) * 8;
            let raw = if cfg_idx < 2 { pmpcfg[cfg_idx] } else { 0 };
            entry.cfg = ((raw >> cfg_shift) & 0xFF) as u8;
            entry.addr = addr;
        }
    }

    /// Check whether a memory access is permitted.
    ///
    /// `addr` and `size` describe the byte range being accessed.
    /// Returns `Ok(())` on success or the appropriate access-
    /// fault `Exception` on denial.
    pub fn check_access(
        &self,
        addr: u64,
        size: u64,
        access: AccessType,
        priv_level: PrivLevel,
    ) -> Result<(), Exception> {
        // M-mode with no locked entries bypasses PMP entirely.
        if priv_level == PrivLevel::Machine {
            let has_locked = self.entries.iter().any(|e| e.cfg & PMP_L != 0);
            if !has_locked {
                return Ok(());
            }
        }

        for i in 0..PMP_COUNT {
            let entry = &self.entries[i];
            let a_mode = PmpAddrMatch::from_cfg(entry.cfg);
            if a_mode == PmpAddrMatch::Off {
                continue;
            }

            let (base, end) = match a_mode {
                PmpAddrMatch::Napot => napot_range(entry.addr),
                PmpAddrMatch::Na4 => {
                    let b = entry.addr << 2;
                    (b, b.wrapping_add(4))
                }
                PmpAddrMatch::Tor => {
                    let prev = if i == 0 {
                        0
                    } else {
                        self.entries[i - 1].addr << 2
                    };
                    (prev, entry.addr << 2)
                }
                PmpAddrMatch::Off => unreachable!(),
            };

            // Check if [addr, addr+size) falls within
            // [base, end).
            if addr >= base && addr.wrapping_add(size) <= end {
                return self.check_permission(entry, access, priv_level);
            }
        }

        // No entry matched.
        if priv_level == PrivLevel::Machine {
            Ok(())
        } else {
            Err(access_fault(access))
        }
    }

    /// Verify R/W/X permission bits against the access type.
    fn check_permission(
        &self,
        entry: &PmpEntry,
        access: AccessType,
        priv_level: PrivLevel,
    ) -> Result<(), Exception> {
        let locked = entry.cfg & PMP_L != 0;

        // M-mode is only restricted by locked entries.
        if priv_level == PrivLevel::Machine && !locked {
            return Ok(());
        }

        let allowed = match access {
            AccessType::Read => entry.cfg & PMP_R != 0,
            AccessType::Write => entry.cfg & PMP_W != 0,
            AccessType::Execute => entry.cfg & PMP_X != 0,
        };

        if allowed {
            Ok(())
        } else {
            Err(access_fault(access))
        }
    }
}

impl Default for Pmp {
    fn default() -> Self {
        Self::new()
    }
}

// ── Helpers ────────────────────────────────────────────────────

/// Decode a NAPOT pmpaddr into (base, end) in byte space.
///
/// The trailing-ones pattern determines the region size:
///   G = number of trailing ones in pmpaddr
///   size = 2^(G + 3) bytes
///   base = (pmpaddr & ~((1 << (G+1)) - 1)) << 2
pub fn napot_range(pmpaddr: u64) -> (u64, u64) {
    let g = (!pmpaddr).trailing_zeros() as u64;
    if g >= 61 {
        return (0, u64::MAX);
    }
    let size: u64 = 1u64 << (g + 3);
    let mask = size - 1;
    let base = (pmpaddr << 2) & !mask;
    (base, base.wrapping_add(size))
}

/// Map an access type to its corresponding access-fault
/// exception.
fn access_fault(access: AccessType) -> Exception {
    match access {
        AccessType::Read => Exception::LoadAccessFault,
        AccessType::Write => Exception::StoreAccessFault,
        AccessType::Execute => Exception::InstructionAccessFault,
    }
}
