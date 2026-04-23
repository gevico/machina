#[cfg(test)]
mod accel_timer;
#[cfg(test)]
mod backend;
#[cfg(test)]
mod cli_netdev;
#[cfg(test)]
mod core;
#[cfg(test)]
mod core_address;
#[cfg(test)]
mod decode;
#[cfg(test)]
mod disas_bitmanip;
#[cfg(test)]
mod exec;
#[cfg(test)]
mod frontend;
#[cfg(test)]
mod gdbstub;
#[cfg(test)]
mod hw_aclint;
#[cfg(test)]
mod hw_chardev;
#[cfg(test)]
mod hw_clock;
#[cfg(test)]
mod hw_fdt;
#[cfg(test)]
mod hw_irq;
#[cfg(test)]
mod hw_loader;
#[cfg(test)]
mod hw_plic;
#[cfg(test)]
mod hw_qdev;
#[cfg(test)]
mod hw_ref_machine;
#[cfg(test)]
mod hw_sysbus;
#[cfg(test)]
mod hw_uart;
#[cfg(test)]
mod integration;
#[cfg(test)]
mod memory_region;
#[cfg(test)]
mod monitor;
#[cfg(test)]
mod riscv_csr;
#[cfg(test)]
mod riscv_exception;
#[cfg(test)]
mod riscv_mmu;
#[cfg(test)]
mod riscv_pmp;
#[cfg(test)]
mod riscv_pmp_internal;
#[cfg(test)]
mod softfloat;
#[cfg(test)]
mod softmmu;
#[cfg(test)]
mod softmmu_exec;
#[cfg(test)]
mod system_cpu_manager;
#[cfg(test)]
mod tools;
#[cfg(test)]
mod trace;
#[cfg(test)]
mod virtio;
#[cfg(all(test, unix))]
mod virtio_net;
