use std::sync::atomic::Ordering;

// sigjmp_buf for helper exception handling.
// On x86-64 Linux, sigjmp_buf = __jmp_buf_tag[1]
// = 200 bytes (__jmp_buf[8 longs] + int + pad +
// __sigset_t[16 longs]).
#[repr(C, align(8))]
struct SigJmpBuf([u8; 200]);

unsafe extern "C" {
    #[link_name = "__sigsetjmp"]
    fn sigsetjmp(env: *mut SigJmpBuf, savemask: i32) -> i32;
}

use super::{ExecEnv, PerCpuState, SharedState, MIN_CODE_BUF_REMAINING};
use crate::cpu::GuestCpu;
use crate::ir::context::Context;
use crate::ir::tb::{
    cflags::CF_SINGLE_STEP, decode_tb_exit, TranslationBlock, EXCP_EBREAK,
    EXCP_ECALL, EXCP_FENCE_I, EXCP_MRET, EXCP_PRIV_CSR, EXCP_SFENCE_VMA,
    EXCP_SRET, EXCP_UNDEF, EXCP_WFI, EXIT_TARGET_NONE, TB_EXIT_NOCHAIN,
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

        let stepping = cpu.gdb_single_step();

        // Suppress interrupts during GDB single-step
        // (IRQ delivery would corrupt step semantics).
        if !stepping && cpu.pending_interrupt() {
            cpu.reset_exit_request();
            cpu.handle_interrupt();
            next_tb_hint = None;
        } else if !stepping && cpu.has_pending_irq() {
            // Interrupts are pending but not deliverable
            // (e.g., SIE=0 in a critical section). Keep
            // neg_align set so goto_tb chains break on
            // every iteration, letting us re-check once
            // the guest re-enables interrupts.
            cpu.set_exit_request();
        } else {
            cpu.reset_exit_request();
        }

        let pc = cpu.get_pc();
        let flags = cpu.get_flags();

        let tb_idx = if stepping {
            // GDB single-step: translate a fresh 1-insn TB,
            // bypassing all caches (QEMU CF_SINGLE_STEP).
            let cf = CF_SINGLE_STEP | 1;
            match tb_gen_code_cflags(shared, per_cpu, cpu, pc, flags, cf) {
                Some(idx) => idx,
                None => {
                    if cpu.check_mem_fault() {
                        continue;
                    }
                    return ExitReason::BufferFull;
                }
            }
        } else {
            // Normal TB lookup (hint or hash table).
            let idx = match next_tb_hint.take() {
                Some(idx) => {
                    per_cpu.stats.hint_used += 1;
                    idx
                }
                None => match tb_find(shared, per_cpu, cpu, pc, flags) {
                    Some(idx) => idx,
                    None => {
                        if cpu.check_mem_fault() {
                            continue;
                        }
                        return ExitReason::BufferFull;
                    }
                },
            };

            // GDB breakpoint within TB: if the found TB
            // spans a breakpoint address, replace it with
            // a 1-insn ephemeral TB so the exec loop can
            // check breakpoints at each instruction
            // boundary. The original TB stays in cache for
            // non-debug execution (QEMU cflags approach).
            let tb = shared.tb_store.get(idx);
            if cpu.gdb_breakpoint_in_tb(tb.pc, tb.size as u64) {
                let cf = CF_SINGLE_STEP | 1;
                match tb_gen_code_cflags(shared, per_cpu, cpu, pc, flags, cf) {
                    Some(ss_idx) => ss_idx,
                    None => {
                        if cpu.check_mem_fault() {
                            continue;
                        }
                        return ExitReason::BufferFull;
                    }
                }
            } else {
                idx
            }
        };

        // GDB breakpoint check: if a breakpoint is set at
        // the current PC, skip TB execution and park.
        // Skip during single-step: the current instruction
        // must execute before breakpoints take effect.
        if !stepping && cpu.gdb_check_breakpoint(cpu.get_pc()) {
            // Save snapshot and park via check_monitor_pause.
            if cpu.check_monitor_pause() {
                return ExitReason::Halted;
            }
            continue;
        }

        let _atomic_guard = if shared.tb_store.get(tb_idx).contains_atomic {
            Some(shared.atomic_lock.lock().unwrap())
        } else {
            None
        };
        let raw_exit = cpu_tb_exec(shared, cpu, tb_idx);
        drop(_atomic_guard);
        let (last_tb, exit_code) = decode_tb_exit(raw_exit);

        let src_tb = last_tb.unwrap_or(tb_idx);

        // Self-modifying code detection: if stores wrote
        // to pages containing translated code, invalidate
        // the affected TBs immediately.  Only code pages
        // are tracked (store helper checks the code-page
        // bitmap), so this is a no-op when the guest only
        // writes to data pages.
        //
        // This is the machina equivalent of QEMU's
        // PAGE_WRITE_INV / notdirty_write mechanism and
        // provides the Ziccid guarantee (I-cache coherence
        // for instruction data) unconditionally.
        {
            let dirty = cpu.take_dirty_pages();
            if !dirty.is_empty() {
                for page in &dirty {
                    // Only invalidate TBs on code pages.
                    // Data-page writes don't affect TBs.
                    if shared.tb_store.is_code_page(*page) {
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
        }

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
                if cpu.check_mem_fault() {
                    continue;
                }

                let pc = cpu.get_pc();
                let flags = cpu.get_flags();

                // Check exit_target cache for indirect
                // jumps (goto_ptr / jalr). The last
                // TB_EXIT_NOCHAIN target is cached per-TB.
                let cached = shared
                    .tb_store
                    .get(src_tb)
                    .exit_target
                    .load(Ordering::Relaxed);
                if cached != EXIT_TARGET_NONE {
                    let tb = shared.tb_store.get(cached);
                    if !tb.invalid.load(Ordering::Acquire)
                        && tb.gen.load(Ordering::Acquire)
                            == shared.tb_store.global_gen()
                        && tb.pc == pc
                        && tb.flags == flags
                    {
                        if cpu.pending_interrupt() {
                            cpu.handle_interrupt();
                        } else if cpu.has_pending_irq() {
                            cpu.set_exit_request();
                        } else {
                            // GDB breakpoint check:
                            // exit_target cache bypasses
                            // the main-loop breakpoint gate.
                            // Must check here to catch
                            // breakpoints at TB entry PCs
                            // (AC-3).
                            if cpu.gdb_check_breakpoint(pc) {
                                if cpu.check_monitor_pause() {
                                    return ExitReason::Halted;
                                }
                                continue;
                            }
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
                cpu.execute_mret();
            }
            v if v == EXCP_SRET as usize => {
                per_cpu.stats.real_exit += 1;
                if !cpu.execute_sret() {
                    // Illegal: TSR trap or U-mode sret.
                    // PC points to next insn; rewind to
                    // the sret itself for mepc.
                    let cur = cpu.get_pc().wrapping_sub(4);
                    cpu.set_pc(cur);
                    cpu.handle_exception(2, 0);
                }
            }
            v if v == EXCP_SFENCE_VMA as usize => {
                per_cpu.stats.real_exit += 1;
                // TVM trap: sfence.vma in S-mode with
                // mstatus.TVM=1 raises IllegalInstruction.
                // PC was set to the sfence.vma instruction
                // by the translator.
                if cpu.check_sfence_trap() {
                    // TVM trap: PC points to next insn;
                    // rewind to the sfence.vma itself.
                    let cur = cpu.get_pc().wrapping_sub(4);
                    cpu.set_pc(cur);
                    cpu.handle_exception(2, 0);
                } else {
                    // Normal path: PC already at next insn.
                    cpu.tlb_flush();
                    shared
                        .tb_store
                        .invalidate_all(shared.code_buf(), &shared.backend);
                    per_cpu.jump_cache.invalidate();
                    next_tb_hint = None;
                }
            }
            v if v == EXCP_FENCE_I as usize => {
                per_cpu.stats.real_exit += 1;
                // fence.i: invalidate TBs on pages that
                // were written since the last fence.i.
                // Only code pages are tracked (store
                // helper checks the code-page bitmap).
                let dirty = cpu.take_dirty_pages();
                if dirty.is_empty() {
                    // No code-page writes tracked:
                    // conservative full flush.
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
                if cpu.pending_wfi_wakeup() {
                    cpu.set_halted(false);
                    if cpu.pending_interrupt() {
                        cpu.handle_interrupt();
                    }
                } else {
                    let woken = cpu.wait_for_interrupt();
                    if !woken {
                        cpu.set_halted(false);
                        return ExitReason::Halted;
                    }
                    // Check monitor pause BEFORE clearing
                    // halted, so snapshot captures WFI state.
                    if cpu.check_monitor_pause() {
                        cpu.set_halted(false);
                        return ExitReason::Halted;
                    }
                    cpu.set_halted(false);
                    // Woken by IRQ or timer. Check for
                    // pending interrupt and handle if any;
                    // otherwise resume (timer expired,
                    // guest will re-read mtime).
                    if cpu.pending_interrupt() {
                        cpu.handle_interrupt();
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
            v if v == EXCP_EBREAK as usize => {
                per_cpu.stats.real_exit += 1;
                let pc = cpu.get_pc();
                cpu.handle_exception(3, pc);
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
            v if v == EXCP_UNDEF as usize => {
                // Illegal instruction — raise exception
                // cause=2 so the guest kernel can handle.
                // tval=0 per RISC-V spec (mtval is either 0
                // or the faulting insn word; we use 0).
                cpu.handle_exception(2, 0);
            }
            _ => {
                per_cpu.stats.real_exit += 1;
                return ExitReason::Exit(exit_code);
            }
        }

        // GDB single-step: complete step immediately
        // after the 1-insn TB executes, before any
        // interrupt delivery or monitor checks.
        if stepping {
            cpu.gdb_complete_step();
            continue;
        }

        // Deliver latched memory faults from JIT helpers.
        // Must precede interrupt check: faults have higher
        // priority and must be delivered precisely.
        if !cpu.check_mem_fault() && cpu.pending_interrupt() {
            cpu.handle_interrupt();
            next_tb_hint = None;
        } else if cpu.has_pending_irq() {
            cpu.set_exit_request();
        }

        // External stop check BEFORE monitor pause.
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
    // Translate current virtual PC to physical for TB
    // validation. After sfence.vma, the TLB is flushed
    // so this triggers a page walk in gen_code.
    // cur_phys == pc means bare/M-mode (no translation).
    // cur_phys == u64::MAX means unknown (skip check).
    let cur_phys = cpu.translate_pc(pc);

    // Fast path: jump cache (per-CPU, no lock needed)
    if let Some(idx) = per_cpu.jump_cache.lookup(pc) {
        let tb = shared.tb_store.get(idx);
        if !tb.invalid.load(Ordering::Acquire)
            && tb.gen.load(Ordering::Acquire) == shared.tb_store.global_gen()
            && tb.pc == pc
            && tb.flags == flags
            && (cur_phys == u64::MAX || tb.phys_pc == cur_phys)
        {
            per_cpu.stats.jc_hit += 1;
            return Some(idx);
        }
    }

    // Slow path: hash table.
    if let Some(idx) = shared.tb_store.lookup(pc, flags) {
        let tb = shared.tb_store.get(idx);
        if cur_phys == u64::MAX || tb.phys_pc == cur_phys {
            per_cpu.jump_cache.insert(pc, idx);
            per_cpu.stats.ht_hit += 1;
            return Some(idx);
        }
    }

    // Miss: translate a new TB.
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

    // Translation with overflow retry (QEMU tcg_raise_tb_overflow
    // equivalent).  If the backend emits more code than fits in
    // the buffer, siglongjmp lands here with rc == -2 and we
    // retry with halved max_insns.
    let mut max_insns = TranslationBlock::max_insns(0);
    let mut jmp_buf: SigJmpBuf = unsafe { std::mem::zeroed() };
    let jmp_ptr = &mut jmp_buf as *mut SigJmpBuf;
    let saved_offset = shared.code_buf().offset();

    loop {
        let rc = unsafe { sigsetjmp(jmp_ptr, 0) };
        if rc == -2 {
            // Overflow: reset code buffer cursor and
            // retry with fewer instructions.
            unsafe {
                shared.code_buf_mut().jmp_trans = std::ptr::null_mut();
                shared.code_buf_mut().set_offset(saved_offset);
            }
            max_insns = (max_insns / 2).max(1);
            if max_insns == 1 {
                // Single-instruction TB still overflows —
                // treat as BufferFull.
                return None;
            }
            continue;
        }
        break;
    }

    // SAFETY: we hold translate_lock.
    let tb_idx = unsafe { shared.tb_store.alloc(pc, flags, 0) }?;

    guard.ir_ctx.reset();
    guard.ir_ctx.tb_idx = tb_idx as u32;
    let guest_size = cpu.gen_code(&mut guard.ir_ctx, pc, max_insns);
    if guest_size == 0 {
        unsafe {
            let tb = shared.tb_store.get_mut(tb_idx);
            tb.invalid.store(true, Ordering::Release);
        }
        return None;
    }
    let phys_pc = cpu.last_phys_pc();
    unsafe {
        let tb = shared.tb_store.get_mut(tb_idx);
        tb.size = guest_size;
        tb.phys_pc = phys_pc;
        // Stamp TB with current global generation so that
        // invalidate_all's O(1) generation bump correctly
        // identifies stale TBs.
        tb.gen
            .store(shared.tb_store.global_gen(), Ordering::Release);
    }
    shared.tb_store.mark_code_page(phys_pc >> 12, tb_idx);

    shared.backend.clear_goto_tb_offsets();

    // Install jmp_trans on code buffer so highwater
    // check can longjmp back here on overflow.
    unsafe {
        shared.code_buf_mut().jmp_trans = jmp_ptr as *mut u8;
    }

    let code_buf_mut = unsafe { shared.code_buf_mut() };
    let host_offset =
        translate(&mut guard.ir_ctx, &shared.backend, code_buf_mut);

    // Clear jmp_trans after translation completes.
    unsafe {
        shared.code_buf_mut().jmp_trans = std::ptr::null_mut();
    }

    let host_size = shared.code_buf().offset() - host_offset;

    // SAFETY: under translate_lock.
    unsafe {
        let tb = shared.tb_store.get_mut(tb_idx);
        tb.host_offset = host_offset;
        tb.host_size = host_size;
        tb.contains_atomic = guard.ir_ctx.contains_atomic;
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

/// Translate a single-step TB with explicit cflags.
/// The TB is allocated but NOT inserted into the hash
/// table or jump cache (ephemeral, one-shot use).
fn tb_gen_code_cflags<B, C>(
    shared: &SharedState<B>,
    per_cpu: &mut PerCpuState,
    cpu: &mut C,
    pc: u64,
    flags: u32,
    cflags: u32,
) -> Option<usize>
where
    B: HostCodeGen,
    C: GuestCpu<IrContext = Context>,
{
    if shared.code_buf().remaining() < MIN_CODE_BUF_REMAINING {
        return None;
    }

    let mut guard = shared.translate_lock.lock().unwrap();

    let max_insns = TranslationBlock::max_insns(cflags);

    let mut jmp_buf: SigJmpBuf = unsafe { std::mem::zeroed() };
    let jmp_ptr = &mut jmp_buf as *mut SigJmpBuf;
    let saved_offset = shared.code_buf().offset();
    let mut cur_max = max_insns;

    loop {
        let rc = unsafe { sigsetjmp(jmp_ptr, 0) };
        if rc == -2 {
            unsafe {
                shared.code_buf_mut().jmp_trans = std::ptr::null_mut();
                shared.code_buf_mut().set_offset(saved_offset);
            }
            cur_max = (cur_max / 2).max(1);
            if cur_max == 1 && max_insns == 1 {
                return None;
            }
            continue;
        }
        break;
    }

    let tb_idx = unsafe { shared.tb_store.alloc(pc, flags, cflags) }?;

    guard.ir_ctx.reset();
    guard.ir_ctx.tb_idx = tb_idx as u32;
    let guest_size = cpu.gen_code(&mut guard.ir_ctx, pc, cur_max);
    if guest_size == 0 {
        unsafe {
            let tb = shared.tb_store.get_mut(tb_idx);
            tb.invalid.store(true, Ordering::Release);
        }
        return None;
    }
    let phys_pc = cpu.last_phys_pc();
    unsafe {
        let tb = shared.tb_store.get_mut(tb_idx);
        tb.size = guest_size;
        tb.phys_pc = phys_pc;
        tb.gen
            .store(shared.tb_store.global_gen(), Ordering::Release);
    }
    shared.tb_store.mark_code_page(phys_pc >> 12, tb_idx);

    shared.backend.clear_goto_tb_offsets();
    unsafe {
        shared.code_buf_mut().jmp_trans = jmp_ptr as *mut u8;
    }

    let code_buf_mut = unsafe { shared.code_buf_mut() };
    let host_offset =
        translate(&mut guard.ir_ctx, &shared.backend, code_buf_mut);

    unsafe {
        shared.code_buf_mut().jmp_trans = std::ptr::null_mut();
    }

    let host_size = shared.code_buf().offset() - host_offset;
    unsafe {
        let tb = shared.tb_store.get_mut(tb_idx);
        tb.host_offset = host_offset;
        tb.host_size = host_size;
        tb.contains_atomic = guard.ir_ctx.contains_atomic;
    }

    let offsets = shared.backend.goto_tb_offsets();
    unsafe {
        let tb = shared.tb_store.get_mut(tb_idx);
        for (i, &(jmp, reset)) in offsets.iter().enumerate().take(2) {
            tb.set_jmp_insn_offset(i, jmp as u32);
            tb.set_jmp_reset_offset(i, reset as u32);
        }
    }

    // Do NOT insert into hash table or jump cache.
    // Single-step TBs are ephemeral.
    per_cpu.stats.translate += 1;

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

    let dst_tb = shared.tb_store.get(dst);
    if dst_tb.invalid.load(Ordering::Acquire)
        || dst_tb.gen.load(Ordering::Acquire) != shared.tb_store.global_gen()
    {
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
