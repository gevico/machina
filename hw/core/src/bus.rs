// Bus-attached device interface.

use crate::qdev::Device;

/// A device that can be attached to a memory-mapped bus.
pub trait BusDevice: Device {
    fn read(&self, offset: u64, size: u32) -> u64;
    fn write(&mut self, offset: u64, size: u32, val: u64);
}
