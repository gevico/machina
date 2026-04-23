// riscv64-ref machine: virt-compatible reference platform.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use machina_core::address::GPA;
use machina_core::machine::{Machine, MachineOpts, MachineState};
use machina_core::mobject::{MObject, MObjectInfo, MObjectNode};
use machina_core::wfi::WfiWaker;
use machina_guest_riscv::riscv::cpu::RiscvCpu;
use machina_hw_char::uart::{Uart16550, Uart16550Mmio};
use machina_hw_core::bus::{SysBus, SysBusMapping};
use machina_hw_core::chardev::{
    CharFrontend, Chardev, ChardevObject, NullChardev, StdioChardev,
};
use machina_hw_core::fdt::FdtBuilder;
use machina_hw_core::irq::{InterruptSource, IrqLine, IrqSink};
use machina_hw_core::mdev::MDevice;
use machina_hw_core::property::{MPropertySpec, MPropertyValue};
use machina_hw_intc::aclint::{Aclint, AclintMmio};
use machina_hw_intc::plic::{Plic, PlicIrqSink, PlicMmio};
use machina_hw_virtio::mmio::VirtioMmio;
use machina_memory::address_space::AddressSpace;
use machina_memory::ram::RamBlock;
use machina_memory::region::{MemoryRegion, MmioOps};

use crate::sifive_test::SifiveTest;

type MonitorCallback = Arc<Mutex<dyn FnMut(u8) + Send>>;

#[derive(Clone, Copy)]
pub struct MemMapEntry {
    pub base: u64,
    pub size: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum RefMemMap {
    Mrom = 0,
    SifiveTest,
    Rtc,
    Aclint,
    Plic,
    Uart0,
    FwCfg,
    Virtio,
    VirtioNet,
    Dram,
    Count,
}

pub const REF_MEMMAP: [MemMapEntry; RefMemMap::Count as usize] = {
    let mut m = [MemMapEntry { base: 0, size: 0 }; RefMemMap::Count as usize];
    m[RefMemMap::Mrom as usize] = MemMapEntry {
        base: 0x0000_1000,
        size: 0x0000_F000,
    };
    m[RefMemMap::SifiveTest as usize] = MemMapEntry {
        base: 0x0010_0000,
        size: 0x0000_1000,
    };
    m[RefMemMap::Rtc as usize] = MemMapEntry {
        base: 0x0010_1000,
        size: 0x0000_1000,
    };
    m[RefMemMap::Aclint as usize] = MemMapEntry {
        base: 0x0200_0000,
        size: 0x0001_0000,
    };
    m[RefMemMap::Plic as usize] = MemMapEntry {
        base: 0x0C00_0000,
        size: 0x0400_0000,
    };
    m[RefMemMap::Uart0 as usize] = MemMapEntry {
        base: 0x1000_0000,
        size: 0x0000_0100,
    };
    m[RefMemMap::FwCfg as usize] = MemMapEntry {
        base: 0x1010_0000,
        size: 0x0000_0018,
    };
    m[RefMemMap::Virtio as usize] = MemMapEntry {
        base: 0x1000_1000,
        size: 0x0000_1000,
    };
    m[RefMemMap::VirtioNet as usize] = MemMapEntry {
        base: 0x1000_2000,
        size: 0x0000_1000,
    };
    m[RefMemMap::Dram as usize] = MemMapEntry {
        base: 0x8000_0000,
        size: 0,
    };
    m
};

// Backward-compatible aliases (used by boot.rs, etc.).
pub const MROM_BASE: u64 = REF_MEMMAP[RefMemMap::Mrom as usize].base;
pub const MROM_SIZE: u64 = REF_MEMMAP[RefMemMap::Mrom as usize].size;
pub const RAM_BASE: u64 = REF_MEMMAP[RefMemMap::Dram as usize].base;

pub struct RefIrqMap {
    pub uart0: u32,
    pub rtc: u32,
    pub virtio_base: u32,
    pub virtio_net: u32,
}

pub const REF_IRQMAP: RefIrqMap = RefIrqMap {
    uart0: 10,
    rtc: 11,
    virtio_base: 1,
    virtio_net: 12,
};

pub const PLIC_NUM_SOURCES: u32 = 96;
pub const VIRTIO_SLOT_COUNT: usize = 8;

// PLIC context count is 2 * cpu_count (M-mode + S-mode per
// hart), computed dynamically in init().

/// Per-hart IRQ sink that updates the real CPU mip bits.
///
/// IRQ numbering (matches RISC-V privilege spec):
///   3 = MSI (machine software interrupt)
///   7 = MTI (machine timer interrupt)
///  11 = MEI (machine external interrupt)
///   1 = SSI (supervisor software interrupt)
///   5 = STI (supervisor timer interrupt)
///   9 = SEI (supervisor external interrupt)
/// IRQ sink that writes to a SharedMip (Arc<AtomicU64>).
/// This is read by FullSystemCpu::pending_interrupt() in
/// the exec loop, ensuring device IRQ delivery reaches
/// the executing CPU without shared mutable CPU access.
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
            // Wake halted CPU waiting in WFI.
            self.wfi_waker.wake();
        } else {
            self.shared_mip.fetch_and(!bit, Ordering::SeqCst);
        }
    }
}

