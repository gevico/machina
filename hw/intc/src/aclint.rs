// RISC-V ACLINT (Advanced Core Local Interruptor).
//
// CLINT-compatible single-window MMIO layout:
//   0x0000 + 4*hart : msip register (1 bit used)
//   0x4000 + 8*hart : mtimecmp[hart]
//   0xBFF8           : mtime
//
// mtime is derived from the host monotonic clock at
// 10 MHz (1 tick = 100 ns). When mtimecmp is set to a
// future value, a timer thread sleeps until the deadline
// and then asserts MTI via the IRQ line.
//
// Interior mutability: register state is in
// DeviceRefCell<AclintRegs>, setup state in
// parking_lot::Mutex<SysBusDeviceState>.  All public
// methods take &self so the device can be shared via
// Arc<Aclint> without an outer Mutex.

use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use machina_core::address::GPA;
use machina_core::device_cell::DeviceRefCell;
use machina_core::mobject::{MObject, MObjectInfo};
use machina_core::wfi::WfiWaker;
use machina_hw_core::bus::{SysBus, SysBusDeviceState, SysBusError};
use machina_hw_core::irq::IrqLine;
use machina_hw_core::mdev::MDevice;
use machina_memory::address_space::AddressSpace;
use machina_memory::region::{MemoryRegion, MmioOps};

const MSIP_BASE: u64 = 0x0000;
const MTIMECMP_BASE: u64 = 0x4000;
const MTIME_OFFSET: u64 = 0xBFF8;

/// Shared timer state for the background timer thread.
/// Each hart has its own cancel token (AtomicU64) that
/// the main thread bumps to invalidate stale timers.
struct TimerState {
    cancel_gen: Vec<AtomicU64>,
}

/// Mutable register state protected by DeviceRefCell.
pub struct AclintRegs {
    epoch: Instant,
    mtime_base: u64,
    mtimecmp: Vec<u64>,
    msip: Vec<u32>,
}

pub struct Aclint {
    // Setup-only state behind parking_lot::Mutex so that
    // attach_to_bus / register_mmio / realize_onto can be
    // called through &self (Arc<Aclint>).
    state: parking_lot::Mutex<SysBusDeviceState>,
    num_harts: u32,
    // Runtime register state.
    regs: DeviceRefCell<AclintRegs>,
    // Output lines. Written only during init (behind
    // parking_lot::Mutex), read at runtime via the lock.
    mti_outputs: parking_lot::Mutex<Vec<Option<IrqLine>>>,
    msi_outputs: parking_lot::Mutex<Vec<Option<IrqLine>>>,
    wfi_waker: parking_lot::Mutex<Option<Arc<WfiWaker>>>,
    timer_state: Arc<TimerState>,
    /// Raw pointer to each hart's neg_align (AtomicI32).
    /// Set to -1 by the timer thread to break goto_tb
    /// chains so the exec loop can deliver the timer IRQ.
    neg_align_ptrs: parking_lot::Mutex<Vec<u64>>,
}

impl Aclint {
    pub fn new(num_harts: u32) -> Self {
        Self::new_named("aclint", num_harts)
    }

    pub fn new_named(local_id: &str, num_harts: u32) -> Self {
        let n = num_harts as usize;
        let mut mti = Vec::with_capacity(n);
        let mut msi = Vec::with_capacity(n);
        let mut gens = Vec::with_capacity(n);
        for _ in 0..n {
            mti.push(None);
            msi.push(None);
            gens.push(AtomicU64::new(0));
        }
        Self {
            state: parking_lot::Mutex::new(SysBusDeviceState::new(local_id)),
            num_harts,
            regs: DeviceRefCell::new(AclintRegs {
                epoch: Instant::now(),
                mtime_base: 0,
                mtimecmp: vec![u64::MAX; n],
                msip: vec![0u32; n],
            }),
            mti_outputs: parking_lot::Mutex::new(mti),
            msi_outputs: parking_lot::Mutex::new(msi),
            wfi_waker: parking_lot::Mutex::new(None),
            timer_state: Arc::new(TimerState { cancel_gen: gens }),
            neg_align_ptrs: parking_lot::Mutex::new(vec![0; n]),
        }
    }

    // ---- Setup methods (delegate to locked state) ----

    pub fn attach_to_bus(&self, bus: &mut SysBus) -> Result<(), SysBusError> {
        self.state.lock().attach_to_bus(bus)
    }

    pub fn register_mmio(
        &self,
        region: MemoryRegion,
        base: GPA,
    ) -> Result<(), SysBusError> {
        self.state.lock().register_mmio(region, base)
    }

