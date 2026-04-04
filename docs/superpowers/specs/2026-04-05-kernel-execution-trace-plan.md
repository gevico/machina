# Kernel Execution Trace Implementation Plan

## Goal Description

Extend Machina's Monitor protocol to support dynamic execution tracing
of tg-rcore-tutorial teaching kernels (ch2-ch4).  Students activate
tracing via CLI flag or HMP command, and the emulator emits structured
events (trap entry/exit, syscall, task switch, timer interrupt,
address-space switch, TLB flush) to the terminal with ELF symbol
resolution.  The design follows a B3 hybrid approach: lightweight
hooks in the frontend translation layer and exec loop, collected by
a ring-buffer TraceCollector, and output through an extensible
TraceSink abstraction.

## Acceptance Criteria

Following TDD philosophy, each criterion includes positive and negative
tests for deterministic verification.

- AC-1: TraceEvent and EventFilter data types
  - Positive Tests (expected to PASS):
    - Construct each TraceEvent variant and verify all fields
    - EventFilter with category "trap" matches TrapEnter, TrapExit,
      Syscall, TimerInterrupt events
    - EventFilter with no categories matches all events
  - Negative Tests (expected to FAIL):
    - EventFilter with category "vm" rejects TrapEnter events
    - EventFilter with category "trap" rejects AddressSpaceSwitch
      events
  - AC-1.1: TraceSink trait is object-safe
    - Positive: TerminalSink implements TraceSink and compiles as
      `dyn TraceSink`
    - Negative: A struct missing `emit()` does not satisfy TraceSink

- AC-2: ELF SymbolTable parsing and lookup
  - Positive Tests (expected to PASS):
    - Parse a minimal ELF with two STT_FUNC symbols; lookup returns
      correct function name for PCs within each symbol range
    - Lookup at the exact start_pc of a symbol returns its name
    - Lookup returns None for PC outside all symbol ranges
  - Negative Tests (expected to FAIL):
    - Lookup on an ELF with no .symtab section returns None for any
      PC
    - Lookup with an empty SymbolTable returns None

- AC-3: TraceCollector with ring buffer
  - Positive Tests (expected to PASS):
    - Push 100 events into an 8192-capacity buffer; all 100 are
      retrievable
    - Push 9000 events; only the latest 8192 are retained (oldest
      overwritten)
    - Monotonic sequence counter increments for each event
  - Negative Tests (expected to FAIL):
    - Reading from a buffer that has never been written returns no
      events
    - Events pushed while `enabled` is false are not recorded

- AC-4: TerminalSink coloured output format
  - Positive Tests (expected to PASS):
    - Emit TrapEnter event; output matches expected format
      `[#NNNN] [trap] ...`
    - Emit Syscall event; output includes syscall id and args
    - Four event categories produce four distinct ANSI colour codes
  - Negative Tests (expected to FAIL):
    - Output for TrapEnter does not match TaskSwitch format
    - Raw PC without symbol resolution does not crash (falls back to
      hex address)

- AC-5: Frontend trace helpers for ecall/sret/mret/sfence
  - Positive Tests (expected to PASS):
    - Translate an ecall instruction with tracing enabled; verify
      generated IR contains a Call opcode targeting the trace helper
    - Translate an sret instruction with tracing enabled; verify
      generated IR contains a Call opcode targeting the trace helper
    - Translate ecall with tracing disabled; verify NO extra Call
      opcode is emitted
  - Negative Tests (expected to FAIL):
    - Helper function receives wrong CSR values (verifies that
      helpers read correct CPU state)
    - Translate a non-privileged instruction (e.g., add); verify no
      trace helper is emitted

- AC-6: Exec loop hooks for CSR diff and PC match
  - Positive Tests (expected to PASS):
    - After a TB that changes satp, an AddressSpaceSwitch event is
      collected
    - After a TB that returns with PC inside `__switch` symbol range,
      a TaskSwitch event is collected
    - When tracing is disabled, exec loop incurs no measurable CSR
      snapshot overhead
  - Negative Tests (expected to FAIL):
    - No AddressSpaceSwitch event when satp is unchanged
    - No TaskSwitch event when PC is outside `__switch` range