// RISC-V interrupt numbers.
const IRQ_MSI: u32 = 3;
const IRQ_MTI: u32 = 7;
const IRQ_MEI: u32 = 11;
const IRQ_SEI: u32 = 9;

struct SifiveTestMmio(Arc<SifiveTest>);

impl MmioOps for SifiveTestMmio {
    fn read(&self, offset: u64, size: u32) -> u64 {
        self.0.read(offset, size)
    }

    fn write(&self, offset: u64, size: u32, val: u64) {
        self.0.write(offset, size, val);
    }
}

pub struct RefMachine {
    name: String,
    machine_state: MachineState,
    ram_size: u64,
    cpu_count: u32,
    chardev_root: Option<MObjectNode>,
    address_space: Option<AddressSpace>,
    sysbus: Option<SysBus>,
    ram_block: Option<Arc<RamBlock>>,
    mrom_block: Option<Arc<RamBlock>>,
    plic: Option<Arc<Plic>>,
    aclint: Option<Arc<Aclint>>,
    uart: Option<Arc<Uart16550>>,
    uart_chardev: Option<Arc<Mutex<ChardevObject>>>,
    virtio_mmio: Option<VirtioMmio>,
    virtio_mmio_net: Option<VirtioMmio>,
    sifive_test: Option<Arc<SifiveTest>>,
    fdt_blob: Option<Vec<u8>>,
    // Per-hart RiscvCpu instances. None after take_cpu().
    pub(crate) cpus: Arc<Mutex<Vec<Option<RiscvCpu>>>>,
    // Shared mip for device IRQ delivery to exec loop.
    pub(crate) shared_mip: Arc<AtomicU64>,
    // WFI waker: IRQ sinks call wake() to unblock halted CPU.
    pub(crate) wfi_waker: Arc<WfiWaker>,
    // Stored boot options (bios / kernel paths).
    pub(crate) bios_path: Option<PathBuf>,
    pub(crate) bios_builtin: bool,
    pub(crate) kernel_path: Option<PathBuf>,
    pub(crate) initrd_path: Option<PathBuf>,
    pub(crate) kernel_cmdline: Option<String>,
    // UART → PLIC IRQ line (source 10).
    uart_irq: Option<IrqLine>,
    // Monitor callbacks for StdioChardev.
    quit_cb: Option<Arc<dyn Fn() + Send + Sync>>,
    monitor_cb: Option<MonitorCallback>,
}

