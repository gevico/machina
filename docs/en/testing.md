# Machina Testing

## 1. Overview

Machina employs a layered testing strategy, verifying correctness
progressively from low-level data structures up to full system-level
emulation. All tests are centralized in a standalone `tests/` crate,
keeping source files clean while ensuring complete coverage of public
APIs.

**Test pyramid**:

```
              +----------------+
              |    Difftest    |  machina vs QEMU
              |   (35 tests)   |
              +----------------+
              |    Frontend    |  decode -> IR -> codegen -> execute
              |   (91 tests)   |  RV32I/RV64I/RVC/RV32F
              +----------------+
              |  Integration   |  IR -> liveness -> regalloc
              |  (105 tests)   |  -> codegen -> execute
              +----------------+
              |    Machine     |  mtest framework, device tests
              |   (48 tests)   |  boot tests, MMIO validation
         +----+----------------+----+
         |        Unit Tests        |  core(192) + accel(256)
         |       (685 tests)        |  + decode(93) + exec(26)
         |                          |  + machine(118)
         +--+----+----+----+----+---+
```

**Total: 964 tests**.

---

## 2. Quick Reference

### Rust Test Commands

```bash
# Run all tests
cargo test

# Run by crate
cargo test -p machina-core        # Core IR data structures
cargo test -p machina-accel       # Backend instruction encoding + execution loop
cargo test -p machina-tests       # Main test crate (all layered tests)

# Filter by module
cargo test -p machina-tests core::        # Core module only
cargo test -p machina-tests backend::     # Backend module only
cargo test -p machina-tests decode::      # Decode module only
cargo test -p machina-tests frontend::    # Frontend instruction tests only
cargo test -p machina-tests integration:: # Integration tests only
cargo test -p machina-tests difftest      # Difftests only
cargo test -p machina-tests machine::     # Machine-level tests only

# Run a single test
cargo test -- test_addi
cargo test -- test_c_li

# Verbose output
cargo test -- --nocapture

# Parallelism control
cargo test -- --test-threads=1    # Sequential (for debugging)
cargo test -- --test-threads=4    # 4 threads
```

### Code Quality Checks

```bash
cargo clippy -- -D warnings       # Zero lint warnings
cargo fmt --check                  # Format check
cargo fmt                          # Auto-format
```

### Multi-Threaded vCPU and Performance Regression

```bash
# Multi-threaded vCPU concurrency regression
cargo test -p machina-tests exec::mttcg -- --nocapture

# Print execution statistics (TB hit rate, chain patches, hint hits)
TCG_STATS=1 target/release/machina <machine-config>

# Simple performance comparison (native baseline)
TIMEFORMAT=%R; time target/release/machina <machine-config>
```

---

## 3. Test Architecture

### Directory Structure

```
tests/
+-- Cargo.toml                    # Dependencies: core, accel, frontend,
|                                 #               decode
+-- src/
|   +-- lib.rs                    # Module declarations
|   +-- core/                     # Core IR unit tests (192)
|   |   +-- context.rs
|   |   +-- label.rs
|   |   +-- op.rs
|   |   +-- opcode.rs
|   |   +-- regset.rs
|   |   +-- tb.rs
|   |   +-- temp.rs
|   |   +-- types.rs
|   +-- backend/                  # Backend unit tests (256)
|   |   +-- code_buffer.rs
|   |   +-- x86_64.rs
|   |   +-- mod.rs
|   +-- decode/                   # Decoder generator tests (93)
|   |   +-- mod.rs
|   +-- frontend/                 # Frontend instruction tests (91 + 35)
|   |   +-- mod.rs                #   RV32I/RV64I/RVC execution
|   |   +-- difftest.rs           #   machina vs QEMU differential
|   +-- integration/              # Integration tests (105)
|   |   +-- mod.rs
|   +-- exec/                     # Execution loop tests (26)
|   |   +-- mod.rs
|   +-- machine/                  # Machine-level tests (48)
|       +-- mod.rs                #   mtest framework entry
|       +-- device.rs             #   Device model tests
|       +-- boot.rs               #   Boot flow tests
+-- mtest/                        # mtest test firmware
    +-- Makefile
    +-- src/
        +-- uart_echo.S           # UART loopback test
        +-- timer_irq.S           # Timer interrupt test
        +-- boot_hello.S          # Minimal boot test
```

### Test Distribution by Module

