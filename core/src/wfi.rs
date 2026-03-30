// WFI wakeup primitive: Condvar-based notification for
// halted CPU wakeup by device IRQ delivery.

use std::sync::{Condvar, Mutex};

/// Wakeup signal for WFI (Wait For Interrupt).
///
/// Device IRQ sinks call `wake()` after updating SharedMip
/// to unblock a CPU waiting in `wait_for_interrupt()`.
pub struct WfiWaker {
    mu: Mutex<bool>,
    cv: Condvar,
}

impl WfiWaker {
    pub fn new() -> Self {
        Self {
            mu: Mutex::new(false),
            cv: Condvar::new(),
        }
    }

    /// Wake a halted CPU. Called by device IRQ sinks.
    pub fn wake(&self) {
        let mut notified = self.mu.lock().unwrap();
        *notified = true;
        self.cv.notify_all();
    }

    /// Block until woken by `wake()`. Returns true when
    /// woken by signal. Does not timeout — blocks
    /// indefinitely until an interrupt arrives.
    pub fn wait(&self) -> bool {
        let mut notified = self.mu.lock().unwrap();
        while !*notified {
            notified = self.cv.wait(notified).unwrap();
        }
        *notified = false;
        true
    }

    /// Block with timeout. Returns true if woken by
    /// signal, false on timeout.
    pub fn wait_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> bool {
        let mut notified = self.mu.lock().unwrap();
        if *notified {
            *notified = false;
            return true;
        }
        let (guard, _result) = self
            .cv
            .wait_timeout(notified, timeout)
            .unwrap();
        let woken = *guard;
        drop(guard);
        if woken {
            let mut n = self.mu.lock().unwrap();
            *n = false;
        }
        woken
    }
}

impl Default for WfiWaker {
    fn default() -> Self {
        Self::new()
    }
}
