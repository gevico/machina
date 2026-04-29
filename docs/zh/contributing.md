# Machina 贡献指南

> 目标读者：向 machina 贡献代码的开发者。

## 目录

- [Part 1: 代码风格](#part-1-代码风格)
- [Part 2: 编码指南](#part-2-编码指南)
- [Part 3: Rust 指南](#part-3-rust-指南)
- [Part 4: Git 指南](#part-4-git-指南)
- [Part 5: 测试指南](#part-5-测试指南)

---

## Part 1: 代码风格

### 1. 行宽与缩进

- **行宽上限 80 列**，所有代码和代码注释均遵守
- `.md` 文档文件不受 80 列限制
- 缩进使用 **4 个空格**，禁止使用 Tab
- 续行对齐到上一行的参数起始位置，或缩进 4 个空格

```rust
// Good: 80 列以内，续行对齐
fn emit_modrm_offset(
    buf: &mut CodeBuffer,
    opc: u32,
    r: Reg,
    base: Reg,
    offset: i32,
) {
    // ...
}

// Good: 短函数签名可以单行
fn emit_ret(buf: &mut CodeBuffer) {
    buf.emit_u8(0xC3);
}
```

### 2. 格式化工具

- 提交前必须运行 `cargo fmt`
- 提交前必须通过 `cargo clippy -- -D warnings`
- 使用 `(-128..=127).contains(&x)` 替代
  `x >= -128 && x <= 127`
- 运算符优先级不明确时必须加括号：
  `(OPC + (x << 3)) | flag` 而非
  `OPC + (x << 3) | flag`

### 3. 命名规范

#### 3.1 通用规则

| 类型 | 风格 | 示例 |
|------|------|------|
| 类型/Trait | UpperCamelCase | `ArithOp`, `CodeBuffer` |
| 函数/方法 | snake_case | `emit_arith_rr`, `low3` |
| 局部变量 | snake_case | `rex`, `offset` |
| 常量 | SCREAMING_SNAKE_CASE | `P_REXW`, `STACK_ADDEND` |
| 枚举变体 | UpperCamelCase | `ArithOp::Add`, `Reg::Rax` |

#### 3.2 QEMU 风格常量

操作码常量使用 QEMU 原始命名风格以便交叉参考，通过
`#![allow(non_upper_case_globals)]` 抑制警告：

```rust
pub const OPC_ARITH_EvIb: u32 = 0x83;
pub const OPC_MOVL_GvEv: u32 = 0x8B;
pub const OPC_JCC_long: u32 = 0x80 | P_EXT;
```

#### 3.3 函数命名模式

指令发射器遵循 `emit_<指令>_<操作数模式>` 模式：

```
emit_arith_rr   -- 算术 reg, reg
emit_arith_ri   -- 算术 reg, imm
emit_arith_mr   -- 算术 [mem], reg
emit_arith_rm   -- 算术 reg, [mem]
emit_mov_rr     -- MOV reg, reg
emit_mov_ri     -- MOV reg, imm
emit_load       -- MOV reg, [mem]
emit_store      -- MOV [mem], reg
emit_shift_ri   -- 移位 reg, imm
emit_shift_cl   -- 移位 reg, CL
```

### 4. 注释

- 注释使用**英文**编写
- 仅在逻辑不自明处添加注释，不注释显而易见的代码
- 公开 API 使用 `///` 文档注释，简明扼要
- 内部实现使用 `//` 行注释
- 代码注释同样遵守 80 列行宽（`.md` 文档文件不受此
  限制）

```rust
/// Emit arithmetic reg, reg (ADD/SUB/AND/OR/XOR/CMP).
pub fn emit_arith_rr(
    buf: &mut CodeBuffer,
    op: ArithOp,
    rexw: bool,
    dst: Reg,
    src: Reg,
) {
    let opc =
        (OPC_ARITH_GvEv + ((op as u32) << 3)) | rexw_flag(rexw);
    emit_modrm(buf, opc, dst, src);
}
```

### 5. 类型与枚举

- 枚举使用 `#[repr(u8)]` 或 `#[repr(u16)]` 确保内存布局
- 枚举值显式赋值，不依赖自动递增
- 派生 `Debug, Clone, Copy, PartialEq, Eq`

```rust
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
```

### 6. 函数设计

- 函数参数顺序：`buf` 在前，配置参数居中，操作数在后
- `rexw: bool` 参数控制 32/64 位操作
- 立即数编码自动选择短形式（imm8 vs imm32）
- 函数体尽量短小，复杂逻辑拆分为子函数

```rust
// Good: buf 在前，rexw 居中，操作数在后
pub fn emit_load(
    buf: &mut CodeBuffer,
    rexw: bool,
    dst: Reg,
    base: Reg,
    offset: i32,
) { ... }
```

### 7. unsafe 使用

- `unsafe` 仅限以下场景：
  - JIT 代码缓冲区分配（mmap/mprotect）
  - 调用生成的宿主代码（函数指针转换）
  - 客户内存模拟的原始指针访问
  - 后端内联汇编
  - FFI 接口
- 每个 `unsafe` 块必须有注释说明安全性保证
- 所有其他代码必须是安全的 Rust

### 8. 测试

- 测试位于独立的 `machina-tests` crate
- 每个指令发射器至少一个测试验证字节编码
- 测试覆盖基础寄存器（Rax-Rdi）和扩展寄存器（R8-R15）
- 使用 `emit_bytes` 辅助函数简化测试编写
- 测试函数名使用 snake_case，描述被测行为

```rust
fn emit_bytes(f: impl FnOnce(&mut CodeBuffer)) -> Vec<u8> {
    let mut buf = CodeBuffer::new(4096).unwrap();
    f(&mut buf);
    buf.as_slice().to_vec()
}

#[test]
fn arith_add_rr_64() {
    // add rax, rcx => 48 03 C1
    let code = emit_bytes(|b| {
        emit_arith_rr(b, ArithOp::Add, true, Reg::Rax, Reg::Rcx)
    });
    assert_eq!(code, [0x48, 0x03, 0xC1]);
}
```

### 9. 文档图表规范

文档中的所有图表必须使用纯 ASCII 字符绘制，禁止使用
Unicode box-drawing 字符或其他非 ASCII 符号。

**允许使用的字符**：

| 字符 | 用途 |
|------|------|
| `+` | 边角连接点 |
| `-` | 水平线 |
| `|` | 垂直线 |
| `-->` | 水平箭头（右） |
| `<--` | 水平箭头（左） |
| `v` | 向下箭头 |
| `^` | 向上箭头 |

**边框对齐规则**：

- 所有矩形框的四角必须使用 `+` 字符
- 水平边和垂直边必须严格对齐，不允许锯齿
- 箭头方向使用 `-->` 或 `v`，不使用 Unicode 箭头

```
+------------+     +------------+
|  Frontend  | --> |  IR Builder |
+------------+     +------------+
                        |
                        v
                   +------------+
                   |  Optimizer  |
                   +------------+
                        |
                        v
                   +------------+
                   |  Backend   |
                   +------------+
```

**禁止示例**：

- `─`, `│`, `┌`, `┐`, `└`, `┘` 等 Unicode box-drawing
  字符
- `→`, `←`, `↑`, `↓` 等 Unicode 箭头
- `├`, `┤`, `┬`, `┴`, `┼` 等 Unicode 连接符

### 10. 模块组织

- 每个 crate 的 `lib.rs` 仅做模块声明和 re-export
- 公开类型通过 `pub use` 在 crate 根导出
- 相关功能放在同一文件，文件内按逻辑分节
- 使用 `// -- Section name --` 分隔文件内的逻辑区域

### 11. 多线程 vCPU 与性能代码约束

- 并发路径优先使用"共享状态 + 每线程私有状态"拆分，
  避免热路径共享锁。
- 新增并发字段时，必须在注释中写清：
  - 谁持有写权限（例如 `translate_lock`）
  - 读路径是否 lock-free
  - 可见性保证（Acquire/Release/Relaxed 的选择理由）
- 性能优化提交必须附带可复现基准命令，至少包含：
  - `machina-riscv64` 对 `dhrystone`
  - `qemu-riscv64` 同程序对照
- 涉及 TB 链路逻辑时必须补充：
  - 并发正确性测试（`tests/src/exec/mttcg.rs`）
  - 回归测试（至少一个 guest 测题）
- 调试辅助输出统一走已有统计入口（`TCG_STATS=1`），
  避免在热路径直接打印日志。

---

## Part 2: 编码指南

Machina 项目的通用编码指南。Rust 特定规则见
[Rust 指南](#part-3-rust-指南)，格式和命名约定见
[代码风格](#part-1-代码风格)。

### 命名

#### 描述性命名

禁止单字母命名或含糊缩写。名称应揭示意图。

```rust
// Bad
let v = t.read();
let p = addr >> 12;

// Good
let value = temp.read();
let page_number = addr >> PAGE_SHIFT;
```

#### 准确命名

名称必须与代码的实际行为一致。如果名称是"count"，
值就应该是计数——而不是索引、偏移量或掩码。

#### 名称中编码单位

当值携带物理单位或量级时，将其嵌入名称。

```rust
let timeout_ms = 5000;
let frame_size_in_pages = 4;
let clock_freq_hz = 12_000_000;
```

#### 布尔值命名

使用断言式命名：`is_*`、`has_*`、`can_*`、`should_*`。

```rust
let is_kernel_mode = mode == Prv::M;
let has_side_effects =
    op.flags().contains(OpFlags::SIDE_EFFECT);
```

### 注释

#### 解释为什么，而不是什么

重复代码的注释是噪音。解释非显而易见决策背后的原因。

```rust
// Bad: 重复代码
// Check if page is present
if pte.flags().contains(PteFlags::V) { ... }

// Good: 解释约束
// Sv39 spec: fetch falls through when V=0 in S-mode
// only traps in M-mode (spec 4.3.1)
if pte.flags().contains(PteFlags::V) { ... }
```

#### 记录设计决策

当存在多种方案时，记录选择当前方案的原因。未来的
读者需要理解权衡，而不仅仅是结果。

#### 引用规范

实现硬件行为时，引用规范章节。

```rust
// RISC-V Privileged Spec 4.3.1 -- PTE attribute for
// global mappings
const PTE_G: u64 = 0x10;
```

### 文件组织

#### 每个文件一个概念

当文件过长或混合了不相关的职责时，拆分文件。名为
`mmu.rs` 的文件不应包含中断处理逻辑。

#### 自上而下的阅读顺序

高层入口点放在前面。辅助函数和内部细节放在后面。
读者应能通过阅读第一个部分来理解公共 API。

#### 分组为逻辑段落

在函数内部，将相关语句分组在一起。用空行分隔不同
的组。每组应表达算法中的一个步骤。

### API 设计

#### 隐藏实现细节

默认使用最窄的可见性。只暴露调用者需要的内容。

```rust
// Prefer
pub(crate) fn translate_one(ctx: &mut Context) { ... }

// Avoid
pub fn translate_one(ctx: &mut Context) { ... }
```

#### 边界验证，内部信任

在公共 API 边界（如 syscall 入口、设备 MMIO 写入）
验证输入。在 crate 内部，信任已验证的值。

#### 用类型强制不变量

如果值有约束，用类型系统编码，而不是在每个使用点
检查。

```rust
// Prefer: 非法状态不可表示
pub struct PhysicalPage(u64);

impl PhysicalPage {
    pub fn new(frame: u64) -> Option<Self> {
        (frame < MAX_PHYS_PAGE)
            .then_some(PhysicalPage(frame))
    }
}

// Avoid: 原始 u64 可以是任何值
fn map_page(frame: u64) { ... }
```

### 错误消息

一致地格式化错误消息。包含失败的操作、涉及的值或
标识符，以及（适用时）期望的范围。

```
"invalid PTE at {vpn}: reserved bits set"
"out of TB cache: capacity {cap}, requested {size}"
```

---

## Part 3: Rust 指南

Machina 项目的 Rust 特定指南。通用编码规则见
[编码指南](#part-2-编码指南)，格式约定见
[代码风格](#part-1-代码风格)。

### Unsafe Rust

#### 每次使用 unsafe 都需要理由

每个 `unsafe` 块都需要 `// SAFETY:` 注释解释操作
为何安全。每个 `unsafe fn` 或 `unsafe trait` 都需要
`# Safety` 文档段落，描述调用者必须满足的条件。

```rust
// SAFETY: buf points to a valid, RWX-mapped region of
// `len` bytes. The mmap call above guaranteed alignment
// and permissions.
unsafe {
    core::ptr::copy_nonoverlapping(src.as_ptr(), buf, len)
}
```

#### unsafe 仅限于特定模块

`unsafe` 仅允许用于 JIT 代码缓冲区管理、生成代码的
函数指针转换、TLB 快速路径中的原始指针访问、后端
发射器中的内联汇编和 FFI。所有其他代码必须是安全
Rust。

### 函数

#### 保持函数小而专注

一个函数做一件事。如果需要注释来分隔段落，考虑
拆分。

#### 最小化嵌套

目标最多 3 层嵌套。使用提前返回、`let...else` 和
`?` 来展平控制流。

```rust
// Prefer
let Some(pte) = page_table.walk(vpn) else {
    return Err(MmuFault::InvalidPte);
};

// Avoid
if let Some(pte) = page_table.walk(vpn) {
    // ... deeply nested logic ...
}
```

#### 避免布尔参数

使用枚举或拆分为两个函数。

```rust
// Prefer
pub fn emit_load_signed(
    buf: &mut CodeBuffer, ...
) { ... }
pub fn emit_load_unsigned(
    buf: &mut CodeBuffer, ...
) { ... }

// Avoid
pub fn emit_load(
    buf: &mut CodeBuffer, signed: bool, ...
) { ... }
```

### 类型与 Trait

#### 封闭集合优先用枚举

当变体集合在编译期已知时，使用枚举。trait 对象
（`dyn Trait`）仅适用于开放式扩展。

```rust
// Prefer
enum Exception {
    InstructionAccessFault,
    LoadAccessFault,
    StoreAccessFault,
    // ...
}

// Avoid
trait Exception { fn handle(&self); }
```

#### 通过 getter 封装字段

通过方法暴露字段而非直接 `pub`。这保留了后续添加
验证或日志的能力。

### 模块与 Crate

#### 默认使用最窄可见性

默认使用 `pub(super)` 或 `pub(crate)`。仅在外部
crate 确实需要访问时才使用 `pub`。

#### 通过父模块限定函数导入

导入父模块，然后通过它调用函数。这使来源更明确。

```rust
// Prefer
use core::mem;
mem::replace(&mut slot, new_value)

// Avoid
use core::mem::replace;
replace(&mut slot, new_value)
```

#### 使用 workspace 依赖

所有共享依赖版本必须在 workspace 根 `Cargo.toml` 的
`[workspace.dependencies]` 中声明。各 crate 使用
`{ workspace = true }` 引用。

### 错误处理

#### 用 `?` 传播错误

在可能失败的地方不要 `.unwrap()` 或 `.expect()`。
使用 `?` 将错误传播给调用者。

#### 定义领域错误类型

使用专用的错误枚举而非泛型 `String` 或
`Box<dyn Error>`。

```rust
#[derive(Debug)]
enum TranslateError {
    InvalidOpcode(u32),
    UnsupportedExtension(char),
    BufferOverflow {
        requested: usize,
        available: usize,
    },
}
```

### 并发

#### 记录锁顺序

当存在多个锁时，文档化获取顺序并始终一致地遵守，
以防止死锁。

#### 自旋锁下不做 I/O

持有自旋锁时绝不执行 I/O 或阻塞操作。这包括内存
分配和打印语句。

#### 避免随意使用原子操作

`Ordering` 非常微妙。默认使用 `SeqCst`。仅在具有
文档化的性能原因且正确性论证已写入注释时，才放宽为
`Acquire`/`Release`/`Relaxed`。

### 性能

#### 热路径禁止 O(n)

翻译快速路径（TB 查找、代码执行、TLB 遍历）必须
避免 O(n) 操作。使用哈希表、直接映射缓存或索引数组。

#### 最小化不必要的拷贝

通过引用传递大型结构体。当不需要所有权时使用
`&[u8]` 而非 `Vec<u8>`。避免在每次迭代中克隆
`Arc`。

#### 不要过早优化

优化提交必须包含显示改进的基准测试。

### 宏与属性

#### 函数优先于宏

仅在函数无法完成工作时使用宏（例如重复声明、生成
match 分支）。

#### 在最窄范围抑制 lint

将 `#[allow(...)]` 或 `#[expect(...)]` 应用于特定项，
而非整个模块。

#### derive trait 按字母排序

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
```

#### Workspace lints

每个 workspace 成员必须包含：

```toml
[lints]
workspace = true
```

---

## Part 4: Git 指南

Machina 项目的 Git 提交和 Pull Request 约定。

### 提交消息

#### 格式

```
module: subject

Body describing what changed and why.

Signed-off-by: Name <email>
```

#### Subject 行

- 格式：`module: subject`
- 祈使语气：`add`、`fix`、`remove`——而非 `added`、
  `fixed`、`removed`
- 小写 subject，不加句号
- 总长度不超过 72 字符

#### 常用 module 前缀

| Module | 范围 |
|--------|------|
| `core` | IR 类型、操作码、CPU trait |
| `accel` | IR 优化、寄存器分配、代码生成、执行引擎 |
| `guest/riscv` | RISC-V 前端（解码、翻译） |
| `decode` | .decode 解析器和代码生成 |
| `system` | CPU 管理器、GDB 桥接、WFI |
| `memory` | AddressSpace、MMIO、RAM 块 |
| `hw/core` | 设备基础设施（qdev、IRQ、FDT） |
| `hw/intc` | PLIC、ACLINT |
| `hw/char` | UART |
| `hw/riscv` | 参考机器、启动、SBI |
| `hw/virtio` | VirtIO MMIO 传输和设备 |
| `monitor` | QMP/HMP 控制台 |
| `gdbstub` | GDB 远程协议 |
| `difftest` | 差分测试客户端 |
| `tests` | 测试套件 |
| `docs` | 文档 |
| `project` | 跨模块变更（CI、Makefile、配置） |

#### 常用动词前缀

| 动词 | 用途 |
|------|------|
| `Fix` | 修复缺陷 |
| `Add` | 引入新功能 |
| `Remove` | 删除代码或功能 |
| `Refactor` | 不改变行为的重构 |
| `Rename` | 更改文件、模块或符号名称 |
| `Implement` | 添加新的子系统或功能 |
| `Enable` | 启用之前禁用的功能 |
| `Clean up` | 无功能变更的小清理 |
| `Bump` | 更新依赖版本 |

#### Body

- 与 subject 之间空一行
- 描述修改了什么以及为什么——而非如何实现
- 每行不超过 80 字符

#### 示例

```
accel: fix register clobber in div/rem helpers

The x86-64 backend used RDX as a scratch register for
division without saving the guest's original value. Add
save/restore around the DIV instruction.

Signed-off-by: Chao Liu <chao.liu.zevorn@gmail.com>
```

```
guest/riscv: implement Zbs (single-bit operations)

Add bclr, bset, binv, bext for both register and immediate
forms. The decoder now recognizes the Zbs extension when
enabled in misa.

Signed-off-by: Chao Liu <chao.liu.zevorn@gmail.com>
```

### 原子提交

#### 一个提交只做一件事

每个提交必须只做一个逻辑变更。不要在单个提交中混合
不相关的变更。如果你在提交消息中写了"以及"，就应该
拆分。

#### 每个提交必须编译通过且测试通过

每个提交之后，代码树必须处于可工作状态。禁止有破坏
性的中间状态。这确保 `git bisect` 始终有效。

#### 提交 PR 前合并临时补丁

在提交 PR 之前审查自己的分支时，将所有临时提交合并
到它们所属的提交中。临时提交包括：

- 修正早期提交中引入的拼写错误或缺陷的修复补丁
- 调整先前变更的补丁（如重命名、重新排序）
- 任何消息以 `fixup!` 或 `squash!` 开头的提交

使用 `git rebase -i` 将这些临时提交折叠到原始提交
中。最终 PR 的历史应该是一组干净的逻辑变更序列，
而不是开发日记。

#### 重构与功能分离

如果功能需要预备性的重构，将重构放在独立的提交中，
在功能提交之前。这使得每个提交更容易审查和二分定位。

### Signed-off-by

本仓库的所有提交必须包含 `Signed-off-by` 行：

```
Signed-off-by: Chao Liu <chao.liu.zevorn@gmail.com>
```

禁止添加 AI 相关的签名行（如
`Co-Authored-By: Claude`）。

### Pull Request

#### 保持 PR 专注

一个 PR 一个主题。混合了缺陷修复、重构和新功能的
PR 难以审查。

#### CI 必须通过

请求 review 前确保所有 CI 检查通过：

- `make test`——所有测试通过
- `make clippy`——零警告
- `make fmt-check`——格式正确

#### 引用 issue

当 PR 解决某个 issue 时，在描述中引用：

```
Closes #42
```

---

## Part 5: 测试指南

### 快速参考

#### Rust 测试命令

```bash
# 全量测试
cargo test

# 按 crate 运行
cargo test -p machina-core        # 核心 IR 数据结构
cargo test -p machina-accel       # 后端指令编码 + 执行循环
cargo test -p machina-tests       # 主测试 crate
                                  #（含全部分层测试）

# 按模块过滤
cargo test -p machina-tests core::        # 仅 core 模块
cargo test -p machina-tests backend::     # 仅 backend 模块
cargo test -p machina-tests decode::      # 仅 decode 模块
cargo test -p machina-tests frontend::    # 仅前端指令测试
cargo test -p machina-tests integration:: # 仅集成测试
cargo test -p machina-tests difftest      # 仅差分测试
cargo test -p machina-tests machine::     # 仅机器级测试

# 运行单个测试
cargo test -- test_addi
cargo test -- test_c_li

# 查看详细输出
cargo test -- --nocapture

# 并行控制
cargo test -- --test-threads=1    # 串行（调试用）
cargo test -- --test-threads=4    # 4 线程
```

#### 代码质量检查

```bash
cargo clippy -- -D warnings       # Lint 零警告
cargo fmt --check                  # 格式检查
cargo fmt                          # 自动格式化
```

#### 多 vCPU 并发与性能回归

```bash
# 多 vCPU 并发回归
cargo test -p machina-tests exec::mttcg -- --nocapture

# 打印执行统计（TB 命中率、链路 patch、hint 命中）
TCG_STATS=1 target/release/machina <machine-config>

# 简单性能对照（本机基线）
TIMEFORMAT=%R; time target/release/machina <machine-config>
```

### RVC 压缩指令测试

**编码器辅助函数**（`tests/src/frontend/mod.rs`）：

| 格式编码器 | RVC 格式 |
|-----------|----------|
| `rv_ci(f3, imm5, rd, imm4_0, op)` | CI 格式 |
| `rv_cr(f4, rd, rs2, op)` | CR 格式 |
| `rv_css(f3, imm, rs2, op)` | CSS 格式 |
| `rv_ciw(f3, imm, rdp, op)` | CIW 格式 |
| `rv_cl(f3, imm_hi, rs1p, imm_lo, rdp, op)` | CL 格式 |
| `rv_cs(f3, imm_hi, rs1p, imm_lo, rs2p, op)` | CS 格式 |
| `rv_cb(f3, off_hi, rs1p, off_lo, op)` | CB 格式 |
| `rv_cj(f3, target, op)` | CJ 格式 |

**指令编码器**：c_li, c_addi, c_lui, c_mv, c_add,
c_sub, c_slli, c_addi4spn, c_addiw, c_j, c_beqz, c_bnez,
c_ebreak。

| 测试 | 验证内容 |
|------|----------|
| `test_c_li` | C.LI rd, imm --> rd = sext(imm) |
| `test_c_addi` | C.ADDI rd, nzimm --> rd += sext(nzimm) |
| `test_c_lui` | C.LUI rd, nzimm --> rd = sext(nzimm<<12) |
| `test_c_mv` | C.MV rd, rs2 --> rd = rs2 |
| `test_c_add` | C.ADD rd, rs2 --> rd += rs2 |
| `test_c_sub` | C.SUB rd', rs2' --> rd' -= rs2' |
| `test_c_slli` | C.SLLI rd, shamt --> rd <<= shamt |
| `test_c_addi4spn` | C.ADDI4SPN rd', nzuimm --> rd' = sp + nzuimm |
| `test_c_addiw` | C.ADDIW rd, imm --> rd = sext32(rd + imm) |
| `test_c_j` | C.J offset --> PC 跳转 |
| `test_c_beqz_*` | C.BEQZ taken / not-taken |
| `test_c_bnez_*` | C.BNEZ taken / not-taken |
| `test_c_ebreak` | C.EBREAK --> exit |
| `test_mixed_32_16` | 混合 32-bit + 16-bit 指令序列 |

```bash
cargo test -p machina-tests frontend::    # 全部前端测试
cargo test -p machina-tests test_c_       # 仅 RVC 测试
cargo test -p machina-tests test_mixed    # 混合指令测试
```

### 差分测试

#### 当前覆盖

| 类别 | 指令 | 数量 |
|------|------|------|
| R-type ALU | add, sub, sll, srl, sra, slt, sltu, xor, or, and | 10 |
| I-type ALU | addi, slti, sltiu, xori, ori, andi, slli, srli, srai | 9 |
| LUI | lui | 1 |
| W-suffix R | addw, subw, sllw, srlw, sraw | 5 |
| W-suffix I | addiw, slliw, srliw, sraiw | 4 |
| Branch | beq, bne, blt, bge, bltu, bgeu | 6 |

**未覆盖**（待扩展）：
- Load/Store（lb/lh/lw/ld/sb/sh/sw/sd）
- M 扩展（mul/div/rem 系列）
- auipc, jal, jalr（PC 相关，需特殊处理）

#### 新增 Difftest 指南

**新增 R-type 指令**（以 `mulw` 为例）：

```rust
#[test]
fn difftest_mulw() {
    let cases: Vec<(u64, u64)> = vec![
        (V0, V0),
        (V1, VNEG1),
        (V32MAX, 2),
        (VPATTERN, V32FF),
    ];
    for (a, b) in cases {
        difftest_alu(&rtype_test(
            "mulw", "mulw", mulw(7, 5, 6), a, b,
        ));
    }
}
```

**新增 I-type 指令**（以 `sltiu` 为例）：

```rust
#[test]
fn difftest_sltiu() {
    let cases: Vec<(u64, i32)> = vec![
        (V0, 0), (V0, 1), (VNEG1, -1),
    ];
    for (a, imm) in cases {
        difftest_alu(&itype_test(
            "sltiu",
            &format!("sltiu t2, t0, {imm}"),
            sltiu(7, 5, imm), a,
        ));
    }
}
```

**新增分支指令**：

```rust
#[test]
fn difftest_beq() {
    let cases = vec![
        (V0, V0), (V0, V1), (VNEG1, VNEG1),
    ];
    for (a, b) in cases {
        difftest_branch(&BranchTest {
            name: "beq", mnemonic: "beq",
            insn_fn: beq, rs1_val: a, rs2_val: b,
        });
    }
}
```

**自定义模式**（如 LUI 无源寄存器）：

```rust
difftest_alu(&AluTest {
    name: "lui",
    asm: format!("lui t2, {upper}"),
    insn: lui(7, imm),
    init: vec![],       // 无需初始化源寄存器
    check_reg: 7,
});
```

`AluTest` 字段：`name`（测试名）、`asm`（QEMU
汇编）、`insn`（机器码）、`init`（初始寄存器）、
`check_reg`（比较目标）。

#### 运行与调试

```bash
# 运行全部 difftest
cargo test -p machina-tests difftest

# 运行单个 difftest
cargo test -p machina-tests difftest_add

# 并行运行
cargo test -p machina-tests difftest -- --test-threads=4

# 查看详细输出
cargo test -p machina-tests difftest -- --nocapture
```

**失败输出示例**：

```
DIFFTEST FAIL [add]: x7 machina=0x64 qemu=0x65
```

含义：`add` 指令的 x7 寄存器，machina 计算结果为
`0x64`，QEMU 参考结果为 `0x65`，存在差异。

#### 限制与未来工作

1. **x3(gp) 不可测试**：QEMU 侧保留用于保存区基址
2. **PC 相关指令**：auipc/jal/jalr 需计算相对偏移后
   比较
3. **Load/Store**：待 QemuLd/QemuSt 完善后扩展
4. **随机化测试**：可引入随机寄存器值生成器提高覆盖率
5. **多指令序列**：可扩展为多指令 difftest

### 机器级测试

#### 设备测试

设备测试直接构造设备实例，验证寄存器级行为：

```rust
#[test]
fn test_uart_tx_fifo() {
    let mut uart = Ns16550::new();
    // Write byte to THR, verify LSR shows TX empty
    // after drain.
    uart.write(UART_THR, b'A');
    assert!(uart.read(UART_LSR) & LSR_THRE == 0);
    let out = uart.drain_tx();
    assert_eq!(out, vec![b'A']);
    assert!(uart.read(UART_LSR) & LSR_THRE != 0);
}
```

#### 引导测试

引导测试在完整 VM 中加载裸机固件，验证从复位到输出
的全路径：

```rust
#[test]
fn test_boot_hello() {
    let vm = MachineBuilder::new()
        .memory_size(64 * 1024 * 1024)
        .load_firmware(
            "tests/mtest/bin/boot_hello.bin",
        )
        .build();
    let output =
        vm.run_until_halt(Duration::from_secs(5));
    assert_eq!(
        output.uart_output(),
        "Hello from M-mode!\n",
    );
    assert_eq!(output.exit_code(), 0);
}
```

#### 运行命令

```bash
# 全部机器级测试
cargo test -p machina-tests machine::

# 仅设备模型测试
cargo test -p machina-tests machine::device

# 仅引导流程测试
cargo test -p machina-tests machine::boot

# 构建 mtest 固件（需要交叉编译器）
cd tests/mtest && make
```

### 新增测试指南

#### 新增前端指令测试

在 `tests/src/frontend/mod.rs` 中添加：

```rust
#[test]
fn test_new_insn() {
    let mut cpu = RiscvCpu::new();
    // Set up initial register state.
    cpu.gpr[1] = 100;
    // Encode and run the instruction.
    let insn = rv_i(42, 1, 0b000, 2, 0b0010011);
    run_rv(&mut cpu, insn);
    assert_eq!(cpu.gpr[2], 142);
}
```

#### 新增 RVC 测试

```rust
#[test]
fn test_c_new() {
    let mut cpu = RiscvCpu::new();
    cpu.gpr[10] = 5;
    let insn = c_addi(10, 3); // C.ADDI x10, 3
    run_rvc(&mut cpu, insn);
    assert_eq!(cpu.gpr[10], 8);
}
```

#### 新增 Difftest

参见本文档「新增 Difftest 指南」一节。

#### 新增机器级测试

1. 在 `tests/mtest/src/` 下创建裸机汇编或 C 固件
   源文件
2. 在 `tests/mtest/Makefile` 中添加构建规则
3. 在 `tests/src/machine/` 下添加对应的 Rust 测试
   函数
4. 使用 `MachineBuilder` 构造 VM 实例并验证输出

**设备测试模板**：

```rust
#[test]
fn test_new_device_register() {
    let mut dev = NewDevice::new();
    dev.write(REG_OFFSET, expected_val);
    assert_eq!(dev.read(REG_OFFSET), expected_val);
}
```

**引导测试模板**：

```rust
#[test]
fn test_new_firmware() {
    let vm = MachineBuilder::new()
        .memory_size(64 * 1024 * 1024)
        .load_firmware(
            "tests/mtest/bin/new_test.bin",
        )
        .build();
    let output =
        vm.run_until_halt(Duration::from_secs(5));
    assert!(
        output.uart_output().contains("PASS"),
    );
}
```
