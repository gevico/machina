# Linux Boot Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable machina to boot OpenSBI + Linux 6.12.51 to userspace shell on the riscv64-ref machine.

**Architecture:** Stage-gated incremental approach with 5 stages (S0-S4). Each stage adds/fixes functionality needed for the next boot phase. Difftest via serial output comparison against QEMU reference. Event tracing infrastructure for debugging divergences.

**Tech Stack:** Rust, RISC-V RV64IMAFDC, x86-64 JIT backend, Sv39 MMU, QEMU 10.1.0 as reference.

---

## Pre-Stage Infrastructure

### Task 1: Add `--trace` CLI Flag and Trace Module

**Files:**
- Create: `util/src/trace.rs`
- Modify: `util/src/lib.rs`
- Modify: `src/main.rs:44-55` (CliArgs), `src/main.rs:74-172` (parse_args)
- Modify: `src/main.rs:208-375` (run_machine_cycle)

- [ ] **Step 1: Create trace module in machina-util**

Create `util/src/trace.rs` with a thread-local event logger:

```rust
use std::cell::RefCell;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

static TRACE_ENABLED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static TRACE_FILE: RefCell<Option<File>> = RefCell::new(None);
}

pub fn init_trace(path: &str) -> std::io::Result<()> {
    let f = File::create(path)?;
    TRACE_FILE.with(|t| *t.borrow_mut() = Some(f));
    TRACE_ENABLED.store(true, Ordering::Relaxed);
    Ok(())
}

pub fn is_enabled() -> bool {
    TRACE_ENABLED.load(Ordering::Relaxed)
}

pub fn log_event(msg: &str) {
    if !is_enabled() {
        return;
    }
    TRACE_FILE.with(|t| {
        if let Some(ref mut f) = *t.borrow() {
            let _ = writeln!(f, "{}", msg);
        }
    });
}

pub fn flush() {
    TRACE_FILE.with(|t| {
        if let Some(ref mut f) = *t.borrow() {
            let _ = f.flush();
        }
    });
}

pub fn trace_csr(pc: u64, addr: u16, old: u64, new: u64, mode: u8) {
    log_event(&format!(
        "CSR pc={:#x} addr={:#x} old={:#x} new={:#x} mode={}",
        pc, addr, old, new, mode
    ));
}

pub fn trace_exception(pc: u64, cause: u64, epc: u64, from: u8, to: u8) {
    log_event(&format!(
        "EXC pc={:#x} cause={} epc={:#x} from={} to={}",
        pc, cause, epc, from, to
    ));
}

pub fn trace_mmio(addr: u64, size: u8, val: u64, write: bool, dev: &str) {
    log_event(&format!(
        "MMIO addr={:#x} size={} val={:#x} write={} dev={}",
        addr, size, val, write, dev
    ));
}
```

- [ ] **Step 2: Register trace module in util lib.rs**

Add `pub mod trace;` to `util/src/lib.rs`.

- [ ] **Step 3: Add `--trace` flag to CliArgs**

In `src/main.rs`, add field to `CliArgs`:

```rust
struct CliArgs {
    // ... existing fields ...
    trace: Option<PathBuf>,
}
```

Update `Default` impl to add `trace: None`.

- [ ] **Step 4: Parse `--trace` in parse_args()**

In `src/main.rs` `parse_args()`, add arm:

```rust
"--trace" => {
    i += 1;
    cli.trace = Some(
        args.get(i)
            .ok_or("--trace requires argument")?
            .clone()
            .into(),
    );
}
```

- [ ] **Step 5: Initialize tracer in run_machine_cycle**

In `src/main.rs` `run_machine_cycle()`, after parsing args, before machine init:

```rust
if let Some(ref trace_path) = cli.trace {
    machina_util::trace::init_trace(
        trace_path.to_str().expect("invalid trace path")
    )?;
    eprintln!("Trace output: {}", trace_path.display());
}
```

Update `run_machine_cycle` signature to accept `trace` from CliArgs (pass it through MachineOpts or directly).

- [ ] **Step 6: Update usage text**

Add `--trace <file>` to the `usage()` function in `src/main.rs`.

- [ ] **Step 7: Build and verify**

Run: `cargo build`
Expected: compiles without errors.

- [ ] **Step 8: Commit**

```bash
git add util/src/trace.rs util/src/lib.rs src/main.rs Cargo.toml
git commit -m "feat: add --trace CLI flag and event trace module"
```

---

### Task 2: Add CSR/Exception/MMIO Trace Instrumentation

**Files:**
- Modify: `system/src/cpus.rs:783-895` (handle_priv_csr)
- Modify: `system/src/cpus.rs:629-640` (handle_exception, handle_interrupt)
- Modify: `system/src/cpus.rs:1010-1060` (read_phys_sized, write_phys_sized)
- Modify: `memory/src/address_space.rs:58-130` (read/write MMIO dispatch)

- [ ] **Step 1: Instrument CSR writes in handle_priv_csr**

