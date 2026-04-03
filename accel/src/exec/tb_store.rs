use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::code_buffer::CodeBuffer;
use crate::ir::tb::{TranslationBlock, TB_HASH_SIZE};
use crate::HostCodeGen;

const MAX_TBS: usize = 65536;
/// Max physical pages tracked (1M pages = 4 GB).
const CODE_BITMAP_PAGES: usize = 1 << 20;
const CODE_BITMAP_BYTES: usize = CODE_BITMAP_PAGES / 8;

/// Thread-safe storage and hash-table lookup for TBs.
///
/// Uses `UnsafeCell<Vec>` + `AtomicUsize` for lock-free reads
/// and a `Mutex` for hash table mutations.
///
/// Also maintains a code-page bitmap: bit N is set when
/// at least one valid TB has `phys_pc` on page N.  The
/// store helper checks this bitmap (lock-free Relaxed
/// load) to decide whether a write needs dirty tracking.
pub struct TbStore {
    tbs: UnsafeCell<Vec<TranslationBlock>>,
    len: AtomicUsize,
    hash: Mutex<Vec<Option<usize>>>,
    /// Per-page refcount (0 = no code, >0 = has code TBs).
    /// Index = phys_page = phys_addr >> 12.  Stored as
    /// AtomicU8 for lock-free read from store helpers.
    /// Saturates at 255 (never decrements past that).
    code_pages: Vec<AtomicU8>,
}

// SAFETY:
// - tbs Vec is pre-allocated (no realloc). New entries are
//   appended under translate_lock, then len is published
//   with Release. Readers use Acquire on len.
// - hash is protected by its own Mutex.
unsafe impl Sync for TbStore {}
unsafe impl Send for TbStore {}

impl TbStore {
    pub fn new() -> Self {
        let mut v = Vec::with_capacity(MAX_TBS);
        // Ensure capacity is reserved upfront.
        assert!(v.capacity() >= MAX_TBS);
        v.clear();
        let mut cp = Vec::with_capacity(CODE_BITMAP_BYTES);
        for _ in 0..CODE_BITMAP_BYTES {
            cp.push(AtomicU8::new(0));
        }
        Self {
            tbs: UnsafeCell::new(v),
            len: AtomicUsize::new(0),
            hash: Mutex::new(vec![None; TB_HASH_SIZE]),
            code_pages: cp,
        }
    }

    /// Allocate a new TB. Must be called under translate_lock.
    ///
    /// # Safety
    /// Caller must hold the translate_lock to ensure exclusive
    /// write access to the tbs Vec.
    pub unsafe fn alloc(
        &self,
        pc: u64,
        flags: u32,
        cflags: u32,
    ) -> Option<usize> {
        let tbs = &mut *self.tbs.get();
        let idx = tbs.len();
        if idx >= MAX_TBS {
            return None;
        }
        tbs.push(TranslationBlock::new(pc, flags, cflags));
        // Publish the new length so readers can see it.
        self.len.store(tbs.len(), Ordering::Release);
        Some(idx)
    }

    /// Get a shared reference to a TB by index.
    pub fn get(&self, idx: usize) -> &TranslationBlock {
        let len = self.len.load(Ordering::Acquire);
        assert!(idx < len, "TB index out of bounds");
        // SAFETY: idx < len, and the entry at idx is fully
        // initialized (written before len was published).
        unsafe { &(&*self.tbs.get())[idx] }
    }

    /// Get a mutable reference to a TB by index.
    ///
    /// # Safety
    /// Caller must ensure exclusive access (e.g. under
    /// translate_lock for immutable fields, or per-TB jmp lock
    /// for chaining fields).
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn get_mut(&self, idx: usize) -> &mut TranslationBlock {
        let len = self.len.load(Ordering::Acquire);
        assert!(idx < len, "TB index out of bounds");
        &mut (&mut *self.tbs.get())[idx]
    }

