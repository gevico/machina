//! RISC-V ISA extension configuration.
//!
//! Provides MISA-style letter-extension bitmask (`MisaExt`)
//! and a per-CPU configuration struct (`RiscvCfg`) that
//! mirrors QEMU's `RISCVCPUConfig`.
//!
//! Reference: ~/qemu/target/riscv/cpu_cfg.h,
//!            ~/qemu/target/riscv/cpu_cfg_fields.h.inc

// ── MISA letter-extension bitmask ──────────────────

/// Bitmask of single-letter RISC-V extensions (MISA
/// bits).  Bit N = extension whose letter is 'A' + N.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MisaExt(u32);

#[allow(non_upper_case_globals)]
impl MisaExt {
    pub const EMPTY: Self = Self(0);
    pub const A: Self = Self(1 << 0);
    pub const C: Self = Self(1 << (b'C' - b'A'));
    pub const D: Self = Self(1 << (b'D' - b'A'));
    pub const F: Self = Self(1 << (b'F' - b'A'));
    pub const I: Self = Self(1 << (b'I' - b'A'));
    pub const M: Self = Self(1 << (b'M' - b'A'));
    pub const S: Self = Self(1 << (b'S' - b'A'));
    pub const U: Self = Self(1 << (b'U' - b'A'));

    /// G = IMAFD (general-purpose).
    pub const G: Self =
        Self(Self::I.0 | Self::M.0 | Self::A.0 | Self::F.0 | Self::D.0);

    #[inline]
    pub const fn bits(self) -> u32 {
        self.0
    }
    #[inline]
    pub const fn from_bits_truncate(bits: u32) -> Self {
        Self(bits & ((1 << 26) - 1))
    }
    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
    #[inline]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

// ── Extension configuration ────────────────────────

/// Per-CPU RISC-V extension configuration.
///
/// `misa` covers single-letter extensions; boolean
/// fields cover Z/S/X-extensions.  Mirrors QEMU's
/// `RISCVCPUConfig` (cpu_cfg_fields.h.inc).
///
/// Only extensions that machina implements (or will
/// soon) are listed.  New fields are added as needed.
#[derive(Clone, Copy, Debug)]
pub struct RiscvCfg {
    pub misa: MisaExt,

    // ── Zicsr / Zifencei ──────────────────────────
    pub ext_zicsr: bool,
    pub ext_zifencei: bool,

    // ── Zicntr (counters) ─────────────────────────
    pub ext_zicntr: bool,

    // ── Zicc* (cache coherence) ───────────────────
    /// Instruction-Cache Coherence for Instruction
    /// Data: stores to code pages automatically
    /// invalidate I-cache (no FENCE.I required).
    pub ext_ziccid: bool,
    /// I-cache Coherence for Instruction Fetches.
    pub ext_ziccif: bool,
    /// Load-Store-Modify coherence.
    pub ext_zicclsm: bool,
    /// AMO Alignment.
    pub ext_ziccamoa: bool,

    // ── Zicbo* (cache-block operations) ───────────
    pub ext_zicbom: bool,
    pub ext_zicbop: bool,
    pub ext_zicboz: bool,

    // ── Bit manipulation ──────────────────────────
    pub ext_zba: bool,
    pub ext_zbb: bool,
    pub ext_zbc: bool,
    pub ext_zbs: bool,

    // ── FP extensions ─────────────────────────────
    pub ext_zfh: bool,
    pub ext_zfhmin: bool,

    // ── Supervisor extensions ─────────────────────
    pub ext_ssvnapot: bool,
    pub ext_svadu: bool,
    pub ext_sstc: bool,
}

// ── Predefined profiles ────────────────────────────

impl RiscvCfg {
    /// RV64GC base profile: RV64IMAFDC + mandatory
    /// Z-extensions implied by priv spec 1.11+.
    pub const RV64GC: Self = Self {
        misa: MisaExt::from_bits_truncate(
            MisaExt::I.0
                | MisaExt::M.0
                | MisaExt::A.0
                | MisaExt::F.0
                | MisaExt::D.0
                | MisaExt::C.0
                | MisaExt::S.0
                | MisaExt::U.0,
        ),
        ext_zicsr: true,
        ext_zifencei: true,
        ext_zicntr: true,
        // Zicc*: implied by priv 1.11 in QEMU.
        ext_ziccid: true,
        ext_ziccif: true,
        ext_zicclsm: true,
        ext_ziccamoa: true,
        // Zicbo*: off by default.
        ext_zicbom: false,
        ext_zicbop: false,
        ext_zicboz: false,
        // Bit manipulation: off by default.
        ext_zba: false,
        ext_zbb: false,
        ext_zbc: false,
        ext_zbs: false,
        // FP extensions.
        ext_zfh: false,
        ext_zfhmin: false,
        // Supervisor extensions.
        ext_ssvnapot: false,
        ext_svadu: true,
        ext_sstc: false,
    };

    /// RV64GC + all bit-manipulation extensions.
    pub const RV64GC_ZB: Self = Self {
        ext_zba: true,
        ext_zbb: true,
        ext_zbc: true,
        ext_zbs: true,
        ..Self::RV64GC
    };

    /// Old alias for compatibility.
    pub const RV64IMAFDC: Self = Self::RV64GC;
}

impl Default for RiscvCfg {
    fn default() -> Self {
        Self::RV64GC
    }
}
