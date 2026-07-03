// K230 machine skeleton compatible with the Kendryte SDK memory map.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use machina_core::address::GPA;
use machina_core::machine::{LoaderSpec, Machine, MachineOpts, MachineState};
use machina_core::mobject::{MObject, MObjectInfo, MObjectTree};
use machina_core::wfi::WfiWaker;
use machina_guest_riscv::riscv::cpu::RiscvCpu;
use machina_guest_riscv::riscv::cpu_model::RiscvCpuModel;
use machina_hw_char::uart::{Uart16550, Uart16550ShiftedMmio};
use machina_hw_core::bus::SysBus;
use machina_hw_core::chardev::{
    ByteCb, CharFrontend, Chardev, NullChardev, StdioChardev,
};
use machina_hw_core::irq::{InterruptSource, IrqLine, IrqSink};
use machina_hw_intc::aclint::{Aclint, AclintMmio};
use machina_hw_intc::plic::{Plic, PlicIrqSink, PlicMmio};
use machina_hw_misc::unimp::{Unimp, UnimpMmio};
use machina_hw_sd::card::{SdCardConfig, SdMemoryCard};
use machina_hw_sd::sdhci::{Sdhci, SdhciMmio};
use machina_hw_sd::{SdBus, SdCard};
use machina_hw_storage::{BlockMedia, FileBackend};
use machina_hw_watchdog::k230::{K230Wdt, K230WdtMmio, MMIO_SIZE};
use machina_memory::address_space::AddressSpace;
use machina_memory::ram::RamBlock;
use machina_memory::region::MemoryRegion;

use crate::k230_gzip_dma::{
    K230GzipDma, K230GzipDmaMmio, K230_GZIP_DMA_MMIO_SIZE,
};
use crate::k230_kpu::{K230Kpu, K230KpuCfgMmio, K230KpuL2Mmio};
use crate::k230_pufs::{K230Pufs, K230PufsMmio, K230_PUFS_MMIO_SIZE};

