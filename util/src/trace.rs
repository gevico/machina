// Thread-local event tracer for debugging and analysis.

use std::cell::RefCell;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static TRACE_FILE: RefCell<Option<File>> =
        const { RefCell::new(None) };
}

/// Open a trace file for the current thread and enable
/// tracing globally. Each thread should call this with its
/// own path if multi-thread tracing is desired.
///
/// # Errors
///
/// Returns an error if the file cannot be created.
pub fn init_trace(path: &str) -> std::io::Result<()> {
    let file = File::create(path)?;
    TRACE_FILE.with(|f| {
        *f.borrow_mut() = Some(file);
    });
    ENABLED.store(true, Ordering::Relaxed);
    Ok(())
}

/// Fast check whether tracing is active. Returns false when
/// no thread has called `init_trace`, making the cost of an
/// unconfigured trace point a single relaxed atomic load.
#[inline]
pub fn trace_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Record a CSR write.
pub fn trace_csr(name: &str, val: u64) {
    if !trace_enabled() {
        return;
    }
    TRACE_FILE.with(|f| {
        if let Some(file) = f.borrow_mut().as_mut() {
            let _ = writeln!(file, "CSR {name} <- 0x{val:016x}");
        }
    });
}

/// Record an exception event.
pub fn trace_exception(cause: u64, pc: u64) {
    if !trace_enabled() {
        return;
    }
    TRACE_FILE.with(|f| {
        if let Some(file) = f.borrow_mut().as_mut() {
            let _ = writeln!(file, "EXC cause={cause} pc=0x{pc:016x}");
        }
    });
}

/// Record an MMIO access.
pub fn trace_mmio(addr: u64, size: u32, val: u64, is_write: bool) {
    if !trace_enabled() {
        return;
    }
    let dir = if is_write { 'W' } else { 'R' };
    TRACE_FILE.with(|f| {
        if let Some(file) = f.borrow_mut().as_mut() {
            let _ = writeln!(
                file,
                "MMIO {dir} addr=0x{addr:016x} \
                 size={size} val=0x{val:016x}"
            );
        }
    });
}
