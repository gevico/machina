# Machina Design Document

## 1. Overview

Machina is a RISC-V full-system emulator that reimplements QEMU's TCG (Tiny Code Generator) dynamic binary translation engine in Rust. It translates guest architecture instructions into host machine code at runtime, and provides complete device models, a memory subsystem, and interrupt controllers to support full-system emulation.

For the current MOM device-model architecture, see `docs/en/mom.md`.

```
+-------------+    +------------+    +--------+    +-----------+    +----------+
| Guest       |--->| Frontend   |--->| TCG IR |--->| Optimizer |--->| Backend  |
| Binary      |    | (decode)   |    |        |    |           |    | (codegen)|
+-------------+    +------------+    +--------+    +-----------+    +----+-----+
                                                                        |
                                                                        v
                                                                   +----+-----+
                                                                   | TB Cache |
                                                                   +----+-----+
                                                                        |
                                                                        v
+----------------+    +----------+    +-----------+    +----------+-----+-----+
| Full-System    |<---| Device   |<---| Memory    |<---| Exec Loop            |
| (WFI/IRQ/SBI) |    | (hw/)    |    | (MMIO)    |    | (Multi-vCPU)              |
+----------------+    +----------+    +-----------+    +----------------------+
```

## 2. Workspace Layering

```
machina/
+-- core/           # IR definition layer: CPU trait, address types, pure data structures
+-- accel/          # Acceleration layer: IR optimization, register allocation, x86-64 codegen, execution engine
+-- guest/riscv/    # RISC-V frontend: RV64GC + privileged ISA, Sv39 MMU
+-- decode/         # Decoder generator: parses .decode files, generates Rust decoders
+-- system/         # Full-system execution: CPU management, WFI wakeup, FullSystemCpu
+-- memory/         # Memory subsystem: AddressSpace, MemoryRegion, MMIO dispatch
+-- hw/core/        # Device infrastructure: qdev, IRQ, chardev, FDT, loader
+-- hw/intc/        # Interrupt controllers: PLIC, ACLINT
+-- hw/char/        # Character devices: UART 16550A
+-- hw/riscv/       # RISC-V machine definitions: riscv64-ref
+-- disas/          # Disassembler
+-- monitor/        # Debug interface
+-- util/           # Shared utilities
+-- tools/irdump/   # IR dump tool
+-- tools/irbackend/# Backend test tool
+-- tests/          # Test layer: unit, integration, difftest, multi-vCPU
+-- tests/mtest/    # Machine-level tests
```

**Design intent**: Following QEMU's separation principle between `include/tcg/` (definitions) and `tcg/` (implementation). `machina-core` is a pure data definition crate containing no platform-specific code or `unsafe`. Both `machina-guest-riscv` and `machina-accel` (including the optimizer) only need to depend on `machina-core`. `decode` is a standalone build-time tool crate that parses QEMU-style `.decode` files and generates Rust decoder code. The `memory/` and `hw/` layers provide the address space model and device tree required for full-system emulation. Tests are separated into their own crate to keep source files clean, and external crate tests can verify the completeness of public APIs.

### 2.1 Multi-threaded vCPU Support and Execution Flow Alignment

The current execution layer already supports the core multi-threaded vCPU model, located at `accel/src/exec/exec_loop.rs`:

1. `cpu_exec_loop_mt(shared, per_cpu, cpu)` serves as the multi-threaded entry point;
2. Lookup order: `JumpCache` (per vCPU) --> global TB hash;
3. On miss, enters `tb_gen_code`, serialized by `translate_lock`;
4. After TB execution, dispatches by exit protocol:
   - `TB_EXIT_IDX0/1`: chainable exits, attempt `tb_add_jump` patch;
   - `TB_EXIT_NOCHAIN`: indirect exit, uses `exit_target` cache + table lookup;
   - Other values: real exceptions / system exit.

This is structurally aligned with QEMU's `cpu_exec` / `tb_lookup` / `tb_gen_code` / `cpu_tb_exec` main flow, with the current focus on "correctness first + hot-path observability".

---

## 3. machina-core Core Data Structures

### 3.1 Type System (`types.rs`)

```
Type: I32 | I64 | I128 | V64 | V128 | V256
```

- `#[repr(u8)]` ensures enum values can be used directly as array indices (`Type as usize`)
- Integer/vector classification methods (`is_integer()` / `is_vector()`) are used for type dispatch in the optimizer and backend
- `TYPE_COUNT = 6` works with `const_table: [HashMap; TYPE_COUNT]` in Context to implement per-type bucketed constant deduplication

### 3.2 Cond Condition Codes (`types.rs`)

```
Cond: Never=0, Always=1, Eq=8, Ne=9, Lt=10, ..., TstEq=18, TstNe=19
```

- **Encoding values directly align with QEMU** (`TCGCond` values in `tcg.h`), enabling zero-cost conversion during frontend translation
- `invert()` and `swap()` are both involutions (self-inverse), which is specifically verified in tests
- `TstEq`/`TstNe` are test-and-branch conditions added in QEMU 7.x+, included proactively

### 3.3 MemOp (`types.rs`)

```
MemOp(u16) -- bit-packed: [1:0]=size, [2]=sign, [3]=bswap, [6:4]=align
```

- Bit-field packing design directly maps to QEMU's `MemOp`, maintaining binary compatibility
- Provides semantic constructors `ub()/sb()/uw()/sw()/ul()/sl()/uq()` to avoid hand-written bit manipulation

### 3.4 RegSet (`types.rs`)

```
RegSet(u64) -- 64-bit bitmap, supports up to 64 host registers
```

- Uses a `u64` bitmap instead of `HashSet` or `Vec`, because register allocation is a hot path and bit operations (union/intersect/subtract) are an order of magnitude faster than set operations
- `const fn` methods allow compile-time construction of constant register sets (e.g., `RESERVED_REGS`)

### 3.5 Unified Polymorphic Opcode (`opcode.rs`)

```
enum Opcode { Mov, Add, Sub, ..., Count }  // 158 variants + sentinel
```

**Key decision: type polymorphism over type splitting**

In QEMU's original design, `add_i32` and `add_i64` are separate opcodes. We use a unified `Add` instead, with the actual type carried in the `Op::op_type` field. Rationale:

1. Reduces opcode count (unified polymorphic design)
2. The optimizer can use unified logic without needing `match (Add32, Add64) => ...`
3. The backend selects 32/64-bit instruction encoding via `op.op_type`, resulting in cleaner logic
4. `OpFlags::INT` marks which opcodes are polymorphic; non-polymorphic ones (e.g., `ExtI32I64`) have fixed types

### 3.6 OpDef Static Table (`opcode.rs`)

```rust
pub static OPCODE_DEFS: [OpDef; Opcode::Count as usize] = [ ... ];
```

- Uses `Opcode::Count` as a sentinel to ensure table size stays in sync with the enum — if a new opcode is added without a table entry, a compile-time error is raised
- Each `OpDef` records `nb_oargs/nb_iargs/nb_cargs/flags`, which is core metadata for the optimizer and register allocator
- `OpFlags` uses bit flags rather than `Vec<Flag>`, because flag checks are extremely frequent in the compilation loop

