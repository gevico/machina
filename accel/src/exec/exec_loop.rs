use std::sync::atomic::Ordering;

// sigjmp_buf for helper exception handling.
// On x86-64 Linux, sigjmp_buf = __jmp_buf_tag[1]
// = 200 bytes (__jmp_buf[8 longs] + int + pad +
// __sigset_t[16 longs]).
#[repr(C, align(8))]
struct SigJmpBuf([u8; 200]);

unsafe extern "C" {
    #[link_name = "__sigsetjmp"]
    fn sigsetjmp(env: *mut SigJmpBuf, savemask: i32)
        -> i32;
}

use super::{ExecEnv, PerCpuState, SharedState, MIN_CODE_BUF_REMAINING};
use crate::cpu::GuestCpu;
use crate::ir::context::Context;
use crate::ir::tb::{
    decode_tb_exit, TranslationBlock, EXCP_ECALL, EXCP_FENCE_I, EXCP_MRET,
    EXCP_PRIV_CSR, EXCP_SFENCE_VMA, EXCP_SRET, EXCP_WFI, EXIT_TARGET_NONE,
    TB_EXIT_NOCHAIN,
};
use crate::translate::translate;
use crate::HostCodeGen;

/// Reason the execution loop exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    /// TB returned a non-zero exit value (EBREAK, UNDEF, etc.).
    Exit(usize),
    /// ECALL exit with the current privilege level.
    Ecall { priv_level: u8 },
    /// WFI: CPU entered halted state.
    Halted,
    /// Code buffer is full; caller should flush and retry.
    BufferFull,
}

/// Convenience wrapper: runs `cpu_exec_loop` using an `ExecEnv`.
///
/// # Safety
/// The caller must ensure `cpu.env_ptr()` points to a valid
/// CPU state struct matching the globals in `ir_ctx`.
pub unsafe fn cpu_exec_loop_env<B, C>(
    env: &mut ExecEnv<B>,
    cpu: &mut C,
) -> ExitReason
where
    B: HostCodeGen,
    C: GuestCpu<IrContext = Context>,
{
    cpu_exec_loop(&env.shared, &mut env.per_cpu, cpu)
}

