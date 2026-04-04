// HMP: Human Monitor Protocol (text commands).
//
// Parses text command lines and calls MonitorService
// methods. Formats responses as human-readable text.

use std::sync::{Arc, Mutex};

use crate::service::MonitorService;

pub const PROMPT: &str = "(machina) ";

/// Handle one HMP command line. Returns the output text
/// (without prompt).
pub fn handle_line(
    line: &str,
    svc: &Arc<Mutex<MonitorService>>,
) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return Some(String::new());
    }

    let parts: Vec<&str> =
        line.splitn(2, ' ').collect();
    let cmd = parts[0];
    let arg = parts.get(1).copied().unwrap_or("");

    match cmd {
        "info" => handle_info(arg, svc),
        "stop" => {
            svc.lock().unwrap().stop();
            Some(String::new())
        }
        "cont" | "c" => {
            svc.lock().unwrap().cont();
            Some(String::new())
        }
        "trace" => handle_trace(arg, svc),
        "quit" | "q" => {
            svc.lock().unwrap().quit();
            None // signals exit
        }
        "help" | "?" => Some(help_text()),
        _ => Some(format!(
            "unknown command: '{}'\n",
            cmd
        )),
    }
}

fn handle_info(
    arg: &str,
    svc: &Arc<Mutex<MonitorService>>,
) -> Option<String> {
    let s = svc.lock().unwrap();
    match arg.trim() {
        "status" => {
            let running = s.query_status();
            if running {
                Some("VM status: running\n".into())
            } else {
                Some("VM status: paused\n".into())
            }
        }
        "registers" | "regs" => {
            if s.query_status() {
                return Some(
                    "VM must be paused to read \
                     registers\n"
                        .into(),
                );
            }
            match s.take_snapshot() {
                Some(snap) => {
                    let mut out = String::new();
                    for i in 0..32 {
                        out.push_str(&format!(
                            " x{:<2} {:#018x}",
                            i, snap.gpr[i]
                        ));
                        if i % 4 == 3 {
                            out.push('\n');
                        }
                    }
                    out.push_str(&format!(
                        " pc  {:#018x}\n",
                        snap.pc
                    ));
                    Some(out)
                }
                None => Some(
                    "CPU snapshot not available\n"
                        .into(),
                ),
            }
        }
        "cpus" => {
            let cpus = s.query_cpus();
            let mut out = String::new();
            for c in &cpus {
                let state = if c.halted {
                    "halted"
                } else {
                    "running"
                };
                out.push_str(&format!(
                    "* CPU #{}: pc={:#x} ({})\n",
                    c.cpu_index, c.pc, state
                ));
            }
            Some(out)
        }
        _ => Some(format!(
            "info: unknown subcommand '{}'\n",
            arg
        )),
    }
}

fn handle_trace(
    arg: &str,
    svc: &Arc<Mutex<MonitorService>>,
) -> Option<String> {
    let parts: Vec<&str> =
        arg.splitn(2, ' ').collect();
    let subcmd = parts.get(0).copied().unwrap_or("");
    let filter_arg = parts.get(1).copied().unwrap_or("");

    let mut s = svc.lock().unwrap();
    match subcmd {
        "start" => {
            match s.trace_start(filter_arg) {
                Ok(()) => Some("tracing started\n".into()),
                Err(e) => {
                    Some(format!("trace start: {}\n", e))
                }
            }
        }
        "stop" => {
            let n = s.trace_stop();
            Some(format!(
                "tracing stopped ({} events)\n",
                n
            ))
        }
        "status" => {
            let (enabled, count) = s.trace_status();
            Some(format!(
                "trace: {} ({} events collected)\n",
                if enabled { "enabled" } else { "disabled" },
                count,
            ))
        }
        _ => Some(format!(
            "trace: unknown subcommand '{}'\n",
            subcmd
        )),
    }
}

fn help_text() -> String {
    "\
info status     -- VM run state\n\
info registers  -- dump GPRs (paused only)\n\
info cpus       -- list vCPUs\n\
stop            -- pause vCPU\n\
cont (c)        -- resume vCPU\n\
trace start [f] -- start tracing (filter: trap,sched,vm,syscall)\n\
trace stop      -- stop tracing\n\
trace status    -- trace state\n\
quit (q)        -- exit emulator\n\
help (?)        -- this message\n"
        .to_string()
}

/// Run an interactive HMP session on the given reader
/// and writer. Blocks until quit or EOF.
pub fn run_interactive<R, W>(
    reader: &mut R,
    writer: &mut W,
    svc: Arc<Mutex<MonitorService>>,
) where
    R: std::io::BufRead,
    W: std::io::Write,
{
    let _ = write!(writer, "{}", PROMPT);
    let _ = writer.flush();

    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(_) => break,
        }
        match handle_line(&line, &svc) {
            Some(output) => {
                let _ = write!(writer, "{}", output);
                let _ = write!(writer, "{}", PROMPT);
                let _ = writer.flush();
            }
            None => break, // quit
        }
    }
}
