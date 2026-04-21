use machina_hw_virtio::net::{
    parse_mac, PipeBackend, VirtioNet, DEFAULT_MAC, VIRTIO_NET_HDR_SIZE_BASE,
    VIRTIO_NET_HDR_SIZE_MRG,
};
use machina_hw_virtio::VirtioDevice;

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
    VirtioNet::new_default(Box::new(pipe))
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
