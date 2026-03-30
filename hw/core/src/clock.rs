// Device clock model.

/// A simple device clock with configurable frequency.
pub struct DeviceClock {
    freq_hz: u64,
    enabled: bool,
}

impl DeviceClock {
    pub fn new(freq_hz: u64) -> Self {
        Self {
            freq_hz,
            enabled: true,
        }
    }

    /// Current frequency in Hz.
    pub fn freq_hz(&self) -> u64 {
        self.freq_hz
    }

    /// Change the frequency.
    pub fn set_freq(&mut self, freq_hz: u64) {
        self.freq_hz = freq_hz;
    }

    /// Whether the clock is enabled.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Enable or disable the clock.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Period in nanoseconds (integer division).
    /// Returns 0 when `freq_hz` is 0.
    pub fn period_ns(&self) -> u64 {
        if self.freq_hz == 0 {
            return 0;
        }
        1_000_000_000 / self.freq_hz
    }
}
