//! Sv39 MMU with software TLB.
//!
//! Implements RISC-V Sv39 virtual-to-physical address
//! translation with a 256-entry direct-mapped TLB.
//! Supports 4KiB pages, 2MiB megapages, and 1GiB
//! gigapages.

use super::csr::PrivLevel;
use super::exception::Exception;
use super::pmp::Pmp;

// ── Sv39 constants ─────────────────────────────────────────────

const PAGE_SIZE: u64 = 4096;
const PTE_SIZE: u64 = 8;
const LEVELS: usize = 3;

/// VPN field width (9 bits per level).
const VPN_BITS: u32 = 9;
const VPN_MASK: u64 = (1 << VPN_BITS) - 1; // 0x1FF

/// SATP mode field: bits [63:60].
const SATP_MODE_SHIFT: u32 = 60;
const SATP_MODE_BARE: u64 = 0;
const SATP_MODE_SV39: u64 = 8;

/// SATP ASID field: bits [59:44].
const SATP_ASID_SHIFT: u32 = 44;
const SATP_ASID_MASK: u64 = 0xFFFF;

/// SATP PPN field: bits [43:0].
const SATP_PPN_MASK: u64 = (1u64 << 44) - 1;

// PTE flag bits
const PTE_V: u8 = 1 << 0;
const PTE_R: u8 = 1 << 1;
const PTE_W: u8 = 1 << 2;
const PTE_X: u8 = 1 << 3;
const PTE_U: u8 = 1 << 4;
#[allow(dead_code)]
const PTE_G: u8 = 1 << 5;
const PTE_A: u8 = 1 << 6;
const PTE_D: u8 = 1 << 7;

// mstatus bits used by permission checks
const MSTATUS_MXR: u64 = 1 << 19;
const MSTATUS_SUM: u64 = 1 << 18;

/// TLB size (direct-mapped, power of 2).
const TLB_SIZE: usize = 256;

// ── Types ──────────────────────────────────────────────────────

/// Memory access type for permission checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessType {
    Read,
    Write,
    Execute,
}

/// Single TLB entry.
#[derive(Default, Clone, Copy)]
struct TlbEntry {
    valid: bool,
    vpn: u64,
    ppn: u64,
    perm: u8,
    asid: u16,
    page_size: u64,
}

/// Translation statistics.
#[derive(Default)]
pub struct MmuStats {
    pub page_walks: u64,
    pub tlb_hits: u64,
    pub tlb_misses: u64,
}

/// Sv39 MMU with software TLB.
pub struct Mmu {
    satp: u64,
    tlb: Box<[TlbEntry; TLB_SIZE]>,
    stats: MmuStats,
}

// ── Helpers ────────────────────────────────────────────────────

/// Extract VPN[level] from a virtual address.
fn vpn_index(va: u64, level: usize) -> u64 {
    (va >> (12 + level as u32 * VPN_BITS)) & VPN_MASK
}

/// Page size for a given leaf level (0=4K, 1=2M, 2=1G).
fn level_page_size(level: usize) -> u64 {
    PAGE_SIZE << (level as u32 * VPN_BITS)
}

/// Map an access type to the corresponding page-fault
/// exception.
fn page_fault(access: AccessType) -> Exception {
    match access {
        AccessType::Read => Exception::LoadPageFault,
        AccessType::Write => Exception::StorePageFault,
        AccessType::Execute => Exception::InstructionPageFault,
    }
}

/// Compute TLB index from a 4K-granularity VPN.
fn tlb_index(vpn: u64) -> usize {
    let h = vpn ^ (vpn >> 8);
    (h as usize) & (TLB_SIZE - 1)
}

// ── Mmu implementation ─────────────────────────────────────────

impl Mmu {
    pub fn new() -> Self {
        Self {
            satp: 0,
            tlb: Box::new([TlbEntry::default(); TLB_SIZE]),
            stats: MmuStats::default(),
        }
    }

    pub fn set_satp(&mut self, val: u64) {
        self.satp = val;
    }

    pub fn get_satp(&self) -> u64 {
        self.satp
    }

