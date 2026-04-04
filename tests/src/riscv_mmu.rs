use std::cell::RefCell;

use machina_guest_riscv::riscv::csr::PrivLevel;
use machina_guest_riscv::riscv::exception::Exception;
use machina_guest_riscv::riscv::mmu::{AccessType, Mmu};
use machina_guest_riscv::riscv::pmp::Pmp;

// -- Helpers --

/// Write a 64-bit PTE into the flat memory array.
fn write_pte(mem: &mut [u8], addr: u64, pte: u64) {
    let off = addr as usize;
    mem[off..off + 8].copy_from_slice(&pte.to_le_bytes());
}

/// Build a `mem_read` closure over a byte slice.
fn mem_reader(mem: &[u8]) -> impl Fn(u64) -> u64 + '_ {
    move |addr: u64| {
        let off = addr as usize;
        u64::from_le_bytes(mem[off..off + 8].try_into().unwrap())
    }
}

/// Build a `mem_read` closure over a RefCell byte slice
/// (for tests that also need `mem_write`).
fn mem_reader_rc(mem: &RefCell<Vec<u8>>) -> impl Fn(u64) -> u64 + '_ {
    move |addr: u64| {
        let m = mem.borrow();
        let off = addr as usize;
        u64::from_le_bytes(m[off..off + 8].try_into().unwrap())
    }
}

/// Build a `mem_write` closure over a RefCell byte slice.
fn mem_writer_rc(mem: &RefCell<Vec<u8>>) -> impl FnMut(u64, u64) + '_ {
    move |addr: u64, val: u64| {
        let mut m = mem.borrow_mut();
        let off = addr as usize;
        m[off..off + 8].copy_from_slice(&val.to_le_bytes());
    }
}

/// No-op writer for tests that don't care about A/D
/// updates.
fn no_write(_addr: u64, _val: u64) {}

// -- Sv39 PTE flag helpers --

const PTE_V: u64 = 1 << 0;
const PTE_R: u64 = 1 << 1;
const PTE_W: u64 = 1 << 2;
const PTE_X: u64 = 1 << 3;
const PTE_U: u64 = 1 << 4;
const PTE_A: u64 = 1 << 6;
const PTE_D: u64 = 1 << 7;

const MSTATUS_SUM: u64 = 1 << 18;

/// Build a leaf PTE: ppn in bits [53:10], flags in [7:0].
fn leaf_pte(ppn: u64, flags: u64) -> u64 {
    (ppn << 10) | flags
}

/// Build a non-leaf (pointer) PTE.
fn ptr_pte(next_ppn: u64) -> u64 {
    (next_ppn << 10) | PTE_V
}

/// SATP value for Sv39 mode.
fn satp_sv39(asid: u16, root_ppn: u64) -> u64 {
    (8u64 << 60) | ((asid as u64) << 44) | root_ppn
}

// -- Tests --

#[test]
fn test_bare_mode_passthrough() {
    let mut mmu = Mmu::new();
    // satp = 0 means MODE = Bare
    mmu.set_satp(0);
    assert_eq!(mmu.get_satp(), 0);

    let dummy = |_addr: u64| -> u64 { 0 };
    let addrs: &[u64] = &[0, 0x1000, 0x8000_0000, u64::MAX];
    for &gva in addrs {
        let pa = mmu.translate(
            gva,
            AccessType::Read,
            PrivLevel::Machine,
            0,
            4,
            None,
            dummy,
            no_write,
        );
        assert_eq!(pa, Ok(gva));
    }
}

#[test]
fn test_sv39_identity_map() {
    let mem_size = 0x10000;
    let mut mem = vec![0u8; mem_size];

    write_pte(&mut mem, 0x1000, ptr_pte(2));
    write_pte(&mut mem, 0x2000, ptr_pte(3));
    let flags = PTE_V | PTE_R | PTE_W | PTE_X | PTE_A | PTE_D;
    write_pte(&mut mem, 0x3000, leaf_pte(0, flags));

    let mut mmu = Mmu::new();
    mmu.set_satp(satp_sv39(0, 1));

    let reader = mem_reader(&mem);

    let pa = mmu.translate(
        0x0100,
        AccessType::Read,
        PrivLevel::Machine,
        0,
        4,
        None,
        &reader,
        no_write,
    );
    assert_eq!(pa, Ok(0x0100));

    let pa = mmu.translate(
        0x0ABC,
        AccessType::Write,
        PrivLevel::Machine,
        0,
        4,
        None,
        &reader,
        no_write,
    );
    assert_eq!(pa, Ok(0x0ABC));
}

