use machina_hw_core::clock::DeviceClock;

#[test]
fn test_clock_period() {
    // 1 GHz clock → 1 ns period.
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
