// VirtIO-PCI transport (modern, v1).
//
// Implements a PCI Type-0 device with VirtIO capabilities.
// The PCI config space contains VirtIO capability structures
// pointing into BAR0, which holds the common config, ISR,
// notify, and device-specific registers.
//
// Two MmioOps are exposed:
//   - PciConfigOps:  handles ECAM config-space accesses
//   - PciBarOps:     handles BAR0 register accesses

use std::sync::{Arc, Mutex};

use machina_hw_core::irq::IrqLine;
use machina_memory::region::MmioOps;

use crate::block::VirtioBlk;
use crate::device::VirtioDevice;
use crate::queue::{VirtQueue, MAX_QUEUE_SIZE};

// PCI vendor/device IDs.
const PCI_VENDOR_VIRTIO: u16 = 0x1AF4;
const PCI_DEVICE_BLK_MODERN: u16 = 0x1042;
const PCI_SUBSYSTEM_VENDOR: u16 = 0x1AF4;
const PCI_SUBSYSTEM_BLK: u16 = 0x0002;

const PCI_CLASS_MASS_STORAGE: u8 = 0x01;

// PCI capability IDs.
const PCI_CAP_ID_VNDR: u8 = 0x09;

// VirtIO PCI capability types.
const VIRTIO_PCI_CAP_COMMON_CFG: u8 = 1;
const VIRTIO_PCI_CAP_NOTIFY_CFG: u8 = 2;
const VIRTIO_PCI_CAP_ISR_CFG: u8 = 3;
const VIRTIO_PCI_CAP_DEVICE_CFG: u8 = 4;

// BAR0 sub-region offsets and sizes.
const BAR0_COMMON_OFF: u32 = 0x000;
const BAR0_COMMON_LEN: u32 = 0x040;
const BAR0_ISR_OFF: u32 = 0x040;
const BAR0_ISR_LEN: u32 = 0x004;
const BAR0_NOTIFY_OFF: u32 = 0x044;
const BAR0_NOTIFY_LEN: u32 = 0x004;
const BAR0_DEVICE_OFF: u32 = 0x048;
const BAR0_DEVICE_LEN: u32 = 0x040;

pub const BAR0_SIZE: u32 = 0x1000; // 4 KiB

const NUM_QUEUES: usize = 1;

// PCI config space offsets.
const PCI_VENDOR_ID: usize = 0x00;
const PCI_DEVICE_ID: usize = 0x02;
const PCI_COMMAND: usize = 0x04;
const PCI_STATUS: usize = 0x06;
const PCI_REVISION_ID: usize = 0x08;
const PCI_CLASS_PROG: usize = 0x09;
const PCI_SUBCLASS: usize = 0x0A;
const PCI_CLASS_DEVICE: usize = 0x0B;
const PCI_HEADER_TYPE: usize = 0x0E;
const PCI_BAR0: usize = 0x10;
const PCI_SUBSYSTEM_VENDOR_ID: usize = 0x2C;
const PCI_SUBSYSTEM_ID: usize = 0x2E;
const PCI_CAPABILITY_PTR: usize = 0x34;
const PCI_INTERRUPT_LINE: usize = 0x3C;
const PCI_INTERRUPT_PIN: usize = 0x3D;

// Capability chain starts at offset 0x40.
const CAP_COMMON: usize = 0x40;
const CAP_ISR: usize = 0x50;
const CAP_NOTIFY: usize = 0x60;
const CAP_DEVICE: usize = 0x74;

pub struct VirtioPciState {
    config: [u8; 256],
    bar0_sizing: bool,

    device: VirtioBlk,
    irq: IrqLine,

    status: u8,
    device_feature_select: u32,
    driver_feature_select: u32,
    driver_features: u64,
    config_generation: u8,
    queue_select: u16,
    queues: [VirtQueue; NUM_QUEUES],
    isr_status: u8,

    ram_ptr: *mut u8,
    ram_base: u64,
    ram_size: u64,
}

unsafe impl Send for VirtioPciState {}

