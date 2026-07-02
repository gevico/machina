use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::read::GzDecoder;
use machina_core::address::GPA;
use machina_hw_core::fdt::FdtBuilder;
use machina_hw_core::loader;
use machina_hw_firmware::{keys, FwCfg};
use machina_memory::AddressSpace;

use crate::interrupt::{
    LOONGARCH_RTC_PCH_IRQ, LOONGARCH_UART_PCH_IRQ,
    LOONGARCH_VIRTIO_PCH_IRQ_BASE,
};
use crate::virt_machine::{
    VIRT_EIOINTC_BASE, VIRT_EIOINTC_SIZE, VIRT_FLASH0_BASE, VIRT_FLASH0_SIZE,
    VIRT_FLASH1_BASE, VIRT_FLASH1_SIZE, VIRT_FWCFG_BASE, VIRT_FWCFG_SIZE,
    VIRT_IPI_BASE, VIRT_IPI_SIZE, VIRT_LEGACY_IO_BASE, VIRT_LEGACY_IO_SIZE,
    VIRT_LEGACY_IPI_BASE, VIRT_LEGACY_IPI_STRIDE, VIRT_PCH_MSI_BASE,
    VIRT_PCH_MSI_SIZE, VIRT_PCH_PIC_BASE, VIRT_PCH_PIC_SIZE, VIRT_PCI_CFG_BASE,
    VIRT_PCI_CFG_SIZE, VIRT_PCI_HT_CFG_BASE, VIRT_RAM_BASE, VIRT_RTC_BASE,
    VIRT_RTC_SIZE, VIRT_TEST_FINISHER_BASE, VIRT_TEST_FINISHER_SIZE,
    VIRT_UART1_BASE, VIRT_UART1_SIZE, VIRT_UART_BASE, VIRT_UART_SIZE,
    VIRT_VIRTIO_BASE, VIRT_VIRTIO_SIZE,
};

pub const KERNEL_ENTRY_DEFAULT: u64 = 0x9000_0000_0020_0000;
pub const SECONDARY_BOOT_ENTRY: u64 = VIRT_RAM_BASE + 0x1000;

type BootResult<T> = Result<T, Box<dyn std::error::Error>>;

const LOW_PHYS_MASK: u64 = 0x0fff_ffff_ffff_ffff;
const LINUX_IMAGE_HEADER_SIZE: usize = 64;
const LINUX_IMAGE_MZ_MAGIC: u16 = 0x5a4d;
const LINUX_IMAGE_PE_MAGIC: u32 = 0x8182_23cd;
const EFI_ZBOOT_HEADER_SIZE: usize = 64;
const EFI_ZBOOT_MAX_DECOMPRESSED_SIZE: u64 = 256 * 1024 * 1024;
const BOOT_PARAM_WINDOW_SIZE: u64 = 0x2_0000;
const BOOT_FDT_OFFSET: u64 = 0x2000;
const BOOT_SYSTEM_TABLE_OFFSET: u64 = 0x1_0000;
const BOOT_CONFIG_TABLE_OFFSET: u64 = 0x1_1000;
const BOOT_MEMMAP_OFFSET: u64 = 0x1_2000;
const BOOT_INITRD_TABLE_OFFSET: u64 = 0x1_3000;
const COMMAND_LINE_SIZE: usize = 4096;
const BOOT_DATA_ALIGN: u64 = 0x1000;
const EFI_BOOT_MODE: u64 = 1;
const EFI_SYSTEM_TABLE_SIGNATURE: u64 = 0x5453_5953_2049_4249;
const EFI_SPECIFICATION_VERSION: u32 = (2 << 16) | 100;
const EFI_SYSTEM_TABLE_SIZE: usize = 120;
const EFI_CONFIG_TABLE_SIZE: usize = 24;
const EFI_BOOT_MEMMAP_SIZE: usize = 40;
const EFI_MEMORY_DESC_SIZE: usize = 40;
const EFI_MEMORY_DESC_VERSION: u32 = 1;
const EFI_RESERVED_MEMORY: u32 = 0;
const EFI_CONVENTIONAL_MEMORY: u32 = 7;
const EFI_PAGE_SIZE: u64 = 4096;
const BOOT_RNG_SEED_SIZE: usize = 32;
const ELF64_EHDR_SIZE: usize = 64;
const ELF64_PHDR_SIZE: usize = 56;
const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const PT_LOAD: u32 = 1;

const SECONDARY_BOOT_CODE: [u32; 30] = [
    0x0400_302c,
    0x0380_100c,
    0x0400_0180,
    0x1400_002d,
    0x0380_81ad,
    0x0648_1da0,
    0x1400_002c,
    0x0400_118c,
    0x02ff_fc0c,
    0x1400_002d,
    0x0380_11ad,
    0x0648_19ac,
    0x1400_002d,
    0x0380_81ad,
    0x0648_8000,
    0x0340_0000,
    0x0648_09ac,
    0x43ff_f59f,
    0x1400_002d,
    0x0648_09ac,
    0x1400_002d,
    0x0380_31ad,
    0x0648_19ac,
    0x1400_002c,
    0x0400_1180,
    0x1400_002d,
    0x0380_81ad,
    0x0648_0dac,
    0x0015_0181,
    0x4c00_0020,
];
const SECONDARY_BOOT_CODE_SIZE: u64 =
    (SECONDARY_BOOT_CODE.len() * std::mem::size_of::<u32>()) as u64;

