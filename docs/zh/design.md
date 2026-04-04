# Machina 设计文档

## 1. 概述

Machina 是一个 RISC-V 全系统仿真器，使用 Rust 重新实现了 QEMU 的 TCG（Tiny Code Generator）动态二进制翻译引擎。在运行时将客户架构指令转换为宿主机器码，并提供完整的设备模型、内存子系统和中断控制器，以支持全系统仿真。

当前 MOM 设备模型架构说明见 `docs/zh/mom.md`。

```
+-------------+    +------------+    +--------+    +-----------+    +----------+
| Guest       |--->| Frontend   |--->| TCG IR |--->| Optimizer |--->| Backend  |
| Binary      |    | (decode)   |    |        |    |           |    | (codegen)|
+-------------+    +------------+    +--------+    +-----------+    +----+-----+
                                                                        |
                                                                        v
                                                                   +----+-----+
                                                                   | TB Cache |
                                                                   +----+-----+
                                                                        |
                                                                        v
+----------------+    +----------+    +-----------+    +----------+-----+-----+
| Full-System    |<---| Device   |<---| Memory    |<---| Exec Loop            |
| (WFI/IRQ/SBI) |    | (hw/)    |    | (MMIO)    |    | (Multi-vCPU)              |
+----------------+    +----------+    +-----------+    +----------------------+
```

## 2. Workspace 分层

```
machina/
+-- core/           # IR 定义层：CPU trait、地址类型、纯数据结构
+-- accel/          # 加速层：IR 优化、寄存器分配、x86-64 codegen、执行引擎
+-- guest/riscv/    # RISC-V 前端：RV64GC + 特权 ISA、Sv39 MMU
+-- decode/         # 解码器生成器：解析 .decode 文件，生成 Rust 解码器
+-- system/         # 全系统执行：CPU 管理、WFI 唤醒、FullSystemCpu
+-- memory/         # 内存子系统：AddressSpace、MemoryRegion、MMIO 分发
+-- hw/core/        # 设备基础设施：qdev、IRQ、chardev、FDT、loader
+-- hw/intc/        # 中断控制器：PLIC、ACLINT
+-- hw/char/        # 字符设备：UART 16550A
+-- hw/riscv/       # RISC-V 机器定义：riscv64-ref
+-- disas/          # 反汇编器
+-- monitor/        # 调试接口
+-- util/           # 共享工具
+-- tools/irdump/   # IR 转储工具
+-- tools/irbackend/# 后端测试工具
+-- tests/          # 测试层：单元、集成、difftest、多 vCPU 并发
+-- tests/mtest/    # 机器级测试
```

**设计意图**：遵循 QEMU 的 `include/tcg/`（定义）与 `tcg/`（实现）分离原则。`machina-core` 是纯粹的数据定义，不包含任何平台相关代码或 `unsafe`，`machina-guest-riscv` 和 `machina-accel`（含优化器）都只需依赖 `machina-core`。`decode` 是独立的构建时工具 crate，解析 QEMU 风格的 `.decode` 文件并生成 Rust 解码器代码。`memory/` 和 `hw/` 层提供全系统仿真所需的地址空间模型和设备树。测试独立成 crate 是为了保持源码文件干净，且外部 crate 测试能验证公共 API 的完整性。

### 2.1 多线程 vCPU 支持与执行流程

当前执行层已经支持 多线程 vCPU 并发模型，路径位于 `accel/src/exec/exec_loop.rs`：

1. `cpu_exec_loop_mt(shared, per_cpu, cpu)` 作为多线程入口；
2. 查找顺序：`JumpCache`（每 vCPU）--> 全局 TB hash；
3. miss 时进入 `tb_gen_code`，由 `translate_lock` 串行翻译；
4. TB 执行后按退出协议分流：
   - `TB_EXIT_IDX0/1`：可链路出口，尝试 `tb_add_jump` patch；
   - `TB_EXIT_NOCHAIN`：间接出口，走 `exit_target` 缓存 + 查表；
   - 其他值：真实异常/系统退出。

这与 QEMU 的 `cpu_exec` / `tb_lookup` / `tb_gen_code` / `cpu_tb_exec` 主流程保持同构，当前重点放在"正确性优先 + 热路径可观测"。

---

## 3. machina-core 核心数据结构

### 3.1 Type 系统 (`types.rs`)

```
Type: I32 | I64 | I128 | V64 | V128 | V256
```

- `#[repr(u8)]` 确保枚举值可直接用作数组索引（`Type as usize`）
- 整数/向量分类方法（`is_integer()` / `is_vector()`）用于后续优化器和后端的类型分派
- `TYPE_COUNT = 6` 配合 Context 中的 `const_table: [HashMap; TYPE_COUNT]` 实现按类型分桶的常量去重

### 3.2 Cond 条件码 (`types.rs`)

```
Cond: Never=0, Always=1, Eq=8, Ne=9, Lt=10, ..., TstEq=18, TstNe=19
```

- **编码值直接对齐 QEMU**（`tcg.h` 中 `TCGCond` 的数值），这样未来做前端翻译时可以零成本转换
- `invert()` 和 `swap()` 都是 involution（自逆），测试中专门验证了这一性质
- `TstEq`/`TstNe` 是 QEMU 7.x+ 新增的 test-and-branch 条件，提前纳入

### 3.3 MemOp (`types.rs`)

```
MemOp(u16) -- bit-packed: [1:0]=size, [2]=sign, [3]=bswap, [6:4]=align
```

- 位域打包设计直接映射 QEMU 的 `MemOp`，保持二进制兼容
- 提供语义化构造器 `ub()/sb()/uw()/sw()/ul()/sl()/uq()` 避免手写位操作

### 3.4 RegSet (`types.rs`)

```
RegSet(u64) -- 64-bit bitmap, supports up to 64 host registers
```

- 用 `u64` 位图而非 `HashSet` 或 `Vec`，因为寄存器分配是热路径，位操作（union/intersect/subtract）比集合操作快一个数量级
- `const fn` 方法允许在编译期构造常量寄存器集（如 `RESERVED_REGS`）

### 3.5 统一多态 Opcode (`opcode.rs`)

```
enum Opcode { Mov, Add, Sub, ..., Count }  // 158 variants + sentinel
```

**关键决策：类型多态而非类型分裂**

QEMU 原始设计中 `add_i32` 和 `add_i64` 是不同的 opcode。我们改为统一的 `Add`，实际类型由 `Op::op_type` 字段携带。原因：

1. 减少 opcode 数量（统一多态设计）
2. 优化器可以用统一逻辑处理，不需要 `match (Add32, Add64) => ...`
3. 后端通过 `op.op_type` 选择 32/64 位指令编码，逻辑更清晰
4. `OpFlags::INT` 标记哪些 opcode 是多态的，非多态的（如 `ExtI32I64`）有固定类型

### 3.6 OpDef 静态表 (`opcode.rs`)

```rust
pub static OPCODE_DEFS: [OpDef; Opcode::Count as usize] = [ ... ];
```

- 用 `Opcode::Count` 作为 sentinel 确保表大小与枚举同步——如果新增 opcode 忘记加表项，编译期就会报错
- 每个 `OpDef` 记录 `nb_oargs/nb_iargs/nb_cargs/flags`，这是优化器和寄存器分配器的核心元数据
- `OpFlags` 用位标志而非 `Vec<Flag>`，因为标志检查在编译循环中极其频繁

