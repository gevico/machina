use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};

use machina_core::monitor::{CpuSnapshot, MonitorState, VmState};
use machina_monitor::hmp;
use machina_monitor::mmp;
use machina_monitor::service::MonitorService;

fn tcp_bind_available() -> bool {
    std::net::TcpListener::bind("127.0.0.1:0").is_ok()
}

fn make_svc() -> Arc<Mutex<MonitorService>> {
    let state = Arc::new(MonitorState::new());
    Arc::new(Mutex::new(MonitorService::new(state)))
}

// ── MonitorState tests ──────────────────────────────

#[test]
fn test_monitor_state_initial() {
    let ms = MonitorState::new();
    assert_eq!(ms.vm_state(), VmState::Running);
    assert!(!ms.is_quit_requested());
}

#[test]
fn test_monitor_state_stop_resume() {
    let ms = Arc::new(MonitorState::new());
    let ms2 = Arc::clone(&ms);

    // Simulate exec loop: keep calling check_pause()
    // until quit is requested.
    let handle = std::thread::spawn(move || {
        while !ms2.check_pause() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });

    // Give the exec-loop thread time to start polling.
    std::thread::sleep(std::time::Duration::from_millis(20));

    // Stop: blocks until the thread parks.
    ms.request_stop();
    assert_eq!(ms.vm_state(), VmState::Paused);

    // Resume: thread continues polling.
    ms.request_cont();
    assert_eq!(ms.vm_state(), VmState::Running);

    // Quit to break the exec-loop thread.
    ms.request_quit();
    handle.join().unwrap();
}

#[test]
fn test_monitor_state_stop_idempotent() {
    let ms = Arc::new(MonitorState::new());
    let ms2 = Arc::clone(&ms);

    let handle = std::thread::spawn(move || {
        while !ms2.check_pause() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });

    std::thread::sleep(std::time::Duration::from_millis(20));

    ms.request_stop();
    // Second stop when already paused is idempotent.
    ms.request_stop();
    assert_eq!(ms.vm_state(), VmState::Paused);

    ms.request_cont();
    ms.request_quit();
    handle.join().unwrap();
}

#[test]
fn test_monitor_state_cont_when_running() {
    let ms = MonitorState::new();
    // cont when already running is a no-op.
    ms.request_cont();
    assert_eq!(ms.vm_state(), VmState::Running);
}

#[test]
fn test_monitor_state_quit() {
    let ms = MonitorState::new();
    assert!(!ms.is_quit_requested());
    ms.request_quit();
    assert!(ms.is_quit_requested());
}

// ── MMP dispatch tests ──────────────────────────────

#[test]
fn test_mmp_qmp_capabilities() {
    let svc = make_svc();
    let resp = mmp::dispatch("qmp_capabilities", &svc);
    assert_eq!(resp["return"], serde_json::json!({}));
}

#[test]
fn test_mmp_query_status_running() {
    let svc = make_svc();
    let resp = mmp::dispatch("query-status", &svc);
    assert_eq!(resp["return"]["running"], true);
}

#[test]
fn test_mmp_unknown_command() {
    let svc = make_svc();
    let resp = mmp::dispatch("nonexistent", &svc);
    assert_eq!(resp["error"]["class"], "CommandNotFound");
}

#[test]
fn test_mmp_query_cpus_fast() {
    let svc = make_svc();
    let resp = mmp::dispatch("query-cpus-fast", &svc);
    let arr = resp["return"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["cpu-index"], 0);
    assert_eq!(arr[0]["arch"], "riscv64");
    // QMP-compatible fields.
    assert!(arr[0]["thread-id"].is_number());
    assert!(arr[0]["props"]["core-id"].is_number());
}

#[test]
fn test_mmp_quit() {
    let svc = make_svc();
    let resp = mmp::dispatch("quit", &svc);
    assert_eq!(resp["return"], serde_json::json!({}));
    assert!(svc.lock().unwrap().state.is_quit_requested());
}

#[test]
fn test_mmp_system_powerdown() {
    // QMP system_powerdown shuts down the VM. machina has no separate
    // ACPI shutdown path, so dispatch must reuse quit() and return
    // the empty success envelope.
    let svc = make_svc();
    let resp = mmp::dispatch("system_powerdown", &svc);
    assert_eq!(resp["return"], serde_json::json!({}));
    assert!(svc.lock().unwrap().state.is_quit_requested());
}

// ── HMP tests ───────────────────────────────────────

#[test]
fn test_hmp_info_status() {
    let svc = make_svc();
    let out = hmp::handle_line("info status", &svc);
    assert_eq!(out, Some("VM status: running\n".to_string()));
}

#[test]
fn test_hmp_info_registers_requires_pause() {
    let svc = make_svc();
    let out = hmp::handle_line("info registers", &svc);
    assert!(out.as_ref().unwrap().contains("must be paused"));
}

#[test]
fn test_hmp_help() {
    let svc = make_svc();
    let out = hmp::handle_line("help", &svc);
    assert!(out.as_ref().unwrap().contains("info status"));
    assert!(out.as_ref().unwrap().contains("quit"));
}

