# IR Ops 设计文档

本文档描述 machina 中间表示（IR）操作的完整设计，涵盖 opcode 体系、
类型系统、Op 结构、参数编码约定和 IR Builder API。

源码位置：`core/src/opcode.rs`、`core/src/op.rs`、`core/src/ir_builder.rs`、
`core/src/types.rs`。

---

## 1. 设计原则

### 1.1 统一多态 vs 类型分裂

QEMU 原始设计中 `add_i32` 和 `add_i64` 是不同的 opcode（类型分裂）。
machina 采用统一的 `Add`，实际类型由 `Op::op_type` 字段携带（类型多态）。

**优势**：

- 减少约 40% 的 opcode 数量
- 优化器用统一逻辑处理，不需要 `match (Add32, Add64) => ...`
- 后端通过 `op.op_type` 选择 32/64 位指令编码，逻辑更清晰
- `OpFlags::INT` 标记哪些 opcode 是多态的，非多态的（如 `ExtI32I64`）
  有固定类型

### 1.2 固定大小参数数组

`Op::args` 使用 `[TempIdx; 10]` 固定数组而非 `Vec`，避免堆分配。
每个 TB 可能有数百个 Op，固定数组消除了大量 allocator 压力。

### 1.3 编译期安全

`OPCODE_DEFS` 表大小为 `Opcode::Count as usize`。新增 opcode 忘记
加表项会导致编译错误，从根本上防止表与枚举不同步。

---

## 2. Opcode 枚举

```rust
#[repr(u8)]
pub enum Opcode { Mov = 0, ..., Count }
```

共 158 个有效 opcode + 1 个 sentinel（`Count`），分为 13 类：

### 2.1 数据移动（4 个）

| Opcode | 语义 | oargs | iargs | cargs | Flags |
|--------|------|-------|-------|-------|-------|
| `Mov` | `d = s` | 1 | 1 | 0 | INT, NP |
| `SetCond` | `d = (a cond b) ? 1 : 0` | 1 | 2 | 1 | INT |
| `NegSetCond` | `d = (a cond b) ? -1 : 0` | 1 | 2 | 1 | INT |
| `MovCond` | `d = (c1 cond c2) ? v1 : v2` | 1 | 4 | 1 | INT |

### 2.2 算术运算（12 个）

| Opcode | 语义 | oargs | iargs | cargs | Flags |
|--------|------|-------|-------|-------|-------|
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

### 2.3 进位/借位算术（8 个）

隐式进位/借位标志通过 `CARRY_OUT`/`CARRY_IN` flags 声明依赖关系。

| Opcode | 语义 | Flags |
|--------|------|-------|
| `AddCO` | `d = a + b`，产生进位 | INT, CO |
| `AddCI` | `d = a + b + carry` | INT, CI |
| `AddCIO` | `d = a + b + carry`，产生进位 | INT, CI, CO |
| `AddC1O` | `d = a + b + 1`，产生进位 | INT, CO |
| `SubBO` | `d = a - b`，产生借位 | INT, CO |
| `SubBI` | `d = a - b - borrow` | INT, CI |
| `SubBIO` | `d = a - b - borrow`，产生借位 | INT, CI, CO |
| `SubB1O` | `d = a - b - 1`，产生借位 | INT, CO |

所有进位 op 均为 1 oarg, 2 iargs, 0 cargs。

### 2.4 逻辑运算（9 个）

| Opcode | 语义 | oargs | iargs |
|--------|------|-------|-------|
| `And` | `d = a & b` | 1 | 2 |
| `Or` | `d = a \| b` | 1 | 2 |
| `Xor` | `d = a ^ b` | 1 | 2 |
| `Not` | `d = ~s` | 1 | 1 |
| `AndC` | `d = a & ~b` | 1 | 2 |
| `OrC` | `d = a \| ~b` | 1 | 2 |
| `Eqv` | `d = ~(a ^ b)` | 1 | 2 |
| `Nand` | `d = ~(a & b)` | 1 | 2 |
| `Nor` | `d = ~(a \| b)` | 1 | 2 |

全部标记 `INT`，0 cargs。

### 2.5 移位/旋转（5 个）