    pub fn realize_onto(
        &self,
        bus: &mut SysBus,
        address_space: &mut AddressSpace,
    ) -> Result<(), SysBusError> {
        self.state.lock().realize_onto(bus, address_space)
    }

    pub fn unrealize_from(
        &self,
        bus: &mut SysBus,
        address_space: &mut AddressSpace,
    ) -> Result<(), SysBusError> {
        self.cancel_timers();
        self.lower_outputs();
        {
            let wk = self.wfi_waker.lock();
            if let Some(ref w) = *wk {
                w.clear_deadline();
            }
        }
        self.state.lock().unrealize_from(bus, address_space)
    }

    pub fn realized(&self) -> bool {
        self.state.lock().device().is_realized()
    }

    pub fn object_info(&self) -> MObjectInfo {
        self.state.lock().object_info()
    }

    /// Access the inner SysBusDeviceState as `&dyn MDevice`
    /// through a closure (for MOM introspection).
    pub fn with_mdevice<T>(&self, f: impl FnOnce(&dyn MDevice) -> T) -> T {
        let guard = self.state.lock();
        f(&*guard)
    }

    pub fn connect_mti(&self, hart: u32, irq: IrqLine) {
        let mut outputs = self.mti_outputs.lock();
        if (hart as usize) < outputs.len() {
            outputs[hart as usize] = Some(irq);
        }
    }

    pub fn connect_msi(&self, hart: u32, irq: IrqLine) {
        let mut outputs = self.msi_outputs.lock();
        if (hart as usize) < outputs.len() {
            outputs[hart as usize] = Some(irq);
        }
    }

    /// Register the neg_align pointer for a hart so timer
    /// interrupts can break goto_tb chains.
    pub fn connect_neg_align(&self, hart: u32, ptr: u64) {
        let mut ptrs = self.neg_align_ptrs.lock();
        if (hart as usize) < ptrs.len() {
            ptrs[hart as usize] = ptr;
        }
    }

    pub fn connect_wfi_waker(&self, wk: Arc<WfiWaker>) {
        *self.wfi_waker.lock() = Some(wk);
    }

    pub fn reset_runtime(&self) {
        self.cancel_timers();
        {
            let mut regs = self.regs.borrow();
            regs.epoch = Instant::now();
            regs.mtime_base = 0;
            regs.mtimecmp.fill(u64::MAX);
            regs.msip.fill(0);
        }
        self.lower_outputs();
        {
            let wk = self.wfi_waker.lock();
            if let Some(ref w) = *wk {
                w.clear_deadline();
            }
        }
    }

    /// Set mtimecmp[hart] directly (for SBI SET_TIMER).
    ///
    /// Equivalent to a guest write at ACLINT_BASE + 0x4000 +
    /// hart * 8; used by the host SBI backend in builtin mode
    /// so it does not need to go through MMIO decode.
    pub fn set_mtimecmp(&self, hart: usize, val: u64) {
        const MTIMECMP_OFF: u64 = 0x4000;
        self.write(MTIMECMP_OFF + hart as u64 * 8, 8, val);
    }

    /// Current mtime value derived from wall clock.
    pub fn read_mtime(&self) -> u64 {
        let regs = self.regs.borrow();
        Self::read_mtime_with(&regs)
    }

    /// Read mtime from an already-borrowed regs guard.
    fn read_mtime_with(regs: &AclintRegs) -> u64 {
        let elapsed = regs.epoch.elapsed();
        let ticks = (elapsed.as_nanos() / 100) as u64;
        regs.mtime_base.wrapping_add(ticks)
    }

    pub fn timer_irq_pending(&self, hart: u32) -> bool {
        if hart < self.num_harts {
            let regs = self.regs.borrow();
            Self::read_mtime_with(&regs) >= regs.mtimecmp[hart as usize]
        } else {
            false
        }
    }

    /// Schedule a background timer for `hart` that will
    /// assert MTI when the wall clock reaches `mtimecmp`.
    fn schedule_timer(&self, hart: usize) {
        // Read values under regs lock.
        let (cmp, now) = {
            let regs = self.regs.borrow();
            (regs.mtimecmp[hart], Self::read_mtime_with(&regs))
        };
        if cmp == u64::MAX {
            return;
        }
        if cmp <= now {
            return;
        }
        let delta_ticks = cmp - now;
        let delta_ns = delta_ticks.saturating_mul(100).min(100_000_000_000);
        let delay = Duration::from_nanos(delta_ns);

        // Bump the cancel generation so any stale timer
        // for this hart is invalidated.
        let gen = self.timer_state.cancel_gen[hart]
            .fetch_add(1, Ordering::SeqCst)
            + 1;

        let line = {
            let outputs = self.mti_outputs.lock();
            match &outputs[hart] {
                Some(l) => l.clone(),
                None => return,
            }
        };
        let waker = self.wfi_waker.lock().clone();
        let state = Arc::clone(&self.timer_state);
        let neg_ptr = self.neg_align_ptrs.lock()[hart];

        std::thread::spawn(move || {
            std::thread::sleep(delay);
            let cur = state.cancel_gen[hart].load(Ordering::SeqCst);
            if cur != gen {
                return;
            }
            line.set(true);
            // Break goto_tb chain so the exec loop can
            // deliver the timer interrupt promptly.
            if neg_ptr != 0 {
                let p = neg_ptr as *const AtomicI32;
                unsafe { &*p }.store(-1, Ordering::Release);
            }
            if let Some(ref wk) = waker {
                wk.wake();
            }
        });
    }