| Module | Tests | Share | Description |
|--------|-------|-------|-------------|
| backend | 256 | 26.6% | x86-64 instruction encoding, code buffer |
| core | 192 | 19.9% | IR types, Opcode, Temp, Label, Op, Context |
| machine | 118 | 12.2% | Device models, MMIO dispatch, boot flow |
| integration | 105 | 10.9% | IR --> codegen --> execute full pipeline |
| decode | 93 | 9.6% | .decode parsing, code generation, field extraction |
| frontend | 91 | 9.4% | RISC-V instruction execution (incl. RVC, RV32F) |
| mtest | 48 | 5.0% | Machine-level firmware tests (UART/Timer/Boot) |
| difftest | 35 | 3.6% | machina vs QEMU differential comparison |
| exec | 26 | 2.7% | TB cache, execution loop, multi-threaded vCPU concurrency |

---

## 4. Unit Tests

### 4.1 Core Module (192 tests)

Verifies correctness of the IR foundational data structures.

| File | Test Coverage |
|------|---------------|
| `types.rs` | Type enum (I32/I64/I128/V64/V128/V256), MemOp bitfield |
| `opcode.rs` | Opcode properties (flags, parameter count, type constraints) |
| `temp.rs` | Temp creation (global/local/const/fixed), TempKind classification |
| `label.rs` | Label creation and reference counting |
| `op.rs` | Op construction, argument access, linked-list operations |
| `context.rs` | Context lifecycle, temp allocation, op emission |
| `regset.rs` | RegSet bitmap operations (insert/remove/contains/iter) |
| `tb.rs` | TranslationBlock creation and caching |

```bash
cargo test -p machina-tests core::
```

### 4.2 Backend Module (256 tests)

Verifies correctness of the x86-64 instruction encoder.

| File | Test Coverage |
|------|---------------|
| `code_buffer.rs` | Code buffer allocation, writes, mprotect switching |
| `x86_64.rs` | All x86-64 instruction encoding (MOV/ADD/SUB/AND/OR/XOR/SHL/SHR/SAR/MUL/DIV/LEA/Jcc/SETcc/CMOVcc/BSF/BSR/LZCNT/TZCNT/POPCNT etc.) |

```bash
cargo test -p machina-tests backend::
```

### 4.3 Decodetree Module (93 tests)

Verifies the `.decode` file parser and code generator.

| Test Group | Count | Description |
|------------|-------|-------------|
| Helper functions | 6 | is_bit_char, is_bit_token, is_inline_field, count_bit_tokens, to_camel |
| Bit-pattern parsing | 4 | Fixed bits, don't-care, inline fields, extra-wide patterns |
| Field parsing | 5 | Unsigned/signed/multi-segment/function-mapped/error handling |
| ArgSet parsing | 4 | Normal/empty/extern/non-extern |
| Continuation & grouping | 4 | Backslash continuation, brace/bracket grouping |
| Full parsing | 5 | mini decode, riscv32, empty input, comment-only, unknown format reference |
| Format inheritance | 2 | args/fields inheritance, bits merging |
| Pattern masks | 4 | R/I/B/Shift type masks |
| Field extraction | 15 | 32-bit register/immediate + 16-bit RVC fields |
| Pattern matching | 18 | 32-bit instruction matching + 11 RVC instruction matching |
| Code generation | 9 | mini/riscv32/ecall/fence/16-bit generation |
| Function handlers | 3 | rvc_register, shift_2, sreg_register |
| 16-bit decode | 2 | insn16.decode parsing and generation |
| Code quality | 2 | No u32 leakage, no duplicate trait methods |

```bash
cargo test -p machina-tests decode::
```

---

## 5. Integration Tests (105 tests)

**Source file**: `tests/src/integration/mod.rs`

Verifies the complete IR --> liveness --> register allocation --> codegen
--> execute pipeline. Uses a minimal RISC-V CPU state and generates
test cases in bulk via macros.

**Test macros**:

| Macro | Purpose |
|-------|---------|
| `riscv_bin_case!` | Binary arithmetic operations (add/sub/and/or/xor) |
| `riscv_shift_case!` | Shift operations (shl/shr/sar/rotl/rotr) |
| `riscv_setcond_case!` | Conditional set operations (eq/ne/lt/ge/ltu/geu) |
| `riscv_branch_case!` | Conditional branches (taken/not-taken) |
| `riscv_mem_case!` | Memory access (load/store at various widths) |

