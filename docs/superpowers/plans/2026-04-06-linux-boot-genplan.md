# Linux Boot Plan: Machina OpenSBI + Linux 6.12.51 Full Boot

## Goal Description

Enable the machina RISC-V full-system emulator to correctly load and run
OpenSBI firmware and Linux kernel 6.12.51, reaching a userspace shell
that responds to interactive commands. The approach is stage-gated
incremental with 5 stages (S0-S4), validated at each stage by serial
output comparison against a QEMU 10.1.0 reference. Each stage follows an
RLCR (Run-Log-Critique-Revise) iteration loop until its acceptance
criteria are met.

### Constraints

- Single hart only (kernel .config has SMP disabled)
- Correctness only; no performance requirement
- Difftest: hybrid serial output + event tracing (no QEMU source
  modification)
- RISC-V ISA: RV64IMAFDC with Zicsr, Zifencei
- MMU: Sv39 minimum
- Backend: x86-64 JIT only

### QEMU Reference Command

```bash
qemu-system-riscv64 \
  -M virt -m 256M -nographic \
  -kernel arch/riscv/boot/Image \
  -initrd ./chytest/rootfs.cpio.gz \
  -append "console=ttyS0 earlycon"
```

### Machina Target Command

```bash
machina \
  -M riscv64-ref -m 256 -nographic \
  -bios /path/to/opensbi-riscv64-generic-fw_dynamic.bin \
  -kernel arch/riscv/boot/Image \
  -initrd ./chytest/rootfs.cpio.gz \
  -append "console=ttyS0 earlycon"
```

---

## Acceptance Criteria

### AC-0: Difftest Infrastructure Ready

**Description:** The serial comparison tool, QEMU reference logs, and
machina event tracer (`--trace` flag) are functional and can be used to
validate all subsequent stages.

**Positive Tests:**
- `tools/difftest/gen_ref.sh` generates a complete QEMU serial log
  containing OpenSBI banner, Linux boot, and shell prompt
- `tools/difftest/serial_diff.py` correctly identifies matching and
  diverging serial output files
- `machina --trace /tmp/trace.log` produces a trace file containing
  CSR, exception, and MMIO event records
- `cargo test` passes with new trace module tests

**Negative Tests:**
- `serial_diff.py` reports divergence when comparing mismatched logs
- `serial_diff.py` reports short output when machina log is truncated
- `machina --trace` with invalid path returns an error

**Deliverables:**
- `util/src/trace.rs` — EventTracer module
- `src/main.rs` — `--trace` CLI flag integration
- `system/src/cpus.rs` — CSR/exception trace instrumentation
- `memory/src/address_space.rs` — MMIO trace instrumentation
- `tools/difftest/gen_ref.sh` — QEMU reference generator
- `tools/difftest/serial_diff.py` — Serial comparison tool
- `tools/difftest/ref/qemu_virt_serial.log` — Reference serial output
- `tools/difftest/ref/qemu_virt_events.log` — Reference event log

---

### AC-1: Boot ROM Executes and Jumps to OpenSBI (S0)

**Description:** Machina's riscv64-ref machine starts at PC=0x1000
(MROM reset vector), executes the 6-instruction boot sequence, loads
a0=mhartid/a1=FDT/a2=fw_dynamic_info, and jumps to the OpenSBI firmware
entry point. OpenSBI's first banner line appears on serial output.

**Positive Tests:**
- Unit test: boot ROM memory at 0x1000 contains the expected 10 x u32
  words (6 instructions + start_addr + fdt_addr hi/lo)
- Unit test: `DynamicInfo` at offset 0x28 has magic=0x4942534F,
  version=2, next_addr=kernel_entry, next_mode=1 (S-mode)
- Integration test: machina serial output contains "OpenSBI"
- QEMU serial diff: serial output matches QEMU reference up to and
  including the first "OpenSBI" line

**Negative Tests:**
- Boot ROM with corrupted instruction encoding causes machina to halt
  (no infinite loop or silent hang)
- Missing firmware file returns clear error message, not a crash
- Wrong fw_dynamic_info.magic causes OpenSBI to reject the handoff

**Deliverables:**
- `hw/riscv/src/boot.rs` — Verified boot ROM + fw_dynamic_info loading
- `tests/src/linux_boot/boot_rom.rs` — Boot ROM content tests
- `tests/src/linux_boot/s0_opensbi.rs` — S0 integration test

