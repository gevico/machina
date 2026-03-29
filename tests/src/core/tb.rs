use machina_core::tb::*;

#[test]
fn tb_new() {
    let tb = TranslationBlock::new(0x1000, 0, 0);
    assert_eq!(tb.pc, 0x1000);
    assert_eq!(tb.size, 0);
    assert_eq!(tb.icount, 0);
    assert_eq!(tb.jmp_insn_offset, [None, None]);
    assert_eq!(tb.jmp_reset_offset, [None, None]);
    let jmp = tb.jmp.lock().unwrap();
    assert_eq!(jmp.jmp_dest, [None, None]);
    assert!(jmp.jmp_list.is_empty());
    drop(jmp);
    assert_eq!(
        tb.exit_target.load(std::sync::atomic::Ordering::Relaxed),
        EXIT_TARGET_NONE
    );
    assert_eq!(tb.hash_next, None);
}

#[test]
fn tb_hash_deterministic() {
    let h1 = TranslationBlock::hash(0x1000, 0);
    let h2 = TranslationBlock::hash(0x1000, 0);
    assert_eq!(h1, h2);
}

#[test]
fn tb_hash_in_range() {
    for pc in [0u64, 0x1000, 0xFFFF_FFFF, 0xFFFF_FFFF_FFFF_FFFF] {
        let h = TranslationBlock::hash(pc, 0);
        assert!(h < TB_HASH_SIZE);
    }
}

#[test]
fn tb_hash_different_pc() {
    let h1 = TranslationBlock::hash(0x1000, 0);
    let h2 = TranslationBlock::hash(0x2000, 0);
    // Not guaranteed to differ, but very likely for these values
    assert_ne!(h1, h2);
}

#[test]
fn tb_hash_different_flags() {
    let h1 = TranslationBlock::hash(0x1000, 0);
    let h2 = TranslationBlock::hash(0x1000, 1);
    assert_ne!(h1, h2);
}

#[test]
fn tb_jmp_offsets() {
    let mut tb = TranslationBlock::new(0x1000, 0, 0);
    tb.set_jmp_insn_offset(0, 100);
    tb.set_jmp_reset_offset(0, 105);
    tb.set_jmp_insn_offset(1, 200);
    tb.set_jmp_reset_offset(1, 205);

    assert_eq!(tb.jmp_insn_offset[0], Some(100));
    assert_eq!(tb.jmp_reset_offset[0], Some(105));
    assert_eq!(tb.jmp_insn_offset[1], Some(200));
    assert_eq!(tb.jmp_reset_offset[1], Some(205));
}

#[test]
fn tb_max_insns() {
    assert_eq!(TranslationBlock::max_insns(0), 512);
    assert_eq!(TranslationBlock::max_insns(100), 100);
    assert_eq!(TranslationBlock::max_insns(1), 1);
}

#[test]
fn tb_cflags() {
    let cf = cflags::CF_SINGLE_STEP | 10;
    assert_eq!(cf & cflags::CF_COUNT_MASK, 10);
    assert_ne!(cf & cflags::CF_SINGLE_STEP, 0);
    assert_eq!(cf & cflags::CF_LAST_IO, 0);
}

#[test]
fn jump_cache_basic() {
    let mut cache = JumpCache::new();
    assert_eq!(cache.lookup(0x1000), None);

    cache.insert(0x1000, 42);
    assert_eq!(cache.lookup(0x1000), Some(42));

    cache.remove(0x1000);
    assert_eq!(cache.lookup(0x1000), None);
}

#[test]
fn jump_cache_overwrite() {
    let mut cache = JumpCache::new();
    cache.insert(0x1000, 1);
    cache.insert(0x1000, 2);
    assert_eq!(cache.lookup(0x1000), Some(2));
}

#[test]
fn jump_cache_invalidate() {
    let mut cache = JumpCache::new();
    cache.insert(0x1000, 1);
    cache.insert(0x2000, 2);
    cache.invalidate();
    assert_eq!(cache.lookup(0x1000), None);
    assert_eq!(cache.lookup(0x2000), None);
}

#[test]
fn jump_cache_collision() {
    let mut cache = JumpCache::new();
    // Two PCs that map to the same index will overwrite each other
    let pc1 = 0x0000;
    let pc2 = pc1 + (TB_JMP_CACHE_SIZE as u64 * 4);
    cache.insert(pc1, 1);
    cache.insert(pc2, 2);
    // pc1's entry was overwritten
    assert_eq!(cache.lookup(pc1), Some(2));
}
