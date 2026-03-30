// Firmware / kernel loader utilities.

use machina_core::address::GPA;
use machina_memory::AddressSpace;

/// Information returned after a successful load.
pub struct LoadInfo {
    /// Entry point address.
    pub entry: GPA,
    /// Total bytes loaded.
    pub size: u64,
}

/// Load a raw binary image at the given guest physical address.
pub fn load_binary(
    data: &[u8],
    addr: GPA,
    as_: &AddressSpace,
) -> Result<LoadInfo, String> {
    let chunks = data.chunks(4);
    for (i, chunk) in chunks.enumerate() {
        let offset = (i * 4) as u64;
        let mut val = 0u32;
        for (j, &b) in chunk.iter().enumerate() {
            val |= (b as u32) << (j * 8);
        }
        as_.write_u32(GPA::new(addr.0 + offset), val);
    }
    Ok(LoadInfo {
        entry: addr,
        size: data.len() as u64,
    })
}

// ---- minimal ELF-64 constants ----

const EI_MAG: [u8; 4] = [0x7f, b'E', b'L', b'F'];
const ELFCLASS64: u8 = 2;
const ET_EXEC: u16 = 2;
const PT_LOAD: u32 = 1;

const ELF64_EHDR_SIZE: usize = 64;
const ELF64_PHDR_SIZE: usize = 56;

/// Load an ELF-64 binary into the address space and return
/// the entry point.
///
/// Only `PT_LOAD` segments are processed.  The binary must
/// be a little-endian ELF-64 executable (e.g. RISC-V).
pub fn load_elf(data: &[u8], as_: &AddressSpace) -> Result<LoadInfo, String> {
    if data.len() < ELF64_EHDR_SIZE {
        return Err("data too small for ELF header".into());
    }

    // e_ident[0..4] = magic
    if data[0..4] != EI_MAG {
        return Err("bad ELF magic".into());
    }
    // EI_CLASS
    if data[4] != ELFCLASS64 {
        return Err("not ELF-64".into());
    }

    let e_type = u16::from_le_bytes(data[16..18].try_into().unwrap());
    if e_type != ET_EXEC {
        return Err(format!("unsupported ELF type {e_type} (need ET_EXEC)"));
    }

    let e_entry = u64::from_le_bytes(data[24..32].try_into().unwrap());
    let e_phoff = u64::from_le_bytes(data[32..40].try_into().unwrap()) as usize;
    let e_phentsize =
        u16::from_le_bytes(data[42..44].try_into().unwrap()) as usize;
    let e_phnum = u16::from_le_bytes(data[44..46].try_into().unwrap()) as usize;

    if e_phentsize < ELF64_PHDR_SIZE {
        return Err(format!("phentsize {e_phentsize} < {ELF64_PHDR_SIZE}"));
    }

    let mut total_loaded: u64 = 0;

    for i in 0..e_phnum {
        let off = e_phoff + i * e_phentsize;
        if off + ELF64_PHDR_SIZE > data.len() {
            return Err("phdr out of bounds".into());
        }

        let p_type = u32::from_le_bytes(data[off..off + 4].try_into().unwrap());
        if p_type != PT_LOAD {
            continue;
        }

        let p_offset =
            u64::from_le_bytes(data[off + 8..off + 16].try_into().unwrap())
                as usize;
        let p_paddr =
            u64::from_le_bytes(data[off + 24..off + 32].try_into().unwrap());
        let p_filesz =
            u64::from_le_bytes(data[off + 32..off + 40].try_into().unwrap())
                as usize;
        let p_memsz =
            u64::from_le_bytes(data[off + 40..off + 48].try_into().unwrap());

        if p_offset + p_filesz > data.len() {
            return Err(format!(
                "PT_LOAD segment {i} file data \
                 out of bounds"
            ));
        }

        let seg = &data[p_offset..p_offset + p_filesz];
        for (j, chunk) in seg.chunks(4).enumerate() {
            let gpa_off = (j * 4) as u64;
            let mut val = 0u32;
            for (k, &b) in chunk.iter().enumerate() {
                val |= (b as u32) << (k * 8);
            }
            as_.write_u32(GPA::new(p_paddr + gpa_off), val);
        }

        // BSS: zero-fill [p_filesz .. p_memsz)
        let bss_start = p_paddr + p_filesz as u64;
        let bss_end = p_paddr + p_memsz;
        let mut cur = bss_start;
        while cur < bss_end {
            let remain = bss_end - cur;
            if remain >= 4 {
                as_.write_u32(GPA::new(cur), 0);
                cur += 4;
            } else {
                as_.write(GPA::new(cur), 1, 0);
                cur += 1;
            }
        }

        total_loaded += p_memsz;
    }

    Ok(LoadInfo {
        entry: GPA::new(e_entry),
        size: total_loaded,
    })
}
