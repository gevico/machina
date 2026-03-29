use crate::constraint::*;
use crate::x86_64::regs::{Reg, ALLOCATABLE_REGS};
use machina_core::Opcode;

const R: machina_core::RegSet = ALLOCATABLE_REGS;
const R_NO_RCX: machina_core::RegSet = machina_core::RegSet::from_raw(
    ALLOCATABLE_REGS.raw() & !(1u64 << Reg::Rcx as u64),
);
const R_NO_RAX_RDX: machina_core::RegSet = machina_core::RegSet::from_raw(
    ALLOCATABLE_REGS.raw()
        & !((1u64 << Reg::Rax as u64) | (1u64 << Reg::Rdx as u64)),
);

/// Return the static register constraint for an opcode on
/// x86-64.
///
/// Mirrors QEMU's `tcg_target_op_def()` in
/// `tcg/i386/tcg-target.c.inc`.
pub fn op_constraint(opc: Opcode) -> &'static OpConstraint {
    match opc {
        // -- Three-address via LEA --
        Opcode::Add => {
            static C: OpConstraint = o1_i2(R, R, R);
            &C
        }
        // -- Destructive binary (output aliases input 0) --
        Opcode::Sub | Opcode::Mul | Opcode::And | Opcode::Or | Opcode::Xor => {
            static C: OpConstraint = o1_i2_alias(R, R, R);
            &C
        }
        // -- Destructive unary (output aliases input 0) --
        Opcode::Neg | Opcode::Not => {
            static C: OpConstraint = o1_i1_alias(R, R);
            &C
        }
        // -- Shifts/rotates: output aliases input 0, count in RCX --
        Opcode::Shl
        | Opcode::Shr
        | Opcode::Sar
        | Opcode::RotL
        | Opcode::RotR => {
            static C: OpConstraint =
                o1_i2_alias_fixed(R_NO_RCX, R_NO_RCX, Reg::Rcx as u8);
            &C
        }
        // -- SetCond: newreg output (setcc writes low byte) --
        Opcode::SetCond => {
            static C: OpConstraint = n1_i2(R, R, R);
            &C
        }
        // -- NegSetCond: newreg output --
        Opcode::NegSetCond => {
            static C: OpConstraint = n1_i2(R, R, R);
            &C
        }
        // -- MovCond: output aliases input 2 (v1) --
        Opcode::MovCond => {
            static C: OpConstraint = o1_i4_alias2(R, R, R, R, R);
            &C
        }
        // -- BrCond: no outputs --
        Opcode::BrCond => {
            static C: OpConstraint = o0_i2(R, R);
            &C
        }
        // -- Double-width multiply: RAX:RDX result --
        Opcode::MulS2 | Opcode::MulU2 => {
            static C: OpConstraint =
                o2_i2_fixed(Reg::Rax as u8, Reg::Rdx as u8, R_NO_RAX_RDX);
            &C
        }
        // -- Double-width divide: RDX:RAX input/output --
        Opcode::DivS2 | Opcode::DivU2 => {
            static C: OpConstraint =
                o2_i3_fixed(Reg::Rax as u8, Reg::Rdx as u8, R_NO_RAX_RDX);
            &C
        }
        // -- Carry/borrow arithmetic: destructive binary --
        Opcode::AddCO
        | Opcode::AddCI
        | Opcode::AddCIO
        | Opcode::AddC1O
        | Opcode::SubBO
        | Opcode::SubBI
        | Opcode::SubBIO
        | Opcode::SubB1O => {
            static C: OpConstraint = o1_i2_alias(R, R, R);
            &C
        }
        // -- AndC: three-address via ANDN (BMI1) --
        Opcode::AndC => {
            static C: OpConstraint = o1_i2(R, R, R);
            &C
        }
        // -- Bit-field extract (unsigned/signed) --
        Opcode::Extract | Opcode::SExtract => {
            static C: OpConstraint = o1_i1(R, R);
            &C
        }
        // -- Deposit: output aliases input 0 --
        Opcode::Deposit => {
            static C: OpConstraint = o1_i2_alias(R, R, R);
            &C
        }
        // -- Extract2 (SHRD): output aliases input 0 --
        Opcode::Extract2 => {
            static C: OpConstraint = o1_i2_alias(R, R, R);
            &C
        }
        // -- Byte swap: destructive unary --
        Opcode::Bswap16 | Opcode::Bswap32 | Opcode::Bswap64 => {
            static C: OpConstraint = o1_i1_alias(R, R);
            &C
        }
        // -- Bit counting: Clz/Ctz have fallback input --
        Opcode::Clz | Opcode::Ctz => {
            static C: OpConstraint = n1_i2(R, R, R);
            &C
        }
        // -- CtPop: unary --
        Opcode::CtPop => {
            static C: OpConstraint = o1_i1(R, R);
            &C
        }
        // -- ExtrhI64I32: destructive unary --
        Opcode::ExtrhI64I32 => {
            static C: OpConstraint = o1_i1_alias(R, R);
            &C
        }
        // -- GotoPtr: single input, no output --
        Opcode::GotoPtr => {
            static C: OpConstraint = o0_i1(R);
            &C
        }
        // -- Load: output, base input --
        Opcode::Ld
        | Opcode::Ld8U
        | Opcode::Ld8S
        | Opcode::Ld16U
        | Opcode::Ld16S
        | Opcode::Ld32U
        | Opcode::Ld32S => {
            static C: OpConstraint = o1_i1(R, R);
            &C
        }
        // -- Store: value input, base input --
        Opcode::St | Opcode::St8 | Opcode::St16 | Opcode::St32 => {
            static C: OpConstraint = o0_i2(R, R);
            &C
        }
        // -- Type conversions: output, input --
        Opcode::ExtI32I64 | Opcode::ExtUI32I64 | Opcode::ExtrlI64I32 => {
            static C: OpConstraint = o1_i1(R, R);
            &C
        }
        // -- Guest load: output, addr input --
        Opcode::QemuLd => {
            static C: OpConstraint = o1_i1(R, R);
            &C
        }
        // -- Guest store: val input, addr input --
        Opcode::QemuSt => {
            static C: OpConstraint = o0_i2(R, R);
            &C
        }
        // -- Call: output + 6 inputs --
        Opcode::Call => {
            const CALL_C: OpConstraint = OpConstraint {
                args: [
                    fixed(Reg::Rax as u8),
                    fixed(Reg::Rdi as u8),
                    fixed(Reg::Rsi as u8),
                    fixed(Reg::Rdx as u8),
                    fixed(Reg::Rcx as u8),
                    fixed(Reg::R8 as u8),
                    fixed(Reg::R9 as u8),
                    ArgConstraint::UNUSED,
                    ArgConstraint::UNUSED,
                    ArgConstraint::UNUSED,
                ],
            };
            &CALL_C
        }
        _ => &OpConstraint::EMPTY,
    }
}
