use std::sync::{Arc, Mutex};

use machina_core::address::GPA;
use machina_hw_core::bus::{SysBus, SysBusDeviceState, SysBusError};
use machina_hw_core::irq::{IrqLine, IrqSink};
use machina_hw_core::mdev::MDevice;
use machina_memory::address_space::AddressSpace;
use machina_memory::region::{MemoryRegion, MmioOps};

struct TestMmio {
    value: Arc<Mutex<u32>>,
}

impl MmioOps for TestMmio {
    fn read(&self, _offset: u64, _size: u32) -> u64 {
        *self.value.lock().unwrap() as u64
    }

    fn write(&self, _offset: u64, _size: u32, val: u64) {
        *self.value.lock().unwrap() = val as u32;
    }
}

struct RecordingIrqSink {
    levels: Arc<Mutex<Vec<(u32, bool)>>>,
}

impl IrqSink for RecordingIrqSink {
    fn set_irq(&self, irq: u32, level: bool) {
        self.levels.lock().unwrap().push((irq, level));
    }
}

fn test_irq_line() -> IrqLine {
    let sink = Arc::new(RecordingIrqSink {
        levels: Arc::new(Mutex::new(Vec::new())),
    });
    IrqLine::new(sink as Arc<dyn IrqSink>, 3)
}

fn make_address_space() -> AddressSpace {
    AddressSpace::new(MemoryRegion::container("system", u64::MAX))
}

#[test]
fn test_sysbus_empty() {
    let bus = SysBus::new("main-bus");
    assert_eq!(bus.name, "main-bus");
    assert!(bus.mappings().is_empty());
}

#[test]
fn test_sysbus_realize_maps_mmio_into_address_space() {
    let mut bus = SysBus::new("sysbus0");
    let mut state = SysBusDeviceState::new("uart0");
    let backing = Arc::new(Mutex::new(0u32));
    let region = MemoryRegion::io(
        "uart0-mmio",
        0x100,
        Box::new(TestMmio {
            value: Arc::clone(&backing),
        }),
    );
    state.register_mmio(region, GPA::new(0x1000_0000)).unwrap();
    state.register_irq(test_irq_line()).unwrap();

    let mut address_space = make_address_space();
    assert!(!address_space.is_mapped(GPA::new(0x1000_0000), 4));

    state.attach_to_bus(&bus).unwrap();
    state.realize_onto(&mut bus, &mut address_space).unwrap();

    assert!(address_space.is_mapped(GPA::new(0x1000_0000), 4));
    address_space.write_u32(GPA::new(0x1000_0000), 0x55aa_1234);
    assert_eq!(address_space.read_u32(GPA::new(0x1000_0000)), 0x55aa_1234);
    assert_eq!(*backing.lock().unwrap(), 0x55aa_1234);

    assert_eq!(state.parent_bus(), Some("sysbus0"));
    assert!(state.is_realized());
    assert_eq!(state.irq_outputs().len(), 1);
    assert_eq!(bus.mappings().len(), 1);
    assert_eq!(bus.mappings()[0].owner, "uart0");
    assert_eq!(bus.mappings()[0].name, "uart0-mmio");
    assert_eq!(bus.mappings()[0].base, GPA::new(0x1000_0000));
    assert_eq!(bus.mappings()[0].size, 0x100);
}

#[test]
fn test_sysbus_requires_mmio_before_realize() {
    let mut bus = SysBus::new("sysbus0");
    let mut state = SysBusDeviceState::new("empty-dev");
    let mut address_space = make_address_space();

    state.attach_to_bus(&bus).unwrap();
    let err = state
        .realize_onto(&mut bus, &mut address_space)
        .expect_err("realize without MMIO must fail");
    assert_eq!(err, SysBusError::MissingMmio("empty-dev".to_string()));
}

#[test]
fn test_sysbus_requires_attach_before_realize() {
    let mut bus = SysBus::new("sysbus0");
    let mut state = SysBusDeviceState::new("uart0");
    let region = MemoryRegion::io(
        "uart0-mmio",
        0x100,
        Box::new(TestMmio {
            value: Arc::new(Mutex::new(0)),
        }),
    );
    state.register_mmio(region, GPA::new(0x1000_0000)).unwrap();

    let mut address_space = make_address_space();
    let err = state
        .realize_onto(&mut bus, &mut address_space)
        .expect_err("realize without bus attachment must fail");
    assert_eq!(err, SysBusError::MissingParentBus);
}

