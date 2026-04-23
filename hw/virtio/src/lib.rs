pub mod block;
pub mod mmio;
#[cfg(unix)]
pub mod net;
pub mod queue;

use crate::queue::VirtQueue;

/// Backend-agnostic VirtIO device interface.
///
/// Each concrete device (block, net, ...) implements this
/// trait. The MMIO transport delegates device-specific
/// operations through it.
pub trait VirtioDevice: Send {
    fn device_id(&self) -> u32;
    fn features(&self) -> u64;
    fn ack_features(&mut self, features: u64);
    fn num_queues(&self) -> usize;
    fn config_read(&self, offset: u64, size: u32) -> u64;
    fn config_write(&mut self, _offset: u64, _size: u32, _val: u64) {}
    fn reset(&mut self) {}
    /// Called after the MMIO transport is fully constructed.
    /// Devices that need the shared transport state (e.g.
    /// for background I/O threads) implement this.
    fn start_io(
        &mut self,
        _mmio: std::sync::Arc<std::sync::Mutex<crate::mmio::VirtioMmioState>>,
    ) {
    }
    /// Process pending requests in the given queue.
    ///
    /// # Safety
    /// Caller must ensure `ram` is valid for the range
    /// [`ram_base`, `ram_base + ram_size`).
    unsafe fn handle_queue(
        &mut self,
        idx: u32,
        queue: &mut VirtQueue,
        ram: *mut u8,
        ram_base: u64,
        ram_size: u64,
    ) -> u32;
}
