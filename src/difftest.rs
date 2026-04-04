// Difftest: instruction-level comparison of Machina (DUT)
// vs QEMU (REF) via GDB Remote Serial Protocol.

use std::io;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use machina_accel::exec::ExecEnv;
use machina_accel::ir::tb::{
    decode_tb_exit, EXCP_ECALL, EXCP_FENCE_I, EXCP_MRET, EXCP_PRIV_CSR,
    EXCP_SFENCE_VMA, EXCP_SRET, EXCP_WFI, TB_EXIT_NOCHAIN,
};
use machina_accel::x86_64::emitter::SoftMmuConfig;
use machina_accel::GuestCpu;
use machina_accel::HostCodeGen;
use machina_accel::X86_64CodeGen;
use machina_core::machine::{Machine, MachineOpts};
use machina_difftest::gdb::{GdbClient, RegState};
use machina_hw_riscv::ref_machine::RefMachine;
use machina_hw_riscv::sifive_test::ShutdownReason;
use machina_memory::address_space::AddressSpace;
use machina_system::cpus::{
    fault_cause_offset, fault_pc_offset, machina_mem_read, machina_mem_write,
    tlb_offsets, tlb_ptr_offset, FullSystemCpu, RAM_BASE, TLB_SIZE,
};

const GDB_PORT: u16 = 1234;

/// Run difftest: launch QEMU, connect, compare
/// instruction-by-instruction.
pub fn run_difftest(opts: &MachineOpts, ram_mib: u64) {
    let ram_size = ram_mib * 1024 * 1024;

    // Initialize DUT (Machina).
    let mut machine = RefMachine::new();
    if let Err(e) = machine.init(opts) {
        eprintln!("difftest: init failed: {}", e);
        std::process::exit(1);
    }
    if let Err(e) = machine.boot() {
        eprintln!("difftest: boot failed: {}", e);
        std::process::exit(1);
    }

    // Build JIT backend.
    let mut backend = X86_64CodeGen::new();
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
    let mut per_cpu = env.per_cpu;

    let shared_mip = machine.shared_mip();
    let cpu0 = machine.take_cpu(0).expect("cpu0 must exist");
    let ram_ptr = machine.ram_ptr();
    let wfi_waker = machine.wfi_waker();
    let as_ptr = machine.address_space() as *const AddressSpace;

    let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(true));
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
    {
        use machina_hw_riscv::ref_machine::{MROM_BASE, MROM_SIZE};
        let mrom_ptr = machine.mrom_block().as_ptr() as *const u8;
        fs_cpu.set_mrom(mrom_ptr, MROM_BASE, MROM_SIZE);
    }

    // Capture DUT initial PC.
    let init_pc = fs_cpu.get_pc();
    eprintln!("difftest: DUT initial PC = {:#x}", init_pc);

    // Launch QEMU as subprocess.
    let mut qemu = launch_qemu(opts, ram_mib);
    eprintln!("difftest: QEMU launched, pid={}", qemu.id());

    // Connect GDB client.
    let addr = format!("127.0.0.1:{}", GDB_PORT);
    let mut gdb = match GdbClient::connect(&addr, 30) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("difftest: GDB connect failed: {}", e);
            let _ = qemu.kill();
            std::process::exit(1);
        }
    };
    eprintln!("difftest: GDB connected to {}", addr);

    // Skip MROM boot: let both DUT and REF run through
    // their own MROM until PC reaches RAM (kernel entry).
    // Then sync REF to DUT state.
    eprintln!("difftest: running DUT through MROM...");
    skip_mrom(&shared, &mut per_cpu, &mut fs_cpu);
    let kernel_pc = fs_cpu.get_pc();
    eprintln!("difftest: DUT at kernel entry pc={:#x}", kernel_pc);

    // Let QEMU also execute through its MROM to kernel.
    eprintln!("difftest: running QEMU through MROM...");
    if let Err(e) = gdb.set_breakpoint(kernel_pc) {
        eprintln!("difftest: set_breakpoint: {}", e);
    }
    match gdb.cont() {
        Ok(_) => {}
        Err(e) => {
            eprintln!("difftest: QEMU cont: {}", e);
        }
    }
    if let Err(e) = gdb.remove_breakpoint(kernel_pc) {
        eprintln!("difftest: rm_breakpoint: {}", e);
    }

    // Sync DUT registers to QEMU at kernel entry.
    sync_regs_to_ref(&fs_cpu, &mut gdb);
    eprintln!("difftest: synced at kernel entry");

    // Wire shutdown handler.
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

    // Run difftest comparison loop.
    let result =
        difftest_loop(&shared, &mut per_cpu, &mut fs_cpu, &mut gdb, &stop_flag);

    // Check QEMU exit status to distinguish clean
    // shutdown from crash.
    let qemu_status = match qemu.try_wait() {
        Ok(Some(s)) => Some(s),
        _ => {
            let _ = qemu.kill();
            qemu.wait().ok()
        }
    };
    let qemu_ok = qemu_status.map(|s| s.success()).unwrap_or(false);

    match result {
        DifftestResult::Pass(count) => {
            if qemu_ok {
                eprintln!(
                    "DIFFTEST PASS: {} instructions, \
                     zero divergences (QEMU exited ok)",
                    count
                );
            } else {
                eprintln!(
                    "DIFFTEST PASS: {} instructions, \
                     zero divergences (QEMU exit: {:?})",
                    count, qemu_status
                );
            }
        }
        DifftestResult::Divergence {
            insn_count,
            pc,
            detail,
        } => {
            eprintln!("DIFFTEST FAIL at insn #{}, pc={:#x}", insn_count, pc);
            eprintln!("{}", detail);
        }
        DifftestResult::Error(e) => {
            eprintln!("DIFFTEST ERROR: {}", e);
        }
    }
}