#[derive(Clone, Copy)]
pub struct MemMapEntry {
    pub base: u64,
    pub size: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum K230MemMap {
    Ddr = 0,
    KpuL2Cache,
    Sram,
    KpuCfg,
    Fft,
    Ai2d,
    Gsdma,
    Dma,
    Gzip,
    NonAi2d,
    Isp,
    Dewarp,
    RxCsi,
    H264,
    Gpu2p5d,
    Vo,
    VoCfg,
    Engine3d,
    Pmu,
    Rtc,
    Cmu,
    Rmu,
    Boot,
    Pwr,
    Mailbox,
    Iomux,
    Timer,
    Wdt0,
    Wdt1,
    Ts,
    Hdi,
    Stc,
    Bootrom,
    Security,
    Noc,
    Uart0,
    Uart1,
    Uart2,
    Uart3,
    Uart4,
    I2c0,
    I2c1,
    I2c2,
    I2c3,
    I2c4,
    Pwm,
    Gpio0,
    Gpio1,
    Adc,
    Codec,
    I2s,
    Usb0,
    Usb1,
    Sd0,
    Sd1,
    Qspi0,
    Qspi1,
    Spi,
    HiSysCfg,
    DdrcCfg,
    Flash,
    Plic,
    Clint,
    Count,
}

pub const K230_MEMMAP: [MemMapEntry; K230MemMap::Count as usize] = {
    let mut m = [MemMapEntry { base: 0, size: 0 }; K230MemMap::Count as usize];
    m[K230MemMap::Ddr as usize] = MemMapEntry {
        base: 0x0000_0000,
        size: 0x8000_0000,
    };
    m[K230MemMap::KpuL2Cache as usize] = MemMapEntry {
        base: 0x8000_0000,
        size: 0x0020_0000,
    };
    m[K230MemMap::Sram as usize] = MemMapEntry {
        base: 0x8020_0000,
        size: 0x0020_0000,
    };
    m[K230MemMap::KpuCfg as usize] = MemMapEntry {
        base: 0x8040_0000,
        size: 0x0000_0800,
    };
    m[K230MemMap::Fft as usize] = MemMapEntry {
        base: 0x8040_0800,
        size: 0x0000_0400,
    };
    m[K230MemMap::Ai2d as usize] = MemMapEntry {
        base: 0x8040_0c00,
        size: 0x0000_0800,
    };
    m[K230MemMap::Gsdma as usize] = MemMapEntry {
        base: 0x8080_0000,
        size: 0x0000_4000,
    };
    m[K230MemMap::Dma as usize] = MemMapEntry {
        base: 0x8080_4000,
        size: 0x0000_4000,
    };
    m[K230MemMap::Gzip as usize] = MemMapEntry {
        base: 0x8080_8000,
        size: 0x0000_4000,
    };
    m[K230MemMap::NonAi2d as usize] = MemMapEntry {
        base: 0x8080_c000,
        size: 0x0000_4000,
    };
    m[K230MemMap::Isp as usize] = MemMapEntry {
        base: 0x9000_0000,
        size: 0x0000_8000,
    };
    m[K230MemMap::Dewarp as usize] = MemMapEntry {
        base: 0x9000_8000,
        size: 0x0000_1000,
    };
    m[K230MemMap::RxCsi as usize] = MemMapEntry {
        base: 0x9000_9000,
        size: 0x0000_2000,
    };
    m[K230MemMap::H264 as usize] = MemMapEntry {
        base: 0x9040_0000,
        size: 0x0001_0000,
    };
    m[K230MemMap::Gpu2p5d as usize] = MemMapEntry {
        base: 0x9080_0000,
        size: 0x0004_0000,
    };
    m[K230MemMap::Vo as usize] = MemMapEntry {
        base: 0x9084_0000,
        size: 0x0001_0000,
    };
    m[K230MemMap::VoCfg as usize] = MemMapEntry {
        base: 0x9085_0000,
        size: 0x0000_1000,
    };
    m[K230MemMap::Engine3d as usize] = MemMapEntry {
        base: 0x90a0_0000,
        size: 0x0000_0800,
    };
    m[K230MemMap::Pmu as usize] = MemMapEntry {
        base: 0x9100_0000,
        size: 0x0000_0c00,
    };
    m[K230MemMap::Rtc as usize] = MemMapEntry {
        base: 0x9100_0c00,
        size: 0x0000_0400,
    };
    m[K230MemMap::Cmu as usize] = MemMapEntry {
        base: 0x9110_0000,
        size: 0x0000_1000,
    };
    m[K230MemMap::Rmu as usize] = MemMapEntry {
        base: 0x9110_1000,
        size: 0x0000_1000,
    };
    m[K230MemMap::Boot as usize] = MemMapEntry {
        base: 0x9110_2000,
        size: 0x0000_1000,
    };
    m[K230MemMap::Pwr as usize] = MemMapEntry {
        base: 0x9110_3000,
        size: 0x0000_1000,
    };
    m[K230MemMap::Mailbox as usize] = MemMapEntry {
        base: 0x9110_4000,
        size: 0x0000_1000,
    };
    m[K230MemMap::Iomux as usize] = MemMapEntry {
        base: 0x9110_5000,
        size: 0x0000_0800,
    };
    m[K230MemMap::Timer as usize] = MemMapEntry {
        base: 0x9110_5800,
        size: 0x0000_0800,
    };
    m[K230MemMap::Wdt0 as usize] = MemMapEntry {
        base: 0x9110_6000,
        size: 0x0000_0800,
    };
    m[K230MemMap::Wdt1 as usize] = MemMapEntry {
        base: 0x9110_6800,
        size: 0x0000_0800,
    };
    m[K230MemMap::Ts as usize] = MemMapEntry {
        base: 0x9110_7000,
        size: 0x0000_0800,
    };
    m[K230MemMap::Hdi as usize] = MemMapEntry {
        base: 0x9110_7800,
        size: 0x0000_0800,
    };
    m[K230MemMap::Stc as usize] = MemMapEntry {
        base: 0x9110_8000,
        size: 0x0000_0800,
    };
    m[K230MemMap::Bootrom as usize] = MemMapEntry {
        base: 0x9120_0000,
        size: 0x0001_0000,
    };
    m[K230MemMap::Security as usize] = MemMapEntry {
        base: 0x9121_0000,
        size: 0x0000_8000,
    };
    m[K230MemMap::Noc as usize] = MemMapEntry {
        base: 0x9130_0000,
        size: 0x0000_4000,
    };
    m[K230MemMap::Uart0 as usize] = MemMapEntry {
        base: 0x9140_0000,
        size: 0x0000_1000,
    };
    m[K230MemMap::Uart1 as usize] = MemMapEntry {
        base: 0x9140_1000,
        size: 0x0000_1000,
    };
    m[K230MemMap::Uart2 as usize] = MemMapEntry {
        base: 0x9140_2000,
        size: 0x0000_1000,
    };
    m[K230MemMap::Uart3 as usize] = MemMapEntry {
        base: 0x9140_3000,
        size: 0x0000_1000,
    };
    m[K230MemMap::Uart4 as usize] = MemMapEntry {
        base: 0x9140_4000,
        size: 0x0000_1000,
    };
    m[K230MemMap::I2c0 as usize] = MemMapEntry {
        base: 0x9140_5000,
        size: 0x0000_1000,
    };
    m[K230MemMap::I2c1 as usize] = MemMapEntry {
        base: 0x9140_6000,
        size: 0x0000_1000,
    };
    m[K230MemMap::I2c2 as usize] = MemMapEntry {
        base: 0x9140_7000,
        size: 0x0000_1000,
    };
    m[K230MemMap::I2c3 as usize] = MemMapEntry {
        base: 0x9140_8000,
        size: 0x0000_1000,
    };
    m[K230MemMap::I2c4 as usize] = MemMapEntry {
        base: 0x9140_9000,
        size: 0x0000_1000,
    };
    m[K230MemMap::Pwm as usize] = MemMapEntry {
        base: 0x9140_a000,
        size: 0x0000_1000,
    };
    m[K230MemMap::Gpio0 as usize] = MemMapEntry {
        base: 0x9140_b000,
        size: 0x0000_1000,
    };
    m[K230MemMap::Gpio1 as usize] = MemMapEntry {
        base: 0x9140_c000,
        size: 0x0000_1000,
    };
    m[K230MemMap::Adc as usize] = MemMapEntry {
        base: 0x9140_d000,
        size: 0x0000_1000,
    };
    m[K230MemMap::Codec as usize] = MemMapEntry {
        base: 0x9140_e000,
        size: 0x0000_1000,
    };
    m[K230MemMap::I2s as usize] = MemMapEntry {
        base: 0x9140_f000,
        size: 0x0000_1000,
    };
    m[K230MemMap::Usb0 as usize] = MemMapEntry {
        base: 0x9150_0000,
        size: 0x0001_0000,
    };
    m[K230MemMap::Usb1 as usize] = MemMapEntry {
        base: 0x9154_0000,
        size: 0x0001_0000,
    };
    m[K230MemMap::Sd0 as usize] = MemMapEntry {
        base: 0x9158_0000,
        size: 0x0000_1000,
    };
    m[K230MemMap::Sd1 as usize] = MemMapEntry {
        base: 0x9158_1000,
        size: 0x0000_1000,
    };
    m[K230MemMap::Qspi0 as usize] = MemMapEntry {
        base: 0x9158_2000,
        size: 0x0000_1000,
    };
    m[K230MemMap::Qspi1 as usize] = MemMapEntry {
        base: 0x9158_3000,
        size: 0x0000_1000,
    };
    m[K230MemMap::Spi as usize] = MemMapEntry {
        base: 0x9158_4000,
        size: 0x0000_1000,
    };
    m[K230MemMap::HiSysCfg as usize] = MemMapEntry {
        base: 0x9158_5000,
        size: 0x0000_0400,
    };
    m[K230MemMap::DdrcCfg as usize] = MemMapEntry {
        base: 0x9800_0000,
        size: 0x0200_0000,
    };
    m[K230MemMap::Flash as usize] = MemMapEntry {
        base: 0xc000_0000,
        size: 0x0800_0000,
    };
    m[K230MemMap::Plic as usize] = MemMapEntry {
        base: 0x000f_0000_0000,
        size: 0x0040_0000,
    };
    m[K230MemMap::Clint as usize] = MemMapEntry {
        base: 0x000f_0400_0000,
        size: 0x0040_0000,
    };
    m
};

pub const K230_PLIC_NUM_SOURCES: u32 = 208;
pub const K230_PLIC_NUM_PRIORITIES: u32 = 7;
pub const K230_UART_COUNT: usize = 5;

pub struct K230IrqMap;

impl K230IrqMap {
    pub const UART0: u32 = 16;
    pub const UART1: u32 = 17;
    pub const UART2: u32 = 18;
    pub const UART3: u32 = 19;
    pub const UART4: u32 = 20;
    pub const WDT0: u32 = 107;
    pub const WDT1: u32 = 108;
    pub const SD0: u32 = 0x8e;
    pub const SD1: u32 = 0x90;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum K230WdtIndex {
    Wdt0,
    Wdt1,
}

const IRQ_MSI: u32 = 3;
const IRQ_MTI: u32 = 7;
const IRQ_MEI: u32 = 11;
const IRQ_SEI: u32 = 9;

pub struct K230Machine {
    name: String,
    machine_state: MachineState,
    mom_tree: MObjectTree,
    ram_size: u64,
    cpu_count: u32,
    address_space: Option<Arc<AddressSpace>>,
    sysbus: Option<SysBus>,
    ram_block: Option<Arc<RamBlock>>,
    sram_block: Option<Arc<RamBlock>>,
    bootrom_block: Option<Arc<RamBlock>>,
    plic: Option<Arc<Plic>>,
    aclint: Option<Arc<Aclint>>,
    uarts: Vec<Arc<Uart16550>>,
    wdts: Vec<Arc<K230Wdt>>,
    sdhcis: Vec<Arc<Sdhci>>,
    gzip_dma: Option<Arc<K230GzipDma>>,
    pufs: Option<Arc<K230Pufs>>,
    kpu: Option<Arc<K230Kpu>>,
    unimp: Vec<Arc<Unimp>>,
    pub(crate) cpus: Arc<Mutex<Vec<Option<RiscvCpu>>>>,
    pub(crate) shared_mip: Arc<AtomicU64>,
    pub(crate) wfi_waker: Arc<WfiWaker>,
    pub(crate) bios_path: Option<PathBuf>,
    pub(crate) bios_builtin: bool,
    pub(crate) kernel_path: Option<PathBuf>,
    pub(crate) initrd_path: Option<PathBuf>,
    pub(crate) dtb_path: Option<PathBuf>,
    pub(crate) loaders: Vec<LoaderSpec>,
    dtb_blob: Option<Vec<u8>>,
    pub(crate) kernel_cmdline: Option<String>,
    quit_cb: Option<Arc<dyn Fn() + Send + Sync>>,
    monitor_cb: Option<ByteCb>,
}

impl K230Machine {
    pub fn new() -> Self {
        let machine_state = MachineState::new_root("machine");
        let mom_tree = Self::new_mom_tree(&machine_state);
        Self {
            name: "k230".to_string(),
            machine_state,
            mom_tree,
            ram_size: 0,
            cpu_count: 0,
            address_space: None,
            sysbus: None,
            ram_block: None,
            sram_block: None,
            bootrom_block: None,
            plic: None,
            aclint: None,
            uarts: Vec::new(),
            wdts: Vec::new(),
            sdhcis: Vec::new(),
            gzip_dma: None,
            pufs: None,
            kpu: None,
            unimp: Vec::new(),
            cpus: Arc::new(Mutex::new(Vec::new())),
            shared_mip: Arc::new(AtomicU64::new(0)),
            wfi_waker: Arc::new(WfiWaker::new()),
            bios_path: None,
            bios_builtin: false,
            kernel_path: None,
            initrd_path: None,
            dtb_path: None,
            loaders: Vec::new(),
            dtb_blob: None,
            kernel_cmdline: None,
            quit_cb: None,
            monitor_cb: None,
        }
    }