impl VirtioPciState {
    pub fn new(
        device: VirtioBlk,
        irq: IrqLine,
        ram_ptr: *mut u8,
        ram_base: u64,
        ram_size: u64,
    ) -> Self {
        let mut config = [0u8; 256];

        // Standard PCI header.
        write16(&mut config, PCI_VENDOR_ID, PCI_VENDOR_VIRTIO);
        write16(&mut config, PCI_DEVICE_ID, PCI_DEVICE_BLK_MODERN);
        // Status: Capabilities List bit.
        write16(&mut config, PCI_STATUS, 0x0010);
        config[PCI_REVISION_ID] = 0x01;
        config[PCI_CLASS_PROG] = 0x00;
        config[PCI_SUBCLASS] = 0x00;
        config[PCI_CLASS_DEVICE] = PCI_CLASS_MASS_STORAGE;
        config[PCI_HEADER_TYPE] = 0x00;
        // BAR0 = 0 (memory-mapped, 32-bit, non-prefetchable).
        write32(&mut config, PCI_BAR0, 0x0000_0000);
        write16(&mut config, PCI_SUBSYSTEM_VENDOR_ID, PCI_SUBSYSTEM_VENDOR);
        write16(&mut config, PCI_SUBSYSTEM_ID, PCI_SUBSYSTEM_BLK);
        config[PCI_CAPABILITY_PTR] = CAP_COMMON as u8;
        config[PCI_INTERRUPT_PIN] = 0x01; // INTA#

        // VirtIO PCI capabilities.
        write_virtio_cap(
            &mut config,
            CAP_COMMON,
            VIRTIO_PCI_CAP_COMMON_CFG,
            CAP_ISR as u8,
            0,
            BAR0_COMMON_OFF,
            BAR0_COMMON_LEN,
        );
        write_virtio_cap(
            &mut config,
            CAP_ISR,
            VIRTIO_PCI_CAP_ISR_CFG,
            CAP_NOTIFY as u8,
            0,
            BAR0_ISR_OFF,
            BAR0_ISR_LEN,
        );
        // Notify cap is 20 bytes (extra u32 for multiplier).
        write_virtio_cap(
            &mut config,
            CAP_NOTIFY,
            VIRTIO_PCI_CAP_NOTIFY_CFG,
            CAP_DEVICE as u8,
            0,
            BAR0_NOTIFY_OFF,
            BAR0_NOTIFY_LEN,
        );
        config[CAP_NOTIFY + 2] = 20; // cap_len = 20 for notify
        write32(&mut config, CAP_NOTIFY + 16, 0); // notify_off_multiplier = 0

        write_virtio_cap(
            &mut config,
            CAP_DEVICE,
            VIRTIO_PCI_CAP_DEVICE_CFG,
            0, // end of chain
            0,
            BAR0_DEVICE_OFF,
            BAR0_DEVICE_LEN,
        );

        Self {
            config,
            bar0_sizing: false,
            device,
            irq,
            status: 0,
            device_feature_select: 0,
            driver_feature_select: 0,
            driver_features: 0,
            config_generation: 0,
            queue_select: 0,
            queues: std::array::from_fn(|_| VirtQueue::new()),
            isr_status: 0,
            ram_ptr,
            ram_base,
            ram_size,
        }
    }

    fn reset(&mut self) {
        self.status = 0;
        self.device_feature_select = 0;
        self.driver_feature_select = 0;
        self.driver_features = 0;
        self.config_generation = 0;
        self.queue_select = 0;
        for q in &mut self.queues {
            q.reset();
        }
        self.isr_status = 0;
        self.irq.set(false);
    }

    fn current_queue(&mut self) -> Option<&mut VirtQueue> {
        self.queues.get_mut(self.queue_select as usize)
    }

    fn process_notify(&mut self, _queue_idx: u16) {
        let sel = _queue_idx as usize;
        if sel >= NUM_QUEUES {
            return;
        }
        let q = &mut self.queues[sel];
        if !q.ready || q.num == 0 {
            return;
        }
        if self.status & 0x04 == 0 {
            return;
        }
        let n = unsafe {
            self.device
                .handle_queue(sel, q, self.ram_ptr, self.ram_base, self.ram_size)
        };
        if n > 0 {
            self.isr_status |= 1;
            self.irq.set(true);
        }
    }

    // ---- PCI config space read/write ----

    pub fn config_read(&self, reg: u64, size: u32) -> u64 {
        let off = reg as usize;
        if off == PCI_BAR0 && self.bar0_sizing {
            let mask = !(BAR0_SIZE as u64 - 1);
            return mask & 0xFFFF_FFFF;
        }
        read_config_bytes(&self.config, off, size)
    }

