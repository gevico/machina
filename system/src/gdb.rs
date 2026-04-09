// GDB stub state management.
//
// Coordinates between the GDB server thread and the
// CPU execution loop for breakpoints, single-step,
// and pause/resume. Includes register snapshot for
// cross-thread CPU state access.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// CPU run state from the GDB stub's perspective.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GdbRunState {
    Running,
    PauseRequested,
    Paused,
    Stepping,
}

/// Watchpoint type (GDB Z2/Z3/Z4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchType {
    Write,
    Read,
    Access,
}

/// Watchpoint hit info recorded by memory helpers.
#[derive(Clone, Copy, Debug)]
pub struct WatchpointHit {
    pub addr: u64,
    pub wtype: WatchType,
}

/// Snapshot of RISC-V CPU registers for GDB access.
/// Filled by the exec loop when CPU pauses, read by
/// the GDB server thread.
#[derive(Clone)]
pub struct GdbCpuSnapshot {
    /// x0-x31 general-purpose registers.
    pub gpr: [u64; 32],
    /// f0-f31 floating-point registers.
    pub fpr: [u64; 32],
    /// Program counter.
    pub pc: u64,
    /// Current privilege level (0=U, 1=S, 3=M).
    pub priv_level: u8,
    /// CSR values (indexed by GDB CSR register number
    /// offset, i.e. gdb_reg - 66).
    pub csr: Vec<u64>,
    /// Set when GDB writes registers that need to be
    /// restored before the CPU resumes.
    pub dirty: bool,
}

#[allow(clippy::derivable_impls)]
impl Default for GdbCpuSnapshot {
    fn default() -> Self {
        Self {
            gpr: [0u64; 32],
            fpr: [0u64; 32],
            pc: 0,
            priv_level: 0,
            csr: Vec::new(),
            dirty: false,
        }
    }
}

/// Shared GDB debug state between the server and exec loop.
pub struct GdbState {
    inner: Mutex<GdbInner>,
    /// Condvar signaled when exec loop parks.
    pause_cv: Condvar,
    /// Condvar signaled when GDB resumes.
    resume_cv: Condvar,
    /// Whether a GDB client is connected.
    connected: AtomicBool,
    /// Per-CPU register snapshots (valid when paused).
    snapshots: Mutex<Vec<GdbCpuSnapshot>>,
    /// Number of vCPUs.
    cpu_count: AtomicUsize,
    /// Host pointer to guest RAM for memory access.
    ram_ptr: AtomicU64,
    /// Guest RAM size in bytes.
    ram_size: AtomicU64,
    /// Guest RAM end (base + size).
    ram_end: AtomicU64,
    /// Host pointer to AddressSpace for MMIO.
    as_ptr: AtomicU64,
    /// Watchpoint count for fast bail-out in helpers.
    watchpoint_count: AtomicUsize,
    /// Physical memory mode (bypass MMU for GDB mem).
    phy_mem_mode: AtomicBool,
}

struct GdbInner {
    state: GdbRunState,
    stop_reason: StopReason,
    /// Thread ID that caused the stop (1-indexed).
    stop_thread: usize,
    /// Current "g" CPU index (0-indexed).
    g_cpu_idx: usize,
    /// Current "c" CPU index (0-indexed).
    c_cpu_idx: usize,
    breakpoints: BTreeSet<u64>,
    hw_breakpoints: BTreeSet<u64>,
    /// Watchpoints: addr -> (len, type).
    watchpoints: BTreeMap<u64, (usize, WatchType)>,
    /// Last watchpoint hit (for stop reply).
    watchpoint_hit: Option<WatchpointHit>,
    detached: bool,
}

