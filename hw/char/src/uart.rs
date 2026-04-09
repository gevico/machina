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
//
// Interior mutability: register state is in
// DeviceRefCell<Uart16550Regs>, setup state in
// parking_lot::Mutex<SysBusDeviceState>.  All public
// methods take &self so the device can be shared via
// Arc<Uart16550> without an outer Mutex.

use std::collections::VecDeque;
use std::sync::Arc;

use machina_core::address::GPA;
use machina_core::device_cell::DeviceRefCell;
use machina_core::mobject::{MObject, MObjectInfo};
use machina_hw_core::bus::{SysBus, SysBusDeviceState, SysBusError};
use machina_hw_core::chardev::{
    ByteCb, CharFrontend, ChardevResolveError, ChardevResolver,
};
use machina_hw_core::irq::IrqLine;
use machina_hw_core::mdev::{MDevice, MDeviceError};
use machina_hw_core::property::{MPropertySpec, MPropertyType, MPropertyValue};
use machina_memory::address_space::AddressSpace;
use machina_memory::region::{MemoryRegion, MmioOps};

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

// MCR bits
const MCR_DTR: u8 = 1 << 0;
const MCR_RTS: u8 = 1 << 1;
const MCR_OUT1: u8 = 1 << 2;
const MCR_OUT2: u8 = 1 << 3;
const MCR_LOOPBACK: u8 = 1 << 4;

// MSR bits
const MSR_CTS: u8 = 1 << 4;
const MSR_DSR: u8 = 1 << 5;
const MSR_RI: u8 = 1 << 6;
const MSR_DCD: u8 = 1 << 7;

const FIFO_SIZE: usize = 16;

#[derive(Debug)]
pub enum UartError {
    Device(MDeviceError),
    SysBus(SysBusError),
    Resolve(ChardevResolveError),
}