| Opcode | 语义 |
|--------|------|
| `Shl` | `d = a << b` |
| `Shr` | `d = a >> b` (logical) |
| `Sar` | `d = a >> b` (arithmetic) |
| `RotL` | `d = a rotl b` |
| `RotR` | `d = a rotr b` |

全部 1 oarg, 2 iargs, 0 cargs, INT。

### 2.6 位域操作（4 个）

| Opcode | 语义 | oargs | iargs | cargs |
|--------|------|-------|-------|-------|
| `Extract` | `d = (src >> ofs) & mask(len)` | 1 | 1 | 2 (ofs, len) |
| `SExtract` | 同上，带符号扩展 | 1 | 1 | 2 (ofs, len) |
| `Deposit` | `d = (a & ~mask) \| ((b << ofs) & mask)` | 1 | 2 | 2 (ofs, len) |
| `Extract2` | `d = (al:ah >> ofs)[N-1:0]` | 1 | 2 | 1 (ofs) |

### 2.7 字节序交换（3 个）

| Opcode | 语义 | cargs |
|--------|------|-------|
| `Bswap16` | 16 位字节序交换 | 1 (flags) |
| `Bswap32` | 32 位字节序交换 | 1 (flags) |
| `Bswap64` | 64 位字节序交换 | 1 (flags) |

全部 1 oarg, 1 iarg, INT。

### 2.8 位计数（3 个）

| Opcode | 语义 | oargs | iargs |
|--------|------|-------|-------|
| `Clz` | count leading zeros, `d = clz(a) ?: b` | 1 | 2 |
| `Ctz` | count trailing zeros, `d = ctz(a) ?: b` | 1 | 2 |
| `CtPop` | population count | 1 | 1 |

`Clz`/`Ctz` 的第二个输入是 fallback 值（当 a==0 时使用）。

### 2.9 类型转换（4 个）

| Opcode | 语义 | 固定类型 |
|--------|------|---------|
| `ExtI32I64` | sign-extend i32 → i64 | I64 |
| `ExtUI32I64` | zero-extend i32 → i64 | I64 |
| `ExtrlI64I32` | truncate i64 → i32 (low) | I32 |
| `ExtrhI64I32` | extract i64 → i32 (high) | I32 |

这些 op 不是类型多态的——有固定的输入/输出类型，不标记 `INT`。

### 2.10 宿主内存访问（11 个）

用于直接访问 CPUState 字段（通过 env 指针 + 偏移量）。

**加载**（1 oarg, 1 iarg, 1 carg=offset）：

| Opcode | 语义 |
|--------|------|
| `Ld8U` | `d = *(u8*)(base + ofs)` |
| `Ld8S` | `d = *(i8*)(base + ofs)` |
| `Ld16U` | `d = *(u16*)(base + ofs)` |
| `Ld16S` | `d = *(i16*)(base + ofs)` |
| `Ld32U` | `d = *(u32*)(base + ofs)` |
| `Ld32S` | `d = *(i32*)(base + ofs)` |
| `Ld` | `d = *(native*)(base + ofs)` |

**存储**（0 oargs, 2 iargs, 1 carg=offset）：

| Opcode | 语义 |
|--------|------|
| `St8` | `*(u8*)(base + ofs) = src` |
| `St16` | `*(u16*)(base + ofs) = src` |
| `St32` | `*(u32*)(base + ofs) = src` |
| `St` | `*(native*)(base + ofs) = src` |

### 2.11 客户内存访问（4 个）

通过软件 TLB 访问客户地址空间。标记 `CALL_CLOBBER | SIDE_EFFECTS | INT`。

| Opcode | 语义 | oargs | iargs | cargs |
|--------|------|-------|-------|-------|
| `QemuLd` | 客户内存加载 | 1 | 1 | 1 (memop) |
| `QemuSt` | 客户内存存储 | 0 | 2 | 1 (memop) |
| `QemuLd2` | 128 位客户加载（双寄存器） | 2 | 1 | 1 (memop) |
| `QemuSt2` | 128 位客户存储（双寄存器） | 0 | 3 | 1 (memop) |

### 2.12 控制流（7 个）

