use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use machina_core::address::GPA;
use machina_core::machine::{LoaderSpec, Machine, MachineOpts, MachineState};
use machina_guest_loongarch::loongarch::cpu::{
    LoongArchCpu, LoongArchCpuInterruptState,
};
use machina_guest_loongarch::loongarch::csr::{CRMD_DA, CSR_CRMD};
use machina_hw_char::uart::{Uart16550, Uart16550Mmio};
use machina_hw_core::bus::{SysBus, SysBusError};
use machina_hw_core::chardev::{CharFrontend, StdioChardev, StdoutChardev};
use machina_hw_core::irq::{InterruptSource, IrqSink};
use machina_hw_core::loader;
use machina_hw_firmware::{keys, FwCfg, FwCfgMmio};
use machina_hw_intc::eiointc::{Eiointc, EiointcIrqSink, EiointcMmio};
use machina_hw_intc::ipi::{LoongArchIpi, LoongArchIpiMmio};
use machina_hw_intc::pch_msi::{PchMsi, PchMsiMmio};
use machina_hw_intc::pch_pic::{PchPic, PchPicIrqSink, PchPicMmio};
use machina_hw_rtc::ls7a_rtc::{Ls7aRtc, Ls7aRtcMmio};
use machina_hw_storage::pflash::{
    PFlashCfi01, PFlashCfi01Config, PFlashCfi01Mmio,
};
use machina_hw_storage::{FlashMedia, MemBackend};
use machina_hw_virtio::mmio::VirtioMmio;
use machina_memory::address_space::AddressSpace;
use machina_memory::ram::RamBlock;
use machina_memory::region::{MemoryRegion, MmioOps};

use crate::boot;
use crate::interrupt::{
    LoongArchInterruptCascade, LOONGARCH_DEVICE_HWI, LOONGARCH_RTC_PCH_IRQ,
    LOONGARCH_UART_PCH_IRQ, LOONGARCH_VIRTIO_PCH_IRQ_BASE,
};
use crate::iocsr::VirtIocsrBus;
use crate::test_finisher::{LoongArchTestFinisher, LoongArchTestFinisherMmio};

type UartRxCallback = Arc<Mutex<dyn FnMut(u8) + Send>>;
type LoongArchPFlash = PFlashCfi01<MemBackend>;
pub type RuntimeLoongArchCpuState =
    (LoongArchCpu, Arc<LoongArchCpuInterruptState>);

struct EmptyPciConfigMmio;

impl MmioOps for EmptyPciConfigMmio {
    fn read(&self, _offset: u64, size: u32) -> u64 {
        match size {
            1 => 0xff,
            2 => 0xffff,
            4 => 0xffff_ffff,
            8 => 0xffff_ffff_ffff_ffff,
            _ => 0,
        }
    }

    fn write(&self, _offset: u64, _size: u32, _val: u64) {}
}

pub const VIRT_UART_BASE: u64 = 0x1FE0_01E0;
pub const VIRT_UART_SIZE: u64 = 0x8;
pub const VIRT_UART1_BASE: u64 = 0x1008_0000;
pub const VIRT_UART1_SIZE: u64 = 0x8;
pub const VIRT_IPI_BASE: u64 = 0x0100_0000;
pub const VIRT_IPI_SIZE: u64 = 0x100;
pub const VIRT_LEGACY_IPI_BASE: u64 = 0x1FE0_1000;
pub const VIRT_LEGACY_IPI_SIZE: u64 = 0x100;
pub const VIRT_LEGACY_IPI_STRIDE: u64 = 0x100;
pub const VIRT_LEGACY_IO_BASE: u64 = 0x8000;
pub const VIRT_LEGACY_IO_SIZE: u64 = 0x1_0000;
pub const VIRT_EIOINTC_BASE: u64 = 0x0200_0000;
pub const VIRT_EIOINTC_SIZE: u64 = 0x1_0000;
pub const VIRT_PCH_PIC_BASE: u64 = 0x1000_0000;
pub const VIRT_PCH_PIC_SIZE: u64 = 0x400;
pub const VIRT_PCH_MSI_BASE: u64 = 0x2FF0_0000;
pub const VIRT_PCH_MSI_SIZE: u64 = 0x8;
pub const LOONGARCH_PCH_MSI_IRQ_BASE: u32 = 32;
pub const LOONGARCH_PCH_MSI_IRQ_NUM: u32 = 224;
pub const VIRT_RTC_BASE: u64 = 0x100D_0100;
pub const VIRT_RTC_SIZE: u64 = 0x100;
pub const VIRT_FLASH0_BASE: u64 = 0x1C00_0000;
pub const VIRT_FLASH0_SIZE: u64 = 0x0100_0000;
pub const VIRT_FLASH1_BASE: u64 = 0x1D00_0000;
pub const VIRT_FLASH1_SIZE: u64 = 0x0100_0000;
pub const VIRT_VIRTIO_BASE: u64 = 0x1000_8000;
pub const VIRT_VIRTIO_SIZE: u64 = 0x1000;
pub const VIRT_FWCFG_BASE: u64 = 0x1E02_0000;
pub const VIRT_FWCFG_SIZE: u64 = 0x18;
pub const VIRT_TEST_FINISHER_BASE: u64 = 0x1FE0_8000;
pub const VIRT_TEST_FINISHER_SIZE: u64 = 0x8;
pub const VIRT_PCI_CFG_BASE: u64 = 0x00FE_0000_0000;
pub const VIRT_PCI_HT_CFG_BASE: u64 = 0x0EFE_0000_0000;
pub const VIRT_PCI_CFG_SIZE: u64 = 0x2000_0000;
pub const VIRT_RAM_BASE: u64 = 0x9000_0000_0000_0000;
pub const VIRT_RAM_SIZE_DEFAULT: u64 = 256 * 1024 * 1024;
pub const VIRT_CPU_COUNT_MAX: u32 = 256;

