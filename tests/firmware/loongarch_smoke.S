# LoongArch64 test-finisher smoke for the loongarch64-ref machine.
#
# loongarch64-ref has no architectural "test finished" signal, so this
# payload exercises a few integer ALU ops and then writes the PASS magic
# (0x5555) to the test-finisher MMIO register at 0x1FE0_8000, which makes
# machina terminate the run with exit code 0. A FAIL run would instead
# write 0x3333 (or fault before reaching the finisher).
#
# Loaded as a raw image: _start must be the first byte of the binary.
.section .text
.globl _start
_start:
    addi.d  $r10, $r0,  0x111   # r10 = 0x111
    addi.d  $r10, $r10, 0x222   # r10 = 0x333
    lu12i.w $r12, 0x1fe08       # r12 = 0x1FE0_8000 (test-finisher base)
    lu12i.w $r13, 0x5
    ori     $r13, $r13, 0x555   # r13 = 0x5555 (PASS magic)
    st.w    $r13, $r12, 0       # write PASS -> machina exits 0
hang:
    b       hang                # spin until the run loop observes stop