impl RefMachine {
    pub fn new() -> Self {
        Self {
            name: "riscv64-ref".to_string(),
            machine_state: MachineState::new_root("machine"),
            ram_size: 0,
            cpu_count: 0,
            chardev_root: None,
            address_space: None,
            sysbus: None,
            ram_block: None,
            mrom_block: None,
            plic: None,
            aclint: None,
            uart: None,
            uart_chardev: None,
            virtio_mmio: None,
            virtio_mmio_net: None,
            sifive_test: None,
            fdt_blob: None,
            cpus: Arc::new(Mutex::new(Vec::new())),
            shared_mip: Arc::new(AtomicU64::new(0)),
            wfi_waker: Arc::new(WfiWaker::new()),
            bios_path: None,
            bios_builtin: false,
            kernel_path: None,
            initrd_path: None,
            kernel_cmdline: None,
            uart_irq: None,
            quit_cb: None,
            monitor_cb: None,
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

    /// Attach a pre-built VirtIO net device. Useful for
    /// tests that cannot create a TapBackend.
    #[cfg(unix)]
    pub fn add_virtio_net(
        &mut self,
        net: machina_hw_virtio::net::VirtioNet,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let plic = self.plic.as_ref().unwrap();
        let ram_ptr = self.ram_block.as_ref().unwrap().as_ptr();
        let ram_size = self.ram_size();
        let sysbus = self.sysbus.as_mut().unwrap();
        let as_ = self.address_space.as_mut().unwrap();
        self.virtio_mmio_net = Some(Self::attach_net_device(
            net,
            plic,
            ram_ptr,
            ram_size,
            sysbus,
            Some(as_),
        )?);
        self.fdt_blob = Some(self.generate_fdt());
        Ok(())
    }

    #[cfg(unix)]
    fn attach_net_device(
        net: machina_hw_virtio::net::VirtioNet,
        plic: &Arc<Plic>,
        ram_ptr: *mut u8,
        ram_size: u64,
        sysbus: &mut SysBus,
        as_opt: Option<&mut AddressSpace>,
    ) -> Result<VirtioMmio, Box<dyn std::error::Error>> {
        let plic_sink = Arc::new(PlicIrqSink(Arc::clone(plic)));
        let net_irq =
            IrqLine::new(plic_sink as Arc<dyn IrqSink>, REF_IRQMAP.virtio_net);
        let net_mm = &REF_MEMMAP[RefMemMap::VirtioNet as usize];
        let mut mmio = VirtioMmio::new_named(
            "virtio-mmio1",
            Box::new(net),
            net_irq,
            ram_ptr,
            RAM_BASE,
            ram_size,
        );
        mmio.attach_to_bus(sysbus)?;
        let region = mmio.make_mmio_region("virtio-mmio1", net_mm.size);
        mmio.register_mmio(region, GPA::new(net_mm.base))?;
        if let Some(as_) = as_opt {
            mmio.realize_onto(sysbus, as_)?;
        }
        Ok(mmio)
    }

    pub fn lookup_object_by_path(&self, path: &str) -> Option<MObjectInfo> {
        self.mom_object_infos()
            .into_iter()
            .find(|info| info.object_path.as_deref() == Some(path))
    }

    pub fn lookup_object_by_local_id(
        &self,
        local_id: &str,
    ) -> Option<MObjectInfo> {
        self.mom_object_infos()
            .into_iter()
            .find(|info| info.local_id == local_id)
    }

    pub fn property_spec(
        &self,
        object_ref: &str,
        property: &str,
    ) -> Option<MPropertySpec> {
        self.with_mdevice(object_ref, |device| {
            device.property_spec(property).cloned()
        })
        .flatten()
    }

    pub fn property_value(
        &self,
        object_ref: &str,
        property: &str,
    ) -> Option<MPropertyValue> {
        self.with_mdevice(object_ref, |device| {
            device.property(property).cloned()
        })
        .flatten()
    }

    fn realized_sysbus_mapping(&self, owner: &str) -> &SysBusMapping {
        self.sysbus()
            .mappings()
            .iter()
            .find(|mapping| mapping.owner == owner)
            .unwrap_or_else(|| panic!("missing sysbus mapping for '{owner}'"))
    }

    fn mom_object_infos(&self) -> Vec<MObjectInfo> {
        let mut infos = vec![self.machine_state.object().info()];
        if let Some(chardev_root) = &self.chardev_root {
            infos.push(chardev_root.object_info());
        }
        if let Some(sysbus) = &self.sysbus {
            infos.push(sysbus.object_info());
        }
        if let Some(chardev) = &self.uart_chardev {
            infos.push(chardev.lock().unwrap().object_info());
        }
        if let Some(plic) = &self.plic {
            infos.push(plic.object_info());
        }
        if let Some(aclint) = &self.aclint {
            infos.push(aclint.object_info());
        }
        if let Some(uart) = &self.uart {
            infos.push(uart.object_info());
        }
        if let Some(virtio_mmio) = &self.virtio_mmio {
            infos.push(virtio_mmio.object_info());
        }
        if let Some(ref net) = self.virtio_mmio_net {
            infos.push(net.object_info());
        }
        infos
    }

    fn object_matches(object_ref: &str, info: &MObjectInfo) -> bool {
        info.local_id == object_ref
            || info.object_path.as_deref() == Some(object_ref)
    }

    fn with_mdevice<T>(
        &self,
        object_ref: &str,
        f: impl FnOnce(&dyn MDevice) -> T,
    ) -> Option<T> {
        if let Some(uart) = &self.uart {
            if Self::object_matches(object_ref, &uart.object_info()) {
                return Some(uart.with_mdevice(f));
            }
        }
        if let Some(plic) = &self.plic {
            if Self::object_matches(object_ref, &plic.object_info()) {
                return Some(plic.with_mdevice(f));
            }
        }
        if let Some(aclint) = &self.aclint {
            if Self::object_matches(object_ref, &aclint.object_info()) {
                return Some(aclint.with_mdevice(f));
            }
        }
        if let Some(virtio_mmio) = &self.virtio_mmio {
            if Self::object_matches(object_ref, &virtio_mmio.object_info()) {
                return Some(f(virtio_mmio));
            }
        }
        if let Some(ref net) = self.virtio_mmio_net {
            if Self::object_matches(object_ref, &net.object_info()) {
                return Some(f(net));
            }
        }
        None
    }

    fn sysbus_reg_cells(&self, owner: &str) -> [u32; 4] {
        let mapping = self.realized_sysbus_mapping(owner);
        [
            (mapping.base.0 >> 32) as u32,
            mapping.base.0 as u32,
            (mapping.size >> 32) as u32,
            mapping.size as u32,
        ]
    }

    pub fn plic(&self) -> &Arc<Plic> {
        self.plic.as_ref().expect("machine not initialized")
    }

    pub fn aclint(&self) -> &Arc<Aclint> {
        self.aclint.as_ref().expect("machine not initialized")
    }

    pub fn uart(&self) -> &Arc<Uart16550> {
        self.uart.as_ref().expect("machine not initialized")
    }

    pub fn sifive_test(&self) -> &Arc<SifiveTest> {
        self.sifive_test.as_ref().expect("machine not initialized")
    }

    /// Set quit callback for StdioChardev (Ctrl+A X).
    pub fn set_quit_cb(&mut self, cb: Arc<dyn Fn() + Send + Sync>) {
        self.quit_cb = Some(cb);
    }

    /// Set monitor callback for StdioChardev (Ctrl+A C).
    pub fn set_monitor_cb(&mut self, cb: Arc<Mutex<dyn FnMut(u8) + Send>>) {
        self.monitor_cb = Some(cb);
    }

    pub fn ram_block(&self) -> &Arc<RamBlock> {
        self.ram_block.as_ref().expect("machine not initialized")
    }

    pub fn mrom_block(&self) -> &Arc<RamBlock> {
        self.mrom_block.as_ref().expect("machine not initialized")
    }

    pub fn fdt_blob(&self) -> &[u8] {
        self.fdt_blob.as_ref().expect("machine not initialized")
    }

    /// Lock the CPU vector for shared access.
    pub fn cpus_lock(&self) -> MutexGuard<'_, Vec<Option<RiscvCpu>>> {
        self.cpus.lock().unwrap()
    }

    /// Take CPU out of the machine for execution.
    /// Returns None if already taken or index invalid.
    pub fn take_cpu(&self, idx: usize) -> Option<RiscvCpu> {
        let mut lock = self.cpus.lock().unwrap();
        lock.get_mut(idx).and_then(|slot| slot.take())
    }

    /// Shared mip for device IRQ delivery.
    pub fn shared_mip(&self) -> Arc<AtomicU64> {
        self.shared_mip.clone()
    }

    /// WFI waker shared with IRQ sinks.
    pub fn wfi_waker(&self) -> Arc<WfiWaker> {
        self.wfi_waker.clone()
    }

    /// Host pointer to the start of guest RAM.
    pub fn ram_ptr(&self) -> *const u8 {
        self.ram_block().as_ptr() as *const u8
    }

    /// UART → PLIC IRQ line reference.
    pub fn uart_irq(&self) -> &IrqLine {
        self.uart_irq.as_ref().expect("machine not initialized")
    }

    /// Write a byte slice into RAM at `offset` bytes from
    /// RAM_BASE.
    pub fn write_ram(
        &self,
        offset: u64,
        data: &[u8],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let block = self.ram_block();
        let end = offset + data.len() as u64;
        if end > block.size() {
            return Err(format!(
                "write_ram: offset {offset:#x} + len {:#x} \
                 exceeds RAM size {:#x}",
                data.len(),
                block.size()
            )
            .into());
        }
        // SAFETY: offset + data.len() is within the
        // mmap'd allocation (checked above).
        unsafe {
            let dst = block.as_ptr().add(offset as usize);
            std::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
        }
        Ok(())
    }

    /// Generate FDT with optional initrd range and cmdline.
    pub fn generate_fdt_with(
        &self,
        initrd_range: Option<(u64, u64)>,
        cmdline: Option<&str>,
    ) -> Vec<u8> {
        self.generate_fdt_inner(initrd_range, cmdline)
    }

    fn generate_fdt(&self) -> Vec<u8> {
        self.generate_fdt_inner(None, self.kernel_cmdline.as_deref())
    }

    fn generate_fdt_inner(
        &self,
        initrd_range: Option<(u64, u64)>,
        cmdline: Option<&str>,
    ) -> Vec<u8> {
        let mut fdt = FdtBuilder::new();
        let plic_mapping = self.realized_sysbus_mapping("plic0");
        let aclint_mapping = self.realized_sysbus_mapping("aclint0");
        let uart_mapping = self.realized_sysbus_mapping("uart0");
        let virtio_mapping = self
            .sysbus()
            .mappings()
            .iter()
            .find(|mapping| mapping.owner == "virtio-mmio0");

        // Phandle allocation: intc_phandle(hart) = hart + 1,
        // PLIC phandle = cpu_count + 1.
        let plic_phandle = self.cpu_count + 1;

        // Root node.
        fdt.begin_node("");
        fdt.property_string("compatible", "machina,riscv64-ref");
        fdt.property_string("model", "Machina RISC-V Reference Platform");
        fdt.property_u32("#address-cells", 2);
        fdt.property_u32("#size-cells", 2);

        // /cpus
        fdt.begin_node("cpus");
        fdt.property_u32("#address-cells", 1);
        fdt.property_u32("#size-cells", 0);
        fdt.property_u32("timebase-frequency", 10_000_000);
        for i in 0..self.cpu_count {
            let intc_phandle = i + 1;
            let name = format!("cpu@{i}");
            fdt.begin_node(&name);
            fdt.property_string("device_type", "cpu");
            fdt.property_u32("reg", i);
            fdt.property_string("compatible", "riscv");
            fdt.property_string(
                "riscv,isa",
                "rv64imafdc_zba_zbb_zbc_zbs_zicsr_zifencei",
            );
            fdt.property_string("mmu-type", "riscv,sv39");
            fdt.property_string("status", "okay");
            // Interrupt controller sub-node.
            fdt.begin_node("interrupt-controller");
            fdt.property_u32("#interrupt-cells", 1);
            fdt.property_bytes("interrupt-controller", &[]);
            fdt.property_string("compatible", "riscv,cpu-intc");
            fdt.property_u32("phandle", intc_phandle);
            fdt.end_node();
            fdt.end_node();
        }
        fdt.end_node(); // /cpus

        // /memory@80000000
        fdt.begin_node("memory@80000000");
        fdt.property_string("device_type", "memory");
        // reg = <0x0 0x80000000 0x0 ram_size>
        fdt.property_u32_list(
            "reg",
            &[
                0,
                RAM_BASE as u32,
                (self.ram_size >> 32) as u32,
                self.ram_size as u32,
            ],
        );
        fdt.end_node();

        // /soc
        fdt.begin_node("soc");
        fdt.property_u32("#address-cells", 2);
        fdt.property_u32("#size-cells", 2);
        fdt.property_string("compatible", "simple-bus");
        fdt.property_bytes("ranges", &[]);

        // /soc/plic@c000000
        // Build interrupts-extended: per hart, two contexts
        // (M-mode external=11, S-mode external=9).
        let mut plic_ext = Vec::with_capacity(self.cpu_count as usize * 4);
        for i in 0..self.cpu_count {
            let intc_ph = i + 1;
            // M-mode external interrupt (IRQ 11).
            plic_ext.push(intc_ph);
            plic_ext.push(IRQ_MEI);
            // S-mode external interrupt (IRQ 9).
            plic_ext.push(intc_ph);
            plic_ext.push(IRQ_SEI);
        }
        fdt.begin_node(&format!("plic@{:x}", plic_mapping.base.0));
        fdt.property_string("compatible", "sifive,plic-1.0.0");
        fdt.property_u32("#interrupt-cells", 1);
        fdt.property_bytes("interrupt-controller", &[]);
        fdt.property_u32("phandle", plic_phandle);
        fdt.property_u32_list("reg", &self.sysbus_reg_cells("plic0"));
        fdt.property_u32("riscv,ndev", PLIC_NUM_SOURCES - 1);
        fdt.property_u32_list("interrupts-extended", &plic_ext);
        fdt.end_node();

        // /soc/clint@2000000
        // Build interrupts-extended: per hart, MTI (7) and
        // MSI (3).
        let mut clint_ext = Vec::with_capacity(self.cpu_count as usize * 4);
        for i in 0..self.cpu_count {
            let intc_ph = i + 1;
            clint_ext.push(intc_ph);
            clint_ext.push(IRQ_MTI);
            clint_ext.push(intc_ph);
            clint_ext.push(IRQ_MSI);
        }
        fdt.begin_node(&format!("clint@{:x}", aclint_mapping.base.0));
        fdt.property_string("compatible", "riscv,clint0");
        fdt.property_u32_list("reg", &self.sysbus_reg_cells("aclint0"));
        fdt.property_u32_list("interrupts-extended", &clint_ext);
        fdt.end_node();

        // /soc/test@100000
        fdt.begin_node("test@100000");
        fdt.property_string("compatible", "sifive,test0");
        let st = &REF_MEMMAP[RefMemMap::SifiveTest as usize];
        fdt.property_u32_list("reg", &[0, st.base as u32, 0, st.size as u32]);
        fdt.end_node();

        // /soc/serial@10000000
        fdt.begin_node(&format!("serial@{:x}", uart_mapping.base.0));
        fdt.property_string("compatible", "ns16550a");
        fdt.property_u32_list("reg", &self.sysbus_reg_cells("uart0"));
        fdt.property_u32("interrupts", REF_IRQMAP.uart0);
        fdt.property_u32("interrupt-parent", plic_phandle);
        fdt.property_u32("clock-frequency", 3686400);
        fdt.end_node();

        // /soc/virtio_mmio@10001000 (if drive configured)
        if let Some(mapping) = virtio_mapping {
            fdt.begin_node(&format!("virtio_mmio@{:x}", mapping.base.0));
            fdt.property_string("compatible", "virtio,mmio");
            fdt.property_u32_list(
                "reg",
                &self.sysbus_reg_cells("virtio-mmio0"),
            );
            fdt.property_u32("interrupts", REF_IRQMAP.virtio_base);
            fdt.property_u32("interrupt-parent", plic_phandle);
            fdt.end_node();
        }

        // /soc/virtio_mmio@10002000 (if netdev configured)
        let net_mapping = self
            .sysbus()
            .mappings()
            .iter()
            .find(|mapping| mapping.owner == "virtio-mmio1");
        if let Some(mapping) = net_mapping {
            fdt.begin_node(&format!("virtio_mmio@{:x}", mapping.base.0));
            fdt.property_string("compatible", "virtio,mmio");
            fdt.property_u32_list(
                "reg",
                &self.sysbus_reg_cells("virtio-mmio1"),
            );
            fdt.property_u32("interrupts", REF_IRQMAP.virtio_net);
            fdt.property_u32("interrupt-parent", plic_phandle);
            fdt.end_node();
        }

        fdt.end_node(); // /soc

        // /chosen
        fdt.begin_node("chosen");
        fdt.property_string(
            "stdout-path",
            &format!("/soc/serial@{:x}", uart_mapping.base.0),
        );
        if let Some(cmdline) = cmdline {
            fdt.property_string("bootargs", cmdline);
        }
        if let Some((start, end)) = initrd_range {
            fdt.property_u32_list(
                "linux,initrd-start",
                &[(start >> 32) as u32, start as u32],
            );
            fdt.property_u32_list(
                "linux,initrd-end",
                &[(end >> 32) as u32, end as u32],
            );
        }
        fdt.end_node();

        fdt.end_node(); // root
        fdt.finish()
    }
}

impl Default for RefMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl Machine for RefMachine {
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
        self.ram_size = opts.ram_size;
        self.cpu_count = opts.cpu_count;
        self.bios_path = opts.bios.clone();
        self.bios_builtin = opts.bios_builtin;
        self.kernel_path = opts.kernel.clone();
        self.initrd_path = opts.initrd.clone();
        self.kernel_cmdline = opts.append.clone();

