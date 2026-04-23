// machina: QEMU-style full-system emulator entry point.

#[cfg(unix)]
mod difftest;

use std::env;
use std::path::PathBuf;
use std::process;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use machina_accel::exec::ExecEnv;
use machina_accel::x86_64::emitter::SoftMmuConfig;
use machina_accel::X86_64CodeGen;
#[cfg(unix)]
use machina_core::machine::NetdevOpts;
use machina_core::machine::{Machine, MachineOpts};
use machina_hw_riscv::ref_machine::RefMachine;
use machina_hw_riscv::sbi::SbiBackend;
use machina_hw_riscv::sifive_test::ShutdownReason;
#[cfg(unix)]
use machina_system::cpus::LAST_TB_PC;
use machina_system::cpus::{
    machina_mem_read, machina_mem_write, FullSystemCpu,
};
use machina_system::{CpuManager, FirmwareCallFn};

fn usage() {
    eprintln!("Usage: machina [options]");
    eprintln!("Options:");
    eprintln!(
        "  -M machine    Machine type \
         (default: riscv64-ref)"
    );
    eprintln!("  -m size       RAM size in MiB (default: 128)");
    eprintln!("  -bios path    BIOS/firmware binary");
    eprintln!(
        "  -bios builtin Boot directly in S-mode \
         with host-side SBI"
    );
    eprintln!("  -kernel path  Kernel binary");
    eprintln!("  -nographic    Disable graphical output");
    eprintln!("  -append args  Kernel command line arguments");
    #[cfg(unix)]
    eprintln!(
        "  --difftest    Instruction-level difftest \
         vs QEMU"
    );
    eprintln!("  -drive file=<path>  Attach raw disk image");
    eprintln!("  -monitor stdio|tcp:host:port  Monitor console");
    eprintln!("  -s            Shorthand for -gdb tcp::1234");
    eprintln!("  -S            Freeze CPU at startup");
    eprintln!(
        "  -gdb dev      GDB server device \
         (e.g. tcp::1234)"
    );
    eprintln!(
        "  -netdev tap,id=<id>,ifname=<name>  \
         TAP network backend"
    );
    eprintln!(
        "  -device virtio-net-device,netdev=<id>\
         [,mac=XX:XX:XX:XX:XX:XX]"
    );
    eprintln!("  --trace file  Trace output file");
    eprintln!("  -h, --help    Show this help");
}

struct CliArgs {
    machine: String,
    ram_mib: u64,
    bios: Option<PathBuf>,
    bios_builtin: bool,
    kernel: Option<PathBuf>,
    append: Option<String>,
    nographic: bool,
    #[cfg(unix)]
    difftest: bool,
    drive: Option<PathBuf>,
    monitor: Option<String>,
    gdb: Option<String>,
    start_paused: bool,
    initrd: Option<PathBuf>,
    #[cfg(unix)]
    netdev_raw: Option<String>,
    #[cfg(unix)]
    device_net_raw: Option<String>,
    trace: Option<PathBuf>,
}