/// Core CPU execution loop.
///
/// Takes shared state (Arc'd across vCPU threads) and
/// per-vCPU state (owned by each thread). Supports both
/// single-threaded serial and multi-threaded concurrent
/// vCPU execution.
///
/// # Safety
/// The caller must ensure `cpu.env_ptr()` points to a valid
/// CPU state struct matching the globals in `ir_ctx`.
pub unsafe fn cpu_exec_loop<B, C>(
    shared: &SharedState<B>,
    per_cpu: &mut PerCpuState,
    cpu: &mut C,
) -> ExitReason
where
    B: HostCodeGen,
    C: GuestCpu<IrContext = Context>,
{
    let mut next_tb_hint: Option<usize> = None;

    // Set up setjmp context for helper longjmp.
    // Helpers call longjmp(jmp_env, 1) when they need
    // to abort TB execution (e.g. illegal CSR access).
    let mut jmp_env: SigJmpBuf = std::mem::zeroed();
    let jmp_ptr = &mut jmp_env as *mut SigJmpBuf;
    cpu.set_jmp_env(jmp_ptr as u64);

    loop {
        if sigsetjmp(jmp_ptr, 0) != 0 {
            // Helper raised an exception via longjmp.
            next_tb_hint = None;
        }
        per_cpu.stats.loop_iters += 1;

        // Check interrupts BEFORE executing the next TB
        // (matching QEMU's cpu_handle_interrupt).
        if cpu.pending_interrupt() {
            cpu.handle_interrupt();
            cpu.trace_post_interrupt();
            next_tb_hint = None;
        }

        let tb_idx = match next_tb_hint.take() {
            Some(idx) => {
                per_cpu.stats.hint_used += 1;
                idx
            }
            None => {
                let pc = cpu.get_pc();
                let flags = cpu.get_flags();
                match tb_find(shared, per_cpu, cpu, pc, flags) {
                    Some(idx) => idx,
                    None => {
                        // Temporary: log fetch failures
                        // in M-mode handler range.
                        if pc >= 0x80000060
                            && pc < 0x80000100
                        {
                            use std::sync::atomic::{
                                AtomicU64,
                                Ordering as AO,
                            };
                            static FF: AtomicU64 =
                                AtomicU64::new(0);
                            let n = FF.fetch_add(
                                1, AO::Relaxed,
                            );
                            if n < 10 {
                                eprintln!(
                                    "FETCH FAIL pc={:#x} \
                                     flags={:#x} fault={}",
                                    pc, flags,
                                    cpu.check_mem_fault(),
                                );
                                // Already consumed
                                // by check_mem_fault
                                continue;
                            }
                        }
                        // Might be a fetch fault.
                        if cpu.check_mem_fault() {
                            continue;
                        }
                        return ExitReason::BufferFull;
                    }
                }
            }
        };

        let raw_exit = cpu_tb_exec(shared, cpu, tb_idx);
        let (last_tb, exit_code) = decode_tb_exit(raw_exit);

        let src_tb = last_tb.unwrap_or(tb_idx);

        match exit_code {
            v @ 0..=1 => {
                let slot = v;
                per_cpu.stats.chain_exit[slot] += 1;

                if cpu.check_mem_fault() {
                    continue;
                }

                let pc = cpu.get_pc();
                let flags = cpu.get_flags();
                let dst = match tb_find(shared, per_cpu, cpu, pc, flags) {
                    Some(idx) => idx,
                    None => {
                        if cpu.check_mem_fault() {
                            continue;
                        }
                        return ExitReason::BufferFull;
                    }
                };

                tb_add_jump(shared, per_cpu, src_tb, slot, dst);
                next_tb_hint = Some(dst);
            }
            v if v == TB_EXIT_NOCHAIN as usize => {
                per_cpu.stats.nochain_exit += 1;

                // Check for latched memory fault BEFORE
                // looking up the next TB. This ensures
                // fault_pc is used for mepc before any
                // tb_find advances the PC (AC-4).
                if cpu.check_mem_fault() {
                    continue;
                }

                let pc = cpu.get_pc();
                let flags = cpu.get_flags();

                // Check exit_target cache (lock-free atomic).
                let stb = shared.tb_store.get(src_tb);
                let cached = stb.exit_target.load(Ordering::Relaxed);
                if cached != EXIT_TARGET_NONE {
                    let tb = shared.tb_store.get(cached);
                    if !tb.invalid.load(Ordering::Acquire)
                        && tb.pc == pc
                        && tb.flags == flags
                    {
                        if cpu.pending_interrupt() {
                            cpu.handle_interrupt();
                            // Interrupt changed PC; don't
                            // reuse cached TB.
                        } else {
                            next_tb_hint = Some(cached);
                        }
                        continue;
                    }
                }

                let dst = match tb_find(shared, per_cpu, cpu, pc, flags) {
                    Some(idx) => idx,
                    None => {
                        if cpu.check_mem_fault() {
                            continue;
                        }
                        return ExitReason::BufferFull;
                    }
                };
                let stb = shared.tb_store.get(src_tb);
                stb.exit_target.store(dst, Ordering::Relaxed);
                next_tb_hint = Some(dst);
            }
            v if v == EXCP_MRET as usize => {
                per_cpu.stats.real_exit += 1;
                let pre_priv = cpu.privilege_level();
                let pre_mepc = cpu.trace_read_mepc();
                cpu.execute_mret();
                cpu.trace_on_mret_exit(pre_priv, pre_mepc);
            }
            v if v == EXCP_SRET as usize => {
                per_cpu.stats.real_exit += 1;
                let pre_priv = cpu.privilege_level();
                let pre_sepc = cpu.trace_read_sepc();
                if !cpu.execute_sret() {
                    cpu.handle_exception(2, 0);
                } else {
                    cpu.trace_on_sret_exit(pre_priv, pre_sepc);
                }
            }
            v if v == EXCP_SFENCE_VMA as usize => {
                per_cpu.stats.real_exit += 1;
                cpu.tlb_flush();
                cpu.trace_on_sfence();
                per_cpu.jump_cache.invalidate();
                next_tb_hint = None;
            }
            v if v == EXCP_FENCE_I as usize => {
                per_cpu.stats.real_exit += 1;
                // fence.i: invalidate TBs by dirty
                // physical page for instruction cache
                // coherence.
                let dirty = cpu.take_dirty_pages();
                if dirty.is_empty() {
                    // No stores tracked: conservative
                    // full flush as fallback.
                    shared
                        .tb_store
                        .invalidate_all(shared.code_buf(), &shared.backend);
                } else {
                    for page in &dirty {
                        shared.tb_store.invalidate_phys_page(
                            *page,
                            shared.code_buf(),
                            &shared.backend,
                        );
                    }
                }
                per_cpu.jump_cache.invalidate();
                next_tb_hint = None;
            }
            v if v == EXCP_WFI as usize => {
                per_cpu.stats.real_exit += 1;
                cpu.set_halted(true);
                if cpu.pending_interrupt() {
                    cpu.set_halted(false);
                    cpu.handle_interrupt();
                    cpu.trace_post_interrupt();
                } else {
                    let woken = cpu.wait_for_interrupt();
                    cpu.set_halted(false);
                    if !woken {
                        return ExitReason::Halted;
                    }
                    // Check monitor pause after WFI
                    // wake (may have been woken for
                    // pause, not IRQ).
                    if cpu.check_monitor_pause() {
                        return ExitReason::Halted;
                    }
                    // Woken by IRQ or timer. Check for
                    // pending interrupt and handle if any;
                    // otherwise resume (timer expired,
                    // guest will re-read mtime).
                    if cpu.pending_interrupt() {
                        cpu.handle_interrupt();
                        cpu.trace_post_interrupt();
                    }
                }
            }
            v if v == EXCP_PRIV_CSR as usize => {
                per_cpu.stats.real_exit += 1;
                if !cpu.handle_priv_csr() {
                    cpu.handle_exception(2, 0);
                }
                if cpu.take_tb_flush_pending() {
                    shared
                        .tb_store
                        .invalidate_all(shared.code_buf(), &shared.backend);
                    per_cpu.jump_cache.invalidate();
                    next_tb_hint = None;
                }
            }
            v if v == EXCP_ECALL as usize => {
                // The translator emits a unified EXCP_ECALL;
                // the per-privilege exception code (EcallFromU
                // / EcallFromS / EcallFromM) is determined
                // here at runtime, because privilege can
                // change between translation and execution.
                per_cpu.stats.real_exit += 1;
                let pl = cpu.privilege_level();
                return ExitReason::Ecall { priv_level: pl };
            }
            _ => {
                per_cpu.stats.real_exit += 1;
                return ExitReason::Exit(exit_code);
            }
        }

        // Per-TB trace: address space switch, task switch.
        cpu.trace_post_tb();

        // Deliver latched memory faults from JIT helpers.
        // Must precede interrupt check: faults have higher
        // priority and must be delivered precisely.
        // If a fault was delivered, skip interrupt check
        // this iteration to preserve priority.
        if !cpu.check_mem_fault() && cpu.pending_interrupt() {
            cpu.handle_interrupt();
            next_tb_hint = None;
        }

        // External stop check BEFORE monitor pause,
        // so guest shutdown/reset is not blocked by
        // a concurrent monitor stop request.
        if cpu.should_exit() {
            return ExitReason::Halted;
        }

        // Monitor pause check (blocks if paused).
        if cpu.check_monitor_pause() {
            return ExitReason::Halted;
        }
    }
}

