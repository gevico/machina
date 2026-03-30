// machina-system: CPU management and GuestCpu bridge.

pub mod cpus;

pub use cpus::FullSystemCpu;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use machina_accel::exec::exec_loop::{vcpu_exec_loop, ExitReason};
use machina_accel::exec::{PerVcpuState, SharedState};
use machina_accel::ir::context::Context;
use machina_accel::GuestCpu;
use machina_accel::HostCodeGen;
use machina_core::wfi::WfiWaker;

pub struct CpuManager {
    running: Arc<AtomicBool>,
    wfi_waker: Option<Arc<WfiWaker>>,
}

impl CpuManager {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(true)),
            wfi_waker: None,
        }
    }

    /// Set the WFI waker so stop() can break WFI wait.
    pub fn set_wfi_waker(&mut self, wk: Arc<WfiWaker>) {
        self.wfi_waker = Some(wk);
    }

    /// Stop execution: flip running flag and wake any
    /// CPU halted in WFI.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(ref wk) = self.wfi_waker {
            wk.stop();
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Run the execution loop for a single CPU.
    /// Blocks until the CPU exits.
    ///
    /// # Safety
    /// `cpu.env_ptr()` must return a valid pointer to
    /// the CPU struct matching translation globals.
    pub unsafe fn run_cpu<B, C>(
        &self,
        cpu: &mut C,
        shared: &SharedState<B>,
    ) -> ExitReason
    where
        B: HostCodeGen,
        C: GuestCpu<IrContext = Context>,
    {
        let mut per_vcpu = PerVcpuState::new();
        vcpu_exec_loop(shared, &mut per_vcpu, cpu)
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
    fn test_cpu_manager_stop_with_waker() {
        let wk = Arc::new(WfiWaker::new());
        let mut mgr = CpuManager::new();
        mgr.set_wfi_waker(wk.clone());
        mgr.stop();
        assert!(!mgr.is_running());
        // Waker should unblock any waiting thread.
        assert!(!wk.wait()); // returns false (stopped)
    }

    #[test]
    fn test_wfi_stop_unblocks_concurrent_wait() {
        // Verify stop() can unblock a thread that is
        // already blocked in wait().
        let wk = Arc::new(WfiWaker::new());
        let wk2 = wk.clone();
        let handle = std::thread::spawn(move || {
            // This will block until stop() is called.
            wk2.wait()
        });
        // Give the thread time to enter wait().
        std::thread::sleep(std::time::Duration::from_millis(50));
        wk.stop();
        let result = handle.join().unwrap();
        assert!(!result, "wait() must return false when stopped");
    }
}
