use memmap2::MmapMut;

/// Anonymous memory allocation backed by the OS.
///
/// On Unix uses `mmap(MAP_PRIVATE | MAP_ANONYMOUS)`.
/// On Windows uses `VirtualAlloc`.
/// Both paths are handled transparently by `memmap2`.
pub struct RamBlock {
    mmap: MmapMut,
}

impl RamBlock {
    pub fn new(size: u64) -> Self {
        let mmap = MmapMut::map_anon(size as usize).unwrap_or_else(|e| {
            panic!("failed to allocate {size} bytes of RAM: {e}")
        });
        Self { mmap }
    }

    pub fn as_ptr(&self) -> *mut u8 {
        self.mmap.as_ptr() as *mut u8
    }

    pub fn size(&self) -> u64 {
        self.mmap.len() as u64
    }
}

// SAFETY: The anonymous mapping is exclusively owned by this
// RamBlock; no aliasing pointers exist outside it.
unsafe impl Send for RamBlock {}
unsafe impl Sync for RamBlock {}