        // Create per-hart CPUs.
        {
            let mut cpus = Vec::with_capacity(opts.cpu_count as usize);
            for _ in 0..opts.cpu_count {
                cpus.push(Some(RiscvCpu::new()));
            }
            self.cpus = Arc::new(Mutex::new(cpus));
        }

        let mut sysbus = SysBus::new("sysbus0");

        // Build the address space.
        let mut root = MemoryRegion::container("system", u64::MAX);

        // RAM at 0x8000_0000.
        let (ram_region, ram_block) = MemoryRegion::ram("ram", opts.ram_size);
        self.ram_block = Some(ram_block);
        root.add_subregion(ram_region, GPA::new(RAM_BASE));

        // PLIC: 2 contexts per hart (M-mode + S-mode).
        let plic_num_contexts = 2 * opts.cpu_count;
        let plic = Arc::new(Plic::new_named(
            "plic0",
            PLIC_NUM_SOURCES,
            plic_num_contexts,
        ));
        plic.attach_to_bus(&mut sysbus)?;
        let plic_mm = &REF_MEMMAP[RefMemMap::Plic as usize];
        let plic_region = MemoryRegion::io(
            "plic",
            plic_mm.size,
            Arc::new(PlicMmio(Arc::clone(&plic))),
        );
        plic.register_mmio(plic_region, GPA::new(plic_mm.base))?;
        self.plic = Some(Arc::clone(&plic));

