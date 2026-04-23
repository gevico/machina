use crate::code_buffer::CodeBuffer;
use crate::ir::Context;
use crate::liveness::liveness_analysis;
use crate::optimize::optimize;
use crate::regalloc::regalloc_and_codegen;
use crate::HostCodeGen;

/// Full translation pipeline: optimize → liveness → regalloc+codegen.
/// Returns the offset where TB code starts in the buffer.
pub fn translate(
    ctx: &mut Context,
    backend: &impl HostCodeGen,
    buf: &mut CodeBuffer,
) -> usize {
    optimize(ctx);
    liveness_analysis(ctx);
    let tb_start = buf.offset();
    regalloc_and_codegen(ctx, backend, buf);
    tb_start
}

/// Translate and execute a TB.
///
/// # Safety
/// `env` must point to a valid CPUState-like struct that
/// matches the globals registered in `ctx`.
pub unsafe fn translate_and_execute(
    ctx: &mut Context,
    backend: &impl HostCodeGen,
    buf: &mut CodeBuffer,
    env: *mut u8,
) -> usize {
    // Buffer is RWX, no permission switch needed.
    let tb_start = translate(ctx, backend, buf);

    // Prologue signature:
    //   fn(env: *mut u8, tb_ptr: *const u8) -> usize
    // RDI = env, RSI = TB code pointer, returns RAX
    let prologue_fn: unsafe extern "sysv64" fn(*mut u8, *const u8) -> usize =
        core::mem::transmute(buf.base_ptr());
    let tb_ptr = buf.ptr_at(tb_start);
    let raw = prologue_fn(env, tb_ptr);
    // Decode: strip the encoded TB index, return only the
    // exit code (slot number or exception code).
    let (_, exit_code) = crate::ir::tb::decode_tb_exit(raw);
    exit_code
}