pub const VIRT_CPUCFG_PRID: u32 = 0x0014_C010;

pub struct VirtMachineConfig {
    pub ram_size: u64,
    pub kernel_path: Option<String>,
}

impl Default for VirtMachineConfig {
    fn default() -> Self {
        Self {
            ram_size: VIRT_RAM_SIZE_DEFAULT,
            kernel_path: None,
        }
    }
}

pub struct LoongArchVirtMachine {
    name: String,
    machine_state: MachineState,
    ram_size: u64,
    cpus: Vec<Arc<Mutex<LoongArchCpu>>>,
    address_space: Option<AddressSpace>,
    sysbus: Option<SysBus>,
    ram_block: Option<Arc<RamBlock>>,
    uart: Option<Arc<Uart16550>>,
    uart1: Option<Arc<Uart16550>>,
    ipi: Option<Arc<LoongArchIpi>>,
    pch_msi: Option<Arc<PchMsi>>,
    rtc: Option<Arc<Ls7aRtc>>,
    pflash0: Option<Arc<LoongArchPFlash>>,
    pflash1: Option<Arc<LoongArchPFlash>>,
    interrupt_cascade: Option<LoongArchInterruptCascade>,
    iocsr_bus: Option<Arc<VirtIocsrBus>>,
    virtio_mmio: Option<VirtioMmio>,
    fw_cfg: Option<Arc<FwCfg>>,
    test_finisher: Option<Arc<LoongArchTestFinisher>>,
    kernel_path: Option<PathBuf>,
    initrd_path: Option<PathBuf>,
    kernel_cmdline: Option<String>,
    loaders: Vec<LoaderSpec>,
    uart_chardev: Option<CharFrontend>,
    uart1_chardev: Option<CharFrontend>,
}

impl LoongArchVirtMachine {
    #[must_use]
    pub fn new() -> Self {
        Self {
            name: "loongarch64-ref".to_string(),
            machine_state: MachineState::new_root("machine"),
            ram_size: 0,
            cpus: Vec::new(),
            address_space: None,
            sysbus: None,
            ram_block: None,
            uart: None,
            uart1: None,
            ipi: None,
            pch_msi: None,
            rtc: None,
            pflash0: None,
            pflash1: None,
            interrupt_cascade: None,
            iocsr_bus: None,
            virtio_mmio: None,
            fw_cfg: None,
            test_finisher: None,
            kernel_path: None,
            initrd_path: None,
            kernel_cmdline: None,
            loaders: Vec::new(),
            uart_chardev: None,
            uart1_chardev: None,
        }
    }

    pub fn address_space(&self) -> &AddressSpace {
        self.address_space
            .as_ref()
            .expect("machine not initialized")
    }

    pub fn sysbus(&self) -> &SysBus {
        self.sysbus.as_ref().expect("machine not initialized")
    }

    #[must_use]
    pub fn cpu(&self) -> Arc<Mutex<LoongArchCpu>> {
        self.cpu_at(0)
    }

    #[must_use]
    pub fn cpu_at(&self, cpu_id: usize) -> Arc<Mutex<LoongArchCpu>> {
        Arc::clone(self.cpus.get(cpu_id).expect("machine CPU not initialized"))
    }

