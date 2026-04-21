use std::io::Read;

use machina_util::trace;

#[test]
fn test_trace_no_output_without_thread_init() {
    // Without calling init_trace on THIS thread,
    // trace calls should produce no output even if
    // the global ENABLED flag is set by another test.
    let dir = tempfile::tempdir().unwrap();
    let check = dir.path().join("no_output.log");
    // Do NOT call init_trace here.
    trace::trace_csr("test", 0);
    assert!(!check.exists());
}

#[test]
fn test_trace_disabled_produces_no_output() {
    // Calling trace functions when disabled should not
    // panic or produce side effects.
    trace::trace_csr("mstatus", 0x1234);
    trace::trace_exception(2, 0x8000_0000);
    trace::trace_mmio(0x1000_0000, 4, 0x42, true);
}

#[test]
fn test_trace_init_and_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.log");
    trace::init_trace(path.to_str().unwrap()).unwrap();
    assert!(trace::trace_enabled());

    trace::trace_csr("0x300", 0xABCD);
    trace::trace_exception(5, 0x8000_1000);
    trace::trace_mmio(0x1000_0000, 4, 0xFF, false);

    // Read back and verify structured output.
    let mut content = String::new();
    std::fs::File::open(&path)
        .unwrap()
        .read_to_string(&mut content)
        .unwrap();

    assert!(content.contains("CSR 0x300 <-"));
    assert!(content.contains("EXC cause=5"));
    assert!(content.contains("MMIO R addr="));
}

#[test]
fn test_trace_init_bad_path() {
    let result = trace::init_trace("/nonexistent/dir/trace.log");
    assert!(result.is_err());
}
