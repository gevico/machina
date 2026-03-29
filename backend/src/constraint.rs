use machina_core::op::MAX_OP_ARGS;
use machina_core::RegSet;

/// Constraint for a single argument of an IR op.
///
/// Maps to QEMU's `TCGArgConstraint`. Each arg has a set of
/// allowed registers and optional alias/newreg flags.
#[derive(Debug, Clone, Copy)]
pub struct ArgConstraint {
    /// Allowed host registers for this argument.
    pub regs: RegSet,
    /// Output aliases an input (output takes input's register).
    pub oalias: bool,
    /// Input is aliased to an output (input may be reused).
    pub ialias: bool,
    /// Index of the aliased arg (input idx for oalias,
    /// output idx for ialias).
    pub alias_index: u8,
    /// Output must not overlap any input register.
    pub newreg: bool,
}

impl ArgConstraint {
    pub const UNUSED: Self = Self {
        regs: RegSet::EMPTY,
        oalias: false,
        ialias: false,
        alias_index: 0,
        newreg: false,
    };
}

/// Per-opcode constraint descriptor.
///
/// Maps to QEMU's per-opcode `TCGArgConstraint` array built
/// by `C_O*_I*` macros.
#[derive(Debug, Clone, Copy)]
pub struct OpConstraint {
    pub args: [ArgConstraint; MAX_OP_ARGS],
}

impl OpConstraint {
    pub const EMPTY: Self = Self {
        args: [ArgConstraint::UNUSED; MAX_OP_ARGS],
    };
}

// -- Argument builders --

/// Regular register constraint (any reg in `regs`).
pub const fn r(regs: RegSet) -> ArgConstraint {
    ArgConstraint {
        regs,
        oalias: false,
        ialias: false,
        alias_index: 0,
        newreg: false,
    }
}

/// Fixed single-register constraint (e.g. RCX for shifts).
pub const fn fixed(reg: u8) -> ArgConstraint {
    ArgConstraint {
        regs: RegSet::from_raw(1u64 << reg),
        oalias: false,
        ialias: false,
        alias_index: 0,
        newreg: false,
    }
}

/// Newreg output constraint — must not overlap any input.
pub const fn newreg(regs: RegSet) -> ArgConstraint {
    ArgConstraint {
        regs,
        oalias: false,
        ialias: false,
        alias_index: 0,
        newreg: true,
    }
}

// -- OpConstraint builders --

/// 1 output, 1 input, output aliases input 0.
pub const fn o1_i1_alias(o0: RegSet, _i0: RegSet) -> OpConstraint {
    let mut args = [ArgConstraint::UNUSED; MAX_OP_ARGS];
    args[0] = ArgConstraint {
        regs: o0,
        oalias: true,
        ialias: false,
        alias_index: 0,
        newreg: false,
    };
    args[1] = ArgConstraint {
        regs: o0,
        oalias: false,
        ialias: true,
        alias_index: 0,
        newreg: false,
    };
    OpConstraint { args }
}

/// 1 output, 1 input, no alias.
pub const fn o1_i1(o0: RegSet, i0: RegSet) -> OpConstraint {
    let mut args = [ArgConstraint::UNUSED; MAX_OP_ARGS];
    args[0] = r(o0);
    args[1] = r(i0);
    OpConstraint { args }
}

/// 1 output, 2 inputs, no alias.
pub const fn o1_i2(o0: RegSet, i0: RegSet, i1: RegSet) -> OpConstraint {
    let mut args = [ArgConstraint::UNUSED; MAX_OP_ARGS];
    args[0] = r(o0);
    args[1] = r(i0);
    args[2] = r(i1);
    OpConstraint { args }
}

/// 1 output, 2 inputs, output aliases input 0.
pub const fn o1_i2_alias(o0: RegSet, _i0: RegSet, i1: RegSet) -> OpConstraint {
    let mut args = [ArgConstraint::UNUSED; MAX_OP_ARGS];
    args[0] = ArgConstraint {
        regs: o0,
        oalias: true,
        ialias: false,
        alias_index: 0,
        newreg: false,
    };
    args[1] = ArgConstraint {
        regs: o0,
        oalias: false,
        ialias: true,
        alias_index: 0,
        newreg: false,
    };
    args[2] = r(i1);
    OpConstraint { args }
}