    pub fn address_space(&self) -> &AddressSpace {
        self.address_space
            .as_deref()
            .expect("machine not initialized")
    }

    pub fn sysbus(&self) -> &SysBus {
        self.sysbus.as_ref().expect("machine not initialized")
    }

    pub fn plic(&self) -> &Arc<Plic> {
        self.plic.as_ref().expect("machine not initialized")
    }

    pub fn aclint(&self) -> &Arc<Aclint> {
        self.aclint.as_ref().expect("machine not initialized")
    }

    pub fn uart(&self, index: usize) -> Option<&Arc<Uart16550>> {
        self.uarts.get(index)
    }

    pub fn wdt(&self, index: K230WdtIndex) -> Option<&Arc<K230Wdt>> {
        match index {
            K230WdtIndex::Wdt0 => self.wdts.first(),
            K230WdtIndex::Wdt1 => self.wdts.get(1),
        }
    }

    pub fn set_quit_cb(&mut self, cb: Arc<dyn Fn() + Send + Sync>) {
        self.quit_cb = Some(cb);
    }

    pub fn set_monitor_cb(&mut self, cb: ByteCb) {
        self.monitor_cb = Some(cb);
    }

    pub fn cpus_lock(&self) -> MutexGuard<'_, Vec<Option<RiscvCpu>>> {
        self.cpus.lock().unwrap()
    }

