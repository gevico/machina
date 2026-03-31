# Firmware build for Machina.
#
# Builds RustSBI Prototyper from the rustsbi/ submodule
# and copies the output binaries to ../pc-bios/.
#
# Usage:
#   make            # build fw_dynamic and fw_jump
#   make clean      # remove build artifacts
#
# Prerequisites:
#   - riscv64imac-unknown-none-elf target installed
#   - rust-objcopy (from cargo-binutils)
#   - nightly Rust toolchain (for rustsbi build)

RUSTSBI_DIR = rustsbi/prototyper
TARGET = riscv64imac-unknown-none-elf
PROFILE = release
OUT_DIR = ../pc-bios

# Prototyper ELF is placed under the workspace target dir
PROTOTYPER_ELF = $(RUSTSBI_DIR)/../target/$(TARGET)/$(PROFILE)/rustsbi-prototyper
OBJCOPY = rust-objcopy

.PHONY: all clean fw_dynamic fw_jump

all: fw_dynamic fw_jump

# Default build (no features) = fw_dynamic mode.
fw_dynamic:
	cd $(RUSTSBI_DIR) && cargo build \
		--package rustsbi-prototyper \
		--release \
		--target $(TARGET) \
		-Z build-std=core,alloc
	$(OBJCOPY) -O binary \
		$(PROTOTYPER_ELF) \
		$(OUT_DIR)/rustsbi-riscv64-machina-fw_dynamic.bin

# Jump mode: kernel entry at 0x80200000.
fw_jump:
	cd $(RUSTSBI_DIR) && cargo build \
		--package rustsbi-prototyper \
		--release \
		--target $(TARGET) \
		-Z build-std=core,alloc \
		--features jump
	$(OBJCOPY) -O binary \
		$(PROTOTYPER_ELF) \
		$(OUT_DIR)/rustsbi-riscv64-machina-fw_jump.bin

clean:
	cd $(RUSTSBI_DIR) && cargo clean
	rm -f $(OUT_DIR)/rustsbi-riscv64-machina-fw_*.bin
