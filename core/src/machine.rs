/// Machine model trait and configuration.
use std::path::PathBuf;

use crate::mobject::MObjectState;

// TODO: use GPA in Machine trait methods (e.g. ram_base)
// use crate::address::GPA;

/// Parsed `-netdev` + `-device virtio-net-device` combo.
#[derive(Clone, Debug)]
pub struct NetdevOpts {
    pub id: String,
    pub ifname: String,
    pub mac: Option<String>,
}

pub struct MachineOpts {
    pub ram_size: u64,
    pub cpu_count: u32,
    pub kernel: Option<PathBuf>,
    pub bios: Option<PathBuf>,
    pub append: Option<String>,
    pub initrd: Option<PathBuf>,
    pub nographic: bool,
    pub drive: Option<PathBuf>,
    pub netdev: Option<NetdevOpts>,
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
