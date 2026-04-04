// Bus-attached device interface.

use std::any::Any;
use std::fmt;

use machina_core::address::GPA;
use machina_core::mobject::{MObject, MObjectState};
use machina_memory::address_space::AddressSpace;
use machina_memory::region::MemoryRegion;

use crate::irq::IrqLine;
use crate::mdev::{MDevice, MDeviceError, MDeviceState};
use crate::qdev::Device;

/// A device that can be attached to a memory-mapped bus.
pub trait BusDevice: Device {
    fn read(&self, offset: u64, size: u32) -> u64;
    fn write(&mut self, offset: u64, size: u32, val: u64);
}

// -- SysBus --------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SysBusMapping {
    pub owner: String,
    pub name: String,
    pub base: GPA,
    pub size: u64,
}

struct RegisteredSysBusMapping {
    desc: SysBusMapping,
    region: Option<MemoryRegion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SysBusError {
    Device(MDeviceError),
    MissingParentBus,
    ParentBusMismatch { attached: String, expected: String },
    MissingMmio(String),
    MissingRealizedMapping(String),
    MmioOverlap { existing: String, requested: String },
}

impl fmt::Display for SysBusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Device(err) => write!(f, "{err}"),
            Self::MissingParentBus => {
                write!(f, "sysbus device must attach to a parent bus first")
            }
            Self::ParentBusMismatch { attached, expected } => {
                write!(
                    f,
                    "sysbus device is attached to '{attached}', expected '{expected}'"
                )
            }
            Self::MissingMmio(device) => {
                write!(f, "sysbus device '{device}' has no MMIO mappings")
            }
            Self::MissingRealizedMapping(name) => {
                write!(f, "sysbus realized mapping '{name}' is missing")
            }
            Self::MmioOverlap {
                existing,
                requested,
            } => {
                write!(
                    f,
                    "sysbus MMIO mapping '{requested}' overlaps existing '{existing}'"
                )
            }
        }
    }
}

impl std::error::Error for SysBusError {}

impl From<MDeviceError> for SysBusError {
    fn from(value: MDeviceError) -> Self {
        Self::Device(value)
    }
}

pub struct SysBusDeviceState {
    device: MDeviceState,
    mappings: Vec<RegisteredSysBusMapping>,
    irq_outputs: Vec<IrqLine>,
}

impl SysBusDeviceState {
    pub fn new(local_id: &str) -> Self {
        Self {
            device: MDeviceState::new(local_id),
            mappings: Vec::new(),
            irq_outputs: Vec::new(),
        }
    }

    pub fn device(&self) -> &MDeviceState {
        &self.device
    }

    pub fn device_mut(&mut self) -> &mut MDeviceState {
        &mut self.device
    }

    pub fn attach_to_bus(&mut self, bus: &SysBus) -> Result<(), SysBusError> {
        self.device.set_parent_bus(&bus.name)?;
        Ok(())
    }

    pub fn register_mmio(
        &mut self,
        region: MemoryRegion,
        base: GPA,
    ) -> Result<(), SysBusError> {
        if self.device.is_realized() {
            return Err(MDeviceError::LateMutation("sysbus_mmio").into());
        }

        let requested = SysBusMapping {
            owner: self.device.local_id().to_string(),
            name: region.name.clone(),
            base,
            size: region.size,
        };

        for existing in &self.mappings {
            if ranges_overlap(&existing.desc, &requested) {
                return Err(SysBusError::MmioOverlap {
                    existing: existing.desc.name.clone(),
                    requested: requested.name.clone(),
                });
            }
        }

        self.mappings.push(RegisteredSysBusMapping {
            desc: requested,
            region: Some(region),
        });
        Ok(())
    }

    pub fn register_irq(&mut self, irq: IrqLine) -> Result<(), SysBusError> {
        if self.device.is_realized() {
            return Err(MDeviceError::LateMutation("sysbus_irq").into());
        }
        self.irq_outputs.push(irq);
        Ok(())
    }

