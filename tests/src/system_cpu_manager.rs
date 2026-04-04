use std::sync::Arc;

use machina_core::wfi::WfiWaker;
use machina_system::CpuManager;

#[test]
fn test_cpu_manager_new() {
    let mgr = CpuManager::new();
    assert!(mgr.is_running());
}

#[test]
fn test_cpu_manager_stop() {
    let mgr = CpuManager::new();
    assert!(mgr.is_running());
    mgr.stop();
    assert!(!mgr.is_running());
}

#[test]
fn test_cpu_manager_stop_with_waker() {
    let wk = Arc::new(WfiWaker::new());
    let mut mgr = CpuManager::new();
    mgr.set_wfi_waker(wk.clone());
    mgr.stop();
    assert!(!mgr.is_running());
    assert!(!wk.wait());
}

#[test]
fn test_wfi_stop_unblocks_concurrent_wait() {
    let wk = Arc::new(WfiWaker::new());
    let wk2 = wk.clone();
    let handle = std::thread::spawn(move || wk2.wait());
    std::thread::sleep(std::time::Duration::from_millis(50));
    wk.stop();
    let result = handle.join().unwrap();
    assert!(!result, "wait() must return false when stopped");
}

#[test]
fn test_wfi_timer_only_wakeup() {
    use std::time::{Duration, Instant};
    let wk = Arc::new(WfiWaker::new());
    let wk2 = wk.clone();
    let t0 = Instant::now();
    wk.set_deadline(t0 + Duration::from_millis(20));
    let handle = std::thread::spawn(move || wk2.wait());
    let result = handle.join().unwrap();
    let elapsed = t0.elapsed();
    assert!(result, "wait() must return true on timer wakeup");
    assert!(
        elapsed < Duration::from_millis(80),
        "timer wakeup took {:?}, expected < 80ms",
        elapsed
    );
}

#[test]
fn test_wfi_irq_preempts_timer() {
    use std::time::{Duration, Instant};
    let wk = Arc::new(WfiWaker::new());
    let wk2 = wk.clone();
    let t0 = Instant::now();
    wk.set_deadline(t0 + Duration::from_secs(1));
    let handle = std::thread::spawn(move || wk2.wait());
    std::thread::sleep(Duration::from_millis(20));
    wk.wake();
    let result = handle.join().unwrap();
    let elapsed = t0.elapsed();
    assert!(result, "wait() must return true on IRQ preempt");
    assert!(
        elapsed < Duration::from_millis(200),
        "IRQ preempt took {:?}, expected < 200ms",
        elapsed
    );
}
