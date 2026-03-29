/// Machine model trait and configuration.
use std::path::PathBuf;

// TODO: use GPA in Machine trait methods (e.g. ram_base)
// use crate::address::GPA;

pub struct MachineOpts {
    pub ram_size: u64,
    pub cpu_count: u32,
    pub kernel: Option<PathBuf>,
    pub bios: Option<PathBuf>,
    pub append: Option<String>,
}

pub trait Machine: Send + Sync {
    fn name(&self) -> &str;
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
