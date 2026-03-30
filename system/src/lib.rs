// CpuManager: orchestrates vCPU execution threads.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use machina_core::machine::Machine;

pub struct CpuManager {
    cpu_count: usize,
    running: Arc<AtomicBool>,
}

impl CpuManager {
    pub fn new(cpu_count: usize) -> Self {
        Self {
            cpu_count,
            running: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Run the execution loop on the machine.
    ///
    /// For now this is a placeholder that prints status
    /// and returns immediately.  Will be replaced with
    /// real cpu_exec_loop integration.
    pub fn run(
        &self,
        _machine: &mut dyn Machine,
    ) -> Result<(), Box<dyn std::error::Error>> {
        eprintln!("CpuManager: {} vCPU(s), starting execution", self.cpu_count);
        // TODO: integrate with accel::exec::cpu_exec_loop
        Ok(())
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_manager_new() {
        let mgr = CpuManager::new(4);
        assert_eq!(mgr.cpu_count, 4);
        assert!(mgr.is_running());
    }

    #[test]
    fn test_cpu_manager_stop() {
        let mgr = CpuManager::new(1);
        assert!(mgr.is_running());
        mgr.stop();
        assert!(!mgr.is_running());
    }
}