---

### AC-2: OpenSBI Initializes and Transitions to Linux (S1)

**Description:** OpenSBI firmware completes M-mode initialization
(PMP setup, trap handler, interrupt delegation, timer init, SBI ecall
handling) and performs mret to enter S-mode at the Linux kernel entry
point. The complete OpenSBI banner (including "Boot HART MEDELEG" etc.)
appears on serial output, and Linux kernel begins executing.

**Positive Tests:**
- Unit test: PMP CSR (pmpcfg0-3, pmpaddr0-15) read/write in M-mode
- Unit test: mret switches from M-mode to S-mode (mstatus.MPP→priv,
  pc=mepc)
- Unit test: ecall from S-mode traps to M-mode (when not delegated)
- Unit test: ecall from S-mode traps to S-mode (when delegated via
  medeleg)
- Integration test: serial output contains full OpenSBI banner
- QEMU serial diff: serial output matches QEMU through OpenSBI banner

**Negative Tests:**
- PMP configuration that blocks all memory access causes OpenSBI to
  trap (visible in exception trace, not silent hang)
- mret with invalid MPP value does not corrupt privilege level
- ecall without mtvec set causes defined error (not host crash)

**Deliverables:**
- `tests/src/riscv_csr.rs` — PMP, mret, ecall tests
- `tests/src/linux_boot/s1_opensbi_full.rs` — S1 integration test
- Fixes to PMP, mret, or exception handling as needed

---

### AC-3: Linux Early Boot Reaches start_kernel (S2)

**Description:** Linux kernel head.S executes: disables interrupts,
clears BSS, creates initial Sv39 page tables, writes satp to enable
MMU, sets stvec, and reaches `start_kernel()`. The "Linux version
6.12.51" banner appears on serial output.

**Positive Tests:**
- Unit test: Sv39 page table walk with known 3-level table produces
  correct PA for a given VA
- Unit test: sfence.vma invalidates TLB (subsequent access triggers
  fresh page walk)
- Unit test: fence.i does not crash or hang
- Integration test: serial output contains "Linux version 6.12.51"
- QEMU serial diff: serial output matches QEMU through Linux banner

**Negative Tests:**
- Sv39 walk with invalid PTE (V=0) raises page fault (not host crash)
- Sv39 walk with insufficient permissions raises access fault
- TLB entries persist after sfence.vma (test verifies flush)

**Deliverables:**
- `tests/src/linux_boot/sv39_walk.rs` — Page table walk tests
- `tests/src/linux_boot/tlb_flush.rs` — TLB flush tests
- `tests/src/linux_boot/s2_linux_banner.rs` — S2 integration test
- Fixes to Sv39 MMU, TLB flush, or fence as needed

---

### AC-4: Linux Kernel Boots to init (S3)

**Description:** Linux `start_kernel()` completes: parses DTB, probes
drivers (UART 16550A, PLIC, timer), initializes scheduler, unpacks
initramfs. Full kernel boot log matches QEMU reference. The init
process starts.

**Positive Tests:**
- Unit test: UART 16550A LSR.THRE bit set after init (transmitter empty)
- Unit test: UART DLL/DLM accessible via DLAB latch in LCR
- Unit test: PLIC claim returns highest-priority pending IRQ
- Unit test: PLIC complete clears the IRQ claim
- Unit test: FDT `/chosen` node contains `bootargs`, `stdout-path`,
  `linux,initrd-start`, `linux,initrd-end`
- Integration test: serial output contains "console [ttyS0] enabled"
- Integration test: serial output contains "Freeing unused kernel memory"
- QEMU serial diff: serial output matches QEMU reference through kernel
  boot (tolerating address/value differences)

**Negative Tests:**
- UART with unmapped LSR register returns 0 (not host crash)
- PLIC claim with no pending IRQ returns 0
- FDT missing `/chosen` node: kernel still boots (no panic on missing
  optional node)

