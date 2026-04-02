// MMP: Machina Monitor Protocol (restricted QMP subset).
//
// JSON wire protocol compatible with QMP format:
// - Greeting on connect
// - {"execute":"cmd","arguments":{}} requests
// - {"return":{}} or {"error":{}} responses

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::service::MonitorService;

const GREETING: &str = r#"{"QMP":{"version":{"machina":{"major":0,"minor":1,"micro":0}},"capabilities":[]}}"#;

/// Run MMP server on a TCP listener. Blocks until
/// quit is requested.
pub fn run_tcp(
    listener: TcpListener,
    svc: Arc<Mutex<MonitorService>>,
) {
    listener
        .set_nonblocking(false)
        .expect("set_nonblocking");
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                if handle_connection(s, &svc) {
                    break; // quit requested
                }
            }
            Err(_) => {
                if svc
                    .lock()
                    .unwrap()
                    .state
                    .is_quit_requested()
                {
                    break;
                }
            }
        }
    }
}

fn handle_connection(
    mut stream: TcpStream,
    svc: &Arc<Mutex<MonitorService>>,
) -> bool {
    // Send greeting.
    let _ = writeln!(stream, "{}", GREETING);
    let _ = stream.flush();

    let reader = BufReader::new(stream.try_clone().unwrap());
    let mut caps_done = false;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }

        let req: Value = match serde_json::from_str(&line)
        {
            Ok(v) => v,
            Err(e) => {
                let err = json!({
                    "error": {
                        "class": "GenericError",
                        "desc": format!(
                            "JSON parse error: {}",
                            e
                        )
                    }
                });
                let _ = writeln!(stream, "{}", err);
                let _ = stream.flush();
                continue;
            }
        };

        let cmd = req["execute"]
            .as_str()
            .unwrap_or("")
            .to_string();

        if !caps_done && cmd != "qmp_capabilities" {
            let err = json!({
                "error": {
                    "class": "CommandNotFound",
                    "desc":
                        "qmp_capabilities required first"
                }
            });
            let _ = writeln!(stream, "{}", err);
            let _ = stream.flush();
            continue;
        }

        let resp = dispatch(&cmd, svc);
        let _ = writeln!(stream, "{}", resp);
        let _ = stream.flush();

        if cmd == "qmp_capabilities" {
            caps_done = true;
        }
        if cmd == "quit" {
            return true;
        }
    }
    false
}

/// Dispatch a command and return the JSON response.
pub fn dispatch(
    cmd: &str,
    svc: &Arc<Mutex<MonitorService>>,
) -> Value {
    let s = svc.lock().unwrap();
    match cmd {
        "qmp_capabilities" => json!({"return": {}}),
        "query-status" => {
            let running = s.query_status();
            json!({"return": {"running": running}})
        }
        "stop" => {
            drop(s);
            svc.lock().unwrap().stop();
            json!({"return": {}})
        }
        "cont" => {
            s.cont();
            json!({"return": {}})
        }
        "quit" => {
            s.quit();
            json!({"return": {}})
        }
        "query-cpus-fast" => {
            let cpus = s.query_cpus();
            let arr: Vec<Value> = cpus
                .iter()
                .map(|c| {
                    json!({
                        "cpu-index": c.cpu_index,
                        "qom-path": format!(
                            "/machine/cpu[{}]",
                            c.cpu_index
                        ),
                        "thread-id": 0,
                        "halted": c.halted,
                        "arch": c.arch,
                        "props": {
                            "core-id": c.cpu_index
                        },
                        "target": c.arch,
                    })
                })
                .collect();
            json!({"return": arr})
        }
        "system_reset" => {
            json!({
                "error": {
                    "class": "GenericError",
                    "desc": "system_reset not \
                             implemented (deferred)"
                }
            })
        }
        _ => {
            json!({
                "error": {
                    "class": "CommandNotFound",
                    "desc": format!(
                        "command '{}' not found",
                        cmd
                    )
                }
            })
        }
    }
}
