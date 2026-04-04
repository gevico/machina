// Shared monitor state for vCPU pause/resume control.
//
// Used by the exec loop (system crate) and monitor
// console (monitor crate) to coordinate vCPU pausing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

/// vCPU execution state as seen by the monitor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VmState {
    Running,
    PauseRequested,
    Paused,
}

/// CPU snapshot stored when paused.
#[derive(Clone, Default)]
pub struct CpuSnapshot {
    pub gpr: [u64; 32],
    pub pc: u64,
    pub priv_level: u8,
    pub halted: bool,
}

/// Shared state between exec loop and monitor.
pub struct MonitorState {
    inner: Mutex<VmState>,
    pause_barrier: Condvar,
    resume_cv: Condvar,
    quit_requested: AtomicBool,
    wfi_waker: Mutex<Option<Arc<crate::wfi::WfiWaker>>>,
    snapshot: Mutex<Option<CpuSnapshot>>,
    /// CpuManager running flag — cleared on quit.
    stop_flag: Mutex<Option<Arc<AtomicBool>>>,
}

impl MonitorState {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(VmState::Running),
            pause_barrier: Condvar::new(),
            resume_cv: Condvar::new(),
            quit_requested: AtomicBool::new(false),
            wfi_waker: Mutex::new(None),
            snapshot: Mutex::new(None),
            stop_flag: Mutex::new(None),
        }
    }

    /// Set the CpuManager stop flag for quit.
    /// Replays latched quit if already requested.
    pub fn set_stop_flag(&self, flag: Arc<AtomicBool>) {
        *self.stop_flag.lock().unwrap() = Some(Arc::clone(&flag));
        // Replay latched quit.
        if self.is_quit_requested() {
            flag.store(false, Ordering::SeqCst);
        }
    }

    /// Store a CPU snapshot (called by exec loop when
    /// parking at pause barrier).
    pub fn store_snapshot(&self, snap: CpuSnapshot) {
        *self.snapshot.lock().unwrap() = Some(snap);
    }

    /// Read the stored CPU snapshot.
    pub fn read_snapshot(&self) -> Option<CpuSnapshot> {
        self.snapshot.lock().unwrap().clone()
    }

    /// Set the WFI waker for CPU wake-on-pause.
    /// Replays latched stop/quit if already pending.
    pub fn set_wfi_waker(&self, wk: Arc<crate::wfi::WfiWaker>) {
        // Check pending state BEFORE locking wfi_waker
        // to maintain lock order: inner -> wfi_waker.
        let needs_wake = self.is_quit_requested() || self.is_pause_requested();
        *self.wfi_waker.lock().unwrap() = Some(Arc::clone(&wk));
        if needs_wake {
            wk.monitor_wake();
        }
    }

    /// Request vCPU to pause. Blocks until the exec
    /// loop confirms it has parked.
    pub fn request_stop(&self) {
        let mut state = self.inner.lock().unwrap();
        if *state == VmState::Paused {
            return;
        }
        *state = VmState::PauseRequested;
        // Only wake WFI if CPU is actually halted.
        // Use cv.notify_all() instead of monitor_wake()
        // to avoid latching a flag that could survive
        // into a future WFI after cont.
        if let Some(ref wk) = *self.wfi_waker.lock().unwrap() {
            wk.monitor_wake();
        }
        // Wait for exec loop to park, or for cancel
        // (cont/quit changed state back to Running).
        while *state == VmState::PauseRequested {
            state = self.pause_barrier.wait(state).unwrap();
        }
    }

    /// Resume from paused state.
    pub fn request_cont(&self) {
        let mut state = self.inner.lock().unwrap();
        if *state == VmState::Running {
            return;
        }
        *state = VmState::Running;
        self.resume_cv.notify_all();
        self.pause_barrier.notify_all();
        // Clear stale monitor_wake so it doesn't affect
        // future WFI instructions.
        if let Some(ref wk) = *self.wfi_waker.lock().unwrap() {
            wk.clear_monitor_wake();
        }
    }

    /// Request clean process exit.
    pub fn request_quit(&self) {
        self.quit_requested.store(true, Ordering::SeqCst);
        // Clear CpuManager running flag so the outer
        // run() loop exits after cpu_exec_loop returns.
        if let Some(ref flag) = *self.stop_flag.lock().unwrap() {
            flag.store(false, Ordering::SeqCst);
        }
        // Resume if paused, so exec loop can exit.
        let mut state = self.inner.lock().unwrap();
        *state = VmState::Running;
        self.resume_cv.notify_all();
        self.pause_barrier.notify_all();
        // Wake WFI if halted.
        if let Some(ref wk) = *self.wfi_waker.lock().unwrap() {
            wk.stop();
        }
    }

    /// Check if quit was requested.
    pub fn is_quit_requested(&self) -> bool {
        self.quit_requested.load(Ordering::SeqCst)
    }

    /// Called by the exec loop at the top of each
    /// iteration. If PauseRequested, parks the vCPU
    /// and blocks until resumed.
    /// Returns true if quit was requested.
    pub fn check_pause(&self) -> bool {
        if self.is_quit_requested() {
            return true;
        }
        let mut state = self.inner.lock().unwrap();
        if *state == VmState::PauseRequested {
            *state = VmState::Paused;
            self.pause_barrier.notify_all();
            // Wait for resume or quit.
            while *state == VmState::Paused {
                state = self.resume_cv.wait(state).unwrap();
            }
        }
        self.is_quit_requested()
    }

    /// Get current VM state.
    pub fn vm_state(&self) -> VmState {
        *self.inner.lock().unwrap()
    }

    /// Check if pause is requested (non-blocking).
    pub fn is_pause_requested(&self) -> bool {
        let s = self.inner.lock().unwrap();
        *s == VmState::PauseRequested || *s == VmState::Paused
    }
}

impl Default for MonitorState {
    fn default() -> Self {
        Self::new()
    }
}
