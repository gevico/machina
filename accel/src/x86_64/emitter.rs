#![allow(non_upper_case_globals)]

use std::sync::Mutex;

use crate::code_buffer::CodeBuffer;
use crate::x86_64::regs::Reg;

// -- Prefix flags (matching QEMU's P_* constants) --

pub const P_EXT: u32 = 0x100; // 0x0F prefix
pub const P_EXT38: u32 = 0x200; // 0x0F 0x38 prefix
pub const P_DATA16: u32 = 0x400; // 0x66 prefix
pub const P_REXW: u32 = 0x1000; // REX.W = 1
pub const P_REXB_R: u32 = 0x2000; // REG field as byte register
pub const P_REXB_RM: u32 = 0x4000; // R/M field as byte register
pub const P_EXT3A: u32 = 0x10000; // 0x0F 0x3A prefix
pub const P_SIMDF3: u32 = 0x20000; // 0xF3 prefix
pub const P_SIMDF2: u32 = 0x40000; // 0xF2 prefix

// -- Opcode constants (OPC_*) --

// Arithmetic
pub const OPC_ARITH_EvIb: u32 = 0x83;
pub const OPC_ARITH_EvIz: u32 = 0x81;
pub const OPC_ARITH_GvEv: u32 = 0x03;
pub const OPC_ARITH_EvGv: u32 = 0x01;

// Shift
pub const OPC_SHIFT_1: u32 = 0xD1;
pub const OPC_SHIFT_Ib: u32 = 0xC1;
pub const OPC_SHIFT_cl: u32 = 0xD3;

// Data movement
pub const OPC_MOVB_EvGv: u32 = 0x88;
pub const OPC_MOVL_EvGv: u32 = 0x89;
pub const OPC_MOVL_GvEv: u32 = 0x8B;
pub const OPC_MOVB_EvIz: u32 = 0xC6;
pub const OPC_MOVL_EvIz: u32 = 0xC7;
pub const OPC_MOVL_Iv: u32 = 0xB8;

// Extensions
pub const OPC_MOVZBL: u32 = 0xB6 | P_EXT;
pub const OPC_MOVZWL: u32 = 0xB7 | P_EXT;
pub const OPC_MOVSBL: u32 = 0xBE | P_EXT;
pub const OPC_MOVSWL: u32 = 0xBF | P_EXT;
pub const OPC_MOVSLQ: u32 = 0x63 | P_REXW;

// Branch
pub const OPC_JCC_long: u32 = 0x80 | P_EXT;
pub const OPC_JMP_short: u32 = 0xEB;
pub const OPC_JMP_long: u32 = 0xE9;
pub const OPC_CALL_Jz: u32 = 0xE8;

// Bit operations
pub const OPC_BSF: u32 = 0xBC | P_EXT;
pub const OPC_BSR: u32 = 0xBD | P_EXT;
pub const OPC_LZCNT: u32 = 0xBD | P_EXT | P_SIMDF3;
pub const OPC_TZCNT: u32 = 0xBC | P_EXT | P_SIMDF3;
pub const OPC_POPCNT: u32 = 0xB8 | P_EXT | P_SIMDF3;
pub const OPC_BSWAP: u32 = 0xC8 | P_EXT;
pub const OPC_ANDN: u32 = 0xF2 | P_EXT38;

// Compare / conditional
pub const OPC_CMOVCC: u32 = 0x40 | P_EXT;
pub const OPC_SETCC: u32 = 0x90 | P_EXT | P_REXB_RM;
pub const OPC_TESTB: u32 = 0x84;
pub const OPC_TESTL: u32 = 0x85;

// Group opcodes
pub const OPC_GRP3_Ev: u32 = 0xF7;
pub const OPC_GRP3_Eb: u32 = 0xF6;
pub const OPC_GRP5: u32 = 0xFF;
pub const OPC_GRPBT: u32 = 0xBA | P_EXT;

// Multiply
pub const OPC_IMUL_GvEv: u32 = 0xAF | P_EXT;
pub const OPC_IMUL_GvEvIb: u32 = 0x6B;
pub const OPC_IMUL_GvEvIz: u32 = 0x69;

// Misc
pub const OPC_LEA: u32 = 0x8D;
pub const OPC_XCHG_ax_r32: u32 = 0x90;
pub const OPC_XCHG_EvGv: u32 = 0x87;
pub const OPC_PUSH_r32: u32 = 0x50;
pub const OPC_POP_r32: u32 = 0x58;
pub const OPC_RET: u32 = 0xC3;
pub const OPC_UD2: u32 = 0x0B | P_EXT;
pub const OPC_PUSH_Iz: u32 = 0x68;
pub const OPC_PUSH_Ib: u32 = 0x6A;

// Double-precision shift
pub const OPC_SHLD_Ib: u32 = 0xA4 | P_EXT;
pub const OPC_SHRD_Ib: u32 = 0xAC | P_EXT;

// -- Sub-operation enums --

/// Arithmetic sub-opcodes (used in /r field of
/// 0x81/0x83 and shifted into GvEv).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ArithOp {
    Add = 0,
    Or = 1,
    Adc = 2,
    Sbb = 3,
    And = 4,
    Sub = 5,
    Xor = 6,
    Cmp = 7,
}

/// Shift sub-opcodes (used in /r field of 0xC1/0xD1/0xD3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ShiftOp {
    Rol = 0,
    Ror = 1,
    Shl = 4,
    Shr = 5,
    Sar = 7,
}

/// Group 3 extension codes (used in /r field of 0xF7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Ext3Op {
    Not = 2,
    Neg = 3,
    Mul = 4,
    Imul = 5,
    Div = 6,
    Idiv = 7,
}

/// Group 5 extension codes (used in /r field of 0xFF).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Ext5Op {
    IncEv = 0,
    DecEv = 1,
    CallN = 2,
    JmpN = 4,
}

/// Bit-test group extension codes (used in /r field of 0xBA).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum GrpBtOp {
    Bt = 4,
    Bts = 5,
    Btr = 6,
    Btc = 7,
}

/// x86 condition codes for Jcc/SETcc/CMOVcc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum X86Cond {
    Jo = 0x0,
    Jno = 0x1,
    Jb = 0x2,
    Jae = 0x3,
    Je = 0x4,
    Jne = 0x5,
    Jbe = 0x6,
    Ja = 0x7,
    Js = 0x8,
    Jns = 0x9,
    Jp = 0xA,
    Jnp = 0xB,
    Jl = 0xC,
    Jge = 0xD,
    Jle = 0xE,
    Jg = 0xF,
}

impl X86Cond {
    /// Map TCG condition to x86 JCC condition code.
    pub fn from_tcg(cond: crate::ir::Cond) -> Self {
        match cond {
            crate::ir::Cond::Eq | crate::ir::Cond::TstEq => X86Cond::Je,
            crate::ir::Cond::Ne | crate::ir::Cond::TstNe => X86Cond::Jne,
            crate::ir::Cond::Lt => X86Cond::Jl,
            crate::ir::Cond::Ge => X86Cond::Jge,
            crate::ir::Cond::Le => X86Cond::Jle,
            crate::ir::Cond::Gt => X86Cond::Jg,
            crate::ir::Cond::Ltu => X86Cond::Jb,
            crate::ir::Cond::Geu => X86Cond::Jae,
            crate::ir::Cond::Leu => X86Cond::Jbe,
            crate::ir::Cond::Gtu => X86Cond::Ja,
            // Caller should not use Jcc for Always/Never
            crate::ir::Cond::Always => X86Cond::Je,
            crate::ir::Cond::Never => X86Cond::Jne,
        }
    }

    /// Return the inverted condition.
    pub fn invert(self) -> Self {
        // Flip the low bit
        unsafe { core::mem::transmute(self as u8 ^ 1) }
    }
}