        // ACLINT (CLINT-compatible) — interior mutability.
        let aclint = Arc::new(Aclint::new_named("aclint0", opts.cpu_count));
        aclint.attach_to_bus(&mut sysbus)?;
        let aclint_mm = &REF_MEMMAP[RefMemMap::Aclint as usize];
        let aclint_region = MemoryRegion::io(
            "clint",
            aclint_mm.size,
            Arc::new(AclintMmio(Arc::clone(&aclint))),
        );
        aclint.register_mmio(aclint_region, GPA::new(aclint_mm.base))?;
        self.aclint = Some(Arc::clone(&aclint));

        // UART0 — interior mutability, no outer Mutex.
        let uart = Arc::new(Uart16550::new_named("uart0"));
        uart.set_chardev_property("/machine/chardev/uart0")?;
        uart.attach_to_bus(&mut sysbus)?;
        let uart_mm = &REF_MEMMAP[RefMemMap::Uart0 as usize];
        let uart_region = MemoryRegion::io(
            "uart0",
            uart_mm.size,
            Arc::new(Uart16550Mmio(Arc::clone(&uart))),
        );
        uart.register_mmio(uart_region, GPA::new(uart_mm.base))?;
        self.uart = Some(uart);

        // SiFive Test (system reset/shutdown).
        let st_mm = &REF_MEMMAP[RefMemMap::SifiveTest as usize];
        let sifive_test = Arc::new(SifiveTest::new());
        let st_region = MemoryRegion::io(
            "sifive_test",
            st_mm.size,
            Arc::new(SifiveTestMmio(Arc::clone(&sifive_test))),
        );
        root.add_subregion(st_region, GPA::new(st_mm.base));
        self.sifive_test = Some(sifive_test);

