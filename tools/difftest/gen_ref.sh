#!/usr/bin/env bash
# Generate QEMU reference serial log for difftest.
#
# Usage: ./gen_ref.sh <kernel-image> <initrd> [output]
#
# Requires: qemu-system-riscv64 in PATH.

set -euo pipefail

KERNEL="${1:?Usage: gen_ref.sh <kernel> <initrd> [output]}"
INITRD="${2:?Usage: gen_ref.sh <kernel> <initrd> [output]}"
OUTPUT="${3:-ref/ref_serial.log}"

timeout 120 qemu-system-riscv64 \
    -M virt -m 256M -nographic \
    -kernel "$KERNEL" \
    -initrd "$INITRD" \
    -append "console=ttyS0 earlycon" \
    2>/dev/null | tee "$OUTPUT"

echo ""
echo "Reference log saved to $OUTPUT"