### 3.7 Temp Variables (`temp.rs`)

```
TempKind: Ebb | Tb | Global | Fixed | Const
```

Five lifetime kinds directly mapping to QEMU's `TCGTempKind`:

| Kind | Lifetime | Typical Usage |
|------|----------|---------------|
| `Ebb` | Single extended basic block | Arithmetic intermediate results |
| `Tb` | Entire translation block | Values spanning BBs |
| `Global` | Cross-TB, backed by CPUState | `pc`, `sp`, etc. |
| `Fixed` | Fixed-bound to a host register | `env` (RBP) |
| `Const` | Compile-time constant | Immediates |

The `Temp` struct carries both IR attributes (`ty`, `kind`) and register allocation state (`val_type`, `reg`, `mem_coherent`). This is QEMU's design — it avoids extra side-table lookups.

### 3.8 Label Forward References (`label.rs`)

```
Label { present, has_value, value, uses: Vec<LabelUse> }
LabelUse { offset, kind: RelocKind::Rel32 }
```

- Supports forward references: branch instructions can reference a label before it is defined
- `uses` records all unresolved reference locations; when `set_value()` is called, the backend traverses `uses` for back-patching
- `RelocKind` currently only has `Rel32` (x86-64 RIP-relative 32-bit displacement); `Adr21` and similar variants will be added when AArch64 support is implemented

### 3.9 Op IR Operations (`op.rs`)

```rust
struct Op {
    opc: Opcode,
    op_type: Type,        // actual type for polymorphic opcodes
    param1/param2: u8,    // opcode-specific (CALLI/CALLO/VECE)
    life: LifeData,       // liveness analysis results
    output_pref: [RegSet; 2],  // register allocation hints
    args: [TempIdx; 10],  // arguments (outputs + inputs + constants)
}
```

- `args` is a fixed-size array rather than `Vec`, avoiding heap allocation — each TB may have hundreds of Ops
- `oargs()/iargs()/cargs()` slice through `OpDef` argument counts, a zero-cost abstraction
- `LifeData(u32)` encodes dead/sync state using 2 bits per arg, compact and efficient

### 3.10 Context Translation Context (`context.rs`)

```rust
struct Context {
    temps: Vec<Temp>,
    ops: Vec<Op>,
    labels: Vec<Label>,
    nb_globals: u32,
    const_table: [HashMap<u64, TempIdx>; TYPE_COUNT],
    // frame, reserved_regs, gen_insn_end_off...
}
```

**Key design points**:

- **Globals at the front of the temps array**: `temps[0..nb_globals]` are global variables. On `reset()`, `truncate(nb_globals)` preserves them while clearing all local variables. This avoids re-registering global variables each time a new TB is translated
- **Constant deduplication**: `const_table` is bucketed by type; constants with the same `(type, value)` create only one Temp. In QEMU this is an important memory optimization, since many instructions share the same immediates (0, 1, -1, etc.)
- **Assertion guards**: `new_global()` and `new_fixed()` must be called before any local variable allocation, enforced by `assert_eq!(temps.len(), nb_globals)`

### 3.11 TranslationBlock (`tb.rs`)

```rust
struct TranslationBlock {
    // immutable after creation
    pc, flags, cflags,
    host_offset, host_size,
    jmp_insn_offset: [Option<u32>; 2],
    jmp_reset_offset: [Option<u32>; 2],
    // mutable chaining state
    jmp: Mutex<TbJmpState>,
    invalid: AtomicBool,
    exit_target: AtomicUsize,
}
```

- **Dual-exit + NoChain protocol**: `TB_EXIT_IDX0/1` take the chainable path, `TB_EXIT_NOCHAIN` takes the indirect path; real exception exit values start from `TB_EXIT_MAX` to avoid protocol conflicts.
- **Concurrent chaining state**: `jmp` maintains incoming/outgoing edge relationships, used for unlinking during TB invalidation; `invalid` uses an atomic bit for lock-free fast checking.
- **Indirect target cache**: `exit_target` provides a most-recent target TB cache for `TB_EXIT_NOCHAIN`, reducing hash lookup overhead.
- **JumpCache**: `Box<[Option<usize>; 4096]>` direct-mapped cache, indexed by `(pc >> 2) & 0xFFF`, O(1) lookup.
- **Hash function**: `pc * 0x9e3779b97f4a7c15 ^ flags`, the golden ratio constant ensures stable distribution.

---

## 4. machina-accel Code Generation Layer

### 4.1 CodeBuffer (`code_buffer.rs`)

```
mmap(PROT_READ|PROT_WRITE) --> emit code --> mprotect(PROT_READ|PROT_EXEC)
```

- **W^X discipline**: write and execute are mutually exclusive; `set_executable()` / `set_writable()` toggles permissions
- `emit_u8/u16/u32/u64/bytes` + `patch_u32` covers all x86-64 instruction encoding needs
- `write_unaligned` handles unaligned writes (x86 allows this, but ARM does not — future consideration needed)

### 4.2 HostCodeGen trait (`lib.rs`)

```rust
trait HostCodeGen {
    fn emit_prologue(&mut self, buf: &mut CodeBuffer);
    fn emit_epilogue(&mut self, buf: &mut CodeBuffer);
    fn patch_jump(&mut self, buf: &mut CodeBuffer, jump_offset, target_offset);
    fn epilogue_offset(&self) -> usize;
    fn init_context(&self, ctx: &mut Context);
    fn op_constraint(&self, opc: Opcode) -> &'static OpConstraint;
    // + register allocator primitives: tcg_out_mov/movi/ld/st/op
}
```

- Trait-based rather than conditional compilation, allowing the same binary to support multiple backends (testing/simulation scenarios)
- `init_context()` lets the backend inject platform-specific configuration into Context (reserved registers, stack frame layout)
- `op_constraint()` returns per-opcode register constraints for the generic register allocator to consume (see 4.3)

### 4.3 Constraint System (`constraint.rs`)

```rust
struct ArgConstraint {
    regs: RegSet,       // allowed registers
    oalias: bool,       // output aliases an input
    ialias: bool,       // input is aliased to an output
    alias_index: u8,    // which arg it aliases
    newreg: bool,       // output must not overlap any input
}

struct OpConstraint {
    args: [ArgConstraint; MAX_OP_ARGS],
}
```

Declaratively describes the register allocation requirements of each opcode, aligned with QEMU's `TCGArgConstraint` + `C_O*_I*` macro system.

**Constraint types**:

| Constraint | Meaning | QEMU Equivalent | Typical Usage |
|------------|---------|-----------------|---------------|
| `oalias` | Output reuses an input's register | `"0"` (alias) | Destructive binary ops (SUB/AND/...) |
| `ialias` | Input can be reused by an output | Corresponds to oalias's input side | Paired with oalias |
| `newreg` | Output must not overlap any input | `"&"` (newreg) | SetCond (setcc only writes low byte) |
| `fixed` | Single-register constraint | `"c"` (RCX) | Shift count must be in RCX |

