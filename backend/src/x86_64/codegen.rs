use crate::code_buffer::CodeBuffer;
use crate::constraint::OpConstraint;
use crate::x86_64::emitter::*;
use crate::x86_64::regs::{
    Reg, CALLEE_SAVED, CALL_ARG_REGS, STACK_ADDEND, STATIC_CALL_ARGS_SIZE,
};
use crate::HostCodeGen;
use machina_core::{Cond, Context, Op, Opcode, Type};

impl HostCodeGen for X86_64CodeGen {
    fn op_constraint(&self, opc: Opcode) -> &'static OpConstraint {
        crate::x86_64::constraints::op_constraint(opc)
    }

    fn emit_prologue(&mut self, buf: &mut CodeBuffer) {
        self.prologue_offset = buf.offset();
        for &reg in CALLEE_SAVED {
            emit_push(buf, reg);
        }
        // mov TCG_AREG0 (rbp), rdi
        emit_mov_rr(buf, true, Reg::Rbp, CALL_ARG_REGS[0]);
        // Load guest_base into R14: mov r14, [rbp+520]
        emit_load(
            buf,
            true,
            Reg::R14,
            Reg::Rbp,
            520, // GUEST_BASE_OFFSET
        );
        // sub rsp, STACK_ADDEND
        emit_arith_ri(buf, ArithOp::Sub, true, Reg::Rsp, STACK_ADDEND as i32);
        // jmp *rsi (TB code pointer)
        emit_jmp_reg(buf, CALL_ARG_REGS[1]);
        self.code_gen_start = buf.offset();
    }

    fn emit_epilogue(&mut self, buf: &mut CodeBuffer) {
        self.epilogue_return_zero_offset = buf.offset();
        emit_mov_ri(buf, false, Reg::Rax, 0);
        self.tb_ret_offset = buf.offset();
        emit_arith_ri(buf, ArithOp::Add, true, Reg::Rsp, STACK_ADDEND as i32);
        for &reg in CALLEE_SAVED.iter().rev() {
            emit_pop(buf, reg);
        }
        emit_ret(buf);
    }

    fn patch_jump(
        &self,
        buf: &CodeBuffer,
        jump_offset: usize,
        target_offset: usize,
    ) {
        let disp = (target_offset as i64) - (jump_offset as i64 + 5);
        assert!(
            (i32::MIN as i64..=i32::MAX as i64).contains(&disp),
            "jump displacement out of i32 range"
        );
        buf.patch_u32(jump_offset + 1, disp as u32);
    }

    fn epilogue_offset(&self) -> usize {
        self.tb_ret_offset
    }

    fn init_context(&self, ctx: &mut machina_core::Context) {
        use crate::x86_64::regs;
        ctx.reserved_regs = regs::RESERVED_REGS;
        ctx.set_frame(
            Reg::Rsp as u8,
            STATIC_CALL_ARGS_SIZE as i64,
            (regs::CPU_TEMP_BUF_NLONGS * 8) as i64,
        );
    }

    fn tcg_out_mov(&self, buf: &mut CodeBuffer, ty: Type, dst: u8, src: u8) {
        if dst == src {
            return;
        }
        let rexw = ty == Type::I64;
        emit_mov_rr(buf, rexw, Reg::from_u8(dst), Reg::from_u8(src));
    }

    fn tcg_out_movi(&self, buf: &mut CodeBuffer, ty: Type, dst: u8, val: u64) {
        let rexw = ty == Type::I64;
        emit_mov_ri(buf, rexw, Reg::from_u8(dst), val);
    }

    fn tcg_out_ld(
        &self,
        buf: &mut CodeBuffer,
        ty: Type,
        dst: u8,
        base: u8,
        offset: i64,
    ) {
        let rexw = ty == Type::I64;
        emit_load(
            buf,
            rexw,
            Reg::from_u8(dst),
            Reg::from_u8(base),
            offset as i32,
        );
    }

    fn tcg_out_st(
        &self,
        buf: &mut CodeBuffer,
        ty: Type,
        src: u8,
        base: u8,
        offset: i64,
    ) {
        let rexw = ty == Type::I64;
        emit_store(
            buf,
            rexw,
            Reg::from_u8(src),
            Reg::from_u8(base),
            offset as i32,
        );
    }

    fn tcg_out_op(
        &self,
        buf: &mut CodeBuffer,
        ctx: &Context,
        op: &Op,
        oregs: &[u8],
        iregs: &[u8],
        cargs: &[u32],
    ) {
        let rexw = op.op_type == Type::I64;
        match op.opc {
            Opcode::Add => {
                let d = Reg::from_u8(oregs[0]);
                let a = Reg::from_u8(iregs[0]);
                let b = Reg::from_u8(iregs[1]);
                if oregs[0] == iregs[0] {
                    emit_arith_rr(buf, ArithOp::Add, rexw, d, b);
                } else if oregs[0] == iregs[1] {
                    emit_arith_rr(buf, ArithOp::Add, rexw, d, a);
                } else {
                    emit_lea_sib(buf, rexw, d, a, b, 0, 0);
                }
            }
            // Constraints guarantee oregs[0] == iregs[0]
            Opcode::Sub => {
                let d = Reg::from_u8(oregs[0]);
                let b = Reg::from_u8(iregs[1]);
                emit_arith_rr(buf, ArithOp::Sub, rexw, d, b);
            }
            Opcode::Mul => {
                let d = Reg::from_u8(oregs[0]);
                let b = Reg::from_u8(iregs[1]);
                emit_imul_rr(buf, rexw, d, b);
            }
            Opcode::And => {
                let d = Reg::from_u8(oregs[0]);
                let b = Reg::from_u8(iregs[1]);
                emit_arith_rr(buf, ArithOp::And, rexw, d, b);
            }
            Opcode::Or => {
                let d = Reg::from_u8(oregs[0]);
                let b = Reg::from_u8(iregs[1]);
                emit_arith_rr(buf, ArithOp::Or, rexw, d, b);
            }
            Opcode::Xor => {
                let d = Reg::from_u8(oregs[0]);
                let b = Reg::from_u8(iregs[1]);
                emit_arith_rr(buf, ArithOp::Xor, rexw, d, b);
            }
            Opcode::Neg => {
                emit_neg(buf, rexw, Reg::from_u8(oregs[0]));
            }
            Opcode::Not => {
                emit_not(buf, rexw, Reg::from_u8(oregs[0]));
            }
            // Constraints guarantee oregs[0] == iregs[0]
            // and iregs[1] == RCX.
            Opcode::Shl | Opcode::Shr | Opcode::Sar => {
                let d = Reg::from_u8(oregs[0]);
                let sop = match op.opc {
                    Opcode::Shl => ShiftOp::Shl,
                    Opcode::Shr => ShiftOp::Shr,
                    Opcode::Sar => ShiftOp::Sar,
                    _ => unreachable!(),
                };
                emit_shift_cl(buf, sop, rexw, d);
            }
            Opcode::SetCond => {
                let d = Reg::from_u8(oregs[0]);
                let a = Reg::from_u8(iregs[0]);
                let b = Reg::from_u8(iregs[1]);
                let cond = cond_from_u32(cargs[0]);
                let x86c = X86Cond::from_tcg(cond);
                if cond.is_tst() {
                    emit_test_rr(buf, rexw, a, b);
                } else {
                    emit_arith_rr(buf, ArithOp::Cmp, rexw, a, b);
                }
                emit_setcc(buf, x86c, d);
                emit_movzx(buf, OPC_MOVZBL | P_REXB_RM, d, d);
            }
            Opcode::BrCond => {
                let a = Reg::from_u8(iregs[0]);
                let b = Reg::from_u8(iregs[1]);
                let cond = cond_from_u32(cargs[0]);
                let label_id = cargs[1];
                let x86c = X86Cond::from_tcg(cond);
                if cond.is_tst() {
                    emit_test_rr(buf, rexw, a, b);
                } else {
                    emit_arith_rr(buf, ArithOp::Cmp, rexw, a, b);
                }
                let label = ctx.label(label_id);
                if label.has_value {
                    emit_jcc(buf, x86c, label.value);
                } else {
                    emit_opc(buf, OPC_JCC_long + (x86c as u32), 0, 0);
                    buf.emit_u32(0);
                }
            }
            Opcode::Ld => {
                let d = Reg::from_u8(oregs[0]);
                let base = Reg::from_u8(iregs[0]);
                let offset = cargs[0] as i32;
                emit_load(buf, rexw, d, base, offset);
            }
            Opcode::Ld8U => {
                let d = Reg::from_u8(oregs[0]);
                let base = Reg::from_u8(iregs[0]);
                let offset = cargs[0] as i32;
                emit_load_zx(buf, OPC_MOVZBL, d, base, offset);
            }
            Opcode::Ld8S => {
                let d = Reg::from_u8(oregs[0]);
                let base = Reg::from_u8(iregs[0]);
                let offset = cargs[0] as i32;
                let opc = if rexw {
                    OPC_MOVSBL | P_REXW
                } else {
                    OPC_MOVSBL
                };
                emit_load_sx(buf, opc, d, base, offset);
            }
            Opcode::Ld16U => {
                let d = Reg::from_u8(oregs[0]);
                let base = Reg::from_u8(iregs[0]);
                let offset = cargs[0] as i32;
                emit_load_zx(buf, OPC_MOVZWL, d, base, offset);
            }
            Opcode::Ld16S => {
                let d = Reg::from_u8(oregs[0]);
                let base = Reg::from_u8(iregs[0]);
                let offset = cargs[0] as i32;
                let opc = if rexw {
                    OPC_MOVSWL | P_REXW
                } else {
                    OPC_MOVSWL
                };
                emit_load_sx(buf, opc, d, base, offset);
            }
            Opcode::Ld32U => {
                let d = Reg::from_u8(oregs[0]);
                let base = Reg::from_u8(iregs[0]);
                let offset = cargs[0] as i32;
                // MOV r32, [mem] implicitly zero-extends
                emit_load(buf, false, d, base, offset);
            }
            Opcode::Ld32S => {
                let d = Reg::from_u8(oregs[0]);
                let base = Reg::from_u8(iregs[0]);
                let offset = cargs[0] as i32;
                emit_load_sx(buf, OPC_MOVSLQ, d, base, offset);
            }
            Opcode::St => {
                let src = Reg::from_u8(iregs[0]);
                let base = Reg::from_u8(iregs[1]);
                let offset = cargs[0] as i32;
                emit_store(buf, rexw, src, base, offset);
            }
            Opcode::St8 => {
                let src = Reg::from_u8(iregs[0]);
                let base = Reg::from_u8(iregs[1]);
                let offset = cargs[0] as i32;
                emit_store_byte(buf, src, base, offset);
            }
            Opcode::St16 => {
                let src = Reg::from_u8(iregs[0]);
                let base = Reg::from_u8(iregs[1]);
                let offset = cargs[0] as i32;
                emit_store_word(buf, src, base, offset);
            }
            Opcode::St32 => {
                let src = Reg::from_u8(iregs[0]);
                let base = Reg::from_u8(iregs[1]);
                let offset = cargs[0] as i32;
                emit_store(buf, false, src, base, offset);
            }
            // -- Type conversions --
            Opcode::ExtI32I64 => {
                let d = Reg::from_u8(oregs[0]);
                let s = Reg::from_u8(iregs[0]);
                emit_movsx(buf, OPC_MOVSLQ, d, s);
            }
            Opcode::ExtUI32I64 | Opcode::ExtrlI64I32 => {
                let d = Reg::from_u8(oregs[0]);
                let s = Reg::from_u8(iregs[0]);
                // MOV r32, r32 zero-extends to 64 bits
                // (also works as truncate: just ignore high bits)
                if d != s {
                    emit_mov_rr(buf, false, d, s);
                }
            }
            Opcode::ExitTb => {
                let val = cargs[0] as u64;
                let encoded = machina_core::tb::encode_tb_exit(ctx.tb_idx, val);
                self.emit_exit_tb(buf, encoded);
            }
            Opcode::GotoTb => {
                let (jmp, reset) = self.emit_goto_tb(buf);
                self.goto_tb_info.lock().unwrap().push((jmp, reset));
            }
            // -- Rotates: same pattern as shifts --
            Opcode::RotL | Opcode::RotR => {
                let d = Reg::from_u8(oregs[0]);
                let sop = match op.opc {
                    Opcode::RotL => ShiftOp::Rol,
                    Opcode::RotR => ShiftOp::Ror,
                    _ => unreachable!(),
                };
                emit_shift_cl(buf, sop, rexw, d);
            }
            // -- Double-width multiply --
            Opcode::MulS2 => {
                let src = Reg::from_u8(iregs[1]);
                emit_imul1(buf, rexw, src);
            }
            Opcode::MulU2 => {
                let src = Reg::from_u8(iregs[1]);
                emit_mul(buf, rexw, src);
            }
            // -- Double-width divide --
            Opcode::DivS2 => {
                let divisor = Reg::from_u8(iregs[2]);
                emit_idiv(buf, rexw, divisor);
            }
            Opcode::DivU2 => {
                let divisor = Reg::from_u8(iregs[2]);
                emit_div(buf, rexw, divisor);
            }
            // -- Carry/borrow arithmetic --
            Opcode::AddCO => {
                let d = Reg::from_u8(oregs[0]);
                let b = Reg::from_u8(iregs[1]);
                emit_arith_rr(buf, ArithOp::Add, rexw, d, b);
            }
            Opcode::AddCI | Opcode::AddCIO => {
                let d = Reg::from_u8(oregs[0]);
                let b = Reg::from_u8(iregs[1]);
                emit_arith_rr(buf, ArithOp::Adc, rexw, d, b);
            }
            Opcode::AddC1O => {
                // ADD with carry-in=1: STC then ADC
                let d = Reg::from_u8(oregs[0]);
                let b = Reg::from_u8(iregs[1]);
                emit_stc(buf);
                emit_arith_rr(buf, ArithOp::Adc, rexw, d, b);
            }
            Opcode::SubBO => {
                let d = Reg::from_u8(oregs[0]);
                let b = Reg::from_u8(iregs[1]);
                emit_arith_rr(buf, ArithOp::Sub, rexw, d, b);
            }
            Opcode::SubBI | Opcode::SubBIO => {
                let d = Reg::from_u8(oregs[0]);
                let b = Reg::from_u8(iregs[1]);
                emit_arith_rr(buf, ArithOp::Sbb, rexw, d, b);
            }
            Opcode::SubB1O => {
                // SUB with borrow-in=1: STC then SBB
                let d = Reg::from_u8(oregs[0]);
                let b = Reg::from_u8(iregs[1]);
                emit_stc(buf);
                emit_arith_rr(buf, ArithOp::Sbb, rexw, d, b);
            }
            // -- AndC: ANDN dst, src2, src1 = src1 & ~src2 --
            Opcode::AndC => {
                let d = Reg::from_u8(oregs[0]);
                let a = Reg::from_u8(iregs[0]);
                let b = Reg::from_u8(iregs[1]);
                // ANDN dst, b, a => a & ~b
                emit_andn(buf, rexw, d, b, a);
            }
            // -- Bit-field extract (unsigned) --
            Opcode::Extract => {
                let d = Reg::from_u8(oregs[0]);
                let s = Reg::from_u8(iregs[0]);
                let ofs = cargs[0];
                let len = cargs[1];
                assert!(ofs == 0, "Extract: only ofs=0 supported");
                match len {
                    8 => emit_movzx(buf, OPC_MOVZBL, d, s),
                    16 => emit_movzx(buf, OPC_MOVZWL, d, s),
                    32 => {
                        emit_mov_rr(buf, false, d, s);
                    }
                    _ => panic!("Extract: unsupported len={len}"),
                }
            }
            // -- Bit-field extract (signed) --
            Opcode::SExtract => {
                let d = Reg::from_u8(oregs[0]);
                let s = Reg::from_u8(iregs[0]);
                let ofs = cargs[0];
                let len = cargs[1];
                assert!(ofs == 0, "SExtract: only ofs=0 supported");
                match len {
                    8 => {
                        let opc = if rexw {
                            OPC_MOVSBL | P_REXW
                        } else {
                            OPC_MOVSBL
                        };
                        emit_movsx(buf, opc, d, s);
                    }
                    16 => {
                        let opc = if rexw {
                            OPC_MOVSWL | P_REXW
                        } else {
                            OPC_MOVSWL
                        };
                        emit_movsx(buf, opc, d, s);
                    }
                    32 => {
                        emit_movsx(buf, OPC_MOVSLQ, d, s);
                    }
                    _ => panic!("SExtract: unsupported len={len}"),
                }
            }
            // -- Deposit: bit-field store (ofs=0, len=8/16) --
            Opcode::Deposit => {
                let d = Reg::from_u8(oregs[0]);
                let src = Reg::from_u8(iregs[1]);
                let ofs = cargs[0];
                let len = cargs[1];
                assert!(ofs == 0, "Deposit: only ofs=0 supported");
                match len {
                    8 => {
                        // MOV byte: overwrite low 8 bits
                        emit_modrm(
                            buf,
                            OPC_MOVB_EvGv | P_REXB_R | P_REXB_RM,
                            src,
                            d,
                        );
                    }
                    16 => {
                        // MOV word: overwrite low 16 bits
                        emit_modrm(buf, P_DATA16 | OPC_MOVL_EvGv, src, d);
                    }
                    _ => panic!("Deposit: unsupported len={len}"),
                }
            }
            // -- Extract2: SHRD dst, src, imm --
            Opcode::Extract2 => {
                let d = Reg::from_u8(oregs[0]);
                let src = Reg::from_u8(iregs[1]);
                let shift = cargs[0] as u8;
                emit_shrd_ri(buf, rexw, d, src, shift);
            }
            // -- Byte swap --
            Opcode::Bswap16 => {
                let d = Reg::from_u8(oregs[0]);
                let flags = cargs[0];
                // TCG_BSWAP_OS = 4, TCG_BSWAP_OZ = 2,
                // TCG_BSWAP_IZ = 1
                if flags & 4 != 0 {
                    // Output sign-extended
                    if rexw {
                        emit_bswap(buf, true, d);
                        emit_shift_ri(buf, ShiftOp::Sar, true, d, 48);
                    } else {
                        emit_bswap(buf, false, d);
                        emit_shift_ri(buf, ShiftOp::Sar, false, d, 16);
                    }
                } else if flags & 3 == 2 {
                    // OZ set, IZ not set
                    emit_bswap(buf, false, d);
                    emit_shift_ri(buf, ShiftOp::Shr, false, d, 16);
                } else {
                    emit_rolw_8(buf, d);
                }
            }
            Opcode::Bswap32 => {
                let d = Reg::from_u8(oregs[0]);
                let flags = cargs[0];
                emit_bswap(buf, false, d);
                if flags & 4 != 0 {
                    // TCG_BSWAP_OS: sign-extend to 64
                    emit_movsx(buf, OPC_MOVSLQ, d, d);
                }
            }
            Opcode::Bswap64 => {
                let d = Reg::from_u8(oregs[0]);
                emit_bswap(buf, true, d);
            }
            // -- Bit counting --
            Opcode::Clz => {
                let d = Reg::from_u8(oregs[0]);
                let a = Reg::from_u8(iregs[0]);
                // Assume LZCNT available (BMI1)
                emit_lzcnt(buf, rexw, d, a);
            }
            Opcode::Ctz => {
                let d = Reg::from_u8(oregs[0]);
                let a = Reg::from_u8(iregs[0]);
                // Assume TZCNT available (BMI1)
                emit_tzcnt(buf, rexw, d, a);
            }
            Opcode::CtPop => {
                let d = Reg::from_u8(oregs[0]);
                let a = Reg::from_u8(iregs[0]);
                emit_popcnt(buf, rexw, d, a);
            }
            // -- NegSetCond: CMP + SETCC + MOVZBL + NEG --
            Opcode::NegSetCond => {
                let d = Reg::from_u8(oregs[0]);
                let a = Reg::from_u8(iregs[0]);
                let b = Reg::from_u8(iregs[1]);
                let cond = cond_from_u32(cargs[0]);
                let x86c = X86Cond::from_tcg(cond);
                if cond.is_tst() {
                    emit_test_rr(buf, rexw, a, b);
                } else {
                    emit_arith_rr(buf, ArithOp::Cmp, rexw, a, b);
                }
                emit_setcc(buf, x86c, d);
                emit_movzx(buf, OPC_MOVZBL | P_REXB_RM, d, d);
                emit_neg(buf, rexw, d);
            }
            // -- MovCond: CMP + CMOV --
            Opcode::MovCond => {
                let d = Reg::from_u8(oregs[0]);
                let c1 = Reg::from_u8(iregs[0]);
                let c2 = Reg::from_u8(iregs[1]);
                // d == v1 (alias), v2 = iregs[3]
                let v2 = Reg::from_u8(iregs[3]);
                let cond = cond_from_u32(cargs[0]);
                let x86c = X86Cond::from_tcg(cond);
                if cond.is_tst() {
                    emit_test_rr(buf, rexw, c1, c2);
                } else {
                    emit_arith_rr(buf, ArithOp::Cmp, rexw, c1, c2);
                }
                // CMOV on inverted condition: if cond is
                // false, move v2 into d (which already
                // holds v1).
                emit_cmovcc(buf, x86c.invert(), rexw, d, v2);
            }
            // -- ExtrhI64I32: SHR reg, 32 --
            Opcode::ExtrhI64I32 => {
                let d = Reg::from_u8(oregs[0]);
                emit_shift_ri(buf, ShiftOp::Shr, true, d, 32);
            }
            // -- GotoPtr: indirect jump through register --
            Opcode::GotoPtr => {
                let reg = Reg::from_u8(iregs[0]);
                X86_64CodeGen::emit_goto_ptr(buf, reg);
            }
            // -- Guest memory load (user-mode: [R14 + addr]) --
            Opcode::QemuLd => {
                let d = Reg::from_u8(oregs[0]);
                let addr = Reg::from_u8(iregs[0]);
                let memop = cargs[0] as u16;
                let size = memop & 0x3;
                let sign = memop & 4 != 0;
                let gb = Reg::R14;
                match (size, sign) {
                    (0, false) => {
                        emit_load_zx_sib(buf, OPC_MOVZBL, d, gb, addr);
                    }
                    (0, true) => {
                        let opc = if rexw {
                            OPC_MOVSBL | P_REXW
                        } else {
                            OPC_MOVSBL
                        };
                        emit_load_sx_sib(buf, opc, d, gb, addr);
                    }
                    (1, false) => {
                        emit_load_zx_sib(buf, OPC_MOVZWL, d, gb, addr);
                    }
                    (1, true) => {
                        let opc = if rexw {
                            OPC_MOVSWL | P_REXW
                        } else {
                            OPC_MOVSWL
                        };
                        emit_load_sx_sib(buf, opc, d, gb, addr);
                    }
                    (2, false) => {
                        // MOV r32 zero-extends to 64
                        emit_load_sib(buf, false, d, gb, addr, 0, 0);
                    }
                    (2, true) => {
                        emit_load_sx_sib(buf, OPC_MOVSLQ, d, gb, addr);
                    }
                    (3, _) => {
                        emit_load_sib(buf, true, d, gb, addr, 0, 0);
                    }
                    _ => unreachable!(),
                }
            }
            // -- Guest memory store (user-mode: [R14 + addr]) --
            Opcode::QemuSt => {
                let val = Reg::from_u8(iregs[0]);
                let addr = Reg::from_u8(iregs[1]);
                let memop = cargs[0] as u16;
                let size = memop & 0x3;
                let gb = Reg::R14;
                match size {
                    0 => {
                        emit_store_byte_sib(buf, val, gb, addr);
                    }
                    1 => {
                        emit_store_word_sib(buf, val, gb, addr);
                    }
                    2 => {
                        emit_store_sib(buf, false, val, gb, addr, 0, 0);
                    }
                    3 => {
                        emit_store_sib(buf, true, val, gb, addr, 0, 0);
                    }
                    _ => unreachable!(),
                }
            }
            Opcode::Call => {
                let func = (cargs[1] as u64) << 32 | (cargs[0] as u64);
                emit_mov_ri(buf, true, Reg::R11, func);
                emit_call_reg(buf, Reg::R11);
            }
            _ => {
                panic!("tcg_out_op: unhandled {:?}", op.opc,);
            }
        }
    }

    fn goto_tb_offsets(&self) -> Vec<(usize, usize)> {
        self.goto_tb_info.lock().unwrap().clone()
    }

    fn clear_goto_tb_offsets(&self) {
        self.goto_tb_info.lock().unwrap().clear();
    }
}

fn cond_from_u32(val: u32) -> Cond {
    match val {
        0 => Cond::Never,
        1 => Cond::Always,
        8 => Cond::Eq,
        9 => Cond::Ne,
        10 => Cond::Lt,
        11 => Cond::Ge,
        12 => Cond::Le,
        13 => Cond::Gt,
        14 => Cond::Ltu,
        15 => Cond::Geu,
        16 => Cond::Leu,
        17 => Cond::Gtu,
        18 => Cond::TstEq,
        19 => Cond::TstNe,
        _ => panic!("invalid Cond value: {val}"),
    }
}
