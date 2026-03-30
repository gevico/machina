# Machina Coding Style

## 1. Line Width and Indentation

- **Line width limit of 80 columns**, applying to all code and code comments
- `.md` documentation files are exempt from the 80-column limit
- Indentation uses **4 spaces**, tabs are forbidden
- Continuation lines align to the start of parameters on the previous line, or indent by 4 spaces

```rust
// Good: within 80 columns, continuation lines aligned
fn emit_modrm_offset(
    buf: &mut CodeBuffer,
    opc: u32,
    r: Reg,
    base: Reg,
    offset: i32,
) {
    // ...
}

// Good: short function signatures can be on a single line
fn emit_ret(buf: &mut CodeBuffer) {
    buf.emit_u8(0xC3);
}
```

## 2. Formatting Tools

- Must run `cargo fmt` before committing
- Must pass `cargo clippy -- -D warnings` before committing
- Use `(-128..=127).contains(&x)` instead of `x >= -128 && x <= 127`
- Parentheses are required when operator precedence is ambiguous: `(OPC + (x << 3)) | flag` not `OPC + (x << 3) | flag`

## 3. Naming Conventions

### 3.1 General Rules

| Type | Style | Example |
|------|-------|---------|
| Types/Traits | UpperCamelCase | `ArithOp`, `CodeBuffer` |
| Functions/Methods | snake_case | `emit_arith_rr`, `low3` |
| Local variables | snake_case | `rex`, `offset` |
| Constants | SCREAMING_SNAKE_CASE | `P_REXW`, `STACK_ADDEND` |
| Enum variants | UpperCamelCase | `ArithOp::Add`, `Reg::Rax` |

### 3.2 QEMU-Style Constants

Opcode constants use QEMU's original naming style for ease of cross-referencing, with warnings suppressed via `#![allow(non_upper_case_globals)]`:

```rust
pub const OPC_ARITH_EvIb: u32 = 0x83;
pub const OPC_MOVL_GvEv: u32 = 0x8B;
pub const OPC_JCC_long: u32 = 0x80 | P_EXT;
```

### 3.3 Function Naming Patterns

Instruction emitters follow the `emit_<instruction>_<operand_pattern>` pattern:

```
emit_arith_rr   -- arithmetic reg, reg
emit_arith_ri   -- arithmetic reg, imm
emit_arith_mr   -- arithmetic [mem], reg
emit_arith_rm   -- arithmetic reg, [mem]
emit_mov_rr     -- MOV reg, reg
emit_mov_ri     -- MOV reg, imm
emit_load       -- MOV reg, [mem]
emit_store      -- MOV [mem], reg
emit_shift_ri   -- shift reg, imm
emit_shift_cl   -- shift reg, CL
```

## 4. Comments

- Comments are written in **English**
- Add comments only where the logic is not self-evident; do not comment obvious code
- Public APIs use `///` doc comments, kept concise
- Internal implementation uses `//` line comments
- Code comments also follow the 80-column line width (`.md` documentation files are exempt)

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

## 5. Types and Enums

- Enums use `#[repr(u8)]` or `#[repr(u16)]` to ensure memory layout
- Enum values are explicitly assigned; do not rely on auto-increment
- Derive `Debug, Clone, Copy, PartialEq, Eq`

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

## 6. Function Design

- Function parameter order: `buf` first, configuration parameters in the middle, operands last
- `rexw: bool` parameter controls 32/64-bit operation
- Immediate encoding automatically selects the short form (imm8 vs imm32)
- Function bodies should be kept short; complex logic should be split into sub-functions

```rust
// Good: buf first, rexw in the middle, operands last
pub fn emit_load(
    buf: &mut CodeBuffer,
    rexw: bool,
    dst: Reg,
    base: Reg,
    offset: i32,
) { ... }
```

## 7. Unsafe Usage

- `unsafe` is restricted to the following scenarios:
  - JIT code buffer allocation (mmap/mprotect)
  - Calling generated host code (function pointer casts)
  - Raw pointer access for guest memory emulation
  - Backend inline assembly
  - FFI interfaces
- Every `unsafe` block must have a comment explaining the safety guarantee
- All other code must be safe Rust

## 8. Testing

- Tests reside in the standalone `machina-tests` crate
- Each instruction emitter has at least one test verifying byte encoding
- Tests cover both base registers (Rax-Rdi) and extended registers (R8-R15)
- The `emit_bytes` helper function simplifies test writing
- Test function names use snake_case and describe the behavior being tested

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

## 9. Documentation Diagram Standards

All diagrams in documentation must be drawn using plain ASCII characters. Unicode box-drawing characters and other non-ASCII symbols are forbidden.

**Allowed characters**:

| Character | Usage |
|-----------|-------|
| `+` | Corner connection points |
| `-` | Horizontal lines |
| `|` | Vertical lines |
| `-->` | Horizontal arrow (right) |
| `<--` | Horizontal arrow (left) |
| `v` | Downward arrow |
| `^` | Upward arrow |

**Box alignment rules**:

- All rectangle corners must use the `+` character
- Horizontal and vertical edges must be strictly aligned, with no jagged edges
- Arrow direction uses `-->` or `v`, not Unicode arrows

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

**Forbidden examples**:

- `─`, `│`, `┌`, `┐`, `└`, `┘` and other Unicode box-drawing characters
- `→`, `←`, `↑`, `↓` and other Unicode arrows
- `├`, `┤`, `┬`, `┴`, `┼` and other Unicode connectors

## 10. Module Organization

- Each crate's `lib.rs` only performs module declarations and re-exports
- Public types are exported at the crate root via `pub use`
- Related functionality is placed in the same file, with logical sections within the file
- Use `// -- Section name --` to separate logical regions within a file

## 11. Multi-Threaded vCPU and Performance Code Constraints

- Concurrent paths should prefer a "shared state + per-thread private state" split to avoid shared locks on hot paths.
- When adding concurrent fields, the comment must clearly state:
  - Who holds write access (e.g., `translate_lock`)
  - Whether the read path is lock-free
  - Visibility guarantees (rationale for choosing Acquire/Release/Relaxed)
- Performance optimization commits must include reproducible benchmark commands, at minimum:
  - `machina-riscv64` against `dhrystone`
  - `qemu-riscv64` with the same program as a baseline
- Changes involving TB chaining logic must include:
  - Concurrency correctness tests (`tests/src/exec/mttcg.rs`)
  - Regression tests (at least one guest test case)
- Debug output should go through the existing statistics entry point (`TCG_STATS=1`); avoid printing logs directly on hot paths.
