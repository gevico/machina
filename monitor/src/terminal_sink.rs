//! Terminal trace sink with coloured output.

use machina_core::trace::{TraceEvent, TraceSink};

// ANSI colour codes.
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const BLUE: &str = "\x1b[34m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";

/// Sink that writes coloured trace events to stderr.
pub struct TerminalSink;

impl TerminalSink {
    pub fn new() -> Self {
        Self
    }
}

impl TraceSink for TerminalSink {
    fn emit(&mut self, seq: u64, event: &TraceEvent) {
        // Collector uses 0-based seq; spec shows 1-based [#NNNN].
        let n = seq + 1;
        let line = match event {
            TraceEvent::TrapEnter {
                from_priv,
                to_priv,
                cause,
                pc,
                fn_name,
            } => {
                let from = priv_label(*from_priv);
                let to = priv_label(*to_priv);
                let fn_part = fn_name
                    .as_deref()
                    .map(|n| format!(" {}", n))
                    .unwrap_or_default();
                format!(
                    "[#{:04}] {}[trap]     {}->{} \
                     cause={}{} pc=0x{:x}{}",
                    n,
                    RED,
                    from,
                    to,
                    cause,
                    cause_desc(*cause),
                    pc,
                    fn_part,
                )
            }
            TraceEvent::TrapExit {
                from_priv,
                to_priv,
                sepc,
                fn_name,
            } => {
                let from = priv_label(*from_priv);
                let to = priv_label(*to_priv);
                let fn_part = fn_name
                    .as_deref()
                    .map(|n| format!(" {}", n))
                    .unwrap_or_default();
                format!(
                    "[#{:04}] {}[trap]     {}->{} \
                     sepc=0x{:x}{}",
                    n, RED, from, to, sepc, fn_part,
                )
            }
            TraceEvent::Syscall { id, args, pc } => {
                format!(
                    "[#{:04}] {}[syscall]  id={}({}) \
                     a0=0x{:x} a1=0x{:x} a2=0x{:x} \
                     pc=0x{:x}",
                    n,
                    YELLOW,
                    id,
                    syscall_name(*id),
                    args[0],
                    args[1],
                    args[2],
                    pc,
                )
            }
            TraceEvent::TaskSwitch {
                from_pc,
                to_pc,
                fn_name,
            } => {
                let fn_part = fn_name
                    .as_deref()
                    .map(|n| format!(" {}", n))
                    .unwrap_or_default();
                format!(
                    "[#{:04}] {}[sched]    task switch \
                     0x{:x}->0x{:x}{}",
                    n, GREEN, from_pc, to_pc, fn_part,
                )
            }
            TraceEvent::TimerInterrupt { pc, priv_level } => {
                let pl = priv_label(*priv_level);
                format!(
                    "[#{:04}] {}[timer]    {}-mode timer \
                     interrupt pc=0x{:x}",
                    n, GREEN, pl, pc,
                )
            }
            TraceEvent::AddressSpaceSwitch {
                old_asid,
                new_asid,
                pc,
                fn_name,
                ..
            } => {
                let fn_part = fn_name
                    .as_deref()
                    .map(|n| format!(" {}", n))
                    .unwrap_or_default();
                format!(
                    "[#{:04}] {}[vm]       satp asid \
                     {}->{} pc=0x{:x}{}",
                    n, BLUE, old_asid, new_asid, pc, fn_part,
                )
            }
            TraceEvent::TlbFlush { pc, fn_name } => {
                let fn_part = fn_name
                    .as_deref()
                    .map(|n| format!(" {}", n))
                    .unwrap_or_default();
                format!(
                    "[#{:04}] {}[vm]       tlb flush \
                     pc=0x{:x}{}",
                    n, BLUE, pc, fn_part,
                )
            }
        };
        eprintln!("{}{}", line, RESET);
    }
}

fn priv_label(p: u8) -> &'static str {
    match p {
        0 => "U",
        1 => "S",
        3 => "M",
        _ => "?",
    }
}

fn cause_desc(cause: u64) -> String {
    let is_interrupt = cause & (1 << 63) != 0;
    let code = cause & !(1u64 << 63);
    if is_interrupt {
        match code {
            5 => "STI".to_string(),
            7 => "MTI".to_string(),
            _ => format!("IRQ({})", code),
        }
    } else {
        match code {
            2 => "IllegalInsn".to_string(),
            8 => "UserEcall".to_string(),
            9 => "SupvEcall".to_string(),
            11 => "MachEcall".to_string(),
            12 => "Breakpoint".to_string(),
            13 => "LoadPageFault".to_string(),
            15 => "StorePageFault".to_string(),
            _ => format!("exc({})", code),
        }
    }
}

fn syscall_name(id: usize) -> &'static str {
    match id {
        56 => "openat",
        57 => "close",
        62 => "lseek",
        63 => "read",
        64 => "write",
        93 => "exit",
        94 => "exit_group",
        129 => "kill",
        172 => "getpid",
        220 => "clone",
        221 => "execve",
        260 => "wait4",
        _ => "unknown",
    }
}

impl Default for TerminalSink {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_trap_enter() {
        let mut sink = TerminalSink::new();
        let evt = TraceEvent::TrapEnter {
            from_priv: 0,
            to_priv: 1,
            cause: 8,
            pc: 0x80400024,
            fn_name: Some("hello_world".into()),
        };
        sink.emit(11, &evt);
    }
}
