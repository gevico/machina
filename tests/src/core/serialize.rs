use std::io::Cursor;

use machina_core::context::Context;
use machina_core::op::Op;
use machina_core::opcode::Opcode;
use machina_core::serialize;
use machina_core::temp::TempIdx;
use machina_core::types::Type;

/// Helper: serialize a Context, then deserialize and return
/// the first Context from the result.
fn round_trip(ctx: &Context) -> Context {
    let mut buf = Vec::new();
    serialize::serialize(ctx, &mut buf).expect("serialize failed");
    let mut cursor = Cursor::new(&buf);
    let mut contexts =
        serialize::deserialize(&mut cursor).expect("deserialize failed");
    assert_eq!(contexts.len(), 1);
    contexts.remove(0)
}

/// Helper: serialize multiple Contexts, then deserialize all.
fn round_trip_multi(ctxs: &[&Context]) -> Vec<Context> {
    let mut buf = Vec::new();
    for ctx in ctxs {
        serialize::serialize(ctx, &mut buf).expect("serialize failed");
    }
    let mut cursor = Cursor::new(&buf);
    serialize::deserialize(&mut cursor).expect("deserialize failed")
}

// -- from_raw_parts --

#[test]
fn from_raw_parts_basic() {
    let ctx = Context::from_raw_parts(Vec::new(), Vec::new(), Vec::new(), 0);
    assert_eq!(ctx.nb_globals(), 0);
    assert_eq!(ctx.nb_temps(), 0);
    assert_eq!(ctx.num_ops(), 0);
    assert!(ctx.labels().is_empty());
}

// -- Round-trip: globals only --

#[test]
fn serialize_globals_only() {
    let mut ctx = Context::new();
    let env = ctx.new_fixed(Type::I64, 5, "env");
    ctx.new_global(Type::I64, env, 8, "pc");
    ctx.new_global(Type::I64, env, 16, "sp");

    let out = round_trip(&ctx);

    assert_eq!(out.nb_globals(), 3);
    assert_eq!(out.nb_temps(), 3);

    // Fixed temp
    let t0 = out.temp(TempIdx(0));
    assert_eq!(t0.kind, machina_core::TempKind::Fixed);
    assert_eq!(t0.ty, Type::I64);
    assert_eq!(t0.reg, Some(5));
    assert_eq!(t0.name, Some("env"));

    // Global temp
    let t1 = out.temp(TempIdx(1));
    assert_eq!(t1.kind, machina_core::TempKind::Global);
    assert_eq!(t1.mem_base, Some(TempIdx(0)));
    assert_eq!(t1.mem_offset, 8);
    assert_eq!(t1.name, Some("pc"));

    let t2 = out.temp(TempIdx(2));
    assert_eq!(t2.mem_offset, 16);
    assert_eq!(t2.name, Some("sp"));
}

// -- Round-trip: globals + locals + consts + ops --

#[test]
fn serialize_full_tb() {
    let mut ctx = Context::new();
    let env = ctx.new_fixed(Type::I64, 5, "env");
    let x1 = ctx.new_global(Type::I64, env, 8, "x1");
    let x2 = ctx.new_global(Type::I64, env, 16, "x2");
    let x3 = ctx.new_global(Type::I64, env, 24, "x3");

    // Locals
    let tmp = ctx.new_temp(Type::I64);

    // Const
    let c42 = ctx.new_const(Type::I64, 42);

    // Ops: insn_start, add x3 = x1 + x2, exit_tb
    let idx0 = ctx.next_op_idx();
    let mut op0 = Op::new(idx0, Opcode::InsnStart, Type::I64);
    op0.nargs = 2;
    op0.args[0] = TempIdx(0x1000); // pc (encoded)
    op0.args[1] = TempIdx(0);
    ctx.emit_op(op0);

    let idx1 = ctx.next_op_idx();
    let op1 = Op::with_args(idx1, Opcode::Add, Type::I64, &[tmp, x1, x2]);
    ctx.emit_op(op1);

    let idx2 = ctx.next_op_idx();
    let op2 = Op::with_args(idx2, Opcode::Add, Type::I64, &[x3, tmp, c42]);
    ctx.emit_op(op2);

    let idx3 = ctx.next_op_idx();
    let mut op3 = Op::new(idx3, Opcode::ExitTb, Type::I64);
    op3.nargs = 1;
    op3.args[0] = TempIdx(0);
    ctx.emit_op(op3);

    let out = round_trip(&ctx);

    // Verify structure
    assert_eq!(out.nb_globals(), 4);
    assert_eq!(out.nb_temps(), 6); // 4 globals + 1 local + 1 const
    assert_eq!(out.num_ops(), 4);

    // Verify const temp
    let ct = out.temp(TempIdx(5));
    assert_eq!(ct.kind, machina_core::TempKind::Const);
    assert_eq!(ct.val, 42);

    // Verify ops
    assert_eq!(out.ops()[0].opc, Opcode::InsnStart);
    assert_eq!(out.ops()[1].opc, Opcode::Add);
    assert_eq!(out.ops()[1].nargs, 3);
    assert_eq!(out.ops()[1].args[0], tmp);
    assert_eq!(out.ops()[1].args[1], x1);
    assert_eq!(out.ops()[1].args[2], x2);
    assert_eq!(out.ops()[2].opc, Opcode::Add);
    assert_eq!(out.ops()[3].opc, Opcode::ExitTb);
}

