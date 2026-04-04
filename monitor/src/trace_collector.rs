//! Per-vCPU trace event collector.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use machina_core::trace::{EventFilter, TraceEvent, TraceSink};

use crate::symbol_table::SymbolTable;

// ---------------------------------------------------------------------------
// TraceCollector
// ---------------------------------------------------------------------------

const RING_BUFFER_CAPACITY: usize = 8192;

/// Per-vCPU ring buffer that collects trace events.
///
/// Each vCPU thread owns its own `TraceCollector`.  The
/// `SymbolTable` is Arc-shared (read-only after init).
pub struct TraceCollector {
    buf: Mutex<VecDeque<(u64, TraceEvent)>>,
    enabled: AtomicBool,
    filter: Mutex<EventFilter>,
    symbols: Arc<SymbolTable>,
    seq: AtomicU64,
}

impl TraceCollector {
    pub fn new(symbols: Arc<SymbolTable>) -> Self {
        Self {
            buf: Mutex::new(VecDeque::with_capacity(RING_BUFFER_CAPACITY)),
            enabled: AtomicBool::new(false),
            filter: Mutex::new(EventFilter::none()),
            symbols,
            seq: AtomicU64::new(0),
        }
    }

    // -- control --

    pub fn enable(&self) {
        self.enabled.store(true, Ordering::Release);
    }

    pub fn disable(&self) {
        self.enabled.store(false, Ordering::Release);
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_filter(&self, f: EventFilter) {
        *self.filter.lock().unwrap() = f;
    }

    pub fn filter(&self) -> EventFilter {
        *self.filter.lock().unwrap()
    }

    // -- produce --

    /// Fast-path check: is tracing enabled *and* does the filter
    /// accept this category?  Callers should check this before
    /// constructing expensive events.
    pub fn should_collect(
        &self,
        category: machina_core::trace::TraceCategory,
    ) -> bool {
        self.enabled.load(Ordering::Relaxed)
            && self.filter.lock().unwrap().matches(category)
    }

    /// Push an event into the ring buffer.  Caller is responsible
    /// for checking `should_collect()` first.
    pub fn push(&self, event: TraceEvent) {
        let n = self.seq.fetch_add(1, Ordering::Relaxed);
        let mut buf = self.buf.lock().unwrap();
        if buf.len() >= RING_BUFFER_CAPACITY {
            buf.pop_front();
        }
        buf.push_back((n, event));
    }

    // -- consume --

    /// Drain and return all buffered events (with seq numbers).
    pub fn drain(&self) -> Vec<(u64, TraceEvent)> {
        let mut buf = self.buf.lock().unwrap();
        buf.drain(..).collect()
    }

    /// Number of events currently in the buffer.
    pub fn len(&self) -> usize {
        self.buf.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Monotonic sequence counter value.
    pub fn seq(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }

    // -- symbol helpers --

    pub fn lookup_symbol(&self, pc: u64) -> Option<String> {
        self.symbols.lookup(pc).map(|s| s.to_owned())
    }

    pub fn in_switch_range(&self, pc: u64) -> bool {
        self.symbols.in_switch_range(pc)
    }

    pub fn symbols(&self) -> &Arc<SymbolTable> {
        &self.symbols
    }
}

// ---------------------------------------------------------------------------
// Drain helper
// ---------------------------------------------------------------------------

/// Merge-sort events from multiple per-vCPU collectors by sequence
/// number and emit through a sink.
pub fn drain_and_emit(
    collectors: &[Arc<TraceCollector>],
    sink: &mut dyn TraceSink,
) {
    let mut all: Vec<(u64, TraceEvent)> = Vec::new();
    for tc in collectors {
        all.extend(tc.drain());
    }
    all.sort_by_key(|(seq, _)| *seq);
    for (seq, event) in &all {
        sink.emit(*seq, event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use machina_core::trace::TraceCategory;

    fn dummy_collector() -> TraceCollector {
        TraceCollector::new(Arc::new(SymbolTable::default()))
    }

    #[test]
    fn push_and_drain() {
        let tc = dummy_collector();
        tc.enable();
        let evt = TraceEvent::TlbFlush {
            pc: 0x1000,
            fn_name: None,
        };
        tc.push(evt.clone());
        assert_eq!(tc.len(), 1);
        let events = tc.drain();
        assert_eq!(events.len(), 1);
        assert_eq!(tc.len(), 0);
    }

    #[test]
    fn disabled_push_is_noop() {
        let tc = dummy_collector();
        // Not enabled -- should_collect returns false.
        assert!(!tc.should_collect(TraceCategory::Vm));
    }

    #[test]
    fn ring_overflow() {
        let tc = dummy_collector();
        tc.enable();
        tc.set_filter(EventFilter::all());
        for i in 0..9000 {
            tc.push(TraceEvent::TlbFlush {
                pc: i,
                fn_name: None,
            });
        }
        assert!(tc.len() <= 8192);
    }
}
