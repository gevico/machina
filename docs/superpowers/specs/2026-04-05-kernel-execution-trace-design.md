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