// -- Core encoding functions --

/// Helper: return P_REXW if `rexw` is true.
#[inline]
fn rexw_flag(rexw: bool) -> u32 {
    if rexw {
        P_REXW
    } else {
        0
    }
}

/// Emit opcode with REX prefix. `r` is the reg field, `rm` is the r/m field.
/// Both are raw register numbers (0-15). Pass 0 for unused fields.
pub fn emit_opc(buf: &mut CodeBuffer, opc: u32, r: u8, rm: u8) {
    // Determine if REX is needed
    let mut rex: u8 = 0;
    if opc & P_REXW != 0 {
        rex |= 0x08; // REX.W
    }
    if r >= 8 {
        rex |= 0x04; // REX.R
    }
    if rm >= 8 {
        rex |= 0x01; // REX.B
    }
    // P_REXB_R / P_REXB_RM force REX for byte register access
    // (SPL, BPL, SIL, DIL need REX prefix to be accessible)
    if opc & P_REXB_R != 0 && r >= 4 && rex == 0 {
        rex = 0x40; // force bare REX
    }
    if opc & P_REXB_RM != 0 && rm >= 4 && rex == 0 {
        rex = 0x40;
    }

    // Emit prefix bytes
    if opc & P_DATA16 != 0 {
        buf.emit_u8(0x66);
    }
    if opc & P_SIMDF3 != 0 {
        buf.emit_u8(0xF3);
    } else if opc & P_SIMDF2 != 0 {
        buf.emit_u8(0xF2);
    }

    // Emit REX
    if rex != 0 {
        buf.emit_u8(0x40 | rex);
    }

    // Emit escape bytes
    if opc & (P_EXT | P_EXT38 | P_EXT3A) != 0 {
        buf.emit_u8(0x0F);
        if opc & P_EXT38 != 0 {
            buf.emit_u8(0x38);
        } else if opc & P_EXT3A != 0 {
            buf.emit_u8(0x3A);
        }
    }

    // Emit the opcode byte
    buf.emit_u8(opc as u8);
}

/// Emit opcode + ModR/M for register-register operation.
pub fn emit_modrm(buf: &mut CodeBuffer, opc: u32, r: Reg, rm: Reg) {
    emit_opc(buf, opc, r as u8, rm as u8);
    buf.emit_u8(0xC0 | (r.low3() << 3) | rm.low3());
}

/// Emit opcode + ModR/M with /r extension (for group opcodes).
pub fn emit_modrm_ext(buf: &mut CodeBuffer, opc: u32, ext: u8, rm: Reg) {
    emit_opc(buf, opc, ext, rm as u8);
    buf.emit_u8(0xC0 | (ext << 3) | rm.low3());
}

/// Emit opcode + ModR/M + displacement for memory [base + offset].
/// Handles special cases: RBP needs explicit disp8=0, RSP needs SIB byte.
pub fn emit_modrm_offset(
    buf: &mut CodeBuffer,
    opc: u32,
    r: Reg,
    base: Reg,
    offset: i32,
) {
    emit_opc(buf, opc, r as u8, base as u8);

    let r3 = r.low3();
    let b3 = base.low3();

    if offset == 0 && b3 != 5 {
        // [base] — mod=00 (RBP/R13 always need disp8)
        if b3 == 4 {
            // RSP/R12 need SIB byte
            buf.emit_u8((r3 << 3) | 0x04);
            buf.emit_u8(0x24); // SIB: index=RSP(none), base=RSP
        } else {
            buf.emit_u8((r3 << 3) | b3);
        }
    } else if (-128..=127).contains(&offset) {
        // [base + disp8] — mod=01
        if b3 == 4 {
            buf.emit_u8(0x44 | (r3 << 3));
            buf.emit_u8(0x24); // SIB
        } else {
            buf.emit_u8(0x40 | (r3 << 3) | b3);
        }
        buf.emit_u8(offset as u8);
    } else {
        // [base + disp32] — mod=10
        if b3 == 4 {
            buf.emit_u8(0x84 | (r3 << 3));
            buf.emit_u8(0x24); // SIB
        } else {
            buf.emit_u8(0x80 | (r3 << 3) | b3);
        }
        buf.emit_u32(offset as u32);
    }
}

/// Emit opcode + ModR/M + SIB for memory [base + index*scale + offset].
pub fn emit_modrm_sib(
    buf: &mut CodeBuffer,
    opc: u32,
    r: Reg,
    base: Reg,
    index: Reg,
    shift: u8,
    offset: i32,
) {
    assert!(
        shift <= 3,
        "SIB scale shift must be in 0..=3 (encoding scale factors 1,2,4,8)"
    );
    assert!(
        index != Reg::Rsp,
        "RSP cannot be encoded as a SIB index register in x86-64"
    );

    emit_opc_3(buf, opc, r as u8, base as u8, index as u8);

    let r3 = r.low3();
    let b3 = base.low3();
    let x3 = index.low3();
    let sib = (shift << 6) | (x3 << 3) | b3;

    if offset == 0 && b3 != 5 {
        buf.emit_u8((r3 << 3) | 0x04);
        buf.emit_u8(sib);
    } else if (-128..=127).contains(&offset) {
        buf.emit_u8(0x44 | (r3 << 3));
        buf.emit_u8(sib);
        buf.emit_u8(offset as u8);
    } else {
        buf.emit_u8(0x84 | (r3 << 3));
        buf.emit_u8(sib);
        buf.emit_u32(offset as u32);
    }
}

/// Emit opcode with REX prefix, 3-register variant (r, rm, index).
fn emit_opc_3(buf: &mut CodeBuffer, opc: u32, r: u8, rm: u8, index: u8) {
    let mut rex: u8 = 0;
    if opc & P_REXW != 0 {
        rex |= 0x08;
    }
    if r >= 8 {
        rex |= 0x04;
    }
    if index >= 8 {
        rex |= 0x02; // REX.X
    }
    if rm >= 8 {
        rex |= 0x01;
    }
    if opc & P_REXB_R != 0 && r >= 4 && rex == 0 {
        rex = 0x40;
    }
    if opc & P_REXB_RM != 0 && rm >= 4 && rex == 0 {
        rex = 0x40;
    }

    if opc & P_DATA16 != 0 {
        buf.emit_u8(0x66);
    }
    if opc & P_SIMDF3 != 0 {
        buf.emit_u8(0xF3);
    } else if opc & P_SIMDF2 != 0 {
        buf.emit_u8(0xF2);
    }
    if rex != 0 {
        buf.emit_u8(0x40 | rex);
    }
    if opc & (P_EXT | P_EXT38 | P_EXT3A) != 0 {
        buf.emit_u8(0x0F);
        if opc & P_EXT38 != 0 {
            buf.emit_u8(0x38);
        } else if opc & P_EXT3A != 0 {
            buf.emit_u8(0x3A);
        }
    }
    buf.emit_u8(opc as u8);
}

/// Emit opcode + ModR/M with /r extension for memory [base + offset].
pub fn emit_modrm_ext_offset(
    buf: &mut CodeBuffer,
    opc: u32,
    ext: u8,
    base: Reg,
    offset: i32,
) {
    // Reuse emit_modrm_offset logic but with ext as the "r" field.
    // We need to manually handle REX since ext is not a real register.
    emit_opc(buf, opc, ext, base as u8);

    let b3 = base.low3();

    if offset == 0 && b3 != 5 {
        if b3 == 4 {
            buf.emit_u8((ext << 3) | 0x04);
            buf.emit_u8(0x24);
        } else {
            buf.emit_u8((ext << 3) | b3);
        }
    } else if (-128..=127).contains(&offset) {
        if b3 == 4 {
            buf.emit_u8(0x44 | (ext << 3));
            buf.emit_u8(0x24);
        } else {
            buf.emit_u8(0x40 | (ext << 3) | b3);
        }
        buf.emit_u8(offset as u8);
    } else {
        if b3 == 4 {
            buf.emit_u8(0x84 | (ext << 3));
            buf.emit_u8(0x24);
        } else {
            buf.emit_u8(0x80 | (ext << 3) | b3);
        }
        buf.emit_u32(offset as u32);
    }
}