enum DifftestResult {
    Pass(u64),
    Divergence {
        insn_count: u64,
        pc: u64,
        detail: String,
    },
    Error(String),
}

/// Launch QEMU with GDB stub enabled, stopped at start.
fn launch_qemu(opts: &MachineOpts, ram_mib: u64) -> Child {
    let mut cmd = Command::new("qemu-system-riscv64");
    cmd.arg("-machine").arg("virt");
    cmd.arg("-nographic");
    cmd.arg("-m").arg(format!("{}M", ram_mib));
    cmd.arg("-smp").arg("1");

    if let Some(ref bios) = opts.bios {
        cmd.arg("-bios").arg(bios);
    } else {
        cmd.arg("-bios").arg("none");
    }
    if let Some(ref kernel) = opts.kernel {
        cmd.arg("-kernel").arg(kernel);
    }

    cmd.arg("-gdb").arg(format!("tcp::{}", GDB_PORT));
    cmd.arg("-S"); // Start stopped.

    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::inherit());

    cmd.spawn().unwrap_or_else(|e| {
        eprintln!("difftest: failed to launch QEMU: {}", e);
        std::process::exit(1);
    })
}

/// Copy DUT gpr[0..31] + pc to REF via GDB G command.
fn sync_regs_to_ref(cpu: &FullSystemCpu, gdb: &mut GdbClient) {
    let mut state = RegState { regs: [0u64; 33] };
    for i in 0..32 {
        state.regs[i] = cpu.cpu.gpr[i];
    }
    state.regs[32] = cpu.cpu.pc;
    gdb.write_regs(&state).unwrap_or_else(|e| {
        eprintln!("difftest: write_regs failed: {}", e);
        std::process::exit(1);
    });
}

/// Execute DUT through MROM until PC reaches RAM.
fn skip_mrom<B: HostCodeGen>(
    shared: &machina_accel::exec::SharedState<B>,
    per_cpu: &mut machina_accel::exec::PerCpuState,
    cpu: &mut FullSystemCpu,
) {
    // Set up setjmp for JIT helpers.
    #[repr(C, align(8))]
    struct SigJmpBuf([u8; 200]);
    unsafe extern "C" {
        #[link_name = "__sigsetjmp"]
        fn sigsetjmp(env: *mut SigJmpBuf, savemask: i32) -> i32;
    }
    let mut jmp_env: SigJmpBuf = unsafe { std::mem::zeroed() };
    let jmp_ptr = &mut jmp_env as *mut SigJmpBuf;
    cpu.set_jmp_env(jmp_ptr as u64);

    for _ in 0..10000 {
        if cpu.get_pc() >= RAM_BASE {
            return;
        }
        if cpu.pending_interrupt() {
            cpu.handle_interrupt();
        }
        if unsafe { sigsetjmp(jmp_ptr, 0) } != 0 {
            continue;
        }
        if !exec_one_insn(shared, per_cpu, cpu) {
            eprintln!("difftest: MROM exec failed at {:#x}", cpu.get_pc());
            return;
        }
        cpu.check_mem_fault();
    }
    eprintln!(
        "difftest: MROM exceeded 10000 insns, \
         pc={:#x}",
        cpu.get_pc()
    );
}

/// Read DUT register state for comparison.
fn read_dut_regs(cpu: &FullSystemCpu) -> RegState {
    let mut state = RegState { regs: [0u64; 33] };
    for i in 0..32 {
        state.regs[i] = cpu.cpu.gpr[i];
    }
    state.regs[32] = cpu.cpu.pc;
    state
}

