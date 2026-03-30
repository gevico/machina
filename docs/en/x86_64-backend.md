# x86-64 Backend

## 1. Overview

`accel/src/x86_64/emitter.rs` implements a complete GPR instruction encoder for the x86-64 host architecture, referencing QEMU's `tcg/i386/tcg-target.c.inc`. It uses a layered encoding architecture:

```
Prefix Flags (P_*) + Opcode Constants (OPC_*)
        |
        v
Core Encoding Functions (emit_opc / emit_modrm / emit_modrm_offset)
        |
        v
Instruction Emitters (emit_arith_rr / emit_mov_ri / emit_jcc / ...)
        |
        v
Codegen Dispatch (tcg_out_op: IR Opcode --> Instruction Emitter Combinations)
        |
        v
X86_64CodeGen (prologue / epilogue / exit_tb / goto_tb)
```

## 2. Encoding Infrastructure

### 2.1 Prefix Flags (P_*)

Opcode constants use the `u32` type, with high bits encoding prefix information:

| Flag | Value | Meaning |
|------|-------|---------|
| `P_EXT` | 0x100 | 0x0F escape prefix |
| `P_EXT38` | 0x200 | 0x0F 0x38 three-byte escape |
| `P_EXT3A` | 0x10000 | 0x0F 0x3A three-byte escape |
| `P_DATA16` | 0x400 | 0x66 operand size prefix |
| `P_REXW` | 0x1000 | REX.W = 1 (64-bit operation) |
| `P_REXB_R` | 0x2000 | Byte register access for REG field |
| `P_REXB_RM` | 0x4000 | Byte register access for R/M field |
| `P_SIMDF3` | 0x20000 | 0xF3 prefix |
| `P_SIMDF2` | 0x40000 | 0xF2 prefix |

### 2.2 Opcode Constants (OPC_*)

Constant naming follows QEMU's `tcg-target.c.inc` style (using `#![allow(non_upper_case_globals)]`):

```rust
pub const OPC_ARITH_EvIb: u32 = 0x83;        // arithmetic reg, imm8
pub const OPC_MOVL_GvEv: u32 = 0x8B;         // MOV load
pub const OPC_JCC_long: u32 = 0x80 | P_EXT;  // conditional jump rel32
pub const OPC_BSF: u32 = 0xBC | P_EXT;       // bit scan
pub const OPC_LZCNT: u32 = 0xBD | P_EXT | P_SIMDF3; // leading zero count
```

### 2.3 Core Encoding Functions

| Function | Purpose |
|----------|---------|
| `emit_opc(buf, opc, r, rm)` | Emit REX prefix + escape bytes + opcode |
| `emit_modrm(buf, opc, r, rm)` | Register-register ModR/M (mod=11) |
| `emit_modrm_ext(buf, opc, ext, rm)` | /r extension for group opcodes |
| `emit_modrm_offset(buf, opc, r, base, offset)` | Memory [base+disp] |
| `emit_modrm_sib(buf, opc, r, base, index, shift, offset)` | SIB addressing |
| `emit_modrm_ext_offset(buf, opc, ext, base, offset)` | Group opcode + memory |

## 3. Instruction Categories

### 3.1 Arithmetic Instructions

| Function | Instruction | Description |
|----------|-------------|-------------|
| `emit_arith_rr(op, rexw, dst, src)` | ADD/SUB/AND/OR/XOR/CMP/ADC/SBB | Register-register |
| `emit_arith_ri(op, rexw, dst, imm)` | Same | Register-immediate (auto-selects imm8/imm32) |
| `emit_arith_mr(op, rexw, base, offset, src)` | Same | Memory-register (store operation) |
| `emit_arith_rm(op, rexw, dst, base, offset)` | Same | Register-memory (load operation) |
| `emit_neg(rexw, reg)` | NEG | Negate |
| `emit_not(rexw, reg)` | NOT | Bitwise NOT |
| `emit_inc(rexw, reg)` | INC | Increment |
| `emit_dec(rexw, reg)` | DEC | Decrement |

