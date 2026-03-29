use machina_core::op::MAX_OP_ARGS;
use machina_core::opcode::*;
use machina_core::types::Type;

fn assert_def(
    opc: Opcode,
    nb_oargs: u8,
    nb_iargs: u8,
    nb_cargs: u8,
    flags: OpFlags,
) {
    let def = opc.def();
    assert_eq!(def.nb_oargs, nb_oargs, "{:?} nb_oargs", opc);
    assert_eq!(def.nb_iargs, nb_iargs, "{:?} nb_iargs", opc);
    assert_eq!(def.nb_cargs, nb_cargs, "{:?} nb_cargs", opc);
    assert_eq!(
        def.nb_args(),
        nb_oargs + nb_iargs + nb_cargs,
        "{:?} nb_args",
        opc
    );
    assert!(
        def.nb_args() as usize <= MAX_OP_ARGS,
        "{:?} args exceed MAX_OP_ARGS",
        opc
    );
    assert_eq!(def.flags.bits(), flags.bits(), "{:?} flags", opc);
    assert!(!def.name.is_empty(), "{:?} empty name", opc);
}

fn assert_group(
    seen: &mut [bool],
    ops: &[Opcode],
    nb_oargs: u8,
    nb_iargs: u8,
    nb_cargs: u8,
    flags: OpFlags,
) {
    for &opc in ops {
        let idx = opc as usize;
        assert!(!seen[idx], "opcode {:?} duplicated", opc);
        seen[idx] = true;
        assert_def(opc, nb_oargs, nb_iargs, nb_cargs, flags);
    }
}

#[test]
fn opcode_def_table_size() {
    assert_eq!(OPCODE_DEFS.len(), Opcode::Count as usize);
}

#[test]
fn opcode_def_lookup() {
    let def = Opcode::Add.def();
    assert_eq!(def.name, "add");
    assert_eq!(def.nb_oargs, 1);
    assert_eq!(def.nb_iargs, 2);
    assert_eq!(def.nb_cargs, 0);
    assert!(def.flags.contains(OpFlags::INT));
}

#[test]
fn opcode_def_nb_args() {
    assert_eq!(Opcode::Add.def().nb_args(), 3); // 1 out + 2 in
    assert_eq!(Opcode::Not.def().nb_args(), 2); // 1 out + 1 in
    assert_eq!(Opcode::BrCond.def().nb_args(), 4); // 0 out + 2 in + 2 const
    assert_eq!(Opcode::Nop.def().nb_args(), 0);
}

#[test]
fn opcode_int_polymorphic() {
    assert!(Opcode::Add.is_int_polymorphic());
    assert!(Opcode::Sub.is_int_polymorphic());
    assert!(Opcode::And.is_int_polymorphic());
    assert!(Opcode::Shl.is_int_polymorphic());
    assert!(!Opcode::ExtI32I64.is_int_polymorphic());
    assert!(!Opcode::Br.is_int_polymorphic());
    assert!(!Opcode::Nop.is_int_polymorphic());
}

#[test]
fn opcode_fixed_type() {
    assert_eq!(Opcode::ExtI32I64.fixed_type(), Some(Type::I64));
    assert_eq!(Opcode::ExtrlI64I32.fixed_type(), Some(Type::I32));
    assert_eq!(Opcode::Add.fixed_type(), None);
}

#[test]
fn opcode_control_flow_flags() {
    let br_def = Opcode::Br.def();
    assert!(br_def.flags.contains(OpFlags::BB_END));

    let brcond_def = Opcode::BrCond.def();
    assert!(brcond_def.flags.contains(OpFlags::BB_END));
    assert!(brcond_def.flags.contains(OpFlags::COND_BRANCH));

    let exit_def = Opcode::ExitTb.def();
    assert!(exit_def.flags.contains(OpFlags::BB_EXIT));
    assert!(exit_def.flags.contains(OpFlags::BB_END));

    let goto_def = Opcode::GotoTb.def();
    assert!(goto_def.flags.contains(OpFlags::BB_EXIT));
}

#[test]
fn opcode_side_effects() {
    assert!(Opcode::QemuLd.def().flags.contains(OpFlags::SIDE_EFFECTS));
    assert!(Opcode::QemuSt.def().flags.contains(OpFlags::SIDE_EFFECTS));
    assert!(Opcode::QemuLd.def().flags.contains(OpFlags::CALL_CLOBBER));
}

#[test]
fn opcode_carry_flags() {
    assert!(Opcode::AddCO.def().flags.contains(OpFlags::CARRY_OUT));
    assert!(!Opcode::AddCO.def().flags.contains(OpFlags::CARRY_IN));
    assert!(Opcode::AddCI.def().flags.contains(OpFlags::CARRY_IN));
    assert!(!Opcode::AddCI.def().flags.contains(OpFlags::CARRY_OUT));
    assert!(Opcode::AddCIO.def().flags.contains(OpFlags::CARRY_IN));
    assert!(Opcode::AddCIO.def().flags.contains(OpFlags::CARRY_OUT));
}

