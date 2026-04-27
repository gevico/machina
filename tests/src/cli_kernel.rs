//! Regression tests for #82: `-kernel` should fail fast with a clear
//! error when the file does not exist, instead of deferring the
//! failure to ELF load inside `machina_system`.

use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn machina_bin() -> PathBuf {
    let base = project_root().join("target").join("debug").join("machina");
    if cfg!(windows) {
        base.with_extension("exe")
    } else {
        base
    }
}

fn ensure_machina_built() {
    // Serialise concurrent builds: on Windows multiple linkers cannot
    // write the same .exe simultaneously.
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = LOCK.get_or_init(Mutex::default).lock().unwrap();

    let status = Command::new("cargo")
        .args(["build", "-p", "machina-emu"])
        .current_dir(project_root())
        .status()
        .expect("cargo build machina-emu failed");
    assert!(status.success(), "cargo build machina-emu failed");
}

#[test]
fn kernel_nonexistent_path_fails_fast() {
    ensure_machina_built();

    let bogus = "/this/path/does/not/exist/kernel.elf";
    let output = Command::new(machina_bin())
        .args(["-kernel", bogus])
        .output()
        .expect("failed to spawn machina");

    assert!(
        !output.status.success(),
        "machina should reject a missing -kernel path; got success exit",
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("-kernel: file not found"),
        "expected 'file not found' message in stderr; got: {stderr}",
    );
    assert!(
        stderr.contains(bogus),
        "expected the offending path in stderr; got: {stderr}",
    );
}
