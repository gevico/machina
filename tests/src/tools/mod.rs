//! Integration tests for machina-irdump --emit-bin and machina-irbackend.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn bin_path(name: &str) -> PathBuf {
    let base = project_root().join("target").join("debug").join(name);
    // On Windows executables have an .exe extension.
    if cfg!(windows) {
        base.with_extension("exe")
    } else {
        base
    }
}

fn guest_elf() -> PathBuf {
    project_root().join("tests/firmware/sbi_smoke.elf")
}

fn tmp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(name)
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
    let tmp = tmp_path("tcg-test-irdump.tcgir");
    let _ = fs::remove_file(&tmp);

    let status = Command::new(bin_path("machina-irdump"))
        .args([
            guest_elf().to_str().unwrap(),
            "--emit-bin",
            tmp.to_str().unwrap(),
            "--count",
            "2",
        ])
        .status()
        .expect("machina-irdump failed to run");
    assert!(status.success(), "machina-irdump exited with error");

    let data = fs::read(&tmp).expect("output file missing");
    // Verify magic header
    assert!(data.len() > 20, "file too small");
    assert_eq!(&data[..4], b"TCIR");

    let _ = fs::remove_file(&tmp);
}

#[test]
fn irbackend_hex_dump() {
    ensure_built();
    let tmp_ir = tmp_path("tcg-test-irbackend.tcgir");
    let _ = fs::remove_file(&tmp_ir);

    // Generate IR
    let status = Command::new(bin_path("machina-irdump"))
        .args([
            guest_elf().to_str().unwrap(),
            "--emit-bin",
            tmp_ir.to_str().unwrap(),
            "--count",
            "1",
        ])
        .status()
        .expect("machina-irdump failed");
    assert!(status.success());

    // Run backend
    let output = Command::new(bin_path("machina-irbackend"))
        .arg(&tmp_ir)
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

    let _ = fs::remove_file(&tmp_ir);
}

#[test]
fn irbackend_raw_output() {
    ensure_built();
    let tmp_ir = tmp_path("tcg-test-irbackend-raw.tcgir");
    let tmp_bin = tmp_path("tcg-test-irbackend-raw.bin");
    let _ = fs::remove_file(&tmp_ir);
    let _ = fs::remove_file(&tmp_bin);

    // Generate IR
    let status = Command::new(bin_path("machina-irdump"))
        .args([
            guest_elf().to_str().unwrap(),
            "--emit-bin",
            tmp_ir.to_str().unwrap(),
            "--count",
            "1",
        ])
        .status()
        .expect("machina-irdump failed");
    assert!(status.success());

    // Run backend with --raw -o
    let status = Command::new(bin_path("machina-irbackend"))
        .args([
            tmp_ir.to_str().unwrap(),
            "--raw",
            "-o",
            tmp_bin.to_str().unwrap(),
        ])
        .status()
        .expect("machina-irbackend failed");
    assert!(status.success());

    let data = fs::read(&tmp_bin).expect("raw output missing");
    assert!(!data.is_empty(), "raw output should not be empty");

    let _ = fs::remove_file(&tmp_ir);
    let _ = fs::remove_file(&tmp_bin);
}

#[test]
fn irbackend_multiple_tbs() {
    ensure_built();
    let tmp_ir = tmp_path("tcg-test-irbackend-multi.tcgir");
    let _ = fs::remove_file(&tmp_ir);

    // Generate 5 TBs
    let status = Command::new(bin_path("machina-irdump"))
        .args([
            guest_elf().to_str().unwrap(),
            "--emit-bin",
            tmp_ir.to_str().unwrap(),
            "--count",
            "5",
        ])
        .status()
        .expect("machina-irdump failed");
    assert!(status.success());

    let output = Command::new(bin_path("machina-irbackend"))
        .arg(&tmp_ir)
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

    let _ = fs::remove_file(&tmp_ir);
}

fn ensure_machina_built() {
    // Serialise concurrent builds: on Windows multiple linkers
    // cannot write the same .exe simultaneously.
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = LOCK.get_or_init(Mutex::default).lock().unwrap();

    let status = Command::new("cargo")
        .args(["build", "-p", "machina-emu"])
        .current_dir(project_root())
        .status()
        .expect("cargo build machina-emu failed");
    assert!(status.success(), "cargo build machina-emu failed");
}

fn sbi_smoke_bin() -> PathBuf {
    project_root().join("tests/firmware/sbi_smoke.bin")
}

/// End-to-end boot test: bundled RustSBI Prototyper +
/// sbi_smoke.bin payload. Asserts RustSBI banner and
/// MACHINA_SBI_OK marker in combined output.
#[test]
fn boot_rustsbi_with_sbi_smoke_payload() {
    ensure_machina_built();

    let child = Command::new(bin_path("machina"))
        .args([
            "-M",
            "riscv64-ref",
            "-m",
            "128",
            "-kernel",
            sbi_smoke_bin().to_str().unwrap(),
            "-nographic",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("machina process failed to start");

    let output = child
        .wait_with_output()
        .expect("failed to wait for machina");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    assert!(
        combined.contains("RustSBI"),
        "output must contain RustSBI banner.\n\
         combined output:\n{}",
        combined,
    );
    assert!(
        combined.contains("MACHINA_SBI_OK"),
        "output must contain MACHINA_SBI_OK marker.\n\
         combined output:\n{}",
        combined,
    );
    assert!(
        output.status.success(),
        "machina must exit with code 0.\n\
         exit status: {:?}\ncombined output:\n{}",
        output.status,
        combined,
    );
}

fn sifive_pass_bin() -> PathBuf {
    project_root().join("tests/firmware/sifive_pass.bin")
}

fn sifive_reset_bin() -> PathBuf {
    project_root().join("tests/firmware/sifive_reset.bin")
}

/// SiFive Test PASS: bare-metal kernel writes 0x5555,
/// machina should exit cleanly with code 0.
#[test]
fn sifive_test_pass_clean_exit() {
    ensure_machina_built();
    let child = Command::new(bin_path("machina"))
        .args([
            "-M",
            "riscv64-ref",
            "-m",
            "128",
            "-bios",
            "none",
            "-kernel",
            sifive_pass_bin().to_str().unwrap(),
            "-nographic",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("machina failed to start");
    let output = child.wait_with_output().expect("wait failed");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        combined.contains("shutdown (pass)"),
        "expected 'shutdown (pass)' in output.\n{}",
        combined,
    );
    assert!(
        output.status.success(),
        "PASS must exit with code 0.\n\
         status: {:?}\n{}",
        output.status,
        combined,
    );
}

/// SiFive Test RESET: bare-metal kernel writes 0x3333,
/// machina should reboot and re-enter execution loop.
#[test]
fn sifive_test_reset_reboots() {
    ensure_machina_built();
    let mut child = Command::new(bin_path("machina"))
        .args([
            "-M",
            "riscv64-ref",
            "-m",
            "128",
            "-bios",
            "none",
            "-kernel",
            sifive_reset_bin().to_str().unwrap(),
            "-nographic",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("machina failed to start");

    // Give it 3 seconds to reboot a few times.
    std::thread::sleep(std::time::Duration::from_secs(3));

    // Kill the process (it will loop forever).
    let _ = child.kill();

    let output = child.wait_with_output().expect("wait failed");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    // Must see at least 2 reboot cycles.
    let reboot_count = combined.matches("reset, rebooting").count();
    assert!(
        reboot_count >= 2,
        "expected >= 2 reboot cycles, got {}.\n{}",
        reboot_count,
        combined,
    );
}
