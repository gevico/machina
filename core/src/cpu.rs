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
    /// Check if any interrupt is pending for WFI wakeup
    /// (ignores privilege-level delegation checks).
    fn pending_wfi_wakeup(&self) -> bool {
        self.pending_interrupt()
    }
    fn is_halted(&self) -> bool {
        false
    }
    fn set_halted(&mut self, _halted: bool) {}
    fn privilege_level(&self) -> u8 {
        0
    }
    fn set_pc(&mut self, _pc: u64) {}
    fn handle_interrupt(&mut self) {}
    fn handle_exception(&mut self, _cause: u64, _tval: u64) {}

    /// Check if sfence.vma should trap (TVM=1 in S-mode).
    fn check_sfence_trap(&self) -> bool {
        false
    }
    fn execute_mret(&mut self) {}
    fn execute_sret(&mut self) -> bool {
        true
    }

    /// Set the jmp_env pointer for longjmp from helpers.
    fn set_jmp_env(&mut self, _ptr: u64) {}
    /// Clear the jmp_env pointer.
    fn clear_jmp_env(&mut self) {}
    fn tlb_flush(&mut self) {}
    fn tlb_flush_page(&mut self, _vpn: u64) {}

    /// Handle a privileged CSR instruction at runtime.
    /// The CPU reads the instruction at the current PC,
    /// decodes it, and executes the CSR operation. Returns
    /// true if successful, false if illegal instruction.
    fn handle_priv_csr(&mut self) -> bool {
        false
    }

    /// Check whether any interrupt is pending (regardless
    /// of whether it can be delivered in the current
    /// privilege/enable state). Used to keep goto_tb
    /// chains broken while interrupts wait for delivery.
    fn has_pending_irq(&self) -> bool {
        self.pending_interrupt()
    }

    /// Set the exit-request flag (neg_align = -1) to
    /// break goto_tb chains on the next iteration. Used
    /// when interrupts are pending but not yet deliverable.
    fn set_exit_request(&mut self) {}

    /// Reset the exit-request flag (neg_align) so that
    /// goto_tb chains are not immediately broken. Called
    /// at the start of each exec loop iteration.
    fn reset_exit_request(&mut self) {}

    /// Check whether the execution loop should exit.
    /// Called after each TB to allow external stop.
    fn should_exit(&self) -> bool {
        false
    }

    /// Check monitor pause request. If paused, blocks
    /// until resumed. Returns true if quit requested.
    fn check_monitor_pause(&self) -> bool {
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

    /// Translate a virtual PC to physical PC using the
    /// current page table.  Returns `u64::MAX` if the
    /// physical address is unknown (TLB miss / not
    /// applicable).  The exec loop skips phys_pc
    /// validation when MAX is returned.
    fn translate_pc(&self, _vpc: u64) -> u64 {
        u64::MAX
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

    /// Check if GDB single-step mode is active.
    fn gdb_single_step(&self) -> bool {
        false
    }

    /// Complete a GDB single step (transition to paused).
    fn gdb_complete_step(&self) {}

    /// Check if a GDB breakpoint is set at `pc`.
    /// Returns true if a breakpoint was hit, in which
    /// case the exec loop should skip TB execution and
    /// proceed to the pause/resume check.
    fn gdb_check_breakpoint(&self, _pc: u64) -> bool {
        false
    }

    /// Check if any GDB breakpoint falls within a TB's
    /// address range (tb_pc, tb_pc + tb_size]. Used to
    /// decide whether to replace a cached multi-insn TB
    /// with a 1-insn ephemeral TB for breakpoint
    /// precision.
    fn gdb_breakpoint_in_tb(&self, _tb_pc: u64, _tb_size: u64) -> bool {
        false
    }
}