**Deliverables:**
- `hw/riscv/src/ref_machine.rs` — FDT alignment with QEMU virt
- `tests/src/linux_boot/fdt_check.rs` — FDT node comparison test
- `tests/src/linux_boot/uart_16550a.rs` — UART register tests
- `tests/src/linux_boot/plic_protocol.rs` — PLIC protocol tests
- `tests/src/linux_boot/s3_kernel_boot.rs` — S3 integration test
- Fixes to FDT, UART, PLIC, CLINT, or initramfs loading as needed

---

### AC-5: Userspace Shell Responds to Commands (S4)

**Description:** The init process from initramfs executes, launches a
shell. A shell prompt appears on serial output. The shell responds to
`echo hello` with `hello`.

**Positive Tests:**
- Unit test: ecall from U-mode traps to S-mode (with medeleg
  delegation)
- Integration test: serial output contains a shell prompt (`#`, `$`,
  `login:`, or `/ #`)
- E2E test: machina reaches shell prompt within 120 seconds
- QEMU serial diff: full serial output matches QEMU reference

**Negative Tests:**
- U-mode access to S-mode-only page raises page fault (not host crash)
- Missing /init in initramfs causes kernel panic with clear message
  (not silent hang)

**Deliverables:**
- `tests/src/linux_boot/umode.rs` — U-mode ecall tests
- `tests/src/linux_boot/s4_shell.rs` — S4 E2E test

---

### AC-6: Final Validation

**Description:** All existing and new tests pass. No regressions. Code
quality checks pass.

**Positive Tests:**
- `cargo test` — all tests pass (existing ~964 + new linux_boot tests)
- `cargo clippy -- -D warnings` — zero warnings
- `cargo fmt --check` — zero formatting issues
- `serial_diff.py ref.log machina.log S4` — full serial match

**Negative Tests:**
- N/A (this is a quality gate, not a feature)

---

## Path Boundaries

### Upper Bound

The maximum scope for this project includes:

- Full serial output match with QEMU (byte-exact where possible)
- All device models fully compatible with Linux 6.12.51 drivers
- Event tracer covering all CSR, exception, and MMIO events
- Comprehensive regression test suite for each stage
- Optional: QEMU TCG plugin for instruction-level difftest
- Optional: multi-hart (SMP) support for future stages
- Optional: performance optimization (native FP, vector backend)

### Lower Bound

The minimum viable product requires:

- Machina can load and run OpenSBI `fw_dynamic.bin` from QEMU's pc-bios
- OpenSBI completes initialization and jumps to Linux
- Linux boots to a shell prompt
- Shell responds to at least one command (`echo hello`)
- At least one integration test per stage (S0-S4) verifying serial
  output contains expected markers
- `cargo test` passes with no regressions to existing tests

### Explicitly Out of Scope

- SMP / multi-hart support
- Performance optimization or benchmarking
- PCIe device emulation
- VirtIO block/net device functionality (beyond what boot requires)
- Vector extension support
- APLIC / IMSIC interrupt controllers
- Svvptc, Sstc, or other optional extensions
- W^X code buffer (security hardening)
- GDB stub for machina itself

---

## Dependencies and Sequence

### Phase 0: Infrastructure (AC-0)

**Milestone:** Difftest tools and trace infrastructure are ready.

```
Task 1: --trace CLI flag + trace module        [util/src/trace.rs]
Task 2: CSR/exception/MMIO instrumentation     [system, memory crates]
Task 3: serial_diff.py + gen_ref.sh            [tools/difftest/]
Task 4: QEMU reference logs                    [tools/difftest/ref/]
```

**Dependencies:** None. Can start immediately.
**Estimated RLCR iterations:** 1-2 (infrastructure, unlikely to have
complex bugs).

---

### Phase 1: Boot ROM + OpenSBI (AC-1 + AC-2)

**Milestone:** OpenSBI completes and Linux kernel entry is reached.

```
Task 5:  Boot ROM + DynamicInfo unit tests      [tests/src/linux_boot/]
Task 6:  OpenSBI firmware loading via -bios      [hw/riscv/src/boot.rs]
Task 7:  S0 integration test (OpenSBI banner)    [tests/src/linux_boot/]
Task 8:  PMP CSR tests + fixes                   [tests/src/riscv_csr.rs]
Task 9:  mret M->S test + fixes                  [tests/src/riscv_csr.rs]
Task 10: ecall trap delegation tests + fixes     [tests/src/riscv_csr.rs]
Task 11: S1 integration test (full OpenSBI)      [tests/src/linux_boot/]
```

