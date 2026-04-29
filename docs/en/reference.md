# Machina Reference

> Target audience: developers working on machina internals.

## Table of Contents

- [Part 1: IR Opcode Reference](#part-1-ir-opcode-reference)
- [Part 2: x86-64 Backend Reference](#part-2-x86-64-backend-reference)
- [Part 3: Device Model Reference](#part-3-device-model-reference)
- [Part 4: Performance Analysis](#part-4-performance-analysis)
- [Part 5: Test Architecture](#part-5-test-architecture)

---

## Part 1: IR Opcode Reference

This document describes the complete design of machina's intermediate
representation (IR) operations, covering the opcode system, type system,
Op structure, argument encoding conventions, and the IR Builder API.

Source locations: `core/src/opcode.rs`, `core/src/op.rs`,
`core/src/ir_builder.rs`, `core/src/types.rs`.

---

### 1. Design Principles

#### 1.1 Unified Polymorphism vs Type-Split

In QEMU's original design, `add_i32` and `add_i64` are distinct opcodes
(type-split). machina uses a unified `Add`, with the actual type carried
by the `Op::op_type` field (type polymorphism).

**Advantages**:

- Reduces opcode count by ~40%
- The optimizer uses unified logic without needing
  `match (Add32, Add64) => ...`
- The backend selects 32/64-bit instruction encoding via `op.op_type`,
  resulting in cleaner logic
- `OpFlags::INT` marks which opcodes are polymorphic; non-polymorphic
  ones (e.g., `ExtI32I64`) have fixed types

#### 1.2 Fixed-Size Argument Array

`Op::args` uses a `[TempIdx; 10]` fixed array instead of `Vec`,
avoiding heap allocation. Each TB may contain hundreds of Ops; fixed
arrays eliminate significant allocator pressure.

#### 1.3 Compile-Time Safety

The `OPCODE_DEFS` table size is `Opcode::Count as usize`. Forgetting
to add a table entry when adding a new opcode causes a compile error,
fundamentally preventing the table and enum from going out of sync.

---

### 2. Opcode Enum

```rust
#[repr(u8)]
pub enum Opcode { Mov = 0, ..., Count }
```

A total of 158 valid opcodes + 1 sentinel (`Count`), divided into
13 categories:

#### 2.1 Data Movement (4)

| Opcode | Semantics | oargs | iargs | cargs | Flags |
|--------|-----------|-------|-------|-------|-------|
| `Mov` | `d = s` | 1 | 1 | 0 | INT, NP |
| `SetCond` | `d = (a cond b) ? 1 : 0` | 1 | 2 | 1 | INT |
| `NegSetCond` | `d = (a cond b) ? -1 : 0` | 1 | 2 | 1 | INT |
| `MovCond` | `d = (c1 cond c2) ? v1 : v2` | 1 | 4 | 1 | INT |

#### 2.2 Arithmetic (12)

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

#### 2.3 Carry/Borrow Arithmetic (8)

Implicit carry/borrow flags declare dependencies through
`CARRY_OUT`/`CARRY_IN` flags.

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

#### 2.4 Logic (9)

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

#### 2.5 Shift/Rotate (5)

| Opcode | Semantics |
|--------|-----------|
| `Shl` | `d = a << b` |
| `Shr` | `d = a >> b` (logical) |
| `Sar` | `d = a >> b` (arithmetic) |
| `RotL` | `d = a rotl b` |
| `RotR` | `d = a rotr b` |

All 1 oarg, 2 iargs, 0 cargs, INT.

#### 2.6 Bit Field Operations (4)

| Opcode | Semantics | oargs | iargs | cargs |
|--------|-----------|-------|-------|-------|
| `Extract` | `d = (src >> ofs) & mask(len)` | 1 | 1 | 2 (ofs, len) |
| `SExtract` | Same as above, with sign extension | 1 | 1 | 2 (ofs, len) |
| `Deposit` | `d = (a & ~mask) \| ((b << ofs) & mask)` | 1 | 2 | 2 (ofs, len) |
| `Extract2` | `d = (al:ah >> ofs)[N-1:0]` | 1 | 2 | 1 (ofs) |

#### 2.7 Byte Swap (3)

| Opcode | Semantics | cargs |
|--------|-----------|-------|
| `Bswap16` | 16-bit byte swap | 1 (flags) |
| `Bswap32` | 32-bit byte swap | 1 (flags) |
| `Bswap64` | 64-bit byte swap | 1 (flags) |

All 1 oarg, 1 iarg, INT.

#### 2.8 Bit Count (3)

| Opcode | Semantics | oargs | iargs |
|--------|-----------|-------|-------|
| `Clz` | count leading zeros, `d = clz(a) ?: b` | 1 | 2 |
| `Ctz` | count trailing zeros, `d = ctz(a) ?: b` | 1 | 2 |
| `CtPop` | population count | 1 | 1 |

The second input of `Clz`/`Ctz` is the fallback value (used when
a==0).

#### 2.9 Type Conversion (4)

| Opcode | Semantics | Fixed Type |
|--------|-----------|------------|
| `ExtI32I64` | sign-extend i32 -> i64 | I64 |
| `ExtUI32I64` | zero-extend i32 -> i64 | I64 |
| `ExtrlI64I32` | truncate i64 -> i32 (low) | I32 |
| `ExtrhI64I32` | extract i64 -> i32 (high) | I32 |

These ops are not type-polymorphic -- they have fixed input/output
types and are not marked `INT`.

#### 2.10 Host Memory Access (11)

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

#### 2.11 Guest Memory Access (4)

Access guest address space through the software TLB. Marked
`CALL_CLOBBER | SIDE_EFFECTS | INT`.

| Opcode | Semantics | oargs | iargs | cargs |
|--------|-----------|-------|-------|-------|
| `QemuLd` | guest memory load | 1 | 1 | 1 (memop) |
| `QemuSt` | guest memory store | 0 | 2 | 1 (memop) |
| `QemuLd2` | 128-bit guest load (dual register) | 2 | 1 | 1 (memop) |
| `QemuSt2` | 128-bit guest store (dual register) | 0 | 3 | 1 (memop) |

#### 2.12 Control Flow (7)

| Opcode | Semantics | oargs | iargs | cargs | Flags |
|--------|-----------|-------|-------|-------|-------|
| `Br` | unconditional jump to label | 0 | 0 | 1 (label) | BB_END, NP |
| `BrCond` | conditional jump | 0 | 2 | 2 (cond, label) | BB_END, COND_BRANCH, INT |
| `SetLabel` | define label position | 0 | 0 | 1 (label) | BB_END, NP |
| `GotoTb` | direct jump to another TB | 0 | 0 | 1 (tb_idx) | BB_EXIT, BB_END, NP |
| `ExitTb` | return to execution loop | 0 | 0 | 1 (val) | BB_EXIT, BB_END, NP |
| `GotoPtr` | indirect jump via register | 0 | 1 | 0 | BB_EXIT, BB_END |
| `Mb` | memory barrier | 0 | 0 | 1 (bar_type) | NP |

##### 2.12.1 `ExitTb` Convention Under Multi-Threaded vCPU

The return value of `ExitTb` indicates not only the "exit reason" but
also participates in the execution loop's chaining protocol:

- `TB_EXIT_IDX0` / `TB_EXIT_IDX1`: correspond to `goto_tb` slots 0/1,
  recognized by the execution loop to trigger direct TB chain patching;
- `TB_EXIT_NOCHAIN`: used for indirect jump paths, the execution loop
  re-looks up a TB based on the current PC/flags and utilizes
  `exit_target` as a single-entry cache;
- `>= TB_EXIT_MAX`: real exceptions/system exits (e.g., `EXCP_ECALL`,
  `EXCP_EBREAK`, `EXCP_UNDEF`), returning directly to the upper layer.

To identify the "actual source TB" after direct chaining, core provides
`encode_tb_exit` / `decode_tb_exit`: the low bits store the exit code,
and the high bits carry the source TB index tag.

#### 2.13 Miscellaneous (5)

| Opcode | Semantics | Flags |
|--------|-----------|-------|
| `Call` | call helper function | CC, NP |
| `PluginCb` | plugin callback | NP |
| `PluginMemCb` | plugin memory callback | NP |
| `Nop` | no operation | NP |
| `Discard` | discard temp | NP |
| `InsnStart` | guest instruction boundary marker | NP |

#### 2.14 32-Bit Host Compatibility (2)

| Opcode | Semantics | Fixed Type |
|--------|-----------|------------|
| `BrCond2I32` | 64-bit conditional branch (32-bit host, register pair) | I32 |
| `SetCond2I32` | 64-bit conditional set (32-bit host) | I32 |

#### 2.15 Vector Operations (57)

All vector ops are marked `VECTOR`, grouped by subcategory:

**Data Movement** (6): `MovVec`, `DupVec`, `Dup2Vec`, `LdVec`,
`StVec`, `DupmVec`

**Arithmetic** (12): `AddVec`, `SubVec`, `MulVec`, `NegVec`,
`AbsVec`, `SsaddVec`, `UsaddVec`, `SssubVec`, `UssubVec`,
`SminVec`, `UminVec`, `SmaxVec`, `UmaxVec`

**Logic** (9): `AndVec`, `OrVec`, `XorVec`, `AndcVec`, `OrcVec`,
`NandVec`, `NorVec`, `EqvVec`, `NotVec`

**Shift -- Immediate** (4): `ShliVec`, `ShriVec`, `SariVec`,
`RotliVec` (1 oarg, 1 iarg, 1 carg=imm)

**Shift -- Scalar** (4): `ShlsVec`, `ShrsVec`, `SarsVec`,
`RotlsVec` (1 oarg, 2 iargs)

**Shift -- Vector** (5): `ShlvVec`, `ShrvVec`, `SarvVec`,
`RotlvVec`, `RotrvVec` (1 oarg, 2 iargs)

**Compare/Select** (3):
- `CmpVec`: 1 oarg, 2 iargs, 1 carg (cond)
- `BitselVec`: 1 oarg, 3 iargs -- `d = (a & c) | (b & ~c)`
- `CmpselVec`: 1 oarg, 4 iargs, 1 carg (cond) --
  `d = (c1 cond c2) ? v1 : v2`

---

### 3. OpFlags Attribute Flags

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

- **Liveness analysis**: `BB_END` triggers global variable liveness
  marking; `SIDE_EFFECTS` prevents DCE
- **Register allocation**: `NOT_PRESENT` ops take a dedicated path
  instead of the generic `regalloc_op()`
- **Code generation**: `BB_EXIT` ops are handled directly by the
  backend (emit_exit_tb, etc.)

---

### 4. OpDef Static Table

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

**Compile-time guarantee**: the array size equals
`Opcode::Count as usize`; adding a new enum variant without a
corresponding table entry causes a compile error.

---

### 5. Op Structure

```rust
pub struct Op {
    pub idx: OpIdx,              // index in the ops list
    pub opc: Opcode,             // opcode
    pub op_type: Type,           // actual type for polymorphic ops
    pub param1: u8,              // opcode-specific parameter
    pub param2: u8,              // opcode-specific parameter
    pub life: LifeData,          // liveness analysis result
    pub output_pref: [RegSet; 2], // register allocation hints
    pub args: [TempIdx; 10],     // argument array
    pub nargs: u8,               // actual argument count
}
```

#### 5.1 Argument Layout

The `args[]` array is arranged in a fixed order:

```
args[0 .. nb_oargs]                          -> output arguments
args[nb_oargs .. nb_oargs+nb_iargs]          -> input arguments
args[nb_oargs+nb_iargs .. nb_oargs+nb_iargs+nb_cargs]
                                             -> constant arguments
```

Corresponding slices are obtained via `oargs()`/`iargs()`/`cargs()`
methods, which slice based on `OpDef`'s argument counts -- a zero-cost
abstraction.

**Example**: `BrCond` (0 oargs, 2 iargs, 2 cargs)

```
args[0] = a        (input: left comparison operand)
args[1] = b        (input: right comparison operand)
args[2] = cond     (const: condition code, encoded as TempIdx)
args[3] = label_id (const: target label, encoded as TempIdx)
```

#### 5.2 Constant Argument Encoding

Constant arguments (condition codes, offsets, label IDs, etc.) are
encoded as `TempIdx(raw_value as u32)` and stored in `args[]`,
consistent with QEMU conventions. In the IR Builder, the helper
function `carg()` performs the conversion:

```rust
fn carg(val: u32) -> TempIdx { TempIdx(val) }
```

#### 5.3 LifeData

```rust
pub struct LifeData(pub u32);  // 2 bit per arg
```

Each argument occupies 2 bits:
- bit `n*2`: dead -- the argument is no longer used after this op
- bit `n*2+1`: sync -- the argument (global variable) needs to be
  synced back to memory

Populated by liveness analysis (`liveness.rs`) and consumed by the
register allocator.

---

### 6. IR Builder API

`gen_*` methods on `impl Context` convert high-level operations into
`Op` instances and append them to the ops list. Internally, helper
methods like `emit_binary()`/`emit_unary()` provide uniform
construction.

#### 6.1 Binary ALU (1 oarg, 2 iargs)

Signature:
`gen_xxx(&mut self, ty: Type, d: TempIdx, a: TempIdx, b: TempIdx)`
`-> TempIdx`

`gen_add`, `gen_sub`, `gen_mul`, `gen_and`, `gen_or`, `gen_xor`,
`gen_shl`, `gen_shr`, `gen_sar`, `gen_rotl`, `gen_rotr`,
`gen_andc`, `gen_orc`, `gen_eqv`, `gen_nand`, `gen_nor`,
`gen_divs`, `gen_divu`, `gen_rems`, `gen_remu`,
`gen_mulsh`, `gen_muluh`,
`gen_clz`, `gen_ctz`

#### 6.2 Unary (1 oarg, 1 iarg)

Signature:
`gen_xxx(&mut self, ty: Type, d: TempIdx, s: TempIdx) -> TempIdx`

`gen_neg`, `gen_not`, `gen_mov`, `gen_ctpop`

#### 6.3 Type Conversion (Fixed Types)

Signature:
`gen_xxx(&mut self, d: TempIdx, s: TempIdx) -> TempIdx`

| Method | Semantics |
|--------|-----------|
| `gen_ext_i32_i64` | sign-extend i32 -> i64 |
| `gen_ext_u32_i64` | zero-extend i32 -> i64 |
| `gen_extrl_i64_i32` | truncate i64 -> i32 (low) |
| `gen_extrh_i64_i32` | extract i64 -> i32 (high) |

#### 6.4 Conditional Operations

| Method | Signature |
|--------|-----------|
| `gen_setcond` | `(ty, d, a, b, cond) -> d` |
| `gen_negsetcond` | `(ty, d, a, b, cond) -> d` |
| `gen_movcond` | `(ty, d, c1, c2, v1, v2, cond) -> d` |

#### 6.5 Bit Field Operations

| Method | Signature |
|--------|-----------|
| `gen_extract` | `(ty, d, src, ofs, len) -> d` |
| `gen_sextract` | `(ty, d, src, ofs, len) -> d` |
| `gen_deposit` | `(ty, d, a, b, ofs, len) -> d` |
| `gen_extract2` | `(ty, d, al, ah, ofs) -> d` |

#### 6.6 Byte Swap

Signature:
`gen_bswapN(&mut self, ty: Type, d: TempIdx, src: TempIdx,`
`flags: u32) -> TempIdx`

`gen_bswap16`, `gen_bswap32`, `gen_bswap64`

#### 6.7 Double-Width Operations

| Method | Signature |
|--------|-----------|
| `gen_divs2` | `(ty, dl, dh, al, ah, b)` |
| `gen_divu2` | `(ty, dl, dh, al, ah, b)` |
| `gen_muls2` | `(ty, dl, dh, a, b)` |
| `gen_mulu2` | `(ty, dl, dh, a, b)` |

#### 6.8 Carry Arithmetic

Same signature as binary ALU:
`gen_xxx(&mut self, ty, d, a, b) -> TempIdx`

`gen_addco`, `gen_addci`, `gen_addcio`, `gen_addc1o`,
`gen_subbo`, `gen_subbi`, `gen_subbio`, `gen_subb1o`

#### 6.9 Host Memory Access

**Loads**: `gen_ld(&mut self, ty, dst, base, offset) -> TempIdx`
and `gen_ld8u`, `gen_ld8s`, `gen_ld16u`, `gen_ld16s`, `gen_ld32u`,
`gen_ld32s`

**Stores**: `gen_st(&mut self, ty, src, base, offset)`
and `gen_st8`, `gen_st16`, `gen_st32`

#### 6.10 Guest Memory Access

| Method | Signature |
|--------|-----------|
| `gen_qemu_ld` | `(ty, dst, addr, memop) -> dst` |
| `gen_qemu_st` | `(ty, val, addr, memop)` |
| `gen_qemu_ld2` | `(ty, dl, dh, addr, memop)` |
| `gen_qemu_st2` | `(ty, vl, vh, addr, memop)` |

#### 6.11 Control Flow

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

#### 6.12 32-Bit Host Compatibility

| Method | Signature |
|--------|-----------|
| `gen_brcond2_i32` | `(al, ah, bl, bh, cond, label_id)` |
| `gen_setcond2_i32` | `(d, al, ah, bl, bh, cond) -> d` |

#### 6.13 Vector Operations

**Data Movement**: `gen_dup_vec`, `gen_dup2_vec`, `gen_ld_vec`,
`gen_st_vec`, `gen_dupm_vec`

**Arithmetic**: `gen_add_vec`, `gen_sub_vec`, `gen_mul_vec`,
`gen_neg_vec`, `gen_abs_vec`, `gen_ssadd_vec`, `gen_usadd_vec`,
`gen_sssub_vec`, `gen_ussub_vec`, `gen_smin_vec`, `gen_umin_vec`,
`gen_smax_vec`, `gen_umax_vec`

**Logic**: `gen_and_vec`, `gen_or_vec`, `gen_xor_vec`,
`gen_andc_vec`, `gen_orc_vec`, `gen_nand_vec`, `gen_nor_vec`,
`gen_eqv_vec`, `gen_not_vec`

**Shift (Immediate)**: `gen_shli_vec`, `gen_shri_vec`,
`gen_sari_vec`, `gen_rotli_vec`

**Shift (Scalar)**: `gen_shls_vec`, `gen_shrs_vec`,
`gen_sars_vec`, `gen_rotls_vec`

**Shift (Vector)**: `gen_shlv_vec`, `gen_shrv_vec`,
`gen_sarv_vec`, `gen_rotlv_vec`, `gen_rotrv_vec`

**Compare/Select**: `gen_cmp_vec`, `gen_bitsel_vec`,
`gen_cmpsel_vec`

---

### 7. Comparison with QEMU

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

### 8. QEMU Reference Mapping

| QEMU | machina | File |
|------|---------|------|
| `TCGOpcode` | `enum Opcode` | `core/src/opcode.rs` |
| `TCGOpDef` | `struct OpDef` | `core/src/opcode.rs` |
| `TCG_OPF_*` | `struct OpFlags` | `core/src/opcode.rs` |
| `TCGOp` | `struct Op` | `core/src/op.rs` |
| `TCGLifeData` | `struct LifeData` | `core/src/op.rs` |
| `tcg_gen_op*` | `Context::gen_*` | `core/src/ir_builder.rs` |

---

## Part 2: x86-64 Backend Reference

### 1. Overview

`accel/src/x86_64/emitter.rs` implements a complete GPR instruction
encoder for the x86-64 host architecture, referencing QEMU's
`tcg/i386/tcg-target.c.inc`. It uses a layered encoding architecture:

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
Codegen Dispatch (tcg_out_op: IR Opcode --> Instruction Emitter
                  Combinations)
        |
        v
X86_64CodeGen (prologue / epilogue / exit_tb / goto_tb)
```

### 2. Encoding Infrastructure

#### 2.1 Prefix Flags (P_*)

Opcode constants use the `u32` type, with high bits encoding prefix
information:

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

#### 2.2 Opcode Constants (OPC_*)

Constant naming follows QEMU's `tcg-target.c.inc` style (using
`#![allow(non_upper_case_globals)]`):

```rust
pub const OPC_ARITH_EvIb: u32 = 0x83;
pub const OPC_MOVL_GvEv: u32 = 0x8B;
pub const OPC_JCC_long: u32 = 0x80 | P_EXT;
pub const OPC_BSF: u32 = 0xBC | P_EXT;
pub const OPC_LZCNT: u32 = 0xBD | P_EXT | P_SIMDF3;
```

#### 2.3 Core Encoding Functions

| Function | Purpose |
|----------|---------|
| `emit_opc(buf, opc, r, rm)` | Emit REX prefix + escape bytes + opcode |
| `emit_modrm(buf, opc, r, rm)` | Register-register ModR/M (mod=11) |
| `emit_modrm_ext(buf, opc, ext, rm)` | /r extension for group opcodes |
| `emit_modrm_offset(buf, opc, r, base, offset)` | Memory [base+disp] |
| `emit_modrm_sib(buf, opc, r, base, index, shift, offset)` | SIB addressing |
| `emit_modrm_ext_offset(buf, opc, ext, base, offset)` | Group opcode + memory |

### 3. Instruction Categories

#### 3.1 Arithmetic Instructions

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

`ArithOp` enum values correspond to the x86 /r field: Add=0, Or=1,
Adc=2, Sbb=3, And=4, Sub=5, Xor=6, Cmp=7.

#### 3.2 Shift Instructions

| Function | Instruction | Description |
|----------|-------------|-------------|
| `emit_shift_ri(op, rexw, dst, imm)` | SHL/SHR/SAR/ROL/ROR | Immediate shift (imm=1 uses short encoding) |
| `emit_shift_cl(op, rexw, dst)` | Same | Shift by CL register |
| `emit_shld_ri(rexw, dst, src, imm)` | SHLD | Double-precision left shift |
| `emit_shrd_ri(rexw, dst, src, imm)` | SHRD | Double-precision right shift |

#### 3.3 Data Movement

| Function | Instruction | Description |
|----------|-------------|-------------|
| `emit_mov_rr(rexw, dst, src)` | MOV r, r | 32/64-bit register transfer |
| `emit_mov_ri(rexw, reg, val)` | MOV r, imm | Smart selection: xor(0) / mov r32(u32) / mov r64 sign-ext(i32) / movabs(i64) |
| `emit_movzx(opc, dst, src)` | MOVZBL/MOVZWL | Zero extension |
| `emit_movsx(opc, dst, src)` | MOVSBL/MOVSWL/MOVSLQ | Sign extension |
| `emit_bswap(rexw, reg)` | BSWAP | Byte swap |

#### 3.4 Memory Operations

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

#### 3.5 Multiply/Divide Instructions

| Function | Instruction | Description |
|----------|-------------|-------------|
| `emit_mul(rexw, reg)` | MUL | Unsigned multiply RDX:RAX = RAX * reg |
| `emit_imul1(rexw, reg)` | IMUL | Signed multiply (single operand) |
| `emit_imul_rr(rexw, dst, src)` | IMUL r, r | Two-operand multiply |
| `emit_imul_ri(rexw, dst, src, imm)` | IMUL r, r, imm | Three-operand multiply |
| `emit_div(rexw, reg)` | DIV | Unsigned divide |
| `emit_idiv(rexw, reg)` | IDIV | Signed divide |
| `emit_cdq()` | CDQ | Sign-extend EAX -> EDX:EAX |
| `emit_cqo()` | CQO | Sign-extend RAX -> RDX:RAX |

#### 3.6 Bit Operations

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

#### 3.7 Branch and Compare

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

#### 3.8 Miscellaneous

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

### 4. Memory Addressing Special Cases

x86-64 ModR/M encoding has two special registers that require extra
handling:

- **RSP/R12 (low3=4)**: When used as a base, a SIB byte is required
  (`0x24` = index=RSP/none, base=RSP)
- **RBP/R13 (low3=5)**: When used as a base with zero offset,
  `mod=01, disp8=0` must be used (because `mod=00, rm=5` is encoded
  as RIP-relative addressing)

`emit_modrm_offset` handles these special cases automatically.

### 5. Condition Code Mapping

The `X86Cond` enum maps TCG conditions to x86 JCC condition codes:

| TCG Cond | X86Cond | JCC Encoding |
|----------|---------|--------------|
| Eq / TstEq | Je | 0x4 |
| Ne / TstNe | Jne | 0x5 |
| Lt | Jl | 0xC |
| Ge | Jge | 0xD |
| Ltu | Jb | 0x2 |
| Geu | Jae | 0x3 |

`X86Cond::invert()` inverts conditions by flipping the low bit
(e.g., Je <-> Jne).

### 6. Constraint Table (`constraints.rs`)

`op_constraint()` returns a static `OpConstraint` for each opcode,
aligned with QEMU's `tcg_target_op_def()`
(`tcg/i386/tcg-target.c.inc`).

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

Where `R = ALLOCATABLE_REGS` (14 GPRs, excluding RSP and RBP),
`R_NO_RCX = R & ~{RCX}`,
`R_NO_RAX_RDX = R & ~{RAX, RDX}`.

The constraint guarantees allow codegen to assume:
- Destructive operations have `oregs[0] == iregs[0]` (no need for
  a preceding mov)
- Shifts have `iregs[1] == RCX` (no need for push/pop RCX juggling)
- Shift output/input0 are not in RCX (excluded by R_NO_RCX)
- The free input of MulS2/DivS2 is not in RAX/RDX (excluded by
  R_NO_RAX_RDX)
- SetCond output does not overlap with any input

### 7. Codegen Dispatch (`codegen.rs`)

`tcg_out_op` is the bridge between the register allocator and the
instruction encoder. It receives IR ops with allocated host registers
and translates them into one or more x86-64 instructions.

#### 7.1 HostCodeGen Register Allocator Primitives

| Method | Purpose |
|--------|---------|
| `tcg_out_mov(ty, dst, src)` | Register-to-register transfer |
| `tcg_out_movi(ty, dst, val)` | Load immediate into register |
| `tcg_out_ld(ty, dst, base, offset)` | Load from memory (global variable reload) |
| `tcg_out_st(ty, src, base, offset)` | Store to memory (global variable sync) |

#### 7.2 IR Opcode --> x86-64 Instruction Mapping

The constraint system guarantees that codegen receives registers
satisfying instruction requirements, so each opcode only needs to
emit the simplest instruction sequence:

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

#### 7.3 TstEq/TstNe Support for SetCond/BrCond

When the condition code is `TstEq` or `TstNe`, `test a,b` (bitwise
AND test) is used instead of `cmp a,b` (subtraction comparison).
This corresponds to the test-and-branch optimization added in
QEMU 7.x+.

### 8. QEMU Reference Cross-Reference

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

---

## Part 3: Device Model Reference

### 1. Scope

This document describes Machina's first-cut device model alignment
with QEMU's object/qdev/sysbus direction, using Machina-native
terminology: `MOM`, `mobject`, `mdev`, and `sysbus`.

This is a direct replacement of the previous thin qdev/sysbus
skeleton. There is no compatibility or migration layer beyond the
temporary qdev bridge already used inside the codebase.

The current first-cut MOM scope covers:

- the root object layer (`mobject`)
- the device layer (`mdev`)
- executable sysbus realization and unrealize
- a lightweight property surface
- migrated platform devices: UART, PLIC, ACLINT, and virtio-mmio

### 2. Layering

#### 2.1 `mobject`

`mobject` is the foundational ownership and identity layer.

- It lives in `machina-core`
- It gives managed objects a local ID and object path
- It enforces a strict parent/child tree
- It is the reason `Machine` now participates in the object tree

#### 2.2 `mdev`

`mdev` is the common device lifecycle layer on top of `mobject`.

- It lives in `machina-hw-core`
- It tracks `realize` / `unrealize`
- It rejects forbidden late structural mutation
- It carries the common error taxonomy for migrated devices

#### 2.3 `sysbus`

`sysbus` is an executable assembly layer, not metadata only.

- Devices must attach to a bus before realization
- Devices must register MMIO regions before realization
- Realization validates overlaps and maps regions into `AddressSpace`
- Unrealize removes realized mappings from `AddressSpace` and the
  bus record

#### 2.4 Properties

The first MOM increment uses a small typed property layer.

- Property schema is defined before realization
- Required/default handling is explicit
- Static-vs-dynamic mutability is explicit
- UART uses a standard `chardev` link property on this surface

### 3. Device Lifecycle

The migrated-device lifecycle is:

1. Create the device object
2. Attach to `sysbus`
3. Register MMIO and any device-specific runtime wiring inputs
4. Apply pre-realize properties
5. Realize onto `AddressSpace`
6. Reset runtime state without rebuilding topology
7. Unrealize by tearing down runtime state and removing realized
   mappings

The key rule is that structural topology is created once and then
preserved across reset. Reset must not rebuild hidden topology as
a side effect.

### 4. First-Cut Migrated Devices

#### 4.1 UART

- Owns a `SysBusDeviceState`
- Exposes `chardev` as a standard property
- Installs frontend runtime wiring during `realize`
- Removes runtime wiring and MMIO mapping during `unrealize`

#### 4.2 PLIC

- Owns a `SysBusDeviceState`
- Keeps context-output routing as device-specific runtime wiring
- Uses runtime reset without rebuilding sysbus topology

#### 4.3 ACLINT

- Owns a `SysBusDeviceState`
- Keeps MTI/MSI and WFI-waker wiring device-specific
- Cancels timer state on reset and unrealize without rebuilding
  topology

#### 4.4 virtio-mmio

- The MMIO transport is the MOM/sysbus device
- The block backend remains transport-local
- The transport owns guest-RAM access, MMIO state, and IRQ delivery

This keeps the transport/proxy boundary explicit and leaves room for
future backend relationships without conflating them with machine
assembly.

### 5. `RefMachine` Assembly Rule

`RefMachine` is the first machine that follows the MOM assembly rule
for the migrated set.

- UART, PLIC, ACLINT, and virtio-mmio are created as MOM-managed
  devices
- They are attached and realized through `sysbus`
- Their realized mappings are visible through `SysBus::mappings()`
- FDT node names and `reg` cells for the migrated set are derived
  from the realized sysbus mappings

For the migrated device set, realized `sysbus` mappings are the
machine-side topology source of truth.

### 6. Testing and Regression Guardrails

The shared `tests` crate verifies:

- object attachment and lifecycle sequencing
- MMIO visibility only after realization
- UART, PLIC, ACLINT, and virtio-mmio guest-visible behavior
- sysbus unrealize/unmap behavior
- machine-visible migrated owner sets
- source-level anti-regression checks against direct root MMIO wiring

### 7. Future Extension Points

The current design intentionally leaves explicit extension points for:

- PCI and non-sysbus transports
- hotplug-aware lifecycle extensions
- richer object/property introspection
- parent/child relationships between transport devices and backend
  devices

These are future directions, not v1 commitments.

---

## Part 4: Performance Analysis

This document summarizes machina JIT engine's unique performance
optimizations compared to QEMU TCG, and analyzes performance
characteristics in full-system mode.

### 1. Execution Loop Optimizations

#### 1.1 `next_tb_hint` -- Skipping TB Lookup

**File**: `accel/src/exec/exec_loop.rs:52-89`

When a TB exits via `goto_tb` chaining, machina stores the target TB
index in `next_tb_hint`. The next iteration directly reuses this
index, completely skipping the jump cache and global hash lookup.

| | machina | QEMU |
|---|--------|------|
| After chained exit | Directly reuse target TB | Still goes through the full `tb_lookup` path |
| Hot loop overhead | Near zero (index comparison) | jump cache hash + comparison |

QEMU's `last_tb` is only used to decide whether to patch a link, not
to skip lookup. In tight loops (e.g., the dhrystone main loop), the
hint hit rate is extremely high.

#### 1.2 `exit_target` Atomic Cache -- Indirect Jump Acceleration

**File**: `accel/src/exec/exec_loop.rs:96-116`, `core/src/tb.rs:55`

For `TB_EXIT_NOCHAIN` (indirect jumps, `jalr`, etc.), each TB
maintains an `AtomicUsize` single-entry cache that records the last
jump target TB.

```
indirect jump exit --> check exit_target cache
                       |-- hit and valid --> reuse directly,
                       |                    skip hash lookup
                       +-- miss --> normal tb_find, update cache
```

QEMU performs a full QHT lookup for all `TB_EXIT_NOCHAIN` exits,
without this caching layer. Combined, these two optimizations ensure
that global hash lookups are triggered almost exclusively during cold
start and TB invalidation in steady-state execution.

**Estimated contribution**: ~8-10%

### 2. Guest Memory Access Optimizations

#### 2.1 Direct guest_base Addressing (Early linux-user Optimization)

**File**: `accel/src/x86_64/codegen.rs:573-639`

> **Note**: The software-TLB-free direct addressing optimization
> described in this section was an early linux-user mode exclusive
> approach. Full-system mode uses Sv39 MMU page table translation +
> software TLB and no longer uses this path.

In the early linux-user mode, guest memory accesses directly generated
`[R14 + addr]` addressing (R14 = guest_base), with no TLB lookup and
no slow-path helper calls.

| | machina (direct addressing) | QEMU |
|---|--------|------|
| load/store generation | `mov reg, [R14+addr]` | Inline TLB fast path + slow path branch |
| Host instructions per access | 1-2 | 5-10 (TLB lookup + comparison + branch) |
| Slow path | None | Helper function call |

QEMU generates the full software TLB path even in linux-user mode,
because its `tcg_out_qemu_ld`/`tcg_out_qemu_st` do not differentiate
between system mode and user mode.

In full-system mode, machina uses Sv39 MMU page table translation with
a software TLB fast path, where memory access overhead is comparable
to QEMU and no longer benefits from direct addressing.

**Estimated contribution**: Only applicable to direct addressing
scenarios, ~8-10%

### 3. Data Structure Optimizations

#### 3.1 Vec-based IR Storage vs QEMU Linked List

**File**: `core/src/context.rs:18-73`

| | machina | QEMU |
|---|--------|------|
| Op storage | `Vec<Op>` contiguous memory | `QTAILQ` doubly linked list |
| Temp storage | `Vec<Temp>` contiguous memory | Array (fixed upper limit) |
| Traversal pattern | Sequential indexing, cache prefetch friendly | Pointer chasing, frequent cache misses |
| Pre-allocation | ops=512, temps=256, labels=32 | Dynamic malloc |

The optimizer traversal, liveness analysis, and register allocation
all require sequential scanning of all ops, where Vec's cache line
prefetch advantage is significant. Pre-allocated capacity avoids
reallocation during translation.

#### 3.2 HashMap Constant Deduplication vs Linear Scan

**File**: `core/src/context.rs:128-138`

machina uses a type-bucketed `HashMap<u64, TempIdx>` for constant
deduplication with O(1) lookup. QEMU's `tcg_constant_internal`
performs a linear scan over `nb_temps`, making constant lookup a
hidden cost in large TBs.

#### 3.3 `#[repr(u8)]` Compact Enums

**File**: `core/src/opcode.rs`

The `Opcode` enum is annotated with `#[repr(u8)]`, occupying 1 byte.
QEMU's `TCGOpcode` is an `int` (4 bytes). The `Op` struct is more
compact, fitting more ops per cache line.

**Estimated contribution**: ~3-5%

### 4. Runtime Concurrency Optimizations

#### 4.1 Lock-free TB Reads

**File**: `accel/src/exec/tb_store.rs:13-64`

TbStore leverages the append-only, never-delete property of TBs,
using `UnsafeCell<Vec<TB>>` + `AtomicUsize` length to implement
lock-free reads.

```
Write path (translation): translate_lock --> push TB
                           --> Release store len
Read path (execution):    Acquire load len --> index access
                           (lock-free)
```

QEMU's QHT uses an RCU mechanism, incurring additional grace period
and synchronize overhead. machina's approach is simpler, exploiting
the append-only invariant of TBs.

#### 4.2 RWX Code Buffer -- No mprotect Switching

**File**: `accel/src/code_buffer.rs:38-49`

machina directly mmaps RWX memory, requiring no mprotect switching
during TB link patching. QEMU, when split-wx mode is enabled (the
default on some distributions), needs an mprotect system call for
each patch.

#### 4.3 Simplified Hash Function

**File**: `core/src/tb.rs:106-109`

```rust
let h = pc.wrapping_mul(0x9e3779b97f4a7c15) ^ (flags as u64);
(h as usize) & (TB_HASH_SIZE - 1)
```

Golden ratio constant multiplication hash, with less computation
than QEMU's xxHash. Saving a few cycles per lookup on the TB lookup
hot path, the cumulative effect is considerable.

**Estimated contribution**: ~2-3%

### 5. Compilation Pipeline Optimizations

#### 5.1 Single-Pass IR Optimizer

**File**: `accel/src/optimize.rs`

| | machina | QEMU |
|---|--------|------|
| Passes | Single pass O(n) | Multiple pass scans |
| Constant folding | Full value-level | Bit-level (z_mask/o_mask/s_mask) |
| Copy propagation | Basic | Advanced |
| Algebraic simplification | Basic identities | Complex pattern matching |

machina's optimization depth is less than QEMU's, but translation
speed is faster. For large numbers of short TBs, the single-pass
design's compilation time advantage is significant.

#### 5.2 Rust Zero-Cost Abstractions

- **Monomorphization**: Frontend `BinOp` function pointers
  (`frontend/src/riscv/trans.rs:26`) are monomorphized and inlined
  by the compiler, eliminating indirect calls
- **Inline annotations**: `CodeBuffer`'s 14 `#[inline]` byte
  emission functions (`accel/src/code_buffer.rs`) are inlined at
  codegen call sites
- **Enum discriminants**: `#[repr(u8)]` generates compact jump
  tables

**Estimated contribution**: ~2-3%

### 6. Instruction Selection Optimizations

#### 6.1 LEA Three-Address Addition

**File**: `accel/src/x86_64/codegen.rs:136-147`

When the output register of `Add` differs from both inputs, LEA is
used for non-destructive three-address addition, avoiding an extra
MOV. QEMU also has this optimization.

#### 6.2 Unconditional BMI1 Instructions

**File**: `accel/src/x86_64/emitter.rs:57-61`

machina unconditionally uses ANDN/LZCNT/TZCNT/POPCNT. QEMU checks
CPU features at runtime before deciding whether to use them; the
detection itself has minor overhead, and the fallback paths are
longer.

#### 6.3 MOV Immediate Tiered Optimization

**File**: `accel/src/x86_64/emitter.rs:547-566`

```
val == 0        --> XOR reg, reg       (2 bytes, breaks dep chain)
val <= u32::MAX --> MOV r32, imm32     (5 bytes, zero-extends)
val fits i32    --> MOV r64, sign-ext   (7 bytes)
otherwise       --> MOV r64, imm64     (10 bytes)
```

### 7. Full-System Mode Performance Characteristics

Full-system mode introduces additional performance overhead. The main
contributing factors are:

#### 7.1 MMU Page Table Translation Overhead

Full-system mode uses Sv39 three-level page table translation. Each
guest memory access requires:

1. Software TLB fast path lookup (inline code, ~5-10 host
   instructions)
2. Page table walk on TLB miss (3-level lookup, one memory read per
   level)
3. Permission checks (read/write/execute, U/S mode, MXR/SUM bits)

TLB hit rate is the key performance metric for full-system mode.
During steady-state execution, TLB hit rate is typically >95%,
amortizing the page table walk overhead.

#### 7.2 MMIO Dispatch Overhead

Device MMIO accesses take a separate dispatch path, bypassing the
TLB fast path:

```
guest load/store --> TLB lookup
                      |-- normal memory --> fast path direct access
                      +-- MMIO region --> AddressSpace dispatch
                                          --> device read/write
                                              callback
```

MMIO dispatch involves address space tree lookup and indirect device
callback calls, with overhead 1-2 orders of magnitude higher than
normal memory access. Device-interaction-intensive workloads (e.g.,
heavy serial I/O) are significantly affected.

#### 7.3 Privilege Level Switching

Full-system mode must handle M/S/U privilege level switching,
interrupts, and exceptions, with each switch involving CSR updates
and TB invalidation. Frequent privilege level switches (e.g.,
high-frequency timer interrupts) reduce TB cache hit rates.

### 8. Performance Contribution Overview

| Optimization Category | Estimated Contribution | Key Technique |
|-----------------------|----------------------|---------------|
| Execution loop (hint + exit_target) | ~8-10% | Skipping TB lookup |
| Data structures (Vec + compact enums) | ~3-5% | Cache-friendly layout |
| Runtime concurrency (lock-free + RWX) | ~2-3% | Lock-free reads, no mprotect |
| Compilation pipeline (single-pass + inlining) | ~2-3% | Rust zero-cost abstractions |
| Hash + constant deduplication | ~1-2% | Simplified computation |

> Note: Direct guest_base addressing (~8-10%) is only applicable to
> the early linux-user mode and does not apply to full-system mode.

### 9. Trade-offs and Limitations

machina's performance advantages are built on the following
trade-offs:

- **RWX memory**: Violates the W^X security principle; forbidden on
  some platforms (iOS)
- **Simplified optimizer**: Lacks QEMU's bit-level tracking,
  resulting in slightly lower generated code quality
- **Unconditional BMI1**: Assumes host CPU support; incompatible
  with older CPUs
- **Simplified hash**: Distribution quality inferior to xxHash;
  degrades under high collision rates
- **Full-system MMU overhead**: Sv39 page table translation
  introduces additional memory access latency; TLB miss penalty
  is high
- **MMIO dispatch**: Device access goes through indirect callback
  paths with non-negligible latency

These trade-offs are reasonable for the target scenario of full-system
RISC-V emulation on modern x86-64 hosts.

---

## Part 5: Test Architecture

### 1. Overview

Machina employs a layered testing strategy, verifying correctness
progressively from low-level data structures up to full system-level
emulation. All tests are centralized in a standalone `tests/` crate,
keeping source files clean while ensuring complete coverage of public
APIs.

**Test pyramid**:

```
              +-------------------+
              |     Difftest      |  machina vs QEMU
              |    (35 tests)     |
              +-------------------+
              |     Frontend      |  decode -> IR -> codegen
              |   (217 tests)     |  -> execute
              +-------------------+  RV32I/RV64I/RVC/RV32F/Zb*
              |   Integration     |  IR -> liveness -> regalloc
              |   (105 tests)     |  -> codegen -> execute
              +-------------------+
              | System & Hardware |  RISC-V CSR/MMU/PMP, devices
              |   (312 tests)     |  VirtIO, boot, exec, tools
         +----+-------------------+----+
         |          Unit Tests         |  core(224) + backend(277)
         |         (756 tests)         |  + decode(93) + softfloat(62)
         |                             |  + gdbstub(57) + misc(43)
         +----+----+----+----+----+----+
```

**Total: 1425 tests**.

---

### 3. Test Architecture

#### Directory Structure

```
tests/
+-- Cargo.toml
+-- src/
|   +-- lib.rs                    # 37 module declarations
|   +-- core.rs                   # Core IR unit tests (219)
|   +-- core_address.rs           # Address type tests (5)
|   +-- backend/                  # Backend unit tests (277)
|   +-- decode/                   # Decoder generator tests (93)
|   +-- frontend/                 # Frontend instruction tests
|   |   +-- mod.rs                #   RV32I/RV64I/RVC/RV32F (116)
|   |   +-- difftest.rs           #   machina vs QEMU (35)
|   |   +-- riscv_zba.rs          #   Zba extension (17)
|   |   +-- riscv_zbb.rs          #   Zbb extension (34)
|   |   +-- riscv_zbc.rs          #   Zbc extension (22)
|   |   +-- riscv_zbs.rs          #   Zbs extension (28)
|   +-- integration/              # Integration tests (105)
|   +-- exec/                     # Execution loop tests (31)
|   +-- softmmu.rs                # Software MMU tests (28)
|   +-- softmmu_exec.rs           # SoftMMU exec tests (11)
|   +-- softfloat.rs              # IEEE 754 tests (62)
|   +-- gdbstub.rs                # GDB protocol tests (57)
|   +-- disas_bitmanip.rs         # Disassembler tests (43)
|   +-- monitor.rs                # Monitor console tests (20)
|   +-- hw_*.rs                   # Hardware device tests (108)
|   +-- virtio.rs                 # VirtIO core tests (16)
|   +-- virtio_net.rs             # VirtIO net tests (28)
|   +-- riscv_*.rs                # RISC-V subsystem tests (38)
|   +-- system_cpu_manager.rs     # CPU manager tests (6)
|   +-- ...                       # Other modules
+-- mtest/                        # Placeholder crate (0 tests)
    +-- Cargo.toml
    +-- src/lib.rs                # Empty; reserved for future use
```

#### Test Distribution by Module

| Module | Tests | Share | Description |
|--------|-------|-------|-------------|
| backend | 277 | 19.4% | x86-64 instruction encoding, code buffer |
| core | 224 | 15.7% | IR types, Opcode, Temp, Label, Op, Context, Address |
| frontend | 217 | 15.2% | RISC-V instruction execution (RV32I/RV64I/RVC/RV32F/Zb*, excl. difftest) |
| hw_* | 108 | 7.6% | Device models: PLIC, ACLINT, UART, QDev, SysBus, FDT |
| integration | 105 | 7.4% | IR --> codegen --> execute full pipeline |
| decode | 93 | 6.5% | .decode parsing, code generation, field extraction |
| other | 91 | 6.4% | accel_timer, cli_netdev, memory_region, monitor, softmmu, softmmu_exec, system_cpu_manager, tools, trace |
| softfloat | 62 | 4.4% | IEEE 754 floating-point operations |
| gdbstub | 57 | 4.0% | GDB remote protocol handling |
| virtio | 44 | 3.1% | VirtIO MMIO transport, block, and net devices |
| disas_bitmanip | 43 | 3.0% | Disassembler and bit-manipulation tests |
| riscv_* | 38 | 2.7% | CSR, MMU, PMP, exception handling |
| difftest | 35 | 2.5% | machina vs QEMU differential comparison |
| exec | 31 | 2.2% | TB cache, execution loop, multi-threaded vCPU |

---

### 4. Unit Tests

#### 4.1 Core Module (224 tests)

Verifies correctness of the IR foundational data structures.

| File | Test Coverage |
|------|---------------|
| `types.rs` | Type enum (I32/I64/I128/V64/V128/V256), MemOp bitfield |
| `opcode.rs` | Opcode properties (flags, parameter count, type constraints) |
| `temp.rs` | Temp creation (global/local/const/fixed), TempKind classification |
| `label.rs` | Label creation and reference counting |
| `op.rs` | Op construction, argument access, linked-list operations |
| `context.rs` | Context lifecycle, temp allocation, op emission |
| `regset.rs` | RegSet bitmap operations (insert/remove/contains/iter) |
| `tb.rs` | TranslationBlock creation and caching |

```bash
cargo test -p machina-tests core::
```

#### 4.2 Backend Module (277 tests)

Verifies correctness of the x86-64 instruction encoder.

| File | Test Coverage |
|------|---------------|
| `code_buffer.rs` | Code buffer allocation, writes, mprotect switching |
| `x86_64.rs` | All x86-64 instruction encoding (MOV/ADD/SUB/AND/OR/XOR/SHL/SHR/SAR/MUL/DIV/LEA/Jcc/SETcc/CMOVcc/BSF/BSR/LZCNT/TZCNT/POPCNT etc.) |

```bash
cargo test -p machina-tests backend::
```

#### 4.3 Decodetree Module (93 tests)

Verifies the `.decode` file parser and code generator.

| Test Group | Count | Description |
|------------|-------|-------------|
| Helper functions | 6 | is_bit_char, is_bit_token, is_inline_field, count_bit_tokens, to_camel |
| Bit-pattern parsing | 4 | Fixed bits, don't-care, inline fields, extra-wide patterns |
| Field parsing | 5 | Unsigned/signed/multi-segment/function-mapped/error handling |
| ArgSet parsing | 4 | Normal/empty/extern/non-extern |
| Continuation & grouping | 4 | Backslash continuation, brace/bracket grouping |
| Full parsing | 5 | mini decode, riscv32, empty input, comment-only, unknown format reference |
| Format inheritance | 2 | args/fields inheritance, bits merging |
| Pattern masks | 4 | R/I/B/Shift type masks |
| Field extraction | 15 | 32-bit register/immediate + 16-bit RVC fields |
| Pattern matching | 18 | 32-bit instruction matching + 11 RVC instruction matching |
| Code generation | 9 | mini/riscv32/ecall/fence/16-bit generation |
| Function handlers | 3 | rvc_register, shift_2, sreg_register |
| 16-bit decode | 2 | insn16.decode parsing and generation |
| Code quality | 2 | No u32 leakage, no duplicate trait methods |

```bash
cargo test -p machina-tests decode::
```

---

### 5. Integration Tests (105 tests)

**Source file**: `tests/src/integration/mod.rs`

Verifies the complete IR --> liveness --> register allocation -->
codegen --> execute pipeline. Uses a minimal RISC-V CPU state and
generates test cases in bulk via macros.

**Test macros**:

| Macro | Purpose |
|-------|---------|
| `riscv_bin_case!` | Binary arithmetic operations (add/sub/and/or/xor) |
| `riscv_shift_case!` | Shift operations (shl/shr/sar/rotl/rotr) |
| `riscv_setcond_case!` | Conditional set operations (eq/ne/lt/ge/ltu/geu) |
| `riscv_branch_case!` | Conditional branches (taken/not-taken) |
| `riscv_mem_case!` | Memory access (load/store at various widths) |

**Coverage**: ALU, shifts, comparisons, branches, memory read/write,
bit operations, rotations, byte swaps, popcount, multiply/divide,
carry/borrow, conditional moves, etc.

```bash
cargo test -p machina-tests integration::
```

---

### 6. Frontend Instruction Tests (217 tests)

**Source files**: `tests/src/frontend/mod.rs` (116 tests),
`tests/src/frontend/riscv_zba.rs` (17),
`tests/src/frontend/riscv_zbb.rs` (34),
`tests/src/frontend/riscv_zbc.rs` (22),
`tests/src/frontend/riscv_zbs.rs` (28).
The 35 differential tests in `tests/src/frontend/difftest.rs`
are excluded here and documented separately in Section 7.

#### 6.1 Test Runners

Frontend tests use four runner functions covering different
instruction formats:

| Function | Input | Purpose |
|----------|-------|---------|
| `run_rv(cpu, insn: u32)` | Single 32-bit instruction | Basic instruction testing |
| `run_rv_insns(cpu, &[u32])` | 32-bit instruction sequence | Multi-instruction sequences |
| `run_rv_bytes(cpu, &[u8])` | Raw byte stream | Mixed 16/32-bit |
| `run_rvc(cpu, insn: u16)` | Single 16-bit instruction | RVC compressed instructions |

**Execution flow** (using `run_rv_insns` as an example):

```
Instruction encoding --> write to guest memory
--> translator_loop decode --> IR generation --> liveness
--> regalloc --> x86-64 codegen --> execute generated code
--> read CPU state --> assertion checks
```

#### 6.2 RV32I / RV64I Tests

| Category | Instructions | Test Count |
|----------|-------------|------------|
| Upper immediate | lui, auipc | 3 |
| Jumps | jal, jalr | 2 |
| Branches | beq, bne, blt, bge, bltu, bgeu | 12 |
| Immediate arithmetic | addi, slti, sltiu, xori, ori, andi | 8 |
| Shifts | slli, srli, srai | 3 |
| Register arithmetic | add, sub, sll, srl, sra, slt, sltu, xor, or, and | 10 |
| W-suffix | addiw, slliw, srliw, sraiw, addw, subw, sllw, srlw, sraw | 10 |
| System | fence, ecall, ebreak | 3 |
| Special | x0 write-ignored, x0 reads-zero | 2 |
| Multi-instruction | addi+addi sequence, lui+addi combination | 2 |

---

### 7. Differential Tests (35 tests)

**Source file**: `tests/src/frontend/difftest.rs`

Differential tests execute the same RISC-V instruction through both
the machina full pipeline and the QEMU reference implementation,
then compare CPU state. If the results match, the machina translation
is considered correct.

**Required tools**:

| Tool | Install Command |
|------|-----------------|
| `riscv64-linux-gnu-gcc` | `apt install gcc-riscv64-linux-gnu` |
| `qemu-riscv64` | `apt install qemu-user` |

#### 7.1 Overall Architecture

```
                    +---------------------+
                    |     Test Case       |
                    |  (insn + init regs) |
                    +---------+-----------+
                              |
              +---------------+---------------+
              v                               v
     +----------------+             +-----------------+
     | machina side   |             |   QEMU side     |
     |                |             |                 |
     | 1. encode insn |             | 1. gen .S asm   |
     | 2. translator  |             | 2. gcc cross    |
     |    _loop       |             | 3. qemu-riscv64 |
     | 3. IR gen      |             |    execute      |
     | 4. liveness    |             | 4. parse stdout |
     | 5. regalloc    |             |    (256 bytes   |
     | 6. x86-64      |             |     reg dump)   |
     |    codegen     |             |                 |
     | 7. execute     |             |                 |
     +-------+--------+             +--------+--------+
              |                               |
              v                               v
     +----------------+             +-----------------+
     | RiscvCpu state |             | [u64; 32] array |
     | .gpr[0..32]    |             | x0..x31 values  |
     +-------+--------+             +--------+--------+
              |                               |
              +--------------+----------------+
                             v
                    +-----------------+
                    |   assert_eq!()  |
                    +-----------------+
```

#### 7.2 QEMU Side Internals

For each test case, the framework dynamically generates a RISC-V
assembly source:

```asm
.global _start
_start:
    la gp, save_area       # x3 = save area base address

    # -- Phase 1: Load initial register values --
    li t0, <val1>
    li t1, <val2>

    # -- Phase 2: Execute the instruction under test --
    add t2, t0, t1

    # -- Phase 3: Save all 32 registers --
    sd x0,  0(gp)
    sd x1,  8(gp)
    ...
    sd x31, 248(gp)

    # -- Phase 4: write(1, save_area, 256) --
    li a7, 64
    li a0, 1
    mv a1, gp
    li a2, 256
    ecall

    # -- Phase 5: exit(0) --
    li a7, 93
    li a0, 0
    ecall

.bss
.align 3
save_area: .space 256       # 32 x 8 bytes
```

Compilation and execution flow:

```
gen_alu_asm()              gen .S source
    |
    v
riscv64-linux-gnu-gcc     cross compile
  -nostdlib -static         no libc, raw syscall
  -o /tmp/xxx.elf           static ELF output
    |
    v
qemu-riscv64 xxx.elf      user-mode execute
    |
    v
stdout (256 bytes)         32 little-endian u64
    |
    v
parse --> [u64; 32]        register array
```

Temporary files are named with `pid_tid` to avoid conflicts during
parallel test execution, and are automatically cleaned up afterward.

Branch instructions use a taken/not-taken pattern, where the value
of x7(t2) determines whether the branch was taken
(1=taken, 0=not-taken).

#### 7.3 machina Side Internals

ALU instructions directly reuse the full-pipeline infrastructure:

```rust
fn run_machina(
    init: &[(usize, u64)],  // Initial register values
    insns: &[u32],           // RISC-V machine code sequence
) -> RiscvCpu
```

Pipeline: `RISC-V machine code --> decode --> trans_* --> TCG IR
--> optimize --> liveness --> regalloc --> x86-64 codegen --> execute`

Branch instructions exit the translation block (TB), and
taken/not-taken is determined by the PC value:
- `PC = offset` --> taken
- `PC = 4` --> not-taken

#### 7.4 Register Conventions

| Register | ABI Name | Purpose |
|----------|----------|---------|
| x3 | gp | **Reserved**: QEMU-side save area base address |
| x5 | t0 | Source operand 1 (rs1) |
| x6 | t1 | Source operand 2 (rs2) |
| x7 | t2 | Destination register (rd) |

x3 cannot be used as a test register because the QEMU-side
`la gp, save_area` overwrites its value.

#### 7.5 Boundary Value Strategy

| Constant | Value | Meaning |
|----------|-------|---------|
| `V0` | `0` | Zero |
| `V1` | `1` | Smallest positive number |
| `VMAX` | `0x7FFF_FFFF_FFFF_FFFF` | i64 maximum |
| `VMIN` | `0x8000_0000_0000_0000` | i64 minimum |
| `VNEG1` | `0xFFFF_FFFF_FFFF_FFFF` | -1 (all ones) |
| `V32MAX` | `0x7FFF_FFFF` | i32 maximum |
| `V32MIN` | `0xFFFF_FFFF_8000_0000` | i32 minimum (sign-extended) |
| `V32FF` | `0xFFFF_FFFF` | u32 maximum |
| `VPATTERN` | `0xDEAD_BEEF_CAFE_BABE` | Random bit pattern |

Each instruction uses 4-7 boundary value combinations, focusing on
overflow boundaries, sign extension, zero behavior, and all-ones
bit patterns.

---

### 8. Hardware and System Tests

Hardware and system tests verify device models, the reference
machine platform, and end-to-end tool integration. Tests live
in individual `hw_*.rs` modules under `tests/src/`, plus
`hw_ref_machine.rs` for platform-level tests and `tools.rs`
for smoke/integration tests against the machina binary.

> The `tests/mtest/` crate exists as a placeholder (0 tests).
> It is reserved for future bare-metal firmware tests.

#### 8.1 Test Modules

| Module | Tests | Description |
|--------|-------|-------------|
| hw_ref_machine | 31 | RefMachine init, memory map, FDT, IRQ wiring, boot, UART, VirtIO, PLIC, multi-hart |
| hw_uart | 12 | UART 16550A register read/write, TX/RX, FIFO |
| hw_aclint | 13 | ACLINT timer MMIO, IPI, mtime/mtimecmp |
| hw_plic | 9 | PLIC priority, pending, enable, claim/complete |
| hw_qdev | 8 | QDev object lifecycle, property, realize |
| hw_sysbus | 7 | SysBus MMIO mapping, device attachment |
| hw_irq | 7 | IRQ line raise/lower, sink/source wiring |
| hw_chardev | 7 | Character device backend interface |
| hw_clock | 6 | Clock frequency, scaling |
| hw_loader | 5 | ELF/binary loader, kernel/initrd loading |
| hw_fdt | 3 | FDT generation and node structure |
| tools | 7 | irdump, irbackend, SBI smoke boot, sifive_test |
