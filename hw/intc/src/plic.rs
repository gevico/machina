// SiFive PLIC (Platform-Level Interrupt Controller).
//
// MMIO layout (per RISC-V PLIC spec):
//   0x000000 .. 0x000FFF  priority[0..N]  (4 bytes each)
//   0x001000 .. 0x001FFF  pending bitmap  (32 sources/word)
//   0x002000 + 0x80*ctx   enable bitmap per context
//   0x200000 + 0x1000*ctx threshold (off 0), claim/complete (off 4)

const PRIORITY_BASE: u64 = 0x00_0000;
const PENDING_BASE: u64 = 0x00_1000;
const ENABLE_BASE: u64 = 0x00_2000;
const ENABLE_STRIDE: u64 = 0x80;
const CONTEXT_BASE: u64 = 0x20_0000;
const CONTEXT_STRIDE: u64 = 0x1000;

pub struct Plic {
    num_sources: u32,
    num_contexts: u32,
    priority: Vec<u32>,
    pending: Vec<u32>,
    enable: Vec<Vec<u32>>,
    threshold: Vec<u32>,
    claim: Vec<u32>,
}

impl Plic {
    pub fn new(num_sources: u32, num_contexts: u32) -> Self {
        let words = num_sources.div_ceil(32) as usize;
        Self {
            num_sources,
            num_contexts,
            priority: vec![0u32; num_sources as usize],
            pending: vec![0u32; words],
            enable: vec![vec![0u32; words]; num_contexts as usize],
            threshold: vec![0u32; num_contexts as usize],
            claim: vec![0u32; num_contexts as usize],
        }
    }

    /// Set or clear the pending bit for `irq`.
    pub fn set_pending(&mut self, irq: u32, level: bool) {
        if irq == 0 || irq >= self.num_sources {
            return;
        }
        let word = (irq / 32) as usize;
        let bit = 1u32 << (irq % 32);
        if level {
            self.pending[word] |= bit;
        } else {
            self.pending[word] &= !bit;
        }
    }

    /// Claim the highest-priority pending+enabled IRQ for
    /// `context`. Returns `None` when nothing is claimable.
    pub fn claim_irq(&mut self, context: u32) -> Option<u32> {
        if context >= self.num_contexts {
            return None;
        }
        let ctx = context as usize;
        let thresh = self.threshold[ctx];

        let mut best_irq: Option<u32> = None;
        let mut best_pri: u32 = 0;

        // IRQ 0 is reserved; scan from 1.
        for irq in 1..self.num_sources {
            let word = (irq / 32) as usize;
            let bit = 1u32 << (irq % 32);

            let is_pending = self.pending[word] & bit != 0;
            let is_enabled = self.enable[ctx][word] & bit != 0;
            let pri = self.priority[irq as usize];

            if is_pending && is_enabled && pri > thresh && pri > best_pri {
                best_pri = pri;
                best_irq = Some(irq);
            }
        }

        if let Some(irq) = best_irq {
            // Clear pending, record claimed.
            let word = (irq / 32) as usize;
            let bit = 1u32 << (irq % 32);
            self.pending[word] &= !bit;
            self.claim[ctx] = irq;
        }

        best_irq
    }

    /// Complete (acknowledge) a previously claimed IRQ.
    pub fn complete_irq(&mut self, context: u32, irq: u32) {
        if context >= self.num_contexts {
            return;
        }
        let ctx = context as usize;
        if self.claim[ctx] == irq {
            self.claim[ctx] = 0;
        }
    }

    // ---- MMIO interface ----

    pub fn read(&self, offset: u64, size: u32) -> u64 {
        let _ = size;
        // Priority registers.
        if offset < PENDING_BASE {
            let idx = (offset - PRIORITY_BASE) as usize / 4;
            if idx < self.priority.len() {
                return self.priority[idx] as u64;
            }
            return 0;
        }
        // Pending bitmap.
        if offset < ENABLE_BASE {
            let idx = (offset - PENDING_BASE) as usize / 4;
            if idx < self.pending.len() {
                return self.pending[idx] as u64;
            }
            return 0;
        }
        // Enable bitmap per context.
        if offset < CONTEXT_BASE {
            let rel = offset - ENABLE_BASE;
            let ctx = (rel / ENABLE_STRIDE) as usize;
            let word = ((rel % ENABLE_STRIDE) / 4) as usize;
            if ctx < self.num_contexts as usize && word < self.enable[ctx].len()
            {
                return self.enable[ctx][word] as u64;
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
            0 => self.threshold[ctx] as u64,
            4 => self.claim[ctx] as u64,
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u64, size: u32, val: u64) {
        let _ = size;
        let v = val as u32;

        // Priority registers.
        if offset < PENDING_BASE {
            let idx = (offset - PRIORITY_BASE) as usize / 4;
            if idx < self.priority.len() {
                self.priority[idx] = v;
            }
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
            if ctx < self.num_contexts as usize && word < self.enable[ctx].len()
            {
                self.enable[ctx][word] = v;
            }
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
            0 => self.threshold[ctx] = v,
            4 => self.complete_irq(ctx as u32, v),
            _ => {}
        }
    }
}