#[test]
fn test_sysbus_rejects_overlapping_realize() {
    let mut bus = SysBus::new("sysbus0");
    let mut first = SysBusDeviceState::new("uart0");
    let mut second = SysBusDeviceState::new("timer0");

    first
        .register_mmio(
            MemoryRegion::io(
                "uart0-mmio",
                0x100,
                Box::new(TestMmio {
                    value: Arc::new(Mutex::new(0)),
                }),
            ),
            GPA::new(0x1000_0000),
        )
        .unwrap();
    second
        .register_mmio(
            MemoryRegion::io(
                "timer0-mmio",
                0x100,
                Box::new(TestMmio {
                    value: Arc::new(Mutex::new(0)),
                }),
            ),
            GPA::new(0x1000_0080),
        )
        .unwrap();

    let mut address_space = make_address_space();
    first.attach_to_bus(&bus).unwrap();
    second.attach_to_bus(&bus).unwrap();
    first.realize_onto(&mut bus, &mut address_space).unwrap();

    let err = second
        .realize_onto(&mut bus, &mut address_space)
        .expect_err("overlapping sysbus MMIO must fail");
    assert_eq!(
        err,
        SysBusError::MmioOverlap {
            existing: "uart0-mmio".to_string(),
            requested: "timer0-mmio".to_string(),
        }
    );
    assert_eq!(bus.mappings().len(), 1);
    assert!(!second.is_realized());
}

#[test]
fn test_sysbus_rejects_late_mutation_after_realize() {
    let mut bus = SysBus::new("sysbus0");
    let mut state = SysBusDeviceState::new("uart0");
    state
        .register_mmio(
            MemoryRegion::io(
                "uart0-mmio",
                0x100,
                Box::new(TestMmio {
                    value: Arc::new(Mutex::new(0)),
                }),
            ),
            GPA::new(0x1000_0000),
        )
        .unwrap();

    let mut address_space = make_address_space();
    state.attach_to_bus(&bus).unwrap();
    state.realize_onto(&mut bus, &mut address_space).unwrap();

    let mmio_err = state
        .register_mmio(
            MemoryRegion::io(
                "late-mmio",
                0x100,
                Box::new(TestMmio {
                    value: Arc::new(Mutex::new(0)),
                }),
            ),
            GPA::new(0x1000_1000),
        )
        .expect_err("late sysbus MMIO mutation must fail");
    let irq_err = state
        .register_irq(test_irq_line())
        .expect_err("late sysbus IRQ mutation must fail");

    assert_eq!(
        mmio_err,
        SysBusError::Device(machina_hw_core::mdev::MDeviceError::LateMutation(
            "sysbus_mmio",
        ))
    );
    assert_eq!(
        irq_err,
        SysBusError::Device(machina_hw_core::mdev::MDeviceError::LateMutation(
            "sysbus_irq",
        ))
    );
}

#[test]
fn test_sysbus_unrealize_removes_mmio_from_address_space() {
    let mut bus = SysBus::new("sysbus0");
    let mut state = SysBusDeviceState::new("uart0");
    state
        .register_mmio(
            MemoryRegion::io(
                "uart0-mmio",
                0x100,
                Box::new(TestMmio {
                    value: Arc::new(Mutex::new(0)),
                }),
            ),
            GPA::new(0x1000_0000),
        )
        .unwrap();

    let mut address_space = make_address_space();
    state.attach_to_bus(&bus).unwrap();
    state.realize_onto(&mut bus, &mut address_space).unwrap();
    assert!(address_space.is_mapped(GPA::new(0x1000_0000), 4));
    assert_eq!(bus.mappings().len(), 1);

    state.unrealize_from(&mut bus, &mut address_space).unwrap();

    assert!(!address_space.is_mapped(GPA::new(0x1000_0000), 4));
    assert!(bus.mappings().is_empty());
    assert!(!state.is_realized());
}
