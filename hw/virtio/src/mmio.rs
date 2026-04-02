// VirtIO MMIO transport (Modern, v2).
//
// Implements the standard VirtIO MMIO register interface
// and delegates device-specific operations to a VirtioBlk
// backend.

use std::sync::Mutex;

use machina_hw_core::irq::IrqLine;
use machina_memory::region::MmioOps;

use crate::block::VirtioBlk;
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

// VirtIO magic value.
const VIRTIO_MAGIC: u32 = 0x74726976;
const VIRTIO_VENDOR: u32 = 0x554D4551;
const VIRTIO_VERSION: u32 = 2;
const VIRTIO_DEVICE_BLK: u32 = 2;

// Max number of queues per device.
const NUM_QUEUES: usize = 1;

struct VirtioMmioState {
    device: VirtioBlk,
    irq: IrqLine,

    // Transport state.
    status: u32,
    device_features_sel: u32,
    driver_features_sel: u32,
    driver_features: u64,
    queue_sel: u32,
    queues: [VirtQueue; NUM_QUEUES],
    interrupt_status: u32,
    // Legacy compat fields.
    guest_page_size: u32,

    // Guest RAM access.
    ram_ptr: *mut u8,
    ram_base: u64,
    ram_size: u64,
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

    fn current_queue(
        &mut self,
    ) -> Option<&mut VirtQueue> {
        let sel = self.queue_sel as usize;
        self.queues.get_mut(sel)
    }

    fn process_notify(&mut self) {
        let sel = self.queue_sel as usize;
        if sel >= NUM_QUEUES {
            return;
        }
        let q = &mut self.queues[sel];
        if !q.ready || q.num == 0 {
            return;
        }
        // Only process when DRIVER_OK (bit 2) is set.
        if self.status & 0x4 == 0 {
            return;
        }
        let n = self.device.handle_queue(
            q,
            self.ram_ptr,
            self.ram_base,
            self.ram_size,
        );
        if n > 0 {
            self.interrupt_status |= 1;
            self.irq.set(true);
        }
    }
}

/// VirtIO MMIO device wrapper implementing MmioOps.
pub struct VirtioMmio {
    state: Mutex<VirtioMmioState>,
}

impl VirtioMmio {
    pub fn new(
        device: VirtioBlk,
        irq: IrqLine,
        ram_ptr: *mut u8,
        ram_base: u64,
        ram_size: u64,
    ) -> Self {
        Self {
            state: Mutex::new(VirtioMmioState {
                device,
                irq,
                status: 0,
                device_features_sel: 0,
                driver_features_sel: 0,
                driver_features: 0,
                queue_sel: 0,
                queues: std::array::from_fn(|_| {
                    VirtQueue::new()
                }),
                interrupt_status: 0,
                guest_page_size: 0,
                ram_ptr,
                ram_base,
                ram_size,
            }),
        }
    }
}

impl MmioOps for VirtioMmio {
    fn read(&self, offset: u64, size: u32) -> u64 {
        let s = self.state.lock().unwrap();
        match offset {
            MAGIC_VALUE => VIRTIO_MAGIC as u64,
            VERSION => VIRTIO_VERSION as u64,
            DEVICE_ID => VIRTIO_DEVICE_BLK as u64,
            VENDOR_ID => VIRTIO_VENDOR as u64,
            DEVICE_FEATURES => {
                let feat = s.device.features();
                let sel = s.device_features_sel;
                if sel == 0 {
                    (feat & 0xFFFF_FFFF) as u64
                } else {
                    ((feat >> 32) & 0xFFFF_FFFF) as u64
                }
            }
            QUEUE_NUM_MAX => {
                MAX_QUEUE_SIZE as u64
            }
            QUEUE_READY => {
                let sel = s.queue_sel as usize;
                s.queues
                    .get(sel)
                    .map(|q| q.ready as u64)
                    .unwrap_or(0)
            }
            INTERRUPT_STATUS => {
                s.interrupt_status as u64
            }
            STATUS => s.status as u64,
            CONFIG_GENERATION => 0,
            // Legacy: queue PFN.
            LEGACY_QUEUE_PFN => {
                let sel = s.queue_sel as usize;
                s.queues
                    .get(sel)
                    .map(|q| {
                        if s.guest_page_size > 0 {
                            (q.desc_addr
                                / s.guest_page_size
                                    as u64)
                                as u64
                        } else {
                            0
                        }
                    })
                    .unwrap_or(0)
            }
            o if o >= CONFIG_BASE => {
                s.device
                    .config_read(o - CONFIG_BASE, size)
            }
            _ => 0,
        }
    }

