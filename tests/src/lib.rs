#[cfg(test)]
mod accel_timer;
#[cfg(test)]
mod arch_exit;
#[cfg(test)]
mod backend;
#[cfg(test)]
mod cli_bios;
#[cfg(test)]
mod cli_drive;
#[cfg(test)]
mod cli_gdb;
#[cfg(test)]
mod cli_help;
#[cfg(test)]
mod cli_initrd;
#[cfg(test)]
mod cli_kernel;
#[cfg(test)]
mod cli_netdev;
#[cfg(test)]
mod cli_ram;
#[cfg(test)]
mod cli_smp;
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
mod hw_dma;
#[cfg(test)]
mod hw_eiointc;
#[cfg(test)]
mod hw_fdt;
#[cfg(test)]
mod hw_firmware;
#[cfg(test)]
mod hw_gpio;
#[cfg(test)]
mod hw_i2c;
#[cfg(test)]
mod hw_intc_loongarch;
#[cfg(test)]
mod hw_intc_riscv;
#[cfg(test)]
mod hw_ipi;
#[cfg(test)]
mod hw_irq;
#[cfg(test)]
mod hw_k230_gzip_dma;
#[cfg(test)]
mod hw_k230_kpu;
#[cfg(test)]
mod hw_k230_machine;
#[cfg(test)]
mod hw_k230_pufs;
#[cfg(test)]
mod hw_k230_wdt;
#[cfg(test)]
mod hw_loader;
#[cfg(test)]
mod hw_loongarch_test_finisher;
#[cfg(test)]
mod hw_misc;
#[cfg(test)]
mod hw_mom;
#[cfg(test)]
mod hw_pch_pic;
#[cfg(test)]
mod hw_pflash;
#[cfg(test)]
mod hw_plic;
#[cfg(test)]
mod hw_qdev;
#[cfg(test)]
mod hw_ref_machine;
#[cfg(test)]
mod hw_rtc;
#[cfg(test)]
mod hw_sd;
#[cfg(test)]
mod hw_sensor;
#[cfg(test)]
mod hw_ssi;
#[cfg(test)]
mod hw_storage;
#[cfg(test)]
mod hw_sysbus;
#[cfg(test)]
mod hw_timer;
#[cfg(test)]
mod hw_uart;
#[cfg(test)]
mod hw_watchdog;
#[cfg(test)]
mod integration;
#[cfg(test)]
mod loongarch_atomic;
#[cfg(test)]
mod loongarch_boot;
#[cfg(test)]
mod loongarch_boot_checkpoint;
#[cfg(test)]
mod loongarch_branch;
#[cfg(test)]
mod loongarch_cpu_layout;
#[cfg(test)]
mod loongarch_decode;
#[cfg(test)]
mod loongarch_difftest;
#[cfg(test)]
mod loongarch_fpu;
#[cfg(test)]
mod loongarch_interrupt;
#[cfg(test)]
mod loongarch_interrupt_matrix;
#[cfg(test)]
mod loongarch_iocsr;
#[cfg(test)]
mod loongarch_irq_cascade;
#[cfg(test)]
mod loongarch_memory;
#[cfg(test)]
mod loongarch_memory_branch_atomic;
#[cfg(test)]
mod loongarch_priv;
#[cfg(test)]
mod loongarch_scheduler;
#[cfg(test)]
mod loongarch_system;
#[cfg(test)]
mod loongarch_translator;
#[cfg(test)]
mod loongarch_virt_board;
#[cfg(test)]
mod memory_region;
#[cfg(test)]
mod monitor;
#[cfg(test)]
mod oracle;
#[cfg(test)]
mod riscv_cpu_model;
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
mod riscv_thead_csr;
#[cfg(test)]
mod softfloat;
#[cfg(test)]
mod softmmu;
#[cfg(test)]
mod softmmu_exec;
#[cfg(test)]
mod source_cleanliness;
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