**Coverage**: ALU, shifts, comparisons, branches, memory read/write,
bit operations, rotations, byte swaps, popcount, multiply/divide,
carry/borrow, conditional moves, etc.

```bash
cargo test -p machina-tests integration::
```

---

## 6. Frontend Instruction Tests (91 tests)

**Source file**: `tests/src/frontend/mod.rs`

### 6.1 Test Runners

Frontend tests use four runner functions covering different instruction
formats:

| Function | Input | Purpose |
|----------|-------|---------|
| `run_rv(cpu, insn: u32)` | Single 32-bit instruction | Basic instruction testing |
| `run_rv_insns(cpu, &[u32])` | 32-bit instruction sequence | Multi-instruction sequences |
| `run_rv_bytes(cpu, &[u8])` | Raw byte stream | Mixed 16/32-bit |
| `run_rvc(cpu, insn: u16)` | Single 16-bit instruction | RVC compressed instructions |

**Execution flow** (using `run_rv_insns` as an example):

```
Instruction encoding --> write to guest memory --> translator_loop decode
--> IR generation --> liveness --> regalloc --> x86-64 codegen
--> execute generated code --> read CPU state --> assertion checks
```

### 6.2 RV32I / RV64I Tests

| Category | Instructions | Test Count |
|----------|-------------|------------|
| Upper immediate | lui, auipc | 3 |
| Jumps | jal, jalr | 2 |
| Branches | beq, bne, blt, bge, bltu, bgeu | 12 |
| Immediate arithmetic | addi, slti, sltiu, xori, ori, andi | 8 |
| Shifts | slli, srli, srai | 3 |
| Register arithmetic | add, sub, sll, srl, sra, slt, sltu, xor, or, and | 10 |
| W-suffix | addiw, slliw, srliw, sraiw, addw, subw, sllw, srlw, sraw | 10 |
| System | fence, ecall, ebreak | 3 |
| Special | x0 write-ignored, x0 reads-zero | 2 |
| Multi-instruction | addi+addi sequence, lui+addi combination | 2 |

### 6.3 RVC Compressed Instruction Tests

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

**Instruction encoders**: c_li, c_addi, c_lui, c_mv, c_add, c_sub,
c_slli, c_addi4spn, c_addiw, c_j, c_beqz, c_bnez, c_ebreak.

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
cargo test -p machina-tests frontend::    # All frontend tests
cargo test -p machina-tests test_c_       # RVC tests only
cargo test -p machina-tests test_mixed    # Mixed instruction tests
```

---

## 7. Differential Tests (35 tests)

**Source file**: `tests/src/frontend/difftest.rs`

Differential tests execute the same RISC-V instruction through both
the machina full pipeline and the QEMU reference implementation,
then compare CPU state. If the results match, the machina translation
is considered correct.

**Required tools**:

| Tool | Install Command |
|------|-----------------|
| `riscv64-linux-gnu-gcc` | `apt install gcc-riscv64-linux-gnu` |
| `qemu-riscv64` | `apt install qemu-user` |

### 7.1 Overall Architecture

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

### 7.2 QEMU Side Internals

For each test case, the framework dynamically generates a RISC-V
assembly source:

```asm
.global _start
_start:
    la gp, save_area       # x3 = save area base address

    # -- Phase 1: Load initial register values --
    li t0, <val1>
    li t1, <val2>

    # -- Phase 2: Execute the instruction under test --
    add t2, t0, t1

    # -- Phase 3: Save all 32 registers --
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
save_area: .space 256       # 32 x 8 bytes
```

Compilation and execution flow:

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

Temporary files are named with `pid_tid` to avoid conflicts during
parallel test execution, and are automatically cleaned up afterward.

Branch instructions use a taken/not-taken pattern, where the value
of x7(t2) determines whether the branch was taken
(1=taken, 0=not-taken).

### 7.3 machina Side Internals

ALU instructions directly reuse the full-pipeline infrastructure:

```rust
fn run_machina(
    init: &[(usize, u64)],  // Initial register values
    insns: &[u32],           // RISC-V machine code sequence
) -> RiscvCpu
```

Pipeline: `RISC-V machine code --> decode --> trans_* --> TCG IR
--> optimize --> liveness --> regalloc --> x86-64 codegen --> execute`

Branch instructions exit the translation block (TB), and
taken/not-taken is determined by the PC value:
- `PC = offset` --> taken
- `PC = 4` --> not-taken

### 7.4 Register Conventions

| Register | ABI Name | Purpose |
|----------|----------|---------|
| x3 | gp | **Reserved**: QEMU-side save area base address |
| x5 | t0 | Source operand 1 (rs1) |
| x6 | t1 | Source operand 2 (rs2) |
| x7 | t2 | Destination register (rd) |

x3 cannot be used as a test register because the QEMU-side
`la gp, save_area` overwrites its value.

### 7.5 Boundary Value Strategy

| Constant | Value | Meaning |
|----------|-------|---------|
| `V0` | `0` | Zero |
| `V1` | `1` | Smallest positive number |
| `VMAX` | `0x7FFF_FFFF_FFFF_FFFF` | i64 maximum |
| `VMIN` | `0x8000_0000_0000_0000` | i64 minimum |
| `VNEG1` | `0xFFFF_FFFF_FFFF_FFFF` | -1 (all ones) |
| `V32MAX` | `0x7FFF_FFFF` | i32 maximum |
| `V32MIN` | `0xFFFF_FFFF_8000_0000` | i32 minimum (sign-extended) |
| `V32FF` | `0xFFFF_FFFF` | u32 maximum |
| `VPATTERN` | `0xDEAD_BEEF_CAFE_BABE` | Random bit pattern |

Each instruction uses 4-7 boundary value combinations, focusing on
overflow boundaries, sign extension, zero behavior, and all-ones
bit patterns.

### 7.6 Current Coverage

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

### 7.7 Adding New Difftests

**Adding an R-type instruction** (using `mulw` as an example):

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

**Adding an I-type instruction** (using `sltiu` as an example):

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
    init: vec![],       // No source register initialization needed
    check_reg: 7,
});
```