#[test]
fn test_hmp_unknown_command() {
    let svc = make_svc();
    let out = hmp::handle_line("foobar", &svc);
    assert!(out.as_ref().unwrap().contains("unknown command"));
}

#[test]
fn test_hmp_quit_returns_none() {
    let svc = make_svc();
    let out = hmp::handle_line("quit", &svc);
    assert!(out.is_none()); // signals exit
}

#[test]
fn test_hmp_empty_line() {
    let svc = make_svc();
    let out = hmp::handle_line("", &svc);
    assert_eq!(out, Some(String::new()));
}

#[test]
fn test_hmp_info_cpus() {
    let svc = make_svc();
    let out = hmp::handle_line("info cpus", &svc);
    assert!(out.as_ref().unwrap().contains("CPU #0"));
}

#[test]
fn test_hmp_info_memory_unconfigured() {
    let svc = make_svc();
    let out = hmp::handle_line("info memory", &svc);
    assert_eq!(out, Some("RAM: not configured\n".to_string()));
}

#[test]
fn test_hmp_info_memory_reports_mib_and_bytes() {
    let svc = make_svc();
    svc.lock().unwrap().set_ram_size(128 * 1024 * 1024);
    let out = hmp::handle_line("info memory", &svc);
    assert_eq!(out, Some("RAM: 128 MiB (134217728 bytes)\n".to_string()),);
}

#[test]
fn test_hmp_help_lists_info_memory() {
    let svc = make_svc();
    let out = hmp::handle_line("help", &svc);
    assert!(out.as_ref().unwrap().contains("info memory"));
}

// ── TCP socket-level tests ──────────────────────────

fn read_json_line(reader: &mut BufReader<TcpStream>) -> serde_json::Value {
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn send_cmd(stream: &mut TcpStream, cmd: &str) {
    writeln!(stream, "{{\"execute\":\"{}\"}}", cmd).unwrap();
    stream.flush().unwrap();
}

#[test]
fn test_tcp_greeting_and_caps() {
    if !tcp_bind_available() {
        eprintln!("skipping: TCP bind not permitted");
        return;
    }
    let state = Arc::new(MonitorState::new());
    let svc = Arc::new(Mutex::new(MonitorService::new(Arc::clone(&state))));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let svc2 = Arc::clone(&svc);
    let handle = std::thread::spawn(move || {
        mmp::run_tcp(listener, svc2);
    });

    let stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;

    // Read greeting.
    let greeting = read_json_line(&mut reader);
    assert!(greeting["QMP"]["version"]["machina"].is_object());

    // Send qmp_capabilities.
    send_cmd(&mut writer, "qmp_capabilities");
    let resp = read_json_line(&mut reader);
    assert!(resp["return"].is_object());

    // Send query-status.
    send_cmd(&mut writer, "query-status");
    let resp = read_json_line(&mut reader);
    assert_eq!(resp["return"]["running"], true);

    // Send unknown command.
    send_cmd(&mut writer, "nonexistent");
    let resp = read_json_line(&mut reader);
    assert_eq!(resp["error"]["class"], "CommandNotFound");

    // Quit.
    send_cmd(&mut writer, "quit");
    let resp = read_json_line(&mut reader);
    assert!(resp["return"].is_object());

    handle.join().unwrap();
}

#[test]
fn test_tcp_pre_caps_rejection() {
    if !tcp_bind_available() {
        eprintln!("skipping: TCP bind not permitted");
        return;
    }
    let state = Arc::new(MonitorState::new());
    let svc = Arc::new(Mutex::new(MonitorService::new(Arc::clone(&state))));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let svc2 = Arc::clone(&svc);
    let handle = std::thread::spawn(move || {
        mmp::run_tcp(listener, svc2);
    });

    let stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;

    // Read greeting.
    let _greeting = read_json_line(&mut reader);

    // Send command before caps → should be rejected.
    send_cmd(&mut writer, "query-status");
    let resp = read_json_line(&mut reader);
    assert!(resp["error"].is_object());
    assert!(resp["error"]["desc"]
        .as_str()
        .unwrap()
        .contains("qmp_capabilities"));

    // Now send caps + quit.
    send_cmd(&mut writer, "qmp_capabilities");
    let _ = read_json_line(&mut reader);
    send_cmd(&mut writer, "quit");
    let _ = read_json_line(&mut reader);

    handle.join().unwrap();
}

// ── HMP interactive session test ────────────────────

#[test]
fn test_hmp_interactive_session() {
    let svc = make_svc();
    let input = b"info status\nhelp\nquit\n";
    let mut reader = std::io::BufReader::new(&input[..]);
    let mut output = Vec::new();

    hmp::run_interactive(&mut reader, &mut output, svc);

    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("VM status: running"));
    assert!(text.contains("info status"));
    assert!(text.contains("(machina)"));
}

// ===== Snapshot and error-path coverage (#60) =====
//
// Each pause-aware test spawns a dedicated polling thread that
// runs `check_pause` until quit is requested, mirroring the
// existing MonitorState tests. The thread always exits via
// `request_quit` so the test cannot hang on a stale handle.

