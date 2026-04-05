# Linux Boot Design: Machina OpenSBI + Linux 6.12.51 Full Boot

Date: 2026-04-06
Status: Approved
Scope: Enable machina to boot OpenSBI + Linux 6.12.51 to userspace shell

## 1. Overview

Enable machina (RISC-V full-system emulator) to correctly load and run
OpenSBI firmware and Linux kernel 6.12.51, reaching a userspace shell.
The approach is a stage-gated incremental strategy with 5 stages, each
validated via serial output comparison against a QEMU 10.1.0 reference.

### Constraints

- Single hart only (kernel config has SMP disabled)
- Correctness only, no performance requirement
- Difftest method: hybrid serial output + event tracing
- No modification to QEMU source code

### QEMU Reference Command

```bash
qemu-system-riscv64 \
  -M virt -m 256M -nographic \
  -kernel arch/riscv/boot/Image \
  -initrd ./chytest/rootfs.cpio.gz \
  -append "console=ttyS0 earlycon"
```

## 2. Strategy: Stage-Gated Incremental with RLCR

The boot process is divided into 5 stages. Each stage has a clear
pass/fail criterion based on serial output comparison. An RLCR
(Run-Log-Critique-Revise) loop iterates within each stage until it
passes.

### Stage Definitions

| Stage | Name         | Scope                                        | Pass Criterion                                    |
|-------|--------------|----------------------------------------------|---------------------------------------------------|
| S0    | Boot ROM     | PC=0x1000 reset vector -> jump to OpenSBI    | OpenSBI banner first line appears                 |
| S1    | OpenSBI      | M-mode firmware init -> mret to S-mode       | Full OpenSBI banner + kernel entry reached        |
| S2    | Linux Early  | head.S -> MMU enable -> start_kernel entry   | "Linux version 6.12.51" kernel banner appears     |
| S3    | Linux Kernel | start_kernel -> driver init -> initramfs     | Full kernel boot log matches QEMU reference       |
| S4    | Userspace    | /init -> execve -> shell                     | Shell prompt appears, echo hello works            |

### RLCR Loop Per Stage

```
R (Run)     -> Run machina, capture serial output
L (Log)     -> Compare with QEMU reference serial output
C (Critique) -> If diverged: analyze CSR/exception/MMIO traces
R (Revise)  -> Fix issue, add regression test, re-iterate
```

### Version Control

Each passing stage is tagged:
- `linux-boot-s0-pass`, `linux-boot-s1-pass`, ..., `linux-boot-s4-pass`

## 3. Difftest Infrastructure

### 3.1 Serial Output Reference Generation

Script `tools/difftest/gen_ref.sh`:

```bash
#!/bin/bash
timeout 60 qemu-system-riscv64 -M virt -m 256M -nographic \
  -kernel arch/riscv/boot/Image \
  -initrd ./chytest/rootfs.cpio.gz \
  -append "console=ttyS0 earlycon" \
  | tee "$1/ref_serial.log"
```

### 3.2 Serial Comparison Tool

`tools/difftest/serial_diff.py`:

- Accepts QEMU reference log and machina log
- Byte-by-byte / line-by-line comparison
- Outputs: matched lines, first divergence position, diff context
- Supports stage-scoped comparison (compare only up to S0/S1/... boundary)

### 3.3 Event Tracer (Machina --trace flag)

New module in machina, enabled at runtime via `--trace` flag. Records
three categories of events to a structured log file:

| Event Type      | Fields                                    | Trigger                     |
|-----------------|-------------------------------------------|-----------------------------|
| CSR access      | pc, csr_addr, old_val, new_val, mode      | CSR instruction execution   |
| Exception/Trap  | pc, cause, epc, from_mode, to_mode        | Exception entry + sret/mret |
| MMIO access     | addr, size, val, is_write, device_name    | MMIO dispatch layer         |

Output format: one structured record per line (JSON or concise text).

### 3.4 QEMU Event Reference