- AC-7: HMP trace commands
  - Positive Tests (expected to PASS):
    - `trace start` activates tracing; subsequent events appear in
      output
    - `trace start trap,sched` activates with filter; only trap and
      sched events appear
    - `trace stop` deactivates; no further events appear
    - `trace status` reports enabled state and event count
  - Negative Tests (expected to FAIL):
    - `trace start` with invalid filter category returns error
      message
    - `trace stop` when already stopped returns informative message

- AC-8: MMP trace commands
  - Positive Tests (expected to PASS):
    - `{"execute":"trace-start","arguments":{"filter":["trap"]}}`
      returns `{"return":{}}`
    - `{"execute":"trace-stop"}` returns
      `{"return":{"events_collected":N}}`
    - `{"execute":"trace-status"}` returns correct enabled and count
      fields
  - Negative Tests (expected to FAIL):
    - `trace-start` with unknown filter returns error response

- AC-9: CLI `-trace` option
  - Positive Tests (expected to PASS):
    - `machina -kernel ... -trace` starts with tracing fully enabled
    - `machina -kernel ... -trace trap,sched` starts with filtered
      tracing
  - Negative Tests (expected to FAIL):
    - `machina -kernel ... -trace bogus` reports unknown category
      and exits

- AC-10: Integration with tg-rcore-ch2 ELF
  - Positive Tests (expected to PASS):
    - Boot tg-rcore-ch2 with `-trace`; TrapEnter and TrapExit events
      are collected
    - Syscall events for write (id=64) and exit (id=93) appear
    - Events include resolved function names from the ELF symbol
      table
  - Negative Tests (expected to FAIL):
    - No TaskSwitch events in ch2 (ch2 has no scheduler)

- AC-11: Integration with tg-rcore-ch3 ELF
  - Positive Tests (expected to PASS):
    - TimerInterrupt events are collected during ch3 execution
    - TaskSwitch events appear when the round-robin scheduler runs
  - Negative Tests (expected to FAIL):
    - No AddressSpaceSwitch events in ch3 (ch3 has no virtual
      memory)

- AC-12: Integration with tg-rcore-ch4 ELF
  - Positive Tests (expected to PASS):
    - AddressSpaceSwitch events appear when processes get separate
      page tables
    - TlbFlush events appear when sfence.vma is executed
  - Negative Tests (expected to FAIL):
    - No AddressSpaceSwitch event when kernel runs with a single
      address space

- AC-13: Zero-overhead when tracing disabled
  - Positive Tests (expected to PASS):
    - Benchmark: execution time without `-trace` is within 1% of
      baseline (no tracing code compiled in at all)
  - Negative Tests (expected to FAIL):
    - N/A (performance criteria are measured, not pass/fail code)

## Path Boundaries

### Upper Bound (Maximum Acceptable Scope)

The implementation includes all seven event types for ch2-ch4, full
ELF symbol table parsing, coloured terminal output, HMP and MMP
commands, CLI `-trace` option, unit tests for every component, and
integration tests against tg-rcore ch2, ch3, ch4 ELF binaries.
Performance benchmarks verify the zero-overhead-disabled property.
The TraceSink abstraction is in place with TerminalSink implemented
and JsonStreamSink interface reserved.

### Lower Bound (Minimum Acceptable Scope)

The implementation includes TrapEnter, TrapExit, and Syscall events
(ch2), terminal output via HMP only, CLI `-trace` flag, basic symbol
table parsing, and unit tests.  MMP commands and integration tests
against real ELF binaries may be minimal.

### Allowed Choices

- Can use: Arc for shared TraceCollector, AtomicBool for fast-path
  enabled check, existing helper call mechanism (extern "C" + IR
  Call opcode), ANSI escape codes for terminal colours
- Can use: `object` crate or hand-rolled ELF section parser for
  .symtab
- Cannot use: global mutable statics for TraceCollector (must use
  Arc)
- Cannot use: conditional compilation (cfg) for tracing (must be
  runtime toggle)

