// VirtIO MMIO transport (Modern, v2).
//
// Implements the standard VirtIO MMIO register interface
// and delegates device-specific operations to a VirtioDevice
// backend (block, net, etc.).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use machina_core::address::GPA;
use machina_hw_core::bus::{SysBus, SysBusDeviceState, SysBusError};
use machina_hw_core::irq::IrqLine;
use machina_memory::address_space::AddressSpace;
use machina_memory::region::MemoryRegion;
use machina_memory::region::MmioOps;

use crate::device::VirtioDevice;
use crate::queue::{VirtQueue, MAX_QUEUE_SIZE};

// MMIO register offsets.
const MAGIC_VALUE: u64 = 0x000;
const VERSION: u64 = 0x004;
const DEVICE_ID: u64 = 0x008;
const VENDOR_ID: u64 = 0x00c;
const DEVICE_FEATURES: u64 = 0x010;
const DEVICE_FEATURES_SEL: u64 = 0x014;
const DRIVER_FEATURES: u64 = 0x020;
const DRIVER_FEATURES_SEL: u64 = 0x024;
const QUEUE_SEL: u64 = 0x030;
const QUEUE_NUM_MAX: u64 = 0x034;
const QUEUE_NUM: u64 = 0x038;
const QUEUE_READY: u64 = 0x044;
const QUEUE_NOTIFY: u64 = 0x050;
const INTERRUPT_STATUS: u64 = 0x060;
const INTERRUPT_ACK: u64 = 0x064;
const STATUS: u64 = 0x070;
const QUEUE_DESC_LOW: u64 = 0x080;
const QUEUE_DESC_HIGH: u64 = 0x084;
const QUEUE_AVAIL_LOW: u64 = 0x090;
const QUEUE_AVAIL_HIGH: u64 = 0x094;
const QUEUE_USED_LOW: u64 = 0x0a0;
const QUEUE_USED_HIGH: u64 = 0x0a4;
const CONFIG_GENERATION: u64 = 0x0fc;
const CONFIG_BASE: u64 = 0x100;

// Legacy register offsets (for driver compat).
const LEGACY_GUEST_PAGE_SIZE: u64 = 0x028;
const LEGACY_QUEUE_PFN: u64 = 0x040;
const LEGACY_QUEUE_ALIGN: u64 = 0x03c;

const VIRTIO_MAGIC: u32 = 0x74726976;
const VIRTIO_VENDOR: u32 = 0x554D4551;
const VIRTIO_VERSION: u32 = 2;

pub(crate) struct VirtioMmioState {
    pub(crate) device: Box<dyn VirtioDevice>,
    pub(crate) irq: IrqLine,

    // Transport state.
    pub(crate) status: u32,
    device_features_sel: u32,
    driver_features_sel: u32,
    driver_features: u64,
    queue_sel: u32,
    pub(crate) queues: Vec<VirtQueue>,
    pub(crate) interrupt_status: u32,
    guest_page_size: u32,

    // Guest RAM access.
    pub(crate) ram_ptr: *mut u8,
    pub(crate) ram_base: u64,
    pub(crate) ram_size: u64,
}

// SAFETY: ram_ptr points to mmap'd memory that outlives
// VirtioMmioState.
unsafe impl Send for VirtioMmioState {}

impl VirtioMmioState {
    fn reset(&mut self) {
        self.status = 0;
        self.device_features_sel = 0;
        self.driver_features_sel = 0;
        self.driver_features = 0;
        self.queue_sel = 0;
        for q in &mut self.queues {
            q.reset();
        }
        self.interrupt_status = 0;
        self.guest_page_size = 0;
        self.irq.set(false);
    }

    fn current_queue(&mut self) -> Option<&mut VirtQueue> {
        let sel = self.queue_sel as usize;
        self.queues.get_mut(sel)
    }

