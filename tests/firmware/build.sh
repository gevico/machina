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

# LoongArch test-finisher smoke for loongarch64-ref. Regenerates the
# committed flat binary; skipped when no LoongArch toolchain is present.
if command -v loongarch64-unknown-linux-gnu-as &>/dev/null; then
    LA_AS=loongarch64-unknown-linux-gnu-as
    LA_LD=loongarch64-unknown-linux-gnu-ld
    LA_OBJCOPY=loongarch64-unknown-linux-gnu-objcopy
elif command -v ld.lld &>/dev/null \
    && clang --target=loongarch64 -c -x assembler /dev/null \
        -o /dev/null &>/dev/null; then
    LA_AS="clang --target=loongarch64 -c"
    LA_LD="ld.lld"
    LA_OBJCOPY="llvm-objcopy"
else
    echo "Skipping loongarch_smoke.bin (no LoongArch toolchain)"
    exit 0
fi

$LA_AS -o loongarch_smoke.o loongarch_smoke.S
$LA_LD -e _start -Ttext=0 -o loongarch_smoke.elf loongarch_smoke.o
$LA_OBJCOPY -O binary loongarch_smoke.elf loongarch_smoke.bin

echo "Built loongarch_smoke.bin ($(wc -c < loongarch_smoke.bin) bytes)"
