# IR Ops Design Document

This document describes the complete design of machina's intermediate representation (IR) operations, covering the opcode system, type system, Op structure, argument encoding conventions, and the IR Builder API.

Source locations: `core/src/opcode.rs`, `core/src/op.rs`, `core/src/ir_builder.rs`, `core/src/types.rs`.

---

## 1. Design Principles

### 1.1 Unified Polymorphism vs Type-Split

In QEMU's original design, `add_i32` and `add_i64` are distinct opcodes (type-split). machina uses a unified `Add`, with the actual type carried by the `Op::op_type` field (type polymorphism).

**Advantages**:

- Reduces opcode count by ~40%
- The optimizer uses unified logic without needing `match (Add32, Add64) => ...`
- The backend selects 32/64-bit instruction encoding via `op.op_type`, resulting in cleaner logic
- `OpFlags::INT` marks which opcodes are polymorphic; non-polymorphic ones (e.g., `ExtI32I64`) have fixed types

### 1.2 Fixed-Size Argument Array

`Op::args` uses a `[TempIdx; 10]` fixed array instead of `Vec`, avoiding heap allocation. Each TB may contain hundreds of Ops; fixed arrays eliminate significant allocator pressure.

### 1.3 Compile-Time Safety

The `OPCODE_DEFS` table size is `Opcode::Count as usize`. Forgetting to add a table entry when adding a new opcode causes a compile error, fundamentally preventing the table and enum from going out of sync.

---

## 2. Opcode Enum

```rust
#[repr(u8)]
pub enum Opcode { Mov = 0, ..., Count }
```

A total of 158 valid opcodes + 1 sentinel (`Count`), divided into 13 categories:

### 2.1 Data Movement (4)

| Opcode | Semantics | oargs | iargs | cargs | Flags |
|--------|-----------|-------|-------|-------|-------|
| `Mov` | `d = s` | 1 | 1 | 0 | INT, NP |
| `SetCond` | `d = (a cond b) ? 1 : 0` | 1 | 2 | 1 | INT |
| `NegSetCond` | `d = (a cond b) ? -1 : 0` | 1 | 2 | 1 | INT |
| `MovCond` | `d = (c1 cond c2) ? v1 : v2` | 1 | 4 | 1 | INT |

### 2.2 Arithmetic (12)

| Opcode | Semantics | oargs | iargs | cargs | Flags |
|--------|-----------|-------|-------|-------|-------|
| `Add` | `d = a + b` | 1 | 2 | 0 | INT |
| `Sub` | `d = a - b` | 1 | 2 | 0 | INT |
| `Mul` | `d = a * b` | 1 | 2 | 0 | INT |
| `Neg` | `d = -s` | 1 | 1 | 0 | INT |
| `DivS` | `d = a /s b` | 1 | 2 | 0 | INT |
| `DivU` | `d = a /u b` | 1 | 2 | 0 | INT |
| `RemS` | `d = a %s b` | 1 | 2 | 0 | INT |
| `RemU` | `d = a %u b` | 1 | 2 | 0 | INT |
| `DivS2` | `(dl,dh) = (al:ah) /s b` | 2 | 3 | 0 | INT |
| `DivU2` | `(dl,dh) = (al:ah) /u b` | 2 | 3 | 0 | INT |
| `MulSH` | `d = (a *s b) >> N` | 1 | 2 | 0 | INT |
| `MulUH` | `d = (a *u b) >> N` | 1 | 2 | 0 | INT |
| `MulS2` | `(dl,dh) = a *s b` (double-width) | 2 | 2 | 0 | INT |
| `MulU2` | `(dl,dh) = a *u b` (double-width) | 2 | 2 | 0 | INT |

### 2.3 Carry/Borrow Arithmetic (8)

Implicit carry/borrow flags declare dependencies through `CARRY_OUT`/`CARRY_IN` flags.

| Opcode | Semantics | Flags |
|--------|-----------|-------|
| `AddCO` | `d = a + b`, produces carry | INT, CO |
| `AddCI` | `d = a + b + carry` | INT, CI |
| `AddCIO` | `d = a + b + carry`, produces carry | INT, CI, CO |
| `AddC1O` | `d = a + b + 1`, produces carry | INT, CO |
| `SubBO` | `d = a - b`, produces borrow | INT, CO |
| `SubBI` | `d = a - b - borrow` | INT, CI |
| `SubBIO` | `d = a - b - borrow`, produces borrow | INT, CI, CO |
| `SubB1O` | `d = a - b - 1`, produces borrow | INT, CO |

