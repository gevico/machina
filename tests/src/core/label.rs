use machina_core::label::*;

#[test]
fn label_new() {
    let l = Label::new(0);
    assert_eq!(l.id, 0);
    assert!(!l.present);
    assert!(!l.has_value);
    assert!(l.uses.is_empty());
}

#[test]
fn label_add_use() {
    let mut l = Label::new(1);
    l.add_use(100, RelocKind::Rel32);
    l.add_use(200, RelocKind::Rel32);
    assert_eq!(l.uses.len(), 2);
    assert_eq!(l.uses[0].offset, 100);
    assert_eq!(l.uses[1].offset, 200);
    assert!(l.has_pending_uses());
}

#[test]
fn label_resolve() {
    let mut l = Label::new(2);
    l.add_use(50, RelocKind::Rel32);
    assert!(l.has_pending_uses());

    l.set_value(300);
    assert!(l.present);
    assert!(l.has_value);
    assert_eq!(l.value, 300);
    assert!(!l.has_pending_uses());
}

#[test]
fn label_no_uses_not_pending() {
    let l = Label::new(3);
    assert!(!l.has_pending_uses());
}

#[test]
fn label_reloc_kind() {
    let u = LabelUse {
        offset: 42,
        kind: RelocKind::Rel32,
    };
    assert_eq!(u.kind, RelocKind::Rel32);
    assert_eq!(u.offset, 42);
}
