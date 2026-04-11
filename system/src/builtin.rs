// Builtin firmware mode framework.
//
// In builtin mode Machina bypasses external firmware and starts
// the guest kernel directly in the privilege level the firmware
// would have established (e.g. S-mode on RISC-V with SBI ready).
// The host provides firmware services via a FirmwareCallFn
// callback installed by the ISA backend.
//
// # Extending to a new ISA
//
//   1. Implement a firmware backend in hw/<isa>/src/builtin.rs
//      (or sbi.rs / hvc.rs / svc.rs as appropriate for the ABI).
//   2. Wrap it in a FirmwareCallFn and install via
//      CpuManager::set_firmware_handler().
//   3. Set FullSystemCpu::builtin_mode = true on the target hart.
//   4. Implement boot_builtin() in hw/<isa>/src/boot.rs that
//      initialises the guest CPU to the post-firmware state
//      (privilege level, a0/a1 convention, delegation CSRs, etc.).
//
// The MTI-to-STI conversion in FullSystemCpu::handle_interrupt()
// is currently RISC-V specific. Future ISAs that need analogous
// timer-interrupt translation should add a similar flag or a
// small hook on FullSystemCpu.

use std::sync::Arc;

use machina_guest_riscv::riscv::cpu::RiscvCpu;

/// Callback type for handling guest firmware calls in builtin
/// mode.
///
/// On RISC-V this is called for S-mode ecalls; the handler
/// reads arguments from a0–a7, performs the service, writes
/// return values into a0/a1, and advances PC past the ecall.
///
/// The type is currently tied to RiscvCpu because the system
/// crate only supports RISC-V. It will be generalised when
/// additional ISAs are added.
pub type FirmwareCallFn = Arc<dyn Fn(&mut RiscvCpu) + Send + Sync>;