// -- Round-trip: labels and branches --

#[test]
fn serialize_labels_and_branches() {
    let mut ctx = Context::new();
    let env = ctx.new_fixed(Type::I64, 5, "env");
    let x1 = ctx.new_global(Type::I64, env, 8, "x1");
    let x2 = ctx.new_global(Type::I64, env, 16, "x2");

    let label = ctx.new_label();

    // brcond x1, x2, EQ, label
    let idx0 = ctx.next_op_idx();
    let mut op0 = Op::new(idx0, Opcode::BrCond, Type::I64);
    op0.nargs = 4;
    op0.args[0] = x1;
    op0.args[1] = x2;
    op0.args[2] = TempIdx(machina_core::Cond::Eq as u32);
    op0.args[3] = TempIdx(label);
    ctx.emit_op(op0);

    // set_label
    let idx1 = ctx.next_op_idx();
    let mut op1 = Op::new(idx1, Opcode::SetLabel, Type::I64);
    op1.nargs = 1;
    op1.args[0] = TempIdx(label);
    ctx.emit_op(op1);

    // exit_tb
    let idx2 = ctx.next_op_idx();
    let mut op2 = Op::new(idx2, Opcode::ExitTb, Type::I64);
    op2.nargs = 1;
    op2.args[0] = TempIdx(0);
    ctx.emit_op(op2);

    let out = round_trip(&ctx);

    assert_eq!(out.num_ops(), 3);
    assert_eq!(out.ops()[0].opc, Opcode::BrCond);
    assert_eq!(out.ops()[1].opc, Opcode::SetLabel);
    assert_eq!(out.ops()[2].opc, Opcode::ExitTb);

    // Labels should be reconstructed
    assert!(!out.labels().is_empty());
    assert_eq!(out.labels()[label as usize].id, label);
}

// -- Round-trip: unconditional branch --

#[test]
fn serialize_br() {
    let mut ctx = Context::new();
    let _env = ctx.new_fixed(Type::I64, 5, "env");

    let label = ctx.new_label();

    let idx0 = ctx.next_op_idx();
    let mut op0 = Op::new(idx0, Opcode::Br, Type::I64);
    op0.nargs = 1;
    op0.args[0] = TempIdx(label);
    ctx.emit_op(op0);

    let idx1 = ctx.next_op_idx();
    let mut op1 = Op::new(idx1, Opcode::SetLabel, Type::I64);
    op1.nargs = 1;
    op1.args[0] = TempIdx(label);
    ctx.emit_op(op1);

    let out = round_trip(&ctx);
    assert_eq!(out.num_ops(), 2);
    assert!(!out.labels().is_empty());
}

// -- Round-trip: multiple concatenated TBs --

