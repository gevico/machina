use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use machina_core::address::GPA;
use machina_hw_core::bus::SysBus;
use machina_hw_core::irq::{IrqLine, IrqSink};
use machina_hw_intc::aclint::{Aclint, AclintMmio};
use machina_memory::address_space::AddressSpace;
use machina_memory::region::MemoryRegion;

struct TestIrqSink {
    levels: Vec<AtomicBool>,
}

impl TestIrqSink {
    fn new(n: usize) -> Self {
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(AtomicBool::new(false));
        }
        Self { levels: v }
    }

    fn level(&self, irq: u32) -> bool {
        self.levels[irq as usize].load(Ordering::Relaxed)
    }
}

impl IrqSink for TestIrqSink {
    fn set_irq(&self, irq: u32, level: bool) {
        if let Some(f) = self.levels.get(irq as usize) {
            f.store(level, Ordering::Relaxed);
        }
    }
}

fn make_address_space() -> AddressSpace {
    AddressSpace::new(MemoryRegion::container("system", u64::MAX))
}

#[test]
fn test_aclint_mtime_wall_clock() {
    let aclint = Aclint::new(2);

    let t0 = aclint.read(0xBFF8, 8);
    std::thread::sleep(std::time::Duration::from_millis(100));
    let t1 = aclint.read(0xBFF8, 8);

    // 100ms at 10 MHz = ~1_000_000 ticks.
    let diff = t1 - t0;
    assert!(
        diff > 500_000 && diff < 2_000_000,
        "mtime diff {} not in expected range for 100ms",
        diff
    );
}

#[test]
fn test_aclint_mtime_write_resets_epoch() {
    let mut aclint = Aclint::new(2);

    aclint.write(0xBFF8, 8, 1_000_000);
    let t = aclint.read(0xBFF8, 8);
    // Should be close to 1_000_000 (just written).
    assert!(
        (1_000_000..1_100_000).contains(&t),
        "mtime {} not near written value 1_000_000",
        t
    );
}

#[test]
fn test_aclint_mtimecmp_set() {
    let mut aclint = Aclint::new(2);

    aclint.write(0x4000, 8, 500);
    assert_eq!(aclint.read(0x4000, 8), 500);

    aclint.write(0x4008, 8, 1000);
    assert_eq!(aclint.read(0x4008, 8), 1000);
}

#[test]
fn test_aclint_msip_set_clear() {
    let mut aclint = Aclint::new(2);

    aclint.write(0x0000, 4, 1);
    assert_eq!(aclint.read(0x0000, 4), 1);

    aclint.write(0x0000, 4, 0);
    assert_eq!(aclint.read(0x0000, 4), 0);

    // Only bit 0 is writable.
    aclint.write(0x0000, 4, 0xFF);
    assert_eq!(aclint.read(0x0000, 4), 1);
}

#[test]
fn test_aclint_timer_compare() {
    let mut aclint = Aclint::new(2);

    // Set mtimecmp[0] = 100.
    aclint.write(0x4000, 8, 100);

    // Set mtime to 50 (below mtimecmp).
    aclint.write(0xBFF8, 8, 50);
    assert!(!aclint.timer_irq_pending(0));

    // Set mtime to 100 (at threshold).
    aclint.write(0xBFF8, 8, 100);
    assert!(aclint.timer_irq_pending(0));

    // Hart 1 has mtimecmp = u64::MAX, not pending.
    assert!(!aclint.timer_irq_pending(1));
}

#[test]
fn test_aclint_mti_output() {
    let mut aclint = Aclint::new(2);

    let sink = Arc::new(TestIrqSink::new(16));
    let mti_irq = 7u32;
    let line = IrqLine::new(Arc::clone(&sink) as Arc<dyn IrqSink>, mti_irq);
    aclint.connect_mti(0, line);

    // Set mtimecmp[0] = 100.
    aclint.write(0x4000, 8, 100);

    // Set mtime = 50 -> MTI low.
    aclint.write(0xBFF8, 8, 50);
    assert!(
        !sink.level(mti_irq),
        "MTI should be low when mtime < mtimecmp"
    );

    // Set mtime = 100 -> MTI high.
    aclint.write(0xBFF8, 8, 100);
    assert!(
        sink.level(mti_irq),
        "MTI should be high when mtime >= mtimecmp"
    );

    // Raise mtimecmp to 200 -> MTI low.
    aclint.write(0x4000, 8, 200);
    assert!(
        !sink.level(mti_irq),
        "MTI should go low after raising mtimecmp"
    );
}

#[test]
fn test_aclint_msi_output() {
    let mut aclint = Aclint::new(2);

    let sink = Arc::new(TestIrqSink::new(16));
    let msi_irq = 3u32;
    let line = IrqLine::new(Arc::clone(&sink) as Arc<dyn IrqSink>, msi_irq);
    aclint.connect_msi(0, line);

    assert!(!sink.level(msi_irq), "MSI should be low");

    aclint.write(0x0000, 4, 1);
    assert!(sink.level(msi_irq), "MSI should go high after msip=1");

    aclint.write(0x0000, 4, 0);
    assert!(!sink.level(msi_irq), "MSI should go low after msip=0");
}

