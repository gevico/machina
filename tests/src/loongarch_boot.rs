use std::io::Write;

use machina_core::address::GPA;
use machina_core::machine::{Machine, MachineOpts};
use machina_guest_loongarch::loongarch::csr::{
    CRMD_DA, CRMD_IE, CRMD_PG, CSR_CRMD,
};
use machina_hw_firmware::keys;
use machina_hw_loongarch::boot::{KERNEL_ENTRY_DEFAULT, SECONDARY_BOOT_ENTRY};
use machina_hw_loongarch::virt_machine::{
    LoongArchVirtMachine, VIRT_EIOINTC_BASE, VIRT_EIOINTC_SIZE,
    VIRT_FLASH0_BASE, VIRT_FLASH0_SIZE, VIRT_FLASH1_BASE, VIRT_FLASH1_SIZE,
    VIRT_FWCFG_BASE, VIRT_FWCFG_SIZE, VIRT_IPI_BASE, VIRT_IPI_SIZE,
    VIRT_LEGACY_IO_BASE, VIRT_LEGACY_IO_SIZE, VIRT_PCH_MSI_BASE,
    VIRT_PCH_MSI_SIZE, VIRT_PCH_PIC_BASE, VIRT_PCH_PIC_SIZE, VIRT_RAM_BASE,
    VIRT_RTC_BASE, VIRT_RTC_SIZE, VIRT_TEST_FINISHER_BASE, VIRT_UART1_BASE,
    VIRT_UART1_SIZE, VIRT_UART_BASE, VIRT_UART_SIZE, VIRT_VIRTIO_BASE,
    VIRT_VIRTIO_SIZE,
};

const EFI_SYSTEM_TABLE_SIGNATURE: u64 = 0x5453_5953_2049_4249;
const EFI_SYSTEM_TABLE_HEADER_SIZE: u32 = 120;
const EFI_CONFIG_TABLE_SIZE: u64 = 24;
const EFI_MEMORY_DESCRIPTOR_SIZE: u64 = 40;
const EFI_RESERVED_MEMORY: u32 = 0;
const EFI_CONVENTIONAL_MEMORY: u32 = 7;
const EFI_PAGE_SIZE: u64 = 4096;
const LOW_PHYS_MASK: u64 = 0x0fff_ffff_ffff_ffff;

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

fn default_opts() -> MachineOpts {
    MachineOpts {
        ram_size: 64 * 1024 * 1024,
        cpu_count: 1,
        kernel: None,
        bios: None,
        bios_builtin: false,
        append: None,
        nographic: false,
        drive: None,
        initrd: None,
        dtb: None,
        loaders: Vec::new(),
        netdev: None,
    }
}