    pub fn config_write(&mut self, reg: u64, _size: u32, val: u64) {
        let off = reg as usize;
        match off {
            PCI_BAR0 => {
                let v = val as u32;
                if v == 0xFFFF_FFFF {
                    self.bar0_sizing = true;
                } else {
                    self.bar0_sizing = false;
                    let addr = v & !(BAR0_SIZE - 1);
                    write32(&mut self.config, PCI_BAR0, addr);
                }
            }
            PCI_COMMAND => {
                let v = (val as u16) & 0x0007;
                write16(&mut self.config, PCI_COMMAND, v);
            }
            PCI_INTERRUPT_LINE => {
                self.config[PCI_INTERRUPT_LINE] = val as u8;
            }
            _ => {}
        }
    }

    // ---- BAR0 register read/write ----

    pub fn bar_read(&self, offset: u64, size: u32) -> u64 {
        let off = offset as u32;
        if off >= BAR0_COMMON_OFF && off < BAR0_COMMON_OFF + BAR0_COMMON_LEN {
            return self.common_read(off - BAR0_COMMON_OFF, size);
        }
        if off >= BAR0_ISR_OFF && off < BAR0_ISR_OFF + BAR0_ISR_LEN {
            return self.isr_read();
        }
        if off >= BAR0_DEVICE_OFF && off < BAR0_DEVICE_OFF + BAR0_DEVICE_LEN {
            return self
                .device
                .config_read((off - BAR0_DEVICE_OFF) as u64, size);
        }
        0
    }

    pub fn bar_write(&mut self, offset: u64, size: u32, val: u64) {
        let off = offset as u32;
        if off >= BAR0_COMMON_OFF && off < BAR0_COMMON_OFF + BAR0_COMMON_LEN {
            self.common_write(off - BAR0_COMMON_OFF, size, val);
            return;
        }
        if off >= BAR0_NOTIFY_OFF && off < BAR0_NOTIFY_OFF + BAR0_NOTIFY_LEN {
            self.process_notify(val as u16);
            return;
        }
    }

    // ---- common config ----

    fn common_read(&self, off: u32, _size: u32) -> u64 {
        match off {
            0x00 => self.device_feature_select as u64,
            0x04 => {
                let f = self.device.features();
                if self.device_feature_select == 0 {
                    f & 0xFFFF_FFFF
                } else {
                    (f >> 32) & 0xFFFF_FFFF
                }
            }
            0x08 => self.driver_feature_select as u64,
            0x0C => {
                if self.driver_feature_select == 0 {
                    self.driver_features & 0xFFFF_FFFF
                } else {
                    (self.driver_features >> 32) & 0xFFFF_FFFF
                }
            }
            0x10 => 0xFFFF, // msix_config (no MSI-X)
            0x12 => NUM_QUEUES as u64,
            0x14 => self.status as u64,
            0x15 => self.config_generation as u64,
            0x16 => self.queue_select as u64,
            0x18 => {
                self.queues
                    .get(self.queue_select as usize)
                    .map(|q| if q.num == 0 { MAX_QUEUE_SIZE } else { q.num })
                    .unwrap_or(0) as u64
            }
            0x1A => 0xFFFF, // queue_msix_vector
            0x1C => {
                self.queues
                    .get(self.queue_select as usize)
                    .map(|q| q.ready as u64)
                    .unwrap_or(0)
            }
            0x1E => 0, // queue_notify_off
            0x20 => {
                self.queues
                    .get(self.queue_select as usize)
                    .map(|q| q.desc_addr as u64 & 0xFFFF_FFFF)
                    .unwrap_or(0)
            }
            0x24 => {
                self.queues
                    .get(self.queue_select as usize)
                    .map(|q| (q.desc_addr >> 32) & 0xFFFF_FFFF)
                    .unwrap_or(0)
            }
            0x28 => {
                self.queues
                    .get(self.queue_select as usize)
                    .map(|q| q.avail_addr & 0xFFFF_FFFF)
                    .unwrap_or(0)
            }
            0x2C => {
                self.queues
                    .get(self.queue_select as usize)
                    .map(|q| (q.avail_addr >> 32) & 0xFFFF_FFFF)
                    .unwrap_or(0)
            }
            0x30 => {
                self.queues
                    .get(self.queue_select as usize)
                    .map(|q| q.used_addr & 0xFFFF_FFFF)
                    .unwrap_or(0)
            }
            0x34 => {
                self.queues
                    .get(self.queue_select as usize)
                    .map(|q| (q.used_addr >> 32) & 0xFFFF_FFFF)
                    .unwrap_or(0)
            }
            _ => 0,
        }
    }