### 3.7 Temp 临时变量 (`temp.rs`)

```
TempKind: Ebb | Tb | Global | Fixed | Const
```

五种生命周期直接映射 QEMU 的 `TCGTempKind`：

| Kind | 生命周期 | 典型用途 |
|------|---------|---------|
| `Ebb` | 单个扩展基本块 | 算术中间结果 |
| `Tb` | 整个翻译块 | 跨 BB 的值 |
| `Global` | 跨 TB，backed by CPUState | `pc`, `sp` 等 |
| `Fixed` | 固定绑定到宿主寄存器 | `env` (RBP) |
| `Const` | 编译期常量 | 立即数 |

`Temp` 结构体同时承载 IR 属性（`ty`, `kind`）和寄存器分配状态（`val_type`, `reg`, `mem_coherent`），这是 QEMU 的设计——避免额外的 side table 查找。

### 3.8 Label 前向引用 (`label.rs`)

```
Label { present, has_value, value, uses: Vec<LabelUse> }
LabelUse { offset, kind: RelocKind::Rel32 }
```

- 支持前向引用：分支指令可以在 label 定义之前引用它
- `uses` 记录所有未解析的引用位置，`set_value()` 时后端遍历 `uses` 做 back-patching
- `RelocKind` 目前只有 `Rel32`（x86-64 的 RIP-relative 32 位位移），未来扩展 AArch64 时加 `Adr21` 等

### 3.9 Op IR 操作 (`op.rs`)

```rust
struct Op {
    opc: Opcode,
    op_type: Type,        // 多态 opcode 的实际类型
    param1/param2: u8,    // opcode-specific (CALLI/CALLO/VECE)
    life: LifeData,       // 活跃性分析结果
    output_pref: [RegSet; 2],  // 寄存器分配提示
    args: [TempIdx; 10],  // 参数（输出+输入+常量）
}
```

- `args` 是固定大小数组而非 `Vec`，避免堆分配——每个 TB 可能有数百个 Op
- `oargs()/iargs()/cargs()` 通过 `OpDef` 的参数计数做切片，零成本抽象
- `LifeData(u32)` 用 2 bit per arg 编码 dead/sync 状态，紧凑且高效

### 3.10 Context 翻译上下文 (`context.rs`)

```rust
struct Context {
    temps: Vec<Temp>,
    ops: Vec<Op>,
    labels: Vec<Label>,
    nb_globals: u32,
    const_table: [HashMap<u64, TempIdx>; TYPE_COUNT],
    // frame, reserved_regs, gen_insn_end_off...
}
```

**关键设计**：

- **Globals 在 temps 数组前端**：`temps[0..nb_globals]` 是全局变量，`reset()` 时 `truncate(nb_globals)` 保留它们，清除所有局部变量。这避免了每次翻译新 TB 时重新注册全局变量
- **常量去重**：`const_table` 按类型分桶，相同 `(type, value)` 的常量只创建一个 Temp。QEMU 中这是重要的内存优化，因为很多指令共享相同的立即数（0, 1, -1 等）
- **断言保护**：`new_global()` 和 `new_fixed()` 要求在任何局部变量分配之前调用，通过 `assert_eq!(temps.len(), nb_globals)` 强制执行

### 3.11 TranslationBlock (`tb.rs`)

```rust
struct TranslationBlock {
    // immutable after creation
    pc, flags, cflags,
    host_offset, host_size,
    jmp_insn_offset: [Option<u32>; 2],
    jmp_reset_offset: [Option<u32>; 2],
    // mutable chaining state
    jmp: Mutex<TbJmpState>,
    invalid: AtomicBool,
    exit_target: AtomicUsize,
}
```

- **双出口 + NoChain 协议**：`TB_EXIT_IDX0/1` 走可链路路径，`TB_EXIT_NOCHAIN` 走间接路径；真实异常退出值从 `TB_EXIT_MAX` 开始，避免协议冲突。
- **并发链路状态**：`jmp` 维护入边/出边关系，用于 TB 失效时解链；`invalid` 使用原子位做 lock-free 快速检查。
- **间接目标缓存**：`exit_target` 为 `TB_EXIT_NOCHAIN` 提供最近目标 TB 缓存，减少 hash 查找开销。
- **JumpCache**：`Box<[Option<usize>; 4096]>` 直接映射缓存，`(pc >> 2) & 0xFFF` 索引，O(1) 查找。
- **哈希函数**：`pc * 0x9e3779b97f4a7c15 ^ flags`，黄金比例常数确保分布稳定。

---

## 4. machina-accel 代码生成层

### 4.1 CodeBuffer (`code_buffer.rs`)

```
mmap(PROT_READ|PROT_WRITE) --> emit code --> mprotect(PROT_READ|PROT_EXEC)
```

- **W^X 纪律**：写入和执行互斥，`set_executable()` / `set_writable()` 切换权限
- `emit_u8/u16/u32/u64/bytes` + `patch_u32` 覆盖了所有 x86-64 指令编码需求
- `write_unaligned` 处理非对齐写入（x86 允许，但 ARM 不允许——未来需要注意）

### 4.2 HostCodeGen trait (`lib.rs`)

```rust
trait HostCodeGen {
    fn emit_prologue(&mut self, buf: &mut CodeBuffer);
    fn emit_epilogue(&mut self, buf: &mut CodeBuffer);
    fn patch_jump(&mut self, buf: &mut CodeBuffer, jump_offset, target_offset);
    fn epilogue_offset(&self) -> usize;
    fn init_context(&self, ctx: &mut Context);
    fn op_constraint(&self, opc: Opcode) -> &'static OpConstraint;
    // + register allocator primitives: tcg_out_mov/movi/ld/st/op
}
```

- Trait-based 而非条件编译，允许同一二进制支持多后端（测试/模拟场景）
- `init_context()` 让后端向 Context 注入平台特定配置（保留寄存器、栈帧布局）
- `op_constraint()` 返回每个 opcode 的寄存器约束，供通用寄存器分配器消费（见 4.3）

### 4.3 约束系统 (`constraint.rs`)

```rust
struct ArgConstraint {
    regs: RegSet,       // allowed registers
    oalias: bool,       // output aliases an input
    ialias: bool,       // input is aliased to an output
    alias_index: u8,    // which arg it aliases
    newreg: bool,       // output must not overlap any input
}

struct OpConstraint {
    args: [ArgConstraint; MAX_OP_ARGS],
}
```

声明式描述每个 opcode 的寄存器分配需求，对齐 QEMU 的 `TCGArgConstraint` + `C_O*_I*` 宏系统。

**约束类型**：

| 约束 | 含义 | QEMU 等价 | 典型用途 |
|------|------|-----------|---------|
| `oalias` | 输出复用输入的寄存器 | `"0"` (alias) | 破坏性二元运算 (SUB/AND/...) |
| `ialias` | 输入可被输出复用 | 对应 oalias 的输入端 | 与 oalias 配对 |
| `newreg` | 输出不得与任何输入重叠 | `"&"` (newreg) | SetCond (setcc 只写低字节) |
| `fixed` | 单寄存器约束 | `"c"` (RCX) | 移位计数必须在 RCX |

**Builder 函数**：

