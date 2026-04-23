pub mod code_buffer;
pub mod constraint;
pub mod cpu;
pub mod exec;
pub mod ir;
pub mod liveness;
pub mod optimize;
pub mod plat;
pub mod regalloc;
pub mod timer;
pub mod translate;
pub mod x86_64;

pub use code_buffer::CodeBuffer;
pub use constraint::{ArgConstraint, OpConstraint};
pub use cpu::GuestCpu;
pub use x86_64::X86_64CodeGen;

/// Trait for host architecture code generators.
///
/// Each target architecture (x86-64, AArch64, RISC-V, etc.)
/// implements this trait to generate host machine code from
/// TCG IR.
///
/// Reference: `~/qemu/tcg/<arch>/tcg-target.c.inc`.
pub trait HostCodeGen {
    /// Emit the prologue: save callee-saved registers, set up
    /// env pointer, allocate stack frame, jump to TB code.
    fn emit_prologue(&mut self, buf: &mut CodeBuffer);

    /// Emit the epilogue: restore callee-saved registers,
    /// deallocate stack frame, return to caller.
    fn emit_epilogue(&mut self, buf: &mut CodeBuffer);

    /// Patch a direct jump at `jump_offset` to point to
    /// `target_offset`. Used for TB chaining.
    ///
    /// Takes `&self` and `&CodeBuffer` so chaining can happen
    /// concurrently from multiple vCPU threads.
    fn patch_jump(
        &self,
        buf: &CodeBuffer,
        jump_offset: usize,
        target_offset: usize,
    );

    /// Return the offset of the TB return path.
    fn epilogue_offset(&self) -> usize;

    /// Initialize a translation context with backend-specific
    /// settings (reserved registers, stack frame layout, etc.).
    fn init_context(&self, ctx: &mut ir::Context);

    /// Return the register constraint for an opcode.
    fn op_constraint(&self, opc: ir::Opcode) -> &'static OpConstraint;

    // -- Register allocator primitives --

    /// Emit host mov between two registers.
    fn tcg_out_mov(&self, buf: &mut CodeBuffer, ty: ir::Type, dst: u8, src: u8);

    /// Emit host load-immediate into a register.
    fn tcg_out_movi(
        &self,
        buf: &mut CodeBuffer,
        ty: ir::Type,
        dst: u8,
        val: u64,
    );

    /// Emit host load from memory [base + offset] into
    /// register.
    fn tcg_out_ld(
        &self,
        buf: &mut CodeBuffer,
        ty: ir::Type,
        dst: u8,
        base: u8,
        offset: i64,
    );

    /// Emit host store from register to memory
    /// [base + offset].
    fn tcg_out_st(
        &self,
        buf: &mut CodeBuffer,
        ty: ir::Type,
        src: u8,
        base: u8,
        offset: i64,
    );

    /// Emit host code for a single IR op. Called by the
    /// register allocator after inputs are loaded and outputs
    /// allocated. `oregs`/`iregs` are allocated host register
    /// numbers. `cargs` are raw constant values from the op's
    /// carg slots.
    fn tcg_out_op(
        &self,
        buf: &mut CodeBuffer,
        ctx: &ir::Context,
        op: &ir::Op,
        oregs: &[u8],
        iregs: &[u8],
        cargs: &[u32],
    );

    /// Byte offset of the exit-request flag (neg_align)
    /// within the CPU struct, or 0 if not configured.
    fn neg_align_offset(&self) -> i32;

    /// Return goto_tb (jmp_offset, reset_offset) pairs
    /// recorded during the last codegen pass.
    fn goto_tb_offsets(&self) -> Vec<(usize, usize)>;

    /// Clear recorded goto_tb offsets before a new codegen
    /// pass.
    fn clear_goto_tb_offsets(&self);
}