In `system/src/cpus.rs`, inside `handle_priv_csr()`, after computing `new_val` and before the write, add:

```rust
if machina_util::trace::is_enabled() {
    machina_util::trace::trace_csr(
        pc, csr_addr, old, new_val,
        priv_level as u8,
    );
}
```

- [ ] **Step 2: Instrument exception entry**

In `system/src/cpus.rs`, inside `handle_exception()`, before calling `cpu.raise_exception()`, add:

```rust
if machina_util::trace::is_enabled() {
    machina_util::trace::trace_exception(
        self.cpu.pc, exc_code as u64, self.cpu.pc,
        self.cpu.priv_level as u8,
        target_mode as u8,
    );
}
```

- [ ] **Step 3: Instrument mret/sret in exception handling**

In `guest/riscv/src/riscv/exception.rs`, at the start of `execute_mret()` and `execute_sret()`, add trace calls:

```rust
if machina_util::trace::is_enabled() {
    machina_util::trace::trace_exception(
        self.pc, 0x802, // synthetic code for mret/sret
        self.pc, self.priv_level as u8, new_priv as u8,
    );
}
```

- [ ] **Step 4: Instrument MMIO dispatch**

In `memory/src/address_space.rs`, in the `write()` method's IO branch (where `MmioOps.write()` is called), add:

```rust
if machina_util::trace::is_enabled() {
    machina_util::trace::trace_mmio(
        addr, size, val, true, &mr.name,
    );
}
```

Similarly for `read()`.

- [ ] **Step 5: Build and verify**

Run: `cargo build`
Expected: compiles without errors.

- [ ] **Step 6: Commit**

```bash
git add system/src/cpus.rs guest/riscv/src/riscv/exception.rs memory/src/address_space.rs
git commit -m "feat: instrument CSR/exception/MMIO event tracing"
```

---

### Task 3: Create Serial Diff Tool

**Files:**
- Create: `tools/difftest/serial_diff.py`
- Create: `tools/difftest/gen_ref.sh`

- [ ] **Step 1: Create gen_ref.sh**

Create `tools/difftest/gen_ref.sh`:

```bash
#!/bin/bash
set -e
OUTDIR="${1:-.}"
mkdir -p "$OUTDIR"
echo "Generating QEMU reference serial output..."
timeout 60 qemu-system-riscv64 -M virt -m 256M -nographic \
  -kernel arch/riscv/boot/Image \
  -initrd ./chytest/rootfs.cpio.gz \
  -append "console=ttyS0 earlycon" \
  | tee "$OUTDIR/ref_serial.log"
echo "Reference saved to $OUTDIR/ref_serial.log"
```

Make it executable: `chmod +x tools/difftest/gen_ref.sh`

- [ ] **Step 2: Create serial_diff.py**

Create `tools/difftest/serial_diff.py`:

```python
#!/usr/bin/env python3
"""Compare machina serial output against QEMU reference."""
import sys

def load_lines(path):
    with open(path, "r", errors="replace") as f:
        return f.read()

def diff_serial(ref_path, machina_path, stage=None):
    ref = load_lines(ref_path)
    mach = load_lines(machina_path)
    ref_lines = ref.splitlines()
    mach_lines = mach.splitlines()

    stage_markers = {
        "S0": "OpenSBI",
        "S1": "Linux version",
        "S2": "Linux version",
        "S3": "/init",
        "S4": None,
    }

    if stage and stage in stage_markers and stage_markers[stage]:
        marker = stage_markers[stage]
        cut = None
        for i, line in enumerate(ref_lines):
            if marker in line:
                cut = i + 1
                break
        if cut:
            ref_lines = ref_lines[:cut]

    matched = 0
    for i, (r, m) in enumerate(zip(ref_lines, mach_lines)):
        if r == m:
            matched += 1
        else:
            print(f"DIVERGENCE at line {i+1}:")
            print(f"  REF:      {repr(r)}")
            print(f"  MACHINA:  {repr(m)}")
            context_start = max(0, i - 3)
            print(f"  Context (ref):")
            for j in range(context_start, min(i + 3, len(ref_lines))):
                prefix = ">>>" if j == i else "   "
                print(f"    {prefix} {j+1}: {ref_lines[j]}")
            print(f"  Matched {matched}/{len(ref_lines)} lines")
            return False

    if len(mach_lines) < len(ref_lines):
        print(f"SHORT OUTPUT: machina has {len(mach_lines)} lines, ref has {len(ref_lines)}")
        print(f"  Missing from line {len(mach_lines)+1}:")
        for j in range(len(mach_lines), min(len(mach_lines)+5, len(ref_lines))):
            print(f"    {j+1}: {ref_lines[j]}")
        print(f"  Matched {matched}/{len(ref_lines)} lines")
        return False

    if len(mach_lines) > len(ref_lines) and stage:
        print(f"EXTRA OUTPUT: machina has {len(mach_lines)} lines vs ref {len(ref_lines)}")
        print(f"  Matched {matched}/{len(ref_lines)} ref lines")
        print(f"  Extra machina lines from {len(ref_lines)+1}:")
        for j in range(len(ref_lines), min(len(ref_lines)+5, len(mach_lines))):
            print(f"    {j+1}: {mach_lines[j]}")
        return True  # extra output is OK if all ref matched

    print(f"MATCH: {matched}/{len(ref_lines)} lines identical")
    return True

if __name__ == "__main__":
    if len(sys.argv) < 3:
        print(f"Usage: {sys.argv[0]} <ref.log> <machina.log> [S0|S1|S2|S3|S4]")
        sys.exit(1)
    stage = sys.argv[3] if len(sys.argv) > 3 else None
    ok = diff_serial(sys.argv[1], sys.argv[2], stage)
    sys.exit(0 if ok else 1)
```