#[test]
fn test_aclint_clint_layout() {
    let mut aclint = Aclint::new(2);

    aclint.write(0x0000, 4, 1);
    assert_eq!(aclint.read(0x0000, 4), 1);

    aclint.write(0x4000, 8, 42);
    assert_eq!(aclint.read(0x4000, 8), 42);

    aclint.write(0xBFF8, 8, 999);
    let t = aclint.read(0xBFF8, 8);
    assert!(
        (999..999 + 100_000).contains(&t),
        "mtime {} not near written value 999",
        t
    );
}

#[test]
fn test_aclint_mtimecmp_disable() {
    let mut aclint = Aclint::new(1);
    let sink = Arc::new(TestIrqSink::new(16));
    let mti = 7u32;
    let line = IrqLine::new(Arc::clone(&sink) as Arc<dyn IrqSink>, mti);
    aclint.connect_mti(0, line);

    // Set mtimecmp to a near-future value, then disable.
    aclint.write(0xBFF8, 8, 0);
    aclint.write(0x4000, 8, 10);
    aclint.write(0xBFF8, 8, 100);
    assert!(sink.level(mti), "MTI should be high");

    // Disable timer by setting mtimecmp to u64::MAX.
    aclint.write(0x4000, 8, u64::MAX);
    assert!(
        !sink.level(mti),
        "MTI should go low after mtimecmp=u64::MAX"
    );
}

#[test]
fn test_aclint_mtimecmp_retarget_past() {
    let mut aclint = Aclint::new(1);
    let sink = Arc::new(TestIrqSink::new(16));
    let mti = 7u32;
    let line = IrqLine::new(Arc::clone(&sink) as Arc<dyn IrqSink>, mti);
    aclint.connect_mti(0, line);

    // Set mtime high, then set mtimecmp to past value.
    aclint.write(0xBFF8, 8, 1000);
    aclint.write(0x4000, 8, 500);
    assert!(sink.level(mti), "MTI should be high when mtimecmp < mtime");
}

#[test]
fn test_aclint_timer_thread_asserts_mti() {
    let mut aclint = Aclint::new(1);
    let sink = Arc::new(TestIrqSink::new(16));
    let mti = 7u32;
    let line = IrqLine::new(Arc::clone(&sink) as Arc<dyn IrqSink>, mti);
    aclint.connect_mti(0, line);

    // Set mtime near current wall clock, mtimecmp 10ms
    // in the future. The timer thread should assert MTI.
    let now = aclint.read(0xBFF8, 8);
    aclint.write(0x4000, 8, now + 100_000); // 10ms

    assert!(!sink.level(mti), "MTI should be low before deadline");

    // Wait for timer thread to fire.
    std::thread::sleep(std::time::Duration::from_millis(30));
    assert!(sink.level(mti), "MTI should be high after timer deadline");
}

#[test]
fn test_aclint_retarget_future_cancels_stale_timer() {
    let mut aclint = Aclint::new(1);
    let sink = Arc::new(TestIrqSink::new(16));
    let mti = 7u32;
    let line = IrqLine::new(Arc::clone(&sink) as Arc<dyn IrqSink>, mti);
    aclint.connect_mti(0, line);

    let now = aclint.read(0xBFF8, 8);

    // Set mtimecmp to now+20ms, then immediately retarget
    // to now+1s. The old 20ms timer must be cancelled.
    aclint.write(0x4000, 8, now + 200_000); // 20ms
    aclint.write(0x4000, 8, now + 10_000_000); // 1s

    // After 50ms, old timer would have fired but MTI
    // should still be low because it was cancelled.
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(
        !sink.level(mti),
        "MTI must stay low after retarget cancelled \
         the old 20ms timer"
    );

    // Now retarget to near future (10ms from current).
    let now2 = aclint.read(0xBFF8, 8);
    aclint.write(0x4000, 8, now2 + 100_000); // 10ms
    std::thread::sleep(std::time::Duration::from_millis(30));
    assert!(
        sink.level(mti),
        "MTI should be high after retarget to near \
         future"
    );
}

#[test]
fn test_aclint_realize_via_sysbus_maps_mmio() {
    let mut bus = SysBus::new("sysbus0");
    let aclint = Arc::new(Mutex::new(Aclint::new_named("aclint0", 2)));
    {
        let mut device = aclint.lock().unwrap();
        device.attach_to_bus(&bus).unwrap();
        device
            .register_mmio(
                MemoryRegion::io(
                    "clint",
                    0x1_0000,
                    Box::new(AclintMmio(Arc::clone(&aclint))),
                ),
                GPA::new(0x0200_0000),
            )
            .unwrap();
    }

    let mut address_space = make_address_space();
    aclint
        .lock()
        .unwrap()
        .realize_onto(&mut bus, &mut address_space)
        .unwrap();

    assert!(address_space.is_mapped(GPA::new(0x0200_0000), 8));
    address_space.write(GPA::new(0x0200_4000), 8, 0x1234);
    assert_eq!(address_space.read(GPA::new(0x0200_4000), 8), 0x1234);
    assert_eq!(bus.mappings().len(), 1);
    assert_eq!(bus.mappings()[0].owner, "aclint0");
}
