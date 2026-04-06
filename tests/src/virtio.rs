use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use machina_core::address::GPA;
use machina_hw_core::bus::SysBus;
use machina_hw_core::irq::{IrqLine, IrqSink};
use machina_hw_virtio::block::VirtioBlk;
use machina_hw_virtio::mmio::VirtioMmio;
use machina_hw_virtio::queue::VirtQueue;
use machina_memory::address_space::AddressSpace;
use machina_memory::region::MemoryRegion;
use machina_memory::region::MmioOps;

struct DummySink {
    level: AtomicBool,
}

impl IrqSink for DummySink {
    fn set_irq(&self, _irq: u32, level: bool) {
        self.level.store(level, Ordering::SeqCst);
    }
}

fn make_test_device() -> (VirtioMmio, Arc<DummySink>) {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(&[0u8; 512]).unwrap();
    let path = f.into_temp_path();
    let blk = VirtioBlk::open(path.as_ref()).unwrap();
    let sink = Arc::new(DummySink {
        level: AtomicBool::new(false),
    });
    let irq = IrqLine::new(sink.clone() as Arc<dyn IrqSink>, 1);
    let mmio = VirtioMmio::new(
        Box::new(blk),
        irq,
        std::ptr::null_mut(),
        0x8000_0000,
        128 * 1024 * 1024,
    );
    (mmio, sink)
}

fn make_address_space() -> AddressSpace {
    AddressSpace::new(MemoryRegion::container("system", u64::MAX))
}

#[test]
fn test_virtio_magic_version_device_id() {
    let (dev, _) = make_test_device();
    assert_eq!(dev.read(0x000, 4), 0x74726976);
    assert_eq!(dev.read(0x004, 4), 2);
    assert_eq!(dev.read(0x008, 4), 2);
    assert_eq!(dev.read(0x00c, 4), 0x554D4551);
}

#[test]
fn test_virtio_status_reset() {
    let (dev, _) = make_test_device();
    dev.write(0x070, 4, 1); // ACKNOWLEDGE
    assert_eq!(dev.read(0x070, 4), 1);
    dev.write(0x070, 4, 3); // +DRIVER
    assert_eq!(dev.read(0x070, 4), 3);
    dev.write(0x070, 4, 0); // Reset
    assert_eq!(dev.read(0x070, 4), 0);
}

#[test]
fn test_virtio_features() {
    let (dev, _) = make_test_device();
    dev.write(0x014, 4, 0);
    let f0 = dev.read(0x010, 4);
    dev.write(0x014, 4, 1);
    let f1 = dev.read(0x010, 4);
    assert_eq!(f0, 0);
    assert_eq!(f1, 1); // VIRTIO_F_VERSION_1
}

#[test]
fn test_virtio_interrupt_ack() {
    let (dev, sink) = make_test_device();
    // Manually trigger interrupt via write path.
    dev.write(0x070, 4, 0x0f); // status = DRIVER_OK
                               // Force interrupt by internal access is not
                               // possible without queue I/O, so test ACK path:
                               // First verify status reads back.
    assert_eq!(dev.read(0x060, 4), 0); // no IRQ
                                       // ACK with no pending is safe.
    dev.write(0x064, 4, 1);
    assert_eq!(dev.read(0x060, 4), 0);
    assert!(!sink.level.load(Ordering::SeqCst));
}

#[test]
fn test_virtio_config_capacity() {
    let (dev, _) = make_test_device();
    // 512 bytes / 512 = 1 sector.
    let cap = dev.read(0x100, 4);
    assert_eq!(cap, 1);
}

#[test]
fn test_virtqueue_new_and_reset() {
    let q = VirtQueue::new();
    assert!(!q.ready);
    assert_eq!(q.num, 0);
    assert_eq!(q.last_avail_idx, 0);

    let mut q2 = VirtQueue::new();
    q2.ready = true;
    q2.num = 128;
    q2.last_avail_idx = 42;
    q2.reset();
    assert!(!q2.ready);
    assert_eq!(q2.num, 0);
    assert_eq!(q2.last_avail_idx, 0);
}

#[test]
fn test_virtio_queue_num_max() {
    let (dev, _) = make_test_device();
    dev.write(0x030, 4, 0); // QUEUE_SEL = 0
    assert_eq!(dev.read(0x034, 4), 256);
}

#[test]
fn test_virtio_realize_via_sysbus_maps_mmio() {
    let (mut dev, _) = make_test_device();
    let mut bus = SysBus::new("sysbus0");
    dev.attach_to_bus(&bus).unwrap();
    let region = dev.make_mmio_region("virtio-mmio0", 0x1000);
    dev.register_mmio(region, GPA::new(0x1000_1000)).unwrap();

    let mut address_space = make_address_space();
    dev.realize_onto(&mut bus, &mut address_space).unwrap();

    assert!(address_space.is_mapped(GPA::new(0x1000_1000), 4));
    assert_eq!(address_space.read(GPA::new(0x1000_1000), 4), 0x74726976);
    assert_eq!(bus.mappings().len(), 1);
    assert_eq!(bus.mappings()[0].owner, "virtio-mmio");
}
