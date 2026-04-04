use crate::code_buffer::CodeBuffer;
use crate::constraint::OpConstraint;
use crate::ir::label::RelocKind;
use crate::ir::temp::TempKind;
use crate::ir::types::{RegSet, TempVal, Type};
use crate::ir::{Context, OpFlags, Opcode, TempIdx, OPCODE_DEFS};
use crate::HostCodeGen;

/// Register allocator state.
struct RegAllocState {
    reg_to_temp: [Option<TempIdx>; 16],
    free_regs: RegSet,
    allocatable: RegSet,
}

impl RegAllocState {
    fn new(allocatable: RegSet) -> Self {
        Self {
            reg_to_temp: [None; 16],
            free_regs: allocatable,
            allocatable,
        }
    }

    fn free_reg(&mut self, reg: u8) {
        self.reg_to_temp[reg as usize] = None;
        if self.allocatable.contains(reg) {
            self.free_regs = self.free_regs.set(reg);
        }
    }

    fn assign(&mut self, reg: u8, tidx: TempIdx) {
        self.reg_to_temp[reg as usize] = Some(tidx);
        self.free_regs = self.free_regs.clear(reg);
    }
}

// -- Helper functions --

/// Evict the current occupant of `reg`. Globals are synced to
/// memory; locals are moved to a free register.
fn evict_reg(
    ctx: &mut Context,
    state: &mut RegAllocState,
    backend: &impl HostCodeGen,
    buf: &mut CodeBuffer,
    reg: u8,
) {
    let Some(tidx) = state.reg_to_temp[reg as usize] else {
        return;
    };
    let temp = ctx.temp(tidx);
    if temp.is_global_or_fixed() {
        // Sync to memory and mark Mem
        temp_sync(ctx, backend, buf, tidx);
        let t = ctx.temp_mut(tidx);
        t.val_type = TempVal::Mem;
        t.reg = None;
        t.mem_coherent = true;
        state.free_reg(reg);
    } else {
        // Local temp: spill to stack frame.
        let ty = temp.ty;
        let offset = ctx.alloc_temp_frame(tidx);
        let frame_reg = ctx.frame_reg.unwrap();
        backend.tcg_out_st(buf, ty, reg, frame_reg, offset);
        state.free_reg(reg);
        let t = ctx.temp_mut(tidx);
        t.val_type = TempVal::Mem;
        t.reg = None;
        t.mem_coherent = true;
    }
}

/// Allocate a register from `required & ~forbidden`, preferring
/// `preferred`. Evicts an occupant if necessary. If all required
/// registers are forbidden (e.g. fixed constraint conflicts with
/// a prior input), evict the forbidden occupant first.
fn reg_alloc(
    ctx: &mut Context,
    state: &mut RegAllocState,
    backend: &impl HostCodeGen,
    buf: &mut CodeBuffer,
    required: RegSet,
    forbidden: RegSet,
    preferred: RegSet,
) -> u8 {
    let candidates = required.intersect(state.allocatable).subtract(forbidden);
    // Try preferred & free first
    let pref_free = candidates.intersect(state.free_regs).intersect(preferred);
    if let Some(r) = pref_free.first() {
        return r;
    }
    // Try any free
    let any_free = candidates.intersect(state.free_regs);
    if let Some(r) = any_free.first() {
        return r;
    }
    // Try evicting a non-forbidden occupant
    if let Some(r) = candidates.first() {
        evict_reg(ctx, state, backend, buf, r);
        return r;
    }
    // All required regs are forbidden — must evict a forbidden
    // occupant (e.g. fixed RCX constraint vs prior input in RCX).
    let forced = required.intersect(state.allocatable);
    let r = forced
        .first()
        .expect("no candidate register for allocation");
    evict_reg(ctx, state, backend, buf, r);
    r
}

