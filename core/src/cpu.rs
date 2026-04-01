//! Architecture-agnostic guest CPU trait.
//!
//! Every guest architecture (RISC-V, ARM, x86, ...) implements
//! this trait to expose its CPU state to the execution engine.

/// Trait for guest CPU state used by the execution loop.
pub trait GuestCpu {
    /// IR translation context type provided by the backend.
    type IrContext;

    // -- Basic methods (required) --

    fn get_pc(&self) -> u64;
    fn get_flags(&self) -> u32;
    fn gen_code(
        &mut self,
        ir: &mut Self::IrContext,
        pc: u64,
        max_insns: u32,
    ) -> u32;
    fn env_ptr(&mut self) -> *mut u8;

    // -- Full-system methods (default no-op) --

    fn pending_interrupt(&self) -> bool {
        false
    }
    fn is_halted(&self) -> bool {
        false
    }
    fn set_halted(&mut self, _halted: bool) {}
    fn privilege_level(&self) -> u8 {
        0
    }
    fn handle_interrupt(&mut self) {}
    fn handle_exception(&mut self, _excp: u32, _tval: u64) {}
    fn execute_mret(&mut self) {}
    fn execute_sret(&mut self) -> bool { true }
    fn tlb_flush(&mut self) {}
    fn tlb_flush_page(&mut self, _vpn: u64) {}

    /// Handle a privileged CSR instruction at runtime.
    /// The CPU reads the instruction at the current PC,
    /// decodes it, and executes the CSR operation. Returns
    /// true if successful, false if illegal instruction.
    fn handle_priv_csr(&mut self) -> bool {
        false
    }

    /// Check whether the execution loop should exit.
    /// Called after each TB to allow external stop.
    fn should_exit(&self) -> bool {
        false
    }

    /// Check and deliver any pending memory fault latched
    /// by JIT helpers. Returns true if a fault was handled.
    fn check_mem_fault(&mut self) -> bool {
        false
    }

    /// Returns true if all TBs should be invalidated
    /// (e.g. after satp write). Clears the flag.
    fn take_tb_flush_pending(&mut self) -> bool {
        false
    }

    /// Returns the physical PC from the last gen_code()
    /// call (for TB phys_pc recording).
    fn last_phys_pc(&self) -> u64 {
        0
    }

    /// Take the set of dirty physical pages (for
    /// fence.i page-granularity TB invalidation).
    fn take_dirty_pages(&mut self) -> Vec<u64> {
        Vec::new()
    }

    /// Wait for an interrupt to arrive (WFI semantics).
    /// Returns true if an interrupt arrived, false if
    /// timed out or not implemented.
    fn wait_for_interrupt(&self) -> bool {
        false
    }

    // -- GDB support (default no-op) --

    fn gdb_read_registers(&self, _buf: &mut [u8]) -> usize {
        0
    }
    fn gdb_write_registers(&mut self, _buf: &[u8]) -> usize {
        0
    }
    fn gdb_read_register(&self, _reg: usize, _buf: &mut [u8]) -> usize {
        0
    }
    fn gdb_write_register(&mut self, _reg: usize, _buf: &[u8]) -> usize {
        0
    }
}
