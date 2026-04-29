# Machina 参考手册

> 目标读者：machina 内部开发者。

## 目录

- [Part 1: IR 操作码参考](#part-1-ir-操作码参考)
- [Part 2: x86-64 后端参考](#part-2-x86-64-后端参考)
- [Part 3: 设备模型参考](#part-3-设备模型参考)
- [Part 4: 性能分析](#part-4-性能分析)
- [Part 5: 测试架构](#part-5-测试架构)

---

## Part 1: IR 操作码参考

本文档描述 machina 中间表示（IR）操作的完整设计，涵盖 opcode 体系、
类型系统、Op 结构、参数编码约定和 IR Builder API。

源码位置：`core/src/opcode.rs`、`core/src/op.rs`、
`core/src/ir_builder.rs`、`core/src/types.rs`。

---

### 1. 设计原则

#### 1.1 统一多态 vs 类型分裂

QEMU 原始设计中 `add_i32` 和 `add_i64` 是不同的 opcode（类型分裂）。
machina 采用统一的 `Add`，实际类型由 `Op::op_type` 字段携带（类型多态）。

**优势**：

- 减少约 40% 的 opcode 数量
- 优化器用统一逻辑处理，不需要 `match (Add32, Add64) => ...`
- 后端通过 `op.op_type` 选择 32/64 位指令编码，逻辑更清晰
- `OpFlags::INT` 标记哪些 opcode 是多态的，非多态的（如 `ExtI32I64`）
  有固定类型

#### 1.2 固定大小参数数组

`Op::args` 使用 `[TempIdx; 10]` 固定数组而非 `Vec`，避免堆分配。
每个 TB 可能有数百个 Op，固定数组消除了大量 allocator 压力。

#### 1.3 编译期安全

`OPCODE_DEFS` 表大小为 `Opcode::Count as usize`。新增 opcode 忘记
加表项会导致编译错误，从根本上防止表与枚举不同步。

---

### 2. Opcode 枚举

```rust
#[repr(u8)]
pub enum Opcode { Mov = 0, ..., Count }
```

共 158 个有效 opcode + 1 个 sentinel（`Count`），分为 13 类：

#### 2.1 数据移动（4 个）

| Opcode | 语义 | oargs | iargs | cargs | Flags |
|--------|------|-------|-------|-------|-------|
| `Mov` | `d = s` | 1 | 1 | 0 | INT, NP |
| `SetCond` | `d = (a cond b) ? 1 : 0` | 1 | 2 | 1 | INT |
| `NegSetCond` | `d = (a cond b) ? -1 : 0` | 1 | 2 | 1 | INT |
| `MovCond` | `d = (c1 cond c2) ? v1 : v2` | 1 | 4 | 1 | INT |

#### 2.2 算术运算（12 个）

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

#### 2.3 进位/借位算术（8 个）

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

#### 2.4 逻辑运算（9 个）

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

#### 2.5 移位/旋转（5 个）

| Opcode | 语义 |
|--------|------|
| `Shl` | `d = a << b` |
| `Shr` | `d = a >> b` (logical) |
| `Sar` | `d = a >> b` (arithmetic) |
| `RotL` | `d = a rotl b` |
| `RotR` | `d = a rotr b` |

全部 1 oarg, 2 iargs, 0 cargs, INT。

#### 2.6 位域操作（4 个）

| Opcode | 语义 | oargs | iargs | cargs |
|--------|------|-------|-------|-------|
| `Extract` | `d = (src >> ofs) & mask(len)` | 1 | 1 | 2 (ofs, len) |
| `SExtract` | 同上，带符号扩展 | 1 | 1 | 2 (ofs, len) |
| `Deposit` | `d = (a & ~mask) \| ((b << ofs) & mask)` | 1 | 2 | 2 (ofs, len) |
| `Extract2` | `d = (al:ah >> ofs)[N-1:0]` | 1 | 2 | 1 (ofs) |

#### 2.7 字节序交换（3 个）

| Opcode | 语义 | cargs |
|--------|------|-------|
| `Bswap16` | 16 位字节序交换 | 1 (flags) |
| `Bswap32` | 32 位字节序交换 | 1 (flags) |
| `Bswap64` | 64 位字节序交换 | 1 (flags) |

全部 1 oarg, 1 iarg, INT。

#### 2.8 位计数（3 个）

| Opcode | 语义 | oargs | iargs |
|--------|------|-------|-------|
| `Clz` | count leading zeros, `d = clz(a) ?: b` | 1 | 2 |
| `Ctz` | count trailing zeros, `d = ctz(a) ?: b` | 1 | 2 |
| `CtPop` | population count | 1 | 1 |

`Clz`/`Ctz` 的第二个输入是 fallback 值（当 a==0 时使用）。

#### 2.9 类型转换（4 个）

| Opcode | 语义 | 固定类型 |
|--------|------|---------|
| `ExtI32I64` | sign-extend i32 -> i64 | I64 |
| `ExtUI32I64` | zero-extend i32 -> i64 | I64 |
| `ExtrlI64I32` | truncate i64 -> i32 (low) | I32 |
| `ExtrhI64I32` | extract i64 -> i32 (high) | I32 |

这些 op 不是类型多态的——有固定的输入/输出类型，不标记 `INT`。

#### 2.10 宿主内存访问（11 个）

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

#### 2.11 客户内存访问（4 个）

通过软件 TLB 访问客户地址空间。标记
`CALL_CLOBBER | SIDE_EFFECTS | INT`。

| Opcode | 语义 | oargs | iargs | cargs |
|--------|------|-------|-------|-------|
| `QemuLd` | 客户内存加载 | 1 | 1 | 1 (memop) |
| `QemuSt` | 客户内存存储 | 0 | 2 | 1 (memop) |
| `QemuLd2` | 128 位客户加载（双寄存器） | 2 | 1 | 1 (memop) |
| `QemuSt2` | 128 位客户存储（双寄存器） | 0 | 3 | 1 (memop) |

#### 2.12 控制流（7 个）

| Opcode | 语义 | oargs | iargs | cargs | Flags |
|--------|------|-------|-------|-------|-------|
| `Br` | 无条件跳转到 label | 0 | 0 | 1 (label) | BB_END, NP |
| `BrCond` | 条件跳转 | 0 | 2 | 2 (cond, label) | BB_END, COND_BRANCH, INT |
| `SetLabel` | 定义 label 位置 | 0 | 0 | 1 (label) | BB_END, NP |
| `GotoTb` | 直接跳转到另一个 TB | 0 | 0 | 1 (tb_idx) | BB_EXIT, BB_END, NP |
| `ExitTb` | 返回执行循环 | 0 | 0 | 1 (val) | BB_EXIT, BB_END, NP |
| `GotoPtr` | 通过寄存器间接跳转 | 0 | 1 | 0 | BB_EXIT, BB_END |
| `Mb` | 内存屏障 | 0 | 0 | 1 (bar_type) | NP |

##### 2.12.1 多线程 vCPU 下的 `ExitTb` 约定

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

#### 2.13 杂项（5 个）

| Opcode | 语义 | Flags |
|--------|------|-------|
| `Call` | 调用辅助函数 | CC, NP |
| `PluginCb` | 插件回调 | NP |
| `PluginMemCb` | 插件内存回调 | NP |
| `Nop` | 空操作 | NP |
| `Discard` | 丢弃 temp | NP |
| `InsnStart` | 客户指令边界标记 | NP |

#### 2.14 32 位宿主兼容（2 个）

| Opcode | 语义 | 固定类型 |
|--------|------|---------|
| `BrCond2I32` | 64 位条件分支（32 位宿主，寄存器对） | I32 |
| `SetCond2I32` | 64 位条件设置（32 位宿主） | I32 |

#### 2.15 向量操作（57 个）

向量 op 全部标记 `VECTOR`，按子类别分组：

**数据移动**（6 个）：`MovVec`, `DupVec`, `Dup2Vec`, `LdVec`,
`StVec`, `DupmVec`

**算术**（12 个）：`AddVec`, `SubVec`, `MulVec`, `NegVec`,
`AbsVec`, `SsaddVec`, `UsaddVec`, `SssubVec`, `UssubVec`,
`SminVec`, `UminVec`, `SmaxVec`, `UmaxVec`

**逻辑**（9 个）：`AndVec`, `OrVec`, `XorVec`, `AndcVec`,
`OrcVec`, `NandVec`, `NorVec`, `EqvVec`, `NotVec`

**移位——立即数**（4 个）：`ShliVec`, `ShriVec`, `SariVec`,
`RotliVec`（1 oarg, 1 iarg, 1 carg=imm）

**移位——标量**（4 个）：`ShlsVec`, `ShrsVec`, `SarsVec`,
`RotlsVec`（1 oarg, 2 iargs）

**移位——向量**（5 个）：`ShlvVec`, `ShrvVec`, `SarvVec`,
`RotlvVec`, `RotrvVec`（1 oarg, 2 iargs）

**比较/选择**（3 个）：
- `CmpVec`：1 oarg, 2 iargs, 1 carg (cond)
- `BitselVec`：1 oarg, 3 iargs — `d = (a & c) | (b & ~c)`
- `CmpselVec`：1 oarg, 4 iargs, 1 carg (cond) —
  `d = (c1 cond c2) ? v1 : v2`

---

### 3. OpFlags 属性标志

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

- **活跃性分析**：`BB_END` 触发全局变量活跃标记；`SIDE_EFFECTS`
  阻止 DCE
- **寄存器分配**：`NOT_PRESENT` 的 op 走专用路径而非通用
  `regalloc_op()`
- **代码生成**：`BB_EXIT` 的 op 由后端直接处理（emit_exit_tb 等）

---

### 4. OpDef 静态表

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

### 5. Op 结构

```rust
pub struct Op {
    pub idx: OpIdx,              // 在 ops 列表中的索引
    pub opc: Opcode,             // 操作码
    pub op_type: Type,           // 多态 op 的实际类型
    pub param1: u8,              // opcode 特定参数
    pub param2: u8,              // opcode 特定参数
    pub life: LifeData,          // 活跃性分析结果
    pub output_pref: [RegSet; 2], // 寄存器分配提示
    pub args: [TempIdx; 10],     // 参数数组
    pub nargs: u8,               // 实际参数数量
}
```

#### 5.1 参数布局

`args[]` 数组按固定顺序排列：

```
args[0 .. nb_oargs]                          -> 输出参数
args[nb_oargs .. nb_oargs+nb_iargs]          -> 输入参数
args[nb_oargs+nb_iargs .. nb_oargs+nb_iargs+nb_cargs]
                                             -> 常量参数
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

#### 5.2 常量参数编码

常量参数（条件码、偏移量、label ID 等）编码为
`TempIdx(raw_value as u32)` 存入 `args[]`，与 QEMU 约定一致。
IR Builder 中通过辅助函数 `carg()` 转换：

```rust
fn carg(val: u32) -> TempIdx { TempIdx(val) }
```

#### 5.3 LifeData

```rust
pub struct LifeData(pub u32);  // 2 bit per arg
```

每个参数占 2 bit：
- bit `n*2`：dead — 该参数在此 op 后不再使用
- bit `n*2+1`：sync — 该参数（全局变量）需要同步回内存

由活跃性分析（`liveness.rs`）填充，供寄存器分配器消费。

---

### 6. IR Builder API

`impl Context` 上的 `gen_*` 方法，将高层操作转换为 `Op` 并追加到
ops 列表。内部通过 `emit_binary()`/`emit_unary()` 等辅助方法
统一构造。

#### 6.1 二元 ALU（1 oarg, 2 iargs）

签名：
`gen_xxx(&mut self, ty: Type, d: TempIdx, a: TempIdx, b: TempIdx)`
`-> TempIdx`

`gen_add`, `gen_sub`, `gen_mul`, `gen_and`, `gen_or`, `gen_xor`,
`gen_shl`, `gen_shr`, `gen_sar`, `gen_rotl`, `gen_rotr`,
`gen_andc`, `gen_orc`, `gen_eqv`, `gen_nand`, `gen_nor`,
`gen_divs`, `gen_divu`, `gen_rems`, `gen_remu`,
`gen_mulsh`, `gen_muluh`,
`gen_clz`, `gen_ctz`

#### 6.2 一元（1 oarg, 1 iarg）

签名：
`gen_xxx(&mut self, ty: Type, d: TempIdx, s: TempIdx) -> TempIdx`

`gen_neg`, `gen_not`, `gen_mov`, `gen_ctpop`

#### 6.3 类型转换（固定类型）

签名：
`gen_xxx(&mut self, d: TempIdx, s: TempIdx) -> TempIdx`

| 方法 | 语义 |
|------|------|
| `gen_ext_i32_i64` | sign-extend i32 -> i64 |
| `gen_ext_u32_i64` | zero-extend i32 -> i64 |
| `gen_extrl_i64_i32` | truncate i64 -> i32 (low) |
| `gen_extrh_i64_i32` | extract i64 -> i32 (high) |

#### 6.4 条件操作

| 方法 | 签名 |
|------|------|
| `gen_setcond` | `(ty, d, a, b, cond) -> d` |
| `gen_negsetcond` | `(ty, d, a, b, cond) -> d` |
| `gen_movcond` | `(ty, d, c1, c2, v1, v2, cond) -> d` |

#### 6.5 位域操作

| 方法 | 签名 |
|------|------|
| `gen_extract` | `(ty, d, src, ofs, len) -> d` |
| `gen_sextract` | `(ty, d, src, ofs, len) -> d` |
| `gen_deposit` | `(ty, d, a, b, ofs, len) -> d` |
| `gen_extract2` | `(ty, d, al, ah, ofs) -> d` |

#### 6.6 字节序交换

签名：
`gen_bswapN(&mut self, ty: Type, d: TempIdx, src: TempIdx,`
`flags: u32) -> TempIdx`

`gen_bswap16`, `gen_bswap32`, `gen_bswap64`

#### 6.7 双宽度运算

| 方法 | 签名 |
|------|------|
| `gen_divs2` | `(ty, dl, dh, al, ah, b)` |
| `gen_divu2` | `(ty, dl, dh, al, ah, b)` |
| `gen_muls2` | `(ty, dl, dh, a, b)` |
| `gen_mulu2` | `(ty, dl, dh, a, b)` |

#### 6.8 进位算术

签名同二元 ALU：
`gen_xxx(&mut self, ty, d, a, b) -> TempIdx`

`gen_addco`, `gen_addci`, `gen_addcio`, `gen_addc1o`,
`gen_subbo`, `gen_subbi`, `gen_subbio`, `gen_subb1o`

#### 6.9 宿主内存访问

**加载**：`gen_ld(&mut self, ty, dst, base, offset) -> TempIdx`
以及 `gen_ld8u`, `gen_ld8s`, `gen_ld16u`, `gen_ld16s`,
`gen_ld32u`, `gen_ld32s`

**存储**：`gen_st(&mut self, ty, src, base, offset)`
以及 `gen_st8`, `gen_st16`, `gen_st32`

#### 6.10 客户内存访问

| 方法 | 签名 |
|------|------|
| `gen_qemu_ld` | `(ty, dst, addr, memop) -> dst` |
| `gen_qemu_st` | `(ty, val, addr, memop)` |
| `gen_qemu_ld2` | `(ty, dl, dh, addr, memop)` |
| `gen_qemu_st2` | `(ty, vl, vh, addr, memop)` |

#### 6.11 控制流

| 方法 | 签名 |
|------|------|
| `gen_br` | `(label_id)` |
| `gen_brcond` | `(ty, a, b, cond, label_id)` |
| `gen_set_label` | `(label_id)` |
| `gen_goto_tb` | `(tb_idx)` |
| `gen_exit_tb` | `(val)` |
| `gen_goto_ptr` | `(ptr)` |
| `gen_mb` | `(bar_type)` |
| `gen_insn_start` | `(pc)` -- 编码为 2 个 cargs (lo, hi) |
| `gen_discard` | `(ty, t)` |

#### 6.12 32 位宿主兼容

| 方法 | 签名 |
|------|------|
| `gen_brcond2_i32` | `(al, ah, bl, bh, cond, label_id)` |
| `gen_setcond2_i32` | `(d, al, ah, bl, bh, cond) -> d` |

#### 6.13 向量操作

**数据移动**：`gen_dup_vec`, `gen_dup2_vec`, `gen_ld_vec`,
`gen_st_vec`, `gen_dupm_vec`

**算术**：`gen_add_vec`, `gen_sub_vec`, `gen_mul_vec`,
`gen_neg_vec`, `gen_abs_vec`, `gen_ssadd_vec`, `gen_usadd_vec`,
`gen_sssub_vec`, `gen_ussub_vec`, `gen_smin_vec`, `gen_umin_vec`,
`gen_smax_vec`, `gen_umax_vec`

**逻辑**：`gen_and_vec`, `gen_or_vec`, `gen_xor_vec`,
`gen_andc_vec`, `gen_orc_vec`, `gen_nand_vec`, `gen_nor_vec`,
`gen_eqv_vec`, `gen_not_vec`

**移位（立即数）**：`gen_shli_vec`, `gen_shri_vec`,
`gen_sari_vec`, `gen_rotli_vec`

**移位（标量）**：`gen_shls_vec`, `gen_shrs_vec`,
`gen_sars_vec`, `gen_rotls_vec`

**移位（向量）**：`gen_shlv_vec`, `gen_shrv_vec`,
`gen_sarv_vec`, `gen_rotlv_vec`, `gen_rotrv_vec`

**比较/选择**：`gen_cmp_vec`, `gen_bitsel_vec`,
`gen_cmpsel_vec`

---

### 7. 与 QEMU 的对比

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

### 8. QEMU 参考映射

| QEMU | machina | 文件 |
|------|---------|------|
| `TCGOpcode` | `enum Opcode` | `core/src/opcode.rs` |
| `TCGOpDef` | `struct OpDef` | `core/src/opcode.rs` |
| `TCG_OPF_*` | `struct OpFlags` | `core/src/opcode.rs` |
| `TCGOp` | `struct Op` | `core/src/op.rs` |
| `TCGLifeData` | `struct LifeData` | `core/src/op.rs` |
| `tcg_gen_op*` | `Context::gen_*` | `core/src/ir_builder.rs` |

---

## Part 2: x86-64 后端参考

### 1. 概述

`accel/src/x86_64/emitter.rs` 实现了 x86-64 宿主架构的完整 GPR
指令编码器，参考 QEMU 的 `tcg/i386/tcg-target.c.inc`。采用分层
编码架构：

```
前缀标志 (P_*) + 操作码常量 (OPC_*)
        |
        v
核心编码函数 (emit_opc / emit_modrm / emit_modrm_offset)
        |
        v
指令发射器 (emit_arith_rr / emit_mov_ri / emit_jcc / ...)
        |
        v
Codegen 分派 (tcg_out_op: IR Opcode --> 指令发射器组合)
        |
        v
X86_64CodeGen (prologue / epilogue / exit_tb / goto_tb)
```

### 2. 编码基础设施

#### 2.1 前缀标志 (P_*)

操作码常量使用 `u32` 类型，高位编码前缀信息：

| 标志 | 值 | 含义 |
|------|-----|------|
| `P_EXT` | 0x100 | 0x0F 转义前缀 |
| `P_EXT38` | 0x200 | 0x0F 0x38 三字节转义 |
| `P_EXT3A` | 0x10000 | 0x0F 0x3A 三字节转义 |
| `P_DATA16` | 0x400 | 0x66 操作数大小前缀 |
| `P_REXW` | 0x1000 | REX.W = 1（64 位操作） |
| `P_REXB_R` | 0x2000 | REG 字段字节寄存器访问 |
| `P_REXB_RM` | 0x4000 | R/M 字段字节寄存器访问 |
| `P_SIMDF3` | 0x20000 | 0xF3 前缀 |
| `P_SIMDF2` | 0x40000 | 0xF2 前缀 |

#### 2.2 操作码常量 (OPC_*)

常量命名遵循 QEMU 的 `tcg-target.c.inc` 风格（使用
`#![allow(non_upper_case_globals)]`）：

```rust
pub const OPC_ARITH_EvIb: u32 = 0x83;
pub const OPC_MOVL_GvEv: u32 = 0x8B;
pub const OPC_JCC_long: u32 = 0x80 | P_EXT;
pub const OPC_BSF: u32 = 0xBC | P_EXT;
pub const OPC_LZCNT: u32 = 0xBD | P_EXT | P_SIMDF3;
```

#### 2.3 核心编码函数

| 函数 | 用途 |
|------|------|
| `emit_opc(buf, opc, r, rm)` | 发射 REX 前缀 + 转义字节 + 操作码 |
| `emit_modrm(buf, opc, r, rm)` | 寄存器-寄存器 ModR/M（mod=11） |
| `emit_modrm_ext(buf, opc, ext, rm)` | 组操作码的 /r 扩展 |
| `emit_modrm_offset(buf, opc, r, base, offset)` | 内存 [base+disp] |
| `emit_modrm_sib(buf, opc, r, base, index, shift, offset)` | SIB 寻址 |
| `emit_modrm_ext_offset(buf, opc, ext, base, offset)` | 组操作码 + 内存 |

### 3. 指令分类

#### 3.1 算术指令

| 函数 | 指令 | 说明 |
|------|------|------|
| `emit_arith_rr(op, rexw, dst, src)` | ADD/SUB/AND/OR/XOR/CMP/ADC/SBB | 寄存器-寄存器 |
| `emit_arith_ri(op, rexw, dst, imm)` | 同上 | 寄存器-立即数（自动选择 imm8/imm32） |
| `emit_arith_mr(op, rexw, base, offset, src)` | 同上 | 内存-寄存器（存储操作） |
| `emit_arith_rm(op, rexw, dst, base, offset)` | 同上 | 寄存器-内存（加载操作） |
| `emit_neg(rexw, reg)` | NEG | 取反 |
| `emit_not(rexw, reg)` | NOT | 按位取反 |
| `emit_inc(rexw, reg)` | INC | 自增 |
| `emit_dec(rexw, reg)` | DEC | 自减 |

`ArithOp` 枚举值对应 x86 的 /r 字段：Add=0, Or=1, Adc=2,
Sbb=3, And=4, Sub=5, Xor=6, Cmp=7。

#### 3.2 移位指令

| 函数 | 指令 | 说明 |
|------|------|------|
| `emit_shift_ri(op, rexw, dst, imm)` | SHL/SHR/SAR/ROL/ROR | 立即数移位（imm=1 使用短编码） |
| `emit_shift_cl(op, rexw, dst)` | 同上 | 按 CL 寄存器移位 |
| `emit_shld_ri(rexw, dst, src, imm)` | SHLD | 双精度左移 |
| `emit_shrd_ri(rexw, dst, src, imm)` | SHRD | 双精度右移 |

#### 3.3 数据移动

| 函数 | 指令 | 说明 |
|------|------|------|
| `emit_mov_rr(rexw, dst, src)` | MOV r, r | 32/64 位寄存器传送 |
| `emit_mov_ri(rexw, reg, val)` | MOV r, imm | 智能选择：xor(0) / mov r32(u32) / mov r64 sign-ext(i32) / movabs(i64) |
| `emit_movzx(opc, dst, src)` | MOVZBL/MOVZWL | 零扩展 |
| `emit_movsx(opc, dst, src)` | MOVSBL/MOVSWL/MOVSLQ | 符号扩展 |
| `emit_bswap(rexw, reg)` | BSWAP | 字节序交换 |

#### 3.4 内存操作

| 函数 | 指令 | 说明 |
|------|------|------|
| `emit_load(rexw, dst, base, offset)` | MOV r, [base+disp] | 加载 |
| `emit_store(rexw, src, base, offset)` | MOV [base+disp], r | 存储 |
| `emit_store_byte(src, base, offset)` | MOV byte [base+disp], r | 字节存储 |
| `emit_store_imm(rexw, base, offset, imm)` | MOV [base+disp], imm32 | 立即数存储 |
| `emit_lea(rexw, dst, base, offset)` | LEA r, [base+disp] | 地址计算 |
| `emit_load_sib(rexw, dst, base, index, shift, offset)` | MOV r, [b+i*s+d] | 索引加载 |
| `emit_store_sib(rexw, src, base, index, shift, offset)` | MOV [b+i*s+d], r | 索引存储 |
| `emit_lea_sib(rexw, dst, base, index, shift, offset)` | LEA r, [b+i*s+d] | 索引地址计算 |
| `emit_load_zx(opc, dst, base, offset)` | MOVZBL/MOVZWL [mem] | 零扩展加载 |
| `emit_load_sx(opc, dst, base, offset)` | MOVSBL/MOVSWL/MOVSLQ [mem] | 符号扩展加载 |

#### 3.5 乘除指令

| 函数 | 指令 | 说明 |
|------|------|------|
| `emit_mul(rexw, reg)` | MUL | 无符号乘法 RDX:RAX = RAX * reg |
| `emit_imul1(rexw, reg)` | IMUL | 有符号乘法（单操作数） |
| `emit_imul_rr(rexw, dst, src)` | IMUL r, r | 双操作数乘法 |
| `emit_imul_ri(rexw, dst, src, imm)` | IMUL r, r, imm | 三操作数乘法 |
| `emit_div(rexw, reg)` | DIV | 无符号除法 |
| `emit_idiv(rexw, reg)` | IDIV | 有符号除法 |
| `emit_cdq()` | CDQ | 符号扩展 EAX -> EDX:EAX |
| `emit_cqo()` | CQO | 符号扩展 RAX -> RDX:RAX |

#### 3.6 位操作

| 函数 | 指令 | 说明 |
|------|------|------|
| `emit_bsf(rexw, dst, src)` | BSF | 位扫描（正向） |
| `emit_bsr(rexw, dst, src)` | BSR | 位扫描（反向） |
| `emit_lzcnt(rexw, dst, src)` | LZCNT | 前导零计数 |
| `emit_tzcnt(rexw, dst, src)` | TZCNT | 尾随零计数 |
| `emit_popcnt(rexw, dst, src)` | POPCNT | 人口计数 |
| `emit_bt_ri(rexw, reg, bit)` | BT | 位测试 |
| `emit_bts_ri(rexw, reg, bit)` | BTS | 位测试并置位 |
| `emit_btr_ri(rexw, reg, bit)` | BTR | 位测试并复位 |
| `emit_btc_ri(rexw, reg, bit)` | BTC | 位测试并取反 |
| `emit_andn(rexw, dst, src1, src2)` | ANDN | BMI1: dst = ~src1 & src2（VEX 编码） |

#### 3.7 分支与比较

| 函数 | 指令 | 说明 |
|------|------|------|
| `emit_jcc(cond, target)` | Jcc rel32 | 条件跳转 |
| `emit_jmp(target)` | JMP rel32 | 无条件跳转 |
| `emit_call(target)` | CALL rel32 | 函数调用 |
| `emit_jmp_reg(reg)` | JMP *reg | 间接跳转 |
| `emit_call_reg(reg)` | CALL *reg | 间接调用 |
| `emit_setcc(cond, dst)` | SETcc | 条件置字节 |
| `emit_cmovcc(cond, rexw, dst, src)` | CMOVcc | 条件传送 |
| `emit_test_rr(rexw, r1, r2)` | TEST r, r | 按位与测试 |
| `emit_test_bi(reg, imm)` | TEST r8, imm8 | 字节测试 |

#### 3.8 杂项

| 函数 | 指令 | 说明 |
|------|------|------|
| `emit_xchg(rexw, r1, r2)` | XCHG | 交换 |
| `emit_push(reg)` | PUSH | 压栈 |
| `emit_pop(reg)` | POP | 出栈 |
| `emit_push_imm(imm)` | PUSH imm | 立即数压栈 |
| `emit_ret()` | RET | 返回 |
| `emit_mfence()` | MFENCE | 内存屏障 |
| `emit_ud2()` | UD2 | 未定义指令（调试陷阱） |
| `emit_nops(n)` | NOP | Intel 推荐的多字节 NOP（1-8 字节） |

### 4. 内存寻址特殊情况

x86-64 ModR/M 编码有两个特殊寄存器需要额外处理：

- **RSP/R12（low3=4）**：作为基址时必须使用 SIB 字节
  （`0x24` = index=RSP/none, base=RSP）
- **RBP/R13（low3=5）**：作为基址且偏移为 0 时，必须使用
  `mod=01, disp8=0`（因为 `mod=00, rm=5` 被编码为 RIP
  相对寻址）

`emit_modrm_offset` 自动处理这些特殊情况。

### 5. 条件码映射

`X86Cond` 枚举映射 TCG 条件到 x86 JCC 条件码：

| TCG Cond | X86Cond | JCC 编码 |
|----------|---------|----------|
| Eq / TstEq | Je | 0x4 |
| Ne / TstNe | Jne | 0x5 |
| Lt | Jl | 0xC |
| Ge | Jge | 0xD |
| Ltu | Jb | 0x2 |
| Geu | Jae | 0x3 |

`X86Cond::invert()` 通过翻转低位实现条件取反（如 Je <-> Jne）。

### 6. 约束表 (`constraints.rs`)

`op_constraint()` 为每个 opcode 返回静态 `OpConstraint`，对齐
QEMU 的 `tcg_target_op_def()`（`tcg/i386/tcg-target.c.inc`）。

| Opcode | 约束 | QEMU 等价 | 说明 |
|--------|------|-----------|------|
| Add | `o1_i2(R, R, R)` | `C_O1_I2(r,r,re)` | 三地址 LEA |
| Sub | `o1_i2_alias(R, R, R)` | `C_O1_I2(r,0,re)` | 破坏性 SUB，dst==lhs |
| Mul | `o1_i2_alias(R, R, R)` | `C_O1_I2(r,0,r)` | IMUL 二地址 |
| And/Or/Xor | `o1_i2_alias(R, R, R)` | `C_O1_I2(r,0,re)` | 破坏性二元运算 |
| Neg/Not | `o1_i1_alias(R, R)` | `C_O1_I1(r,0)` | 原地一元运算 |
| Shl/Shr/Sar/RotL/RotR | `o1_i2_alias_fixed(R_NO_RCX, R_NO_RCX, RCX)` | `C_O1_I2(r,0,ci)` | 别名 + count 固定 RCX，R_NO_RCX 排除 RCX 防冲突 |
| SetCond/NegSetCond | `n1_i2(R, R, R)` | `C_N1_I2(r,r,re)` | newreg（setcc 只写低字节） |
| MovCond | `o1_i4_alias2(R, R, R, R, R)` | `C_O1_I4(r,r,r,0,r)` | 输出别名 input2（CMP+CMOV） |
| BrCond | `o0_i2(R, R)` | `C_O0_I2(r,re)` | 无输出 |
| MulS2/MulU2 | `o2_i2_fixed(RAX, RDX, R_NO_RAX_RDX)` | `C_O2_I2(r,r,0,r)` | 双固定输出，R_NO_RAX_RDX 排除 RAX/RDX 防冲突 |
| DivS2/DivU2 | `o2_i3_fixed(RAX, RDX, R_NO_RAX_RDX)` | `C_O2_I3(r,r,0,1,r)` | 双固定输出+双别名，R_NO_RAX_RDX 排除 RAX/RDX |
| AddCO/AddCI/AddCIO/AddC1O | `o1_i2_alias(R, R, R)` | -- | 进位算术，破坏性 |
| SubBO/SubBI/SubBIO/SubB1O | `o1_i2_alias(R, R, R)` | -- | 借位算术，破坏性 |
| AndC | `o1_i2(R, R, R)` | -- | 三地址 ANDN (BMI1) |
| Extract/SExtract | `o1_i1(R, R)` | -- | 位域提取 |
| Deposit | `o1_i2_alias(R, R, R)` | -- | 位域插入，破坏性 |
| Extract2 | `o1_i2_alias(R, R, R)` | -- | 双寄存器提取 (SHRD) |
| Bswap16/32/64 | `o1_i1_alias(R, R)` | -- | 字节交换，原地 |
| Clz/Ctz | `n1_i2(R, R, R)` | -- | 位计数 + fallback |
| CtPop | `o1_i1(R, R)` | -- | 人口计数 |
| ExtrhI64I32 | `o1_i1_alias(R, R)` | -- | 高 32 位提取 |
| Ld/Ld* | `o1_i1(R, R)` | -- | 无别名 |
| St/St* | `o0_i2(R, R)` | -- | 无输出 |
| GotoPtr | `o0_i1(R)` | -- | 间接跳转 |

其中 `R = ALLOCATABLE_REGS`（14 个 GPR，排除 RSP 和 RBP），
`R_NO_RCX = R & ~{RCX}`，
`R_NO_RAX_RDX = R & ~{RAX, RDX}`。

约束保证使 codegen 可以假设：
- 破坏性运算的 `oregs[0] == iregs[0]`（无需 mov 前置）
- 移位的 `iregs[1] == RCX`（无需 push/pop RCX 杂耍）
- 移位的 output/input0 不在 RCX（R_NO_RCX 排除）
- MulS2/DivS2 的自由 input 不在 RAX/RDX（R_NO_RAX_RDX 排除）
- SetCond 的输出不与任何输入重叠

### 7. Codegen 分派 (`codegen.rs`)

`tcg_out_op` 是寄存器分配器与指令编码器之间的桥梁。它接收已分配
宿主寄存器的 IR op，将其翻译为一个或多个 x86-64 指令。

#### 7.1 HostCodeGen 寄存器分配器原语

| 方法 | 用途 |
|------|------|
| `tcg_out_mov(ty, dst, src)` | 寄存器间传送 |
| `tcg_out_movi(ty, dst, val)` | 加载立即数到寄存器 |
| `tcg_out_ld(ty, dst, base, offset)` | 从内存加载（全局变量 reload） |
| `tcg_out_st(ty, src, base, offset)` | 存储到内存（全局变量 sync） |

#### 7.2 IR Opcode -> x86-64 指令映射

约束系统保证 codegen 收到的寄存器满足指令需求，因此每个 opcode
只需发射最简指令序列：

| IR Opcode | x86-64 指令 | 约束保证 |
|-----------|------------|---------|
| Add | d==a: `add d,b`; d==b: `add d,a`; else: `lea d,[a+b]` | 三地址，无别名 |
| Sub | `sub d,b` | d==a (oalias) |
| Mul | `imul d,b` | d==a (oalias) |
| And/Or/Xor | `op d,b` | d==a (oalias) |
| Neg/Not | `neg/not d` | d==a (oalias) |
| Shl/Shr/Sar/RotL/RotR | `shift d,cl` | d==a (oalias), count==RCX (fixed) |
| SetCond | `cmp a,b; setcc d; movzbl d,d` | d!=a, d!=b (newreg) |
| NegSetCond | `cmp a,b; setcc d; movzbl d,d; neg d` | d!=a, d!=b (newreg) |
| MovCond | `cmp a,b; cmovcc d,v2` | d==v1 (oalias input2) |
| BrCond | `cmp a,b; jcc label` | 无输出 |
| MulS2/MulU2 | `mul/imul b` (RAX implicit) | o0=RAX, o1=RDX (fixed) |
| DivS2/DivU2 | `cqo/xor; div/idiv b` | o0=RAX, o1=RDX (fixed) |
| AddCO/SubBO | `add/sub d,b` (sets CF) | d==a (oalias) |
| AddCI/SubBI | `adc/sbb d,b` (reads CF) | d==a (oalias) |
| AddCIO/SubBIO | `adc/sbb d,b` (reads+sets CF) | d==a (oalias) |
| AddC1O/SubB1O | `stc; adc/sbb d,b` | d==a (oalias) |
| AndC | `andn d,b,a` (BMI1) | 三地址 |
| Extract/SExtract | `shr`+`and` / `movzx` / `movsx` | -- |
| Deposit | `and`+`or` 组合 | d==a (oalias) |
| Extract2 | `shrd d,b,imm` | d==a (oalias) |
| Bswap16/32/64 | `ror`/`bswap` | d==a (oalias) |
| Clz/Ctz | `lzcnt`/`tzcnt` | d!=a (newreg) |
| CtPop | `popcnt d,a` | -- |
| ExtrhI64I32 | `shr d,32` | d==a (oalias) |
| Ld/Ld* | `mov d,[base+offset]` | -- |
| St/St* | `mov [base+offset],s` | -- |
| ExitTb | `mov rax,val; jmp tb_ret` | -- |
| GotoTb | `jmp rel32` (可修补) | -- |
| GotoPtr | `jmp *reg` | -- |

#### 7.3 SetCond/BrCond 的 TstEq/TstNe 支持

当条件码为 `TstEq` 或 `TstNe` 时，使用 `test a,b`（按位与测试）
代替 `cmp a,b`（减法比较）。这对应 QEMU 7.x+ 新增的
test-and-branch 优化。

### 8. QEMU 参考对照

| machina 函数 | QEMU 函数 |
|-------------|-----------|
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

## Part 3: 设备模型参考

### 1. 范围

本文档说明 Machina 当前第一阶段设备模型如何对齐 QEMU 的
object/qdev/sysbus 方向，并统一使用 Machina 自身术语：
`MOM`、`mobject`、`mdev`、`sysbus`。

这一轮是对旧薄壳 qdev/sysbus 结构的直接替换。除代码里暂时保留的
qdev bridge 外，不再提供额外兼容层或迁移层。

当前第一阶段 MOM 覆盖范围包括：

- 根对象层（`mobject`）
- 设备层（`mdev`）
- 可执行的 sysbus realize / unrealize
- 轻量属性表面
- 已迁移的平台设备：UART、PLIC、ACLINT、virtio-mmio

### 2. 分层

#### 2.1 `mobject`

`mobject` 是所有权和身份标识的基础层。

- 位于 `machina-core`
- 为受管对象提供 local ID 和 object path
- 强制父子严格树结构
- 也是 `Machine` 进入对象树的基础

#### 2.2 `mdev`

`mdev` 是建立在 `mobject` 之上的公共设备生命周期层。

- 位于 `machina-hw-core`
- 负责跟踪 `realize` / `unrealize`
- 拒绝非法的 realize 后结构性修改
- 为已迁移设备提供统一错误语义

#### 2.3 `sysbus`

`sysbus` 是可执行装配层，而不只是元数据。

- 设备在 realize 前必须先 attach 到 bus
- 设备在 realize 前必须先注册 MMIO region
- realize 会校验重叠并把 region 映射进 `AddressSpace`
- unrealize 会把已实现映射从 `AddressSpace` 和 bus 记录里移除

#### 2.4 属性

当前 MOM 第一阶段使用轻量、强类型的属性层。

- 属性 schema 在 realize 前定义
- required/default 语义显式化
- static / dynamic 可变性边界显式化
- UART 的 `chardev` 就通过这层作为标准属性暴露

### 3. 设备生命周期

已迁移设备的生命周期为：

1. 创建设备对象
2. attach 到 `sysbus`
3. 注册 MMIO 和设备特定运行时接线输入
4. 应用 realize 前属性
5. realize 到 `AddressSpace`
6. reset 仅重置运行时状态，不重建拓扑
7. unrealize 时先拆运行时状态，再移除已实现映射

核心规则是：结构性拓扑只创建一次，并跨 reset 保持稳定。reset
不能隐式重建拓扑。

### 4. 第一阶段已迁移设备

#### 4.1 UART

- 持有 `SysBusDeviceState`
- 通过标准属性暴露 `chardev`
- 在 `realize` 时安装 frontend 运行时接线
- 在 `unrealize` 时同时拆运行时接线和 MMIO 映射

#### 4.2 PLIC

- 持有 `SysBusDeviceState`
- context output 路由仍保持为设备特定运行时接线
- reset 仅重置运行时状态，不重建 sysbus 拓扑

#### 4.3 ACLINT

- 持有 `SysBusDeviceState`
- MTI/MSI 和 WFI-waker 保持为设备特定运行时接线
- reset / unrealize 时清理定时器状态，但不重建拓扑

#### 4.4 virtio-mmio

- MOM/sysbus 设备是 MMIO transport 本身
- block backend 仍保持为 transport 内部关系
- transport 自己拥有 guest RAM 访问、MMIO 状态和 IRQ 传递

这样可以明确 transport/proxy 边界，并为后续更复杂的 backend
关系预留扩展空间，而不会把它们和 machine assembly 混在一起。

### 5. `RefMachine` 装配规则

`RefMachine` 是第一台完整遵守 MOM 装配规则的机器。

- UART、PLIC、ACLINT、virtio-mmio 都作为 MOM 设备创建
- 它们统一通过 `sysbus` attach 和 realize
- realized mapping 统一通过 `SysBus::mappings()` 暴露
- migrated set 的 FDT node name 和 `reg` 字段从 realized
  sysbus mapping 派生

对于已迁移设备集合，realized `sysbus` mapping 就是 machine 侧
拓扑的单一事实来源。

### 6. 测试与防回退护栏

共享 `tests` crate 当前覆盖：

- 对象挂接和生命周期顺序
- MMIO 只有在 realize 后才可见
- UART、PLIC、ACLINT、virtio-mmio 的客户可见行为
- sysbus unrealize / unmap 行为
- machine 侧 migrated owner 集合
- 防止回退到 direct root MMIO wiring 的源码级检查

### 7. 未来扩展点

当前设计明确保留了以下扩展点：

- PCI 和非 sysbus transport
- 支持 hotplug 的生命周期扩展
- 更丰富的对象/属性 introspection
- transport device 和 backend device 之间更正式的父子关系

这些是未来扩展方向，不属于 v1 承诺。

---

## Part 4: 性能分析

本文档总结 machina JIT 引擎相比 QEMU TCG 的独有性能优化手段，
并分析全系统模式下的性能特征。

### 1. 执行循环优化

#### 1.1 `next_tb_hint` -- 跳过 TB 查找

**文件**: `accel/src/exec/exec_loop.rs:52-89`

当 TB 通过 `goto_tb` 链式退出时，machina 将目标 TB 索引存入
`next_tb_hint`。下一轮循环直接复用该索引，完全跳过 jump cache
和全局 hash 查找。

| | machina | QEMU |
|---|--------|------|
| 链式退出后 | 直接复用目标 TB | 仍走 `tb_lookup` 完整路径 |
| 热循环开销 | 接近零（索引比较） | jump cache hash + 比较 |

QEMU 的 `last_tb` 仅用于决定是否 patch 链接，不跳过查找。
在紧密循环（如 dhrystone 主循环）中，hint 命中率极高。

#### 1.2 `exit_target` 原子缓存 -- 间接跳转加速

**文件**: `accel/src/exec/exec_loop.rs:96-116`, `core/src/tb.rs:55`

对 `TB_EXIT_NOCHAIN`（间接跳转、`jalr` 等），每个 TB 维护一个
`AtomicUsize` 单项缓存，记录上次跳转的目标 TB。

```
间接跳转退出 --> 检查 exit_target 缓存
                  |-- 命中且有效 --> 直接复用，跳过 hash 查找
                  +-- 未命中 --> 走正常 tb_find，更新缓存
```

QEMU 对所有 `TB_EXIT_NOCHAIN` 都走完整的 QHT 查找路径，没有这层
缓存。两个优化组合后，稳态执行中全局 hash 查找几乎只在冷启动和
TB 失效时触发。

**估算贡献**: ~8-10%

### 2. Guest 内存访问优化

#### 2.1 直接 guest_base 寻址（原 linux-user 优化）

**文件**: `accel/src/x86_64/codegen.rs:573-639`

> **注意**: 本节描述的无软件 TLB 直接寻址优化是早期 linux-user
> 模式的专属方案。全系统模式使用 Sv39 MMU 页表翻译 + 软件 TLB，
> 不再适用此路径。

在早期 linux-user 模式中，guest 内存访问直接生成
`[R14 + addr]` 寻址（R14 = guest_base），无 TLB 查找、无慢速路径
helper 调用。

| | machina (直接寻址) | QEMU |
|---|--------|------|
| load/store 生成 | `mov reg, [R14+addr]` | 内联 TLB 快速路径 + 慢速路径分支 |
| 每次访问指令数 | 1-2 条 | 5-10 条（TLB 查找 + 比较 + 分支） |
| 慢速路径 | 无 | helper 函数调用 |

QEMU 即使在 linux-user 模式下也生成完整的软件 TLB 路径，因为其
`tcg_out_qemu_ld`/`tcg_out_qemu_st` 不区分系统模式和用户模式。

在全系统模式中，machina 采用 Sv39 MMU 页表翻译配合软件 TLB
快速路径，此时内存访问开销与 QEMU 相当，不再享有直接寻址的优势。

**估算贡献**: 仅适用于直接寻址场景，约 ~8-10%

### 3. 数据结构优化

#### 3.1 Vec-based IR 存储 vs QEMU 链表

**文件**: `core/src/context.rs:18-73`

| | machina | QEMU |
|---|--------|------|
| Op 存储 | `Vec<Op>` 连续内存 | `QTAILQ` 双向链表 |
| Temp 存储 | `Vec<Temp>` 连续内存 | 数组（固定上限） |
| 遍历模式 | 顺序索引，缓存预取友好 | 指针追踪，cache miss 多 |
| 预分配 | ops=512, temps=256, labels=32 | 动态 malloc |

优化器遍历、liveness 分析、寄存器分配都需要顺序扫描全部 ops，
Vec 的缓存行预取优势在这些阶段显著。预分配容量避免了翻译期间
的 realloc。

#### 3.2 HashMap 常量去重 vs 线性扫描

**文件**: `core/src/context.rs:128-138`

machina 用按类型分桶的 `HashMap<u64, TempIdx>` 做常量去重，
O(1) 查找。QEMU 的 `tcg_constant_internal` 线性扫描
`nb_temps`，大型 TB 中常量查找是隐性开销。

#### 3.3 `#[repr(u8)]` 紧凑枚举

**文件**: `core/src/opcode.rs`

`Opcode` 枚举用 `#[repr(u8)]` 标注，占 1 字节。QEMU 的
`TCGOpcode` 是 `int`（4 字节）。`Op` 结构体更紧凑，单个缓存行
容纳更多 ops。

**估算贡献**: ~3-5%

### 4. 运行时并发优化

#### 4.1 Lock-free TB 读取

**文件**: `accel/src/exec/tb_store.rs:13-64`

TbStore 利用 TB 只追加不删除的特性，用
`UnsafeCell<Vec<TB>>` + `AtomicUsize` 长度实现无锁读取。

```
写入路径（翻译）: translate_lock --> push TB
                   --> Release store len
读取路径（执行）: Acquire load len --> 索引访问（无锁）
```

QEMU 的 QHT 使用 RCU 机制，有额外的 grace period 和
synchronize 开销。machina 的方案更简单，利用了 TB 只追加的
不变量。

#### 4.2 RWX 代码缓冲区 -- 无 mprotect 切换

**文件**: `accel/src/code_buffer.rs:38-49`

machina 直接 mmap RWX 内存，TB 链接 patch 时无需 mprotect
切换。QEMU 在启用 split-wx 模式时（某些发行版默认开启），每次
patch 需要 mprotect 系统调用。

#### 4.3 简化哈希函数

**文件**: `core/src/tb.rs:106-109`

```rust
let h = pc.wrapping_mul(0x9e3779b97f4a7c15) ^ (flags as u64);
(h as usize) & (TB_HASH_SIZE - 1)
```

黄金比例常数乘法哈希，计算量比 QEMU 的 xxHash 更小。TB 查找
热路径上每次省几个 cycle，累积效果可观。

**估算贡献**: ~2-3%

### 5. 编译管线优化

#### 5.1 单遍 IR 优化器

**文件**: `accel/src/optimize.rs`

| | machina | QEMU |
|---|--------|------|
| 遍数 | 单遍 O(n) | 多遍扫描 |
| 常量折叠 | 完整值级别 | 位级（z_mask/o_mask/s_mask） |
| 拷贝传播 | 基础 | 高级 |
| 代数简化 | 基础恒等式 | 复杂模式匹配 |

machina 的优化深度不如 QEMU，但翻译速度更快。对大量短 TB 的翻译，
单遍设计的编译时间优势明显。

#### 5.2 Rust 零成本抽象

- **单态化**: 前端 `BinOp` 函数指针
  （`frontend/src/riscv/trans.rs:26`）经编译器单态化后内联，
  消除间接调用
- **内联标注**: `CodeBuffer` 的 14 个 `#[inline]` 字节发射函数
  （`accel/src/code_buffer.rs`）被内联到 codegen 调用点
- **枚举判别式**: `#[repr(u8)]` 生成紧凑跳转表

**估算贡献**: ~2-3%

### 6. 指令选择优化

#### 6.1 LEA 三地址加法

**文件**: `accel/src/x86_64/codegen.rs:136-147`

当 `Add` 的输出寄存器与两个输入都不同时，使用 LEA 实现非破坏性
三地址加法，避免额外 MOV。QEMU 也有此优化。

#### 6.2 无条件 BMI1 指令

**文件**: `accel/src/x86_64/emitter.rs:57-61`

machina 无条件使用 ANDN/LZCNT/TZCNT/POPCNT。QEMU 运行时检测
CPU 特性后才决定是否使用，检测本身有微小开销，且 fallback 路径
更长。

#### 6.3 MOV 立即数分级优化

**文件**: `accel/src/x86_64/emitter.rs:547-566`

```
val == 0        --> XOR reg, reg       (2 bytes, 破坏依赖链)
val <= u32::MAX --> MOV r32, imm32     (5 bytes, 零扩展)
val fits i32    --> MOV r64, sign-ext   (7 bytes)
otherwise       --> MOV r64, imm64     (10 bytes)
```

### 7. 全系统模式性能特征

全系统模式引入了额外的性能开销，以下是主要影响因素：

#### 7.1 MMU 页表翻译开销

全系统模式采用 Sv39 三级页表翻译。每次 guest 内存访问需要：

1. 软件 TLB 快速路径查找（内联代码，~5-10 条宿主指令）
2. TLB 未命中时的页表遍历（3 级查找，每级一次内存读取）
3. 权限检查（读/写/执行、U/S 模式、MXR/SUM 位）

TLB 命中率是全系统模式性能的关键指标。稳态运行时 TLB 命中率
通常 >95%，页表遍历开销被摊薄。

#### 7.2 MMIO 分派开销

设备 MMIO 访问走独立的分派路径，不经过 TLB 快速路径：

```
guest load/store --> TLB 查找
                      |-- 普通内存 --> 快速路径直接访问
                      +-- MMIO 区域 --> AddressSpace 分派
                                          --> 设备 read/write
                                              回调
```

MMIO 分派涉及地址空间树查找和设备回调的间接调用，开销比普通内存
访问高 1-2 个数量级。设备交互密集的工作负载（如大量串口 I/O）
受此影响较大。

#### 7.3 特权级切换

全系统模式需要处理 M/S/U 特权级切换、中断和异常，每次切换涉及
CSR 更新和 TB 失效。频繁的特权级切换（如高频 timer 中断）会降低
TB 缓存命中率。

### 8. 性能贡献总览

| 优化类别 | 估算贡献 | 关键技术 |
|---------|---------|---------|
| 执行循环（hint + exit_target） | ~8-10% | 跳过 TB 查找 |
| 数据结构（Vec + 紧凑枚举） | ~3-5% | 缓存友好布局 |
| 运行时并发（lock-free + RWX） | ~2-3% | 无锁读取、无 mprotect |
| 编译管线（单遍 + 内联） | ~2-3% | Rust 零成本抽象 |
| 哈希 + 常量去重 | ~1-2% | 简化计算 |

> 注: 直接 guest_base 寻址（~8-10%）仅适用于早期 linux-user
> 模式，全系统模式不适用。

### 9. 权衡与局限

machina 的性能优势建立在以下权衡之上：

- **RWX 内存**: 违反 W^X 安全原则，某些平台（iOS）禁止
- **简化优化器**: 缺少 QEMU 的位级追踪，生成代码质量略低
- **无条件 BMI1**: 假设宿主 CPU 支持，不兼容老旧 CPU
- **简化哈希**: 分布质量不如 xxHash，高冲突率下退化
- **全系统 MMU 开销**: Sv39 页表翻译引入额外内存访问延迟，
  TLB 未命中代价高
- **MMIO 分派**: 设备访问走间接回调路径，延迟不可忽略

这些权衡在全系统 RISC-V 仿真 + 现代 x86-64 宿主的目标场景下
是合理的。

---

## Part 5: 测试架构

### 1. 概述

Machina 采用分层测试策略，从底层数据结构到完整的全系统模拟器，
逐层验证正确性。测试统一集中在独立的 `tests/` crate 中，保持
源码文件干净，同时验证公共 API 的完整性。

**测试金字塔**：

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

**总计：1425 个测试**。

---

### 3. 测试架构

#### 目录结构

```
tests/
+-- Cargo.toml
+-- src/
|   +-- lib.rs                    # 37 个模块声明
|   +-- core.rs                   # 核心 IR 单元测试 (219)
|   +-- core_address.rs           # 地址类型测试 (5)
|   +-- backend/                  # 后端单元测试 (277)
|   +-- decode/                   # 解码器生成器测试 (93)
|   +-- frontend/                 # 前端指令测试
|   |   +-- mod.rs                #   RV32I/RV64I/RVC/RV32F (116)
|   |   +-- difftest.rs           #   machina vs QEMU (35)
|   |   +-- riscv_zba.rs          #   Zba 扩展 (17)
|   |   +-- riscv_zbb.rs          #   Zbb 扩展 (34)
|   |   +-- riscv_zbc.rs          #   Zbc 扩展 (22)
|   |   +-- riscv_zbs.rs          #   Zbs 扩展 (28)
|   +-- integration/              # 集成测试 (105)
|   +-- exec/                     # 执行循环测试 (31)
|   +-- softmmu.rs                # 软件 MMU 测试 (28)
|   +-- softmmu_exec.rs           # SoftMMU 执行测试 (11)
|   +-- softfloat.rs              # IEEE 754 测试 (62)
|   +-- gdbstub.rs                # GDB 协议测试 (57)
|   +-- disas_bitmanip.rs         # 反汇编器测试 (43)
|   +-- monitor.rs                # 控制台测试 (20)
|   +-- hw_*.rs                   # 硬件设备测试 (108)
|   +-- virtio.rs                 # VirtIO 核心测试 (16)
|   +-- virtio_net.rs             # VirtIO 网络测试 (28)
|   +-- riscv_*.rs                # RISC-V 子系统测试 (38)
|   +-- system_cpu_manager.rs     # CPU 管理器测试 (6)
|   +-- ...                       # 其他模块
+-- mtest/                        # mtest 测试固件
    +-- Makefile
    +-- src/
        +-- uart_echo.S           # UART 回环测试
        +-- timer_irq.S           # Timer 中断测试
        +-- boot_hello.S          # 最小引导测试
```

#### 模块测试分布

| 模块 | 测试数 | 占比 | 说明 |
|------|--------|------|------|
| backend | 277 | 19.4% | x86-64 指令编码、代码缓冲区 |
| core | 224 | 15.7% | IR 类型、Opcode、Temp、Label、Op、Context、Address |
| frontend | 217 | 15.2% | RISC-V 指令执行（RV32I/RV64I/RVC/RV32F/Zb*，不含 difftest） |
| hw_* | 108 | 7.6% | 设备模型：PLIC、ACLINT、UART、QDev、SysBus、FDT |
| integration | 105 | 7.4% | IR --> codegen --> 执行全流水线 |
| decode | 93 | 6.5% | .decode 解析、代码生成、字段提取 |
| 其他 | 91 | 6.4% | 定时器、CLI netdev、内存区域、控制台、softmmu、CPU 管理器、工具、跟踪 |
| softfloat | 62 | 4.4% | IEEE 754 浮点运算 |
| gdbstub | 57 | 4.0% | GDB 远程协议处理 |
| virtio | 44 | 3.1% | VirtIO MMIO 传输、块和网络设备 |
| disas_bitmanip | 43 | 3.0% | 反汇编器和位操作测试 |
| riscv_* | 38 | 2.7% | CSR、MMU、PMP、异常处理 |
| difftest | 35 | 2.5% | machina vs QEMU 差分对比 |
| exec | 31 | 2.2% | TB 缓存、执行循环、多 vCPU |

---

### 4. 单元测试

#### 4.1 Core 模块（224 tests）

验证 IR 基础数据结构的正确性。

| 文件 | 测试内容 |
|------|----------|
| `types.rs` | Type 枚举（I32/I64/I128/V64/V128/V256）、MemOp 位域 |
| `opcode.rs` | Opcode 属性（flags、参数数量、类型约束） |
| `temp.rs` | Temp 创建（global/local/const/fixed）、TempKind 分类 |
| `label.rs` | Label 创建与引用计数 |
| `op.rs` | Op 构造、参数访问、链表操作 |
| `context.rs` | Context 生命周期、temp 分配、op 发射 |
| `regset.rs` | RegSet 位图操作（insert/remove/contains/iter） |
| `tb.rs` | TranslationBlock 创建与缓存 |

```bash
cargo test -p machina-tests core::
```

#### 4.2 Backend 模块（277 tests）

验证 x86-64 指令编码器的正确性。

| 文件 | 测试内容 |
|------|----------|
| `code_buffer.rs` | 代码缓冲区分配、写入、mprotect 切换 |
| `x86_64.rs` | 全部 x86-64 指令编码（MOV/ADD/SUB/AND/OR/XOR/SHL/SHR/SAR/MUL/DIV/LEA/Jcc/SETcc/CMOVcc/BSF/BSR/LZCNT/TZCNT/POPCNT 等） |

```bash
cargo test -p machina-tests backend::
```

#### 4.3 Decodetree 模块（93 tests）

验证 `.decode` 文件解析器和代码生成器。

| 测试分组 | 数量 | 说明 |
|----------|------|------|
| Helper 函数 | 6 | is_bit_char, is_bit_token, is_inline_field, count_bit_tokens, to_camel |
| Bit-pattern 解析 | 4 | 固定位、don't-care、内联字段、超宽模式 |
| Field 解析 | 5 | 无符号/有符号/多段/函数映射/错误处理 |
| ArgSet 解析 | 4 | 普通/空/extern/非 extern |
| 续行与分组 | 4 | 反斜杠续行、花括号/方括号分组 |
| 完整解析 | 5 | mini decode、riscv32、空输入、纯注释、未知格式引用 |
| 格式继承 | 2 | args/fields 继承、bits 合并 |
| Pattern masks | 4 | R/I/B/Shift 类型掩码 |
| 字段提取 | 15 | 32-bit 寄存器/立即数 + 16-bit RVC 字段 |
| Pattern 匹配 | 18 | 32-bit 指令匹配 + 11 条 RVC 指令匹配 |
| 代码生成 | 9 | mini/riscv32/ecall/fence/16-bit 生成 |
| 函数处理器 | 3 | rvc_register, shift_2, sreg_register |
| 16-bit decode | 2 | insn16.decode 解析与生成 |
| 代码质量 | 2 | 无 u32 泄漏、trait 方法无重复 |

```bash
cargo test -p machina-tests decode::
```

---

### 5. 集成测试（105 tests）

**源文件**：`tests/src/integration/mod.rs`

验证完整的 IR --> liveness --> register allocation --> codegen -->
执行流水线。使用最小 RISC-V CPU 状态，通过宏批量生成测试用例。

**测试宏**：

| 宏 | 用途 |
|----|------|
| `riscv_bin_case!` | 二元算术运算（add/sub/and/or/xor） |
| `riscv_shift_case!` | 移位操作（shl/shr/sar/rotl/rotr） |
| `riscv_setcond_case!` | 条件设置（eq/ne/lt/ge/ltu/geu） |
| `riscv_branch_case!` | 条件分支（taken/not-taken） |
| `riscv_mem_case!` | 内存访问（load/store 各宽度） |

**覆盖范围**：ALU、移位、比较、分支、内存读写、位操作、
旋转、字节交换、popcount、乘除法、进位/借位、条件移动等。

```bash
cargo test -p machina-tests integration::
```

---

### 6. 前端指令测试（217 tests）

**源文件**：`tests/src/frontend/mod.rs`

#### 6.1 测试运行器

前端测试使用四个运行器函数，覆盖不同的指令格式：

| 函数 | 输入 | 用途 |
|------|------|------|
| `run_rv(cpu, insn: u32)` | 单条 32-bit 指令 | 基础指令测试 |
| `run_rv_insns(cpu, &[u32])` | 32-bit 指令序列 | 多指令序列 |
| `run_rv_bytes(cpu, &[u8])` | 原始字节流 | 混合 16/32-bit |
| `run_rvc(cpu, insn: u16)` | 单条 16-bit 指令 | RVC 压缩指令 |

**执行流程**（以 `run_rv_insns` 为例）：

```
指令编码 --> 写入 guest 内存
--> translator_loop 解码 --> IR 生成 --> liveness
--> regalloc --> x86-64 codegen --> 执行生成代码
--> 读取 CPU 状态 --> 断言验证
```

#### 6.2 RV32I / RV64I 测试

| 类别 | 指令 | 测试数 |
|------|------|--------|
| 上部立即数 | lui, auipc | 3 |
| 跳转 | jal, jalr | 2 |
| 分支 | beq, bne, blt, bge, bltu, bgeu | 12 |
| 立即数算术 | addi, slti, sltiu, xori, ori, andi | 8 |
| 移位 | slli, srli, srai | 3 |
| 寄存器算术 | add, sub, sll, srl, sra, slt, sltu, xor, or, and | 10 |
| W-suffix | addiw, slliw, srliw, sraiw, addw, subw, sllw, srlw, sraw | 10 |
| 系统 | fence, ecall, ebreak | 3 |
| 特殊 | x0 写忽略, x0 读零 | 2 |
| 多指令 | addi+addi 序列, lui+addi 组合 | 2 |

---

### 7. 差分测试（35 tests）

**源文件**：`tests/src/frontend/difftest.rs`

差分测试对同一条 RISC-V 指令，分别通过 machina 全流水线和
QEMU 参考实现执行，比较 CPU 状态。如果结果一致，则认为
machina 的翻译是正确的。

**依赖工具**：

| 工具 | 安装命令 |
|------|----------|
| `riscv64-linux-gnu-gcc` | `apt install gcc-riscv64-linux-gnu` |
| `qemu-riscv64` | `apt install qemu-user` |

#### 7.1 整体架构

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

#### 7.2 QEMU 侧原理

对每个测试用例，框架动态生成一段 RISC-V 汇编源码：

```asm
.global _start
_start:
    la gp, save_area       # x3 = 保存区基址

    # -- Phase 1: 加载初始寄存器值 --
    li t0, <val1>
    li t1, <val2>

    # -- Phase 2: 执行被测指令 --
    add t2, t0, t1

    # -- Phase 3: 保存全部 32 个寄存器 --
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
save_area: .space 256       # 32 x 8 字节
```

编译与执行流程：

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

临时文件使用 `pid_tid` 命名避免并行测试冲突，执行完毕后
自动清理。

分支指令使用 taken/not-taken 模式，通过 x7(t2) 的值判断
分支是否被执行（1=taken, 0=not-taken）。

#### 7.3 machina 侧原理

ALU 指令直接复用全流水线基础设施：

```rust
fn run_machina(
    init: &[(usize, u64)],  // 初始寄存器值
    insns: &[u32],           // RISC-V 机器码序列
) -> RiscvCpu
```

流水线：`RISC-V 机器码 --> decode 解码 --> trans_* --> TCG IR
--> optimize --> liveness --> regalloc --> x86-64 codegen --> 执行`

分支指令会退出翻译块（TB），通过 PC 值判断 taken/not-taken：
- `PC = offset` --> taken
- `PC = 4` --> not-taken

#### 7.4 寄存器约定

| 寄存器 | ABI 名 | 用途 |
|--------|--------|------|
| x3 | gp | **保留**：QEMU 侧保存区基址 |
| x5 | t0 | 源操作数 1（rs1） |
| x6 | t1 | 源操作数 2（rs2） |
| x7 | t2 | 目标寄存器（rd） |

x3 不能作为测试寄存器，因为 QEMU 侧的 `la gp, save_area`
会覆盖其值。

#### 7.5 边界值策略

| 常量 | 值 | 含义 |
|------|----|------|
| `V0` | `0` | 零 |
| `V1` | `1` | 最小正数 |
| `VMAX` | `0x7FFF_FFFF_FFFF_FFFF` | i64 最大值 |
| `VMIN` | `0x8000_0000_0000_0000` | i64 最小值 |
| `VNEG1` | `0xFFFF_FFFF_FFFF_FFFF` | -1（全 1） |
| `V32MAX` | `0x7FFF_FFFF` | i32 最大值 |
| `V32MIN` | `0xFFFF_FFFF_8000_0000` | i32 最小值（符号扩展） |
| `V32FF` | `0xFFFF_FFFF` | u32 最大值 |
| `VPATTERN` | `0xDEAD_BEEF_CAFE_BABE` | 随机位模式 |

每条指令使用 4-7 组边界值组合，重点覆盖溢出边界、符号扩展、
零值行为和全 1 位模式。

---

### 8. 机器级测试（mtest 框架）

**目录**：`tests/mtest/`

mtest 是 machina 的全系统级测试框架，在完整的虚拟机环境中
运行裸机固件，验证设备模型、中断控制器、内存映射 I/O 以及
引导流程的端到端正确性。

#### 8.1 架构概览

```
+------------------+     +------------------+
|   mtest runner   |     |  machina binary  |
|  (Rust test fn)  |---->|  (full VM boot)  |
+------------------+     +--------+---------+
                                  |
                    +-------------+-------------+
                    |             |             |
                    v             v             v
              +---------+  +-----------+  +----------+
              |  UART   |  |   CLINT   |  |  Memory  |
              | (ns16550)|  |  (timer)  |  |  (DRAM)  |
              +---------+  +-----------+  +----------+
                    |             |             |
                    v             v             v
              +---------+  +-----------+  +----------+
              | stdout  |  |  IRQ trap |  |  R/W ok  |
              | capture |  |  handler  |  |  verify  |
              +---------+  +-----------+  +----------+
```

#### 8.2 测试类别

| 类别 | 测试数 | 说明 |
|------|--------|------|
| 设备模型 | 20 | UART 寄存器读写、CLINT MMIO、PLIC 分发 |
| MMIO 分发 | 10 | AddressSpace 路由、重叠区间、未映射访问 |
| 引导流程 | 8 | 最小固件加载、PC 复位向量、M-mode 初始化 |
| 中断 | 6 | Timer 中断触发与响应、外部中断路由 |
| 多核 | 4 | SMP 启动、IPI 发送与接收 |