/// Load a temp into a register satisfying the constraint.
/// Returns the allocated host register.
#[allow(clippy::too_many_arguments)]
fn temp_load_to(
    ctx: &mut Context,
    state: &mut RegAllocState,
    backend: &impl HostCodeGen,
    buf: &mut CodeBuffer,
    tidx: TempIdx,
    required: RegSet,
    forbidden: RegSet,
    preferred: RegSet,
) -> u8 {
    let temp = ctx.temp(tidx);
    match temp.val_type {
        TempVal::Reg => {
            let cur = temp.reg.unwrap();
            if required.contains(cur) && !forbidden.contains(cur) {
                return cur;
            }
            // Current reg doesn't satisfy — move
            let ty = temp.ty;
            let dst = reg_alloc(
                ctx, state, backend, buf, required, forbidden, preferred,
            );
            backend.tcg_out_mov(buf, ty, dst, cur);
            state.free_reg(cur);
            state.assign(dst, tidx);
            let t = ctx.temp_mut(tidx);
            t.reg = Some(dst);
            dst
        }
        TempVal::Const => {
            let val = temp.val;
            let ty = temp.ty;
            let reg = reg_alloc(
                ctx, state, backend, buf, required, forbidden, preferred,
            );
            state.assign(reg, tidx);
            backend.tcg_out_movi(buf, ty, reg, val);
            let t = ctx.temp_mut(tidx);
            t.val_type = TempVal::Reg;
            t.reg = Some(reg);
            reg
        }
        TempVal::Mem => {
            let ty = temp.ty;
            let mem_base = temp.mem_base;
            let mem_offset = temp.mem_offset;
            let mem_allocated = temp.mem_allocated;
            let reg = reg_alloc(
                ctx, state, backend, buf, required, forbidden, preferred,
            );
            state.assign(reg, tidx);
            if let Some(base_idx) = mem_base {
                // Global temp: load from [env + offset]
                let base_reg = ctx.temp(base_idx).reg.unwrap();
                backend.tcg_out_ld(buf, ty, reg, base_reg, mem_offset);
            } else if mem_allocated {
                // Local temp: load from [frame_reg + offset]
                let frame_reg = ctx.frame_reg.unwrap();
                backend.tcg_out_ld(buf, ty, reg, frame_reg, mem_offset);
            }
            let t = ctx.temp_mut(tidx);
            t.val_type = TempVal::Reg;
            t.reg = Some(reg);
            t.mem_coherent = true;
            reg
        }
        TempVal::Dead => {
            panic!("temp_load_to on dead temp {tidx:?}");
        }
    }
}

/// Sync a temp back to memory (globals and spilled locals).
fn temp_sync(
    ctx: &Context,
    backend: &impl HostCodeGen,
    buf: &mut CodeBuffer,
    tidx: TempIdx,
) {
    let temp = ctx.temp(tidx);
    if temp.mem_coherent {
        return;
    }
    let Some(reg) = temp.reg else { return };
    if let Some(base_idx) = temp.mem_base {
        // Global temp
        let base_reg = ctx.temp(base_idx).reg.unwrap();
        backend.tcg_out_st(buf, temp.ty, reg, base_reg, temp.mem_offset);
    } else if temp.mem_allocated {
        // Local temp with allocated stack slot
        let frame_reg = ctx.frame_reg.unwrap();
        backend.tcg_out_st(buf, temp.ty, reg, frame_reg, temp.mem_offset);
    }
}

/// Mark all globals in caller-saved registers as
/// needing reload from memory. Call after CALL_CLOBBER
/// ops whose inline code may clobber host registers.
///
/// The op's output temp is exempted: the inline fast
/// path and slow path both guarantee the output
/// register holds the correct value at the join point,
/// so it must NOT be invalidated here.
fn clobber_caller_saved(
    ctx: &mut Context,
    state: &mut RegAllocState,
    output: Option<TempIdx>,
) {
    const CALLER_SAVED: [u8; 9] = [0, 1, 2, 6, 7, 8, 9, 10, 11];
    for &reg in &CALLER_SAVED {
        if let Some(tidx) = state.reg_to_temp[reg as usize] {
            // Skip the op's output — its register
            // value is correct and must be preserved.
            if output == Some(tidx) {
                continue;
            }
            let temp = ctx.temp(tidx);
            if temp.is_global_or_fixed() {
                let t = ctx.temp_mut(tidx);
                t.val_type = TempVal::Mem;
                t.reg = None;
                t.mem_coherent = true;
                state.free_reg(reg);
            }
        }
    }
}