| 函数 | 签名 | 用途 |
|------|------|------|
| `o1_i2(o0, i0, i1)` | 三地址 | Add (LEA) |
| `o1_i2_alias(o0, i0, i1)` | 输出别名 input0 | Sub/Mul/And/Or/Xor |
| `o1_i1_alias(o0, i0)` | 一元别名 | Neg/Not |
| `o1_i2_alias_fixed(o0, i0, reg)` | 别名 + 固定 | Shl/Shr/Sar (RCX) |
| `n1_i2(o0, i0, i1)` | newreg 输出 | SetCond |
| `o0_i2(i0, i1)` | 无输出 | BrCond/St |
| `o2_i2_fixed(o0, o1, i1)` | 双固定输出 + 别名 | MulS2/MulU2 (RAX:RDX) |
| `o2_i3_fixed(o0, o1, i2)` | 双固定输出 + 双别名 | DivS2/DivU2 (RAX:RDX) |
| `o1_i4_alias2(o0, i0..i3)` | 输出别名 input2 | MovCond (CMOV) |

### 4.4 x86-64 栈帧布局 (`x86_64/regs.rs`)

```
High address
+---------------------+
| return address (8B) |  <-- call pushes this
+---------------------+
| push rbp    (8B)    |  <-- CALLEE_SAVED[0]
| push rbx    (8B)    |
| push r12    (8B)    |
| push r13    (8B)    |
| push r14    (8B)    |
| push r15    (8B)    |  PUSH_SIZE = 56B
+---------------------+
| STATIC_CALL_ARGS    |  128B (outgoing call args)
| CPU_TEMP_BUF        |  1024B (spill slots)
|                     |  STACK_ADDEND = FRAME_SIZE - PUSH_SIZE
+---------------------+
|                     |  <-- RSP (16-byte aligned)
+---------------------+
Low address
```

- `FRAME_SIZE` 编译期计算并 16 字节对齐，满足 System V ABI 要求
- `TCG_AREG0 = RBP`：env 指针固定在 RBP，匹配 QEMU 约定。所有 TB 代码通过 RBP 访问 CPUState

### 4.5 Prologue/Epilogue (`x86_64/emitter.rs`)

**Prologue**:

1. `push` 6 个 callee-saved 寄存器（RBP 在最前）
2. `mov rbp, rdi` -- 将第一个参数（env 指针）存入 TCG_AREG0
3. `sub rsp, STACK_ADDEND` -- 分配栈帧
4. `jmp *rsi` -- 跳转到第二个参数（TB 宿主代码地址）

**Epilogue（双入口）**:

- `epilogue_return_zero`: `xor eax, eax` --> fall through（用于 `goto_ptr` 查找失败）
- `tb_ret`: `add rsp` --> `pop` 寄存器 --> `ret`（用于 `exit_tb` 正常返回）

这个双入口设计避免了 `exit_tb(0)` 时多余的 `mov rax, 0` 指令。

### 4.6 TB 控制流指令

- **`exit_tb(val)`**：val==0 时直接 `jmp epilogue_return_zero`；否则 `mov rax, val` + `jmp tb_ret`
- **`goto_tb`**：发射 `E9 00000000`（JMP rel32），NOP 填充确保 disp32 字段 4 字节对齐，使得 TB chaining 时的原子修补是安全的
- **`goto_ptr(reg)`**：`jmp *reg`，用于间接跳转（lookup_and_goto_ptr 之后）

---

## 5. 翻译流水线

完整的翻译流水线将 TCG IR 转换为可执行的宿主机器码：

```
Guest Binary --> Frontend (decode) --> IR Builder (gen_*) --> Optimize --> Liveness --> RegAlloc + Codegen --> Execute
                 riscv/trans.rs        ir_builder.rs          optimize.rs  liveness.rs  regalloc.rs          translate.rs
                                                                                         codegen.rs
```

### 5.1 IR Builder (`ir_builder.rs`)

`impl Context` 上的 `gen_*` 方法，将高层操作转换为 `Op` 并追加到 ops 列表。每个方法创建 `Op::with_args()` 并设置正确的 opcode、type 和 args 布局。

**常量参数编码**：条件码、偏移量、label ID 等常量参数编码为 `TempIdx(raw_value as u32)` 存入 `args[]`，与 QEMU 约定一致。

**已实现的 IR 生成方法**：

| 类别 | 方法 | 签名 |
|------|------|------|
| 二元 ALU | `gen_add/sub/mul/and/or/xor/shl/shr/sar` | (ty, d, a, b) --> d |
| 一元 | `gen_neg/not/mov` | (ty, d, s) --> d |
| 条件设置 | `gen_setcond` | (ty, d, a, b, cond) --> d |
| 内存访问 | `gen_ld` / `gen_st` | (ty, dst/src, base, offset) |
| 控制流 | `gen_br/brcond/set_label` | (label_id) / (ty, a, b, cond, label) |
| TB 出口 | `gen_goto_tb/exit_tb` | (tb_idx) / (val) |
| 边界 | `gen_insn_start` | (pc) |

### 5.2 IR 优化器 (`optimize.rs`)

在活跃性分析之前运行的单遍前向扫描优化器，对齐 QEMU 的 `tcg/optimize.c`。使用 per-temp `TempInfo` 追踪常量值和拷贝源。

**数据结构**：

```rust
struct TempInfo {
    is_const: bool,
    val: u64,
    copy_of: Option<TempIdx>,  // canonical copy source
}
```

初始化时从已有的 `TempKind::Const` temp 中读取常量信息。

**优化类别**：

| 类别 | 触发条件 | 操作 |
|------|---------|------|
| 拷贝传播 | 输入 temp 有 `copy_of` | 替换为源 temp |
| 常量折叠（一元） | Neg/Not 输入为常量 | --> `Mov dst, const` |
| 常量折叠（二元） | Add/Sub/Mul/And/Or/Xor/AndC/Shl/Shr/Sar/RotL/RotR 两输入均为常量 | --> `Mov dst, const` |
| 常量折叠（类型转换） | ExtI32I64/ExtUI32I64/ExtrlI64I32/ExtrhI64I32 输入为常量 | --> `Mov dst, const` |
| 代数简化 | 一个输入为常量（0, 1, -1） | `x+0-->x`, `x*0-->0`, `x&-1-->x` 等 |
| 同操作数恒等式 | 两输入相同 | `x&x-->x`, `x^x-->0`, `x-x-->0` |
| 分支折叠 | BrCond 两输入均为常量 | 恒真-->Br, 恒假-->Nop |
| 强度削减 | `0 - x` | --> `Neg x` |

**BB 边界处理**：遇到 SetLabel/Br/ExitTb/GotoTb/GotoPtr/Call 时清除所有拷贝关系，因为跨 BB 的拷贝信息不可靠。

**类型掩码**：I32 操作结果截断到 32 位（`val & 0xFFFF_FFFF`），I64 保持 64 位。

**Op 替换策略**：优化后的 op 原地替换——常量折叠结果改为 `Mov dst, const_temp`，代数简化改为 `Mov dst, surviving_input`，恒假分支改为 `Nop`，恒真分支改为 `Br`。

**关键设计决策**：`replace_with_mov` 使用保守策略——仅 `invalidate_one(dst)` 而非 `set_copy(dst, src)`。这避免了源 temp 被后续 op 重定义时目标 temp 保留过期常量信息的 bug。只有显式的 `Mov` op（`fold_mov`）才建立拷贝关系。