    /// Lookup a valid TB by (pc, flags) in the hash table.
    pub fn lookup(&self, pc: u64, flags: u32) -> Option<usize> {
        let hash = self.hash.lock().unwrap();
        let bucket = TranslationBlock::hash(pc, flags);
        let mut cur = hash[bucket];
        while let Some(idx) = cur {
            let tb = self.get(idx);
            if !tb.invalid.load(Ordering::Acquire)
                && tb.pc == pc
                && tb.flags == flags
            {
                return Some(idx);
            }
            cur = tb.hash_next;
        }
        None
    }

    /// Insert a TB into the hash table (prepend to bucket).
    pub fn insert(&self, tb_idx: usize) {
        let tb = self.get(tb_idx);
        let pc = tb.pc;
        let flags = tb.flags;
        let bucket = TranslationBlock::hash(pc, flags);
        let mut hash = self.hash.lock().unwrap();
        // SAFETY: we need to set hash_next on the TB. This is
        // only called under translate_lock.
        unsafe {
            let tb_mut = self.get_mut(tb_idx);
            tb_mut.hash_next = hash[bucket];
        }
        hash[bucket] = Some(tb_idx);
    }

    /// Mark a TB as invalid, unlink all chained jumps, and
    /// remove it from the hash chain.
    pub fn invalidate<B: HostCodeGen>(
        &self,
        tb_idx: usize,
        code_buf: &CodeBuffer,
        backend: &B,
    ) {
        let tb = self.get(tb_idx);
        tb.invalid.store(true, Ordering::Release);

        // 1. Unlink incoming edges.
        let jmp_list = {
            let mut jmp = tb.jmp.lock().unwrap();
            std::mem::take(&mut jmp.jmp_list)
        };
        for (src, slot) in jmp_list {
            Self::reset_jump(self.get(src), code_buf, backend, slot);
            let src_tb = self.get(src);
            let mut src_jmp = src_tb.jmp.lock().unwrap();
            src_jmp.jmp_dest[slot] = None;
        }

        // 2. Unlink outgoing edges.
        let outgoing = {
            let mut jmp = tb.jmp.lock().unwrap();
            let mut out = [(0usize, 0usize); 2];
            let mut count = 0;
            for slot in 0..2 {
                if let Some(dst) = jmp.jmp_dest[slot].take() {
                    out[count] = (slot, dst);
                    count += 1;
                }
            }
            (out, count)
        };
        let (out, count) = outgoing;
        for &(_slot, dst) in out.iter().take(count) {
            let dst_tb = self.get(dst);
            let mut dst_jmp = dst_tb.jmp.lock().unwrap();
            dst_jmp
                .jmp_list
                .retain(|&(s, n)| !(s == tb_idx && n == _slot));
        }

        // 3. Remove from hash chain.
        let pc = tb.pc;
        let flags = tb.flags;
        let bucket = TranslationBlock::hash(pc, flags);
        let mut hash = self.hash.lock().unwrap();
        let mut prev: Option<usize> = None;
        let mut cur = hash[bucket];
        while let Some(idx) = cur {
            if idx == tb_idx {
                let next = self.get(idx).hash_next;
                if let Some(p) = prev {
                    unsafe {
                        self.get_mut(p).hash_next = next;
                    }
                } else {
                    hash[bucket] = next;
                }
                unsafe {
                    self.get_mut(idx).hash_next = None;
                }
                return;
            }
            prev = cur;
            cur = self.get(idx).hash_next;
        }
    }

    /// Reset a goto_tb jump back to its original target.
    fn reset_jump<B: HostCodeGen>(
        tb: &TranslationBlock,
        code_buf: &CodeBuffer,
        backend: &B,
        slot: usize,
    ) {
        if let (Some(jmp_off), Some(reset_off)) =
            (tb.jmp_insn_offset[slot], tb.jmp_reset_offset[slot])
        {
            backend.patch_jump(code_buf, jmp_off as usize, reset_off as usize);
        }
    }