All carry ops have 1 oarg, 2 iargs, 0 cargs.

### 2.4 Logic (9)

| Opcode | Semantics | oargs | iargs |
|--------|-----------|-------|-------|
| `And` | `d = a & b` | 1 | 2 |
| `Or` | `d = a \| b` | 1 | 2 |
| `Xor` | `d = a ^ b` | 1 | 2 |
| `Not` | `d = ~s` | 1 | 1 |
| `AndC` | `d = a & ~b` | 1 | 2 |
| `OrC` | `d = a \| ~b` | 1 | 2 |
| `Eqv` | `d = ~(a ^ b)` | 1 | 2 |
| `Nand` | `d = ~(a & b)` | 1 | 2 |
| `Nor` | `d = ~(a \| b)` | 1 | 2 |

All marked `INT`, 0 cargs.

### 2.5 Shift/Rotate (5)

| Opcode | Semantics |
|--------|-----------|
| `Shl` | `d = a << b` |
| `Shr` | `d = a >> b` (logical) |
| `Sar` | `d = a >> b` (arithmetic) |
| `RotL` | `d = a rotl b` |
| `RotR` | `d = a rotr b` |

All 1 oarg, 2 iargs, 0 cargs, INT.

### 2.6 Bit Field Operations (4)

| Opcode | Semantics | oargs | iargs | cargs |
|--------|-----------|-------|-------|-------|
| `Extract` | `d = (src >> ofs) & mask(len)` | 1 | 1 | 2 (ofs, len) |
| `SExtract` | Same as above, with sign extension | 1 | 1 | 2 (ofs, len) |
| `Deposit` | `d = (a & ~mask) \| ((b << ofs) & mask)` | 1 | 2 | 2 (ofs, len) |
| `Extract2` | `d = (al:ah >> ofs)[N-1:0]` | 1 | 2 | 1 (ofs) |

### 2.7 Byte Swap (3)

| Opcode | Semantics | cargs |
|--------|-----------|-------|
| `Bswap16` | 16-bit byte swap | 1 (flags) |
| `Bswap32` | 32-bit byte swap | 1 (flags) |
| `Bswap64` | 64-bit byte swap | 1 (flags) |

All 1 oarg, 1 iarg, INT.

### 2.8 Bit Count (3)

| Opcode | Semantics | oargs | iargs |
|--------|-----------|-------|-------|
| `Clz` | count leading zeros, `d = clz(a) ?: b` | 1 | 2 |
| `Ctz` | count trailing zeros, `d = ctz(a) ?: b` | 1 | 2 |
| `CtPop` | population count | 1 | 1 |

The second input of `Clz`/`Ctz` is the fallback value (used when a==0).

### 2.9 Type Conversion (4)

| Opcode | Semantics | Fixed Type |
|--------|-----------|------------|
| `ExtI32I64` | sign-extend i32 → i64 | I64 |
| `ExtUI32I64` | zero-extend i32 → i64 | I64 |
| `ExtrlI64I32` | truncate i64 → i32 (low) | I32 |
| `ExtrhI64I32` | extract i64 → i32 (high) | I32 |

These ops are not type-polymorphic -- they have fixed input/output types and are not marked `INT`.

### 2.10 Host Memory Access (11)

Used for direct access to CPUState fields (via env pointer + offset).

**Loads** (1 oarg, 1 iarg, 1 carg=offset):

| Opcode | Semantics |
|--------|-----------|
| `Ld8U` | `d = *(u8*)(base + ofs)` |
| `Ld8S` | `d = *(i8*)(base + ofs)` |
| `Ld16U` | `d = *(u16*)(base + ofs)` |
| `Ld16S` | `d = *(i16*)(base + ofs)` |
| `Ld32U` | `d = *(u32*)(base + ofs)` |
| `Ld32S` | `d = *(i32*)(base + ofs)` |
| `Ld` | `d = *(native*)(base + ofs)` |

**Stores** (0 oargs, 2 iargs, 1 carg=offset):