fn pause_polling_thread(
    state: Arc<MonitorState>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while !state.check_pause() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    })
}

#[test]
fn test_hmp_info_registers_dumps_gprs_and_pc_when_paused() {
    let svc = make_svc();
    let state = Arc::clone(&svc.lock().unwrap().state);
    let h = pause_polling_thread(Arc::clone(&state));
    std::thread::sleep(std::time::Duration::from_millis(20));

    state.request_stop();
    let mut snap = CpuSnapshot {
        pc: 0x8000_1234,
        ..Default::default()
    };
    snap.gpr[1] = 0xDEAD_BEEF;
    snap.gpr[2] = 0x1000_0000;
    state.store_snapshot(snap);

    let out = hmp::handle_line("info registers", &svc).unwrap();
    assert!(out.contains(" x0 "));
    assert!(out.contains(" x31 "));
    assert!(out.contains(" pc "));
    assert!(
        out.contains("0x00000000deadbeef"),
        "register dump should include x1 = DEADBEEF, got: {out}",
    );
    assert!(
        out.contains("0x0000000080001234"),
        "register dump should include pc = 0x80001234, got: {out}",
    );

    state.request_quit();
    h.join().unwrap();
}

#[test]
fn test_hmp_info_cpus_paused_reports_snapshot_pc_and_state() {
    let svc = make_svc();
    let state = Arc::clone(&svc.lock().unwrap().state);
    let h = pause_polling_thread(Arc::clone(&state));
    std::thread::sleep(std::time::Duration::from_millis(20));

    state.request_stop();
    state.store_snapshot(CpuSnapshot {
        pc: 0xc000_0010,
        halted: true,
        ..Default::default()
    });

    let out = hmp::handle_line("info cpus", &svc).unwrap();
    assert!(out.contains("CPU #0"));
    assert!(
        out.contains("pc=0xc0000010"),
        "info cpus should print snapshot PC when paused, got: {out}",
    );
    assert!(
        out.contains("halted"),
        "info cpus should mark halted snapshots, got: {out}",
    );

    state.request_quit();
    h.join().unwrap();
}

#[test]
fn test_tcp_invalid_json_returns_generic_error() {
    if !tcp_bind_available() {
        eprintln!("skipping: TCP bind not permitted");
        return;
    }
    let state = Arc::new(MonitorState::new());
    let svc = Arc::new(Mutex::new(MonitorService::new(Arc::clone(&state))));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let svc2 = Arc::clone(&svc);
    let handle = std::thread::spawn(move || mmp::run_tcp(listener, svc2));

    let stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;

    let _greeting = read_json_line(&mut reader);

    // Caps first so caps-gating doesn't shadow the JSON-parse error.
    send_cmd(&mut writer, "qmp_capabilities");
    let _ = read_json_line(&mut reader);

    // Malformed JSON: missing closing brace.
    writeln!(&mut writer, "{{not json").unwrap();
    writer.flush().unwrap();

    let resp = read_json_line(&mut reader);
    assert_eq!(resp["error"]["class"], "GenericError");
    assert!(
        resp["error"]["desc"]
            .as_str()
            .unwrap_or("")
            .contains("JSON parse error"),
        "GenericError desc must mention JSON parse error, got {resp}",
    );

    // Subsequent valid command must still work — the bad line
    // did not poison the connection.
    send_cmd(&mut writer, "query-status");
    let resp = read_json_line(&mut reader);
    assert_eq!(resp["return"]["running"], true);

    send_cmd(&mut writer, "quit");
    let _ = read_json_line(&mut reader);
    handle.join().unwrap();
}

#[test]
fn test_tcp_blank_lines_then_unknown_command_after_caps() {
    if !tcp_bind_available() {
        eprintln!("skipping: TCP bind not permitted");
        return;
    }
    let state = Arc::new(MonitorState::new());
    let svc = Arc::new(Mutex::new(MonitorService::new(Arc::clone(&state))));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let svc2 = Arc::clone(&svc);
    let handle = std::thread::spawn(move || mmp::run_tcp(listener, svc2));

    let stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;

    let _greeting = read_json_line(&mut reader);

    // Blank lines before caps must be silently skipped.
    writeln!(&mut writer).unwrap();
    writeln!(&mut writer, "   ").unwrap();
    writer.flush().unwrap();

    send_cmd(&mut writer, "qmp_capabilities");
    let _ = read_json_line(&mut reader);

    // Blank lines after caps too.
    writeln!(&mut writer).unwrap();
    writer.flush().unwrap();

    // Unknown post-caps command returns CommandNotFound, not
    // CommandNotFound-because-of-caps.
    send_cmd(&mut writer, "no-such-cmd");
    let resp = read_json_line(&mut reader);
    assert_eq!(resp["error"]["class"], "CommandNotFound");
    assert!(
        resp["error"]["desc"]
            .as_str()
            .unwrap_or("")
            .contains("no-such-cmd"),
        "CommandNotFound desc must echo the command, got {resp}",
    );

    send_cmd(&mut writer, "quit");
    let _ = read_json_line(&mut reader);
    handle.join().unwrap();
}
