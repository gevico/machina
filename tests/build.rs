//! Emit `has_riscv_gcc` when `riscv64-linux-gnu-gcc` is available so
//! QEMU-based frontend difftests can compile (`frontend/difftest.rs`).

fn main() {
    println!("cargo::rustc-check-cfg=cfg(has_riscv_gcc)");
    let ok = std::process::Command::new("riscv64-linux-gnu-gcc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if ok {
        println!("cargo:rustc-cfg=has_riscv_gcc");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
