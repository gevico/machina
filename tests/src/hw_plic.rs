use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use machina_core::address::GPA;
use machina_hw_core::bus::SysBus;
use machina_hw_core::irq::{IrqLine, IrqSink};
use machina_hw_intc::plic::{Plic, PlicMmio};
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
fn test_plic_set_priority() {
    let mut plic = Plic::new(64, 2);

    // Write priority for IRQ 1 via MMIO (offset 4).
    plic.write(0x04, 4, 7);
    assert_eq!(plic.read(0x04, 4), 7);

    // Write priority for IRQ 10 (offset 0x28).
    plic.write(0x28, 4, 3);
    assert_eq!(plic.read(0x28, 4), 3);
}

#[test]
fn test_plic_claim_highest() {
    let mut plic = Plic::new(64, 2);

    // Set priorities: IRQ 1 = 2, IRQ 2 = 5.
    plic.write(0x04, 4, 2);
    plic.write(0x08, 4, 5);

    // Enable IRQ 1 and IRQ 2 for context 0.
    plic.write(0x2000, 4, 0x06);

    // Set both pending.
    plic.set_pending(1, true);
    plic.set_pending(2, true);

    // Claim should return IRQ 2 (higher priority).
    let claimed = plic.claim_irq(0);
    assert_eq!(claimed, Some(2));
}

#[test]
fn test_plic_complete() {
    let mut plic = Plic::new(64, 1);

    plic.write(0x04, 4, 1);
    plic.write(0x2000, 4, 0x02);
    plic.set_pending(1, true);

    let claimed = plic.claim_irq(0);
    assert_eq!(claimed, Some(1));

    // Complete via MMIO.
    plic.write(0x200004, 4, 1);

    // After completion, claim register should be 0.
    assert_eq!(plic.read(0x200004, 4), 0);
}

#[test]
fn test_plic_threshold() {
    let mut plic = Plic::new(64, 1);

    plic.write(0x04, 4, 3);
    plic.write(0x2000, 4, 0x02);
    plic.set_pending(1, true);

    // Set threshold for context 0 to 5.
    plic.write(0x200000, 4, 5);

    // IRQ 1 priority 3 <= threshold 5 -> not claimable.
    assert_eq!(plic.claim_irq(0), None);
}

#[test]
fn test_plic_no_pending() {
    let mut plic = Plic::new(64, 1);

    plic.write(0x04, 4, 1);
    plic.write(0x2000, 4, 0x02);

    // Nothing pending.
    assert_eq!(plic.claim_irq(0), None);
}

#[test]
fn test_plic_set_irq_propagates() {
    let mut plic = Plic::new(64, 2);

    // Connect output for context 0.
    let sink = Arc::new(TestIrqSink::new(16));
    let out_irq = 11u32; // MEI
    let line = IrqLine::new(Arc::clone(&sink) as Arc<dyn IrqSink>, out_irq);
    plic.connect_context_output(0, line);

    // Set priority[1] = 1, enable IRQ 1 for ctx 0.
    plic.write(0x04, 4, 1);
    plic.write(0x2000, 4, 0x02);

    // No interrupt yet.
    assert!(!sink.level(out_irq), "output should be low before set_irq");

    // Assert source 1.
    plic.set_irq(1, true);
    assert!(sink.level(out_irq), "output should go high after set_irq");

    // Deassert source 1.
    plic.set_irq(1, false);
    assert!(
        !sink.level(out_irq),
        "output should go low after clearing IRQ"
    );
}

#[test]
fn test_plic_claim_on_read() {
    let mut plic = Plic::new(64, 1);

    plic.write(0x04, 4, 1);
    plic.write(0x2000, 4, 0x02);
    plic.set_pending(1, true);

    // MMIO read at claim offset should perform the claim.
    let claimed = plic.read(0x200004, 4);
    assert_eq!(claimed, 1, "MMIO claim read should return IRQ 1");

    // Pending bit should now be cleared.
    let pending = plic.read(0x1000, 4);
    assert_eq!(
        pending & (1 << 1),
        0,
        "pending bit should be cleared after claim"
    );

    // Second claim read should return 0.
    let claimed2 = plic.read(0x200004, 4);
    assert_eq!(claimed2, 0, "second claim should return 0");
}

#[test]
fn test_plic_level_triggered_resample() {
    let mut plic = Plic::new(64, 1);

    // Connect output for context 0.
    let sink = Arc::new(TestIrqSink::new(16));
    let out_irq = 11u32; // MEI
    let line = IrqLine::new(Arc::clone(&sink) as Arc<dyn IrqSink>, out_irq);
    plic.connect_context_output(0, line);

    // priority[1] = 1, enable IRQ 1 for ctx 0.
    plic.write(0x04, 4, 1);
    plic.write(0x2000, 4, 0x02);

    // Assert source 1 (level-triggered).
    plic.set_irq(1, true);
    assert!(sink.level(out_irq), "output should be high");

    // Claim via MMIO.
    let claimed = plic.read(0x200004, 4);
    assert_eq!(claimed, 1);

    // Complete without lowering source -- should re-pend.
    plic.write(0x200004, 4, 1);

    let pending = plic.read(0x1000, 4);
    assert_ne!(
        pending & (1 << 1),
        0,
        "pending should be re-set after complete \
         while source high"
    );
    assert!(
        sink.level(out_irq),
        "output should remain high after resample"
    );

    // Now lower the source, claim and complete again.
    plic.set_irq(1, false);

    plic.set_irq(1, true);
    let claimed2 = plic.read(0x200004, 4);
    assert_eq!(claimed2, 1);

    // Lower source before completing.
    plic.set_irq(1, false);
    plic.write(0x200004, 4, 1);

    let pending2 = plic.read(0x1000, 4);
    assert_eq!(
        pending2 & (1 << 1),
        0,
        "pending should stay cleared when source is low"
    );
}

#[test]
fn test_plic_realize_via_sysbus_maps_mmio() {
    let mut bus = SysBus::new("sysbus0");
    let plic = Arc::new(Mutex::new(Plic::new_named("plic0", 64, 2)));
    {
        let mut device = plic.lock().unwrap();
        device.attach_to_bus(&bus).unwrap();
        device
            .register_mmio(
                MemoryRegion::io(
                    "plic",
                    0x0400_0000,
                    Box::new(PlicMmio(Arc::clone(&plic))),
                ),
                GPA::new(0x0C00_0000),
            )
            .unwrap();
    }

    let mut address_space = make_address_space();
    plic.lock()
        .unwrap()
        .realize_onto(&mut bus, &mut address_space)
        .unwrap();

    assert!(address_space.is_mapped(GPA::new(0x0C00_0000), 4));
    address_space.write(GPA::new(0x0C00_0004), 4, 7);
    assert_eq!(address_space.read(GPA::new(0x0C00_0004), 4), 7);
    assert_eq!(bus.mappings().len(), 1);
    assert_eq!(bus.mappings()[0].owner, "plic0");
}