fn build_minimal_elf(entry: u64, p_paddr: u64, payload: &[u8]) -> Vec<u8> {
    let mut elf = vec![0u8; 64 + 56];

    elf[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
    elf[4] = 2;
    elf[5] = 1;
    elf[6] = 1;
    elf[16..18].copy_from_slice(&2u16.to_le_bytes());
    elf[20..24].copy_from_slice(&1u32.to_le_bytes());
    elf[24..32].copy_from_slice(&entry.to_le_bytes());
    elf[32..40].copy_from_slice(&64u64.to_le_bytes());
    elf[52..54].copy_from_slice(&64u16.to_le_bytes());
    elf[54..56].copy_from_slice(&56u16.to_le_bytes());
    elf[56..58].copy_from_slice(&1u16.to_le_bytes());

    let ph = 64usize;
    let p_offset = 120u64;
    let p_filesz = payload.len() as u64;
    let p_memsz = p_filesz + 8;
    elf[ph..ph + 4].copy_from_slice(&1u32.to_le_bytes());
    elf[ph + 8..ph + 16].copy_from_slice(&p_offset.to_le_bytes());
    elf[ph + 16..ph + 24].copy_from_slice(&p_paddr.to_le_bytes());
    elf[ph + 24..ph + 32].copy_from_slice(&p_paddr.to_le_bytes());
    elf[ph + 32..ph + 40].copy_from_slice(&p_filesz.to_le_bytes());
    elf[ph + 40..ph + 48].copy_from_slice(&p_memsz.to_le_bytes());

    elf.extend_from_slice(payload);
    elf
}

fn build_linux_image(entry: u64, load_offset: u64, payload: &[u8]) -> Vec<u8> {
    let mut image = vec![0u8; 64];
    image[0..2].copy_from_slice(&0x5a4du16.to_le_bytes());
    image[8..16].copy_from_slice(&entry.to_le_bytes());
    image[16..24].copy_from_slice(&((64 + payload.len()) as u64).to_le_bytes());
    image[24..32].copy_from_slice(&load_offset.to_le_bytes());
    image[56..60].copy_from_slice(&0x8182_23cdu32.to_le_bytes());
    image.extend_from_slice(payload);
    image
}

fn read_bytes(
    machine: &LoongArchVirtMachine,
    addr: u64,
    len: usize,
) -> Vec<u8> {
    (0..len)
        .map(|offset| {
            machine
                .address_space()
                .read(GPA::new(addr + offset as u64), 1) as u8
        })
        .collect()
}

fn boot_guest_addr(addr: u64) -> u64 {
    if addr < VIRT_RAM_BASE {
        VIRT_RAM_BASE + addr
    } else {
        addr
    }
}

fn read_guest_u32(machine: &LoongArchVirtMachine, addr: u64) -> u32 {
    u32::from_le_bytes(read_bytes(machine, addr, 4).try_into().unwrap())
}

fn read_guest_be_u32(machine: &LoongArchVirtMachine, addr: u64) -> u32 {
    u32::from_be_bytes(read_bytes(machine, addr, 4).try_into().unwrap())
}

fn read_guest_u64(machine: &LoongArchVirtMachine, addr: u64) -> u64 {
    u64::from_le_bytes(read_bytes(machine, addr, 8).try_into().unwrap())
}

fn read_fw_cfg_bytes(
    machine: &LoongArchVirtMachine,
    selector: u16,
    len: usize,
) -> Vec<u8> {
    let as_ = machine.address_space();
    as_.write(GPA::new(VIRT_FWCFG_BASE + 0x08), 2, u64::from(selector));
    (0..len)
        .map(|_| as_.read(GPA::new(VIRT_FWCFG_BASE), 1) as u8)
        .collect()
}

fn read_fw_cfg_u32(machine: &LoongArchVirtMachine, selector: u16) -> u32 {
    u32::from_le_bytes(
        read_fw_cfg_bytes(machine, selector, 4).try_into().unwrap(),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EfiMemDesc {
    ty: u32,
    phys: u64,
    pages: u64,
}

fn align_up(value: u64, align: u64) -> u64 {
    value.saturating_add(align - 1) & !(align - 1)
}

fn expected_low_ram_reserved_ranges(
    ram_size: u64,
    cpu_count: u32,
) -> Vec<(u64, u64)> {
    let ram_size = ram_size & !(EFI_PAGE_SIZE - 1);
    let mut ranges = Vec::new();
    for (base, size) in [
        (VIRT_LEGACY_IO_BASE, VIRT_LEGACY_IO_SIZE),
        (VIRT_IPI_BASE, VIRT_IPI_SIZE),
        (VIRT_EIOINTC_BASE, VIRT_EIOINTC_SIZE),
        (VIRT_PCH_PIC_BASE, VIRT_PCH_PIC_SIZE),
        (VIRT_VIRTIO_BASE, VIRT_VIRTIO_SIZE),
        (VIRT_UART1_BASE, VIRT_UART1_SIZE),
        (VIRT_RTC_BASE, VIRT_RTC_SIZE),
        (VIRT_PCH_MSI_BASE, VIRT_PCH_MSI_SIZE),
        (VIRT_FLASH0_BASE, VIRT_FLASH0_SIZE),
        (VIRT_FLASH1_BASE, VIRT_FLASH1_SIZE),
        (VIRT_FWCFG_BASE, VIRT_FWCFG_SIZE),
        (VIRT_UART_BASE, VIRT_UART_SIZE),
    ] {
        let start = base & !(EFI_PAGE_SIZE - 1);
        let end = align_up(base + size, EFI_PAGE_SIZE);
        if start < ram_size {
            ranges.push((start, end.min(ram_size) - start));
        }
    }
    if cpu_count > 1 {
        let start = SECONDARY_BOOT_ENTRY & LOW_PHYS_MASK;
        if start < ram_size {
            ranges.push((start, EFI_PAGE_SIZE.min(ram_size - start)));
        }
    }
    ranges.sort_by_key(|(start, _)| *start);
    ranges
}

fn expected_low_ram_usable_ranges(
    ram_size: u64,
    cpu_count: u32,
) -> Vec<(u64, u64)> {
    let ram_size = ram_size & !(EFI_PAGE_SIZE - 1);
    if ram_size == 0 {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    let mut cursor = 0;
    for (base, size) in expected_low_ram_reserved_ranges(ram_size, cpu_count) {
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

fn read_efi_memmap(
    machine: &LoongArchVirtMachine,
    memmap_guest: u64,
) -> Vec<EfiMemDesc> {
    let desc_size = read_guest_u64(machine, memmap_guest);
    let map_size = read_guest_u64(machine, memmap_guest + 8);
    assert_eq!(desc_size, EFI_MEMORY_DESCRIPTOR_SIZE);
    assert_eq!(map_size % desc_size, 0);
    assert_eq!(read_guest_u32(machine, memmap_guest + 16), 1);

    (0..map_size / desc_size)
        .map(|index| {
            let desc = memmap_guest + 40 + index * desc_size;
            EfiMemDesc {
                ty: read_guest_u32(machine, desc),
                phys: read_guest_u64(machine, desc + 8),
                pages: read_guest_u64(machine, desc + 24),
            }
        })
        .collect()
}

fn desc_ranges(descs: &[EfiMemDesc], ty: u32) -> Vec<(u64, u64)> {
    descs
        .iter()
        .filter(|desc| desc.ty == ty)
        .map(|desc| (desc.phys, desc.pages * EFI_PAGE_SIZE))
        .collect()
}

fn read_be_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap())
}

#[derive(Debug)]
struct FdtProp {
    path: String,
    name: String,
    data: Vec<u8>,
}

fn read_cstr(data: &[u8], offset: usize) -> String {
    let end = data[offset..]
        .iter()
        .position(|b| *b == 0)
        .map(|pos| offset + pos)
        .unwrap();
    String::from_utf8(data[offset..end].to_vec()).unwrap()
}

fn parse_fdt_props(blob: &[u8]) -> Vec<FdtProp> {
    assert_eq!(read_be_u32(blob, 0), 0xd00d_feed);
    let off_struct = read_be_u32(blob, 8) as usize;
    let off_strings = read_be_u32(blob, 12) as usize;
    let size_strings = read_be_u32(blob, 32) as usize;
    let strings = &blob[off_strings..off_strings + size_strings];

    let mut props = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut offset = off_struct;
    loop {
        let token = read_be_u32(blob, offset);
        offset += 4;
        match token {
            1 => {
                let name = read_cstr(blob, offset);
                offset += name.len() + 1;
                offset = (offset + 3) & !3;
                if name.is_empty() {
                    stack.clear();
                } else {
                    stack.push(name);
                }
            }
            2 => {
                stack.pop();
            }
            3 => {
                let len = read_be_u32(blob, offset) as usize;
                let nameoff = read_be_u32(blob, offset + 4) as usize;
                offset += 8;
                let data = blob[offset..offset + len].to_vec();
                offset = (offset + len + 3) & !3;
                let path = if stack.is_empty() {
                    "/".to_string()
                } else {
                    format!("/{}", stack.join("/"))
                };
                props.push(FdtProp {
                    path,
                    name: read_cstr(strings, nameoff),
                    data,
                });
            }
            9 => break,
            other => panic!("unexpected FDT token {other:#x} at {offset:#x}"),
        }
    }
    props
}

fn fdt_prop<'a>(props: &'a [FdtProp], path: &str, name: &str) -> &'a [u8] {
    props
        .iter()
        .find(|prop| prop.path == path && prop.name == name)
        .unwrap_or_else(|| panic!("missing FDT property {path}:{name}"))
        .data
        .as_slice()
}

fn assert_fdt_string(props: &[FdtProp], path: &str, name: &str, value: &str) {
    let mut expected = value.as_bytes().to_vec();
    expected.push(0);
    assert_eq!(fdt_prop(props, path, name), expected.as_slice());
}

fn assert_fdt_string_list(
    props: &[FdtProp],
    path: &str,
    name: &str,
    values: &[&str],
) {
    let mut expected = Vec::new();
    for value in values {
        expected.extend_from_slice(value.as_bytes());
        expected.push(0);
    }
    assert_eq!(fdt_prop(props, path, name), expected.as_slice());
}

fn cells_for_pairs(pairs: &[(u64, u64)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (base, size) in pairs {
        out.extend_from_slice(&((*base >> 32) as u32).to_be_bytes());
        out.extend_from_slice(&((*base & 0xffff_ffff) as u32).to_be_bytes());
        out.extend_from_slice(&((*size >> 32) as u32).to_be_bytes());
        out.extend_from_slice(&((*size & 0xffff_ffff) as u32).to_be_bytes());
    }
    out
}

fn fdt_u64_prop(props: &[FdtProp], path: &str, name: &str) -> u64 {
    u64::from_be_bytes(fdt_prop(props, path, name).try_into().unwrap())
}

fn config_table_ptr(
    machine: &LoongArchVirtMachine,
    system_table_addr: u64,
    guid: [u8; 16],
) -> u64 {
    let system_table_addr = boot_guest_addr(system_table_addr);
    assert_eq!(
        read_guest_u64(machine, system_table_addr),
        EFI_SYSTEM_TABLE_SIGNATURE
    );
    assert_eq!(
        read_guest_u32(machine, system_table_addr + 12),
        EFI_SYSTEM_TABLE_HEADER_SIZE
    );
    let nr_tables = read_guest_u64(machine, system_table_addr + 104);
    let tables = read_guest_u64(machine, system_table_addr + 112);
    for index in 0..nr_tables {
        let entry = boot_guest_addr(tables + index * EFI_CONFIG_TABLE_SIZE);
        if read_bytes(machine, entry, 16) == guid {
            return read_guest_u64(machine, entry + 16);
        }
    }
    panic!("missing EFI config table GUID {guid:02x?}");
}

fn boot_minimal_elf(
    opts: &mut MachineOpts,
) -> (LoongArchVirtMachine, tempfile::NamedTempFile) {
    let entry = VIRT_RAM_BASE + 0x20_0000;
    let segment_addr = VIRT_RAM_BASE + 0x30_0000;
    let elf = build_minimal_elf(entry, segment_addr, &[0x13, 0x57, 0x9b]);
    let mut kernel = tempfile::NamedTempFile::new().unwrap();
    kernel.write_all(&elf).unwrap();
    opts.kernel = Some(kernel.path().to_path_buf());

    let mut machine = LoongArchVirtMachine::new();
    machine.init(opts).expect("init loongarch ref");
    machine.boot().expect("boot direct ELF");
    (machine, kernel)
}

#[test]
fn task43_direct_boot_loads_linux_image_header_at_load_offset() {
    let load_offset = 0x20_0000;
    let entry = load_offset;
    let payload = [0x13, 0x57, 0x9b, 0xdf];
    let image = build_linux_image(entry, load_offset, &payload);

    let mut kernel = tempfile::NamedTempFile::new().unwrap();
    kernel.write_all(&image).unwrap();

    let mut opts = default_opts();
    opts.kernel = Some(kernel.path().to_path_buf());

    let mut machine = LoongArchVirtMachine::new();
    machine.init(&opts).expect("init loongarch ref");
    machine.boot().expect("boot LoongArch Linux Image");

    let load_addr = VIRT_RAM_BASE + load_offset;
    assert_eq!(read_bytes(&machine, load_addr, image.len()), image);
    assert_eq!(
        read_bytes(&machine, VIRT_RAM_BASE, payload.len()),
        vec![0; payload.len()]
    );

    let cpu = machine.cpu();
    let cpu = cpu.lock().unwrap();
    assert_eq!(cpu.pc(), VIRT_RAM_BASE + entry);
    assert_eq!(cpu.read_gpr(4), 1);
    assert_ne!(cpu.read_gpr(5), 0);
    assert_ne!(cpu.read_gpr(6), 0);
}

#[test]
fn task44_direct_boot_builds_efi_system_table_and_fdt() {
    let mut opts = default_opts();
    opts.append = Some("console=ttyS0 rdinit=/init".to_string());
    let (machine, _kernel) = boot_minimal_elf(&mut opts);

    let cpu = machine.cpu();
    let cpu = cpu.lock().unwrap();
    assert_eq!(cpu.read_gpr(4), 1);
    let cmdline_addr = cpu.read_gpr(5);
    let system_table_addr = cpu.read_gpr(6);
    drop(cpu);

    let mut expected_cmdline = opts.append.clone().unwrap().into_bytes();
    expected_cmdline.push(0);
    assert_eq!(
        read_bytes(
            &machine,
            boot_guest_addr(cmdline_addr),
            expected_cmdline.len()
        ),
        expected_cmdline
    );

    let memmap_addr = config_table_ptr(
        &machine,
        system_table_addr,
        LINUX_EFI_BOOT_MEMMAP_GUID,
    );
    let memmap_guest = boot_guest_addr(memmap_addr);
    let memmap_descs = read_efi_memmap(&machine, memmap_guest);
    let expected_memory =
        expected_low_ram_usable_ranges(opts.ram_size, opts.cpu_count);
    let expected_reserved =
        expected_low_ram_reserved_ranges(opts.ram_size, opts.cpu_count);
    assert_eq!(
        desc_ranges(&memmap_descs, EFI_CONVENTIONAL_MEMORY),
        expected_memory
    );
    assert_eq!(
        desc_ranges(&memmap_descs, EFI_RESERVED_MEMORY),
        expected_reserved
    );

    let fdt_addr =
        config_table_ptr(&machine, system_table_addr, DEVICE_TREE_GUID);
    let fdt_guest = boot_guest_addr(fdt_addr);
    let fdt_size = read_guest_be_u32(&machine, fdt_guest + 4) as usize;
    let fdt = read_bytes(&machine, fdt_guest, fdt_size);
    let props = parse_fdt_props(&fdt);

    assert_fdt_string(&props, "/", "compatible", "machina,loongarch64-ref");
    assert_fdt_string(
        &props,
        "/chosen",
        "bootargs",
        opts.append.as_ref().unwrap(),
    );
    assert_eq!(fdt_prop(&props, "/chosen", "rng-seed").len(), 32);
    assert_eq!(
        fdt_prop(&props, "/memory@0", "reg"),
        cells_for_pairs(&expected_memory).as_slice()
    );
    let fw_cfg_node = format!("/fw_cfg@{VIRT_FWCFG_BASE:x}");
    assert_fdt_string(&props, &fw_cfg_node, "compatible", "qemu,fw-cfg-mmio");
    assert_eq!(
        fdt_prop(&props, &fw_cfg_node, "reg"),
        cells_for_pairs(&[(VIRT_FWCFG_BASE, VIRT_FWCFG_SIZE)]).as_slice()
    );
    assert!(fdt_prop(&props, &fw_cfg_node, "dma-coherent").is_empty());
    assert_eq!(
        fdt_prop(&props, "/cpus", "#address-cells"),
        1u32.to_be_bytes().as_slice()
    );
    assert_eq!(
        fdt_prop(&props, "/cpus", "#size-cells"),
        0u32.to_be_bytes().as_slice()
    );
    assert_fdt_string(&props, "/cpus/cpu@0", "device_type", "cpu");
    assert_fdt_string(&props, "/cpus/cpu@0", "compatible", "loongarch,la464");
    assert_eq!(
        fdt_prop(&props, "/cpus/cpu@0", "reg"),
        0u32.to_be_bytes().as_slice()
    );
    assert_fdt_string(
        &props,
        "/cpuic",
        "compatible",
        "loongson,cpu-interrupt-controller",
    );
    assert!(fdt_prop(&props, "/cpuic", "interrupt-controller").is_empty());
    assert_eq!(
        fdt_prop(&props, &format!("/ipi@{VIRT_IPI_BASE:x}"), "reg"),
        cells_for_pairs(&[(VIRT_IPI_BASE, VIRT_IPI_SIZE)]).as_slice()
    );
    assert_eq!(
        fdt_prop(&props, &format!("/eiointc@{VIRT_EIOINTC_BASE:x}"), "reg"),
        cells_for_pairs(&[(VIRT_EIOINTC_BASE, VIRT_EIOINTC_SIZE)]).as_slice()
    );
    assert_fdt_string_list(
        &props,
        &format!("/eiointc@{VIRT_EIOINTC_BASE:x}"),
        "compatible",
        &["loongson,ls2k2000-eiointc", "loongson,htvec-1.0"],
    );
    assert_eq!(
        fdt_prop(
            &props,
            &format!("/eiointc@{VIRT_EIOINTC_BASE:x}"),
            "interrupt-parent"
        ),
        1u32.to_be_bytes().as_slice()
    );
    assert_eq!(
        fdt_prop(
            &props,
            &format!("/eiointc@{VIRT_EIOINTC_BASE:x}"),
            "interrupts"
        ),
        3u32.to_be_bytes().as_slice()
    );
    assert_eq!(
        fdt_prop(&props, &format!("/platic@{VIRT_PCH_PIC_BASE:x}"), "reg"),
        cells_for_pairs(&[(VIRT_PCH_PIC_BASE, VIRT_PCH_PIC_SIZE)]).as_slice()
    );
    assert_eq!(
        fdt_prop(
            &props,
            &format!("/platic@{VIRT_PCH_PIC_BASE:x}"),
            "interrupt-parent"
        ),
        2u32.to_be_bytes().as_slice()
    );
    assert_eq!(
        fdt_prop(&props, &format!("/msi@{VIRT_PCH_MSI_BASE:x}"), "reg"),
        cells_for_pairs(&[(VIRT_PCH_MSI_BASE, VIRT_PCH_MSI_SIZE)]).as_slice()
    );
    assert_eq!(
        fdt_prop(
            &props,
            &format!("/msi@{VIRT_PCH_MSI_BASE:x}"),
            "interrupt-parent"
        ),
        2u32.to_be_bytes().as_slice()
    );
    let isa_node = format!("/isa@{VIRT_LEGACY_IO_BASE:x}");
    assert_fdt_string(&props, &isa_node, "compatible", "isa");
    assert_eq!(
        fdt_prop(&props, &isa_node, "#address-cells"),
        2u32.to_be_bytes().as_slice()
    );
    assert_eq!(
        fdt_prop(&props, &isa_node, "#size-cells"),
        1u32.to_be_bytes().as_slice()
    );
    assert_eq!(
        fdt_prop(&props, &isa_node, "ranges"),
        [
            1u32.to_be_bytes(),
            0u32.to_be_bytes(),
            ((VIRT_LEGACY_IO_BASE >> 32) as u32).to_be_bytes(),
            (VIRT_LEGACY_IO_BASE as u32).to_be_bytes(),
            (VIRT_LEGACY_IO_SIZE as u32).to_be_bytes(),
        ]
        .concat()
        .as_slice()
    );
    assert_fdt_string(
        &props,
        &format!("/rtc@{VIRT_RTC_BASE:x}"),
        "compatible",
        "loongson,ls7a-rtc",
    );
    assert_eq!(
        fdt_prop(&props, &format!("/rtc@{VIRT_RTC_BASE:x}"), "reg"),
        cells_for_pairs(&[(VIRT_RTC_BASE, VIRT_RTC_SIZE)]).as_slice()
    );
    assert_eq!(
        fdt_prop(&props, &format!("/rtc@{VIRT_RTC_BASE:x}"), "interrupts"),
        [6u32.to_be_bytes(), 4u32.to_be_bytes()].concat()
    );
    assert_eq!(
        fdt_prop(
            &props,
            &format!("/rtc@{VIRT_RTC_BASE:x}"),
            "interrupt-parent"
        ),
        3u32.to_be_bytes().as_slice()
    );
    assert_fdt_string(
        &props,
        &format!("/flash@{VIRT_FLASH0_BASE:x}"),
        "compatible",
        "cfi-flash",
    );
    assert_eq!(
        fdt_prop(&props, &format!("/flash@{VIRT_FLASH0_BASE:x}"), "reg"),
        cells_for_pairs(&[
            (VIRT_FLASH0_BASE, VIRT_FLASH0_SIZE),
            (VIRT_FLASH1_BASE, VIRT_FLASH1_SIZE),
        ])
        .as_slice()
    );
    assert_eq!(
        fdt_prop(
            &props,
            &format!("/flash@{VIRT_FLASH0_BASE:x}"),
            "bank-width"
        ),
        4u32.to_be_bytes().as_slice()
    );
    assert_eq!(
        fdt_prop(&props, &format!("/serial@{VIRT_UART_BASE:x}"), "reg"),
        cells_for_pairs(&[(VIRT_UART_BASE, VIRT_UART_SIZE)]).as_slice()
    );
    assert_eq!(
        fdt_prop(
            &props,
            &format!("/serial@{VIRT_UART_BASE:x}"),
            "interrupt-parent"
        ),
        3u32.to_be_bytes().as_slice()
    );
    assert!(
        !props
            .iter()
            .any(|prop| prop.path
                == format!("/virtio_mmio@{VIRT_VIRTIO_BASE:x}")),
        "virtio node must not be emitted when no virtio device is present"
    );
}

#[test]
fn task47_direct_boot_exposes_physical_boot_data_addresses() {
    let initrd_bytes = [0x33, 0x44, 0x55, 0x66];
    let mut initrd = tempfile::NamedTempFile::new().unwrap();
    initrd.write_all(&initrd_bytes).unwrap();

    let mut opts = default_opts();
    opts.append = Some("console=ttyS0 earlycon rdinit=/init".to_string());
    opts.initrd = Some(initrd.path().to_path_buf());
    let (machine, _kernel) = boot_minimal_elf(&mut opts);

    let cpu = machine.cpu();
    let cpu = cpu.lock().unwrap();
    let cmdline_addr = cpu.read_gpr(5);
    let system_table_addr = cpu.read_gpr(6);
    drop(cpu);

    assert!(
        cmdline_addr < VIRT_RAM_BASE,
        "a1 must expose a physical command-line address"
    );
    assert!(
        system_table_addr < VIRT_RAM_BASE,
        "a2 must expose a physical EFI system-table address"
    );

    let system_table_guest = boot_guest_addr(system_table_addr);
    let memmap_addr = config_table_ptr(
        &machine,
        system_table_guest,
        LINUX_EFI_BOOT_MEMMAP_GUID,
    );
    assert!(
        memmap_addr < VIRT_RAM_BASE,
        "EFI config entries must expose physical table addresses"
    );
    let memmap_guest = boot_guest_addr(memmap_addr);
    let memmap_descs = read_efi_memmap(&machine, memmap_guest);
    let expected_memory =
        expected_low_ram_usable_ranges(opts.ram_size, opts.cpu_count);
    let expected_reserved =
        expected_low_ram_reserved_ranges(opts.ram_size, opts.cpu_count);
    assert_eq!(
        desc_ranges(&memmap_descs, EFI_CONVENTIONAL_MEMORY),
        expected_memory
    );
    assert_eq!(
        desc_ranges(&memmap_descs, EFI_RESERVED_MEMORY),
        expected_reserved
    );

    let fdt_addr =
        config_table_ptr(&machine, system_table_guest, DEVICE_TREE_GUID);
    assert!(fdt_addr < VIRT_RAM_BASE, "FDT pointer must be physical");
    let fdt_guest = boot_guest_addr(fdt_addr);
    let fdt_size = read_guest_be_u32(&machine, fdt_guest + 4) as usize;
    let props = parse_fdt_props(&read_bytes(&machine, fdt_guest, fdt_size));
    assert_eq!(
        fdt_prop(&props, "/memory@0", "reg"),
        cells_for_pairs(&expected_memory).as_slice()
    );

    let initrd_table_addr = config_table_ptr(
        &machine,
        system_table_guest,
        LINUX_EFI_INITRD_MEDIA_GUID,
    );
    assert!(
        initrd_table_addr < VIRT_RAM_BASE,
        "initrd config entry must be physical"
    );
    let initrd_table_guest = boot_guest_addr(initrd_table_addr);
    let initrd_start = read_guest_u64(&machine, initrd_table_guest);
    assert!(initrd_start < VIRT_RAM_BASE, "initrd base must be physical");
    assert_eq!(
        read_bytes(&machine, boot_guest_addr(initrd_start), initrd_bytes.len()),
        initrd_bytes
    );
    assert_eq!(
        fdt_u64_prop(&props, "/chosen", "linux,initrd-start"),
        initrd_start
    );
}

#[test]
fn task44_direct_boot_adds_initrd_and_optional_virtio_to_fdt() {
    let mut drive = tempfile::NamedTempFile::new().unwrap();
    drive.write_all(&[0u8; 512]).unwrap();
    let initrd_bytes = [0xaa, 0xbb, 0xcc, 0xdd, 0xee];
    let mut initrd = tempfile::NamedTempFile::new().unwrap();
    initrd.write_all(&initrd_bytes).unwrap();

    let mut opts = default_opts();
    opts.drive = Some(drive.path().to_path_buf());
    opts.initrd = Some(initrd.path().to_path_buf());
    let (machine, _kernel) = boot_minimal_elf(&mut opts);

    let system_table_addr = machine.cpu().lock().unwrap().read_gpr(6);
    let initrd_table_addr = config_table_ptr(
        &machine,
        system_table_addr,
        LINUX_EFI_INITRD_MEDIA_GUID,
    );
    let initrd_table_guest = boot_guest_addr(initrd_table_addr);
    let initrd_start = read_guest_u64(&machine, initrd_table_guest);
    let initrd_size = read_guest_u64(&machine, initrd_table_guest + 8);
    assert_eq!(initrd_size, initrd_bytes.len() as u64);
    assert_eq!(
        read_bytes(&machine, boot_guest_addr(initrd_start), initrd_bytes.len()),
        initrd_bytes
    );

    let fdt_addr =
        config_table_ptr(&machine, system_table_addr, DEVICE_TREE_GUID);
    let fdt_guest = boot_guest_addr(fdt_addr);
    let fdt_size = read_guest_be_u32(&machine, fdt_guest + 4) as usize;
    let props = parse_fdt_props(&read_bytes(&machine, fdt_guest, fdt_size));
    assert_eq!(
        fdt_u64_prop(&props, "/chosen", "linux,initrd-start"),
        initrd_start
    );
    assert_eq!(
        fdt_u64_prop(&props, "/chosen", "linux,initrd-end"),
        initrd_start + initrd_size
    );
    assert_eq!(
        fdt_prop(&props, &format!("/virtio_mmio@{VIRT_VIRTIO_BASE:x}"), "reg"),
        cells_for_pairs(&[(VIRT_VIRTIO_BASE, VIRT_VIRTIO_SIZE)]).as_slice()
    );
}

#[test]
fn task44_direct_boot_fdt_describes_all_configured_cpus() {
    let mut opts = default_opts();
    opts.cpu_count = 4;
    let (machine, _kernel) = boot_minimal_elf(&mut opts);

    let cpu = machine.cpu();
    let cpu = cpu.lock().unwrap();
    let system_table_addr = cpu.read_gpr(6);
    drop(cpu);

    let fdt_addr =
        config_table_ptr(&machine, system_table_addr, DEVICE_TREE_GUID);
    let fdt_guest = boot_guest_addr(fdt_addr);
    let fdt_size = read_guest_be_u32(&machine, fdt_guest + 4) as usize;
    let fdt = read_bytes(&machine, fdt_guest, fdt_size);
    let props = parse_fdt_props(&fdt);

    for cpu_id in 0..opts.cpu_count {
        let node = format!("/cpus/cpu@{cpu_id}");
        assert_fdt_string(&props, &node, "device_type", "cpu");
        assert_fdt_string(&props, &node, "compatible", "loongarch,la464");
        assert_eq!(
            fdt_prop(&props, &node, "reg"),
            cpu_id.to_be_bytes().as_slice()
        );
    }

    let memmap_addr = config_table_ptr(
        &machine,
        system_table_addr,
        LINUX_EFI_BOOT_MEMMAP_GUID,
    );
    let memmap_descs = read_efi_memmap(&machine, boot_guest_addr(memmap_addr));
    let expected_reserved =
        expected_low_ram_reserved_ranges(opts.ram_size, opts.cpu_count);
    let expected_memory =
        expected_low_ram_usable_ranges(opts.ram_size, opts.cpu_count);

    assert!(expected_reserved
        .contains(&(SECONDARY_BOOT_ENTRY & LOW_PHYS_MASK, EFI_PAGE_SIZE,)));
    assert_eq!(
        desc_ranges(&memmap_descs, EFI_RESERVED_MEMORY),
        expected_reserved
    );
    assert_eq!(
        desc_ranges(&memmap_descs, EFI_CONVENTIONAL_MEMORY),
        expected_memory
    );
    assert_eq!(
        fdt_prop(&props, "/memory@0", "reg"),
        cells_for_pairs(&expected_memory).as_slice()
    );
}

#[test]
fn task44_direct_boot_memory_map_omits_low_mmio_holes() {
    let mut opts = default_opts();
    opts.ram_size = VIRT_RTC_BASE + 0x2000;
    let (machine, _kernel) = boot_minimal_elf(&mut opts);

    let system_table_addr = machine.cpu().lock().unwrap().read_gpr(6);
    let memmap_addr = config_table_ptr(
        &machine,
        system_table_addr,
        LINUX_EFI_BOOT_MEMMAP_GUID,
    );
    let memmap_descs = read_efi_memmap(&machine, boot_guest_addr(memmap_addr));
    let expected_reserved =
        expected_low_ram_reserved_ranges(opts.ram_size, opts.cpu_count);
    let expected_memory =
        expected_low_ram_usable_ranges(opts.ram_size, opts.cpu_count);

    assert_eq!(
        desc_ranges(&memmap_descs, EFI_RESERVED_MEMORY),
        expected_reserved
    );
    assert_eq!(
        desc_ranges(&memmap_descs, EFI_CONVENTIONAL_MEMORY),
        expected_memory
    );
    assert!(expected_reserved.contains(&(VIRT_PCH_PIC_BASE, EFI_PAGE_SIZE)));
    assert!(expected_reserved.contains(&(VIRT_VIRTIO_BASE, EFI_PAGE_SIZE)));
    assert!(expected_reserved.contains(&(VIRT_UART1_BASE, EFI_PAGE_SIZE)));
    assert!(expected_reserved
        .contains(&(VIRT_RTC_BASE & !(EFI_PAGE_SIZE - 1), EFI_PAGE_SIZE)));

    let fdt_addr =
        config_table_ptr(&machine, system_table_addr, DEVICE_TREE_GUID);
    let fdt_guest = boot_guest_addr(fdt_addr);
    let fdt_size = read_guest_be_u32(&machine, fdt_guest + 4) as usize;
    let props = parse_fdt_props(&read_bytes(&machine, fdt_guest, fdt_size));
    assert_eq!(
        fdt_prop(&props, "/memory@0", "reg"),
        cells_for_pairs(&expected_memory).as_slice()
    );
}

#[test]
fn task45_direct_boot_populates_fw_cfg_kernel_initrd_and_cmdline() {
    let entry = VIRT_RAM_BASE + 0x20_0000;
    let segment_addr = VIRT_RAM_BASE + 0x30_0000;
    let kernel_image =
        build_minimal_elf(entry, segment_addr, &[0x10, 0x20, 0x30]);
    let mut kernel = tempfile::NamedTempFile::new().unwrap();
    kernel.write_all(&kernel_image).unwrap();

    let initrd_bytes = [0xaa, 0xbb, 0xcc, 0xdd];
    let mut initrd = tempfile::NamedTempFile::new().unwrap();
    initrd.write_all(&initrd_bytes).unwrap();

    let mut opts = default_opts();
    opts.kernel = Some(kernel.path().to_path_buf());
    opts.initrd = Some(initrd.path().to_path_buf());
    opts.append = Some("console=ttyS0 rdinit=/init".to_string());

    let mut machine = LoongArchVirtMachine::new();
    machine.init(&opts).expect("init loongarch ref");
    machine.boot().expect("boot direct ELF");

    assert_eq!(
        read_fw_cfg_u32(&machine, keys::KERNEL_SIZE),
        kernel_image.len() as u32
    );
    assert_eq!(
        read_fw_cfg_bytes(&machine, keys::KERNEL_DATA, kernel_image.len()),
        kernel_image
    );

    assert_eq!(
        read_fw_cfg_u32(&machine, keys::INITRD_SIZE),
        initrd_bytes.len() as u32
    );
    assert_eq!(
        read_fw_cfg_bytes(&machine, keys::INITRD_DATA, initrd_bytes.len()),
        initrd_bytes
    );

    let mut cmdline = opts.append.as_ref().unwrap().as_bytes().to_vec();
    cmdline.push(0);
    assert_eq!(
        read_fw_cfg_u32(&machine, keys::CMDLINE_SIZE),
        cmdline.len() as u32
    );
    assert_eq!(
        read_fw_cfg_bytes(&machine, keys::CMDLINE_DATA, cmdline.len()),
        cmdline
    );
}

#[test]
fn task44_direct_boot_rejects_oversized_cmdline_boot_data() {
    let mut opts = default_opts();
    opts.append = Some("x".repeat(4096));
    let entry = VIRT_RAM_BASE + 0x20_0000;
    let elf = build_minimal_elf(entry, VIRT_RAM_BASE + 0x30_0000, &[0x11]);
    let mut kernel = tempfile::NamedTempFile::new().unwrap();
    kernel.write_all(&elf).unwrap();
    opts.kernel = Some(kernel.path().to_path_buf());

    let mut machine = LoongArchVirtMachine::new();
    machine.init(&opts).expect("init loongarch ref");
    let err = machine.boot().expect_err("oversized cmdline must fail");
    assert!(
        err.to_string().contains("command line"),
        "unexpected error: {err}"
    );
}

#[test]
fn task44_direct_boot_rejects_kernel_overlapping_boot_data_window() {
    let mut opts = default_opts();
    opts.ram_size = 4 * 1024 * 1024;
    let boot_base = VIRT_RAM_BASE + opts.ram_size - 0x2_0000;
    let entry = VIRT_RAM_BASE + 0x20_0000;
    let elf = build_minimal_elf(entry, boot_base, &[0x5a, 0x6b, 0x7c]);
    let mut kernel = tempfile::NamedTempFile::new().unwrap();
    kernel.write_all(&elf).unwrap();
    opts.kernel = Some(kernel.path().to_path_buf());

    let mut machine = LoongArchVirtMachine::new();
    machine.init(&opts).expect("init loongarch ref");
    let err = machine
        .boot()
        .expect_err("kernel overlapping boot data must fail");
    let err = err.to_string();
    assert!(
        err.contains("overlap") && err.contains("boot data"),
        "unexpected error: {err}"
    );
}

#[test]
fn task44_direct_boot_rejects_initrd_overlapping_raw_kernel() {
    let mut kernel = tempfile::NamedTempFile::new().unwrap();
    kernel.write_all(&vec![0x5a; 0x1_0000]).unwrap();
    let mut initrd = tempfile::NamedTempFile::new().unwrap();
    initrd.write_all(&vec![0xa5; 0x21_0000]).unwrap();

    let mut opts = default_opts();
    opts.ram_size = 0x23_0000;
    opts.kernel = Some(kernel.path().to_path_buf());
    opts.initrd = Some(initrd.path().to_path_buf());

    let mut machine = LoongArchVirtMachine::new();
    machine.init(&opts).expect("init loongarch ref");
    let err = machine
        .boot()
        .expect_err("initrd overlapping raw kernel must fail");
    let err = err.to_string();
    assert!(
        err.contains("overlap") && err.contains("initrd"),
        "unexpected error: {err}"
    );
}

#[test]
fn task44_direct_boot_rejects_initrd_overlapping_smp_trampoline() {
    let mut opts = default_opts();
    opts.cpu_count = 2;
    opts.ram_size = 0x60_0000;

    let entry = VIRT_RAM_BASE + 0x20_0000;
    let segment_addr = VIRT_RAM_BASE + 0x30_0000;
    let elf = build_minimal_elf(entry, segment_addr, &[0x13, 0x57, 0x9b]);
    let mut kernel = tempfile::NamedTempFile::new().unwrap();
    kernel.write_all(&elf).unwrap();
    opts.kernel = Some(kernel.path().to_path_buf());

    let mut initrd = tempfile::NamedTempFile::new().unwrap();
    initrd.write_all(&vec![0xa5; 0x2f_f000]).unwrap();
    opts.initrd = Some(initrd.path().to_path_buf());

    let mut machine = LoongArchVirtMachine::new();
    machine.init(&opts).expect("init loongarch ref");
    let err = machine
        .boot()
        .expect_err("initrd overlapping SMP trampoline must fail");
    let err = err.to_string();
    assert!(
        err.contains("initrd") && err.contains("secondary boot"),
        "unexpected error: {err}"
    );
}

#[test]
fn task43_direct_boot_loads_elf_and_sets_initial_cpu_state() {
    let entry = VIRT_RAM_BASE + 0x20_0000;
    let segment_addr = VIRT_RAM_BASE + 0x30_0000;
    let payload = [0xde, 0xad, 0xbe, 0xef, 0xca];
    let elf = build_minimal_elf(entry, segment_addr, &payload);

    let mut kernel = tempfile::NamedTempFile::new().unwrap();
    kernel.write_all(&elf).unwrap();

    let mut opts = default_opts();
    opts.kernel = Some(kernel.path().to_path_buf());
    opts.append = Some("console=ttyS0 rdinit=/init".to_string());

    let mut machine = LoongArchVirtMachine::new();
    machine.init(&opts).expect("init loongarch ref");
    for offset in payload.len()..payload.len() + 8 {
        machine.address_space().write(
            GPA::new(segment_addr + offset as u64),
            1,
            0xff,
        );
    }

    machine.boot().expect("boot direct ELF");

    assert_eq!(read_bytes(&machine, segment_addr, payload.len()), payload);
    assert_eq!(
        read_bytes(&machine, segment_addr + payload.len() as u64, 8),
        vec![0; 8],
        "ELF BSS must be zero-filled"
    );

    let cpu = machine.cpu();
    let cpu = cpu.lock().unwrap();
    assert_eq!(cpu.pc(), entry);
    let crmd = cpu.csr_read(CSR_CRMD);
    assert_eq!(crmd & 0x3, 0, "direct boot starts in PLV0");
    assert_ne!(crmd & CRMD_DA, 0, "direct boot must start in DA mode");
    assert_eq!(crmd & CRMD_PG, 0, "direct boot must leave paging disabled");
    assert_eq!(crmd & CRMD_IE, 0, "direct boot must leave interrupts off");
    assert_eq!(cpu.read_gpr(4), 1, "a0 must carry EFI boot mode");

    let cmdline_addr = cpu.read_gpr(5);
    let system_table_addr = cpu.read_gpr(6);
    let cmdline_len = opts.append.as_ref().unwrap().len() as u64 + 1;
    assert!(
        machine
            .address_space()
            .is_mapped(GPA::new(boot_guest_addr(cmdline_addr)), 1)
            && machine.address_space().is_mapped(
                GPA::new(boot_guest_addr(cmdline_addr + cmdline_len - 1)),
                1
            ),
        "a1 must point at the guest command line"
    );
    assert!(
        machine
            .address_space()
            .is_mapped(GPA::new(boot_guest_addr(system_table_addr)), 8),
        "a2 must point at guest boot-system-table storage"
    );
    drop(cpu);

    let mut expected_cmdline = opts.append.clone().unwrap().into_bytes();
    expected_cmdline.push(0);
    assert_eq!(
        read_bytes(
            &machine,
            boot_guest_addr(cmdline_addr),
            expected_cmdline.len()
        ),
        expected_cmdline
    );
}

#[test]
fn task43_direct_boot_loads_raw_image_at_default_entry() {
    let image = [0x11, 0x22, 0x33, 0x44, 0x55];
    let mut kernel = tempfile::NamedTempFile::new().unwrap();
    kernel.write_all(&image).unwrap();

    let mut opts = default_opts();
    opts.kernel = Some(kernel.path().to_path_buf());

    let mut machine = LoongArchVirtMachine::new();
    machine.init(&opts).expect("init loongarch ref");
    machine.boot().expect("boot raw image");

    assert_eq!(
        read_bytes(&machine, KERNEL_ENTRY_DEFAULT, image.len()),
        image
    );
    assert_eq!(
        read_bytes(&machine, VIRT_RAM_BASE, image.len()),
        vec![0; image.len()]
    );
    let cpu = machine.cpu();
    let cpu = cpu.lock().unwrap();
    assert_eq!(cpu.pc(), KERNEL_ENTRY_DEFAULT);
    assert_eq!(cpu.read_gpr(4), 1);
    assert_ne!(cpu.read_gpr(5), 0);
    assert_ne!(cpu.read_gpr(6), 0);
}

#[test]
fn task43_direct_boot_rejects_bad_linux_image_magic() {
    let mut image = build_linux_image(0x20_0000, 0x20_0000, &[0xaa]);
    image[56..60].copy_from_slice(&0u32.to_le_bytes());
    let mut kernel = tempfile::NamedTempFile::new().unwrap();
    kernel.write_all(&image).unwrap();

    let mut opts = default_opts();
    opts.kernel = Some(kernel.path().to_path_buf());

    let mut machine = LoongArchVirtMachine::new();
    machine.init(&opts).expect("init loongarch ref");
    let err = machine.boot().expect_err("bad Linux Image magic must fail");
    assert!(
        err.to_string().contains("LoongArch Linux Image"),
        "unexpected error: {err}"
    );
}

#[test]
fn task43_direct_boot_rejects_out_of_ram_raw_image() {
    let image = [0x66, 0x77, 0x88, 0x99];
    let mut kernel = tempfile::NamedTempFile::new().unwrap();
    kernel.write_all(&image).unwrap();

    let mut opts = default_opts();
    opts.ram_size = 1024 * 1024;
    opts.kernel = Some(kernel.path().to_path_buf());

    let mut machine = LoongArchVirtMachine::new();
    machine.init(&opts).expect("init loongarch ref");
    let err = machine.boot().expect_err("out-of-RAM raw image must fail");
    assert!(
        err.to_string().contains("outside LoongArch RAM"),
        "unexpected error: {err}"
    );
}

#[test]
fn direct_boot_reserves_test_finisher_page() {
    // With enough RAM to cover the finisher's low-physical address, the
    // EFI memory map must carve out the finisher MMIO page so a guest
    // kernel never treats it as conventional RAM and corrupts allocations
    // or accidentally triggers the finisher (#160).
    let mut opts = default_opts();
    opts.ram_size = VIRT_TEST_FINISHER_BASE + 0x2000;
    let (machine, _kernel) = boot_minimal_elf(&mut opts);

    let system_table_addr = machine.cpu().lock().unwrap().read_gpr(6);
    let memmap_addr = config_table_ptr(
        &machine,
        system_table_addr,
        LINUX_EFI_BOOT_MEMMAP_GUID,
    );
    let memmap_descs = read_efi_memmap(&machine, boot_guest_addr(memmap_addr));

    let addr = VIRT_TEST_FINISHER_BASE;
    let covers = |ranges: &[(u64, u64)]| {
        ranges
            .iter()
            .any(|&(start, size)| start <= addr && addr < start + size)
    };
    let reserved = desc_ranges(&memmap_descs, EFI_RESERVED_MEMORY);
    let conventional = desc_ranges(&memmap_descs, EFI_CONVENTIONAL_MEMORY);
    assert!(
        covers(&reserved),
        "finisher page must be EFI-reserved; reserved={reserved:?}"
    );
    assert!(
        !covers(&conventional),
        "finisher must not be conventional RAM; got {conventional:?}"
    );
}