const DEVICE_TREE_GUID: [u8; 16] = [
    0xd5, 0x21, 0xb6, 0xb1, 0x9c, 0xf1, 0xa5, 0x41, 0x83, 0x0b, 0xd9, 0x15,
    0x2c, 0x69, 0xaa, 0xe0,
];
const LINUX_EFI_BOOT_MEMMAP_GUID: [u8; 16] = [
    0x3f, 0x68, 0x0f, 0x80, 0x8b, 0xd0, 0x3a, 0x42, 0xa2, 0x93, 0x96, 0x5c,
    0x3c, 0x6f, 0xe2, 0xb4,
];
const LINUX_EFI_INITRD_MEDIA_GUID: [u8; 16] = [
    0x27, 0xe4, 0x68, 0x55, 0xfc, 0x68, 0x3d, 0x4f, 0xac, 0x74, 0xca, 0x55,
    0x52, 0x31, 0xcc, 0x68,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectKernelBoot {
    pub entry: u64,
    pub efi_boot: u64,
    pub cmdline_addr: u64,
    pub system_table_addr: u64,
}

pub struct DirectKernelBootConfig<'a> {
    pub cmdline: Option<&'a str>,
    pub initrd_path: Option<&'a Path>,
    pub cpu_count: u32,
    pub has_virtio_mmio: bool,
    pub fw_cfg: Option<Arc<FwCfg>>,
}

#[derive(Debug, Clone, Copy)]
struct GuestRange {
    start: u64,
    end: u64,
    label: &'static str,
}

impl GuestRange {
    fn in_ram(
        start: u64,
        len: u64,
        ram_size: u64,
        label: &'static str,
    ) -> BootResult<Self> {
        let end = start.checked_add(len).ok_or_else(|| {
            format!("LoongArch {label} range overflows address space")
        })?;
        ensure_ram_range(start, len, ram_size, label)?;
        Ok(Self { start, end, label })
    }

    fn overlaps(self, other: Self) -> bool {
        self.start < other.end && other.start < self.end
    }
}

#[derive(Debug, Clone, Copy)]
enum KernelLoadKind {
    Elf,
    LinuxImage { load_addr: u64 },
    Raw,
}

#[derive(Debug)]
struct KernelLoadPlan {
    entry: u64,
    ranges: Vec<GuestRange>,
    kind: KernelLoadKind,
}

#[derive(Debug)]
struct InitrdPlan {
    data: Vec<u8>,
    range: GuestRange,
    len: u64,
}

fn is_elf(data: &[u8]) -> bool {
    data.len() >= 4 && data[0..4] == [0x7f, b'E', b'L', b'F']
}

fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap())
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

fn read_u64(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
}

fn ram_end(ram_size: u64) -> BootResult<u64> {
    VIRT_RAM_BASE
        .checked_add(ram_size)
        .ok_or_else(|| "LoongArch RAM size overflows address space".into())
}

fn boot_phys_addr(guest_addr: u64) -> u64 {
    guest_addr & LOW_PHYS_MASK
}

fn ensure_ram_range(
    addr: u64,
    len: u64,
    ram_size: u64,
    label: &str,
) -> BootResult<()> {
    let end = addr.checked_add(len).ok_or_else(|| {
        format!("LoongArch {label} range overflows address space")
    })?;
    let ram_end = ram_end(ram_size)?;
    if addr < VIRT_RAM_BASE || end > ram_end {
        return Err(format!(
            "LoongArch {label} range {addr:#x}..{end:#x} is outside \
             LoongArch RAM {:#x}..{ram_end:#x}",
            VIRT_RAM_BASE
        )
        .into());
    }
    Ok(())
}

fn load_binary_checked(
    data: &[u8],
    addr: u64,
    ram_size: u64,
    address_space: &AddressSpace,
    label: &str,
) -> BootResult<()> {
    ensure_ram_range(addr, data.len() as u64, ram_size, label)?;
    loader::load_binary(data, GPA::new(addr), address_space)?;
    Ok(())
}

fn normalize_ram_addr(
    value: u64,
    ram_size: u64,
    label: &str,
) -> BootResult<u64> {
    let end = ram_end(ram_size)?;
    if (VIRT_RAM_BASE..end).contains(&value) {
        return Ok(value);
    }

    let low = value & LOW_PHYS_MASK;
    let addr = VIRT_RAM_BASE.checked_add(low).ok_or_else(|| {
        format!("LoongArch {label} canonical address overflows")
    })?;
    ensure_ram_range(addr, 1, ram_size, label)?;
    Ok(addr)
}

pub fn install_secondary_boot_code(
    ram_size: u64,
    address_space: &AddressSpace,
) -> BootResult<u64> {
    ensure_ram_range(
        SECONDARY_BOOT_ENTRY,
        SECONDARY_BOOT_CODE_SIZE,
        ram_size,
        "secondary boot",
    )?;
    for (index, insn) in SECONDARY_BOOT_CODE.iter().enumerate() {
        address_space.write(
            GPA::new(SECONDARY_BOOT_ENTRY + (index as u64 * 4)),
            4,
            u64::from(*insn),
        );
    }
    Ok(SECONDARY_BOOT_ENTRY)
}

#[derive(Debug, Clone, Copy)]
struct LinuxImageHeader {
    kernel_entry: u64,
    kernel_size: u64,
    load_offset: u64,
}

