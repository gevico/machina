//! C-callable helper functions for RISC-V instruction
//! translation.  These are invoked from JIT-generated
//! code to implement operations with special-case
//! semantics that cannot be expressed in pure IR.

// ── C-callable signed division helpers ──────────────
// Called from JIT code to implement RISC-V signed
// div/rem with the correct special-case semantics.

/// RV64 signed division: div-by-zero -> -1,
/// MIN/-1 -> MIN.
#[no_mangle]
pub extern "C" fn helper_divs64(a: i64, b: i64) -> i64 {
    if b == 0 {
        -1
    } else if a == i64::MIN && b == -1 {
        i64::MIN
    } else {
        a.wrapping_div(b)
    }
}

/// RV64 signed remainder: div-by-zero -> a,
/// MIN/-1 -> 0.
#[no_mangle]
pub extern "C" fn helper_rems64(a: i64, b: i64) -> i64 {
    if b == 0 {
        a
    } else if a == i64::MIN && b == -1 {
        0
    } else {
        a.wrapping_rem(b)
    }
}

/// RV64 DIVW: 32-bit signed div, sign-extended to 64.
#[no_mangle]
pub extern "C" fn helper_divw64(a: i64, b: i64) -> i64 {
    let a32 = a as i32;
    let b32 = b as i32;
    let r = if b32 == 0 {
        -1i32
    } else if a32 == i32::MIN && b32 == -1 {
        i32::MIN
    } else {
        a32.wrapping_div(b32)
    };
    r as i64 // sign-extend
}

/// RV64 REMW: 32-bit signed rem, sign-extended to 64.
#[no_mangle]
pub extern "C" fn helper_remw64(a: i64, b: i64) -> i64 {
    let a32 = a as i32;
    let b32 = b as i32;
    let r = if b32 == 0 {
        a32
    } else if a32 == i32::MIN && b32 == -1 {
        0i32
    } else {
        a32.wrapping_rem(b32)
    };
    r as i64 // sign-extend
}

/// SC helper: check reservation, conditionally store.
/// Returns 0 on success, 1 on failure.
///
/// Uses TLB addend for address translation, matching
/// how qemu_ld/qemu_st resolve guest virtual addresses.
#[no_mangle]
pub extern "C" fn helper_sc(
    env: *mut super::super::cpu::RiscvCpu,
    addr: u64,
    val: u64,
    size: u64,
) -> u64 {
    let cpu = unsafe { &mut *env };
    if cpu.load_res != addr {
        cpu.load_res = u64::MAX;
        return 1;
    }
    // Look up TLB for the write addend. The LR that
    // set load_res already populated the TLB entry.
    let idx = super::super::mmu::tlb_index(addr);
    let entry = cpu.mmu.tlb[idx];
    let tag = addr & super::super::mmu::PAGE_MASK;
    let addend = if entry.addr_write == tag
        && entry.addend != super::super::mmu::TLB_MMIO_ADDEND
    {
        entry.addend
    } else if entry.addr_read == tag
        && entry.addend != super::super::mmu::TLB_MMIO_ADDEND
    {
        // Write tag not set (e.g. clean page). Fall
        // back to read addend — same host mapping.
        entry.addend
    } else {
        // TLB miss: use guest_base as fallback
        // (works for M-mode / bare translation).
        cpu.guest_base as usize
    };
    let host = (addr as usize).wrapping_add(addend) as *mut u8;
    let current = unsafe {
        if size == 4 {
            (*(host as *const u32) as i32) as i64 as u64
        } else {
            *(host as *const u64)
        }
    };
    if current != cpu.load_val {
        cpu.load_res = u64::MAX;
        return 1;
    }
    unsafe {
        if size == 4 {
            *(host as *mut u32) = val as u32;
        } else {
            *(host as *mut u64) = val;
        }
    }
    cpu.load_res = u64::MAX;
    0
}