> **Note on Deterministic Designs**: The design spec prescribes
> specific event types, hook locations, and output format.  Path
> boundaries reflect this narrow constraint.  Implementation choices
> are limited to internal data structures and helper call plumbing.

## Feasibility Hints and Suggestions

> **Note**: This section is for reference and understanding only.
> These are conceptual suggestions, not prescriptive requirements.

### Conceptual Approach

**Phase 1 -- Foundation (core/)**

Create `core/src/trace.rs` with TraceEvent enum (7 variants),
TraceSink trait (one method: `emit(&mut self, event: &TraceEvent)`),
and EventFilter (bitfield with 4 category bits: trap/sched/vm/syscall).
Add `pub mod trace;` to `core/src/lib.rs`.

**Phase 2 -- Collection infrastructure (monitor/)**

Create `monitor/src/symbol_table.rs`: parse ELF .symtab sections,
build sorted Vec of (start_pc, end_pc, name) tuples, binary search
for lookup(pc) -> Option<&str>.  The existing loader in
`hw/core/src/loader.rs` only parses PT_LOAD segments and returns
LoadInfo; symbol parsing needs to read the raw ELF bytes before
segments are loaded.

Create `monitor/src/trace_collector.rs`: RingBuffer<TraceEvent> with
8192 capacity, AtomicBool enabled flag, EventFilter, Arc<SymbolTable>,
AtomicU64 sequence counter.  Methods: `push(event)`, `drain()`,
`set_filter()`, `enable()/disable()`.

Create `monitor/src/terminal_sink.rs`: implement TraceSink, format
each event as a coloured line with sequence number and optional
symbol name.

**Phase 3 -- Frontend hooks (guest/riscv/)**

Create `guest/riscv/src/riscv/trace_helpers.rs` with extern "C"
helper functions:
- `helper_trace_ecall(env: *mut RiscvCpu, ...)` reads scause, sepc,
  priv_level from the CPU, constructs TrapEnter + Syscall events
- `helper_trace_sret(env: *mut RiscvCpu, ...)` constructs TrapExit
- `helper_trace_mret(env: *mut RiscvCpu, ...)` constructs TrapExit
- `helper_trace_sfence(env: *mut RiscvCpu, ...)` constructs TlbFlush

Each helper checks `TraceCollector::enabled()` first (fast exit).
The helpers need access to a TraceCollector reference.  Since
helpers receive the CPU pointer (env), store an
`Option<Arc<TraceCollector>>` inside RiscvCpu or pass it through
the existing env pointer mechanism.

Modify `guest/riscv/src/riscv/trans/mod.rs`: in the translation
branches for SYSTEM instructions (ecall, sret, mret) and sfence.vma,
conditionally emit a helper call using the existing `gen_helper_call`
pattern from `helpers.rs`.

**Phase 4 -- Exec loop hooks (accel/)**

Modify `accel/src/exec/exec_loop.rs`: after each TB execution in
`cpu_exec_loop()`, when tracing is enabled:
- Compare current priv_level and satp with previous snapshot
- If satp changed: emit AddressSpaceSwitch event
- If PC falls within `__switch` symbol range: emit TaskSwitch event
- Update snapshot

The exec loop already has access to the CPU object through the
GuestCpu trait.  Add CSR read methods to GuestCpu or use downcast
to RiscvCpu for direct CSR access.

**Phase 5 -- Monitor + CLI integration**

Extend `monitor/src/hmp.rs` with trace/stop/status command handlers.
Extend `monitor/src/mmp.rs` with trace-start/trace-stop/trace-status
JSON command handlers.  Wire TraceCollector into MonitorService.

Modify `src/main.rs`: add `-trace` and `-trace <filter>` CLI option
parsing.  When `-trace` is specified, enable TraceCollector before
the exec loop starts.  Parse ELF symbols at kernel load time.

### Relevant References

- `accel/src/ir/ir_builder.rs` -- `gen_call()` for helper call IR
  emission
- `guest/riscv/src/riscv/trans/helpers.rs` -- existing helper call
  patterns (`gen_csr_helper`, `gen_helper_call`)
- `guest/riscv/src/riscv/cpu.rs` -- RiscvCpu struct with CSR fields
  and `csr_read()`/`csr_write()` methods
