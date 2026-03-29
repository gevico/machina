use machina_core::context::Context;
use machina_core::op::{Op, OpIdx};
use machina_core::opcode::Opcode;
use machina_core::temp::{TempIdx, TempKind};
use machina_core::types::{RegSet, Type};

#[test]
fn context_new_temp() {
    let mut ctx = Context::new();
    let t0 = ctx.new_temp(Type::I32);
    let t1 = ctx.new_temp(Type::I64);
    assert_eq!(t0, TempIdx(0));
    assert_eq!(t1, TempIdx(1));
    assert_eq!(ctx.nb_temps(), 2);
    assert_eq!(ctx.temp(t0).ty, Type::I32);
    assert_eq!(ctx.temp(t1).ty, Type::I64);
    assert_eq!(ctx.temp(t0).kind, TempKind::Ebb);
}

#[test]
fn context_new_temp_tb() {
    let mut ctx = Context::new();
    let t = ctx.new_temp_tb(Type::I64);
    assert_eq!(ctx.temp(t).kind, TempKind::Tb);
}

#[test]
fn context_const_dedup() {
    let mut ctx = Context::new();
    let c1 = ctx.new_const(Type::I64, 42);
    let c2 = ctx.new_const(Type::I64, 42);
    assert_eq!(c1, c2, "same constant should be deduplicated");

    let c3 = ctx.new_const(Type::I64, 99);
    assert_ne!(c1, c3, "different constants should be different");

    // Same value but different type should NOT be deduplicated
    let c4 = ctx.new_const(Type::I32, 42);
    assert_ne!(c1, c4, "same value different type should not dedup");
}

#[test]
fn context_globals() {
    let mut ctx = Context::new();
    let env = ctx.new_fixed(Type::I64, 5, "env");
    let pc = ctx.new_global(Type::I64, env, 128, "pc");
    let sp = ctx.new_global(Type::I64, env, 136, "sp");

    assert_eq!(ctx.nb_globals(), 3);
    assert_eq!(ctx.globals().len(), 3);
    assert_eq!(ctx.temp(pc).name, Some("pc"));
    assert_eq!(ctx.temp(sp).mem_offset, 136);

    // Now allocate a local — should not affect globals count
    let _t = ctx.new_temp(Type::I32);
    assert_eq!(ctx.nb_globals(), 3);
    assert_eq!(ctx.nb_temps(), 4);
}

#[test]
fn context_reset_preserves_globals() {
    let mut ctx = Context::new();
    let env = ctx.new_fixed(Type::I64, 5, "env");
    let _pc = ctx.new_global(Type::I64, env, 128, "pc");
    assert_eq!(ctx.nb_globals(), 2);

    // Add some locals and ops
    ctx.new_temp(Type::I32);
    ctx.new_temp(Type::I64);
    let idx = ctx.next_op_idx();
    ctx.emit_op(Op::new(idx, Opcode::Nop, Type::I32));
    ctx.new_label();

    assert_eq!(ctx.nb_temps(), 4);
    assert_eq!(ctx.num_ops(), 1);

    ctx.reset();

    // Globals preserved, locals/ops/labels cleared
    assert_eq!(ctx.nb_globals(), 2);
    assert_eq!(ctx.nb_temps(), 2);
    assert_eq!(ctx.num_ops(), 0);
    assert!(ctx.labels().is_empty());
}

#[test]
fn context_emit_ops() {
    let mut ctx = Context::new();
    let t0 = ctx.new_temp(Type::I64);
    let t1 = ctx.new_temp(Type::I64);
    let t2 = ctx.new_temp(Type::I64);

    let idx = ctx.next_op_idx();
    let op = Op::with_args(idx, Opcode::Add, Type::I64, &[t0, t1, t2]);
    ctx.emit_op(op);

    assert_eq!(ctx.num_ops(), 1);
    assert_eq!(ctx.op(OpIdx(0)).opc, Opcode::Add);
}

#[test]
fn context_labels() {
    let mut ctx = Context::new();
    let l0 = ctx.new_label();
    let l1 = ctx.new_label();
    assert_eq!(l0, 0);
    assert_eq!(l1, 1);
    assert_eq!(ctx.labels().len(), 2);

    ctx.label_mut(l0).set_value(100);
    assert!(ctx.label(l0).has_value);
    assert_eq!(ctx.label(l0).value, 100);
}

#[test]
fn context_frame() {
    let mut ctx = Context::new();
    assert_eq!(ctx.frame_reg, None);

    ctx.set_frame(4, 128, 1024); // RSP, offset 128, size 1024
    assert_eq!(ctx.frame_reg, Some(4));
    assert_eq!(ctx.frame_start, 128);
    assert_eq!(ctx.frame_end, 1152);
}

#[test]
fn context_reserved_regs() {
    let mut ctx = Context::new();
    assert!(ctx.reserved_regs.is_empty());

    ctx.reserved_regs = RegSet::EMPTY.set(4).set(5); // RSP, RBP
    assert!(ctx.reserved_regs.contains(4));
    assert!(ctx.reserved_regs.contains(5));
    assert!(!ctx.reserved_regs.contains(0));
}

#[test]
#[should_panic(expected = "globals must be registered before locals")]
fn context_global_after_local_panics() {
    let mut ctx = Context::new();
    let env = ctx.new_fixed(Type::I64, 5, "env");
    ctx.new_temp(Type::I32); // local
    ctx.new_global(Type::I64, env, 0, "x"); // should panic
}