#[test]
fn test_sv39_page_fault() {
    let mem_size = 0x10000;
    let mut mem = vec![0u8; mem_size];

    write_pte(&mut mem, 0x1000, ptr_pte(2));
    write_pte(&mut mem, 0x2000, ptr_pte(3));
    let flags = PTE_V | PTE_R | PTE_W | PTE_X | PTE_A | PTE_D;
    write_pte(&mut mem, 0x3000, leaf_pte(0, flags));

    let mut mmu = Mmu::new();
    mmu.set_satp(satp_sv39(0, 1));

    let reader = mem_reader(&mem);

    // Page 0 works
    assert!(mmu
        .translate(
            0x0000,
            AccessType::Read,
            PrivLevel::Machine,
            0,
            4,
            None,
            &reader,
            no_write,
        )
        .is_ok());

    // Flush TLB so we force a walk
    mmu.flush();

    // Page 1 (VA 0x1000) is unmapped -> LoadPageFault
    let err = mmu.translate(
        0x1000,
        AccessType::Read,
        PrivLevel::Machine,
        0,
        4,
        None,
        &reader,
        no_write,
    );
    assert_eq!(err, Err(Exception::LoadPageFault));

    // Execute fault
    mmu.flush();
    let err = mmu.translate(
        0x1000,
        AccessType::Execute,
        PrivLevel::Machine,
        0,
        4,
        None,
        &reader,
        no_write,
    );
    assert_eq!(err, Err(Exception::InstructionPageFault));

    // Store fault
    mmu.flush();
    let err = mmu.translate(
        0x1000,
        AccessType::Write,
        PrivLevel::Machine,
        0,
        4,
        None,
        &reader,
        no_write,
    );
    assert_eq!(err, Err(Exception::StorePageFault));
}

#[test]
fn test_tlb_hit() {
    let mem_size = 0x10000;
    let mut mem = vec![0u8; mem_size];

    write_pte(&mut mem, 0x1000, ptr_pte(2));
    write_pte(&mut mem, 0x2000, ptr_pte(3));
    let flags = PTE_V | PTE_R | PTE_W | PTE_X | PTE_A | PTE_D;
    write_pte(&mut mem, 0x3000, leaf_pte(0, flags));

    let mut mmu = Mmu::new();
    mmu.set_satp(satp_sv39(0, 1));

    let reader = mem_reader(&mem);

    // First access: TLB miss -> page walk
    let pa1 = mmu.translate(
        0x0042,
        AccessType::Read,
        PrivLevel::Machine,
        0,
        4,
        None,
        &reader,
        no_write,
    );
    assert_eq!(pa1, Ok(0x0042));
    let walks_after_first = mmu.stats().page_walks;
    assert_eq!(walks_after_first, 1);
    assert_eq!(mmu.stats().tlb_misses, 1);

    // Second access to same page: TLB hit, no new walk
    let pa2 = mmu.translate(
        0x0084,
        AccessType::Read,
        PrivLevel::Machine,
        0,
        4,
        None,
        &reader,
        no_write,
    );
    assert_eq!(pa2, Ok(0x0084));
    assert_eq!(mmu.stats().page_walks, walks_after_first);
    assert_eq!(mmu.stats().tlb_hits, 1);
}

#[test]
fn test_superpage() {
    let mem_size = 0x10000;
    let mut mem = vec![0u8; mem_size];

    write_pte(&mut mem, 0x1000, ptr_pte(2));
    let mega_ppn: u64 = 0x200;
    let flags = PTE_V | PTE_R | PTE_W | PTE_X | PTE_U | PTE_A | PTE_D;
    write_pte(&mut mem, 0x2000, leaf_pte(mega_ppn, flags));

    let mut mmu = Mmu::new();
    mmu.set_satp(satp_sv39(0, 1));

    let reader = mem_reader(&mem);

    let pa = mmu.translate(
        0x0000_0000,
        AccessType::Read,
        PrivLevel::User,
        0,
        4,
        None,
        &reader,
        no_write,
    );
    assert_eq!(pa, Ok(0x0020_0000));

    mmu.flush();
    let pa = mmu.translate(
        0x0010_0000,
        AccessType::Write,
        PrivLevel::User,
        0,
        4,
        None,
        &reader,
        no_write,
    );
    assert_eq!(pa, Ok(0x0030_0000));

    mmu.flush();
    let pa = mmu.translate(
        0x001F_FFFF,
        AccessType::Execute,
        PrivLevel::User,
        0,
        4,
        None,
        &reader,
        no_write,
    );
    assert_eq!(pa, Ok(0x003F_FFFF));
}