### 5.3 活跃性分析 (`liveness.rs`)

反向遍历 ops 列表，为每个 op 计算 `LifeData`，标记哪些参数在该 op 之后死亡（dead）以及哪些全局变量需要同步回内存（sync）。

**算法**：

1. 初始化 `temp_state[0..nb_temps]` = false（全部死亡）
2. TB 末尾：所有全局变量标记为活跃
3. 反向遍历每个 op：
   - 遇到 `BB_END` 标志：所有全局变量标记为活跃
   - 输出参数：若 `!temp_state[tidx]` --> 标记 dead；然后 `temp_state[tidx] = false`
   - 输入参数：若 `!temp_state[tidx]` --> 标记 dead（最后使用），若为全局变量则标记 sync；然后 `temp_state[tidx] = true`
4. 将计算的 `LifeData` 写回 `op.life`

### 5.4 寄存器分配器 (`regalloc.rs`)

约束驱动的贪心逐 op 分配器，前向遍历 ops 列表，对齐 QEMU 的 `tcg_reg_alloc_op()`。MVP 不支持溢出（spill）——14 个可分配 GPR 对简单 TB 足够。

#### 5.4.1 架构概述

QEMU 的寄存器分配器 `tcg_reg_alloc_op()`（`tcg/tcg.c`）是完全通用的——不含任何 per-opcode 分支。每个 opcode 的特殊需求（如 SUB 的破坏性语义、SHL 的 RCX 要求）全部通过 `TCGArgConstraint` 声明式描述，分配器只需读取约束并执行统一逻辑。

machina 的 `regalloc_op()` 对齐这一架构：

```
                    +--------------+
                    | OpConstraint |  <-- backend.op_constraint(opc)
                    +------+-------+
                           |
                           v
  +--------------------------------------------------+
  |           regalloc_op() -- generic path           |
  |                                                   |
  |  1. load inputs   -> 2. fixup -> 3. free dead in  |
  |  4. alloc outputs -> 5. emit  -> 6. free dead out |
  |                       7. sync globals             |
  +---------------------------------------------------+
```

这意味着新增 opcode 时只需在约束表中添加一行，分配器和 codegen 无需任何修改。

#### 5.4.2 分配器状态

```rust
struct RegAllocState {
    reg_to_temp: [Option<TempIdx>; 16],
    free_regs: RegSet,
    allocatable: RegSet,
}
```

| 字段 | 含义 |
|------|------|
| `reg_to_temp` | 16 个宿主寄存器各自映射到哪个 temp（None=空闲） |
| `free_regs` | 当前空闲且可分配的寄存器位图 |
| `allocatable` | 可分配寄存器集合（不变，排除 RSP/RBP） |

**初始化**：`free_regs = allocatable`，然后遍历所有 Fixed temp（如 env/RBP），将其标记为已占用（`assign(reg, tidx)`）。

**Temp 状态机**：每个 `Temp` 有 `val_type` 字段追踪其当前位置：

```
                 temp_load_to()
    +------+    +--------------+    +-----+
    | Dead |--->| Const / Mem  |--->| Reg |
    +------+    +--------------+    +--+--+
       ^                               |
       +--------- temp_dead() ---------+
                                 (local temp)
```

- **Dead**：未分配，不占用任何资源
- **Const**：编译期常量，需要 `movi` 加载到寄存器
- **Mem**：全局变量在内存中，需要 `ld` 加载到寄存器
- **Reg**：已在宿主寄存器中，可直接使用

全局变量和固定 temp 不会进入 Dead 状态——`temp_dead()` 对它们是 no-op。

#### 5.4.3 主循环分派

`regalloc_and_codegen()` 前向遍历 ops 列表，按 opcode 分派：

| Op 类型 | 处理策略 | 原因 |
|---------|---------|------|
| Nop/InsnStart | 跳过 | 无代码生成 |
| Mov | 专用路径 | 寄存器重命名优化（QEMU 也单独处理） |
| SetLabel | sync --> 解析 label --> back-patch | 控制流汇合点 |
| Br | sync --> emit jmp | 无条件跳转 |
| BrCond | 约束加载 --> sync --> emit cmp+jcc | 需要 sync 在 emit 之前 |
| ExitTb/GotoTb | sync --> 委托 tcg_out_op | TB 退出 |
| GotoPtr | 约束加载 --> sync --> emit jmp *reg | 间接跳转 |
| Mb | 直接 emit mfence | 内存屏障 |
| **其他** | **`regalloc_op()`** | **通用约束驱动路径** |

**为什么 BrCond 不走通用路径？** 因为 BrCond 需要在 emit 之前 sync globals（分支目标可能是另一个 BB），而通用路径的 sync 在 emit 之后。此外 BrCond 的前向引用需要在 emit 之后记录 `label.add_use()`。

#### 5.4.4 与 QEMU 的差异

| 方面 | QEMU | machina |
|------|------|---------|
| 溢出 | 支持溢出到栈帧 `CPU_TEMP_BUF` | 不支持（14 GPR 足够） |
| 立即数约束 | `re`/`ri` 允许立即数直接编码 | 所有输入必须在寄存器中 |
| 输出偏好 | `output_pref` 由约束系统设置 | 由活跃性分析设置 |
| 常量输入 | 可内联到指令编码 | 必须先 `movi` 到寄存器 |
| 内存输入 | 部分指令支持 `[mem]` 操作数 | 必须先 `ld` 到寄存器 |

### 5.5 流水线编排 (`translate.rs`)

将各阶段串联为完整流水线：

```
translate():
    optimize(ctx)
    liveness_analysis(ctx)
    tb_start = buf.offset()
    regalloc_and_codegen(ctx, backend, buf)
    return tb_start

translate_and_execute():
    buf.set_writable()
    tb_start = translate(ctx, backend, buf)
    buf.set_executable()
    prologue_fn = transmute(buf.base_ptr())
    return prologue_fn(env, tb_ptr)
```

**Prologue 调用约定**：
`fn(env: *mut u8, tb_ptr: *const u8) -> usize`
- RDI = env 指针（prologue 存入 RBP）
- RSI = TB 代码地址（prologue 跳转到此处）
- 返回值 RAX = `exit_tb` 的值

### 5.6 端到端集成测试

`tests/src/integration/mod.rs` 使用最小 RISC-V CPU 状态验证完整流水线：

```rust
#[repr(C)]
struct RiscvCpuState {
    regs: [u64; 32],  // x0-x31, offset 0..256
    pc: u64,          // offset 256
}
```

通过 `ctx.new_global()` 将 x0-x31 和 pc 注册为全局变量，backed by `RiscvCpuState` 字段。

**测试用例**：

| 测试 | 验证内容 |
|------|---------|
| `test_addi_x1_x0_42` | 常量加法：x1 = x0 + 42 |
| `test_add_x3_x1_x2` | 寄存器加法：x3 = x1 + x2 |
| `test_sub_x3_x1_x2` | 寄存器减法：x3 = x1 - x2 |
| `test_beq_taken` | 条件分支（taken 路径） |
| `test_beq_not_taken` | 条件分支（not-taken 路径） |
| `test_sum_loop` | 循环：计算 1+2+3+4+5=15 |