#[test]
fn opcode_names_unique() {
    let mut names: Vec<&str> = OPCODE_DEFS.iter().map(|d| d.name).collect();
    let len_before = names.len();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), len_before, "duplicate opcode names found");
}

#[test]
fn opcode_load_store_args() {
    // Host loads: 1 output, 1 input (base), 1 const (offset)
    let ld = Opcode::Ld.def();
    assert_eq!(ld.nb_oargs, 1);
    assert_eq!(ld.nb_iargs, 1);
    assert_eq!(ld.nb_cargs, 1);

    // Host stores: 0 output, 2 input (value, base), 1 const (offset)
    let st = Opcode::St.def();
    assert_eq!(st.nb_oargs, 0);
    assert_eq!(st.nb_iargs, 2);
    assert_eq!(st.nb_cargs, 1);
}

#[test]
fn opcode_def_full_coverage() {
    let int = OpFlags::INT;
    let np = OpFlags::NOT_PRESENT;
    let se = OpFlags::SIDE_EFFECTS;
    let cc = OpFlags::CALL_CLOBBER;
    let be = OpFlags::BB_END;
    let bx = OpFlags::BB_EXIT;
    let cb = OpFlags::COND_BRANCH;
    let co = OpFlags::CARRY_OUT;
    let ci = OpFlags::CARRY_IN;
    let vc = OpFlags::VECTOR;
    let none = OpFlags::NONE;

    let int_np = int.union(np);
    let int_co = int.union(co);
    let int_ci = int.union(ci);
    let int_ci_co = int.union(ci).union(co);
    let be_np = be.union(np);
    let be_cb = be.union(cb);
    let be_cb_int = be.union(cb).union(int);
    let bx_be_np = bx.union(be).union(np);
    let bx_be = bx.union(be);
    let cc_np = cc.union(np);
    let cc_se_int = cc.union(se).union(int);
    let vc_np = vc.union(np);

    let mut seen = vec![false; Opcode::Count as usize];

    assert_group(&mut seen, &[Opcode::Mov], 1, 1, 0, int_np);
    assert_group(
        &mut seen,
        &[Opcode::SetCond, Opcode::NegSetCond],
        1,
        2,
        1,
        int,
    );
    assert_group(&mut seen, &[Opcode::MovCond], 1, 4, 1, int);

    assert_group(
        &mut seen,
        &[
            Opcode::Add,
            Opcode::Sub,
            Opcode::Mul,
            Opcode::DivS,
            Opcode::DivU,
            Opcode::RemS,
            Opcode::RemU,
            Opcode::MulSH,
            Opcode::MulUH,
            Opcode::And,
            Opcode::Or,
            Opcode::Xor,
            Opcode::AndC,
            Opcode::OrC,
            Opcode::Eqv,
            Opcode::Nand,
            Opcode::Nor,
            Opcode::Shl,
            Opcode::Shr,
            Opcode::Sar,
            Opcode::RotL,
            Opcode::RotR,
            Opcode::Clz,
            Opcode::Ctz,
        ],
        1,
        2,
        0,
        int,
    );
    assert_group(
        &mut seen,
        &[Opcode::Neg, Opcode::Not, Opcode::CtPop],
        1,
        1,
        0,
        int,
    );
    assert_group(&mut seen, &[Opcode::DivS2, Opcode::DivU2], 2, 3, 0, int);
    assert_group(&mut seen, &[Opcode::MulS2, Opcode::MulU2], 2, 2, 0, int);
    assert_group(
        &mut seen,
        &[Opcode::AddCO, Opcode::AddC1O, Opcode::SubBO, Opcode::SubB1O],
        1,
        2,
        0,
        int_co,
    );
    assert_group(&mut seen, &[Opcode::AddCI, Opcode::SubBI], 1, 2, 0, int_ci);
    assert_group(
        &mut seen,
        &[Opcode::AddCIO, Opcode::SubBIO],
        1,
        2,
        0,
        int_ci_co,
    );

    assert_group(
        &mut seen,
        &[Opcode::Extract, Opcode::SExtract],
        1,
        1,
        2,
        int,
    );
    assert_group(&mut seen, &[Opcode::Deposit], 1, 2, 2, int);
    assert_group(&mut seen, &[Opcode::Extract2], 1, 2, 1, int);
    assert_group(
        &mut seen,
        &[Opcode::Bswap16, Opcode::Bswap32, Opcode::Bswap64],
        1,
        1,
        1,
        int,
    );

    assert_group(&mut seen, &[Opcode::BrCond2I32], 0, 4, 2, be_cb);
    assert_group(&mut seen, &[Opcode::SetCond2I32], 1, 4, 1, none);
    assert_group(
        &mut seen,
        &[
            Opcode::ExtI32I64,
            Opcode::ExtUI32I64,
            Opcode::ExtrlI64I32,
            Opcode::ExtrhI64I32,
        ],
        1,
        1,
        0,
        none,
    );

    assert_group(
        &mut seen,
        &[
            Opcode::Ld8U,
            Opcode::Ld8S,
            Opcode::Ld16U,
            Opcode::Ld16S,
            Opcode::Ld32U,
            Opcode::Ld32S,
            Opcode::Ld,
        ],
        1,
        1,
        1,
        int,
    );
    assert_group(
        &mut seen,
        &[Opcode::St8, Opcode::St16, Opcode::St32, Opcode::St],
        0,
        2,
        1,
        int,
    );

    assert_group(&mut seen, &[Opcode::QemuLd], 1, 1, 1, cc_se_int);
    assert_group(&mut seen, &[Opcode::QemuSt], 0, 2, 1, cc_se_int);
    assert_group(&mut seen, &[Opcode::QemuLd2], 2, 1, 1, cc_se_int);
    assert_group(&mut seen, &[Opcode::QemuSt2], 0, 3, 1, cc_se_int);

    assert_group(&mut seen, &[Opcode::Br, Opcode::SetLabel], 0, 0, 1, be_np);
    assert_group(&mut seen, &[Opcode::BrCond], 0, 2, 2, be_cb_int);
    assert_group(
        &mut seen,
        &[Opcode::GotoTb, Opcode::ExitTb],
        0,
        0,
        1,
        bx_be_np,
    );
    assert_group(&mut seen, &[Opcode::GotoPtr], 0, 1, 0, bx_be);
    assert_group(&mut seen, &[Opcode::Mb, Opcode::PluginCb], 0, 0, 1, np);

    assert_group(&mut seen, &[Opcode::Call], 1, 6, 2, cc_np);
    assert_group(&mut seen, &[Opcode::PluginMemCb], 0, 1, 1, np);
    assert_group(&mut seen, &[Opcode::Nop], 0, 0, 0, np);
    assert_group(&mut seen, &[Opcode::Discard], 1, 0, 0, np);
    assert_group(&mut seen, &[Opcode::InsnStart], 0, 0, 2, np);

    assert_group(&mut seen, &[Opcode::MovVec], 1, 1, 0, vc_np);
    assert_group(
        &mut seen,
        &[
            Opcode::DupVec,
            Opcode::NegVec,
            Opcode::AbsVec,
            Opcode::NotVec,
        ],
        1,
        1,
        0,
        vc,
    );
    assert_group(
        &mut seen,
        &[
            Opcode::Dup2Vec,
            Opcode::AddVec,
            Opcode::SubVec,
            Opcode::MulVec,
            Opcode::SsaddVec,
            Opcode::UsaddVec,
            Opcode::SssubVec,
            Opcode::UssubVec,
            Opcode::SminVec,
            Opcode::UminVec,
            Opcode::SmaxVec,
            Opcode::UmaxVec,
            Opcode::AndVec,
            Opcode::OrVec,
            Opcode::XorVec,
            Opcode::AndcVec,
            Opcode::OrcVec,
            Opcode::NandVec,
            Opcode::NorVec,
            Opcode::EqvVec,
            Opcode::ShlsVec,
            Opcode::ShrsVec,
            Opcode::SarsVec,
            Opcode::RotlsVec,
            Opcode::ShlvVec,
            Opcode::ShrvVec,
            Opcode::SarvVec,
            Opcode::RotlvVec,
            Opcode::RotrvVec,
        ],
        1,
        2,
        0,
        vc,
    );
    assert_group(
        &mut seen,
        &[
            Opcode::LdVec,
            Opcode::DupmVec,
            Opcode::ShliVec,
            Opcode::ShriVec,
            Opcode::SariVec,
            Opcode::RotliVec,
        ],
        1,
        1,
        1,
        vc,
    );
    assert_group(&mut seen, &[Opcode::StVec], 0, 2, 1, vc);
    assert_group(&mut seen, &[Opcode::CmpVec], 1, 2, 1, vc);
    assert_group(&mut seen, &[Opcode::BitselVec], 1, 3, 0, vc);
    assert_group(&mut seen, &[Opcode::CmpselVec], 1, 4, 1, vc);

    let missing: Vec<&'static str> = seen
        .iter()
        .enumerate()
        .filter(|(_, covered)| !**covered)
        .map(|(idx, _)| OPCODE_DEFS[idx].name)
        .collect();
    assert!(missing.is_empty(), "opcodes not covered: {:?}", missing);
}