- `accel/src/x86_64/constraints.rs` -- Call opcode register
  constraints (RDI=env, RSI-R9=args)
- `accel/src/exec/exec_loop.rs` -- `cpu_exec_loop()` main execution
  loop
- `hw/core/src/loader.rs` -- ELF loading, extend for symbol parsing
- `monitor/src/hmp.rs` -- HMP command dispatch pattern
- `monitor/src/mmp.rs` -- MMP command dispatch pattern
- `monitor/src/service.rs` -- MonitorService bridge
- `core/src/monitor.rs` -- MonitorState and CpuSnapshot

## Dependencies and Sequence

### Milestones

1. Foundation: Core data types and symbol table
   - Phase A: TraceEvent enum, TraceSink trait, EventFilter in
     `core/src/trace.rs`
   - Phase B: SymbolTable in `monitor/src/symbol_table.rs` with ELF
     .symtab parser
   - Phase C: TraceCollector ring buffer in
     `monitor/src/trace_collector.rs`
   - Phase D: TerminalSink in `monitor/src/terminal_sink.rs`
   - Phase B and C depend on Phase A; Phase D depends on Phase A.

2. Instrumentation: Frontend and exec loop hooks
   - Phase E: Trace helper functions in
     `guest/riscv/src/riscv/trace_helpers.rs`
   - Phase F: Translation branch modifications in
     `guest/riscv/src/riscv/trans/mod.rs`
   - Phase G: Exec loop CSR diff + PC match in
     `accel/src/exec/exec_loop.rs`
   - Phase E depends on Phase A; Phase F depends on Phase E;
     Phase G depends on Phase A and Phase B.

3. Integration: Monitor commands and CLI
   - Phase H: HMP trace/stop/status commands in `monitor/src/hmp.rs`
   - Phase I: MMP trace-start/trace-stop/trace-status in
     `monitor/src/mmp.rs`
   - Phase J: CLI `-trace` option in `src/main.rs`
   - Phase H and I depend on Phase C and Phase D; Phase J depends
     on all prior phases.

4. Verification: Tests and benchmarks
   - Phase K: Unit tests for SymbolTable, TraceCollector,
     EventFilter, TerminalSink
   - Phase L: Integration tests with tg-rcore ch2/ch3/ch4 ELF
     binaries
   - Phase M: Performance benchmarks (disabled overhead)
   - Phase K depends on Milestones 1 and 2; Phase L depends on
     Milestone 3; Phase M depends on Milestone 3.

## Task Breakdown

| Task ID | Description | Target AC | Tag | Depends On |
|---------|-------------|-----------|-----|------------|
| task1 | Create core/src/trace.rs with TraceEvent enum (7 variants), TraceSink trait, EventFilter bitfield | AC-1 | coding | - |
| task2 | Create monitor/src/symbol_table.rs: parse ELF .symtab, sorted intervals, binary search lookup | AC-2 | coding | task1 |
| task3 | Create monitor/src/trace_collector.rs: ring buffer (8192), AtomicBool enabled, EventFilter, seq counter | AC-3 | coding | task1, task2 |
| task4 | Create monitor/src/terminal_sink.rs: implement TraceSink with coloured output format | AC-4 | coding | task1 |
| task5 | Create guest/riscv trace helpers: extern "C" functions for ecall/sret/mret/sfence event construction | AC-5 | coding | task1, task3 |
| task6 | Modify guest/riscv trans/mod.rs: emit trace helper calls in ecall/sret/mret/sfence translation branches | AC-5 | coding | task5 |
| task7 | Modify accel exec_loop.rs: add CSR snapshot diff and PC range match after TB execution | AC-6 | coding | task1, task2, task3 |
| task8 | Extend monitor/src/hmp.rs: add trace start/stop/status command handlers | AC-7 | coding | task3, task4 |
| task9 | Extend monitor/src/mmp.rs: add trace-start/trace-stop/trace-status JSON command handlers | AC-8 | coding | task3 |
| task10 | Modify src/main.rs: add -trace CLI option, wire SymbolTable loading at kernel boot | AC-9 | coding | task2, task3, task8 |
| task11 | Unit tests for SymbolTable, TraceCollector, EventFilter, TerminalSink | AC-2, AC-3, AC-4 | coding | task2, task3, task4 |
| task12 | Integration test: boot tg-rcore-ch2 ELF with tracing, verify trap/syscall events | AC-10 | coding | task10 |
| task13 | Integration test: boot tg-rcore-ch3 ELF with tracing, verify timer/task switch events | AC-11 | coding | task10 |
| task14 | Integration test: boot tg-rcore-ch4 ELF with tracing, verify VM events | AC-12 | coding | task10 |
| task15 | Performance benchmark: verify zero overhead when tracing disabled | AC-13 | coding | task10 |

