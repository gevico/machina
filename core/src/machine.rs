/// Machine model trait and configuration.
use std::any::Any;
use std::path::PathBuf;

use crate::mobject::{MObject, MObjectState};

// TODO: use GPA in Machine trait methods (e.g. ram_base)
// use crate::address::GPA;

pub struct MachineOpts {
    pub ram_size: u64,
    pub cpu_count: u32,
    pub kernel: Option<PathBuf>,
    pub bios: Option<PathBuf>,
    /// Boot directly in post-firmware mode (e.g. S-mode on
    /// RISC-V) with firmware services provided by the host.
    pub bios_builtin: bool,
    pub append: Option<String>,
    pub nographic: bool,
    pub drive: Option<PathBuf>,
    pub initrd: Option<PathBuf>,
}

pub struct MachineState {
    object: MObjectState,
}

impl MachineState {
    pub fn new_root(local_id: &str) -> Self {
        Self {
            object: MObjectState::new_root(local_id)
                .expect("machine local_id must be valid"),
        }
    }

    pub fn object(&self) -> &MObjectState {
        &self.object
    }

    pub fn object_mut(&mut self) -> &mut MObjectState {
        &mut self.object
    }
}

impl MObject for MachineState {
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

pub trait Machine: Send + Sync {
    fn name(&self) -> &str;
    fn machine_state(&self) -> &MachineState;
    fn machine_state_mut(&mut self) -> &mut MachineState;
    fn init(
        &mut self,
        opts: &MachineOpts,
    ) -> Result<(), Box<dyn std::error::Error>>;
    fn reset(&mut self);
    fn pause(&mut self);
    fn resume(&mut self);
    fn shutdown(&mut self);
    fn boot(&mut self) -> Result<(), Box<dyn std::error::Error>>;
    fn cpu_count(&self) -> usize;
    fn ram_size(&self) -> u64;
}