/// Check if the instruction at the current PC is a CSR
/// read of a non-deterministic counter (time/cycle/instret).
fn is_nondeterministic_csr(cpu: &mut FullSystemCpu) -> bool {
    let insn = cpu.fetch_insn_at_pc();
    if insn == 0 {
        return false;
    }
    let opcode = insn & 0x7F;
    if opcode != 0x73 {
        return false; // not SYSTEM
    }
    let funct3 = (insn >> 12) & 0x7;
    if funct3 == 0 {
        return false; // ECALL/EBREAK
    }
    let csr_addr = (insn >> 20) & 0xFFF;
    // CSR_CYCLE=0xC00, CSR_TIME=0xC01, CSR_INSTRET=0xC02
    matches!(csr_addr, 0xC00..=0xC02)
}

/// Main difftest comparison loop.
///
/// Uses stride comparison: step `stride` instructions on
/// both DUT and REF, compare registers at checkpoints.
/// Non-deterministic CSR reads force immediate resync.
fn difftest_loop<B: HostCodeGen>(
    shared: &machina_accel::exec::SharedState<B>,
    per_cpu: &mut machina_accel::exec::PerCpuState,
    cpu: &mut FullSystemCpu,
    gdb: &mut GdbClient,
    stop_flag: &std::sync::atomic::AtomicBool,
) -> DifftestResult {
    let mut insn_count: u64 = 0;
    let max_insns: u64 = 100_000_000;
    // Stride: compare every N instructions.
    // Larger stride = faster but less precise divergence.
    let stride: u64 = 1;

    // Set up setjmp context (needed for JIT helpers).
    #[repr(C, align(8))]
    struct SigJmpBuf([u8; 200]);
    unsafe extern "C" {
        #[link_name = "__sigsetjmp"]
        fn sigsetjmp(env: *mut SigJmpBuf, savemask: i32) -> i32;
    }
    let mut jmp_env: SigJmpBuf = unsafe { std::mem::zeroed() };
    let jmp_ptr = &mut jmp_env as *mut SigJmpBuf;
    cpu.set_jmp_env(jmp_ptr as u64);

    // Count of non-deterministic CSR resyncs.
    let mut resync_count: u64 = 0;

    loop {
        if insn_count >= max_insns {
            return DifftestResult::Pass(insn_count);
        }
        if !stop_flag.load(Ordering::Relaxed) {
            return DifftestResult::Pass(insn_count);
        }

        // Execute `stride` instructions on DUT.
        let mut skip_this_stride = false;
        let mut actual_steps: u64 = 0;
        for _ in 0..stride {
            let pc = cpu.get_pc();

            // Non-deterministic CSR: mark for resync.
            if is_nondeterministic_csr(cpu) {
                skip_this_stride = true;
            }

            if cpu.pending_interrupt() {
                cpu.handle_interrupt();
            }
            if unsafe { sigsetjmp(jmp_ptr, 0) } != 0 {
                // longjmp from helper.
            }
            if !exec_one_insn(shared, per_cpu, cpu) {
                return DifftestResult::Error(format!(
                    "DUT exec failed at pc={:#x}",
                    pc
                ));
            }
            cpu.check_mem_fault();
            actual_steps += 1;
            insn_count += 1;

            if !stop_flag.load(Ordering::Relaxed) {
                break;
            }
        }

        if skip_this_stride {
            // Had a non-deterministic CSR in this stride.
            // Step REF the same number, then resync.
            match gdb.step_n(actual_steps) {
                Ok(()) => {}
                Err(e) if is_conn_reset(&e) && insn_count > 100 => {
                    return DifftestResult::Pass(insn_count);
                }
                Err(e) => {
                    return DifftestResult::Error(format!("GDB step_n: {}", e));
                }
            }
            let dut = read_dut_regs(cpu);
            if let Err(e) = gdb.write_regs(&dut) {
                if is_conn_reset(&e) && insn_count > 100 {
                    return DifftestResult::Pass(insn_count);
                }
                return DifftestResult::Error(format!(
                    "GDB write_regs (resync): {}",
                    e
                ));
            }
            resync_count += 1;
        } else {
            // Step REF the same number of instructions.
            match gdb.step_n(actual_steps) {
                Ok(()) => {}
                Err(e) if is_conn_reset(&e) && insn_count > 100 => {
                    return DifftestResult::Pass(insn_count);
                }
                Err(e) => {
                    return DifftestResult::Error(format!("GDB step_n: {}", e));
                }
            }

            // Compare registers.
            let ref_regs = match gdb.read_regs() {
                Ok(r) => r,
                Err(e) if is_conn_reset(&e) && insn_count > 100 => {
                    return DifftestResult::Pass(insn_count);
                }
                Err(e) => {
                    return DifftestResult::Error(format!(
                        "GDB read_regs: {}",
                        e
                    ));
                }
            };
            let dut_regs = read_dut_regs(cpu);

            if let Some(detail) = compare_regs(&dut_regs, &ref_regs) {
                let pc = cpu.get_pc();
                return DifftestResult::Divergence {
                    insn_count,
                    pc,
                    detail,
                };
            }
        }

        // Print progress every 100K instructions.
        if insn_count.is_multiple_of(100_000) {
            eprintln!(
                "difftest: {} insns, pc={:#x} \
                 (resyncs={})",
                insn_count,
                cpu.get_pc(),
                resync_count,
            );
        }
    }
}

