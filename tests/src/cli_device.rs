//! Regression tests for #85: recognised-but-ignored `-device` values
//! used to be dropped without feedback. `virtio-blk-device` is wired
//! up via `-drive`, so machina now warns and points at the right
//! option instead of silently ignoring it, while genuinely unknown
//! device classes are still rejected outright.

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

/// Pair the device under test with a sentinel `-kernel` arg that fails
/// fast, so machina never boots but warnings still land in stderr.
fn run_with_device(value: &str) -> String {
    ensure_machina_built();

    let output = Command::new(machina_bin())
        .args(["-device", value, "-kernel", "/no/such/kernel.img"])
        .output()
        .expect("failed to spawn machina");

    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn virtio_blk_device_hints_at_drive() {
    let stderr = run_with_device("virtio-blk-device,drive=hd0");
    assert!(
        stderr.contains("warning")
            && stderr.contains("virtio-blk-device")
            && stderr.contains("-drive"),
        "expected -drive hint for virtio-blk-device; got: {stderr}",
    );
    assert!(
        !stderr.contains("panicked at"),
        "expected friendly warning, not panic; got: {stderr}",
    );
}

#[test]
fn unknown_device_class_is_rejected() {
    let stderr = run_with_device("nonexistent-device-xyz");
    assert!(
        stderr.contains("unsupported device")
            && stderr.contains("nonexistent-device-xyz"),
        "expected unsupported-device error naming the class; got: \
         {stderr}",
    );
}

#[cfg(unix)]
#[test]
fn virtio_net_device_recognised_on_unix() {
    // virtio-net-device is a recognised class on Unix: it must not be
    // reported as unsupported nor confused with the block-device path.
    let stderr = run_with_device("virtio-net-device,netdev=net0");
    assert!(
        !stderr.contains("unsupported device")
            && !stderr.contains("warning: -device virtio-blk"),
        "Unix host should recognise virtio-net-device; got: {stderr}",
    );
}
