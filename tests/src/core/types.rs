use machina_core::types::*;

#[test]
fn type_sizes() {
    assert_eq!(Type::I32.size_bits(), 32);
    assert_eq!(Type::I64.size_bits(), 64);
    assert_eq!(Type::I128.size_bits(), 128);
    assert_eq!(Type::V64.size_bits(), 64);
    assert_eq!(Type::V128.size_bits(), 128);
    assert_eq!(Type::V256.size_bits(), 256);

    assert_eq!(Type::I32.size_bytes(), 4);
    assert_eq!(Type::I64.size_bytes(), 8);
}

#[test]
fn type_classification() {
    assert!(Type::I32.is_integer());
    assert!(Type::I64.is_integer());
    assert!(Type::I128.is_integer());
    assert!(!Type::I32.is_vector());

    assert!(Type::V64.is_vector());
    assert!(Type::V128.is_vector());
    assert!(Type::V256.is_vector());
    assert!(!Type::V64.is_integer());
}

#[test]
fn cond_invert() {
    assert_eq!(Cond::Eq.invert(), Cond::Ne);
    assert_eq!(Cond::Ne.invert(), Cond::Eq);
    assert_eq!(Cond::Lt.invert(), Cond::Ge);
    assert_eq!(Cond::Ge.invert(), Cond::Lt);
    assert_eq!(Cond::Ltu.invert(), Cond::Geu);
    assert_eq!(Cond::Never.invert(), Cond::Always);
    assert_eq!(Cond::TstEq.invert(), Cond::TstNe);
}

#[test]
fn cond_invert_is_involution() {
    let conds = [
        Cond::Never,
        Cond::Always,
        Cond::Eq,
        Cond::Ne,
        Cond::Lt,
        Cond::Ge,
        Cond::Le,
        Cond::Gt,
        Cond::Ltu,
        Cond::Geu,
        Cond::Leu,
        Cond::Gtu,
        Cond::TstEq,
        Cond::TstNe,
    ];
    for c in conds {
        assert_eq!(
            c.invert().invert(),
            c,
            "invert is not involution for {:?}",
            c
        );
    }
}

#[test]
fn cond_swap() {
    assert_eq!(Cond::Lt.swap(), Cond::Gt);
    assert_eq!(Cond::Gt.swap(), Cond::Lt);
    assert_eq!(Cond::Le.swap(), Cond::Ge);
    assert_eq!(Cond::Ge.swap(), Cond::Le);
    assert_eq!(Cond::Ltu.swap(), Cond::Gtu);
    assert_eq!(Cond::Eq.swap(), Cond::Eq);
    assert_eq!(Cond::Ne.swap(), Cond::Ne);
}

#[test]
fn cond_swap_is_involution() {
    let conds = [
        Cond::Eq,
        Cond::Ne,
        Cond::Lt,
        Cond::Ge,
        Cond::Le,
        Cond::Gt,
        Cond::Ltu,
        Cond::Geu,
        Cond::Leu,
        Cond::Gtu,
    ];
    for c in conds {
        assert_eq!(c.swap().swap(), c, "swap is not involution for {:?}", c);
    }
}

#[test]
fn cond_signed_unsigned() {
    assert!(Cond::Lt.is_signed());
    assert!(Cond::Ge.is_signed());
    assert!(!Cond::Ltu.is_signed());
    assert!(Cond::Ltu.is_unsigned());
    assert!(Cond::Geu.is_unsigned());
    assert!(!Cond::Lt.is_unsigned());
    assert!(!Cond::Eq.is_signed());
    assert!(!Cond::Eq.is_unsigned());
}

#[test]
fn memop_constructors() {
    assert_eq!(MemOp::ub().size_bytes(), 1);
    assert!(!MemOp::ub().is_signed());
    assert_eq!(MemOp::sb().size_bytes(), 1);
    assert!(MemOp::sb().is_signed());
    assert_eq!(MemOp::uw().size_bytes(), 2);
    assert_eq!(MemOp::ul().size_bytes(), 4);
    assert_eq!(MemOp::uq().size_bytes(), 8);
}

#[test]
fn memop_bswap() {
    let op = MemOp::new(MemOp::SIZE_32 | MemOp::BSWAP);
    assert!(op.is_bswap());
    assert_eq!(op.size_bytes(), 4);
    assert!(!op.is_signed());
}

#[test]
fn regset_basic() {
    let empty = RegSet::EMPTY;
    assert!(empty.is_empty());
    assert_eq!(empty.count(), 0);
    assert_eq!(empty.first(), None);

    let s = empty.set(0).set(5).set(15);
    assert!(!s.is_empty());
    assert_eq!(s.count(), 3);
    assert!(s.contains(0));
    assert!(s.contains(5));
    assert!(s.contains(15));
    assert!(!s.contains(1));
    assert_eq!(s.first(), Some(0));
}

#[test]
fn regset_operations() {
    let a = RegSet::EMPTY.set(1).set(3).set(5);
    let b = RegSet::EMPTY.set(3).set(5).set(7);

    let u = a.union(b);
    assert_eq!(u.count(), 4);
    assert!(u.contains(1));
    assert!(u.contains(7));

    let i = a.intersect(b);
    assert_eq!(i.count(), 2);
    assert!(i.contains(3));
    assert!(i.contains(5));
    assert!(!i.contains(1));

    let d = a.subtract(b);
    assert_eq!(d.count(), 1);
    assert!(d.contains(1));
    assert!(!d.contains(3));
}

#[test]
fn regset_clear() {
    let s = RegSet::EMPTY.set(3).set(7);
    let s2 = s.clear(3);
    assert!(!s2.contains(3));
    assert!(s2.contains(7));
    assert_eq!(s2.count(), 1);
}

#[test]
fn tempval_variants() {
    assert_ne!(TempVal::Dead, TempVal::Reg);
    assert_ne!(TempVal::Reg, TempVal::Mem);
    assert_ne!(TempVal::Mem, TempVal::Const);
}
