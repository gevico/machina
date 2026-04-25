# Machina 测试体系

## 1. 概述

Machina 采用分层测试策略，从底层数据结构到完整的全系统模拟器，
逐层验证正确性。测试统一集中在独立的 `tests/` crate 中，保持
源码文件干净，同时验证公共 API 的完整性。

**测试金字塔**：

```
+---------------------------------+
| System / Full VM |
| machine(48) + softmmu(31) |
| + riscv_mmu(12) + riscv_pmp(6)|
| + softmmu_exec(9) |
+---------------------------------+
| hw_* Device Tests |
| aclint(12) + plic(10) + uart(11)|
| + virtio(10) + loader(3) + fdt(3)|
| + ref_machine(25) + clock(6) |
| (118 tests) |
+----+---------------------------------+----+
| Integration / Pipeline | |
| IR → liveness → regalloc → codegen | |
| → execute (94 tests) | |
+---------------------------------------+ |
| Frontend / Difftest / Exec | |
| RV32I/RV64I/RVC/RV32F/Zb*/Zicbom | |
| + differential vs QEMU (35 tests) | |
| + TB cache, exec loop (31 tests) | |
| + softfloat (48 tests) | |
| (276 tests) | |
+--+---+---+---+---+---+---+---+---+---+ |
| Unit Tests |
| core(242) + backend(248) + decode(93) |
| + gdbstub(55) + monitor(19) + trace(4) |
| + tools(6) + disas(42) |
| + riscv_*(34) + accel_timer(4) |
| + memory_region(4) + cli_netdev(7) |
| + system_cpu_manager(5) |
| (763 tests) |
+--+----+----+----+----+----+----+----+----+
```

**总计：1356 个测试**（通过 `cargo test --list` 于 2026-04-25 统计，实际数量可能因编译特性而略有差异）。

---

## 2. 快速参考

### Rust 测试命令

```bash
# 全量测试
cargo test

# 按 crate 运行
cargo test -p machina-core        # 核心 IR 数据结构
cargo test -p machina-accel       # 后端指令编码 + 执行循环
cargo test -p machina-tests       # 主测试 crate（含全部分层测试）

# 按模块过滤
cargo test -p machina-tests core::        # 仅 core 模块
cargo test -p machina-tests backend::     # 仅 backend 模块
cargo test -p machina-tests decode::      # 仅 decode 模块
cargo test -p machina-tests frontend::    # 仅前端指令测试
cargo test -p machina-tests integration:: # 仅集成测试
cargo test -p machina-tests difftest      # 仅差分测试
cargo test -p machina-tests machine::     # 仅机器级测试
cargo test -p machina-tests gdbstub::      # GDB 桩测试
cargo test -p machina-tests monitor::      # 监控接口测试
cargo test -p machina-tests virtio::       # virtio 设备测试
cargo test -p machina-tests softfloat::    # 软浮点测试
cargo test -p machina-tests softmmu::      # 软 MMU 测试
cargo test -p machina-tests tools::        # 辅助工具测试
cargo test -p machina-tests riscv_::       # RISC-V 特权相关测试
cargo test -p machina-tests hw_::          # 硬件设备测试

# 运行单个测试
cargo test -- test_addi
cargo test -- test_c_li

# 查看详细输出
cargo test -- --nocapture

# 并行控制
cargo test -- --test-threads=1    # 串行（调试用）
cargo test -- --test-threads=4    # 4 线程
```

### 代码质量检查

```bash
cargo clippy -- -D warnings       # Lint 零警告
cargo fmt --check                  # 格式检查
cargo fmt                          # 自动格式化
```

### 多 vCPU 并发与性能回归

```bash
# 多 vCPU 并发回归
cargo test -p machina-tests exec::mttcg -- --nocapture

# 打印执行统计（TB 命中率、链路 patch、hint 命中）
TCG_STATS=1 target/release/machina <machine-config>

# 简单性能对照（本机基线）
TIMEFORMAT=%R; time target/release/machina <machine-config>
```

### 按改动范围运行测试

以下速查表帮助开发者根据改动的模块快速选择所需测试命令，避免运行全量测试浪费时间。

