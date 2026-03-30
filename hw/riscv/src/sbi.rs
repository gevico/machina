// Minimal SBI (Supervisor Binary Interface) ecall dispatch.
//
// Provides a built-in SBI stub for `-bios none` fallback so
// that S-mode software can probe basic SBI functionality
// without a real firmware image.

pub const SBI_EXT_BASE: u64 = 0x10;
pub const SBI_EXT_TIMER: u64 = 0x5449_4D45;
pub const SBI_EXT_IPI: u64 = 0x0073_5049;
pub const SBI_EXT_RFENCE: u64 = 0x5246_4E43;
pub const SBI_EXT_HSM: u64 = 0x0048_534D;
pub const SBI_EXT_SRST: u64 = 0x5352_5354;

pub struct SbiHandler;

impl SbiHandler {
    pub fn handle_ecall(
        ext_id: u64,
        func_id: u64,
        _args: &[u64; 6],
    ) -> SbiResult {
        match ext_id {
            SBI_EXT_BASE => handle_base(func_id),
            _ => SbiResult::not_supported(),
        }
    }
}

/// SBI return value pair (a0 = error, a1 = value).
pub struct SbiResult {
    pub error: i64,
    pub value: u64,
}

impl SbiResult {
    pub fn success(value: u64) -> Self {
        Self { error: 0, value }
    }

    pub fn not_supported() -> Self {
        Self {
            error: -2,
            value: 0,
        }
    }
}

fn handle_base(func_id: u64) -> SbiResult {
    match func_id {
        // SBI spec version (major=0, minor=2 → 0.2).
        0 => SbiResult::success(2),
        // Implementation ID: 0 = machina.
        1 => SbiResult::success(0),
        // Implementation version.
        2 => SbiResult::success(1),
        // Probe extension: 0 = not available.
        3 => SbiResult::success(0),
        _ => SbiResult::not_supported(),
    }
}
