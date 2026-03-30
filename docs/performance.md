# Machina 性能优化：与 QEMU TCG 对比分析

本文档总结 machina 相比 QEMU TCG 的独有性能优化手段，解释为何
machina 在 linux-user 模式下可以比 QEMU 快接近 30%。

## 1. 执行循环优化

### 1.1 `next_tb_hint` — 跳过 TB 查找

**文件**: `exec/src/exec_loop.rs:52-89`

当 TB 通过 `goto_tb` 链式退出时，machina 将目标 TB 索引存入
`next_tb_hint`。下一轮循环直接复用该索引，完全跳过 jump cache
和全局 hash 查找。

| | machina | QEMU |
|---|--------|------|
| 链式退出后 | 直接复用目标 TB | 仍走 `tb_lookup` 完整路径 |
| 热循环开销 | 接近零（索引比较） | jump cache hash + 比较 |

QEMU 的 `last_tb` 仅用于决定是否 patch 链接，不跳过查找。
在紧密循环（如 dhrystone 主循环）中，hint 命中率极高。

### 1.2 `exit_target` 原子缓存 — 间接跳转加速

**文件**: `exec/src/exec_loop.rs:96-116`, `core/src/tb.rs:55`

对 `TB_EXIT_NOCHAIN`（间接跳转、`jalr` 等），每个 TB 维护一个
`AtomicUsize` 单项缓存，记录上次跳转的目标 TB。

```
间接跳转退出 → 检查 exit_target 缓存
                  ├─ 命中且有效 → 直接复用，跳过 hash 查找
                  └─ 未命中 → 走正常 tb_find，更新缓存
```

QEMU 对所有 `TB_EXIT_NOCHAIN` 都走完整的 QHT 查找路径，
没有这层缓存。两个优化组合后，稳态执行中全局 hash 查找几乎
只在冷启动和 TB 失效时触发。

**估算贡献**: ~8-10%

## 2. Guest 内存访问优化

### 2.1 无软件 TLB — 直接 guest_base 寻址

**文件**: `backend/src/x86_64/codegen.rs:573-639`

machina 在 linux-user 模式下，guest 内存访问直接生成
`[R14 + addr]` 寻址（R14 = guest_base），无 TLB 查找、
无慢速路径 helper 调用。

| | machina | QEMU |
|---|--------|------|
| load/store 生成 | `mov reg, [R14+addr]` | 内联 TLB 快速路径 + 慢速路径分支 |
| 每次访问指令数 | 1-2 条 | 5-10 条（TLB 查找 + 比较 + 分支） |
| 慢速路径 | 无 | helper 函数调用 |

QEMU 即使在 linux-user 模式下也生成完整的软件 TLB 路径，
因为其 `tcg_out_qemu_ld`/`tcg_out_qemu_st` 不区分系统模式
和用户模式。machina 针对 linux-user 场景做了专门优化。

**估算贡献**: ~8-10%

## 3. 数据结构优化

### 3.1 Vec-based IR 存储 vs QEMU 链表

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

### 3.2 HashMap 常量去重 vs 线性扫描

**文件**: `core/src/context.rs:128-138`

machina 用按类型分桶的 `HashMap<u64, TempIdx>` 做常量去重，
O(1) 查找。QEMU 的 `tcg_constant_internal` 线性扫描
`nb_temps`，大型 TB 中常量查找是隐性开销。

### 3.3 `#[repr(u8)]` 紧凑枚举

**文件**: `core/src/opcode.rs`

`Opcode` 枚举用 `#[repr(u8)]` 标注，占 1 字节。QEMU 的
`TCGOpcode` 是 `int`（4 字节）。`Op` 结构体更紧凑，单个
缓存行容纳更多 ops。

**估算贡献**: ~3-5%

## 4. 运行时并发优化

### 4.1 Lock-free TB 读取

**文件**: `exec/src/tb_store.rs:13-64`

TbStore 利用 TB 只追加不删除的特性，用
`UnsafeCell<Vec<TB>>` + `AtomicUsize` 长度实现无锁读取。

```
写入路径（翻译）: translate_lock → push TB → Release store len
读取路径（执行）: Acquire load len → 索引访问（无锁）
```

QEMU 的 QHT 使用 RCU 机制，有额外的 grace period 和
synchronize 开销。machina 的方案更简单，利用了 TB 只追加的
不变量。

