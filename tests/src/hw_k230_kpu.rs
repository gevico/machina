//! Regression tests for the K230 KPU device model (#157).
//!
//! Mainline QEMU models the two KPU windows as `unimplemented_device`
//! stubs; machina promotes them to a real SysBus device. The CFG window
//! is a scratch register file (writes retained, read back); the L2-cache
//! window is a read-as-zero, writes-ignored stub. These tests pin that
//! behaviour, the access-width handling, reset, and the device lifecycle
//! driven through an `AddressSpace` at the real K230 GPAs.

use std::sync::Arc;

use machina_core::address::GPA;
use machina_hw_core::bus::SysBus;
use machina_hw_riscv::k230_kpu::{
    K230Kpu, K230KpuCfgMmio, K230KpuL2Mmio, K230_KPU_CFG_MMIO_SIZE,
};
use machina_memory::address_space::AddressSpace;
use machina_memory::region::{MemoryRegion, MmioOps};

const KPU_CFG_BASE: u64 = 0x8040_0000;
const KPU_L2_BASE: u64 = 0x8000_0000;
const KPU_L2_SIZE: u64 = 0x0020_0000;

fn make_test_aspace() -> (AddressSpace, SysBus) {
    let root = MemoryRegion::container("root", 0x1_0000_0000);
    let aspace = AddressSpace::new(root);
    let bus = SysBus::new("sysbus");
    (aspace, bus)
}

#[test]
fn cfg_window_retains_writes() {
    let kpu = K230Kpu::new_named("k230-kpu");
    let cfg = K230KpuCfgMmio(Arc::clone(&kpu));

    cfg.write(0x10, 4, 0xdead_beef);
    assert_eq!(cfg.read(0x10, 4), 0xdead_beef);

    // Little-endian sub-word reads of the stored word.
    assert_eq!(cfg.read(0x10, 1), 0xef);
    assert_eq!(cfg.read(0x12, 2), 0xdead);

    // A location that was never written reads back as zero.
    assert_eq!(cfg.read(0x40, 4), 0);
}

#[test]
fn cfg_window_rejects_unaligned_and_oob() {
    let kpu = K230Kpu::new_named("k230-kpu");
    let cfg = K230KpuCfgMmio(Arc::clone(&kpu));

    // Unaligned accesses are ignored: the write is dropped and the read
    // returns zero rather than a partial value.
    cfg.write(0x11, 4, 0xaaaa_aaaa);
    assert_eq!(cfg.read(0x11, 4), 0);
    assert_eq!(cfg.read(0x10, 4), 0);

    // The last aligned word of the 2 KiB window is addressable...
    let last = K230_KPU_CFG_MMIO_SIZE - 4;
    cfg.write(last, 4, 0x1234_5678);
    assert_eq!(cfg.read(last, 4), 0x1234_5678);

    // ...but an access running past the end of the window is dropped.
    assert_eq!(cfg.read(K230_KPU_CFG_MMIO_SIZE, 4), 0);
}

#[test]
fn l2_window_is_zero_stub() {
    let kpu = K230Kpu::new_named("k230-kpu");
    let l2 = K230KpuL2Mmio(Arc::clone(&kpu));

    // Writes are dropped and reads always return zero, mirroring QEMU's
    // unimplemented-device behaviour for "kpu.l2-cache".
    l2.write(0x0, 4, 0xffff_ffff);
    l2.write(0x1_0000, 8, 0xdead_beef_dead_beef);
    assert_eq!(l2.read(0x0, 4), 0);
    assert_eq!(l2.read(0x1_0000, 8), 0);
}

#[test]
fn reset_clears_cfg_scratch() {
    let kpu = K230Kpu::new_named("k230-kpu");
    let cfg = K230KpuCfgMmio(Arc::clone(&kpu));

    cfg.write(0x20, 4, 0xcafe_f00d);
    assert_eq!(cfg.read(0x20, 4), 0xcafe_f00d);

    kpu.reset_runtime();
    assert_eq!(cfg.read(0x20, 4), 0);
}

#[test]
fn lifecycle_maps_both_windows_and_mom_identity() {
    let kpu = K230Kpu::new_named("k230-kpu");
    assert!(!kpu.realized());
    kpu.with_mdevice(|device| assert_eq!(device.local_id(), "k230-kpu"));
    assert_eq!(kpu.object_info().local_id, "k230-kpu");

    let (mut aspace, mut bus) = make_test_aspace();
    kpu.attach_to_bus(&mut bus).unwrap();
    kpu.register_mmio(
        MemoryRegion::io(
            "k230-kpu-cfg",
            K230_KPU_CFG_MMIO_SIZE,
            Arc::new(K230KpuCfgMmio(Arc::clone(&kpu))),
        ),
        GPA(KPU_CFG_BASE),
    )
    .unwrap();
    kpu.register_mmio(
        MemoryRegion::io(
            "k230-kpu-l2-cache",
            KPU_L2_SIZE,
            Arc::new(K230KpuL2Mmio(Arc::clone(&kpu))),
        ),
        GPA(KPU_L2_BASE),
    )
    .unwrap();
    kpu.realize_onto(&mut bus, &mut aspace).unwrap();
    assert!(kpu.realized());

    // CFG window retains writes through the address space...
    aspace.write(GPA(KPU_CFG_BASE + 0x20), 4, 0x1234);
    assert_eq!(aspace.read(GPA(KPU_CFG_BASE + 0x20), 4), 0x1234);
    // ...and the L2 window always reads back as zero.
    aspace.write(GPA(KPU_L2_BASE), 4, 0xffff_ffff);
    assert_eq!(aspace.read(GPA(KPU_L2_BASE), 4), 0);

    let err = kpu.realize_onto(&mut bus, &mut aspace).unwrap_err();
    assert!(err.to_string().contains("already realized"));

    kpu.unrealize_from(&mut bus, &mut aspace).unwrap();
    assert!(!kpu.realized());
    assert_eq!(aspace.read(GPA(KPU_CFG_BASE + 0x20), 4), 0);
}
