use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use machina_backend::code_buffer::CodeBuffer;
use machina_backend::HostCodeGen;
use machina_core::tb::{TranslationBlock, TB_HASH_SIZE};

const MAX_TBS: usize = 65536;

/// Thread-safe storage and hash-table lookup for TBs.
///
/// Uses `UnsafeCell<Vec>` + `AtomicUsize` for lock-free reads
/// and a `Mutex` for hash table mutations.
pub struct TbStore {
    tbs: UnsafeCell<Vec<TranslationBlock>>,
    len: AtomicUsize,
    hash: Mutex<Vec<Option<usize>>>,
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
        Self {
            tbs: UnsafeCell::new(v),
            len: AtomicUsize::new(0),
            hash: Mutex::new(vec![None; TB_HASH_SIZE]),
        }
    }

    /// Allocate a new TB. Must be called under translate_lock.
    ///
    /// # Safety
    /// Caller must hold the translate_lock to ensure exclusive
    /// write access to the tbs Vec.
    pub unsafe fn alloc(&self, pc: u64, flags: u32, cflags: u32) -> usize {
        let tbs = &mut *self.tbs.get();
        let idx = tbs.len();
        assert!(idx < MAX_TBS, "TB store full");
        tbs.push(TranslationBlock::new(pc, flags, cflags));
        // Publish the new length so readers can see it.
        self.len.store(tbs.len(), Ordering::Release);
        idx
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

    /// Flush all TBs and reset the hash table.
    ///
    /// # Safety
    /// Caller must ensure no other threads are accessing TBs.
    pub unsafe fn flush(&self) {
        let tbs = &mut *self.tbs.get();
        tbs.clear();
        self.len.store(0, Ordering::Release);
        self.hash.lock().unwrap().fill(None);
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
