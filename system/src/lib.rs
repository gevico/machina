// machina-system: CPU management and GuestCpu bridge.

pub mod cpus;

pub use cpus::FullSystemCpu;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use machina_accel::exec::exec_loop::{cpu_exec_loop, ExitReason};
use machina_accel::exec::{PerCpuState, SharedState};
use machina_accel::GuestCpu;
use machina_accel::HostCodeGen;
use machina_core::wfi::WfiWaker;

pub struct CpuManager {
    running: Arc<AtomicBool>,
    wfi_waker: Option<Arc<WfiWaker>>,
    cpus: Vec<FullSystemCpu>,
}

impl CpuManager {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(true)),
            wfi_waker: None,
            cpus: Vec::new(),
        }
    }

    pub fn set_wfi_waker(&mut self, wk: Arc<WfiWaker>) {
        self.wfi_waker = Some(wk);
    }

    /// Get a clone of the running flag for external stop.
    pub fn running_flag(&self) -> Arc<AtomicBool> {
        self.running.clone()
    }

    /// Add a CPU to be managed. Ownership is transferred.
    pub fn add_cpu(&mut self, cpu: FullSystemCpu) {
        self.cpus.push(cpu);
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(ref wk) = self.wfi_waker {
            wk.stop();
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Run all owned CPUs. For single-CPU, runs on the
    /// current thread. Blocks until execution exits.
    ///
    /// # Safety
    /// Each CPU's `env_ptr()` must return a valid pointer
    /// to its RiscvCpu struct matching translation globals.
    pub unsafe fn run<B>(&mut self, shared: &SharedState<B>) -> ExitReason
    where
        B: HostCodeGen,
    {
        if self.cpus.is_empty() {
            return ExitReason::Exit(0);
        }
        let running = Arc::clone(&self.running);
        let cpu = &mut self.cpus[0];
        let mut per_cpu = PerCpuState::new();
        loop {
            let r = cpu_exec_loop(shared, &mut per_cpu, cpu);
            cpu.drain_trace_output();
            match r {
                ExitReason::Halted => {
                    if !running.load(Ordering::SeqCst) {
                        return r;
                    }
                }
                ExitReason::BufferFull => {
                    let _guard = shared.translate_lock.lock().unwrap();
                    shared
                        .tb_store
                        .invalidate_all(shared.code_buf(), &shared.backend);
                    shared.tb_store.flush();
                    shared.code_buf_mut().set_offset(shared.code_gen_start);
                    per_cpu.jump_cache.invalidate();
                }
                ExitReason::Ecall { priv_level } => {
                    // Route ECALL as trap exception.
                    // 8=EcallFromU, 9=EcallFromS,
                    // 11=EcallFromM.
                    let cause = match priv_level {
                        0 => 8,
                        1 => 9,
                        3 => 11,
                        _ => 11,
                    };
                    cpu.handle_exception(cause, 0);
                    cpu.trace_on_ecall_trap(priv_level);
                }
                other => return other,
            }
        }
    }
}

impl Default for CpuManager {
    fn default() -> Self {
        Self::new()
    }
}
