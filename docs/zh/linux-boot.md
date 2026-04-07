# 在 Machina 上启动 RISC-V Linux 内核

本文档介绍如何在 machina `riscv64-ref` 平台上启动标准
RISC-V Linux 内核。

## 环境要求

| 组件 | 版本 | 说明 |
|------|------|------|
| Rust 工具链 | stable 1.80+ | `cargo build --release` |
| RISC-V Linux 内核 | 6.12+ | flat `Image` 格式 |
| SBI 固件 | OpenSBI 1.4+ 或内嵌 RustSBI | 见下文 |
| 根文件系统 | initramfs (cpio.gz) | 推荐 busybox |
| 交叉编译工具链 | riscv64-linux-gnu-gcc | 编译内核用 |

## 快速开始

```bash
# 1. 编译 machina
cargo build --release

# 2. 使用系统 OpenSBI + initramfs 启动
./target/release/mchn \
    -nographic -m 256 \
    -bios /usr/share/qemu/opensbi-riscv64-generic-fw_dynamic.bin \
    -kernel /path/to/Image \
    -initrd /path/to/rootfs.cpio.gz \
    -append "earlycon=ns16550a,mmio,0x10000000 console=ttyS0 root=/dev/ram rdinit=/sbin/init"
```

预期输出（节选）：

```
OpenSBI v1.5.1
...
Boot HART Base ISA        : rv64imafdc
...
Linux version 6.12.51 ...
...
Please press Enter to activate this console.
```

## 启动模式

### 模式一：OpenSBI（推荐）

使用外部 OpenSBI `fw_dynamic.bin`：

```bash
./target/release/mchn \
    -nographic -m 256 \
    -bios /usr/share/qemu/opensbi-riscv64-generic-fw_dynamic.bin \
    -kernel Image \
    -initrd rootfs.cpio.gz \
    -append "earlycon=ns16550a,mmio,0x10000000 console=ttyS0 root=/dev/ram rdinit=/sbin/init"
```

OpenSBI 获取方式：
- **Ubuntu/Debian**：`apt install qemu-system-misc` 会安装
  `/usr/share/qemu/opensbi-riscv64-generic-fw_dynamic.bin`
- **Buildroot**：编译产物在 `output/host/share/qemu/` 下
- **手动编译**：https://github.com/riscv-software-src/opensbi

### 模式二：内嵌 RustSBI

省略 `-bios` 参数即使用内置的 RustSBI v0.4.0：

```bash
./target/release/mchn \
    -nographic -m 256 \
    -kernel Image \
    -initrd rootfs.cpio.gz \
    -append "earlycon=ns16550a,mmio,0x10000000 console=ttyS0 root=/dev/ram rdinit=/sbin/init"
```

### 模式三：裸机模式（无 SBI）

适用于裸机固件或 riscv-tests：

```bash
./target/release/mchn \
    -nographic -m 128 \
    -bios none \
    -kernel firmware.bin
```

二进制文件加载到 `0x80000000`，以 M-mode 启动。

## 命令行参数

| 参数 | 说明 |
|------|------|
| `-m SIZE` | 内存大小（MiB，默认 128） |
| `-bios PATH` | SBI 固件（`none` = 跳过，省略 = RustSBI） |
| `-kernel PATH` | 内核镜像（flat binary 或 ELF） |
| `-initrd PATH` | initramfs 根文件系统（cpio.gz） |
| `-append STR` | 内核启动命令行 |
| `-nographic` | 禁用图形输出，串口重定向到 stdio |
| `-drive file=PATH` | 挂载 VirtIO 块设备 |
| `-s` | 在 `tcp::1234` 启动 GDB 服务器 |
| `-S` | 启动时冻结 CPU（配合 GDB 使用） |

## 内核命令行参数

推荐参数：

```
earlycon=ns16550a,mmio,0x10000000 console=ttyS0 root=/dev/ram rdinit=/sbin/init
```

| 参数 | 作用 |
|------|------|
| `earlycon=ns16550a,mmio,0x10000000` | 通过 UART MMIO 启用早期控制台 |
| `console=ttyS0` | 运行时控制台使用第一个串口 |
| `root=/dev/ram` | 根文件系统为 initramfs |
| `rdinit=/sbin/init` | initramfs 中 init 进程路径 |

## 编译内核

最小内核配置（无模块、无网络、使用 initramfs）：

```bash
# 交叉编译 RISC-V 内核
export ARCH=riscv
export CROSS_COMPILE=riscv64-linux-gnu-

# 从 defconfig 开始，精简配置
make defconfig
# 禁用模块，启用 initramfs
scripts/config --disable MODULES
scripts/config --enable BLK_DEV_INITRD

make -j$(nproc) Image
```

产出的 `Image` 文件在 `arch/riscv/boot/Image`。

## 制作根文件系统

使用 busybox 制作最小 initramfs：

```bash
# 编译静态链接的 RISC-V busybox
wget https://busybox.net/downloads/busybox-1.37.0.tar.bz2
tar xf busybox-1.37.0.tar.bz2
cd busybox-1.37.0
make ARCH=riscv CROSS_COMPILE=riscv64-linux-gnu- defconfig
sed -i 's/# CONFIG_STATIC is not set/CONFIG_STATIC=y/' .config
make ARCH=riscv CROSS_COMPILE=riscv64-linux-gnu- -j$(nproc)
make ARCH=riscv CROSS_COMPILE=riscv64-linux-gnu- install

# 打包 initramfs
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

## 平台硬件信息

`riscv64-ref` 模拟的设备：

| 设备 | 地址 | 中断号 |
|------|------|--------|
| MROM（复位向量） | `0x0000_1000` | — |
| SiFive Test（关机） | `0x0010_0000` | — |
| ACLINT（定时器+IPI） | `0x0200_0000` | MTI/MSI |
| PLIC（中断控制器） | `0x0C00_0000` | MEI/SEI |
| UART 16550A | `0x1000_0000` | 10 |
| VirtIO MMIO 插槽 0 | `0x1000_1000` | 1 |
| DRAM | `0x8000_0000` | — |

指令集：`rv64imafdc_zba_zbb_zbc_zbs_zicsr_zifencei`

## 常见问题

**没有控制台输出**：确认 `-append` 中包含
`earlycon=ns16550a,mmio,0x10000000`。

**内核崩溃 / 非法指令**：内核必须为 `rv64imafdc`（RV64GC）
编译。需要 Zfh、Zbkb 或 Vector 扩展的内核不受支持。

**卡在 DMA 初始化**：请更新到最新版 machina
（PR #23 中的 neg_align 修复解决了此问题）。
