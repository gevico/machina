# Machina 测试体系

## 1. 概述

machina 采用分层测试策略，从底层数据结构到完整的用户态模拟器，
逐层验证正确性。测试统一集中在独立的 `tests/` crate 中，保持
源码文件干净，同时验证公共 API 的完整性。

**测试金字塔**：

```
            ┌──────────────┐
            │  Guest 程序   │  linux-user 端到端
            │  (18 tests)  │  ELF 加载 → 执行 → 输出
            ├──────────────┤
            │   Difftest   │  machina vs QEMU
            │  (35 tests)  │  指令级差分对比
            ├──────────────┤
            │  前端指令测试  │  decode → IR → codegen → 执行
            │  (91 tests)  │  RV32I/RV64I/RVC/RV32F
            ├──────────────┤
            │   集成测试    │  IR → liveness → regalloc
            │ (105 tests)  │  → codegen → 执行
       ┌────┴──────────────┴────┐
       │       单元测试          │  core(192) + backend(256)
       │      (567 tests)       │  + decode(93) + exec(26)
       └────────────────────────┘
```

**总计：816 个测试**。

---

## 2. 快速参考

### Rust 测试命令

```bash
# 全量测试
cargo test

# 按 crate 运行
cargo test -p tcg-core          # 核心 IR 数据结构
cargo test -p tcg-backend       # 后端指令编码
cargo test -p tcg-tests         # 主测试 crate（含全部分层测试）
cargo test -p tcg-exec          # 执行循环

# 按模块过滤
cargo test -p tcg-tests core::           # 仅 core 模块
cargo test -p tcg-tests backend::        # 仅 backend 模块
cargo test -p tcg-tests decode::     # 仅 decode 模块
cargo test -p tcg-tests frontend::       # 仅前端指令测试
cargo test -p tcg-tests integration::    # 仅集成测试
cargo test -p tcg-tests difftest         # 仅差分测试
cargo test -p tcg-tests linux_user::     # 仅 linux-user 端到端

# 运行单个测试
cargo test -- test_addi
cargo test -- test_c_li

# 查看详细输出
cargo test -- --nocapture

# 并行控制
cargo test -- --test-threads=1   # 串行（调试用）
cargo test -- --test-threads=4   # 4 线程
```

### 代码质量检查

```bash
cargo clippy -- -D warnings      # Lint 零警告
cargo fmt --check                 # 格式检查
cargo fmt                         # 自动格式化
```

### 客户程序构建

```bash
cd tests/guest
make                              # 构建全部客户程序
make clean                        # 清理构建产物
```

### 客户程序运行

```bash
# 使用 QEMU 验证
qemu-riscv64 target/guest/riscv64/hello
qemu-riscv64 target/guest/riscv64/hello_printf

# 使用 machina 运行（需先 cargo build --release）
cargo run --release --bin tcg-riscv64 -- \
    target/guest/riscv64/hello
```

### MTTCG 与性能回归

```bash
# MTTCG 并发回归
cargo test -p tcg-tests exec::mttcg -- --nocapture

# dhrystone 端到端回归
cargo test -p tcg-tests linux_user::guest_dhrystone -- --nocapture

# 打印执行统计（TB 命中率、链路 patch、hint 命中）
TCG_STATS=1 target/release/tcg-riscv64 target/guest/riscv64/dhrystone

# 简单性能对照（本机基线）
TIMEFORMAT=%R; time target/release/tcg-riscv64 target/guest/riscv64/dhrystone
TIMEFORMAT=%R; time qemu-riscv64 target/guest/riscv64/dhrystone
```

---

## 3. 测试架构

### 目录结构

```
tests/
├── Cargo.toml                    # 依赖：core, backend, frontend,
│                                 #        exec, decode
├── src/
│   ├── lib.rs                    # 模块声明
│   ├── core/                     # 核心 IR 单元测试 (192)
│   │   ├── context.rs
│   │   ├── label.rs
│   │   ├── op.rs
│   │   ├── opcode.rs
│   │   ├── regset.rs
│   │   ├── tb.rs
│   │   ├── temp.rs
│   │   └── types.rs
│   ├── backend/                  # 后端单元测试 (256)
│   │   ├── code_buffer.rs
│   │   ├── x86_64.rs
│   │   └── mod.rs
│   ├── decode/                  # 解码器生成器测试 (93)
│   │   └── mod.rs
│   ├── frontend/                 # 前端指令测试 (109 + 35)
│   │   ├── mod.rs                #   RV32I/RV64I/RVC 执行
│   │   └── difftest.rs           #   machina vs QEMU 差分
│   ├── integration/              # 集成测试 (105)
│   │   └── mod.rs
│   ├── exec/                     # 执行循环测试 (12)
│   │   └── mod.rs
│   └── linux_user/               # linux-user 端到端测试
│       └── mod.rs
└── guest/                        # 交叉编译客户程序
    ├── Makefile
    ├── riscv/
    │   ├── hello.c               # 裸机 syscall
    │   └── hello_printf.c        # 标准 C printf
    └── build/riscv64/            # 编译输出（.gitignore）
```