/// Find a TB for the given (pc, flags), translating if needed.
fn tb_find<B, C>(
    shared: &SharedState<B>,
    per_cpu: &mut PerCpuState,
    cpu: &mut C,
    pc: u64,
    flags: u32,
) -> Option<usize>
where
    B: HostCodeGen,
    C: GuestCpu<IrContext = Context>,
{
    // Fast path: jump cache (per-CPU, no lock needed)
    if let Some(idx) = per_cpu.jump_cache.lookup(pc) {
        let tb = shared.tb_store.get(idx);
        if !tb.invalid.load(Ordering::Acquire)
            && tb.pc == pc
            && tb.flags == flags
        {
            per_cpu.stats.jc_hit += 1;
            return Some(idx);
        }
    }

    // Slow path: hash table
    if let Some(idx) = shared.tb_store.lookup(pc, flags) {
        per_cpu.jump_cache.insert(pc, idx);
        per_cpu.stats.ht_hit += 1;
        return Some(idx);
    }

    // Miss: translate a new TB
    per_cpu.stats.translate += 1;
    tb_gen_code(shared, per_cpu, cpu, pc, flags)
}

/// Translate guest code at `pc` into a new TB.
fn tb_gen_code<B, C>(
    shared: &SharedState<B>,
    per_cpu: &mut PerCpuState,
    cpu: &mut C,
    pc: u64,
    flags: u32,
) -> Option<usize>
where
    B: HostCodeGen,
    C: GuestCpu<IrContext = Context>,
{
    if shared.code_buf().remaining() < MIN_CODE_BUF_REMAINING {
        return None;
    }

    // Acquire translate_lock for exclusive code generation.
    let mut guard = shared.translate_lock.lock().unwrap();

    // Double-check: another thread may have translated this
    // PC while we waited for the lock.
    if let Some(idx) = shared.tb_store.lookup(pc, flags) {
        per_cpu.jump_cache.insert(pc, idx);
        return Some(idx);
    }

    // SAFETY: we hold translate_lock, so exclusive access to
    // tbs Vec and code_buf emit methods.
    let tb_idx = unsafe { shared.tb_store.alloc(pc, flags, 0) };

    guard.ir_ctx.reset();
    guard.ir_ctx.tb_idx = tb_idx as u32;
    let guest_size =
        cpu.gen_code(&mut guard.ir_ctx, pc, TranslationBlock::max_insns(0));
    if guest_size == 0 {
        // Fetch fault or PC outside RAM. The fault is
        // latched in mem_fault_cause. Mark TB invalid
        // so the exec loop re-checks.
        unsafe {
            let tb = shared.tb_store.get_mut(tb_idx);
            tb.invalid.store(true, Ordering::Release);
        }
        return None;
    }
    unsafe {
        let tb = shared.tb_store.get_mut(tb_idx);
        tb.size = guest_size;
        tb.phys_pc = cpu.last_phys_pc();
    }

    shared.backend.clear_goto_tb_offsets();

    // SAFETY: translate_lock guarantees exclusive access to
    // code_buf's write cursor.
    let code_buf_mut = unsafe { shared.code_buf_mut() };
    let host_offset =
        translate(&mut guard.ir_ctx, &shared.backend, code_buf_mut);
    let host_size = shared.code_buf().offset() - host_offset;

    // SAFETY: under translate_lock.
    unsafe {
        let tb = shared.tb_store.get_mut(tb_idx);
        tb.host_offset = host_offset;
        tb.host_size = host_size;
    }

    let offsets = shared.backend.goto_tb_offsets();
    unsafe {
        let tb = shared.tb_store.get_mut(tb_idx);
        for (i, &(jmp, reset)) in offsets.iter().enumerate().take(2) {
            tb.set_jmp_insn_offset(i, jmp as u32);
            tb.set_jmp_reset_offset(i, reset as u32);
        }
    }

    shared.tb_store.insert(tb_idx);
    per_cpu.jump_cache.insert(pc, tb_idx);

    Some(tb_idx)
}

