use std::any::Any;

use machina_core::mobject::{MObject, MObjectState};
use machina_hw_core::mdev::{MDevice, MDeviceLifecycle};
use machina_hw_core::qdev::{Device, DeviceState};

struct TestDevice {
    state: DeviceState,
    counter: u32,
}

impl TestDevice {
    fn new(name: &str) -> Self {
        Self {
            state: DeviceState::new(name),
            counter: 0,
        }
    }
}

impl Device for TestDevice {
    fn realize(&mut self) -> Result<(), machina_hw_core::mdev::MDeviceError> {
        self.state.mark_realized()?;
        Ok(())
    }

    fn unrealize(&mut self) -> Result<(), machina_hw_core::mdev::MDeviceError> {
        self.state.mark_unrealized()?;
        Ok(())
    }

    fn reset(&mut self) {
        self.counter = 0;
    }
}

impl MObject for TestDevice {
    fn mobject_state(&self) -> &MObjectState {
        self.state.object()
    }

    fn mobject_state_mut(&mut self) -> &mut MObjectState {
        self.state.object_mut()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl MDevice for TestDevice {
    fn mdevice_state(&self) -> &DeviceState {
        &self.state
    }

    fn mdevice_state_mut(&mut self) -> &mut DeviceState {
        &mut self.state
    }
}

#[test]
fn test_device_realize() {
    let mut dev = TestDevice::new("test-dev");
    assert!(!dev.realized());
    assert!(dev.realize().is_ok());
    assert!(dev.realized());
}

#[test]
fn test_device_reset() {
    let mut dev = TestDevice::new("test-dev");
    dev.realize().unwrap();
    dev.counter = 42;
    assert_eq!(dev.counter, 42);
    dev.reset();
    assert_eq!(dev.counter, 0);
}

#[test]
fn test_device_name() {
    let dev = TestDevice::new("uart0");
    assert_eq!(dev.name(), "uart0");
}

#[test]
fn test_device_as_any_downcast() {
    let mut dev = TestDevice::new("dev");
    dev.realize().unwrap();
    dev.counter = 7;

    let any_ref = MObject::as_any(&dev);
    let downcasted = any_ref.downcast_ref::<TestDevice>().unwrap();
    assert_eq!(downcasted.counter, 7);
}

// -- parent_bus tests --

#[test]
fn test_parent_bus_default_none() {
    let state = DeviceState::new("dev0");
    assert!(state.parent_bus().is_none());
}

#[test]
fn test_parent_bus_set_and_get() {
    let mut state = DeviceState::new("dev0");
    state.set_parent_bus("sysbus0").unwrap();
    assert_eq!(state.parent_bus(), Some("sysbus0"));
}

#[test]
fn test_device_unrealize() {
    let mut dev = TestDevice::new("test-dev");
    dev.realize().unwrap();
    assert!(dev.realized());
    dev.unrealize().unwrap();
    assert!(!dev.realized());
}

#[test]
fn test_mdevice_lifecycle_created() {
    let state = DeviceState::new("dev0");
    assert_eq!(state.lifecycle(), MDeviceLifecycle::Created);
}