`ArithOp` enum values correspond to the x86 /r field: Add=0, Or=1, Adc=2, Sbb=3, And=4, Sub=5, Xor=6, Cmp=7.

### 3.2 Shift Instructions

| Function | Instruction | Description |
|----------|-------------|-------------|
| `emit_shift_ri(op, rexw, dst, imm)` | SHL/SHR/SAR/ROL/ROR | Immediate shift (imm=1 uses short encoding) |
| `emit_shift_cl(op, rexw, dst)` | Same | Shift by CL register |
| `emit_shld_ri(rexw, dst, src, imm)` | SHLD | Double-precision left shift |
| `emit_shrd_ri(rexw, dst, src, imm)` | SHRD | Double-precision right shift |

### 3.3 Data Movement

| Function | Instruction | Description |
|----------|-------------|-------------|
| `emit_mov_rr(rexw, dst, src)` | MOV r, r | 32/64-bit register transfer |
| `emit_mov_ri(rexw, reg, val)` | MOV r, imm | Smart selection: xor(0) / mov r32(u32) / mov r64 sign-ext(i32) / movabs(i64) |
| `emit_movzx(opc, dst, src)` | MOVZBL/MOVZWL | Zero extension |
| `emit_movsx(opc, dst, src)` | MOVSBL/MOVSWL/MOVSLQ | Sign extension |
| `emit_bswap(rexw, reg)` | BSWAP | Byte swap |

### 3.4 Memory Operations

| Function | Instruction | Description |
|----------|-------------|-------------|
| `emit_load(rexw, dst, base, offset)` | MOV r, [base+disp] | Load |
| `emit_store(rexw, src, base, offset)` | MOV [base+disp], r | Store |
| `emit_store_byte(src, base, offset)` | MOV byte [base+disp], r | Byte store |
| `emit_store_imm(rexw, base, offset, imm)` | MOV [base+disp], imm32 | Immediate store |
| `emit_lea(rexw, dst, base, offset)` | LEA r, [base+disp] | Address calculation |
| `emit_load_sib(rexw, dst, base, index, shift, offset)` | MOV r, [b+i*s+d] | Indexed load |
| `emit_store_sib(rexw, src, base, index, shift, offset)` | MOV [b+i*s+d], r | Indexed store |
| `emit_lea_sib(rexw, dst, base, index, shift, offset)` | LEA r, [b+i*s+d] | Indexed address calculation |
| `emit_load_zx(opc, dst, base, offset)` | MOVZBL/MOVZWL [mem] | Zero-extending load |
| `emit_load_sx(opc, dst, base, offset)` | MOVSBL/MOVSWL/MOVSLQ [mem] | Sign-extending load |

### 3.5 Multiply/Divide Instructions

| Function | Instruction | Description |
|----------|-------------|-------------|
| `emit_mul(rexw, reg)` | MUL | Unsigned multiply RDX:RAX = RAX * reg |
| `emit_imul1(rexw, reg)` | IMUL | Signed multiply (single operand) |
| `emit_imul_rr(rexw, dst, src)` | IMUL r, r | Two-operand multiply |
| `emit_imul_ri(rexw, dst, src, imm)` | IMUL r, r, imm | Three-operand multiply |
| `emit_div(rexw, reg)` | DIV | Unsigned divide |
| `emit_idiv(rexw, reg)` | IDIV | Signed divide |
| `emit_cdq()` | CDQ | Sign-extend EAX → EDX:EAX |
| `emit_cqo()` | CQO | Sign-extend RAX → RDX:RAX |

### 3.6 Bit Operations

| Function | Instruction | Description |
|----------|-------------|-------------|
| `emit_bsf(rexw, dst, src)` | BSF | Bit scan forward |
| `emit_bsr(rexw, dst, src)` | BSR | Bit scan reverse |
| `emit_lzcnt(rexw, dst, src)` | LZCNT | Leading zero count |
| `emit_tzcnt(rexw, dst, src)` | TZCNT | Trailing zero count |
| `emit_popcnt(rexw, dst, src)` | POPCNT | Population count |
| `emit_bt_ri(rexw, reg, bit)` | BT | Bit test |
| `emit_bts_ri(rexw, reg, bit)` | BTS | Bit test and set |
| `emit_btr_ri(rexw, reg, bit)` | BTR | Bit test and reset |
| `emit_btc_ri(rexw, reg, bit)` | BTC | Bit test and complement |
| `emit_andn(rexw, dst, src1, src2)` | ANDN | BMI1: dst = ~src1 & src2 (VEX encoding) |

