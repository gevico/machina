use std::io;
use std::ptr;

/// Default code buffer size: 16 MiB.
const DEFAULT_CODE_BUF_SIZE: usize = 16 * 1024 * 1024;

/// Safety margin below the code buffer ceiling.  Any
/// single guest instruction's host code must fit within
/// this amount.  Matches QEMU's HIGHWATER concept.
const HIGHWATER_MARGIN: usize = 1024;

/// JIT code buffer backed by mmap'd memory.
///
/// Manages a region of memory for writing and executing
/// generated host code.  Includes a *highwater* check:
/// when the write cursor passes `size - HIGHWATER_MARGIN`,
/// the current translation is aborted via `siglongjmp` and
/// retried with fewer guest instructions (QEMU's
/// `tcg_raise_tb_overflow` equivalent).
pub struct CodeBuffer {
    ptr: *mut u8,
    size: usize,
    offset: usize,
    /// Pointer to a `sigjmp_buf` set by `tb_gen_code`.
    /// Non-null only during active translation.  When
    /// the highwater mark is exceeded, we `siglongjmp`
    /// here with value -2.
    pub(crate) jmp_trans: *mut u8,
}

// SAFETY: CodeBuffer owns its mmap'd memory exclusively.
// - emit_* methods require &mut self, serialized by translate_lock.
// - patch_* methods use &self; aligned u32 writes are atomic.
// - read methods (ptr_at, base_ptr) are inherently safe.
unsafe impl Send for CodeBuffer {}
unsafe impl Sync for CodeBuffer {}