    fn process_notify(&mut self, queue_idx: u32) {
        let idx = queue_idx as usize;
        if idx >= self.queues.len() {
            return;
        }
        let q = &mut self.queues[idx];
        if !q.ready || q.num == 0 {
            return;
        }
        if self.status & 0x4 == 0 {
            return;
        }
        let n = unsafe {
            self.device.handle_queue(
                idx,
                q,
                self.ram_ptr,
                self.ram_base,
                self.ram_size,
            )
        };
        if n > 0 {
            self.interrupt_status |= 1;
            self.irq.set(true);
        }
    }
}

/// VirtIO MMIO device wrapper implementing MmioOps.
pub struct VirtioMmio {
    device: SysBusDeviceState,
    state: Arc<Mutex<VirtioMmioState>>,
    rx_stop: Option<Arc<AtomicBool>>,
    rx_thread: Option<std::thread::JoinHandle<()>>,
}

impl VirtioMmio {
    pub fn new(
        device: Box<dyn VirtioDevice>,
        irq: IrqLine,
        ram_ptr: *mut u8,
        ram_base: u64,
        ram_size: u64,
    ) -> Self {
        Self::new_named(
            "virtio-mmio",
            device,
            irq,
            ram_ptr,
            ram_base,
            ram_size,
        )
    }

    pub fn new_named(
        local_id: &str,
        device: Box<dyn VirtioDevice>,
        irq: IrqLine,
        ram_ptr: *mut u8,
        ram_base: u64,
        ram_size: u64,
    ) -> Self {
        let num_queues = device.num_queues();
        let rx_fd = device.rx_fd();

        let mut sysbus_state = SysBusDeviceState::new(local_id);
        sysbus_state
            .register_irq(irq.clone())
            .expect("virtio-mmio IRQ registration must succeed at creation");

        let queues = (0..num_queues).map(|_| VirtQueue::new()).collect();

        let state = Arc::new(Mutex::new(VirtioMmioState {
            device,
            irq,
            status: 0,
            device_features_sel: 0,
            driver_features_sel: 0,
            driver_features: 0,
            queue_sel: 0,
            queues,
            interrupt_status: 0,
            guest_page_size: 0,
            ram_ptr,
            ram_base,
            ram_size,
        }));

        let (rx_stop, rx_thread) = if let Some(fd) = rx_fd {
            let stop = Arc::new(AtomicBool::new(false));
            let stop2 = Arc::clone(&stop);
            let st = Arc::clone(&state);
            let handle = std::thread::Builder::new()
                .name(format!("{}-rx", local_id))
                .spawn(move || {
                    rx_loop(fd, st, stop2);
                })
                .expect("failed to spawn virtio-net RX thread");
            (Some(stop), Some(handle))
        } else {
            (None, None)
        };

        Self {
            device: sysbus_state,
            state,
            rx_stop,
            rx_thread,
        }
    }

    pub fn attach_to_bus(
        &mut self,
        bus: &SysBus,
    ) -> Result<(), SysBusError> {
        self.device.attach_to_bus(bus)
    }

    pub fn register_mmio(
        &mut self,
        region: MemoryRegion,
        base: GPA,
    ) -> Result<(), SysBusError> {
        self.device.register_mmio(region, base)
    }

    pub fn make_mmio_region(&self, name: &str, size: u64) -> MemoryRegion {
        MemoryRegion::io(
            name,
            size,
            Box::new(VirtioMmioRegion(Arc::clone(&self.state))),
        )
    }

    pub fn realize_onto(
        &mut self,
        bus: &mut SysBus,
        address_space: &mut AddressSpace,
    ) -> Result<(), SysBusError> {
        self.device.realize_onto(bus, address_space)
    }

    pub fn unrealize_from(
        &mut self,
        bus: &mut SysBus,
        address_space: &mut AddressSpace,
    ) -> Result<(), SysBusError> {
        self.reset_runtime();
        self.device.unrealize_from(bus, address_space)
    }

    pub fn realized(&self) -> bool {
        self.device.device().is_realized()
    }

    pub fn reset_runtime(&mut self) {
        self.state.lock().unwrap().reset();
    }

