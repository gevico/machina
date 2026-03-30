# CLAUDE.md

本文件为 Claude Code (claude.ai/code) 在本仓库中工作时提供指导。

## 项目概述

Machina 是一个用 Rust 编写的模块化 RISC-V 模拟器，基于 JIT 动态二进制翻译引擎。JIT 核心源自对 QEMU TCG（Tiny Code Generator）的重新实现，参考实现位于 `~/qemu/tcg/`、`~/qemu/accel/tcg/`、`~/qemu/include/tcg/` 和 `~/qemu/hw/`。

**当前状态快照（2026-03-30）**：

- **JIT 引擎**：完整翻译流水线可用——RISC-V 解码、IR 生成、优化、
  寄存器分配、x86-64 代码生成。MTTCG 执行、直接 TB 链路、
  MMIO helper 分发均已就绪。
- **Linux-user 模式**：端到端可用，支持 guest argv 传递，
  包含 dhrystone、hello_printf、hello_float、argv_echo 测题。
- **Full-system 模式**：RISC-V 参考机器（riscv64-ref）可引导，
  集成 PLIC、ACLINT、UART、Sv39 MMU、SBI 固件接口。
- **RISC-V 前端**：188 条指令（RV64GC + 特权指令），含 Sv39 MMU、
  TLB、PMP、M/S/U 特权级。
- **性能**：linux-user 模式下比 QEMU TCG 快约 30%。
- **测试**：964 个测试覆盖全流水线。

## 构建与开发命令

```bash
cargo build                          # 构建所有 crate
cargo build --release                # Release 构建
cargo test --workspace               # 运行所有测试
cargo test -p machina-core           # 测试单个 crate
cargo test -- test_name              # 运行指定测试
cargo clippy -- -D warnings          # Lint 检查
cargo fmt --check                    # 格式检查
cargo fmt                            # 自动格式化
cargo doc --open                     # 生成并打开文档
```

## Git Commit 规范

Commit message 必须使用英文编写。格式如下：

```
module: subject

具体修改内容的详细说明。

Signed-off-by: Name <email>
```

**Subject 行规则**：

- 格式为 `module: subject`，其中 `module` 是受影响的主要模块名
- 常用 module 名：`core`、`accel`、`guest/riscv`、`decode`、`system`、`memory`、`hw/core`、`hw/intc`、`hw/char`、`hw/riscv`、`tests`、`docs`、`project`（跨模块变更）
- subject 使用小写开头，祈使语气（如 `add`、`fix`、`remove`），不加句号
- 总长度不超过 72 字符

**Body 规则**：

- 与 subject 之间空一行
- 说明本次变更的内容和原因（what & why），而非如何实现（how）
- 每行不超过 80 字符

**示例**：

```
core: add vector opcode support

Add V64/V128/V256 vector opcodes to the unified opcode enum.
Each vector op carries OpFlags::VECTOR for backend dispatch.

Signed-off-by: Chao Liu <chao.liu.zevorn@gmail.com>
```

## 架构

### 翻译流水线

```
Guest Binary → Frontend (decode) → TCG IR → Optimizer → Backend (codegen) → Host Binary
                                      ↓
                              TranslationBlock Cache
                                      ↓
                         ┌────────────┴────────────┐
                         │                         │
                    linux-user                full-system
                 (ELF + syscall)        (Devices + MMU + IRQ)
```

### Workspace 结构

| Crate | 路径 | 职责 | QEMU 参考 |
|-------|------|------|----------|
| `machina` | `src/` | CLI 入口 | `softmmu/main.c` |
| `machina-core` | `core/` | IR 定义、CPU trait、地址类型 | `include/tcg/tcg.h`、`tcg/tcg-opc.h` |
| `machina-accel` | `accel/` | IR 优化、寄存器分配、x86-64 codegen、执行引擎 | `tcg/tcg.c`、`tcg/optimize.c`、`tcg/i386/`、`accel/tcg/cpu-exec.c` |
| `machina-guest-riscv` | `guest/riscv/` | RISC-V 前端：RV64GC + 特权 ISA、Sv39 MMU | `target/riscv/translate.c` |
| `machina-decode` | `decode/` | `.decode` 解析器与 Rust 解码器生成器 | `scripts/decodetree.py` |
| `machina-system` | `system/` | 全系统 CPU 桥接、CpuManager、WFI | `softmmu/cpus.c` |
| `machina-memory` | `memory/` | AddressSpace、MMIO 分发、RAM 块 | `softmmu/memory.c` |
| `machina-hw-core` | `hw/core/` | 设备基础设施：qdev、IRQ、chardev、FDT | `hw/core/` |
| `machina-hw-intc` | `hw/intc/` | PLIC、ACLINT（MTIMER + MSWI） | `hw/intc/` |
| `machina-hw-char` | `hw/char/` | UART 16550A | `hw/char/` |
| `machina-hw-riscv` | `hw/riscv/` | RISC-V 参考机器、boot、SBI | `hw/riscv/` |
| `machina-disas` | `disas/` | RISC-V 反汇编器 | `disas/` |
| `machina-monitor` | `monitor/` | 调试接口（WIP） | `monitor/` |
| `machina-util` | `util/` | 共享工具 | `util/` |
| `machina-tests` | `tests/` | 964 个测试 | `tests/tcg/` |
| `machina-mtest` | `tests/mtest/` | 机器级测试 | — |

### 核心数据结构（C → Rust 映射）