    pub fn take_cpu(&self, idx: usize) -> Option<RiscvCpu> {
        let mut lock = self.cpus.lock().unwrap();
        lock.get_mut(idx).and_then(|slot| slot.take())
    }

    pub fn shared_mip(&self) -> Arc<AtomicU64> {
        self.shared_mip.clone()
    }

    pub fn wfi_waker(&self) -> Arc<WfiWaker> {
        self.wfi_waker.clone()
    }

    pub fn ram_block(&self) -> &Arc<RamBlock> {
        self.ram_block.as_ref().expect("machine not initialized")
    }

    pub fn bootrom_block(&self) -> &Arc<RamBlock> {
        self.bootrom_block
            .as_ref()
            .expect("machine not initialized")
    }

    pub fn bios_path(&self) -> Option<&PathBuf> {
        self.bios_path.as_ref()
    }

    pub fn kernel_path(&self) -> Option<&PathBuf> {
        self.kernel_path.as_ref()
    }

    pub fn initrd_path(&self) -> Option<&PathBuf> {
        self.initrd_path.as_ref()
    }

    pub fn dtb_path(&self) -> Option<&PathBuf> {
        self.dtb_path.as_ref()
    }

    pub fn dtb_blob(&self) -> Option<&[u8]> {
        self.dtb_blob.as_deref()
    }

    pub fn loaders(&self) -> &[LoaderSpec] {
        &self.loaders
    }

    pub fn kernel_cmdline(&self) -> Option<&str> {
        self.kernel_cmdline.as_deref()
    }

    pub fn set_dtb_blob(&mut self, blob: Vec<u8>) {
        self.dtb_blob = Some(blob);
    }

    pub fn set_boot_cpu_pc(
        &self,
        pc: u64,
        priv_level: machina_guest_riscv::riscv::csr::PrivLevel,
    ) {
        let mut cpus = self.cpus_lock();
        if let Some(Some(cpu)) = cpus.get_mut(0) {
            cpu.pc = pc;
            cpu.set_priv(priv_level);
        }
    }

    pub fn ram_ptr(&self) -> *const u8 {
        self.ram_block().as_ptr() as *const u8
    }

    pub fn write_ram(
        &self,
        offset: u64,
        data: &[u8],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let block = self.ram_block();
        let end = offset + data.len() as u64;
        if end > block.size() {
            return Err(format!(
                "write_ram: offset {offset:#x} + len {:#x} exceeds RAM size {:#x}",
                data.len(),
                block.size()
            )
            .into());
        }
        unsafe {
            let dst = block.as_ptr().add(offset as usize);
            std::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
        }
        Ok(())
    }

    pub fn read_ram_bytes(
        &self,
        gpa: u64,
        len: usize,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let base = K230_MEMMAP[K230MemMap::Ddr as usize].base;
        let offset = gpa.checked_sub(base).ok_or("address below K230 DDR")?;
        let block = self.ram_block();
        if offset + len as u64 > block.size() {
            return Err("read exceeds K230 DDR".into());
        }
        let mut out = vec![0u8; len];
        unsafe {
            std::ptr::copy_nonoverlapping(
                block.as_ptr().add(offset as usize),
                out.as_mut_ptr(),
                len,
            );
        }
        Ok(out)
    }

    pub fn mom_object_infos(&self) -> Vec<MObjectInfo> {
        self.mom_tree.infos().cloned().collect()
    }

