// SiFive PLIC (Platform-Level Interrupt Controller).
//
// MMIO layout (per RISC-V PLIC spec):
//   0x000000 .. 0x000FFF  priority[0..N]  (4 bytes each)
//   0x001000 .. 0x001FFF  pending bitmap  (32 sources/word)
//   0x002000 + 0x80*ctx   enable bitmap per context
//   0x200000 + 0x1000*ctx threshold (off 0), claim/complete (off 4)
//
// Source-layer state (priority, pending, source_level) uses
// AtomicU32 for lock-free IRQ propagation.  Context-layer
// state (enable, threshold, claim) is behind DeviceRefCell
// for MMIO claim/complete serialization.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use machina_core::address::GPA;
use machina_core::device_cell::DeviceRefCell;
use machina_core::mobject::{MObject, MObjectInfo};
use machina_hw_core::bus::{SysBus, SysBusDeviceState, SysBusError};
use machina_hw_core::irq::InterruptSource;
use machina_hw_core::mdev::MDevice;
use machina_memory::address_space::AddressSpace;
use machina_memory::region::{MemoryRegion, MmioOps};

const PRIORITY_BASE: u64 = 0x00_0000;
const PENDING_BASE: u64 = 0x00_1000;
const ENABLE_BASE: u64 = 0x00_2000;
const ENABLE_STRIDE: u64 = 0x80;
const CONTEXT_BASE: u64 = 0x20_0000;
const CONTEXT_STRIDE: u64 = 0x1000;

/// Per-context mutable state protected by DeviceRefCell.
pub struct PlicContexts {
    enable: Vec<Vec<u32>>,
    threshold: Vec<u32>,
    claim: Vec<u32>,
}

pub struct Plic {
    // Setup-only state behind parking_lot::Mutex so that
    // attach_to_bus / register_mmio / realize_onto can be
    // called through &self (Arc<Plic>).
    state: parking_lot::Mutex<SysBusDeviceState>,
    num_sources: u32,
    num_contexts: u32,
    // Lock-free source layer.
    priority: Vec<AtomicU32>,
    pending: Vec<AtomicU32>,
    source_level: Vec<AtomicU32>,
    // Locked context layer.
    contexts: DeviceRefCell<PlicContexts>,
    // Output lines. Written only during init (behind
    // parking_lot::Mutex), read lock-free at runtime via
    // the immutable Vec after init completes.
    context_outputs: parking_lot::Mutex<Vec<Option<InterruptSource>>>,
}

// SAFETY: All mutable state is either atomic or behind
// DeviceRefCell / parking_lot::Mutex.
unsafe impl Sync for Plic {}

impl Plic {
    pub fn new(num_sources: u32, num_contexts: u32) -> Self {
        Self::new_named("plic", num_sources, num_contexts)
    }

    pub fn new_named(
        local_id: &str,
        num_sources: u32,
        num_contexts: u32,
    ) -> Self {
        let words = num_sources.div_ceil(32) as usize;
        let mut priority = Vec::with_capacity(num_sources as usize);
        for _ in 0..num_sources {
            priority.push(AtomicU32::new(0));
        }
        let mut pending = Vec::with_capacity(words);
        for _ in 0..words {
            pending.push(AtomicU32::new(0));
        }
        let mut source_level = Vec::with_capacity(num_sources as usize);
        for _ in 0..num_sources {
            source_level.push(AtomicU32::new(0));
        }
        let mut outputs = Vec::with_capacity(num_contexts as usize);
        for _ in 0..num_contexts {
            outputs.push(None);
        }
        Self {
            state: parking_lot::Mutex::new(SysBusDeviceState::new(local_id)),
            num_sources,
            num_contexts,
            priority,
            pending,
            source_level,
            contexts: DeviceRefCell::new(PlicContexts {
                enable: vec![vec![0u32; words]; num_contexts as usize],
                threshold: vec![0u32; num_contexts as usize],
                claim: vec![0u32; num_contexts as usize],
            }),
            context_outputs: parking_lot::Mutex::new(outputs),
        }
    }

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
        self.lower_outputs();
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

    /// Connect an output IRQ line for `ctx`.
    pub fn connect_context_output(&self, ctx: u32, irq: InterruptSource) {
        let mut outputs = self.context_outputs.lock();
        if (ctx as usize) < outputs.len() {
            outputs[ctx as usize] = Some(irq);
        }
    }

