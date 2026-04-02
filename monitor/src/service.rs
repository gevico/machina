// MonitorService: shared backend for MMP and HMP.

use std::sync::Arc;

use machina_core::monitor::{
    CpuSnapshot, MonitorState, VmState,
};

/// Central monitor service shared by all transports.
pub struct MonitorService {
    pub state: Arc<MonitorState>,
}

impl MonitorService {
    pub fn new(state: Arc<MonitorState>) -> Self {
        Self { state }
    }

    pub fn query_status(&self) -> bool {
        // Only report paused when actually parked.
        let s = self.state.vm_state();
        s == VmState::Running
            || s == VmState::PauseRequested
    }

    pub fn stop(&self) {
        self.state.request_stop();
    }

    pub fn cont(&self) {
        self.state.request_cont();
    }

    pub fn quit(&self) {
        self.state.request_quit();
    }

    pub fn query_cpus(&self) -> Vec<CpuInfo> {
        let running = self.query_status();
        let snap = self.state.read_snapshot();
        vec![CpuInfo {
            cpu_index: 0,
            // PC is only accurate when paused.
            pc: if running {
                0
            } else {
                snap.as_ref()
                    .map(|s| s.pc)
                    .unwrap_or(0)
            },
            halted: if running {
                false
            } else {
                snap.as_ref()
                    .map(|s| s.halted)
                    .unwrap_or(false)
            },
            arch: "riscv64".to_string(),
        }]
    }

    pub fn take_snapshot(
        &self,
    ) -> Option<CpuSnapshot> {
        self.state.read_snapshot()
    }
}

pub struct CpuInfo {
    pub cpu_index: u32,
    pub pc: u64,
    pub halted: bool,
    pub arch: String,
}
