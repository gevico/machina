use std::io;
use std::ptr;

use crate::plat;

/// Default code buffer size: 16 MiB.
const DEFAULT_CODE_BUF_SIZE: usize = 16 * 1024 * 1024;

/// Safety margin below the code buffer ceiling.  Any
/// single guest instruction's host code must fit within
/// this amount.  Matches QEMU's HIGHWATER concept.
const HIGHWATER_MARGIN: usize = 1024;

/// JIT code buffer backed by OS-managed executable memory.
///
/// Manages a region of memory for writing and executing
/// generated host code.  Includes a *highwater* check:
/// when the write cursor passes `size - HIGHWATER_MARGIN`,
/// the current translation is aborted via longjmp and
/// retried with fewer guest instructions (QEMU's
/// `tcg_raise_tb_overflow` equivalent).
pub struct CodeBuffer {
    ptr: *mut u8,
    size: usize,
    offset: usize,
    /// Pointer to a `JmpBuf` set by the execution loop.
    /// Non-null only during active translation.  When
    /// the highwater mark is exceeded, we longjmp here
    /// with value -2.
    pub(crate) jmp_trans: *mut u8,
}

// SAFETY: CodeBuffer owns its memory exclusively.
// - emit_* methods require &mut self, serialized by translate_lock.
// - patch_* methods use &self; aligned u32 writes are atomic.
// - read methods (ptr_at, base_ptr) are inherently safe.
unsafe impl Send for CodeBuffer {}
unsafe impl Sync for CodeBuffer {}

impl CodeBuffer {
    /// Allocate a new code buffer of the given size
    /// (rounded up to page size).
    pub fn new(size: usize) -> io::Result<Self> {
        let page = plat::page_size();
        let size = (size + page - 1) & !(page - 1);
        if size == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "code buffer size must be non-zero",
            ));
        }
        // SAFETY: size is non-zero and page-aligned.
        let ptr = unsafe { plat::alloc_rwx(size)? };
        Ok(Self {
            ptr,
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

    /// Set the write offset (e.g. to resume writing at a
    /// saved position).
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
            // SAFETY: jmp_trans was set by the exec loop's
            // do_setjmp call and the frame is still live.
            unsafe { plat::do_longjmp(self.jmp_trans as *mut plat::JmpBuf, -2) }
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

    /// Patch a u8 at the given offset (for back-patching
    /// jumps).
    #[inline]
    pub fn patch_u8(&self, offset: usize, val: u8) {
        assert!(offset < self.size);
        unsafe { self.ptr.add(offset).write(val) };
    }

    /// Patch a u32 at the given offset.
    ///
    /// For 4-byte aligned addresses, uses an atomic store
    /// so concurrent readers (executing JIT code) see a
    /// consistent value.  Unaligned writes use a plain
    /// store (caller must ensure no concurrent readers for
    /// unaligned patches).
    #[inline]
    pub fn patch_u32(&self, offset: usize, val: u32) {
        assert!(offset + 4 <= self.size);
        let ptr = unsafe { self.ptr.add(offset) };
        if (ptr as usize).is_multiple_of(4) {
            use std::sync::atomic::{AtomicU32, Ordering};
            // SAFETY: ptr is within our mapped region and
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
        plat::set_rx(self.ptr, self.size)
    }

    /// Make the buffer writable and non-executable.
    pub fn set_writable(&self) -> io::Result<()> {
        plat::set_rw(self.ptr, self.size)
    }

    /// Get the generated code as a byte slice (up to
    /// current offset).
    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: ptr..ptr+offset has been written.
        unsafe { std::slice::from_raw_parts(self.ptr, self.offset) }
    }
}

impl Drop for CodeBuffer {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: ptr/size were produced by alloc_rwx
            // in `new` and have not been modified since.
            unsafe { plat::free_rwx(self.ptr, self.size) }
        }
    }
}