    #[must_use]
    pub fn ram_block(&self) -> &Arc<RamBlock> {
        self.ram_block.as_ref().expect("machine not initialized")
    }

    #[must_use]
    pub fn uart(&self) -> Arc<Uart16550> {
        Arc::clone(self.uart.as_ref().expect("machine not initialized"))
    }

    #[must_use]
    pub fn uart1(&self) -> Arc<Uart16550> {
        Arc::clone(self.uart1.as_ref().expect("machine not initialized"))
    }

    /// Install the callback fired when a guest writes the test-finisher
    /// MMIO register. `pass` is true for a passing result.
    pub fn install_finish_handler(&self, handler: Box<dyn Fn(bool) + Send>) {
        if let Some(test_finisher) = &self.test_finisher {
            test_finisher.set_finish_handler(handler);
        }
    }

    pub fn take_runtime_cpu_state(
        &mut self,
    ) -> Result<RuntimeLoongArchCpuState, Box<dyn std::error::Error>> {
        self.take_runtime_cpu_state_at(0)
    }

    pub fn take_runtime_cpu_state_at(
        &mut self,
        cpu_id: usize,
    ) -> Result<RuntimeLoongArchCpuState, Box<dyn std::error::Error>> {
        let interrupts = Arc::new(LoongArchCpuInterruptState::default());
        self.ipi().connect_output(
            cpu_id as u32,
            runtime_ipi_source(Arc::clone(&interrupts)),
        );
        self.interrupt_cascade
            .as_ref()
            .expect("machine not initialized")
            .connect_cpu_hwi_async(
                cpu_id as u32,
                LOONGARCH_DEVICE_HWI,
                Arc::clone(&interrupts),
            );

        let cpu_arc = self.cpu_at(cpu_id);
        let mut guard = cpu_arc.lock().unwrap();
        let mut replacement = LoongArchCpu::new();
        replacement.set_cpuid(cpu_id as u64);
        let cpu = std::mem::replace(&mut *guard, replacement);
        drop(guard);

        Ok((cpu, interrupts))
    }

    pub fn take_runtime_cpu_states(
        &mut self,
    ) -> Result<Vec<RuntimeLoongArchCpuState>, Box<dyn std::error::Error>> {
        let count = self.cpus.len();
        let mut states = Vec::with_capacity(count);
        for cpu_id in 0..count {
            states.push(self.take_runtime_cpu_state_at(cpu_id)?);
        }
        Ok(states)
    }

    #[must_use]
    pub fn ipi(&self) -> Arc<LoongArchIpi> {
        Arc::clone(self.ipi.as_ref().expect("machine not initialized"))
    }

    #[must_use]
    pub fn eiointc(&self) -> Arc<Eiointc> {
        self.interrupt_cascade
            .as_ref()
            .expect("machine not initialized")
            .eiointc()
    }

    #[must_use]
    pub fn pch_pic(&self) -> Arc<PchPic> {
        self.interrupt_cascade
            .as_ref()
            .expect("machine not initialized")
            .pch_pic()
    }

    #[must_use]
    pub fn pch_msi(&self) -> Arc<PchMsi> {
        Arc::clone(self.pch_msi.as_ref().expect("machine not initialized"))
    }

    #[must_use]
    pub fn rtc(&self) -> Arc<Ls7aRtc> {
        Arc::clone(self.rtc.as_ref().expect("machine not initialized"))
    }

    #[must_use]
    pub fn pflash0(&self) -> Arc<LoongArchPFlash> {
        Arc::clone(self.pflash0.as_ref().expect("machine not initialized"))
    }

    #[must_use]
    pub fn pflash1(&self) -> Arc<LoongArchPFlash> {
        Arc::clone(self.pflash1.as_ref().expect("machine not initialized"))
    }

    #[must_use]
    pub fn iocsr_bus(&self) -> Arc<VirtIocsrBus> {
        Arc::clone(self.iocsr_bus.as_ref().expect("machine not initialized"))
    }

    #[must_use]
    pub fn interrupt_cascade(&self) -> &LoongArchInterruptCascade {
        self.interrupt_cascade
            .as_ref()
            .expect("machine not initialized")
    }

    pub fn set_uart_chardev(
        &mut self,
        frontend: CharFrontend,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if self.uart.is_some() {
            return Err(
                "loongarch64-ref UART chardev must be set before init".into()
            );
        }
        self.uart_chardev = Some(frontend);
        Ok(())
    }

