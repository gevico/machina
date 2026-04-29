# Getting Started with Machina

> Target audience: developers who want to build, run, and boot
> guest software on machina.

## Quick Start

### Build

```bash
git clone https://github.com/gevico/machina.git
cd machina
make release
```

### Run

```bash
# Boot a kernel
./target/release/machina -nographic -bios none \
    -kernel path/to/kernel.elf

# With VirtIO block device
./target/release/machina -nographic \
    -drive file=path/to/disk.img \
    -kernel path/to/kernel.elf

# With monitor console (QMP over TCP)
./target/release/machina -nographic \
    -monitor tcp:127.0.0.1:4444 \
    -bios none -kernel path/to/kernel.elf
```

## Build Commands

| Command | Description |
|---------|-------------|
| `make build` | Build all crates (debug) |
| `make release` | Build all crates (release) |
| `make test` | Run all tests |
| `make clippy` | Lint with `-D warnings` |
| `make fmt` | Auto-format all code |

## Booting RISC-V Linux on Machina

This section describes how to boot a standard RISC-V Linux
kernel on the machina `riscv64-ref` platform.

### Prerequisites

| Component | Version | Notes |
|-----------|---------|-------|
| Rust toolchain | stable 1.80+ | `cargo build --release` |
| RISC-V Linux kernel | 6.12+ | flat `Image` format |
| SBI firmware | OpenSBI 1.4+ **or** embedded RustSBI | see below |
| Root filesystem | initramfs (cpio.gz) | busybox recommended |
| Cross toolchain | riscv64-linux-gnu-gcc | for building kernel |

### Linux Boot Quick Start

```bash
# 1. Build machina
cargo build --release

# 2. Boot with system OpenSBI + initramfs
./target/release/machina \
    -nographic -m 256 \
    -bios /usr/share/qemu/opensbi-riscv64-generic-fw_dynamic.bin \
    -kernel /path/to/Image \
    -initrd /path/to/rootfs.cpio.gz \
    -append "earlycon=ns16550a,mmio,0x10000000 console=ttyS0 root=/dev/ram rdinit=/sbin/init"
```

Expected output (abbreviated):

```
OpenSBI v1.5.1
...
Boot HART Base ISA        : rv64imafdc
...
Linux version 6.12.51 ...
...
Please press Enter to activate this console.
```

### Boot Modes

#### Mode 1: OpenSBI (Recommended)

Use an external OpenSBI `fw_dynamic.bin`:

```bash
./target/release/machina \
    -nographic -m 256 \
    -bios /usr/share/qemu/opensbi-riscv64-generic-fw_dynamic.bin \
    -kernel Image \
    -initrd rootfs.cpio.gz \
    -append "earlycon=ns16550a,mmio,0x10000000 console=ttyS0 root=/dev/ram rdinit=/sbin/init"
```

OpenSBI sources:
- **Ubuntu/Debian**: `apt install qemu-system-misc` installs
  `/usr/share/qemu/opensbi-riscv64-generic-fw_dynamic.bin`
- **Buildroot**: built under `output/host/share/qemu/`
- **Manual build**: https://github.com/riscv-software-src/opensbi

#### Mode 2: Embedded RustSBI

Omit the `-bios` flag to use the built-in RustSBI v0.4.0:

```bash
./target/release/machina \
    -nographic -m 256 \
    -kernel Image \
    -initrd rootfs.cpio.gz \
    -append "earlycon=ns16550a,mmio,0x10000000 console=ttyS0 root=/dev/ram rdinit=/sbin/init"
```

#### Mode 3: Bare-metal (No SBI)

For firmware or bare-metal binaries without SBI:

```bash
./target/release/machina \
    -nographic -m 128 \
    -bios none \
    -kernel firmware.bin
```

The binary loads at `0x80000000` and starts in M-mode.

### CLI Options