| Opcode | 语义 | oargs | iargs | cargs | Flags |
|--------|------|-------|-------|-------|-------|
| `Br` | 无条件跳转到 label | 0 | 0 | 1 (label) | BB_END, NP |
| `BrCond` | 条件跳转 | 0 | 2 | 2 (cond, label) | BB_END, COND_BRANCH, INT |
| `SetLabel` | 定义 label 位置 | 0 | 0 | 1 (label) | BB_END, NP |
| `GotoTb` | 直接跳转到另一个 TB | 0 | 0 | 1 (tb_idx) | BB_EXIT, BB_END, NP |
| `ExitTb` | 返回执行循环 | 0 | 0 | 1 (val) | BB_EXIT, BB_END, NP |
| `GotoPtr` | 通过寄存器间接跳转 | 0 | 1 | 0 | BB_EXIT, BB_END |
| `Mb` | 内存屏障 | 0 | 0 | 1 (bar_type) | NP |

#### 2.12.1 多线程 vCPU 下的 `ExitTb` 约定

`ExitTb` 的返回值不仅表示"退出原因"，还参与执行循环的链路协议：

- `TB_EXIT_IDX0` / `TB_EXIT_IDX1`：对应 `goto_tb` 槽位 0/1，可被
  执行循环识别并触发 TB 直接链路 patch；
- `TB_EXIT_NOCHAIN`：用于间接跳转类路径，执行循环会按当前 PC/flags
  重新查找 TB，并利用 `exit_target` 做单项缓存；
- `>= TB_EXIT_MAX`：真实异常/系统退出（如 `EXCP_ECALL`、
  `EXCP_EBREAK`、`EXCP_UNDEF`），直接返回上层。

为了在 direct chaining 后仍可识别"真正退出的源 TB"，core 里提供
`encode_tb_exit` / `decode_tb_exit`：低位保存 exit code，高位携带
源 TB 索引标记。

### 2.13 杂项（5 个）

| Opcode | 语义 | Flags |
|--------|------|-------|
| `Call` | 调用辅助函数 | CC, NP |
| `PluginCb` | 插件回调 | NP |
| `PluginMemCb` | 插件内存回调 | NP |
| `Nop` | 空操作 | NP |
| `Discard` | 丢弃 temp | NP |
| `InsnStart` | 客户指令边界标记 | NP |

### 2.14 32 位宿主兼容（2 个）

| Opcode | 语义 | 固定类型 |
|--------|------|---------|
| `BrCond2I32` | 64 位条件分支（32 位宿主，寄存器对） | I32 |
| `SetCond2I32` | 64 位条件设置（32 位宿主） | I32 |

### 2.15 向量操作（57 个）

向量 op 全部标记 `VECTOR`，按子类别分组：

**数据移动**（6 个）：`MovVec`, `DupVec`, `Dup2Vec`, `LdVec`, `StVec`, `DupmVec`

**算术**（12 个）：`AddVec`, `SubVec`, `MulVec`, `NegVec`, `AbsVec`,
`SsaddVec`, `UsaddVec`, `SssubVec`, `UssubVec`, `SminVec`, `UminVec`,
`SmaxVec`, `UmaxVec`

**逻辑**（9 个）：`AndVec`, `OrVec`, `XorVec`, `AndcVec`, `OrcVec`,
`NandVec`, `NorVec`, `EqvVec`, `NotVec`

**移位——立即数**（4 个）：`ShliVec`, `ShriVec`, `SariVec`, `RotliVec`
（1 oarg, 1 iarg, 1 carg=imm）

**移位——标量**（4 个）：`ShlsVec`, `ShrsVec`, `SarsVec`, `RotlsVec`
（1 oarg, 2 iargs）

**移位——向量**（5 个）：`ShlvVec`, `ShrvVec`, `SarvVec`, `RotlvVec`, `RotrvVec`
（1 oarg, 2 iargs）

**比较/选择**（3 个）：
- `CmpVec`：1 oarg, 2 iargs, 1 carg (cond)
- `BitselVec`：1 oarg, 3 iargs — `d = (a & c) | (b & ~c)`
- `CmpselVec`：1 oarg, 4 iargs, 1 carg (cond) — `d = (c1 cond c2) ? v1 : v2`

