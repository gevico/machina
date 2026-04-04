// Device object model bridge — transitional shim toward MOM mdev.

use crate::mdev::{MDevice, MDeviceError, MDeviceState};

/// Base trait for all emulated devices.
pub trait Device: MDevice {
    fn name(&self) -> &str {
        self.mdevice_state().local_id()
    }

    fn realize(&mut self) -> Result<(), MDeviceError>;
    fn unrealize(&mut self) -> Result<(), MDeviceError>;
    fn reset(&mut self);

    fn realized(&self) -> bool {
        self.is_realized()
    }
}

pub type DeviceState = MDeviceState;
