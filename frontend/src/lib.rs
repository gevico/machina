//! TCG Frontend — guest instruction decoding and IR generation.
//!
//! Provides the generic translation framework (`TranslatorOps` trait
//! and `translator_loop`) plus architecture-specific decoders.

pub mod riscv;

use machina_core::Context;

// ---------------------------------------------------------------
// Generic translation framework
// ---------------------------------------------------------------

/// TB termination reason set by `translate_insn`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisasJumpType {
    /// Continue to the next sequential instruction.
    Next,
    /// Reached the maximum number of instructions per TB.
    TooMany,
    /// Unconditional branch / exit — no fall-through.
    NoReturn,
}

/// Base context shared by all guest architectures.
///
/// Mirrors QEMU's `DisasContextBase`.
pub struct DisasContextBase {
    /// PC of the first instruction in this TB.
    pub pc_first: u64,
    /// PC of the *next* instruction to decode.
    pub pc_next: u64,
    /// How the current instruction terminates.
    pub is_jmp: DisasJumpType,
    /// Number of guest instructions translated so far.
    pub num_insns: u32,
    /// Maximum instructions allowed in one TB.
    pub max_insns: u32,
}

/// Per-architecture translation operations.
///
/// Mirrors QEMU's `TranslatorOps` vtable.
pub trait TranslatorOps {
    /// Architecture-specific disassembly context.
    type DisasContext;

    /// One-time setup before the translation loop.
    fn init_disas_context(ctx: &mut Self::DisasContext, ir: &mut Context);

    /// Called once at the start of the TB (after init).
    fn tb_start(ctx: &mut Self::DisasContext, ir: &mut Context);

    /// Emit `insn_start` marker for the current guest PC.
    fn insn_start(ctx: &mut Self::DisasContext, ir: &mut Context);

    /// Decode and translate one guest instruction.
    ///
    /// Must advance `base().pc_next` and set `base().is_jmp`
    /// when the instruction terminates the TB.
    fn translate_insn(ctx: &mut Self::DisasContext, ir: &mut Context);

    /// Emit TB epilogue (exit / goto_tb for fall-through).
    fn tb_stop(ctx: &mut Self::DisasContext, ir: &mut Context);

    /// Access the base context embedded in the arch context.
    fn base(ctx: &Self::DisasContext) -> &DisasContextBase;

    /// Mutable access to the base context.
    fn base_mut(ctx: &mut Self::DisasContext) -> &mut DisasContextBase;
}

/// Generic translation loop — drives the decode → translate
/// cycle.
///
/// Mirrors QEMU's `translator_loop()` in
/// `accel/tcg/translator.c`.
pub fn translator_loop<T: TranslatorOps>(
    ctx: &mut T::DisasContext,
    ir: &mut Context,
) {
    T::init_disas_context(ctx, ir);
    T::tb_start(ctx, ir);

    loop {
        T::insn_start(ctx, ir);
        T::translate_insn(ctx, ir);

        let base = T::base(ctx);
        if base.is_jmp != DisasJumpType::Next {
            break;
        }
        if base.num_insns >= base.max_insns {
            T::base_mut(ctx).is_jmp = DisasJumpType::TooMany;
            break;
        }
    }

    T::tb_stop(ctx, ir);
}