    fn read_locked(state: &VirtioMmioState, offset: u64, size: u32) -> u64 {
        match offset {
            MAGIC_VALUE => VIRTIO_MAGIC as u64,
            VERSION => VIRTIO_VERSION as u64,
            DEVICE_ID => state.device.device_id() as u64,
            VENDOR_ID => VIRTIO_VENDOR as u64,
            DEVICE_FEATURES => {
                let feat = state.device.features();
                let sel = state.device_features_sel;
                if sel == 0 {
                    feat & 0xFFFF_FFFF
                } else {
                    (feat >> 32) & 0xFFFF_FFFF
                }
            }
            QUEUE_NUM_MAX => MAX_QUEUE_SIZE as u64,
            QUEUE_READY => {
                let sel = state.queue_sel as usize;
                state
                    .queues
                    .get(sel)
                    .map(|queue| queue.ready as u64)
                    .unwrap_or(0)
            }
            INTERRUPT_STATUS => state.interrupt_status as u64,
            STATUS => state.status as u64,
            CONFIG_GENERATION => 0,
            LEGACY_QUEUE_PFN => {
                let sel = state.queue_sel as usize;
                state
                    .queues
                    .get(sel)
                    .map(|queue| {
                        if state.guest_page_size > 0 {
                            queue.desc_addr / state.guest_page_size as u64
                        } else {
                            0
                        }
                    })
                    .unwrap_or(0)
            }
            value if value >= CONFIG_BASE => {
                state.device.config_read(value - CONFIG_BASE, size)
            }
            _ => 0,
        }
    }

    fn write_locked(state: &mut VirtioMmioState, offset: u64, val: u64) {
        let v32 = val as u32;
        match offset {
            DEVICE_FEATURES_SEL => {
                state.device_features_sel = v32;
            }
            DRIVER_FEATURES => {
                let sel = state.driver_features_sel;
                if sel == 0 {
                    state.driver_features = (state.driver_features
                        & 0xFFFF_FFFF_0000_0000)
                        | (v32 as u64);
                } else {
                    state.driver_features = (state.driver_features
                        & 0x0000_0000_FFFF_FFFF)
                        | ((v32 as u64) << 32);
                }
            }
            DRIVER_FEATURES_SEL => {
                state.driver_features_sel = v32;
            }
            QUEUE_SEL => {
                state.queue_sel = v32;
            }
            QUEUE_NUM => {
                if let Some(queue) = state.current_queue() {
                    queue.num = v32.min(MAX_QUEUE_SIZE);
                }
            }
            QUEUE_READY => {
                if let Some(queue) = state.current_queue() {
                    queue.ready = v32 != 0;
                }
            }
            QUEUE_NOTIFY => {
                state.process_notify(v32);
            }
            INTERRUPT_ACK => {
                state.interrupt_status &= !v32;
                if state.interrupt_status == 0 {
                    state.irq.set(false);
                }
            }
            STATUS => {
                if v32 == 0 {
                    state.reset();
                } else {
                    state.status = v32;
                }
            }
            QUEUE_DESC_LOW => {
                if let Some(queue) = state.current_queue() {
                    queue.desc_addr = (queue.desc_addr
                        & 0xFFFF_FFFF_0000_0000)
                        | (v32 as u64);
                }
            }
            QUEUE_DESC_HIGH => {
                if let Some(queue) = state.current_queue() {
                    queue.desc_addr = (queue.desc_addr
                        & 0x0000_0000_FFFF_FFFF)
                        | ((v32 as u64) << 32);
                }
            }
            QUEUE_AVAIL_LOW => {
                if let Some(queue) = state.current_queue() {
                    queue.avail_addr = (queue.avail_addr
                        & 0xFFFF_FFFF_0000_0000)
                        | (v32 as u64);
                }
            }
            QUEUE_AVAIL_HIGH => {
                if let Some(queue) = state.current_queue() {
                    queue.avail_addr = (queue.avail_addr
                        & 0x0000_0000_FFFF_FFFF)
                        | ((v32 as u64) << 32);
                }
            }
            QUEUE_USED_LOW => {
                if let Some(queue) = state.current_queue() {
                    queue.used_addr = (queue.used_addr
                        & 0xFFFF_FFFF_0000_0000)
                        | (v32 as u64);
                }
            }
            QUEUE_USED_HIGH => {
                if let Some(queue) = state.current_queue() {
                    queue.used_addr = (queue.used_addr
                        & 0x0000_0000_FFFF_FFFF)
                        | ((v32 as u64) << 32);
                }
            }
            LEGACY_GUEST_PAGE_SIZE => {
                state.guest_page_size = v32;
            }
            LEGACY_QUEUE_PFN => {
                let gps = state.guest_page_size;
                let sel = state.queue_sel as usize;
                if let Some(queue) = state.queues.get_mut(sel) {
                    if v32 == 0 {
                        queue.reset();
                    } else if gps > 0 {
                        let base = (v32 as u64) * (gps as u64);
                        queue.desc_addr = base;
                        let align = gps as u64;
                        let avail_off = (queue.num as u64) * 16;
                        queue.avail_addr = base + avail_off;
                        let used_off =
                            (base + avail_off + 6 + (queue.num as u64) * 2)
                                .div_ceil(align)
                                * align;
                        queue.used_addr = used_off;
                        queue.ready = true;
                    }
                }
            }
            LEGACY_QUEUE_ALIGN => {}
            _ => {}
        }
    }
}

