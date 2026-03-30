use machina_hw_core::fdt::FdtBuilder;

/// Read a big-endian u32 from a byte slice at `offset`.
fn read_be_u32(blob: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(blob[offset..offset + 4].try_into().unwrap())
}

#[test]
fn test_fdt_magic() {
    let mut fdt = FdtBuilder::new();
    fdt.begin_node("");
    fdt.end_node();
    let blob = fdt.finish();
    assert_eq!(read_be_u32(&blob, 0), 0xd00d_feed);
}

#[test]
fn test_fdt_simple_node() {
    let mut fdt = FdtBuilder::new();
    fdt.begin_node("");
    fdt.property_u32("#address-cells", 2);
    fdt.property_u32("#size-cells", 2);
    fdt.begin_node("memory@80000000");
    fdt.property_string("device_type", "memory");
    fdt.end_node();
    fdt.end_node();
    let blob = fdt.finish();

    // Verify header fields.
    let magic = read_be_u32(&blob, 0);
    assert_eq!(magic, 0xd00d_feed);

    let totalsize = read_be_u32(&blob, 4) as usize;
    assert_eq!(totalsize, blob.len());

    let version = read_be_u32(&blob, 20);
    assert_eq!(version, 17);
}

#[test]
fn test_fdt_string_property() {
    let mut fdt = FdtBuilder::new();
    fdt.begin_node("");
    fdt.property_string("compatible", "riscv-virtio");
    fdt.end_node();
    let blob = fdt.finish();

    // The string "riscv-virtio\0" must appear in the
    // structure block as the property value.
    let needle = b"riscv-virtio\0";
    let found = blob.windows(needle.len()).any(|w| w == needle);
    assert!(found, "string property value must be null-terminated");
}