/// Sync all live globals back to memory.
fn sync_globals(
    ctx: &mut Context,
    backend: &impl HostCodeGen,
    buf: &mut CodeBuffer,
) {
    let nb_globals = ctx.nb_globals() as usize;
    for i in 0..nb_globals {
        let tidx = TempIdx(i as u32);
        let temp = ctx.temp(tidx);
        if temp.val_type == TempVal::Reg && !temp.mem_coherent {
            temp_sync(ctx, backend, buf, tidx);
            ctx.temp_mut(tidx).mem_coherent = true;
        }
    }
}

/// Dedicated register allocation for Call ops.
///
/// Unlike `regalloc_op`, this function:
/// - Syncs globals before the call (helper reads CPU state)
/// - Loads inputs into fixed regs without altering temp state
/// - Clobbers caller-saved regs after the call
/// - Restores Fixed temps to their original registers
///
/// Mirrors QEMU's `tcg_reg_alloc_call()`.
#[allow(clippy::needless_range_loop)]
fn regalloc_call(
    ctx: &mut Context,
    state: &mut RegAllocState,
    backend: &impl HostCodeGen,
    buf: &mut CodeBuffer,
    op: &crate::ir::Op,
    ct: &OpConstraint,
) {
    let def = &OPCODE_DEFS[op.opc as usize];
    let nb_oargs = def.nb_oargs as usize;
    let nb_iargs = def.nb_iargs as usize;
    let nb_cargs = def.nb_cargs as usize;
    let life = op.life;

    // x86-64 System V caller-saved registers.
    const CALLER_SAVED: [u8; 9] = [0, 1, 2, 6, 7, 8, 9, 10, 11];

    // 1. Sync all globals to memory (helper reads
    //    CPU state via env pointer).
    sync_globals(ctx, backend, buf);

    // 2. Spill any live local temps in caller-saved
    //    regs (they will be clobbered by the call).
    for &reg in &CALLER_SAVED {
        if let Some(tidx) = state.reg_to_temp[reg as usize] {
            let temp = ctx.temp(tidx);
            if !temp.is_global_or_fixed() {
                evict_reg(ctx, state, backend, buf, reg);
            }
        }
    }

    // 3. Load each input into its fixed register.
    //    Two-pass approach to avoid source-target
    //    overlap: resolve reg-to-reg moves first, then
    //    load const/mem args so they cannot clobber a
    //    live source register needed by a later move.
    let mut i_regs = [0u8; 10];
    let mut pending_nonreg: Vec<(TempIdx, u8)> = Vec::new();

    // Collect (src_reg, target_reg, type) for reg-
    // sourced inputs. Load const/mem inputs first
    // since they don't risk overwriting source regs.
    struct RegMove {
        _idx: usize,
        src: u8,
        dst: u8,
        ty: Type,
    }
    let mut moves: Vec<RegMove> = Vec::new();

    for i in 0..nb_iargs {
        let tidx = op.args[nb_oargs + i];
        let target = ct.args[nb_oargs + i].regs.first().unwrap();
        let temp = ctx.temp(tidx);
        match temp.val_type {
            TempVal::Reg => {
                let src = temp.reg.unwrap();
                if src != target {
                    moves.push(RegMove {
                        _idx: i,
                        src,
                        dst: target,
                        ty: temp.ty,
                    });
                }
                // Already in target — nothing to do.
            }
            TempVal::Const | TempVal::Mem | TempVal::Dead => {
                pending_nonreg.push((tidx, target));
            }
        }
        i_regs[i] = target;
    }

    // Resolve reg-to-reg moves with overlap detection.
    // Use a topological-sort approach: emit moves
    // whose dst is not any remaining src, then repeat.
    // For cycles, break with R11 as temp register.
    let mut done = vec![false; moves.len()];
    let mut progress = true;
    while progress {
        progress = false;
        for mi in 0..moves.len() {
            if done[mi] {
                continue;
            }
            let m = &moves[mi];
            // Check if m.dst is a source of any
            // remaining undone move.
            let blocked = moves
                .iter()
                .enumerate()
                .any(|(j, other)| !done[j] && j != mi && other.src == m.dst);
            if !blocked {
                backend.tcg_out_mov(buf, m.ty, m.dst, m.src);
                done[mi] = true;
                progress = true;
            }
        }
    }
    // Any remaining undone moves form a cycle.
    // Break cycles using R11 as temporary.
    for mi in 0..moves.len() {
        if done[mi] {
            continue;
        }
        let m = &moves[mi];
        backend.tcg_out_mov(buf, m.ty, 11, m.src);
        // Follow the cycle from m.dst.
        let mut cur_dst = m.dst;
        loop {
            let next = moves
                .iter()
                .enumerate()
                .find(|(j, other)| !done[*j] && other.src == cur_dst);
            match next {
                Some((j, _)) => {
                    let n = &moves[j];
                    backend.tcg_out_mov(buf, n.ty, n.dst, n.src);
                    done[j] = true;
                    cur_dst = n.dst;
                }
                None => break,
            }
        }
        backend.tcg_out_mov(buf, m.ty, m.dst, 11);
        done[mi] = true;
    }

    for (tidx, target) in pending_nonreg {
        let temp = ctx.temp(tidx);
        match temp.val_type {
            TempVal::Const => {
                backend.tcg_out_movi(buf, temp.ty, target, temp.val);
            }
            TempVal::Mem => {
                if let Some(base_idx) = temp.mem_base {
                    let base_reg = ctx.temp(base_idx).reg.unwrap();
                    backend.tcg_out_ld(
                        buf,
                        temp.ty,
                        target,
                        base_reg,
                        temp.mem_offset,
                    );
                } else if temp.mem_allocated {
                    let frame_reg = ctx.frame_reg.unwrap();
                    backend.tcg_out_ld(
                        buf,
                        temp.ty,
                        target,
                        frame_reg,
                        temp.mem_offset,
                    );
                }
            }
            TempVal::Dead => {
                backend.tcg_out_movi(buf, temp.ty, target, 0);
            }
            TempVal::Reg => unreachable!(),
        }
    }

    // 4. Free dead inputs.
    for i in 0..nb_iargs {
        if life.is_dead((nb_oargs + i) as u32) {
            let tidx = op.args[nb_oargs + i];
            temp_dead_input(ctx, state, tidx);
        }
    }

    // 5. Clobber all caller-saved registers AND
    // invalidate all globals. Helpers may modify CPU
    // state in memory, so cached register values for
    // globals are stale. QEMU does this with
    // save_globals() which sets all globals to
    // TEMP_VAL_MEM.
    for &reg in &CALLER_SAVED {
        if let Some(tidx) = state.reg_to_temp[reg as usize] {
            let temp = ctx.temp(tidx);
            if temp.is_global_or_fixed() {
                let t = ctx.temp_mut(tidx);
                t.val_type = TempVal::Mem;
                t.reg = None;
                t.mem_coherent = true;
            }
            state.free_reg(reg);
        }
    }
    // Invalidate globals in callee-saved registers too:
    // the helper may have modified CPU state via env ptr.
    // Skip Fixed temps (env pointer stays in RBP).
    let nb_globals = ctx.nb_globals() as usize;
    for i in 0..nb_globals {
        let tidx = TempIdx(i as u32);
        let temp = ctx.temp(tidx);
        if temp.kind == TempKind::Fixed {
            continue;
        }
        if temp.val_type == TempVal::Reg {
            if let Some(reg) = temp.reg {
                let t = ctx.temp_mut(tidx);
                t.val_type = TempVal::Mem;
                t.reg = None;
                t.mem_coherent = true;
                state.free_reg(reg);
            }
        }
    }

    // 6. Collect cargs and emit call.
    let cstart = nb_oargs + nb_iargs;
    let cargs: Vec<u32> =
        (0..nb_cargs).map(|i| op.args[cstart + i].0).collect();
    let out_reg = ct.args[0].regs.first().unwrap();
    backend.tcg_out_op(buf, ctx, op, &[out_reg], &i_regs[..nb_iargs], &cargs);

    // 7. Assign output to return register (RAX).
    let dst_tidx = op.args[0];
    state.assign(out_reg, dst_tidx);
    let t = ctx.temp_mut(dst_tidx);
    t.val_type = TempVal::Reg;
    t.reg = Some(out_reg);
    t.mem_coherent = false;

    // 8. Free dead output.
    if life.is_dead(0) {
        temp_dead(ctx, state, dst_tidx);
    }
}

