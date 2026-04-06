// VirtioDevice trait: abstracts device-specific behaviour
// from the MMIO transport layer.

use crate::queue::VirtQueue;

pub const VIRTIO_F_VERSION_1: u64 = 1 << 32;

pub trait VirtioDevice: Send {
    /// VirtIO device ID (1 = net, 2 = block, etc.).
    fn device_id(&self) -> u32;

    /// Number of virtqueues this device uses.
    fn num_queues(&self) -> usize;

    /// Device feature bits.
    fn features(&self) -> u64;

    /// Read device-specific config space.
    fn config_read(&self, offset: u64, size: u32) -> u64;

    /// Write device-specific config space (optional).
    fn config_write(&mut self, _offset: u64, _size: u32, _val: u64) {}

    /// Process a queue notification from the driver.
    ///
    /// # Safety
    /// Caller must ensure `ram` is valid for
    /// [`ram_base`, `ram_base + ram_size`).
    unsafe fn handle_queue(
        &mut self,
        queue_idx: usize,
        queue: &mut VirtQueue,
        ram: *mut u8,
        ram_base: u64,
        ram_size: u64,
    ) -> u32;

    /// If this device has an asynchronous receive source
    /// (e.g. a TAP fd for net), return the raw fd.
    /// The transport will spawn an RX thread polling it.
    fn rx_fd(&self) -> Option<i32> {
        None
    }
}

/// Read a sub-field of `bytes` at `off` with given `size`.
pub fn read_config_sub(bytes: &[u8], off: usize, size: u32) -> u64 {
    match size {
        1 => bytes.get(off).copied().unwrap_or(0) as u64,
        2 => {
            let b = [
                bytes.get(off).copied().unwrap_or(0),
                bytes.get(off + 1).copied().unwrap_or(0),
            ];
            u16::from_le_bytes(b) as u64
        }
        4 => {
            let mut b = [0u8; 4];
            for (i, item) in b.iter_mut().enumerate() {
                *item = bytes.get(off + i).copied().unwrap_or(0);
            }
            u32::from_le_bytes(b) as u64
        }
        8 => {
            let mut b = [0u8; 8];
            for (i, item) in b.iter_mut().enumerate() {
                *item = bytes.get(off + i).copied().unwrap_or(0);
            }
            u64::from_le_bytes(b)
        }
        _ => 0,
    }
}