    pub fn set_uart1_chardev(
        &mut self,
        frontend: CharFrontend,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if self.uart1.is_some() {
            return Err(
                "loongarch64-ref UART1 chardev must be set before init".into(),
            );
        }
        self.uart1_chardev = Some(frontend);
        Ok(())
    }

    #[must_use]
    pub fn virtio_mmio(&self) -> Option<&VirtioMmio> {
        self.virtio_mmio.as_ref()
    }

    fn attach_interrupt_devices(
        sysbus: &mut SysBus,
        ipi: &Arc<LoongArchIpi>,
        cpu_count: u32,
        eiointc: &Arc<Eiointc>,
        pch_pic: &Arc<PchPic>,
        pch_msi: &Arc<PchMsi>,
    ) -> Result<(), SysBusError> {
        ipi.attach_to_bus(sysbus)?;
        ipi.register_mmio(
            MemoryRegion::io(
                "ipi0-mmio",
                VIRT_IPI_SIZE,
                Arc::new(LoongArchIpiMmio(Arc::clone(ipi), 0)),
            ),
            GPA::new(VIRT_IPI_BASE),
        )?;
        for cpu_id in 0..cpu_count {
            ipi.register_mmio(
                MemoryRegion::io(
                    &format!("ipi{cpu_id}"),
                    VIRT_LEGACY_IPI_SIZE,
                    Arc::new(LoongArchIpiMmio(Arc::clone(ipi), cpu_id)),
                ),
                GPA::new(
                    VIRT_LEGACY_IPI_BASE
                        + u64::from(cpu_id) * VIRT_LEGACY_IPI_STRIDE,
                ),
            )?;
        }

        eiointc.attach_to_bus(sysbus)?;
        eiointc.register_mmio(
            MemoryRegion::io(
                "eiointc0-mmio",
                VIRT_EIOINTC_SIZE,
                Arc::new(EiointcMmio(Arc::clone(eiointc))),
            ),
            GPA::new(VIRT_EIOINTC_BASE),
        )?;

        pch_pic.attach_to_bus(sysbus)?;
        pch_pic.register_mmio(
            MemoryRegion::io(
                "pch-pic0-mmio",
                VIRT_PCH_PIC_SIZE,
                Arc::new(PchPicMmio(Arc::clone(pch_pic))),
            ),
            GPA::new(VIRT_PCH_PIC_BASE),
        )?;

        pch_msi.attach_to_bus(sysbus)?;
        pch_msi.register_mmio(
            MemoryRegion::io(
                "pch-msi0-mmio",
                VIRT_PCH_MSI_SIZE,
                Arc::new(PchMsiMmio(Arc::clone(pch_msi))),
            ),
            GPA::new(VIRT_PCH_MSI_BASE),
        )
    }

    fn realize_interrupt_devices(
        sysbus: &mut SysBus,
        address_space: &mut AddressSpace,
        ipi: &Arc<LoongArchIpi>,
        eiointc: &Arc<Eiointc>,
        pch_pic: &Arc<PchPic>,
        pch_msi: &Arc<PchMsi>,
    ) -> Result<(), SysBusError> {
        ipi.realize_onto(sysbus, address_space)?;
        eiointc.realize_onto(sysbus, address_space)?;
        pch_pic.realize_onto(sysbus, address_space)?;
        pch_msi.realize_onto(sysbus, address_space)
    }

    fn create_pflash_bank(
        local_id: &str,
        base: u64,
        size: u64,
        sysbus: &mut SysBus,
    ) -> Result<Arc<LoongArchPFlash>, Box<dyn std::error::Error>> {
        let flash = FlashMedia::new(
            MemBackend::new(vec![0xff; size as usize], false),
            256 * 1024,
        )?;
        let pflash = Arc::new(PFlashCfi01::new_named(
            local_id,
            flash,
            PFlashCfi01Config {
                bank_width: 4,
                device_width: 2,
                sector_len: 256 * 1024,
                num_blocks: u32::try_from(size / (256 * 1024))?,
                ident0: 0x89,
                ident1: 0x18,
                ident2: 0,
                ident3: 0,
                ..PFlashCfi01Config::default()
            },
        )?);
        pflash.attach_to_bus(sysbus)?;
        pflash.register_mmio(
            MemoryRegion::io(
                local_id,
                size,
                Arc::new(PFlashCfi01Mmio(Arc::clone(&pflash))),
            ),
            GPA::new(base),
        )?;
        Ok(pflash)
    }

