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
    write_bytes(as_, addr, data);
    Ok(LoadInfo {
        entry: addr,
        size: data.len() as u64,
    })
}

/// Write `data` into `as_` starting at `base`, using 4-byte
/// writes for aligned chunks and single-byte writes for the
/// trailing remainder so that no bytes beyond `data.len()`
/// are overwritten.
fn write_bytes(as_: &AddressSpace, base: GPA, data: &[u8]) {
    let full = data.len() / 4;
    for i in 0..full {
        let off = (i * 4) as u64;
        let val =
            u32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
        as_.write_u32(GPA::new(base.0 + off), val);
    }
    let rem_start = full * 4;
    for (j, &b) in data[rem_start..].iter().enumerate() {
        let off = (rem_start + j) as u64;
        as_.write(GPA::new(base.0 + off), 1, b as u64);
    }
}

// ---- minimal ELF-64 constants ----

const EI_MAG: [u8; 4] = [0x7f, b'E', b'L', b'F'];
const ELFCLASS64: u8 = 2;
const ET_EXEC: u16 = 2;
const PT_LOAD: u32 = 1;
const SHT_SYMTAB: u32 = 2;

const ELF64_EHDR_SIZE: usize = 64;
const ELF64_PHDR_SIZE: usize = 56;
const ELF64_SHDR_SIZE: usize = 64;
const ELF64_SYM_SIZE: usize = 24;

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
    // e_shoff(40..48), e_flags(48..52), e_ehsize(52..54)
    let e_phentsize =
        u16::from_le_bytes(data[54..56].try_into().unwrap()) as usize;
    let e_phnum = u16::from_le_bytes(data[56..58].try_into().unwrap()) as usize;

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
        write_bytes(as_, GPA::new(p_paddr), seg);

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

/// Find a named symbol in an ELF-64 binary and return its
/// value (address).  Returns `None` if the symbol is not
/// found or the ELF lacks a symbol table.
pub fn elf_find_symbol(data: &[u8], name: &str) -> Option<u64> {
    if data.len() < ELF64_EHDR_SIZE
        || data[0..4] != EI_MAG
        || data[4] != ELFCLASS64
    {
        return None;
    }

    let e_shoff = u64::from_le_bytes(data[40..48].try_into().unwrap()) as usize;
    let e_shentsize =
        u16::from_le_bytes(data[58..60].try_into().unwrap()) as usize;
    let e_shnum = u16::from_le_bytes(data[60..62].try_into().unwrap()) as usize;

    if e_shentsize < ELF64_SHDR_SIZE {
        return None;
    }

    // Walk section headers to find SHT_SYMTAB.
    for i in 0..e_shnum {
        let sh = e_shoff + i * e_shentsize;
        if sh + ELF64_SHDR_SIZE > data.len() {
            break;
        }

        let sh_type =
            u32::from_le_bytes(data[sh + 4..sh + 8].try_into().unwrap());
        if sh_type != SHT_SYMTAB {
            continue;
        }

        // ELF64_Shdr layout:
        //   0: sh_name(4), 4: sh_type(4),
        //   8: sh_flags(8), 16: sh_addr(8),
        //  24: sh_offset(8), 32: sh_size(8),
        //  40: sh_link(4), 44: sh_info(4),
        //  48: sh_addralign(8), 56: sh_entsize(8)
        let sym_offset =
            u64::from_le_bytes(data[sh + 24..sh + 32].try_into().unwrap())
                as usize;
        let sym_size =
            u64::from_le_bytes(data[sh + 32..sh + 40].try_into().unwrap())
                as usize;
        let strtab_idx =
            u32::from_le_bytes(data[sh + 40..sh + 44].try_into().unwrap())
                as usize;
        let sym_entsize =
            u64::from_le_bytes(data[sh + 56..sh + 64].try_into().unwrap())
                as usize;
        let ent = if sym_entsize >= ELF64_SYM_SIZE {
            sym_entsize
        } else {
            ELF64_SYM_SIZE
        };

        // Locate the string table section.
        let str_sh = e_shoff + strtab_idx * e_shentsize;
        if str_sh + ELF64_SHDR_SIZE > data.len() {
            return None;
        }
        let str_off = u64::from_le_bytes(
            data[str_sh + 24..str_sh + 32].try_into().unwrap(),
        ) as usize;
        let str_size = u64::from_le_bytes(
            data[str_sh + 32..str_sh + 40].try_into().unwrap(),
        ) as usize;

        // Iterate symbols.
        let nsym = sym_size / ent;
        for j in 0..nsym {
            let s = sym_offset + j * ent;
            if s + ELF64_SYM_SIZE > data.len() {
                break;
            }
            // Elf64_Sym: st_name(4), st_info(1),
            //   st_other(1), st_shndx(2), st_value(8),
            //   st_size(8)
            let st_name =
                u32::from_le_bytes(data[s..s + 4].try_into().unwrap()) as usize;
            let st_value =
                u64::from_le_bytes(data[s + 8..s + 16].try_into().unwrap());

            // Resolve name from strtab.
            let name_start = str_off + st_name;
            if name_start >= str_off + str_size {
                continue;
            }
            let name_end = data[name_start..]
                .iter()
                .position(|&b| b == 0)
                .map(|p| name_start + p)
                .unwrap_or(data.len());
            let sym_name =
                std::str::from_utf8(&data[name_start..name_end]).unwrap_or("");
            if sym_name == name {
                return Some(st_value);
            }
        }
    }

    None
}
