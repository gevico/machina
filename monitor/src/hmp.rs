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

    let parts: Vec<&str> = line.splitn(2, ' ').collect();
    let cmd = parts[0];
    let arg = parts.get(1).copied().unwrap_or("");

    match cmd {
        "info" => handle_info(arg, svc),
        "stop" => {
            svc.lock().expect("monitor mutex poisoned").stop();
            Some(String::new())
        }
        "cont" | "c" => {
            svc.lock().expect("monitor mutex poisoned").cont();
            Some(String::new())
        }
        "quit" | "q" => {
            svc.lock().expect("monitor mutex poisoned").quit();
            None // signals exit
        }
        "help" | "?" => Some(help_text()),
        _ => Some(format!("unknown command: '{}'\n", cmd)),
    }
}

fn handle_info(arg: &str, svc: &Arc<Mutex<MonitorService>>) -> Option<String> {
    let s = svc.lock().expect("monitor mutex poisoned");
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
                    out.push_str(&format!(" pc  {:#018x}\n", snap.pc));
                    Some(out)
                }
                None => Some("CPU snapshot not available\n".into()),
            }
        }
        "cpus" => {
            let running = s.query_status();
            let cpus = s.query_cpus();
            let mut out = String::new();
            for c in &cpus {
                if running {
                    out.push_str(&format!(
                        "* CPU #{}: (running)\n",
                        c.cpu_index
                    ));
                } else {
                    let state = if c.halted { "halted" } else { "stopped" };
                    out.push_str(&format!(
                        "* CPU #{}: pc={:#x} ({})\n",
                        c.cpu_index, c.pc, state
                    ));
                }
            }
            Some(out)
        }
        _ => Some(format!("info: unknown subcommand '{}'\n", arg)),
    }
}

fn help_text() -> String {
    "\
info status     -- VM run state\n\
info registers  -- dump GPRs (paused only)\n\
info cpus       -- list vCPUs\n\
stop            -- pause vCPU\n\
cont (c)        -- resume vCPU\n\
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
