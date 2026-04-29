# CLAUDE.md

This file is the authoritative source of rules for all AI agents and human
contributors working in this repository. Tool-specific files (e.g. AGENTS.md)
point here and add only tool-specific context on top.

Detailed coding guidelines live under `docs/en/` (English) and `docs/zh/`
(Chinese). When in doubt, this file takes precedence.

## Project Overview

Machina is a modular RISC-V full-system emulator written in Rust, featuring a
JIT dynamic binary translation engine. It reimplements core QEMU concepts
(TCG, device models, full-system emulation) to boot rCore-Tutorial ch1-ch8.

## Quick Start

```bash
make build          # build all crates (debug)
make release        # build all crates (release)
make test           # run all tests
make clippy         # lint with -D warnings
make fmt            # auto-format
make fmt-check      # check formatting
make docs           # generate rustdocs
make clean          # clean build artifacts
```

See the `Makefile` for the full list of targets.

## Boundary Constraints

### Unsafe Rust

`unsafe` is only permitted in these scenarios:

- JIT code buffer allocation and execution (mmap + mprotect RWX)
- Calling generated host code (function pointer cast from code buffer)
- Raw pointer access for guest memory emulation (TLB fast path)
- Inline assembly in the backend code emitter
- FFI interfaces to external libraries

Everything else must be safe Rust. Every `unsafe` block requires a short
justification comment and must remain narrowly scoped.

### Test Centralization

All tests **must** live in the `tests/` crate (`machina-tests`). Do not add
tests in individual crate `#[cfg(test)] mod tests` blocks or per-crate
`tests/` directories. Individual crates expose `pub` interfaces for the test
crate to call.

### Code Style

- 80-column line width for all code and code comments
- 4-space indentation, no tabs
- `cargo fmt` before committing
- `cargo clippy -- -D warnings` must pass with zero warnings
- English comments only, and only at key logic points

Full style guide: `docs/en/contributing.md` / `docs/zh/contributing.md`

## Commit Rules

- English commit messages
- Format: `module: subject` (imperative mood, <= 72 characters)
- Body: describe what changed and why, <= 80 characters per line
- Add `Signed-off-by: Name <email>` for commits in this repository
- No AI-related sign-off lines (e.g. `Co-Authored-By: Claude`)

Full git guidelines: `docs/en/contributing.md#part-4-git-guidelines` / `docs/zh/contributing.md#part-4-git-指南`

## Testing Rules

- Every bug fix must add or update a regression test
- Every new feature must include tests for expected path, edge cases, and
  failure cases
- A change is not done if tests fail, are silently skipped, or do not check
  the claimed behavior
- Run the narrowest useful tests while iterating, then full validation before
  finishing

Full testing guide: `docs/en/contributing.md#part-5-testing-guide` / `docs/zh/contributing.md#part-5-测试指南`

## Documentation

When behavior, interfaces, or assumptions change, update the corresponding
docs under `docs/`. Existing documentation:

| File | Content |
|------|---------|
| `getting-started.md` | Build, run, and boot guide |
| `architecture.md` | Architecture and design docs |
| `reference.md` | IR opcodes, x86-64 backend, device model, performance, test architecture |
| `contributing.md` | Code style, coding/Rust/Git guidelines, testing guide |

## Quality Gates

Before submitting any change:

1. `make fmt-check` passes
2. `make clippy` passes
3. `make test` passes
4. Relevant documentation is updated