// -- Arithmetic instructions --

/// Emit arithmetic reg, reg (ADD/SUB/AND/OR/XOR/CMP/ADC/SBB).
pub fn emit_arith_rr(
    buf: &mut CodeBuffer,
    op: ArithOp,
    rexw: bool,
    dst: Reg,
    src: Reg,
) {
    let opc = (OPC_ARITH_GvEv + ((op as u32) << 3)) | rexw_flag(rexw);
    emit_modrm(buf, opc, dst, src);
}

/// Emit arithmetic reg, imm (auto-selects imm8 vs imm32).
pub fn emit_arith_ri(
    buf: &mut CodeBuffer,
    op: ArithOp,
    rexw: bool,
    dst: Reg,
    imm: i32,
) {
    let w = rexw_flag(rexw);
    if (-128..=127).contains(&imm) {
        emit_modrm_ext(buf, OPC_ARITH_EvIb | w, op as u8, dst);
        buf.emit_u8(imm as u8);
    } else {
        emit_modrm_ext(buf, OPC_ARITH_EvIz | w, op as u8, dst);
        buf.emit_u32(imm as u32);
    }
}

/// Emit arithmetic [base+offset], reg (store-op).
pub fn emit_arith_mr(
    buf: &mut CodeBuffer,
    op: ArithOp,
    rexw: bool,
    base: Reg,
    offset: i32,
    src: Reg,
) {
    let opc = (OPC_ARITH_EvGv + ((op as u32) << 3)) | rexw_flag(rexw);
    emit_modrm_offset(buf, opc, src, base, offset);
}

/// Emit arithmetic reg, [base+offset] (load-op).
pub fn emit_arith_rm(
    buf: &mut CodeBuffer,
    op: ArithOp,
    rexw: bool,
    dst: Reg,
    base: Reg,
    offset: i32,
) {
    let opc = (OPC_ARITH_GvEv + ((op as u32) << 3)) | rexw_flag(rexw);
    emit_modrm_offset(buf, opc, dst, base, offset);
}

/// Emit NEG reg.
pub fn emit_neg(buf: &mut CodeBuffer, rexw: bool, reg: Reg) {
    emit_modrm_ext(buf, OPC_GRP3_Ev | rexw_flag(rexw), Ext3Op::Neg as u8, reg);
}

/// Emit NOT reg.
pub fn emit_not(buf: &mut CodeBuffer, rexw: bool, reg: Reg) {
    emit_modrm_ext(buf, OPC_GRP3_Ev | rexw_flag(rexw), Ext3Op::Not as u8, reg);
}

// -- Shift instructions --

/// Emit shift reg, imm8.
pub fn emit_shift_ri(
    buf: &mut CodeBuffer,
    op: ShiftOp,
    rexw: bool,
    dst: Reg,
    imm: u8,
) {
    let w = rexw_flag(rexw);
    if imm == 1 {
        emit_modrm_ext(buf, OPC_SHIFT_1 | w, op as u8, dst);
    } else {
        emit_modrm_ext(buf, OPC_SHIFT_Ib | w, op as u8, dst);
        buf.emit_u8(imm);
    }
}

/// Emit shift reg, CL.
pub fn emit_shift_cl(buf: &mut CodeBuffer, op: ShiftOp, rexw: bool, dst: Reg) {
    emit_modrm_ext(buf, OPC_SHIFT_cl | rexw_flag(rexw), op as u8, dst);
}

// -- Data movement --

/// Emit MOV reg, reg (32-bit or 64-bit).
pub fn emit_mov_rr(buf: &mut CodeBuffer, rexw: bool, dst: Reg, src: Reg) {
    emit_modrm(buf, OPC_MOVL_EvGv | rexw_flag(rexw), src, dst);
}

/// Emit MOV reg, imm (32-bit or 64-bit).
pub fn emit_mov_ri(buf: &mut CodeBuffer, rexw: bool, reg: Reg, val: u64) {
    if val == 0 {
        emit_modrm(buf, 0x31, reg, reg);
    } else if !rexw || val <= u32::MAX as u64 {
        emit_opc(buf, OPC_MOVL_Iv + (reg.low3() as u32), 0, reg as u8);
        buf.emit_u32(val as u32);
    } else if val as i64 >= i32::MIN as i64 && val as i64 <= i32::MAX as i64 {
        emit_modrm_ext(buf, OPC_MOVL_EvIz | P_REXW, 0, reg);
        buf.emit_u32(val as u32);
    } else {
        emit_opc(
            buf,
            (OPC_MOVL_Iv + (reg.low3() as u32)) | P_REXW,
            0,
            reg as u8,
        );
        buf.emit_u64(val);
    }
}

/// Emit zero-extend: MOVZBL or MOVZWL.
pub fn emit_movzx(buf: &mut CodeBuffer, opc: u32, dst: Reg, src: Reg) {
    emit_modrm(buf, opc, dst, src);
}

/// Emit sign-extend: MOVSBL, MOVSWL, or MOVSLQ.
pub fn emit_movsx(buf: &mut CodeBuffer, opc: u32, dst: Reg, src: Reg) {
    emit_modrm(buf, opc, dst, src);
}

/// Emit BSWAP reg (32-bit or 64-bit).
pub fn emit_bswap(buf: &mut CodeBuffer, rexw: bool, reg: Reg) {
    // BSWAP encodes register in the opcode byte: 0F C8+rd
    emit_opc(
        buf,
        (OPC_BSWAP + reg.low3() as u32) | rexw_flag(rexw),
        0,
        reg as u8,
    );
}

// -- Memory operations --

/// Emit MOV reg, [base+offset] (load).
pub fn emit_load(
    buf: &mut CodeBuffer,
    rexw: bool,
    dst: Reg,
    base: Reg,
    offset: i32,
) {
    emit_modrm_offset(buf, OPC_MOVL_GvEv | rexw_flag(rexw), dst, base, offset);
}

/// Emit MOV [base+offset], reg (store).
pub fn emit_store(
    buf: &mut CodeBuffer,
    rexw: bool,
    src: Reg,
    base: Reg,
    offset: i32,
) {
    emit_modrm_offset(buf, OPC_MOVL_EvGv | rexw_flag(rexw), src, base, offset);
}

/// Emit MOV byte [base+offset], reg (byte store).
pub fn emit_store_byte(buf: &mut CodeBuffer, src: Reg, base: Reg, offset: i32) {
    emit_modrm_offset(buf, OPC_MOVB_EvGv | P_REXB_R, src, base, offset);
}

/// Emit MOV word [base+offset], reg (16-bit store).
pub fn emit_store_word(buf: &mut CodeBuffer, src: Reg, base: Reg, offset: i32) {
    emit_modrm_offset(buf, P_DATA16 | OPC_MOVL_EvGv, src, base, offset);
}

/// Emit MOV [base+offset], imm32 (store immediate).
pub fn emit_store_imm(
    buf: &mut CodeBuffer,
    rexw: bool,
    base: Reg,
    offset: i32,
    imm: i32,
) {
    emit_modrm_ext_offset(
        buf,
        OPC_MOVL_EvIz | rexw_flag(rexw),
        0,
        base,
        offset,
    );
    buf.emit_u32(imm as u32);
}

/// Emit LEA dst, [base+offset].
pub fn emit_lea(
    buf: &mut CodeBuffer,
    rexw: bool,
    dst: Reg,
    base: Reg,
    offset: i32,
) {
    emit_modrm_offset(buf, OPC_LEA | rexw_flag(rexw), dst, base, offset);
}

