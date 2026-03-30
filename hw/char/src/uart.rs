// NS16550A UART emulation.
//
// Register map (offsets 0-7):
//   0: RBR(R)/THR(W) when DLAB=0, DLL when DLAB=1
//   1: IER when DLAB=0, DLM when DLAB=1
//   2: IIR(R) / FCR(W)
//   3: LCR  (bit 7 = DLAB)
//   4: MCR
//   5: LSR  (bit0=DR, bit5=THRE, bit6=TEMT)
//   6: MSR
//   7: SCR

use std::collections::VecDeque;

// IER bits
const IER_RX_AVAIL: u8 = 1 << 0;

// IIR values
const IIR_NONE: u8 = 0x01; // no interrupt pending
const IIR_RX_AVAIL: u8 = 0x04; // rx data available
const IIR_THR_EMPTY: u8 = 0x02; // THR empty

// LSR bits
const LSR_DR: u8 = 1 << 0; // data ready
const LSR_THRE: u8 = 1 << 5; // THR empty
const LSR_TEMT: u8 = 1 << 6; // transmitter empty

// LCR bits
const LCR_DLAB: u8 = 1 << 7;

const FIFO_SIZE: usize = 16;

pub struct Uart16550 {
    rbr: u8,
    thr: u8,
    ier: u8,
    iir: u8,
    fcr: u8,
    lcr: u8,
    mcr: u8,
    lsr: u8,
    msr: u8,
    scr: u8,
    dll: u8,
    dlm: u8,
    rx_fifo: VecDeque<u8>,
    irq_pending: bool,
}

impl Uart16550 {
    pub fn new() -> Self {
        Self {
            rbr: 0,
            thr: 0,
            ier: 0,
            iir: IIR_NONE,
            fcr: 0,
            lcr: 0,
            mcr: 0,
            lsr: LSR_THRE | LSR_TEMT,
            msr: 0,
            scr: 0,
            dll: 0,
            dlm: 0,
            rx_fifo: VecDeque::with_capacity(FIFO_SIZE),
            irq_pending: false,
        }
    }

    /// Push a byte into the receive FIFO.
    pub fn receive(&mut self, ch: u8) {
        if self.rx_fifo.len() < FIFO_SIZE {
            self.rx_fifo.push_back(ch);
        }
        self.lsr |= LSR_DR;
        self.update_irq();
    }

    pub fn irq_pending(&self) -> bool {
        self.irq_pending
    }

    pub fn update_irq(&mut self) {
        let mut iir = IIR_NONE;

        // RX data available has higher priority.
        if (self.ier & IER_RX_AVAIL) != 0 && (self.lsr & LSR_DR) != 0 {
            iir = IIR_RX_AVAIL;
        } else if (self.ier & 0x02) != 0 && (self.lsr & LSR_THRE) != 0 {
            iir = IIR_THR_EMPTY;
        }

        self.iir = iir;
        self.irq_pending = iir != IIR_NONE;
    }

    pub fn read(&mut self, offset: u64) -> u8 {
        match offset & 0x7 {
            0 => {
                if self.lcr & LCR_DLAB != 0 {
                    self.dll
                } else {
                    self.read_rbr()
                }
            }
            1 => {
                if self.lcr & LCR_DLAB != 0 {
                    self.dlm
                } else {
                    self.ier
                }
            }
            2 => self.iir,
            3 => self.lcr,
            4 => self.mcr,
            5 => self.lsr,
            6 => self.msr,
            7 => self.scr,
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u64, val: u8) {
        match offset & 0x7 {
            0 => {
                if self.lcr & LCR_DLAB != 0 {
                    self.dll = val;
                } else {
                    self.write_thr(val);
                }
            }
            1 => {
                if self.lcr & LCR_DLAB != 0 {
                    self.dlm = val;
                } else {
                    self.ier = val & 0x0F;
                    self.update_irq();
                }
            }
            2 => {
                self.fcr = val;
                if val & 0x02 != 0 {
                    // Clear RX FIFO.
                    self.rx_fifo.clear();
                    self.lsr &= !LSR_DR;
                    self.update_irq();
                }
            }
            3 => self.lcr = val,
            4 => self.mcr = val,
            5 => {} // LSR is read-only
            6 => {} // MSR is read-only
            7 => self.scr = val,
            _ => {}
        }
    }

    fn read_rbr(&mut self) -> u8 {
        if let Some(ch) = self.rx_fifo.pop_front() {
            self.rbr = ch;
            if self.rx_fifo.is_empty() {
                self.lsr &= !LSR_DR;
            }
            self.update_irq();
            ch
        } else {
            self.rbr
        }
    }

    fn write_thr(&mut self, val: u8) {
        self.thr = val;
        // In emulation the byte is "transmitted"
        // instantly, so THRE stays set.
        self.lsr |= LSR_THRE | LSR_TEMT;
        self.update_irq();
    }
}

impl Default for Uart16550 {
    fn default() -> Self {
        Self::new()
    }
}
