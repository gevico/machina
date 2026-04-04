use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

/// Sentinel value for "no exit target cached".
pub const EXIT_TARGET_NONE: usize = usize::MAX;

/// Mutable chaining state protected by per-TB lock.
pub struct TbJmpState {
    /// Outgoing edge: destination TB index for each slot.
    pub jmp_dest: [Option<usize>; 2],
    /// Incoming edges: (source_tb_idx, slot) pairs.
    pub jmp_list: Vec<(usize, usize)>,
}

impl TbJmpState {
    fn new() -> Self {
        Self {
            jmp_dest: [None; 2],
            jmp_list: Vec::new(),
        }
    }
}

/// A cached translated code block.
///
/// Maps to QEMU's `TranslationBlock`. Represents the mapping
/// from a guest code region to generated host machine code.
///
/// Fields above `jmp` are immutable after creation (set during
/// translation under translate_lock). The `jmp` mutex protects
/// mutable chaining state. `invalid` is atomic for lock-free
/// checking.
pub struct TranslationBlock {
    // -- Immutable after creation --
    pub pc: u64,
    pub cs_base: u64,
    pub flags: u32,
    pub cflags: u32,
    pub size: u32,
    pub icount: u16,
    pub host_offset: usize,
    pub host_size: usize,
    pub jmp_insn_offset: [Option<u32>; 2],
    pub jmp_reset_offset: [Option<u32>; 2],
    pub phys_pc: u64,
    /// Protected by TbStore hash lock.
    pub hash_next: Option<usize>,

    // -- Per-TB lock for chaining state --
    pub jmp: Mutex<TbJmpState>,
    pub contains_atomic: bool,

    // -- Atomic --
    pub invalid: AtomicBool,
    /// Single-entry target cache for indirect exits (atomic,
    /// lock-free). EXIT_TARGET_NONE means no cached target.
    pub exit_target: AtomicUsize,
}

/// Compile flags for TranslationBlock.cflags.
pub mod cflags {
    /// Mask for the instruction count limit (0 = no limit).
    pub const CF_COUNT_MASK: u32 = 0x0000_FFFF;
    /// Last I/O instruction in the TB.
    pub const CF_LAST_IO: u32 = 0x0001_0000;
    /// TB is being single-stepped.
    pub const CF_SINGLE_STEP: u32 = 0x0002_0000;
    /// Use icount (deterministic execution).
    pub const CF_USE_ICOUNT: u32 = 0x0004_0000;
}

impl std::fmt::Debug for TranslationBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TranslationBlock")
            .field("pc", &self.pc)
            .field("flags", &self.flags)
            .field("size", &self.size)
            .field("host_offset", &self.host_offset)
            .field("host_size", &self.host_size)
            .field("invalid", &self.invalid.load(Ordering::Relaxed))
            .finish()
    }
}

impl TranslationBlock {
    pub fn new(pc: u64, flags: u32, cflags: u32) -> Self {
        Self {
            pc,
            cs_base: 0,
            flags,
            cflags,
            size: 0,
            icount: 0,
            host_offset: 0,
            host_size: 0,
            jmp_insn_offset: [None; 2],
            jmp_reset_offset: [None; 2],
            phys_pc: 0,
            hash_next: None,
            jmp: Mutex::new(TbJmpState::new()),
            contains_atomic: false,
            invalid: AtomicBool::new(false),
            exit_target: AtomicUsize::new(EXIT_TARGET_NONE),
        }
    }

    /// Compute hash bucket index for TB lookup.
    pub fn hash(pc: u64, flags: u32) -> usize {
        let h = pc.wrapping_mul(0x9e3779b97f4a7c15) ^ (flags as u64);
        (h as usize) & (TB_HASH_SIZE - 1)
    }

    /// Record the offset of a `goto_tb` jump instruction for exit slot `n`.
    pub fn set_jmp_insn_offset(&mut self, n: usize, offset: u32) {
        assert!(n < 2);
        self.jmp_insn_offset[n] = Some(offset);
    }

    /// Record the reset offset for exit slot `n`.
    pub fn set_jmp_reset_offset(&mut self, n: usize, offset: u32) {
        assert!(n < 2);
        self.jmp_reset_offset[n] = Some(offset);
    }

    /// Maximum number of guest instructions per TB.
    pub fn max_insns(cflags: u32) -> u32 {
        let count = cflags & cflags::CF_COUNT_MASK;
        if count == 0 {
            512
        } else {
            count
        }
    }
}

