//! Minimal test-completion device for the loongarch64-ref machine.
//!
//! LoongArch full-system emulation has no architectural "test finished"
//! signal: a bare-metal ISA test cannot report pass/fail by halting,
//! because `idle` parks the CPU waiting for an interrupt rather than
//! terminating the machine. This device is the LoongArch analog of the
//! RISC-V SiFive Test finisher (`hw/riscv/src/sifive_test.rs`): a guest
//! writes a magic word to a single MMIO register to terminate the
//! machine with a pass/fail result.
//!
//!   write 0x5555 -> PASS  (machina exits 0)
//!   write 0x3333 -> FAIL  (machina exits non-zero)
//!   read          -> 0    (no side effects)
//!
//! The machine installs the actual termination callback at run time
//! (see `LoongArchVirtMachine::install_finish_handler`), so this device
//! stays free of any execution-loop dependency.

use std::sync::{Arc, Mutex};

use machina_memory::region::MmioOps;

/// MMIO write value that ends the run with a passing result.
pub const TEST_FINISHER_PASS: u32 = 0x5555;
/// MMIO write value that ends the run with a failing result.
pub const TEST_FINISHER_FAIL: u32 = 0x3333;

type FinishFn = Mutex<Option<Box<dyn Fn(bool) + Send>>>;

/// Test-completion device. `on_finish` receives `true` on pass.
pub struct LoongArchTestFinisher {
    on_finish: FinishFn,
}

impl LoongArchTestFinisher {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            on_finish: Mutex::new(None),
        })
    }

    /// Install the callback invoked when the guest writes a magic word.
    pub fn set_finish_handler(&self, handler: Box<dyn Fn(bool) + Send>) {
        *self.on_finish.lock().unwrap() = Some(handler);
    }

    fn finish(&self, pass: bool) {
        if let Some(handler) = self.on_finish.lock().unwrap().as_ref() {
            handler(pass);
        }
    }
}

/// MMIO adapter mapping the finisher into the address space.
pub struct LoongArchTestFinisherMmio(pub Arc<LoongArchTestFinisher>);

impl MmioOps for LoongArchTestFinisherMmio {
    fn read(&self, _offset: u64, _size: u32) -> u64 {
        0
    }

    fn write(&self, offset: u64, size: u32, val: u64) {
        // The finisher is a single 32-bit register at offset 0. Ignore
        // any other offset or access width so a stray write elsewhere in
        // the window cannot be misread as a pass/fail result.
        if offset != 0 || size != 4 {
            return;
        }
        match val as u32 {
            TEST_FINISHER_PASS => self.0.finish(true),
            TEST_FINISHER_FAIL => self.0.finish(false),
            _ => {}
        }
    }
}