/// 1 output, 2 inputs, output aliases input 0,
/// input 1 is a fixed register.
pub const fn o1_i2_alias_fixed(
    o0: RegSet,
    _i0: RegSet,
    i1_reg: u8,
) -> OpConstraint {
    let mut args = [ArgConstraint::UNUSED; MAX_OP_ARGS];
    args[0] = ArgConstraint {
        regs: o0,
        oalias: true,
        ialias: false,
        alias_index: 0,
        newreg: false,
    };
    args[1] = ArgConstraint {
        regs: o0,
        oalias: false,
        ialias: true,
        alias_index: 0,
        newreg: false,
    };
    args[2] = fixed(i1_reg);
    OpConstraint { args }
}

/// 0 outputs, 2 inputs.
pub const fn o0_i2(i0: RegSet, i1: RegSet) -> OpConstraint {
    let mut args = [ArgConstraint::UNUSED; MAX_OP_ARGS];
    args[0] = r(i0);
    args[1] = r(i1);
    OpConstraint { args }
}

/// 1 newreg output, 2 inputs.
pub const fn n1_i2(o0: RegSet, i0: RegSet, i1: RegSet) -> OpConstraint {
    let mut args = [ArgConstraint::UNUSED; MAX_OP_ARGS];
    args[0] = newreg(o0);
    args[1] = r(i0);
    args[2] = r(i1);
    OpConstraint { args }
}

/// 0 outputs, 1 input.
pub const fn o0_i1(i0: RegSet) -> OpConstraint {
    let mut args = [ArgConstraint::UNUSED; MAX_OP_ARGS];
    args[0] = r(i0);
    OpConstraint { args }
}

/// 2 fixed outputs, 2 inputs (o0 alias i0, i1 free).
/// For MulS2/MulU2: o0=RAX, o1=RDX, i0=RAX, i1=R.
pub const fn o2_i2_fixed(o0_reg: u8, o1_reg: u8, i1: RegSet) -> OpConstraint {
    let mut args = [ArgConstraint::UNUSED; MAX_OP_ARGS];
    args[0] = ArgConstraint {
        regs: RegSet::from_raw(1u64 << o0_reg),
        oalias: true,
        ialias: false,
        alias_index: 0,
        newreg: false,
    };
    args[1] = fixed(o1_reg);
    args[2] = ArgConstraint {
        regs: RegSet::from_raw(1u64 << o0_reg),
        oalias: false,
        ialias: true,
        alias_index: 0,
        newreg: false,
    };
    args[3] = r(i1);
    OpConstraint { args }
}

/// 2 fixed outputs, 3 inputs (o0 alias i0, o1 alias i1,
/// i2 free).
/// For DivS2/DivU2: o0=RAX, o1=RDX, i0=RAX, i1=RDX,
/// i2=R.
pub const fn o2_i3_fixed(o0_reg: u8, o1_reg: u8, i2: RegSet) -> OpConstraint {
    let mut args = [ArgConstraint::UNUSED; MAX_OP_ARGS];
    args[0] = ArgConstraint {
        regs: RegSet::from_raw(1u64 << o0_reg),
        oalias: true,
        ialias: false,
        alias_index: 0,
        newreg: false,
    };
    args[1] = ArgConstraint {
        regs: RegSet::from_raw(1u64 << o1_reg),
        oalias: true,
        ialias: false,
        alias_index: 1,
        newreg: false,
    };
    args[2] = ArgConstraint {
        regs: RegSet::from_raw(1u64 << o0_reg),
        oalias: false,
        ialias: true,
        alias_index: 0,
        newreg: false,
    };
    args[3] = ArgConstraint {
        regs: RegSet::from_raw(1u64 << o1_reg),
        oalias: false,
        ialias: true,
        alias_index: 1,
        newreg: false,
    };
    args[4] = r(i2);
    OpConstraint { args }
}

/// 1 output, 4 inputs, output aliases input 2.
/// For MovCond: CMP i0,i1 -> CMOV d=i2,i3.
pub const fn o1_i4_alias2(
    o0: RegSet,
    i0: RegSet,
    i1: RegSet,
    _i2: RegSet,
    i3: RegSet,
) -> OpConstraint {
    let mut args = [ArgConstraint::UNUSED; MAX_OP_ARGS];
    args[0] = ArgConstraint {
        regs: o0,
        oalias: true,
        ialias: false,
        alias_index: 2,
        newreg: false,
    };
    args[1] = r(i0);
    args[2] = r(i1);
    args[3] = ArgConstraint {
        regs: o0,
        oalias: false,
        ialias: true,
        alias_index: 0,
        newreg: false,
    };
    args[4] = r(i3);
    OpConstraint { args }
}
