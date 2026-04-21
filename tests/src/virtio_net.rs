use std::sync::Arc;

use machina_hw_virtio::net::{
    parse_mac, PipeBackend, TapBackend, VirtioNet, DEFAULT_MAC,
    VIRTIO_NET_HDR_SIZE_BASE, VIRTIO_NET_HDR_SIZE_MRG,
};
use machina_hw_virtio::VirtioDevice;

// ── TapBackend error ─────────────────────────────────

#[test]
fn test_tap_backend_invalid_ifname() {
    let result = TapBackend::new("nonexistent_xyz99");
    assert!(result.is_err());
}

// ── parse_mac ─────────────────────────────────────────

#[test]
fn test_parse_mac_valid() {
    let mac = parse_mac("52:54:00:12:34:56").unwrap();
    assert_eq!(mac, [0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
}

#[test]
fn test_parse_mac_all_ff() {
    let mac = parse_mac("ff:ff:ff:ff:ff:ff").unwrap();
    assert_eq!(mac, [0xff; 6]);
}

#[test]
fn test_parse_mac_empty() {
    assert!(parse_mac("").is_err());
}

#[test]
fn test_parse_mac_too_few() {
    assert!(parse_mac("52:54:00:12:34").is_err());
}

#[test]
fn test_parse_mac_too_many() {
    assert!(parse_mac("52:54:00:12:34:56:78").is_err());
}

#[test]
fn test_parse_mac_bad_hex() {
    assert!(parse_mac("ZZ:54:00:12:34:56").is_err());
}

// ── VirtioDevice trait ────────────────────────────────

fn make_net() -> VirtioNet {
    let pipe = PipeBackend::new().expect("pipe backend");
    VirtioNet::new_default(Arc::new(pipe))
}

#[test]
fn test_net_device_id() {
    let net = make_net();
    assert_eq!(net.device_id(), 1);
}

#[test]
fn test_net_num_queues() {
    let net = make_net();
    assert_eq!(net.num_queues(), 2);
}

#[test]
fn test_net_features() {
    let net = make_net();
    let f = net.features();
    assert_ne!(f & (1 << 32), 0); // VERSION_1
    assert_ne!(f & (1 << 5), 0); // MAC
    assert_ne!(f & (1 << 16), 0); // STATUS
    assert_ne!(f & (1 << 15), 0); // MRG_RXBUF
}

// ── Config space ──────────────────────────────────────

#[test]
fn test_net_config_mac() {
    let net = make_net();
    for i in 0..6u64 {
        let byte = net.config_read(i, 1) as u8;
        assert_eq!(byte, DEFAULT_MAC[i as usize]);
    }
}

#[test]
fn test_net_config_status() {
    let net = make_net();
    let status = net.config_read(6, 2) as u16;
    assert_eq!(status, 1); // link up
}

#[test]
fn test_net_config_max_vq_pairs() {
    let net = make_net();
    let pairs = net.config_read(8, 2) as u16;
    assert_eq!(pairs, 1);
}

#[test]
fn test_net_config_out_of_range() {
    let net = make_net();
    assert_eq!(net.config_read(100, 1), 0);
}

// ── Feature negotiation ──────────────────────────────

#[test]
fn test_net_hdr_size_base() {
    let mut net = make_net();
    // Ack features without MRG_RXBUF.
    net.ack_features((1u64 << 32) | (1 << 5));
    assert_eq!(net.hdr_size(), VIRTIO_NET_HDR_SIZE_BASE);
}

#[test]
fn test_net_hdr_size_mrg() {
    let mut net = make_net();
    // Ack features with MRG_RXBUF.
    net.ack_features((1u64 << 32) | (1 << 5) | (1 << 15));
    assert_eq!(net.hdr_size(), VIRTIO_NET_HDR_SIZE_MRG);
}

#[test]
fn test_net_reset_clears_features() {
    let mut net = make_net();
    net.ack_features(0xFFFF_FFFF);
    net.reset();
    assert_eq!(net.acked_features, 0);
}

// ── TX path (via MMIO transport) ─────────────────────

use std::sync::atomic::{AtomicBool, Ordering};

use machina_hw_core::irq::{IrqLine, IrqSink};
use machina_hw_virtio::mmio::VirtioMmio;
use machina_memory::region::MmioOps;

struct DummySink {
    level: AtomicBool,
}

impl IrqSink for DummySink {
    fn set_irq(&self, _irq: u32, level: bool) {
        self.level.store(level, Ordering::SeqCst);
    }
}

fn make_net_mmio() -> (VirtioMmio, Arc<DummySink>) {
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

#[test]
fn test_net_queue0_notify_is_noop() {
    let (dev, sink) = make_net_mmio();
    dev.write(0x070, 4, 0x0f); // DRIVER_OK
    dev.write(0x050, 4, 0); // QUEUE_NOTIFY queue 0 (RX)
    assert!(!sink.level.load(Ordering::SeqCst));
}

#[test]
fn test_net_rx_worker_starts_on_create() {
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
    // The RX worker should be running. Dropping
    // the VirtioMmio should join the worker thread
    // without hanging.
    std::thread::sleep(std::time::Duration::from_millis(50));
    drop(mmio);
}

#[test]
fn test_net_drop_joins_rx_thread() {
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
    let start = std::time::Instant::now();
    drop(mmio);
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "drop took too long: {:?}",
        elapsed
    );
}