impl Default for CliArgs {
    fn default() -> Self {
        Self {
            machine: "riscv64-ref".to_string(),
            ram_mib: 128,
            bios: None,
            bios_builtin: false,
            kernel: None,
            append: None,
            nographic: false,
            #[cfg(unix)]
            difftest: false,
            drive: None,
            monitor: None,
            gdb: None,
            start_paused: false,
            initrd: None,
            #[cfg(unix)]
            netdev_raw: None,
            #[cfg(unix)]
            device_net_raw: None,
            trace: None,
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
                let arg = args.get(i).ok_or("-bios requires argument")?.clone();
                if arg == "builtin" {
                    cli.bios_builtin = true;
                    cli.bios = None;
                } else {
                    cli.bios_builtin = false;
                    cli.bios = Some(arg.into());
                }
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
            "-append" => {
                i += 1;
                cli.append = Some(
                    args.get(i).ok_or("-append requires argument")?.clone(),
                );
            }
            #[cfg(unix)]
            "--difftest" => {
                cli.difftest = true;
            }
            "-drive" => {
                i += 1;
                let s = args.get(i).ok_or("-drive requires argument")?;
                // Parse file=<path> from the option
                // string (ignore if=, format=, id=).
                let mut path = None;
                for part in s.split(',') {
                    if let Some(p) = part.strip_prefix("file=") {
                        path = Some(p.to_string());
                    }
                }
                cli.drive = path.map(PathBuf::from);
                if cli.drive.is_none() {
                    return Err("-drive: missing file=<path>".to_string());
                }
            }
            #[cfg(unix)]
            "-netdev" => {
                i += 1;
                cli.netdev_raw = Some(
                    args.get(i).ok_or("-netdev requires argument")?.clone(),
                );
            }
            "-device" => {
                i += 1;
                let val = args.get(i).ok_or("-device requires argument")?;
                // virtio-net-device is Unix-only (TAP backend).
                #[cfg(unix)]
                if val.starts_with("virtio-net-device,") {
                    cli.device_net_raw = Some(val.clone());
                }
                // virtio-blk-device and other devices are
                // handled via -drive; accept without error.
                let _ = val;
            }
            "--trace" => {
                i += 1;
                let s = args.get(i).ok_or("--trace requires argument")?;
                cli.trace = Some(PathBuf::from(s));
            }
            "-initrd" => {
                i += 1;
                let s = args.get(i).ok_or("-initrd requires argument")?;
                cli.initrd = Some(PathBuf::from(s));
            }
            "-monitor" => {
                i += 1;
                let s = args.get(i).ok_or("-monitor requires argument")?;
                if s == "stdio" || s.starts_with("tcp:") {
                    cli.monitor = Some(s.clone());
                } else {
                    return Err(format!("-monitor: unsupported: {}", s));
                }
            }
            "-s" => {
                cli.gdb = Some("tcp::1234".to_string());
            }
            "-S" => {
                cli.start_paused = true;
            }
            "-gdb" => {
                i += 1;
                let s = args.get(i).ok_or("-gdb requires argument")?;
                if s.starts_with("tcp:") || s == "stdio" {
                    cli.gdb = Some(s.clone());
                } else {
                    return Err(format!("-gdb: unsupported: {}", s));
                }
            }
            "-h" | "--help" => {
                usage();
                machina_hw_core::chardev::restore_terminal();
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

// parse_netdev_opts moved to NetdevOpts::parse() in
// core/src/machine.rs for testability.

/// Install a SIGSEGV handler that prints the last TB PC
/// and host register state before exiting.
/// On Windows there is no SIGSEGV; this is a no-op.
#[cfg(unix)]
fn install_crash_handler() {
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = crash_handler as *const () as usize;
        sa.sa_flags = libc::SA_SIGINFO | libc::SA_NODEFER;
        libc::sigaction(libc::SIGSEGV, &sa, std::ptr::null_mut());
    }
}

#[cfg(not(unix))]
fn install_crash_handler() {
    // No POSIX signals on Windows; crash handler not installed.
}

#[cfg(unix)]
extern "C" fn crash_handler(
    _sig: libc::c_int,
    info: *mut libc::siginfo_t,
    ctx: *mut libc::c_void,
) {
    let pc = LAST_TB_PC.load(Ordering::Relaxed);
    let fault_addr = unsafe { (*info).si_addr() };
    let uctx = ctx as *const libc::ucontext_t;
    let rbp = unsafe { (*uctx).uc_mcontext.gregs[libc::REG_RBP as usize] };
    let rip = unsafe { (*uctx).uc_mcontext.gregs[libc::REG_RIP as usize] };
    machina_hw_core::chardev::restore_terminal();
    eprintln!(
        "\nmachina: SIGSEGV at host {:#x}\n\
         rip={:#x} rbp={:#x}\n\
         last TB pc={:#x}",
        fault_addr as u64, rip as u64, rbp as u64, pc,
    );
    machina_hw_core::chardev::restore_terminal();
    std::process::exit(139);
}

/// Run one machine cycle: init, boot, execute.
/// Returns the ShutdownReason if SiFive Test or HTIF
/// triggered, or None if execution ended without either.
fn run_machine_cycle(
    opts: &MachineOpts,
    ram_size: u64,
    monitor_state: Option<Arc<machina_core::monitor::MonitorState>>,
    monitor_svc: Arc<
        std::sync::Mutex<machina_monitor::service::MonitorService>,
    >,
    htif_tohost: Option<u64>,
    gdb_state: Option<Arc<machina_system::gdb::GdbState>>,
) -> Option<ShutdownReason> {
    let mut machine = RefMachine::new();

    // Set Ctrl+A X quit callback + Ctrl+A C monitor mux.
    if let Some(ref ms) = monitor_state {
        let ms_quit = Arc::clone(ms);
        machine.set_quit_cb(Arc::new(move || {
            ms_quit.request_quit();
        }));

        // Ctrl+A C: route input bytes to HMP console.
        // Buffer bytes into lines, dispatch via HMP.
        let mon_svc = Arc::clone(&monitor_svc);
        let line_buf = Arc::new(std::sync::Mutex::new(String::new()));
        let mon_cb: Arc<std::sync::Mutex<dyn FnMut(u8) + Send>> =
            Arc::new(std::sync::Mutex::new(move |byte: u8| {
                use std::io::Write;
                let ch = byte as char;
                let mut buf = line_buf.lock().unwrap();
                if ch == '\r' || ch == '\n' {
                    let line = buf.clone();
                    buf.clear();
                    // quit handled
                    if let Some(output) =
                        machina_monitor::hmp::handle_line(&line, &mon_svc)
                    {
                        let mut out = std::io::stderr().lock();
                        let _ = write!(out, "\r{}", output);
                        let _ = write!(out, "{}", machina_monitor::hmp::PROMPT);
                        let _ = out.flush();
                    }
                } else if byte == 0x7f || byte == 0x08 {
                    // Backspace.
                    buf.pop();
                    eprint!("\x08 \x08");
                } else {
                    buf.push(ch);
                    eprint!("{}", ch);
                }
            }));
        machine.set_monitor_cb(mon_cb);
    }

    if let Err(e) = machine.init(opts) {
        machina_hw_core::chardev::restore_terminal();
        eprintln!("machina: init failed: {}", e);
        machina_hw_core::chardev::restore_terminal();
        process::exit(1);
    }

    if let Err(e) = machine.boot() {
        eprintln!("machina: boot failed: {}", e);
        machina_hw_core::chardev::restore_terminal();
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
    backend.neg_align_off = machina_system::cpus::neg_align_offset();
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
    let ram_base = machina_hw_riscv::ref_machine::RAM_BASE;
    let mut fs_cpu = unsafe {
        FullSystemCpu::new(
            cpu0,
            ram_ptr,
            ram_base,
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
        let mrom_ptr = machine.mrom_block().as_ptr() as *const u8;
        fs_cpu.set_mrom(mrom_ptr, MROM_BASE, MROM_SIZE);
    }
    // Configure HTIF tohost polling if provided.
    if let Some(tohost_gpa) = htif_tohost {
        fs_cpu.set_htif_tohost(tohost_gpa);
    }
    // Set code-page bitmap for store helper.
    fs_cpu.set_code_pages(
        shared.tb_store.code_pages_ptr(),
        shared.tb_store.code_pages_len(),
    );
    let htif_exit = fs_cpu.htif_exit_code();
    if let Some(ref ms) = monitor_state {
        ms.set_wfi_waker(wfi_waker.clone());
        ms.set_stop_flag(Arc::clone(&stop_flag));
        fs_cpu.set_monitor_state(Arc::clone(ms));
    }
    if let Some(ref gs) = gdb_state {
        fs_cpu.set_gdb_state(Arc::clone(gs));
        gs.set_mem_access(ram_ptr, ram_size, ram_base, as_ptr as u64);
    }
    cpu_mgr.add_cpu(fs_cpu);
    // Connect neg_align pointer to ACLINT so timer
    // interrupts can break goto_tb chains. Must be
    // AFTER add_cpu because the move changes the
    // address of the RiscvCpu/neg_align field.
    {
        let ptr = cpu_mgr.cpu(0).neg_align_ptr();
        machine.aclint().connect_neg_align(0, ptr);
        // Also give the pointer to MonitorState so
        // request_quit can break goto_tb chains.
        if let Some(ref ms) = monitor_state {
            ms.set_neg_align_ptr(ptr);
        }
    }

    // Shared shutdown reason slot (used by both SBI
    // and SiFive Test).
    let shutdown_reason: Arc<std::sync::Mutex<Option<ShutdownReason>>> =
        Arc::new(std::sync::Mutex::new(None));

    // Wire builtin SBI backend (-bios builtin).
    if opts.bios_builtin {
        let uart = machine.uart().clone();
        let aclint = machine.aclint().clone();
        let sbi_stop = Arc::clone(&stop_flag);
        let sbi_wk = machine.wfi_waker();
        let sbi_reason = Arc::clone(&shutdown_reason);
        let shutdown_cb: Arc<dyn Fn(u32) + Send + Sync> =
            Arc::new(move |reset_type| {
                let reason = match reset_type {
                    1 | 2 => ShutdownReason::Reset,
                    _ => ShutdownReason::Pass,
                };
                *sbi_reason.lock().unwrap() = Some(reason);
                sbi_stop.store(false, Ordering::SeqCst);
                sbi_wk.stop();
            });
        let backend = Arc::new(SbiBackend::new(uart, aclint, shutdown_cb));
        let fw_fn: FirmwareCallFn =
            Arc::new(move |cpu| backend.handle_call(cpu));
        cpu_mgr.set_firmware_handler(fw_fn);
        cpu_mgr.cpu_mut(0).builtin_mode = true;
    }

    // Wire SiFive Test to execution control.
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

    // Cancel all pending ACLINT timer threads immediately
    // so they do not write through the neg_align pointer
    // after cpu_mgr is dropped at function return.
    // Any sleeping timer thread that wakes up after this
    // point will see a bumped cancel generation and exit
    // without touching the CPU's neg_align field.
    machine.aclint().cancel_timers();

    // Invalidate the CPU pointer in MonitorState so
    // a late quit does not dereference freed memory.
    if let Some(ref ms) = monitor_state {
        ms.set_neg_align_ptr(0);
    }

    // Check SiFive Test first.
    let result = shutdown_reason.lock().unwrap().take();
    if result.is_some() {
        return result;
    }
    // Check HTIF tohost exit code.
    let code = htif_exit.load(Ordering::SeqCst);
    if code != 0 {
        if code == 1 {
            return Some(ShutdownReason::Pass);
        } else {
            return Some(ShutdownReason::Fail(code as u32));
        }
    }
    None
}

fn main() {
    install_crash_handler();
    let cli = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("machina: {}", e);
            usage();
            machina_hw_core::chardev::restore_terminal();
            process::exit(1);
        }
    };

    if cli.machine == "?" {
        eprintln!("Available machines:");
        eprintln!("  riscv64-ref    RISC-V reference machine");
        machina_hw_core::chardev::restore_terminal();
        process::exit(0);
    }
    if cli.machine != "riscv64-ref" {
        eprintln!("machina: unknown machine: {}", cli.machine);
        machina_hw_core::chardev::restore_terminal();
        process::exit(1);
    }

    // Reject -device virtio-net-device without -netdev.
    #[cfg(unix)]
    if cli.device_net_raw.is_some() && cli.netdev_raw.is_none() {
        eprintln!(
            "machina: -device virtio-net-device \
             requires a matching -netdev"
        );
        machina_hw_core::chardev::restore_terminal();
        process::exit(1);
    }

    // Parse netdev options if provided (Unix only: TAP is
    // POSIX-specific).
    #[cfg(unix)]
    let netdev = if let Some(ref raw) = cli.netdev_raw {
        match NetdevOpts::parse(raw, cli.device_net_raw.as_deref()) {
            Ok(nd) => Some(nd),
            Err(e) => {
                eprintln!("machina: {}", e);
                machina_hw_core::chardev::restore_terminal();
                process::exit(1);
            }
        }
    } else {
        None
    };
    #[cfg(not(unix))]
    let netdev = None;

    if let Some(ref trace_path) = cli.trace {
        if let Err(e) =
            machina_util::trace::init_trace(trace_path.to_str().unwrap_or(""))
        {
            eprintln!("machina: --trace: {}", e);
            std::process::exit(1);
        }
    }

    let ram_size = cli.ram_mib * 1024 * 1024;
    let opts = MachineOpts {
        ram_size,
        cpu_count: 1,
        kernel: cli.kernel.clone(),
        bios: cli.bios.clone(),
        bios_builtin: cli.bios_builtin,
        append: cli.append.clone(),
        nographic: cli.nographic,
        drive: cli.drive.clone(),
        initrd: cli.initrd.clone(),
        netdev,
    };

    // Check -monitor stdio + -nographic conflict.
    if cli.monitor.as_deref() == Some("stdio") && cli.nographic {
        machina_hw_core::chardev::restore_terminal();
        eprintln!(
            "machina: -monitor stdio and -nographic \
             are mutually exclusive"
        );
        machina_hw_core::chardev::restore_terminal();
        process::exit(1);
    }

    // Find HTIF tohost symbol from kernel ELF.
    let htif_tohost: Option<u64> = cli.kernel.as_ref().and_then(|p| {
        let data = std::fs::read(p).ok()?;
        let addr = machina_hw_core::loader::elf_find_symbol(&data, "tohost")?;
        let bias = if machina_hw_core::loader::elf_is_dyn(&data) {
            use machina_hw_riscv::boot::KERNEL_OFFSET;
            let bios_none = cli
                .bios
                .as_ref()
                .is_some_and(|p| p.to_str() == Some("none"));
            let has_fw = !cli.bios_builtin && !bios_none;
            if has_fw {
                machina_hw_riscv::ref_machine::RAM_BASE + KERNEL_OFFSET
            } else {
                machina_hw_riscv::ref_machine::RAM_BASE
            }
        } else {
            0
        };
        Some(addr + bias)
    });

    eprintln!("machina: riscv64-ref, {} MiB RAM", cli.ram_mib,);
    if let Some(addr) = htif_tohost {
        eprintln!("machina: HTIF tohost at {:#x}", addr);
    }

    #[cfg(unix)]
    if cli.difftest {
        if cli.bios_builtin {
            eprintln!(
                "machina: --difftest is incompatible \
                 with -bios builtin"
            );
            machina_hw_core::chardev::restore_terminal();
            process::exit(1);
        }
        difftest::run_difftest(&opts, cli.ram_mib);
        return;
    }

    // Create shared monitor state and service.
    let monitor_state = Arc::new(machina_core::monitor::MonitorState::new());
    let monitor_svc = Arc::new(std::sync::Mutex::new(
        machina_monitor::service::MonitorService::new(Arc::clone(
            &monitor_state,
        )),
    ));

    // Start monitor transport thread (if configured).
    if let Some(ref mon) = cli.monitor {
        let svc = Arc::clone(&monitor_svc);
        if let Some(addr) = mon.strip_prefix("tcp:") {
            let listener =
                std::net::TcpListener::bind(addr).unwrap_or_else(|e| {
                    eprintln!("machina: monitor tcp: {}", e);
                    machina_hw_core::chardev::restore_terminal();
                    process::exit(1);
                });
            let svc2 = Arc::clone(&svc);
            std::thread::spawn(move || {
                machina_monitor::mmp::run_tcp(listener, svc2);
            });
            eprintln!("machina: monitor on tcp:{}", addr);
        } else if mon == "stdio" {
            let svc2 = Arc::clone(&svc);
            std::thread::spawn(move || {
                let stdin = std::io::stdin();
                let stdout = std::io::stdout();
                let mut r = std::io::BufReader::new(stdin.lock());
                let mut w = stdout.lock();
                machina_monitor::hmp::run_interactive(&mut r, &mut w, svc2);
            });
            eprintln!("machina: monitor on stdio");
        }
    }

    // GDB stub setup.
    let gdb_state: Option<Arc<machina_system::gdb::GdbState>> =
        if cli.gdb.is_some() {
            let gs = Arc::new(machina_system::gdb::GdbState::new());
            if cli.start_paused {
                gs.set_connected(true);
            }
            Some(gs)
        } else {
            None
        };

    // GDB accept+server thread.
    if let Some(ref gdb_dev) = cli.gdb {
        if let Some(addr) = gdb_dev.strip_prefix("tcp:") {
            let gs = gdb_state.as_ref().unwrap().clone();
            let addr = addr.to_string();
            std::thread::spawn(move || {
                let listener = std::net::TcpListener::bind(&addr)
                    .unwrap_or_else(|e| {
                        eprintln!("machina: gdb bind: {}", e);
                        process::exit(1);
                    });
                eprintln!("machina: gdbstub waiting on {}", addr);
                let (stream, _) = listener.accept().unwrap();
                eprintln!("machina: gdb client connected");
                // The server runs a simplified RSP loop.
                // It communicates with the exec loop via
                // GdbState (pause/resume/breakpoints).
                // Register access is done while CPU is
                // paused via a shared snapshot.
                gs.set_connected(true);
                if let Err(e) = machina_system::gdb::serve(stream, &gs) {
                    eprintln!("machina: gdb error: {}", e);
                }
                gs.set_connected(false);
            });
            eprintln!("machina: gdb on {}", gdb_dev);
        }
    }

    // Outer loop: supports machine reset via SiFive Test.
    loop {
        eprintln!("machina: entering execution loop");
        let ms = if cli.monitor.is_some() || cli.nographic {
            Some(Arc::clone(&monitor_state))
        } else {
            None
        };
        let gs = gdb_state.as_ref().map(Arc::clone);
        let reason = run_machine_cycle(
            &opts,
            ram_size,
            ms,
            Arc::clone(&monitor_svc),
            htif_tohost,
            gs,
        );

        match reason {
            Some(ShutdownReason::Pass) => {
                machina_hw_core::chardev::restore_terminal();
                eprintln!("machina: shutdown (pass)");
                machina_hw_core::chardev::restore_terminal();
                process::exit(0);
            }
            Some(ShutdownReason::Reset) => {
                eprintln!("machina: reset, rebooting...");
            }
            Some(ShutdownReason::Fail(code)) => {
                machina_hw_core::chardev::restore_terminal();
                eprintln!("machina: fail (code {:#x})", code);
                machina_hw_core::chardev::restore_terminal();
                process::exit(1);
            }
            None => {
                machina_hw_core::chardev::restore_terminal();
                eprintln!("machina: execution exited");
                machina_hw_core::chardev::restore_terminal();
                process::exit(0);
            }
        }
    }
}