    pub fn reset_runtime(&self) {
        for p in &self.priority {
            p.store(0, Ordering::Relaxed);
        }
        for p in &self.pending {
            p.store(0, Ordering::Relaxed);
        }
        for s in &self.source_level {
            s.store(0, Ordering::Relaxed);
        }
        {
            let mut ctx = self.contexts.borrow();
            for words in &mut ctx.enable {
                words.fill(0);
            }
            ctx.threshold.fill(0);
            ctx.claim.fill(0);
        }
        self.lower_outputs();
    }

    fn lower_outputs(&self) {
        let outputs = self.context_outputs.lock();
        for line in outputs.iter().flatten() {
            line.lower();
        }
    }

    /// Update the source wire level.  Only a rising edge
    /// (0→1) latches the pending bit, matching QEMU
    /// semantics and preventing interrupt storms when the
    /// guest defers source-clearing to a task/bottom-half.
    pub fn set_irq(&self, source: u32, level: bool) {
        if source == 0 || source >= self.num_sources {
            return;
        }
        let prev = self.source_level[source as usize]
            .swap(level as u32, Ordering::Relaxed);
        if level && prev == 0 {
            // Rising edge: latch pending.
            self.set_pending(source, true);
        }
        self.update_outputs();
    }

    /// Re-evaluate all context outputs based on current
    /// pending, enable, priority, and threshold state.
    pub fn update_outputs(&self) {
        let ctx_guard = self.contexts.borrow();
        let outputs = self.context_outputs.lock();
        for ctx in 0..self.num_contexts as usize {
            let thresh = ctx_guard.threshold[ctx];
            let mut active = false;

            for irq in 1..self.num_sources {
                let word = (irq / 32) as usize;
                let bit = 1u32 << (irq % 32);
                let pend = self.pending[word].load(Ordering::Relaxed);
                let pending = pend & bit != 0;
                let enabled = ctx_guard.enable[ctx][word] & bit != 0;
                let pri = self.priority[irq as usize].load(Ordering::Relaxed);
                if pending && enabled && pri > thresh {
                    active = true;
                    break;
                }
            }

            if let Some(ref line) = outputs[ctx] {
                line.set(active);
            }
        }
    }

    /// Set or clear the pending bit for `irq` (lock-free).
    pub fn set_pending(&self, irq: u32, level: bool) {
        if irq == 0 || irq >= self.num_sources {
            return;
        }
        let word = (irq / 32) as usize;
        let bit = 1u32 << (irq % 32);
        if level {
            self.pending[word].fetch_or(bit, Ordering::Relaxed);
        } else {
            self.pending[word].fetch_and(!bit, Ordering::Relaxed);
        }
    }

    /// Claim the highest-priority pending+enabled IRQ for
    /// `context`. Returns `None` when nothing is claimable.
    pub fn claim_irq(&self, context: u32) -> Option<u32> {
        if context >= self.num_contexts {
            return None;
        }
        let ctx = context as usize;
        let mut ctx_guard = self.contexts.borrow();
        let thresh = ctx_guard.threshold[ctx];

        let mut best_irq: Option<u32> = None;
        let mut best_pri: u32 = 0;

        // IRQ 0 is reserved; scan from 1.
        for irq in 1..self.num_sources {
            let word = (irq / 32) as usize;
            let bit = 1u32 << (irq % 32);

            let pend = self.pending[word].load(Ordering::Relaxed);
            let is_pending = pend & bit != 0;
            let is_enabled = ctx_guard.enable[ctx][word] & bit != 0;
            let pri = self.priority[irq as usize].load(Ordering::Relaxed);

            if is_pending && is_enabled && pri > thresh && pri > best_pri {
                best_pri = pri;
                best_irq = Some(irq);
            }
        }

        if let Some(irq) = best_irq {
            // Atomically clear pending bit.
            let word = (irq / 32) as usize;
            let bit = 1u32 << (irq % 32);
            self.pending[word].fetch_and(!bit, Ordering::Relaxed);
            ctx_guard.claim[ctx] = irq;
        }

        best_irq
    }

    /// Complete (acknowledge) a previously claimed IRQ.
    /// If the source is still asserted (level-triggered),
    /// Complete (acknowledge) a previously claimed IRQ.
    /// Clears the claim record and re-evaluates outputs.
    /// Does NOT automatically re-pend based on source wire
    /// level — the device must de-assert and re-assert to
    /// generate a new interrupt (matching QEMU semantics).
    pub fn complete_irq(&self, context: u32, irq: u32) {
        if context >= self.num_contexts {
            return;
        }
        let ctx = context as usize;
        {
            let mut ctx_guard = self.contexts.borrow();
            if ctx_guard.claim[ctx] == irq {
                ctx_guard.claim[ctx] = 0;
            }
        }
        self.update_outputs();
    }

