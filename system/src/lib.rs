// machina-system: CPU management and GuestCpu bridge.

pub mod builtin;
pub mod cpus;
pub mod gdb;
pub mod gdb_csr;

pub use builtin::FirmwareCallFn;
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
    /// Optional firmware call handler for builtin mode.
    /// When set, S-mode ecalls are dispatched here instead
    /// of being delivered as CPU trap exceptions.
    firmware_handler: Option<FirmwareCallFn>,
}

impl CpuManager {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(true)),
            wfi_waker: None,
            cpus: Vec::new(),
            firmware_handler: None,
        }
    }

    /// Install a firmware call handler for builtin mode.
    /// S-mode ecalls will be dispatched to this handler
    /// instead of being raised as exceptions.
    pub fn set_firmware_handler(&mut self, handler: FirmwareCallFn) {
        self.firmware_handler = Some(handler);
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

    /// Access a managed CPU by index.
    pub fn cpu(&self, idx: usize) -> &FullSystemCpu {
        &self.cpus[idx]
    }

    /// Access a managed CPU mutably by index.
    pub fn cpu_mut(&mut self, idx: usize) -> &mut FullSystemCpu {
        &mut self.cpus[idx]
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
    /// When GDB is active, after the exec loop pauses,
    /// this method hands control to the GDB server to
    /// process commands synchronously on this thread.
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
            if !running.load(Ordering::SeqCst) {
                return ExitReason::Halted;
            }
            match r {
                ExitReason::Halted => {}
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
                    // In builtin mode, S-mode ecalls (priv 1)
                    // are dispatched to the host firmware
                    // handler instead of being raised as traps.
                    if priv_level == 1 {
                        if let Some(ref fw) = self.firmware_handler {
                            fw(&mut cpu.cpu);
                            continue;
                        }
                    }
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