## Claude-Codex Deliberation

### Agreements

(Claude-only analysis; Codex CLI was unavailable for cross-review.)

- TraceEvent data model with 7 variants covers ch2-ch4 teaching
  concepts completely.
- EventFilter bitfield approach avoids unnecessary event construction
  overhead.
- TraceSink trait provides clean terminal/Web separation.
- Ring buffer with 8192 capacity is adequate for teaching scenarios.
- ELF symbol parsing should be a separate pass after existing
  segment loading, not modifying the existing loader.

### Resolved Disagreements

- N/A (Claude-only analysis, no Codex cross-review)

### Convergence Status

- Final Status: `partially_converged` (Codex CLI unavailable;
  Claude-only analysis with user-validated design)

## Pending User Decisions

- DEC-1: TraceCollector sharing strategy for multi-vCPU
  - Claude Position: Use a single global Arc<TraceCollector> shared
    across all vCPU threads; teaching scenarios typically use 1-2
    vCPUs and the simplicity outweighs lock contention.
  - Codex Position: N/A (Codex unavailable)
  - Tradeoff Summary: Global collector is simpler but may need
    per-vCPU buffers if contention appears.  Start simple, optimize
    later.
  - Decision Status: `PENDING`

- DEC-2: ELF symbol parser implementation
  - Claude Position: Hand-rolled parser for .symtab/.strtab
    sections, since the `object` crate is a heavy dependency and
    tg-rcore ELFs have simple symbol tables.
  - Codex Position: N/A (Codex unavailable)
  - Tradeoff Summary: Hand-rolled is fewer dependencies but more
    code to maintain.  The `object` crate is battle-tested for edge
    cases.
  - Decision Status: `PENDING`

## Implementation Notes

### Code Style Requirements

- Implementation code and comments must NOT contain plan-specific
  terminology such as "AC-", "Milestone", "Step", "Phase", or
  similar workflow markers.
- These terms are for plan documentation only, not for the resulting
  codebase.
- Use descriptive, domain-appropriate naming in code instead (e.g.,
  `trace_event`, `ring_buffer`, `symbol_lookup`).

### Key Integration Points

- Helper calls follow the existing pattern in
  `guest/riscv/src/riscv/trans/helpers.rs`: use `gen_helper_call()`
  with `self.env` as the first argument (RDI = CPU pointer).
- The x86-64 backend's Call constraint already maps args to
  RDI, RSI, RDX, RCX, R8, R9 (System V ABI).  Trace helpers
  receive the CPU env pointer in RDI and additional CSR/PC values
  in subsequent registers.
- Exec loop hooks should use `AtomicBool::load(Ordering::Relaxed)`
  for the enabled check -- this is a single instruction on x86-64
  and adds negligible overhead.
- TraceCollector should be wired into the system via
  `FullSystemCpu` in `system/src/cpus.rs`, which already holds
  `monitor_state: Option<Arc<MonitorState>>`.

--- Original Design Draft Start ---

# Kernel Execution Trace Design

Add dynamic execution tracing to Machina so OS course students can
observe how a teaching kernel (tg-rcore-tutorial ch2-ch4) runs inside
the emulator.  The feature reuses and extends the existing Monitor
infrastructure (MMP/HMP), outputs structured trace events to the
terminal first, and reserves a clean path for a future Web GUI.

## Background