`AluTest` fields: `name` (test name), `asm` (QEMU assembly),
`insn` (machine code), `init` (initial registers),
`check_reg` (comparison target).

### 7.8 Running and Debugging

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

Meaning: for the `add` instruction, register x7 computed by machina
is `0x64`, while the QEMU reference result is `0x65` -- a mismatch.

### 7.9 Limitations and Future Work

1. **x3(gp) is not testable**: reserved on the QEMU side for the save area base address
2. **PC-relative instructions**: auipc/jal/jalr require computing relative offsets before comparison
3. **Load/Store**: to be expanded after QemuLd/QemuSt is complete
4. **Randomized testing**: a random register value generator could be introduced to improve coverage
5. **Multi-instruction sequences**: difftests can be extended to multi-instruction sequences

---

## 8. Machine-Level Tests (mtest Framework)

**Directory**: `tests/mtest/`

mtest is machina's full system-level test framework. It runs bare-metal
firmware inside a complete virtual machine environment, verifying
end-to-end correctness of device models, interrupt controllers,
memory-mapped I/O, and boot flows.

### 8.1 Architecture Overview

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

### 8.2 Test Categories

| Category | Tests | Description |
|----------|-------|-------------|
| Device models | 20 | UART register read/write, CLINT MMIO, PLIC dispatch |
| MMIO dispatch | 10 | AddressSpace routing, overlapping regions, unmapped access |
| Boot flow | 8 | Minimal firmware loading, PC reset vector, M-mode initialization |
| Interrupts | 6 | Timer interrupt trigger and response, external interrupt routing |
| Multi-core | 4 | SMP startup, IPI send and receive |

### 8.3 Device Tests

Device tests directly instantiate device instances and verify
register-level behavior:

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

### 8.4 Boot Tests

Boot tests load bare-metal firmware in a complete VM and verify the
full path from reset to output:

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

### 8.5 Running Commands

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

---

## 9. Adding New Tests

### Adding a Frontend Instruction Test

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

### Adding an RVC Test

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

### Adding a Difftest

See Section 7.7 "Adding New Difftests" in this document.

### Adding a Machine-Level Test

1. Create a bare-metal assembly or C firmware source file under `tests/mtest/src/`
2. Add build rules to `tests/mtest/Makefile`
3. Add a corresponding Rust test function under `tests/src/machine/`
4. Use `MachineBuilder` to construct a VM instance and verify output

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
        .load_firmware("tests/mtest/bin/new_test.bin")
        .build();
    let output = vm.run_until_halt(Duration::from_secs(5));
    assert!(output.uart_output().contains("PASS"));
}
```