| Opcode | Semantics |
|--------|-----------|
| `St8` | `*(u8*)(base + ofs) = src` |
| `St16` | `*(u16*)(base + ofs) = src` |
| `St32` | `*(u32*)(base + ofs) = src` |
| `St` | `*(native*)(base + ofs) = src` |

### 2.11 Guest Memory Access (4)

Access guest address space through the software TLB. Marked `CALL_CLOBBER | SIDE_EFFECTS | INT`.

| Opcode | Semantics | oargs | iargs | cargs |
|--------|-----------|-------|-------|-------|
| `QemuLd` | guest memory load | 1 | 1 | 1 (memop) |
| `QemuSt` | guest memory store | 0 | 2 | 1 (memop) |
| `QemuLd2` | 128-bit guest load (dual register) | 2 | 1 | 1 (memop) |
| `QemuSt2` | 128-bit guest store (dual register) | 0 | 3 | 1 (memop) |

### 2.12 Control Flow (7)

| Opcode | Semantics | oargs | iargs | cargs | Flags |
|--------|-----------|-------|-------|-------|-------|
| `Br` | unconditional jump to label | 0 | 0 | 1 (label) | BB_END, NP |
| `BrCond` | conditional jump | 0 | 2 | 2 (cond, label) | BB_END, COND_BRANCH, INT |
| `SetLabel` | define label position | 0 | 0 | 1 (label) | BB_END, NP |
| `GotoTb` | direct jump to another TB | 0 | 0 | 1 (tb_idx) | BB_EXIT, BB_END, NP |
| `ExitTb` | return to execution loop | 0 | 0 | 1 (val) | BB_EXIT, BB_END, NP |
| `GotoPtr` | indirect jump via register | 0 | 1 | 0 | BB_EXIT, BB_END |
| `Mb` | memory barrier | 0 | 0 | 1 (bar_type) | NP |

#### 2.12.1 `ExitTb` Convention Under Multi-Threaded vCPU

The return value of `ExitTb` indicates not only the "exit reason" but also participates in the execution loop's chaining protocol:

- `TB_EXIT_IDX0` / `TB_EXIT_IDX1`: correspond to `goto_tb` slots 0/1, recognized by the execution loop to trigger direct TB chain patching;
- `TB_EXIT_NOCHAIN`: used for indirect jump paths, the execution loop re-looks up a TB based on the current PC/flags and utilizes `exit_target` as a single-entry cache;
- `>= TB_EXIT_MAX`: real exceptions/system exits (e.g., `EXCP_ECALL`, `EXCP_EBREAK`, `EXCP_UNDEF`), returning directly to the upper layer.

To identify the "actual source TB" after direct chaining, core provides `encode_tb_exit` / `decode_tb_exit`: the low bits store the exit code, and the high bits carry the source TB index tag.

### 2.13 Miscellaneous (5)

| Opcode | Semantics | Flags |
|--------|-----------|-------|
| `Call` | call helper function | CC, NP |
| `PluginCb` | plugin callback | NP |
| `PluginMemCb` | plugin memory callback | NP |
| `Nop` | no operation | NP |
| `Discard` | discard temp | NP |
| `InsnStart` | guest instruction boundary marker | NP |

### 2.14 32-Bit Host Compatibility (2)

| Opcode | Semantics | Fixed Type |
|--------|-----------|------------|
| `BrCond2I32` | 64-bit conditional branch (32-bit host, register pair) | I32 |
| `SetCond2I32` | 64-bit conditional set (32-bit host) | I32 |

### 2.15 Vector Operations (57)

All vector ops are marked `VECTOR`, grouped by subcategory:

**Data Movement** (6): `MovVec`, `DupVec`, `Dup2Vec`, `LdVec`, `StVec`, `DupmVec`

**Arithmetic** (12): `AddVec`, `SubVec`, `MulVec`, `NegVec`, `AbsVec`,
`SsaddVec`, `UsaddVec`, `SssubVec`, `UssubVec`, `SminVec`, `UminVec`,
`SmaxVec`, `UmaxVec`

**Logic** (9): `AndVec`, `OrVec`, `XorVec`, `AndcVec`, `OrcVec`,
`NandVec`, `NorVec`, `EqvVec`, `NotVec`

**Shift -- Immediate** (4): `ShliVec`, `ShriVec`, `SariVec`, `RotliVec`
(1 oarg, 1 iarg, 1 carg=imm)