impl GdbState {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(GdbInner {
                state: GdbRunState::Paused,
                stop_reason: StopReason::Pause,
                stop_thread: 1,
                g_cpu_idx: 0,
                c_cpu_idx: 0,
                breakpoints: BTreeSet::new(),
                hw_breakpoints: BTreeSet::new(),
                watchpoints: BTreeMap::new(),
                watchpoint_hit: None,
                detached: false,
            }),
            pause_cv: Condvar::new(),
            resume_cv: Condvar::new(),
            connected: AtomicBool::new(false),
            snapshots: Mutex::new(vec![GdbCpuSnapshot::default()]),
            cpu_count: AtomicUsize::new(1),
            ram_ptr: AtomicU64::new(0),
            ram_size: AtomicU64::new(0),
            ram_end: AtomicU64::new(0),
            as_ptr: AtomicU64::new(0),
            watchpoint_count: AtomicUsize::new(0),
            phy_mem_mode: AtomicBool::new(false),
        }
    }

    /// Set the number of vCPUs.
    pub fn set_cpu_count(&self, count: usize) {
        self.cpu_count.store(count, Ordering::SeqCst);
        let mut snaps = self.snapshots.lock().unwrap();
        snaps.resize_with(count, GdbCpuSnapshot::default);
    }

    pub fn cpu_count(&self) -> usize {
        self.cpu_count.load(Ordering::SeqCst)
    }

    /// Get the current "g" CPU index (0-indexed).
    pub fn g_cpu_idx(&self) -> usize {
        self.inner.lock().unwrap().g_cpu_idx
    }

    /// Set the "g" CPU index. Returns false if invalid.
    pub fn set_g_cpu(&self, idx: usize) -> bool {
        let count = self.cpu_count();
        if idx >= count {
            return false;
        }
        self.inner.lock().unwrap().g_cpu_idx = idx;
        true
    }

    /// Get the current "c" CPU index (0-indexed).
    pub fn c_cpu_idx(&self) -> usize {
        self.inner.lock().unwrap().c_cpu_idx
    }

    /// Set the "c" CPU index. Returns false if invalid.
    pub fn set_c_cpu(&self, idx: usize) -> bool {
        let count = self.cpu_count();
        if idx >= count {
            return false;
        }
        self.inner.lock().unwrap().c_cpu_idx = idx;
        true
    }

    /// Set the thread that caused the stop (1-indexed).
    pub fn set_stop_thread(&self, tid: usize) {
        self.inner.lock().unwrap().stop_thread = tid;
    }

    /// Get the stop thread ID (1-indexed).
    pub fn stop_thread(&self) -> usize {
        self.inner.lock().unwrap().stop_thread
    }

    pub fn set_connected(&self, connected: bool) {
        self.connected.store(connected, Ordering::SeqCst);
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    // -- Memory access configuration --

    /// Set the host RAM pointer and size for memory R/W.
    /// Called once during machine setup.
    pub fn set_mem_access(
        &self,
        ram_ptr: *const u8,
        ram_size: u64,
        ram_base: u64,
        as_ptr: u64,
    ) {
        self.ram_ptr.store(ram_ptr as u64, Ordering::SeqCst);
        self.ram_size.store(ram_size, Ordering::SeqCst);
        self.ram_end.store(ram_base + ram_size, Ordering::SeqCst);
        self.as_ptr.store(as_ptr, Ordering::SeqCst);
    }

    /// Read guest memory at physical address.
    pub fn read_memory(&self, addr: u64, len: usize) -> Vec<u8> {
        let ram_ptr = self.ram_ptr.load(Ordering::SeqCst);
        let ram_end = self.ram_end.load(Ordering::SeqCst);
        let as_ptr = self.as_ptr.load(Ordering::SeqCst);
        if ram_ptr == 0 || len == 0 {
            return vec![0; len];
        }
        let ram_base = 0x8000_0000u64; // RAM_BASE
        if addr >= ram_base && addr + len as u64 <= ram_end {
            let off = (addr - ram_base) as usize;
            let ptr = unsafe { (ram_ptr as *const u8).add(off) };
            let mut buf = vec![0u8; len];
            unsafe {
                std::ptr::copy_nonoverlapping(ptr, buf.as_mut_ptr(), len);
            }
            buf
        } else if as_ptr != 0 {
            // MMIO: fall back to AddressSpace.
            let mut buf = vec![0u8; len];
            use machina_core::address::GPA;
            use machina_memory::address_space::AddressSpace;
            let as_ = unsafe { &*(as_ptr as *const AddressSpace) };
            for (i, byte) in buf.iter_mut().enumerate() {
                *byte = as_.read(GPA::new(addr + i as u64), 1) as u8;
            }
            buf
        } else {
            vec![0; len]
        }
    }

    /// Write guest memory at physical address.
    pub fn write_memory(&self, addr: u64, data: &[u8]) -> bool {
        let ram_ptr = self.ram_ptr.load(Ordering::SeqCst);
        let ram_end = self.ram_end.load(Ordering::SeqCst);
        let as_ptr = self.as_ptr.load(Ordering::SeqCst);
        if ram_ptr == 0 || data.is_empty() {
            return false;
        }
        let ram_base = 0x8000_0000u64;
        if addr >= ram_base && addr + data.len() as u64 <= ram_end {
            let off = (addr - ram_base) as usize;
            let ptr = unsafe { (ram_ptr as *mut u8).add(off) };
            unsafe {
                std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
            }
            true
        } else if as_ptr != 0 {
            use machina_core::address::GPA;
            use machina_memory::address_space::AddressSpace;
            let as_ = unsafe { &*(as_ptr as *const AddressSpace) };
            for (i, &byte) in data.iter().enumerate() {
                as_.write(GPA::new(addr + i as u64), 1, byte as u64);
            }
            true
        } else {
            false
        }
    }

    // -- Register snapshot --

    /// Save CPU register state into snapshot for cpu_idx.
    pub fn save_snapshot(
        &self,
        cpu_idx: usize,
        gpr: &[u64; 32],
        fpr: &[u64; 32],
        pc: u64,
        priv_level: u8,
        csr: &[u64],
    ) {
        let mut snaps = self.snapshots.lock().unwrap();
        if cpu_idx >= snaps.len() {
            snaps.resize_with(cpu_idx + 1, GdbCpuSnapshot::default);
        }
        let snap = &mut snaps[cpu_idx];
        snap.gpr.copy_from_slice(gpr);
        snap.fpr.copy_from_slice(fpr);
        snap.pc = pc;
        snap.priv_level = priv_level;
        snap.csr = csr.to_vec();
        snap.dirty = false;
    }

    /// Read the snapshot for the current "g" CPU.
    pub fn read_snapshot(&self) -> GdbCpuSnapshot {
        let idx = self.inner.lock().unwrap().g_cpu_idx;
        self.read_snapshot_for(idx)
    }

    /// Read the snapshot for a specific CPU.
    pub fn read_snapshot_for(&self, cpu_idx: usize) -> GdbCpuSnapshot {
        let snaps = self.snapshots.lock().unwrap();
        snaps.get(cpu_idx).cloned().unwrap_or_default()
    }

    /// Take dirty snapshot for cpu_idx if modified.
    pub fn take_dirty_snapshot(
        &self,
        cpu_idx: usize,
    ) -> Option<GdbCpuSnapshot> {
        let mut snaps = self.snapshots.lock().unwrap();
        if let Some(snap) = snaps.get_mut(cpu_idx) {
            if snap.dirty {
                snap.dirty = false;
                return Some(snap.clone());
            }
        }
        None
    }

    /// Write a single register in the current "g" CPU
    /// snapshot.
    /// reg: 0-31=GPR, 32=PC, 33-64=FPR, 65=priv,
    /// 66+=CSR.
    pub fn write_register(&self, reg: usize, val: u64) -> bool {
        let idx = self.inner.lock().unwrap().g_cpu_idx;
        let mut snaps = self.snapshots.lock().unwrap();
        let snap = match snaps.get_mut(idx) {
            Some(s) => s,
            None => return false,
        };
        match reg {
            0 => { /* x0 hardwired to 0 */ }
            1..=31 => snap.gpr[reg] = val,
            32 => snap.pc = val,
            33..=64 => snap.fpr[reg - 33] = val,
            65 => { /* priv level read-only */ }
            r if r >= 66 => {
                let csr_idx = r - 66;
                if csr_idx < snap.csr.len() {
                    snap.csr[csr_idx] = val;
                } else {
                    return false;
                }
            }
            _ => return false,
        }
        snap.dirty = true;
        true
    }

    /// Write all registers from GDB G packet data.
    pub fn write_all_registers(&self, data: &[u8]) -> bool {
        let idx = self.inner.lock().unwrap().g_cpu_idx;
        let mut snaps = self.snapshots.lock().unwrap();
        let snap = match snaps.get_mut(idx) {
            Some(s) => s,
            None => return false,
        };
        // Need at least 65 * 8 bytes (32 GPR + PC + 32
        // FPR).
        let need = 65 * 8;
        if data.len() < need {
            return false;
        }
        for i in 0..32 {
            let off = i * 8;
            snap.gpr[i] =
                u64::from_le_bytes(data[off..off + 8].try_into().unwrap());
        }
        snap.pc = u64::from_le_bytes(data[256..264].try_into().unwrap());
        for i in 0..32 {
            let off = (33 + i) * 8;
            snap.fpr[i] =
                u64::from_le_bytes(data[off..off + 8].try_into().unwrap());
        }
        snap.dirty = true;
        true
    }

    // -- Breakpoint management --

    pub fn set_breakpoint(&self, addr: u64) -> bool {
        self.inner.lock().unwrap().breakpoints.insert(addr);
        true
    }

    pub fn remove_breakpoint(&self, addr: u64) -> bool {
        self.inner.lock().unwrap().breakpoints.remove(&addr);
        true
    }

    pub fn set_hw_breakpoint(&self, addr: u64) -> bool {
        self.inner.lock().unwrap().hw_breakpoints.insert(addr);
        true
    }

    pub fn remove_hw_breakpoint(&self, addr: u64) -> bool {
        self.inner.lock().unwrap().hw_breakpoints.remove(&addr);
        true
    }

    pub fn hit_breakpoint(&self, pc: u64) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.breakpoints.contains(&pc) || inner.hw_breakpoints.contains(&pc)
    }

    pub fn has_breakpoints(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        !inner.breakpoints.is_empty() || !inner.hw_breakpoints.is_empty()
    }

    /// Check if any breakpoint falls within (start, end).
    /// Used by the exec loop to detect TBs that span a
    /// breakpoint and need single-step replacement.
    pub fn breakpoint_in_range(&self, start: u64, end: u64) -> bool {
        let inner = self.inner.lock().unwrap();
        inner
            .breakpoints
            .iter()
            .chain(inner.hw_breakpoints.iter())
            .any(|&addr| addr > start && addr < end)
    }

    // -- Watchpoint management --

    pub fn set_watchpoint(
        &self,
        addr: u64,
        len: usize,
        wtype: WatchType,
    ) -> bool {
        let mut inner = self.inner.lock().unwrap();
        inner.watchpoints.insert(addr, (len, wtype));
        self.watchpoint_count
            .store(inner.watchpoints.len(), Ordering::SeqCst);
        true
    }

    pub fn remove_watchpoint(&self, addr: u64) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let removed = inner.watchpoints.remove(&addr);
        self.watchpoint_count
            .store(inner.watchpoints.len(), Ordering::SeqCst);
        removed.is_some()
    }

    /// Check if an address range hits a watchpoint.
    /// Returns the matching watchpoint if found.
    pub fn check_watchpoint(
        &self,
        addr: u64,
        size: u32,
        is_write: bool,
    ) -> Option<WatchpointHit> {
        if self.watchpoint_count.load(Ordering::Relaxed) == 0 {
            return None;
        }
        let inner = self.inner.lock().unwrap();
        let access_end = addr + size as u64;
        for (&wp_addr, &(wp_len, wtype)) in &inner.watchpoints {
            let wp_end = wp_addr + wp_len as u64;
            if addr < wp_end && access_end > wp_addr {
                let hit = match wtype {
                    WatchType::Write => is_write,
                    WatchType::Read => !is_write,
                    WatchType::Access => true,
                };
                if hit {
                    return Some(WatchpointHit {
                        addr: wp_addr,
                        wtype,
                    });
                }
            }
        }
        None
    }

    /// Record a watchpoint hit for the stop reply.
    pub fn set_watchpoint_hit(&self, hit: WatchpointHit) {
        self.inner.lock().unwrap().watchpoint_hit = Some(hit);
    }

    /// Take the last watchpoint hit (clears it).
    pub fn take_watchpoint_hit(&self) -> Option<WatchpointHit> {
        self.inner.lock().unwrap().watchpoint_hit.take()
    }

    // -- Physical memory mode --

    pub fn set_phy_mem_mode(&self, enabled: bool) {
        self.phy_mem_mode.store(enabled, Ordering::SeqCst);
    }

    pub fn phy_mem_mode(&self) -> bool {
        self.phy_mem_mode.load(Ordering::SeqCst)
    }

    // -- Run state management --

    /// Set the stop reason (called from exec loop).
    pub fn set_stop_reason(&self, reason: StopReason) {
        self.inner.lock().unwrap().stop_reason = reason;
    }

    /// Get the current stop reason.
    pub fn get_stop_reason(&self) -> StopReason {
        self.inner.lock().unwrap().stop_reason
    }

    pub fn run_state(&self) -> GdbRunState {
        self.inner.lock().unwrap().state
    }

    /// Request the CPU to pause (non-blocking).
    pub fn request_pause(&self) {
        let mut inner = self.inner.lock().unwrap();
        if inner.state == GdbRunState::Paused {
            return;
        }
        inner.state = GdbRunState::PauseRequested;
    }

    /// Wait until the exec loop has parked.
    pub fn wait_paused(&self) {
        let mut inner = self.inner.lock().unwrap();
        while inner.state != GdbRunState::Paused {
            inner = self.pause_cv.wait(inner).unwrap();
        }
    }

    /// Wait until the exec loop has parked, with timeout.
    /// Returns true if paused, false if timed out.
    pub fn wait_paused_timeout(&self, timeout: Duration) -> bool {
        let inner = self.inner.lock().unwrap();
        if inner.state == GdbRunState::Paused {
            return true;
        }
        let result = self.pause_cv.wait_timeout(inner, timeout).unwrap();
        result.0.state == GdbRunState::Paused
    }

    /// Resume the CPU from paused state.
    pub fn request_resume(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.state = GdbRunState::Running;
        self.resume_cv.notify_all();
    }

    /// Request single-step.
    pub fn request_step(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.state = GdbRunState::Stepping;
        self.resume_cv.notify_all();
    }

    pub fn detach(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.detached = true;
        inner.state = GdbRunState::Running;
        self.resume_cv.notify_all();
        self.connected.store(false, Ordering::SeqCst);
    }

    pub fn is_detached(&self) -> bool {
        self.inner.lock().unwrap().detached
    }

    /// Called by the exec loop to check if it should
    /// pause. If PauseRequested/Paused, parks and
    /// blocks until resumed. Returns true if quit.
    pub fn check_and_wait(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.detached {
            return false;
        }
        match inner.state {
            GdbRunState::PauseRequested | GdbRunState::Paused => {
                inner.state = GdbRunState::Paused;
                self.pause_cv.notify_all();
                while inner.state == GdbRunState::Paused {
                    inner = self.resume_cv.wait(inner).unwrap();
                }
                false
            }
            _ => false,
        }
    }

    /// Called by the exec loop after executing one TB in
    /// stepping mode. Transitions Stepping -> Paused.
    pub fn complete_step(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.state == GdbRunState::Stepping {
            inner.state = GdbRunState::Paused;
            self.pause_cv.notify_all();
            while inner.state == GdbRunState::Paused {
                inner = self.resume_cv.wait(inner).unwrap();
            }
            return true;
        }
        false
    }

    pub fn is_stepping(&self) -> bool {
        self.inner.lock().unwrap().state == GdbRunState::Stepping
    }
}

