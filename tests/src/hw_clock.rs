use std::sync::{Arc, Mutex};

use machina_hw_core::clock::DeviceClock;

#[test]
fn test_clock_period() {
    // 1 GHz clock -> 1 ns period.
    let clk = DeviceClock::new(1_000_000_000);
    assert_eq!(clk.period_ns(), 1);
}

#[test]
fn test_clock_disabled() {
    let mut clk = DeviceClock::new(100_000_000);
    clk.set_enabled(false);
    assert!(!clk.enabled());
    // Frequency is still reported even when disabled.
    assert_eq!(clk.freq_hz(), 100_000_000);
}

#[test]
fn test_clock_zero_freq() {
    let clk = DeviceClock::new(0);
    assert_eq!(clk.period_ns(), 0);
}

// -- Propagation tests --

#[test]
fn test_clock_propagation() {
    let mut parent = DeviceClock::new(100_000_000);
    let child = Arc::new(Mutex::new(DeviceClock::new(0)));
    parent.add_child(&child);

    // Propagate 100 MHz to child (1:1 ratio).
    parent.propagate();
    assert_eq!(child.lock().unwrap().freq_hz(), 100_000_000);

    // Change parent and propagate again.
    parent.set_freq_and_propagate(200_000_000);
    assert_eq!(child.lock().unwrap().freq_hz(), 200_000_000);
}

#[test]
fn test_clock_multiplier() {
    let mut parent = DeviceClock::new(50_000_000);
    let child = Arc::new(Mutex::new(DeviceClock::new(0)));
    child.lock().unwrap().multiplier = 2;

    parent.add_child(&child);
    parent.propagate();

    // Child should get 2x the parent frequency.
    assert_eq!(child.lock().unwrap().freq_hz(), 100_000_000);
}

#[test]
fn test_clock_dead_child() {
    let mut parent = DeviceClock::new(100_000_000);
    {
        let child = Arc::new(Mutex::new(DeviceClock::new(0)));
        parent.add_child(&child);
        // child dropped here
    }
    // Propagating with a dead weak ref must not panic.
    parent.propagate();
}
