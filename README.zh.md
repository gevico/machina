<h1 align="center">Machina (/ˈmɑːkɪnə/)</h1>
<p align="center">
  <a href="README.md">English</a> | 中文
</p>

一个用 Rust 编写的模块化 RISC-V 全系统模拟器，采用 JIT 动态二进制翻译引擎，支持硬件设备模型、中断控制器和机器固件。

> **状态**：JIT 流水线——RISC-V 客户指令解码、TCG IR 生成、优化（常量折叠、拷贝传播、代数简化）、寄存器分配和 x86-64 代码生成——已完整实现，支持多线程 vCPU 并发模拟和直接 TB 链路。Full-system 模式可引导 RISC-V 参考机器，包含 PLIC、ACLINT、UART、Sv39 MMU 和 SBI 固件接口。

## 架构

```
+-----------+   +----------+   +----------+   +-----------+   +----------+
|   Guest   |-->| Frontend |-->| IR Build |-->| Optimizer |-->| Backend  |
|   Binary  |   | (decode, |   | (gen_*)  |   |           |   | (x86-64) |
|   (RV64)  |   |  trans_*)|   +----------+   +-----------+   +-----+----+
+-----------+   +----------+                                        |
                                                                    v
                        +----------------------------------------------+
                        |               Execution Engine               |
                        |  TB Cache + Multi-vCPU + Chaining + MMIO      |
                        +----------------------+-----------------------+
                                               |
                                               v
                        +----------------------------------------------+
                        |            Full-System Emulation             |
                        |  riscv64-ref: PLIC + ACLINT + UART + FDT     |
                        |  Sv39 MMU + SBI Firmware Interface           |
                        +----------------------------------------------+
```

## Workspace 结构

| Crate | 路径 | 描述 |
|-------|------|------|
| **machina** | `src/` | CLI 入口（`machina -M riscv64-ref -bios fw.bin`） |
| **machina-core** | `core/` | IR 定义（opcodes、types、temps、ops、context、labels、TBs）、CPU trait、地址类型 |
| **machina-accel** | `accel/` | IR 优化器、活跃性分析、寄存器分配器、x86-64 代码生成、多线程 vCPU 执行引擎 |
| **machina-guest-riscv** | `guest/riscv/` | RISC-V 前端：RV64GC + 特权指令（188 条指令）、Sv39 MMU、TLB、PMP |
| **machina-decode** | `decode/` | QEMU 风格 `.decode` 文件解析器与 Rust 解码器生成器 |
| **machina-system** | `system/` | 全系统 CPU 桥接、CpuManager、WFI 唤醒 |
| **machina-memory** | `memory/` | AddressSpace、内存区域、MMIO 分发、RAM 块 |
| **machina-hw-core** | `hw/core/` | 设备基础设施：qdev 模型、IRQ、chardev、clock、FDT、镜像加载器 |
| **machina-hw-intc** | `hw/intc/` | 中断控制器：PLIC、ACLINT（MTIMER + MSWI） |
| **machina-hw-char** | `hw/char/` | 字符设备：UART 16550A |
| **machina-hw-riscv** | `hw/riscv/` | RISC-V 参考机器（`riscv64-ref`）、引导序列、SBI 桩 |
| **machina-disas** | `disas/` | RISC-V 指令反汇编器 |
| **machina-monitor** | `monitor/` | 调试/监控接口（开发中） |
| **machina-util** | `util/` | 共享工具库 |
| **machina-tests** | `tests/` | 964 个测试：单元、后端、前端、差分、集成、多 vCPU 并发、机器级 |
| **machina-mtest** | `tests/mtest/` | 机器级测试框架 |
| **machina-irdump** | `tools/irdump/` | IR 转储调试工具 |
| **machina-irbackend** | `tools/irbackend/` | IR 后端检查工具 |

## 构建

```bash
cargo build                  # 构建所有 crate
cargo build --release        # Release 构建
cargo test --workspace       # 运行全部 964 个测试
cargo clippy -- -D warnings  # Lint 检查
cargo fmt --check            # 格式检查
```

## 运行

```bash
# 引导 RISC-V 参考机器
cargo run --release --bin machina -- -M riscv64-ref -m 128M -bios fw.bin -nographic
```

## 关键设计决策

