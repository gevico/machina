use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};

use machina_core::monitor::{MonitorState, VmState};
use machina_monitor::hmp;
use machina_monitor::mmp;
use machina_monitor::service::MonitorService;

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
            std::thread::sleep(
                std::time::Duration::from_millis(1),
            );
        }
    });

    // Give the exec-loop thread time to start polling.
    std::thread::sleep(
        std::time::Duration::from_millis(20),
    );

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
            std::thread::sleep(
                std::time::Duration::from_millis(1),
            );
        }
    });

    std::thread::sleep(
        std::time::Duration::from_millis(20),
    );

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