/// Free a temp's register if it's dead after this op.
fn temp_dead(ctx: &mut Context, state: &mut RegAllocState, tidx: TempIdx) {
    let temp = ctx.temp(tidx);
    if temp.is_global_or_fixed() {
        return;
    }
    if let Some(reg) = temp.reg {
        state.free_reg(reg);
    }
    let t = ctx.temp_mut(tidx);
    t.val_type = TempVal::Dead;
    t.reg = None;
}

fn temp_dead_input(
    ctx: &mut Context,
    state: &mut RegAllocState,
    tidx: TempIdx,
) {
    let temp = ctx.temp(tidx);
    if temp.is_global_or_fixed() {
        return;
    }
    if let Some(reg) = temp.reg {
        if state.reg_to_temp[reg as usize] == Some(tidx) {
            state.free_reg(reg);
        }
    }
    let t = ctx.temp_mut(tidx);
    t.val_type = TempVal::Dead;
    t.reg = None;
}

/// Generic constraint-driven register allocation for one op.
///
/// Mirrors QEMU's `tcg_reg_alloc_op()`.
#[allow(clippy::needless_range_loop)]
fn regalloc_op(
    ctx: &mut Context,
    state: &mut RegAllocState,
    backend: &impl HostCodeGen,
    buf: &mut CodeBuffer,
    op: &crate::ir::Op,
    ct: &OpConstraint,
) {
    let def = &OPCODE_DEFS[op.opc as usize];
    let nb_oargs = def.nb_oargs as usize;
    let nb_iargs = def.nb_iargs as usize;
    let nb_cargs = def.nb_cargs as usize;
    let life = op.life;

    let mut i_regs = [0u8; 10];
    let mut i_allocated = RegSet::EMPTY;
    // Track which aliased inputs can be reused for output
    let mut i_reusable = [false; 10];
    // Track Fixed temps moved away from their home register
    // so we can restore them after the op.
    let mut fixed_moves: Vec<(TempIdx, u8, u8)> = Vec::new();

    // 1. Process inputs
    for i in 0..nb_iargs {
        let arg_ct = &ct.args[nb_oargs + i];
        let tidx = op.args[nb_oargs + i];
        let required = arg_ct.regs;
        let is_dead = life.is_dead((nb_oargs + i) as u32);
        let temp = ctx.temp(tidx);
        let is_readonly = temp.is_global_or_fixed() || temp.is_const();
        let orig_fixed = if temp.is_fixed() { temp.reg } else { None };

        if arg_ct.ialias && is_dead && !is_readonly {
            // Can reuse this input's register for the
            // aliased output.
            let preferred = op.output_pref[arg_ct.alias_index as usize];
            let reg = temp_load_to(
                ctx,
                state,
                backend,
                buf,
                tidx,
                required,
                i_allocated,
                preferred,
            );
            i_regs[i] = reg;
            i_allocated = i_allocated.set(reg);
            i_reusable[i] = true;
        } else {
            let reg = temp_load_to(
                ctx,
                state,
                backend,
                buf,
                tidx,
                required,
                i_allocated,
                RegSet::EMPTY,
            );
            i_regs[i] = reg;
            i_allocated = i_allocated.set(reg);
        }

        // Record if a Fixed temp was moved away from
        // its home register.
        if let Some(orig_reg) = orig_fixed {
            if let Some(cur) = ctx.temp(tidx).reg {
                if cur != orig_reg {
                    fixed_moves.push((tidx, orig_reg, cur));
                }
            }
        }
    }

    // Fixup: re-read actual registers after all inputs are
    // processed. A later input's allocation may have evicted
    // an earlier input (e.g. fixed RCX constraint).
    i_allocated = RegSet::EMPTY;
    for i in 0..nb_iargs {
        let tidx = op.args[nb_oargs + i];
        let temp = ctx.temp(tidx);
        if temp.val_type == TempVal::Reg {
            let reg = temp.reg.unwrap();
            i_regs[i] = reg;
            i_allocated = i_allocated.set(reg);
        }
    }

    // 2. Process outputs
    let mut o_regs = [0u8; 10];
    let mut o_allocated = RegSet::EMPTY;
    for k in 0..nb_oargs {
        let arg_ct = &ct.args[k];
        let dst_tidx = op.args[k];

        let reg = if arg_ct.oalias {
            let ai = arg_ct.alias_index as usize;
            if i_reusable[ai] {
                // Reuse the dead input's register
                i_regs[ai]
            } else {
                // Input is still live — copy it away,
                // take its register for the output.
                let old_reg = i_regs[ai];
                let src_tidx = op.args[nb_oargs + ai];
                let src_temp = ctx.temp(src_tidx);
                let ty = src_temp.ty;
                let copy_reg = reg_alloc(
                    ctx,
                    state,
                    backend,
                    buf,
                    state.allocatable,
                    i_allocated.union(o_allocated),
                    RegSet::EMPTY,
                );
                backend.tcg_out_mov(buf, ty, copy_reg, old_reg);
                state.assign(copy_reg, src_tidx);
                let t = ctx.temp_mut(src_tidx);
                t.reg = Some(copy_reg);
                old_reg
            }
        } else if arg_ct.newreg {
            reg_alloc(
                ctx,
                state,
                backend,
                buf,
                arg_ct.regs,
                i_allocated.union(o_allocated),
                RegSet::EMPTY,
            )
        } else {
            reg_alloc(
                ctx,
                state,
                backend,
                buf,
                arg_ct.regs,
                o_allocated,
                RegSet::EMPTY,
            )
        };

        state.assign(reg, dst_tidx);
        let t = ctx.temp_mut(dst_tidx);
        t.val_type = TempVal::Reg;
        t.reg = Some(reg);
        t.mem_coherent = false;
        o_regs[k] = reg;
        o_allocated = o_allocated.set(reg);
    }

    // Fixup: outputs may have evicted/moved inputs.
    for i in 0..nb_iargs {
        let tidx = op.args[nb_oargs + i];
        let temp = ctx.temp(tidx);
        if temp.val_type == TempVal::Reg {
            if let Some(reg) = temp.reg {
                i_regs[i] = reg;
            }
        }
    }

    // 3. Collect constant args
    let cstart = nb_oargs + nb_iargs;
    let cargs: Vec<u32> =
        (0..nb_cargs).map(|i| op.args[cstart + i].0).collect();

    // 4. Emit host code
    backend.tcg_out_op(
        buf,
        ctx,
        op,
        &o_regs[..nb_oargs],
        &i_regs[..nb_iargs],
        &cargs,
    );

    // 5. Free dead inputs
    for i in 0..nb_iargs {
        if life.is_dead((nb_oargs + i) as u32) {
            let tidx = op.args[nb_oargs + i];
            let mut aliased = false;
            for k in 0..nb_oargs {
                if op.args[k] == tidx {
                    aliased = true;
                    break;
                }
            }
            if !aliased {
                temp_dead_input(ctx, state, tidx);
            }
        }
    }

    // 6. Restore Fixed temps to their home registers.
    for (tidx, orig_reg, moved_reg) in fixed_moves {
        if ctx.temp(tidx).is_fixed() {
            let t = ctx.temp_mut(tidx);
            t.val_type = TempVal::Reg;
            t.reg = Some(orig_reg);
            state.assign(orig_reg, tidx);
        }
        if state.reg_to_temp[moved_reg as usize] == Some(tidx) {
            state.free_reg(moved_reg);
        }
    }

    // 7. Free dead outputs
    for k in 0..nb_oargs {
        if life.is_dead(k as u32) {
            let tidx = op.args[k];
            temp_dead(ctx, state, tidx);
        }
    }

    // 8. Sync globals if needed
    for i in 0..nb_iargs {
        let arg_pos = (nb_oargs + i) as u32;
        if life.is_sync(arg_pos) {
            let tidx = op.args[nb_oargs + i];
            temp_sync(ctx, backend, buf, tidx);
            ctx.temp_mut(tidx).mem_coherent = true;
        }
    }
}