---

## 3. OpFlags 属性标志

```rust
pub struct OpFlags(u16);
```

| 标志 | 值 | 含义 |
|------|-----|------|
| `BB_EXIT` | 0x01 | 退出翻译块 |
| `BB_END` | 0x02 | 结束基本块（下一个 op 开始新 BB） |
| `CALL_CLOBBER` | 0x04 | 破坏调用者保存寄存器 |
| `SIDE_EFFECTS` | 0x08 | 有副作用，不可被 DCE 消除 |
| `INT` | 0x10 | 类型多态（I32/I64） |
| `NOT_PRESENT` | 0x20 | 不直接生成宿主代码（由分配器特殊处理） |
| `VECTOR` | 0x40 | 向量操作 |
| `COND_BRANCH` | 0x80 | 条件分支 |
| `CARRY_OUT` | 0x100 | 产生进位/借位输出 |
| `CARRY_IN` | 0x200 | 消耗进位/借位输入 |

标志可组合使用，例如 `BrCond` = `BB_END | COND_BRANCH | INT`。

**标志对流水线各阶段的影响**：

- **活跃性分析**：`BB_END` 触发全局变量活跃标记；`SIDE_EFFECTS` 阻止 DCE
- **寄存器分配**：`NOT_PRESENT` 的 op 走专用路径而非通用 `regalloc_op()`
- **代码生成**：`BB_EXIT` 的 op 由后端直接处理（emit_exit_tb 等）

---

## 4. OpDef 静态表

```rust
pub struct OpDef {
    pub name: &'static str,  // 调试/dump 用名称
    pub nb_oargs: u8,        // 输出参数数量
    pub nb_iargs: u8,        // 输入参数数量
    pub nb_cargs: u8,        // 常量参数数量
    pub flags: OpFlags,
}

pub static OPCODE_DEFS: [OpDef; Opcode::Count as usize] = [ ... ];
```

通过 `Opcode::def()` 方法查表：

```rust
impl Opcode {
    pub fn def(self) -> &'static OpDef {
        &OPCODE_DEFS[self as usize]
    }
}
```

**编译期保证**：数组大小 = `Opcode::Count as usize`，枚举新增变体
但忘记在表中添加对应项会导致编译错误。

---

## 5. Op 结构

```rust
pub struct Op {
    pub idx: OpIdx,              // 在 ops 列表中的索引
    pub opc: Opcode,             // 操作码
    pub op_type: Type,           // 多态 op 的实际类型
    pub param1: u8,              // opcode 特定参数 (CALLI/TYPE/VECE)
    pub param2: u8,              // opcode 特定参数 (CALLO/FLAGS/VECE)
    pub life: LifeData,          // 活跃性分析结果
    pub output_pref: [RegSet; 2], // 寄存器分配提示
    pub args: [TempIdx; 10],     // 参数数组
    pub nargs: u8,               // 实际参数数量
}
```

### 5.1 参数布局

`args[]` 数组按固定顺序排列：

```
args[0 .. nb_oargs]                          → 输出参数
args[nb_oargs .. nb_oargs+nb_iargs]          → 输入参数
args[nb_oargs+nb_iargs .. nb_oargs+nb_iargs+nb_cargs] → 常量参数
```

通过 `oargs()`/`iargs()`/`cargs()` 方法获取对应切片，
这些方法根据 `OpDef` 的参数计数做切片，零成本抽象。

**示例**：`BrCond`（0 oargs, 2 iargs, 2 cargs）

```
args[0] = a        (input: 比较左操作数)
args[1] = b        (input: 比较右操作数)
args[2] = cond     (const: 条件码，编码为 TempIdx)
args[3] = label_id (const: 目标 label，编码为 TempIdx)
```

### 5.2 常量参数编码

常量参数（条件码、偏移量、label ID 等）编码为 `TempIdx(raw_value as u32)`
存入 `args[]`，与 QEMU 约定一致。IR Builder 中通过辅助函数 `carg()` 转换：

```rust
fn carg(val: u32) -> TempIdx { TempIdx(val) }
```

### 5.3 LifeData