    fn loader_addr_to_ram_alias(
        &self,
        addr: u64,
        size: u64,
    ) -> Result<u64, Box<dyn std::error::Error>> {
        let end = addr
            .checked_add(size)
            .ok_or("loongarch64-ref loader range end overflows u64")?;
        let high_ram_end = VIRT_RAM_BASE
            .checked_add(self.ram_size)
            .ok_or("loongarch64-ref high RAM range end overflows u64")?;

        if end <= self.ram_size {
            return Ok(VIRT_RAM_BASE + addr);
        }
        if addr >= VIRT_RAM_BASE && end <= high_ram_end {
            return Ok(addr);
        }

        Err(format!(
            "loongarch64-ref loader range {:#x}..{:#x} is outside RAM \
             0x0..{:#x} / {:#x}..{:#x}",
            addr, end, self.ram_size, VIRT_RAM_BASE, high_ram_end
        )
        .into())
    }

    fn apply_loaders(&self) -> Result<(), Box<dyn std::error::Error>> {
        for loader_spec in &self.loaders {
            if !loader_spec.force_raw {
                return Err(
                    "loongarch64-ref loader requires force-raw=on".into()
                );
            }
            let data = std::fs::read(&loader_spec.file)?;
            let load_addr = self.loader_addr_to_ram_alias(
                loader_spec.addr,
                data.len() as u64,
            )?;
            loader::load_binary(
                &data,
                GPA::new(load_addr),
                self.address_space(),
            )?;
        }
        Ok(())
    }
}

impl Default for LoongArchVirtMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl Machine for LoongArchVirtMachine {
    fn name(&self) -> &str {
        &self.name
    }

    fn machine_state(&self) -> &MachineState {
        &self.machine_state
    }

    fn machine_state_mut(&mut self) -> &mut MachineState {
        &mut self.machine_state
    }