### 3.7 Branch and Compare

| Function | Instruction | Description |
|----------|-------------|-------------|
| `emit_jcc(cond, target)` | Jcc rel32 | Conditional jump |
| `emit_jmp(target)` | JMP rel32 | Unconditional jump |
| `emit_call(target)` | CALL rel32 | Function call |
| `emit_jmp_reg(reg)` | JMP *reg | Indirect jump |
| `emit_call_reg(reg)` | CALL *reg | Indirect call |
| `emit_setcc(cond, dst)` | SETcc | Conditional set byte |
| `emit_cmovcc(cond, rexw, dst, src)` | CMOVcc | Conditional move |
| `emit_test_rr(rexw, r1, r2)` | TEST r, r | Bitwise AND test |
| `emit_test_bi(reg, imm)` | TEST r8, imm8 | Byte test |

### 3.8 Miscellaneous

| Function | Instruction | Description |
|----------|-------------|-------------|
| `emit_xchg(rexw, r1, r2)` | XCHG | Exchange |
| `emit_push(reg)` | PUSH | Push to stack |
| `emit_pop(reg)` | POP | Pop from stack |
| `emit_push_imm(imm)` | PUSH imm | Push immediate |
| `emit_ret()` | RET | Return |
| `emit_mfence()` | MFENCE | Memory fence |
| `emit_ud2()` | UD2 | Undefined instruction (debug trap) |
| `emit_nops(n)` | NOP | Intel-recommended multi-byte NOP (1-8 bytes) |

## 4. Memory Addressing Special Cases

x86-64 ModR/M encoding has two special registers that require extra handling:

- **RSP/R12 (low3=4)**: When used as a base, a SIB byte is required (`0x24` = index=RSP/none, base=RSP)
- **RBP/R13 (low3=5)**: When used as a base with zero offset, `mod=01, disp8=0` must be used (because `mod=00, rm=5` is encoded as RIP-relative addressing)

`emit_modrm_offset` handles these special cases automatically.

## 5. Condition Code Mapping

The `X86Cond` enum maps TCG conditions to x86 JCC condition codes:

| TCG Cond | X86Cond | JCC Encoding |
|----------|---------|--------------|
| Eq / TstEq | Je | 0x4 |
| Ne / TstNe | Jne | 0x5 |
| Lt | Jl | 0xC |
| Ge | Jge | 0xD |
| Ltu | Jb | 0x2 |
| Geu | Jae | 0x3 |

`X86Cond::invert()` inverts conditions by flipping the low bit (e.g., Je <-> Jne).

## 6. Constraint Table (`constraints.rs`)

`op_constraint()` returns a static `OpConstraint` for each opcode, aligned with QEMU's `tcg_target_op_def()` (`tcg/i386/tcg-target.c.inc`).

