use std::any::Any;

use machina_core::mobject::{MObject, MObjectState};
use machina_hw_core::mdev::{MDevice, MDeviceError, MDeviceState};
use machina_hw_core::property::{
    MPropertyMutability, MPropertySpec, MPropertyType, MPropertyValue,
};

struct TestPropertyDevice {
    state: MDeviceState,
}

impl TestPropertyDevice {
    fn new(local_id: &str) -> Self {
        let mut state = MDeviceState::new(local_id);
        state
            .define_property(
                MPropertySpec::new("label", MPropertyType::String)
                    .default(MPropertyValue::String("serial0".to_string())),
            )
            .unwrap();
        state
            .define_property(
                MPropertySpec::new("chardev", MPropertyType::Link).required(),
            )
            .unwrap();
        state
            .define_property(
                MPropertySpec::new("loopback", MPropertyType::Bool).dynamic(),
            )
            .unwrap();
        Self { state }
    }
}

impl MObject for TestPropertyDevice {
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

impl MDevice for TestPropertyDevice {
    fn mdevice_state(&self) -> &MDeviceState {
        &self.state
    }

    fn mdevice_state_mut(&mut self) -> &mut MDeviceState {
        &mut self.state
    }
}

#[test]
fn test_property_default_and_lookup() {
    let dev = TestPropertyDevice::new("uart0");
    assert_eq!(
        dev.mdevice_state().property("label"),
        Some(&MPropertyValue::String("serial0".to_string()))
    );
    assert_eq!(
        dev.mdevice_state().property_names(),
        vec!["chardev", "label", "loopback"]
    );
}

#[test]
fn test_property_required_blocks_realize() {
    let mut dev = TestPropertyDevice::new("uart0");
    let err = dev
        .mdevice_state_mut()
        .mark_realized()
        .expect_err("missing required property must block realize");
    assert_eq!(
        err,
        MDeviceError::MissingRequiredProperty("chardev".to_string())
    );
}

#[test]
fn test_property_set_and_realize() {
    let mut dev = TestPropertyDevice::new("uart0");
    dev.mdevice_state_mut()
        .set_property(
            "chardev",
            MPropertyValue::Link("/machine/chardev/serial0".to_string()),
        )
        .unwrap();
    dev.mdevice_state_mut().mark_realized().unwrap();
    assert!(dev.is_realized());
}

#[test]
fn test_property_type_mismatch_rejected() {
    let mut dev = TestPropertyDevice::new("uart0");
    let err = dev
        .mdevice_state_mut()
        .set_property("chardev", MPropertyValue::Bool(true))
        .expect_err("type mismatch must fail");
    assert_eq!(
        err,
        MDeviceError::PropertyTypeMismatch {
            name: "chardev".to_string(),
            expected: MPropertyType::Link,
            actual: MPropertyType::Bool,
        }
    );
}

#[test]
fn test_property_static_late_mutation_rejected() {
    let mut dev = TestPropertyDevice::new("uart0");
    dev.mdevice_state_mut()
        .set_property(
            "chardev",
            MPropertyValue::Link("/machine/chardev/serial0".to_string()),
        )
        .unwrap();
    dev.mdevice_state_mut().mark_realized().unwrap();

    let err = dev
        .mdevice_state_mut()
        .set_property("label", MPropertyValue::String("ttyS0".to_string()))
        .expect_err("static property mutation after realize must fail");
    assert_eq!(err, MDeviceError::LateMutation("property"));
}

#[test]
fn test_property_dynamic_mutation_after_realize() {
    let mut dev = TestPropertyDevice::new("uart0");
    dev.mdevice_state_mut()
        .set_property(
            "chardev",
            MPropertyValue::Link("/machine/chardev/serial0".to_string()),
        )
        .unwrap();
    dev.mdevice_state_mut().mark_realized().unwrap();
    dev.mdevice_state_mut()
        .set_property("loopback", MPropertyValue::Bool(true))
        .unwrap();

    assert_eq!(
        dev.mdevice_state().property("loopback"),
        Some(&MPropertyValue::Bool(true))
    );
}

#[test]
fn test_property_duplicate_definition_rejected() {
    let mut state = MDeviceState::new("uart0");
    state
        .define_property(MPropertySpec::new("label", MPropertyType::String))
        .unwrap();
    let err = state
        .define_property(MPropertySpec::new("label", MPropertyType::String))
        .expect_err("duplicate property definition must fail");
    assert_eq!(err, MDeviceError::DuplicateProperty("label".to_string()));
}

#[test]
fn test_property_schema_late_mutation_rejected() {
    let mut state = MDeviceState::new("uart0");
    state
        .define_property(
            MPropertySpec::new("chardev", MPropertyType::Link).required(),
        )
        .unwrap();
    state
        .set_property(
            "chardev",
            MPropertyValue::Link("/machine/chardev/serial0".to_string()),
        )
        .unwrap();
    state.mark_realized().unwrap();

    let err = state
        .define_property(MPropertySpec::new("baud", MPropertyType::U32))
        .expect_err("property schema must freeze after realize");
    assert_eq!(err, MDeviceError::LateMutation("property_schema"));
}

#[test]
fn test_property_dynamic_flag_is_recorded() {
    let spec = MPropertySpec::new("loopback", MPropertyType::Bool).dynamic();
    assert_eq!(spec.mutability, MPropertyMutability::Dynamic);
}