### 模块测试分布

| 模块 | 测试数 | 占比 | 说明 |
|------|--------|------|------|
| backend | 256 | 31.4% | x86-64 指令编码、代码缓冲区 |
| core | 192 | 23.5% | IR 类型、Opcode、Temp、Label、Op、Context |
| integration | 105 | 12.9% | IR → codegen → 执行全流水线 |
| decode | 93 | 11.4% | .decode 解析、代码生成、字段提取 |
| frontend | 91 | 11.2% | RISC-V 指令执行（含 RVC、RV32F） |
| difftest | 35 | 4.3% | machina vs QEMU 差分对比 |
| exec | 26 | 3.2% | TB 缓存、执行循环、MTTCG 并发 |
| linux_user | 18 | 2.2% | ELF 解析/加载、GuestSpace 与客户程序执行 |

---

## 4. 单元测试

### 4.1 Core 模块（192 tests）

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
cargo test -p tcg-tests core::
```

### 4.2 Backend 模块（256 tests）

验证 x86-64 指令编码器的正确性。

| 文件 | 测试内容 |
|------|----------|
| `code_buffer.rs` | 代码缓冲区分配、写入、mprotect 切换 |
| `x86_64.rs` | 全部 x86-64 指令编码（MOV/ADD/SUB/AND/OR/XOR/SHL/SHR/SAR/MUL/DIV/LEA/Jcc/SETcc/CMOVcc/BSF/BSR/LZCNT/TZCNT/POPCNT 等） |

```bash
cargo test -p tcg-tests backend::
```

### 4.3 Decodetree 模块（93 tests）

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
cargo test -p tcg-tests decode::
```

---

## 5. 集成测试（105 tests）

**源文件**：`tests/src/integration/mod.rs`

验证完整的 IR → liveness → register allocation → codegen → 执行
流水线。使用最小 RISC-V CPU 状态，通过宏批量生成测试用例。

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
cargo test -p tcg-tests integration::
```

---

## 6. 前端指令测试（91 tests）

**源文件**：`tests/src/frontend/mod.rs`

### 6.1 测试运行器

前端测试使用四个运行器函数，覆盖不同的指令格式：

| 函数 | 输入 | 用途 |
|------|------|------|
| `run_rv(cpu, insn: u32)` | 单条 32-bit 指令 | 基础指令测试 |
| `run_rv_insns(cpu, &[u32])` | 32-bit 指令序列 | 多指令序列 |
| `run_rv_bytes(cpu, &[u8])` | 原始字节流 | 混合 16/32-bit |
| `run_rvc(cpu, insn: u16)` | 单条 16-bit 指令 | RVC 压缩指令 |

**执行流程**（以 `run_rv_insns` 为例）：

```
指令编码 → 写入 guest 内存 → translator_loop 解码
→ IR 生成 → liveness → regalloc → x86-64 codegen
→ 执行生成代码 → 读取 CPU 状态 → 断言验证
```

### 6.2 RV32I / RV64I 测试

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

### 6.3 RVC 压缩指令测试

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

**指令编码器**：c_li, c_addi, c_lui, c_mv, c_add, c_sub, c_slli,
c_addi4spn, c_addiw, c_j, c_beqz, c_bnez, c_ebreak。

| 测试 | 验证内容 |
|------|----------|
| `test_c_li` | C.LI rd, imm → rd = sext(imm) |
| `test_c_addi` | C.ADDI rd, nzimm → rd += sext(nzimm) |
| `test_c_lui` | C.LUI rd, nzimm → rd = sext(nzimm<<12) |
| `test_c_mv` | C.MV rd, rs2 → rd = rs2 |
| `test_c_add` | C.ADD rd, rs2 → rd += rs2 |
| `test_c_sub` | C.SUB rd', rs2' → rd' -= rs2' |
| `test_c_slli` | C.SLLI rd, shamt → rd <<= shamt |
| `test_c_addi4spn` | C.ADDI4SPN rd', nzuimm → rd' = sp + nzuimm |
| `test_c_addiw` | C.ADDIW rd, imm → rd = sext32(rd + imm) |
| `test_c_j` | C.J offset → PC 跳转 |
| `test_c_beqz_*` | C.BEQZ taken / not-taken |
| `test_c_bnez_*` | C.BNEZ taken / not-taken |
| `test_c_ebreak` | C.EBREAK → exit |
| `test_mixed_32_16` | 混合 32-bit + 16-bit 指令序列 |

```bash
cargo test -p tcg-tests frontend::       # 全部前端测试
cargo test -p tcg-tests test_c_          # 仅 RVC 测试
cargo test -p tcg-tests test_mixed       # 混合指令测试
```

---

## 7. 差分测试（35 tests）

**源文件**：`tests/src/frontend/difftest.rs`

差分测试对同一条 RISC-V 指令，分别通过 machina 全流水线和
QEMU 参考实现执行，比较 CPU 状态。如果结果一致，则认为
machina 的翻译是正确的。

**依赖工具**：

| 工具 | 安装命令 |
|------|----------|
| `riscv64-linux-gnu-gcc` | `apt install gcc-riscv64-linux-gnu` |
| `qemu-riscv64` | `apt install qemu-user` |

### 7.1 整体架构

```
                    ┌─────────────────────┐
                    │   Test Case 定义     │
                    │  (insn + init regs)  │
                    └────────┬────────────┘
                             │
              ┌──────────────┴──────────────┐
              ▼                             ▼
     ┌────────────────┐           ┌─────────────────┐
     │   machina 侧    │           │    QEMU 侧      │
     │                │           │                 │
     │ 1. 编码指令     │           │ 1. 生成 .S 汇编  │
     │ 2. translator   │           │ 2. gcc 交叉编译  │
     │    _loop 解码   │           │ 3. qemu-riscv64  │
     │ 3. IR 生成      │           │    执行          │
     │ 4. liveness     │           │ 4. 解析 stdout   │
     │ 5. regalloc     │           │    (256 字节     │
     │ 6. x86-64       │           │     寄存器转储)  │
     │    codegen      │           │                 │
     │ 7. 执行         │           │                 │
     └───────┬────────┘           └────────┬────────┘
             │                             │
             ▼                             ▼
     ┌────────────────┐           ┌─────────────────┐
     │  RiscvCpu 状态  │           │  [u64; 32] 数组  │
     │  .gpr[0..32]   │           │  x0..x31 值      │
     └───────┬────────┘           └────────┬────────┘
             │                             │
             └──────────────┬──────────────┘
                            ▼
                   ┌─────────────────┐
                   │  assert_eq!()   │
                   │  比较指定寄存器  │
                   └─────────────────┘