    fn new_mom_tree(machine_state: &MachineState) -> MObjectTree {
        let mut tree = MObjectTree::default();
        tree.track_root(machine_state.object())
            .expect("machine root must have an object path");
        tree
    }

    fn refresh_mom_tree(&mut self) {
        let mut tree = Self::new_mom_tree(&self.machine_state);
        if let Some(sysbus) = &self.sysbus {
            Self::track_mom_info(&mut tree, sysbus.object_info());
        }
        if let Some(plic) = &self.plic {
            Self::track_mom_info(&mut tree, plic.object_info());
        }
        if let Some(aclint) = &self.aclint {
            Self::track_mom_info(&mut tree, aclint.object_info());
        }
        for uart in &self.uarts {
            Self::track_mom_info(&mut tree, uart.object_info());
        }
        for wdt in &self.wdts {
            Self::track_mom_info(&mut tree, wdt.object_info());
        }
        for sdhci in &self.sdhcis {
            Self::track_mom_info(&mut tree, sdhci.object_info());
        }
        if let Some(gzip_dma) = &self.gzip_dma {
            Self::track_mom_info(&mut tree, gzip_dma.object_info());
        }
        if let Some(pufs) = &self.pufs {
            Self::track_mom_info(&mut tree, pufs.object_info());
        }
        if let Some(kpu) = &self.kpu {
            Self::track_mom_info(&mut tree, kpu.object_info());
        }
        for dev in &self.unimp {
            Self::track_mom_info(
                &mut tree,
                dev.with_mdevice(|device| device.object_info()),
            );
        }
        self.mom_tree = tree;
    }

    fn track_mom_info(tree: &mut MObjectTree, info: MObjectInfo) {
        if info.object_path.is_none() {
            return;
        }
        tree.track_info(info)
            .expect("attached MOM object must have a path");
    }

    fn map_unimp(
        sysbus: &mut SysBus,
        name: &str,
        entry: MemMapEntry,
    ) -> Result<Arc<Unimp>, Box<dyn std::error::Error>> {
        let dev = Unimp::new(name, entry.size);
        dev.attach_to_bus(sysbus)?;
        let region = MemoryRegion::io(
            name,
            entry.size,
            Arc::new(UnimpMmio(Arc::clone(&dev))),
        );
        dev.register_mmio(region, GPA::new(entry.base))?;
        Ok(dev)
    }

    fn map_gzip_dma(
        sysbus: &mut SysBus,
    ) -> Result<Arc<K230GzipDma>, Box<dyn std::error::Error>> {
        let dev = K230GzipDma::new_named("k230-gzip-dma");
        dev.attach_to_bus(sysbus)?;
        let entry = K230_MEMMAP[K230MemMap::Gsdma as usize];
        let region = MemoryRegion::io(
            "k230-gzip-dma",
            K230_GZIP_DMA_MMIO_SIZE,
            Arc::new(K230GzipDmaMmio(Arc::clone(&dev))),
        );
        dev.register_mmio(region, GPA::new(entry.base))?;
        Ok(dev)
    }

    fn map_pufs(
        sysbus: &mut SysBus,
    ) -> Result<Arc<K230Pufs>, Box<dyn std::error::Error>> {
        let dev = K230Pufs::new_named("k230-pufs");
        dev.attach_to_bus(sysbus)?;
        let entry = K230_MEMMAP[K230MemMap::Security as usize];
        let region = MemoryRegion::io(
            "k230-pufs",
            K230_PUFS_MMIO_SIZE,
            Arc::new(K230PufsMmio(Arc::clone(&dev))),
        );
        dev.register_mmio(region, GPA::new(entry.base))?;
        Ok(dev)
    }

    fn map_kpu(
        sysbus: &mut SysBus,
    ) -> Result<Arc<K230Kpu>, Box<dyn std::error::Error>> {
        let dev = K230Kpu::new_named("k230-kpu");
        dev.attach_to_bus(sysbus)?;
        let cfg = K230_MEMMAP[K230MemMap::KpuCfg as usize];
        let cfg_region = MemoryRegion::io(
            "k230-kpu-cfg",
            cfg.size,
            Arc::new(K230KpuCfgMmio(Arc::clone(&dev))),
        );
        dev.register_mmio(cfg_region, GPA::new(cfg.base))?;
        let l2 = K230_MEMMAP[K230MemMap::KpuL2Cache as usize];
        let l2_region = MemoryRegion::io(
            "k230-kpu-l2-cache",
            l2.size,
            Arc::new(K230KpuL2Mmio(Arc::clone(&dev))),
        );
        dev.register_mmio(l2_region, GPA::new(l2.base))?;
        Ok(dev)
    }

    fn map_sdhci(
        sysbus: &mut SysBus,
        name: &str,
        entry: MemMapEntry,
    ) -> Result<Arc<Sdhci>, Box<dyn std::error::Error>> {
        let dev = Arc::new(Sdhci::new_named(name));
        dev.attach_to_bus(sysbus)?;
        let region = MemoryRegion::io(
            name,
            entry.size,
            Arc::new(SdhciMmio(Arc::clone(&dev))),
        );
        dev.register_mmio(region, GPA::new(entry.base))?;
        Ok(dev)
    }