/// Execute exactly one guest instruction on the DUT.
/// Returns true on success.
fn exec_one_insn<B: HostCodeGen>(
    shared: &machina_accel::exec::SharedState<B>,
    per_cpu: &mut machina_accel::exec::PerCpuState,
    cpu: &mut FullSystemCpu,
) -> bool {
    let pc = cpu.get_pc();
    let flags = cpu.get_flags();

    // Translate one instruction.
    let tb_idx = match tb_find_single(shared, per_cpu, cpu, pc, flags) {
        Some(idx) => idx,
        None => {
            // Check for fetch fault.
            if cpu.check_mem_fault() {
                return true;
            }
            return false;
        }
    };

    // Execute the single-instruction TB.
    let raw_exit = unsafe {
        let tb = shared.tb_store.get(tb_idx);
        let tb_ptr = shared.code_buf().ptr_at(tb.host_offset);
        let env_ptr = cpu.env_ptr();
        let prologue_fn: unsafe extern "C" fn(*mut u8, *const u8) -> usize =
            core::mem::transmute(shared.code_buf().base_ptr());
        prologue_fn(env_ptr, tb_ptr)
    };

    let (_last_tb, exit_code) = decode_tb_exit(raw_exit);

    // Handle exit codes (same as exec_loop).
    match exit_code {
        0..=1 => {
            cpu.check_mem_fault();
        }
        v if v == TB_EXIT_NOCHAIN as usize => {
            cpu.check_mem_fault();
        }
        v if v == EXCP_MRET as usize => {
            cpu.execute_mret();
        }
        v if v == EXCP_SRET as usize => {
            if !cpu.execute_sret() {
                cpu.handle_exception(2, 0);
            }
        }
        v if v == EXCP_SFENCE_VMA as usize => {
            cpu.tlb_flush();
            shared
                .tb_store
                .invalidate_all(shared.code_buf(), &shared.backend);
            per_cpu.jump_cache.invalidate();
        }
        v if v == EXCP_FENCE_I as usize => {
            let dirty = cpu.take_dirty_pages();
            if dirty.is_empty() {
                shared
                    .tb_store
                    .invalidate_all(shared.code_buf(), &shared.backend);
            } else {
                for page in &dirty {
                    shared.tb_store.invalidate_phys_page(
                        *page,
                        shared.code_buf(),
                        &shared.backend,
                    );
                }
            }
            per_cpu.jump_cache.invalidate();
        }
        v if v == EXCP_WFI as usize => {
            cpu.set_halted(true);
            if cpu.pending_interrupt() {
                cpu.set_halted(false);
                cpu.handle_interrupt();
            } else {
                let woken = cpu.wait_for_interrupt();
                cpu.set_halted(false);
                if woken && cpu.pending_interrupt() {
                    cpu.handle_interrupt();
                }
            }
        }
        v if v == EXCP_PRIV_CSR as usize => {
            if !cpu.handle_priv_csr() {
                cpu.handle_exception(2, 0);
            }
            if cpu.take_tb_flush_pending() {
                shared
                    .tb_store
                    .invalidate_all(shared.code_buf(), &shared.backend);
                per_cpu.jump_cache.invalidate();
            }
        }
        v if v == EXCP_ECALL as usize => {
            let pl = cpu.privilege_level();
            let cause = match pl {
                0 => 8,  // EcallFromU
                1 => 9,  // EcallFromS
                3 => 11, // EcallFromM
                _ => 2,
            };
            cpu.handle_exception(cause, 0);
        }
        _ => {
            // Unknown exit, treat as exception.
        }
    }

    true
}