| Flag | Description |
|------|-------------|
| `-m SIZE` | RAM in MiB (default: 128) |
| `-bios PATH` | SBI firmware (`none` = skip, omit = RustSBI) |
| `-kernel PATH` | Kernel image (flat binary or ELF) |
| `-initrd PATH` | Initramfs (cpio.gz) |
| `-append STR` | Kernel command line |
| `-nographic` | Disable graphical output, serial on stdio |
| `-drive file=PATH` | Attach VirtIO block device |
| `-s` | GDB server on `tcp::1234` |
| `-S` | Freeze CPU at startup (use with GDB) |

### Kernel Command Line

Recommended parameters:

```
earlycon=ns16550a,mmio,0x10000000 console=ttyS0 root=/dev/ram rdinit=/sbin/init
```

| Parameter | Purpose |
|-----------|---------|
| `earlycon=ns16550a,mmio,0x10000000` | Early console via UART MMIO |
| `console=ttyS0` | Runtime console on first serial port |
| `root=/dev/ram` | Root filesystem is initramfs |
| `rdinit=/sbin/init` | Init process path in initramfs |

### Building the Kernel

A minimal kernel config for machina (no modules, no
network, initramfs):

```bash
# Cross-compile for RISC-V
export ARCH=riscv
export CROSS_COMPILE=riscv64-linux-gnu-

# Start from defconfig and trim
make defconfig
# Disable modules, enable initramfs
scripts/config --disable MODULES
scripts/config --enable BLK_DEV_INITRD

make -j$(nproc) Image
```

The `Image` file is in `arch/riscv/boot/Image`.

### Building the Root Filesystem

Using busybox for a minimal initramfs:

```bash
# Build static busybox for RISC-V
wget https://busybox.net/downloads/busybox-1.37.0.tar.bz2
tar xf busybox-1.37.0.tar.bz2
cd busybox-1.37.0
make ARCH=riscv CROSS_COMPILE=riscv64-linux-gnu- defconfig
sed -i 's/# CONFIG_STATIC is not set/CONFIG_STATIC=y/' .config
make ARCH=riscv CROSS_COMPILE=riscv64-linux-gnu- -j$(nproc)
make ARCH=riscv CROSS_COMPILE=riscv64-linux-gnu- install

# Create initramfs
cd _install
mkdir -p proc sys dev etc/init.d
cat > etc/init.d/rcS << 'INIT'
#!/bin/sh
mount -t proc none /proc
mount -t sysfs none /sys
INIT
chmod +x etc/init.d/rcS
cat > init << 'INIT'
#!/bin/sh
exec /sbin/init
INIT
chmod +x init
find . | cpio -o --format=newc | gzip > ../rootfs.cpio.gz
```

### Platform Details

The `riscv64-ref` machine emulates:

| Device | Address | IRQ |
|--------|---------|-----|
| MROM (reset vector) | `0x0000_1000` | -- |
| SiFive Test (shutdown) | `0x0010_0000` | -- |
| ACLINT (timer+IPI) | `0x0200_0000` | MTI/MSI |
| PLIC (interrupt controller) | `0x0C00_0000` | MEI/SEI |
| UART 16550A | `0x1000_0000` | 10 |
| VirtIO MMIO slot 0 | `0x1000_1000` | 1 |
| DRAM | `0x8000_0000` | -- |

ISA: `rv64imafdc_zba_zbb_zbc_zbs_zicsr_zifencei`

### Troubleshooting

**No console output**: Ensure `-append` includes
`earlycon=ns16550a,mmio,0x10000000`.

**Kernel panic / illegal instruction**: The kernel must
be built for `rv64imafdc` (RV64GC). Kernels requiring
Zfh, Zbkb, or Vector extensions are not supported.

**Stuck at DMA init**: Upgrade to the latest machina
(the neg_align fix in PR #23 resolves this).

## Keyboard Shortcuts

In `-nographic` mode, the escape prefix is `Ctrl+A`:

| Key | Action |
|-----|--------|
| Ctrl+A, X | Exit emulator |
| Ctrl+A, C | Toggle monitor console |
| Ctrl+A, H | Show help |