**Shift -- Scalar** (4): `ShlsVec`, `ShrsVec`, `SarsVec`, `RotlsVec`
(1 oarg, 2 iargs)

**Shift -- Vector** (5): `ShlvVec`, `ShrvVec`, `SarvVec`, `RotlvVec`, `RotrvVec`
(1 oarg, 2 iargs)

**Compare/Select** (3):
- `CmpVec`: 1 oarg, 2 iargs, 1 carg (cond)
- `BitselVec`: 1 oarg, 3 iargs -- `d = (a & c) | (b & ~c)`
- `CmpselVec`: 1 oarg, 4 iargs, 1 carg (cond) -- `d = (c1 cond c2) ? v1 : v2`

---

## 3. OpFlags Attribute Flags

```rust
pub struct OpFlags(u16);
```

| Flag | Value | Meaning |
|------|-------|---------|
| `BB_EXIT` | 0x01 | Exits the translation block |
| `BB_END` | 0x02 | Ends the basic block (next op starts a new BB) |
| `CALL_CLOBBER` | 0x04 | Clobbers caller-saved registers |
| `SIDE_EFFECTS` | 0x08 | Has side effects, cannot be eliminated by DCE |
| `INT` | 0x10 | Type-polymorphic (I32/I64) |
| `NOT_PRESENT` | 0x20 | Does not directly generate host code (handled specially by the allocator) |
| `VECTOR` | 0x40 | Vector operation |
| `COND_BRANCH` | 0x80 | Conditional branch |
| `CARRY_OUT` | 0x100 | Produces carry/borrow output |
| `CARRY_IN` | 0x200 | Consumes carry/borrow input |

Flags can be combined, e.g., `BrCond` = `BB_END | COND_BRANCH | INT`.

**Impact of flags on pipeline stages**:

- **Liveness analysis**: `BB_END` triggers global variable liveness marking; `SIDE_EFFECTS` prevents DCE
- **Register allocation**: `NOT_PRESENT` ops take a dedicated path instead of the generic `regalloc_op()`
- **Code generation**: `BB_EXIT` ops are handled directly by the backend (emit_exit_tb, etc.)

---

## 4. OpDef Static Table

```rust
pub struct OpDef {
    pub name: &'static str,  // name for debug/dump
    pub nb_oargs: u8,        // number of output arguments
    pub nb_iargs: u8,        // number of input arguments
    pub nb_cargs: u8,        // number of constant arguments
    pub flags: OpFlags,
}

pub static OPCODE_DEFS: [OpDef; Opcode::Count as usize] = [ ... ];
```

Accessed via the `Opcode::def()` method:

```rust
impl Opcode {
    pub fn def(self) -> &'static OpDef {
        &OPCODE_DEFS[self as usize]
    }
}
```

**Compile-time guarantee**: the array size equals `Opcode::Count as usize`; adding a new enum variant without a corresponding table entry causes a compile error.

---

## 5. Op Structure

```rust
pub struct Op {
    pub idx: OpIdx,              // index in the ops list
    pub opc: Opcode,             // opcode
    pub op_type: Type,           // actual type for polymorphic ops
    pub param1: u8,              // opcode-specific parameter (CALLI/TYPE/VECE)
    pub param2: u8,              // opcode-specific parameter (CALLO/FLAGS/VECE)
    pub life: LifeData,          // liveness analysis result
    pub output_pref: [RegSet; 2], // register allocation hints
    pub args: [TempIdx; 10],     // argument array
    pub nargs: u8,               // actual argument count
}
```

### 5.1 Argument Layout

The `args[]` array is arranged in a fixed order:

```
args[0 .. nb_oargs]                          → output arguments
args[nb_oargs .. nb_oargs+nb_iargs]          → input arguments
args[nb_oargs+nb_iargs .. nb_oargs+nb_iargs+nb_cargs] → constant arguments
```

Corresponding slices are obtained via `oargs()`/`iargs()`/`cargs()` methods, which slice based on `OpDef`'s argument counts -- a zero-cost abstraction.

**Example**: `BrCond` (0 oargs, 2 iargs, 2 cargs)

```
args[0] = a        (input: left comparison operand)
args[1] = b        (input: right comparison operand)
args[2] = cond     (const: condition code, encoded as TempIdx)
args[3] = label_id (const: target label, encoded as TempIdx)
```

### 5.2 Constant Argument Encoding