For debugging divergences, generate QEMU event log:

```bash
qemu-system-riscv64 -M virt -m 256M -nographic \
  -kernel arch/riscv/boot/Image \
  -initrd ./chytest/rootfs.cpio.gz \
  -append "console=ttyS0 earlycon" \
  -d int,mmu,guest_errors \
  -D qemu_events.log
```

Compare with machina event trace to locate behavioral differences.

## 4. Stage Technical Analysis

### S0: Boot ROM

**Execution**: 10 instructions at 0x1000 (reset vector):
- auipc/addi to compute fw_dynamic_info address
- csrr a0, mhartid
- ld a1, FDT address
- ld t0, firmware entry
- jr t0 -> jump to OpenSBI

**Machina capabilities**: All implemented (RV64I, CSR read, MROM,
FDT loading, initrd loading).

**Key dependencies**:
- Boot ROM code correctly written to MROM at 0x1000
- fw_dynamic_info struct at 0x1028 (magic, version, next_addr,
  next_mode)
- OpenSBI firmware loaded to RAM base
- FDT placed near end of DRAM

**Tests**:
- Unit: boot ROM memory content matches expected instruction encodings
- Unit: fw_dynamic_info fields are correct
- Integration: machina serial output contains "OpenSBI"

**Potential blockers**:
- Boot ROM not populated correctly
- fw_dynamic_info not placed at expected offset
- OpenSBI load address mismatch

---

### S1: OpenSBI

**Execution**: OpenSBI runs in M-mode:
- Parse fw_dynamic_info
- Initialize PMP (pmpcfg0-3, pmpaddr0-15)
- Set M-mode trap handler (mtvec)
- Configure interrupt delegation (mideleg, medeleg)
- Initialize CLINT timer (mtimecmp)
- Handle SBI ecalls (base, timer, IPI, RFence, HSM, DBCN)
- mret to S-mode Linux kernel entry

**Machina capabilities**: Mostly implemented (CSR r/w, mret, CLINT,
PLIC, exception handling, UART).

**Key dependencies**:
- PMP CSR full read/write support
- mret correct M->S mode switch (mstatus.MPP -> privilege, pc=mepc)
- ecall S->M correct trap (sepc, scause, enter M-mode handler)
- UART TX functional (OpenSBI console output)
- CLINT mtime monotonic increment

**Tests**:
- Unit: PMP CSR read/write
- Unit: mret M->S mode switch
- Unit: ecall S->M trap
- Integration: full OpenSBI banner on serial output
- Regression: mtest firmware for PMP behavior

**Potential blockers**:
- PMP config blocks legitimate memory access
- mret mode switch incorrect
- SBI ecall not trapped to M-mode correctly
- UART TX not working for OpenSBI console

---

### S2: Linux Early Boot

**Execution** (head.S):
- _start_kernel: disable interrupts, disable FPU, clear BSS
- setup_vm(): create initial Sv39 page tables
- relocate_enable_mmu: write satp CSR
- Set stvec, tail start_kernel

**Machina capabilities**: Sv39 MMU implemented, CSR support complete.

**Key dependencies**:
- satp write triggers correct Sv39 page table walk
- fence.i / sfence.vma correctly flush TLB
- High volume memory operations for page table fill

**Tests**:
- Unit: Sv39 page table walk (known table + VA -> expected PA)
- Unit: sfence.vma TLB invalidation
- Integration: "Linux version 6.12.51" on serial output

**Potential blockers**:
- Sv39 page table walk edge cases (permission bits, A/D bits)
- fence.i / sfence.vma not flushing TLB properly
- Memory corruption during page table operations

---

### S3: Linux Kernel Boot

**Execution** (start_kernel):
- setup_arch(): parse DTB, SBI init, paging_init
- trap_init(), time_init()
- Driver probe: UART 16550A, PLIC, timer
- Scheduler init
- initramfs unpack to rootfs

**Machina capabilities**: UART, PLIC, CLINT devices implemented.