        // MROM at 0x1000 (mask ROM for reset vector).
        let (mrom_region, mrom_block) = MemoryRegion::ram("mrom", MROM_SIZE);
        self.mrom_block = Some(mrom_block);
        root.add_subregion(mrom_region, GPA::new(MROM_BASE));

        // VirtIO block device (if -drive configured).
        if let Some(ref drive_path) = opts.drive {
            use machina_hw_virtio::block::VirtioBlk;

            let blk = VirtioBlk::open(drive_path).unwrap_or_else(|e| {
                panic!(
                    "virtio-blk: failed to open \
                         {:?}: {}",
                    drive_path, e
                );
            });
            let plic_sink =
                Arc::new(PlicIrqSink(Arc::clone(self.plic.as_ref().unwrap())));
            let vio_mm = &REF_MEMMAP[RefMemMap::Virtio as usize];
            let virtio_irq = IrqLine::new(
                plic_sink as Arc<dyn IrqSink>,
                REF_IRQMAP.virtio_base,
            );
            let ram_ptr = self.ram_block.as_ref().unwrap().as_ptr();
            let mut virtio_mmio = VirtioMmio::new_named(
                "virtio-mmio0",
                Box::new(blk),
                virtio_irq,
                ram_ptr,
                RAM_BASE,
                opts.ram_size,
            );
            virtio_mmio.attach_to_bus(&mut sysbus)?;
            let virtio_region =
                virtio_mmio.make_mmio_region("virtio-mmio0", vio_mm.size);
            virtio_mmio.register_mmio(virtio_region, GPA::new(vio_mm.base))?;
            self.virtio_mmio = Some(virtio_mmio);
        }