impl Drop for VirtioMmio {
    fn drop(&mut self) {
        if let Some(stop) = &self.rx_stop {
            stop.store(true, Ordering::SeqCst);
        }
        if let Some(handle) = self.rx_thread.take() {
            let _ = handle.join();
        }
    }
}

impl MmioOps for VirtioMmio {
    fn read(&self, offset: u64, size: u32) -> u64 {
        let state = self.state.lock().unwrap();
        Self::read_locked(&state, offset, size)
    }

    fn write(&self, offset: u64, _size: u32, val: u64) {
        let mut state = self.state.lock().unwrap();
        Self::write_locked(&mut state, offset, val);
    }
}

struct VirtioMmioRegion(Arc<Mutex<VirtioMmioState>>);

impl MmioOps for VirtioMmioRegion {
    fn read(&self, offset: u64, size: u32) -> u64 {
        let state = self.0.lock().unwrap();
        VirtioMmio::read_locked(&state, offset, size)
    }

    fn write(&self, offset: u64, _size: u32, val: u64) {
        let mut state = self.0.lock().unwrap();
        VirtioMmio::write_locked(&mut state, offset, val);
    }
}

// ---- RX receive loop (runs in a dedicated thread) ----

fn rx_loop(
    tap_fd: i32,
    state: Arc<Mutex<VirtioMmioState>>,
    stop: Arc<AtomicBool>,
) {
    use crate::net::fill_rx_queue;

    let mut buf = vec![0u8; 65536];

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }

        // Poll the TAP fd with a 100ms timeout so we can
        // check the stop flag periodically.
        let mut pfd = libc::pollfd {
            fd: tap_fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ret = unsafe { libc::poll(&mut pfd, 1, 100) };
        if ret <= 0 {
            continue;
        }

        let n = unsafe {
            libc::read(
                tap_fd,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
            )
        };
        if n <= 0 {
            continue;
        }

        let packet = &buf[..n as usize];

        let mut st = state.lock().unwrap();
        if st.status & 0x4 == 0 {
            continue;
        }
        if st.queues.is_empty()
            || !st.queues[0].ready
            || st.queues[0].num == 0
        {
            continue;
        }

        // Copy scalar fields before splitting the mutable
        // borrow on `queues`.
        let ram_ptr = st.ram_ptr;
        let ram_base = st.ram_base;
        let ram_size = st.ram_size;
        let injected = unsafe {
            fill_rx_queue(
                packet,
                &mut st.queues[0],
                ram_ptr,
                ram_base,
                ram_size,
            )
        };

        if injected > 0 {
            st.interrupt_status |= 1;
            st.irq.set(true);
        }
    }
}