---

## 6. machina-accel 执行层

### 6.1 SharedState / PerCpuState 分离

执行层将状态拆分为共享和每 CPU 两部分，对齐多线程 vCPU 模型：

```rust
struct SharedState<B: HostCodeGen> {
    tb_store: TbStore,              // 全局 TB 缓存 + 哈希表
    code_buf: UnsafeCell<CodeBuffer>, // JIT 代码缓冲区
    backend: B,                     // 宿主代码生成器
    code_gen_start: usize,          // prologue 之后的代码起始偏移
    translate_lock: Mutex<TranslateGuard>, // 串行化翻译
}

struct PerCpuState {
    jump_cache: JumpCache,  // 4096 项直接映射 TB 缓存
    stats: ExecStats,       // 执行统计
}
```

`SharedState` 通过 `&` 共享给所有 vCPU 线程——`code_buf` 用 `UnsafeCell` 包装，写入路径由 `translate_lock` 保护，读取路径（执行生成代码、patch 跳转）无锁。`PerCpuState` 每线程独占，无需同步。

**TbStore** 使用 `UnsafeCell<Vec<TranslationBlock>>` + `AtomicUsize` 长度计数器实现 lock-free 读：新 TB 通过 `Acquire/Release` 语义发布，读者无需加锁。哈希表（32768 桶）用 `Mutex` 保护写入。

### 6.2 GuestCpu trait

```rust
trait GuestCpu {
    fn get_pc(&self) -> u64;
    fn get_flags(&self) -> u32;
    fn gen_code(
        &mut self, ir: &mut Context, pc: u64, max_insns: u32,
    ) -> u32;
    fn env_ptr(&mut self) -> *mut u8;
}
```

每个客户架构（如 RISC-V）实现此 trait，将前端解码与执行引擎解耦。`gen_code()` 负责解码客户指令并生成 TCG IR，返回翻译的客户字节数。`env_ptr()` 返回 CPU 状态结构指针，传递给生成的宿主代码（通过 RBP 访问）。

### 6.3 执行循环

`cpu_exec_loop_mt()` 是多线程 vCPU 主循环，对齐 QEMU 的 `cpu_exec`：

```
loop {
    1. next_tb_hint 快速路径：复用上一跳目标 TB
    2. tb_find(pc, flags):
       jump_cache --> hash table --> tb_gen_code()
    3. cpu_tb_exec(tb_idx) --> raw_exit
    4. decode_tb_exit(raw_exit) --> (last_tb, exit_code)
    5. 按 exit_code 分流：
       0/1  --> tb_add_jump() 链接 + 设置 next_tb_hint
       NOCHAIN --> exit_target 缓存 + 查表
       >= MAX --> 返回 ExitReason
}
```

**tb_gen_code** 流程：检查缓冲区空间 --> 获取 `translate_lock` --> 双重检查（其他线程可能已翻译）--> 分配 TB --> 前端生成 IR --> 后端生成宿主代码 --> 记录 `goto_tb` 偏移 --> 插入哈希表和 jump cache。

### 6.4 TB 生命周期

```
查找 --> 未命中 --> 翻译 --> 缓存 --> 执行 --> 链接 --> [失效]
```

**链接**（`tb_add_jump`）：验证源 TB 的 `jmp_insn_offset[slot]` 有效且目标未失效 --> 锁定源 TB --> 调用 `backend.patch_jump()` 修改跳转指令 --> 更新出边 `jmp_dest[slot]` --> 锁定目标 TB --> 添加反向边 `jmp_list.push((src, slot))`。

**失效**（`TbStore::invalidate`）：标记 `tb.invalid = true` --> 遍历入边 `jmp_list` 调用 `reset_jump()` 恢复跳转 --> 清空出边 `jmp_dest` 并从目标 TB 的 `jmp_list` 中移除 --> 从哈希链中移除。

---

## 7. machina-guest-riscv 客户解码层

### 7.1 decode 解码器生成器

`decode` crate 实现了 QEMU 的 decodetree 工具的 Rust 版本，解析 `.decode` 文件并生成 Rust 解码器代码。

**输入**：`guest/riscv/src/riscv/insn32.decode`（RV64IMAFDC 指令模式）

**生成的代码**：
- `Args*` 结构体：每个参数集对应一个结构体（如 `ArgsR { rd, rs1, rs2 }`）
- `extract_*` 函数：从 32 位指令字中提取字段（支持多段拼接、符号扩展）
- `Decode<Ir>` trait：每个模式对应一个 `trans_*` 方法
- `decode()` 函数：if-else 链按 fixedmask/fixedbits 匹配指令

**构建集成**：`guest/riscv/build.rs` 在编译时调用 `decode::generate()`，输出到 `$OUT_DIR/riscv32_decode.rs`，通过 `include!` 宏引入。

### 7.2 TranslatorOps trait

`guest/riscv/src/lib.rs` 定义了架构无关的翻译框架：

```rust
trait TranslatorOps {
    type Disas;
    fn init_disas_context(ctx: &mut Self::Disas, ir: &mut Context);
    fn tb_start(ctx: &mut Self::Disas, ir: &mut Context);
    fn insn_start(ctx: &mut Self::Disas, ir: &mut Context);
    fn translate_insn(ctx: &mut Self::Disas, ir: &mut Context);
    fn tb_stop(ctx: &mut Self::Disas, ir: &mut Context);
}
```

`translator_loop()` 实现了 QEMU `accel/tcg/translator.c` 中的翻译循环：`tb_start --> (insn_start + translate_insn)* --> tb_stop`。

### 7.3 RISC-V 前端（含浮点）

**CPU 状态**（`riscv/cpu.rs`）：

```rust
#[repr(C)]
struct RiscvCpu {
    gpr: [u64; 32],     // x0-x31
    fpr: [u64; 32],     // f0-f31 (raw bits, NaN-boxed)
    pc: u64,
    guest_base: u64,
    load_res: u64,       // LR 保留地址
    load_val: u64,       // LR 加载值
    fflags: u64,         // 浮点异常标志
    frm: u64,            // 浮点舍入模式
    ustatus: u64,        // 用户态状态寄存器
}
```

**翻译上下文**（`riscv/mod.rs`）：`RiscvDisasContext` 将 32 个 GPR、32 个 FPR、PC 及浮点 CSR 注册为 TCG 全局变量（backed by `RiscvCpu` 字段），env 指针固定到 RBP。

**指令翻译**（`riscv/trans/`）：实现 `Decode<Context>` trait 的 `trans_*` 方法，覆盖 RV64IMAFDC 整数、浮点和压缩指令集，使用 QEMU 风格的 `gen_xxx` 辅助函数模式：

```rust
type BinOp =
    fn(&mut Context, Type, TempIdx, TempIdx, TempIdx) -> TempIdx;

fn gen_arith(ir: &mut Context, a: &ArgsR, op: BinOp) -> bool;
fn gen_arith_imm(ir: &mut Context, a: &ArgsI, op: BinOp) -> bool;
fn gen_branch(
    ir: &mut Context, rs1: usize, rs2: usize,
    imm: i64, cond: Cond,
) -> bool;
```

每个 `trans_*` 方法成为一行调用，如 `trans_add --> gen_arith(ir, a, Context::gen_add)`。