    fn write(&self, offset: u64, _size: u32, val: u64) {
        let mut s = self.state.lock().unwrap();
        let v32 = val as u32;
        match offset {
            DEVICE_FEATURES_SEL => {
                s.device_features_sel = v32;
            }
            DRIVER_FEATURES => {
                let sel = s.driver_features_sel;
                if sel == 0 {
                    s.driver_features = (s.driver_features
                        & 0xFFFF_FFFF_0000_0000)
                        | (v32 as u64);
                } else {
                    s.driver_features = (s.driver_features
                        & 0x0000_0000_FFFF_FFFF)
                        | ((v32 as u64) << 32);
                }
            }
            DRIVER_FEATURES_SEL => {
                s.driver_features_sel = v32;
            }
            QUEUE_SEL => {
                s.queue_sel = v32;
            }
            QUEUE_NUM => {
                if let Some(q) = s.current_queue() {
                    q.num = v32.min(MAX_QUEUE_SIZE);
                }
            }
            QUEUE_READY => {
                if let Some(q) = s.current_queue() {
                    q.ready = v32 != 0;
                }
            }
            QUEUE_NOTIFY => {
                // val is the queue index to notify.
                let saved_sel = s.queue_sel;
                s.queue_sel = v32;
                s.process_notify();
                s.queue_sel = saved_sel;
            }
            INTERRUPT_ACK => {
                s.interrupt_status &= !v32;
                if s.interrupt_status == 0 {
                    s.irq.set(false);
                }
            }
            STATUS => {
                if v32 == 0 {
                    s.reset();
                } else {
                    // Cumulative bit semantics.
                    s.status = v32;
                }
            }
            QUEUE_DESC_LOW => {
                if let Some(q) = s.current_queue() {
                    q.desc_addr = (q.desc_addr
                        & 0xFFFF_FFFF_0000_0000)
                        | (v32 as u64);
                }
            }
            QUEUE_DESC_HIGH => {
                if let Some(q) = s.current_queue() {
                    q.desc_addr = (q.desc_addr
                        & 0x0000_0000_FFFF_FFFF)
                        | ((v32 as u64) << 32);
                }
            }
            QUEUE_AVAIL_LOW => {
                if let Some(q) = s.current_queue() {
                    q.avail_addr = (q.avail_addr
                        & 0xFFFF_FFFF_0000_0000)
                        | (v32 as u64);
                }
            }
            QUEUE_AVAIL_HIGH => {
                if let Some(q) = s.current_queue() {
                    q.avail_addr = (q.avail_addr
                        & 0x0000_0000_FFFF_FFFF)
                        | ((v32 as u64) << 32);
                }
            }
            QUEUE_USED_LOW => {
                if let Some(q) = s.current_queue() {
                    q.used_addr = (q.used_addr
                        & 0xFFFF_FFFF_0000_0000)
                        | (v32 as u64);
                }
            }
            QUEUE_USED_HIGH => {
                if let Some(q) = s.current_queue() {
                    q.used_addr = (q.used_addr
                        & 0x0000_0000_FFFF_FFFF)
                        | ((v32 as u64) << 32);
                }
            }
            // Legacy compat.
            LEGACY_GUEST_PAGE_SIZE => {
                s.guest_page_size = v32;
            }
            LEGACY_QUEUE_PFN => {
                let gps = s.guest_page_size;
                let sel = s.queue_sel as usize;
                if let Some(q) = s.queues.get_mut(sel) {
                    if v32 == 0 {
                        q.reset();
                    } else if gps > 0 {
                        let base =
                            (v32 as u64) * (gps as u64);
                        q.desc_addr = base;
                        let align = gps as u64;
                        let avail_off =
                            (q.num as u64) * 16;
                        q.avail_addr = base + avail_off;
                        let used_off = ((base
                            + avail_off
                            + 6
                            + (q.num as u64) * 2
                            + align
                            - 1)
                            / align)
                            * align;
                        q.used_addr = used_off;
                        q.ready = true;
                    }
                }
            }
            LEGACY_QUEUE_ALIGN => {
                // Accept but ignore (alignment is
                // implicit from guest_page_size).
            }
            _ => {}
        }
    }
}