impl std::fmt::Display for UartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Device(err) => write!(f, "{err}"),
            Self::SysBus(err) => write!(f, "{err}"),
            Self::Resolve(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for UartError {}

impl From<MDeviceError> for UartError {
    fn from(value: MDeviceError) -> Self {
        Self::Device(value)
    }
}

impl From<SysBusError> for UartError {
    fn from(value: SysBusError) -> Self {
        Self::SysBus(value)
    }
}

impl From<ChardevResolveError> for UartError {
    fn from(value: ChardevResolveError) -> Self {
        Self::Resolve(value)
    }
}

/// Mutable register state protected by DeviceRefCell.
pub struct Uart16550Regs {
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

impl Uart16550Regs {
    fn new() -> Self {
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

    fn reset(&mut self, has_chardev: bool) {
        self.rbr = 0;
        self.thr = 0;
        self.ier = 0;
        self.iir = IIR_NONE;
        self.fcr = 0;
        self.lcr = 0;
        self.mcr = 0;
        self.lsr = LSR_THRE | LSR_TEMT;
        self.scr = 0;
        self.dll = 0;
        self.dlm = 0;
        self.rx_fifo.clear();
        self.irq_pending = false;
        self.update_msr(has_chardev);
    }

    /// Recompute MSR based on MCR loopback state and
    /// chardev presence.
    fn update_msr(&mut self, has_chardev: bool) {
        if self.mcr & MCR_LOOPBACK != 0 {
            // 16550 loopback: MCR outputs routed to MSR.
            let mut msr = 0u8;
            if self.mcr & MCR_DTR != 0 {
                msr |= MSR_DSR;
            }
            if self.mcr & MCR_RTS != 0 {
                msr |= MSR_CTS;
            }
            if self.mcr & MCR_OUT1 != 0 {
                msr |= MSR_RI;
            }
            if self.mcr & MCR_OUT2 != 0 {
                msr |= MSR_DCD;
            }
            self.msr = msr;
        } else if has_chardev {
            self.msr = MSR_CTS | MSR_DSR | MSR_DCD;
        } else {
            self.msr = 0;
        }
    }
}

pub struct Uart16550 {
    // Setup-only state behind parking_lot::Mutex so that
    // attach_to_bus / register_mmio / realize_onto can be
    // called through &self (Arc<Uart16550>).
    state: parking_lot::Mutex<SysBusDeviceState>,
    // Runtime register state.
    regs: DeviceRefCell<Uart16550Regs>,
    // IRQ line. Written during realize, read at runtime.
    irq_line: parking_lot::Mutex<Option<IrqLine>>,
    // Chardev frontend for TX output.
    chardev: DeviceRefCell<Option<CharFrontend>>,
    // Pre-realize chardev config.
    configured_chardev: parking_lot::Mutex<Option<CharFrontend>>,
    // Resolved chardev path for unrealize.
    resolved_chardev_path: parking_lot::Mutex<Option<String>>,
}

// SAFETY: All mutable state is behind DeviceRefCell or
// parking_lot::Mutex.
unsafe impl Sync for Uart16550 {}

impl Uart16550 {
    pub fn new() -> Self {
        Self::new_named("uart")
    }

    pub fn new_named(local_id: &str) -> Self {
        let mut state = SysBusDeviceState::new(local_id);
        state
            .device_mut()
            .define_property(MPropertySpec::new("chardev", MPropertyType::Link))
            .expect("UART chardev property schema must be valid");

        Self {
            state: parking_lot::Mutex::new(state),
            regs: DeviceRefCell::new(Uart16550Regs::new()),
            irq_line: parking_lot::Mutex::new(None),
            chardev: DeviceRefCell::new(None),
            configured_chardev: parking_lot::Mutex::new(None),
            resolved_chardev_path: parking_lot::Mutex::new(None),
        }
    }

    pub fn set_chardev_property(&self, path: &str) -> Result<(), MDeviceError> {
        self.state
            .lock()
            .device_mut()
            .set_property("chardev", MPropertyValue::Link(path.to_string()))
    }

    pub fn chardev_property(&self) -> Option<String> {
        match self.state.lock().device().property("chardev") {
            Some(MPropertyValue::Link(path)) => Some(path.clone()),
            _ => None,
        }
    }

    pub fn realized(&self) -> bool {
        self.state.lock().device().is_realized()
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

    pub fn attach_irq(&self, irq: IrqLine) -> Result<(), SysBusError> {
        self.state.lock().register_irq(irq)
    }

    pub fn attach_chardev(&self, fe: CharFrontend) -> Result<(), MDeviceError> {
        if self.state.lock().device().is_realized() {
            return Err(MDeviceError::LateMutation("chardev_frontend"));
        }
        *self.configured_chardev.lock() = Some(fe);
        Ok(())
    }

    pub fn realize_onto(
        &self,
        bus: &mut SysBus,
        address_space: &mut AddressSpace,
        rx_cb: ByteCb,
    ) -> Result<(), UartError> {
        self.realize_onto_with_resolver(bus, address_space, rx_cb, None)
    }

    pub fn realize_onto_with_resolver(
        &self,
        bus: &mut SysBus,
        address_space: &mut AddressSpace,
        rx_cb: ByteCb,
        resolver: Option<&dyn ChardevResolver>,
    ) -> Result<(), UartError> {
        {
            let mut st = self.state.lock();
            st.realize_onto(bus, address_space)?;
            let line = st.irq_outputs().first().cloned();
            *self.irq_line.lock() = line;
        }

        if let Some((path, mut fe)) = self.resolve_chardev(resolver)? {
            fe.start_input(rx_cb);
            *self.chardev.borrow() = Some(fe);
            *self.resolved_chardev_path.lock() = path;
            // Set modem status lines when chardev present.
            self.regs.borrow().msr = MSR_CTS | MSR_DSR | MSR_DCD;
        }

        Ok(())
    }

    pub fn unrealize_from(
        &self,
        bus: &mut SysBus,
        address_space: &mut AddressSpace,
    ) -> Result<(), UartError> {
        self.unrealize_from_with_resolver(bus, address_space, None)
    }

    pub fn unrealize_from_with_resolver(
        &self,
        bus: &mut SysBus,
        address_space: &mut AddressSpace,
        resolver: Option<&dyn ChardevResolver>,
    ) -> Result<(), UartError> {
        if let Some(frontend) = self.chardev.borrow().take() {
            let path = self.resolved_chardev_path.lock().take();
            if let (Some(path), Some(resolver)) = (path, resolver) {
                resolver.put_frontend(&path, frontend)?;
            }
        }
        *self.irq_line.lock() = None;
        self.state.lock().unrealize_from(bus, address_space)?;
        Ok(())
    }

    fn resolve_chardev(
        &self,
        resolver: Option<&dyn ChardevResolver>,
    ) -> Result<Option<(Option<String>, CharFrontend)>, UartError> {
        if let Some(frontend) = self.configured_chardev.lock().take() {
            return Ok(Some((None, frontend)));
        }

        let path = {
            let st = self.state.lock();
            match st.device().property("chardev") {
                Some(MPropertyValue::Link(p)) => Some(p.clone()),
                _ => None,
            }
        };

        let Some(path) = path else {
            return Ok(None);
        };
        let Some(resolver) = resolver else {
            return Ok(None);
        };
        let frontend = resolver.take_frontend(&path)?;
        Ok(Some((Some(path), frontend)))
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

    pub fn reset_runtime(&self) {
        let has_cd = self.chardev.borrow().is_some();
        self.regs.borrow().reset(has_cd);
        let line = self.irq_line.lock();
        if let Some(ref l) = *line {
            l.lower();
        }
    }

    /// Push a byte into the receive FIFO.
    pub fn receive(&self, ch: u8) {
        {
            let mut regs = self.regs.borrow();
            if regs.rx_fifo.len() < FIFO_SIZE {
                regs.rx_fifo.push_back(ch);
            }
            regs.lsr |= LSR_DR;
        }
        self.update_irq();
    }

    pub fn irq_pending(&self) -> bool {
        self.regs.borrow().irq_pending
    }

    pub fn update_irq(&self) {
        let pending;
        {
            let mut regs = self.regs.borrow();
            let mut iir = IIR_NONE;

            // RX data available has higher priority.
            if (regs.ier & IER_RX_AVAIL) != 0 && (regs.lsr & LSR_DR) != 0 {
                iir = IIR_RX_AVAIL;
            } else if (regs.ier & 0x02) != 0 && (regs.lsr & LSR_THRE) != 0 {
                iir = IIR_THR_EMPTY;
            }

            regs.iir = iir;
            regs.irq_pending = iir != IIR_NONE;
            pending = regs.irq_pending;
        }

        let line = self.irq_line.lock();
        if let Some(ref l) = *line {
            l.set(pending);
        }
    }

    pub fn read(&self, offset: u64) -> u8 {
        let mut regs = self.regs.borrow();
        match offset & 0x7 {
            0 => {
                if regs.lcr & LCR_DLAB != 0 {
                    regs.dll
                } else {
                    let ch = Self::read_rbr(&mut regs);
                    drop(regs);
                    self.update_irq();
                    ch
                }
            }
            1 => {
                if regs.lcr & LCR_DLAB != 0 {
                    regs.dlm
                } else {
                    regs.ier
                }
            }
            2 => regs.iir,
            3 => regs.lcr,
            4 => regs.mcr,
            5 => regs.lsr,
            6 => regs.msr,
            7 => regs.scr,
            _ => 0,
        }
    }

    pub fn write(&self, offset: u64, val: u8) {
        match offset & 0x7 {
            0 => {
                let dlab = self.regs.borrow().lcr & LCR_DLAB != 0;
                if dlab {
                    self.regs.borrow().dll = val;
                } else {
                    self.write_thr(val);
                }
            }
            1 => {
                let dlab = self.regs.borrow().lcr & LCR_DLAB != 0;
                if dlab {
                    self.regs.borrow().dlm = val;
                } else {
                    self.regs.borrow().ier = val & 0x0F;
                    self.update_irq();
                }
            }
            2 => {
                let mut regs = self.regs.borrow();
                regs.fcr = val;
                if val & 0x02 != 0 {
                    // Clear RX FIFO.
                    regs.rx_fifo.clear();
                    regs.lsr &= !LSR_DR;
                    drop(regs);
                    self.update_irq();
                }
            }
            3 => self.regs.borrow().lcr = val,
            4 => {
                let has_cd = self.chardev.borrow().is_some();
                let mut regs = self.regs.borrow();
                regs.mcr = val;
                regs.update_msr(has_cd);
            }
            5 => {} // LSR is read-only
            6 => {} // MSR is read-only
            7 => self.regs.borrow().scr = val,
            _ => {}
        }
    }

    fn read_rbr(regs: &mut parking_lot::MutexGuard<'_, Uart16550Regs>) -> u8 {
        if let Some(ch) = regs.rx_fifo.pop_front() {
            regs.rbr = ch;
            if regs.rx_fifo.is_empty() {
                regs.lsr &= !LSR_DR;
            }
            ch
        } else {
            regs.rbr
        }
    }

    fn write_thr(&self, val: u8) {
        let loopback = {
            let mut regs = self.regs.borrow();
            regs.thr = val;
            // In emulation the byte is "transmitted"
            // instantly, so THRE stays set.
            regs.lsr |= LSR_THRE | LSR_TEMT;
            regs.mcr & MCR_LOOPBACK != 0
        };
        if loopback {
            // Loopback: route THR → RX FIFO.
            let mut regs = self.regs.borrow();
            if regs.rx_fifo.len() < FIFO_SIZE {
                regs.rx_fifo.push_back(val);
            }
            regs.lsr |= LSR_DR;
        } else if let Some(ref mut fe) = *self.chardev.borrow() {
            fe.write(&[val]);
        }
        self.update_irq();
    }
}

impl Default for Uart16550 {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Uart16550Mmio(pub Arc<Uart16550>);

impl MmioOps for Uart16550Mmio {
    fn read(&self, offset: u64, _size: u32) -> u64 {
        self.0.read(offset) as u64
    }

    fn write(&self, offset: u64, _size: u32, val: u64) {
        self.0.write(offset, val as u8);
    }
}