**浮点支持**：RV64F/RV64D 浮点指令通过 `gen_helper_call` 调用 `fpu.rs` 中的 C ABI 辅助函数，由后端 `regalloc_call` 处理 caller-saved 寄存器保存/恢复。实现浮点相关用户态 CSR（`fflags`、`frm`、`fcsr`）及 U-mode 状态/陷阱 CSR，带 FS 状态追踪（仅在写入 FPR 时标记 dirty）。

---

## 8. 全系统架构

Machina 的全系统仿真层覆盖 CPU 管理、内存子系统、设备模型和机器定义，使 JIT 引擎能够运行完整的 RISC-V 特权固件和操作系统内核。

### 8.1 system/ -- CPU 管理与 WFI

#### CpuManager

`system/src/lib.rs` 中的 `CpuManager` 负责管理 vCPU 线程的生命周期：

- `running: Arc<AtomicBool>` -- 全局运行标志，所有 vCPU 线程轮询此标志决定是否退出
- `stop()` -- 清除运行标志并唤醒所有 WFI 阻塞的 CPU
- 支持多 vCPU 并行执行（多线程 vCPU 模型）

#### FullSystemCpu

`system/src/cpus.rs` 定义了 `FullSystemCpu`，桥接 `RiscvCpu` 到执行循环的 `GuestCpu` trait：

```
+------------------+       +------------------+
| FullSystemCpu    |       | Exec Loop        |
|                  |       | (GuestCpu trait)  |
|  +------------+  |       |                  |
|  | RiscvCpu   |  | <---> | gen_code()       |
|  +------------+  |       | get_pc()         |
|  ram_ptr         |       | env_ptr()        |
|  shared_mip      |       | pending_interrupt|
|  wfi_waker       |       +------------------+
+------------------+
```

- `guest_base` 设置为 `ram_ptr - RAM_BASE`，使 JIT 生成的内存访问指令可以直接通过 `guest_base + guest_addr` 计算宿主地址
- `SharedMip (Arc<AtomicU64>)` -- 设备写入此原子变量传递中断，执行循环在 `pending_interrupt()` 中读取并同步到 CPU 的 CSR
- `WfiWaker` -- 基于 `Condvar` 的唤醒原语，设备 IRQ sink 调用 `wake()` 唤醒 WFI 阻塞的 CPU

#### WFI 机制 (`core/src/wfi.rs`)

```
Device IRQ --> IrqSink::set_irq()
                  |
                  v
          SharedMip.fetch_or(bit)
                  |
                  v
          WfiWaker::wake()
                  |
                  v
          Condvar::notify_all()
                  |
                  v
          CPU exits WFI wait
                  |
                  v
          pending_interrupt() syncs mip
```

`WfiWaker` 内部用单个 `Mutex` 保护 `irq_pending` 和 `stopped` 两个标志，`wake()`、`stop()` 和 `wait()` 三个方法都获取同一把锁，从而消除 lost-wakeup 竞态。

### 8.2 memory/ -- 内存子系统

#### AddressSpace

`memory/src/address_space.rs` 实现顶层地址空间，由 `MemoryRegion` 树和缓存的 `FlatView` 组成：

```
AddressSpace
+-- root: MemoryRegion (Container "system")
|   +-- SubRegion @ 0x80000000: MemoryRegion (Ram)
|   +-- SubRegion @ 0x0C000000: MemoryRegion (Io, PLIC)
|   +-- SubRegion @ 0x02000000: MemoryRegion (Io, ACLINT)
|   +-- SubRegion @ 0x10000000: MemoryRegion (Io, UART)
+-- flat_view: RwLock<FlatView>  (cached dispatch table)
```

- `read(addr, size)` / `write(addr, size, val)` -- 通过 FlatView 快速分发到 RAM 或 MMIO 回调
- `update_flat_view()` -- 修改 region 树后重建扁平视图

#### MemoryRegion

`memory/src/region.rs` 定义了内存区域树节点：

| RegionType | 含义 | 典型用途 |
|------------|------|---------|
| `Ram` | `Arc<RamBlock>` 支持的读写内存 | 主存 |
| `Rom` | `Arc<RamBlock>` 支持的只读内存 | 固件 |
| `Io` | `Arc<Mutex<Box<dyn MmioOps>>>` | MMIO 设备 |
| `Alias` | 另一个 region 的偏移别名 | 地址重映射 |
| `Container` | 纯容器，无后端存储 | 地址空间根 |

每个 region 有 `priority`、`enabled`、`subregions` 属性，支持树状嵌套和优先级覆盖。

#### MmioOps trait

```rust
pub trait MmioOps: Send {
    fn read(&self, offset: u64, size: u32) -> u64;
    fn write(&self, offset: u64, size: u32, val: u64);
}
```

设备模型实现此 trait 以处理 MMIO 读写。使用 `&self`（共享引用）配合内部可变性（`Mutex`），匹配内存树的共享所有权模型。

#### FlatView 与 RamBlock

- `FlatView` -- 将 region 树扁平化为有序的 `FlatRange` 数组，地址查找时二分搜索 O(log n)
- `RamBlock` -- 通过 `mmap` 分配的连续物理内存后端，提供 `ptr()` 访问原始指针

### 8.3 hw/ -- 设备模型层

#### hw/core -- 设备基础设施

| 模块 | 职责 | QEMU 参考 |
|------|------|-----------|
| `qdev.rs` | `Device` trait + `DeviceState` 基础 | `hw/core/qdev.c` |
| `irq.rs` | `IrqSink` trait, `IrqLine`, `OrIrq`, `SplitIrq` | `hw/core/irq.c` |
| `chardev.rs` | `Chardev` trait, `CharFrontend`, `StdioChardev` | `chardev/char.c` |
| `fdt.rs` | `FdtBuilder` -- 构建 DTB blob | `hw/core/fdt_generic_util.c` |
| `loader.rs` | `load_binary()` 二进制加载 | `hw/core/loader.c` |
| `bus.rs` | 总线模型 | `hw/core/bus.c` |
| `clock.rs` | 时钟模型 | `hw/core/clock.c` |

**qdev 设备模型**：

```rust
pub trait Device: Send + Sync {
    fn name(&self) -> &str;
    fn realize(&mut self) -> Result<(), String>;
    fn reset(&mut self);
    fn realized(&self) -> bool;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}
```

每个设备实现 `Device` trait，通过 `realize()` 完成初始化，`reset()` 恢复默认状态。`as_any()` 支持向下转型（downcasting）。

**IRQ 路由**：

```
Device --> IrqLine::raise()
               |
               v
         IrqSink::set_irq(irq, level)
               |
      +--------+--------+
      |                  |
      v                  v
   OrIrq            SplitIrq
   (N:1 OR)         (1:N fan-out)
      |                  |
      v                  v
  output IrqLine    output IrqLines
```

`IrqLine` 将源和汇连接为单根中断线。`OrIrq` 实现多输入或门（如 PLIC 的多源汇聚），`SplitIrq` 实现单输入多输出扇出。

**chardev 字符设备**：

`Chardev` trait 提供字节级 I/O 后端（stdio、null、Unix socket），`CharFrontend` 桥接设备前端（如 UART）到后端。`StdioChardev` 将 guest 串口输出转发到宿主标准 I/O，支持 `-nographic` 模式。

**FDT 构建器**：