/// Emit LEA dst, [base+index*scale+offset].
pub fn emit_lea_sib(
    buf: &mut CodeBuffer,
    rexw: bool,
    dst: Reg,
    base: Reg,
    index: Reg,
    shift: u8,
    offset: i32,
) {
    emit_modrm_sib(
        buf,
        OPC_LEA | rexw_flag(rexw),
        dst,
        base,
        index,
        shift,
        offset,
    );
}

/// Emit MOV reg, [base+index*scale+offset] (indexed load).
pub fn emit_load_sib(
    buf: &mut CodeBuffer,
    rexw: bool,
    dst: Reg,
    base: Reg,
    index: Reg,
    shift: u8,
    offset: i32,
) {
    emit_modrm_sib(
        buf,
        OPC_MOVL_GvEv | rexw_flag(rexw),
        dst,
        base,
        index,
        shift,
        offset,
    );
}

/// Emit MOV [base+index*scale+offset], reg (indexed store).
pub fn emit_store_sib(
    buf: &mut CodeBuffer,
    rexw: bool,
    src: Reg,
    base: Reg,
    index: Reg,
    shift: u8,
    offset: i32,
) {
    emit_modrm_sib(
        buf,
        OPC_MOVL_EvGv | rexw_flag(rexw),
        src,
        base,
        index,
        shift,
        offset,
    );
}

/// Emit MOV byte [base+index+0], reg (SIB byte store).
pub fn emit_store_byte_sib(
    buf: &mut CodeBuffer,
    src: Reg,
    base: Reg,
    index: Reg,
) {
    emit_modrm_sib(buf, OPC_MOVB_EvGv | P_REXB_R, src, base, index, 0, 0);
}

/// Emit MOV word [base+index+0], reg (SIB 16-bit store).
pub fn emit_store_word_sib(
    buf: &mut CodeBuffer,
    src: Reg,
    base: Reg,
    index: Reg,
) {
    emit_modrm_sib(buf, P_DATA16 | OPC_MOVL_EvGv, src, base, index, 0, 0);
}

/// Emit zero-extend SIB load: MOVZBL/MOVZWL [base+index].
pub fn emit_load_zx_sib(
    buf: &mut CodeBuffer,
    opc: u32,
    dst: Reg,
    base: Reg,
    index: Reg,
) {
    emit_modrm_sib(buf, opc, dst, base, index, 0, 0);
}

/// Emit sign-extend SIB load: MOVSBL/MOVSWL/MOVSLQ
/// [base+index].
pub fn emit_load_sx_sib(
    buf: &mut CodeBuffer,
    opc: u32,
    dst: Reg,
    base: Reg,
    index: Reg,
) {
    emit_modrm_sib(buf, opc, dst, base, index, 0, 0);
}

/// Emit zero-extend load: MOVZBL/MOVZWL [base+offset].
pub fn emit_load_zx(
    buf: &mut CodeBuffer,
    opc: u32,
    dst: Reg,
    base: Reg,
    offset: i32,
) {
    emit_modrm_offset(buf, opc, dst, base, offset);
}

/// Emit sign-extend load: MOVSBL/MOVSWL/MOVSLQ [base+offset].
pub fn emit_load_sx(
    buf: &mut CodeBuffer,
    opc: u32,
    dst: Reg,
    base: Reg,
    offset: i32,
) {
    emit_modrm_offset(buf, opc, dst, base, offset);
}

// -- Multiply / Divide --

/// Emit single-operand MUL (unsigned): RDX:RAX = RAX * reg.
pub fn emit_mul(buf: &mut CodeBuffer, rexw: bool, reg: Reg) {
    emit_modrm_ext(buf, OPC_GRP3_Ev | rexw_flag(rexw), Ext3Op::Mul as u8, reg);
}

/// Emit single-operand IMUL (signed): RDX:RAX = RAX * reg.
pub fn emit_imul1(buf: &mut CodeBuffer, rexw: bool, reg: Reg) {
    emit_modrm_ext(buf, OPC_GRP3_Ev | rexw_flag(rexw), Ext3Op::Imul as u8, reg);
}

/// Emit two-operand IMUL: dst = dst * src.
pub fn emit_imul_rr(buf: &mut CodeBuffer, rexw: bool, dst: Reg, src: Reg) {
    emit_modrm(buf, OPC_IMUL_GvEv | rexw_flag(rexw), dst, src);
}

/// Emit three-operand IMUL: dst = src * imm.
pub fn emit_imul_ri(
    buf: &mut CodeBuffer,
    rexw: bool,
    dst: Reg,
    src: Reg,
    imm: i32,
) {
    let w = rexw_flag(rexw);
    if (-128..=127).contains(&imm) {
        emit_modrm(buf, OPC_IMUL_GvEvIb | w, dst, src);
        buf.emit_u8(imm as u8);
    } else {
        emit_modrm(buf, OPC_IMUL_GvEvIz | w, dst, src);
        buf.emit_u32(imm as u32);
    }
}

/// Emit DIV (unsigned): RAX = RDX:RAX / reg, RDX = remainder.
pub fn emit_div(buf: &mut CodeBuffer, rexw: bool, reg: Reg) {
    emit_modrm_ext(buf, OPC_GRP3_Ev | rexw_flag(rexw), Ext3Op::Div as u8, reg);
}

/// Emit IDIV (signed): RAX = RDX:RAX / reg, RDX = remainder.
pub fn emit_idiv(buf: &mut CodeBuffer, rexw: bool, reg: Reg) {
    emit_modrm_ext(buf, OPC_GRP3_Ev | rexw_flag(rexw), Ext3Op::Idiv as u8, reg);
}

/// Emit CDQ: sign-extend EAX into EDX:EAX.
pub fn emit_cdq(buf: &mut CodeBuffer) {
    buf.emit_u8(0x99);
}

/// Emit CQO: sign-extend RAX into RDX:RAX.
pub fn emit_cqo(buf: &mut CodeBuffer) {
    buf.emit_u8(0x48);
    buf.emit_u8(0x99);
}

// -- Bit operations --

/// Emit BSF dst, src (bit scan forward).
pub fn emit_bsf(buf: &mut CodeBuffer, rexw: bool, dst: Reg, src: Reg) {
    emit_modrm(buf, OPC_BSF | rexw_flag(rexw), dst, src);
}

/// Emit BSR dst, src (bit scan reverse).
pub fn emit_bsr(buf: &mut CodeBuffer, rexw: bool, dst: Reg, src: Reg) {
    emit_modrm(buf, OPC_BSR | rexw_flag(rexw), dst, src);
}

/// Emit LZCNT dst, src (leading zero count).
pub fn emit_lzcnt(buf: &mut CodeBuffer, rexw: bool, dst: Reg, src: Reg) {
    emit_modrm(buf, OPC_LZCNT | rexw_flag(rexw), dst, src);
}

/// Emit TZCNT dst, src (trailing zero count).
pub fn emit_tzcnt(buf: &mut CodeBuffer, rexw: bool, dst: Reg, src: Reg) {
    emit_modrm(buf, OPC_TZCNT | rexw_flag(rexw), dst, src);
}

/// Emit POPCNT dst, src (population count).
pub fn emit_popcnt(buf: &mut CodeBuffer, rexw: bool, dst: Reg, src: Reg) {
    emit_modrm(buf, OPC_POPCNT | rexw_flag(rexw), dst, src);
}

/// Emit BT reg, imm8 (bit test).
pub fn emit_bt_ri(buf: &mut CodeBuffer, rexw: bool, reg: Reg, bit: u8) {
    emit_modrm_ext(buf, OPC_GRPBT | rexw_flag(rexw), GrpBtOp::Bt as u8, reg);
    buf.emit_u8(bit);
}

