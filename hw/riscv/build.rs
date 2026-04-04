use std::path::Path;

const FW_NAME: &str = "rustsbi-riscv64-machina-fw_dynamic.bin";

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let src = Path::new(&manifest).join("../../pc-bios").join(FW_NAME);
    println!("cargo:rerun-if-changed={}", src.display());

    if std::env::var("CARGO_FEATURE_EMBED_FIRMWARE").is_err() {
        return;
    }

    let out = std::env::var("OUT_DIR").unwrap();
    let dst = Path::new(&out).join(FW_NAME);

    if src.exists() {
        std::fs::copy(&src, &dst).unwrap();
    } else {
        // Registry build: firmware unavailable, write empty
        // stub. User must supply firmware at runtime.
        std::fs::write(&dst, []).unwrap();
    }
}
