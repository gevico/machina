// Boot setup for the riscv64-ref machine.
//
// CPU boot convention (matches OpenSBI / QEMU virt):
//   a0 = hart_id
//   a1 = fdt_addr (guest physical)
//   PC = entry_pc
//   privilege = Machine mode

use machina_core::address::GPA;
use machina_core::machine::Machine;
use machina_guest_riscv::riscv::csr::PrivLevel;
use machina_hw_core::loader;

use crate::ref_machine::{RefMachine, RAM_BASE};

/// Kernel is loaded 2 MiB above RAM_BASE.
pub const KERNEL_OFFSET: u64 = 0x20_0000;

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

/// Real boot path for RefMachine: load bios/kernel from
/// stored file paths, place FDT, and set CPU0 boot state.
///
/// Called by `Machine::boot()`.
pub fn boot_ref_machine(
    machine: &mut RefMachine,
) -> Result<(), Box<dyn std::error::Error>> {
    let as_ = machine.address_space();

    // Load BIOS at RAM_BASE.
    if let Some(ref bios_path) = machine.bios_path {
        let data = std::fs::read(bios_path)?;
        loader::load_binary(&data, GPA::new(RAM_BASE), as_)
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    }

    // Load kernel at RAM_BASE + KERNEL_OFFSET.
    if let Some(ref kernel_path) = machine.kernel_path {
        let data = std::fs::read(kernel_path)?;
        loader::load_binary(&data, GPA::new(RAM_BASE + KERNEL_OFFSET), as_)
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    }

    // Place FDT at top of RAM, aligned to 8 bytes.
    let fdt = machine.fdt_blob().to_vec();
    let fdt_len = fdt.len() as u64;
    let ram_size = machine.ram_size();
    if fdt_len > ram_size {
        return Err("FDT blob larger than available RAM".into());
    }
    let fdt_offset = (ram_size - fdt_len) & !0x7;
    let fdt_addr = RAM_BASE + fdt_offset;

    let as_ = machine.address_space();
    loader::load_binary(&fdt, GPA::new(fdt_addr), as_)
        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

    // Set CPU0 boot state.
    {
        let mut cpus = machine.cpus_lock();
        if let Some(cpu) = cpus.get_mut(0) {
            cpu.gpr[10] = 0; // a0 = hart_id
            cpu.gpr[11] = fdt_addr; // a1 = fdt_addr
            cpu.pc = RAM_BASE; // entry point
            cpu.set_priv(PrivLevel::Machine);
        }
    }

    Ok(())
}