    pub fn mappings(&self) -> Vec<SysBusMapping> {
        self.mappings
            .iter()
            .map(|mapping| mapping.desc.clone())
            .collect()
    }

    pub fn irq_outputs(&self) -> &[IrqLine] {
        &self.irq_outputs
    }

    pub fn realize_onto(
        &mut self,
        bus: &mut SysBus,
        address_space: &mut AddressSpace,
    ) -> Result<(), SysBusError> {
        if self.mappings.is_empty() {
            return Err(SysBusError::MissingMmio(
                self.device.local_id().to_string(),
            ));
        }

        match self.device.parent_bus() {
            Some(attached) if attached == bus.name => {}
            Some(attached) => {
                return Err(SysBusError::ParentBusMismatch {
                    attached: attached.to_string(),
                    expected: bus.name.clone(),
                });
            }
            None => return Err(SysBusError::MissingParentBus),
        }

        for mapping in &self.mappings {
            bus.validate_mapping(&mapping.desc)?;
        }

        self.device.mark_realized()?;

        for mapping in &mut self.mappings {
            let desc = mapping.desc.clone();
            bus.record_mapping(desc);
            let region = mapping
                .region
                .take()
                .expect("sysbus MMIO region must exist before realize");
            address_space
                .root_mut()
                .add_subregion(region, mapping.desc.base);
        }

        address_space.update_flat_view();
        Ok(())
    }

    pub fn unrealize_from(
        &mut self,
        bus: &mut SysBus,
        address_space: &mut AddressSpace,
    ) -> Result<(), SysBusError> {
        if !self.device.is_realized() {
            return Err(MDeviceError::NotRealized.into());
        }

        for mapping in self.mappings.iter_mut().rev() {
            let region = address_space
                .remove_subregion(mapping.desc.base, &mapping.desc.name)
                .ok_or_else(|| {
                    SysBusError::MissingRealizedMapping(
                        mapping.desc.name.clone(),
                    )
                })?;
            mapping.region = Some(region);
            bus.remove_mapping(&mapping.desc).ok_or_else(|| {
                SysBusError::MissingRealizedMapping(mapping.desc.name.clone())
            })?;
        }

        self.device.mark_unrealized()?;
        Ok(())
    }
}

impl MObject for SysBusDeviceState {
    fn mobject_state(&self) -> &MObjectState {
        self.device.mobject_state()
    }

    fn mobject_state_mut(&mut self) -> &mut MObjectState {
        self.device.mobject_state_mut()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl MDevice for SysBusDeviceState {
    fn mdevice_state(&self) -> &MDeviceState {
        &self.device
    }

    fn mdevice_state_mut(&mut self) -> &mut MDeviceState {
        &mut self.device
    }
}

/// System bus — the default bus for platform devices.
pub struct SysBus {
    pub name: String,
    mappings: Vec<SysBusMapping>,
}

impl SysBus {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            mappings: Vec::new(),
        }
    }

    fn validate_mapping(
        &self,
        requested: &SysBusMapping,
    ) -> Result<(), SysBusError> {
        for existing in &self.mappings {
            if ranges_overlap(existing, requested) {
                return Err(SysBusError::MmioOverlap {
                    existing: existing.name.clone(),
                    requested: requested.name.clone(),
                });
            }
        }
        Ok(())
    }

    fn record_mapping(&mut self, mapping: SysBusMapping) {
        self.mappings.push(mapping);
    }

    fn remove_mapping(
        &mut self,
        target: &SysBusMapping,
    ) -> Option<SysBusMapping> {
        let index =
            self.mappings.iter().position(|mapping| mapping == target)?;
        Some(self.mappings.remove(index))
    }

    pub fn mappings(&self) -> &[SysBusMapping] {
        &self.mappings
    }
}

fn ranges_overlap(lhs: &SysBusMapping, rhs: &SysBusMapping) -> bool {
    let lhs_end = lhs.base.0 + lhs.size;
    let rhs_end = rhs.base.0 + rhs.size;
    lhs.base.0 < rhs_end && rhs.base.0 < lhs_end
}