    fn init(
        &mut self,
        opts: &MachineOpts,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if opts.ram_size == 0 {
            return Err("ram_size must be greater than 0".into());
        }
        if opts.cpu_count == 0 {
            return Err("loongarch64-ref requires at least one CPU".into());
        }
        if opts.cpu_count > VIRT_CPU_COUNT_MAX {
            return Err(format!(
                "loongarch64-ref supports at most {VIRT_CPU_COUNT_MAX} CPUs"
            )
            .into());
        }
        if opts.netdev.is_some() {
            return Err(
                "loongarch64-ref does not support virtio-net-device/-netdev"
                    .into(),
            );
        }

        self.ram_size = opts.ram_size;
        self.kernel_path = opts.kernel.clone();
        self.initrd_path = opts.initrd.clone();
        self.kernel_cmdline = opts.append.clone();
        self.loaders = opts.loaders.clone();

        let mut sysbus = SysBus::new("sysbus0");
        let mut root = MemoryRegion::container("system", u64::MAX);

        let (ram_region, ram_block) = MemoryRegion::ram("ram", opts.ram_size);
        root.add_subregion(ram_region, GPA::new(VIRT_RAM_BASE));
        root.add_subregion(
            MemoryRegion::io(
                "pci-cfg-empty",
                VIRT_PCI_CFG_SIZE,
                Arc::new(EmptyPciConfigMmio),
            ),
            GPA::new(VIRT_PCI_CFG_BASE),
        );
        root.add_subregion(
            MemoryRegion::io(
                "pci-ht-cfg-empty",
                VIRT_PCI_CFG_SIZE,
                Arc::new(EmptyPciConfigMmio),
            ),
            GPA::new(VIRT_PCI_HT_CFG_BASE),
        );
        root.add_subregion(
            MemoryRegion::io(
                "legacy-io-empty",
                VIRT_LEGACY_IO_SIZE,
                Arc::new(EmptyPciConfigMmio),
            ),
            GPA::new(VIRT_LEGACY_IO_BASE),
        );

        // Test-completion device: lets bare-metal ISA tests report
        // pass/fail by an MMIO write (loongarch64-ref has no other way
        // to terminate the run cleanly). The callback is installed by
        // the execution driver at run time.
        let test_finisher = LoongArchTestFinisher::new();
        root.add_subregion(
            MemoryRegion::io(
                "test-finisher",
                VIRT_TEST_FINISHER_SIZE,
                Arc::new(LoongArchTestFinisherMmio(Arc::clone(&test_finisher))),
            ),
            GPA::new(VIRT_TEST_FINISHER_BASE),
        );
        self.test_finisher = Some(test_finisher);

        let fw_cfg = FwCfg::new_named("fw_cfg0");
        let fw_cfg_cpu_count = u16::try_from(opts.cpu_count)
            .expect("LoongArch CPU count is validated for fw_cfg");
        fw_cfg.add_i16(keys::MAX_CPUS, fw_cfg_cpu_count);
        fw_cfg.add_i64(keys::RAM_SIZE, opts.ram_size);
        fw_cfg.add_i16(keys::NB_CPUS, fw_cfg_cpu_count);
        fw_cfg.attach_to_bus(&mut sysbus)?;
        let fw_cfg_region = MemoryRegion::io(
            "fw_cfg0",
            VIRT_FWCFG_SIZE,
            Arc::new(FwCfgMmio::new(Arc::clone(&fw_cfg))),
        );
        fw_cfg.register_mmio(fw_cfg_region, GPA::new(VIRT_FWCFG_BASE))?;

        let ram_ptr = ram_block.as_ptr() as usize;
        let mut cpus = Vec::with_capacity(opts.cpu_count as usize);
        for cpu_id in 0..opts.cpu_count {
            let cpu = Arc::new(Mutex::new(LoongArchCpu::new()));
            {
                let mut cpu_guard = cpu.lock().unwrap();
                cpu_guard.set_cpuid(u64::from(cpu_id));
                cpu_guard.set_guest_base(
                    ram_ptr.wrapping_sub(VIRT_RAM_BASE as usize) as u64,
                );
                cpu_guard.set_ram_base(VIRT_RAM_BASE);
                cpu_guard.set_ram_end(VIRT_RAM_BASE + opts.ram_size);
            }
            cpus.push(cpu);
        }

        let ipi = Arc::new(LoongArchIpi::new_named("ipi0", opts.cpu_count));
        for (cpu_id, cpu) in cpus.iter().enumerate() {
            ipi.connect_output(
                cpu_id as u32,
                InterruptSource::new(
                    Arc::new(LoongArchCpuIpiSink {
                        cpu: Arc::clone(cpu),
                    }) as Arc<dyn IrqSink>,
                    0,
                ),
            );
        }

        let eiointc = Arc::new(Eiointc::new_named("eiointc0", opts.cpu_count));
        let pch_pic = Arc::new(PchPic::new_named("pch-pic0", 32));
        let pch_msi = Arc::new(PchMsi::new_named(
            "pch-msi0",
            LOONGARCH_PCH_MSI_IRQ_BASE,
            LOONGARCH_PCH_MSI_IRQ_NUM,
        ));
        let cascade = LoongArchInterruptCascade::from_devices(
            Arc::clone(&pch_pic),
            Arc::clone(&eiointc),
        );
        for (cpu_id, cpu) in cpus.iter().enumerate() {
            cascade.connect_cpu_hwi(
                cpu_id as u32,
                LOONGARCH_DEVICE_HWI,
                Arc::clone(cpu),
            );
        }
        for irq in 0..LOONGARCH_PCH_MSI_IRQ_NUM {
            let eio_irq = LOONGARCH_PCH_MSI_IRQ_BASE + irq;
            pch_msi.connect_output(
                irq,
                InterruptSource::new(
                    Arc::new(EiointcIrqSink(Arc::clone(&eiointc)))
                        as Arc<dyn IrqSink>,
                    eio_irq,
                ),
            );
            cascade.route_eio_irq_to_cpu_hwi(eio_irq, 0, LOONGARCH_DEVICE_HWI);
        }

        let iocsr_bus =
            VirtIocsrBus::new(Arc::clone(&ipi), Arc::clone(&eiointc));
        for cpu in &cpus {
            iocsr_bus.install_on(&mut cpu.lock().unwrap());
        }

        Self::attach_interrupt_devices(
            &mut sysbus,
            &ipi,
            opts.cpu_count,
            &eiointc,
            &pch_pic,
            &pch_msi,
        )?;

        let rtc = Arc::new(Ls7aRtc::new_named("ls7a-rtc0"));
        rtc.attach_to_bus(&mut sysbus)?;
        rtc.register_mmio(
            MemoryRegion::io(
                "ls7a-rtc0-mmio",
                VIRT_RTC_SIZE,
                Arc::new(Ls7aRtcMmio(Arc::clone(&rtc))),
            ),
            GPA::new(VIRT_RTC_BASE),
        )?;
        rtc.connect_output(InterruptSource::new(
            Arc::new(PchPicIrqSink(Arc::clone(&pch_pic))) as Arc<dyn IrqSink>,
            LOONGARCH_RTC_PCH_IRQ,
        ));
        cascade.route_pch_irq_to_cpu_hwi(
            LOONGARCH_RTC_PCH_IRQ,
            LOONGARCH_RTC_PCH_IRQ,
            0,
            LOONGARCH_DEVICE_HWI,
        );

        let pflash0 = Self::create_pflash_bank(
            "pflash0",
            VIRT_FLASH0_BASE,
            VIRT_FLASH0_SIZE,
            &mut sysbus,
        )?;
        let pflash1 = Self::create_pflash_bank(
            "pflash1",
            VIRT_FLASH1_BASE,
            VIRT_FLASH1_SIZE,
            &mut sysbus,
        )?;

        let uart = Arc::new(Uart16550::new_named("uart0"));
        uart.attach_to_bus(&mut sysbus)?;
        uart.register_mmio(
            MemoryRegion::io(
                "uart0",
                VIRT_UART_SIZE,
                Arc::new(Uart16550Mmio(Arc::clone(&uart))),
            ),
            GPA::new(VIRT_UART_BASE),
        )?;
        cascade.attach_uart(&uart)?;
        cascade.route_pch_irq_to_cpu_hwi(
            LOONGARCH_UART_PCH_IRQ,
            LOONGARCH_UART_PCH_IRQ,
            0,
            LOONGARCH_DEVICE_HWI,
        );
        if let Some(frontend) = self.uart_chardev.take() {
            uart.attach_chardev(frontend)?;
        } else if opts.nographic {
            uart.attach_chardev(CharFrontend::new(Box::new(
                StdioChardev::new(),
            )))?;
        }

        let uart1 = Arc::new(Uart16550::new_named("uart1"));
        uart1.attach_to_bus(&mut sysbus)?;
        uart1.register_mmio(
            MemoryRegion::io(
                "uart1",
                VIRT_UART1_SIZE,
                Arc::new(Uart16550Mmio(Arc::clone(&uart1))),
            ),
            GPA::new(VIRT_UART1_BASE),
        )?;
        if let Some(frontend) = self.uart1_chardev.take() {
            uart1.attach_chardev(frontend)?;
        } else if opts.nographic {
            uart1.attach_chardev(CharFrontend::new(Box::new(
                StdoutChardev::new(),
            )))?;
        }

        let mut virtio_mmio = if let Some(drive_path) = &opts.drive {
            use machina_hw_virtio::block::VirtioBlk;

            let blk = VirtioBlk::open(drive_path)?;
            cascade.route_pch_irq_to_cpu_hwi(
                LOONGARCH_VIRTIO_PCH_IRQ_BASE,
                LOONGARCH_VIRTIO_PCH_IRQ_BASE,
                0,
                LOONGARCH_DEVICE_HWI,
            );
            let mut mmio = VirtioMmio::new_named(
                "virtio-mmio0",
                Box::new(blk),
                cascade.virtio_irq_line(0),
                ram_block.as_ptr(),
                // Linux programs virtio descriptors with low physical
                // addresses from the memory@0 FDT node, not VIRT_RAM_BASE.
                0,
                opts.ram_size,
            );
            mmio.attach_to_bus(&mut sysbus)?;
            let region =
                mmio.make_mmio_region("virtio-mmio0", VIRT_VIRTIO_SIZE);
            mmio.register_mmio(region, GPA::new(VIRT_VIRTIO_BASE))?;
            Some(mmio)
        } else {
            None
        };

        let mut address_space = AddressSpace::new(root);
        Self::realize_interrupt_devices(
            &mut sysbus,
            &mut address_space,
            &ipi,
            &eiointc,
            &pch_pic,
            &pch_msi,
        )?;

        rtc.realize_onto(&mut sysbus, &mut address_space)?;
        fw_cfg.realize_onto(&mut sysbus, &mut address_space)?;
        pflash0.realize_onto(&mut sysbus, &mut address_space)?;
        pflash1.realize_onto(&mut sysbus, &mut address_space)?;

        if let Some(mmio) = virtio_mmio.as_mut() {
            mmio.realize_onto(&mut sysbus, &mut address_space)?;
        }

        let uart_for_rx = Arc::clone(&uart);
        let rx_cb: UartRxCallback = Arc::new(Mutex::new(move |byte: u8| {
            uart_for_rx.receive(byte);
        }));
        uart.realize_onto(&mut sysbus, &mut address_space, rx_cb)?;
        let uart1_for_rx = Arc::clone(&uart1);
        let rx_cb: UartRxCallback = Arc::new(Mutex::new(move |byte: u8| {
            uart1_for_rx.receive(byte);
        }));
        uart1.realize_onto(&mut sysbus, &mut address_space, rx_cb)?;

        self.cpus = cpus;
        self.address_space = Some(address_space);
        self.sysbus = Some(sysbus);
        self.ram_block = Some(ram_block);
        self.uart = Some(uart);
        self.uart1 = Some(uart1);
        self.ipi = Some(ipi);
        self.pch_msi = Some(pch_msi);
        self.rtc = Some(rtc);
        self.pflash0 = Some(pflash0);
        self.pflash1 = Some(pflash1);
        self.interrupt_cascade = Some(cascade);
        self.iocsr_bus = Some(iocsr_bus);
        self.virtio_mmio = virtio_mmio;
        self.fw_cfg = Some(fw_cfg);

        Ok(())
    }