/// Main register allocation + code generation pass.
pub fn regalloc_and_codegen(
    ctx: &mut Context,
    backend: &impl HostCodeGen,
    buf: &mut CodeBuffer,
) {
    let allocatable = crate::x86_64::regs::ALLOCATABLE_REGS;
    let mut state = RegAllocState::new(allocatable);

    // Initialize fixed temps (always in their register)
    let nb_globals = ctx.nb_globals();
    for i in 0..nb_globals {
        let tidx = TempIdx(i);
        let temp = ctx.temp(tidx);
        if temp.kind == TempKind::Fixed {
            if let Some(reg) = temp.reg {
                state.assign(reg, tidx);
            }
        }
    }

    let num_ops = ctx.num_ops();
    for oi in 0..num_ops {
        let op = ctx.ops()[oi].clone();
        let def = &OPCODE_DEFS[op.opc as usize];
        let flags = def.flags;

        match op.opc {
            Opcode::Nop => continue,
            Opcode::InsnStart => {
                // Pass through to backend for fault_pc.
                let ca: Vec<u32> = op.cargs().iter().map(|t| t.0).collect();
                backend.tcg_out_op(buf, ctx, &op, &[], &[], &ca);
                // Highwater check: if code buffer is
                // nearly full, longjmp back to tb_gen_code
                // which retries with fewer instructions
                // (QEMU tcg_raise_tb_overflow equiv).
                buf.check_highwater();
                continue;
            }

            Opcode::Mov => {
                let dst_idx = op.args[0];
                let src_idx = op.args[1];
                let life = op.life;
                let src_reg = temp_load_to(
                    ctx,
                    &mut state,
                    backend,
                    buf,
                    src_idx,
                    allocatable,
                    RegSet::EMPTY,
                    RegSet::EMPTY,
                );
                if life.is_dead(1) {
                    temp_dead(ctx, &mut state, src_idx);
                }
                let dst_reg = reg_alloc(
                    ctx,
                    &mut state,
                    backend,
                    buf,
                    allocatable,
                    RegSet::EMPTY,
                    RegSet::EMPTY,
                );
                state.assign(dst_reg, dst_idx);
                let t = ctx.temp_mut(dst_idx);
                t.val_type = TempVal::Reg;
                t.reg = Some(dst_reg);
                t.mem_coherent = false;
                if dst_reg != src_reg {
                    backend.tcg_out_mov(buf, op.op_type, dst_reg, src_reg);
                }
                if life.is_dead(0) {
                    temp_dead(ctx, &mut state, dst_idx);
                }
            }

            Opcode::SetLabel => {
                let label_id = op.args[0].0;
                sync_globals(ctx, backend, buf);
                let offset = buf.offset();
                let label = ctx.label_mut(label_id);
                label.set_value(offset);
                let uses: Vec<_> = label.uses.drain(..).collect();
                for u in uses {
                    match u.kind {
                        RelocKind::Rel32 => {
                            let disp = (offset as i64) - (u.offset as i64 + 4);
                            buf.patch_u32(u.offset, disp as u32);
                        }
                    }
                }
            }

            Opcode::Br => {
                let label_id = op.args[0].0;
                sync_globals(ctx, backend, buf);
                let label = ctx.label(label_id);
                if label.has_value {
                    crate::x86_64::emitter::emit_jmp(buf, label.value);
                } else {
                    buf.emit_u8(0xE9);
                    let patch_off = buf.offset();
                    buf.emit_u32(0);
                    ctx.label_mut(label_id)
                        .add_use(patch_off, RelocKind::Rel32);
                }
            }

            Opcode::ExitTb | Opcode::GotoTb => {
                sync_globals(ctx, backend, buf);
                let nb_cargs = def.nb_cargs as usize;
                let cstart = (def.nb_oargs + def.nb_iargs) as usize;
                let cargs: Vec<u32> =
                    (0..nb_cargs).map(|i| op.args[cstart + i].0).collect();
                backend.tcg_out_op(buf, ctx, &op, &[], &[], &cargs);
            }

            Opcode::Call => {
                let ct = backend.op_constraint(op.opc);
                regalloc_call(ctx, &mut state, backend, buf, &op, ct);
            }

            Opcode::GotoPtr => {
                // Load input register, sync globals,
                // then emit indirect jump.
                let ct = backend.op_constraint(op.opc);
                let tidx = op.args[0];
                let arg_ct = &ct.args[0];
                let reg = temp_load_to(
                    ctx,
                    &mut state,
                    backend,
                    buf,
                    tidx,
                    arg_ct.regs,
                    RegSet::EMPTY,
                    RegSet::EMPTY,
                );
                let life = op.life;
                if life.is_dead(0) {
                    temp_dead(ctx, &mut state, tidx);
                }
                sync_globals(ctx, backend, buf);
                backend.tcg_out_op(buf, ctx, &op, &[], &[reg], &[]);
            }

            Opcode::Mb => {
                // NP (NOT_PRESENT): no register allocation,
                // emit directly.
                crate::x86_64::emitter::emit_mfence(buf);
            }

            Opcode::BrCond => {
                let ct = backend.op_constraint(op.opc);
                let nb_iargs = def.nb_iargs as usize;
                let nb_oargs = def.nb_oargs as usize;
                let nb_cargs = def.nb_cargs as usize;
                let life = op.life;

                let mut iregs = Vec::new();
                let mut i_allocated = RegSet::EMPTY;
                for i in 0..nb_iargs {
                    let tidx = op.args[nb_oargs + i];
                    let arg_ct = &ct.args[nb_oargs + i];
                    let reg = temp_load_to(
                        ctx,
                        &mut state,
                        backend,
                        buf,
                        tidx,
                        arg_ct.regs,
                        i_allocated,
                        RegSet::EMPTY,
                    );
                    iregs.push(reg);
                    i_allocated = i_allocated.set(reg);
                }

                let cstart = nb_oargs + nb_iargs;
                let cargs: Vec<u32> =
                    (0..nb_cargs).map(|i| op.args[cstart + i].0).collect();

                for i in 0..nb_iargs {
                    let arg_pos = (nb_oargs + i) as u32;
                    if life.is_dead(arg_pos) {
                        let tidx = op.args[nb_oargs + i];
                        temp_dead(ctx, &mut state, tidx);
                    }
                }

                sync_globals(ctx, backend, buf);

                let label_id = cargs[1];
                let label = ctx.label(label_id);
                let label_resolved = label.has_value;

                backend.tcg_out_op(buf, ctx, &op, &[], &iregs, &cargs);

                if !label_resolved {
                    let patch_off = buf.offset() - 4;
                    ctx.label_mut(label_id)
                        .add_use(patch_off, RelocKind::Rel32);
                }
            }

            _ => {
                if flags.contains(OpFlags::CALL_CLOBBER) {
                    sync_globals(ctx, backend, buf);
                }
                let ct = backend.op_constraint(op.opc);
                regalloc_op(ctx, &mut state, backend, buf, &op, ct);
                if flags.contains(OpFlags::CALL_CLOBBER) {
                    let def = op.opc.def();
                    let out = if def.nb_oargs > 0 {
                        Some(op.args[0])
                    } else {
                        None
                    };
                    clobber_caller_saved(ctx, &mut state, out);
                }
                if flags.contains(OpFlags::BB_END) {
                    sync_globals(ctx, backend, buf);
                }
            }
        }
    }
}