Machina can already boot the tg-rcore-tutorial teaching kernels
(ch1-ch8).  The monitor console (MMP/HMP) supports pause, resume,
CPU state snapshots, and a small command set.  However, students
cannot observe the dynamic execution process of the kernel -- trap
entry/exit, task switching, address-space changes, syscalls -- in
real time.

This spec covers the first phase: **ch2-ch4 concepts** (traps,
scheduling, virtual memory), terminal text output, and ELF symbol
resolution.

## Requirements

- **Target users**: OS course students who need to see kernel
  execution dynamics to understand trap, scheduling, and virtual
  memory concepts.
- **Presentation**: terminal text output first; Web graphical
  interface later.
- **Scope**: ch2 (batch + trap + syscall), ch3 (timer interrupt +
  task scheduling), ch4 (Sv39 virtual memory).
- **Integration**: extend the existing Monitor protocol (MMP/HMP).
- **Symbols**: parse the ELF symbol table so events show function
  names, not raw PC values.
- **Performance**: tracing is off by default; zero overhead when
  disabled.  When enabled, execution slowdown should stay under 30%.
- **Prerequisite**: tg-rcore kernels already run correctly on
  Machina.

## Architecture

```
+----------------------------------------------------------+
|                    tg-rcore ELF                           |
|              (loaded at boot, symbols parsed)             |
+-----------------------------+----------------------------+
                              |
                              v
+----------------------------------------------------------+
|                    Machina Runtime                        |
|                                                          |
|  +------------------+    +---------------------------+   |
|  |  ELF SymbolTable |    |     TraceCollector        |   |
|  |  (pc->fn lookup) |<---|  (ring buffer + config)   |   |
|  +------------------+    +--+-----+-----+------------+   |
|                             |     |     |                |
|  +------------+    +--------+     |     +--------+       |
|  | Exec Loop  |    | Frontend     |              |       |
|  | (TB hook)  |    | (ecall/sret) |              |       |
|  +------------+    +--------------+              |       |
|                                                  |       |
+--------------------------------------------------|-------+
                                                   v
                              +----------------------------------+
                              |       Monitor Output Layer       |
                              |  +-------------+  +----------+  |
                              |  | HMP (term)  |  | MMP (TCP)|  |
                              |  | trace cmd   |  | JSON evt |  |
                              |  +-------------+  +----------+  |
                              +----------------------------------+
```

### Component responsibilities

| Component | Crate | Responsibility |
|-----------|-------|----------------|
| `TraceEvent` enum | `core/` | Shared event data types |
| `TraceSink` trait | `core/` | Output abstraction (terminal / JSON) |
| `EventFilter` | `core/` | Per-type event filtering |
| Exec loop hook | `accel/` | CSR snapshot diff, PC range match |
| Frontend hook | `guest/riscv/` | ecall/sret/mret/sfence helpers |
| `TraceCollector` | `monitor/` | Ring buffer, filtering, symbol lookup |
| `SymbolTable` | `monitor/` | ELF `.symtab` parser, pc-to-name |
| `TerminalSink` | `monitor/` | Coloured HMP output |
| CLI `-trace` | `src/` | Command-line option parsing |

### Data flow

1. Machina loads the kernel ELF at boot; `SymbolTable` parses
   `.symtab` once.
2. User activates tracing via `-trace` CLI flag or HMP `trace start`.
3. Instrumentation hooks fire during execution, construct
   `TraceEvent` structs, and write them into the `TraceCollector`
   ring buffer.
4. The active `TraceSink` drains the buffer -- `TerminalSink`
   prints coloured text to the HMP console; future `JsonStreamSink`
   pushes JSON over the MMP TCP connection.

## Event Types (ch2-ch4)