fn parse_linux_image_header(
    data: &[u8],
) -> BootResult<Option<LinuxImageHeader>> {
    if data.len() < 2 || read_u16(data, 0) != LINUX_IMAGE_MZ_MAGIC {
        return Ok(None);
    }
    if data.len() < LINUX_IMAGE_HEADER_SIZE {
        return Err(
            "LoongArch Linux Image header is smaller than 64 bytes".into()
        );
    }
    let pe_magic = read_u32(data, 56);
    if pe_magic != LINUX_IMAGE_PE_MAGIC {
        return Err(format!(
            "LoongArch Linux Image PE magic {pe_magic:#x} does not match \
             {LINUX_IMAGE_PE_MAGIC:#x}"
        )
        .into());
    }

    Ok(Some(LinuxImageHeader {
        kernel_entry: read_u64(data, 8),
        kernel_size: read_u64(data, 16),
        load_offset: read_u64(data, 24),
    }))
}

fn unpack_efi_zboot_image(data: &[u8]) -> BootResult<Option<Vec<u8>>> {
    if data.len() < EFI_ZBOOT_HEADER_SIZE {
        return Ok(None);
    }
    let is_zboot = &data[0..2] == b"MZ"
        && &data[4..8] == b"zimg"
        && read_u32(data, 56) == LINUX_IMAGE_PE_MAGIC;
    if !is_zboot {
        return Ok(None);
    }

    let payload_offset = read_u32(data, 8) as usize;
    let payload_size = read_u32(data, 12) as usize;
    let payload_end = payload_offset
        .checked_add(payload_size)
        .ok_or("LoongArch EFI zboot compressed payload range overflows")?;
    if payload_end > data.len() {
        return Err(
            "LoongArch EFI zboot compressed payload is out of bounds".into()
        );
    }

    let compression =
        data[24..56].split(|byte| *byte == 0).next().unwrap_or(&[]);
    if compression != b"gzip" {
        return Err(format!(
            "LoongArch EFI zboot compression '{}' is unsupported",
            String::from_utf8_lossy(compression)
        )
        .into());
    }

    let payload = &data[payload_offset..payload_end];
    let mut decoder = GzDecoder::new(payload);
    let mut image = Vec::new();
    decoder
        .by_ref()
        .take(EFI_ZBOOT_MAX_DECOMPRESSED_SIZE + 1)
        .read_to_end(&mut image)?;
    if image.len() as u64 > EFI_ZBOOT_MAX_DECOMPRESSED_SIZE {
        return Err(format!(
            "LoongArch EFI zboot decompressed image exceeds {} bytes",
            EFI_ZBOOT_MAX_DECOMPRESSED_SIZE
        )
        .into());
    }
    Ok(Some(image))
}

fn analyze_elf_kernel(
    data: &[u8],
    base_addr: u64,
    ram_size: u64,
) -> BootResult<KernelLoadPlan> {
    if data.len() < ELF64_EHDR_SIZE {
        return Err("ELF header is truncated".into());
    }
    if data[0..4] != [0x7f, b'E', b'L', b'F'] {
        return Err("bad ELF magic".into());
    }
    if data[4] != 2 {
        return Err("LoongArch direct boot only supports ELF-64 kernels".into());
    }

    let e_type = read_u16(data, 16);
    if e_type != ET_EXEC && e_type != ET_DYN {
        return Err(format!("unsupported ELF type {e_type}").into());
    }
    let is_dyn = e_type == ET_DYN;
    let e_entry = read_u64(data, 24);
    let e_phoff = read_u64(data, 32) as usize;
    let e_phentsize = read_u16(data, 54) as usize;
    let e_phnum = read_u16(data, 56) as usize;
    if e_phentsize < ELF64_PHDR_SIZE {
        return Err(
            format!("phentsize {e_phentsize} < {ELF64_PHDR_SIZE}").into()
        );
    }

    let entry = if is_dyn {
        base_addr
            .checked_add(e_entry)
            .ok_or("ELF entry overflows")?
    } else {
        e_entry
    };
    ensure_ram_range(entry, 1, ram_size, "ELF entry")?;
    let mut ranges = Vec::new();

    for i in 0..e_phnum {
        let off = e_phoff
            .checked_add(i.checked_mul(e_phentsize).ok_or("ELF phdr overflow")?)
            .ok_or("ELF phdr overflow")?;
        if off
            .checked_add(ELF64_PHDR_SIZE)
            .filter(|end| *end <= data.len())
            .is_none()
        {
            return Err("ELF phdr out of bounds".into());
        }

        if read_u32(data, off) != PT_LOAD {
            continue;
        }

        let p_offset = read_u64(data, off + 8) as usize;
        let p_vaddr = read_u64(data, off + 16);
        let p_paddr = read_u64(data, off + 24);
        let p_filesz = read_u64(data, off + 32);
        let p_memsz = read_u64(data, off + 40);
        if p_filesz > p_memsz {
            return Err(
                format!("ELF PT_LOAD segment {i} has filesz > memsz").into()
            );
        }
        if p_offset
            .checked_add(p_filesz as usize)
            .filter(|end| *end <= data.len())
            .is_none()
        {
            return Err(format!(
                "ELF PT_LOAD segment {i} file data out of bounds"
            )
            .into());
        }

        let load_addr = if is_dyn {
            base_addr
                .checked_add(p_vaddr)
                .ok_or("ELF PT_LOAD address overflows")?
        } else {
            p_paddr
        };
        ranges.push(GuestRange::in_ram(
            load_addr,
            p_memsz,
            ram_size,
            "ELF PT_LOAD",
        )?);
    }

    Ok(KernelLoadPlan {
        entry,
        ranges,
        kind: KernelLoadKind::Elf,
    })
}