**Builder functions**:

| Function | Signature | Usage |
|----------|-----------|-------|
| `o1_i2(o0, i0, i1)` | Three-address | Add (LEA) |
| `o1_i2_alias(o0, i0, i1)` | Output aliases input0 | Sub/Mul/And/Or/Xor |
| `o1_i1_alias(o0, i0)` | Unary alias | Neg/Not |
| `o1_i2_alias_fixed(o0, i0, reg)` | Alias + fixed | Shl/Shr/Sar (RCX) |
| `n1_i2(o0, i0, i1)` | Newreg output | SetCond |
| `o0_i2(i0, i1)` | No output | BrCond/St |
| `o2_i2_fixed(o0, o1, i1)` | Dual fixed output + alias | MulS2/MulU2 (RAX:RDX) |
| `o2_i3_fixed(o0, o1, i2)` | Dual fixed output + dual alias | DivS2/DivU2 (RAX:RDX) |
| `o1_i4_alias2(o0, i0..i3)` | Output aliases input2 | MovCond (CMOV) |

### 4.4 x86-64 Stack Frame Layout (`x86_64/regs.rs`)

```
High address
+---------------------+
| return address (8B) |  <-- call pushes this
+---------------------+
| push rbp    (8B)    |  <-- CALLEE_SAVED[0]
| push rbx    (8B)    |
| push r12    (8B)    |
| push r13    (8B)    |
| push r14    (8B)    |
| push r15    (8B)    |  PUSH_SIZE = 56B
+---------------------+
| STATIC_CALL_ARGS    |  128B (outgoing call args)
| CPU_TEMP_BUF        |  1024B (spill slots)
|                     |  STACK_ADDEND = FRAME_SIZE - PUSH_SIZE
+---------------------+
|                     |  <-- RSP (16-byte aligned)
+---------------------+
Low address
```

- `FRAME_SIZE` is computed at compile time and 16-byte aligned, satisfying the System V ABI requirement
- `TCG_AREG0 = RBP`: the env pointer is fixed in RBP, matching the QEMU convention. All TB code accesses CPUState through RBP

### 4.5 Prologue/Epilogue (`x86_64/emitter.rs`)

**Prologue**:

1. `push` 6 callee-saved registers (RBP first)
2. `mov rbp, rdi` -- store the first argument (env pointer) into TCG_AREG0
3. `sub rsp, STACK_ADDEND` -- allocate the stack frame
4. `jmp *rsi` -- jump to the second argument (TB host code address)

**Epilogue (dual entry)**:

- `epilogue_return_zero`: `xor eax, eax` --> fall through (used when `goto_ptr` lookup fails)
- `tb_ret`: `add rsp` --> `pop` registers --> `ret` (used for normal `exit_tb` return)

This dual-entry design avoids a redundant `mov rax, 0` instruction when `exit_tb(0)` is called.

### 4.6 TB Control Flow Instructions

- **`exit_tb(val)`**: when val==0, directly `jmp epilogue_return_zero`; otherwise `mov rax, val` + `jmp tb_ret`
- **`goto_tb`**: emits `E9 00000000` (JMP rel32), with NOP padding to ensure the disp32 field is 4-byte aligned, making atomic patching during TB chaining safe
- **`goto_ptr(reg)`**: `jmp *reg`, used for indirect jumps (after lookup_and_goto_ptr)

---

## 5. Translation Pipeline

The complete translation pipeline converts TCG IR into executable host machine code:

```
Guest Binary --> Frontend (decode) --> IR Builder (gen_*) --> Optimize --> Liveness --> RegAlloc + Codegen --> Execute
                 riscv/trans.rs        ir_builder.rs          optimize.rs  liveness.rs  regalloc.rs          translate.rs
                                                                                         codegen.rs
```

### 5.1 IR Builder (`ir_builder.rs`)

The `gen_*` methods on `impl Context` convert high-level operations into `Op` and append them to the ops list. Each method creates `Op::with_args()` and sets the correct opcode, type, and args layout.

**Constant argument encoding**: Constant arguments such as condition codes, offsets, and label IDs are encoded as `TempIdx(raw_value as u32)` and stored in `args[]`, consistent with the QEMU convention.

**Implemented IR generation methods**:

| Category | Method | Signature |
|----------|--------|-----------|
| Binary ALU | `gen_add/sub/mul/and/or/xor/shl/shr/sar` | (ty, d, a, b) --> d |
| Unary | `gen_neg/not/mov` | (ty, d, s) --> d |
| Conditional set | `gen_setcond` | (ty, d, a, b, cond) --> d |
| Memory access | `gen_ld` / `gen_st` | (ty, dst/src, base, offset) |
| Control flow | `gen_br/brcond/set_label` | (label_id) / (ty, a, b, cond, label) |
| TB exit | `gen_goto_tb/exit_tb` | (tb_idx) / (val) |
| Boundary | `gen_insn_start` | (pc) |

### 5.2 IR Optimizer (`optimize.rs`)

A single-pass forward-scan optimizer that runs before liveness analysis, aligned with QEMU's `tcg/optimize.c`. It uses per-temp `TempInfo` to track constant values and copy sources.

**Data structures**:

```rust
struct TempInfo {
    is_const: bool,
    val: u64,
    copy_of: Option<TempIdx>,  // canonical copy source
}
```

Initialized by reading constant information from existing `TempKind::Const` temps.

**Optimization categories**:

| Category | Trigger Condition | Action |
|----------|-------------------|--------|
| Copy propagation | Input temp has `copy_of` | Replace with source temp |
| Constant folding (unary) | Neg/Not input is constant | --> `Mov dst, const` |
| Constant folding (binary) | Add/Sub/Mul/And/Or/Xor/AndC/Shl/Shr/Sar/RotL/RotR both inputs are constants | --> `Mov dst, const` |
| Constant folding (type conversion) | ExtI32I64/ExtUI32I64/ExtrlI64I32/ExtrhI64I32 input is constant | --> `Mov dst, const` |
| Algebraic simplification | One input is a constant (0, 1, -1) | `x+0-->x`, `x*0-->0`, `x&-1-->x`, etc. |
| Same-operand identity | Both inputs are identical | `x&x-->x`, `x^x-->0`, `x-x-->0` |
| Branch folding | BrCond both inputs are constants | Always-true-->Br, always-false-->Nop |
| Strength reduction | `0 - x` | --> `Neg x` |

**BB boundary handling**: When encountering SetLabel/Br/ExitTb/GotoTb/GotoPtr/Call, all copy relationships are cleared, because cross-BB copy information is unreliable.

**Type masking**: I32 operation results are truncated to 32 bits (`val & 0xFFFF_FFFF`); I64 results are kept at 64 bits.

**Op replacement strategy**: Optimized ops are replaced in-place — constant folding results become `Mov dst, const_temp`, algebraic simplifications become `Mov dst, surviving_input`, always-false branches become `Nop`, and always-true branches become `Br`.