| QEMU C 结构 | Rust 等价物 | 用途 |
|-------------|------------|------|
| `TCGOpcode`（DEF 宏枚举） | `enum Opcode` | 158 个统一多态 IR opcodes |
| `TCGType` | `enum Type { I32, I64, I128, V64, V128, V256 }` | IR 值类型 |
| `TCGTemp` | `struct Temp` | IR 变量（global、local、const、fixed-reg） |
| `TCGTempKind` | `enum TempKind { Ebb, Tb, Global, Fixed, Const }` | 变量生命周期/作用域 |
| `TCGOp` | `struct Op` | 单个 IR 操作（opcode + args） |
| `TCGContext` | `struct Context` | 每线程翻译状态 |
| `TCGLabel` | `struct Label` | TB 内的分支目标 |
| `TranslationBlock` | `struct TranslationBlock` | 缓存的翻译代码块 |
| `CPUJumpCache` | `struct JumpCache` | 每 CPU 直接映射 TB 缓存，4096 项 |
| `MemoryRegion` | `struct MemoryRegion` | 层级内存区域 |
| `AddressSpace` | `struct AddressSpace` | 地址空间与 MMIO 分发 |

### 翻译块生命周期

1. **查找**：PC 哈希 → jump cache（每 CPU，4096 项）→ 全局哈希表（32K 桶）
2. **未命中 → 翻译**：前端解码客户指令 → 发射 TCG IR → 优化器运行 → 后端生成宿主代码
3. **缓存**：插入哈希表和 jump cache
4. **执行**：跳转到生成的宿主代码
5. **链接**：修补 TB 间的直接跳转（`goto_tb`/`exit_tb`）
6. **失效**：自修改代码、页面取消映射或缓存满时——解链并移除

热路径优化：

- `next_tb_hint`：执行循环在同一条链路上复用上一次目标 TB，减少重复查找。
- `exit_target`：对 `TB_EXIT_NOCHAIN` 场景缓存最近目标 TB（原子读写）。

### 前端 Trait 设计

```rust
trait TranslatorOps {
    fn init(&mut self, ctx: &mut Context);
    fn translate_insn(&mut self, ctx: &mut Context) -> TranslateResult;
    fn tb_stop(&mut self, ctx: &mut Context);
}
```

### 后端 Trait 设计

```rust
trait HostCodeGen {
    fn emit_prologue(&mut self, buf: &mut CodeBuffer);
    fn emit_epilogue(&mut self, buf: &mut CodeBuffer);
    fn patch_jump(&mut self, buf: &mut CodeBuffer, jump_offset: usize, target_offset: usize);
    fn epilogue_offset(&self) -> usize;
    fn init_context(&self, ctx: &mut Context);
}
```

### Unsafe 边界

`unsafe` 仅在以下场景允许使用：

- JIT 代码缓冲区分配和执行（mmap + mprotect RWX 转换）
- 调用生成的宿主代码（从代码缓冲区进行 `fn()` 指针转换）
- 客户内存模拟的原始指针访问（TLB 快速路径）
- 后端代码发射器中的内联汇编
- 与外部库的 FFI 接口

所有其他代码必须是安全的 Rust。

## QEMU 参考路径

理解原始实现的关键源文件：

- **TCG 核心**：`~/qemu/tcg/tcg.c`、`~/qemu/tcg/tcg-op.c`
- **优化器**：`~/qemu/tcg/optimize.c`
- **执行循环**：`~/qemu/accel/tcg/cpu-exec.c`
- **TB 管理**：`~/qemu/accel/tcg/translate-all.c`、`~/qemu/accel/tcg/tb-maint.c`
- **软件 TLB**：`~/qemu/accel/tcg/cputlb.c`
- **硬件模型**：`~/qemu/hw/riscv/`、`~/qemu/hw/intc/`、`~/qemu/hw/char/`
- **后端**：`~/qemu/tcg/i386/`、`~/qemu/tcg/aarch64/`
- **前端**：`~/qemu/target/riscv/translate.c`
- **Decodetree**：`~/qemu/docs/devel/decodetree.rst`

## 调试手段

- `TCG_STATS=1 target/release/tcg-riscv64 <elf> [args...]`：
  打印 TB 命中率、链路 patch 次数、hint 命中等统计。
- `cargo test -p machina-tests exec::mttcg -- --nocapture`：
  优先复现并发相关问题。
- `RUST_BACKTRACE=1 cargo test -p machina-tests linux_user::guest_dhrystone`：
  复现并定位 guest 执行崩溃。
- 性能对比：
  - `TIMEFORMAT=%R; time target/release/tcg-riscv64 target/guest/riscv64/dhrystone`
  - `TIMEFORMAT=%R; time qemu-riscv64 target/guest/riscv64/dhrystone`

## 代码风格

代码行宽不超过 **80 列**（`.md` 文档文件不受此限制）。详细规范见 [`docs/coding-style.md`](docs/coding-style.md)。

核心规则：

- 缩进使用 4 个空格，禁止 Tab
- 代码行宽上限 80 列，代码注释同样遵守；`.md` 文档文件不限列宽
- 运行 `cargo fmt` 格式化，`cargo clippy -- -D warnings` 零警告
- 注释使用英文，仅在关键逻辑处添加
- 常量命名：QEMU 风格的操作码常量允许 `non_upper_case_globals`
- `unsafe` 仅限 JIT 执行和客户内存访问

## 设计原则

- **不向后兼容**：自由破坏、积极清理，不做迁移垫片。
- **基于 Trait 的可扩展性**：前端和后端是 trait 实现，而非条件编译。
- **枚举驱动的 Opcodes**：用带 `#[repr(u8)]` 的 Rust 枚举替代 C 的 `DEF()` 宏模式。
- **类型安全的 IR 构建器**：利用 Rust 类型系统在编译期防止混用 I32/I64 操作数。
- **QEMU 兼容设备模型**：qdev 层级、IRQ sink、FDT 生成、chardev 抽象。
- **最小化 `unsafe`**：限制在 JIT 执行和客户内存访问中；其他一切使用安全 Rust。
