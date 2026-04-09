// Boot setup for the riscv64-ref machine.
//
// QEMU virt boot convention:
//   CPU starts at PC = 0x1000 (MROM base).
//   MROM contains a reset vector that sets:
//     a0 = mhartid
//     a1 = fdt_addr
//     a2 = &fw_dynamic_info
//   then jumps to start_addr (firmware or kernel entry).

use machina_core::address::GPA;
use machina_core::machine::Machine;
use machina_guest_riscv::riscv::csr::PrivLevel;
use machina_hw_core::loader;

use crate::ref_machine::{RefMachine, MROM_BASE, RAM_BASE};

/// Kernel is loaded 2 MiB above RAM_BASE.
pub const KERNEL_OFFSET: u64 = 0x20_0000;

/// Default embedded firmware (fw_dynamic.bin).
#[cfg(feature = "embed-firmware")]
const EMBEDDED_FW: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/rustsbi-riscv64-machina-fw_dynamic.bin"
));

#[cfg(not(feature = "embed-firmware"))]
const EMBEDDED_FW: &[u8] = &[];

/// OpenSBI-compatible DynamicInfo structure (RV64).
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
    pub fn new(next_addr: u64, next_mode: u64) -> Self {
        Self {
            magic: DYNAMIC_INFO_MAGIC,
            version: DYNAMIC_INFO_VERSION,
            next_addr,
            next_mode,
            options: 0,
            boot_hart: 0,
        }
    }

    pub fn to_bytes(&self) -> [u8; 48] {
        let mut buf = [0u8; 48];
        buf[0..8].copy_from_slice(&self.magic.to_le_bytes());
        buf[8..16].copy_from_slice(&self.version.to_le_bytes());
        buf[16..24].copy_from_slice(&self.next_addr.to_le_bytes());
        buf[24..32].copy_from_slice(&self.next_mode.to_le_bytes());
        buf[32..40].copy_from_slice(&self.options.to_le_bytes());
        buf[40..48].copy_from_slice(&self.boot_hart.to_le_bytes());
        buf
    }
}

const FW_FILENAME: &str = "rustsbi-riscv64-machina-fw_dynamic.bin";

/// Resolve the bios source: embedded, file, or none.
enum BiosSource {
    None,
    File(std::path::PathBuf),
    Embedded,
}

fn is_elf(data: &[u8]) -> bool {
    data.len() >= 4 && data[0..4] == [0x7f, b'E', b'L', b'F']
}

/// Search for firmware in standard data directories,
/// following QEMU's `qemu_find_file()` convention.
///
/// Search order:
///   1. $MACHINA_DATADIR/<name>
///   2. <exe_dir>/../pc-bios/<name>   (workspace dev)
///   3. <exe_dir>/../share/machina/<name> (FHS install)
///   4. /usr/share/machina/<name>
///   5. /usr/local/share/machina/<name>
fn find_firmware(name: &str) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    let mut dirs: Vec<PathBuf> = Vec::new();

    if let Ok(d) = std::env::var("MACHINA_DATADIR") {
        dirs.push(PathBuf::from(d));
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(bin) = exe.parent() {
            let base = bin.join("..");
            dirs.push(base.join("pc-bios"));
            dirs.push(base.join("share/machina"));
        }
    }

    dirs.push(PathBuf::from("/usr/share/machina"));
    dirs.push(PathBuf::from("/usr/local/share/machina"));

    for dir in &dirs {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn resolve_bios(bios_path: &Option<std::path::PathBuf>) -> BiosSource {
    match bios_path {
        Some(p) => {
            let s = p.to_str().unwrap_or("");
            if s == "none" {
                BiosSource::None
            } else {
                BiosSource::File(p.clone())
            }
        }
        None => BiosSource::Embedded,
    }
}

/// Write QEMU-compatible reset vector into MROM.
///
/// Layout at MROM_BASE (0x1000):
///   0x00: auipc  t0, %pcrel_hi(fw_dyn)   // 0x00000297
///   0x04: addi   a2, t0, %pcrel_lo(1b)    // 0x02828613
///   0x08: csrr   a0, mhartid              // 0xf1402573
///   0x0c: ld     a1, 32(t0)               // 0x0202b583
///   0x10: ld     t0, 24(t0)               // 0x0182b283
///   0x14: jr     t0                       // 0x00028067
///   0x18: .dword start_addr
///   0x20: .dword fdt_load_addr
///   0x28: fw_dynamic_info (48 bytes)
fn write_mrom(
    machine: &RefMachine,
    start_addr: u64,
    fdt_addr: u64,
    kernel_entry: u64,
    has_firmware: bool,
) {
    // RV64 reset vector (matches QEMU exactly).
    let reset_vec: [u32; 10] = [
        0x0000_0297, // auipc  t0, %pcrel_hi(fw_dyn)
        0x0282_8613, // addi   a2, t0, %pcrel_lo(1b)
        0xf140_2573, // csrr   a0, mhartid
        0x0202_b583, // ld     a1, 32(t0)
        0x0182_b283, // ld     t0, 24(t0)
        0x0002_8067, // jr     t0
        start_addr as u32,
        (start_addr >> 32) as u32,
        fdt_addr as u32,
        (fdt_addr >> 32) as u32,
    ];

    let mrom = machine.mrom_block();
    let ptr = mrom.as_ptr();

    // Write reset vector instructions + data.
    for (i, &word) in reset_vec.iter().enumerate() {
        let bytes = word.to_le_bytes();
        unsafe {
            let dst = ptr.add(i * 4);
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, 4);
        }
    }

    // Write fw_dynamic_info right after the reset vector
    // (offset 0x28 = 40 bytes).
    let next_mode = if has_firmware { 1u64 } else { 3u64 };
    let dinfo = DynamicInfo::new(kernel_entry, next_mode);
    let dinfo_bytes = dinfo.to_bytes();
    unsafe {
        let dst = ptr.add(40);
        std::ptr::copy_nonoverlapping(
            dinfo_bytes.as_ptr(),
            dst,
            dinfo_bytes.len(),
        );
    }
}

