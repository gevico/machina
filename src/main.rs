// machina: QEMU-style full-system emulator entry point.

use std::env;
use std::path::PathBuf;
use std::process;
use std::sync::Arc;

use machina_accel::exec::ExecEnv;
use machina_accel::X86_64CodeGen;
use machina_core::machine::{Machine, MachineOpts};
use machina_hw_riscv::ref_machine::RefMachine;
use machina_system::cpus::{FullSystemCpu, WfiWaker};
use machina_system::CpuManager;

fn usage() {
    eprintln!("Usage: machina [options]");
    eprintln!("Options:");
    eprintln!(
        "  -M machine    Machine type \
         (default: riscv64-ref)"
    );
    eprintln!(
        "  -m size       RAM size in MiB (default: 128)"
    );
    eprintln!("  -bios path    BIOS/firmware binary");
    eprintln!("  -kernel path  Kernel binary");
    eprintln!("  -nographic    Disable graphical output");
    eprintln!("  -h, --help    Show this help");
}

struct CliArgs {
    machine: String,
    ram_mib: u64,
    bios: Option<PathBuf>,
    kernel: Option<PathBuf>,
    #[allow(dead_code)]
    nographic: bool,
}

impl Default for CliArgs {
    fn default() -> Self {
        Self {
            machine: "riscv64-ref".to_string(),
            ram_mib: 128,
            bios: None,
            kernel: None,
            nographic: false,
        }
    }
}

fn parse_args() -> Result<CliArgs, String> {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut cli = CliArgs::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-M" | "-machine" => {
                i += 1;
                cli.machine = args
                    .get(i)
                    .ok_or("-M requires argument")?
                    .clone();
            }
            "-m" => {
                i += 1;
                let s = args
                    .get(i)
                    .ok_or("-m requires argument")?;
                cli.ram_mib = s
                    .trim_end_matches('M')
                    .parse::<u64>()
                    .map_err(|e| format!("-m: {}", e))?;
            }
            "-bios" => {
                i += 1;
                cli.bios = Some(
                    args.get(i)
                        .ok_or("-bios requires argument")?
                        .clone()
                        .into(),
                );
            }
            "-kernel" => {
                i += 1;
                cli.kernel = Some(
                    args.get(i)
                        .ok_or(
                            "-kernel requires argument",
                        )?
                        .clone()
                        .into(),
                );
            }
            "-nographic" => {
                cli.nographic = true;
            }
            "-h" | "--help" => {
                usage();
                process::exit(0);
            }
            other => {
                return Err(format!(
                    "Unknown option: {}",
                    other
                ));
            }
        }
        i += 1;
    }
    Ok(cli)
}

fn main() {
    let cli = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("machina: {}", e);
            usage();
            process::exit(1);
        }
    };

    // Machine selection.
    if cli.machine == "?" {
        eprintln!("Available machines:");
        eprintln!(
            "  riscv64-ref    RISC-V reference machine"
        );
        process::exit(0);
    }
    if cli.machine != "riscv64-ref" {
        eprintln!(
            "machina: unknown machine: {}",
            cli.machine
        );
        process::exit(1);
    }

    let mut machine = RefMachine::new();

    let opts = MachineOpts {
        ram_size: cli.ram_mib * 1024 * 1024,
        cpu_count: 1,
        kernel: cli.kernel.clone(),
        bios: cli.bios.clone(),
        append: None,
    };

    if let Err(e) = machine.init(&opts) {
        eprintln!("machina: init failed: {}", e);
        process::exit(1);
    }

    if let Err(e) = machine.boot() {
        eprintln!("machina: boot failed: {}", e);
        process::exit(1);
    }

    eprintln!(
        "machina: {} booted, {} MiB RAM",
        machine.name(),
        cli.ram_mib
    );

    // Create JIT backend.
    let backend = X86_64CodeGen::new();
    let env = ExecEnv::new(backend);
    let shared = env.shared.clone();

    // Take CPU0 state from machine for execution.
    // Get shared_mip from machine — same atomic that
    // device IRQ sinks write to.
    let shared_mip = machine.shared_mip();
    let cpus = machine.cpus_shared();
    let cpu0 = {
        let mut lock = cpus.lock().unwrap();
        lock.remove(0)
    };

    let ram_ptr = machine.ram_ptr();
    let ram_size = machine.ram_size();

    let wfi_waker = Arc::new(WfiWaker::new());
    let mut fs_cpu = unsafe {
        FullSystemCpu::new(
            cpu0,
            ram_ptr,
            ram_size,
            shared_mip,
            wfi_waker,
        )
    };

    let cpu_mgr = CpuManager::new();

    eprintln!(
        "machina: cpu0 pc=0x{:x} priv={}, \
         entering execution loop",
        fs_cpu.cpu.pc,
        fs_cpu.cpu.priv_level as u8
    );

    // Block in the execution loop.
    let exit =
        unsafe { cpu_mgr.run_cpu(&mut fs_cpu, &shared) };

    eprintln!("machina: execution exited: {:?}", exit);
}
