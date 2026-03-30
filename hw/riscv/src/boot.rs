// Boot setup for the riscv64-ref machine.
//
// CPU boot convention (matches OpenSBI / QEMU virt):
//   a0 = hart_id
//   a1 = fdt_addr (guest physical)
//   PC = entry_pc
//   privilege = Machine mode

use machina_core::address::GPA;
use machina_core::machine::Machine;
use machina_hw_core::loader;

use crate::ref_machine::{RefMachine, RAM_BASE};

/// Kernel is loaded 2 MiB above RAM_BASE.
const KERNEL_OFFSET: u64 = 0x20_0000;

/// Addresses and entry point produced by boot setup.
pub struct BootInfo {
    pub entry_pc: u64,
    pub fdt_addr: u64,
    pub hart_id: u32,
}

/// Load bios/kernel into the address space and place the FDT
/// blob at the top of RAM.
///
/// `opts` provides optional `bios` / `kernel` paths.  When
/// paths are `None`, callers may pass raw data via the
/// `bios_data` / `kernel_data` slices instead (useful for
/// tests).
pub fn setup_boot(
    machine: &RefMachine,
    bios_data: Option<&[u8]>,
    kernel_data: Option<&[u8]>,
) -> Result<BootInfo, Box<dyn std::error::Error>> {
    let as_ = machine.address_space();

    // Load BIOS at RAM_BASE.
    if let Some(bios) = bios_data {
        loader::load_binary(bios, GPA::new(RAM_BASE), as_)
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    }

    // Load kernel at RAM_BASE + KERNEL_OFFSET.
    if let Some(kernel) = kernel_data {
        loader::load_binary(kernel, GPA::new(RAM_BASE + KERNEL_OFFSET), as_)
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    }

    // Place FDT at top of RAM, aligned down to 8 bytes.
    let fdt = machine.fdt_blob();
    let fdt_len = fdt.len() as u64;
    let ram_size = machine.ram_size();
    if fdt_len > ram_size {
        return Err("FDT blob larger than available RAM".into());
    }
    let fdt_offset = (ram_size - fdt_len) & !0x7;
    loader::load_binary(fdt, GPA::new(RAM_BASE + fdt_offset), as_)
        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

    Ok(BootInfo {
        entry_pc: RAM_BASE,
        fdt_addr: RAM_BASE + fdt_offset,
        hart_id: 0,
    })
}
