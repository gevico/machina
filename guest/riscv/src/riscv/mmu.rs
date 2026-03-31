//! Sv39 MMU with software TLB.
//!
//! Implements RISC-V Sv39 virtual-to-physical address
//! translation with a 256-entry direct-mapped TLB.
//! Supports 4KiB pages, 2MiB megapages, and 1GiB
//! gigapages.
//!
//! The TLB uses QEMU-style three-tag entries with a host
//! address addend for JIT fast-path access.

use super::csr::PrivLevel;
use super::exception::Exception;
use super::pmp::Pmp;

// ── Sv39 constants ────────────────────────────────────

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
pub const TLB_SIZE: usize = 256;

/// Page mask for tag comparison (low 12 bits zeroed).
pub const PAGE_MASK: u64 = !(PAGE_SIZE - 1);

/// Sentinel tag value: never matches any page-aligned
/// address (used for invalid/empty entries).
const TLB_INVALID_TAG: u64 = u64::MAX;

/// Sentinel addend: forces the JIT slow path. Used for
/// MMIO entries where direct host pointer access is
/// invalid.
pub const TLB_MMIO_ADDEND: usize = usize::MAX;

// ── Types ─────────────────────────────────────────────

/// Memory access type for permission checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessType {
    Read,
    Write,
    Execute,
}

/// QEMU-style three-tag TLB entry with host addend.
///
/// `#[repr(C)]` for stable field offsets accessed by
/// JIT-generated code.
///
/// Tag fields store the page-aligned VA (gva & PAGE_MASK)
/// when the entry has the corresponding permission, or
/// TLB_INVALID_TAG when the permission is denied or the
/// entry is empty. The JIT inline check compares
/// `(addr & PAGE_MASK) == entry.addr_read` for a load,
/// and on match uses `addr + entry.addend` as the host
/// address.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TlbEntry {
    /// Tag for read access (page-aligned VA or invalid).
    pub addr_read: u64,
    /// Tag for write access (page-aligned VA or invalid).
    pub addr_write: u64,
    /// Tag for execute/fetch access (page-aligned VA or
    /// invalid).
    pub addr_code: u64,
    /// Host address addend: host_addr = gva + addend.
    /// TLB_MMIO_ADDEND for MMIO (forces slow path).
    pub addend: usize,
    // -- Internal fields (not accessed by JIT) --
    perm: u8,
    asid: u16,
    page_size: u64,
}

/// Field offsets within TlbEntry (bytes from start).
pub mod tlb_offsets {
    pub const ADDR_READ: usize = 0;
    pub const ADDR_WRITE: usize = 8;
    pub const ADDR_CODE: usize = 16;
    pub const ADDEND: usize = 24;
    /// Size of one TlbEntry in bytes.
    pub const ENTRY_SIZE: usize = core::mem::size_of::<super::TlbEntry>();
}

impl Default for TlbEntry {
    fn default() -> Self {
        Self {
            addr_read: TLB_INVALID_TAG,
            addr_write: TLB_INVALID_TAG,
            addr_code: TLB_INVALID_TAG,
            addend: 0,
            perm: 0,
            asid: 0,
            page_size: 0,
        }
    }
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
    pub tlb: Box<[TlbEntry; TLB_SIZE]>,
    stats: MmuStats,
}

// ── Helpers ───────────────────────────────────────────

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

/// Map an access type to the corresponding access-fault
/// exception (for PMP violations).
pub fn access_fault(access: AccessType) -> Exception {
    match access {
        AccessType::Read => Exception::LoadAccessFault,
        AccessType::Write => Exception::StoreAccessFault,
        AccessType::Execute => Exception::InstructionAccessFault,
    }
}

/// Compute TLB index from a virtual address.
pub fn tlb_index(va: u64) -> usize {
    let vpn = va >> 12;
    let h = vpn ^ (vpn >> 8);
    (h as usize) & (TLB_SIZE - 1)
}

