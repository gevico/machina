# Machina 代码风格规范

## 1. 行宽与缩进

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

## 2. 格式化工具

- 提交前必须运行 `cargo fmt`
- 提交前必须通过 `cargo clippy -- -D warnings`
- 使用 `(-128..=127).contains(&x)` 替代 `x >= -128 && x <= 127`
- 运算符优先级不明确时必须加括号：
  `(OPC + (x << 3)) | flag` 而非 `OPC + (x << 3) | flag`

## 3. 命名规范

### 3.1 通用规则

| 类型 | 风格 | 示例 |
|------|------|------|
| 类型/Trait | UpperCamelCase | `ArithOp`, `CodeBuffer` |
| 函数/方法 | snake_case | `emit_arith_rr`, `low3` |
| 局部变量 | snake_case | `rex`, `offset` |
| 常量 | SCREAMING_SNAKE_CASE | `P_REXW`, `STACK_ADDEND` |
| 枚举变体 | UpperCamelCase | `ArithOp::Add`, `Reg::Rax` |

### 3.2 QEMU 风格常量

操作码常量使用 QEMU 原始命名风格以便交叉参考，
通过 `#![allow(non_upper_case_globals)]` 抑制警告：

```rust
pub const OPC_ARITH_EvIb: u32 = 0x83;
pub const OPC_MOVL_GvEv: u32 = 0x8B;
pub const OPC_JCC_long: u32 = 0x80 | P_EXT;
```

### 3.3 函数命名模式

指令发射器遵循 `emit_<指令>_<操作数模式>` 模式：

```
emit_arith_rr   — 算术 reg, reg
emit_arith_ri   — 算术 reg, imm
emit_arith_mr   — 算术 [mem], reg
emit_arith_rm   — 算术 reg, [mem]
emit_mov_rr     — MOV reg, reg
emit_mov_ri     — MOV reg, imm
emit_load       — MOV reg, [mem]
emit_store      — MOV [mem], reg
emit_shift_ri   — 移位 reg, imm
emit_shift_cl   — 移位 reg, CL
```

## 4. 注释

- 注释使用**英文**编写
- 仅在逻辑不自明处添加注释，不注释显而易见的代码
- 公开 API 使用 `///` 文档注释，简明扼要
- 内部实现使用 `//` 行注释
- 代码注释同样遵守 80 列行宽（`.md` 文档文件不受此限制）

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

## 5. 类型与枚举

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

## 6. 函数设计

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

## 7. unsafe 使用

- `unsafe` 仅限以下场景：
  - JIT 代码缓冲区分配（mmap/mprotect）
  - 调用生成的宿主代码（函数指针转换）
  - 客户内存模拟的原始指针访问
  - 后端内联汇编
  - FFI 接口
- 每个 `unsafe` 块必须有注释说明安全性保证
- 所有其他代码必须是安全的 Rust

## 8. 测试

- 测试位于独立的 `tcg-tests` crate
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

## 9. 模块组织

- 每个 crate 的 `lib.rs` 仅做模块声明和 re-export
- 公开类型通过 `pub use` 在 crate 根导出
- 相关功能放在同一文件，文件内按逻辑分节
- 使用 `// -- Section name --` 分隔文件内的逻辑区域

## 10. MTTCG 与性能代码约束

- 并发路径优先使用“共享状态 + 每线程私有状态”拆分，避免热路径共享锁。
- 新增并发字段时，必须在注释中写清：
  - 谁持有写权限（例如 `translate_lock`）
  - 读路径是否 lock-free
  - 可见性保证（Acquire/Release/Relaxed 的选择理由）
- 性能优化提交必须附带可复现基准命令，至少包含：
  - `tcg-riscv64` 对 `dhrystone`
  - `qemu-riscv64` 同程序对照
- 涉及 TB 链路逻辑时必须补充：
  - 并发正确性测试（`tests/src/exec/mttcg.rs`）
  - 回归测试（至少一个 linux-user guest 测题）
- 调试辅助输出统一走已有统计入口（`TCG_STATS=1`），避免在热路径直接打印日志。