/// Boot the riscv64-ref machine.
///
/// Loads firmware/kernel, places FDT and reset vector,
/// sets CPU0 to start at MROM (PC = 0x1000).
pub fn boot_ref_machine(
    machine: &mut RefMachine,
) -> Result<(), Box<dyn std::error::Error>> {
    let bios_source = resolve_bios(&machine.bios_path);
    let has_firmware = !matches!(bios_source, BiosSource::None);

    // Load firmware.
    let mut fw_entry: Option<u64> = None;
    match bios_source {
        BiosSource::File(path) => {
            let data = std::fs::read(path)?;
            let as_ = machine.address_space();
            if is_elf(&data) {
                let info = loader::load_elf(&data, RAM_BASE, as_)
                    .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
                fw_entry = Some(info.entry.0);
            } else {
                loader::load_binary(&data, GPA::new(RAM_BASE), as_)
                    .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            }
        }
        BiosSource::Embedded => {
            if !EMBEDDED_FW.is_empty() {
                let as_ = machine.address_space();
                loader::load_binary(EMBEDDED_FW, GPA::new(RAM_BASE), as_)
                    .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            } else if let Some(path) = find_firmware(FW_FILENAME) {
                let data = std::fs::read(&path)?;
                let as_ = machine.address_space();
                if is_elf(&data) {
                    let info = loader::load_elf(&data, RAM_BASE, as_).map_err(
                        |e| -> Box<dyn std::error::Error> { e.into() },
                    )?;
                    fw_entry = Some(info.entry.0);
                } else {
                    loader::load_binary(&data, GPA::new(RAM_BASE), as_)
                        .map_err(|e| -> Box<dyn std::error::Error> {
                            e.into()
                        })?;
                }
            } else {
                return Err("no firmware found; use \
                     -bios <path>, set \
                     $MACHINA_DATADIR, or build \
                     with embed-firmware feature"
                    .into());
            }
        }
        BiosSource::None => {}
    }

    // Load kernel.
    let mut kernel_entry: Option<u64> = None;
    if let Some(ref kernel_path) = machine.kernel_path {
        let data = std::fs::read(kernel_path)?;
        let as_ = machine.address_space();
        let load_addr = if has_firmware {
            RAM_BASE + KERNEL_OFFSET
        } else {
            RAM_BASE
        };
        if is_elf(&data) {
            let info = loader::load_elf(&data, load_addr, as_)
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            kernel_entry = Some(info.entry.0);
        } else {
            loader::load_binary(&data, GPA::new(load_addr), as_)
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            kernel_entry = Some(load_addr);
        }
    }

    // Load initrd (if provided) after the kernel.
    let mut initrd_range: Option<(u64, u64)> = None;
    if let Some(ref initrd_path) = machine.initrd_path {
        let data = std::fs::read(initrd_path)?;
        // Place initrd 32 MiB after kernel start.
        let initrd_start = RAM_BASE + KERNEL_OFFSET + 0x200_0000;
        let initrd_end = initrd_start + data.len() as u64;
        let as_ = machine.address_space();
        loader::load_binary(&data, GPA::new(initrd_start), as_)
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
        initrd_range = Some((initrd_start, initrd_end));
    }

    // Regenerate FDT with initrd/bootargs info.
    let fdt = machine
        .generate_fdt_with(initrd_range, machine.kernel_cmdline.as_deref());

    // Place FDT at top of RAM, aligned to 8 bytes.
    let fdt_len = fdt.len() as u64;
    let ram_size = machine.ram_size();
    if fdt_len > ram_size {
        return Err("FDT blob larger than available RAM".into());
    }
    // Leave 64 KB margin at top of RAM for OpenSBI
    // scratch/workspace so it doesn't access beyond RAM.
    let margin = 0x10000u64; // 64 KB
    let fdt_offset = (ram_size - margin - fdt_len) & !0x7;
    let fdt_addr = RAM_BASE + fdt_offset;
    let as_ = machine.address_space();
    loader::load_binary(&fdt, GPA::new(fdt_addr), as_)
        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

    // Compute start_addr for reset vector jump target.
    let start_addr = if let Some(entry) = fw_entry {
        entry
    } else if has_firmware {
        RAM_BASE
    } else if let Some(entry) = kernel_entry {
        entry
    } else {
        RAM_BASE
    };

    // Write MROM: reset vector + fw_dynamic_info.
    let dinfo_final = kernel_entry.unwrap_or(RAM_BASE + KERNEL_OFFSET);
    write_mrom(machine, start_addr, fdt_addr, dinfo_final, has_firmware);

    // CPU0 starts at MROM base in M-mode.
    {
        let mut cpus = machine.cpus_lock();
        if let Some(Some(cpu)) = cpus.get_mut(0) {
            cpu.pc = MROM_BASE;
            cpu.set_priv(PrivLevel::Machine);
        }
    }

    Ok(())
}