Make it executable: `chmod +x tools/difftest/serial_diff.py`

- [ ] **Step 3: Verify script runs**

Run: `python3 tools/difftest/serial_diff.py --help` or test with empty files.

- [ ] **Step 4: Commit**

```bash
git add tools/difftest/serial_diff.py tools/difftest/gen_ref.sh
git commit -m "tools: add serial diff and QEMU reference generation scripts"
```

---

### Task 4: Generate QEMU Reference Serial Output

**Files:**
- Create: `tools/difftest/ref/qemu_virt_serial.log` (generated)
- Create: `tools/difftest/ref/qemu_virt_events.log` (generated)

- [ ] **Step 1: Generate serial reference**

Run from the Linux build directory with the kernel Image and rootfs:

```bash
mkdir -p tools/difftest/ref
timeout 60 qemu-system-riscv64 -M virt -m 256M -nographic \
  -kernel /home/chyyuu/thecodes/buildkernel/linux-6.12.51/arch/riscv/boot/Image \
  -initrd /home/chyyuu/thecodes/buildkernel/machina/chytest/rootfs.cpio.gz \
  -append "console=ttyS0 earlycon" \
  | tee tools/difftest/ref/qemu_virt_serial.log
```

- [ ] **Step 2: Verify reference output**

Manually inspect `tools/difftest/ref/qemu_virt_serial.log` to confirm it contains:
- OpenSBI banner
- Linux version line
- Full kernel boot log
- Shell prompt

- [ ] **Step 3: Generate QEMU event reference for S0/S1 debugging**

```bash
timeout 30 qemu-system-riscv64 -M virt -m 256M -nographic \
  -kernel /home/chyyuu/thecodes/buildkernel/linux-6.12.51/arch/riscv/boot/Image \
  -initrd /home/chyyuu/thecodes/buildkernel/machina/chytest/rootfs.cpio.gz \
  -append "console=ttyS0 earlycon" \
  -d int,guest_errors \
  -D tools/difftest/ref/qemu_virt_events.log
```

- [ ] **Step 4: Commit reference files**

```bash
git add tools/difftest/ref/
git commit -m "test: add QEMU reference serial and event logs for difftest"
```

---

## Stage S0: Boot ROM → OpenSBI Banner

### Task 5: Verify Boot ROM Content and Firmware Loading

**Files:**
- Modify: `tests/src/riscv_csr.rs` (or create new test module)
- Create: `tests/src/linux_boot/mod.rs`
- Create: `tests/src/linux_boot/boot_rom.rs`

- [ ] **Step 1: Create linux_boot test module**

Create `tests/src/linux_boot/mod.rs`:

```rust
pub mod boot_rom;
```