```

### 7.2 QEMU 侧原理

对每个测试用例，框架动态生成一段 RISC-V 汇编源码：

```asm
.global _start
_start:
    la gp, save_area       # x3 = 保存区基址

    # ── Phase 1: 加载初始寄存器值 ──
    li t0, <val1>
    li t1, <val2>

    # ── Phase 2: 执行被测指令 ──
    add t2, t0, t1

    # ── Phase 3: 保存全部 32 个寄存器 ──
    sd x0,  0(gp)
    sd x1,  8(gp)
    ...
    sd x31, 248(gp)

    # ── Phase 4: write(1, save_area, 256) ──
    li a7, 64
    li a0, 1
    mv a1, gp
    li a2, 256
    ecall

    # ── Phase 5: exit(0) ──
    li a7, 93
    li a0, 0
    ecall

.bss
.align 3
save_area: .space 256       # 32 × 8 字节
```

编译与执行流程：

```
gen_alu_asm()          生成 .S 源码
    │
    ▼
riscv64-linux-gnu-gcc  交叉编译
  -nostdlib -static      无 libc，纯系统调用
  -o /tmp/xxx.elf        输出静态 ELF
    │
    ▼
qemu-riscv64 xxx.elf   用户态模拟执行
    │
    ▼
stdout (256 bytes)     32 个 little-endian u64
    │
    ▼
parse → [u64; 32]     解析为寄存器数组
```

临时文件使用 `pid_tid` 命名避免并行测试冲突，执行完毕后
自动清理。

分支指令使用 taken/not-taken 模式，通过 x7(t2) 的值判断
分支是否被执行（1=taken, 0=not-taken）。

### 7.3 machina 侧原理

ALU 指令直接复用全流水线基础设施：

```rust
fn run_tcgrs(
    init: &[(usize, u64)],  // 初始寄存器值
    insns: &[u32],           // RISC-V 机器码序列
) -> RiscvCpu
```

流水线：`RISC-V 机器码 → decode 解码 → trans_* → TCG IR
→ optimize → liveness → regalloc → x86-64 codegen → 执行`

分支指令会退出翻译块（TB），通过 PC 值判断 taken/not-taken：
- `PC = offset` → taken
- `PC = 4` → not-taken

### 7.4 寄存器约定

| 寄存器 | ABI 名 | 用途 |
|--------|--------|------|
| x3 | gp | **保留**：QEMU 侧保存区基址 |
| x5 | t0 | 源操作数 1（rs1） |
| x6 | t1 | 源操作数 2（rs2） |
| x7 | t2 | 目标寄存器（rd） |

x3 不能作为测试寄存器，因为 QEMU 侧的 `la gp, save_area`
会覆盖其值。

### 7.5 边界值策略

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

### 7.6 当前覆盖

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

### 7.7 新增 Difftest 指南

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

`AluTest` 字段：`name`（测试名）、`asm`（QEMU 汇编）、
`insn`（机器码）、`init`（初始寄存器）、`check_reg`（比较目标）。

### 7.8 运行与调试

```bash
# 运行全部 difftest
cargo test -p tcg-tests difftest