- **统一类型多态 Opcodes**：单个 `Add` 同时适用于 I32 和 I64（类型由 `Op::op_type` 携带），相比 QEMU 的分裂设计减少约 40% 的 opcode 数量。
- **约束驱动寄存器分配**：声明式 `ArgConstraint`/`OpConstraint` 类型——分配器完全通用，无 per-opcode 分支。新增 opcode 只需添加约束表条目。
- **基于 Trait 的可扩展性**：后端 `HostCodeGen`、前端 `TranslatorOps`、客户架构 `Cpu`——无条件编译。
- **最小化 `unsafe`**：限制在 JIT 缓冲区（mmap/mprotect）、生成代码执行和客户内存访问中。所有 IR 操作均为安全 Rust。
- **QEMU 兼容设备模型**：qdev 层级、IRQ sink、FDT 生成、chardev 抽象——遵循 QEMU hw/ 设计模式。

## 已实现内容

### JIT 引擎（machina-accel）

- **IR 优化器**：常量折叠、拷贝传播、代数简化、分支常量折叠
- **活跃性分析**：反向遍历计算 dead/sync 标志
- **寄存器分配器**：约束驱动贪心分配器，对齐 QEMU 的 `tcg_reg_alloc_op()`
- **x86-64 后端**：完整 GPR 指令编码器（算术、移位、数据移动、内存、乘除、位操作、分支、setcc/cmovcc），System V ABI prologue/epilogue，`goto_tb`/`exit_tb`/`goto_ptr`
- **执行引擎**：多线程 vCPU 执行循环、TB 存储（jump cache + 全局 hash）、直接 TB 链路、`next_tb_hint`、`exit_target` 原子缓存、MMIO helper 分发

### RISC-V 前端（machina-guest-riscv）

- **188 条指令**：RV64I（完整）、RV64M（mul/div/rem）、RV64F/RV64D（浮点算术、load/store、类型转换、比较、FMA）、RVC（压缩指令）、特权指令（CSR、ECALL、MRET/SRET、SFENCE.VMA、WFI）
- **特权 ISA**：Sv39 MMU（含 TLB）、物理内存保护（PMP）、M/S/U 特权级
- **解码器生成**：QEMU 风格 `.decode` 文件在编译时生成 Rust 解码器

### 全系统仿真（machina-system + hw/*）

- **参考机器**（`riscv64-ref`）：集成 CPU、RAM、PLIC、ACLINT、UART、FDT 的完整板级
- **中断控制器**：PLIC（外部中断，优先级/阈值）、ACLINT（MTIMER + MSWI）
- **字符设备**：UART 16550A，支持 chardev 后端和 `-nographic` stdio
- **内存子系统**：层级内存区域、AddressSpace 平坦视图、MMIO 分发
- **CPU 管理**：CpuManager，WFI condvar 唤醒，IRQ 传递到 `mip`
- **引导**：固件/内核加载、FDT 生成（含设备 phandle）、SBI 桩
- **设备基础设施**：qdev 模型、IRQ sink、clock、FDT 构建器、镜像加载器

### 测试体系（964 个测试）

- **单元测试**：核心数据结构、IR API、后端指令编码
- **前端测试**：91 个 RISC-V 指令测试，覆盖完整 decode -> IR -> codegen -> execute 流水线
- **差分测试**：对比 QEMU 验证指令正确性
- **集成测试**：端到端流水线——ALU、分支、循环、内存、复杂序列
- **多 vCPU 并发测试**：并发查找/翻译/链路（26 个测试）
- **机器测试**：全系统引导和设备测试

## QEMU 参考

本项目参考 QEMU 源码树获取架构指导：

- **TCG 核心**：`tcg/tcg.c`、`tcg/tcg-op.c`、`tcg/optimize.c`、`include/tcg/tcg.h`、`include/tcg/tcg-opc.h`
- **x86-64 后端**：`tcg/i386/tcg-target.c.inc`
- **RISC-V 前端**：`target/riscv/translate.c`、`target/riscv/insn_trans/`
- **执行层**：`accel/tcg/cpu-exec.c`、`accel/tcg/tb-maint.c`、`accel/tcg/translator.c`
- **硬件模型**：`hw/riscv/`、`hw/intc/`、`hw/char/`、`hw/core/`
- **文档**：`docs/devel/tcg.rst`、`multi-thread-tcg.rst`、`decodetree.rst`

## 文档

- [设计文档](docs/zh/design.md) — 架构、数据结构、翻译流水线
- [IR Ops](docs/zh/ir-ops.md) — Opcode 目录、Op 结构、IR Builder API
- [x86-64 后端](docs/zh/x86_64-backend.md) — 指令编码器、约束表、codegen 分派
- [性能分析](docs/zh/performance.md) — 优化手段与 QEMU 对比
- [测试体系](docs/zh/testing.md) — 测试架构、差分测试
- [代码风格](docs/zh/coding-style.md) — 命名规范、格式规则

## 许可证

[MIT](LICENSE)
