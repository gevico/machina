use machina_core::address::GPA;
use machina_memory::region::MemoryRegion;
use machina_memory::AddressSpace;

use machina_hw_core::loader::{load_binary, load_elf};

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

#[test]
fn test_load_binary_odd_size() {
    // Load 5 bytes; verify byte 5 is correct and byte 6
    // (which was never written) stays at its initial value.
    let as_ = make_ram_as(0x1000);

    // Pre-fill sentinel at offset 0x105 so we can detect
    // an overwrite.
    as_.write(GPA::new(0x105), 1, 0xFF);

    let data: [u8; 5] = [0x10, 0x20, 0x30, 0x40, 0x50];
    let info =
        load_binary(&data, GPA::new(0x100), &as_).expect("load_binary failed");
    assert_eq!(info.size, 5);

    // Byte 4 (index 4) must be 0x50.
    let b4 = as_.read(GPA::new(0x104), 1) as u8;
    assert_eq!(b4, 0x50, "byte 5 must be correct");

    // Byte 5 (index 5) must still be the sentinel 0xFF,
    // proving the loader did not overwrite it.
    let b5 = as_.read(GPA::new(0x105), 1) as u8;
    assert_eq!(b5, 0xFF, "byte 6 must be untouched");
}

#[test]
fn test_load_elf_simple() {
    // Build a minimal ELF-64 LE executable with one
    // PT_LOAD segment.
    let as_ = make_ram_as(0x10000);

    let entry: u64 = 0x1000;
    let payload: [u8; 7] = [0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0x42];
    let p_paddr: u64 = 0x2000;

    // -- ELF header (64 bytes) --
    let mut elf = vec![0u8; 64 + 56];

    // e_ident
    elf[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
    elf[4] = 2; // ELFCLASS64
    elf[5] = 1; // ELFDATA2LSB
    elf[6] = 1; // EV_CURRENT

    // e_type = ET_EXEC (2)
    elf[16..18].copy_from_slice(&2u16.to_le_bytes());
    // e_machine (don't care for loader)
    // e_version
    elf[20..24].copy_from_slice(&1u32.to_le_bytes());
    // e_entry
    elf[24..32].copy_from_slice(&entry.to_le_bytes());
    // e_phoff = 64 (immediately after ehdr)
    elf[32..40].copy_from_slice(&64u64.to_le_bytes());
    // e_shoff = 0
    // e_flags = 0
    // e_ehsize = 64
    elf[40..42].copy_from_slice(&64u16.to_le_bytes());
    // e_phentsize = 56
    elf[42..44].copy_from_slice(&56u16.to_le_bytes());
    // e_phnum = 1
    elf[44..46].copy_from_slice(&1u16.to_le_bytes());

    // -- Program header (56 bytes at offset 64) --
    let ph = 64usize;
    // p_type = PT_LOAD (1)
    elf[ph..ph + 4].copy_from_slice(&1u32.to_le_bytes());
    // p_flags (don't care)
    // p_offset = end of headers (64 + 56 = 120)
    let p_offset: u64 = 120;
    elf[ph + 8..ph + 16].copy_from_slice(&p_offset.to_le_bytes());
    // p_vaddr
    elf[ph + 16..ph + 24].copy_from_slice(&p_paddr.to_le_bytes());
    // p_paddr
    elf[ph + 24..ph + 32].copy_from_slice(&p_paddr.to_le_bytes());
    // p_filesz
    let filesz = payload.len() as u64;
    elf[ph + 32..ph + 40].copy_from_slice(&filesz.to_le_bytes());
    // p_memsz (same as filesz, no BSS)
    elf[ph + 40..ph + 48].copy_from_slice(&filesz.to_le_bytes());

    // Append payload
    elf.extend_from_slice(&payload);

    let info = load_elf(&elf, &as_).expect("load_elf failed");
    assert_eq!(info.entry, GPA::new(entry));
    assert_eq!(info.size, payload.len() as u64);

    // Verify loaded bytes.
    for (i, &expected) in payload.iter().enumerate() {
        let actual = as_.read(GPA::new(p_paddr + i as u64), 1) as u8;
        assert_eq!(actual, expected, "mismatch at offset {i}");
    }
}