`FdtBuilder` 在内存中构建 Flattened Devicetree blob（DTB），供 bootloader 和内核发现硬件拓扑。API 风格为 `begin_node()` / `property_*()` / `end_node()` 嵌套调用。

#### hw/intc -- 中断控制器

**PLIC (Platform-Level Interrupt Controller)**：

`hw/intc/src/plic.rs` 实现 SiFive PLIC 规范，MMIO 布局：

```
+----------+----------+-----------------------------------------+
| 偏移     | 大小     | 寄存器                                  |
+----------+----------+-----------------------------------------+
| 0x000000 | 4B * N   | priority[0..N] -- 中断源优先级          |
| 0x001000 | bitmap   | pending -- 中断挂起位图                 |
| 0x002000 | 0x80*ctx | enable[ctx] -- 每上下文使能位图         |
| 0x200000 | 0x1000*c | threshold (off 0), claim/complete (off 4)|
+----------+----------+-----------------------------------------+
```

- 支持 96 个中断源，上下文数 = 2 * hart 数（M-mode + S-mode）
- `context_outputs` 通过 `IrqLine` 连接到每个 hart 的 MEI/SEI 中断线
- 电平触发，支持 claim/complete 协议

**ACLINT (Advanced Core Local Interruptor)**：

`hw/intc/src/aclint.rs` 实现 CLINT 兼容的定时器和 IPI 接口：

```
+----------+----------+------------------------------+
| 偏移     | 大小     | 寄存器                       |
+----------+----------+------------------------------+
| 0x0000   | 4B*hart  | msip[hart] -- 软件中断       |
| 0x4000   | 8B*hart  | mtimecmp[hart] -- 定时器比较  |
| 0xBFF8   | 8B       | mtime -- 全局定时器           |
+----------+----------+------------------------------+
```

- `tick()` 方法递增 `mtime`，比较 `mtimecmp` 产生 MTI（机器定时器中断）
- `mti_outputs` / `msi_outputs` 通过 `IrqLine` 连接到 hart

#### hw/char -- 字符设备

**UART 16550A**：

`hw/char/src/uart.rs` 实现 NS16550A 串口仿真：

- 寄存器映射：RBR/THR (0), IER (1), IIR/FCR (2), LCR (3), MCR (4), LSR (5), MSR (6), SCR (7)
- 16 字节 FIFO 接收缓冲
- 支持 DLAB（除数锁存器访问）
- 通过 `IrqLine` 连接到 PLIC 的 UART_IRQ (10)
- 通过 `CharFrontend` 桥接到 chardev 后端

### 8.4 hw/riscv -- RISC-V 机器定义

#### riscv64-ref 参考机器

`hw/riscv/src/ref_machine.rs` 定义了 virt 兼容的参考平台 `RefMachine`，实现 `Machine` trait：

**内存映射**：

```
+------------------+------------------+------------------+
| 地址范围         | 大小             | 设备             |
+------------------+------------------+------------------+
| 0x0200_0000      | 64 KiB           | ACLINT (CLINT)   |
| 0x0C00_0000      | 64 MiB           | PLIC             |
| 0x1000_0000      | 256 B            | UART0 (16550A)   |
| 0x8000_0000      | configurable     | RAM              |
+------------------+------------------+------------------+
```

**初始化流程** (`init()`):

1. 创建 `AddressSpace`（Container 根 region）
2. 分配 RAM（`MemoryRegion::ram()`），获取 `Arc<RamBlock>`
3. 创建 PLIC、ACLINT、UART 设备实例
4. 建立 IRQ 路由：UART --> PLIC source 10 --> hart MEI/SEI
5. 将设备 MMIO region 挂载到地址空间
6. 生成 FDT blob（`build_fdt()`），描述完整硬件拓扑
7. `update_flat_view()` 重建扁平视图

**IRQ 路由图**：

```
UART -----> PLIC source 10
                 |
                 v
            PLIC context
            +-------+-------+
            |               |
            v               v
        MEI (hart 0)   SEI (hart 0)
            |               |
            v               v
    RiscvCpuIrqSink  RiscvCpuIrqSink
            |               |
            v               v
      SharedMip.fetch_or(bit)
            |
            v
      WfiWaker::wake()
```

`RiscvCpuIrqSink` 实现 `IrqSink` trait，将中断转换为 `SharedMip` 的原子位设置和 WFI 唤醒。

#### boot.rs -- 启动设置

`hw/riscv/src/boot.rs` 负责加载 bios/kernel 并初始化启动上下文：

- **BIOS 加载**：二进制映像加载到 `RAM_BASE (0x8000_0000)`
- **Kernel 加载**：加载到 `RAM_BASE + 0x20_0000`（2 MiB 偏移）
- **FDT 放置**：对齐放置在 RAM 顶部
- **启动约定**（对齐 OpenSBI / QEMU virt）：
  - `a0 = hart_id`
  - `a1 = fdt_addr`（客户物理地址）
  - `PC = entry_pc`
  - 特权级 = Machine mode

#### sbi.rs -- SBI 存根

`hw/riscv/src/sbi.rs` 提供最小的 SBI（Supervisor Binary Interface）ecall 处理，作为 `-bios none` 的 fallback，使 S-mode 软件可以探测基本 SBI 功能：

- `SBI_EXT_BASE (0x10)` -- 规范版本（0.2）、实现 ID、扩展探测
- 其他扩展返回 `SBI_ERR_NOT_SUPPORTED (-2)`
- `SbiResult { error, value }` 对应 `a0`/`a1` 返回值

### 8.5 Sv39 MMU 集成

`guest/riscv/src/riscv/mmu.rs` 实现 RISC-V Sv39 虚拟内存管理单元，集成到全系统仿真路径中。

**Sv39 页表结构**：

```
Virtual Address (39 bits):
+--------+--------+--------+---------------+
| VPN[2] | VPN[1] | VPN[0] | Page Offset   |
| 9 bits | 9 bits | 9 bits | 12 bits       |
+--------+--------+--------+---------------+

Page Table Walk (3 levels):
satp.PPN --> L2 PTE --> L1 PTE --> L0 PTE --> Physical Page
             |          |          |
             v          v          v
          1 GiB      2 MiB      4 KiB
         gigapage   megapage     page
```

**核心特性**：

- **三级页表遍历**：支持 4 KiB 页、2 MiB 大页和 1 GiB 巨页
- **256 项直接映射 TLB**：按虚拟页号哈希索引，缓存最近的翻译结果
- **权限检查**：根据当前特权级（U/S/M）和 PTE 标志位（R/W/X/U）验证访问权限
- **mstatus 标志**：MXR（Make eXecutable Readable）允许读取可执行页，SUM（Supervisor User Memory access）允许 S-mode 访问 U 页
- **A/D 位**：检查 Access 和 Dirty 位，未设置时产生 page fault
- **PMP 集成**：`pmp.rs` 实现物理内存保护，在 MMU 翻译后进行额外的访问检查
- **SATP 寄存器**：mode（Bare/Sv39）、ASID（16 位）、PPN（44 位）字段解析
- **TLB 刷新**：`sfence.vma` 指令触发 TLB 全部或按 ASID/虚拟地址部分刷新

---

## 9. 设计权衡总结

