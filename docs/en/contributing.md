# Contributing to Machina

> Target audience: developers contributing code to machina.

## Table of Contents

- [Part 1: Code Style](#part-1-code-style)
- [Part 2: Coding Guidelines](#part-2-coding-guidelines)
- [Part 3: Rust Guidelines](#part-3-rust-guidelines)
- [Part 4: Git Guidelines](#part-4-git-guidelines)
- [Part 5: Testing Guide](#part-5-testing-guide)

---

## Part 1: Code Style

### 1. Line Width and Indentation

- **Line width limit of 80 columns**, applying to all code
  and code comments
- `.md` documentation files are exempt from the 80-column
  limit
- Indentation uses **4 spaces**, tabs are forbidden
- Continuation lines align to the start of parameters on the
  previous line, or indent by 4 spaces

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

### 2. Formatting Tools

- Must run `cargo fmt` before committing
- Must pass `cargo clippy -- -D warnings` before committing
- Use `(-128..=127).contains(&x)` instead of
  `x >= -128 && x <= 127`
- Parentheses are required when operator precedence is
  ambiguous: `(OPC + (x << 3)) | flag` not
  `OPC + (x << 3) | flag`

### 3. Naming Conventions

#### 3.1 General Rules

| Type | Style | Example |
|------|-------|---------|
| Types/Traits | UpperCamelCase | `ArithOp`, `CodeBuffer` |
| Functions/Methods | snake_case | `emit_arith_rr`, `low3` |
| Local variables | snake_case | `rex`, `offset` |
| Constants | SCREAMING_SNAKE_CASE | `P_REXW`, `STACK_ADDEND` |
| Enum variants | UpperCamelCase | `ArithOp::Add`, `Reg::Rax` |

#### 3.2 QEMU-Style Constants

Opcode constants use QEMU's original naming style for ease of
cross-referencing, with warnings suppressed via
`#![allow(non_upper_case_globals)]`:

```rust
pub const OPC_ARITH_EvIb: u32 = 0x83;
pub const OPC_MOVL_GvEv: u32 = 0x8B;
pub const OPC_JCC_long: u32 = 0x80 | P_EXT;
```

#### 3.3 Function Naming Patterns

Instruction emitters follow the
`emit_<instruction>_<operand_pattern>` pattern:

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

### 4. Comments

- Comments are written in **English**
- Add comments only where the logic is not self-evident; do
  not comment obvious code
- Public APIs use `///` doc comments, kept concise
- Internal implementation uses `//` line comments
- Code comments also follow the 80-column line width (`.md`
  documentation files are exempt)

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

### 5. Types and Enums

- Enums use `#[repr(u8)]` or `#[repr(u16)]` to ensure memory
  layout
- Enum values are explicitly assigned; do not rely on
  auto-increment
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

### 6. Function Design

- Function parameter order: `buf` first, configuration
  parameters in the middle, operands last
- `rexw: bool` parameter controls 32/64-bit operation
- Immediate encoding automatically selects the short form
  (imm8 vs imm32)
- Function bodies should be kept short; complex logic should
  be split into sub-functions

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

### 7. Unsafe Usage

- `unsafe` is restricted to the following scenarios:
  - JIT code buffer allocation (mmap/mprotect)
  - Calling generated host code (function pointer casts)
  - Raw pointer access for guest memory emulation
  - Backend inline assembly
  - FFI interfaces
- Every `unsafe` block must have a comment explaining the
  safety guarantee
- All other code must be safe Rust

### 8. Testing

- Tests reside in the standalone `machina-tests` crate
- Each instruction emitter has at least one test verifying
  byte encoding
- Tests cover both base registers (Rax-Rdi) and extended
  registers (R8-R15)
- The `emit_bytes` helper function simplifies test writing
- Test function names use snake_case and describe the behavior
  being tested

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

### 9. Documentation Diagram Standards

All diagrams in documentation must be drawn using plain ASCII
characters. Unicode box-drawing characters and other non-ASCII
symbols are forbidden.

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
- Horizontal and vertical edges must be strictly aligned,
  with no jagged edges
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

- `─`, `│`, `┌`, `┐`, `└`, `┘` and other Unicode
  box-drawing characters
- `→`, `←`, `↑`, `↓` and other Unicode arrows
- `├`, `┤`, `┬`, `┴`, `┼` and other Unicode connectors

### 10. Module Organization

- Each crate's `lib.rs` only performs module declarations and
  re-exports
- Public types are exported at the crate root via `pub use`
- Related functionality is placed in the same file, with
  logical sections within the file
- Use `// -- Section name --` to separate logical regions
  within a file

### 11. Multi-Threaded vCPU and Performance Code Constraints

- Concurrent paths should prefer a "shared state + per-thread
  private state" split to avoid shared locks on hot paths.
- When adding concurrent fields, the comment must clearly
  state:
  - Who holds write access (e.g., `translate_lock`)
  - Whether the read path is lock-free
  - Visibility guarantees (rationale for choosing
    Acquire/Release/Relaxed)
- Performance optimization commits must include reproducible
  benchmark commands, at minimum:
  - `machina-riscv64` against `dhrystone`
  - `qemu-riscv64` with the same program as a baseline
- Changes involving TB chaining logic must include:
  - Concurrency correctness tests
    (`tests/src/exec/mttcg.rs`)
  - Regression tests (at least one guest test case)
- Debug output should go through the existing statistics
  entry point (`TCG_STATS=1`); avoid printing logs directly
  on hot paths.

---

## Part 2: Coding Guidelines

General coding guidelines for the Machina project. For
Rust-specific rules, see
[Rust Guidelines](#part-3-rust-guidelines). For formatting and
naming conventions, see [Code Style](#part-1-code-style).

### Naming

#### Be descriptive

No single-letter names or ambiguous abbreviations. Names
should reveal intent.

```rust
// Bad
let v = t.read();
let p = addr >> 12;

// Good
let value = temp.read();
let page_number = addr >> PAGE_SHIFT;
```

#### Be accurate

Names must match what the code actually does. If the name
says "count", the value should be a count -- not an index,
offset, or mask.

#### Encode units in names

When a value carries a physical unit or scale, embed it in
the name.

```rust
let timeout_ms = 5000;
let frame_size_in_pages = 4;
let clock_freq_hz = 12_000_000;
```

#### Boolean naming

Use assertion-style names for booleans: `is_*`, `has_*`,
`can_*`, `should_*`.

```rust
let is_kernel_mode = mode == Prv::M;
let has_side_effects =
    op.flags().contains(OpFlags::SIDE_EFFECT);
```

### Comments

#### Explain why, not what

Comments that restate code are noise. Explain the reasoning
behind non-obvious decisions.

```rust
// Bad: restates the code
// Check if page is present
if pte.flags().contains(PteFlags::V) { ... }

// Good: explains the constraint
// Sv39 spec: fetch falls through when V=0 in S-mode
// only traps in M-mode (spec 4.3.1)
if pte.flags().contains(PteFlags::V) { ... }
```

#### Document design decisions

When multiple approaches exist, record why this one was
chosen. Future readers need to understand the trade-off, not
just the outcome.

#### Cite specifications

When implementing hardware behavior, cite the specification
section.

```rust
// RISC-V Privileged Spec 4.3.1 -- PTE attribute for
// global mappings
const PTE_G: u64 = 0x10;
```

### File Organization

#### One concept per file

Split files when they grow long or mix unrelated
responsibilities. A file named `mmu.rs` should not contain
interrupt handling logic.

#### Organize for top-down reading

Place high-level entry points first. Helper functions and
internal details follow. A reader should understand the
public API by reading the first section.

#### Group into logical paragraphs

Within a function, group related statements together.
Separate groups with a blank line. Each group should express
one step in the algorithm.

### API Design

#### Hide implementation details

Default to the narrowest visibility. Expose only what callers
need.

```rust
// Prefer
pub(crate) fn translate_one(ctx: &mut Context) { ... }

// Avoid
pub fn translate_one(ctx: &mut Context) { ... }
```

#### Validate at boundaries, trust internally

Validate inputs at public API boundaries (e.g., syscall
entry, device MMIO write). Inside the crate, trust
already-validated values.

#### Use types to enforce invariants

If a value has constraints, encode them in the type system
rather than checking at every use site.

```rust
// Prefer: invalid states are unrepresentable
pub struct PhysicalPage(u64);

impl PhysicalPage {
    pub fn new(frame: u64) -> Option<Self> {
        (frame < MAX_PHYS_PAGE)
            .then_some(PhysicalPage(frame))
    }
}

// Avoid: raw u64 could be any value
fn map_page(frame: u64) { ... }
```

### Error Messages

Format error messages consistently. Include the operation
that failed, the value or identifier involved, and (where
applicable) the expected range.

```
"invalid PTE at {vpn}: reserved bits set"
"out of TB cache: capacity {cap}, requested {size}"
```

---

## Part 3: Rust Guidelines

Rust-specific guidelines for the Machina project. For general
coding rules, see
[Coding Guidelines](#part-2-coding-guidelines). For
formatting conventions, see
[Code Style](#part-1-code-style).

### Unsafe Rust

#### Justify every use of unsafe

Every `unsafe` block requires a `// SAFETY:` comment
explaining why the operation is sound. Every `unsafe fn` or
`unsafe trait` requires a `# Safety` doc section describing
the conditions callers must uphold.

```rust
// SAFETY: buf points to a valid, RWX-mapped region of
// `len` bytes. The mmap call above guaranteed alignment
// and permissions.
unsafe {
    core::ptr::copy_nonoverlapping(src.as_ptr(), buf, len)
}
```

#### Unsafe is confined to specific modules

`unsafe` is only permitted for JIT code buffer management,
function pointer casts for generated code, raw pointer access
in the TLB fast path, inline assembly in the backend emitter,
and FFI. All other code must be safe Rust.

### Functions

#### Keep functions small and focused

A function should do one thing. If it needs a comment to
separate sections, consider splitting it.

#### Minimize nesting

Target at most 3 levels of nesting. Use early returns,
`let...else`, and `?` to flatten control flow.

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

#### Avoid boolean parameters

Use an enum or split into two functions.

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

### Types and Traits

#### Prefer enums over trait objects for closed sets

When the set of variants is known at compile time, use an
enum. Trait objects (`dyn Trait`) are appropriate only for
open-ended extensibility.

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

#### Encapsulate fields behind getters

Expose fields through methods rather than making them `pub`.
This preserves the ability to add validation or logging
later.

### Modules and Crates

#### Default to narrow visibility

Use `pub(super)` or `pub(crate)` by default. Only use `pub`
when external crates genuinely need access.

#### Qualify function imports via parent module

Import the parent module, then call the function through it.
This makes the origin explicit.

```rust
// Prefer
use core::mem;
mem::replace(&mut slot, new_value)

// Avoid
use core::mem::replace;
replace(&mut slot, new_value)
```

#### Use workspace dependencies

All shared dependency versions must be declared in the
workspace root `Cargo.toml` under
`[workspace.dependencies]`. Individual crates reference them
with `{ workspace = true }`.

### Error Handling

#### Propagate errors with `?`

Do not `.unwrap()` or `.expect()` where failure is possible.
Use `?` to propagate errors to the caller.

#### Define domain error types

Use dedicated error enums rather than generic `String` or
`Box<dyn Error>`.

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

### Concurrency

#### Document lock ordering

When multiple locks exist, document the acquisition order and
follow it consistently to prevent deadlocks.

#### No I/O under spinlock

Never perform I/O or blocking operations while holding a
spinlock. This includes memory allocation and print
statements.

#### Avoid casual atomics

`Ordering` is subtle. Use `SeqCst` by default. Only relax to
`Acquire`/`Release`/`Relaxed` when there is a documented
performance reason and the correctness argument is written in
a comment.

### Performance

#### No O(n) on hot paths

The translation fast path (TB lookup, code execution, TLB
walk) must avoid O(n) operations. Use hash tables,
direct-mapped caches, or indexed arrays.

#### Minimize unnecessary copies

Pass large structures by reference. Use `&[u8]` instead of
`Vec<u8>` when ownership is not needed. Avoid cloning `Arc`
on every iteration.

#### No premature optimization

Optimization commits must include a benchmark showing the
improvement.

### Macros and Attributes

#### Prefer functions over macros

Use a macro only when a function cannot do the job (e.g.,
repeating declarations, generating match arms).

#### Suppress lints at narrowest scope

Apply `#[allow(...)]` or `#[expect(...)]` to the specific
item, not the entire module.

#### Sort derive traits alphabetically

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
```

#### Workspace lints

Every workspace member must include:

```toml
[lints]
workspace = true
```

---

## Part 4: Git Guidelines

Git commit and pull request conventions for the Machina
project.

### Commit Messages

#### Format

```
module: subject

Body describing what changed and why.

Signed-off-by: Name <email>
```

#### Subject line

- Format: `module: subject`
- Imperative mood: `add`, `fix`, `remove` -- not `added`,
  `fixed`, `removed`
- Lowercase subject, no trailing period
- Total length at or below 72 characters

#### Common module prefixes

| Module | Scope |
|--------|-------|
| `core` | IR types, opcodes, CPU trait |
| `accel` | IR optimization, regalloc, codegen, exec engine |
| `guest/riscv` | RISC-V frontend (decode, translate) |
| `decode` | .decode parser and codegen |
| `system` | CPU manager, GDB bridge, WFI |
| `memory` | AddressSpace, MMIO, RAM blocks |
| `hw/core` | Device infrastructure (qdev, IRQ, FDT) |
| `hw/intc` | PLIC, ACLINT |
| `hw/char` | UART |
| `hw/riscv` | Reference machine, boot, SBI |
| `hw/virtio` | VirtIO MMIO transport and devices |
| `monitor` | QMP/HMP console |
| `gdbstub` | GDB remote protocol |
| `difftest` | Difftest client |
| `tests` | Test suite |
| `docs` | Documentation |
| `project` | Cross-cutting changes (CI, Makefile, configs) |

#### Common verb prefixes

| Verb | Usage |
|------|-------|
| `Fix` | Correct a bug |
| `Add` | Introduce new functionality |
| `Remove` | Delete code or features |
| `Refactor` | Restructure without changing behavior |
| `Rename` | Change names of files, modules, or symbols |
| `Implement` | Add a new subsystem or feature |
| `Enable` | Turn on a previously disabled capability |
| `Clean up` | Minor tidying without functional change |
| `Bump` | Update a dependency version |

#### Body

- Separated from subject by a blank line
- Describe what changed and why -- not how
- Each line at or below 80 characters

#### Examples

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

### Atomic Commits

#### One commit, one logical change

Each commit must do exactly one thing. Do not mix unrelated
changes in a single commit. If you find yourself writing
"and also" in the commit message, split it.

#### Every commit must compile and pass tests

The tree must be in a working state after every commit. No
broken intermediate states. This ensures `git bisect` always
works.

#### Squash fixup commits before submitting

When reviewing your own branch before opening a PR, squash
any temporary commits into the commit they belong to.
Temporary commits include:

- Fixup commits that correct a typo or bug introduced
  earlier in the branch
- Adjustment commits that tweak a previous change (e.g.,
  renaming, reordering)
- Any commit whose message starts with `fixup!` or `squash!`

Use `git rebase -i` to fold these into the original commit.
The final PR history should read as a clean sequence of
logical changes, not a development journal.

#### Separate refactoring from features

If a feature requires preparatory refactoring, put the
refactoring in its own commit(s) before the feature commit.
This makes each commit easier to review and bisect.

### Signed-off-by

All commits in this repository must include a `Signed-off-by`
line:

```
Signed-off-by: Chao Liu <chao.liu.zevorn@gmail.com>
```

Do not add AI-related sign-off lines (e.g.
`Co-Authored-By: Claude`).

### Pull Requests

#### Keep PRs focused

One topic per PR. A PR that mixes a bug fix, a refactoring,
and a new feature is difficult to review.

#### CI must pass

Ensure all CI checks pass before requesting review:

- `make test` -- all tests pass
- `make clippy` -- zero warnings
- `make fmt-check` -- formatting is clean

#### Reference issues

When a PR addresses an issue, reference it in the
description:

```
Closes #42
```

---

## Part 5: Testing Guide

### Quick Reference

#### Rust Test Commands

```bash
# Run all tests
cargo test

# Run by crate
cargo test -p machina-core        # Core IR data structures
cargo test -p machina-accel       # Backend instruction
                                  # encoding + execution loop
cargo test -p machina-tests       # Main test crate
                                  # (all layered tests)

# Filter by module
cargo test -p machina-tests core::        # Core module only
cargo test -p machina-tests backend::     # Backend module
cargo test -p machina-tests decode::      # Decode module
cargo test -p machina-tests frontend::    # Frontend tests
cargo test -p machina-tests integration:: # Integration tests
cargo test -p machina-tests difftest      # Difftests only
cargo test -p machina-tests machine::     # Machine-level

# Run a single test
cargo test -- test_addi
cargo test -- test_c_li

# Verbose output
cargo test -- --nocapture

# Parallelism control
cargo test -- --test-threads=1    # Sequential (debugging)
cargo test -- --test-threads=4    # 4 threads
```

#### Code Quality Checks

```bash
cargo clippy -- -D warnings       # Zero lint warnings
cargo fmt --check                  # Format check
cargo fmt                          # Auto-format
```

#### Multi-Threaded vCPU and Performance Regression

```bash
# Multi-threaded vCPU concurrency regression
cargo test -p machina-tests exec::mttcg -- --nocapture

# Print execution statistics
# (TB hit rate, chain patches, hint hits)
TCG_STATS=1 target/release/machina <machine-config>

# Simple performance comparison (native baseline)
TIMEFORMAT=%R; time target/release/machina <machine-config>
```

### RVC Compressed Instruction Tests

**Encoder helper functions** (`tests/src/frontend/mod.rs`):

| Format Encoder | RVC Format |
|----------------|------------|
| `rv_ci(f3, imm5, rd, imm4_0, op)` | CI format |
| `rv_cr(f4, rd, rs2, op)` | CR format |
| `rv_css(f3, imm, rs2, op)` | CSS format |
| `rv_ciw(f3, imm, rdp, op)` | CIW format |
| `rv_cl(f3, imm_hi, rs1p, imm_lo, rdp, op)` | CL format |
| `rv_cs(f3, imm_hi, rs1p, imm_lo, rs2p, op)` | CS format |
| `rv_cb(f3, off_hi, rs1p, off_lo, op)` | CB format |
| `rv_cj(f3, target, op)` | CJ format |

**Instruction encoders**: c_li, c_addi, c_lui, c_mv, c_add,
c_sub, c_slli, c_addi4spn, c_addiw, c_j, c_beqz, c_bnez,
c_ebreak.

| Test | Verification |
|------|--------------|
| `test_c_li` | C.LI rd, imm --> rd = sext(imm) |
| `test_c_addi` | C.ADDI rd, nzimm --> rd += sext(nzimm) |
| `test_c_lui` | C.LUI rd, nzimm --> rd = sext(nzimm<<12) |
| `test_c_mv` | C.MV rd, rs2 --> rd = rs2 |
| `test_c_add` | C.ADD rd, rs2 --> rd += rs2 |
| `test_c_sub` | C.SUB rd', rs2' --> rd' -= rs2' |
| `test_c_slli` | C.SLLI rd, shamt --> rd <<= shamt |
| `test_c_addi4spn` | C.ADDI4SPN rd', nzuimm --> rd' = sp + nzuimm |
| `test_c_addiw` | C.ADDIW rd, imm --> rd = sext32(rd + imm) |
| `test_c_j` | C.J offset --> PC jump |
| `test_c_beqz_*` | C.BEQZ taken / not-taken |
| `test_c_bnez_*` | C.BNEZ taken / not-taken |
| `test_c_ebreak` | C.EBREAK --> exit |
| `test_mixed_32_16` | Mixed 32-bit + 16-bit instruction sequence |

```bash
cargo test -p machina-tests frontend::    # All frontend
cargo test -p machina-tests test_c_       # RVC tests only
cargo test -p machina-tests test_mixed    # Mixed tests
```

### Differential Tests

#### Current Coverage

| Category | Instructions | Count |
|----------|-------------|-------|
| R-type ALU | add, sub, sll, srl, sra, slt, sltu, xor, or, and | 10 |
| I-type ALU | addi, slti, sltiu, xori, ori, andi, slli, srli, srai | 9 |
| LUI | lui | 1 |
| W-suffix R | addw, subw, sllw, srlw, sraw | 5 |
| W-suffix I | addiw, slliw, srliw, sraiw | 4 |
| Branch | beq, bne, blt, bge, bltu, bgeu | 6 |

**Not yet covered** (to be expanded):
- Load/Store (lb/lh/lw/ld/sb/sh/sw/sd)
- M extension (mul/div/rem family)
- auipc, jal, jalr (PC-relative, requires special handling)

#### Adding New Difftests

**Adding an R-type instruction** (using `mulw` as an
example):

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

**Adding an I-type instruction** (using `sltiu` as an
example):

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

**Adding a branch instruction**:

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

**Custom patterns** (e.g., LUI has no source register):

```rust
difftest_alu(&AluTest {
    name: "lui",
    asm: format!("lui t2, {upper}"),
    insn: lui(7, imm),
    init: vec![],       // No source register init needed
    check_reg: 7,
});
```

`AluTest` fields: `name` (test name), `asm` (QEMU assembly),
`insn` (machine code), `init` (initial registers),
`check_reg` (comparison target).

#### Running and Debugging

```bash
# Run all difftests
cargo test -p machina-tests difftest

# Run a single difftest
cargo test -p machina-tests difftest_add

# Run in parallel
cargo test -p machina-tests difftest -- --test-threads=4

# Verbose output
cargo test -p machina-tests difftest -- --nocapture
```

**Example failure output**:

```
DIFFTEST FAIL [add]: x7 machina=0x64 qemu=0x65
```

Meaning: for the `add` instruction, register x7 computed by
machina is `0x64`, while the QEMU reference result is
`0x65` -- a mismatch.

#### Limitations and Future Work

1. **x3(gp) is not testable**: reserved on the QEMU side for
   the save area base address
2. **PC-relative instructions**: auipc/jal/jalr require
   computing relative offsets before comparison
3. **Load/Store**: to be expanded after QemuLd/QemuSt is
   complete
4. **Randomized testing**: a random register value generator
   could be introduced to improve coverage
5. **Multi-instruction sequences**: difftests can be extended
   to multi-instruction sequences

### Machine-Level Tests

#### Device Tests

Device tests directly instantiate device instances and
verify register-level behavior:

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

#### Boot Tests

Boot tests load bare-metal firmware in a complete VM and
verify the full path from reset to output:

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

#### Running Commands

```bash
# All machine-level tests
cargo test -p machina-tests machine::

# Device model tests only
cargo test -p machina-tests machine::device

# Boot flow tests only
cargo test -p machina-tests machine::boot

# Build mtest firmware (requires cross compiler)
cd tests/mtest && make
```

### Adding New Tests

#### Adding a Frontend Instruction Test

Add to `tests/src/frontend/mod.rs`:

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

#### Adding an RVC Test

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

#### Adding a Difftest

See "Adding New Difftests" earlier in this part.

#### Adding a Machine-Level Test

1. Create a bare-metal assembly or C firmware source file
   under `tests/mtest/src/`
2. Add build rules to `tests/mtest/Makefile`
3. Add a corresponding Rust test function under
   `tests/src/machine/`
4. Use `MachineBuilder` to construct a VM instance and verify
   output

**Device test template**:

```rust
#[test]
fn test_new_device_register() {
    let mut dev = NewDevice::new();
    dev.write(REG_OFFSET, expected_val);
    assert_eq!(dev.read(REG_OFFSET), expected_val);
}
```

**Boot test template**:

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