| 改动范围 | 建议运行命令 | 说明 |
|----------|-------------|------|
| Loader（镜像加载） | `cargo test -p machina-tests hw_loader::` | ELF/Binary 加载 |
| Monitor（监控接口） | `cargo test -p machina-tests monitor::` | QMP/HMP 交互测试 |
| Virtio 设备 | `cargo test -p machina-tests virtio::` | virtio-blk/net 模拟 |
| GDB 远程调试 | `cargo test -p machina-tests gdbstub::` | 数据包解析、寄存器访问 |
| 软浮点 | `cargo test -p machina-tests softfloat::` | IEEE 754 运算 |
| 独立工具 | `cargo test -p machina-tests tools::` | irbackend, irdump, sifive_test |
| 硬件设备模型 | `cargo test -p machina-tests hw_` | UART/PLIC/CLINT/Clock/FDT/IRQ |
| RISC-V 特权架构 | `cargo test -p machina-tests riscv_` | CSR、异常、PMP、MMU |
| 前端指令执行 | `cargo test -p machina-tests frontend::` | 全部 RISC-V 扩展指令 |
| 集成流水线 | `cargo test -p machina-tests integration::` | IR – 代码生成 – 执行 |
| 执行循环 | `cargo test -p machina-tests exec::` | TB 缓存、分支、循环 |
| 后端编码 | `cargo test -p machina-tests backend::` | x86-64 指令编码 |
| 解码器生成 | `cargo test -p machina-tests decode::` | .decode 解析与生成 |
| Sysbus/MMIO 分发 | `cargo test -p machina-tests hw_sysbus::` | 地址空间、设备挂载 |
| 参考机器 | `cargo test -p machina-tests hw_ref_machine::` | 整机启动、FDT、IRQ 接线 |

**耗时较长或依赖外部工具的测试**：

- **差分测试**（`cargo test -p machina-tests difftest`）：需要安装 `gcc-riscv64-linux-gnu` 和 `qemu-riscv64`，运行时间较长。
- **多 vCPU 并发测试**（`cargo test -p machina-tests exec::multi_vcpu`）：建议在调试时使用 `--test-threads=1`。
- **机器引导测试**（`hw_ref_machine::`）：可能需要先构建 `tests/mtest/` 下的固件（交叉编译器）。


---

## 3. 测试架构

### 目录结构

```
tests/
+-- Cargo.toml                    # 依赖：core, accel, frontend,
|                                 #        decode
+-- src/
|   +-- lib.rs                    # 模块声明
|   +-- core/                     # 核心 IR 单元测试 (192)
|   |   +-- context.rs
|   |   +-- label.rs
|   |   +-- op.rs
|   |   +-- opcode.rs
|   |   +-- regset.rs
|   |   +-- tb.rs
|   |   +-- temp.rs
|   |   +-- types.rs
|   +-- backend/                  # 后端单元测试 (256)
|   |   +-- code_buffer.rs
|   |   +-- x86_64.rs
|   |   +-- mod.rs
|   +-- decode/                   # 解码器生成器测试 (93)
|   |   +-- mod.rs
|   +-- frontend/                 # 前端指令测试 (91 + 35)
|   |   +-- mod.rs                #   RV32I/RV64I/RVC 执行
|   |   +-- difftest.rs           #   machina vs QEMU 差分
|   +-- integration/              # 集成测试 (105)
|   |   +-- mod.rs
|   +-- exec/                     # 执行循环测试 (26)
|   |   +-- mod.rs
|   +-- machine/                  # 机器级测试 (48)
|       +-- mod.rs                #   mtest 框架入口
|       +-- device.rs             #   设备模型测试
|       +-- boot.rs               #   引导流程测试
+-- mtest/                        # mtest 测试固件
    +-- Makefile
    +-- src/
        +-- uart_echo.S           # UART 回环测试
        +-- timer_irq.S           # Timer 中断测试
        +-- boot_hello.S          # 最小引导测试
```

### 模块测试分布

| 模块 | 测试数 | 占比 | 说明 |
|------|--------|------|------|
| backend | 248 | 18.3% | x86-64 指令编码与扩展 |
| core | 242 | 17.8% | IR 类型、Opcode、Temp、Label、Context、regset、mdev、mom、property、serialize |
| frontend | 162 | 11.9% | RISC-V 指令执行（RV32I/RV64I/RVC/RV32F/Zba/Zbb/Zbc/Zbs/Zicbom） |
| hw_* | 118 | 8.7% | 硬件设备：aclint、plic、uart、clock、loader、fdt、ref_machine、sysbus、irq、chardev、virtio |
| integration | 94 | 6.9% | IR → codegen → 执行全流水线 |
| decode | 93 | 6.9% | .decode 解析、代码生成、字段提取 |
| gdbstub | 55 | 4.1% | GDB 远程调试协议 |
| softfloat | 48 | 3.5% | IEEE 754 浮点运算 |
| disas_bitmanip | 42 | 3.1% | 位操作反汇编测试 |
| difftest | 35 | 2.6% | machina vs QEMU 差分对比 |
| riscv_* | 34 | 2.5% | RISC-V CSR、异常、PMP、MMU |
| exec | 31 | 2.3% | TB 缓存、执行循环、多 vCPU 并发 |
| softmmu | 31 | 2.3% | 软 MMU/TLB 测试 |
| monitor | 19 | 1.4% | QMP/HMP 监控命令 |
| virtio | 10 | 0.7% | virtio 设备模型 |
| cli_netdev | 7 | 0.5% | 网络设备 CLI 解析 |
| tools | 6 | 0.4% | 独立工具（irbackend、irdump、sifive_test） |
| system_cpu_manager | 5 | 0.4% | CPU 管理器 |
| trace | 4 | 0.3% | 日志跟踪 |
| memory_region | 4 | 0.3% | 内存区域 |
| accel_timer | 4 | 0.3% | 虚拟定时器 |

