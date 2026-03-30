# Machina Performance Analysis

This document summarizes machina JIT engine's unique performance optimizations compared to QEMU TCG, and analyzes performance characteristics in full-system mode.

## 1. Execution Loop Optimizations

### 1.1 `next_tb_hint` -- Skipping TB Lookup

**File**: `accel/src/exec/exec_loop.rs:52-89`

When a TB exits via `goto_tb` chaining, machina stores the target TB index in `next_tb_hint`. The next iteration directly reuses this index, completely skipping the jump cache and global hash lookup.

| | machina | QEMU |
|---|--------|------|
| After chained exit | Directly reuse target TB | Still goes through the full `tb_lookup` path |
| Hot loop overhead | Near zero (index comparison) | jump cache hash + comparison |

QEMU's `last_tb` is only used to decide whether to patch a link, not to skip lookup. In tight loops (e.g., the dhrystone main loop), the hint hit rate is extremely high.

### 1.2 `exit_target` Atomic Cache -- Indirect Jump Acceleration

**File**: `accel/src/exec/exec_loop.rs:96-116`, `core/src/tb.rs:55`

For `TB_EXIT_NOCHAIN` (indirect jumps, `jalr`, etc.), each TB maintains an `AtomicUsize` single-entry cache that records the last jump target TB.

```
indirect jump exit --> check exit_target cache
                       |-- hit and valid --> reuse directly, skip hash lookup
                       +-- miss --> normal tb_find, update cache
```

QEMU performs a full QHT lookup for all `TB_EXIT_NOCHAIN` exits, without this caching layer. Combined, these two optimizations ensure that global hash lookups are triggered almost exclusively during cold start and TB invalidation in steady-state execution.

**Estimated contribution**: ~8-10%

## 2. Guest Memory Access Optimizations

### 2.1 Direct guest_base Addressing (Early linux-user Optimization)

**File**: `accel/src/x86_64/codegen.rs:573-639`

> **Note**: The software-TLB-free direct addressing optimization described in this section was an early linux-user mode exclusive approach. Full-system mode uses Sv39 MMU page table translation + software TLB and no longer uses this path.

In the early linux-user mode, guest memory accesses directly generated `[R14 + addr]` addressing (R14 = guest_base), with no TLB lookup and no slow-path helper calls.

| | machina (direct addressing) | QEMU |
|---|--------|------|
| load/store generation | `mov reg, [R14+addr]` | Inline TLB fast path + slow path branch |
| Host instructions per access | 1-2 | 5-10 (TLB lookup + comparison + branch) |
| Slow path | None | Helper function call |

QEMU generates the full software TLB path even in linux-user mode, because its `tcg_out_qemu_ld`/`tcg_out_qemu_st` do not differentiate between system mode and user mode.

In full-system mode, machina uses Sv39 MMU page table translation with a software TLB fast path, where memory access overhead is comparable to QEMU and no longer benefits from direct addressing.

**Estimated contribution**: Only applicable to direct addressing scenarios, ~8-10%

## 3. Data Structure Optimizations

### 3.1 Vec-based IR Storage vs QEMU Linked List

**File**: `core/src/context.rs:18-73`

| | machina | QEMU |
|---|--------|------|
| Op storage | `Vec<Op>` contiguous memory | `QTAILQ` doubly linked list |
| Temp storage | `Vec<Temp>` contiguous memory | Array (fixed upper limit) |
| Traversal pattern | Sequential indexing, cache prefetch friendly | Pointer chasing, frequent cache misses |
| Pre-allocation | ops=512, temps=256, labels=32 | Dynamic malloc |

The optimizer traversal, liveness analysis, and register allocation all require sequential scanning of all ops, where Vec's cache line prefetch advantage is significant. Pre-allocated capacity avoids reallocation during translation.

### 3.2 HashMap Constant Deduplication vs Linear Scan

**File**: `core/src/context.rs:128-138`

machina uses a type-bucketed `HashMap<u64, TempIdx>` for constant deduplication with O(1) lookup. QEMU's `tcg_constant_internal` performs a linear scan over `nb_temps`, making constant lookup a hidden cost in large TBs.

### 3.3 `#[repr(u8)]` Compact Enums

**File**: `core/src/opcode.rs`

The `Opcode` enum is annotated with `#[repr(u8)]`, occupying 1 byte. QEMU's `TCGOpcode` is an `int` (4 bytes). The `Op` struct is more compact, fitting more ops per cache line.

**Estimated contribution**: ~3-5%

## 4. Runtime Concurrency Optimizations

### 4.1 Lock-free TB Reads

**File**: `accel/src/exec/tb_store.rs:13-64`

TbStore leverages the append-only, never-delete property of TBs, using `UnsafeCell<Vec<TB>>` + `AtomicUsize` length to implement lock-free reads.

```
Write path (translation): translate_lock --> push TB --> Release store len
Read path (execution):    Acquire load len --> index access (lock-free)
```

QEMU's QHT uses an RCU mechanism, incurring additional grace period and synchronize overhead. machina's approach is simpler, exploiting the append-only invariant of TBs.

### 4.2 RWX Code Buffer -- No mprotect Switching

**File**: `accel/src/code_buffer.rs:38-49`

machina directly mmaps RWX memory, requiring no mprotect switching during TB link patching. QEMU, when split-wx mode is enabled (the default on some distributions), needs an mprotect system call for each patch.

### 4.3 Simplified Hash Function

**File**: `core/src/tb.rs:106-109`

