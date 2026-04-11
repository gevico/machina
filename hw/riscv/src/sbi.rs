// Host-side SBI backend for -bios builtin mode.
//
// Implements RISC-V SBI spec v2.0 on the host side.
// Guest S-mode ecalls are intercepted by the exec loop
// and dispatched here instead of being delivered as CPU
// traps.
//
// Extensions implemented:
//   EID 0x00        Legacy SET_TIMER
//   EID 0x01        Legacy CONSOLE_PUTCHAR
//   EID 0x02        Legacy CONSOLE_GETCHAR
//   EID 0x08        Legacy SHUTDOWN
//   EID 0x10        Base extension (spec query)
//   EID 0x54494D45  TIME (sbi_set_timer)
//   EID 0x735049    IPI  (noop for single CPU)
//   EID 0x52464E43  RFENCE (noop for single CPU)
//   EID 0x53525354  SRST (system_reset)
//   EID 0x4442434E  DBCN (debug console write_byte)

use std::sync::Arc;

use machina_guest_riscv::riscv::cpu::RiscvCpu;
use machina_hw_char::uart::Uart16550;
use machina_hw_intc::aclint::Aclint;

// EID and FID constants from sbi-spec; cast to u64 for match
// arms since gpr[] is u64.
const EID_LEGACY_SET_TIMER: u64 = sbi_spec::legacy::LEGACY_SET_TIMER as u64;
const EID_LEGACY_PUTCHAR: u64 = sbi_spec::legacy::LEGACY_CONSOLE_PUTCHAR as u64;
const EID_LEGACY_GETCHAR: u64 = sbi_spec::legacy::LEGACY_CONSOLE_GETCHAR as u64;
const EID_LEGACY_SHUTDOWN: u64 = sbi_spec::legacy::LEGACY_SHUTDOWN as u64;
const EID_BASE: u64 = sbi_spec::base::EID_BASE as u64;
const EID_TIME: u64 = sbi_spec::time::EID_TIME as u64;
const EID_IPI: u64 = sbi_spec::spi::EID_SPI as u64;
const EID_RFENCE: u64 = sbi_spec::rfnc::EID_RFNC as u64;
const EID_SRST: u64 = sbi_spec::srst::EID_SRST as u64;
const EID_DBCN: u64 = sbi_spec::dbcn::EID_DBCN as u64;

const FID_SET_TIMER: u64 = sbi_spec::time::SET_TIMER as u64;
const FID_SYSTEM_RESET: u64 = sbi_spec::srst::SYSTEM_RESET as u64;
const FID_WRITE_BYTE: u64 = sbi_spec::dbcn::CONSOLE_WRITE_BYTE as u64;
const FID_GET_SPEC_VERSION: u64 = sbi_spec::base::GET_SBI_SPEC_VERSION as u64;
const FID_GET_IMPL_ID: u64 = sbi_spec::base::GET_SBI_IMPL_ID as u64;
const FID_GET_IMPL_VERSION: u64 = sbi_spec::base::GET_SBI_IMPL_VERSION as u64;
const FID_PROBE_EXTENSION: u64 = sbi_spec::base::PROBE_EXTENSION as u64;
const FID_GET_MVENDORID: u64 = sbi_spec::base::GET_MVENDORID as u64;
const FID_GET_MARCHID: u64 = sbi_spec::base::GET_MARCHID as u64;
const FID_GET_MIMPID: u64 = sbi_spec::base::GET_MIMPID as u64;

// SBI return codes as i64 (spec uses signed convention).
const SBI_SUCCESS: i64 = sbi_spec::binary::RET_SUCCESS as i64;
const SBI_ERR_NOT_SUPPORTED: i64 =
    sbi_spec::binary::RET_ERR_NOT_SUPPORTED as i64;

// S-mode timer interrupt pending bit (STIP = bit 5).
const STIP: u64 = 1 << 5;

fn is_supported_eid(eid: u64) -> bool {
    matches!(
        eid,
        EID_LEGACY_SET_TIMER
            | EID_LEGACY_PUTCHAR
            | EID_LEGACY_GETCHAR
            | EID_LEGACY_SHUTDOWN
            | EID_BASE
            | EID_TIME
            | EID_IPI
            | EID_RFENCE
            | EID_SRST
            | EID_DBCN
    )
}

/// Host-side SBI backend for -bios builtin mode.
pub struct SbiBackend {
    uart: Arc<Uart16550>,
    aclint: Arc<Aclint>,
    /// Called on SBI system reset/shutdown.
    /// Argument is the SBI reset type
    /// (0 = shutdown, 1 = cold reboot, 2 = warm reboot).
    shutdown_cb: Arc<dyn Fn(u32) + Send + Sync>,
}

impl SbiBackend {
    pub fn new(
        uart: Arc<Uart16550>,
        aclint: Arc<Aclint>,
        shutdown_cb: Arc<dyn Fn(u32) + Send + Sync>,
    ) -> Self {
        Self {
            uart,
            aclint,
            shutdown_cb,
        }
    }

