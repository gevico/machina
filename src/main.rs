// machina: QEMU-style full-system emulator entry point.

mod difftest;

use std::env;
use std::path::PathBuf;
use std::process;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use machina_accel::exec::ExecEnv;
use machina_accel::x86_64::emitter::SoftMmuConfig;
use machina_accel::X86_64CodeGen;
use machina_core::machine::{Machine, MachineOpts};
use machina_hw_riscv::ref_machine::RefMachine;
use machina_hw_riscv::sifive_test::ShutdownReason;
use machina_system::cpus::{
    machina_mem_read, machina_mem_write, FullSystemCpu, LAST_TB_PC,
};
use machina_system::CpuManager;

fn usage() {
    eprintln!("Usage: machina [options]");
    eprintln!("Options:");
    eprintln!(
        "  -M machine    Machine type \
         (default: riscv64-ref)"
    );
    eprintln!("  -m size       RAM size in MiB (default: 128)");
    eprintln!("  -bios path    BIOS/firmware binary");
    eprintln!("  -kernel path  Kernel binary");
    eprintln!("  -nographic    Disable graphical output");
    eprintln!(
        "  --difftest    Instruction-level difftest \
         vs QEMU"
    );
    eprintln!("  -h, --help    Show this help");
}

struct CliArgs {
    machine: String,
    ram_mib: u64,
    bios: Option<PathBuf>,
    kernel: Option<PathBuf>,
    #[allow(dead_code)]
    nographic: bool,
    difftest: bool,
}

