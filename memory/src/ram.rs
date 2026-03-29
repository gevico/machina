use std::ptr;

/// mmap-backed host memory allocation.
///
/// Owns a contiguous region of anonymous memory obtained via
/// `mmap(MAP_PRIVATE | MAP_ANONYMOUS)`.  Freed on drop.
pub struct RamBlock {
    ptr: *mut u8,
    size: u64,
}

impl RamBlock {
    pub fn new(size: u64) -> Self {
        // SAFETY: mmap with MAP_ANONYMOUS returns a fresh
        // zero-filled mapping or MAP_FAILED.
        let ptr = unsafe {
            libc::mmap(
                ptr::null_mut(),
                size as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            ) as *mut u8
        };
        assert!(
            !ptr.is_null() && ptr != libc::MAP_FAILED as *mut u8,
            "mmap failed for size {size}"
        );
        Self { ptr, size }
    }

    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr
    }

    pub fn size(&self) -> u64 {
        self.size
    }
}

// SAFETY: The mmap'd memory is exclusively owned by this
// RamBlock instance; no aliasing pointers exist.
unsafe impl Send for RamBlock {}
unsafe impl Sync for RamBlock {}

impl Drop for RamBlock {
    fn drop(&mut self) {
        // SAFETY: ptr/size were produced by a successful mmap
        // in `new` and have not been modified since.
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, self.size as usize);
        }
    }
}