**统计说明**：基于 `cargo test -p machina-tests -- --list` 输出归类，日期 2026-04-25，具体数量可能随功能开关变化。---

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
cargo test -p machina-tests core::
```

### 4.2 Backend 模块（256 tests）

验证 x86-64 指令编码器的正确性。

| 文件 | 测试内容 |
|------|----------|
| `code_buffer.rs` | 代码缓冲区分配、写入、mprotect 切换 |
| `x86_64.rs` | 全部 x86-64 指令编码（MOV/ADD/SUB/AND/OR/XOR/SHL/SHR/SAR/MUL/DIV/LEA/Jcc/SETcc/CMOVcc/BSF/BSR/LZCNT/TZCNT/POPCNT 等） |

```bash
cargo test -p machina-tests backend::
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
cargo test -p machina-tests decode::
```

---

## 5. 集成测试（105 tests）

**源文件**：`tests/src/integration/mod.rs`

验证完整的 IR --> liveness --> register allocation --> codegen --> 执行
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
cargo test -p machina-tests integration::
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
指令编码 --> 写入 guest 内存 --> translator_loop 解码
--> IR 生成 --> liveness --> regalloc --> x86-64 codegen
--> 执行生成代码 --> 读取 CPU 状态 --> 断言验证
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

### 7.2 QEMU 侧原理

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

### 7.3 machina 侧原理

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

含义：`add` 指令的 x7 寄存器，machina 计算结果为 `0x64`，
QEMU 参考结果为 `0x65`，存在差异。

### 7.9 限制与未来工作

1. **x3(gp) 不可测试**：QEMU 侧保留用于保存区基址
2. **PC 相关指令**：auipc/jal/jalr 需计算相对偏移后比较
3. **Load/Store**：待 QemuLd/QemuSt 完善后扩展
4. **随机化测试**：可引入随机寄存器值生成器提高覆盖率
5. **多指令序列**：可扩展为多指令 difftest

---

## 8. 机器级测试（mtest 框架）

**目录**：`tests/mtest/`

mtest 是 machina 的全系统级测试框架，在完整的虚拟机环境中
运行裸机固件，验证设备模型、中断控制器、内存映射 I/O 以及
引导流程的端到端正确性。

### 8.1 架构概览

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

### 8.2 测试类别

| 类别 | 测试数 | 说明 |
|------|--------|------|
| 设备模型 | 20 | UART 寄存器读写、CLINT MMIO、PLIC 分发 |
| MMIO 分发 | 10 | AddressSpace 路由、重叠区间、未映射访问 |
| 引导流程 | 8 | 最小固件加载、PC 复位向量、M-mode 初始化 |
| 中断 | 6 | Timer 中断触发与响应、外部中断路由 |
| 多核 | 4 | SMP 启动、IPI 发送与接收 |

### 8.3 设备测试

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

### 8.4 引导测试

引导测试在完整 VM 中加载裸机固件，验证从复位到输出的
全路径：

```rust
#[test]
fn test_boot_hello() {
    let vm = MachineBuilder::new()
        .memory_size(64 * 1024 * 1024)
        .load_firmware("tests/mtest/bin/boot_hello.bin")
        .build();
    let output = vm.run_until_halt(Duration::from_secs(5));
    assert_eq!(output.uart_output(), "Hello from M-mode!\n");
    assert_eq!(output.exit_code(), 0);
}
```

### 8.5 运行命令

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
    let insn = rv_i(42, 1, 0b000, 2, 0b0010011);
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

### 新增机器级测试

1. 在 `tests/mtest/src/` 下创建裸机汇编或 C 固件源文件
2. 在 `tests/mtest/Makefile` 中添加构建规则
3. 在 `tests/src/machine/` 下添加对应的 Rust 测试函数
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
        .load_firmware("tests/mtest/bin/new_test.bin")
        .build();
    let output = vm.run_until_halt(Duration::from_secs(5));
    assert!(output.uart_output().contains("PASS"));
}
```
