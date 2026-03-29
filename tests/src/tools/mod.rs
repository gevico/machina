//! Integration tests for machina-irdump --emit-bin and machina-irbackend.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn bin_path(name: &str) -> PathBuf {
    project_root().join("target").join("debug").join(name)
}

fn guest_dhrystone() -> PathBuf {
    project_root().join("target/guest/riscv64/dhrystone")
}

/// Build both tools before running tests.
fn ensure_built() {
    let status = Command::new("cargo")
        .args(["build", "-p", "machina-irdump", "-p", "machina-irbackend"])
        .current_dir(project_root())
        .status()
        .expect("cargo build failed");
    assert!(status.success(), "cargo build failed");
}

#[test]
fn irdump_emit_bin_produces_file() {
    ensure_built();
    let tmp = "/tmp/tcg-test-irdump.tcgir";
    let _ = fs::remove_file(tmp);

    let status = Command::new(bin_path("machina-irdump"))
        .args([
            guest_dhrystone().to_str().unwrap(),
            "--emit-bin",
            tmp,
            "--count",
            "2",
        ])
        .status()
        .expect("machina-irdump failed to run");
    assert!(status.success(), "machina-irdump exited with error");

    let data = fs::read(tmp).expect("output file missing");
    // Verify magic header
    assert!(data.len() > 20, "file too small");
    assert_eq!(&data[..4], b"TCIR");

    let _ = fs::remove_file(tmp);
}

#[test]
fn irbackend_hex_dump() {
    ensure_built();
    let tmp_ir = "/tmp/tcg-test-irbackend.tcgir";
    let _ = fs::remove_file(tmp_ir);

    // Generate IR
    let status = Command::new(bin_path("machina-irdump"))
        .args([
            guest_dhrystone().to_str().unwrap(),
            "--emit-bin",
            tmp_ir,
            "--count",
            "1",
        ])
        .status()
        .expect("machina-irdump failed");
    assert!(status.success());

    // Run backend
    let output = Command::new(bin_path("machina-irbackend"))
        .arg(tmp_ir)
        .output()
        .expect("machina-irbackend failed");
    assert!(
        output.status.success(),
        "machina-irbackend failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should contain hex dump lines like "0000:  xx xx ..."
    assert!(
        stdout.contains("0000:"),
        "expected hex dump output, got: {stdout}"
    );

    let _ = fs::remove_file(tmp_ir);
}

#[test]
fn irbackend_raw_output() {
    ensure_built();
    let tmp_ir = "/tmp/tcg-test-irbackend-raw.tcgir";
    let tmp_bin = "/tmp/tcg-test-irbackend-raw.bin";
    let _ = fs::remove_file(tmp_ir);
    let _ = fs::remove_file(tmp_bin);

    // Generate IR
    let status = Command::new(bin_path("machina-irdump"))
        .args([
            guest_dhrystone().to_str().unwrap(),
            "--emit-bin",
            tmp_ir,
            "--count",
            "1",
        ])
        .status()
        .expect("machina-irdump failed");
    assert!(status.success());

    // Run backend with --raw -o
    let status = Command::new(bin_path("machina-irbackend"))
        .args([tmp_ir, "--raw", "-o", tmp_bin])
        .status()
        .expect("machina-irbackend failed");
    assert!(status.success());

    let data = fs::read(tmp_bin).expect("raw output missing");
    assert!(!data.is_empty(), "raw output should not be empty");

    let _ = fs::remove_file(tmp_ir);
    let _ = fs::remove_file(tmp_bin);
}

#[test]
fn irbackend_multiple_tbs() {
    ensure_built();
    let tmp_ir = "/tmp/tcg-test-irbackend-multi.tcgir";
    let _ = fs::remove_file(tmp_ir);

    // Generate 5 TBs
    let status = Command::new(bin_path("machina-irdump"))
        .args([
            guest_dhrystone().to_str().unwrap(),
            "--emit-bin",
            tmp_ir,
            "--count",
            "5",
        ])
        .status()
        .expect("machina-irdump failed");
    assert!(status.success());

    let output = Command::new(bin_path("machina-irbackend"))
        .arg(tmp_ir)
        .output()
        .expect("machina-irbackend failed");
    assert!(
        output.status.success(),
        "machina-irbackend failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    // Should report loading 5 TBs
    assert!(
        stderr.contains("loaded 5 TB(s)"),
        "expected 5 TBs loaded, got: {stderr}"
    );

    let _ = fs::remove_file(tmp_ir);
}
