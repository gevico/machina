<h1 align="center">Machina</h1>
<p align="center">
  English | <a href="README.zh.md">中文</a>
</p>

A modular RISC-V emulator written in Rust, featuring a JIT dynamic binary translation engine. Supports both **linux-user** mode (user-space ELF execution with syscall emulation) and **full-system** mode (hardware emulation with device models, interrupt controllers, and machine firmware).

> **Status**: Both emulation modes are operational. The JIT pipeline — RISC-V guest decode, TCG IR generation, optimization (constant folding, copy propagation, algebraic simplification), register allocation, and x86-64 code generation — is fully functional with MTTCG support and direct TB chaining. Linux-user mode runs real guest programs (dhrystone, printf, float). Full-system mode boots a RISC-V reference machine with PLIC, ACLINT, UART, Sv39 MMU, and SBI firmware interface.

## Architecture

```
                          ┌──────────────────────────────────────────────────────────┐
                          │                     JIT Pipeline                         │
┌──────────────┐    ┌─────┴─────┐    ┌──────────┐    ┌───────────┐    ┌─────────────┴──┐    ┌─────────┐
│ Guest Binary │───→│ Frontend  │───→│ IR Build │───→│ Optimizer │───→│ RegAlloc+Code  │───→│ Execute │
│ (RISC-V)     │    │ (decode + │    │ (gen_*)  │    │           │    │ gen (x86-64)   │    │ (JIT)   │
└──────────────┘    │  trans_*) │    └──────────┘    └───────────┘    └────────────────┘    └─────────┘
                    └───────────┘
                                              ┌───────────────────────────────┐
                                              │        Execution Modes        │
                                    ┌─────────┴─────────┐    ┌───────────────┴───────────┐
                                    │    linux-user      │    │       full-system          │
                                    │ ELF load + syscall │    │ Devices + MMU + IRQ + SBI  │
                                    └───────────────────┘    └───────────────────────────┘
```

## Workspace

| Crate | Path | Description |
|-------|------|-------------|
| **machina** | `src/` | CLI entry point (`machina -M riscv64-ref -bios fw.bin`) |
| **machina-core** | `core/` | IR definitions (opcodes, types, temps, ops, context, labels, TBs), CPU trait, address types |
| **machina-accel** | `accel/` | IR optimizer, liveness analysis, register allocator, x86-64 codegen, MTTCG execution engine |
| **machina-guest-riscv** | `guest/riscv/` | RISC-V frontend: RV64GC + privileged ISA (188 instructions), Sv39 MMU, TLB, PMP |
| **machina-decode** | `decode/` | QEMU-style `.decode` file parser and Rust decoder code generator |
| **machina-system** | `system/` | Full-system CPU bridge, CpuManager, WFI wakeup |
| **machina-memory** | `memory/` | AddressSpace, memory regions, MMIO dispatch, RAM blocks |
| **machina-hw-core** | `hw/core/` | Device infrastructure: qdev model, IRQ, chardev, clock, FDT, image loader |
| **machina-hw-intc** | `hw/intc/` | Interrupt controllers: PLIC, ACLINT (MTIMER + MSWI) |
| **machina-hw-char** | `hw/char/` | Character devices: UART 16550A |
| **machina-hw-riscv** | `hw/riscv/` | RISC-V reference machine (`riscv64-ref`), boot sequence, SBI stub |
| **machina-disas** | `disas/` | RISC-V instruction disassembler |
| **machina-monitor** | `monitor/` | Debug/monitor interface (WIP) |
| **machina-util** | `util/` | Shared utilities |
| **machina-tests** | `tests/` | 964 tests: unit, backend, frontend, difftest, integration, MTTCG, linux-user, machine |
| **machina-mtest** | `tests/mtest/` | Machine-level test framework |
| **machina-irdump** | `tools/irdump/` | IR dump tool for debugging |
| **machina-irbackend** | `tools/irbackend/` | IR backend inspection tool |

## Building

```bash
cargo build                  # Build all crates
cargo build --release        # Release build
cargo test --workspace       # Run all 964 tests
cargo clippy -- -D warnings  # Lint
cargo fmt --check            # Format check
```

## Running

```bash
# Linux-user mode: run a RISC-V ELF directly
cargo run --release --bin tcg-riscv64 -- target/guest/riscv64/dhrystone

# Full-system mode: boot a RISC-V reference machine
cargo run --release --bin machina -- -M riscv64-ref -m 128M -bios fw.bin -nographic

# Performance stats
TCG_STATS=1 target/release/tcg-riscv64 target/guest/riscv64/dhrystone
```

## Key Design Decisions

- **Unified type-polymorphic opcodes**: A single `Add` works on both I32 and I64 (type in `Op::op_type`), ~40% fewer opcodes than QEMU's split design.
- **Constraint-driven register allocation**: Declarative `ArgConstraint`/`OpConstraint` types — the allocator is fully generic, no per-opcode branches. New opcodes need only a constraint table entry.
- **Trait-based extensibility**: `HostCodeGen` for backends, `TranslatorOps` for frontends, `Cpu` for guest architectures — no conditional compilation.
- **Minimal `unsafe`**: Confined to JIT buffer (mmap/mprotect), generated code execution, and guest memory access. All IR manipulation is safe Rust.
- **QEMU-compatible device model**: qdev hierarchy, IRQ sinks, FDT generation, chardev abstraction — following proven QEMU hw/ patterns.