# 运行单个 difftest
cargo test -p tcg-tests difftest_add

# 并行运行
cargo test -p tcg-tests difftest -- --test-threads=4

# 查看详细输出
cargo test -p tcg-tests difftest -- --nocapture
```

**失败输出示例**：

```
DIFFTEST FAIL [add]: x7 machina=0x64 qemu=0x65
```

含义：`add` 指令的 x7 寄存器，machina 计算结果为 `0x64`，
QEMU 参考结果为 `0x65`，存在差异。

### 7.9 限制与未来工作

1. **x3(gp) 不可测试**：QEMU 侧保留用于保存区基址
2. **PC 相关指令**：auipc/jal/jalr 需计算相对偏移后比较
3. **Load/Store**：待 QemuLd/QemuSt 完善后扩展
4. **随机化测试**：可引入随机寄存器值生成器提高覆盖率
5. **多指令序列**：可扩展为多指令 difftest

---

## 8. 客户程序测试

**目录**：`tests/guest/`

### 8.1 裸机程序：hello

| 属性 | 值 |
|------|-----|
| 源文件 | `riscv/hello.c` |
| 编译标志 | `-static -nostdlib -march=rv64im -mabi=lp64 -O2` |
| 入口点 | `_start`（无 libc） |
| 系统调用 | write(1, msg, 14) + exit(0)，内联汇编 ecall |
| 二进制大小 | ~1.8 KB |
| 预期输出 | `Hello, World!\n` |

### 8.2 标准 C 程序集合（glibc 静态链接）

| 程序 | 源文件 | 用途 | 关键检查点 |
|------|--------|------|-----------|
| `hello_printf` | `riscv/hello_printf.c` | 最小 `printf` 通路 | 基础 libc 输出 |
| `hello_float` | `riscv/hello_float.c` | 浮点格式化输出 | FPU helper 与 ABI |
| `argv_echo` | `riscv/argv_echo.c` | 参数传递回显 | guest argv 栈布局 |
| `dhrystone` | `riscv/dhrystone/dhry_1.c` + `dhry_2.c` | 端到端性能 smoke | TB 链路与热路径 |

这些程序统一使用 `-static -march=rv64gc -mabi=lp64d -O2`，
用于验证完整 linux-user 执行路径。

### 8.3 构建与运行

**构建**（需要交叉编译器）：

```bash
# 安装交叉编译器
apt install gcc-riscv64-linux-gnu

# 构建全部客户程序
cd tests/guest && make

# 输出到 target/guest/riscv64/
ls ../../target/guest/riscv64/
# hello  hello_printf  hello_float  argv_echo  dhrystone
```

**使用 QEMU 验证**：

```bash
apt install qemu-user
qemu-riscv64 target/guest/riscv64/hello
qemu-riscv64 target/guest/riscv64/hello_printf
```

**使用 machina 运行**：

```bash
cargo run --release --bin tcg-riscv64 -- \
    target/guest/riscv64/hello
```

**自动化集成测试**（`tests/src/linux_user/mod.rs`）：

```bash
cargo test -p tcg-tests linux_user::
```

该测试自动调用 Makefile 构建客户程序，然后通过 tcg-riscv64
二进制执行 guest ELF 集合（包含 dhrystone 与 argv 启动参数
场景）。如果交叉编译器未安装，测试会自动跳过。

---

## 9. 新增测试指南

### 新增前端指令测试

在 `tests/src/frontend/mod.rs` 中添加：

```rust
#[test]
fn test_new_insn() {
    let mut cpu = RiscvCpu::new();
    // Set up initial register state.
    cpu.gpr[1] = 100;
    // Encode and run the instruction.
    let insn = rv_i(42, 1, 0b000, 2, 0b0010011); // addi x2, x1, 42
    run_rv(&mut cpu, insn);
    assert_eq!(cpu.gpr[2], 142);
}
```

### 新增 RVC 测试

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

### 新增 Difftest

参见本文档第 7.7 节「新增 Difftest 指南」。

### 新增客户程序

1. 在 `tests/guest/riscv/` 下创建 `.c` 源文件
2. 在 `tests/guest/Makefile` 中将文件名加入对应列表：
   - `BARE_SRCS`：裸机程序（`-nostdlib`）
   - `LIBC_SRCS`：标准 C 程序（链接 glibc）
3. 运行 `make` 构建
4. 用 `qemu-riscv64` 验证输出
