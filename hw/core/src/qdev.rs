// Device object model — analogous to QEMU hw/core/qdev.c

use std::any::Any;

/// Base trait for all emulated devices.
pub trait Device: Send + Sync {
    fn name(&self) -> &str;
    fn realize(&mut self) -> Result<(), String>;
    fn reset(&mut self);
    fn realized(&self) -> bool;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// Common state shared by every device instance.
pub struct DeviceState {
    pub name: String,
    pub realized: bool,
}

impl DeviceState {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            realized: false,
        }
    }
}