**Key design decision**: `replace_with_mov` uses a conservative strategy — only `invalidate_one(dst)` rather than `set_copy(dst, src)`. This avoids a bug where the destination temp retains stale constant information when the source temp is redefined by a subsequent op. Only explicit `Mov` ops (`fold_mov`) establish copy relationships.

### 5.3 Liveness Analysis (`liveness.rs`)

Traverses the ops list in reverse, computing `LifeData` for each op, marking which arguments die after that op (dead) and which global variables need to be synced back to memory (sync).

**Algorithm**:

1. Initialize `temp_state[0..nb_temps]` = false (all dead)
2. At TB end: mark all global variables as live
3. Reverse traversal of each op:
   - On encountering the `BB_END` flag: mark all global variables as live
   - Output arguments: if `!temp_state[tidx]` --> mark dead; then `temp_state[tidx] = false`
   - Input arguments: if `!temp_state[tidx]` --> mark dead (last use), mark sync if global; then `temp_state[tidx] = true`
4. Write the computed `LifeData` back to `op.life`

### 5.4 Register Allocator (`regalloc.rs`)

A constraint-driven greedy per-op allocator that traverses the ops list forward, aligned with QEMU's `tcg_reg_alloc_op()`. The MVP does not support spilling — 14 allocatable GPRs are sufficient for simple TBs.

#### 5.4.1 Architecture Overview

QEMU's register allocator `tcg_reg_alloc_op()` (in `tcg/tcg.c`) is fully generic — it contains no per-opcode branches. Each opcode's special requirements (such as SUB's destructive semantics or SHL's RCX requirement) are entirely described declaratively through `TCGArgConstraint`, and the allocator simply reads constraints and executes unified logic.

machina's `regalloc_op()` aligns with this architecture:

```
                    +--------------+
                    | OpConstraint |  <-- backend.op_constraint(opc)
                    +------+-------+
                           |
                           v
  +--------------------------------------------------+
  |           regalloc_op() -- generic path           |
  |                                                   |
  |  1. load inputs   -> 2. fixup -> 3. free dead in  |
  |  4. alloc outputs -> 5. emit  -> 6. free dead out |
  |                       7. sync globals             |
  +---------------------------------------------------+
```

This means that adding a new opcode only requires adding one row to the constraint table — no modifications to the allocator or codegen are needed.

#### 5.4.2 Allocator State

```rust
struct RegAllocState {
    reg_to_temp: [Option<TempIdx>; 16],
    free_regs: RegSet,
    allocatable: RegSet,
}
```

| Field | Meaning |
|-------|---------|
| `reg_to_temp` | Maps each of the 16 host registers to a temp (None = free) |
| `free_regs` | Bitmap of currently free and allocatable registers |
| `allocatable` | Set of allocatable registers (invariant, excludes RSP/RBP) |

**Initialization**: `free_regs = allocatable`, then iterate over all Fixed temps (e.g., env/RBP) and mark them as occupied (`assign(reg, tidx)`).

**Temp state machine**: Each `Temp` has a `val_type` field tracking its current location:

```
                 temp_load_to()
    +------+    +--------------+    +-----+
    | Dead |--->| Const / Mem  |--->| Reg |
    +------+    +--------------+    +--+--+
       ^                               |
       +--------- temp_dead() ---------+
                                 (local temp)
```

- **Dead**: Unallocated, not occupying any resources
- **Const**: Compile-time constant, needs `movi` to load into a register
- **Mem**: Global variable in memory, needs `ld` to load into a register
- **Reg**: Already in a host register, can be used directly

Global variables and fixed temps never enter the Dead state — `temp_dead()` is a no-op for them.

#### 5.4.3 Main Loop Dispatch

`regalloc_and_codegen()` traverses the ops list forward, dispatching by opcode:

| Op Type | Handling Strategy | Reason |
|---------|-------------------|--------|
| Nop/InsnStart | Skip | No code generation |
| Mov | Dedicated path | Register rename optimization (QEMU also handles this separately) |
| SetLabel | sync --> resolve label --> back-patch | Control flow merge point |
| Br | sync --> emit jmp | Unconditional jump |
| BrCond | Constraint load --> sync --> emit cmp+jcc | Needs sync before emit |
| ExitTb/GotoTb | sync --> delegate to tcg_out_op | TB exit |
| GotoPtr | Constraint load --> sync --> emit jmp *reg | Indirect jump |
| Mb | Directly emit mfence | Memory barrier |
| **Others** | **`regalloc_op()`** | **Generic constraint-driven path** |

**Why doesn't BrCond use the generic path?** Because BrCond needs to sync globals before emit (the branch target may be another BB), whereas the generic path syncs after emit. Additionally, BrCond's forward references require recording `label.add_use()` after emit.

#### 5.4.4 Differences from QEMU

| Aspect | QEMU | machina |
|--------|------|---------|
| Spilling | Supports spilling to stack frame `CPU_TEMP_BUF` | Not supported (14 GPRs are sufficient) |
| Immediate constraints | `re`/`ri` allow immediates to be encoded directly | All inputs must be in registers |
| Output preference | `output_pref` set by the constraint system | Set by liveness analysis |
| Constant inputs | Can be inlined into instruction encoding | Must first `movi` into a register |
| Memory inputs | Some instructions support `[mem]` operands | Must first `ld` into a register |

### 5.5 Pipeline Orchestration (`translate.rs`)

Chains the stages into a complete pipeline:

```
translate():
    optimize(ctx)
    liveness_analysis(ctx)
    tb_start = buf.offset()
    regalloc_and_codegen(ctx, backend, buf)
    return tb_start

translate_and_execute():
    buf.set_writable()
    tb_start = translate(ctx, backend, buf)
    buf.set_executable()
    prologue_fn = transmute(buf.base_ptr())
    return prologue_fn(env, tb_ptr)
```

**Prologue calling convention**:
`fn(env: *mut u8, tb_ptr: *const u8) -> usize`
- RDI = env pointer (prologue stores it into RBP)
- RSI = TB code address (prologue jumps to this)
- Return value RAX = `exit_tb` value

### 5.6 End-to-End Integration Tests

`tests/src/integration/mod.rs` validates the complete pipeline using a minimal RISC-V CPU state:

```rust
#[repr(C)]
struct RiscvCpuState {
    regs: [u64; 32],  // x0-x31, offset 0..256
    pc: u64,          // offset 256
}
```

Registers x0-x31 and pc are registered as global variables via `ctx.new_global()`, backed by `RiscvCpuState` fields.

**Test cases**:

| Test | Verified Behavior |
|------|-------------------|
| `test_addi_x1_x0_42` | Constant addition: x1 = x0 + 42 |
| `test_add_x3_x1_x2` | Register addition: x3 = x1 + x2 |
| `test_sub_x3_x1_x2` | Register subtraction: x3 = x1 - x2 |
| `test_beq_taken` | Conditional branch (taken path) |
| `test_beq_not_taken` | Conditional branch (not-taken path) |
| `test_sum_loop` | Loop: compute 1+2+3+4+5=15 |

---