| Opcode | Constraint | QEMU Equivalent | Description |
|--------|-----------|-----------------|-------------|
| Add | `o1_i2(R, R, R)` | `C_O1_I2(r,r,re)` | Three-address LEA |
| Sub | `o1_i2_alias(R, R, R)` | `C_O1_I2(r,0,re)` | Destructive SUB, dst==lhs |
| Mul | `o1_i2_alias(R, R, R)` | `C_O1_I2(r,0,r)` | IMUL two-address |
| And/Or/Xor | `o1_i2_alias(R, R, R)` | `C_O1_I2(r,0,re)` | Destructive binary ops |
| Neg/Not | `o1_i1_alias(R, R)` | `C_O1_I1(r,0)` | In-place unary ops |
| Shl/Shr/Sar/RotL/RotR | `o1_i2_alias_fixed(R_NO_RCX, R_NO_RCX, RCX)` | `C_O1_I2(r,0,ci)` | Alias + count fixed to RCX, R_NO_RCX excludes RCX to prevent conflicts |
| SetCond/NegSetCond | `n1_i2(R, R, R)` | `C_N1_I2(r,r,re)` | newreg (setcc writes only the low byte) |
| MovCond | `o1_i4_alias2(R, R, R, R, R)` | `C_O1_I4(r,r,r,0,r)` | Output aliases input2 (CMP+CMOV) |
| BrCond | `o0_i2(R, R)` | `C_O0_I2(r,re)` | No output |
| MulS2/MulU2 | `o2_i2_fixed(RAX, RDX, R_NO_RAX_RDX)` | `C_O2_I2(r,r,0,r)` | Dual fixed output, R_NO_RAX_RDX excludes RAX/RDX to prevent conflicts |
| DivS2/DivU2 | `o2_i3_fixed(RAX, RDX, R_NO_RAX_RDX)` | `C_O2_I3(r,r,0,1,r)` | Dual fixed output + dual alias, R_NO_RAX_RDX excludes RAX/RDX |
| AddCO/AddCI/AddCIO/AddC1O | `o1_i2_alias(R, R, R)` | -- | Carry arithmetic, destructive |
| SubBO/SubBI/SubBIO/SubB1O | `o1_i2_alias(R, R, R)` | -- | Borrow arithmetic, destructive |
| AndC | `o1_i2(R, R, R)` | -- | Three-address ANDN (BMI1) |
| Extract/SExtract | `o1_i1(R, R)` | -- | Bit field extraction |
| Deposit | `o1_i2_alias(R, R, R)` | -- | Bit field insertion, destructive |
| Extract2 | `o1_i2_alias(R, R, R)` | -- | Dual-register extraction (SHRD) |
| Bswap16/32/64 | `o1_i1_alias(R, R)` | -- | Byte swap, in-place |
| Clz/Ctz | `n1_i2(R, R, R)` | -- | Bit count + fallback |
| CtPop | `o1_i1(R, R)` | -- | Population count |
| ExtrhI64I32 | `o1_i1_alias(R, R)` | -- | High 32-bit extraction |
| Ld/Ld* | `o1_i1(R, R)` | -- | No alias |
| St/St* | `o0_i2(R, R)` | -- | No output |
| GotoPtr | `o0_i1(R)` | -- | Indirect jump |

Where `R = ALLOCATABLE_REGS` (14 GPRs, excluding RSP and RBP), `R_NO_RCX = R & ~{RCX}`, `R_NO_RAX_RDX = R & ~{RAX, RDX}`.

The constraint guarantees allow codegen to assume:
- Destructive operations have `oregs[0] == iregs[0]` (no need for a preceding mov)
- Shifts have `iregs[1] == RCX` (no need for push/pop RCX juggling)
- Shift output/input0 are not in RCX (excluded by R_NO_RCX)
- The free input of MulS2/DivS2 is not in RAX/RDX (excluded by R_NO_RAX_RDX)
- SetCond output does not overlap with any input

## 7. Codegen Dispatch (`codegen.rs`)

`tcg_out_op` is the bridge between the register allocator and the instruction encoder. It receives IR ops with allocated host registers and translates them into one or more x86-64 instructions.

### 7.1 HostCodeGen Register Allocator Primitives

| Method | Purpose |
|--------|---------|
| `tcg_out_mov(ty, dst, src)` | Register-to-register transfer |
| `tcg_out_movi(ty, dst, val)` | Load immediate into register |
| `tcg_out_ld(ty, dst, base, offset)` | Load from memory (global variable reload) |
| `tcg_out_st(ty, src, base, offset)` | Store to memory (global variable sync) |

### 7.2 IR Opcode --> x86-64 Instruction Mapping

The constraint system guarantees that codegen receives registers satisfying instruction requirements, so each opcode only needs to emit the simplest instruction sequence:

| IR Opcode | x86-64 Instruction | Constraint Guarantee |
|-----------|--------------------|--------------------|
| Add | d==a: `add d,b`; d==b: `add d,a`; else: `lea d,[a+b]` | Three-address, no alias |
| Sub | `sub d,b` | d==a (oalias) |
| Mul | `imul d,b` | d==a (oalias) |
| And/Or/Xor | `op d,b` | d==a (oalias) |
| Neg/Not | `neg/not d` | d==a (oalias) |
| Shl/Shr/Sar/RotL/RotR | `shift d,cl` | d==a (oalias), count==RCX (fixed) |
| SetCond | `cmp a,b; setcc d; movzbl d,d` | d!=a, d!=b (newreg) |
| NegSetCond | `cmp a,b; setcc d; movzbl d,d; neg d` | d!=a, d!=b (newreg) |
| MovCond | `cmp a,b; cmovcc d,v2` | d==v1 (oalias input2) |
| BrCond | `cmp a,b; jcc label` | No output |
| MulS2/MulU2 | `mul/imul b` (RAX implicit) | o0=RAX, o1=RDX (fixed) |
| DivS2/DivU2 | `cqo/xor; div/idiv b` | o0=RAX, o1=RDX (fixed) |
| AddCO/SubBO | `add/sub d,b` (sets CF) | d==a (oalias) |
| AddCI/SubBI | `adc/sbb d,b` (reads CF) | d==a (oalias) |
| AddCIO/SubBIO | `adc/sbb d,b` (reads+sets CF) | d==a (oalias) |
| AddC1O/SubB1O | `stc; adc/sbb d,b` | d==a (oalias) |
| AndC | `andn d,b,a` (BMI1) | Three-address |
| Extract/SExtract | `shr`+`and` / `movzx` / `movsx` | -- |
| Deposit | `and`+`or` combination | d==a (oalias) |
| Extract2 | `shrd d,b,imm` | d==a (oalias) |
| Bswap16/32/64 | `ror`/`bswap` | d==a (oalias) |
| Clz/Ctz | `lzcnt`/`tzcnt` | d!=a (newreg) |
| CtPop | `popcnt d,a` | -- |
| ExtrhI64I32 | `shr d,32` | d==a (oalias) |
| Ld/Ld* | `mov d,[base+offset]` | -- |
| St/St* | `mov [base+offset],s` | -- |
| ExitTb | `mov rax,val; jmp tb_ret` | -- |
| GotoTb | `jmp rel32` (patchable) | -- |
| GotoPtr | `jmp *reg` | -- |

### 7.3 TstEq/TstNe Support for SetCond/BrCond

When the condition code is `TstEq` or `TstNe`, `test a,b` (bitwise AND test) is used instead of `cmp a,b` (subtraction comparison). This corresponds to the test-and-branch optimization added in QEMU 7.x+.

## 8. QEMU Reference Cross-Reference

| machina Function | QEMU Function |
|-----------------|---------------|
| `emit_opc` | `tcg_out_opc` |
| `emit_modrm` | `tcg_out_modrm` |
| `emit_modrm_offset` | `tcg_out_modrm_sib_offset` |
| `emit_arith_rr` | `tgen_arithr` |
| `emit_arith_ri` | `tgen_arithi` |
| `emit_mov_ri` | `tcg_out_movi` |
| `emit_jcc` | `tcg_out_jxx` |
| `emit_vex_modrm` | `tcg_out_vex_modrm` |
| `X86_64CodeGen::emit_prologue` | `tcg_target_qemu_prologue` |
| `X86_64CodeGen::tcg_out_op` | `tcg_out_op` |
| `X86_64CodeGen::tcg_out_mov` | `tcg_out_mov` |
| `X86_64CodeGen::tcg_out_movi` | `tcg_out_movi` |
| `X86_64CodeGen::tcg_out_ld` | `tcg_out_ld` |
| `X86_64CodeGen::tcg_out_st` | `tcg_out_st` |
| `op_constraint()` | `tcg_target_op_def()` |
| `cond_from_u32` | implicit in QEMU (enum cast) |
