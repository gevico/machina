use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use machina_hw_core::irq::{IrqLine, IrqSink, OrIrq, SplitIrq};

/// Simple test sink that records the last level and
/// the number of set_irq calls.
struct TestSink {
    level: AtomicBool,
    call_count: AtomicUsize,
}

impl TestSink {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            level: AtomicBool::new(false),
            call_count: AtomicUsize::new(0),
        })
    }

    fn level(&self) -> bool {
        self.level.load(Ordering::SeqCst)
    }

    fn calls(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

impl IrqSink for TestSink {
    fn set_irq(&self, _irq: u32, level: bool) {
        self.level.store(level, Ordering::SeqCst);
        self.call_count.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn test_irq_set_clear() {
    let sink = TestSink::new();
    let line = IrqLine::new(sink.clone(), 0);

    line.raise();
    assert!(sink.level());

    line.lower();
    assert!(!sink.level());
}

#[test]
fn test_or_irq() {
    let sink = TestSink::new();
    let output = IrqLine::new(sink.clone(), 0);
    let or_gate = Arc::new(OrIrq::new(output, 3));

    // Raise input 0 -> output high.
    or_gate.set_irq(0, true);
    assert!(sink.level());

    // Raise input 1 -> still high.
    or_gate.set_irq(1, true);
    assert!(sink.level());

    // Lower input 0 -> still high (input 1 holds).
    or_gate.set_irq(0, false);
    assert!(sink.level());

    // Lower input 1 -> all low, output low.
    or_gate.set_irq(1, false);
    assert!(!sink.level());
}

#[test]
fn test_or_irq_all_low() {
    let sink = TestSink::new();
    let output = IrqLine::new(sink.clone(), 0);
    let or_gate = Arc::new(OrIrq::new(output, 4));

    // All inputs default low; output must be low.
    or_gate.set_irq(0, false);
    assert!(!sink.level());

    or_gate.set_irq(1, false);
    assert!(!sink.level());

    or_gate.set_irq(2, false);
    assert!(!sink.level());

    or_gate.set_irq(3, false);
    assert!(!sink.level());
}

#[test]
fn test_split_irq() {
    let s0 = TestSink::new();
    let s1 = TestSink::new();
    let s2 = TestSink::new();

    let outputs = vec![
        IrqLine::new(s0.clone(), 0),
        IrqLine::new(s1.clone(), 1),
        IrqLine::new(s2.clone(), 2),
    ];
    let split = Arc::new(SplitIrq::new(outputs));

    // Raise -> all outputs high.
    split.set_irq(0, true);
    assert!(s0.level());
    assert!(s1.level());
    assert!(s2.level());

    // Lower -> all outputs low.
    split.set_irq(0, false);
    assert!(!s0.level());
    assert!(!s1.level());
    assert!(!s2.level());
}

#[test]
fn test_irq_no_spurious() {
    let sink = TestSink::new();
    let line = IrqLine::new(sink.clone(), 0);

    // Line starts low. Lowering it again should not
    // change the level.
    assert!(!sink.level());
    let before = sink.calls();
    line.lower();
    assert!(!sink.level());

    // The sink was called, but level stays false.
    assert_eq!(sink.calls(), before + 1);
    assert!(!sink.level());
}