/// Emit BTS reg, imm8 (bit test and set).
pub fn emit_bts_ri(buf: &mut CodeBuffer, rexw: bool, reg: Reg, bit: u8) {
    emit_modrm_ext(buf, OPC_GRPBT | rexw_flag(rexw), GrpBtOp::Bts as u8, reg);
    buf.emit_u8(bit);
}

/// Emit BTR reg, imm8 (bit test and reset).
pub fn emit_btr_ri(buf: &mut CodeBuffer, rexw: bool, reg: Reg, bit: u8) {
    emit_modrm_ext(buf, OPC_GRPBT | rexw_flag(rexw), GrpBtOp::Btr as u8, reg);
    buf.emit_u8(bit);
}

/// Emit BTC reg, imm8 (bit test and complement).
pub fn emit_btc_ri(buf: &mut CodeBuffer, rexw: bool, reg: Reg, bit: u8) {
    emit_modrm_ext(buf, OPC_GRPBT | rexw_flag(rexw), GrpBtOp::Btc as u8, reg);
    buf.emit_u8(bit);
}

/// Emit ANDN dst, src1, src2 (BMI1: dst = ~src1 & src2). Uses VEX encoding.
pub fn emit_andn(
    buf: &mut CodeBuffer,
    rexw: bool,
    dst: Reg,
    src1: Reg,
    src2: Reg,
) {
    emit_vex_modrm(buf, OPC_ANDN | rexw_flag(rexw), dst, src1, src2);
}

// -- Branches and comparisons --

/// Emit Jcc rel32 (conditional jump to absolute offset).
pub fn emit_jcc(buf: &mut CodeBuffer, cond: X86Cond, target_offset: usize) {
    emit_opc(buf, OPC_JCC_long + (cond as u32), 0, 0);
    let after = buf.offset() + 4;
    let disp = target_offset as i128 - after as i128;
    assert!(
        (i32::MIN as i128..=i32::MAX as i128).contains(&disp),
        "jcc displacement out of i32 range"
    );
    buf.emit_u32(disp as i32 as u32);
}

/// Emit JMP rel32 to absolute offset.
pub fn emit_jmp(buf: &mut CodeBuffer, target_offset: usize) {
    buf.emit_u8(OPC_JMP_long as u8);
    let after = buf.offset() + 4;
    let disp = target_offset as i128 - after as i128;
    assert!(
        (i32::MIN as i128..=i32::MAX as i128).contains(&disp),
        "jmp displacement out of i32 range"
    );
    buf.emit_u32(disp as i32 as u32);
}

/// Emit CALL rel32 to absolute offset.
pub fn emit_call(buf: &mut CodeBuffer, target_offset: usize) {
    buf.emit_u8(OPC_CALL_Jz as u8);
    let after = buf.offset() + 4;
    let disp = target_offset as i128 - after as i128;
    assert!(
        (i32::MIN as i128..=i32::MAX as i128).contains(&disp),
        "call displacement out of i32 range"
    );
    buf.emit_u32(disp as i32 as u32);
}

/// Emit indirect JMP through register.
pub fn emit_jmp_reg(buf: &mut CodeBuffer, reg: Reg) {
    emit_modrm_ext(buf, OPC_GRP5, Ext5Op::JmpN as u8, reg);
}

/// Emit indirect CALL through register.
pub fn emit_call_reg(buf: &mut CodeBuffer, reg: Reg) {
    emit_modrm_ext(buf, OPC_GRP5, Ext5Op::CallN as u8, reg);
}

/// Emit SETcc dst (set byte on condition).
pub fn emit_setcc(buf: &mut CodeBuffer, cond: X86Cond, dst: Reg) {
    emit_modrm_ext(buf, OPC_SETCC + (cond as u32), 0, dst);
}

/// Emit CMOVcc dst, src (conditional move).
pub fn emit_cmovcc(
    buf: &mut CodeBuffer,
    cond: X86Cond,
    rexw: bool,
    dst: Reg,
    src: Reg,
) {
    emit_modrm(
        buf,
        (OPC_CMOVCC + (cond as u32)) | rexw_flag(rexw),
        dst,
        src,
    );
}

/// Emit TEST reg, reg.
pub fn emit_test_rr(buf: &mut CodeBuffer, rexw: bool, r1: Reg, r2: Reg) {
    emit_modrm(buf, OPC_TESTL | rexw_flag(rexw), r1, r2);
}

/// Emit TEST byte reg, imm8.
pub fn emit_test_bi(buf: &mut CodeBuffer, reg: Reg, imm: u8) {
    emit_modrm_ext(buf, OPC_GRP3_Eb | P_REXB_RM, 0, reg);
    buf.emit_u8(imm);
}

// -- Miscellaneous --

/// Emit XCHG r1, r2.
pub fn emit_xchg(buf: &mut CodeBuffer, rexw: bool, r1: Reg, r2: Reg) {
    emit_modrm(buf, OPC_XCHG_EvGv | rexw_flag(rexw), r1, r2);
}

/// Emit PUSH reg.
pub fn emit_push(buf: &mut CodeBuffer, reg: Reg) {
    emit_opc(buf, OPC_PUSH_r32 + (reg.low3() as u32), 0, reg as u8);
}

/// Emit POP reg.
pub fn emit_pop(buf: &mut CodeBuffer, reg: Reg) {
    emit_opc(buf, OPC_POP_r32 + (reg.low3() as u32), 0, reg as u8);
}

/// Emit PUSH imm32.
pub fn emit_push_imm(buf: &mut CodeBuffer, imm: i32) {
    if (-128..=127).contains(&imm) {
        buf.emit_u8(OPC_PUSH_Ib as u8);
        buf.emit_u8(imm as u8);
    } else {
        buf.emit_u8(OPC_PUSH_Iz as u8);
        buf.emit_u32(imm as u32);
    }
}

/// Emit RET.
pub fn emit_ret(buf: &mut CodeBuffer) {
    buf.emit_u8(OPC_RET as u8);
}

/// Emit MFENCE (memory barrier).
pub fn emit_mfence(buf: &mut CodeBuffer) {
    buf.emit_u8(0x0F);
    buf.emit_u8(0xAE);
    buf.emit_u8(0xF0);
}

/// Emit UD2 (undefined instruction, for debugging traps).
pub fn emit_ud2(buf: &mut CodeBuffer) {
    emit_opc(buf, OPC_UD2, 0, 0);
}

/// Emit `n` bytes of NOP padding using recommended multi-byte NOPs.
pub fn emit_nops(buf: &mut CodeBuffer, mut n: usize) {
    while n > 0 {
        match n {
            1 => {
                buf.emit_u8(0x90);
                n -= 1;
            }
            2 => {
                buf.emit_u8(0x66);
                buf.emit_u8(0x90);
                n -= 2;
            }
            3 => {
                buf.emit_u8(0x0F);
                buf.emit_u8(0x1F);
                buf.emit_u8(0x00);
                n -= 3;
            }
            4 => {
                buf.emit_u8(0x0F);
                buf.emit_u8(0x1F);
                buf.emit_u8(0x40);
                buf.emit_u8(0x00);
                n -= 4;
            }
            5 => {
                buf.emit_u8(0x0F);
                buf.emit_u8(0x1F);
                buf.emit_u8(0x44);
                buf.emit_u8(0x00);
                buf.emit_u8(0x00);
                n -= 5;
            }
            6 => {
                buf.emit_u8(0x66);
                buf.emit_u8(0x0F);
                buf.emit_u8(0x1F);
                buf.emit_u8(0x44);
                buf.emit_u8(0x00);
                buf.emit_u8(0x00);
                n -= 6;
            }
            7 => {
                buf.emit_u8(0x0F);
                buf.emit_u8(0x1F);
                buf.emit_u8(0x80);
                buf.emit_u32(0);
                n -= 7;
            }
            _ => {
                buf.emit_u8(0x0F);
                buf.emit_u8(0x1F);
                buf.emit_u8(0x84);
                buf.emit_u8(0x00);
                buf.emit_u32(0);
                n -= 8;
            }
        }
    }
}