    fn common_write(&mut self, off: u32, _size: u32, val: u64) {
        match off {
            0x00 => self.device_feature_select = val as u32,
            0x08 => self.driver_feature_select = val as u32,
            0x0C => {
                let v = val as u32;
                if self.driver_feature_select == 0 {
                    self.driver_features =
                        (self.driver_features & 0xFFFF_FFFF_0000_0000) | (v as u64);
                } else {
                    self.driver_features =
                        (self.driver_features & 0x0000_0000_FFFF_FFFF) | ((v as u64) << 32);
                }
            }
            0x14 => {
                let v = val as u8;
                if v == 0 {
                    self.reset();
                } else {
                    self.status = v;
                }
            }
            0x16 => self.queue_select = val as u16,
            0x18 => {
                if let Some(q) = self.current_queue() {
                    q.num = (val as u32).min(MAX_QUEUE_SIZE);
                }
            }
            0x1C => {
                if let Some(q) = self.current_queue() {
                    q.ready = val as u16 != 0;
                }
            }
            0x20 => {
                if let Some(q) = self.current_queue() {
                    q.desc_addr =
                        (q.desc_addr & 0xFFFF_FFFF_0000_0000) | (val as u32 as u64);
                }
            }
            0x24 => {
                if let Some(q) = self.current_queue() {
                    q.desc_addr =
                        (q.desc_addr & 0x0000_0000_FFFF_FFFF) | ((val as u32 as u64) << 32);
                }
            }
            0x28 => {
                if let Some(q) = self.current_queue() {
                    q.avail_addr =
                        (q.avail_addr & 0xFFFF_FFFF_0000_0000) | (val as u32 as u64);
                }
            }
            0x2C => {
                if let Some(q) = self.current_queue() {
                    q.avail_addr =
                        (q.avail_addr & 0x0000_0000_FFFF_FFFF) | ((val as u32 as u64) << 32);
                }
            }
            0x30 => {
                if let Some(q) = self.current_queue() {
                    q.used_addr =
                        (q.used_addr & 0xFFFF_FFFF_0000_0000) | (val as u32 as u64);
                }
            }
            0x34 => {
                if let Some(q) = self.current_queue() {
                    q.used_addr =
                        (q.used_addr & 0x0000_0000_FFFF_FFFF) | ((val as u32 as u64) << 32);
                }
            }
            _ => {}
        }
    }

    fn isr_read(&self) -> u64 {
        let v = self.isr_status as u64;
        // ISR read clears interrupt; deassert after return.
        // (Interior mutability handled by caller holding the lock.)
        v
    }

    pub fn bar0_addr(&self) -> u32 {
        read32(&self.config, PCI_BAR0) & !(BAR0_SIZE - 1)
    }

    pub fn command(&self) -> u16 {
        read16(&self.config, PCI_COMMAND)
    }

    pub fn memory_enabled(&self) -> bool {
        self.command() & 0x02 != 0
    }
}

// ---- ECAM config-space MmioOps ----

/// Handles PCI ECAM config-space reads/writes.
///
/// ECAM address encoding: bus[27:20] | device[19:15] | function[14:12] | reg[11:0]
/// We only support bus 0, one device at a configurable slot.
pub struct PciEcamOps {
    device_slot: u32,
    state: Arc<Mutex<VirtioPciState>>,
}

impl PciEcamOps {
    pub fn new(device_slot: u32, state: Arc<Mutex<VirtioPciState>>) -> Self {
        Self { device_slot, state }
    }
}

