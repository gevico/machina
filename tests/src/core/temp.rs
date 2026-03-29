use machina_core::temp::*;
use machina_core::types::*;

#[test]
fn temp_ebb_defaults() {
    let t = Temp::new_ebb(TempIdx(0), Type::I64);
    assert_eq!(t.kind, TempKind::Ebb);
    assert_eq!(t.ty, Type::I64);
    assert_eq!(t.base_type, Type::I64);
    assert_eq!(t.val_type, TempVal::Dead);
    assert_eq!(t.reg, None);
    assert!(!t.mem_coherent);
    assert!(!t.is_const());
    assert!(!t.is_global());
}

#[test]
fn temp_tb_kind() {
    let t = Temp::new_tb(TempIdx(1), Type::I32);
    assert_eq!(t.kind, TempKind::Tb);
    assert_eq!(t.val_type, TempVal::Dead);
}

#[test]
fn temp_const_value() {
    let t = Temp::new_const(TempIdx(2), Type::I64, 0xDEAD_BEEF);
    assert!(t.is_const());
    assert_eq!(t.val, 0xDEAD_BEEF);
    assert_eq!(t.val_type, TempVal::Const);
}

#[test]
fn temp_global_mem_info() {
    let env_idx = TempIdx(0);
    let t = Temp::new_global(TempIdx(1), Type::I64, env_idx, 128, "pc");
    assert!(t.is_global());
    assert_eq!(t.mem_base, Some(env_idx));
    assert_eq!(t.mem_offset, 128);
    assert_eq!(t.name, Some("pc"));
    assert_eq!(t.val_type, TempVal::Mem);
    assert!(t.mem_coherent);
    assert!(t.mem_allocated);
}

#[test]
fn temp_fixed_reg() {
    let t = Temp::new_fixed(TempIdx(3), Type::I64, 5, "rbp");
    assert!(t.is_fixed());
    assert!(t.is_global_or_fixed());
    assert_eq!(t.reg, Some(5));
    assert_eq!(t.val_type, TempVal::Reg);
}

#[test]
fn temp_global_or_fixed() {
    let ebb = Temp::new_ebb(TempIdx(0), Type::I32);
    assert!(!ebb.is_global_or_fixed());

    let tb = Temp::new_tb(TempIdx(1), Type::I32);
    assert!(!tb.is_global_or_fixed());

    let g = Temp::new_global(TempIdx(2), Type::I64, TempIdx(0), 0, "x");
    assert!(g.is_global_or_fixed());

    let f = Temp::new_fixed(TempIdx(3), Type::I64, 0, "rax");
    assert!(f.is_global_or_fixed());
}