    /// Set the WFI condvar deadline for timer wakeup.
    fn update_wfi_deadline(&self) {
        let wk = self.wfi_waker.lock();
        if let Some(ref w) = *wk {
            let regs = self.regs.borrow();
            let nearest =
                regs.mtimecmp.iter().copied().min().unwrap_or(u64::MAX);
            if nearest == u64::MAX {
                w.clear_deadline();
                return;
            }
            let now = Self::read_mtime_with(&regs);
            if nearest <= now {
                return;
            }
            let delta_ticks = nearest - now;
            let delta_ns = delta_ticks.saturating_mul(100).min(100_000_000_000);
            let deadline = Instant::now() + Duration::from_nanos(delta_ns);
            w.set_deadline(deadline);
        }
    }

    fn update_mti(&self) {
        let regs = self.regs.borrow();
        let mtime = Self::read_mtime_with(&regs);
        let outputs = self.mti_outputs.lock();
        for hart in 0..self.num_harts as usize {
            let pending = mtime >= regs.mtimecmp[hart];
            if let Some(ref line) = outputs[hart] {
                line.set(pending);
            }
        }
    }

    fn cancel_timers(&self) {
        for gen in &self.timer_state.cancel_gen {
            gen.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn lower_outputs(&self) {
        let mti = self.mti_outputs.lock();
        for line in mti.iter().flatten() {
            line.lower();
        }
        let msi = self.msi_outputs.lock();
        for line in msi.iter().flatten() {
            line.lower();
        }
    }

    pub fn read(&self, offset: u64, _size: u32) -> u64 {
        if offset == MTIME_OFFSET {
            return self.read_mtime();
        }
        let regs = self.regs.borrow();
        if offset >= MTIMECMP_BASE {
            let hart = ((offset - MTIMECMP_BASE) / 8) as usize;
            if hart < self.num_harts as usize {
                return regs.mtimecmp[hart];
            }
            return 0;
        }
        let hart = ((offset - MSIP_BASE) / 4) as usize;
        if hart < self.num_harts as usize {
            regs.msip[hart] as u64
        } else {
            0
        }
    }

    pub fn write(&self, offset: u64, _size: u32, val: u64) {
        if offset == MTIME_OFFSET {
            self.cancel_timers();
            {
                let mut regs = self.regs.borrow();
                regs.mtime_base = val;
                regs.epoch = Instant::now();
            }
            self.update_mti();
            return;
        }
        if offset >= MTIMECMP_BASE {
            let hart = ((offset - MTIMECMP_BASE) / 8) as usize;
            if hart < self.num_harts as usize {
                self.timer_state.cancel_gen[hart]
                    .fetch_add(1, Ordering::SeqCst);

                let pending = {
                    let mut regs = self.regs.borrow();
                    regs.mtimecmp[hart] = val;
                    Self::read_mtime_with(&regs) >= regs.mtimecmp[hart]
                };
                {
                    let outputs = self.mti_outputs.lock();
                    if let Some(ref line) = outputs[hart] {
                        line.set(pending);
                    }
                }
                if !pending {
                    self.schedule_timer(hart);
                }
                self.update_wfi_deadline();
            }
            return;
        }
        let hart = ((offset - MSIP_BASE) / 4) as usize;
        if hart < self.num_harts as usize {
            let v = (val as u32) & 1;
            self.regs.borrow().msip[hart] = v;
            let outputs = self.msi_outputs.lock();
            if let Some(ref line) = outputs[hart] {
                line.set(v != 0);
            }
        }
    }
}

pub struct AclintMmio(pub Arc<Aclint>);

impl MmioOps for AclintMmio {
    fn read(&self, offset: u64, size: u32) -> u64 {
        self.0.read(offset, size)
    }

    fn write(&self, offset: u64, size: u32, val: u64) {
        self.0.write(offset, size, val);
    }
}
