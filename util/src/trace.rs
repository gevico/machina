// Event tracing for full-system debugging.
//
// Enabled via --trace <file> CLI flag. When active, records
// CSR writes, exception entries, and MMIO accesses to a
// structured log file for comparison with QEMU -d traces.

use std::cell::RefCell;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

static TRACE_ENABLED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static TRACE_FILE: RefCell<Option<File>> = RefCell::new(None);
}

pub fn init_trace(path: &str) -> std::io::Result<()> {
    let f = File::create(path)?;
    TRACE_FILE.with(|t| *t.borrow_mut() = Some(f));
    TRACE_ENABLED.store(true, Ordering::Relaxed);
    Ok(())
}

pub fn is_enabled() -> bool {
    TRACE_ENABLED.load(Ordering::Relaxed)
}

pub fn flush() {
    TRACE_FILE.with(|t| {
        if let Some(ref mut f) = *t.borrow_mut() {
            let _ = f.flush();
        }
    });
}

pub fn trace_csr(pc: u64, addr: u16, old: u64, new: u64, mode: u8) {
    if !is_enabled() {
        return;
    }
    log_event(&format!(
        "CSR pc={:#x} addr={:#x} old={:#x} new={:#x} mode={}",
        pc, addr, old, new, mode
    ));
}

pub fn trace_exception(pc: u64, cause: u64, epc: u64, from: u8, to: u8) {
    if !is_enabled() {
        return;
    }
    log_event(&format!(
        "EXC pc={:#x} cause={} epc={:#x} from={} to={}",
        pc, cause, epc, from, to
    ));
}

pub fn trace_mmio(addr: u64, size: u8, val: u64, write: bool, dev: &str) {
    if !is_enabled() {
        return;
    }
    log_event(&format!(
        "MMIO addr={:#x} size={} val={:#x} write={} dev={}",
        addr, size, val, write, dev
    ));
}

fn log_event(msg: &str) {
    TRACE_FILE.with(|t| {
        if let Some(ref mut f) = *t.borrow_mut() {
            let _ = writeln!(f, "{}", msg);
        }
    });
}