/// Emit INC reg (via GRP5).
pub fn emit_inc(buf: &mut CodeBuffer, rexw: bool, reg: Reg) {
    emit_modrm_ext(buf, OPC_GRP5 | rexw_flag(rexw), Ext5Op::IncEv as u8, reg);
}

/// Emit DEC reg (via GRP5).
pub fn emit_dec(buf: &mut CodeBuffer, rexw: bool, reg: Reg) {
    emit_modrm_ext(buf, OPC_GRP5 | rexw_flag(rexw), Ext5Op::DecEv as u8, reg);
}

/// Emit SHLD dst, src, imm8 (double-precision shift left).
pub fn emit_shld_ri(
    buf: &mut CodeBuffer,
    rexw: bool,
    dst: Reg,
    src: Reg,
    imm: u8,
) {
    emit_modrm(buf, OPC_SHLD_Ib | rexw_flag(rexw), src, dst);
    buf.emit_u8(imm);
}

/// Emit SHRD dst, src, imm8 (double-precision shift right).
pub fn emit_shrd_ri(
    buf: &mut CodeBuffer,
    rexw: bool,
    dst: Reg,
    src: Reg,
    imm: u8,
) {
    emit_modrm(buf, OPC_SHRD_Ib | rexw_flag(rexw), src, dst);
    buf.emit_u8(imm);
}

/// Emit ROL word, 8 (DATA16 + ROL r16, 8) for bswap16.
pub fn emit_rolw_8(buf: &mut CodeBuffer, reg: Reg) {
    emit_modrm_ext(buf, OPC_SHIFT_Ib | P_DATA16, ShiftOp::Rol as u8, reg);
    buf.emit_u8(8);
}

/// Emit STC (set carry flag).
pub fn emit_stc(buf: &mut CodeBuffer) {
    buf.emit_u8(0xF9);
}

// -- VEX encoding (for BMI instructions) --

/// Emit 2-byte or 3-byte VEX prefix + opcode + ModR/M (reg-reg).
fn emit_vex_modrm(buf: &mut CodeBuffer, opc: u32, r: Reg, v: Reg, rm: Reg) {
    let r_bit: u8 = if r.needs_rex() { 0 } else { 0x80 };
    let x_bit: u8 = 0x40;
    let b_bit: u8 = if rm.needs_rex() { 0 } else { 0x20 };
    let vvvv: u8 = (!(v as u8) & 0x0F) << 3;
    let w: u8 = if opc & P_REXW != 0 { 0x80 } else { 0 };
    let pp: u8 = if opc & P_DATA16 != 0 {
        1
    } else if opc & P_SIMDF3 != 0 {
        2
    } else if opc & P_SIMDF2 != 0 {
        3
    } else {
        0
    };
    let mm: u8 = if opc & P_EXT38 != 0 {
        2
    } else if opc & P_EXT3A != 0 {
        3
    } else {
        1
    };

    if mm == 1 && w == 0 && b_bit != 0 && x_bit != 0 {
        buf.emit_u8(0xC5);
        buf.emit_u8(r_bit | vvvv | pp);
    } else {
        buf.emit_u8(0xC4);
        buf.emit_u8(r_bit | x_bit | b_bit | mm);
        buf.emit_u8(w | vvvv | pp);
    }
    buf.emit_u8(opc as u8);
    buf.emit_u8(0xC0 | (r.low3() << 3) | rm.low3());
}

// ==========================================================
// X86_64CodeGen — backend code generator struct
// ==========================================================

/// SoftMMU/TLB configuration for full-system emulation.
///
/// When present, QemuLd/QemuSt codegen emits an inline
/// TLB tag check: TLB hits use the addend-based fast
/// path; misses call the slow-path helper which does
/// MMU translation and TLB fill.
pub struct SoftMmuConfig {
    /// Offset of the TLB base pointer from env (rbp).
    /// The TLB is stored as Box<[TlbEntry; N]>, so this
    /// offset points to the Box's inner pointer.
    pub tlb_ptr_offset: usize,
    /// Size of one TlbEntry in bytes.
    pub entry_size: usize,
    /// Offset of addr_read within TlbEntry.
    pub addr_read_off: usize,
    /// Offset of addr_write within TlbEntry.
    pub addr_write_off: usize,
    /// Offset of addend within TlbEntry.
    pub addend_off: usize,
    /// TLB index mask (TLB_SIZE - 1).
    pub index_mask: u64,
    /// Address of the load slow-path helper:
    /// `machina_mem_read(env, gva, size) -> u64`.
    pub load_helper: u64,
    /// Address of the store slow-path helper:
    /// `machina_mem_write(env, gva, val, size)`.
    pub store_helper: u64,
    /// Offset of `mem_fault_cause` within env (for
    /// checking after helper returns).
    pub fault_cause_offset: usize,
    /// Epilogue offset for exiting TB on helper fault.
    pub tb_ret_addr: u64,
}

/// x86-64 backend code generator.
pub struct X86_64CodeGen {
    pub prologue_offset: usize,
    pub epilogue_return_zero_offset: usize,
    pub tb_ret_offset: usize,
    pub code_gen_start: usize,
    /// Recorded (jmp_offset, reset_offset) for each goto_tb.
    pub(crate) goto_tb_info: Mutex<Vec<(usize, usize)>>,
    /// SoftMMU config for full-system mode. When `None`,
    /// all guest memory accesses use direct [R14+addr].
    pub mmio: Option<SoftMmuConfig>,
}

impl X86_64CodeGen {
    pub fn new() -> Self {
        Self {
            prologue_offset: 0,
            epilogue_return_zero_offset: 0,
            tb_ret_offset: 0,
            code_gen_start: 0,
            goto_tb_info: Mutex::new(Vec::new()),
            mmio: None,
        }
    }

    /// Emit `exit_tb(val)`: load return value into rax and jump to epilogue.
    pub fn emit_exit_tb(&self, buf: &mut CodeBuffer, val: u64) {
        if val == 0 {
            emit_jmp(buf, self.epilogue_return_zero_offset);
        } else {
            emit_mov_ri(buf, true, Reg::Rax, val);
            emit_jmp(buf, self.tb_ret_offset);
        }
    }

    /// Emit `goto_tb(n)`: a patchable direct jump (5 bytes: E9 + disp32).
    ///
    /// The disp32 field is aligned to 4 bytes so that concurrent
    /// patching is atomic on x86-64 (safe for concurrent vCPUs).
    pub fn emit_goto_tb(&self, buf: &mut CodeBuffer) -> (usize, usize) {
        // Align disp32 to 4 bytes for atomic patching.
        let disp_addr = buf.offset() + 1; // after E9 opcode
        let aligned = (disp_addr + 3) & !3;
        let pad = aligned - disp_addr;
        if pad > 0 {
            emit_nops(buf, pad);
        }
        let jmp_offset = buf.offset();
        buf.emit_u8(0xE9);
        buf.emit_u32(0);
        let reset_offset = buf.offset();
        (jmp_offset, reset_offset)
    }

    /// Emit `goto_ptr(reg)`: indirect jump through a register.
    pub fn emit_goto_ptr(buf: &mut CodeBuffer, reg: Reg) {
        emit_jmp_reg(buf, reg);
    }

    // ---- MMIO slow-path helpers ----