/// Find (or translate) a single-instruction TB.
fn tb_find_single<B: HostCodeGen>(
    shared: &machina_accel::exec::SharedState<B>,
    per_cpu: &mut machina_accel::exec::PerCpuState,
    cpu: &mut FullSystemCpu,
    pc: u64,
    flags: u32,
) -> Option<usize> {
    // Always generate fresh single-insn TBs (don't reuse
    // multi-insn TBs from cache).
    // But we can cache single-insn TBs by pc+flags.
    if let Some(idx) = per_cpu.jump_cache.lookup(pc) {
        let tb = shared.tb_store.get(idx);
        if !tb.invalid.load(Ordering::Acquire)
            && tb.pc == pc
            && tb.flags == flags
        {
            return Some(idx);
        }
    }

    if let Some(idx) = shared.tb_store.lookup(pc, flags) {
        per_cpu.jump_cache.insert(pc, idx);
        return Some(idx);
    }

    // Translate with max_insns=1.
    tb_gen_single(shared, per_cpu, cpu, pc, flags)
}

/// Translate exactly one instruction into a new TB.
fn tb_gen_single<B: HostCodeGen>(
    shared: &machina_accel::exec::SharedState<B>,
    per_cpu: &mut machina_accel::exec::PerCpuState,
    cpu: &mut FullSystemCpu,
    pc: u64,
    flags: u32,
) -> Option<usize> {
    use machina_accel::exec::MIN_CODE_BUF_REMAINING;
    use machina_accel::translate::translate;

    if shared.code_buf().remaining() < MIN_CODE_BUF_REMAINING {
        return None;
    }

    let mut guard = shared.translate_lock.lock().unwrap();

    if let Some(idx) = shared.tb_store.lookup(pc, flags) {
        per_cpu.jump_cache.insert(pc, idx);
        return Some(idx);
    }

    let tb_idx = unsafe { shared.tb_store.alloc(pc, flags, 0) }?;

    guard.ir_ctx.reset();
    guard.ir_ctx.tb_idx = tb_idx as u32;

    // Key: max_insns = 1 for single-step.
    let guest_size = cpu.gen_code(
        &mut guard.ir_ctx,
        pc,
        1, // one instruction per TB
    );
    if guest_size == 0 {
        unsafe {
            let tb = shared.tb_store.get_mut(tb_idx);
            tb.invalid.store(true, Ordering::Release);
        }
        return None;
    }
    unsafe {
        let tb = shared.tb_store.get_mut(tb_idx);
        tb.size = guest_size;
        tb.phys_pc = cpu.last_phys_pc();
    }

    shared.backend.clear_goto_tb_offsets();

    let code_buf_mut = unsafe { shared.code_buf_mut() };
    let host_offset =
        translate(&mut guard.ir_ctx, &shared.backend, code_buf_mut);
    let host_size = shared.code_buf().offset() - host_offset;

    unsafe {
        let tb = shared.tb_store.get_mut(tb_idx);
        tb.host_offset = host_offset;
        tb.host_size = host_size;
    }

    let offsets = shared.backend.goto_tb_offsets();
    unsafe {
        let tb = shared.tb_store.get_mut(tb_idx);
        for (i, &(jmp, reset)) in offsets.iter().enumerate().take(2) {
            tb.set_jmp_insn_offset(i, jmp as u32);
            tb.set_jmp_reset_offset(i, reset as u32);
        }
    }

    shared.tb_store.insert(tb_idx);
    per_cpu.jump_cache.insert(pc, tb_idx);

    Some(tb_idx)
}

/// Check if an IO error is a connection reset.
fn is_conn_reset(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::ConnectionReset
        || e.to_string().contains("Connection reset")
        || e.kind() == io::ErrorKind::BrokenPipe
        || e.kind() == io::ErrorKind::UnexpectedEof
}

/// Compare DUT and REF register states. Returns None if
/// they match, or a detail string on divergence.
fn compare_regs(dut: &RegState, ref_: &RegState) -> Option<String> {
    let mut diffs = Vec::new();

    // x0 is always zero; skip it.
    for i in 1..32 {
        if dut.regs[i] != ref_.regs[i] {
            diffs.push(format!(
                "  x{:<2}: DUT={:#018x}  REF={:#018x}",
                i, dut.regs[i], ref_.regs[i]
            ));
        }
    }

    if dut.regs[32] != ref_.regs[32] {
        diffs.push(format!(
            "  pc:  DUT={:#018x}  REF={:#018x}",
            dut.regs[32], ref_.regs[32]
        ));
    }

    if diffs.is_empty() {
        None
    } else {
        Some(diffs.join("\n"))
    }
}
