use machina_accel::timer::{ClockType, VirtualClock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[test]
fn test_virtual_clock_step() {
    let clock = VirtualClock::new(ClockType::Virtual);
    assert_eq!(clock.get_ns(), 0);
    clock.step(1_000_000);
    assert_eq!(clock.get_ns(), 1_000_000);
}

#[test]
fn test_timer_expiry() {
    let clock = VirtualClock::new(ClockType::Virtual);
    let fired = Arc::new(AtomicBool::new(false));
    let fired_clone = fired.clone();
    clock.add_timer(500_000, move || {
        fired_clone.store(true, Ordering::SeqCst);
    });
    clock.step(499_999);
    assert!(!fired.load(Ordering::SeqCst));
    clock.step(1);
    assert!(fired.load(Ordering::SeqCst));
}

#[test]
fn test_timer_remove() {
    let clock = VirtualClock::new(ClockType::Virtual);
    let fired = Arc::new(AtomicBool::new(false));
    let fired_clone = fired.clone();
    let id = clock.add_timer(100, move || {
        fired_clone.store(true, Ordering::SeqCst);
    });
    assert!(clock.remove_timer(id));
    clock.step(200);
    assert!(!fired.load(Ordering::SeqCst));
}

#[test]
fn test_multiple_timers_order() {
    let clock = VirtualClock::new(ClockType::Virtual);
    let order = Arc::new(std::sync::Mutex::new(Vec::new()));
    let o1 = order.clone();
    let o2 = order.clone();
    let o3 = order.clone();
    clock.add_timer(300, move || o3.lock().unwrap().push(3));
    clock.add_timer(100, move || o1.lock().unwrap().push(1));
    clock.add_timer(200, move || o2.lock().unwrap().push(2));
    clock.step(300);
    assert_eq!(*order.lock().unwrap(), vec![1, 2, 3]);
}