impl Default for CliArgs {
    fn default() -> Self {
        Self {
            machine: "riscv64-ref".to_string(),
            ram_mib: 128,
            bios: None,
            kernel: None,
            nographic: false,
            difftest: false,
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
            "--difftest" => {
                cli.difftest = true;
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

fn install_crash_handler() {
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction =
            crash_handler as usize;
        sa.sa_flags =
            libc::SA_SIGINFO | libc::SA_NODEFER;
        libc::sigaction(libc::SIGSEGV, &sa, std::ptr::null_mut());
    }
}

extern "C" fn crash_handler(
    _sig: libc::c_int,
    info: *mut libc::siginfo_t,
    ctx: *mut libc::c_void,
) {
    let pc = LAST_TB_PC.load(Ordering::Relaxed);
    let fault_addr = unsafe { (*info).si_addr() };
    let uctx = ctx as *const libc::ucontext_t;
    let rbp = unsafe {
        (*uctx).uc_mcontext.gregs
            [libc::REG_RBP as usize]
    };
    let rip = unsafe {
        (*uctx).uc_mcontext.gregs
            [libc::REG_RIP as usize]
    };
    eprintln!(
        "\nmachina: SIGSEGV at host {:#x}\n\
         rip={:#x} rbp={:#x}\n\
         last TB pc={:#x}",
        fault_addr as u64,
        rip as u64,
        rbp as u64,
        pc,
    );
    std::process::exit(139);
}

/// Run one machine cycle: init, boot, execute.
/// Returns the ShutdownReason if SiFive Test triggered,
/// or None if execution ended without shutdown device.
fn run_machine_cycle(
    opts: &MachineOpts,
    ram_size: u64,
) -> Option<ShutdownReason> {
    let mut machine = RefMachine::new();

    if let Err(e) = machine.init(opts) {
        eprintln!("machina: init failed: {}", e);
        process::exit(1);
    }

    if let Err(e) = machine.boot() {
        eprintln!("machina: boot failed: {}", e);
        process::exit(1);
    }

    // JIT backend with SoftMMU/TLB config.
    let mut backend = X86_64CodeGen::new();
    #[allow(unused_imports)]
    use machina_system::cpus::fault_pc_offset;
    use machina_system::cpus::{
        fault_cause_offset, tlb_offsets, tlb_ptr_offset, TLB_SIZE,
    };
    backend.mmio = Some(SoftMmuConfig {
        tlb_ptr_offset: tlb_ptr_offset(),
        entry_size: tlb_offsets::ENTRY_SIZE,
        addr_read_off: tlb_offsets::ADDR_READ,
        addr_write_off: tlb_offsets::ADDR_WRITE,
        addend_off: tlb_offsets::ADDEND,
        index_mask: (TLB_SIZE - 1) as u64,
        load_helper: machina_mem_read as *const () as u64,
        store_helper: machina_mem_write as *const () as u64,
        fault_cause_offset: fault_cause_offset(),
        fault_pc_offset: fault_pc_offset(),
        dirty_offset: tlb_offsets::DIRTY,
        tb_ret_addr: 0,
    });
    let env = ExecEnv::new(backend);
    let shared = env.shared.clone();

    let shared_mip = machine.shared_mip();
    let cpu0 = machine.take_cpu(0).expect("cpu0 must exist after boot");

    let ram_ptr = machine.ram_ptr();
    let wfi_waker = machine.wfi_waker();
    let as_ptr = machine.address_space()
        as *const machina_memory::address_space::AddressSpace;

    let mut cpu_mgr = CpuManager::new();
    cpu_mgr.set_wfi_waker(wfi_waker.clone());

    let stop_flag = cpu_mgr.running_flag();
    let mut fs_cpu = unsafe {
        FullSystemCpu::new(
            cpu0,
            ram_ptr,
            ram_size,
            shared_mip,
            wfi_waker.clone(),
            as_ptr,
            Arc::clone(&stop_flag),
        )
    };
    // Register MROM for instruction fetch at 0x1000.
    {
        use machina_hw_riscv::ref_machine::{MROM_BASE, MROM_SIZE};
        let mrom_ptr =
            machine.mrom_block().as_ptr() as *const u8;
        fs_cpu.set_mrom(mrom_ptr, MROM_BASE, MROM_SIZE);
    }
    cpu_mgr.add_cpu(fs_cpu);

    // Wire SiFive Test to execution control.
    let shutdown_reason: Arc<std::sync::Mutex<Option<ShutdownReason>>> =
        Arc::new(std::sync::Mutex::new(None));
    {
        let reason_slot = Arc::clone(&shutdown_reason);
        let flag = Arc::clone(&stop_flag);
        let wk = wfi_waker;
        machine
            .sifive_test()
            .set_shutdown_handler(Box::new(move |reason| {
                *reason_slot.lock().unwrap() = Some(reason);
                flag.store(false, Ordering::SeqCst);
                wk.stop();
            }));
    }

    let _exit = unsafe { cpu_mgr.run(&shared) };

    let result = shutdown_reason.lock().unwrap().take();
    result
}

fn main() {
    install_crash_handler();
    let cli = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("machina: {}", e);
            usage();
            process::exit(1);
        }
    };

    if cli.machine == "?" {
        eprintln!("Available machines:");
        eprintln!("  riscv64-ref    RISC-V reference machine");
        process::exit(0);
    }
    if cli.machine != "riscv64-ref" {
        eprintln!("machina: unknown machine: {}", cli.machine);
        process::exit(1);
    }

    let ram_size = cli.ram_mib * 1024 * 1024;
    let opts = MachineOpts {
        ram_size,
        cpu_count: 1,
        kernel: cli.kernel.clone(),
        bios: cli.bios.clone(),
        append: None,
        nographic: cli.nographic,
    };

    eprintln!("machina: riscv64-ref, {} MiB RAM", cli.ram_mib,);

    if cli.difftest {
        difftest::run_difftest(&opts, cli.ram_mib);
        return;
    }

    // Outer loop: supports machine reset via SiFive Test.
    loop {
        eprintln!("machina: entering execution loop");
        let reason = run_machine_cycle(&opts, ram_size);

        match reason {
            Some(ShutdownReason::Pass) => {
                eprintln!("machina: shutdown (pass)");
                process::exit(0);
            }
            Some(ShutdownReason::Reset) => {
                eprintln!("machina: reset, rebooting...");
                // Loop continues: re-init + re-boot.
            }
            Some(ShutdownReason::Fail(code)) => {
                eprintln!("machina: fail (code {:#x})", code);
                process::exit(1);
            }
            None => {
                eprintln!("machina: execution exited");
                process::exit(0);
            }
        }
    }
}