impl Default for GdbState {
    fn default() -> Self {
        Self::new()
    }
}

// ---- GdbStateTarget: GdbTarget via GdbState ----

use machina_gdbstub::handler::{GdbHandler, GdbTarget, StopReason};
use machina_gdbstub::protocol;

const NUM_GPRS: usize = 32;
const NUM_FPRS: usize = 32;
const GDB_NUM_REGS: usize = NUM_GPRS + 1 + NUM_FPRS;

/// Bridge that implements `GdbTarget` by delegating to
/// `GdbState` for cross-thread CPU access.
pub struct GdbStateTarget<'a> {
    gs: &'a GdbState,
}

impl<'a> GdbStateTarget<'a> {
    pub fn new(gs: &'a GdbState) -> Self {
        Self { gs }
    }
}

impl GdbTarget for GdbStateTarget<'_> {
    fn read_registers(&self) -> Vec<u8> {
        let snap = self.gs.read_snapshot();
        let mut buf = Vec::with_capacity(GDB_NUM_REGS * 8);
        for &val in &snap.gpr {
            buf.extend_from_slice(&val.to_le_bytes());
        }
        buf.extend_from_slice(&snap.pc.to_le_bytes());
        for &val in &snap.fpr {
            buf.extend_from_slice(&val.to_le_bytes());
        }
        buf
    }

    fn write_registers(&mut self, data: &[u8]) -> bool {
        self.gs.write_all_registers(data)
    }

    fn read_register(&self, reg: usize) -> Vec<u8> {
        let snap = self.gs.read_snapshot();
        match reg {
            0..=31 => snap.gpr[reg].to_le_bytes().to_vec(),
            32 => snap.pc.to_le_bytes().to_vec(),
            33..=64 => snap.fpr[reg - 33].to_le_bytes().to_vec(),
            65 => (snap.priv_level as u64).to_le_bytes().to_vec(),
            r if r >= 66 => {
                let idx = r - 66;
                if idx < snap.csr.len() {
                    snap.csr[idx].to_le_bytes().to_vec()
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        }
    }

    fn write_register(&mut self, reg: usize, val: &[u8]) -> bool {
        if val.len() < 8 {
            return false;
        }
        let v = u64::from_le_bytes(val[..8].try_into().unwrap());
        self.gs.write_register(reg, v)
    }

    fn read_memory(&self, addr: u64, len: usize) -> Vec<u8> {
        self.gs.read_memory(addr, len)
    }

    fn write_memory(&mut self, addr: u64, data: &[u8]) -> bool {
        self.gs.write_memory(addr, data)
    }

    fn set_breakpoint(&mut self, type_: u8, addr: u64, kind: u32) -> bool {
        match type_ {
            0 => self.gs.set_breakpoint(addr),
            1 => self.gs.set_hw_breakpoint(addr),
            2 => self
                .gs
                .set_watchpoint(addr, kind as usize, WatchType::Write),
            3 => self.gs.set_watchpoint(addr, kind as usize, WatchType::Read),
            4 => self
                .gs
                .set_watchpoint(addr, kind as usize, WatchType::Access),
            _ => false,
        }
    }

    fn remove_breakpoint(&mut self, type_: u8, addr: u64, _kind: u32) -> bool {
        match type_ {
            0 => self.gs.remove_breakpoint(addr),
            1 => self.gs.remove_hw_breakpoint(addr),
            2..=4 => self.gs.remove_watchpoint(addr),
            _ => false,
        }
    }

    fn resume(&mut self) {
        self.gs.request_resume();
    }

    fn step(&mut self) {
        self.gs.request_step();
    }

    fn get_pc(&self) -> u64 {
        self.gs.read_snapshot().pc
    }

    fn get_stop_reason(&self) -> StopReason {
        self.gs.get_stop_reason()
    }

    fn cpu_count(&self) -> usize {
        self.gs.cpu_count()
    }

    fn set_g_cpu(&mut self, idx: usize) -> bool {
        self.gs.set_g_cpu(idx)
    }

    fn set_c_cpu(&mut self, idx: usize) -> bool {
        self.gs.set_c_cpu(idx)
    }

    fn thread_alive(&self, tid: usize) -> bool {
        tid > 0 && tid <= self.gs.cpu_count()
    }

    fn set_phy_mem_mode(&mut self, enabled: bool) -> bool {
        self.gs.set_phy_mem_mode(enabled);
        true
    }

    fn phy_mem_mode(&self) -> bool {
        self.gs.phy_mem_mode()
    }

    fn stop_thread(&self) -> usize {
        self.gs.stop_thread()
    }

    fn take_watchpoint_hit(&mut self) -> Option<(u64, u8)> {
        self.gs.take_watchpoint_hit().map(|hit| {
            let code = match hit.wtype {
                WatchType::Write => 0,
                WatchType::Read => 1,
                WatchType::Access => 2,
            };
            (hit.addr, code)
        })
    }
}

// ---- Resume/step action detection ----

/// Resume action intercepted by serve().
enum ResumeAction {
    Continue,
    Step,
}

/// Check if a packet is a resume/step command that
/// serve() should handle directly (with Ctrl-C support)
/// instead of delegating to the handler.
fn check_resume_packet(packet: &str) -> Option<ResumeAction> {
    let first = packet.chars().next()?;
    match first {
        'c' => Some(ResumeAction::Continue),
        'C' => Some(ResumeAction::Continue),
        's' => Some(ResumeAction::Step),
        'S' => Some(ResumeAction::Step),
        'v' => {
            let rest = packet.strip_prefix("vCont;")?;
            if rest.is_empty() {
                return None;
            }
            let first_action = rest.split(';').next().unwrap_or("");
            let cmd = first_action.split(':').next().unwrap_or("");
            match cmd {
                "c" | "C" => Some(ResumeAction::Continue),
                "s" | "S" => Some(ResumeAction::Step),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Format a GDB stop reply for the given reason.
fn stop_reply(reason: StopReason, gs: &GdbState) -> String {
    let tid = gs.stop_thread();
    match reason {
        StopReason::Breakpoint => {
            format!("T05thread:{:02x};swbreak:;", tid,)
        }
        StopReason::Watchpoint { addr, wtype } => {
            let prefix = match wtype {
                0 => "watch",
                1 => "rwatch",
                _ => "awatch",
            };
            format!("T05thread:{:02x};{}:{:x};", tid, prefix, addr,)
        }
        StopReason::Step => {
            format!("T05thread:{:02x};", tid)
        }
        StopReason::Pause => {
            format!("T02thread:{:02x};", tid)
        }
        StopReason::Terminated => "W00".to_string(),
    }
}

/// Wait for the CPU to stop during a continue, while
/// polling the TCP socket for Ctrl-C (0x03 byte).
/// Uses non-blocking peek to avoid consuming data.
fn wait_for_stop_with_ctrl_c(
    gs: &GdbState,
    stream: &mut std::net::TcpStream,
) -> std::io::Result<StopReason> {
    stream.set_nonblocking(true)?;
    let result = loop {
        // Check if CPU has paused.
        if gs.wait_paused_timeout(Duration::from_millis(50)) {
            break Ok(gs.get_stop_reason());
        }

        // Timeout: check for Ctrl-C on socket.
        let mut peek_buf = [0u8; 1];
        match stream.peek(&mut peek_buf) {
            Ok(1) if peek_buf[0] == 0x03 => {
                // Consume the Ctrl-C byte.
                use std::io::Read;
                let _ = stream.read(&mut peek_buf);
                gs.set_stop_reason(StopReason::Pause);
                gs.request_pause();
                // Continue loop to wait for CPU to
                // actually park.
            }
            Ok(_) => {
                // Unexpected data during continue.
                // Break and let main loop handle it.
                break Ok(gs.get_stop_reason());
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No Ctrl-C, loop back.
            }
            Err(e) => break Err(e),
        }
    };
    stream.set_nonblocking(false)?;
    result
}

// ---- GDB server entry point ----

/// Run the GDB RSP server loop on an accepted TCP stream.
///
/// Handles c/s/vCont by resuming the CPU and waiting for
/// a stop event (breakpoint, step completion, Ctrl-C) with
/// non-blocking Ctrl-C polling. All other packets are
/// dispatched to GdbHandler.
pub fn serve(
    mut stream: std::net::TcpStream,
    gs: &GdbState,
) -> std::io::Result<()> {
    stream.set_nodelay(true)?;

    // Wait for CPU to be paused.
    gs.request_pause();
    gs.wait_paused();

    let mut target = GdbStateTarget::new(gs);
    let xml = crate::gdb_csr::build_target_xml();
    let handler = GdbHandler::with_target_xml(Box::leak(xml.into_boxed_str()));
    let mut handler = handler;
    // Initial stop reply on attach.
    protocol::send_packet(&mut stream, "T05thread:01;")?;

    loop {
        let packet = match protocol::recv_packet(&mut stream) {
            Ok(p) => p,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::UnexpectedEof {
                    break;
                }
                continue;
            }
        };

        // Check if this is a resume/step command.
        if let Some(action) = check_resume_packet(&packet) {
            match action {
                ResumeAction::Continue => {
                    gs.request_resume();
                    let reason = wait_for_stop_with_ctrl_c(gs, &mut stream)?;
                    let reply = stop_reply(reason, gs);
                    protocol::send_packet(&mut stream, &reply)?;
                    continue;
                }
                ResumeAction::Step => {
                    gs.request_step();
                    gs.wait_paused();
                    let reason = gs.get_stop_reason();
                    let reply = stop_reply(reason, gs);
                    protocol::send_packet(&mut stream, &reply)?;
                    continue;
                }
            }
        }

        // Ctrl-C: pause CPU before handler generates
        // stop reply.
        if packet == "\x03" {
            gs.set_stop_reason(StopReason::Pause);
            gs.request_pause();
            gs.wait_paused();
            let reply = stop_reply(StopReason::Pause, gs);
            protocol::send_packet(&mut stream, &reply)?;
            continue;
        }

        // All other packets: dispatch to handler.
        let response = match handler.handle(&packet, &mut target) {
            Some(resp) => resp,
            None => {
                let _ = protocol::send_packet(&mut stream, "OK");
                break;
            }
        };

        if let Err(e) = protocol::send_packet(&mut stream, &response) {
            let _ = e;
            break;
        }
    }

    gs.detach();
    Ok(())
}
