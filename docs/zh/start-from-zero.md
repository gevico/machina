# Machina 本地开发与测试指南

本文档面向希望参与 Machina 开发的新贡献者，提供从克隆代码到提交 PR 的完整可复现流程。

借助github codespace
已经具备了make环境，执行curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh，构建rust环境，source "$HOME/.cargo/env"在当前终端下构建环境变量 执行make release随后执行sudo apt-get update
sudo apt-get install gcc-riscv64-linux-gnu

sudo apt-get install -y qemu-system-riscv64 qemu-user




# Machina 本地开发与测试指南

本文档面向希望参与 Machina 开发的新贡献者，提供从克隆代码到提交 PR 的完整可复现流程。

> **推荐环境**：GitHub Codespace（已预置 make 等基础工具链）

## 环境准备

### 1. 安装 Rust 工具链

GitHub Codespace 已具备 make 环境，需要手动安装 Rust：

```bash
# 安装 Rust（选择默认安装选项 1）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 在当前终端加载环境变量
source "$HOME/.cargo/env"

# 验证安装
rustc --version
cargo --version

# 更新软件包列表
sudo apt-get update

# 安装 RISC-V 64 位 GCC 工具链
sudo apt-get install -y gcc-riscv64-linux-gnu

# 验证安装
riscv64-linux-gnu-gcc --version

# 安装系统模式和用户模式 QEMU
sudo apt-get install -y qemu-system-riscv64 qemu-user

# 验证安装
qemu-system-riscv64 --version
