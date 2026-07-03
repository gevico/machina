//! Minimal K230 KPU (Knowledge Process Unit) device model.
//!
//! The K230 KPU is Canaan's AI/NPU inference accelerator. Its
//! register/command interface is undocumented and driven by the
//! closed-source `nncase` runtime, so mainline QEMU models the two KPU
//! windows as `create_unimplemented_device` stubs ("kpu.l2-cache" and
//! "kpu_cfg"). This model promotes them to a real SysBus device:
//!
//! - The CFG window (`0x8040_0000`, 2 KiB) is modelled as a plain
//!   scratch register file: written values are retained and read back,
//!   with no invented per-register semantics.
//! - The L2-cache window (`0x8000_0000`, 2 MiB) is a read-as-zero,
//!   writes-ignored stub, matching QEMU's unimplemented-device.
//!
//! No KPU register offsets, IRQ, or DMA behaviour is invented here; the
//! real command/descriptor format is sealed inside `nncase`.

use std::sync::Arc;

use machina_core::device_cell::DeviceRegs;
use machina_hw_core::bus::SysBusDeviceState;
use machina_memory::region::MmioOps;

/// Size of the KPU CFG register window (matches `K230MemMap::KpuCfg`).
pub const K230_KPU_CFG_MMIO_SIZE: u64 = 0x0000_0800;

struct K230KpuRegs {
    cfg: [u8; K230_KPU_CFG_MMIO_SIZE as usize],
}

impl Default for K230KpuRegs {
    fn default() -> Self {
        Self {
            cfg: [0; K230_KPU_CFG_MMIO_SIZE as usize],
        }
    }
}

#[derive(machina_hw_core::SysBusDevice)]
#[mom(state = state, lock = "std")]
pub struct K230Kpu {
    state: std::sync::Mutex<SysBusDeviceState>,
    regs: DeviceRegs<K230KpuRegs>,
}

impl K230Kpu {
    #[must_use]
    pub fn new_named(local_id: &str) -> Arc<Self> {
        Arc::new(Self {
            state: std::sync::Mutex::new(SysBusDeviceState::new(local_id)),
            regs: DeviceRegs::new(K230KpuRegs::default()),
        })
    }

    pub fn reset_runtime(&self) {
        *self.regs.lock() = K230KpuRegs::default();
    }

    /// CFG window: a scratch register file. Written values are retained
    /// and read back; locations never written read back as zero.
    fn read_cfg(&self, offset: u64, size: u32) -> u64 {
        if !valid_mmio_access(offset, size) {
            return 0;
        }
        let regs = self.regs.lock();
        read_bytes_le(&regs.cfg, offset, size)
    }

    fn write_cfg(&self, offset: u64, size: u32, value: u64) {
        if !valid_mmio_access(offset, size) {
            return;
        }
        let mut regs = self.regs.lock();
        write_bytes_le(&mut regs.cfg, offset, size, value);
    }

    /// L2-cache window: a faithful unimplemented-device stub. Reads
    /// return zero and writes are dropped, matching mainline QEMU.
    fn read_l2(&self, _offset: u64, _size: u32) -> u64 {
        0
    }

    fn write_l2(&self, _offset: u64, _size: u32, _value: u64) {}
}

pub struct K230KpuCfgMmio(pub Arc<K230Kpu>);

impl MmioOps for K230KpuCfgMmio {
    fn read(&self, offset: u64, size: u32) -> u64 {
        self.0.read_cfg(offset, size)
    }

    fn write(&self, offset: u64, size: u32, value: u64) {
        self.0.write_cfg(offset, size, value);
    }
}

pub struct K230KpuL2Mmio(pub Arc<K230Kpu>);

impl MmioOps for K230KpuL2Mmio {
    fn read(&self, offset: u64, size: u32) -> u64 {
        self.0.read_l2(offset, size)
    }

    fn write(&self, offset: u64, size: u32, value: u64) {
        self.0.write_l2(offset, size, value);
    }
}

fn valid_mmio_access(offset: u64, size: u32) -> bool {
    matches!(size, 1 | 2 | 4 | 8) && offset.is_multiple_of(u64::from(size))
}

fn read_bytes_le(bytes: &[u8], offset: u64, size: u32) -> u64 {
    let index = offset as usize;
    let len = size as usize;
    if index + len > bytes.len() {
        return 0;
    }
    let mut value = 0;
    for (shift, byte) in bytes[index..index + len].iter().copied().enumerate() {
        value |= u64::from(byte) << (shift * 8);
    }
    value
}

fn write_bytes_le(bytes: &mut [u8], offset: u64, size: u32, value: u64) {
    let index = offset as usize;
    let len = size as usize;
    if index + len > bytes.len() {
        return;
    }
    let encoded = value.to_le_bytes();
    bytes[index..index + len].copy_from_slice(&encoded[..len]);
}