## What's Implemented

### JIT Engine (machina-accel)

- **IR Optimizer**: Constant folding, copy propagation, algebraic simplification, branch constant folding
- **Liveness Analysis**: Backward pass computing dead/sync flags
- **Register Allocator**: Constraint-driven greedy allocator mirroring QEMU's `tcg_reg_alloc_op()`
- **x86-64 Backend**: Full GPR instruction encoder (arithmetic, shifts, data movement, memory, mul/div, bit ops, branches, setcc/cmovcc), System V ABI prologue/epilogue, `goto_tb`/`exit_tb`/`goto_ptr`
- **Execution Engine**: MTTCG-capable loop, TB store (jump cache + global hash), direct TB chaining, `next_tb_hint`, `exit_target` atomic cache, MMIO helper dispatch

### RISC-V Frontend (machina-guest-riscv)

- **188 instructions**: RV64I (full), RV64M (mul/div/rem), RV64F/RV64D (float arithmetic, load/store, conversions, comparisons, FMA), RVC (compressed), privileged (CSR, ECALL, MRET/SRET, SFENCE.VMA, WFI)
- **Privileged ISA**: Sv39 MMU with TLB, Physical Memory Protection (PMP), M/S/U privilege levels
- **Decode generator**: QEMU-style `.decode` files compiled to Rust decoders at build time

### Full-System Emulation (machina-system + hw/*)

- **Reference Machine** (`riscv64-ref`): Integrated board with CPU, RAM, PLIC, ACLINT, UART, FDT
- **Interrupt Controllers**: PLIC (external interrupts, priority/threshold), ACLINT (MTIMER + MSWI)
- **Character Devices**: UART 16550A with chardev backend, stdio support for `-nographic`
- **Memory Subsystem**: Hierarchical memory regions, AddressSpace with flat views, MMIO dispatch
- **CPU Management**: CpuManager with WFI condvar wakeup, IRQ delivery to `mip`
- **Boot**: Firmware/kernel loading, FDT generation with device phandles, SBI stub
- **Device Infrastructure**: qdev model, IRQ sinks, clock, FDT builder, image loader

### Linux-User Emulation

- **ELF Loader**: RISC-V ELF loading with guest `argv` propagation and auxv layout
- **Guest Address Space**: mmap/brk handling for user-mode memory management
- **Syscall Emulation**: Core Linux syscalls for user-mode workloads
- **Runner**: `tcg-riscv64 <elf> [args...]`

### Testing (964 tests)

- **Unit**: Core data structures, IR APIs, backend instruction encoding
- **Frontend**: 91 RISC-V instruction tests through full decode → IR → codegen → execute pipeline
- **Difftest**: Differential testing against QEMU (qemu-riscv64) with edge-case values
- **Integration**: End-to-end pipeline — ALU, branches, loops, memory, complex sequences
- **MTTCG**: Concurrent lookup/translation/chaining (26 tests)
- **Linux-user**: Guest programs — hello, hello_printf, hello_float, dhrystone, argv_echo
- **Machine**: Full-system boot and device tests

## Performance

In linux-user mode, machina achieves ~30% faster execution than QEMU TCG on benchmarks like dhrystone, thanks to:

- **`next_tb_hint`**: Skips TB lookup entirely on chained exits (hot loop hit rate near 100%)
- **`exit_target` atomic cache**: Single-entry cache for indirect jumps, avoiding hash lookups
- **Simplified codegen**: Constraint-driven design eliminates register shuffling overhead

See [Performance Analysis](docs/performance.md) for details.

## QEMU Reference

This project references the QEMU source tree for architectural guidance:

- **TCG core**: `tcg/tcg.c`, `tcg/tcg-op.c`, `tcg/optimize.c`, `include/tcg/tcg.h`, `include/tcg/tcg-opc.h`
- **x86-64 backend**: `tcg/i386/tcg-target.c.inc`
- **RISC-V frontend**: `target/riscv/translate.c`, `target/riscv/insn_trans/`
- **Execution**: `accel/tcg/cpu-exec.c`, `accel/tcg/tb-maint.c`, `accel/tcg/translator.c`
- **Hardware**: `hw/riscv/`, `hw/intc/`, `hw/char/`, `hw/core/`
- **Documentation**: `docs/devel/tcg.rst`, `multi-thread-tcg.rst`, `decodetree.rst`

## Documentation

- [Design Document](docs/design.md) — Architecture, data structures, translation pipeline
- [IR Ops](docs/ir-ops.md) — Opcode catalog, Op structure, IR builder API
- [x86-64 Backend](docs/x86_64-backend.md) — Instruction encoder, constraint table, codegen
- [Performance](docs/performance.md) — Optimization techniques and QEMU comparison
- [Testing](docs/testing.md) — Test architecture, difftest framework, guest programs
- [Coding Style](docs/coding-style.md) — Naming conventions, formatting rules

## License

[MIT](LICENSE)
