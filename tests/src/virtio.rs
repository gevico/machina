use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use machina_core::address::GPA;
use machina_hw_core::bus::SysBus;
use machina_hw_core::irq::{IrqLine, IrqSink};
use machina_hw_virtio::block::VirtioBlk;
use machina_hw_virtio::mmio::VirtioMmio;
#[cfg(unix)]
use machina_hw_virtio::net::PipeBackend;
#[cfg(unix)]
use machina_hw_virtio::net::VirtioNet;
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
    dev.attach_to_bus(&mut bus).unwrap();
    let region = dev.make_mmio_region("virtio-mmio0", 0x1000);
    dev.register_mmio(region, GPA::new(0x1000_1000)).unwrap();

    let mut address_space = make_address_space();
    dev.realize_onto(&mut bus, &mut address_space).unwrap();

    assert!(address_space.is_mapped(GPA::new(0x1000_1000), 4));
    assert_eq!(address_space.read(GPA::new(0x1000_1000), 4), 0x74726976);
    assert_eq!(bus.mappings().len(), 1);
    assert_eq!(bus.mappings()[0].owner, "virtio-mmio");
}

// ── Multi-queue transport tests (AC-1) ───────────────

#[cfg(unix)]
fn make_net_device() -> (VirtioMmio, Arc<DummySink>) {
    let pipe = PipeBackend::new().unwrap();
    let net = VirtioNet::new_default(Arc::new(pipe));
    let sink = Arc::new(DummySink {
        level: AtomicBool::new(false),
    });
    let irq = IrqLine::new(sink.clone() as Arc<dyn IrqSink>, 1);
    let mmio = VirtioMmio::new(
        Box::new(net),
        irq,
        std::ptr::null_mut(),
        0x8000_0000,
        128 * 1024 * 1024,
    );
    (mmio, sink)
}

#[cfg(unix)]
#[test]
fn test_multiqueue_num_max_queue0() {
    let (dev, _) = make_net_device();
    dev.write(0x030, 4, 0); // QUEUE_SEL = 0
    assert_ne!(dev.read(0x034, 4), 0); // QUEUE_NUM_MAX
}

#[cfg(unix)]
#[test]
fn test_multiqueue_num_max_queue1() {
    let (dev, _) = make_net_device();
    dev.write(0x030, 4, 1); // QUEUE_SEL = 1
    assert_ne!(dev.read(0x034, 4), 0); // QUEUE_NUM_MAX
}

#[cfg(unix)]
#[test]
fn test_multiqueue_invalid_queue_sel() {
    let (dev, _) = make_net_device();
    dev.write(0x030, 4, 99); // invalid queue
    assert_eq!(dev.read(0x034, 4), 0); // 0 for invalid
}

#[cfg(unix)]
#[test]
fn test_multiqueue_notify_without_driver_ok() {
    let (dev, sink) = make_net_device();
    dev.write(0x070, 4, 1); // ACKNOWLEDGE only, no OK
    dev.write(0x050, 4, 1); // QUEUE_NOTIFY queue 1
    assert!(!sink.level.load(Ordering::SeqCst));
}

#[cfg(unix)]
#[test]
fn test_multiqueue_net_device_id() {
    let (dev, _) = make_net_device();
    assert_eq!(dev.read(0x008, 4), 1); // net = 1
}

#[test]
fn test_multiqueue_blk_still_one_queue() {
    let (dev, _) = make_test_device();
    dev.write(0x030, 4, 0);
    assert_ne!(dev.read(0x034, 4), 0);
    dev.write(0x030, 4, 1);
    assert_eq!(dev.read(0x034, 4), 0); // only 1 queue
}

#[cfg(unix)]
#[test]
fn test_multiqueue_notify_queue1_with_driver_ok() {
    let (dev, _) = make_net_device();
    dev.write(0x070, 4, 0x0f);
    dev.write(0x050, 4, 1);
}

use std::sync::atomic::AtomicU32;

struct SpyDevice {
    called: Arc<AtomicU32>,
    last_queue: Arc<AtomicU32>,
}

impl machina_hw_virtio::VirtioDevice for SpyDevice {
    fn device_id(&self) -> u32 {
        99
    }
    fn features(&self) -> u64 {
        1 << 32
    }
    fn ack_features(&mut self, _f: u64) {}
    fn num_queues(&self) -> usize {
        2
    }
    fn config_read(&self, _offset: u64, _size: u32) -> u64 {
        0
    }
    unsafe fn handle_queue(
        &mut self,
        idx: u32,
        _queue: &mut VirtQueue,
        _ram: *mut u8,
        _ram_base: u64,
        _ram_size: u64,
    ) -> u32 {
        self.called.fetch_add(1, Ordering::SeqCst);
        self.last_queue.store(idx, Ordering::SeqCst);
        0
    }
}

#[test]
fn test_queue1_notify_dispatches_handle_queue() {
    let called = Arc::new(AtomicU32::new(0));
    let last_q = Arc::new(AtomicU32::new(u32::MAX));
    let spy = SpyDevice {
        called: Arc::clone(&called),
        last_queue: Arc::clone(&last_q),
    };
    let sink = Arc::new(DummySink {
        level: AtomicBool::new(false),
    });
    let irq = IrqLine::new(sink.clone() as Arc<dyn IrqSink>, 1);
    let dev = VirtioMmio::new(
        Box::new(spy),
        irq,
        std::ptr::null_mut(),
        0x8000_0000,
        128 * 1024 * 1024,
    );
    // Set up queue 1: select, set size, mark ready.
    dev.write(0x030, 4, 1); // QUEUE_SEL = 1
    dev.write(0x038, 4, 16); // QUEUE_NUM = 16
    dev.write(0x044, 4, 1); // QUEUE_READY = 1
    dev.write(0x070, 4, 0x0f); // DRIVER_OK
    dev.write(0x050, 4, 1); // QUEUE_NOTIFY q=1
    assert!(
        called.load(Ordering::SeqCst) > 0,
        "handle_queue was not called"
    );
    assert_eq!(
        last_q.load(Ordering::SeqCst),
        1,
        "wrong queue index dispatched"
    );
}