    // Caller-saved GPRs that need saving in the slow path.
    // Slots [rsp+0..rsp+72) hold these 9 registers.
    const CALLER_SAVED: [(Reg, i32); 9] = [
        (Reg::Rax, 0),
        (Reg::Rcx, 8),
        (Reg::Rdx, 16),
        (Reg::Rsi, 24),
        (Reg::Rdi, 32),
        (Reg::R8, 40),
        (Reg::R9, 48),
        (Reg::R10, 56),
        (Reg::R11, 64),
    ];
    // Scratch slot for passing values across the call.
    const MMIO_SCRATCH: i32 = 80;
    // Extra scratch slots for saving registers clobbered
    // by the TLB inline check (RAX, R10, R11).
    const TLB_SAVE_RAX: i32 = 96;
    const TLB_SAVE_R10: i32 = 104;
    const TLB_SAVE_R11: i32 = 112;

    /// Save all caller-saved GPRs to the stack frame.
    fn emit_save_caller_regs(buf: &mut CodeBuffer) {
        for &(r, off) in &Self::CALLER_SAVED {
            emit_store(buf, true, r, Reg::Rsp, off);
        }
    }

    /// Restore all caller-saved GPRs from the stack frame.
    fn emit_restore_caller_regs(buf: &mut CodeBuffer) {
        for &(r, off) in &Self::CALLER_SAVED {
            emit_load(buf, true, r, Reg::Rsp, off);
        }
    }

    /// Emit QemuLd with inline TLB check.
    ///
    /// ```text
    /// ; Compute TLB index
    ///   mov r11, addr; shr r11, 12; and r11, mask
    /// ; Load TLB base pointer
    ///   mov r10, [rbp + tlb_ptr_offset]
    /// ; entry = base + index * entry_size
    ///   imul r11, entry_size; add r10, r11
    /// ; Compare tag
    ///   mov r11, [r10 + addr_read_off]
    ///   mov rax, addr; and rax, PAGE_MASK
    ///   cmp r11, rax
    ///   jne slow_path
    /// ; Load addend, check MMIO sentinel
    ///   mov r11, [r10 + addend_off]
    ///   cmp r11, -1; je slow_path
    /// ; Fast path: host_addr = addr + addend
    ///   lea rax, [addr + r11]
    ///   <load dst from [rax]>
    ///   jmp done
    /// slow_path:
    ///   <call helper(env, gva, size)>
    ///   mov dst, rax
    /// done:
    /// ```
    /// Emit QemuLd with inline TLB check and
    /// post-helper fault exit.
    ///
    /// Fast path: TLB tag match → addend → inline load.
    /// Slow path: call helper → check fault → exit TB
    /// if faulted.
    pub(crate) fn emit_qemu_ld_mmio(
        &self,
        buf: &mut CodeBuffer,
        cfg: &SoftMmuConfig,
        rexw: bool,
        dst: Reg,
        addr: Reg,
        memop: u16,
    ) {
        let size = memop & 0x3;
        let sign = memop & 4 != 0;
        let size_bytes: u32 = 1 << size;

        // Save addr and scratch regs clobbered by the
        // TLB inline check so live IR temps survive.
        emit_store(buf, true, addr, Reg::Rsp, Self::MMIO_SCRATCH);
        emit_store(buf, true, Reg::Rax, Reg::Rsp, Self::TLB_SAVE_RAX);
        emit_store(buf, true, Reg::R10, Reg::Rsp, Self::TLB_SAVE_R10);
        emit_store(buf, true, Reg::R11, Reg::Rsp, Self::TLB_SAVE_R11);

        // -- TLB inline check --
        // r11 = TLB index = ((vpn ^ (vpn >> 8)) & mask)
        // Must match mmu.rs tlb_index().
        emit_mov_rr(buf, true, Reg::R11, addr);
        emit_shift_ri(buf, ShiftOp::Shr, true, Reg::R11, 12);
        // r10 = r11 >> 8
        emit_mov_rr(buf, true, Reg::R10, Reg::R11);
        emit_shift_ri(buf, ShiftOp::Shr, true, Reg::R10, 8);
        // r11 = r11 ^ r10
        emit_arith_rr(buf, ArithOp::Xor, true, Reg::R11, Reg::R10);
        emit_arith_ri(buf, ArithOp::And, true, Reg::R11, cfg.index_mask as i32);
        // r10 = TLB base pointer
        emit_load(buf, true, Reg::R10, Reg::Rbp, cfg.tlb_ptr_offset as i32);
        // r11 = index * entry_size
        emit_imul_ri(buf, true, Reg::R11, Reg::R11, cfg.entry_size as i32);
        // r10 = entry pointer
        emit_arith_rr(buf, ArithOp::Add, true, Reg::R10, Reg::R11);
        // r11 = entry.addr_read (tag)
        emit_load(buf, true, Reg::R11, Reg::R10, cfg.addr_read_off as i32);
        // Reload addr from stack, compute tag
        emit_load(buf, true, Reg::Rax, Reg::Rsp, Self::MMIO_SCRATCH);
        emit_shift_ri(buf, ShiftOp::Shr, true, Reg::Rax, 12);
        emit_shift_ri(buf, ShiftOp::Shl, true, Reg::Rax, 12);
        // Tag compare
        emit_arith_rr(buf, ArithOp::Cmp, true, Reg::R11, Reg::Rax);
        emit_opc(buf, OPC_JCC_long + (X86Cond::Jne as u32), 0, 0);
        let jne_off = buf.offset();
        buf.emit_u32(0);

        // r11 = entry.addend
        emit_load(buf, true, Reg::R11, Reg::R10, cfg.addend_off as i32);
        // MMIO sentinel check
        emit_arith_ri(buf, ArithOp::Cmp, true, Reg::R11, -1i32);
        emit_opc(buf, OPC_JCC_long + (X86Cond::Je as u32), 0, 0);
        let je_off = buf.offset();
        buf.emit_u32(0);

        // -- Fast path: host_addr = addr + addend --
        emit_load(buf, true, Reg::Rax, Reg::Rsp, Self::MMIO_SCRATCH);
        emit_arith_rr(buf, ArithOp::Add, true, Reg::Rax, Reg::R11);
        // Inline load from [rax]
        match (size, sign) {
            (0, false) => {
                emit_load_zx(buf, OPC_MOVZBL, dst, Reg::Rax, 0);
            }
            (0, true) => {
                let o = if rexw {
                    OPC_MOVSBL | P_REXW
                } else {
                    OPC_MOVSBL
                };
                emit_load_sx(buf, o, dst, Reg::Rax, 0);
            }
            (1, false) => {
                emit_load_zx(buf, OPC_MOVZWL, dst, Reg::Rax, 0);
            }
            (1, true) => {
                let o = if rexw {
                    OPC_MOVSWL | P_REXW
                } else {
                    OPC_MOVSWL
                };
                emit_load_sx(buf, o, dst, Reg::Rax, 0);
            }
            (2, false) => {
                emit_load(buf, false, dst, Reg::Rax, 0);
            }
            (2, true) => {
                emit_load_sx(buf, OPC_MOVSLQ, dst, Reg::Rax, 0);
            }
            (3, _) => {
                emit_load(buf, true, dst, Reg::Rax, 0);
            }
            _ => unreachable!(),
        }
        // Restore scratch regs (skip if dst == reg).
        if dst != Reg::R10 {
            emit_load(buf, true, Reg::R10, Reg::Rsp, Self::TLB_SAVE_R10);
        }
        if dst != Reg::R11 {
            emit_load(buf, true, Reg::R11, Reg::Rsp, Self::TLB_SAVE_R11);
        }
        if dst != Reg::Rax {
            emit_load(buf, true, Reg::Rax, Reg::Rsp, Self::TLB_SAVE_RAX);
        }
        // Skip slow path
        buf.emit_u8(OPC_JMP_long as u8);
        let jmp_done = buf.offset();
        buf.emit_u32(0);

        // -- Slow path --
        let slow = buf.offset();
        Self::patch_disp(buf, jne_off, slow);
        Self::patch_disp(buf, je_off, slow);

        Self::emit_save_caller_regs(buf);
        emit_mov_rr(buf, true, Reg::Rdi, Reg::Rbp);
        emit_load(buf, true, Reg::Rsi, Reg::Rsp, Self::MMIO_SCRATCH);
        emit_mov_ri(buf, false, Reg::Rdx, size_bytes as u64);
        emit_mov_ri(buf, true, Reg::R11, cfg.load_helper);
        emit_call_reg(buf, Reg::R11);

        // Save result, restore regs, load result.
        emit_store(buf, true, Reg::Rax, Reg::Rsp, Self::MMIO_SCRATCH);
        Self::emit_restore_caller_regs(buf);
        emit_load(buf, true, dst, Reg::Rsp, Self::MMIO_SCRATCH);

        // Sign-extend if needed
        if sign {
            match size {
                0 => {
                    let o = if rexw {
                        OPC_MOVSBL | P_REXW
                    } else {
                        OPC_MOVSBL
                    };
                    emit_movsx(buf, o, dst, dst);
                }
                1 => {
                    let o = if rexw {
                        OPC_MOVSWL | P_REXW
                    } else {
                        OPC_MOVSWL
                    };
                    emit_movsx(buf, o, dst, dst);
                }
                2 => {
                    emit_movsx(buf, OPC_MOVSLQ, dst, dst);
                }
                _ => {}
            }
        }

        // Check mem_fault_cause after helper.
        self.emit_fault_check(buf, cfg);

        // done:
        Self::patch_disp(buf, jmp_done, buf.offset());
    }

