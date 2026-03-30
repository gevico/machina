use machina_core::address::GPA;
use machina_memory::region::MemoryRegion;
use machina_memory::AddressSpace;

use machina_hw_core::loader::load_binary;

/// Create a minimal AddressSpace with `size` bytes of RAM
/// starting at guest physical address 0.
fn make_ram_as(size: u64) -> AddressSpace {
    let (ram, _block) = MemoryRegion::ram("ram", size);
    let mut root = MemoryRegion::container("root", size);
    root.add_subregion(ram, GPA::new(0));
    let mut as_ = AddressSpace::new(root);
    as_.update_flat_view();
    as_
}

#[test]
fn test_load_binary() {
    let as_ = make_ram_as(0x1000);
    let data: Vec<u8> = (0u8..16).collect();
    let info =
        load_binary(&data, GPA::new(0x100), &as_).expect("load_binary failed");
    assert_eq!(info.entry, GPA::new(0x100));
    assert_eq!(info.size, 16);

    // Read back and verify.
    let v0 = as_.read_u32(GPA::new(0x100));
    assert_eq!(v0, u32::from_le_bytes([0, 1, 2, 3]),);
    let v1 = as_.read_u32(GPA::new(0x104));
    assert_eq!(v1, u32::from_le_bytes([4, 5, 6, 7]),);
}

#[test]
fn test_load_binary_alignment() {
    let as_ = make_ram_as(0x1000);
    // 5 bytes — not a multiple of 4.
    let data = vec![0xAA, 0xBB, 0xCC, 0xDD, 0xEE];
    let info =
        load_binary(&data, GPA::new(0x200), &as_).expect("load_binary failed");
    assert_eq!(info.size, 5);

    // First 4 bytes form one u32 write.
    let v0 = as_.read_u32(GPA::new(0x200));
    assert_eq!(v0, u32::from_le_bytes([0xAA, 0xBB, 0xCC, 0xDD]),);
    // Remaining 1 byte is written as a partial u32
    // (upper 3 bytes zero).
    let v1 = as_.read_u32(GPA::new(0x204));
    assert_eq!(v1, 0xEE);
}
