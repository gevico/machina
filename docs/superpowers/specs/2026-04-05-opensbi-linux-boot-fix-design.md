# machina OpenSBI + Linux Boot Fix — Phase 1

## Goal

Fix machina so OpenSBI can complete initialization and hand off to the
Linux kernel without crashing, by aligning two critical behaviors with
QEMU's virt machine: FDT placement and unmapped store handling.

## Root Cause Analysis

### Crash Trace

```
sbi_trap_error: hart0: trap0: store fault handler failed (error -3)
sbi_trap_error: hart0: trap0: mcause=0x0000000000000007 mtval=0x0000000090000015
sbi_trap_error: hart0: trap0: mepc=0x000000008000480e mstatus=0x8000000a00007800
```

OpenSBI (at PC 0x8000480e) triggers a store access fault when writing
to address 0x90000015, which is 21 bytes past the 256 MiB RAM boundary
(0x80000000–0x8FFFFFFF).

### Cause 1: FDT Placement

**QEMU** (hw/riscv/boot.c `riscv_compute_fdt_addr`):
```c
temp = (dram_base < 3072 * MiB) ? MIN(dram_end, 3072 * MiB) : dram_end;
return QEMU_ALIGN_DOWN(temp - fdtsize, 2 * MiB);
```
Result with 256 MiB: FDT at **0x8FE00000** (2 MiB from RAM top).

**machina** (hw/riscv/src/boot.rs):
```rust
let fdt_offset = (ram_size - fdt_len) & !0x7;
```
Result with 256 MiB: FDT at **~0x8FFFFF80** (~128 bytes from RAM top).

OpenSBI modifies the FDT during init (adds/enhances `/chosen` properties).
When the FDT is at the very top of RAM, modifications overflow past the RAM
boundary into unmapped address space.

### Cause 2: Unmapped Store Fault

**QEMU**: Writes to addresses with no MemoryRegion are silently ignored.
The MemoryRegion hierarchy acts as a filter — unmapped writes are no-ops.

**machina** (system/src/cpus.rs `machina_mem_write`):
```rust
if let Some(pa) = translate_for_helper(...) {
    if !is_phys_backed(cpu, pa, size) {
        cpu.mem_fault_cause = 7; // StoreAccessFault!
        cpu.mem_fault_tval = gva;
        return;
    }
}
```
When `is_phys_backed` returns false (address not backed by RAM or MMIO),
machina injects a StoreAccessFault into the guest. OpenSBI's own trap
handler cannot recover from this, leading to the crash.

Even after fixing FDT placement, OpenSBI may probe or write to other
unmapped addresses during platform detection. QEMU silently ignores
these; machina must do the same.

## Changes

### 1. FDT Placement — Match QEMU Algorithm

**File**: `hw/riscv/src/boot.rs`

Replace the current FDT placement logic:

```rust
// Current (broken):
let fdt_offset = (ram_size - fdt_len) & !0x7;
let fdt_addr = RAM_BASE + fdt_offset;
```

With QEMU-style 2 MiB-aligned placement:

```rust
// QEMU-style: place FDT 2MiB-aligned, well below RAM top.
let dram_end = RAM_BASE + ram_size;
let temp = if RAM_BASE < 0xC000_0000 {
    std::cmp::min(dram_end, 0xC000_0000)
} else {
    dram_end
};
let fdt_addr = (temp - fdt_len) & !0x1F_FFFF; // align down to 2MiB
```

This gives OpenSBI ~2 MiB of headroom between the FDT and the end of
RAM, matching QEMU's behavior exactly.

### 2. Silent Unmapped Writes for M-Mode

**File**: `system/src/cpus.rs`

In `machina_mem_write`, change the unmapped-store handling from
faulting to silently dropping:

```rust
// Current (crashes OpenSBI):
if !is_phys_backed(cpu, pa, size) {
    cpu.mem_fault_cause = 7;
    cpu.mem_fault_tval = gva;
    return;
}

// Fixed (matches QEMU):
if !is_phys_backed(cpu, pa, size) {
    return; // silently drop unmapped writes
}
```

Rationale: In QEMU, the MemoryRegion dispatch silently ignores writes
to unmapped regions. This is essential for firmware that probes the
address space. The RISC-V spec allows M-mode to access any address;
a real system would simply have no device at that address (BUS ERROR
or no-op depending on the bus). QEMU chooses no-op, and firmware
relies on this behavior.

### 3. Silent Unmapped Reads for M-Mode (Consistency)

In `machina_mem_read`, the current behavior for unmapped reads already
returns 0, which is correct. Verify this path also handles the
PMP/translate errors gracefully for M-mode.

## Testing

1. **Existing tests**: All 1138+ tests must continue to pass
2. **FDT placement test**: Verify FDT is at 2MiB-aligned address
3. **Smoke test**: Boot OpenSBI + Linux and verify no store fault:
   ```bash
   machina -m 256 -nographic \
     -bios <opensbi-fw_dynamic.bin> \
     -kernel <Image> \
     -append "console=ttyS0"
   ```
   Expected: OpenSBI completes init, shows full platform info (like QEMU),
   then attempts to jump to Linux kernel

## Files Changed

| File | Change |
|------|--------|
| `hw/riscv/src/boot.rs` | FDT placement: 2MiB-aligned |
| `system/src/cpus.rs` | Silent unmapped writes in `machina_mem_write` |

## Future Work (Phase 2+)

After Phase 1 enables OpenSBI to complete init:
- Add missing devices (RTC, FW_CFG)
- Investigate any remaining Linux kernel boot issues
- Consider ACLINT SSWI support for multi-hart IPI