        // VirtIO net device (if -netdev configured,
        // Unix only: TAP requires POSIX sockets).
        #[cfg(unix)]
        if let Some(ref netdev) = opts.netdev {
            use machina_hw_virtio::net::{
                parse_mac, TapBackend, VirtioNet, DEFAULT_MAC,
            };
            let tap = TapBackend::new(&netdev.ifname)?;
            let mac = if let Some(ref m) = netdev.mac {
                parse_mac(m).map_err(|e| format!("bad MAC: {}", e))?
            } else {
                DEFAULT_MAC
            };
            let net = VirtioNet::new(Arc::new(tap), mac);
            let plic = self.plic.as_ref().unwrap();
            let ram_ptr = self.ram_block.as_ref().unwrap().as_ptr();
            self.virtio_mmio_net = Some(Self::attach_net_device(
                net,
                plic,
                ram_ptr,
                opts.ram_size,
                &mut sysbus,
                None,
            )?);
        }

        // UART IRQ source -> PLIC.
        let plic_as_sink =
            Arc::new(PlicIrqSink(Arc::clone(self.plic.as_ref().unwrap())));
        let uart_irq_line = IrqLine::new(
            Arc::clone(&plic_as_sink) as Arc<dyn IrqSink>,
            REF_IRQMAP.uart0,
        );
        self.uart_irq = Some(IrqLine::new(
            plic_as_sink as Arc<dyn IrqSink>,
            REF_IRQMAP.uart0,
        ));

