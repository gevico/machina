use std::sync::Arc;

use machina_core::address::GPA;

use crate::ram::RamBlock;

// ----- MMIO callback trait -----

/// Device-model I/O callbacks for MMIO regions.
///
/// Implementors should use interior mutability (e.g. `Mutex`)
/// for any mutable state so that `write` can take `&self`,
/// matching the shared-ownership model of the memory tree.
pub trait MmioOps: Send + Sync {
    fn read(&self, offset: u64, size: u32) -> u64;

    fn write(&self, offset: u64, size: u32, val: u64);
}

// ----- Region type discriminant -----

pub enum RegionType {
    Ram {
        block: Arc<RamBlock>,
    },
    Rom {
        block: Arc<RamBlock>,
    },
    Io {
        ops: Arc<dyn MmioOps>,
    },
    Alias {
        target: Box<MemoryRegion>,
        offset: u64,
    },
    Container,
}

// ----- SubRegion (child placed at an offset in the parent) --

pub struct SubRegion {
    pub region: MemoryRegion,
    pub offset: GPA,
}

// ----- MemoryRegion tree node -----

pub struct MemoryRegion {
    pub name: String,
    pub size: u64,
    pub region_type: RegionType,
    pub priority: i32,
    pub subregions: Vec<SubRegion>,
    pub enabled: bool,
}

impl MemoryRegion {
    /// Create a pure container (no backing storage).
    pub fn container(name: &str, size: u64) -> Self {
        Self {
            name: name.to_string(),
            size,
            region_type: RegionType::Container,
            priority: 0,
            subregions: Vec::new(),
            enabled: true,
        }
    }

    /// Create a RAM-backed region and return the shared
    /// `RamBlock` handle alongside it.
    pub fn ram(name: &str, size: u64) -> (Self, Arc<RamBlock>) {
        let block = Arc::new(RamBlock::new(size));
        let region = Self {
            name: name.to_string(),
            size,
            region_type: RegionType::Ram {
                block: Arc::clone(&block),
            },
            priority: 0,
            subregions: Vec::new(),
            enabled: true,
        };
        (region, block)
    }

    /// Create a read-only ROM region and return the shared
    /// `RamBlock` handle alongside it.  Writes to ROM are
    /// silently dropped.
    pub fn rom(name: &str, size: u64) -> (Self, Arc<RamBlock>) {
        let block = Arc::new(RamBlock::new(size));
        let region = Self {
            name: name.to_string(),
            size,
            region_type: RegionType::Rom {
                block: Arc::clone(&block),
            },
            priority: 0,
            subregions: Vec::new(),
            enabled: true,
        };
        (region, block)
    }

    /// Create an MMIO region backed by device callbacks.
    pub fn io(name: &str, size: u64, ops: Arc<dyn MmioOps>) -> Self {
        Self {
            name: name.to_string(),
            size,
            region_type: RegionType::Io { ops },
            priority: 0,
            subregions: Vec::new(),
            enabled: true,
        }
    }

    /// Create an alias window into `target` starting at byte
    /// `offset` within the target region.
    pub fn alias(
        name: &str,
        target: MemoryRegion,
        offset: u64,
        size: u64,
    ) -> Self {
        Self {
            name: name.to_string(),
            size,
            region_type: RegionType::Alias {
                target: Box::new(target),
                offset,
            },
            priority: 0,
            subregions: Vec::new(),
            enabled: true,
        }
    }

    /// Add a child region at `offset` within this region's
    /// address space, using the child's existing priority.
    pub fn add_subregion(&mut self, region: MemoryRegion, offset: GPA) {
        self.subregions.push(SubRegion { region, offset });
    }

    /// Add a child region at `offset`, overriding its priority.
    pub fn add_subregion_with_priority(
        &mut self,
        mut region: MemoryRegion,
        offset: GPA,
        priority: i32,
    ) {
        region.priority = priority;
        self.subregions.push(SubRegion { region, offset });
    }

    /// Remove the first child region matching both `offset`
    /// and `name`, returning the detached region when found.
    pub fn remove_subregion(
        &mut self,
        offset: GPA,
        name: &str,
    ) -> Option<MemoryRegion> {
        let index = self.subregions.iter().position(|subregion| {
            subregion.offset == offset && subregion.region.name == name
        })?;
        Some(self.subregions.remove(index).region)
    }
}