impl CodeBuffer {
    /// Allocate a new code buffer of the given size (rounded up to page size).
    pub fn new(size: usize) -> io::Result<Self> {
        let page_size = page_size();
        let size = (size + page_size - 1) & !(page_size - 1);
        if size == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "code buffer size must be non-zero",
            ));
        }

        // SAFETY: mmap with MAP_ANONYMOUS | MAP_PRIVATE, no file backing.
        // Use RWX so the exec loop can patch goto_tb jumps at runtime
        // without mprotect round-trips (matches QEMU non-split-wx).
        let ptr = unsafe {
            libc::mmap(
                ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };

        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }

        Ok(Self {
            ptr: ptr as *mut u8,
            size,
            offset: 0,
            jmp_trans: ptr::null_mut(),
        })
    }

    /// Allocate with the default size (16 MiB).
    pub fn with_default_size() -> io::Result<Self> {
        Self::new(DEFAULT_CODE_BUF_SIZE)
    }

    /// Current write offset.
    #[inline]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Total capacity in bytes.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.size
    }

    /// Remaining writable bytes.
    #[inline]
    pub fn remaining(&self) -> usize {
        self.size - self.offset
    }

    /// Raw pointer to the start of the buffer.
    #[inline]
    pub fn base_ptr(&self) -> *const u8 {
        self.ptr as *const u8
    }

    /// Pointer to the current write position.
    #[inline]
    pub fn current_ptr(&self) -> *const u8 {
        // SAFETY: offset is always <= size.
        unsafe { self.ptr.add(self.offset) as *const u8 }
    }

    /// Pointer at a given offset.
    #[inline]
    pub fn ptr_at(&self, offset: usize) -> *const u8 {
        assert!(offset <= self.size);
        unsafe { self.ptr.add(offset) as *const u8 }
    }

    /// Set the write offset (e.g. to resume writing at a saved position).
    #[inline]
    pub fn set_offset(&mut self, offset: usize) {
        assert!(offset <= self.size);
        self.offset = offset;
    }

    /// Check whether the write cursor has passed the
    /// highwater mark.  Called after emitting each guest
    /// instruction's host code.  If exceeded and
    /// `jmp_trans` is set, longjmps back to tb_gen_code
    /// which retries with fewer instructions.
    #[inline]
    pub fn check_highwater(&self) {
        if self.offset + HIGHWATER_MARGIN > self.size
            && !self.jmp_trans.is_null()
        {
            unsafe {
                extern "C" {
                    #[link_name = "siglongjmp"]
                    fn siglongjmp(env: *mut u8, val: i32) -> !;
                }
                siglongjmp(self.jmp_trans, -2);
            }
        }
    }

    // -- Emit methods --

    #[inline]
    pub fn emit_u8(&mut self, val: u8) {
        debug_assert!(self.offset < self.size, "code buffer overflow");
        unsafe { self.ptr.add(self.offset).write(val) };
        self.offset += 1;
    }

    #[inline]
    pub fn emit_u16(&mut self, val: u16) {
        debug_assert!(self.offset + 2 <= self.size, "code buffer overflow");
        unsafe { (self.ptr.add(self.offset) as *mut u16).write_unaligned(val) };
        self.offset += 2;
    }

    #[inline]
    pub fn emit_u32(&mut self, val: u32) {
        debug_assert!(self.offset + 4 <= self.size, "code buffer overflow");
        unsafe { (self.ptr.add(self.offset) as *mut u32).write_unaligned(val) };
        self.offset += 4;
    }

    #[inline]
    pub fn emit_u64(&mut self, val: u64) {
        debug_assert!(self.offset + 8 <= self.size, "code buffer overflow");
        unsafe { (self.ptr.add(self.offset) as *mut u64).write_unaligned(val) };
        self.offset += 8;
    }

    #[inline]
    pub fn emit_bytes(&mut self, data: &[u8]) {
        assert!(
            self.offset + data.len() <= self.size,
            "code buffer overflow"
        );
        unsafe {
            ptr::copy_nonoverlapping(
                data.as_ptr(),
                self.ptr.add(self.offset),
                data.len(),
            );
        }
        self.offset += data.len();
    }

    /// Patch a u8 at the given offset (for back-patching jumps).
    #[inline]
    pub fn patch_u8(&self, offset: usize, val: u8) {
        assert!(offset < self.size);
        unsafe { self.ptr.add(offset).write(val) };
    }

    /// Patch a u32 at the given offset.
    ///
    /// For 4-byte aligned addresses, uses an atomic store so
    /// concurrent readers (executing JIT code) see a consistent
    /// value. Unaligned writes use a plain store (caller must
    /// ensure no concurrent readers for unaligned patches).
    #[inline]
    pub fn patch_u32(&self, offset: usize, val: u32) {
        assert!(offset + 4 <= self.size);
        let ptr = unsafe { self.ptr.add(offset) };
        if (ptr as usize).is_multiple_of(4) {
            use std::sync::atomic::{AtomicU32, Ordering};
            // SAFETY: ptr is within our mmap'd region and
            // 4-byte aligned.
            let atomic = unsafe { &*(ptr as *const AtomicU32) };
            atomic.store(val, Ordering::Release);
        } else {
            unsafe { (ptr as *mut u32).write_unaligned(val) };
        }
    }

    /// Read a u32 at the given offset.
    #[inline]
    pub fn read_u32(&self, offset: usize) -> u32 {
        assert!(offset + 4 <= self.size);
        unsafe { (self.ptr.add(offset) as *const u32).read_unaligned() }
    }

    // -- Permission management (W^X) --

    /// Make the buffer executable and non-writable.
    pub fn set_executable(&self) -> io::Result<()> {
        let ret = unsafe {
            libc::mprotect(
                self.ptr as *mut libc::c_void,
                self.size,
                libc::PROT_READ | libc::PROT_EXEC,
            )
        };
        if ret != 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Make the buffer writable and non-executable.
    pub fn set_writable(&self) -> io::Result<()> {
        let ret = unsafe {
            libc::mprotect(
                self.ptr as *mut libc::c_void,
                self.size,
                libc::PROT_READ | libc::PROT_WRITE,
            )
        };
        if ret != 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Get the generated code as a byte slice (up to current offset).
    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: ptr..ptr+offset has been written.
        unsafe { std::slice::from_raw_parts(self.ptr, self.offset) }
    }
}

impl Drop for CodeBuffer {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                libc::munmap(self.ptr as *mut libc::c_void, self.size);
            }
        }
    }
}

fn page_size() -> usize {
    // SAFETY: sysconf is always safe to call.
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if size <= 0 {
        4096
    } else {
        size as usize
    }
}