/// Number of buckets in the global TB hash table.
pub const TB_HASH_SIZE: usize = 1 << 15; // 32768

/// Number of entries in the per-CPU jump cache.
pub const TB_JMP_CACHE_SIZE: usize = 1 << 12; // 4096

/// TB exit value encoding (following QEMU `TB_EXIT_*` convention).
///
/// The low values are reserved for the exec loop's internal TB
/// chaining protocol.  Real guest exits (ECALL, EBREAK, etc.)
/// must use values >= `TB_EXIT_MAX`.
///
/// | Value | Constant | Meaning |
/// |-------|----------|---------|
/// | 0 | `TB_EXIT_IDX0` | `goto_tb` slot 0 — chainable |
/// | 1 | `TB_EXIT_IDX1` | `goto_tb` slot 1 — chainable |
/// | 2 | `TB_EXIT_NOCHAIN` | Indirect jump — look up by PC |
/// | >=3 | `TB_EXIT_MAX`.. | Real exit — returned to caller |
pub const TB_EXIT_IDX0: u64 = 0;
pub const TB_EXIT_IDX1: u64 = 1;
pub const TB_EXIT_NOCHAIN: u64 = 2;
pub const TB_EXIT_MAX: u64 = 3;

/// Guest exception exit codes (must be >= `TB_EXIT_MAX`).
pub const EXCP_ECALL: u64 = TB_EXIT_MAX;
pub const EXCP_EBREAK: u64 = TB_EXIT_MAX + 1;
pub const EXCP_UNDEF: u64 = TB_EXIT_MAX + 2;
pub const EXCP_MRET: u64 = TB_EXIT_MAX + 3;
pub const EXCP_SRET: u64 = TB_EXIT_MAX + 4;
pub const EXCP_WFI: u64 = TB_EXIT_MAX + 5;
pub const EXCP_SFENCE_VMA: u64 = TB_EXIT_MAX + 6;
pub const EXCP_PRIV_CSR: u64 = TB_EXIT_MAX + 7;
pub const EXCP_FENCE_I: u64 = TB_EXIT_MAX + 8;

/// Encode an exit_tb return value with the source TB index.
///
/// For chainable exits (val < `TB_EXIT_MAX`), the upper 32 bits
/// carry `tb_idx + 1` so the exec loop can identify which TB
/// actually exited after direct chaining.  Real exits (val >=
/// `TB_EXIT_MAX`) are returned unchanged.
#[inline]
pub fn encode_tb_exit(tb_idx: u32, val: u64) -> u64 {
    if val < TB_EXIT_MAX {
        ((tb_idx as u64 + 1) << 32) | val
    } else {
        val
    }
}

/// Decode an exit_tb return value.
///
/// Returns `(source_tb_idx, exit_code)`.  For chainable exits
/// `source_tb_idx` is `Some(idx)`; for real exits it is `None`.
#[inline]
pub fn decode_tb_exit(raw: usize) -> (Option<usize>, usize) {
    let marker = raw >> 32;
    if marker != 0 {
        let tb_idx = marker - 1;
        let slot = raw & 3;
        (Some(tb_idx), slot)
    } else {
        (None, raw)
    }
}

/// Per-CPU direct-mapped TB jump cache.
///
/// Indexed by `(pc >> 2) & (TB_JMP_CACHE_SIZE - 1)`.
/// Provides O(1) lookup for the common case of re-executing the same PC.
pub struct JumpCache {
    entries: Box<[Option<usize>; TB_JMP_CACHE_SIZE]>,
}

impl JumpCache {
    pub fn new() -> Self {
        Self {
            entries: Box::new([None; TB_JMP_CACHE_SIZE]),
        }
    }

    fn index(pc: u64) -> usize {
        (pc as usize >> 2) & (TB_JMP_CACHE_SIZE - 1)
    }

    pub fn lookup(&self, pc: u64) -> Option<usize> {
        self.entries[Self::index(pc)]
    }

    pub fn insert(&mut self, pc: u64, tb_idx: usize) {
        self.entries[Self::index(pc)] = Some(tb_idx);
    }

    pub fn remove(&mut self, pc: u64) {
        self.entries[Self::index(pc)] = None;
    }

    pub fn invalidate(&mut self) {
        self.entries.fill(None);
    }
}

impl Default for JumpCache {
    fn default() -> Self {
        Self::new()
    }
}
