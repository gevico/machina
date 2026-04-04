use std::any::Any;

use machina_core::mobject::{MObject, MObjectState};
use machina_hw_core::mdev::{
    MDevice, MDeviceError, MDeviceLifecycle, MDeviceState,
};

struct TestMDevice {
    state: MDeviceState,
}

impl TestMDevice {
    fn new(local_id: &str) -> Self {
        Self {
            state: MDeviceState::new(local_id),
        }
    }
}

impl MObject for TestMDevice {
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

impl MDevice for TestMDevice {
    fn mdevice_state(&self) -> &MDeviceState {
        &self.state
    }

    fn mdevice_state_mut(&mut self) -> &mut MDeviceState {
        &mut self.state
    }
}

#[test]
fn test_mdevice_state_defaults_to_created() {
    let dev = TestMDevice::new("uart0");
    assert_eq!(dev.lifecycle(), MDeviceLifecycle::Created);
    assert!(!dev.is_realized());
    assert_eq!(dev.local_id(), "uart0");
}

#[test]
fn test_mdevice_realize_and_unrealize_transitions() {
    let mut dev = TestMDevice::new("uart0");
    dev.mdevice_state_mut().mark_realized().unwrap();
    assert_eq!(dev.lifecycle(), MDeviceLifecycle::Realized);
    dev.mdevice_state_mut().mark_unrealized().unwrap();
    assert_eq!(dev.lifecycle(), MDeviceLifecycle::Created);
}

#[test]
fn test_mdevice_parent_bus_late_mutation_rejected() {
    let mut dev = TestMDevice::new("uart0");
    dev.mdevice_state_mut().mark_realized().unwrap();
    let err = dev
        .mdevice_state_mut()
        .set_parent_bus("sysbus0")
        .expect_err("late parent_bus mutation must fail");
    assert_eq!(err, MDeviceError::LateMutation("parent_bus"));
}
