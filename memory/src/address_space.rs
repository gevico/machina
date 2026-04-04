use std::sync::RwLock;

use machina_core::address::GPA;

use crate::flat_view::{FlatRangeKind, FlatView};
use crate::region::MemoryRegion;

/// Top-level address space built from a `MemoryRegion` tree.
///
/// Holds the root region and a cached `FlatView` for fast
/// dispatch.  Call `update_flat_view` after modifying the
/// tree to rebuild the cache.
pub struct AddressSpace {
    root: MemoryRegion,
    flat_view: RwLock<FlatView>,
}

impl AddressSpace {
    pub fn new(root: MemoryRegion) -> Self {
        let flat_view = FlatView::from_region(&root);
        Self {
            root,
            flat_view: RwLock::new(flat_view),
        }
    }

    /// Rebuild the flat view after the region tree changes.
    pub fn update_flat_view(&mut self) {
        let fv = FlatView::from_region(&self.root);
        *self.flat_view.write().unwrap() = fv;
    }

    /// Mutable access to the root region (e.g. to add/remove
    /// subregions).  Caller must call `update_flat_view`
    /// afterwards.
    pub fn root_mut(&mut self) -> &mut MemoryRegion {
        &mut self.root
    }

    /// Remove one direct child region from the root memory
    /// container and rebuild the flat view if successful.
    pub fn remove_subregion(
        &mut self,
        offset: GPA,
        name: &str,
    ) -> Option<MemoryRegion> {
        let removed = self.root.remove_subregion(offset, name);
        if removed.is_some() {
            self.update_flat_view();
        }
        removed
    }

    // ----- sized read / write -----

    /// Read 1/2/4/8 bytes from guest physical address `addr`.
    /// Returns the value zero-extended to `u64`.
    pub fn read(&self, addr: GPA, size: u32) -> u64 {
        debug_assert!(
            matches!(size, 1 | 2 | 4 | 8),
            "unsupported read size {size}"
        );
        let fv = self.flat_view.read().unwrap();
        let fr = match fv.lookup(addr) {
            Some(fr) => fr,
            None => return 0, // Unmapped read returns 0.
        };

        let region_off = fr.offset_in_region + (addr.0 - fr.addr.0);

        match &fr.kind {
            FlatRangeKind::Ram { block } | FlatRangeKind::Rom { block } => {
                let mut buf = [0u8; 8];
                // SAFETY: region_off + size is within the
                // mmap'd allocation.
                let src = unsafe { block.as_ptr().add(region_off as usize) };
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        src,
                        buf.as_mut_ptr(),
                        size as usize,
                    );
                }
                u64::from_le_bytes(buf)
            }
            FlatRangeKind::Io { ops } => {
                let ops = ops.lock().unwrap();
                ops.read(region_off, size)
            }
        }
    }

    /// Write 1/2/4/8 bytes to guest physical address `addr`.
    /// Writes to ROM regions are silently dropped.
    pub fn write(&self, addr: GPA, size: u32, val: u64) {
        debug_assert!(
            matches!(size, 1 | 2 | 4 | 8),
            "unsupported write size {size}"
        );
        let fv = self.flat_view.read().unwrap();
        let fr = match fv.lookup(addr) {
            Some(fr) => fr,
            None => return, // Silently drop unmapped writes.
        };

        let region_off = fr.offset_in_region + (addr.0 - fr.addr.0);

        match &fr.kind {
            FlatRangeKind::Ram { block } => {
                let bytes = val.to_le_bytes();
                // SAFETY: region_off + size is within the
                // mmap'd allocation.
                let dst = unsafe { block.as_ptr().add(region_off as usize) };
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        bytes.as_ptr(),
                        dst,
                        size as usize,
                    );
                }
            }
            FlatRangeKind::Rom { .. } => {
                // Writes to ROM are silently dropped.
            }
            FlatRangeKind::Io { ops } => {
                let ops = ops.lock().unwrap();
                ops.write(region_off, size, val);
            }
        }
    }

    /// Return whether the full byte range `[addr, addr + size)`
    /// is mapped by some region.
    pub fn is_mapped(&self, addr: GPA, size: u32) -> bool {
        debug_assert!(
            matches!(size, 1 | 2 | 4 | 8),
            "unsupported access size {size}"
        );
        let fv = self.flat_view.read().unwrap();
        let Some(fr) = fv.lookup(addr) else {
            return false;
        };
        addr.0
            .checked_add(size as u64)
            .is_some_and(|end| end <= fr.addr.0 + fr.size)
    }

    // ----- convenience accessors -----

    pub fn read_u32(&self, addr: GPA) -> u32 {
        self.read(addr, 4) as u32
    }

    pub fn write_u32(&self, addr: GPA, val: u32) {
        self.write(addr, 4, val as u64);
    }
}
