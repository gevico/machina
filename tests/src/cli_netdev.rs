use machina_core::machine::NetdevOpts;

#[test]
fn test_parse_netdev_valid() {
    let nd = NetdevOpts::parse("tap,id=net0,ifname=tap0", None).unwrap();
    assert_eq!(nd.id, "net0");
    assert_eq!(nd.ifname, "tap0");
    assert_eq!(nd.mac, None);
}

#[test]
fn test_parse_netdev_with_device_mac() {
    let nd = NetdevOpts::parse(
        "tap,id=net0,ifname=tap0",
        Some(
            "virtio-net-device,\
             netdev=net0,mac=52:54:00:12:34:56",
        ),
    )
    .unwrap();
    assert_eq!(nd.id, "net0");
    assert_eq!(nd.mac.as_deref(), Some("52:54:00:12:34:56"));
}

#[test]
fn test_parse_netdev_missing_id() {
    let result = NetdevOpts::parse("tap,ifname=tap0", None);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("missing id="));
}

#[test]
fn test_parse_netdev_missing_ifname() {
    let result = NetdevOpts::parse("tap,id=net0", None);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("missing ifname="));
}

#[test]
fn test_parse_netdev_unsupported_type() {
    let result = NetdevOpts::parse("user,id=net0", None);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("unsupported type"));
}

#[test]
fn test_parse_device_without_netdev_field() {
    let result = NetdevOpts::parse(
        "tap,id=net0,ifname=tap0",
        Some(
            "virtio-net-device,\
             mac=52:54:00:12:34:56",
        ),
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("missing netdev="));
}

#[test]
fn test_parse_device_netdev_id_mismatch() {
    let result = NetdevOpts::parse(
        "tap,id=net0,ifname=tap0",
        Some("virtio-net-device,netdev=other"),
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("does not match"));
}