**Dependencies:** Phase 0 complete (need trace for debugging).
**Estimated RLCR iterations:** 2-5 per task (OpenSBI exercises many
subsystems).

**Key risk:** OpenSBI uses SBI DBCN extension for console output.
If machina's UART TX path doesn't work from M-mode firmware, this will
block S1. Mitigation: trace MMIO writes to UART address to verify
OpenSBI's console output attempts.

---

### Phase 2: Linux Early Boot (AC-3)

**Milestone:** Linux kernel banner appears on serial output.

```
Task 12: Sv39 page table walk tests + fixes      [tests/src/linux_boot/]
Task 13: sfence.vma / fence.i tests + fixes      [tests/src/linux_boot/]
Task 14: S2 integration test (Linux banner)       [tests/src/linux_boot/]
```

**Dependencies:** Phase 1 complete (need kernel to start executing).
**Estimated RLCR iterations:** 3-8 (Sv39 walk has many edge cases).

**Key risk:** Linux creates large page tables with mixed page sizes
(4KiB, 2MiB). Any Sv39 walk bug will cause page faults. Mitigation:
pre-write comprehensive page table walk tests covering all page sizes
and permission combinations.

---

### Phase 3: Linux Kernel Boot (AC-4)

**Milestone:** Full kernel boot log matches QEMU reference.

```
Task 15: FDT alignment with QEMU virt             [hw/riscv/src/ref_machine.rs]
Task 16: UART 16550A register completeness tests   [tests/src/linux_boot/]
Task 17: PLIC claim/complete protocol tests        [tests/src/linux_boot/]
Task 18: S3 integration test (full kernel boot)    [tests/src/linux_boot/]
```

**Dependencies:** Phase 2 complete (need MMU working for kernel).
**Estimated RLCR iterations:** 3-10 (many devices must cooperate).

**Key risk:** FDT node mismatch causes kernel to skip driver probe.
Mitigation: dump and compare machina DTB with QEMU DTB before running
kernel.

---

### Phase 4: Userspace (AC-5)

**Milestone:** Shell prompt appears and responds to commands.

```
Task 19: U-mode ecall trap tests + fixes          [tests/src/linux_boot/]
Task 20: S4 integration test (shell prompt)        [tests/src/linux_boot/]
```

**Dependencies:** Phase 3 complete (need kernel fully booted).
**Estimated RLCR iterations:** 1-4 (U-mode usually works if S-mode
works).

**Key risk:** initramfs /init is missing or not statically linked.
Mitigation: verify rootfs.cpio.gz with QEMU before testing on machina.

---

### Phase 5: Final Validation (AC-6)

**Milestone:** All tests pass, all quality gates met.

```
Task 21: Full cargo test + clippy + fmt check
```

**Dependencies:** All phases complete.

---

## Execution Summary

| Phase | AC   | Tasks | Key Risk                          | Est. RLCR Iters |
|-------|------|-------|-----------------------------------|------------------|
| 0     | AC-0 | 1-4   | Low (tooling)                     | 1-2              |
| 1     | AC-1 | 5-7   | Firmware loading / boot ROM       | 2-5              |
| 1     | AC-2 | 8-11  | SBI ecall / PMP / mret            | 2-5              |
| 2     | AC-3 | 12-14 | Sv39 page table walk edge cases   | 3-8              |
| 3     | AC-4 | 15-18 | FDT mismatch / device bugs        | 3-10             |
| 4     | AC-5 | 19-20 | U-mode / initramfs                | 1-4              |
| 5     | AC-6 | 21    | Regression detection              | 1                |

**Total tasks:** 21
**Estimated total RLCR iterations:** 30-80 (varies with bug count)

## Version Control Tags

Each AC milestone is tagged when its acceptance criteria are fully met:
- `linux-boot-ac0` — Infrastructure ready
- `linux-boot-ac1` — S0 Boot ROM passes
- `linux-boot-ac2` — S1 OpenSBI passes
- `linux-boot-ac3` — S2 Linux early boot passes
- `linux-boot-ac4` — S3 Full kernel boot passes
- `linux-boot-ac5` — S4 Shell prompt passes
- `linux-boot-done` — AC-6 final validation passes
