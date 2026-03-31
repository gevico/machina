/* Linker script for minimal S-mode test payload.
   Loaded at 0x80200000 (standard kernel entry). */

OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS {
    . = 0x80200000;
    .text : { *(.text) }
    .rodata : { *(.rodata) }
}
