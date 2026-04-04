use std::any::Any;
use std::fmt;

use machina_core::mobject::{MObject, MObjectState};

use crate::property::{MPropertySet, MPropertySpec, MPropertyValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MDeviceLifecycle {
    Created,
    Realized,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MDeviceError {
    AlreadyRealized,
    NotRealized,
    LateMutation(&'static str),
    DuplicateProperty(String),
    UnknownProperty(String),
    MissingRequiredProperty(String),
    PropertyTypeMismatch {
        name: String,
        expected: crate::property::MPropertyType,
        actual: crate::property::MPropertyType,
    },
}

impl fmt::Display for MDeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRealized => write!(f, "device is already realized"),
            Self::NotRealized => write!(f, "device is not realized"),
            Self::LateMutation(what) => {
                write!(f, "cannot mutate {what} after realize")
            }
            Self::DuplicateProperty(name) => {
                write!(f, "property '{name}' is already defined")
            }
            Self::UnknownProperty(name) => {
                write!(f, "unknown property '{name}'")
            }
            Self::MissingRequiredProperty(name) => {
                write!(f, "required property '{name}' is missing")
            }
            Self::PropertyTypeMismatch {
                name,
                expected,
                actual,
            } => {
                write!(f, "property '{name}' expects {expected}, got {actual}")
            }
        }
    }
}

impl std::error::Error for MDeviceError {}

pub struct MDeviceState {
    object: MObjectState,
    lifecycle: MDeviceLifecycle,
    parent_bus: Option<String>,
    properties: MPropertySet,
}

impl MDeviceState {
    pub fn new(local_id: &str) -> Self {
        Self {
            object: MObjectState::new_detached(local_id)
                .expect("device local_id must be valid"),
            lifecycle: MDeviceLifecycle::Created,
            parent_bus: None,
            properties: MPropertySet::default(),
        }
    }

    pub fn object(&self) -> &MObjectState {
        &self.object
    }

    pub fn object_mut(&mut self) -> &mut MObjectState {
        &mut self.object
    }

    pub fn local_id(&self) -> &str {
        self.object.local_id()
    }

    pub fn lifecycle(&self) -> MDeviceLifecycle {
        self.lifecycle
    }

    pub fn is_realized(&self) -> bool {
        self.lifecycle == MDeviceLifecycle::Realized
    }

    pub fn set_parent_bus(&mut self, bus: &str) -> Result<(), MDeviceError> {
        if self.is_realized() {
            return Err(MDeviceError::LateMutation("parent_bus"));
        }
        self.parent_bus = Some(bus.to_string());
        Ok(())
    }

    pub fn parent_bus(&self) -> Option<&str> {
        self.parent_bus.as_deref()
    }

    pub fn define_property(
        &mut self,
        spec: MPropertySpec,
    ) -> Result<(), MDeviceError> {
        if self.is_realized() {
            return Err(MDeviceError::LateMutation("property_schema"));
        }
        self.properties.define(spec)
    }

    pub fn set_property(
        &mut self,
        name: &str,
        value: MPropertyValue,
    ) -> Result<(), MDeviceError> {
        self.properties.set(self.lifecycle, name, value)
    }

    pub fn property(&self, name: &str) -> Option<&MPropertyValue> {
        self.properties.get(name)
    }

    pub fn property_names(&self) -> Vec<&str> {
        self.properties.names()
    }

    pub fn validate_properties(&self) -> Result<(), MDeviceError> {
        self.properties.validate_required()
    }

    pub fn mark_realized(&mut self) -> Result<(), MDeviceError> {
        if self.is_realized() {
            return Err(MDeviceError::AlreadyRealized);
        }
        self.validate_properties()?;
        self.lifecycle = MDeviceLifecycle::Realized;
        Ok(())
    }

    pub fn mark_unrealized(&mut self) -> Result<(), MDeviceError> {
        if !self.is_realized() {
            return Err(MDeviceError::NotRealized);
        }
        self.lifecycle = MDeviceLifecycle::Created;
        Ok(())
    }
}

pub trait MDevice: MObject {
    fn mdevice_state(&self) -> &MDeviceState;
    fn mdevice_state_mut(&mut self) -> &mut MDeviceState;

    fn lifecycle(&self) -> MDeviceLifecycle {
        self.mdevice_state().lifecycle()
    }

    fn is_realized(&self) -> bool {
        self.mdevice_state().is_realized()
    }

    fn parent_bus(&self) -> Option<&str> {
        self.mdevice_state().parent_bus()
    }
}

impl MObject for MDeviceState {
    fn mobject_state(&self) -> &MObjectState {
        self.object()
    }

    fn mobject_state_mut(&mut self) -> &mut MObjectState {
        self.object_mut()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
