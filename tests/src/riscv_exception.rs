//! Integration tests for the RISC-V exception and
//! interrupt model.

use machina_guest_riscv::riscv::cpu::RiscvCpu;
use machina_guest_riscv::riscv::csr::PrivLevel;
use machina_guest_riscv::riscv::exception::Exception;

// -- Helpers --

fn make_cpu() -> RiscvCpu {
    let mut cpu = RiscvCpu::new();
    // Start in M-mode (default).
    assert_eq!(cpu.priv_level, PrivLevel::Machine);
    cpu.pc = 0;
    cpu
}

// -- Exception tests --

/// Exception from S-mode goes to M-mode (no delegation).
#[test]
fn test_exception_to_m_mode() {
    let mut cpu = make_cpu();
    // Switch to S-mode.
    cpu.priv_level = PrivLevel::Supervisor;
    cpu.pc = 0x1000;
    cpu.csr.mtvec = 0x8000_0000;
    // No delegation -- medeleg is 0.
    cpu.csr.medeleg = 0;
    // Enable MIE so we can verify it gets cleared.
    cpu.csr.mstatus |= 1 << 3; // MIE

    cpu.raise_exception(Exception::IllegalInstruction, 0xDEAD);

    // Should trap to M-mode.
    assert_eq!(cpu.priv_level, PrivLevel::Machine);
    assert_eq!(cpu.pc, 0x8000_0000);
    assert_eq!(cpu.csr.mepc, 0x1000);
    assert_eq!(cpu.csr.mcause, 2); // IllegalInstruction
    assert_eq!(cpu.csr.mtval, 0xDEAD);
    // MPP should be 1 (Supervisor).
    let mpp = (cpu.csr.mstatus >> 11) & 0x3;
    assert_eq!(mpp, 1);
    // MPIE should be 1 (was MIE=1).
    assert_ne!(cpu.csr.mstatus & (1 << 7), 0);
    // MIE should be 0.
    assert_eq!(cpu.csr.mstatus & (1 << 3), 0);
}

/// Exception delegated to S-mode via medeleg.
#[test]
fn test_exception_delegated_to_s_mode() {
    let mut cpu = make_cpu();
    // Switch to U-mode.
    cpu.priv_level = PrivLevel::User;
    cpu.pc = 0x2000;
    cpu.csr.stvec = 0x4000_0000;
    // Delegate IllegalInstruction (bit 2) to S-mode.
    cpu.csr.medeleg = 1 << 2;
    // Enable SIE so we can verify it gets cleared.
    cpu.csr.mstatus |= 1 << 1; // SIE

    cpu.raise_exception(Exception::IllegalInstruction, 0xBEEF);

    // Should trap to S-mode.
    assert_eq!(cpu.priv_level, PrivLevel::Supervisor);
    assert_eq!(cpu.pc, 0x4000_0000);
    assert_eq!(cpu.csr.sepc, 0x2000);
    assert_eq!(cpu.csr.scause, 2);
    assert_eq!(cpu.csr.stval, 0xBEEF);
    // SPP should be 0 (was User).
    assert_eq!(cpu.csr.mstatus & (1 << 8), 0);
    // SPIE should be 1 (was SIE=1).
    assert_ne!(cpu.csr.mstatus & (1 << 5), 0);
    // SIE should be 0.
    assert_eq!(cpu.csr.mstatus & (1 << 1), 0);
}

// -- MRET / SRET tests --

/// MRET restores privilege and PC correctly.
#[test]
fn test_mret() {
    let mut cpu = make_cpu();
    cpu.priv_level = PrivLevel::Machine;
    // Set MPP = Supervisor (1), MPIE = 1.
    cpu.csr.mstatus |= (1u64 << 11) | (1u64 << 7);
    cpu.csr.mepc = 0x3000;

    cpu.execute_mret();

    assert_eq!(cpu.priv_level, PrivLevel::Supervisor);
    assert_eq!(cpu.pc, 0x3000);
    // MIE should be restored from MPIE (was 1).
    assert_ne!(cpu.csr.mstatus & (1 << 3), 0);
    // MPIE should be set to 1.
    assert_ne!(cpu.csr.mstatus & (1 << 7), 0);
    // MPP should be cleared to User (0).
    let mpp = (cpu.csr.mstatus >> 11) & 0x3;
    assert_eq!(mpp, 0);
}

/// SRET restores privilege and PC correctly.
#[test]
fn test_sret() {
    let mut cpu = make_cpu();
    cpu.priv_level = PrivLevel::Supervisor;
    // Set SPP = 1 (Supervisor), SPIE = 1.
    cpu.csr.mstatus |= (1u64 << 8) | (1u64 << 5);
    cpu.csr.sepc = 0x5000;

    cpu.execute_sret();

    assert_eq!(cpu.priv_level, PrivLevel::Supervisor);
    assert_eq!(cpu.pc, 0x5000);
    // SIE should be restored from SPIE (was 1).
    assert_ne!(cpu.csr.mstatus & (1 << 1), 0);
    // SPIE should be set to 1.
    assert_ne!(cpu.csr.mstatus & (1 << 5), 0);
    // SPP should be cleared to 0.
    assert_eq!(cpu.csr.mstatus & (1 << 8), 0);
}

// -- Interrupt priority test --

/// MEI is taken before STI when both are pending.
#[test]
fn test_interrupt_priority() {
    let mut cpu = make_cpu();
    cpu.priv_level = PrivLevel::Machine;
    cpu.pc = 0x6000;
    cpu.csr.mtvec = 0xA000_0000;
    // Enable MIE in mstatus.
    cpu.csr.mstatus |= 1 << 3;

    // Enable MEI (bit 11) and STI (bit 5) in mie.
    cpu.csr.mie = (1 << 11) | (1 << 5);
    // Pend both MEI (bit 11) and STI (bit 5) in mip.
    cpu.csr.mip = (1 << 11) | (1 << 5);
    // Do NOT delegate MEI -- leave mideleg = 0.
    cpu.csr.mideleg = 0;

    let taken = cpu.handle_interrupt();
    assert!(taken);

    // MEI (code 11) should be taken, not STI (code 5).
    let expected_cause = (1u64 << 63) | 11;
    assert_eq!(cpu.csr.mcause, expected_cause);
    assert_eq!(cpu.priv_level, PrivLevel::Machine);
}