## 6. machina-accel Execution Layer

### 6.1 SharedState / PerCpuState Separation

The execution layer splits state into shared and per-CPU parts, aligned with the multi-threaded vCPU model:

```rust
struct SharedState<B: HostCodeGen> {
    tb_store: TbStore,              // global TB cache + hash table
    code_buf: UnsafeCell<CodeBuffer>, // JIT code buffer
    backend: B,                     // host code generator
    code_gen_start: usize,          // code start offset after prologue
    translate_lock: Mutex<TranslateGuard>, // serializes translation
}

struct PerCpuState {
    jump_cache: JumpCache,  // 4096-entry direct-mapped TB cache
    stats: ExecStats,       // execution statistics
}
```

`SharedState` is shared via `&` across all vCPU threads — `code_buf` is wrapped in `UnsafeCell`, with the write path protected by `translate_lock` and the read path (executing generated code, patching jumps) being lock-free. `PerCpuState` is exclusively owned by each thread and requires no synchronization.

**TbStore** uses `UnsafeCell<Vec<TranslationBlock>>` + `AtomicUsize` length counter to implement lock-free reads: new TBs are published via `Acquire/Release` semantics, so readers need no locking. The hash table (32768 buckets) uses `Mutex` to protect writes.

### 6.2 GuestCpu trait

```rust
trait GuestCpu {
    fn get_pc(&self) -> u64;
    fn get_flags(&self) -> u32;
    fn gen_code(
        &mut self, ir: &mut Context, pc: u64, max_insns: u32,
    ) -> u32;
    fn env_ptr(&mut self) -> *mut u8;
}
```

Each guest architecture (e.g., RISC-V) implements this trait, decoupling frontend decoding from the execution engine. `gen_code()` is responsible for decoding guest instructions and generating TCG IR, returning the number of translated guest bytes. `env_ptr()` returns a pointer to the CPU state structure, passed to the generated host code (accessed via RBP).

### 6.3 Execution Loop

`cpu_exec_loop_mt()` is the main multi-threaded vCPU loop, aligned with QEMU's `cpu_exec`:

```
loop {
    1. next_tb_hint fast path: reuse the previous jump's target TB
    2. tb_find(pc, flags):
       jump_cache --> hash table --> tb_gen_code()
    3. cpu_tb_exec(tb_idx) --> raw_exit
    4. decode_tb_exit(raw_exit) --> (last_tb, exit_code)
    5. dispatch by exit_code:
       0/1  --> tb_add_jump() link + set next_tb_hint
       NOCHAIN --> exit_target cache + table lookup
       >= MAX --> return ExitReason
}
```

**tb_gen_code** flow: check buffer space --> acquire `translate_lock` --> double-check (another thread may have already translated) --> allocate TB --> frontend generates IR --> backend generates host code --> record `goto_tb` offsets --> insert into hash table and jump cache.

### 6.4 TB Lifecycle

```
Lookup --> Miss --> Translate --> Cache --> Execute --> Link --> [Invalidate]
```

**Linking** (`tb_add_jump`): Verify that the source TB's `jmp_insn_offset[slot]` is valid and the target is not invalidated --> lock source TB --> call `backend.patch_jump()` to modify the jump instruction --> update outgoing edge `jmp_dest[slot]` --> lock target TB --> add reverse edge `jmp_list.push((src, slot))`.

**Invalidation** (`TbStore::invalidate`): Mark `tb.invalid = true` --> traverse incoming edges `jmp_list` calling `reset_jump()` to restore jumps --> clear outgoing edges `jmp_dest` and remove from target TB's `jmp_list` --> remove from hash chain.

---

## 7. machina-guest-riscv Guest Decoding Layer

### 7.1 decode Decoder Generator

The `decode` crate implements a Rust version of QEMU's decodetree tool, parsing `.decode` files and generating Rust decoder code.

**Input**: `guest/riscv/src/riscv/insn32.decode` (RV64IMAFDC instruction patterns)

**Generated code**:
- `Args*` structs: one struct per argument set (e.g., `ArgsR { rd, rs1, rs2 }`)
- `extract_*` functions: extract fields from 32-bit instruction words (supporting multi-segment concatenation, sign extension)
- `Decode<Ir>` trait: one `trans_*` method per pattern
- `decode()` function: if-else chain matching instructions by fixedmask/fixedbits

**Build integration**: `guest/riscv/build.rs` calls `decode::generate()` at compile time, outputting to `$OUT_DIR/riscv32_decode.rs`, included via the `include!` macro.

### 7.2 TranslatorOps trait

`guest/riscv/src/lib.rs` defines an architecture-independent translation framework:

```rust
trait TranslatorOps {
    type Disas;
    fn init_disas_context(ctx: &mut Self::Disas, ir: &mut Context);
    fn tb_start(ctx: &mut Self::Disas, ir: &mut Context);
    fn insn_start(ctx: &mut Self::Disas, ir: &mut Context);
    fn translate_insn(ctx: &mut Self::Disas, ir: &mut Context);
    fn tb_stop(ctx: &mut Self::Disas, ir: &mut Context);
}
```

`translator_loop()` implements the translation loop from QEMU's `accel/tcg/translator.c`: `tb_start --> (insn_start + translate_insn)* --> tb_stop`.

### 7.3 RISC-V Frontend (Including Floating Point)

**CPU state** (`riscv/cpu.rs`):

```rust
#[repr(C)]
struct RiscvCpu {
    gpr: [u64; 32],     // x0-x31
    fpr: [u64; 32],     // f0-f31 (raw bits, NaN-boxed)
    pc: u64,
    guest_base: u64,
    load_res: u64,       // LR reservation address
    load_val: u64,       // LR loaded value
    fflags: u64,         // floating-point exception flags
    frm: u64,            // floating-point rounding mode
    ustatus: u64,        // user-mode status register
}
```

**Translation context** (`riscv/mod.rs`): `RiscvDisasContext` registers all 32 GPRs, 32 FPRs, PC, and floating-point CSRs as TCG global variables (backed by `RiscvCpu` fields), with the env pointer fixed to RBP.

**Instruction translation** (`riscv/trans/`): Implements the `Decode<Context>` trait's `trans_*` methods, covering RV64IMAFDC integer, floating-point, and compressed instruction sets, using QEMU-style `gen_xxx` helper function patterns:

```rust
type BinOp =
    fn(&mut Context, Type, TempIdx, TempIdx, TempIdx) -> TempIdx;

fn gen_arith(ir: &mut Context, a: &ArgsR, op: BinOp) -> bool;
fn gen_arith_imm(ir: &mut Context, a: &ArgsI, op: BinOp) -> bool;
fn gen_branch(
    ir: &mut Context, rs1: usize, rs2: usize,
    imm: i64, cond: Cond,
) -> bool;
```

Each `trans_*` method becomes a single-line call, e.g., `trans_add --> gen_arith(ir, a, Context::gen_add)`.