    /// Emit QemuSt with inline TLB check and
    /// post-helper fault exit.
    pub(crate) fn emit_qemu_st_mmio(
        &self,
        buf: &mut CodeBuffer,
        cfg: &SoftMmuConfig,
        val: Reg,
        addr: Reg,
        memop: u16,
    ) {
        let size = memop & 0x3;
        let size_bytes: u32 = 1 << size;

        // Save addr, val, and scratch regs.
        emit_store(buf, true, addr, Reg::Rsp, Self::MMIO_SCRATCH);
        emit_store(buf, true, val, Reg::Rsp, Self::MMIO_SCRATCH + 8);
        emit_store(buf, true, Reg::Rax, Reg::Rsp, Self::TLB_SAVE_RAX);
        emit_store(buf, true, Reg::R10, Reg::Rsp, Self::TLB_SAVE_R10);
        emit_store(buf, true, Reg::R11, Reg::Rsp, Self::TLB_SAVE_R11);

        // -- TLB inline check --
        // index = ((vpn ^ (vpn >> 8)) & mask)
        emit_mov_rr(buf, true, Reg::R11, addr);
        emit_shift_ri(buf, ShiftOp::Shr, true, Reg::R11, 12);
        emit_mov_rr(buf, true, Reg::R10, Reg::R11);
        emit_shift_ri(buf, ShiftOp::Shr, true, Reg::R10, 8);
        emit_arith_rr(buf, ArithOp::Xor, true, Reg::R11, Reg::R10);
        emit_arith_ri(buf, ArithOp::And, true, Reg::R11, cfg.index_mask as i32);
        emit_load(buf, true, Reg::R10, Reg::Rbp, cfg.tlb_ptr_offset as i32);
        emit_imul_ri(buf, true, Reg::R11, Reg::R11, cfg.entry_size as i32);
        emit_arith_rr(buf, ArithOp::Add, true, Reg::R10, Reg::R11);
        // r11 = entry.addr_write (tag)
        emit_load(buf, true, Reg::R11, Reg::R10, cfg.addr_write_off as i32);
        // Reload addr, compute tag
        emit_load(buf, true, Reg::Rax, Reg::Rsp, Self::MMIO_SCRATCH);
        emit_shift_ri(buf, ShiftOp::Shr, true, Reg::Rax, 12);
        emit_shift_ri(buf, ShiftOp::Shl, true, Reg::Rax, 12);
        emit_arith_rr(buf, ArithOp::Cmp, true, Reg::R11, Reg::Rax);
        emit_opc(buf, OPC_JCC_long + (X86Cond::Jne as u32), 0, 0);
        let jne_off = buf.offset();
        buf.emit_u32(0);

        // r11 = entry.addend
        emit_load(buf, true, Reg::R11, Reg::R10, cfg.addend_off as i32);
        emit_arith_ri(buf, ArithOp::Cmp, true, Reg::R11, -1i32);
        emit_opc(buf, OPC_JCC_long + (X86Cond::Je as u32), 0, 0);
        let je_off = buf.offset();
        buf.emit_u32(0);

        // -- Fast path: host_addr = addr + addend --
        emit_load(buf, true, Reg::Rax, Reg::Rsp, Self::MMIO_SCRATCH);
        emit_arith_rr(buf, ArithOp::Add, true, Reg::Rax, Reg::R11);
        // Reload val from stack into R11
        emit_load(buf, true, Reg::R11, Reg::Rsp, Self::MMIO_SCRATCH + 8);
        // Inline store to [rax]
        match size {
            0 => emit_store_byte(buf, Reg::R11, Reg::Rax, 0),
            1 => emit_store_word(buf, Reg::R11, Reg::Rax, 0),
            2 => emit_store(buf, false, Reg::R11, Reg::Rax, 0),
            3 => emit_store(buf, true, Reg::R11, Reg::Rax, 0),
            _ => unreachable!(),
        }
        // Restore scratch regs after fast-path store.
        emit_load(buf, true, Reg::R10, Reg::Rsp, Self::TLB_SAVE_R10);
        emit_load(buf, true, Reg::R11, Reg::Rsp, Self::TLB_SAVE_R11);
        emit_load(buf, true, Reg::Rax, Reg::Rsp, Self::TLB_SAVE_RAX);
        // Skip slow path
        buf.emit_u8(OPC_JMP_long as u8);
        let jmp_done = buf.offset();
        buf.emit_u32(0);

        // -- Slow path --
        let slow = buf.offset();
        Self::patch_disp(buf, jne_off, slow);
        Self::patch_disp(buf, je_off, slow);

        Self::emit_save_caller_regs(buf);
        emit_mov_rr(buf, true, Reg::Rdi, Reg::Rbp);
        emit_load(buf, true, Reg::Rsi, Reg::Rsp, Self::MMIO_SCRATCH);
        emit_load(buf, true, Reg::Rdx, Reg::Rsp, Self::MMIO_SCRATCH + 8);
        emit_mov_ri(buf, false, Reg::Rcx, size_bytes as u64);
        emit_mov_ri(buf, true, Reg::R11, cfg.store_helper);
        emit_call_reg(buf, Reg::R11);
        Self::emit_restore_caller_regs(buf);

        // Check mem_fault_cause after helper.
        self.emit_fault_check(buf, cfg);

        // done:
        Self::patch_disp(buf, jmp_done, buf.offset());
    }

    /// Emit a post-helper fault check: if
    /// mem_fault_cause != 0, exit TB immediately.
    fn emit_fault_check(&self, _buf: &mut CodeBuffer, _cfg: &SoftMmuConfig) {
        // Disabled — faults handled at TB boundary by
        // check_mem_fault() in exec loop.
    }

    /// Patch a jmp/jcc disp32 field.
    fn patch_disp(buf: &mut CodeBuffer, disp_off: usize, target: usize) {
        let d = (target as i64) - (disp_off as i64 + 4);
        buf.patch_u32(disp_off, d as u32);
    }
}

impl Default for X86_64CodeGen {
    fn default() -> Self {
        Self::new()
    }
}
