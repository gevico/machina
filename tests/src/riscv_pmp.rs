use machina_guest_riscv::riscv::csr::PrivLevel;
use machina_guest_riscv::riscv::exception::Exception;
use machina_guest_riscv::riscv::mmu::AccessType;
use machina_guest_riscv::riscv::pmp::Pmp;

// -- NAPOT match --

#[test]
fn test_pmp_napot_match() {
    let mut pmp = Pmp::new();

    // NAPOT region: pmpaddr = 0x1F  =>  G=5, size=256,
    // base=0x00, range [0, 256).
    // cfg = R|W|X | NAPOT(0x18) = 0x07 | 0x18 = 0x1F
    pmp.set_cfg(0, 0x1F);
    pmp.set_addr(0, 0x1F);

    // S-mode read within range succeeds.
    assert!(pmp
        .check_access(0, 4, AccessType::Read, PrivLevel::Supervisor,)
        .is_ok());

    // S-mode write at offset 128 succeeds.
    assert!(pmp
        .check_access(128, 8, AccessType::Write, PrivLevel::Supervisor,)
        .is_ok());

    // S-mode execute at offset 252 (last 4 bytes).
    assert!(pmp
        .check_access(252, 4, AccessType::Execute, PrivLevel::Supervisor,)
        .is_ok());

    // S-mode read outside the region is denied.
    assert_eq!(
        pmp.check_access(256, 4, AccessType::Read, PrivLevel::Supervisor,),
        Err(Exception::LoadAccessFault),
    );
}

// -- TOR match --

#[test]
fn test_pmp_tor_match() {
    let mut pmp = Pmp::new();

    // Entry 0: sets the lower bound to 0 (implicit).
    // Entry 1: TOR with pmpaddr = 0x100 => range
    // [0, 0x400). cfg = R|X | TOR(0x08) = 0x0D
    pmp.set_addr(0, 0x00);
    pmp.set_cfg(0, 0x00);

    pmp.set_addr(1, 0x100);
    pmp.set_cfg(1, 0x0D); // R|X | TOR

    // S-mode read within [0, 0x400) succeeds.
    assert!(pmp
        .check_access(0, 4, AccessType::Read, PrivLevel::Supervisor,)
        .is_ok());

    // S-mode execute within range succeeds.
    assert!(pmp
        .check_access(0x3FC, 4, AccessType::Execute, PrivLevel::Supervisor,)
        .is_ok());

    // S-mode write is denied (no W bit).
    assert_eq!(
        pmp.check_access(0, 4, AccessType::Write, PrivLevel::Supervisor,),
        Err(Exception::StoreAccessFault),
    );

    // S-mode read outside the region is denied.
    assert_eq!(
        pmp.check_access(0x400, 4, AccessType::Read, PrivLevel::Supervisor,),
        Err(Exception::LoadAccessFault),
    );
}

// -- S-mode default deny --

#[test]
fn test_pmp_no_match_deny_s_mode() {
    let pmp = Pmp::new(); // all entries OFF

    // S-mode access with no matching entries is denied.
    assert_eq!(
        pmp.check_access(0x1000, 4, AccessType::Read, PrivLevel::Supervisor,),
        Err(Exception::LoadAccessFault),
    );

    assert_eq!(
        pmp.check_access(0x1000, 4, AccessType::Write, PrivLevel::User,),
        Err(Exception::StoreAccessFault),
    );

    assert_eq!(
        pmp.check_access(0x1000, 4, AccessType::Execute, PrivLevel::User,),
        Err(Exception::InstructionAccessFault),
    );
}

// -- M-mode default allow --

#[test]
fn test_pmp_m_mode_default_allow() {
    let pmp = Pmp::new();

    assert!(pmp
        .check_access(0x1000, 4, AccessType::Read, PrivLevel::Machine,)
        .is_ok());

    assert!(pmp
        .check_access(0x2000, 4, AccessType::Write, PrivLevel::Machine,)
        .is_ok());

    assert!(pmp
        .check_access(0x3000, 4, AccessType::Execute, PrivLevel::Machine,)
        .is_ok());
}

// -- Locked entry restricts M-mode --

#[test]
fn test_pmp_locked_restricts_m_mode() {
    let mut pmp = Pmp::new();

    // Entry 0: locked NAPOT region [0, 256) with R only.
    // cfg = R | NAPOT(0x18) | L(0x80) = 0x99
    pmp.set_cfg(0, 0x99);
    pmp.set_addr(0, 0x1F);

    // M-mode read within the locked region (R=1).
    assert!(pmp
        .check_access(0, 4, AccessType::Read, PrivLevel::Machine,)
        .is_ok());

    // M-mode write denied (W=0, lock enforces).
    assert_eq!(
        pmp.check_access(0, 4, AccessType::Write, PrivLevel::Machine,),
        Err(Exception::StoreAccessFault),
    );

    // M-mode execute denied (X=0).
    assert_eq!(
        pmp.check_access(0, 4, AccessType::Execute, PrivLevel::Machine,),
        Err(Exception::InstructionAccessFault),
    );

    // M-mode access outside (no match) still allowed.
    assert!(pmp
        .check_access(0x1000, 4, AccessType::Read, PrivLevel::Machine,)
        .is_ok());
}

// -- sync_from_csr integration --

#[test]
fn test_pmp_sync_from_csr_tor_deny() {
    use machina_guest_riscv::riscv::csr::PMP_COUNT;

    let mut pmp = Pmp::new();
    let mut pmpcfg = [0u64; 4];
    let mut pmpaddr = [0u64; PMP_COUNT];

    // Entry 1: TOR, R|W (no X), range [0, 0x400).
    // cfg byte = R|W | TOR = 0x0B
    pmpcfg[0] = 0x0B << 8;
    pmpaddr[1] = 0x100;

    pmp.sync_from_csr(&pmpcfg, &pmpaddr);

    // S-mode read in range: allowed.
    assert!(pmp
        .check_access(0, 4, AccessType::Read, PrivLevel::Supervisor,)
        .is_ok());

    // S-mode execute in range: denied (no X).
    assert_eq!(
        pmp.check_access(0, 4, AccessType::Execute, PrivLevel::Supervisor,),
        Err(Exception::InstructionAccessFault),
    );

    // S-mode read outside range: denied (no match).
    assert_eq!(
        pmp.check_access(0x500, 4, AccessType::Read, PrivLevel::Supervisor,),
        Err(Exception::LoadAccessFault),
    );
}