**Floating-point support**: RV64F/RV64D floating-point instructions call C ABI helper functions in `fpu.rs` via `gen_helper_call`, with caller-saved register save/restore handled by the backend's `regalloc_call`. Implements floating-point related user-mode CSRs (`fflags`, `frm`, `fcsr`) and U-mode status/trap CSRs, with FS state tracking (dirty flag set only on FPR writes).

---

## 8. Full-System Architecture

Machina's full-system emulation layer covers CPU management, the memory subsystem, device models, and machine definitions, enabling the JIT engine to run complete RISC-V privileged firmware and operating system kernels.

### 8.1 system/ -- CPU Management and WFI

#### CpuManager

`CpuManager` in `system/src/lib.rs` manages vCPU thread lifecycles:

- `running: Arc<AtomicBool>` -- global running flag; all vCPU threads poll this flag to decide whether to exit
- `stop()` -- clears the running flag and wakes all WFI-blocked CPUs
- Supports parallel multi-vCPU execution (multi-threaded vCPU model)

#### FullSystemCpu

`system/src/cpus.rs` defines `FullSystemCpu`, bridging `RiscvCpu` to the execution loop's `GuestCpu` trait:

```
+------------------+       +------------------+
| FullSystemCpu    |       | Exec Loop        |
|                  |       | (GuestCpu trait)  |
|  +------------+  |       |                  |
|  | RiscvCpu   |  | <---> | gen_code()       |
|  +------------+  |       | get_pc()         |
|  ram_ptr         |       | env_ptr()        |
|  shared_mip      |       | pending_interrupt|
|  wfi_waker       |       +------------------+
+------------------+
```

- `guest_base` is set to `ram_ptr - RAM_BASE`, so JIT-generated memory access instructions can compute host addresses directly via `guest_base + guest_addr`
- `SharedMip (Arc<AtomicU64>)` -- devices write to this atomic variable to deliver interrupts; the execution loop reads it in `pending_interrupt()` and syncs to the CPU's CSRs
- `WfiWaker` -- a `Condvar`-based wakeup primitive; the device IRQ sink calls `wake()` to wake WFI-blocked CPUs

#### WFI Mechanism (`core/src/wfi.rs`)

```
Device IRQ --> IrqSink::set_irq()
                  |
                  v
          SharedMip.fetch_or(bit)
                  |
                  v
          WfiWaker::wake()
                  |
                  v
          Condvar::notify_all()
                  |
                  v
          CPU exits WFI wait
                  |
                  v
          pending_interrupt() syncs mip
```

`WfiWaker` internally uses a single `Mutex` to protect the `irq_pending` and `stopped` flags. The three methods `wake()`, `stop()`, and `wait()` all acquire the same lock, thereby eliminating the lost-wakeup race condition.

### 8.2 memory/ -- Memory Subsystem

#### AddressSpace

`memory/src/address_space.rs` implements the top-level address space, consisting of a `MemoryRegion` tree and a cached `FlatView`:

```
AddressSpace
+-- root: MemoryRegion (Container "system")
|   +-- SubRegion @ 0x80000000: MemoryRegion (Ram)
|   +-- SubRegion @ 0x0C000000: MemoryRegion (Io, PLIC)
|   +-- SubRegion @ 0x02000000: MemoryRegion (Io, ACLINT)
|   +-- SubRegion @ 0x10000000: MemoryRegion (Io, UART)
+-- flat_view: RwLock<FlatView>  (cached dispatch table)
```

- `read(addr, size)` / `write(addr, size, val)` -- dispatches quickly via FlatView to RAM or MMIO callbacks
- `update_flat_view()` -- rebuilds the flat view after modifying the region tree

#### MemoryRegion

`memory/src/region.rs` defines memory region tree nodes:

| RegionType | Meaning | Typical Usage |
|------------|---------|---------------|
| `Ram` | Read-write memory backed by `Arc<RamBlock>` | Main memory |
| `Rom` | Read-only memory backed by `Arc<RamBlock>` | Firmware |
| `Io` | `Arc<Mutex<Box<dyn MmioOps>>>` | MMIO devices |
| `Alias` | Offset alias of another region | Address remapping |
| `Container` | Pure container, no backend storage | Address space root |

Each region has `priority`, `enabled`, and `subregions` attributes, supporting tree-structured nesting and priority overrides.

#### MmioOps trait

```rust
pub trait MmioOps: Send {
    fn read(&self, offset: u64, size: u32) -> u64;
    fn write(&self, offset: u64, size: u32, val: u64);
}
```

Device models implement this trait to handle MMIO reads and writes. Uses `&self` (shared reference) combined with interior mutability (`Mutex`), matching the memory tree's shared ownership model.

#### FlatView and RamBlock

- `FlatView` -- flattens the region tree into a sorted `FlatRange` array; address lookups use binary search O(log n)
- `RamBlock` -- contiguous physical memory backend allocated via `mmap`, providing `ptr()` for raw pointer access

### 8.3 hw/ -- Device Model Layer

#### hw/core -- Device Infrastructure

| Module | Responsibility | QEMU Reference |
|--------|---------------|----------------|
| `qdev.rs` | `Device` trait + `DeviceState` base | `hw/core/qdev.c` |
| `irq.rs` | `IrqSink` trait, `IrqLine`, `OrIrq`, `SplitIrq` | `hw/core/irq.c` |
| `chardev.rs` | `Chardev` trait, `CharFrontend`, `StdioChardev` | `chardev/char.c` |
| `fdt.rs` | `FdtBuilder` -- constructs DTB blobs | `hw/core/fdt_generic_util.c` |
| `loader.rs` | `load_binary()` binary loading | `hw/core/loader.c` |
| `bus.rs` | Bus model | `hw/core/bus.c` |
| `clock.rs` | Clock model | `hw/core/clock.c` |

**qdev device model**:

```rust
pub trait Device: Send + Sync {
    fn name(&self) -> &str;
    fn realize(&mut self) -> Result<(), String>;
    fn reset(&mut self);
    fn realized(&self) -> bool;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}
```

Each device implements the `Device` trait, completing initialization via `realize()` and restoring default state via `reset()`. `as_any()` supports downcasting.

**IRQ routing**:

```
Device --> IrqLine::raise()
               |
               v
         IrqSink::set_irq(irq, level)
               |
      +--------+--------+
      |                  |
      v                  v
   OrIrq            SplitIrq
   (N:1 OR)         (1:N fan-out)
      |                  |
      v                  v
  output IrqLine    output IrqLines
```