    fn attach_drive_to_sd1(
        drive: &Option<PathBuf>,
        buses: &[Arc<SdBus>],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let Some(path) = drive else {
            return Ok(());
        };
        let backend = FileBackend::open(path.clone(), false)?;
        let media = BlockMedia::new(backend, 512)?;
        let card = Arc::new(SdMemoryCard::new_named(
            "k230-sd1-card",
            media,
            SdCardConfig::default(),
        )?);
        let card_for_bus: Arc<dyn SdCard> = card;
        buses[1].insert_card(card_for_bus);
        Ok(())
    }

    fn plic_irq_line(plic: &Arc<Plic>, source: u32) -> IrqLine {
        let sink = Arc::new(PlicIrqSink(Arc::clone(plic)));
        IrqLine::new(sink as Arc<dyn IrqSink>, source)
    }

    fn plic_irq_source(plic: &Arc<Plic>, source: u32) -> InterruptSource {
        let sink = Arc::new(PlicIrqSink(Arc::clone(plic)));
        InterruptSource::new(sink as Arc<dyn IrqSink>, source)
    }

    fn connect_cpu_interrupts(
        plic: &Arc<Plic>,
        aclint: &Arc<Aclint>,
        cpu_count: u32,
        shared_mip: &Arc<AtomicU64>,
        wfi_waker: &Arc<WfiWaker>,
    ) {
        for hart in 0..cpu_count as usize {
            let mei_sink: Arc<dyn IrqSink> = Arc::new(RiscvCpuIrqSink::new(
                shared_mip.clone(),
                wfi_waker.clone(),
            ));
            plic.connect_context_output(
                (2 * hart) as u32,
                InterruptSource::new(mei_sink, IRQ_MEI),
            );

            let sei_sink: Arc<dyn IrqSink> = Arc::new(RiscvCpuIrqSink::new(
                shared_mip.clone(),
                wfi_waker.clone(),
            ));
            plic.connect_context_output(
                (2 * hart + 1) as u32,
                InterruptSource::new(sei_sink, IRQ_SEI),
            );
        }

        aclint.connect_wfi_waker(wfi_waker.clone());
        for hart in 0..cpu_count as usize {
            let mti_sink = Arc::new(RiscvCpuIrqSink::new(
                shared_mip.clone(),
                wfi_waker.clone(),
            ));
            aclint.connect_mti(
                hart as u32,
                IrqLine::new(mti_sink as Arc<dyn IrqSink>, IRQ_MTI),
            );
            let msi_sink = Arc::new(RiscvCpuIrqSink::new(
                shared_mip.clone(),
                wfi_waker.clone(),
            ));
            aclint.connect_msi(
                hart as u32,
                IrqLine::new(msi_sink as Arc<dyn IrqSink>, IRQ_MSI),
            );
        }
    }

    fn unimp_specs() -> &'static [(K230MemMap, &'static str)] {
        &[
            (K230MemMap::Fft, "fft"),
            (K230MemMap::Ai2d, "2d-engine.ai"),
            (K230MemMap::NonAi2d, "2d-engine.non-ai"),
            (K230MemMap::Isp, "isp"),
            (K230MemMap::Dewarp, "dewarp"),
            (K230MemMap::RxCsi, "rx-csi"),
            (K230MemMap::H264, "vpu"),
            (K230MemMap::Gpu2p5d, "gpu"),
            (K230MemMap::Vo, "vo"),
            (K230MemMap::VoCfg, "vo_cfg"),
            (K230MemMap::Engine3d, "3d-engine"),
            (K230MemMap::Pmu, "pmu"),
            (K230MemMap::Rtc, "rtc"),
            (K230MemMap::Cmu, "cmu"),
            (K230MemMap::Rmu, "rmu"),
            (K230MemMap::Boot, "boot"),
            (K230MemMap::Pwr, "pwr"),
            (K230MemMap::Mailbox, "ipcm"),
            (K230MemMap::Iomux, "iomux"),
            (K230MemMap::Timer, "timer"),
            (K230MemMap::Ts, "ts"),
            (K230MemMap::Hdi, "hdi"),
            (K230MemMap::Stc, "stc"),
            (K230MemMap::Noc, "noc"),
            (K230MemMap::I2c0, "i2c0"),
            (K230MemMap::I2c1, "i2c1"),
            (K230MemMap::I2c2, "i2c2"),
            (K230MemMap::I2c3, "i2c3"),
            (K230MemMap::I2c4, "i2c4"),
            (K230MemMap::Pwm, "pwm"),
            (K230MemMap::Gpio0, "gpio0"),
            (K230MemMap::Gpio1, "gpio1"),
            (K230MemMap::Adc, "adc"),
            (K230MemMap::Codec, "codec"),
            (K230MemMap::I2s, "i2s"),
            (K230MemMap::Usb0, "usb0"),
            (K230MemMap::Usb1, "usb1"),
            (K230MemMap::Qspi0, "qspi0"),
            (K230MemMap::Qspi1, "qspi1"),
            (K230MemMap::Spi, "spi"),
            (K230MemMap::HiSysCfg, "hi_sys_cfg"),
            (K230MemMap::DdrcCfg, "ddrc_cfg"),
            (K230MemMap::Flash, "flash"),
        ]
    }
}

impl Default for K230Machine {
    fn default() -> Self {
        Self::new()
    }
}

