//! Kernel execution trace event types and abstractions.

// ---------------------------------------------------------------------------
// Event filter
// ---------------------------------------------------------------------------

/// Bitfield for filtering trace events by category.
#[derive(Clone, Copy, Debug, Default)]
pub struct EventFilter(u8);

impl EventFilter {
    const TRAP: u8 = 1 << 0;
    const SCHED: u8 = 1 << 1;
    const VM: u8 = 1 << 2;
    const SYSCALL: u8 = 1 << 3;

    /// Accept every event category.
    pub fn all() -> Self {
        Self(Self::TRAP | Self::SCHED | Self::VM | Self::SYSCALL)
    }

    /// Reject every event.
    pub fn none() -> Self {
        Self(0)
    }

    /// Parse a comma-separated category string such as `"trap,sched"`.
    pub fn from_str(s: &str) -> Result<Self, String> {
        if s.trim().is_empty() {
            return Ok(Self::all());
        }
        let mut bits: u8 = 0;
        for token in s.split(',') {
            match token.trim() {
                "trap" => bits |= Self::TRAP,
                "sched" => bits |= Self::SCHED,
                "vm" => bits |= Self::VM,
                "syscall" => bits |= Self::SYSCALL,
                other => return Err(format!("unknown trace category: '{}'", other)),
            }
        }
        Ok(Self(bits))
    }

    /// Whether this filter accepts the given category bit.
    pub fn matches(&self, category: TraceCategory) -> bool {
        self.0 & (1 << category as u8) != 0
    }
}

// ---------------------------------------------------------------------------
// Trace categories
// ---------------------------------------------------------------------------

/// Maps each event variant to a filter category bit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TraceCategory {
    Trap = 0,
    Sched = 1,
    Vm = 2,
    Syscall = 3,
}

// ---------------------------------------------------------------------------
// Trace event
// ---------------------------------------------------------------------------

/// A single observable event during guest kernel execution.
#[derive(Clone, Debug)]
pub enum TraceEvent {
    // -- ch2: privilege switch & trap --
    TrapEnter {
        from_priv: u8,
        to_priv: u8,
        cause: u64,
        pc: u64,
        fn_name: Option<String>,
    },
    TrapExit {
        from_priv: u8,
        to_priv: u8,
        sepc: u64,
        fn_name: Option<String>,
    },

    // -- ch2: syscall --
    Syscall {
        id: usize,
        args: [u64; 3],
        pc: u64,
    },

    // -- ch3: task switch --
    TaskSwitch {
        from_pc: u64,
        to_pc: u64,
        fn_name: Option<String>,
    },

    // -- ch3: timer interrupt --
    TimerInterrupt {
        pc: u64,
        priv_level: u8,
    },

    // -- ch4: address space --
    AddressSpaceSwitch {
        old_satp: u64,
        new_satp: u64,
        old_asid: u16,
        new_asid: u16,
        pc: u64,
        fn_name: Option<String>,
    },

    // -- ch4: TLB --
    TlbFlush {
        pc: u64,
        fn_name: Option<String>,
    },
}

impl TraceEvent {
    /// Category used for filtering.
    pub fn category(&self) -> TraceCategory {
        match self {
            Self::TrapEnter { .. } | Self::TrapExit { .. } => {
                TraceCategory::Trap
            }
            Self::Syscall { .. } => TraceCategory::Syscall,
            Self::TaskSwitch { .. } | Self::TimerInterrupt { .. } => {
                TraceCategory::Sched
            }
            Self::AddressSpaceSwitch { .. } | Self::TlbFlush { .. } => {
                TraceCategory::Vm
            }
        }
    }

    /// Monotonically-increasing sequence number attached by the
    /// collector before the event enters the ring buffer.
    pub fn seq(&self) -> u64 {
        match self {
            Self::TrapEnter { .. } => 0,
            _ => 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Trace sink
// ---------------------------------------------------------------------------

/// Abstraction for consuming trace events (terminal, JSON stream, …).
pub trait TraceSink {
    fn emit(&mut self, event: &TraceEvent);
}
