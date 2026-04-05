use crate::ir::RegSet;

/// x86-64 general-purpose register indices.
///
/// Encoding matches the x86-64 ModR/M and REX register numbering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Reg {
    Rax = 0,
    Rcx = 1,
    Rdx = 2,
    Rbx = 3,
    Rsp = 4,
    Rbp = 5,
    Rsi = 6,
    Rdi = 7,
    R8 = 8,
    R9 = 9,
    R10 = 10,
    R11 = 11,
    R12 = 12,
    R13 = 13,
    R14 = 14,
    R15 = 15,
}

impl Reg {
    /// Low 3 bits of the register encoding (for ModR/M).
    #[inline]
    pub const fn low3(self) -> u8 {
        (self as u8) & 0x7
    }

    /// Whether this register requires a REX prefix (R8-R15).
    #[inline]
    pub const fn needs_rex(self) -> bool {
        (self as u8) >= 8
    }

    /// Convert a raw register number (0-15) to Reg.
    #[inline]
    pub fn from_u8(val: u8) -> Self {
        assert!(val < 16, "invalid register number: {val}");
        // SAFETY: Reg is repr(u8) with variants 0..=15.
        unsafe { core::mem::transmute(val) }
    }
}

/// TCG_AREG0 = RBP: pointer to CPUArchState (env).
///
/// Matches QEMU's x86-64 convention where EBP/RBP is used as the
/// persistent env pointer across all generated TB code.
pub const TCG_AREG0: Reg = Reg::Rbp;

/// TCG_GUEST_BASE_REG = R14: guest memory base pointer.
///
/// Holds the host address of guest address 0. Generated code
/// accesses guest memory via [R14 + guest_addr].
pub const TCG_GUEST_BASE_REG: Reg = Reg::R14;

/// Callee-saved registers that the prologue must save/restore.
/// Order matches QEMU's `tcg_target_callee_save_regs` (System V ABI).
pub const CALLEE_SAVED: &[Reg] =
    &[Reg::Rbp, Reg::Rbx, Reg::R12, Reg::R13, Reg::R14, Reg::R15];

/// Function argument registers (System V AMD64 ABI).
pub const CALL_ARG_REGS: &[Reg] =
    &[Reg::Rdi, Reg::Rsi, Reg::Rdx, Reg::Rcx, Reg::R8, Reg::R9];

/// Registers reserved by the backend — not available for
/// register allocation.
/// RSP (stack), RBP (env), R14 (guest_base).
pub const RESERVED_REGS: RegSet = RegSet::from_raw(
    (1 << Reg::Rsp as u64) | (1 << Reg::Rbp as u64) | (1 << Reg::R14 as u64),
);

/// Stack frame constants (matching QEMU's layout).
pub const STACK_ALIGN: usize = 16;
/// Space reserved for outgoing call arguments on the stack.
pub const STATIC_CALL_ARGS_SIZE: usize = 128;
/// Number of longs in the CPU temp buffer (for spilling).
pub const CPU_TEMP_BUF_NLONGS: usize = 512;

/// Total push size: return address (implicit) + callee-saved pushes.
pub const PUSH_SIZE: usize = (1 + CALLEE_SAVED.len()) * 8;

/// Total frame size (16-byte aligned).
pub const FRAME_SIZE: usize = {
    let raw = PUSH_SIZE + STATIC_CALL_ARGS_SIZE + CPU_TEMP_BUF_NLONGS * 8;
    (raw + STACK_ALIGN - 1) & !(STACK_ALIGN - 1)
};

/// Stack adjustment after pushes.
pub const STACK_ADDEND: usize = FRAME_SIZE - PUSH_SIZE;

/// All GPRs available for register allocation (excludes
/// RSP, RBP, and R14 which are reserved).
pub const ALLOCATABLE_REGS: RegSet = RegSet::from_raw(
    0xFFFF
        & !((1 << Reg::Rsp as u64)
            | (1 << Reg::Rbp as u64)
            | (1 << Reg::R14 as u64)),
);