```rust
pub struct LifeData(pub u32);  // 2 bit per arg
```

每个参数占 2 bit：
- bit `n*2`：dead — 该参数在此 op 后不再使用
- bit `n*2+1`：sync — 该参数（全局变量）需要同步回内存

由活跃性分析（`liveness.rs`）填充，供寄存器分配器消费。

---

## 6. IR Builder API

`impl Context` 上的 `gen_*` 方法，将高层操作转换为 `Op` 并追加到
ops 列表。内部通过 `emit_binary()`/`emit_unary()` 等辅助方法统一构造。

### 6.1 二元 ALU（1 oarg, 2 iargs）

签名：`gen_xxx(&mut self, ty: Type, d: TempIdx, a: TempIdx, b: TempIdx) -> TempIdx`

`gen_add`, `gen_sub`, `gen_mul`, `gen_and`, `gen_or`, `gen_xor`,
`gen_shl`, `gen_shr`, `gen_sar`, `gen_rotl`, `gen_rotr`,
`gen_andc`, `gen_orc`, `gen_eqv`, `gen_nand`, `gen_nor`,
`gen_divs`, `gen_divu`, `gen_rems`, `gen_remu`,
`gen_mulsh`, `gen_muluh`,
`gen_clz`, `gen_ctz`

### 6.2 一元（1 oarg, 1 iarg）

签名：`gen_xxx(&mut self, ty: Type, d: TempIdx, s: TempIdx) -> TempIdx`

`gen_neg`, `gen_not`, `gen_mov`, `gen_ctpop`

### 6.3 类型转换（固定类型）

签名：`gen_xxx(&mut self, d: TempIdx, s: TempIdx) -> TempIdx`

| 方法 | 语义 |
|------|------|
| `gen_ext_i32_i64` | sign-extend i32 → i64 |
| `gen_ext_u32_i64` | zero-extend i32 → i64 |
| `gen_extrl_i64_i32` | truncate i64 → i32 (low) |
| `gen_extrh_i64_i32` | extract i64 → i32 (high) |

### 6.4 条件操作

| 方法 | 签名 |
|------|------|
| `gen_setcond` | `(ty, d, a, b, cond) → d` |
| `gen_negsetcond` | `(ty, d, a, b, cond) → d` |
| `gen_movcond` | `(ty, d, c1, c2, v1, v2, cond) → d` |

### 6.5 位域操作

| 方法 | 签名 |
|------|------|
| `gen_extract` | `(ty, d, src, ofs, len) → d` |
| `gen_sextract` | `(ty, d, src, ofs, len) → d` |
| `gen_deposit` | `(ty, d, a, b, ofs, len) → d` |
| `gen_extract2` | `(ty, d, al, ah, ofs) → d` |

### 6.6 字节序交换

签名：`gen_bswapN(&mut self, ty: Type, d: TempIdx, src: TempIdx, flags: u32) -> TempIdx`

`gen_bswap16`, `gen_bswap32`, `gen_bswap64`

### 6.7 双宽度运算

| 方法 | 签名 |
|------|------|
| `gen_divs2` | `(ty, dl, dh, al, ah, b)` |
| `gen_divu2` | `(ty, dl, dh, al, ah, b)` |
| `gen_muls2` | `(ty, dl, dh, a, b)` |
| `gen_mulu2` | `(ty, dl, dh, a, b)` |

### 6.8 进位算术

签名同二元 ALU：`gen_xxx(&mut self, ty, d, a, b) -> TempIdx`

`gen_addco`, `gen_addci`, `gen_addcio`, `gen_addc1o`,
`gen_subbo`, `gen_subbi`, `gen_subbio`, `gen_subb1o`

### 6.9 宿主内存访问

**加载**：`gen_ld(&mut self, ty, dst, base, offset) -> TempIdx`
以及 `gen_ld8u`, `gen_ld8s`, `gen_ld16u`, `gen_ld16s`, `gen_ld32u`, `gen_ld32s`

**存储**：`gen_st(&mut self, ty, src, base, offset)`
以及 `gen_st8`, `gen_st16`, `gen_st32`

### 6.10 客户内存访问