fn analyze_linux_image_kernel(
    data: &[u8],
    header: LinuxImageHeader,
    ram_size: u64,
) -> BootResult<KernelLoadPlan> {
    let load_addr = normalize_ram_addr(
        header.load_offset,
        ram_size,
        "Linux Image load offset",
    )?;
    let entry =
        normalize_ram_addr(header.kernel_entry, ram_size, "Linux Image entry")?;
    let load_len = (data.len() as u64).max(header.kernel_size);
    let range =
        GuestRange::in_ram(load_addr, load_len, ram_size, "Linux Image")?;
    Ok(KernelLoadPlan {
        entry,
        ranges: vec![range],
        kind: KernelLoadKind::LinuxImage { load_addr },
    })
}

fn analyze_raw_kernel(
    data: &[u8],
    ram_size: u64,
) -> BootResult<KernelLoadPlan> {
    let range = GuestRange::in_ram(
        KERNEL_ENTRY_DEFAULT,
        data.len() as u64,
        ram_size,
        "raw kernel image",
    )?;
    Ok(KernelLoadPlan {
        entry: KERNEL_ENTRY_DEFAULT,
        ranges: vec![range],
        kind: KernelLoadKind::Raw,
    })
}

fn boot_param_base(ram_size: u64) -> BootResult<u64> {
    if ram_size < BOOT_PARAM_WINDOW_SIZE {
        return Err(
            "LoongArch RAM is too small for direct boot parameters".into()
        );
    }
    Ok(ram_end(ram_size)? - BOOT_PARAM_WINDOW_SIZE)
}

fn secondary_boot_reserved_range(
    ram_size: u64,
    cpu_count: u32,
) -> BootResult<Option<GuestRange>> {
    if cpu_count <= 1 {
        return Ok(None);
    }

    GuestRange::in_ram(
        SECONDARY_BOOT_ENTRY,
        EFI_PAGE_SIZE,
        ram_size,
        "secondary boot",
    )
    .map(Some)
}

fn push_le_u32(data: &mut [u8], offset: usize, val: u32) {
    data[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
}

fn push_le_u64(data: &mut [u8], offset: usize, val: u64) {
    data[offset..offset + 8].copy_from_slice(&val.to_le_bytes());
}

fn align_up(value: u64, align: u64) -> BootResult<u64> {
    value
        .checked_add(align - 1)
        .map(|v| v & !(align - 1))
        .ok_or_else(|| "LoongArch boot data alignment overflows".into())
}

fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}

fn push_reserved_range(ranges: &mut Vec<(u64, u64)>, base: u64, size: u64) {
    let start = align_down(base, EFI_PAGE_SIZE);
    let end =
        align_up(base.saturating_add(size), EFI_PAGE_SIZE).unwrap_or(u64::MAX);
    if start < end {
        ranges.push((start, end));
    }
}

