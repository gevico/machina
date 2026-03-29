use std::sync::atomic::Ordering;

use crate::{
    ExecEnv, GuestCpu, PerCpuState, SharedState, MIN_CODE_BUF_REMAINING,
};
use machina_backend::translate::translate;
use machina_backend::HostCodeGen;
use machina_core::tb::{decode_tb_exit, EXIT_TARGET_NONE, TB_EXIT_NOCHAIN};

/// Reason the execution loop exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    /// TB returned a non-zero exit value.
    Exit(usize),
    /// Code buffer is full; caller should flush and retry.
    BufferFull,
}

/// Main CPU execution loop (single-threaded convenience).
///
/// # Safety
/// The caller must ensure `cpu.env_ptr()` points to a valid
/// CPU state struct matching the globals in `ir_ctx`.
pub unsafe fn cpu_exec_loop<B, C>(
    env: &mut ExecEnv<B>,
    cpu: &mut C,
) -> ExitReason
where
    B: HostCodeGen,
    C: GuestCpu,
{
    cpu_exec_loop_mt(&env.shared, &mut env.per_cpu, cpu)
}

/// Multi-thread capable execution loop.
///
/// Takes shared state (Arc'd across vCPU threads) and
/// per-CPU state (owned by each thread).
///
/// # Safety
/// The caller must ensure `cpu.env_ptr()` points to a valid
/// CPU state struct matching the globals in `ir_ctx`.
pub unsafe fn cpu_exec_loop_mt<B, C>(
    shared: &SharedState<B>,
    per_cpu: &mut PerCpuState,
    cpu: &mut C,
) -> ExitReason
where
    B: HostCodeGen,
    C: GuestCpu,
{
    let mut next_tb_hint: Option<usize> = None;

    loop {
        per_cpu.stats.loop_iters += 1;

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
                    None => return ExitReason::BufferFull,
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

                let pc = cpu.get_pc();
                let flags = cpu.get_flags();
                let dst = match tb_find(shared, per_cpu, cpu, pc, flags) {
                    Some(idx) => idx,
                    None => return ExitReason::BufferFull,
                };

                tb_add_jump(shared, per_cpu, src_tb, slot, dst);
                next_tb_hint = Some(dst);
            }
            v if v == TB_EXIT_NOCHAIN as usize => {
                per_cpu.stats.nochain_exit += 1;
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
                        next_tb_hint = Some(cached);
                        continue;
                    }
                }

                let dst = match tb_find(shared, per_cpu, cpu, pc, flags) {
                    Some(idx) => idx,
                    None => return ExitReason::BufferFull,
                };
                let stb = shared.tb_store.get(src_tb);
                stb.exit_target.store(dst, Ordering::Relaxed);
                next_tb_hint = Some(dst);
            }
            _ => {
                per_cpu.stats.real_exit += 1;
                return ExitReason::Exit(exit_code);
            }
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
    C: GuestCpu,
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
    C: GuestCpu,
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
    let guest_size = cpu.gen_code(
        &mut guard.ir_ctx,
        pc,
        machina_core::tb::TranslationBlock::max_insns(0),
    );
    unsafe {
        shared.tb_store.get_mut(tb_idx).size = guest_size;
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
    C: GuestCpu,
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