    pub fn read(&self, offset: u64, size: u32) -> u64 {
        let _ = size;
        // Priority registers.
        if offset < PENDING_BASE {
            let idx = (offset - PRIORITY_BASE) as usize / 4;
            if idx < self.priority.len() {
                return self.priority[idx].load(Ordering::Relaxed) as u64;
            }
            return 0;
        }
        // Pending bitmap.
        if offset < ENABLE_BASE {
            let idx = (offset - PENDING_BASE) as usize / 4;
            if idx < self.pending.len() {
                return self.pending[idx].load(Ordering::Relaxed) as u64;
            }
            return 0;
        }
        // Enable bitmap per context.
        if offset < CONTEXT_BASE {
            let rel = offset - ENABLE_BASE;
            let ctx = (rel / ENABLE_STRIDE) as usize;
            let word = ((rel % ENABLE_STRIDE) / 4) as usize;
            let ctx_guard = self.contexts.borrow();
            if ctx < self.num_contexts as usize
                && word < ctx_guard.enable[ctx].len()
            {
                return ctx_guard.enable[ctx][word] as u64;
            }
            return 0;
        }
        // Threshold / claim per context.
        let rel = offset - CONTEXT_BASE;
        let ctx = (rel / CONTEXT_STRIDE) as usize;
        let reg = rel % CONTEXT_STRIDE;
        if ctx >= self.num_contexts as usize {
            return 0;
        }
        match reg {
            0 => {
                let ctx_guard = self.contexts.borrow();
                ctx_guard.threshold[ctx] as u64
            }
            4 => {
                // Perform claim: find highest-priority
                // pending+enabled source, clear pending,
                // update outputs.
                let irq = self.claim_irq(ctx as u32).unwrap_or(0);
                self.update_outputs();
                irq as u64
            }
            _ => 0,
        }
    }

    pub fn write(&self, offset: u64, size: u32, val: u64) {
        let _ = size;
        let v = val as u32;

        // Priority registers.
        if offset < PENDING_BASE {
            let idx = (offset - PRIORITY_BASE) as usize / 4;
            if idx < self.priority.len() {
                self.priority[idx].store(v, Ordering::Relaxed);
            }
            self.update_outputs();
            return;
        }
        // Pending bitmap is read-only from software.
        if offset < ENABLE_BASE {
            return;
        }
        // Enable bitmap per context.
        if offset < CONTEXT_BASE {
            let rel = offset - ENABLE_BASE;
            let ctx = (rel / ENABLE_STRIDE) as usize;
            let word = ((rel % ENABLE_STRIDE) / 4) as usize;
            {
                let mut ctx_guard = self.contexts.borrow();
                if ctx < self.num_contexts as usize
                    && word < ctx_guard.enable[ctx].len()
                {
                    ctx_guard.enable[ctx][word] = v;
                }
            }
            self.update_outputs();
            return;
        }
        // Threshold / claim-complete per context.
        let rel = offset - CONTEXT_BASE;
        let ctx = (rel / CONTEXT_STRIDE) as usize;
        let reg = rel % CONTEXT_STRIDE;
        if ctx >= self.num_contexts as usize {
            return;
        }
        match reg {
            0 => {
                {
                    let mut ctx_guard = self.contexts.borrow();
                    ctx_guard.threshold[ctx] = v;
                }
                self.update_outputs();
            }
            4 => self.complete_irq(ctx as u32, v),
            _ => {}
        }
    }
}

pub struct PlicMmio(pub Arc<Plic>);

impl MmioOps for PlicMmio {
    fn read(&self, offset: u64, size: u32) -> u64 {
        self.0.read(offset, size)
    }

    fn write(&self, offset: u64, size: u32, val: u64) {
        self.0.write(offset, size, val);
    }
}

/// Routes device IRQ level changes to PLIC pending bits.
pub struct PlicIrqSink(pub Arc<Plic>);

impl machina_hw_core::irq::IrqSink for PlicIrqSink {
    fn set_irq(&self, irq: u32, level: bool) {
        self.0.set_irq(irq, level);
    }
}