fn low_ram_reserved_ranges(ram_size: u64, cpu_count: u32) -> Vec<(u64, u64)> {
    let ram_size = align_down(ram_size, EFI_PAGE_SIZE);
    let mut ranges = Vec::new();
    push_reserved_range(&mut ranges, VIRT_LEGACY_IO_BASE, VIRT_LEGACY_IO_SIZE);
    push_reserved_range(&mut ranges, VIRT_IPI_BASE, VIRT_IPI_SIZE);
    push_reserved_range(&mut ranges, VIRT_EIOINTC_BASE, VIRT_EIOINTC_SIZE);
    push_reserved_range(&mut ranges, VIRT_PCH_PIC_BASE, VIRT_PCH_PIC_SIZE);
    push_reserved_range(&mut ranges, VIRT_VIRTIO_BASE, VIRT_VIRTIO_SIZE);
    push_reserved_range(&mut ranges, VIRT_UART1_BASE, VIRT_UART1_SIZE);
    push_reserved_range(&mut ranges, VIRT_RTC_BASE, VIRT_RTC_SIZE);
    push_reserved_range(&mut ranges, VIRT_PCH_MSI_BASE, VIRT_PCH_MSI_SIZE);
    push_reserved_range(&mut ranges, VIRT_FLASH0_BASE, VIRT_FLASH0_SIZE);
    push_reserved_range(&mut ranges, VIRT_FLASH1_BASE, VIRT_FLASH1_SIZE);
    push_reserved_range(&mut ranges, VIRT_FWCFG_BASE, VIRT_FWCFG_SIZE);
    push_reserved_range(&mut ranges, VIRT_UART_BASE, VIRT_UART_SIZE);
    push_reserved_range(
        &mut ranges,
        VIRT_TEST_FINISHER_BASE,
        VIRT_TEST_FINISHER_SIZE,
    );
    if cpu_count > 1 {
        push_reserved_range(
            &mut ranges,
            boot_phys_addr(SECONDARY_BOOT_ENTRY),
            SECONDARY_BOOT_CODE_SIZE,
        );
    }
    if cpu_count > 0 {
        push_reserved_range(
            &mut ranges,
            VIRT_LEGACY_IPI_BASE,
            u64::from(cpu_count).saturating_mul(VIRT_LEGACY_IPI_STRIDE),
        );
    }
    push_reserved_range(&mut ranges, VIRT_PCI_CFG_BASE, VIRT_PCI_CFG_SIZE);
    push_reserved_range(&mut ranges, VIRT_PCI_HT_CFG_BASE, VIRT_PCI_CFG_SIZE);

    ranges.sort_by_key(|(start, _)| *start);
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for (start, end) in ranges {
        if start >= ram_size {
            break;
        }
        let end = end.min(ram_size);
        if let Some((_, last_end)) = merged.last_mut() {
            if start <= *last_end {
                *last_end = (*last_end).max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    merged
        .into_iter()
        .map(|(start, end)| (start, end - start))
        .collect()
}

fn low_ram_usable_ranges(ram_size: u64, cpu_count: u32) -> Vec<(u64, u64)> {
    let ram_size = align_down(ram_size, EFI_PAGE_SIZE);
    if ram_size == 0 {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    let mut cursor = 0;
    for (base, size) in low_ram_reserved_ranges(ram_size, cpu_count) {
        if cursor < base {
            ranges.push((cursor, base - cursor));
        }
        cursor = cursor.max(base + size);
    }
    if cursor < ram_size {
        ranges.push((cursor, ram_size - cursor));
    }
    ranges
}

fn reject_overlap(
    range: GuestRange,
    reserved: &[GuestRange],
) -> BootResult<()> {
    for other in reserved {
        if range.overlaps(*other) {
            return Err(format!(
                "LoongArch {} range {:#x}..{:#x} overlaps {} range \
                 {:#x}..{:#x}",
                range.label,
                range.start,
                range.end,
                other.label,
                other.start,
                other.end
            )
            .into());
        }
    }
    Ok(())
}

fn make_reg_cells(regions: &[(u64, u64)]) -> Vec<u8> {
    let mut data = Vec::with_capacity(regions.len() * 16);
    for (base, size) in regions {
        data.extend_from_slice(&((*base >> 32) as u32).to_be_bytes());
        data.extend_from_slice(&((*base & 0xffff_ffff) as u32).to_be_bytes());
        data.extend_from_slice(&((*size >> 32) as u32).to_be_bytes());
        data.extend_from_slice(&((*size & 0xffff_ffff) as u32).to_be_bytes());
    }
    data
}

fn property_reg(fdt: &mut FdtBuilder, regions: &[(u64, u64)]) {
    fdt.property_bytes("reg", &make_reg_cells(regions));
}

fn isa_io_range_cells(base: u64, size: u32) -> [u32; 5] {
    [1, 0, (base >> 32) as u32, (base & 0xffff_ffff) as u32, size]
}

fn property_string_list(fdt: &mut FdtBuilder, name: &str, values: &[&str]) {
    let mut data = Vec::new();
    for value in values {
        data.extend_from_slice(value.as_bytes());
        data.push(0);
    }
    fdt.property_bytes(name, &data);
}

fn boot_rng_seed() -> [u8; BOOT_RNG_SEED_SIZE] {
    let mut seed = [0u8; BOOT_RNG_SEED_SIZE];
    if File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut seed))
        .is_ok()
    {
        return seed;
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos() as u64);
    let pid = u64::from(std::process::id());
    for (idx, chunk) in seed.chunks_mut(8).enumerate() {
        let value = now.rotate_left((idx * 13) as u32)
            ^ pid.rotate_left((idx * 7) as u32)
            ^ (idx as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        chunk.copy_from_slice(&value.to_le_bytes());
    }
    seed
}

fn build_fdt(
    cmdline: &str,
    ram_size: u64,
    cpu_count: u32,
    initrd: Option<(u64, u64)>,
    has_virtio_mmio: bool,
) -> Vec<u8> {
    let cpuintc_phandle = 1;
    let eiointc_phandle = 2;
    let pch_pic_phandle = 3;

    let mut fdt = FdtBuilder::new();
    fdt.begin_node("");
    fdt.property_string("compatible", "machina,loongarch64-ref");
    fdt.property_u32("#address-cells", 2);
    fdt.property_u32("#size-cells", 2);

    fdt.begin_node("chosen");
    fdt.property_bytes("rng-seed", &boot_rng_seed());
    fdt.property_string("bootargs", cmdline);
    fdt.property_string("stdout-path", &format!("/serial@{VIRT_UART_BASE:x}"));
    if let Some((start, size)) = initrd {
        fdt.property_u64("linux,initrd-start", start);
        fdt.property_u64("linux,initrd-end", start + size);
    }
    fdt.end_node();

    fdt.begin_node("cpus");
    fdt.property_u32("#address-cells", 1);
    fdt.property_u32("#size-cells", 0);
    for cpu_id in 0..cpu_count {
        fdt.begin_node(&format!("cpu@{cpu_id}"));
        fdt.property_string("device_type", "cpu");
        fdt.property_string("compatible", "loongarch,la464");
        fdt.property_u32("reg", cpu_id);
        fdt.end_node();
    }
    fdt.end_node();

    fdt.begin_node("cpuic");
    fdt.property_u32("phandle", cpuintc_phandle);
    fdt.property_string("compatible", "loongson,cpu-interrupt-controller");
    fdt.property_bytes("interrupt-controller", &[]);
    fdt.property_u32("#interrupt-cells", 1);
    fdt.end_node();

    fdt.begin_node("memory@0");
    fdt.property_string("device_type", "memory");
    property_reg(&mut fdt, &low_ram_usable_ranges(ram_size, cpu_count));
    fdt.end_node();

    fdt.begin_node(&format!("fw_cfg@{VIRT_FWCFG_BASE:x}"));
    fdt.property_string("compatible", "qemu,fw-cfg-mmio");
    property_reg(&mut fdt, &[(VIRT_FWCFG_BASE, VIRT_FWCFG_SIZE)]);
    fdt.property_bytes("dma-coherent", &[]);
    fdt.end_node();

    fdt.begin_node(&format!("ipi@{VIRT_IPI_BASE:x}"));
    fdt.property_string("compatible", "loongson,ipi");
    property_reg(&mut fdt, &[(VIRT_IPI_BASE, VIRT_IPI_SIZE)]);
    fdt.end_node();

    fdt.begin_node(&format!("eiointc@{VIRT_EIOINTC_BASE:x}"));
    fdt.property_u32("phandle", eiointc_phandle);
    property_string_list(
        &mut fdt,
        "compatible",
        &["loongson,ls2k2000-eiointc", "loongson,htvec-1.0"],
    );
    property_reg(&mut fdt, &[(VIRT_EIOINTC_BASE, VIRT_EIOINTC_SIZE)]);
    fdt.property_bytes("interrupt-controller", &[]);
    fdt.property_u32("#interrupt-cells", 1);
    fdt.property_u32("interrupt-parent", cpuintc_phandle);
    fdt.property_u32("interrupts", 3);
    fdt.end_node();

    fdt.begin_node(&format!("platic@{VIRT_PCH_PIC_BASE:x}"));
    fdt.property_u32("phandle", pch_pic_phandle);
    fdt.property_string("compatible", "loongson,pch-pic-1.0");
    property_reg(&mut fdt, &[(VIRT_PCH_PIC_BASE, VIRT_PCH_PIC_SIZE)]);
    fdt.property_bytes("interrupt-controller", &[]);
    fdt.property_u32("#interrupt-cells", 2);
    fdt.property_u32("interrupt-parent", eiointc_phandle);
    fdt.property_u32("loongson,pic-base-vec", 0);
    fdt.end_node();

    fdt.begin_node(&format!("msi@{VIRT_PCH_MSI_BASE:x}"));
    fdt.property_string("compatible", "loongson,pch-msi-1.0");
    property_reg(&mut fdt, &[(VIRT_PCH_MSI_BASE, VIRT_PCH_MSI_SIZE)]);
    fdt.property_bytes("msi-controller", &[]);
    fdt.property_u32("interrupt-parent", eiointc_phandle);
    fdt.end_node();

    fdt.begin_node(&format!("rtc@{VIRT_RTC_BASE:x}"));
    fdt.property_string("compatible", "loongson,ls7a-rtc");
    property_reg(&mut fdt, &[(VIRT_RTC_BASE, VIRT_RTC_SIZE)]);
    fdt.property_u32_list("interrupts", &[LOONGARCH_RTC_PCH_IRQ, 4]);
    fdt.property_u32("interrupt-parent", pch_pic_phandle);
    fdt.end_node();

    fdt.begin_node(&format!("isa@{VIRT_LEGACY_IO_BASE:x}"));
    fdt.property_string("compatible", "isa");
    fdt.property_u32("#address-cells", 2);
    fdt.property_u32("#size-cells", 1);
    fdt.property_u32_list(
        "ranges",
        &isa_io_range_cells(VIRT_LEGACY_IO_BASE, VIRT_LEGACY_IO_SIZE as u32),
    );
    fdt.end_node();

    fdt.begin_node(&format!("flash@{VIRT_FLASH0_BASE:x}"));
    fdt.property_string("compatible", "cfi-flash");
    property_reg(
        &mut fdt,
        &[
            (VIRT_FLASH0_BASE, VIRT_FLASH0_SIZE),
            (VIRT_FLASH1_BASE, VIRT_FLASH1_SIZE),
        ],
    );
    fdt.property_u32("bank-width", 4);
    fdt.end_node();

    fdt.begin_node(&format!("serial@{VIRT_UART_BASE:x}"));
    fdt.property_string("compatible", "ns16550a");
    property_reg(&mut fdt, &[(VIRT_UART_BASE, VIRT_UART_SIZE)]);
    fdt.property_u32("clock-frequency", 100_000_000);
    fdt.property_u32_list("interrupts", &[LOONGARCH_UART_PCH_IRQ, 4]);
    fdt.property_u32("interrupt-parent", pch_pic_phandle);
    fdt.end_node();

    if has_virtio_mmio {
        fdt.begin_node(&format!("virtio_mmio@{VIRT_VIRTIO_BASE:x}"));
        fdt.property_string("compatible", "virtio,mmio");
        property_reg(&mut fdt, &[(VIRT_VIRTIO_BASE, VIRT_VIRTIO_SIZE)]);
        fdt.property_u32_list(
            "interrupts",
            &[LOONGARCH_VIRTIO_PCH_IRQ_BASE, 4],
        );
        fdt.property_u32("interrupt-parent", pch_pic_phandle);
        fdt.end_node();
    }

    fdt.end_node();
    fdt.finish()
}

fn build_efi_system_table(nr_tables: u64, config_table_addr: u64) -> Vec<u8> {
    let mut table = vec![0u8; EFI_SYSTEM_TABLE_SIZE];
    push_le_u64(&mut table, 0, EFI_SYSTEM_TABLE_SIGNATURE);
    push_le_u32(&mut table, 8, EFI_SPECIFICATION_VERSION);
    push_le_u32(&mut table, 12, EFI_SYSTEM_TABLE_SIZE as u32);
    push_le_u32(&mut table, 32, 0x0001_0000);
    push_le_u64(&mut table, 104, nr_tables);
    push_le_u64(&mut table, 112, config_table_addr);
    table
}

fn append_config_table(entries: &mut Vec<u8>, guid: [u8; 16], table: u64) {
    entries.extend_from_slice(&guid);
    entries.extend_from_slice(&table.to_le_bytes());
}

fn build_boot_memmap(ram_size: u64, cpu_count: u32) -> Vec<u8> {
    let mut descriptors = Vec::new();
    for (base, size) in low_ram_usable_ranges(ram_size, cpu_count) {
        descriptors.push((base, size, EFI_CONVENTIONAL_MEMORY));
    }
    for (base, size) in low_ram_reserved_ranges(ram_size, cpu_count) {
        descriptors.push((base, size, EFI_RESERVED_MEMORY));
    }
    descriptors.sort_unstable_by_key(|(base, _, _)| *base);

    let map_size = descriptors.len() * EFI_MEMORY_DESC_SIZE;
    let mut memmap = vec![0u8; EFI_BOOT_MEMMAP_SIZE + map_size];
    push_le_u64(&mut memmap, 0, EFI_MEMORY_DESC_SIZE as u64);
    push_le_u64(&mut memmap, 8, map_size as u64);
    push_le_u32(&mut memmap, 16, EFI_MEMORY_DESC_VERSION);

    for (index, (base, size, ty)) in descriptors.into_iter().enumerate() {
        let desc = EFI_BOOT_MEMMAP_SIZE + index * EFI_MEMORY_DESC_SIZE;
        push_le_u32(&mut memmap, desc, ty);
        push_le_u64(&mut memmap, desc + 8, base);
        push_le_u64(&mut memmap, desc + 24, size / EFI_PAGE_SIZE);
    }
    memmap
}

fn build_initrd_table(initrd: (u64, u64)) -> Vec<u8> {
    let mut table = vec![0u8; 16];
    push_le_u64(&mut table, 0, initrd.0);
    push_le_u64(&mut table, 8, initrd.1);
    table
}

fn fw_cfg_add_sized_bytes(
    fw_cfg: &FwCfg,
    size_key: u16,
    data_key: u16,
    data: Vec<u8>,
    label: &str,
) -> BootResult<()> {
    let size = u32::try_from(data.len()).map_err(|_| {
        format!("LoongArch fw_cfg {label} data exceeds 32-bit size field")
    })?;
    fw_cfg.add_i32(size_key, size);
    fw_cfg.add_bytes(data_key, data);
    Ok(())
}

fn plan_initrd(
    initrd_path: Option<&Path>,
    ram_size: u64,
    boot_base: u64,
    reserved: &[GuestRange],
) -> BootResult<Option<InitrdPlan>> {
    let Some(path) = initrd_path else {
        return Ok(None);
    };
    let data = std::fs::read(path)?;
    let len = data.len() as u64;
    let aligned_len = align_up(len.max(1), BOOT_DATA_ALIGN)?;

    let mut end = boot_base;
    let mut last_overlap = None;
    loop {
        let Some(addr) = end.checked_sub(aligned_len) else {
            return Err("LoongArch initrd placement underflows RAM".into());
        };
        if addr < VIRT_RAM_BASE {
            let detail = last_overlap
                .map(|range: GuestRange| {
                    format!(
                        " after overlapping {} range {:#x}..{:#x}",
                        range.label, range.start, range.end
                    )
                })
                .unwrap_or_default();
            return Err(format!(
                "LoongArch initrd placement cannot find a non-overlapping \
                 RAM range below boot data{detail}"
            )
            .into());
        }

        let range = GuestRange::in_ram(addr, aligned_len, ram_size, "initrd")?;
        if let Some(overlap) = reserved
            .iter()
            .copied()
            .find(|candidate| range.overlaps(*candidate))
        {
            last_overlap = Some(overlap);
            let next_end = align_down(overlap.start, BOOT_DATA_ALIGN);
            if next_end >= end {
                return Err(format!(
                    "LoongArch initrd placement overlaps {} range \
                     {:#x}..{:#x}",
                    overlap.label, overlap.start, overlap.end
                )
                .into());
            }
            end = next_end;
            continue;
        }

        return Ok(Some(InitrdPlan { data, range, len }));
    }
}

fn write_boot_parameters(
    config: &DirectKernelBootConfig<'_>,
    ram_size: u64,
    address_space: &AddressSpace,
    kernel_ranges: &[GuestRange],
) -> BootResult<(u64, u64)> {
    let base = boot_param_base(ram_size)?;
    let boot_window = GuestRange::in_ram(
        base,
        BOOT_PARAM_WINDOW_SIZE,
        ram_size,
        "boot data",
    )?;
    reject_overlap(boot_window, kernel_ranges)?;

    let cmdline_guest_addr = base;
    let fdt_guest_addr = base + BOOT_FDT_OFFSET;
    let system_table_guest_addr = base + BOOT_SYSTEM_TABLE_OFFSET;
    let config_table_guest_addr = base + BOOT_CONFIG_TABLE_OFFSET;
    let memmap_guest_addr = base + BOOT_MEMMAP_OFFSET;
    let initrd_table_guest_addr = base + BOOT_INITRD_TABLE_OFFSET;

    let cmdline_addr = boot_phys_addr(cmdline_guest_addr);
    let fdt_addr = boot_phys_addr(fdt_guest_addr);
    let system_table_addr = boot_phys_addr(system_table_guest_addr);
    let config_table_addr = boot_phys_addr(config_table_guest_addr);
    let memmap_addr = boot_phys_addr(memmap_guest_addr);
    let initrd_table_addr = boot_phys_addr(initrd_table_guest_addr);

    let cmdline = config.cmdline.unwrap_or("");
    if cmdline.len() + 1 > COMMAND_LINE_SIZE {
        return Err(format!(
            "LoongArch kernel command line is too long: {} bytes",
            cmdline.len()
        )
        .into());
    }

    let mut initrd_reserved = kernel_ranges.to_vec();
    if let Some(range) =
        secondary_boot_reserved_range(ram_size, config.cpu_count)?
    {
        initrd_reserved.push(range);
    }
    let initrd_plan =
        plan_initrd(config.initrd_path, ram_size, base, &initrd_reserved)?;
    let initrd = initrd_plan
        .as_ref()
        .map(|plan| (boot_phys_addr(plan.range.start), plan.len));
    let fdt = build_fdt(
        cmdline,
        ram_size,
        config.cpu_count,
        initrd,
        config.has_virtio_mmio,
    );
    let fdt_limit = BOOT_SYSTEM_TABLE_OFFSET - BOOT_FDT_OFFSET;
    if fdt.len() as u64 > fdt_limit {
        return Err(format!(
            "LoongArch FDT is too large: {} bytes > {fdt_limit}",
            fdt.len()
        )
        .into());
    }

    let memmap = build_boot_memmap(ram_size, config.cpu_count);
    let mut entries = Vec::new();
    append_config_table(&mut entries, LINUX_EFI_BOOT_MEMMAP_GUID, memmap_addr);
    if let Some(plan) = &initrd_plan {
        load_binary_checked(
            &plan.data,
            plan.range.start,
            ram_size,
            address_space,
            "initrd",
        )?;
    }
    if let Some(initrd) = initrd {
        append_config_table(
            &mut entries,
            LINUX_EFI_INITRD_MEDIA_GUID,
            initrd_table_addr,
        );
        let initrd_table = build_initrd_table(initrd);
        load_binary_checked(
            &initrd_table,
            initrd_table_guest_addr,
            ram_size,
            address_space,
            "EFI initrd table",
        )?;
    }
    append_config_table(&mut entries, DEVICE_TREE_GUID, fdt_addr);

    let system_table = build_efi_system_table(
        (entries.len() / EFI_CONFIG_TABLE_SIZE) as u64,
        config_table_addr,
    );
    let mut cmdline_bytes = cmdline.as_bytes().to_vec();
    cmdline_bytes.push(0);
    if let Some(fw_cfg) = &config.fw_cfg {
        if !cmdline.is_empty() {
            fw_cfg_add_sized_bytes(
                fw_cfg,
                keys::CMDLINE_SIZE,
                keys::CMDLINE_DATA,
                cmdline_bytes.clone(),
                "cmdline",
            )?;
        }
        if let Some(plan) = &initrd_plan {
            fw_cfg_add_sized_bytes(
                fw_cfg,
                keys::INITRD_SIZE,
                keys::INITRD_DATA,
                plan.data.clone(),
                "initrd",
            )?;
        }
    }
    load_binary_checked(
        &cmdline_bytes,
        cmdline_guest_addr,
        ram_size,
        address_space,
        "kernel command line",
    )?;
    load_binary_checked(&fdt, fdt_guest_addr, ram_size, address_space, "FDT")?;
    load_binary_checked(
        &system_table,
        system_table_guest_addr,
        ram_size,
        address_space,
        "boot system table",
    )?;
    load_binary_checked(
        &entries,
        config_table_guest_addr,
        ram_size,
        address_space,
        "EFI config table",
    )?;
    load_binary_checked(
        &memmap,
        memmap_guest_addr,
        ram_size,
        address_space,
        "EFI boot memmap",
    )?;

    Ok((cmdline_addr, system_table_addr))
}

pub fn load_direct_kernel(
    kernel_path: &Path,
    config: &DirectKernelBootConfig<'_>,
    ram_size: u64,
    address_space: &AddressSpace,
) -> BootResult<DirectKernelBoot> {
    let kernel_file = std::fs::read(kernel_path)?;
    let kernel = unpack_efi_zboot_image(&kernel_file)?.unwrap_or(kernel_file);
    if let Some(fw_cfg) = &config.fw_cfg {
        fw_cfg_add_sized_bytes(
            fw_cfg,
            keys::KERNEL_SIZE,
            keys::KERNEL_DATA,
            kernel.clone(),
            "kernel",
        )?;
    }
    let plan = if is_elf(&kernel) {
        analyze_elf_kernel(&kernel, VIRT_RAM_BASE, ram_size)?
    } else if let Some(header) = parse_linux_image_header(&kernel)? {
        analyze_linux_image_kernel(&kernel, header, ram_size)?
    } else {
        analyze_raw_kernel(&kernel, ram_size)?
    };
    let (cmdline_addr, system_table_addr) =
        write_boot_parameters(config, ram_size, address_space, &plan.ranges)?;

    match plan.kind {
        KernelLoadKind::Elf => {
            let loaded =
                loader::load_elf(&kernel, VIRT_RAM_BASE, address_space)?;
            debug_assert_eq!(loaded.entry.0, plan.entry);
        }
        KernelLoadKind::LinuxImage { load_addr } => {
            loader::load_binary(&kernel, GPA::new(load_addr), address_space)?;
        }
        KernelLoadKind::Raw => {
            load_binary_checked(
                &kernel,
                KERNEL_ENTRY_DEFAULT,
                ram_size,
                address_space,
                "raw kernel image",
            )?;
        }
    }

    Ok(DirectKernelBoot {
        entry: plan.entry,
        efi_boot: EFI_BOOT_MODE,
        cmdline_addr,
        system_table_addr,
    })
}