    /// Invalidate all valid TBs by iterating and calling
    /// `invalidate()` on each. Safe to call from the exec
    /// loop (does not require exclusive access).
    pub fn invalidate_all<B: HostCodeGen>(
        &self,
        code_buf: &CodeBuffer,
        backend: &B,
    ) {
        let len = self.len.load(Ordering::Acquire);
        for i in 0..len {
            let tb = self.get(i);
            if !tb.invalid.load(Ordering::Acquire) {
                self.invalidate(i, code_buf, backend);
            }
        }
        // Clear code-page bitmap: no valid TBs remain.
        for b in &self.code_pages {
            b.store(0, Ordering::Relaxed);
        }
    }

    /// Invalidate all TBs whose phys_pc falls within the
    /// given physical page (page-granularity fence.i).
    pub fn invalidate_phys_page<B: HostCodeGen>(
        &self,
        phys_page: u64,
        code_buf: &CodeBuffer,
        backend: &B,
    ) {
        let len = self.len.load(Ordering::Acquire);
        let mut any = false;
        for i in 0..len {
            let tb = self.get(i);
            if !tb.invalid.load(Ordering::Acquire)
                && (tb.phys_pc >> 12) == phys_page
            {
                self.invalidate(i, code_buf, backend);
                any = true;
            }
        }
        // If we invalidated TBs, rebuild bitmap so the
        // page is unmarked if no valid TBs remain on it.
        if any {
            self.rebuild_code_bitmap();
        }
    }

    /// Flush all TBs and reset the hash table.
    ///
    /// # Safety
    /// Caller must ensure no other threads are accessing TBs.
    pub unsafe fn flush(&self) {
        let tbs = &mut *self.tbs.get();
        tbs.clear();
        self.len.store(0, Ordering::Release);
        self.hash.lock().unwrap().fill(None);
        for b in &self.code_pages {
            b.store(0, Ordering::Relaxed);
        }
    }

    // ── Code-page bitmap ──────────────────────────────

    /// Mark a physical page as containing translated code.
    /// Called after TB allocation with the TB's phys_pc.
    pub fn mark_code_page(&self, phys_page: u64) {
        let idx = (phys_page as usize) / 8;
        let bit = (phys_page as usize) % 8;
        if idx < self.code_pages.len() {
            self.code_pages[idx].fetch_or(1u8 << bit, Ordering::Relaxed);
        }
    }

    /// Check whether a physical page contains code TBs.
    /// Lock-free; safe to call from store helpers.
    pub fn is_code_page(&self, phys_page: u64) -> bool {
        let idx = (phys_page as usize) / 8;
        let bit = (phys_page as usize) % 8;
        if idx < self.code_pages.len() {
            self.code_pages[idx].load(Ordering::Relaxed) & (1u8 << bit) != 0
        } else {
            false
        }
    }

    /// Rebuild the code-page bitmap from all valid TBs.
    /// Called after invalidation to keep the bitmap
    /// conservative (a page stays marked if any valid TB
    /// remains on it).
    fn rebuild_code_bitmap(&self) {
        for b in &self.code_pages {
            b.store(0, Ordering::Relaxed);
        }
        let len = self.len.load(Ordering::Acquire);
        for i in 0..len {
            let tb = self.get(i);
            if !tb.invalid.load(Ordering::Acquire) {
                self.mark_code_page(tb.phys_pc >> 12);
            }
        }
    }

    /// Return a raw pointer to the code_pages array for
    /// embedding in the CPU struct (store helper lookup).
    pub fn code_pages_ptr(&self) -> *const AtomicU8 {
        self.code_pages.as_ptr()
    }

    /// Number of bytes in the code-page bitmap.
    pub fn code_pages_len(&self) -> usize {
        self.code_pages.len()
    }

    pub fn len(&self) -> usize {
        self.len.load(Ordering::Acquire)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for TbStore {
    fn default() -> Self {
        Self::new()
    }
}
