//! Tests for the loongarch64-ref test-finisher device (#152).
//!
//! The device lets a bare-metal LoongArch test report pass/fail by an
//! MMIO write (the machine has no other way to terminate a run cleanly).
//! The unit test pins the magic-word decoding; the Unix-only integration
//! test runs the committed smoke binary end-to-end and asserts machina
//! exits 0.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

use machina_hw_loongarch::test_finisher::{
    LoongArchTestFinisher, LoongArchTestFinisherMmio, TEST_FINISHER_FAIL,
    TEST_FINISHER_PASS,
};
use machina_memory::region::MmioOps;

#[test]
fn finisher_fires_only_on_magic_words() {
    let finisher = LoongArchTestFinisher::new();
    // -1 = not fired, 1 = pass, 0 = fail.
    let result = Arc::new(AtomicI32::new(-1));
    {
        let slot = Arc::clone(&result);
        finisher.set_finish_handler(Box::new(move |pass| {
            slot.store(i32::from(pass), Ordering::SeqCst);
        }));
    }
    let mmio = LoongArchTestFinisherMmio(Arc::clone(&finisher));

    // A non-magic write must not terminate the run.
    mmio.write(0, 4, 0x1234);
    assert_eq!(result.load(Ordering::SeqCst), -1);

    mmio.write(0, 4, u64::from(TEST_FINISHER_PASS));
    assert_eq!(result.load(Ordering::SeqCst), 1);

    mmio.write(0, 4, u64::from(TEST_FINISHER_FAIL));
    assert_eq!(result.load(Ordering::SeqCst), 0);

    // A magic value written outside the single 32-bit register at
    // offset 0 (wrong offset or access width) must be ignored.
    let prev = result.load(Ordering::SeqCst);
    mmio.write(4, 4, u64::from(TEST_FINISHER_PASS));
    mmio.write(0, 8, u64::from(TEST_FINISHER_PASS));
    assert_eq!(result.load(Ordering::SeqCst), prev);

    // The register reads back as zero.
    assert_eq!(mmio.read(0, 4), 0);
}

// The end-to-end smoke spawns the machina binary to run loongarch64-ref.
// It is Unix-only: a full-system loongarch64-ref run overflows the default
// Windows thread stack (a pre-existing platform limitation unrelated to
// the finisher device), and the loongarch-tests CI runs on Linux anyway.
#[cfg(unix)]
mod smoke {
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::{Mutex, OnceLock};

    fn project_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
    }

    fn machina_bin() -> PathBuf {
        project_root().join("target").join("debug").join("machina")
    }

    fn ensure_machina_built() {
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
    fn loongarch_smoke_binary_passes_via_finisher() {
        ensure_machina_built();
        let smoke = project_root()
            .join("tests")
            .join("firmware")
            .join("loongarch_smoke.bin");

        let output = Command::new(machina_bin())
            .args([
                "-M",
                "loongarch64-ref",
                "-m",
                "128",
                "-kernel",
                smoke.to_str().unwrap(),
                "-nographic",
            ])
            .output()
            .expect("failed to spawn machina");

        assert!(
            output.status.success(),
            "loongarch smoke should pass (exit 0); status={:?} stderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}
