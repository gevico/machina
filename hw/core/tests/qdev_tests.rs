use std::any::Any;

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
    fn name(&self) -> &str {
        &self.state.name
    }

    fn realize(&mut self) -> Result<(), String> {
        self.state.realized = true;
        Ok(())
    }

    fn reset(&mut self) {
        self.counter = 0;
    }

    fn realized(&self) -> bool {
        self.state.realized
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
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

    let any_ref = dev.as_any();
    let downcasted = any_ref.downcast_ref::<TestDevice>().unwrap();
    assert_eq!(downcasted.counter, 7);
}