    /// Intercept an S-mode ecall and service it as an SBI
    /// call.
    ///
    /// Reads EID from a7, FID from a6, and arguments from
    /// a0–a5. Writes the SBI return values (error, value)
    /// into a0 and a1, then advances PC past the ecall.
    pub fn handle_call(&self, cpu: &mut RiscvCpu) {
        let eid = cpu.gpr[17];
        let fid = cpu.gpr[16];
        let args = [
            cpu.gpr[10],
            cpu.gpr[11],
            cpu.gpr[12],
            cpu.gpr[13],
            cpu.gpr[14],
            cpu.gpr[15],
        ];
        let (err, val) = self.dispatch(eid, fid, args, cpu);
        cpu.gpr[10] = err as u64;
        cpu.gpr[11] = val as u64;
        cpu.pc = cpu.pc.wrapping_add(4);
    }

    fn dispatch(
        &self,
        eid: u64,
        fid: u64,
        args: [u64; 6],
        cpu: &mut RiscvCpu,
    ) -> (i64, i64) {
        match eid {
            EID_LEGACY_SET_TIMER => {
                self.sbi_set_timer(args[0], &mut cpu.csr.mip);
                (0, 0)
            }
            EID_LEGACY_PUTCHAR => {
                self.sbi_putchar(args[0] as u8);
                (0, 0)
            }
            EID_LEGACY_GETCHAR => (self.sbi_getchar(), 0),
            EID_LEGACY_SHUTDOWN => {
                (self.shutdown_cb)(0);
                (SBI_SUCCESS, 0)
            }
            EID_BASE => self.dispatch_base(fid, args[0]),
            EID_TIME => {
                if fid == FID_SET_TIMER {
                    self.sbi_set_timer(args[0], &mut cpu.csr.mip);
                    (SBI_SUCCESS, 0)
                } else {
                    (SBI_ERR_NOT_SUPPORTED, 0)
                }
            }
            // Single CPU: IPI and RFENCE are no-ops.
            EID_IPI | EID_RFENCE => (SBI_SUCCESS, 0),
            EID_SRST => {
                if fid == FID_SYSTEM_RESET {
                    (self.shutdown_cb)(args[0] as u32);
                    (SBI_SUCCESS, 0)
                } else {
                    (SBI_ERR_NOT_SUPPORTED, 0)
                }
            }
            EID_DBCN => self.dispatch_dbcn(fid, args),
            _ => (SBI_ERR_NOT_SUPPORTED, 0),
        }
    }

    fn dispatch_base(&self, fid: u64, probe_eid: u64) -> (i64, i64) {
        match fid {
            // SBI spec version 2.0 → encoded as 0x0200_0000.
            FID_GET_SPEC_VERSION => (SBI_SUCCESS, 0x0200_0000),
            // Machina implementation ID (unofficial).
            FID_GET_IMPL_ID => (SBI_SUCCESS, 99),
            FID_GET_IMPL_VERSION => (SBI_SUCCESS, 0x0001_0000),
            FID_PROBE_EXTENSION => {
                let present = if is_supported_eid(probe_eid) { 1 } else { 0 };
                (SBI_SUCCESS, present)
            }
            FID_GET_MVENDORID | FID_GET_MARCHID | FID_GET_MIMPID => {
                (SBI_SUCCESS, 0)
            }
            _ => (SBI_ERR_NOT_SUPPORTED, 0),
        }
    }

    fn dispatch_dbcn(&self, fid: u64, args: [u64; 6]) -> (i64, i64) {
        match fid {
            // write_byte: a0 = byte value.
            FID_WRITE_BYTE => {
                self.sbi_putchar(args[0] as u8);
                (SBI_SUCCESS, 0)
            }
            // write / read: not supported in minimal impl.
            _ => (SBI_ERR_NOT_SUPPORTED, 0),
        }
    }

    /// Program mtimecmp and clear the pending S-mode timer
    /// interrupt bit (STIP) from the CPU mip register.
    fn sbi_set_timer(&self, stime_value: u64, mip: &mut u64) {
        // Clear STIP so the kernel's timer handler doesn't
        // see a spurious interrupt from the previous timer.
        *mip &= !STIP;
        // Program hart 0 mtimecmp via ACLINT.
        self.aclint.set_mtimecmp(0, stime_value);
    }

    /// Transmit one byte through the emulated UART.
    fn sbi_putchar(&self, ch: u8) {
        // Write to UART THR (offset 0) bypasses DLAB check
        // because the UART emulation resets DLAB=0 on init.
        self.uart.write(0, ch);
    }

    /// Read one byte from the emulated UART.
    /// Returns the byte value or -1 if no data is available.
    fn sbi_getchar(&self) -> i64 {
        match self.uart.read_rbr_nonblocking() {
            Some(ch) => ch as i64,
            None => -1,
        }
    }
}