```rust
let h = pc.wrapping_mul(0x9e3779b97f4a7c15) ^ (flags as u64);
(h as usize) & (TB_HASH_SIZE - 1)
```

Golden ratio constant multiplication hash, with less computation than QEMU's xxHash. Saving a few cycles per lookup on the TB lookup hot path, the cumulative effect is considerable.

**Estimated contribution**: ~2-3%

## 5. Compilation Pipeline Optimizations

### 5.1 Single-Pass IR Optimizer

**File**: `accel/src/optimize.rs`

| | machina | QEMU |
|---|--------|------|
| Passes | Single pass O(n) | Multiple pass scans |
| Constant folding | Full value-level | Bit-level (z_mask/o_mask/s_mask) |
| Copy propagation | Basic | Advanced |
| Algebraic simplification | Basic identities | Complex pattern matching |

machina's optimization depth is less than QEMU's, but translation speed is faster. For large numbers of short TBs, the single-pass design's compilation time advantage is significant.

### 5.2 Rust Zero-Cost Abstractions

- **Monomorphization**: Frontend `BinOp` function pointers (`frontend/src/riscv/trans.rs:26`) are monomorphized and inlined by the compiler, eliminating indirect calls
- **Inline annotations**: `CodeBuffer`'s 14 `#[inline]` byte emission functions (`accel/src/code_buffer.rs`) are inlined at codegen call sites
- **Enum discriminants**: `#[repr(u8)]` generates compact jump tables

**Estimated contribution**: ~2-3%

## 6. Instruction Selection Optimizations

### 6.1 LEA Three-Address Addition

**File**: `accel/src/x86_64/codegen.rs:136-147`

When the output register of `Add` differs from both inputs, LEA is used for non-destructive three-address addition, avoiding an extra MOV. QEMU also has this optimization.

### 6.2 Unconditional BMI1 Instructions

**File**: `accel/src/x86_64/emitter.rs:57-61`

machina unconditionally uses ANDN/LZCNT/TZCNT/POPCNT. QEMU checks CPU features at runtime before deciding whether to use them; the detection itself has minor overhead, and the fallback paths are longer.

### 6.3 MOV Immediate Tiered Optimization

**File**: `accel/src/x86_64/emitter.rs:547-566`

```
val == 0        --> XOR reg, reg          (2 bytes, breaks dependency chain)
val <= u32::MAX --> MOV r32, imm32        (5 bytes, zero-extends)
val fits i32    --> MOV r64, sign-ext imm (7 bytes)
otherwise       --> MOV r64, imm64        (10 bytes)
```

## 7. Full-System Mode Performance Characteristics

Full-system mode introduces additional performance overhead. The main contributing factors are:

### 7.1 MMU Page Table Translation Overhead

Full-system mode uses Sv39 three-level page table translation. Each guest memory access requires:

1. Software TLB fast path lookup (inline code, ~5-10 host instructions)
2. Page table walk on TLB miss (3-level lookup, one memory read per level)
3. Permission checks (read/write/execute, U/S mode, MXR/SUM bits)

TLB hit rate is the key performance metric for full-system mode. During steady-state execution, TLB hit rate is typically >95%, amortizing the page table walk overhead.

### 7.2 MMIO Dispatch Overhead

Device MMIO accesses take a separate dispatch path, bypassing the TLB fast path:

```
guest load/store --> TLB lookup
                      |-- normal memory --> fast path direct access
                      +-- MMIO region --> AddressSpace dispatch
                                          --> device read/write callback
```

MMIO dispatch involves address space tree lookup and indirect device callback calls, with overhead 1-2 orders of magnitude higher than normal memory access. Device-interaction-intensive workloads (e.g., heavy serial I/O) are significantly affected.

### 7.3 Privilege Level Switching

Full-system mode must handle M/S/U privilege level switching, interrupts, and exceptions, with each switch involving CSR updates and TB invalidation. Frequent privilege level switches (e.g., high-frequency timer interrupts) reduce TB cache hit rates.

## 8. Performance Contribution Overview

| Optimization Category | Estimated Contribution | Key Technique |
|-----------------------|----------------------|---------------|
| Execution loop (hint + exit_target) | ~8-10% | Skipping TB lookup |
| Data structures (Vec + compact enums) | ~3-5% | Cache-friendly layout |
| Runtime concurrency (lock-free + RWX) | ~2-3% | Lock-free reads, no mprotect |
| Compilation pipeline (single-pass + inlining) | ~2-3% | Rust zero-cost abstractions |
| Hash + constant deduplication | ~1-2% | Simplified computation |

> Note: Direct guest_base addressing (~8-10%) is only applicable to the early linux-user mode and does not apply to full-system mode.

## 9. Trade-offs and Limitations

machina's performance advantages are built on the following trade-offs:

- **RWX memory**: Violates the W^X security principle; forbidden on some platforms (iOS)
- **Simplified optimizer**: Lacks QEMU's bit-level tracking, resulting in slightly lower generated code quality
- **Unconditional BMI1**: Assumes host CPU support; incompatible with older CPUs
- **Simplified hash**: Distribution quality inferior to xxHash; degrades under high collision rates
- **Full-system MMU overhead**: Sv39 page table translation introduces additional memory access latency; TLB miss penalty is high
- **MMIO dispatch**: Device access goes through indirect callback paths with non-negligible latency

These trade-offs are reasonable for the target scenario of full-system RISC-V emulation on modern x86-64 hosts.