// ── Mmu implementation ────────────────────────────────

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

    /// Return the SATP mode field (bits [63:60]).
    pub fn satp_mode(&self) -> u64 {
        (self.satp >> SATP_MODE_SHIFT) & 0xF
    }

    // ── Three-way TLB lookup API ──────────────────────

    /// Lookup TLB for a code/execute access.
    /// Returns `Some(addend)` on hit (non-MMIO), None on
    /// miss or MMIO.
    pub fn tlb_lookup_code(&self, gva: u64) -> Option<usize> {
        let idx = tlb_index(gva);
        let entry = &self.tlb[idx];
        let tag = gva & PAGE_MASK;
        if entry.addr_code == tag && entry.addend != TLB_MMIO_ADDEND {
            Some(entry.addend)
        } else {
            None
        }
    }

    /// Lookup TLB for a read/load access.
    /// Returns `Some(addend)` on hit (non-MMIO), None on
    /// miss or MMIO.
    pub fn tlb_lookup_read(&self, gva: u64) -> Option<usize> {
        let idx = tlb_index(gva);
        let entry = &self.tlb[idx];
        let tag = gva & PAGE_MASK;
        if entry.addr_read == tag && entry.addend != TLB_MMIO_ADDEND {
            Some(entry.addend)
        } else {
            None
        }
    }

    /// Lookup TLB for a write/store access.
    /// Returns `Some(addend)` on hit (non-MMIO), None on
    /// miss or MMIO.
    pub fn tlb_lookup_write(&self, gva: u64) -> Option<usize> {
        let idx = tlb_index(gva);
        let entry = &self.tlb[idx];
        let tag = gva & PAGE_MASK;
        if entry.addr_write == tag && entry.addend != TLB_MMIO_ADDEND {
            Some(entry.addend)
        } else {
            None
        }
    }

    // ── Translation ───────────────────────────────────

    /// Translate a guest virtual address to a physical
    /// address using Sv39 page tables and TLB.
    ///
    /// This is the full translation path used by the slow
    /// path helper. It handles TLB lookup, page walk,
    /// A/D bit management, TLB refill, and PMP check.
    ///
    /// `host_ram_ptr` is the host base address of guest
    /// RAM; if Some, the TLB entry is filled with a valid
    /// host addend. If None, the addend is not computed
    /// (used for non-JIT callers).
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
        let mode = self.satp_mode();
        if mode == SATP_MODE_BARE {
            return Ok(gva);
        }
        if mode != SATP_MODE_SV39 {
            return Err(page_fault(access));
        }

        let tag = gva & PAGE_MASK;
        let idx = tlb_index(gva);
        let entry = self.tlb[idx];

        // TLB hit check (three-tag style)
        let tag_match = match access {
            AccessType::Read => entry.addr_read == tag,
            AccessType::Write => entry.addr_write == tag,
            AccessType::Execute => entry.addr_code == tag,
        };

        if tag_match {
            self.stats.tlb_hits += 1;
            let pa_page = (gva.wrapping_add(entry.addend as u64)) & PAGE_MASK;
            let offset = gva & (PAGE_SIZE - 1);
            let pa = pa_page | offset;
            // For MMIO entries, addend is sentinel — the
            // caller must detect this and route through
            // AddressSpace. But tag match means
            // permissions are OK, so we can return the PA.
            // However, for writes we must re-check dirty
            // bit: if addr_write matched, dirty is set.
            if let Some(p) = pmp {
                p.check_access(pa, access_size, access, priv_level)?;
            }
            return Ok(pa);
        }

        // TLB miss — walk the page table
        self.translate_miss(
            gva,
            access,
            priv_level,
            mstatus,
            access_size,
            pmp,
            &mem_read,
            &mut mem_write,
        )
    }

    /// TLB miss handler: walk page table, refill TLB,
    /// and return the physical address.
    #[allow(clippy::too_many_arguments)]
    pub fn translate_miss(
        &mut self,
        gva: u64,
        access: AccessType,
        priv_level: PrivLevel,
        mstatus: u64,
        access_size: u64,
        pmp: Option<&Pmp>,
        mem_read: &impl Fn(u64) -> u64,
        mem_write: &mut impl FnMut(u64, u64),
    ) -> Result<u64, Exception> {
        self.stats.tlb_misses += 1;
        self.stats.page_walks += 1;

        let (ppn, perm, pg_size, pte_addr, pte) =
            self.walk_page_table(gva, access, pmp, priv_level, mem_read)?;

        self.check_perm(perm, access, priv_level, mstatus)?;

        // Hardware A/D bit management
        let mut new_pte = pte | (PTE_A as u64);
        if access == AccessType::Write {
            new_pte |= PTE_D as u64;
        }
        if new_pte != pte {
            // PMP check on PTE write for A/D update.
            if let Some(p) = pmp {
                p.check_access(
                    pte_addr,
                    PTE_SIZE,
                    AccessType::Write,
                    priv_level,
                )
                .map_err(|_| access_fault(access))?;
            }
            mem_write(pte_addr, new_pte);
        }
        let updated_perm = (new_pte & 0xFF) as u8;

        let offset = gva & (pg_size - 1);
        let pa = (ppn << 12) | offset;

        if let Some(p) = pmp {
            p.check_access(pa, access_size, access, priv_level)?;
        }

        // Refill TLB entry with three-tag format.
        // Tags are set based on the page's permissions.
        let tag = gva & PAGE_MASK;
        let r = updated_perm & PTE_R != 0;
        let w = updated_perm & PTE_W != 0;
        let x = updated_perm & PTE_X != 0;
        let d = updated_perm & PTE_D != 0;
        let mxr = mstatus & MSTATUS_MXR != 0;
        let asid = ((self.satp >> SATP_ASID_SHIFT) & SATP_ASID_MASK) as u16;

        let idx = tlb_index(gva);
        let entry = &mut self.tlb[idx];
        entry.addr_read = if r || (mxr && x) {
            tag
        } else {
            TLB_INVALID_TAG
        };
        entry.addr_write = if w && d { tag } else { TLB_INVALID_TAG };
        entry.addr_code = if x { tag } else { TLB_INVALID_TAG };
        // Addend will be set by the caller (fill_addend)
        // after checking RAM vs MMIO. Default to 0.
        entry.addend = 0;
        entry.perm = updated_perm;
        entry.asid = asid;
        entry.page_size = pg_size;

        Ok(pa)
    }

    /// Fill the addend field of a TLB entry for the given
    /// VA. Called after translate_miss() succeeds.
    ///
    /// For RAM addresses: addend = host_ptr - gva_page
    /// (so host_addr = gva + addend).
    /// For MMIO: addend = TLB_MMIO_ADDEND (sentinel).
    pub fn fill_addend(&mut self, gva: u64, addend: usize) {
        let idx = tlb_index(gva);
        self.tlb[idx].addend = addend;
    }

    /// Fill TLB with an identity mapping for M-mode
    /// (VA == PA, all permissions granted).
    pub fn fill_identity(&mut self, gva: u64, addend: usize) {
        let tag = gva & PAGE_MASK;
        let idx = tlb_index(gva);
        let entry = &mut self.tlb[idx];
        entry.addr_read = tag;
        entry.addr_write = tag;
        entry.addr_code = tag;
        entry.addend = addend;
        entry.perm = PTE_R | PTE_W | PTE_X | PTE_A | PTE_D;
        entry.asid = 0;
        entry.page_size = PAGE_SIZE;
    }

    /// Three-level Sv39 page table walk.
    ///
    /// Returns `(ppn, perm_bits, page_size, pte_addr, pte)`
    /// on success.
    fn walk_page_table(
        &self,
        gva: u64,
        access: AccessType,
        pmp: Option<&Pmp>,
        priv_level: PrivLevel,
        mem_read: &impl Fn(u64) -> u64,
    ) -> Result<(u64, u8, u64, u64, u64), Exception> {
        let root_ppn = self.satp & SATP_PPN_MASK;
        let mut a = root_ppn * PAGE_SIZE;

        for level in (0..LEVELS).rev() {
            let idx = vpn_index(gva, level);
            let pte_addr = a + idx * PTE_SIZE;
            // PMP check on PTE read (AC-12).
            if let Some(p) = pmp {
                p.check_access(
                    pte_addr,
                    PTE_SIZE,
                    AccessType::Read,
                    priv_level,
                )
                .map_err(|_| access_fault(access))?;
            }
            let pte = mem_read(pte_addr);

            let flags = (pte & 0xFF) as u8;

            if flags & PTE_V == 0 {
                return Err(page_fault(access));
            }

            let r = flags & PTE_R != 0;
            let w = flags & PTE_W != 0;
            let x = flags & PTE_X != 0;
            if !r && w {
                return Err(page_fault(access));
            }

            // Leaf PTE
            if r || x {
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

            // Non-leaf: descend
            a = (pte >> 10) * PAGE_SIZE;
        }

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

        let ok = match access {
            AccessType::Read => r || (mxr && x),
            AccessType::Write => w,
            AccessType::Execute => x,
        };
        if !ok {
            return Err(page_fault(access));
        }

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
            PrivLevel::Machine => {}
        }

        Ok(())
    }

    /// Flush the entire TLB (sfence.vma with rs1=x0,
    /// rs2=x0).
    pub fn flush(&mut self) {
        for e in self.tlb.iter_mut() {
            e.addr_read = TLB_INVALID_TAG;
            e.addr_write = TLB_INVALID_TAG;
            e.addr_code = TLB_INVALID_TAG;
        }
    }

    /// Invalidate any TLB entry matching the given VPN
    /// (4K granularity).
    pub fn flush_page(&mut self, vpn: u64) {
        let gva = vpn << 12;
        let idx = tlb_index(gva);
        let entry = &mut self.tlb[idx];
        let tag = gva & PAGE_MASK;
        // Check if any tag matches this page.
        if entry.addr_read == tag
            || entry.addr_write == tag
            || entry.addr_code == tag
        {
            entry.addr_read = TLB_INVALID_TAG;
            entry.addr_write = TLB_INVALID_TAG;
            entry.addr_code = TLB_INVALID_TAG;
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
