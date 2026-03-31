// Boot setup for the riscv64-ref machine.
//
// CPU boot convention (matches OpenSBI / QEMU virt):
//   a0 = hart_id
//   a1 = fdt_addr (guest physical)
//   a2 = dynamic_info_addr (for fw_dynamic firmware)
//   PC = entry_pc
//   privilege = Machine mode

use machina_core::address::GPA;
use machina_core::machine::Machine;
use machina_guest_riscv::riscv::csr::PrivLevel;
use machina_hw_core::loader;

use crate::ref_machine::{RefMachine, RAM_BASE};

/// Kernel is loaded 2 MiB above RAM_BASE.
pub const KERNEL_OFFSET: u64 = 0x20_0000;

/// Default embedded firmware (fw_dynamic.bin).
/// This is included at compile time from pc-bios/.
/// When pc-bios/ doesn't exist yet, use an empty slice.
#[cfg(feature = "embed-firmware")]
const EMBEDDED_FW: &[u8] = include_bytes!(
    "../../../pc-bios/rustsbi-riscv64-machina-fw_dynamic.bin"
);

#[cfg(not(feature = "embed-firmware"))]
const EMBEDDED_FW: &[u8] = &[];

/// OpenSBI-compatible DynamicInfo structure.
/// Passed to fw_dynamic firmware via a2 register.
#[repr(C)]
pub struct DynamicInfo {
    pub magic: u64,
    pub version: u64,
    pub next_addr: u64,
    pub next_mode: u64,
    pub options: u64,
    pub boot_hart: u64,
}

const DYNAMIC_INFO_MAGIC: u64 = 0x4942534f; // "OSBI"
const DYNAMIC_INFO_VERSION: u64 = 2;

impl DynamicInfo {
    pub fn new(next_addr: u64) -> Self {
        Self {
            magic: DYNAMIC_INFO_MAGIC,
            version: DYNAMIC_INFO_VERSION,
            next_addr,
            next_mode: 1, // S-mode
            options: 0,
            boot_hart: u64::MAX, // any hart
        }
    }

    pub fn to_bytes(&self) -> [u8; 48] {
        let mut buf = [0u8; 48];
        buf[0..8].copy_from_slice(
            &self.magic.to_le_bytes(),
        );
        buf[8..16].copy_from_slice(
            &self.version.to_le_bytes(),
        );
        buf[16..24].copy_from_slice(
            &self.next_addr.to_le_bytes(),
        );
        buf[24..32].copy_from_slice(
            &self.next_mode.to_le_bytes(),
        );
        buf[32..40].copy_from_slice(
            &self.options.to_le_bytes(),
        );
        buf[40..48].copy_from_slice(
            &self.boot_hart.to_le_bytes(),
        );
        buf
    }
}

/// Addresses and entry point produced by boot setup.
pub struct BootInfo {
    pub entry_pc: u64,
    pub fdt_addr: u64,
    pub hart_id: u32,
    pub dynamic_info_addr: u64,
}

/// Resolve the bios source: embedded, file, or none.
enum BiosSource<'a> {
    /// No firmware: bare-metal M-mode.
    None,
    /// Load from file path.
    File(&'a std::path::Path),
    /// Use embedded default firmware.
    Embedded,
}

fn resolve_bios(
    bios_path: &Option<std::path::PathBuf>,
) -> BiosSource<'_> {
    match bios_path {
        Some(p) => {
            let s = p.to_str().unwrap_or("");
            if s == "none" {
                BiosSource::None
            } else {
                BiosSource::File(p)
            }
        }
        // No -bios flag: use embedded firmware.
        None => BiosSource::Embedded,
    }
}

/// Real boot path for RefMachine: load bios/kernel,
/// place FDT and DynamicInfo, set CPU0 boot state.
pub fn boot_ref_machine(
    machine: &mut RefMachine,
) -> Result<(), Box<dyn std::error::Error>> {
    let bios_source = resolve_bios(&machine.bios_path);
    let has_firmware = !matches!(
        bios_source, BiosSource::None
    );

    // Load firmware at RAM_BASE.
    match bios_source {
        BiosSource::File(path) => {
            let data = std::fs::read(path)?;
            let as_ = machine.address_space();
            loader::load_binary(
                &data, GPA::new(RAM_BASE), as_,
            )
            .map_err(|e| -> Box<dyn std::error::Error> {
                e.into()
            })?;
        }
        BiosSource::Embedded => {
            if EMBEDDED_FW.is_empty() {
                eprintln!(
                    "machina: no embedded firmware \
                     (build with embed-firmware feature)"
                );
            } else {
                let as_ = machine.address_space();
                loader::load_binary(
                    EMBEDDED_FW,
                    GPA::new(RAM_BASE),
                    as_,
                )
                .map_err(
                    |e| -> Box<dyn std::error::Error> {
                        e.into()
                    },
                )?;
            }
        }
        BiosSource::None => {}
    }

    // Load kernel at RAM_BASE + KERNEL_OFFSET.
    if let Some(ref kernel_path) = machine.kernel_path {
        let data = std::fs::read(kernel_path)?;
        let as_ = machine.address_space();
        loader::load_binary(
            &data,
            GPA::new(RAM_BASE + KERNEL_OFFSET),
            as_,
        )
        .map_err(|e| -> Box<dyn std::error::Error> {
            e.into()
        })?;
    }

    // Place FDT at top of RAM, aligned to 8 bytes.
    let fdt = machine.fdt_blob().to_vec();
    let fdt_len = fdt.len() as u64;
    let ram_size = machine.ram_size();
    if fdt_len > ram_size {
        return Err(
            "FDT blob larger than available RAM".into()
        );
    }
    let fdt_offset = (ram_size - fdt_len) & !0x7;
    let fdt_addr = RAM_BASE + fdt_offset;
    let as_ = machine.address_space();
    loader::load_binary(&fdt, GPA::new(fdt_addr), as_)
        .map_err(|e| -> Box<dyn std::error::Error> {
            e.into()
        })?;

    // Place DynamicInfo before FDT (8-byte aligned).
    let mut dynamic_info_addr: u64 = 0;
    if has_firmware {
        let kernel_addr = if machine.kernel_path.is_some()
        {
            RAM_BASE + KERNEL_OFFSET
        } else {
            RAM_BASE + KERNEL_OFFSET // default
        };
        let info = DynamicInfo::new(kernel_addr);
        let info_bytes = info.to_bytes();
        let info_offset =
            (fdt_offset - info_bytes.len() as u64) & !0x7;
        dynamic_info_addr = RAM_BASE + info_offset;
        let as_ = machine.address_space();
        loader::load_binary(
            &info_bytes,
            GPA::new(dynamic_info_addr),
            as_,
        )
        .map_err(|e| -> Box<dyn std::error::Error> {
            e.into()
        })?;
    }

    // Set CPU0 boot state.
    {
        let mut cpus = machine.cpus_lock();
        if let Some(Some(cpu)) = cpus.get_mut(0) {
            cpu.gpr[10] = 0; // a0 = hart_id
            cpu.gpr[11] = fdt_addr; // a1 = fdt_addr
            cpu.gpr[12] = dynamic_info_addr; // a2
            if has_firmware {
                cpu.pc = RAM_BASE;
            } else if machine.kernel_path.is_some() {
                cpu.pc = RAM_BASE + KERNEL_OFFSET;
            } else {
                cpu.pc = RAM_BASE;
            }
            cpu.set_priv(PrivLevel::Machine);
        }
    }

    Ok(())
}
