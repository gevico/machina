use crate::flat_view::{FlatRangeKind, FlatView};
use crate::region::MemoryRegion;

/// Top-level address space built from a `MemoryRegion` tree.
///
/// Holds the root region and a cached `FlatView` for fast
/// dispatch.  Call `update_flat_view` after modifying the
/// tree to rebuild the cache.
pub struct AddressSpace {
    root: MemoryRegion,
    flat_view: FlatView,
}

impl AddressSpace {
    pub fn new(root: MemoryRegion) -> Self {
        let flat_view = FlatView::from_region(&root);
        Self { root, flat_view }
    }

    /// Rebuild the flat view after the region tree changes.
    pub fn update_flat_view(&mut self) {
        self.flat_view = FlatView::from_region(&self.root);
    }

    /// Mutable access to the root region (e.g. to add/remove
    /// subregions).  Caller must call `update_flat_view`
    /// afterwards.
    pub fn root_mut(&mut self) -> &mut MemoryRegion {
        &mut self.root
    }

    // ----- bulk read / write -----

    pub fn read(&self, addr: u64, buf: &mut [u8]) {
        let len = buf.len() as u64;
        let mut offset = 0u64;

        while offset < len {
            let cur = addr + offset;
            let fr = self
                .flat_view
                .lookup(cur)
                .unwrap_or_else(|| panic!("unmapped read at {cur:#x}"));

            let into_range = cur - fr.addr;
            let avail = fr.size - into_range;
            let chunk = avail.min(len - offset) as usize;
            let region_off = fr.offset_in_region + into_range;

            match &fr.kind {
                FlatRangeKind::Ram { block } => {
                    // SAFETY: region_off + chunk is within
                    // the mmap'd allocation.
                    let src =
                        unsafe { block.as_ptr().add(region_off as usize) };
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            src,
                            buf.as_mut_ptr().add(offset as usize),
                            chunk,
                        );
                    }
                }
                FlatRangeKind::Io { ops } => {
                    let ops = ops.lock().unwrap();
                    let mut pos = 0usize;
                    while pos < chunk {
                        let sz = Self::access_size(chunk - pos);
                        let val = ops.read(region_off + pos as u64, sz);
                        let dst = &mut buf[(offset as usize + pos)..];
                        dst[..sz as usize]
                            .copy_from_slice(&val.to_le_bytes()[..sz as usize]);
                        pos += sz as usize;
                    }
                }
            }
            offset += chunk as u64;
        }
    }

    pub fn write(&self, addr: u64, buf: &[u8]) {
        let len = buf.len() as u64;
        let mut offset = 0u64;

        while offset < len {
            let cur = addr + offset;
            let fr = self
                .flat_view
                .lookup(cur)
                .unwrap_or_else(|| panic!("unmapped write at {cur:#x}"));

            let into_range = cur - fr.addr;
            let avail = fr.size - into_range;
            let chunk = avail.min(len - offset) as usize;
            let region_off = fr.offset_in_region + into_range;

            match &fr.kind {
                FlatRangeKind::Ram { block } => {
                    // SAFETY: region_off + chunk is within
                    // the mmap'd allocation.
                    let dst =
                        unsafe { block.as_ptr().add(region_off as usize) };
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            buf.as_ptr().add(offset as usize),
                            dst,
                            chunk,
                        );
                    }
                }
                FlatRangeKind::Io { ops } => {
                    let ops = ops.lock().unwrap();
                    let mut pos = 0usize;
                    while pos < chunk {
                        let sz = Self::access_size(chunk - pos);
                        let mut bytes = [0u8; 8];
                        let src = &buf[(offset as usize + pos)..];
                        bytes[..sz as usize]
                            .copy_from_slice(&src[..sz as usize]);
                        let val = u64::from_le_bytes(bytes);
                        ops.write(region_off + pos as u64, sz, val);
                        pos += sz as usize;
                    }
                }
            }
            offset += chunk as u64;
        }
    }

    // ----- convenience accessors -----

    pub fn read_u32(&self, addr: u64) -> u32 {
        let mut buf = [0u8; 4];
        self.read(addr, &mut buf);
        u32::from_le_bytes(buf)
    }

    pub fn write_u32(&self, addr: u64, val: u32) {
        self.write(addr, &val.to_le_bytes());
    }

    // ----- helpers -----

    /// Pick the largest power-of-two access size that fits
    /// within `remaining` bytes, capped at 8.
    fn access_size(remaining: usize) -> u32 {
        if remaining >= 8 {
            8
        } else if remaining >= 4 {
            4
        } else if remaining >= 2 {
            2
        } else {
            1
        }
    }
}
