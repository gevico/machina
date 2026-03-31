# Minimal S-mode test payload for RustSBI Prototyper.
#
# This program runs in S-mode after Prototyper boot.
# It performs:
#   1. SBI get_spec_version (EID=0x10, FID=0)
#   2. Print "MACHINA_SBI_OK\n" via SBI console_putchar
#   3. SBI system_reset (EID=0x53525354, FID=0)
#
# Build:
#   riscv64-unknown-elf-as -o sbi_smoke.o sbi_smoke.S
#   riscv64-unknown-elf-ld -T sbi_smoke.ld -o sbi_smoke.elf sbi_smoke.o
#   riscv64-unknown-elf-objcopy -O binary sbi_smoke.elf sbi_smoke.bin
#
# Or with Rust toolchain:
#   Build as a no_std Rust binary targeting riscv64gc-unknown-none-elf

.section .text
.globl _start
_start:
    # 1. SBI get_spec_version (Base extension)
    #    a7 = EID = 0x10 (Base)
    #    a6 = FID = 0 (get_spec_version)
    li      a7, 0x10
    li      a6, 0
    ecall
    # a0 = error (0 = success), a1 = spec version

    # 2. Print marker string via legacy console_putchar
    #    a7 = 0x01 (legacy putchar), a0 = char
    la      t0, marker_str
    li      t1, 15          # string length
print_loop:
    beqz    t1, print_done
    lb      a0, 0(t0)
    li      a7, 0x01
    ecall
    addi    t0, t0, 1
    addi    t1, t1, -1
    j       print_loop
print_done:

    # 3. SBI system_reset (SRST extension)
    #    a7 = EID = 0x53525354
    #    a6 = FID = 0 (system_reset)
    #    a0 = reset_type = 0 (shutdown)
    #    a1 = reset_reason = 0 (no reason)
    li      a7, 0x53525354
    li      a6, 0
    li      a0, 0
    li      a1, 0
    ecall

    # Should not reach here.
hang:
    wfi
    j       hang

.section .rodata
marker_str:
    .ascii "MACHINA_SBI_OK\n"