    fn reset(&mut self) {
        if let Some(uart) = &self.uart {
            uart.reset_runtime();
        }
        if let Some(uart1) = &self.uart1 {
            uart1.reset_runtime();
        }
        if let Some(pch_msi) = &self.pch_msi {
            pch_msi.reset_runtime();
        }
        if let Some(rtc) = &self.rtc {
            rtc.reset_runtime();
        }
        if let Some(fw_cfg) = &self.fw_cfg {
            fw_cfg.reset_runtime();
        }
        if let Some(pflash0) = &self.pflash0 {
            pflash0.reset_runtime();
        }
        if let Some(pflash1) = &self.pflash1 {
            pflash1.reset_runtime();
        }
        if let Some(virtio_mmio) = &mut self.virtio_mmio {
            virtio_mmio.reset_runtime();
        }
    }

    fn pause(&mut self) {}

    fn resume(&mut self) {}

    fn shutdown(&mut self) {}

    fn boot(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let kernel_path = self
            .kernel_path
            .as_ref()
            .ok_or("loongarch64-ref boot requires a kernel image")?;
        let boot_config = boot::DirectKernelBootConfig {
            cmdline: self.kernel_cmdline.as_deref(),
            initrd_path: self.initrd_path.as_deref(),
            cpu_count: self.cpus.len() as u32,
            has_virtio_mmio: self.virtio_mmio.is_some(),
            fw_cfg: self.fw_cfg.as_ref().map(Arc::clone),
        };
        let boot_info = boot::load_direct_kernel(
            kernel_path,
            &boot_config,
            self.ram_size,
            self.address_space(),
        )?;
        self.apply_loaders()?;

        let cpu = self.cpu();
        let mut cpu = cpu.lock().unwrap();
        cpu.set_pc(boot_info.entry);
        cpu.csr_write(CSR_CRMD, CRMD_DA);
        cpu.write_gpr(4, boot_info.efi_boot);
        cpu.write_gpr(5, boot_info.cmdline_addr);
        cpu.write_gpr(6, boot_info.system_table_addr);
        drop(cpu);

        if self.cpus.len() > 1 {
            let secondary_entry = boot::install_secondary_boot_code(
                self.ram_size,
                self.address_space(),
            )?;
            for cpu_id in 1..self.cpus.len() {
                let cpu = self.cpu_at(cpu_id);
                let mut cpu = cpu.lock().unwrap();
                cpu.set_pc(secondary_entry);
                cpu.csr_write(CSR_CRMD, CRMD_DA);
                cpu.write_gpr(5, boot_info.cmdline_addr);
            }
        }
        Ok(())
    }

    fn cpu_count(&self) -> usize {
        self.cpus.len()
    }

    fn ram_size(&self) -> u64 {
        self.ram_size
    }
}

struct LoongArchCpuIpiSink {
    cpu: Arc<Mutex<LoongArchCpu>>,
}

impl IrqSink for LoongArchCpuIpiSink {
    fn set_irq(&self, _irq: u32, level: bool) {
        self.cpu.lock().unwrap().set_ipi_interrupt_pending(level);
    }
}

struct LoongArchCpuAsyncIpiSink {
    interrupts: Arc<LoongArchCpuInterruptState>,
}

impl IrqSink for LoongArchCpuAsyncIpiSink {
    fn set_irq(&self, _irq: u32, level: bool) {
        self.interrupts.set_ipi_interrupt_pending(level);
    }
}

fn runtime_ipi_source(
    interrupts: Arc<LoongArchCpuInterruptState>,
) -> InterruptSource {
    InterruptSource::new(
        Arc::new(LoongArchCpuAsyncIpiSink { interrupts }) as Arc<dyn IrqSink>,
        0,
    )
}