```rust
enum TraceEvent {
    // -- ch2: privilege & trap --
    TrapEnter {
        from_priv: u8,       // U=0, S=1, M=3
        to_priv: u8,
        cause: u64,          // scause value
        pc: u64,
        fn_name: Option<String>,
    },
    TrapExit {
        from_priv: u8,
        to_priv: u8,
        sepc: u64,
        fn_name: Option<String>,
    },

    // -- ch2: syscall --
    Syscall {
        id: usize,
        args: [u64; 3],      // a0, a1, a2
        pc: u64,
    },

    // -- ch3: task switch --
    TaskSwitch {
        from_pc: u64,
        to_pc: u64,
        fn_name: Option<String>,
    },

    // -- ch3: timer interrupt --
    TimerInterrupt {
        pc: u64,
        priv_level: u8,
    },

    // -- ch4: address space --
    AddressSpaceSwitch {
        old_satp: u64,
        new_satp: u64,
        old_asid: u16,
        new_asid: u16,
        pc: u64,
        fn_name: Option<String>,
    },

    // -- ch4: TLB --
    TlbFlush {
        pc: u64,
        fn_name: Option<String>,
    },
}
```

### Trigger points

| Event | Trigger | Detection method |
|-------|---------|-----------------|
| `TrapEnter` | ecall / sret / mret execution | Frontend helper call |
| `TrapExit` | sret / mret execution | Frontend helper call |
| `Syscall` | ecall from U-mode (scause=8) | Subset of TrapEnter |
| `TaskSwitch` | TB ret, PC in `__switch` range | Exec loop PC range match |
| `TimerInterrupt` | scause=5 (STI) | Subset of TrapEnter |
| `AddressSpaceSwitch` | satp CSR write | Exec loop CSR snapshot diff |
| `TlbFlush` | sfence.vma | Frontend helper call |

## Hook Placement

### Hook 1 -- Frontend translation (guest/riscv)

When tracing is enabled, the frontend emits an extra helper-call IR
instruction after translating certain opcodes:

| Instruction | Recorded data |
|-------------|---------------|
| `ecall` | `TrapEnter` + optional `Syscall` or `TimerInterrupt` |
| `sret` | `TrapExit { from_priv: S, ... }` |
| `mret` | `TrapExit { from_priv: M, ... }` |
| `sfence.vma` | `TlbFlush { pc, fn_name }` |

Implementation: at the end of each relevant opcode branch in
`translate_insn()`, check `TraceCollector::enabled()`.  If true,
emit a `tcg_gen_call()` helper that reads the current CSR values
and PC, constructs the appropriate `TraceEvent`, and pushes it
into the collector.

### Hook 2 -- Exec loop (accel/exec)

After each TB execution, when tracing is enabled:

```
if trace_enabled:
    new_priv = cpu.get_priv_level()
    new_satp = cpu.read_csr(SATP)
    if new_satp != prev_satp:
        emit AddressSpaceSwitch { ... }
    if pc_in_range(new_pc, "__switch"):
        emit TaskSwitch { ... }
    prev_priv = new_priv
    prev_satp = new_satp
```

Fast path: a single `AtomicBool` load.  CSR comparison is two
word-sized checks.  PC range match is O(log n) via sorted symbol
intervals.

### Hook 3 -- Symbol table (one-time at boot)

Parse `.symtab` from the loaded ELF:
- Extract all `STT_FUNC` symbols, store `[start_pc, end_pc) -> name`.
- Sort by address for binary search.
- Pre-tag teaching-critical functions: `__alltraps`, `__restore`,
  `__switch`, `trap_handler`, etc.

## TraceCollector

```rust
struct TraceCollector {
    buf: RingBuffer<TraceEvent>,     // capacity: 8192
    enabled: AtomicBool,             // fast-path check
    filter: EventFilter,             // per-type filtering
    symbols: SymbolTable,            // pc -> function name
    seq: AtomicU64,                  // monotonic event counter
}
```

### EventFilter

Users select which event categories to receive:

```
trace start trap,sched    # only TrapEnter/TrapExit/Syscall/TaskSwitch/TimerInterrupt
trace start vm            # only AddressSpaceSwitch/TlbFlush
trace start               # all events
```

The filter is a bitfield; each `TraceEvent` variant maps to one bit.
The hook checks the bit before constructing the event, avoiding
unnecessary allocation.

## TraceSink Abstraction

```rust
trait TraceSink {
    fn emit(&mut self, event: &TraceEvent);
}
```

Two implementations:

- **`TerminalSink`** (this phase): coloured text printed to the HMP
  console.
- **`JsonStreamSink`** (future Web phase): JSON objects pushed over
  the MMP TCP connection.