    /// Translate a guest virtual address to a physical
    /// address using Sv39 page tables and TLB.
    ///
    /// `access_size` is the real memory access width in bytes
    /// (1, 2, 4, or 8) and is forwarded to PMP checks.
    ///
    /// `mem_read` reads an 8-byte value from guest physical
    /// memory.  `mem_write` writes an 8-byte value (used for
    /// hardware A/D bit updates in PTEs).
    ///
    /// If `pmp` is `Some`, a PMP access check is performed on
    /// the resulting GPA before returning.
    #[allow(clippy::too_many_arguments)]
    pub fn translate(
        &mut self,
        gva: u64,
        access: AccessType,
        priv_level: PrivLevel,
        mstatus: u64,
        access_size: u64,
        pmp: Option<&Pmp>,
        mem_read: impl Fn(u64) -> u64,
        mut mem_write: impl FnMut(u64, u64),
    ) -> Result<u64, Exception> {
        let mode = (self.satp >> SATP_MODE_SHIFT) & 0xF;
        if mode == SATP_MODE_BARE {
            return Ok(gva);
        }
        if mode != SATP_MODE_SV39 {
            return Err(page_fault(access));
        }

        let vpn = gva >> 12;
        let asid = ((self.satp >> SATP_ASID_SHIFT) & SATP_ASID_MASK) as u16;

        // TLB lookup
        let idx = tlb_index(vpn);
        let entry = self.tlb[idx];
        if entry.valid && entry.asid == asid {
            let pages = entry.page_size >> 12;
            let mask = !(pages - 1);
            if (entry.vpn & mask) == (vpn & mask) {
                self.check_perm(entry.perm, access, priv_level, mstatus)?;
                // Write to a page with D=0 requires a
                // re-walk to set the dirty bit in the PTE.
                let need_dirty =
                    access == AccessType::Write && entry.perm & PTE_D == 0;
                if !need_dirty {
                    self.stats.tlb_hits += 1;
                    let offset = gva & (entry.page_size - 1);
                    let pa = (entry.ppn << 12) | offset;
                    if let Some(p) = pmp {
                        p.check_access(pa, access_size, access, priv_level)?;
                    }
                    return Ok(pa);
                }
                // Invalidate and fall through to walk.
                self.tlb[idx].valid = false;
            }
        }

        // TLB miss — walk the page table
        self.stats.tlb_misses += 1;
        self.stats.page_walks += 1;

        let (ppn, perm, pg_size, pte_addr, pte) =
            self.walk_page_table(gva, access, &mem_read)?;

        self.check_perm(perm, access, priv_level, mstatus)?;

        // Hardware A/D bit management: set A (and D for
        // writes) in the PTE rather than faulting.
        let mut new_pte = pte | (PTE_A as u64);
        if access == AccessType::Write {
            new_pte |= PTE_D as u64;
        }
        if new_pte != pte {
            mem_write(pte_addr, new_pte);
        }
        let updated_perm = (new_pte & 0xFF) as u8;

        // Refill TLB with updated permissions
        self.tlb[idx] = TlbEntry {
            valid: true,
            vpn,
            ppn,
            perm: updated_perm,
            asid,
            page_size: pg_size,
        };

        let offset = gva & (pg_size - 1);
        let pa = (ppn << 12) | offset;

        if let Some(p) = pmp {
            p.check_access(pa, access_size, access, priv_level)?;
        }

        Ok(pa)
    }

    /// Three-level Sv39 page table walk.
    ///
    /// Returns `(ppn, perm_bits, page_size, pte_addr, pte)`
    /// on success.  The caller uses `pte_addr` and `pte` for
    /// hardware A/D bit updates.
    fn walk_page_table(
        &self,
        gva: u64,
        access: AccessType,
        mem_read: &impl Fn(u64) -> u64,
    ) -> Result<(u64, u8, u64, u64, u64), Exception> {
        let root_ppn = self.satp & SATP_PPN_MASK;
        let mut a = root_ppn * PAGE_SIZE;

        for level in (0..LEVELS).rev() {
            let idx = vpn_index(gva, level);
            let pte_addr = a + idx * PTE_SIZE;
            let pte = mem_read(pte_addr);

            let flags = (pte & 0xFF) as u8;

            // V bit must be set
            if flags & PTE_V == 0 {
                return Err(page_fault(access));
            }

            // W without R is reserved/invalid
            let r = flags & PTE_R != 0;
            let w = flags & PTE_W != 0;
            let x = flags & PTE_X != 0;
            if !r && w {
                return Err(page_fault(access));
            }

            // Leaf PTE: at least one of R, X is set
            if r || x {
                // Superpage alignment check
                if level > 0 {
                    let ppn_raw = pte >> 10;
                    let align_mask = (1u64 << (level as u32 * VPN_BITS)) - 1;
                    if ppn_raw & align_mask != 0 {
                        return Err(page_fault(access));
                    }
                }
                let ppn = pte >> 10;
                let pg_size = level_page_size(level);
                return Ok((ppn, flags, pg_size, pte_addr, pte));
            }

            // Non-leaf: descend to next level
            a = (pte >> 10) * PAGE_SIZE;
        }

        // Reached level 0 without finding a leaf
        Err(page_fault(access))
    }

    /// Check R/W/X permissions, U-bit, MXR, and SUM.
    fn check_perm(
        &self,
        perm: u8,
        access: AccessType,
        priv_level: PrivLevel,
        mstatus: u64,
    ) -> Result<(), Exception> {
        let r = perm & PTE_R != 0;
        let w = perm & PTE_W != 0;
        let x = perm & PTE_X != 0;
        let u = perm & PTE_U != 0;
        let mxr = mstatus & MSTATUS_MXR != 0;
        let sum = mstatus & MSTATUS_SUM != 0;

        // Access-type permission check
        let ok = match access {
            AccessType::Read => r || (mxr && x),
            AccessType::Write => w,
            AccessType::Execute => x,
        };
        if !ok {
            return Err(page_fault(access));
        }

        // U-bit vs privilege check.
        // SUM only affects S-mode data access (R/W), never
        // instruction fetch — spec §4.3.1.
        match priv_level {
            PrivLevel::User => {
                if !u {
                    return Err(page_fault(access));
                }
            }
            PrivLevel::Supervisor => {
                if u {
                    if access == AccessType::Execute {
                        return Err(page_fault(access));
                    }
                    if !sum {
                        return Err(page_fault(access));
                    }
                }
            }
            PrivLevel::Machine => {
                // M-mode bypasses U-bit check
            }
        }

        Ok(())
    }

    /// Flush the entire TLB (sfence.vma with rs1=x0,
    /// rs2=x0).
    pub fn flush(&mut self) {
        for e in self.tlb.iter_mut() {
            e.valid = false;
        }
    }

    /// Invalidate any TLB entry matching the given VPN
    /// (4K granularity).
    pub fn flush_page(&mut self, vpn: u64) {
        let idx = tlb_index(vpn);
        let entry = &mut self.tlb[idx];
        if entry.valid {
            let pages = entry.page_size >> 12;
            let mask = !(pages - 1);
            if (entry.vpn & mask) == (vpn & mask) {
                entry.valid = false;
            }
        }
    }

    pub fn stats(&self) -> &MmuStats {
        &self.stats
    }
}

impl Default for Mmu {
    fn default() -> Self {
        Self::new()
    }
}
