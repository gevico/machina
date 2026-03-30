use machina_core::address::GPA;
use machina_core::machine::{Machine, MachineOpts};
use machina_hw_riscv::boot;
use machina_hw_riscv::ref_machine::RefMachine;
use machina_hw_riscv::sbi::{SbiHandler, SBI_EXT_BASE, SBI_EXT_TIMER};

fn default_opts() -> MachineOpts {
    MachineOpts {
        ram_size: 128 * 1024 * 1024, // 128 MiB
        cpu_count: 1,
        kernel: None,
        bios: None,
        append: None,
    }
}

#[test]
fn test_ref_machine_init() {
    let mut m = RefMachine::new();
    assert_eq!(m.name(), "riscv64-ref");
    m.init(&default_opts()).expect("init failed");
    assert_eq!(m.cpu_count(), 1);
    assert_eq!(m.ram_size(), 128 * 1024 * 1024);
}

#[test]
fn test_ref_machine_memory_map() {
    let mut m = RefMachine::new();
    m.init(&default_opts()).expect("init failed");
    let as_ = m.address_space();

    // Write 0xDEADBEEF at RAM_BASE, read it back.
    let ram_base = GPA::new(0x8000_0000);
    as_.write(ram_base, 4, 0xDEAD_BEEF);
    let val = as_.read(ram_base, 4);
    assert_eq!(val, 0xDEAD_BEEF);

    // Write/read at RAM_BASE + 8.
    let addr2 = GPA::new(0x8000_0008);
    as_.write(addr2, 8, 0x1234_5678_9ABC_DEF0);
    let val2 = as_.read(addr2, 8);
    assert_eq!(val2, 0x1234_5678_9ABC_DEF0);
}

#[test]
fn test_ref_machine_uart_mmio() {
    let mut m = RefMachine::new();
    m.init(&default_opts()).expect("init failed");
    let as_ = m.address_space();

    // Read UART LSR (offset 5). THRE and TEMT should be set
    // on a freshly initialized UART: 0x60.
    let lsr_addr = GPA::new(0x1000_0005);
    let lsr = as_.read(lsr_addr, 1);
    assert_eq!(lsr & 0x60, 0x60, "THRE+TEMT not set");
}

#[test]
fn test_ref_machine_fdt_valid() {
    let mut m = RefMachine::new();
    m.init(&default_opts()).expect("init failed");
    let fdt = m.fdt_blob();

    // FDT magic: 0xD00DFEED in big-endian at offset 0.
    assert!(fdt.len() >= 4, "FDT too short");
    let magic = u32::from_be_bytes([fdt[0], fdt[1], fdt[2], fdt[3]]);
    assert_eq!(magic, 0xD00D_FEED, "bad FDT magic: {magic:#010x}");

    // FDT must contain "riscv,sv39" (not sv48).
    let sv39 = b"riscv,sv39";
    let has_sv39 = fdt.windows(sv39.len()).any(|w| w == sv39);
    assert!(has_sv39, "FDT missing riscv,sv39 mmu-type");

    // Must NOT contain sv48.
    let sv48 = b"riscv,sv48";
    let has_sv48 = fdt.windows(sv48.len()).any(|w| w == sv48);
    assert!(!has_sv48, "FDT still contains riscv,sv48");
}

#[test]
fn test_ref_machine_fdt_has_cpu() {
    let mut m = RefMachine::new();
    m.init(&default_opts()).expect("init failed");
    let fdt = m.fdt_blob();

    // The FDT blob should contain "cpu" as part of
    // the cpu@0 node name.
    let has_cpu = fdt.windows(3).any(|w| w == b"cpu");
    assert!(has_cpu, "FDT does not contain 'cpu'");
}

#[test]
fn test_ref_machine_zero_ram_fails() {
    let mut m = RefMachine::new();
    let opts = MachineOpts {
        ram_size: 0,
        cpu_count: 1,
        kernel: None,
        bios: None,
        append: None,
    };
    let result = m.init(&opts);
    assert!(result.is_err(), "init with 0 RAM should fail");
}

#[test]
fn test_ref_machine_boot_setup() {
    let mut m = RefMachine::new();
    m.init(&default_opts()).expect("init failed");

    let bios = [0x13u8; 64]; // NOP sled
    let kernel = [0xAAu8; 128];

    let info = boot::setup_boot(&m, Some(&bios), Some(&kernel))
        .expect("setup_boot failed");

    // Entry PC should be RAM_BASE.
    assert_eq!(info.entry_pc, 0x8000_0000);

    // FDT address should be within RAM range.
    let ram_end = 0x8000_0000u64 + m.ram_size();
    assert!(
        info.fdt_addr >= 0x8000_0000 && info.fdt_addr < ram_end,
        "fdt_addr {:#x} out of RAM range",
        info.fdt_addr
    );

    // Verify BIOS bytes were written at RAM_BASE.
    let as_ = m.address_space();
    let first_word = as_.read(GPA::new(0x8000_0000), 4);
    assert_eq!(first_word, 0x13131313, "bios data mismatch");

    // Verify kernel bytes at RAM_BASE + 0x20_0000.
    let kernel_word = as_.read(GPA::new(0x8020_0000), 4);
    assert_eq!(kernel_word, 0xAAAAAAAA, "kernel data mismatch");
}

