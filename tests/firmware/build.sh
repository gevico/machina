#!/bin/bash
# Build the minimal S-mode test payload.
# Requires: riscv64-unknown-elf-as, riscv64-unknown-elf-ld,
# riscv64-unknown-elf-objcopy (from riscv-gnu-toolchain)
# OR: use LLVM tools (clang --target=riscv64)

set -e
cd "$(dirname "$0")"

# Try GNU toolchain first, then LLVM.
AS=riscv64-unknown-elf-as
LD=riscv64-unknown-elf-ld
OBJCOPY=riscv64-unknown-elf-objcopy

if ! command -v "$AS" &>/dev/null; then
    # Try clang/llvm
    AS="clang --target=riscv64 -march=rv64gc -c"
    LD="ld.lld"
    OBJCOPY="llvm-objcopy"
fi

$AS -o sbi_smoke.o sbi_smoke.S
$LD -T sbi_smoke.ld -o sbi_smoke.elf sbi_smoke.o
$OBJCOPY -O binary sbi_smoke.elf sbi_smoke.bin

echo "Built sbi_smoke.bin ($(wc -c < sbi_smoke.bin) bytes)"