/// Execute a single TB and return the exit value.
unsafe fn cpu_tb_exec<B, C>(
    shared: &SharedState<B>,
    cpu: &mut C,
    tb_idx: usize,
) -> usize
where
    B: HostCodeGen,
    C: GuestCpu<IrContext = Context>,
{
    let tb = shared.tb_store.get(tb_idx);
    let tb_ptr = shared.code_buf().ptr_at(tb.host_offset);
    let env_ptr = cpu.env_ptr();

    let prologue_fn: unsafe extern "C" fn(*mut u8, *const u8) -> usize =
        core::mem::transmute(shared.code_buf().base_ptr());
    prologue_fn(env_ptr, tb_ptr)
}

/// Patch a goto_tb jump to directly chain src -> dst.
///
/// Lock ordering: always lock src first, then dst, to
/// prevent deadlocks.
fn tb_add_jump<B: HostCodeGen>(
    shared: &SharedState<B>,
    per_cpu: &mut PerCpuState,
    src: usize,
    slot: usize,
    dst: usize,
) {
    let src_tb = shared.tb_store.get(src);
    let jmp_off = match src_tb.jmp_insn_offset[slot] {
        Some(off) => off as usize,
        None => return,
    };

    if shared.tb_store.get(dst).invalid.load(Ordering::Acquire) {
        return;
    }

    // Lock src TB's jmp state.
    let mut src_jmp = src_tb.jmp.lock().unwrap();

    if src_jmp.jmp_dest[slot] == Some(dst) {
        per_cpu.stats.chain_already += 1;
        return;
    }

    let abs_dst = shared.tb_store.get(dst).host_offset;
    shared
        .backend
        .patch_jump(shared.code_buf(), jmp_off, abs_dst);

    src_jmp.jmp_dest[slot] = Some(dst);
    drop(src_jmp);

    // Lock dst TB's jmp state to add incoming edge.
    let dst_tb = shared.tb_store.get(dst);
    let mut dst_jmp = dst_tb.jmp.lock().unwrap();
    dst_jmp.jmp_list.push((src, slot));

    per_cpu.stats.chain_patched += 1;
}