// -- A/D bit update tests --

#[test]
fn test_ad_bit_update_on_read() {
    // PTE has V|R|W|X but NOT A or D.
    let mem_size = 0x10000;
    let mem = RefCell::new(vec![0u8; mem_size]);

    {
        let mut m = mem.borrow_mut();
        write_pte(&mut m, 0x1000, ptr_pte(2));
        write_pte(&mut m, 0x2000, ptr_pte(3));
        let flags = PTE_V | PTE_R | PTE_W | PTE_X;
        write_pte(&mut m, 0x3000, leaf_pte(0, flags));
    }

    let mut mmu = Mmu::new();
    mmu.set_satp(satp_sv39(0, 1));

    let pa = mmu.translate(
        0x0042,
        AccessType::Read,
        PrivLevel::Machine,
        0,
        4,
        None,
        mem_reader_rc(&mem),
        mem_writer_rc(&mem),
    );
    assert_eq!(pa, Ok(0x0042));

    // Verify PTE now has A=1, D still 0.
    let updated_pte = {
        let m = mem.borrow();
        u64::from_le_bytes(m[0x3000..0x3008].try_into().unwrap())
    };
    assert_ne!(updated_pte & PTE_A, 0, "A bit must be set");
    assert_eq!(updated_pte & PTE_D, 0, "D bit must remain clear on read");
}

#[test]
fn test_ad_bit_update_on_write() {
    // PTE has V|R|W|X but NOT A or D.
    let mem_size = 0x10000;
    let mem = RefCell::new(vec![0u8; mem_size]);

    {
        let mut m = mem.borrow_mut();
        write_pte(&mut m, 0x1000, ptr_pte(2));
        write_pte(&mut m, 0x2000, ptr_pte(3));
        let flags = PTE_V | PTE_R | PTE_W | PTE_X;
        write_pte(&mut m, 0x3000, leaf_pte(0, flags));
    }

    let mut mmu = Mmu::new();
    mmu.set_satp(satp_sv39(0, 1));

    let pa = mmu.translate(
        0x0042,
        AccessType::Write,
        PrivLevel::Machine,
        0,
        4,
        None,
        mem_reader_rc(&mem),
        mem_writer_rc(&mem),
    );
    assert_eq!(pa, Ok(0x0042));

    // Verify PTE now has A=1 and D=1.
    let updated_pte = {
        let m = mem.borrow();
        u64::from_le_bytes(m[0x3000..0x3008].try_into().unwrap())
    };
    assert_ne!(updated_pte & PTE_A, 0, "A bit must be set");
    assert_ne!(updated_pte & PTE_D, 0, "D bit must be set");
}

// -- SUM semantics tests --

#[test]
fn test_sum_blocks_s_mode_execute_on_u_page() {
    // U-page with SUM=1: S-mode execute must still fail.
    let mem_size = 0x10000;
    let mut mem = vec![0u8; mem_size];

    write_pte(&mut mem, 0x1000, ptr_pte(2));
    write_pte(&mut mem, 0x2000, ptr_pte(3));
    let flags = PTE_V | PTE_R | PTE_W | PTE_X | PTE_U | PTE_A | PTE_D;
    write_pte(&mut mem, 0x3000, leaf_pte(0, flags));

    let mut mmu = Mmu::new();
    mmu.set_satp(satp_sv39(0, 1));

    let reader = mem_reader(&mem);

    let err = mmu.translate(
        0x0000,
        AccessType::Execute,
        PrivLevel::Supervisor,
        MSTATUS_SUM,
        4,
        None,
        &reader,
        no_write,
    );
    assert_eq!(
        err,
        Err(Exception::InstructionPageFault),
        "S-mode execute on U-page must fault even with SUM"
    );
}