| 方法 | 签名 |
|------|------|
| `gen_qemu_ld` | `(ty, dst, addr, memop) → dst` |
| `gen_qemu_st` | `(ty, val, addr, memop)` |
| `gen_qemu_ld2` | `(ty, dl, dh, addr, memop)` |
| `gen_qemu_st2` | `(ty, vl, vh, addr, memop)` |

### 6.11 控制流

| 方法 | 签名 |
|------|------|
| `gen_br` | `(label_id)` |
| `gen_brcond` | `(ty, a, b, cond, label_id)` |
| `gen_set_label` | `(label_id)` |
| `gen_goto_tb` | `(tb_idx)` |
| `gen_exit_tb` | `(val)` |
| `gen_goto_ptr` | `(ptr)` |
| `gen_mb` | `(bar_type)` |
| `gen_insn_start` | `(pc)` — 编码为 2 个 cargs (lo, hi) |
| `gen_discard` | `(ty, t)` |

### 6.12 32 位宿主兼容

| 方法 | 签名 |
|------|------|
| `gen_brcond2_i32` | `(al, ah, bl, bh, cond, label_id)` |
| `gen_setcond2_i32` | `(d, al, ah, bl, bh, cond) → d` |

### 6.13 向量操作

**数据移动**：`gen_dup_vec`, `gen_dup2_vec`, `gen_ld_vec`, `gen_st_vec`, `gen_dupm_vec`

**算术**：`gen_add_vec`, `gen_sub_vec`, `gen_mul_vec`, `gen_neg_vec`, `gen_abs_vec`,
`gen_ssadd_vec`, `gen_usadd_vec`, `gen_sssub_vec`, `gen_ussub_vec`,
`gen_smin_vec`, `gen_umin_vec`, `gen_smax_vec`, `gen_umax_vec`

**逻辑**：`gen_and_vec`, `gen_or_vec`, `gen_xor_vec`, `gen_andc_vec`, `gen_orc_vec`,
`gen_nand_vec`, `gen_nor_vec`, `gen_eqv_vec`, `gen_not_vec`

**移位（立即数）**：`gen_shli_vec`, `gen_shri_vec`, `gen_sari_vec`, `gen_rotli_vec`

**移位（标量）**：`gen_shls_vec`, `gen_shrs_vec`, `gen_sars_vec`, `gen_rotls_vec`

**移位（向量）**：`gen_shlv_vec`, `gen_shrv_vec`, `gen_sarv_vec`, `gen_rotlv_vec`, `gen_rotrv_vec`

**比较/选择**：`gen_cmp_vec`, `gen_bitsel_vec`, `gen_cmpsel_vec`

---

## 7. 与 QEMU 的对比

| 方面 | QEMU | machina |
|------|------|---------|
| Opcode 设计 | 类型分裂（`add_i32`/`add_i64`） | 统一多态（`Add` + `op_type`） |
| Opcode 定义 | `DEF()` 宏 + `tcg-opc.h` | `enum Opcode` + `OPCODE_DEFS` 数组 |
| Op 参数存储 | 链表 + 动态分配 | 固定数组 `[TempIdx; 10]` |
| 常量参数 | 编码为 `TCGArg` | 编码为 `TempIdx(raw_value)` |
| 标志系统 | `TCG_OPF_*` 宏 | `OpFlags(u16)` 位域 |
| 编译期安全 | 无（运行时断言） | 数组大小 = `Count`，编译期验证 |
| 向量 op | 独立的 `_vec` 后缀 opcode | 同样独立，标记 `VECTOR` |

---

## 8. QEMU 参考映射

| QEMU | machina | 文件 |
|------|---------|------|
| `TCGOpcode` | `enum Opcode` | `core/src/opcode.rs` |
| `TCGOpDef` | `struct OpDef` | `core/src/opcode.rs` |
| `TCG_OPF_*` | `struct OpFlags` | `core/src/opcode.rs` |
| `TCGOp` | `struct Op` | `core/src/op.rs` |
| `TCGLifeData` | `struct LifeData` | `core/src/op.rs` |
| `tcg_gen_op*` | `Context::gen_*` | `core/src/ir_builder.rs` |
