<h1 align="center">Machina (/ˈmɑːkɪnə/)</h1>
<p align="center">
  English | <a href="README.zh.md">中文</a>
</p>

<p align="center">
  A modular RISC-V full-system emulator written in Rust, featuring a JIT dynamic binary translation engine.
</p>

<p align="center">
  <b>An AI-agent collaborative development case study</b> — this project is primarily developed through collaboration between human developers and AI agents (Claude, Codex), serving as an educational example of AI-assisted systems programming.
</p>

## Overview

Machina is a Rust reimplementation of core QEMU concepts — TCG (Tiny Code Generator), device models, and full-system emulation — for the RISC-V architecture.

### Features

- **JIT binary translation**: RISC-V → x86-64 with TB caching, chaining, and optimization
- **Full-system emulation**: PLIC, ACLINT, UART, Sv39 MMU, SBI firmware
- **VirtIO block device**: mmap'd raw disk images
- **Monitor console**: QMP-compatible JSON protocol + HMP text commands
- **Difftest**: Instruction-level comparison against QEMU via GDB RSP
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
./target/release/machina -nographic -bios none -kernel path/to/kernel.elf

# With VirtIO block device
./target/release/machina -nographic \
  -drive file=path/to/disk.img \
  -kernel path/to/kernel.elf

# With monitor console (QMP over TCP)
./target/release/machina -nographic \
  -monitor tcp:127.0.0.1:4444 \
  -bios none -kernel path/to/kernel.elf
```

### Keyboard Shortcuts (-nographic)

| Key | Action |
|-----|--------|
| Ctrl+A, X | Exit emulator |
| Ctrl+A, C | Toggle monitor console |
| Ctrl+A, H | Show help |

## Documentation

Full documentation is available in English and Chinese:
**[Documentation Index](docs/README.md)**

## Contributing

Machina is an AI-agent collaborative development project. Contributions are welcome from both humans and AI agents.

### How to Contribute

1. **Open an Issue first** — describe the bug, feature, or improvement
2. **Fork the repository** — create your own copy
3. **Create a branch** — `git checkout -b feature/your-feature`
4. **Make changes** — follow the coding style (80-column, `cargo fmt`, `cargo clippy`)
5. **Test** — `make test` must pass
6. **Submit a Pull Request** — reference the issue number

### AI Agent Workflow

This project uses [Humanize](https://github.com/humania-org/humanize) for structured AI development:

- **RLCR Loop**: Round → Loop → Codex Review — iterative development with automated code review
- **BitLesson**: Persistent knowledge base capturing debugging insights across sessions
- **Plan-driven**: Design specs → implementation plans → RLCR execution

## References

| Project | Description | Link |
|---------|-------------|------|
| QEMU | Reference implementation | https://github.com/qemu/qemu |
| rCore-Tutorial-v3 | Target OS tutorial | https://github.com/rcore-os/rCore-Tutorial-v3 |
| tg-rcore-tutorial | Componentized tutorial series | https://github.com/rcore-os |
| rust-vmm | Rust virtualization components | https://github.com/rust-vmm |

## License

[MIT](LICENSE)