        // All IRQ sinks write to shared_mip which is
        // read by FullSystemCpu::pending_interrupt().
        {
            let mip = &self.shared_mip;
            let wk = &self.wfi_waker;
            let p = self.plic.as_ref().unwrap();
            for hart in 0..opts.cpu_count as usize {
                let mei_sink: Arc<dyn IrqSink> = Arc::new(
                    RiscvCpuIrqSink::new(Arc::clone(mip), Arc::clone(wk)),
                );
                p.connect_context_output(
                    (2 * hart) as u32,
                    InterruptSource::new(Arc::clone(&mei_sink), IRQ_MEI),
                );
                let sei_sink: Arc<dyn IrqSink> = Arc::new(
                    RiscvCpuIrqSink::new(Arc::clone(mip), Arc::clone(wk)),
                );
                p.connect_context_output(
                    (2 * hart + 1) as u32,
                    InterruptSource::new(Arc::clone(&sei_sink), IRQ_SEI),
                );
            }
        }

        {
            let mip = &self.shared_mip;
            let wk = &self.wfi_waker;
            let a = self.aclint.as_ref().unwrap();
            a.connect_wfi_waker(Arc::clone(wk));
            for hart in 0..opts.cpu_count as usize {
                let mti_sink = Arc::new(RiscvCpuIrqSink::new(
                    Arc::clone(mip),
                    Arc::clone(wk),
                ));
                let mti_line =
                    IrqLine::new(mti_sink as Arc<dyn IrqSink>, IRQ_MTI);
                a.connect_mti(hart as u32, mti_line);
                let msi_sink = Arc::new(RiscvCpuIrqSink::new(
                    Arc::clone(mip),
                    Arc::clone(wk),
                ));
                let msi_line =
                    IrqLine::new(msi_sink as Arc<dyn IrqSink>, IRQ_MSI);
                a.connect_msi(hart as u32, msi_line);
            }
        }

        self.address_space = Some(AddressSpace::new(root));

        {
            let address_space = self.address_space.as_mut().unwrap();
            self.plic
                .as_ref()
                .unwrap()
                .realize_onto(&mut sysbus, address_space)?;
            self.aclint
                .as_ref()
                .unwrap()
                .realize_onto(&mut sysbus, address_space)?;
            if let Some(virtio_mmio) = self.virtio_mmio.as_mut() {
                virtio_mmio.realize_onto(&mut sysbus, address_space)?;
            }
            if let Some(ref mut net) = self.virtio_mmio_net {
                net.realize_onto(&mut sysbus, address_space)?;
            }
        }

        {
            let backend: Box<dyn Chardev + Send> = if opts.nographic {
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
            let fe = CharFrontend::new(backend);

            // Wire backend input -> UART receive.
            let uart_for_rx = Arc::clone(self.uart.as_ref().unwrap());
            let rx_cb: Arc<Mutex<dyn FnMut(u8) + Send>> =
                Arc::new(Mutex::new(move |byte: u8| {
                    uart_for_rx.receive(byte);
                }));

            let u = self.uart.as_ref().unwrap();
            u.attach_irq(uart_irq_line)?;
            u.attach_chardev(fe)?;
            u.realize_onto(
                &mut sysbus,
                self.address_space.as_mut().unwrap(),
                rx_cb,
            )?;
        }

        self.sysbus = Some(sysbus);

        // Generate FDT.
        self.fdt_blob = Some(self.generate_fdt());

        Ok(())
    }

    fn reset(&mut self) {
        if let Some(plic) = &self.plic {
            plic.reset_runtime();
        }
        if let Some(aclint) = &self.aclint {
            aclint.reset_runtime();
        }
        if let Some(uart) = &self.uart {
            uart.reset_runtime();
        }
        if let Some(virtio_mmio) = &mut self.virtio_mmio {
            virtio_mmio.reset_runtime();
        }
        if let Some(ref mut net) = self.virtio_mmio_net {
            net.reset_runtime();
        }
    }

    fn pause(&mut self) {
        // No-op for now.
    }

    fn resume(&mut self) {
        // No-op for now.
    }

    fn shutdown(&mut self) {
        // No-op for now.
    }

    fn boot(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.bios_builtin {
            use crate::boot::boot_builtin;
            boot_builtin(self)
        } else {
            use crate::boot::boot_ref_machine;
            boot_ref_machine(self)
        }
    }

    fn cpu_count(&self) -> usize {
        self.cpu_count as usize
    }

    fn ram_size(&self) -> u64 {
        self.ram_size
    }
}
