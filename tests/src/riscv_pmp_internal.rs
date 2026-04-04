use machina_guest_riscv::riscv::pmp::{napot_range, PmpAddrMatch};

#[test]
fn test_napot_decode() {
    // pmpaddr = 0x1F (binary ...0001_1111, G=5)
    // size = 2^(5+3) = 256, base = 0
    let (base, end) = napot_range(0x1F);
    assert_eq!(base, 0);
    assert_eq!(end, 256);

    // pmpaddr = 0x23 (binary ...0010_0011, G=2)
    // size = 2^(2+3) = 32
    // base = (0x23 << 2) & !31 = 0x8C & !0x1F
    //      = 140 & !31 = 128
    let (base, end) = napot_range(0x23);
    assert_eq!(base, 128);
    assert_eq!(end, 160);
}

#[test]
fn test_pmp_addr_match_from_cfg() {
    assert_eq!(PmpAddrMatch::from_cfg(0x00), PmpAddrMatch::Off,);
    assert_eq!(PmpAddrMatch::from_cfg(0x08), PmpAddrMatch::Tor,);
    assert_eq!(PmpAddrMatch::from_cfg(0x10), PmpAddrMatch::Na4,);
    assert_eq!(PmpAddrMatch::from_cfg(0x18), PmpAddrMatch::Napot,);
}
