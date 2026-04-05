#!/bin/bash
set -e
OUTDIR="${1:-.}"
mkdir -p "$OUTDIR"
KERNEL="/home/chyyuu/thecodes/buildkernel/linux-6.12.51/arch/riscv/boot/Image"
INITRD="/home/chyyuu/thecodes/buildkernel/machina/chytest/rootfs.cpio.gz"
echo "Generating QEMU reference serial output..."
timeout 60 qemu-system-riscv64 -M virt -m 256M -nographic \
  -kernel "$KERNEL" \
  -initrd "$INITRD" \
  -append "console=ttyS0 earlycon" \
  | tee "$OUTDIR/ref_serial.log"
echo "Reference saved to $OUTDIR/ref_serial.log"
