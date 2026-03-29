use machina_core::op::*;
use machina_core::opcode::Opcode;
use machina_core::temp::TempIdx;
use machina_core::types::{RegSet, Type};

#[test]
fn op_new_defaults() {
    let op = Op::new(OpIdx(0), Opcode::Add, Type::I64);
    assert_eq!(op.opc, Opcode::Add);
    assert_eq!(op.op_type, Type::I64);
    assert_eq!(op.param1, 0);
    assert_eq!(op.param2, 0);
    assert_eq!(op.nargs, 0);
    assert_eq!(op.life, LifeData::default());
}

#[test]
fn op_with_args() {
    let args = [TempIdx(1), TempIdx(2), TempIdx(3)];
    let op = Op::with_args(OpIdx(0), Opcode::Add, Type::I32, &args);
    assert_eq!(op.nargs, 3);
    assert_eq!(op.args[0], TempIdx(1));
    assert_eq!(op.args[1], TempIdx(2));
    assert_eq!(op.args[2], TempIdx(3));
}

#[test]
fn op_arg_slices() {
    // Add: 1 oarg, 2 iargs, 0 cargs
    let args = [TempIdx(10), TempIdx(20), TempIdx(30)];
    let op = Op::with_args(OpIdx(0), Opcode::Add, Type::I64, &args);

    assert_eq!(op.oargs(), &[TempIdx(10)]);
    assert_eq!(op.iargs(), &[TempIdx(20), TempIdx(30)]);
    assert!(op.cargs().is_empty());
}

#[test]
fn op_arg_slices_with_cargs() {
    // BrCond: 0 oargs, 2 iargs, 2 cargs
    let args = [TempIdx(1), TempIdx(2), TempIdx(3), TempIdx(4)];
    let op = Op::with_args(OpIdx(0), Opcode::BrCond, Type::I64, &args);

    assert!(op.oargs().is_empty());
    assert_eq!(op.iargs(), &[TempIdx(1), TempIdx(2)]);
    assert_eq!(op.cargs(), &[TempIdx(3), TempIdx(4)]);
}

#[test]
fn life_data_dead_sync() {
    let mut life = LifeData::default();
    assert!(!life.is_dead(0));
    assert!(!life.is_sync(0));

    life.set_dead(0);
    assert!(life.is_dead(0));
    assert!(!life.is_sync(0));
    assert!(!life.is_dead(1));

    life.set_sync(1);
    assert!(life.is_sync(1));
    assert!(!life.is_dead(1));
}

#[test]
fn life_data_multiple_args() {
    let mut life = LifeData::default();
    life.set_dead(0);
    life.set_dead(2);
    life.set_sync(1);

    assert!(life.is_dead(0));
    assert!(!life.is_dead(1));
    assert!(life.is_dead(2));
    assert!(life.is_sync(1));
    assert!(!life.is_sync(0));
}

#[test]
fn op_output_pref() {
    let mut op = Op::new(OpIdx(0), Opcode::Add, Type::I64);
    op.output_pref[0] = RegSet::EMPTY.set(0).set(1);
    assert!(op.output_pref[0].contains(0));
    assert!(op.output_pref[0].contains(1));
    assert!(!op.output_pref[0].contains(2));
}