Constant arguments (condition codes, offsets, label IDs, etc.) are encoded as `TempIdx(raw_value as u32)` and stored in `args[]`, consistent with QEMU conventions. In the IR Builder, the helper function `carg()` performs the conversion:

```rust
fn carg(val: u32) -> TempIdx { TempIdx(val) }
```

### 5.3 LifeData

```rust
pub struct LifeData(pub u32);  // 2 bit per arg
```

Each argument occupies 2 bits:
- bit `n*2`: dead -- the argument is no longer used after this op
- bit `n*2+1`: sync -- the argument (global variable) needs to be synced back to memory

Populated by liveness analysis (`liveness.rs`) and consumed by the register allocator.

---

## 6. IR Builder API

`gen_*` methods on `impl Context` convert high-level operations into `Op` instances and append them to the ops list. Internally, helper methods like `emit_binary()`/`emit_unary()` provide uniform construction.

### 6.1 Binary ALU (1 oarg, 2 iargs)

Signature: `gen_xxx(&mut self, ty: Type, d: TempIdx, a: TempIdx, b: TempIdx) -> TempIdx`

`gen_add`, `gen_sub`, `gen_mul`, `gen_and`, `gen_or`, `gen_xor`,
`gen_shl`, `gen_shr`, `gen_sar`, `gen_rotl`, `gen_rotr`,
`gen_andc`, `gen_orc`, `gen_eqv`, `gen_nand`, `gen_nor`,
`gen_divs`, `gen_divu`, `gen_rems`, `gen_remu`,
`gen_mulsh`, `gen_muluh`,
`gen_clz`, `gen_ctz`

### 6.2 Unary (1 oarg, 1 iarg)

Signature: `gen_xxx(&mut self, ty: Type, d: TempIdx, s: TempIdx) -> TempIdx`

`gen_neg`, `gen_not`, `gen_mov`, `gen_ctpop`

### 6.3 Type Conversion (Fixed Types)

Signature: `gen_xxx(&mut self, d: TempIdx, s: TempIdx) -> TempIdx`

| Method | Semantics |
|--------|-----------|
| `gen_ext_i32_i64` | sign-extend i32 → i64 |
| `gen_ext_u32_i64` | zero-extend i32 → i64 |
| `gen_extrl_i64_i32` | truncate i64 → i32 (low) |
| `gen_extrh_i64_i32` | extract i64 → i32 (high) |

### 6.4 Conditional Operations

| Method | Signature |
|--------|-----------|
| `gen_setcond` | `(ty, d, a, b, cond) → d` |
| `gen_negsetcond` | `(ty, d, a, b, cond) → d` |
| `gen_movcond` | `(ty, d, c1, c2, v1, v2, cond) → d` |

### 6.5 Bit Field Operations

| Method | Signature |
|--------|-----------|
| `gen_extract` | `(ty, d, src, ofs, len) → d` |
| `gen_sextract` | `(ty, d, src, ofs, len) → d` |
| `gen_deposit` | `(ty, d, a, b, ofs, len) → d` |
| `gen_extract2` | `(ty, d, al, ah, ofs) → d` |

### 6.6 Byte Swap

Signature: `gen_bswapN(&mut self, ty: Type, d: TempIdx, src: TempIdx, flags: u32) -> TempIdx`

`gen_bswap16`, `gen_bswap32`, `gen_bswap64`

### 6.7 Double-Width Operations

| Method | Signature |
|--------|-----------|
| `gen_divs2` | `(ty, dl, dh, al, ah, b)` |
| `gen_divu2` | `(ty, dl, dh, al, ah, b)` |
| `gen_muls2` | `(ty, dl, dh, a, b)` |
| `gen_mulu2` | `(ty, dl, dh, a, b)` |

### 6.8 Carry Arithmetic

Same signature as binary ALU: `gen_xxx(&mut self, ty, d, a, b) -> TempIdx`

`gen_addco`, `gen_addci`, `gen_addcio`, `gen_addc1o`,
`gen_subbo`, `gen_subbi`, `gen_subbio`, `gen_subb1o`

### 6.9 Host Memory Access

**Loads**: `gen_ld(&mut self, ty, dst, base, offset) -> TempIdx`
and `gen_ld8u`, `gen_ld8s`, `gen_ld16u`, `gen_ld16s`, `gen_ld32u`, `gen_ld32s`