**Key dependencies**:
- UART 16550A full register compatibility (LCR, LSR, IER, DLL, DLM)
- FDT nodes match kernel expectations (cpus, memory, uart, plic, chosen)
- PLIC full behavior (priority, threshold, claim, complete, enable)
- CLINT timer interrupt at 10 MHz mtime frequency
- initramfs loaded with correct chosen node (initrd-start/end)

**Tests**:
- Unit: UART 16550A register behavior
- Unit: PLIC claim/complete protocol
- Integration: full kernel boot log matches QEMU reference
- Regression: mtest PLIC timer interrupt test

**Potential blockers**:
- FDT nodes incomplete or incompatible
- PLIC interrupt routing wrong
- Timer interrupt frequency mismatch
- initramfs load address / chosen node wrong

---

### S4: Userspace Shell

**Execution**: /init -> execve -> shell:
- Kernel execve (ecall U->S inside guest)
- Shell program execution
- stdin/stdout via UART

**Machina capabilities**: U-mode support, ecall exception handling.

**Key dependencies**:
- U-mode correct execution (sstatus.SUM, U-mode page permissions)
- ecall U->S correct trap (sepc, scause=8)
- UART RX functional (shell reads input)
- initramfs /init executable (statically linked)

**Tests**:
- Integration: shell prompt appears
- Integration: send "echo hello", receive "hello"
- E2E: full boot -> shell -> execute command -> correct output

**Potential blockers**:
- U-mode switch incorrect
- UART RX not working
- initramfs /init not executable or missing

## 5. RLCR Integration with humanize

Each stage is executed as a humanize-rlcr task:

1. **Plan**: Generate implementation plan for the stage (writing-plans)
2. **RLCR iterations**:
   - Run machina -> compare serial output
   - If fail -> Log traces + Critique (optionally ask-model for
     external analysis)
   - Revise fix + add tests
   - Next iteration
3. **Stage acceptance**: Serial output matches QEMU reference, all new
   tests pass
4. **Advance**: git tag, proceed to next stage

### humanize-rlcr Per-Stage Checklist

For each stage:
- [ ] Generate QEMU reference serial output
- [ ] Run machina, capture serial output
- [ ] Compare with serial_diff.py
- [ ] If match: stage passes
- [ ] If divergence: enable --trace, rerun machina
- [ ] Generate QEMU event reference (-d int,mmu)
- [ ] Analyze traces to identify root cause
- [ ] Implement fix
- [ ] Add regression test
- [ ] Run cargo test + cargo clippy
- [ ] Re-iterate until stage passes
- [ ] Tag: linux-boot-sN-pass

## 6. Risk Management

| Risk                                        | Mitigation                                        |
|---------------------------------------------|---------------------------------------------------|
| OpenSBI uses unimplemented instruction/CSR  | EventTracer catches illegal instruction traps     |
| Sv39 page table walk edge case bugs         | Pre-write page table walk unit tests              |
| FDT mismatch causes kernel panic            | Compare machina DTB with QEMU dumpdtb output      |
| initramfs format/loading issues             | Verify initramfs with QEMU first, reuse same file |
| Timer interrupt frequency drift             | S3 stage specifically tests CLINT timer precision  |
| Long RLCR cycles on hard bugs               | Use ask-model for external analysis of traces     |
| Regression from fixes                       | cargo test gate on every RLCR iteration           |

## 7. Success Criteria

The project succeeds when:

1. machina runs the following command and reaches a shell prompt:
   ```bash
   machina -M virt -m 256M -nographic \
     -kernel arch/riscv/boot/Image \
     -initrd ./chytest/rootfs.cpio.gz \
     -append "console=ttyS0 earlycon"
   ```
2. Serial output matches QEMU 10.1.0 reference through kernel boot
3. Shell prompt appears and responds to `echo hello` with `hello`
4. All stage regression tests pass (`cargo test`)
5. No clippy warnings (`cargo clippy -- -D warnings`)
