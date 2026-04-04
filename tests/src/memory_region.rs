use std::sync::Mutex;

use machina_core::address::GPA;
use machina_memory::*;

// -- helpers --

struct MockUart {
    regs: Mutex<[u8; 256]>,
}

impl MockUart {
    fn new() -> Self {
        Self {
            regs: Mutex::new([0u8; 256]),
        }
    }
}

impl MmioOps for MockUart {
    fn read(&self, offset: u64, _size: u32) -> u64 {
        let regs = self.regs.lock().unwrap();
        regs[offset as usize] as u64
    }

    fn write(&self, offset: u64, _size: u32, val: u64) {
        let mut regs = self.regs.lock().unwrap();
        regs[offset as usize] = val as u8;
    }
}

// -- test cases --

#[test]
fn test_ram_block_alloc() {
    let block = RamBlock::new(4096);
    assert_eq!(block.size(), 4096);

    // Write pattern then read it back.
    let ptr = block.as_ptr();
    unsafe {
        for i in 0..4096u64 {
            *ptr.add(i as usize) = (i & 0xff) as u8;
        }
        for i in 0..4096u64 {
            assert_eq!(*ptr.add(i as usize), (i & 0xff) as u8,);
        }
    }
}

#[test]
fn test_flat_view_single_ram() {
    let (ram, _block) = MemoryRegion::ram("dram", 0x1_0000);

    let mut root = MemoryRegion::container("system", 0x10_0000);
    root.add_subregion(ram, GPA::new(0x8_0000));

    let fv = FlatView::from_region(&root);
    assert_eq!(fv.ranges.len(), 1);
    assert_eq!(fv.ranges[0].addr, GPA::new(0x8_0000));
    assert_eq!(fv.ranges[0].size, 0x1_0000);
    assert!(!fv.ranges[0].is_io());

    // Lookup inside the range.
    assert!(fv.lookup(GPA::new(0x8_0000)).is_some());
    assert!(fv.lookup(GPA::new(0x8_FFFF)).is_some());

    // Lookup outside.
    assert!(fv.lookup(GPA::new(0x7_FFFF)).is_none());
    assert!(fv.lookup(GPA::new(0x9_0000)).is_none());
}

#[test]
fn test_flat_view_overlap_priority() {
    // Low-priority RAM spanning 0..0x10000.
    let (ram_lo, _blk_lo) = MemoryRegion::ram("lo-ram", 0x1_0000);

    // High-priority IO covering 0x1000..0x2000 (4 KiB)
    // overlapping the RAM.
    let uart = MockUart::new();
    let io_hi = MemoryRegion::io("uart", 0x1000, Box::new(uart));

    let mut root = MemoryRegion::container("root", 0x10_0000);
    root.add_subregion(ram_lo, GPA::new(0));
    root.add_subregion_with_priority(io_hi, GPA::new(0x1000), 10);

    let fv = FlatView::from_region(&root);

    // There should be 3 ranges:
    //   [0x0000, 0x1000) RAM
    //   [0x1000, 0x2000) IO  (higher priority)
    //   [0x2000, 0x10000) RAM
    assert_eq!(fv.ranges.len(), 3);

    let r0 = &fv.ranges[0];
    assert_eq!(r0.addr, GPA::new(0x0000));
    assert_eq!(r0.size, 0x1000);
    assert!(!r0.is_io());

    let r1 = &fv.ranges[1];
    assert_eq!(r1.addr, GPA::new(0x1000));
    assert_eq!(r1.size, 0x1000);
    assert!(r1.is_io());

    let r2 = &fv.ranges[2];
    assert_eq!(r2.addr, GPA::new(0x2000));
    assert_eq!(r2.size, 0xE000);
    assert!(!r2.is_io());
}

#[test]
fn test_address_space_read_write() {
    let (ram, _block) = MemoryRegion::ram("dram", 0x1_0000);
    let mut root = MemoryRegion::container("root", 0x10_0000);
    root.add_subregion(ram, GPA::new(0));

    let a = AddressSpace::new(root);

    // Write a u32 then read it back.
    a.write_u32(GPA::new(0x100), 0xDEAD_BEEF);
    assert_eq!(a.read_u32(GPA::new(0x100)), 0xDEAD_BEEF);

    // Write individual bytes and read back as u32.
    let msg = b"abcd";
    for (i, &b) in msg.iter().enumerate() {
        a.write(GPA::new(0x200 + i as u64), 1, b as u64);
    }
    let val = a.read(GPA::new(0x200), 4) as u32;
    assert_eq!(&val.to_le_bytes(), msg);
}