### 4.2 RWX 代码缓冲区 — 无 mprotect 切换

**文件**: `backend/src/code_buffer.rs:38-49`

machina 直接 mmap RWX 内存，TB 链接 patch 时无需 mprotect
切换。QEMU 在启用 split-wx 模式时（某些发行版默认开启），
每次 patch 需要 mprotect 系统调用。

### 4.3 简化哈希函数

**文件**: `core/src/tb.rs:106-109`

```rust
let h = pc.wrapping_mul(0x9e3779b97f4a7c15) ^ (flags as u64);
(h as usize) & (TB_HASH_SIZE - 1)
```

黄金比例常数乘法哈希，计算量比 QEMU 的 xxHash 更小。
TB 查找热路径上每次省几个 cycle，累积效果可观。

**估算贡献**: ~2-3%

## 5. 编译管线优化

### 5.1 单遍 IR 优化器

**文件**: `backend/src/optimize.rs`

| | machina | QEMU |
|---|--------|------|
| 遍数 | 单遍 O(n) | 多遍扫描 |
| 常量折叠 | 完整值级别 | 位级（z_mask/o_mask/s_mask） |
| 拷贝传播 | 基础 | 高级 |
| 代数简化 | 基础恒等式 | 复杂模式匹配 |

machina 的优化深度不如 QEMU，但翻译速度更快。对 linux-user
场景下大量短 TB 的翻译，单遍设计的编译时间优势明显。

### 5.2 Rust 零成本抽象

- **单态化**: 前端 `BinOp` 函数指针
  （`frontend/src/riscv/trans.rs:26`）经编译器单态化后内联，
  消除间接调用
- **内联标注**: `CodeBuffer` 的 14 个 `#[inline]` 字节发射
  函数（`backend/src/code_buffer.rs`）被内联到 codegen 调用点
- **枚举判别式**: `#[repr(u8)]` 生成紧凑跳转表

**估算贡献**: ~2-3%

## 6. 指令选择优化

### 6.1 LEA 三地址加法

**文件**: `backend/src/x86_64/codegen.rs:136-147`

当 `Add` 的输出寄存器与两个输入都不同时，使用 LEA 实现
非破坏性三地址加法，避免额外 MOV。QEMU 也有此优化。

### 6.2 无条件 BMI1 指令

**文件**: `backend/src/x86_64/emitter.rs:57-61`

machina 无条件使用 ANDN/LZCNT/TZCNT/POPCNT。QEMU 运行时
检测 CPU 特性后才决定是否使用，检测本身有微小开销，且
fallback 路径更长。

### 6.3 MOV 立即数分级优化

**文件**: `backend/src/x86_64/emitter.rs:547-566`

```
val == 0        → XOR reg, reg          (2 bytes, 破坏依赖链)
val <= u32::MAX → MOV r32, imm32        (5 bytes, 零扩展)
val fits i32    → MOV r64, sign-ext imm (7 bytes)
otherwise       → MOV r64, imm64        (10 bytes)
```

## 7. 性能贡献总览

| 优化类别 | 估算贡献 | 关键技术 |
|---------|---------|---------|
| 执行循环（hint + exit_target） | ~8-10% | 跳过 TB 查找 |
| Guest 内存访问（无 TLB） | ~8-10% | 直接 guest_base 寻址 |
| 数据结构（Vec + 紧凑枚举） | ~3-5% | 缓存友好布局 |
| 运行时并发（lock-free + RWX） | ~2-3% | 无锁读取、无 mprotect |
| 编译管线（单遍 + 内联） | ~2-3% | Rust 零成本抽象 |
| 哈希 + 常量去重 | ~1-2% | 简化计算 |
| **合计** | **~24-33%** | |

## 8. 权衡与局限

machina 的性能优势建立在以下权衡之上：

- **仅 linux-user 模式**: 无软件 TLB 意味着不支持系统模式
- **RWX 内存**: 违反 W^X 安全原则，某些平台（iOS）禁止
- **简化优化器**: 缺少 QEMU 的位级追踪，生成代码质量略低
- **无条件 BMI1**: 假设宿主 CPU 支持，不兼容老旧 CPU
- **简化哈希**: 分布质量不如 xxHash，高冲突率下退化

这些权衡在 linux-user + 现代 x86-64 宿主的目标场景下是合理的。
