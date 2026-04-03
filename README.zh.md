<h1 align="center">Machina (/ˈmɑːkɪnə/)</h1>
<p align="center">
  <a href="README.md">English</a> | 中文
</p>

<p align="center">
  用 Rust 编写的模块化 RISC-V 全系统模拟器，具有 JIT 动态二进制翻译引擎。
</p>

<p align="center">
  <b>AI Agent 协作开发案例</b> — 本项目主要由人类开发者与 AI Agent（Claude、Codex）协作开发，作为 AI 辅助系统编程的教育示例。
</p>

## 概述

Machina 是对 QEMU 核心概念的 Rust 重实现 — TCG（Tiny Code Generator）、设备模型和全系统模拟 — 旨在在 RISC-V 虚拟机上引导和运行 [rCore-Tutorial](https://github.com/rcore-os/rCore-Tutorial-v3) 第 1-8 章。

### 功能

- **JIT 二进制翻译**：RISC-V → x86-64，具有 TB 缓存、链接和优化
- **全系统模拟**：PLIC、ACLINT、UART、Sv39 MMU、SBI 固件
- **VirtIO 块设备**：mmap 原始磁盘镜像，支持文件系统章节
- **Monitor 控制台**：QMP 兼容 JSON 协议 + HMP 文本命令
- **Difftest**：通过 GDB RSP 与 QEMU 进行逐指令对比
- **1039 个测试**，零失败

### rCore-Tutorial 支持

| 章节 | 功能 | 状态 |
|------|------|------|
| ch1 | Hello World | ✅ 通过 |
| ch2 | 批处理系统 | ✅ 通过 |
| ch3 | 多任务调度 + 时钟中断 | ✅ 通过 |
| ch4 | Sv39 虚拟内存 | ✅ 通过 |
| ch5 | 进程管理 | ✅ 通过（shell） |
| ch6 | 文件系统（VirtIO） | ✅ 通过（shell） |
| ch7 | 进程间通信 | ✅ 通过（shell） |
| ch8 | 并发 | ✅ 通过（shell） |

## 快速开始

### 构建

```bash
git clone https://github.com/gevico/machina.git
cd machina
cargo build --release
```

### 运行 rCore-Tutorial

```bash
# Ch1-Ch5：裸机内核（无需磁盘）
./target/release/machina -nographic -bios none -kernel path/to/ch5.elf

# Ch6-Ch8：带 VirtIO 块设备
./target/release/machina -nographic \
  -drive file=path/to/fs.img \
  -kernel path/to/ch6.elf

# 带 Monitor 控制台（QMP over TCP）
./target/release/machina -nographic \
  -monitor tcp:127.0.0.1:4444 \
  -bios none -kernel path/to/ch5.elf

# 与 QEMU 逐指令对比测试
./target/release/machina --difftest \
  -bios none -kernel path/to/ch1.elf

```

### 快捷键（-nographic 模式）

| 按键 | 功能 |
|------|------|
| Ctrl+A, X | 退出模拟器 |
| Ctrl+A, C | 切换 Monitor 控制台 |
| Ctrl+A, H | 显示帮助 |

## 贡献

Machina 是一个 AI Agent 协作开发项目，欢迎人类和 AI Agent 贡献。

### 贡献流程

1. **先提 Issue** — 描述 bug、功能或改进
2. **Fork 仓库** — 创建你自己的副本
3. **创建分支** — `git checkout -b feature/your-feature`
4. **修改代码** — 遵循代码风格（80 列、`cargo fmt`、`cargo clippy`）
5. **测试** — `cargo test --workspace` 必须通过
6. **提交 Pull Request** — 引用 Issue 编号

### AI Agent 工作流

本项目使用 [Humanize](https://github.com/humania/humanize) 进行结构化 AI 开发：

- **RLCR 循环**：Round → Loop → Codex Review — 迭代开发 + 自动化代码审查
- **BitLesson**：跨会话持久化调试知识库
- **计划驱动**：设计文档 → 实现计划 → RLCR 执行

## 参考项目

| 项目 | 说明 | 链接 |
|------|------|------|
| QEMU | 参考实现 | https://github.com/qemu/qemu |
| rCore-Tutorial-v3 | 目标 OS 教程 | https://github.com/rcore-os/rCore-Tutorial-v3 |
| tg-rcore-tutorial | 组件化教程系列 | https://github.com/rcore-os |
| rust-vmm | Rust 虚拟化组件 | https://github.com/rust-vmm |

## 许可证

[MIT](LICENSE)
