//! SoftMMU/TLB regression tests covering plan ACs.

use machina_guest_riscv::riscv::csr::PrivLevel;
use machina_guest_riscv::riscv::mmu::{
    AccessType, Mmu, TLB_MMIO_ADDEND, TLB_SIZE,
};
use machina_guest_riscv::riscv::pmp::Pmp;

/// Helper: create an Mmu with Sv39 enabled (satp mode=8).
fn sv39_mmu(root_ppn: u64) -> Mmu {
    let mut mmu = Mmu::new();
    let satp = (8u64 << 60) | root_ppn;
    mmu.set_satp(satp);
    mmu
}

// ── AC-1: get_flags encodes priv + satp mode ─────────

#[test]
fn test_satp_mode_encoding() {
    let mmu = Mmu::new();
    // BARE mode: satp=0, mode=0
    assert_eq!(mmu.satp_mode(), 0);

    let sv39 = sv39_mmu(0x80000);
    assert_eq!(sv39.satp_mode(), 8);
}

// ── AC-5: sfence.vma flushes TLB ─────────────────────

#[test]
fn test_tlb_flush_clears_all_entries() {
    let mut mmu = Mmu::new();
    // Fill identity entries.
    mmu.fill_identity(0x8000_0000, 0x1234);
    mmu.fill_identity(0x8000_1000, 0x5678);

    assert!(mmu.tlb_lookup_read(0x8000_0000).is_some());
    assert!(mmu.tlb_lookup_read(0x8000_1000).is_some());

    mmu.flush();

    assert!(mmu.tlb_lookup_read(0x8000_0000).is_none());
    assert!(mmu.tlb_lookup_read(0x8000_1000).is_none());
}

// ── AC-7: MMIO sentinel in TLB ───────────────────────

#[test]
fn test_mmio_sentinel_forces_miss() {
    let mut mmu = Mmu::new();
    // Fill with MMIO sentinel addend.
    let gva = 0x1000_0000u64; // UART address
    mmu.fill_identity(gva, TLB_MMIO_ADDEND);

    // lookup_read returns None for MMIO sentinel.
    assert!(mmu.tlb_lookup_read(gva).is_none());
    assert!(mmu.tlb_lookup_write(gva).is_none());
}

// ── AC-8: three-way TLB API ──────────────────────────

#[test]
fn test_three_way_tlb_permissions() {
    let mut mmu = Mmu::new();
    let gva = 0x8000_2000u64;
    let addend = 0x7f00_0000_0000usize;

    // fill_identity sets all three tags (R+W+X).
    mmu.fill_identity(gva, addend);

    assert_eq!(mmu.tlb_lookup_read(gva), Some(addend));
    assert_eq!(mmu.tlb_lookup_code(gva), Some(addend));
    assert_eq!(mmu.tlb_lookup_write(gva), Some(addend));

    // After flush, all lookups miss.
    mmu.flush();
    assert!(mmu.tlb_lookup_read(gva).is_none());
    assert!(mmu.tlb_lookup_write(gva).is_none());
    assert!(mmu.tlb_lookup_code(gva).is_none());
}

// ── AC-13: M-mode identity mapping ───────────────────

#[test]
fn test_mmode_identity_fill() {
    let mut mmu = Mmu::new();
    let gva = 0x8000_3000u64;
    let guest_base = 0x7f00_0000_0000usize;

    mmu.fill_identity(gva, guest_base);

    assert_eq!(mmu.tlb_lookup_read(gva), Some(guest_base),);
    assert_eq!(mmu.tlb_lookup_write(gva), Some(guest_base),);
    assert_eq!(mmu.tlb_lookup_code(gva), Some(guest_base),);
}

// ── AC-12: PMP on page table walk ────────────────────

#[test]
fn test_pmp_deny_on_pte_read() {
    use machina_guest_riscv::riscv::csr::CsrFile;
    use machina_guest_riscv::riscv::exception::Exception;

    let mut mmu = sv39_mmu(0x80000);
    let mut pmp = Pmp::new();
    let mut csr = CsrFile::new();

    // Configure PMP: deny access to the page table
    // region (0x80000000 range) for S-mode by setting
    // a TOR entry with no permissions.
    // PMP entry 0: TOR up to 0x80000000, no permission
    use machina_guest_riscv::riscv::csr::{CSR_PMPADDR0, CSR_PMPCFG0};
    // pmpaddr0 = 0x80000000 >> 2 = 0x20000000
    csr.write(CSR_PMPADDR0, 0x2000_0000, PrivLevel::Machine)
        .unwrap();
    // pmpcfg0: TOR mode (0x08), no R/W/X
    csr.write(CSR_PMPCFG0, 0x08, PrivLevel::Machine).unwrap();
    pmp.sync_from_csr(&csr.pmpcfg, &csr.pmpaddr);

    let mem_read = |_pa: u64| -> u64 { 0 };
    let mut mem_write = |_pa: u64, _val: u64| {};

    // Attempting a translate should fail because the
    // page walk tries to read PTE at a physical address
    // denied by PMP.
    let result = mmu.translate_miss(
        0xC000_0000, // some VA
        AccessType::Read,
        PrivLevel::Supervisor,
        0, // mstatus
        8, // access_size
        Some(&pmp),
        &mem_read,
        &mut mem_write,
    );

    // Should get an access fault (not page fault)
    // because PMP denied the PTE read.
    assert!(
        matches!(result, Err(Exception::LoadAccessFault)),
        "expected LoadAccessFault, got {:?}",
        result,
    );
}

// ── Store fast-path hash regression ──────────────────

#[test]
fn test_tlb_index_consistency() {
    // Verify that tlb_index produces consistent
    // results for the same address.
    let gva = 0x87ff_fa88u64;
    let idx = machina_guest_riscv::riscv::mmu::tlb_index(gva);
    // The hash should be: vpn=0x87fff,
    // h = 0x87fff ^ (0x87fff >> 8) = 0x87fff ^ 0x87f
    let vpn = gva >> 12;
    let h = vpn ^ (vpn >> 8);
    let expected = (h as usize) & (TLB_SIZE - 1);
    assert_eq!(idx, expected);
    assert_eq!(idx, 128); // known value
}