`IrqLine` connects a source and a sink as a single interrupt line. `OrIrq` implements a multi-input OR gate (e.g., PLIC's multi-source aggregation), and `SplitIrq` implements single-input multi-output fan-out.

**chardev character devices**:

The `Chardev` trait provides byte-level I/O backends (stdio, null, Unix socket). `CharFrontend` bridges a device frontend (e.g., UART) to the backend. `StdioChardev` forwards guest serial output to host standard I/O, supporting `-nographic` mode.

**FDT builder**:

`FdtBuilder` constructs a Flattened Devicetree blob (DTB) in memory, allowing bootloaders and kernels to discover hardware topology. The API style is nested `begin_node()` / `property_*()` / `end_node()` calls.

#### hw/intc -- Interrupt Controllers

**PLIC (Platform-Level Interrupt Controller)**:

`hw/intc/src/plic.rs` implements the SiFive PLIC specification, MMIO layout:

```
+----------+----------+-----------------------------------------+
| Offset   | Size     | Register                                |
+----------+----------+-----------------------------------------+
| 0x000000 | 4B * N   | priority[0..N] -- interrupt source priorities |
| 0x001000 | bitmap   | pending -- interrupt pending bitmap     |
| 0x002000 | 0x80*ctx | enable[ctx] -- per-context enable bitmap|
| 0x200000 | 0x1000*c | threshold (off 0), claim/complete (off 4)|
+----------+----------+-----------------------------------------+
```

- Supports 96 interrupt sources; number of contexts = 2 * number of harts (M-mode + S-mode)
- `context_outputs` are connected to each hart's MEI/SEI interrupt lines via `IrqLine`
- Level-triggered, supporting the claim/complete protocol

**ACLINT (Advanced Core Local Interruptor)**:

`hw/intc/src/aclint.rs` implements a CLINT-compatible timer and IPI interface:

```
+----------+----------+------------------------------+
| Offset   | Size     | Register                     |
+----------+----------+------------------------------+
| 0x0000   | 4B*hart  | msip[hart] -- software interrupt |
| 0x4000   | 8B*hart  | mtimecmp[hart] -- timer compare  |
| 0xBFF8   | 8B       | mtime -- global timer            |
+----------+----------+------------------------------+
```

- `tick()` method increments `mtime` and compares against `mtimecmp` to generate MTI (Machine Timer Interrupt)
- `mti_outputs` / `msi_outputs` are connected to harts via `IrqLine`

#### hw/char -- Character Devices

**UART 16550A**:

`hw/char/src/uart.rs` implements NS16550A serial port emulation:

- Register mapping: RBR/THR (0), IER (1), IIR/FCR (2), LCR (3), MCR (4), LSR (5), MSR (6), SCR (7)
- 16-byte FIFO receive buffer
- Supports DLAB (Divisor Latch Access)
- Connected to PLIC's UART_IRQ (10) via `IrqLine`
- Bridged to chardev backend via `CharFrontend`

### 8.4 hw/riscv -- RISC-V Machine Definitions

#### riscv64-ref Reference Machine

`hw/riscv/src/ref_machine.rs` defines a virt-compatible reference platform `RefMachine`, implementing the `Machine` trait:

**Memory map**:

```
+------------------+------------------+------------------+
| Address Range    | Size             | Device           |
+------------------+------------------+------------------+
| 0x0200_0000      | 64 KiB           | ACLINT (CLINT)   |
| 0x0C00_0000      | 64 MiB           | PLIC             |
| 0x1000_0000      | 256 B            | UART0 (16550A)   |
| 0x8000_0000      | configurable     | RAM              |
+------------------+------------------+------------------+
```

**Initialization flow** (`init()`):

1. Create `AddressSpace` (Container root region)
2. Allocate RAM (`MemoryRegion::ram()`), obtaining `Arc<RamBlock>`
3. Create PLIC, ACLINT, and UART device instances
4. Establish IRQ routing: UART --> PLIC source 10 --> hart MEI/SEI
5. Mount device MMIO regions into the address space
6. Generate FDT blob (`build_fdt()`), describing the complete hardware topology
7. `update_flat_view()` to rebuild the flat view

**IRQ routing diagram**:

```
UART -----> PLIC source 10
                 |
                 v
            PLIC context
            +-------+-------+
            |               |
            v               v
        MEI (hart 0)   SEI (hart 0)
            |               |
            v               v
    RiscvCpuIrqSink  RiscvCpuIrqSink
            |               |
            v               v
      SharedMip.fetch_or(bit)
            |
            v
      WfiWaker::wake()
```

`RiscvCpuIrqSink` implements the `IrqSink` trait, converting interrupts into `SharedMip` atomic bit-set operations and WFI wakeups.

#### boot.rs -- Boot Setup

`hw/riscv/src/boot.rs` handles loading bios/kernel and initializing the boot context:

- **BIOS loading**: binary image loaded to `RAM_BASE (0x8000_0000)`
- **Kernel loading**: loaded to `RAM_BASE + 0x20_0000` (2 MiB offset)
- **FDT placement**: aligned placement at the top of RAM
- **Boot convention** (aligned with OpenSBI / QEMU virt):
  - `a0 = hart_id`
  - `a1 = fdt_addr` (guest physical address)
  - `PC = entry_pc`
  - Privilege level = Machine mode

#### sbi.rs -- SBI Stub

`hw/riscv/src/sbi.rs` provides minimal SBI (Supervisor Binary Interface) ecall handling, serving as a fallback for `-bios none`, allowing S-mode software to probe basic SBI functionality:

- `SBI_EXT_BASE (0x10)` -- specification version (0.2), implementation ID, extension probing
- Other extensions return `SBI_ERR_NOT_SUPPORTED (-2)`
- `SbiResult { error, value }` corresponds to `a0`/`a1` return values

### 8.5 Sv39 MMU Integration

`guest/riscv/src/riscv/mmu.rs` implements the RISC-V Sv39 virtual memory management unit, integrated into the full-system emulation path.

**Sv39 page table structure**:

```
Virtual Address (39 bits):
+--------+--------+--------+---------------+
| VPN[2] | VPN[1] | VPN[0] | Page Offset   |
| 9 bits | 9 bits | 9 bits | 12 bits       |
+--------+--------+--------+---------------+

Page Table Walk (3 levels):
satp.PPN --> L2 PTE --> L1 PTE --> L0 PTE --> Physical Page
             |          |          |
             v          v          v
          1 GiB      2 MiB      4 KiB
         gigapage   megapage     page
```

**Core features**:

- **Three-level page table walk**: supports 4 KiB pages, 2 MiB megapages, and 1 GiB gigapages
- **256-entry direct-mapped TLB**: indexed by virtual page number hash, caching recent translation results
- **Permission checking**: validates access permissions based on current privilege level (U/S/M) and PTE flag bits (R/W/X/U)
- **mstatus flags**: MXR (Make eXecutable Readable) allows reading executable pages; SUM (Supervisor User Memory access) allows S-mode access to U pages
- **A/D bits**: checks Access and Dirty bits; produces a page fault when they are not set
- **PMP integration**: `pmp.rs` implements Physical Memory Protection, performing additional access checks after MMU translation
- **SATP register**: mode (Bare/Sv39), ASID (16-bit), PPN (44-bit) field parsing
- **TLB flush**: `sfence.vma` instruction triggers full or partial TLB flush by ASID/virtual address

---

## 9. Design Trade-off Summary

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Opcode polymorphism vs. splitting | Unified polymorphism | Reduces opcodes by 40%, simplifies the optimizer |
| Op.args fixed array vs. Vec | Fixed `[TempIdx; 10]` | Avoids heap allocation; hundreds of Ops per TB |
| RegSet bitmap vs. HashSet | `u64` bitmap | Register allocation hot path; bit operations are faster |
| Backend trait vs. conditional compilation | Trait | Testability, future multi-backend support |
| Constant deduplication | Per-type bucketed HashMap | Avoids duplicate Temps, saves memory |
| JumpCache heap allocation | `Box<[_; 4096]>` | 32KB is too large for the stack |
| TCG_AREG0 = RBP | Matches QEMU | Binary compatibility, easier reference verification |
| IRQ delivery via AtomicU64 | `SharedMip` | Avoids shared mutable references between devices and CPU |
| WFI via Condvar | `WfiWaker` | Correctly handles lost-wakeup race condition |
| MemoryRegion tree | QEMU-style region model | Supports priority overrides and dynamic reconfiguration |

---

## 10. QEMU Reference Mapping

| QEMU C Struct/Concept | Rust Equivalent | File |
|------------------------|-----------------|------|
| `TCGType` | `enum Type` | `core/src/types.rs` |
| `TCGTempVal` | `enum TempVal` | `core/src/types.rs` |
| `TCGCond` | `enum Cond` | `core/src/types.rs` |
| `MemOp` | `struct MemOp(u16)` | `core/src/types.rs` |
| `TCGRegSet` | `struct RegSet(u64)` | `core/src/types.rs` |
| `TCGOpcode` + DEF macros | `enum Opcode` | `core/src/opcode.rs` |
| `TCGOpDef` | `struct OpDef` + `OPCODE_DEFS` | `core/src/opcode.rs` |
| `TCG_OPF_*` | `struct OpFlags` | `core/src/opcode.rs` |
| `TCGTempKind` | `enum TempKind` | `core/src/temp.rs` |
| `TCGTemp` | `struct Temp` | `core/src/temp.rs` |
| `TCGLabel` | `struct Label` | `core/src/label.rs` |
| `TCGLifeData` | `struct LifeData(u32)` | `core/src/op.rs` |
| `TCGOp` | `struct Op` | `core/src/op.rs` |
| `TCGContext` | `struct Context` | `core/src/context.rs` |
| `TranslationBlock` | `struct TranslationBlock` | `core/src/tb.rs` |
| `CPUJumpCache` | `struct JumpCache` | `core/src/tb.rs` |
| `tcg_target_callee_save_regs` | `CALLEE_SAVED` | `accel/src/x86_64/regs.rs` |
| `tcg_out_tb_start` (prologue) | `HostCodeGen::emit_prologue` | `accel/src/x86_64/emitter.rs` |
| `tcg_code_gen_epilogue` | `HostCodeGen::emit_epilogue` | `accel/src/x86_64/emitter.rs` |
| `tcg_out_exit_tb` | `X86_64CodeGen::emit_exit_tb` | `accel/src/x86_64/emitter.rs` |
| `tcg_out_goto_tb` | `X86_64CodeGen::emit_goto_tb` | `accel/src/x86_64/emitter.rs` |
| `tcg_out_goto_ptr` | `X86_64CodeGen::emit_goto_ptr` | `accel/src/x86_64/emitter.rs` |
| `tcg_gen_op*` (IR emission) | `Context::gen_*` | `core/src/ir_builder.rs` |
| `liveness_pass_1` | `liveness_analysis()` | `accel/src/liveness.rs` |
| `tcg_optimize` | `optimize()` | `accel/src/optimize.rs` |
| `tcg_reg_alloc_op` | `regalloc_op()` | `accel/src/regalloc.rs` |
| `TCGArgConstraint` | `ArgConstraint` | `accel/src/constraint.rs` |
| `C_O*_I*` macros | `o1_i2()` / `o1_i2_alias()` etc. | `accel/src/constraint.rs` |
| `tcg_target_op_def` | `op_constraint()` | `accel/src/x86_64/constraints.rs` |
| `tcg_out_op` (dispatch) | `HostCodeGen::tcg_out_op` | `accel/src/x86_64/codegen.rs` |
| `tcg_out_mov` | `HostCodeGen::tcg_out_mov` | `accel/src/x86_64/codegen.rs` |
| `tcg_out_movi` | `HostCodeGen::tcg_out_movi` | `accel/src/x86_64/codegen.rs` |
| `tcg_out_ld` | `HostCodeGen::tcg_out_ld` | `accel/src/x86_64/codegen.rs` |
| `tcg_out_st` | `HostCodeGen::tcg_out_st` | `accel/src/x86_64/codegen.rs` |
| `tcg_gen_code` | `translate()` | `accel/src/translate.rs` |
| `translator_loop` | `translator_loop()` | `guest/riscv/src/lib.rs` |
| `DisasContextBase` | `DisasContextBase` | `guest/riscv/src/lib.rs` |
| `disas_log` (decodetree) | `decode::generate()` | `decode/src/lib.rs` |
| `target/riscv/translate.c` | `RiscvDisasContext` | `guest/riscv/src/riscv/mod.rs` |
| `trans_rvi.c.inc` (gen_xxx) | `gen_arith/gen_branch/...` | `guest/riscv/src/riscv/trans/` |
| `cpu_exec` | `cpu_exec_loop_mt()` | `accel/src/exec/exec_loop.rs` |
| `tb_lookup` | `tb_find()` | `accel/src/exec/exec_loop.rs` |
| `tb_gen_code` | `tb_gen_code()` | `accel/src/exec/exec_loop.rs` |
| `cpu_tb_exec` | `cpu_tb_exec()` | `accel/src/exec/exec_loop.rs` |
| `tb_add_jump` | `tb_add_jump()` | `accel/src/exec/exec_loop.rs` |
| `TBContext.htable` | `TbStore` | `accel/src/exec/tb_store.rs` |
| `hw/core/qdev.c` | `Device` trait | `hw/core/src/qdev.rs` |
| `hw/core/irq.c` | `IrqSink/IrqLine` | `hw/core/src/irq.rs` |
| `chardev/char.c` | `Chardev` trait | `hw/core/src/chardev.rs` |
| `hw/riscv/virt.c` | `RefMachine` | `hw/riscv/src/ref_machine.rs` |
| `hw/intc/sifive_plic.c` | `Plic` | `hw/intc/src/plic.rs` |
| `hw/intc/sifive_clint.c` | `Aclint` | `hw/intc/src/aclint.rs` |
| `hw/char/serial.c` | `Uart16550` | `hw/char/src/uart.rs` |
| `hw/core/loader.c` | `loader::load_binary()` | `hw/core/src/loader.rs` |
| `softmmu/memory.c` | `AddressSpace/MemoryRegion` | `memory/src/` |
| `target/riscv/cpu_helper.c` (MMU) | `Sv39Mmu` | `guest/riscv/src/riscv/mmu.rs` |
| `target/riscv/pmp.c` | `Pmp` | `guest/riscv/src/riscv/pmp.rs` |
| `cpus.c` (cpu_thread) | `CpuManager` | `system/src/lib.rs` |
| `Machine` (QOM) | `Machine` trait | `core/src/machine.rs` |
