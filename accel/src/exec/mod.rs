//! Execution Engine — TB cache and vCPU execution loop.
//!
//! Provides the execution loop that drives the
//! lookup -> translate -> execute cycle, with TB caching via
//! a global hash table and per-vCPU jump cache.
//!
//! Reference: `~/qemu/accel/tcg/cpu-exec.c`,
//! `~/qemu/accel/tcg/translate-all.c`.

pub mod exec_loop;
pub mod tb_store;

pub use exec_loop::{cpu_exec_loop_env, ExitReason};
pub use tb_store::TbStore;

use std::cell::UnsafeCell;
use std::fmt;
use std::sync::{Arc, Mutex};

use crate::code_buffer::CodeBuffer;
use crate::ir::tb::JumpCache;
use crate::ir::Context;
use crate::HostCodeGen;

/// Execution statistics for profiling the TB lookup/chain
/// pipeline.
#[derive(Default)]
pub struct ExecStats {
    pub loop_iters: u64,
    // TB lookup
    pub jc_hit: u64,
    pub ht_hit: u64,
    pub translate: u64,
    // Exit types
    pub chain_exit: [u64; 2],
    pub nochain_exit: u64,
    pub real_exit: u64,
    // Chaining
    pub chain_patched: u64,
    pub chain_already: u64,
    // Hint
    pub hint_used: u64,
}

impl fmt::Display for ExecStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total_lookup = self.jc_hit + self.ht_hit + self.translate;
        writeln!(f, "=== Execution Stats ===")?;
        writeln!(f, "loop iters:    {}", self.loop_iters)?;
        writeln!(f, "--- TB lookup ---")?;
        writeln!(
            f,
            "  jc hit:      {} ({:.1}%)",
            self.jc_hit,
            pct(self.jc_hit, total_lookup)
        )?;
        writeln!(
            f,
            "  ht hit:      {} ({:.1}%)",
            self.ht_hit,
            pct(self.ht_hit, total_lookup)
        )?;
        writeln!(
            f,
            "  translate:   {} ({:.1}%)",
            self.translate,
            pct(self.translate, total_lookup)
        )?;
        writeln!(f, "--- Exit types ---")?;
        writeln!(f, "  chain[0]:    {}", self.chain_exit[0])?;
        writeln!(f, "  chain[1]:    {}", self.chain_exit[1])?;
        writeln!(f, "  nochain:     {}", self.nochain_exit)?;
        writeln!(f, "  real exit:   {}", self.real_exit)?;
        writeln!(f, "--- Chaining ---")?;
        writeln!(f, "  patched:     {}", self.chain_patched)?;
        writeln!(f, "  already:     {}", self.chain_already)?;
        writeln!(f, "--- Hint ---")?;
        writeln!(f, "  hint used:   {}", self.hint_used)?;
        Ok(())
    }
}

fn pct(n: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        n as f64 / total as f64 * 100.0
    }
}

/// State protected by translate_lock.
pub struct TranslateGuard {
    pub ir_ctx: Context,
}

/// Shared across all vCPU threads.
pub struct SharedState<B: HostCodeGen> {
    pub tb_store: TbStore,
    /// Code buffer wrapped in UnsafeCell: emit methods need
    /// &mut (under translate_lock), patch/read methods use
    /// &self.
    code_buf: UnsafeCell<CodeBuffer>,
    pub backend: B,
    pub code_gen_start: usize,
    /// Serializes code generation (IR + emit).
    pub translate_lock: Mutex<TranslateGuard>,
    pub atomic_lock: Mutex<()>,
}

// SAFETY: code_buf emit is serialized by translate_lock;
// patch methods are atomic for aligned writes; read methods
// are inherently safe.
unsafe impl<B: HostCodeGen + Send> Send for SharedState<B> {}
unsafe impl<B: HostCodeGen + Sync> Sync for SharedState<B> {}

impl<B: HostCodeGen> SharedState<B> {
    /// Get shared reference to code buffer (for patch/read).
    pub fn code_buf(&self) -> &CodeBuffer {
        // SAFETY: patch/read methods only need &self.
        unsafe { &*self.code_buf.get() }
    }

    /// Get mutable reference to code buffer.
    ///
    /// # Safety
    /// Caller must hold translate_lock.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn code_buf_mut(&self) -> &mut CodeBuffer {
        &mut *self.code_buf.get()
    }
}

/// Per-vCPU state (not shared across threads).
pub struct PerCpuState {
    pub jump_cache: JumpCache,
    pub stats: ExecStats,
}

impl PerCpuState {
    pub fn new() -> Self {
        Self {
            jump_cache: JumpCache::new(),
            stats: ExecStats::default(),
        }
    }
}

impl Default for PerCpuState {
    fn default() -> Self {
        Self::new()
    }
}

/// Minimum remaining bytes in code buffer before refusing
/// to translate a new TB.
pub const MIN_CODE_BUF_REMAINING: usize = 4096;

/// Convenience wrapper for single-threaded use.
pub struct ExecEnv<B: HostCodeGen> {
    pub shared: Arc<SharedState<B>>,
    pub per_cpu: PerCpuState,
}

impl<B: HostCodeGen> ExecEnv<B> {
    pub fn new(mut backend: B) -> Self {
        let mut code_buf =
            CodeBuffer::new(16 * 1024 * 1024).expect("mmap failed");
        backend.emit_prologue(&mut code_buf);
        backend.emit_epilogue(&mut code_buf);
        let code_gen_start = code_buf.offset();

        let mut ir_ctx = Context::new();
        backend.init_context(&mut ir_ctx);

        let shared = Arc::new(SharedState {
            tb_store: TbStore::new(),
            code_buf: UnsafeCell::new(code_buf),
            backend,
            code_gen_start,
            translate_lock: Mutex::new(TranslateGuard { ir_ctx }),
            atomic_lock: Mutex::new(()),
        });

        Self {
            shared,
            per_cpu: PerCpuState {
                jump_cache: JumpCache::new(),
                stats: ExecStats::default(),
            },
        }
    }
}