| 决策 | 选择 | 理由 |
|------|------|------|
| Opcode 多态 vs 分裂 | 统一多态 | 减少 40% opcode，简化优化器 |
| Op.args 固定数组 vs Vec | 固定 `[TempIdx; 10]` | 避免堆分配，TB 内有数百个 Op |
| RegSet 位图 vs HashSet | `u64` 位图 | 寄存器分配热路径，位操作更快 |
| 后端 trait vs 条件编译 | Trait | 可测试性，未来多后端支持 |
| 常量去重 | 按类型分桶 HashMap | 避免重复 Temp，节省内存 |
| JumpCache 堆分配 | `Box<[_; 4096]>` | 32KB 不适合放栈上 |
| TCG_AREG0 = RBP | 匹配 QEMU | 二进制兼容，便于参考验证 |
| IRQ 传递用 AtomicU64 | `SharedMip` | 避免设备与 CPU 共享可变引用 |
| WFI 用 Condvar | `WfiWaker` | 正确处理 lost-wakeup 竞态 |
| MemoryRegion 树 | QEMU 风格 region 模型 | 支持优先级覆盖和动态重配置 |

---

## 10. QEMU 参考映射

| QEMU C 结构/概念 | Rust 对应 | 文件 |
|-----------------|----------|------|
| `TCGType` | `enum Type` | `core/src/types.rs` |
| `TCGTempVal` | `enum TempVal` | `core/src/types.rs` |
| `TCGCond` | `enum Cond` | `core/src/types.rs` |
| `MemOp` | `struct MemOp(u16)` | `core/src/types.rs` |
| `TCGRegSet` | `struct RegSet(u64)` | `core/src/types.rs` |
| `TCGOpcode` + DEF macros | `enum Opcode` | `core/src/opcode.rs` |
| `TCGOpDef` | `struct OpDef` + `OPCODE_DEFS` | `core/src/opcode.rs` |
| `TCG_OPF_*` | `struct OpFlags` | `core/src/opcode.rs` |
| `TCGTempKind` | `enum TempKind` | `core/src/temp.rs` |
| `TCGTemp` | `struct Temp` | `core/src/temp.rs` |
| `TCGLabel` | `struct Label` | `core/src/label.rs` |
| `TCGLifeData` | `struct LifeData(u32)` | `core/src/op.rs` |
| `TCGOp` | `struct Op` | `core/src/op.rs` |
| `TCGContext` | `struct Context` | `core/src/context.rs` |
| `TranslationBlock` | `struct TranslationBlock` | `core/src/tb.rs` |
| `CPUJumpCache` | `struct JumpCache` | `core/src/tb.rs` |
| `tcg_target_callee_save_regs` | `CALLEE_SAVED` | `accel/src/x86_64/regs.rs` |
| `tcg_out_tb_start` (prologue) | `HostCodeGen::emit_prologue` | `accel/src/x86_64/emitter.rs` |
| `tcg_code_gen_epilogue` | `HostCodeGen::emit_epilogue` | `accel/src/x86_64/emitter.rs` |
| `tcg_out_exit_tb` | `X86_64CodeGen::emit_exit_tb` | `accel/src/x86_64/emitter.rs` |
| `tcg_out_goto_tb` | `X86_64CodeGen::emit_goto_tb` | `accel/src/x86_64/emitter.rs` |
| `tcg_out_goto_ptr` | `X86_64CodeGen::emit_goto_ptr` | `accel/src/x86_64/emitter.rs` |
| `tcg_gen_op*` (IR emission) | `Context::gen_*` | `core/src/ir_builder.rs` |
| `liveness_pass_1` | `liveness_analysis()` | `accel/src/liveness.rs` |
| `tcg_optimize` | `optimize()` | `accel/src/optimize.rs` |
| `tcg_reg_alloc_op` | `regalloc_op()` | `accel/src/regalloc.rs` |
| `TCGArgConstraint` | `ArgConstraint` | `accel/src/constraint.rs` |
| `C_O*_I*` macros | `o1_i2()` / `o1_i2_alias()` etc. | `accel/src/constraint.rs` |
| `tcg_target_op_def` | `op_constraint()` | `accel/src/x86_64/constraints.rs` |
| `tcg_out_op` (dispatch) | `HostCodeGen::tcg_out_op` | `accel/src/x86_64/codegen.rs` |
| `tcg_out_mov` | `HostCodeGen::tcg_out_mov` | `accel/src/x86_64/codegen.rs` |
| `tcg_out_movi` | `HostCodeGen::tcg_out_movi` | `accel/src/x86_64/codegen.rs` |
| `tcg_out_ld` | `HostCodeGen::tcg_out_ld` | `accel/src/x86_64/codegen.rs` |
| `tcg_out_st` | `HostCodeGen::tcg_out_st` | `accel/src/x86_64/codegen.rs` |
| `tcg_gen_code` | `translate()` | `accel/src/translate.rs` |
| `translator_loop` | `translator_loop()` | `guest/riscv/src/lib.rs` |
| `DisasContextBase` | `DisasContextBase` | `guest/riscv/src/lib.rs` |
| `disas_log` (decodetree) | `decode::generate()` | `decode/src/lib.rs` |
| `target/riscv/translate.c` | `RiscvDisasContext` | `guest/riscv/src/riscv/mod.rs` |
| `trans_rvi.c.inc` (gen_xxx) | `gen_arith/gen_branch/...` | `guest/riscv/src/riscv/trans/` |
| `cpu_exec` | `cpu_exec_loop_mt()` | `accel/src/exec/exec_loop.rs` |
| `tb_lookup` | `tb_find()` | `accel/src/exec/exec_loop.rs` |
| `tb_gen_code` | `tb_gen_code()` | `accel/src/exec/exec_loop.rs` |
| `cpu_tb_exec` | `cpu_tb_exec()` | `accel/src/exec/exec_loop.rs` |
| `tb_add_jump` | `tb_add_jump()` | `accel/src/exec/exec_loop.rs` |
| `TBContext.htable` | `TbStore` | `accel/src/exec/tb_store.rs` |
| `hw/core/qdev.c` | `Device` trait | `hw/core/src/qdev.rs` |
| `hw/core/irq.c` | `IrqSink/IrqLine` | `hw/core/src/irq.rs` |
| `chardev/char.c` | `Chardev` trait | `hw/core/src/chardev.rs` |
| `hw/riscv/virt.c` | `RefMachine` | `hw/riscv/src/ref_machine.rs` |
| `hw/intc/sifive_plic.c` | `Plic` | `hw/intc/src/plic.rs` |
| `hw/intc/sifive_clint.c` | `Aclint` | `hw/intc/src/aclint.rs` |
| `hw/char/serial.c` | `Uart16550` | `hw/char/src/uart.rs` |
| `hw/core/loader.c` | `loader::load_binary()` | `hw/core/src/loader.rs` |
| `softmmu/memory.c` | `AddressSpace/MemoryRegion` | `memory/src/` |
| `target/riscv/cpu_helper.c` (MMU) | `Sv39Mmu` | `guest/riscv/src/riscv/mmu.rs` |
| `target/riscv/pmp.c` | `Pmp` | `guest/riscv/src/riscv/pmp.rs` |
| `cpus.c` (cpu_thread) | `CpuManager` | `system/src/lib.rs` |
| `Machine` (QOM) | `Machine` trait | `core/src/machine.rs` |
