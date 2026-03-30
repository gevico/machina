// RISC-V ACLINT (Advanced Core Local Interruptor).
//
// Two MMIO regions:
//
// MSWI region (mapped at its own base address):
//   0x0000 + 4*hart : msip register (1 bit used)
//
// MTIMER region (mapped at its own base address):
//   0x0000 + 8*hart : mtimecmp[hart]
//   0xBFF8           : mtime

const MTIMER_MTIME_OFFSET: u64 = 0xBFF8;

pub struct Aclint {
    num_harts: u32,
    mtime: u64,
    mtimecmp: Vec<u64>,
    msip: Vec<u32>,
    /// Per-hart timer-interrupt pending flag, set by `tick`.
    timer_pending: Vec<bool>,
}

impl Aclint {
    pub fn new(num_harts: u32) -> Self {
        Self {
            num_harts,
            mtime: 0,
            mtimecmp: vec![u64::MAX; num_harts as usize],
            msip: vec![0u32; num_harts as usize],
            timer_pending: vec![false; num_harts as usize],
        }
    }

    /// Increment mtime by 1 and check mtimecmp for each
    /// hart, updating `timer_pending` accordingly.
    pub fn tick(&mut self) {
        self.mtime = self.mtime.wrapping_add(1);
        for hart in 0..self.num_harts as usize {
            self.timer_pending[hart] = self.mtime >= self.mtimecmp[hart];
        }
    }

    /// Returns whether the timer interrupt is pending for
    /// `hart`.
    pub fn timer_irq_pending(&self, hart: u32) -> bool {
        if hart < self.num_harts {
            self.timer_pending[hart as usize]
        } else {
            false
        }
    }

    // ---- MSWI region ----

    pub fn mswi_read(&self, offset: u64, _size: u32) -> u64 {
        let hart = (offset / 4) as usize;
        if hart < self.num_harts as usize {
            self.msip[hart] as u64
        } else {
            0
        }
    }

    pub fn mswi_write(&mut self, offset: u64, _size: u32, val: u64) {
        let hart = (offset / 4) as usize;
        if hart < self.num_harts as usize {
            // Only bit 0 is writable.
            self.msip[hart] = (val as u32) & 1;
        }
    }

    // ---- MTIMER region ----

    pub fn mtimer_read(&self, offset: u64, _size: u32) -> u64 {
        if offset == MTIMER_MTIME_OFFSET {
            return self.mtime;
        }
        // mtimecmp registers at 0x0000 + 8*hart.
        let hart = (offset / 8) as usize;
        if hart < self.num_harts as usize {
            self.mtimecmp[hart]
        } else {
            0
        }
    }

    pub fn mtimer_write(&mut self, offset: u64, _size: u32, val: u64) {
        if offset == MTIMER_MTIME_OFFSET {
            self.mtime = val;
            return;
        }
        let hart = (offset / 8) as usize;
        if hart < self.num_harts as usize {
            self.mtimecmp[hart] = val;
            // Re-evaluate pending state immediately.
            self.timer_pending[hart] = self.mtime >= self.mtimecmp[hart];
        }
    }
}