impl Machine for K230Machine {
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
        let ddr = K230_MEMMAP[K230MemMap::Ddr as usize];
        if opts.ram_size != ddr.size {
            return Err(format!(
                "K230 RAM size must be exactly {:#x} bytes",
                ddr.size
            )
            .into());
        }

        self.ram_size = opts.ram_size;
        self.cpu_count = opts.cpu_count;
        self.bios_path = opts.bios.clone();
        self.bios_builtin = opts.bios_builtin;
        self.kernel_path = opts.kernel.clone();
        self.initrd_path = opts.initrd.clone();
        self.dtb_path = opts.dtb.clone();
        self.loaders = opts.loaders.clone();
        self.dtb_blob = None;
        self.kernel_cmdline = opts.append.clone();

        let mut cpus = Vec::with_capacity(opts.cpu_count as usize);
        for _ in 0..opts.cpu_count {
            cpus.push(Some(RiscvCpu::new_with_model(RiscvCpuModel::TheadC908)));
        }
        self.cpus = Arc::new(Mutex::new(cpus));

        let mut sysbus = SysBus::new("sysbus0");
        let mut root = MemoryRegion::container("system", u64::MAX);

        let (ram_region, ram_block) = MemoryRegion::ram("ram", opts.ram_size);
        root.add_subregion(ram_region, GPA::new(ddr.base));

        let sram = K230_MEMMAP[K230MemMap::Sram as usize];
        let (sram_region, sram_block) = MemoryRegion::ram("sram", sram.size);
        root.add_subregion(sram_region, GPA::new(sram.base));

        let bootrom = K230_MEMMAP[K230MemMap::Bootrom as usize];
        let (bootrom_region, bootrom_block) =
            MemoryRegion::rom("bootrom", bootrom.size);
        root.add_subregion(bootrom_region, GPA::new(bootrom.base));

        let plic_num_contexts = 2 * opts.cpu_count;
        let plic = Arc::new(Plic::new_named(
            "plic0",
            K230_PLIC_NUM_SOURCES,
            plic_num_contexts,
        ));
        plic.attach_to_bus(&mut sysbus)?;
        let plic_mm = K230_MEMMAP[K230MemMap::Plic as usize];
        let plic_region = MemoryRegion::io(
            "plic",
            plic_mm.size,
            Arc::new(PlicMmio(Arc::clone(&plic))),
        );
        plic.register_mmio(plic_region, GPA::new(plic_mm.base))?;

        let aclint = Arc::new(Aclint::new_named("aclint0", opts.cpu_count));
        aclint.attach_to_bus(&mut sysbus)?;
        let aclint_mm = K230_MEMMAP[K230MemMap::Clint as usize];
        let aclint_region = MemoryRegion::io(
            "clint",
            aclint_mm.size,
            Arc::new(AclintMmio(Arc::clone(&aclint))),
        );
        aclint.register_mmio(aclint_region, GPA::new(aclint_mm.base))?;

        Self::connect_cpu_interrupts(
            &plic,
            &aclint,
            opts.cpu_count,
            &self.shared_mip,
            &self.wfi_waker,
        );

        let mut uarts = Vec::with_capacity(K230_UART_COUNT);
        for index in 0..K230_UART_COUNT {
            let name = format!("uart{index}");
            let uart = Arc::new(Uart16550::new_named(&name));
            uart.attach_to_bus(&mut sysbus)?;
            let entry = K230_MEMMAP[K230MemMap::Uart0 as usize + index];
            let region = MemoryRegion::io(
                &name,
                entry.size,
                Arc::new(Uart16550ShiftedMmio::new(Arc::clone(&uart), 2)),
            );
            uart.register_mmio(region, GPA::new(entry.base))?;
            uarts.push(uart);
        }

        let mut wdts = Vec::with_capacity(2);
        for (index, map, irq) in [
            (0, K230MemMap::Wdt0, K230IrqMap::WDT0),
            (1, K230MemMap::Wdt1, K230IrqMap::WDT1),
        ] {
            let name = format!("k230-wdt{index}");
            let wdt = K230Wdt::new_named(&name);
            wdt.connect_irq(Self::plic_irq_source(&plic, irq));
            wdt.attach_to_bus(&mut sysbus)?;
            let entry = K230_MEMMAP[map as usize];
            let region = MemoryRegion::io(
                &name,
                MMIO_SIZE,
                Arc::new(K230WdtMmio(Arc::clone(&wdt))),
            );
            wdt.register_mmio(region, GPA::new(entry.base))?;
            wdts.push(wdt);
        }

        let mut sdhcis = Vec::with_capacity(2);
        let mut sd_buses = Vec::with_capacity(2);
        for (index, map) in
            [(0usize, K230MemMap::Sd0), (1usize, K230MemMap::Sd1)]
        {
            let name = format!("sd{index}");
            let bus = Arc::new(SdBus::new());
            let sdhci =
                Self::map_sdhci(&mut sysbus, &name, K230_MEMMAP[map as usize])?;
            let irq = if index == 0 {
                K230IrqMap::SD0
            } else {
                K230IrqMap::SD1
            };
            sdhci.connect_irq(Self::plic_irq_source(&plic, irq));
            sdhci.connect_bus(Arc::clone(&bus));
            bus.set_host(
                Arc::clone(&sdhci) as Arc<dyn machina_hw_sd::SdBusHost>
            );
            sd_buses.push(bus);
            sdhcis.push(sdhci);
        }
        Self::attach_drive_to_sd1(&opts.drive, &sd_buses)?;

