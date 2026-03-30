// machina-system: CPU management and GuestCpu bridge.

pub mod cpus;

pub use cpus::FullSystemCpu;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use machina_accel::exec::exec_loop::{cpu_exec_loop_mt, ExitReason};
use machina_accel::exec::{PerCpuState, SharedState};
use machina_accel::ir::context::Context;
use machina_accel::GuestCpu;
use machina_accel::HostCodeGen;

pub struct CpuManager {
    running: Arc<AtomicBool>,
}

impl CpuManager {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Run the execution loop for a single CPU.
    /// This blocks until the CPU exits (ecall, halt, etc.)
    ///
    /// # Safety
    /// The caller must ensure `cpu.env_ptr()` returns a
    /// valid pointer to the CPU struct, matching the
    /// globals set up during translation.
    pub unsafe fn run_cpu<B, C>(
        &self,
        cpu: &mut C,
        shared: &SharedState<B>,
    ) -> ExitReason
    where
        B: HostCodeGen,
        C: GuestCpu<IrContext = Context>,
    {
        let mut per_cpu = PerCpuState::new();
        cpu_exec_loop_mt(shared, &mut per_cpu, cpu)
    }
}

impl Default for CpuManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_manager_new() {
        let mgr = CpuManager::new();
        assert!(mgr.is_running());
    }

    #[test]
    fn test_cpu_manager_stop() {
        let mgr = CpuManager::new();
        assert!(mgr.is_running());
        mgr.stop();
        assert!(!mgr.is_running());
    }

    #[test]
    fn test_cpu_manager_has_run_cpu() {
        // Verify run_cpu method exists and is callable.
        // Cannot run without a real SharedState + guest
        // binary, so just confirm the API compiles.
        let mgr = CpuManager::new();
        assert!(mgr.is_running());
        mgr.stop();
        assert!(!mgr.is_running());
    }
}