// ---- New tests ----

#[test]
fn test_ref_machine_plic_contexts_multi_hart() {
    let mut m = RefMachine::new();
    let opts = MachineOpts {
        ram_size: 128 * 1024 * 1024,
        cpu_count: 2,
        kernel: None,
        bios: None,
        append: None,
    };
    m.init(&opts).expect("init failed");

    // 2 harts → 4 PLIC contexts (M+S per hart).
    // Verify by writing enable bits for context 3
    // (the last valid context) and reading back.
    let as_ = m.address_space();
    let plic_base = 0x0C00_0000u64;
    // Enable base = 0x2000, stride = 0x80 per context.
    // Context 3 enable[0] at offset 0x2000 + 3*0x80 = 0x2180.
    let ctx3_enable = GPA::new(plic_base + 0x2000 + 3 * 0x80);
    as_.write(ctx3_enable, 4, 0xFFFF_FFFF);
    let val = as_.read(ctx3_enable, 4);
    assert_eq!(
        val, 0xFFFF_FFFF,
        "PLIC context 3 enable should be writable \
         with 2 harts (4 contexts)"
    );

    // Context 4 should be out of range → read returns 0.
    let ctx4_enable = GPA::new(plic_base + 0x2000 + 4 * 0x80);
    let val4 = as_.read(ctx4_enable, 4);
    assert_eq!(val4, 0, "PLIC context 4 should be out of range");
}

#[test]
fn test_ref_machine_irq_wiring() {
    let mut m = RefMachine::new();
    m.init(&default_opts()).expect("init failed");

    // UART IRQ line exists and routes to PLIC.
    // Raise UART IRQ (source 10) → PLIC pending bit 10.
    m.uart_irq().raise();
    {
        let plic = m.plic().lock().unwrap();
        // Read pending register: IRQ 10 is in word 0
        // (bits 0-31 cover IRQs 0-31).
        let pending = plic.read(0x1000, 4);
        assert_ne!(
            pending & (1 << 10),
            0,
            "UART IRQ 10 should be pending in PLIC"
        );
    }

    // Lower UART IRQ → pending bit cleared.
    m.uart_irq().lower();
    {
        let plic = m.plic().lock().unwrap();
        let pending = plic.read(0x1000, 4);
        assert_eq!(
            pending & (1 << 10),
            0,
            "UART IRQ 10 should be cleared in PLIC"
        );
    }

    // Verify CPU IRQ sink exists for hart 0.
    let sink = m.cpu_irq_sink(0);
    // MEI (11) should initially be low.
    assert!(!sink.pending(11), "MEI should be low initially");
}

#[test]
fn test_ref_machine_boot_cpu_state() {
    let mut m = RefMachine::new();
    m.init(&default_opts()).expect("init failed");

    let bios = [0x13u8; 16]; // minimal NOP sled
    let info =
        boot::setup_boot(&m, Some(&bios), None).expect("setup_boot failed");

    // a0 = hart_id = 0.
    assert_eq!(info.hart_id, 0);
    // a1 = fdt_addr, must be within RAM.
    assert!(info.fdt_addr >= 0x8000_0000, "fdt_addr below RAM_BASE");
    // PC = entry_pc = RAM_BASE.
    assert_eq!(info.entry_pc, 0x8000_0000);
}

#[test]
fn test_sbi_base_extension() {
    let args = [0u64; 6];

    // func 0: spec version = 2 (SBI 0.2).
    let r = SbiHandler::handle_ecall(SBI_EXT_BASE, 0, &args);
    assert_eq!(r.error, 0);
    assert_eq!(r.value, 2);

    // func 1: impl id = 0 (machina).
    let r = SbiHandler::handle_ecall(SBI_EXT_BASE, 1, &args);
    assert_eq!(r.error, 0);
    assert_eq!(r.value, 0);

    // func 2: impl version = 1.
    let r = SbiHandler::handle_ecall(SBI_EXT_BASE, 2, &args);
    assert_eq!(r.error, 0);
    assert_eq!(r.value, 1);

    // func 3: probe extension = 0 (not available).
    let r = SbiHandler::handle_ecall(SBI_EXT_BASE, 3, &args);
    assert_eq!(r.error, 0);
    assert_eq!(r.value, 0);
}

#[test]
fn test_sbi_unsupported_extension() {
    let args = [0u64; 6];

    // Timer extension not implemented → not_supported.
    let r = SbiHandler::handle_ecall(SBI_EXT_TIMER, 0, &args);
    assert_eq!(r.error, -2, "should return SBI_ERR_NOT_SUPPORTED");
    assert_eq!(r.value, 0);

    // Completely unknown extension.
    let r = SbiHandler::handle_ecall(0xDEAD, 0, &args);
    assert_eq!(r.error, -2, "unknown ext → not supported");
}