        let gzip_dma = Self::map_gzip_dma(&mut sysbus)?;
        let pufs = Self::map_pufs(&mut sysbus)?;
        let kpu = Self::map_kpu(&mut sysbus)?;

        let mut unimp = Vec::with_capacity(Self::unimp_specs().len());
        for &(map, name) in Self::unimp_specs() {
            unimp.push(Self::map_unimp(
                &mut sysbus,
                name,
                K230_MEMMAP[map as usize],
            )?);
        }

        let mut address_space = AddressSpace::new(root);
        {
            let address_space = &mut address_space;
            plic.realize_onto(&mut sysbus, address_space)?;
            aclint.realize_onto(&mut sysbus, address_space)?;
            for (index, uart) in uarts.iter().enumerate() {
                uart.attach_irq(Self::plic_irq_line(
                    &plic,
                    K230IrqMap::UART0 + index as u32,
                ))?;
                let backend: Box<dyn Chardev + Send> =
                    if index == 0 && opts.nographic {
                        let mut sc = StdioChardev::new();
                        if let Some(ref qcb) = self.quit_cb {
                            sc.set_quit_cb(Arc::clone(qcb));
                        }
                        if let Some(ref mcb) = self.monitor_cb {
                            sc.set_monitor_cb(Arc::clone(mcb));
                        }
                        Box::new(sc)
                    } else {
                        Box::new(NullChardev)
                    };
                uart.attach_chardev(CharFrontend::new(backend))?;
                let uart_for_rx = Arc::clone(uart);
                let rx_cb: Arc<Mutex<dyn FnMut(u8) + Send>> =
                    Arc::new(Mutex::new(move |byte: u8| {
                        uart_for_rx.receive(byte);
                    }));
                uart.realize_onto(&mut sysbus, address_space, rx_cb)?;
            }
            for wdt in &wdts {
                wdt.realize_onto(&mut sysbus, address_space)?;
            }
            for sdhci in &sdhcis {
                sdhci.realize_onto(&mut sysbus, address_space)?;
            }
            gzip_dma.realize_onto(&mut sysbus, address_space)?;
            pufs.realize_onto(&mut sysbus, address_space)?;
            kpu.realize_onto(&mut sysbus, address_space)?;
            for dev in &unimp {
                dev.realize_onto(&mut sysbus, address_space)?;
            }
        }
        let address_space = Arc::new(address_space);
        for sdhci in &sdhcis {
            sdhci.set_dma_address_space(Arc::clone(&address_space));
        }
        gzip_dma.set_dma_address_space(Arc::clone(&address_space));
        pufs.set_dma_address_space(Arc::clone(&address_space));

        self.address_space = Some(address_space);
        self.ram_block = Some(ram_block);
        self.sram_block = Some(sram_block);
        self.bootrom_block = Some(bootrom_block);
        self.plic = Some(plic);
        self.aclint = Some(aclint);
        self.uarts = uarts;
        self.wdts = wdts;
        self.sdhcis = sdhcis;
        self.gzip_dma = Some(gzip_dma);
        self.pufs = Some(pufs);
        self.kpu = Some(kpu);
        self.unimp = unimp;
        self.sysbus = Some(sysbus);
        self.refresh_mom_tree();

        Ok(())
    }

    fn reset(&mut self) {
        if let Some(plic) = &self.plic {
            plic.reset_runtime();
        }
        if let Some(aclint) = &self.aclint {
            aclint.reset_runtime();
        }
        for uart in &self.uarts {
            uart.reset_runtime();
        }
        for wdt in &self.wdts {
            wdt.reset_runtime();
        }
        for sdhci in &self.sdhcis {
            sdhci.reset_runtime();
        }
        if let Some(gzip_dma) = &self.gzip_dma {
            gzip_dma.reset_runtime();
        }
        if let Some(pufs) = &self.pufs {
            pufs.reset_runtime();
        }
        if let Some(kpu) = &self.kpu {
            kpu.reset_runtime();
        }
        for dev in &self.unimp {
            dev.reset_runtime();
        }
    }

    fn pause(&mut self) {}

    fn resume(&mut self) {}

    fn shutdown(&mut self) {}

    fn boot(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        crate::k230_boot::boot_k230(self)
    }

    fn cpu_count(&self) -> usize {
        self.cpu_count as usize
    }

    fn ram_size(&self) -> u64 {
        self.ram_size
    }
}

pub struct RiscvCpuIrqSink {
    shared_mip: Arc<AtomicU64>,
    wfi_waker: Arc<WfiWaker>,
}

impl RiscvCpuIrqSink {
    pub fn new(shared_mip: Arc<AtomicU64>, wfi_waker: Arc<WfiWaker>) -> Self {
        Self {
            shared_mip,
            wfi_waker,
        }
    }
}

impl IrqSink for RiscvCpuIrqSink {
    fn set_irq(&self, irq: u32, level: bool) {
        let bit = 1u64 << irq;
        if level {
            self.shared_mip.fetch_or(bit, Ordering::SeqCst);
            self.wfi_waker.wake();
        } else {
            self.shared_mip.fetch_and(!bit, Ordering::SeqCst);
        }
    }
}
