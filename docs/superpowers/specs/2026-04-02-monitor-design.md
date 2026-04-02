# Monitor Console Design for Machina

## Goal

Implement a QEMU-compatible monitor system for Machina with MMP (QMP-compatible machine protocol) as the core and HMP (human text commands) as a convenience layer. Support three access methods: `-nographic` Ctrl+A C switching, `-monitor stdio`, and `-monitor tcp:host:port`.

## Architecture

```
+--------------+     +-----+     +-----+
| -nographic   |---->| HMP |---->| MMP |---> Machine
| Ctrl+A C     |     |(text|     |(QMP |    / CpuManager
| -monitor stdio     | cmds|     | JSON|
+--------------+     | parse)    | wire)
                     +-----+     +-----+
+--------------+                   |
| -monitor tcp |-------------------+
| (raw QMP/MMP)|     (direct JSON)
+--------------+
```

## MMP (Machina Monitor Protocol)

QMP-compatible JSON wire protocol. Full compatibility with QMP format.

### Wire Format

Connection handshake:
```json
{"QMP": {"version": {"machina": {"major": 0, "minor": 1, "micro": 0}}, "capabilities": []}}
```

Client request:
```json
{"execute": "command-name", "arguments": {"key": "value"}}
```

Success response:
```json
{"return": {}}
```

Error response:
```json
{"error": {"class": "GenericError", "desc": "description"}}
```

### First-Version Commands

| Command | Arguments | Response | Description |
|---------|-----------|----------|-------------|
| `qmp_capabilities` | none | `{}` | Required handshake |
| `query-status` | none | `{"running": bool}` | VM run state |
| `stop` | none | `{}` | Pause all vCPUs |
| `cont` | none | `{}` | Resume all vCPUs |
| `quit` | none | `{}` | Exit emulator |
| `system_reset` | none | `{}` | Reset machine |
| `query-cpus-fast` | none | `[{"cpu-index": 0, "thread-id": N, "props": {"core-id": 0}}]` | List vCPUs |

### Implementation

In `monitor/src/mmp.rs`:
- `MmpEngine` struct: holds `Arc` references to `CpuManager`, `Machine`, stop flag
- `fn execute(&self, cmd: &str, args: Value) -> Value` — dispatch command, return JSON response
- No I/O handling — pure command execution logic

## HMP (Human Monitor Protocol)

Text command interface built on top of MMP.

### Commands

| HMP Command | MMP Equivalent | Output Format |
|------------|----------------|---------------|
| `info status` | `query-status` | `VM status: running` or `paused` |
| `info registers` | (direct CPU access) | GPR x0-x31 + pc dump |
| `info cpus` | `query-cpus-fast` | `* CPU #0: pc=0x... (running)` |
| `stop` | `stop` | `(machina)` |
| `cont` / `c` | `cont` | `(machina)` |
| `quit` / `q` | `quit` | (exits) |
| `system_reset` | `system_reset` | `(machina)` |
| `help` | (local) | Command list |

### Implementation

In `monitor/src/hmp.rs`:
- `HmpConsole` struct: wraps `MmpEngine`, provides text I/O
- `fn handle_line(&mut self, line: &str) -> String` — parse text command, call MMP, format response
- Prompt: `(machina) `
- `info registers` reads CPU state directly (not through MMP, since QMP has no register dump command in QEMU either)

## Transport

### -nographic (Ctrl+A C)

Extend `StdioChardev` to support mode switching:
- Default mode: guest serial (current behavior)
- Ctrl+A C: switch to monitor mode
- In monitor mode: input goes to HMP, output comes from HMP
- Ctrl+A C again: switch back to guest serial
- `(machina)` prompt shown when entering monitor mode

Implementation: add `MonitorMux` state to `StdioChardev` that routes bytes between guest chardev callback and HMP console.

### -monitor stdio

When `-monitor stdio` is specified without `-nographic`:
- stdio is dedicated to HMP console
- Guest serial output goes to NullChardev (no display)
- HMP prompt appears immediately

### -monitor tcp:host:port

TCP listener accepting MMP/QMP JSON connections:
- One connection at a time (like QEMU)
- Send greeting on connect
- Read JSON lines, dispatch via MmpEngine
- Write JSON responses
- Close on disconnect

### CLI

```
-monitor stdio          HMP on stdin/stdout
-monitor tcp:host:port  MMP/QMP on TCP
```

With `-nographic`: always has Ctrl+A C built-in (no extra flag needed).

## Crate Structure

```
monitor/
  Cargo.toml
  src/
    lib.rs        pub mod mmp, hmp;
    mmp.rs        MmpEngine: QMP-compatible command dispatch
    hmp.rs        HmpConsole: text commands wrapping MMP
```

Dependencies: machina-core (Machine trait), machina-system (CpuManager), serde_json.

## Scope Boundaries

**In scope**: MMP (7 commands), HMP (8 commands), Ctrl+A C mux, -monitor stdio, -monitor tcp.

**Out of scope**: QMP events, qom-*, blockdev-*, chardev-*, migration-*, full QMP command set. These are deferred to future iterations.