The Web phase only needs to implement `JsonStreamSink` and build a
browser frontend; no core logic changes.

## Monitor Protocol Extensions

### HMP commands

```
trace start [filter]    # activate tracing, optional type filter
trace stop              # deactivate tracing
trace status            # show enabled state, filter, event count
```

Filter syntax: comma-separated type names.
Categories: `trap`, `sched`, `vm`, `syscall`.

### MMP commands

```json
{ "execute": "trace-start",
  "arguments": { "filter": ["trap", "sched"] } }
-> { "return": {} }

{ "execute": "trace-stop" }
-> { "return": { "events_collected": 4237 } }

{ "execute": "trace-status" }
-> { "return": { "enabled": true, "filter": ["trap"],
                 "events_collected": 4237 } }
```

### MMP async events (reserved for Web phase)

```json
{
  "event": "TRACE_EVENT",
  "timestamp": 1042,
  "data": {
    "type": "trap-enter",
    "from_priv": "U",
    "to_priv": "S",
    "cause": 8,
    "cause_desc": "UserEcall",
    "pc": "0x80400024",
    "fn": "hello_world"
  }
}
```

## Terminal Output Format

One event per line.  Monotonic sequence number for cross-referencing.
Colour-coded by category: trap=red, sched=green, vm=blue,
syscall=yellow.

```
[#0012] [trap]     U->S cause=8(UserEcall) pc=0x80400024 hello_world
[#0013] [syscall]  id=64(write) a0=1 a1=0x80401000 a2=14
[#0014] [trap]     S->U sepc=0x80400028 hello_world
[#0015] [trap]     U->S cause=8(UserEcall) pc=0x80400030 hello_world
[#0016] [syscall]  id=93(exit) a0=0
[#0017] [sched]    task switch @ pc=0x80201A00 __switch
[#0018] [timer]    S-mode timer interrupt pc=0x80202040
[#0019] [vm]       satp asid 0->1 pc=0x80202B00 create_user_page_table
[#0020] [vm]       tlb flush pc=0x80202B10 create_user_page_table
```

## CLI Options

```bash
# Enable full tracing from start
machina -kernel tg-rcore-ch3.bin -trace

# Enable specific categories from start
machina -kernel tg-rcore-ch3.bin -trace trap,sched

# No -trace: activate on demand via HMP
machina -kernel tg-rcore-ch3.bin -monitor stdio
(machina) trace start
```

## Testing Strategy

| Type | Scope | Method |
|------|-------|--------|
| Unit | `SymbolTable` lookup, `TraceEvent` construction, ring buffer, `EventFilter` | `#[test]` |
| Unit | `TerminalSink` / `JsonStreamSink` output format | string/JSON assertion |
| Integration | tg-rcore-ch2 ELF: trap enter/exit + syscall events collected | boot + trace + assert event sequence |
| Integration | tg-rcore-ch3 ELF: timer interrupt + task switch events | same |
| Integration | tg-rcore-ch4 ELF: address space switch + tlb flush events | same |
| Perf | Zero overhead when tracing disabled (vs baseline) | benchmark |
| Perf | < 30% slowdown when tracing enabled | benchmark |

## Module Change Summary

| Module | Action | Content |
|--------|--------|---------|
| `core/` | New | `TraceEvent`, `TraceSink`, `EventFilter` |
| `guest/riscv/` | Modify | ecall/sret/mret/sfence translation branches |
| `accel/` | Modify | Exec loop CSR diff + PC range match |
| `monitor/` | New | `TraceCollector`, `SymbolTable`, `TerminalSink` |
| `monitor/` | Modify | HMP/MMP command dispatch |
| `src/` (CLI) | Modify | `-trace` command-line option |

## Out of Scope

- ch5-ch8 events (fork/exec, file system, pipe/signal, thread/sync)
  -- deferred to a later spec.
- GDB stub or single-step debugging.
- Web GUI implementation (only the `JsonStreamSink` interface is
  reserved).
- Performance profiling (hot-path detection, instruction counts).
- QMP event async push (format defined, implementation deferred).

--- Original Design Draft End ---