#[test]
fn serialize_multiple_tbs() {
    // TB 0: simple add
    let mut ctx0 = Context::new();
    let env = ctx0.new_fixed(Type::I64, 5, "env");
    let x1 = ctx0.new_global(Type::I64, env, 8, "x1");
    let x2 = ctx0.new_global(Type::I64, env, 16, "x2");
    let tmp = ctx0.new_temp(Type::I64);
    let idx = ctx0.next_op_idx();
    ctx0.emit_op(Op::with_args(idx, Opcode::Add, Type::I64, &[tmp, x1, x2]));
    let idx = ctx0.next_op_idx();
    let mut exit = Op::new(idx, Opcode::ExitTb, Type::I64);
    exit.nargs = 1;
    exit.args[0] = TempIdx(0);
    ctx0.emit_op(exit);

    // TB 1: simple sub
    let mut ctx1 = Context::new();
    let env1 = ctx1.new_fixed(Type::I64, 5, "env");
    let y1 = ctx1.new_global(Type::I64, env1, 8, "x1");
    let y2 = ctx1.new_global(Type::I64, env1, 16, "x2");
    let tmp1 = ctx1.new_temp(Type::I64);
    let idx = ctx1.next_op_idx();
    ctx1.emit_op(Op::with_args(idx, Opcode::Sub, Type::I64, &[tmp1, y1, y2]));
    let idx = ctx1.next_op_idx();
    let mut exit1 = Op::new(idx, Opcode::ExitTb, Type::I64);
    exit1.nargs = 1;
    exit1.args[0] = TempIdx(1);
    ctx1.emit_op(exit1);

    let results = round_trip_multi(&[&ctx0, &ctx1]);
    assert_eq!(results.len(), 2);

    // TB 0
    assert_eq!(results[0].num_ops(), 2);
    assert_eq!(results[0].ops()[0].opc, Opcode::Add);
    assert_eq!(results[0].ops()[1].opc, Opcode::ExitTb);

    // TB 1
    assert_eq!(results[1].num_ops(), 2);
    assert_eq!(results[1].ops()[0].opc, Opcode::Sub);
    assert_eq!(results[1].ops()[1].opc, Opcode::ExitTb);
    assert_eq!(results[1].ops()[1].args[0], TempIdx(1));
}

// -- Round-trip: op params preserved --

#[test]
fn serialize_op_params() {
    let mut ctx = Context::new();
    let _env = ctx.new_fixed(Type::I64, 5, "env");

    let idx = ctx.next_op_idx();
    let mut op = Op::new(idx, Opcode::Extract, Type::I64);
    op.param1 = 7;
    op.param2 = 3;
    op.nargs = 4;
    op.args[0] = TempIdx(1);
    op.args[1] = TempIdx(2);
    op.args[2] = TempIdx(8); // pos
    op.args[3] = TempIdx(16); // len
    ctx.emit_op(op);

    let out = round_trip(&ctx);
    let op_out = &out.ops()[0];
    assert_eq!(op_out.opc, Opcode::Extract);
    assert_eq!(op_out.op_type, Type::I64);
    assert_eq!(op_out.param1, 7);
    assert_eq!(op_out.param2, 3);
    assert_eq!(op_out.nargs, 4);
}

// -- Round-trip: I32 type --

#[test]
fn serialize_i32_ops() {
    let mut ctx = Context::new();
    let env = ctx.new_fixed(Type::I64, 5, "env");
    let x1 = ctx.new_global(Type::I32, env, 0, "x1_32");
    let x2 = ctx.new_global(Type::I32, env, 4, "x2_32");
    let tmp = ctx.new_temp(Type::I32);

    let idx = ctx.next_op_idx();
    ctx.emit_op(Op::with_args(idx, Opcode::Add, Type::I32, &[tmp, x1, x2]));

    let out = round_trip(&ctx);
    assert_eq!(out.temp(TempIdx(1)).ty, Type::I32);
    assert_eq!(out.ops()[0].op_type, Type::I32);
}

// -- Deserialize: bad magic --

#[test]
fn deserialize_bad_magic() {
    let data =
        b"BAAD\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x01\x00\x00\x00";
    let mut cursor = Cursor::new(&data[..]);
    let result = serialize::deserialize(&mut cursor);
    assert!(result.is_err());
}

// -- Deserialize: empty file --

#[test]
fn deserialize_empty_file() {
    let data: &[u8] = &[];
    let mut cursor = Cursor::new(data);
    let result =
        serialize::deserialize(&mut cursor).expect("empty file should be OK");
    assert!(result.is_empty());
}
