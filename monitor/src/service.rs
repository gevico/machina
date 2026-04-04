// MonitorService: shared backend for MMP and HMP.

use std::sync::Arc;

use machina_core::monitor::{
    CpuSnapshot, MonitorState, VmState,
};
use machina_core::trace::EventFilter;

use crate::symbol_table::SymbolTable;
use crate::terminal_sink::TerminalSink;
use crate::trace_collector::TraceCollector;

/// Central monitor service shared by all transports.
pub struct MonitorService {
    pub state: Arc<MonitorState>,

    // -- Trace state --
    trace_collectors: Vec<Arc<TraceCollector>>,
    symbol_table: Option<Arc<SymbolTable>>,
    trace_enabled: bool,
}

impl MonitorService {
    pub fn new(state: Arc<MonitorState>) -> Self {
        Self {
            state,
            trace_collectors: Vec::new(),
            symbol_table: None,
            trace_enabled: false,
        }
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

    // -- Trace management --

    /// Attach per-vCPU trace collectors.
    pub fn set_trace_collectors(
        &mut self,
        collectors: Vec<Arc<TraceCollector>>,
    ) {
        self.trace_collectors = collectors;
    }

    /// Attach symbol table (parsed from kernel ELF).
    pub fn set_symbol_table(
        &mut self,
        st: Arc<SymbolTable>,
    ) {
        self.symbol_table = Some(st);
    }

    /// Start tracing with an optional filter string.
    pub fn trace_start(
        &mut self,
        filter_str: &str,
    ) -> Result<(), String> {
        let filter = EventFilter::from_str(filter_str)?;
        for tc in &self.trace_collectors {
            tc.set_filter(filter);
            tc.enable();
        }
        self.trace_enabled = true;
        // Drain buffered events in a background thread.
        // For now, drain synchronously on each status
        // query. A proper drain thread can be added later.
        Ok(())
    }

    /// Stop tracing.
    pub fn trace_stop(&mut self) -> u64 {
        let mut total: u64 = 0;
        for tc in &self.trace_collectors {
            tc.disable();
            total += tc.seq();
        }
        self.trace_enabled = false;
        total
    }

    /// Drain all collectors and emit through terminal sink.
    pub fn trace_drain(&mut self) {
        if !self.trace_enabled {
            return;
        }
        let mut sink = TerminalSink::new();
        crate::trace_collector::drain_and_emit(
            &self.trace_collectors,
            &mut sink,
        );
    }

    /// Query trace status.
    pub fn trace_status(&self) -> (bool, u64) {
        let total: u64 = self
            .trace_collectors
            .iter()
            .map(|tc| tc.seq())
            .sum();
        (self.trace_enabled, total)
    }

    /// Acknowledge trace status query from HMP.
    pub fn acknowledge_trace_status(&self) -> String {
        let (enabled, count) = self.trace_status();
        format!(
            "trace: {} ({} events collected)\n",
            if enabled { "enabled" } else { "disabled" },
            count,
        )
    }
}

pub struct CpuInfo {
    pub cpu_index: u32,
    pub pc: u64,
    pub halted: bool,
    pub arch: String,
}