**Stores**: `gen_st(&mut self, ty, src, base, offset)`
and `gen_st8`, `gen_st16`, `gen_st32`

### 6.10 Guest Memory Access

| Method | Signature |
|--------|-----------|
| `gen_qemu_ld` | `(ty, dst, addr, memop) → dst` |
| `gen_qemu_st` | `(ty, val, addr, memop)` |
| `gen_qemu_ld2` | `(ty, dl, dh, addr, memop)` |
| `gen_qemu_st2` | `(ty, vl, vh, addr, memop)` |

### 6.11 Control Flow

| Method | Signature |
|--------|-----------|
| `gen_br` | `(label_id)` |
| `gen_brcond` | `(ty, a, b, cond, label_id)` |
| `gen_set_label` | `(label_id)` |
| `gen_goto_tb` | `(tb_idx)` |
| `gen_exit_tb` | `(val)` |
| `gen_goto_ptr` | `(ptr)` |
| `gen_mb` | `(bar_type)` |
| `gen_insn_start` | `(pc)` -- encoded as 2 cargs (lo, hi) |
| `gen_discard` | `(ty, t)` |

### 6.12 32-Bit Host Compatibility

| Method | Signature |
|--------|-----------|
| `gen_brcond2_i32` | `(al, ah, bl, bh, cond, label_id)` |
| `gen_setcond2_i32` | `(d, al, ah, bl, bh, cond) → d` |

### 6.13 Vector Operations

**Data Movement**: `gen_dup_vec`, `gen_dup2_vec`, `gen_ld_vec`, `gen_st_vec`, `gen_dupm_vec`

**Arithmetic**: `gen_add_vec`, `gen_sub_vec`, `gen_mul_vec`, `gen_neg_vec`, `gen_abs_vec`,
`gen_ssadd_vec`, `gen_usadd_vec`, `gen_sssub_vec`, `gen_ussub_vec`,
`gen_smin_vec`, `gen_umin_vec`, `gen_smax_vec`, `gen_umax_vec`

**Logic**: `gen_and_vec`, `gen_or_vec`, `gen_xor_vec`, `gen_andc_vec`, `gen_orc_vec`,
`gen_nand_vec`, `gen_nor_vec`, `gen_eqv_vec`, `gen_not_vec`

**Shift (Immediate)**: `gen_shli_vec`, `gen_shri_vec`, `gen_sari_vec`, `gen_rotli_vec`

**Shift (Scalar)**: `gen_shls_vec`, `gen_shrs_vec`, `gen_sars_vec`, `gen_rotls_vec`

**Shift (Vector)**: `gen_shlv_vec`, `gen_shrv_vec`, `gen_sarv_vec`, `gen_rotlv_vec`, `gen_rotrv_vec`

**Compare/Select**: `gen_cmp_vec`, `gen_bitsel_vec`, `gen_cmpsel_vec`

---

## 7. Comparison with QEMU

| Aspect | QEMU | machina |
|--------|------|---------|
| Opcode design | Type-split (`add_i32`/`add_i64`) | Unified polymorphism (`Add` + `op_type`) |
| Opcode definition | `DEF()` macros + `tcg-opc.h` | `enum Opcode` + `OPCODE_DEFS` array |
| Op argument storage | Linked list + dynamic allocation | Fixed array `[TempIdx; 10]` |
| Constant arguments | Encoded as `TCGArg` | Encoded as `TempIdx(raw_value)` |
| Flag system | `TCG_OPF_*` macros | `OpFlags(u16)` bitfield |
| Compile-time safety | None (runtime asserts) | Array size = `Count`, compile-time verification |
| Vector ops | Separate `_vec` suffix opcodes | Also separate, marked `VECTOR` |

---

## 8. QEMU Reference Mapping

| QEMU | machina | File |
|------|---------|------|
| `TCGOpcode` | `enum Opcode` | `core/src/opcode.rs` |
| `TCGOpDef` | `struct OpDef` | `core/src/opcode.rs` |
| `TCG_OPF_*` | `struct OpFlags` | `core/src/opcode.rs` |
| `TCGOp` | `struct Op` | `core/src/op.rs` |
| `TCGLifeData` | `struct LifeData` | `core/src/op.rs` |
| `tcg_gen_op*` | `Context::gen_*` | `core/src/ir_builder.rs` |
