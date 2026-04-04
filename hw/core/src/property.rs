use std::collections::BTreeMap;
use std::fmt;

use crate::mdev::{MDeviceError, MDeviceLifecycle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MPropertyType {
    Bool,
    U32,
    U64,
    String,
    Link,
}

impl fmt::Display for MPropertyType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool => write!(f, "bool"),
            Self::U32 => write!(f, "u32"),
            Self::U64 => write!(f, "u64"),
            Self::String => write!(f, "string"),
            Self::Link => write!(f, "link"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MPropertyMutability {
    Static,
    Dynamic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MPropertyValue {
    Bool(bool),
    U32(u32),
    U64(u64),
    String(String),
    Link(String),
}

impl MPropertyValue {
    pub fn property_type(&self) -> MPropertyType {
        match self {
            Self::Bool(_) => MPropertyType::Bool,
            Self::U32(_) => MPropertyType::U32,
            Self::U64(_) => MPropertyType::U64,
            Self::String(_) => MPropertyType::String,
            Self::Link(_) => MPropertyType::Link,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MPropertySpec {
    pub name: String,
    pub property_type: MPropertyType,
    pub mutability: MPropertyMutability,
    pub required: bool,
    pub default_value: Option<MPropertyValue>,
}

impl MPropertySpec {
    pub fn new(name: &str, property_type: MPropertyType) -> Self {
        Self {
            name: name.to_string(),
            property_type,
            mutability: MPropertyMutability::Static,
            required: false,
            default_value: None,
        }
    }

    pub fn dynamic(mut self) -> Self {
        self.mutability = MPropertyMutability::Dynamic;
        self
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn default(mut self, value: MPropertyValue) -> Self {
        self.default_value = Some(value);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MPropertyEntry {
    spec: MPropertySpec,
    value: Option<MPropertyValue>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MPropertySet {
    entries: BTreeMap<String, MPropertyEntry>,
}

impl MPropertySet {
    pub fn define(&mut self, spec: MPropertySpec) -> Result<(), MDeviceError> {
        if let Some(default_value) = &spec.default_value {
            Self::validate_type(&spec.name, spec.property_type, default_value)?;
        }

        if self.entries.contains_key(&spec.name) {
            return Err(MDeviceError::DuplicateProperty(spec.name.clone()));
        }

        let name = spec.name.clone();
        let value = spec.default_value.clone();
        self.entries.insert(name, MPropertyEntry { spec, value });
        Ok(())
    }

    pub fn set(
        &mut self,
        lifecycle: MDeviceLifecycle,
        name: &str,
        value: MPropertyValue,
    ) -> Result<(), MDeviceError> {
        let entry = self
            .entries
            .get_mut(name)
            .ok_or_else(|| MDeviceError::UnknownProperty(name.to_string()))?;
        Self::validate_type(name, entry.spec.property_type, &value)?;

        if lifecycle == MDeviceLifecycle::Realized
            && entry.spec.mutability == MPropertyMutability::Static
        {
            return Err(MDeviceError::LateMutation("property"));
        }

        entry.value = Some(value);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&MPropertyValue> {
        self.entries
            .get(name)
            .and_then(|entry| entry.value.as_ref())
    }

    pub fn spec(&self, name: &str) -> Option<&MPropertySpec> {
        self.entries.get(name).map(|entry| &entry.spec)
    }

    pub fn validate_required(&self) -> Result<(), MDeviceError> {
        for entry in self.entries.values() {
            if entry.spec.required && entry.value.is_none() {
                return Err(MDeviceError::MissingRequiredProperty(
                    entry.spec.name.clone(),
                ));
            }
        }
        Ok(())
    }

    pub fn names(&self) -> Vec<&str> {
        self.entries.keys().map(String::as_str).collect()
    }

    fn validate_type(
        name: &str,
        expected: MPropertyType,
        value: &MPropertyValue,
    ) -> Result<(), MDeviceError> {
        let actual = value.property_type();
        if actual != expected {
            return Err(MDeviceError::PropertyTypeMismatch {
                name: name.to_string(),
                expected,
                actual,
            });
        }
        Ok(())
    }
}