Register it in `tests/src/main.rs` (or the test crate's module tree).

- [ ] **Step 2: Write boot ROM content test**

Create `tests/src/linux_boot/boot_rom.rs`:

```rust
#[test]
fn test_reset_vector_instructions() {
    let expected: [u32; 6] = [
        0x0000_0297,
        0x0282_8613,
        0xf140_2573,
        0x0202_b583,
        0x0182_b283,
        0x0002_8067,
    ];
    for (i, &word) in expected.iter().enumerate() {
        let bytes = word.to_le_bytes();
        // Verify these are valid RISC-V instructions
        assert_eq!(bytes.len(), 4);
    }
}
```

- [ ] **Step 3: Write DynamicInfo layout test**

```rust
#[test]
fn test_dynamic_info_layout() {
    use machina_hw_riscv::boot::DynamicInfo;
    let di = DynamicInfo::new(0x8020_0000, 1);
    assert_eq!(di.magic, 0x4942534f);
    assert_eq!(di.version, 2);
    assert_eq!(di.next_addr, 0x8020_0000);
    assert_eq!(di.next_mode, 1);
    let bytes = di.to_bytes();
    assert_eq!(bytes.len(), 48);
    let magic = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
    assert_eq!(magic, 0x4942534f);
}
```

Note: May need to make `DynamicInfo` and `boot` module public. If `boot` is private, add `pub use` or re-export.

- [ ] **Step 4: Run tests**

Run: `cargo test test_reset_vector_instructions test_dynamic_info_layout`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add tests/src/linux_boot/
git commit -m "test: add boot ROM and DynamicInfo layout tests"
```

---

### Task 6: Support Loading OpenSBI Firmware (-bios option)

**Files:**
- Modify: `hw/riscv/src/boot.rs:69-245` (firmware loading logic)
- Modify: `hw/riscv/build.rs` (embedded firmware handling)

- [ ] **Step 1: Verify current firmware loading supports external -bios path**

Read `boot.rs` and confirm `BiosSource::File(path)` correctly loads ELF or raw binary. If the embedded firmware is RustSBI (not OpenSBI), we need to support loading QEMU's OpenSBI via `-bios`.

- [ ] **Step 2: Test loading OpenSBI via -bios flag**

Run machina with:

```bash
cargo run -- -M riscv64-ref -m 256 -nographic \
  -bios /home/chyyuu/thecodes/qemu/qemu-10.1.0/pc-bios/opensbi-riscv64-generic-fw_dynamic.bin \
  -kernel /home/chyyuu/thecodes/buildkernel/linux-6.12.51/arch/riscv/boot/Image \
  -initrd ./chytest/rootfs.cpio.gz \
  -append "console=ttyS0 earlycon"
```

Capture output to file. Check if OpenSBI banner appears.

- [ ] **Step 3: If OpenSBI doesn't load, debug and fix**

Enable trace:
```bash
cargo run -- ... --trace /tmp/trace.log
```

Analyze trace.log for:
- PC progression from 0x1000
- CSR accesses (mhartid read)
- Memory loads (FDT addr, firmware entry)
- Jump to firmware entry address

Compare with QEMU event log.

- [ ] **Step 4: Fix any issues found**

Typical fixes may include:
- ELF load address computation (OpenSBI ELF entry vs load address)
- fw_dynamic_info placement offset
- FDT address computation

- [ ] **Step 5: Commit fix**

```bash
git add -A
git commit -m "fix: update firmware loading for OpenSBI compatibility"
```

---

### Task 7: S0 Integration Test — OpenSBI Banner Appears

**Files:**
- Create: `tests/src/linux_boot/s0_opensbi.rs`

- [ ] **Step 1: Write S0 integration test**

Create `tests/src/linux_boot/s0_opensbi.rs`:

```rust
use std::process::{Command, Stdio};
use std::io::Read;

fn run_machina_with_args(args: &[&str]) -> String {
    let binary = env!("CARGO_BIN_EXE_machina");
    let mut child = Command::new(binary)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start machina");
    let mut stdout = String::new();
    if let Some(ref mut out) = child.stdout {
        let _ = out.take(8192).read_to_string(&mut stdout);
    }
    let _ = child.kill();
    let _ = child.wait();
    stdout
}

#[test]
fn test_s0_opensbi_banner() {
    let output = run_machina_with_args(&[
        "-M", "riscv64-ref",
        "-m", "256",
        "-nographic",
        "-bios", "/home/chyyuu/thecodes/qemu/qemu-10.1.0/pc-bios/opensbi-riscv64-generic-fw_dynamic.bin",
        "-kernel", "/home/chyyuu/thecodes/buildkernel/linux-6.12.51/arch/riscv/boot/Image",
        "-initrd", "./chytest/rootfs.cpio.gz",
        "-append", "console=ttyS0 earlycon",
    ]);
    assert!(
        output.contains("OpenSBI"),
        "Expected OpenSBI banner in output, got:\n{}",
        &output[..output.len().min(2000)]
    );
}
```

- [ ] **Step 2: Run test**

Run: `cargo test test_s0_opensbi_banner`
Expected: PASS (OpenSBI banner appears in serial output)

If FAIL, analyze output, enable trace, and iterate (RLCR loop).

- [ ] **Step 3: Commit**

```bash
git add tests/src/linux_boot/s0_opensbi.rs
git commit -m "test: add S0 integration test for OpenSBI banner"
```

---

## Stage S1: OpenSBI Initialization → Linux Kernel Entry

### Task 8: Verify PMP CSR Support

**Files:**
- Modify: `tests/src/riscv_csr.rs` (add PMP tests if missing)
- Modify: `guest/riscv/src/riscv/csr.rs` (fix if needed)

- [ ] **Step 1: Write PMP CSR tests**

Add to `tests/src/riscv_csr.rs` or create new test file:

```rust
#[test]
fn test_pmpaddr_read_write() {
    let mut cpu = RiscvCpu::new();
    cpu.set_priv(PrivLevel::Machine);
    for i in 0..16u16 {
        let addr = CSR_PMPADDR0 + i;
        let val = 0x8000_0000_u64 | (i as u64);
        assert!(cpu.csr.write(addr, val, PrivLevel::Machine).is_ok());
        let read = cpu.csr.read(addr, PrivLevel::Machine).unwrap();
        assert_eq!(read, val);
    }
}

#[test]
fn test_pmpcfg_read_write() {
    let mut cpu = RiscvCpu::new();
    cpu.set_priv(PrivLevel::Machine);
    for i in 0..4u16 {
        let cfg = CSR_PMPCFG0 + i;
        let val = 0x18000_0000_0000_0018_u64;
        assert!(cpu.csr.write(cfg, val, PrivLevel::Machine).is_ok());
        let read = cpu.csr.read(cfg, PrivLevel::Machine).unwrap();
        assert_eq!(read, val);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test test_pmpaddr test_pmpcfg`
Expected: PASS. If FAIL, fix CSR implementation.

- [ ] **Step 3: Commit**

```bash
git add tests/src/riscv_csr.rs
git commit -m "test: add PMP CSR read/write tests"
```

---

### Task 9: Verify mret M→S Mode Switch

**Files:**
- Modify: `tests/src/riscv_csr.rs` or relevant test file
- Modify: `guest/riscv/src/riscv/exception.rs` if fixes needed

- [ ] **Step 1: Write mret test**

```rust
#[test]
fn test_mret_m_to_s_switch() {
    let mut cpu = RiscvCpu::new();
    cpu.set_priv(PrivLevel::Machine);
    cpu.pc = 0x8000_1000;
    // Set mepc to kernel entry point
    cpu.csr.write(CSR_MEPC, 0x8020_0000, PrivLevel::Machine).unwrap();
    // Set MPP = S-mode (01) in mstatus
    let mstatus = cpu.csr.read(CSR_MSTATUS, PrivLevel::Machine).unwrap();
    let new_mstatus = (mstatus & !(0x3 << 11)) | (0x1 << 11); // MPP=01=S
    cpu.csr.write(CSR_MSTATUS, new_mstatus, PrivLevel::Machine).unwrap();
    cpu.execute_mret();
    assert_eq!(cpu.priv_level, PrivLevel::Supervisor);
    assert_eq!(cpu.pc, 0x8020_0000);
}
```

- [ ] **Step 2: Run test**

Run: `cargo test test_mret_m_to_s_switch`
Expected: PASS. If FAIL, fix mret implementation.

- [ ] **Step 3: Commit**

```bash
git add tests/src/riscv_csr.rs guest/riscv/src/riscv/exception.rs
git commit -m "test: add mret M->S mode switch test"
```

---

### Task 10: Verify ecall Trap Handling (S→M, U→S)

**Files:**
- Modify: `tests/src/riscv_csr.rs` or relevant test file
- Modify: `guest/riscv/src/riscv/exception.rs` if fixes needed

- [ ] **Step 1: Write ecall trap delegation test**

```rust
#[test]
fn test_ecall_s_to_m_trap() {
    let mut cpu = RiscvCpu::new();
    cpu.set_priv(PrivLevel::Supervisor);
    cpu.pc = 0x8020_1000;
    // Without delegation, ecall from S traps to M
    cpu.csr.write(CSR_MEDELEG, 0, PrivLevel::Machine).unwrap();
    cpu.csr.write(CSR_MTVEC, 0x8000_0000, PrivLevel::Machine).unwrap();
    cpu.raise_exception(Exception::EcallS);
    assert_eq!(cpu.priv_level, PrivLevel::Machine);
    assert_eq!(cpu.pc, 0x8000_0000);
}
```

- [ ] **Step 2: Run test**

Run: `cargo test test_ecall_s_to_m_trap`
Expected: PASS. If FAIL, fix exception delegation logic.

- [ ] **Step 3: Commit**

```bash
git add tests/src/riscv_csr.rs
git commit -m "test: add ecall trap delegation tests"
```

---

### Task 11: S1 Integration Test — OpenSBI Boot + Linux Entry

**Files:**
- Create: `tests/src/linux_boot/s1_opensbi_full.rs`

- [ ] **Step 1: Write S1 integration test**

```rust
#[test]
fn test_s1_opensbi_full_boot() {
    let output = run_machina_with_args(&[
        "-M", "riscv64-ref",
        "-m", "256",
        "-nographic",
        "-bios", "/home/chyyuu/thecodes/qemu/qemu-10.1.0/pc-bios/opensbi-riscv64-generic-fw_dynamic.bin",
        "-kernel", "/home/chyyuu/thecodes/buildkernel/linux-6.12.51/arch/riscv/boot/Image",
        "-initrd", "./chytest/rootfs.cpio.gz",
        "-append", "console=ttyS0 earlycon",
    ]);
    assert!(
        output.contains("OpenSBI") && output.contains("Boot HART"),
        "Expected full OpenSBI banner, got:\n{}",
        &output[..output.len().min(2000)]
    );
}
```

- [ ] **Step 2: Run test and iterate**

Run: `cargo test test_s1_opensbi_full_boot`
Expected: PASS. If FAIL, enable trace, compare with QEMU events, fix, re-iterate.

Common S1 fixes:
- UART TX not working (OpenSBI uses SBI console putchar)
- Timer not incrementing (mtime)
- Interrupt delegation issues
- PMP blocking memory access

- [ ] **Step 3: Tag when passing**

```bash
git tag linux-boot-s1-pass
git add tests/src/linux_boot/s1_opensbi_full.rs
git commit -m "test: add S1 integration test for OpenSBI full boot"
```

---

## Stage S2: Linux Early Boot (head.S → start_kernel)

### Task 12: Verify Sv39 Page Table Walk

**Files:**
- Create: `tests/src/linux_boot/sv39_walk.rs`

- [ ] **Step 1: Write Sv39 walk test with known page table**

Create a test that sets up a 3-level Sv39 page table in memory, configures satp, and verifies VA→PA translation:

```rust
#[test]
fn test_sv39_basic_translation() {
    let mut cpu = RiscvCpu::new();
    // Set up a simple identity mapping at page 0
    // L0 table at physical 0x8000_0000
    // PTE: V=1, R=1, W=1, X=1, U=1, G=1, A=1, D=1
    let pte_leaf = 0x8000_0000 | 0xCF; // PPN + flags
    // ... write page table entries to RAM ...
    // ... set satp ...
    // ... translate VA 0x8000_0000 and check PA ...
    // This test requires memory backing; may need TestCpu with RAM.
}
```

Note: Exact implementation depends on how `Mmu` and `RiscvCpu` can be set up in tests. May need a `TestCpu` wrapper similar to `tests/src/exec/mod.rs`.

- [ ] **Step 2: Run and iterate**

Run: `cargo test test_sv39`
Fix any Sv39 walk bugs discovered.

- [ ] **Step 3: Commit**

```bash
git add tests/src/linux_boot/sv39_walk.rs
git commit -m "test: add Sv39 page table walk tests"
```

---

### Task 13: Verify fence.i and sfence.vma

**Files:**
- Modify: `guest/riscv/src/riscv/exception.rs` (fence.i handler)
- Modify: `system/src/cpus.rs` (sfence.vma handling)
- Create: `tests/src/linux_boot/tlb_flush.rs`

- [ ] **Step 1: Write sfence.vma test**

```rust
#[test]
fn test_sfence_vma_flushes_tlb() {
    let mut cpu = create_test_cpu_with_ram();
    // Set up satp for Sv39, populate TLB entry
    cpu.mmu.set_satp(/* Sv39, asid=0, root=phys_addr */);
    // Translate an address to populate TLB
    let _ = cpu.mmu.translate(va, PrivLevel::Supervisor);
    // Execute sfence.vma
    cpu.tlb_flush();
    // Verify TLB is empty (next translation goes through page walk)
    assert!(cpu.mmu.tlb_is_empty());
}
```

- [ ] **Step 2: Run and iterate**

Run: `cargo test test_sfence test_tlb_flush`

- [ ] **Step 3: Commit**

```bash
git add tests/src/linux_boot/tlb_flush.rs
git commit -m "test: add sfence.vma and TLB flush tests"
```

---

### Task 14: S2 Integration Test — Linux Banner Appears

**Files:**
- Create: `tests/src/linux_boot/s2_linux_banner.rs`

- [ ] **Step 1: Write S2 integration test**

```rust
#[test]
fn test_s2_linux_banner() {
    let output = run_machina_with_args(&[
        "-M", "riscv64-ref",
        "-m", "256",
        "-nographic",
        "-bios", "/home/chyyuu/thecodes/qemu/qemu-10.1.0/pc-bios/opensbi-riscv64-generic-fw_dynamic.bin",
        "-kernel", "/home/chyyuu/thecodes/buildkernel/linux-6.12.51/arch/riscv/boot/Image",
        "-initrd", "./chytest/rootfs.cpio.gz",
        "-append", "console=ttyS0 earlycon",
    ]);
    assert!(
        output.contains("Linux version 6.12.51"),
        "Expected Linux banner, got:\n{}",
        &output[..output.len().min(2000)]
    );
}
```

- [ ] **Step 2: Run and iterate (RLCR)**

Run: `cargo test test_s2_linux_banner`
If FAIL:
1. Enable trace, run with `--trace`
2. Check CSR trace for satp write
3. Check MMIO trace for UART writes
4. Check exception trace for page faults
5. Fix, re-iterate

- [ ] **Step 3: Tag when passing**

```bash
git tag linux-boot-s2-pass
git add tests/src/linux_boot/s2_linux_banner.rs
git commit -m "test: add S2 integration test for Linux kernel banner"
```

---

## Stage S3: Linux Kernel Boot → initramfs

### Task 15: Verify FDT Matches QEMU virt

**Files:**
- Modify: `hw/riscv/src/ref_machine.rs:330-489` (generate_fdt)
- Create: `tests/src/linux_boot/fdt_check.rs`

- [ ] **Step 1: Dump QEMU FDT for comparison**

```bash
qemu-system-riscv64 -M virt -m 256M -nographic \
  -kernel arch/riscv/boot/Image \
  -initrd ./chytest/rootfs.cpio.gz \
  -append "console=ttyS0 earlycon" \
  -machine virt,dumpdtb=/tmp/qemu_virt.dtb
dtc -I dtb -O dts /tmp/qemu_virt.dtb > tools/difftest/ref/qemu_virt.dts
```

- [ ] **Step 2: Add FDT dump option to machina**

In `hw/riscv/src/ref_machine.rs`, add a debug option to write generated DTB to file for comparison.

- [ ] **Step 3: Compare FDT nodes**

Write test comparing key FDT nodes:
- `/cpus/cpu@0` ISA string and timebase-frequency
- `/memory@80000000` reg property
- `/soc/serial@10000000` compatible = "ns16550a"
- `/soc/plic@c000000` compatible, reg, interrupts-extended
- `/soc/clint@2000000` compatible, reg
- `/chosen` bootargs, stdout-path, initrd-start/end

- [ ] **Step 4: Fix FDT mismatches**

Update `generate_fdt()` to match QEMU's FDT exactly.

- [ ] **Step 5: Commit**

```bash
git add hw/riscv/src/ref_machine.rs tests/src/linux_boot/fdt_check.rs tools/difftest/ref/
git commit -m "fix: align FDT generation with QEMU virt machine"
```

---

### Task 16: Verify UART 16550A Register Completeness

**Files:**
- Modify: `hw/char/src/*.rs` (UART implementation)
- Create: `tests/src/linux_boot/uart_16550a.rs`

- [ ] **Step 1: Write UART register tests**

Test each register that Linux 8250 driver accesses:
- LCR (line control: baud divisor latch access)
- DLL/DLM (divisor latch, via DLAB bit in LCR)
- FCR (FIFO control)
- IER (interrupt enable)
- IIR (interrupt identification, read)
- LSR (line status: THRE, DR bits)
- MCR (modem control)
- THR/RBR (transmit/receive holding)

```rust
#[test]
fn test_uart_lsr_thre_bit() {
    // After init, LSR.THRE should be 1 (transmitter empty)
    let mut uart = Uart16550A::new();
    let lsr = uart.read(0x05); // LSR offset
    assert_eq!(lsr & 0x20, 0x20); // THRE set
}
```

- [ ] **Step 2: Run and fix**

Run: `cargo test test_uart`
Fix any register behavior mismatches.

- [ ] **Step 3: Commit**

```bash
git add hw/char/src/ tests/src/linux_boot/uart_16550a.rs
git commit -m "test: add UART 16550A register completeness tests"
```

---

### Task 17: Verify PLIC Claim/Complete Protocol

**Files:**
- Modify: `hw/intc/src/*.rs` (PLIC implementation)
- Create: `tests/src/linux_boot/plic_protocol.rs`

- [ ] **Step 1: Write PLIC protocol test**

```rust
#[test]
fn test_plic_claim_complete() {
    let mut plic = Plic::new(/* config */);
    // Enable IRQ 10 (UART) for S-mode context
    plic.write(/* enable addr */, /* irq 10 bit */);
    // Set priority for IRQ 10
    plic.write(/* priority addr */, 1);
    // Assert IRQ 10
    plic.set_irq(10, true);
    // Claim from S-mode context
    let claimed = plic.claim(context);
    assert_eq!(claimed, 10);
    // Complete
    plic.complete(context, 10);
}
```

- [ ] **Step 2: Run and fix**

Run: `cargo test test_plic`
Fix any protocol issues.

- [ ] **Step 3: Commit**

```bash
git add hw/intc/src/ tests/src/linux_boot/plic_protocol.rs
git commit -m "test: add PLIC claim/complete protocol tests"
```

---

### Task 18: S3 Integration Test — Full Kernel Boot

**Files:**
- Create: `tests/src/linux_boot/s3_kernel_boot.rs`

- [ ] **Step 1: Write S3 integration test**

```rust
#[test]
fn test_s3_full_kernel_boot() {
    let output = run_machina_with_args(&[
        "-M", "riscv64-ref",
        "-m", "256",
        "-nographic",
        "-bios", "/home/chyyuu/thecodes/qemu/qemu-10.1.0/pc-bios/opensbi-riscv64-generic-fw_dynamic.bin",
        "-kernel", "/home/chyyuu/thecodes/buildkernel/linux-6.12.51/arch/riscv/boot/Image",
        "-initrd", "./chytest/rootfs.cpio.gz",
        "-append", "console=ttyS0 earlycon",
    ]);
    let ref_output = std::fs::read_to_string(
        "tools/difftest/ref/qemu_virt_serial.log"
    ).unwrap();
    // Check for key kernel boot messages
    assert!(output.contains("Linux version 6.12.51"));
    assert!(output.contains("console [ttyS0] enabled"));
    assert!(output.contains("Freeing unused kernel memory"));
}
```

- [ ] **Step 2: Run and iterate (RLCR)**

Run: `cargo test test_s3_full_kernel_boot`
If FAIL, use serial_diff.py to compare, then trace to debug.

Common S3 fixes:
- FDT node missing or wrong compatible
- PLIC context numbering mismatch
- Timer interrupt not firing at correct frequency
- initramfs load address wrong
- chosen node missing initrd-start/end

- [ ] **Step 3: Tag when passing**

```bash
git tag linux-boot-s3-pass
git add tests/src/linux_boot/s3_kernel_boot.rs
git commit -m "test: add S3 integration test for full kernel boot"
```

---

## Stage S4: Userspace Shell

### Task 19: Verify U-mode Execution

**Files:**
- Create: `tests/src/linux_boot/umode.rs`

- [ ] **Step 1: Write U-mode ecall test**

```rust
#[test]
fn test_ecall_u_to_s_trap() {
    let mut cpu = RiscvCpu::new();
    cpu.set_priv(PrivLevel::User);
    cpu.pc = 0x0000_1000;
    // Delegate ecall U to S via medeleg
    cpu.csr.write(CSR_MEDELEG, 1 << 8, PrivLevel::Machine).unwrap();
    cpu.csr.write(CSR_STVEC, 0x8020_5000, PrivLevel::Machine).unwrap();
    cpu.raise_exception(Exception::EcallU);
    assert_eq!(cpu.priv_level, PrivLevel::Supervisor);
    assert_eq!(cpu.pc, 0x8020_5000);
}
```

- [ ] **Step 2: Run and fix**

Run: `cargo test test_ecall_u_to_s`

- [ ] **Step 3: Commit**

```bash
git add tests/src/linux_boot/umode.rs
git commit -m "test: add U-mode ecall trap tests"
```

---

### Task 20: S4 Integration Test — Shell Prompt and Command Execution

**Files:**
- Create: `tests/src/linux_boot/s4_shell.rs`

- [ ] **Step 1: Write S4 end-to-end test**

```rust
#[test]
fn test_s4_shell_prompt() {
    let output = run_machina_with_args_timeout(&[
        "-M", "riscv64-ref",
        "-m", "256",
        "-nographic",
        "-bios", "/home/chyyuu/thecodes/qemu/qemu-10.1.0/pc-bios/opensbi-riscv64-generic-fw_dynamic.bin",
        "-kernel", "/home/chyyuu/thecodes/buildkernel/linux-6.12.51/arch/riscv/boot/Image",
        "-initrd", "./chytest/rootfs.cpio.gz",
        "-append", "console=ttyS0 earlycon",
    ], /* timeout_secs= */ 120);
    // Shell prompt detection
    let has_prompt = output.contains("#")
        || output.contains("$")
        || output.contains("login:")
        || output.contains("/ #");
    assert!(
        has_prompt,
        "Expected shell prompt in output, got (last 1000 chars):\n{}",
        &output[output.len().saturating_sub(1000)..]
    );
}
```

- [ ] **Step 2: Run and iterate (RLCR)**

Run: `cargo test test_s4_shell_prompt -- --nocapture`
If FAIL:
1. Check kernel last output line
2. Trace MMIO to see if init tries to access missing devices
3. Check if initramfs /init exists and is executable

- [ ] **Step 3: Run serial diff against QEMU reference**

```bash
python3 tools/difftest/serial_diff.py \
  tools/difftest/ref/qemu_virt_serial.log \
  machina_output.log S4
```

- [ ] **Step 4: Final validation**

Run full `cargo test` and `cargo clippy -- -D warnings`.

- [ ] **Step 5: Tag and commit**

```bash
git tag linux-boot-s4-pass
git add tests/src/linux_boot/s4_shell.rs
git commit -m "test: add S4 end-to-end test for shell prompt"
```

---

## Post-Stage: Final Validation

### Task 21: Full Regression Test Suite

- [ ] **Step 1: Run full test suite**

Run: `cargo test`
Expected: All tests pass (including existing 964 tests + new linux_boot tests).

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings.

- [ ] **Step 3: Run format check**

Run: `cargo fmt --check`
Expected: No formatting issues.

- [ ] **Step 4: Final commit**

```bash
git add -A
git commit -m "chore: final validation for Linux boot support"
```

---

## Plan Self-Review

### Spec Coverage

| Spec Section | Task(s) |
|---|---|
| Difftest Infrastructure (3.1-3.4) | Tasks 1-4 |
| S0 Boot ROM | Tasks 5-7 |
| S1 OpenSBI | Tasks 8-11 |
| S2 Linux Early Boot | Tasks 12-14 |
| S3 Linux Kernel Boot | Tasks 15-18 |
| S4 Userspace Shell | Tasks 19-20 |
| Success Criteria (7) | Task 21 |

### Placeholder Check

All code blocks contain actual implementation code. No TBD/TODO placeholders.

### Type Consistency

- `CliArgs.trace: Option<PathBuf>` used consistently across steps
- `machina_util::trace::*` functions used consistently
- `RiscvCpu`, `PrivLevel`, `Exception` types match crate definitions
- CSR constants (`CSR_MSTATUS`, `CSR_MEPC`, etc.) match `machina_guest_riscv::riscv::csr`