impl MmioOps for PciEcamOps {
    fn read(&self, offset: u64, size: u32) -> u64 {
        let bus = (offset >> 20) & 0xFF;
        let device = (offset >> 15) & 0x1F;
        let function = (offset >> 12) & 0x7;
        let reg = offset & 0xFFF;

        if bus != 0 || function != 0 {
            return 0xFFFF_FFFF_FFFF_FFFF;
        }
        if device as u32 != self.device_slot {
            return 0xFFFF_FFFF_FFFF_FFFF;
        }

        let st = self.state.lock().unwrap();
        st.config_read(reg, size)
    }

    fn write(&self, offset: u64, size: u32, val: u64) {
        let bus = (offset >> 20) & 0xFF;
        let device = (offset >> 15) & 0x1F;
        let function = (offset >> 12) & 0x7;
        let reg = offset & 0xFFF;

        if bus != 0 || function != 0 || device as u32 != self.device_slot {
            return;
        }

        let mut st = self.state.lock().unwrap();
        st.config_write(reg, size, val);
    }
}

// ---- BAR0 MmioOps ----

/// Handles VirtIO-PCI BAR0 register reads/writes.
///
/// Registered at the PCI MMIO window base. On each access
/// it checks whether the BAR0 address has been programmed
/// to cover this offset.
pub struct PciBarOps {
    mmio_base: u64,
    state: Arc<Mutex<VirtioPciState>>,
}

impl PciBarOps {
    pub fn new(mmio_base: u64, state: Arc<Mutex<VirtioPciState>>) -> Self {
        Self { mmio_base, state }
    }
}

impl MmioOps for PciBarOps {
    fn read(&self, offset: u64, size: u32) -> u64 {
        let mut st = self.state.lock().unwrap();
        if !st.memory_enabled() {
            return 0xFFFF_FFFF;
        }
        let bar_addr = st.bar0_addr() as u64;
        if bar_addr == 0 {
            return 0xFFFF_FFFF;
        }
        let abs = self.mmio_base + offset;
        if abs < bar_addr || abs >= bar_addr + BAR0_SIZE as u64 {
            return 0xFFFF_FFFF;
        }
        let bar_off = abs - bar_addr;

        if bar_off >= BAR0_ISR_OFF as u64
            && bar_off < (BAR0_ISR_OFF + BAR0_ISR_LEN) as u64
        {
            let v = st.isr_status as u64;
            st.isr_status = 0;
            st.irq.set(false);
            return v;
        }

        st.bar_read(bar_off, size)
    }

    fn write(&self, offset: u64, size: u32, val: u64) {
        let mut st = self.state.lock().unwrap();
        if !st.memory_enabled() {
            return;
        }
        let bar_addr = st.bar0_addr() as u64;
        if bar_addr == 0 {
            return;
        }
        let abs = self.mmio_base + offset;
        if abs < bar_addr || abs >= bar_addr + BAR0_SIZE as u64 {
            return;
        }
        st.bar_write(abs - bar_addr, size, val);
    }
}

// ---- Helpers ----

fn write16(buf: &mut [u8], off: usize, val: u16) {
    buf[off..off + 2].copy_from_slice(&val.to_le_bytes());
}

fn write32(buf: &mut [u8], off: usize, val: u32) {
    buf[off..off + 4].copy_from_slice(&val.to_le_bytes());
}

fn read16(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

fn read32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn read_config_bytes(config: &[u8], off: usize, size: u32) -> u64 {
    match size {
        1 => config.get(off).copied().unwrap_or(0xFF) as u64,
        2 => {
            let lo = config.get(off).copied().unwrap_or(0xFF) as u64;
            let hi = config.get(off + 1).copied().unwrap_or(0xFF) as u64;
            lo | (hi << 8)
        }
        4 => {
            let mut v = 0u64;
            for i in 0..4 {
                v |= (config.get(off + i).copied().unwrap_or(0xFF) as u64) << (i * 8);
            }
            v
        }
        _ => 0xFFFF_FFFF,
    }
}

fn write_virtio_cap(
    config: &mut [u8],
    off: usize,
    cfg_type: u8,
    cap_next: u8,
    bar: u8,
    bar_offset: u32,
    bar_length: u32,
) {
    config[off] = PCI_CAP_ID_VNDR;
    config[off + 1] = cap_next;
    config[off + 2] = 16; // cap_len (standard)
    config[off + 3] = cfg_type;
    config[off + 4] = bar;
    // padding [5..8]
    write32(config, off + 8, bar_offset);
    write32(config, off + 12, bar_length);
}
