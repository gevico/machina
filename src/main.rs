// machina: QEMU-style full-system emulator entry point.

use std::env;
use std::path::PathBuf;
use std::process;

use machina_core::machine::{Machine, MachineOpts};
use machina_hw_riscv::ref_machine::RefMachine;
use machina_system::CpuManager;

fn usage() {
    eprintln!("Usage: machina [options]");
    eprintln!("Options:");
    eprintln!("  -M machine    Machine type (default: riscv64-ref)");
    eprintln!("  -m size       RAM size in MiB (default: 128)");
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
                cli.machine =
                    args.get(i).ok_or("-M requires argument")?.clone();
            }
            "-m" => {
                i += 1;
                let s = args.get(i).ok_or("-m requires argument")?;
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
                        .ok_or("-kernel requires argument")?
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
                return Err(format!("Unknown option: {}", other));
            }
        }
        i += 1;
    }
    Ok(cli)
}

fn create_machine(name: &str) -> Result<Box<dyn Machine>, String> {
    match name {
        "riscv64-ref" | "" => Ok(Box::new(RefMachine::new())),
        _ => Err(format!("Unknown machine: {}", name)),
    }
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

    let mut machine = match create_machine(&cli.machine) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("machina: {}", e);
            process::exit(1);
        }
    };

    let opts = MachineOpts {
        ram_size: cli.ram_mib * 1024 * 1024,
        cpu_count: 1,
        kernel: cli.kernel,
        bios: cli.bios,
        append: None,
    };

    if let Err(e) = machine.init(&opts) {
        eprintln!("machina: init failed: {}", e);
        process::exit(1);
    }

    eprintln!(
        "machina: {} initialized, {} MiB RAM",
        machine.name(),
        cli.ram_mib
    );

    if let Err(e) = machine.boot() {
        eprintln!("machina: boot failed: {}", e);
        process::exit(1);
    }

    let cpu_mgr = CpuManager::new(1);
    if let Err(e) = cpu_mgr.run(machine.as_mut()) {
        eprintln!("machina: execution error: {}", e);
        process::exit(1);
    }
}