#[test]
fn test_sum_allows_s_mode_read_on_u_page() {
    // U-page with SUM=1: S-mode read must succeed.
    let mem_size = 0x10000;
    let mut mem = vec![0u8; mem_size];

    write_pte(&mut mem, 0x1000, ptr_pte(2));
    write_pte(&mut mem, 0x2000, ptr_pte(3));
    let flags = PTE_V | PTE_R | PTE_W | PTE_X | PTE_U | PTE_A | PTE_D;
    write_pte(&mut mem, 0x3000, leaf_pte(0, flags));

    let mut mmu = Mmu::new();
    mmu.set_satp(satp_sv39(0, 1));

    let reader = mem_reader(&mem);

    let pa = mmu.translate(
        0x0100,
        AccessType::Read,
        PrivLevel::Supervisor,
        MSTATUS_SUM,
        4,
        None,
        &reader,
        no_write,
    );
    assert_eq!(
        pa,
        Ok(0x0100),
        "S-mode read on U-page must succeed with SUM"
    );
}

// -- PMP integration test --

#[test]
fn test_pmp_deny_after_translation() {
    // Successful page walk, but PMP denies the access.
    let mem_size = 0x10000;
    let mut mem = vec![0u8; mem_size];

    write_pte(&mut mem, 0x1000, ptr_pte(2));
    write_pte(&mut mem, 0x2000, ptr_pte(3));
    let flags = PTE_V | PTE_R | PTE_W | PTE_X | PTE_A | PTE_D;
    write_pte(&mut mem, 0x3000, leaf_pte(0, flags));

    let mut mmu = Mmu::new();
    mmu.set_satp(satp_sv39(0, 1));

    // PMP: NAPOT region [0, 256) with X-only (no R).
    // cfg = X | NAPOT = 0x04 | 0x18 = 0x1C
    let mut pmp = Pmp::new();
    pmp.set_cfg(0, 0x1C);
    pmp.set_addr(0, 0x1F); // [0, 256)

    let reader = mem_reader(&mem);

    // S-mode read to PA within PMP region that lacks R.
    let err = mmu.translate(
        0x0000,
        AccessType::Read,
        PrivLevel::Supervisor,
        0,
        4,
        Some(&pmp),
        &reader,
        no_write,
    );
    assert_eq!(
        err,
        Err(Exception::LoadAccessFault),
        "PMP must deny read when R bit is not set"
    );
}

#[test]
fn test_pmp_subpage_deny() {
    // PMP entry covers only 4 bytes (NA4 at PA 0x80).
    let mem_size = 0x10000;
    let mut mem = vec![0u8; mem_size];

    // Identity-map page 0.
    write_pte(&mut mem, 0x1000, ptr_pte(2));
    write_pte(&mut mem, 0x2000, ptr_pte(3));
    let flags = PTE_V | PTE_R | PTE_W | PTE_X | PTE_A | PTE_D;
    write_pte(&mut mem, 0x3000, leaf_pte(0, flags));

    let mut mmu = Mmu::new();
    mmu.set_satp(satp_sv39(0, 1));

    // NA4 at addr 0x80: pmpaddr = 0x80 >> 2 = 0x20.
    // cfg = L=1 | NA4, no R/W/X => 0x90.
    let mut pmp = Pmp::new();
    pmp.set_cfg(0, 0x90);
    pmp.set_addr(0, 0x20);

    let reader = mem_reader(&mem);

    // 4-byte read at PA 0x80 must be denied.
    let err = mmu.translate(
        0x0080,
        AccessType::Read,
        PrivLevel::Machine,
        0,
        4,
        Some(&pmp),
        &reader,
        no_write,
    );
    assert_eq!(
        err,
        Err(Exception::LoadAccessFault),
        "PMP must deny 4-byte read at subpage region"
    );

    // Flush TLB so the next call does a fresh walk.
    mmu.flush();

    // 4-byte read at PA 0x84 must succeed.
    let pa = mmu.translate(
        0x0084,
        AccessType::Read,
        PrivLevel::Machine,
        0,
        4,
        Some(&pmp),
        &reader,
        no_write,
    );
    assert_eq!(
        pa,
        Ok(0x0084),
        "access outside PMP subpage region must succeed"
    );
}
