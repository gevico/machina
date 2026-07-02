use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use machina_accel::code_buffer::CodeBuffer;
use machina_accel::exec::{cpu_exec_loop_env, ExecEnv, ExitReason};
use machina_accel::ir::tb::EXCP_LOONGARCH_DONE;
use machina_accel::ir::Context;
use machina_accel::translate::translate_and_execute;
use machina_accel::{GuestCpu, HostCodeGen, X86_64CodeGen};
use machina_guest_loongarch::loongarch::cpu::{
    LoongArchCpu, GUEST_BASE_CPU_OFFSET, NEG_ALIGN_CPU_OFFSET, NUM_SAVE,
};
use machina_guest_loongarch::loongarch::csr::*;
use machina_guest_loongarch::loongarch::exception::*;
use machina_guest_loongarch::loongarch::ext::LoongArchCfg;
use machina_guest_loongarch::loongarch::mmu;
use machina_guest_loongarch::loongarch::trans::{
    LoongArchDisasContext, LoongArchTranslator,
};
use machina_guest_loongarch::translator_loop;
use machina_system::loongarch_cpu::{
    loongarch_mem_read, loongarch_soft_mmu_config, LoongArchFullSystemCpu,
    LOONGARCH_TB_FLAG_DA, LOONGARCH_TB_FLAG_FPE, LOONGARCH_TB_FLAG_PG,
    LOONGARCH_TB_FLAG_PLV_MASK,
};

const TLBREHI_EXPECTED_WRITE_MASK: u64 = !0x1FC0_u64;
const ERTN_INSN: u32 = 0x0648_3800;
const SYSCALL_OP: u32 = 0b00000000001010110;
const BREAK_OP: u32 = 0b00000000001010100;
const HVCL_OP: u32 = 0b00000000001010111;
const IDLE_OP: u32 = 0b00000110010010001;
const IOCSRRD_W_OP: u32 = 0b0000011001001000000010;
const IOCSRWR_W_OP: u32 = 0b0000011001001000000110;
const CPUCFG_OP: u32 = 0b0000000000000000011011;
const OP_ADDI_D: u32 = 0b0000001011;
const OP_LD_D: u32 = 0b0010100011;
const OP_ST_W: u32 = 0b0010100110;
const OP_ST_D: u32 = 0b0010100111;
const OP_JIRL: u32 = 0b010011;
const TLBSRCH_INSN: u32 = 0x0648_2800;
const TLBCLR_INSN: u32 = 0x0648_2000;
const TLBFLUSH_INSN: u32 = 0x0648_2400;
const TLBRD_INSN: u32 = 0x0648_2C00;
const TLBWR_INSN: u32 = 0x0648_3000;
const TLBFILL_INSN: u32 = 0x0648_3400;
const CACOP_OP: u32 = 0b0000011000;
const INVTLB_OP: u32 = 0b00000110010010011;
const IBAR_OP: u32 = 0b00111000011100101;
const LDDIR_OP: u32 = 0b00000110010000;
const LDPTE_OP: u32 = 0b00000110010001;
const TIMER_INTERRUPT: u64 = 1_u64 << 11;
const TARGET_VIRT_MASK: u64 = (1_u64 << 48) - 1;
const PAGE_MASK_4K: u64 = !0xFFF_u64;
const HW_PTE_MASK_LA64: u64 = 0xE000_FFFF_FFFF_F1FF;
static NOP_CODE: [u32; 1] = [0x0340_0000];

fn exception_vector(base: u64, ecode: u32, vs: u64) -> u64 {
    if vs == 0 {
        base
    } else {
        base + u64::from(ecode) * ((1_u64 << vs) * 4)
    }
}

fn interrupt_vector(base: u64, irq: u32, vs: u64) -> u64 {
    if vs == 0 {
        base
    } else {
        base + u64::from(64 + irq) * ((1_u64 << vs) * 4)
    }
}

fn dmw64(vseg: u64, pseg32: u64, plv_mask: u64) -> u64 {
    ((vseg & 0xF) << 60) | ((pseg32 & 0x7) << 25) | (plv_mask & 0xF)
}

fn tlbelo_test(
    ppn: u64,
    v: bool,
    d: bool,
    plv: u8,
    mat: u8,
    g: bool,
    nr: bool,
) -> u64 {
    tlbelo_test_with_perm(ppn, v, d, plv, mat, g, nr, false)
}

#[allow(clippy::too_many_arguments)]
fn tlbelo_test_with_perm(
    ppn: u64,
    v: bool,
    d: bool,
    plv: u8,
    mat: u8,
    g: bool,
    nr: bool,
    nx: bool,
) -> u64 {
    u64::from(v)
        | (u64::from(d) << 1)
        | (u64::from(plv & 0x3) << 2)
        | (u64::from(mat & 0x3) << 4)
        | (u64::from(g) << 6)
        | ((ppn & 0xF_FFFF_FFFF) << 12)
        | (u64::from(nr) << 61)
        | (u64::from(nx) << 62)
}

#[allow(clippy::too_many_arguments)]
fn tlbelo_test_with_rplv(
    ppn: u64,
    v: bool,
    d: bool,
    plv: u8,
    mat: u8,
    g: bool,
    nr: bool,
    nx: bool,
    rplv: bool,
) -> u64 {
    tlbelo_test_with_perm(ppn, v, d, plv, mat, g, nr, nx)
        | (u64::from(rplv) << 63)
}

fn assert_tlb_hit(result: mmu::TlbLookupResult, pa: u64, mat: u8) {
    assert_eq!(result, mmu::TlbLookupResult::Hit { pa, mat });
}

fn tlb_pair_base(va: u64, page_size: u8) -> u64 {
    va & !((1_u64 << (u32::from(page_size) + 1)) - 1)
}

fn write_test_tlb_entry(
    cpu: &mut LoongArchCpu,
    index: usize,
    va: u64,
    page_size: u8,
    asid: u16,
    elo0: u64,
    elo1: u64,
) {
    cpu.csr_write(CSR_TLBEHI, tlb_pair_base(va, page_size) & !0x1FFF);
    cpu.csr_write(CSR_TLBELO0, elo0);
    cpu.csr_write(CSR_TLBELO1, elo1);
    cpu.csr_write(CSR_ASID, u64::from(asid));
    cpu.csr_write(CSR_TLBIDX, (u64::from(page_size) << 24) | index as u64);
    cpu.tlb_write(index);
}

fn write_guest_test_tlb_entry(
    cpu: &mut LoongArchCpu,
    index: usize,
    va: u64,
    page_size: u8,
    asid: u16,
    elo0: u64,
    elo1: u64,
) {
    cpu.gcsr_write(CSR_TLBEHI, tlb_pair_base(va, page_size) & !0x1FFF);
    cpu.gcsr_write(CSR_TLBELO0, elo0);
    cpu.gcsr_write(CSR_TLBELO1, elo1);
    cpu.gcsr_write(CSR_ASID, u64::from(asid));
    cpu.gcsr_write(CSR_TLBIDX, (u64::from(page_size) << 24) | index as u64);
    cpu.tlb_write(index);
}

fn expected_tlb_addend(cpu: &LoongArchCpu, va: u64, pa: u64) -> usize {
    cpu.guest_base_val()
        .wrapping_add(pa & PAGE_MASK_4K)
        .wrapping_sub(va & PAGE_MASK_4K) as usize
}

fn csr_insn(csr_num: u32, rj: u32, rd: u32) -> u32 {
    (0b00000100 << 24) | ((csr_num & 0x3FFF) << 10) | (rj << 5) | rd
}

fn gcsr_insn(csr_num: u32, rj: u32, rd: u32) -> u32 {
    (0b00000101 << 24) | ((csr_num & 0x3FFF) << 10) | (rj << 5) | rd
}

fn code15_insn(op: u32, code: u32) -> u32 {
    (op << 15) | (code & 0x7FFF)
}

fn r2_insn(op: u32, rj: u32, rd: u32) -> u32 {
    (op << 10) | (rj << 5) | rd
}

fn r2_si12(op: u32, si12: i16, rj: u32, rd: u32) -> u32 {
    (op << 22) | ((si12 as u16 as u32 & 0x0FFF) << 10) | (rj << 5) | rd
}

fn cop_r_si12(op: u32, si12: i16, rj: u32, cop: u32) -> u32 {
    (op << 22) | ((si12 as u16 as u32 & 0x0FFF) << 10) | (rj << 5) | cop
}

fn r2_si16(op: u32, si16: i16, rj: u32, rd: u32) -> u32 {
    (op << 26) | ((si16 as u16 as u32) << 10) | (rj << 5) | rd
}

fn r3_insn(op: u32, rk: u32, rj: u32, rd: u32) -> u32 {
    (op << 15) | (rk << 10) | (rj << 5) | rd
}

fn r2_ui8(op: u32, ui8: u8, rj: u32, rd: u32) -> u32 {
    (op << 18) | (u32::from(ui8) << 10) | (rj << 5) | rd
}

fn run_priv_la(cpu: &mut LoongArchCpu, insns: &[u32]) -> usize {
    run_priv_la_at(cpu, insns, 0)
}

fn run_priv_la_at(cpu: &mut LoongArchCpu, insns: &[u32], pc: u64) -> usize {
    let start = usize::try_from(pc).unwrap();
    let mut code = vec![0_u8; start + insns.len() * 4];
    for (idx, insn) in insns.iter().enumerate() {
        let off = start + idx * 4;
        code[off..off + 4].copy_from_slice(&insn.to_le_bytes());
    }

    let mut backend = X86_64CodeGen::new();
    let mut buf = CodeBuffer::new(4096).unwrap();
    backend.emit_prologue(&mut buf);
    backend.emit_epilogue(&mut buf);

    let mut ir = Context::new();
    backend.init_context(&mut ir);

    let mut ctx =
        LoongArchDisasContext::new(pc, code.as_ptr(), LoongArchCfg::default());
    ctx.base.max_insns = insns.len() as u32;
    translator_loop::<LoongArchTranslator>(&mut ctx, &mut ir);

    unsafe { translate_and_execute(&mut ir, &backend, &mut buf, cpu.env_ptr()) }
}

fn enter_guest_mode_for_test(cpu: &mut LoongArchCpu) {
    cpu.csr_xchg(CSR_GSTAT, GSTAT_PVM, GSTAT_PVM);
    let _ = unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_ertn(cpu.env_ptr())
    };
    assert_ne!(cpu.csr_read(CSR_GSTAT) & GSTAT_VM, 0);
    assert_eq!(cpu.csr_read(CSR_GSTAT) & GSTAT_PVM, 0);
}

fn full_system_cpu(cpu: LoongArchCpu) -> LoongArchFullSystemCpu {
    let stop = Arc::new(AtomicBool::new(true));
    unsafe {
        LoongArchFullSystemCpu::new(
            cpu,
            NOP_CODE.as_ptr().cast::<u8>(),
            0,
            (NOP_CODE.len() * 4) as u64,
            0,
            stop,
        )
    }
}

fn full_system_cpu_with_code(
    cpu: LoongArchCpu,
    code: &[u32],
) -> LoongArchFullSystemCpu {
    let stop = Arc::new(AtomicBool::new(true));
    unsafe {
        LoongArchFullSystemCpu::new(
            cpu,
            code.as_ptr().cast::<u8>(),
            0,
            (code.len() * 4) as u64,
            0,
            stop,
        )
    }
}

fn loongarch_soft_mmu_env() -> ExecEnv<X86_64CodeGen> {
    let mut backend = X86_64CodeGen::new();
    backend.set_guest_base_offset(GUEST_BASE_CPU_OFFSET);
    backend.neg_align_off = i32::try_from(NEG_ALIGN_CPU_OFFSET).unwrap();
    backend.mmio = Some(loongarch_soft_mmu_config());
    ExecEnv::new(backend)
}

fn pwcl(
    ptbase: u8,
    ptwidth: u8,
    dir1_base: u8,
    dir1_width: u8,
    dir2_base: u8,
    dir2_width: u8,
) -> u64 {
    u64::from(ptbase & 0x1F)
        | (u64::from(ptwidth & 0x1F) << 5)
        | (u64::from(dir1_base & 0x1F) << 10)
        | (u64::from(dir1_width & 0x1F) << 15)
        | (u64::from(dir2_base & 0x1F) << 20)
        | (u64::from(dir2_width & 0x1F) << 25)
}

fn pwch(dir3_base: u8, dir3_width: u8, dir4_base: u8, dir4_width: u8) -> u64 {
    u64::from(dir3_base & 0x3F)
        | (u64::from(dir3_width & 0x3F) << 6)
        | (u64::from(dir4_base & 0x3F) << 12)
        | (u64::from(dir4_width & 0x3F) << 18)
}

fn attach_page_walk_ram(cpu: &mut LoongArchCpu, ram: &[u8]) {
    cpu.set_guest_base(ram.as_ptr() as u64);
    cpu.set_ram_base(0);
    cpu.set_ram_end(ram.len() as u64);
}

fn write_ram_u64(ram: &mut [u8], pa: u64, val: u64) {
    let off = pa as usize;
    ram[off..off + 8].copy_from_slice(&val.to_le_bytes());
}

fn write_code_u64(code: &mut [u32], pa: usize, val: u64) {
    code[pa / 4] = val as u32;
    code[pa / 4 + 1] = (val >> 32) as u32;
}

#[test]
fn csr_crmd_reset_value() {
    let cpu = LoongArchCpu::new();
    assert_eq!(cpu.csr_read(CSR_CRMD), 0x8);
}

#[test]
fn cpu_reset_profile_matches_direct_boot_baseline() {
    let cpu = LoongArchCpu::new();

    assert_eq!(cpu.pc(), 0);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PLV_MASK, 0);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_DA, CRMD_DA);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PG, 0);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_IE, 0);
    assert_eq!(cpu.csr_read(CSR_EUEN), 0);
    assert_eq!(cpu.csr_read(CSR_ECFG), 0);
    assert_eq!(cpu.csr_read(CSR_ESTAT), 0);
    assert_eq!(cpu.csr_read(CSR_ERA), 0);
    assert_eq!(cpu.csr_read(CSR_BADV), 0);
    assert_eq!(cpu.csr_read(CSR_EENTRY), 0);
    assert_eq!(cpu.csr_read(CSR_TID), 0);
    assert_eq!(cpu.csr_read(CSR_TCFG), 0);
    assert_eq!(cpu.csr_read(CSR_TVAL), 0);
    assert_eq!(cpu.csr_read(CSR_CNTC), 0);
    assert_eq!(cpu.csr_read(CSR_DMW0), 0);
    assert_eq!(cpu.csr_read(CSR_DMW1), 0);
    assert_eq!(cpu.csr_read(CSR_DMW2), 0);
    assert_eq!(cpu.csr_read(CSR_DMW3), 0);
    assert_eq!(cpu.csr_read(CSR_PRCFG1), 0x0000_72F8);
    assert_eq!(cpu.csr_read(CSR_PRCFG2), 0x4020_5000);
    assert_eq!(cpu.csr_read(CSR_PRCFG3), 0x0080_73F2);
    assert_eq!((cpu.csr_read(CSR_ASID) >> 16) & 0xFF, 10);
    assert_eq!(cpu.neg_align_val(), 0);
    assert_eq!(cpu.last_phys_pc_val(), 0);
    assert!(!cpu.is_halted());
}

#[test]
fn csr_read_write_save_regs() {
    let mut cpu = LoongArchCpu::new();
    for i in 0..NUM_SAVE as u32 {
        cpu.csr_write(CSR_SAVE0 + i, 0xDEAD_0000 + u64::from(i));
    }
    for i in 0..NUM_SAVE as u32 {
        assert_eq!(cpu.csr_read(CSR_SAVE0 + i), 0xDEAD_0000 + u64::from(i));
    }
}

#[test]
fn save_csr_range_matches_prcfg1_save_num() {
    let cpu = LoongArchCpu::new();
    let save_num = (cpu.csr_read(CSR_PRCFG1) & 0xF) as u32;

    assert_eq!(NUM_SAVE, save_num as usize);
    assert_eq!(CSR_SAVE_LAST, CSR_SAVE0 + save_num - 1);
}

#[test]
fn save_csr_after_prcfg1_count_is_not_implemented() {
    let mut cpu = LoongArchCpu::new();
    let first_unimplemented = CSR_SAVE0 + NUM_SAVE as u32;

    assert_eq!(first_unimplemented, CSR_SAVE_LAST + 1);
    assert_eq!(csr_write_mask(first_unimplemented), 0);
    cpu.csr_write(first_unimplemented, 0xBAD0_CAFE);
    assert_eq!(cpu.csr_read(first_unimplemented), 0);
}

#[test]
fn csr_warl_mask_readonly_fields() {
    let mut cpu = LoongArchCpu::new();
    let cpuid_init = cpu.csr_read(CSR_CPUID);
    cpu.csr_write(CSR_CPUID, 0xFFFF_FFFF);
    assert_eq!(cpu.csr_read(CSR_CPUID), cpuid_init);

    let prcfg1_init = cpu.csr_read(CSR_PRCFG1);
    cpu.csr_write(CSR_PRCFG1, 0xFFFF_FFFF);
    assert_eq!(cpu.csr_read(CSR_PRCFG1), prcfg1_init);

    cpu.csr_write(CSR_TVAL, 0xFFFF_FFFF);
    assert_eq!(cpu.csr_read(CSR_TVAL), 0);
}

#[test]
fn csr_warl_crmd_mask() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_CRMD, 0xFFFF_FFFF_FFFF_FFFF);
    let val = cpu.csr_read(CSR_CRMD);
    assert_eq!(val, CRMD_WRITE_MASK);
}

#[test]
fn csr_warl_eentry_aligned() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_EENTRY, 0xFFFF_FFFF_FFFF_FFFF);
    let val = cpu.csr_read(CSR_EENTRY);
    assert_eq!(val & 0x3F, 0);
}

#[test]
fn csr_xchg_partial_write() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_SAVE0, 0xAAAA_BBBB_CCCC_DDDD);
    let old = cpu.csr_xchg(CSR_SAVE0, 0x1111_2222_3333_4444, 0xFFFF_0000);
    assert_eq!(old, 0xAAAA_BBBB_CCCC_DDDD);
    assert_eq!(cpu.csr_read(CSR_SAVE0), 0xAAAA_BBBB_3333_DDDD);
}

#[test]
fn csr_estat_only_swi_writable() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_ESTAT, 0xFFFF_FFFF_FFFF_FFFF);
    assert_eq!(cpu.csr_read(CSR_ESTAT), 0x3);
}

#[test]
fn csr_ticlr_clears_timer_interrupt() {
    let mut cpu = LoongArchCpu::new();
    // Simulate timer interrupt pending
    cpu.set_estat_hw(1 << 11);
    assert_ne!(cpu.csr_read(CSR_ESTAT) & (1 << 11), 0);
    cpu.csr_write(CSR_TICLR, 1);
    assert_eq!(cpu.csr_read(CSR_ESTAT) & (1 << 11), 0);
}

#[test]
fn csr_tcfg_write_initializes_tval() {
    let mut cpu = LoongArchCpu::new();
    // Enable timer with initial value 0x1000 (bit0=enable, bit1=periodic)
    cpu.csr_write(CSR_TCFG, 0x1001);
    assert_eq!(cpu.csr_read(CSR_TVAL), 0x1000);
}

#[test]
fn csr_asid_reports_width() {
    let cpu = LoongArchCpu::new();
    let asid = cpu.csr_read(CSR_ASID);
    // Bits [23:16] = ASIDBITS = 10
    assert_eq!((asid >> 16) & 0xFF, 10);
}

#[test]
fn csr_pgd_reads_pgdl_or_pgdh() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_PGDL, 0x1000_0000);
    cpu.csr_write(CSR_PGDH, 0x2000_0000);
    // BADV bit63=0 -> PGDL
    cpu.set_badv_raw(0x0000_1234_5678_0000);
    assert_eq!(cpu.csr_read(CSR_PGD), 0x1000_0000);
    // BADV bit63=1 -> PGDH
    cpu.set_badv_raw(0x8000_1234_5678_0000);
    assert_eq!(cpu.csr_read(CSR_PGD), 0x2000_0000);
}

#[test]
fn task15_csr_register_file_reset_and_warl_matrix() {
    let reset_cases: &[(&str, u32, u64)] = &[
        ("CRMD", CSR_CRMD, 0x8),
        ("PRMD", CSR_PRMD, 0),
        ("EUEN", CSR_EUEN, 0),
        ("MISC", CSR_MISC, 0),
        ("ECFG", CSR_ECFG, 0),
        ("ESTAT", CSR_ESTAT, 0),
        ("ERA", CSR_ERA, 0),
        ("BADV", CSR_BADV, 0),
        ("BADI", CSR_BADI, 0),
        ("EENTRY", CSR_EENTRY, 0),
        ("TLBIDX", CSR_TLBIDX, 0),
        ("TLBEHI", CSR_TLBEHI, 0),
        ("TLBELO0", CSR_TLBELO0, 0),
        ("TLBELO1", CSR_TLBELO1, 0),
        ("ASID", CSR_ASID, 0x0A << 16),
        ("PGDL", CSR_PGDL, 0),
        ("PGDH", CSR_PGDH, 0),
        ("PGD", CSR_PGD, 0),
        ("PWCL", CSR_PWCL, 0),
        ("PWCH", CSR_PWCH, 0),
        ("STLBPS", CSR_STLBPS, 0),
        ("RVACFG", CSR_RVACFG, 0),
        ("CPUID", CSR_CPUID, 0),
        ("PRCFG1", CSR_PRCFG1, 0x0000_72F8),
        ("PRCFG2", CSR_PRCFG2, 0x4020_5000),
        ("PRCFG3", CSR_PRCFG3, 0x0080_73F2),
        ("TID", CSR_TID, 0),
        ("TCFG", CSR_TCFG, 0),
        ("TVAL", CSR_TVAL, 0),
        ("CNTC", CSR_CNTC, 0),
        ("TICLR", CSR_TICLR, 0),
        ("LLBCTL", CSR_LLBCTL, 0),
        ("TLBRENTRY", CSR_TLBRENTRY, 0),
        ("TLBRBADV", CSR_TLBRBADV, 0),
        ("TLBRERA", CSR_TLBRERA, 0),
        ("TLBRSAVE", CSR_TLBRSAVE, 0),
        ("TLBRELO0", CSR_TLBRELO0, 0),
        ("TLBRELO1", CSR_TLBRELO1, 0),
        ("TLBREHI", CSR_TLBREHI, 0),
        ("TLBRPRMD", CSR_TLBRPRMD, 0),
        ("DMW0", CSR_DMW0, 0),
        ("DMW1", CSR_DMW1, 0),
        ("DMW2", CSR_DMW2, 0),
        ("DMW3", CSR_DMW3, 0),
    ];

    let cpu = LoongArchCpu::new();
    for &(name, csr, expected) in reset_cases {
        assert_eq!(cpu.csr_read(csr), expected, "{name}");
    }

    let write_cases: &[(&str, u32, u64, u64, u64)] = &[
        ("CRMD", CSR_CRMD, u64::MAX, CRMD_WRITE_MASK, CRMD_WRITE_MASK),
        ("PRMD", CSR_PRMD, u64::MAX, PRMD_WRITE_MASK, PRMD_WRITE_MASK),
        ("EUEN", CSR_EUEN, u64::MAX, EUEN_WRITE_MASK, EUEN_WRITE_MASK),
        ("MISC", CSR_MISC, u64::MAX, MISC_WRITE_MASK, 0),
        ("ECFG", CSR_ECFG, u64::MAX, ECFG_WRITE_MASK, ECFG_WRITE_MASK),
        (
            "ESTAT",
            CSR_ESTAT,
            u64::MAX,
            ESTAT_WRITE_MASK,
            ESTAT_WRITE_MASK,
        ),
        ("ERA", CSR_ERA, u64::MAX, ERA_WRITE_MASK, u64::MAX),
        ("BADV", CSR_BADV, u64::MAX, BADV_WRITE_MASK, u64::MAX),
        ("BADI", CSR_BADI, u64::MAX, 0, 0),
        (
            "EENTRY",
            CSR_EENTRY,
            u64::MAX,
            EENTRY_WRITE_MASK,
            EENTRY_WRITE_MASK,
        ),
        (
            "TLBIDX",
            CSR_TLBIDX,
            u64::MAX,
            TLBIDX_WRITE_MASK,
            TLBIDX_WRITE_MASK,
        ),
        (
            "TLBEHI",
            CSR_TLBEHI,
            u64::MAX,
            TLBEHI_WRITE_MASK,
            TLBEHI_WRITE_MASK,
        ),
        (
            "TLBELO0",
            CSR_TLBELO0,
            u64::MAX,
            TLBELO_WRITE_MASK,
            TLBELO_WRITE_MASK,
        ),
        (
            "TLBELO1",
            CSR_TLBELO1,
            u64::MAX,
            TLBELO_WRITE_MASK,
            TLBELO_WRITE_MASK,
        ),
        (
            "ASID",
            CSR_ASID,
            u64::MAX,
            ASID_WRITE_MASK,
            (0x0A << 16) | ASID_WRITE_MASK,
        ),
        ("PGDL", CSR_PGDL, u64::MAX, PGDL_WRITE_MASK, PGDL_WRITE_MASK),
        ("PGDH", CSR_PGDH, u64::MAX, PGDH_WRITE_MASK, PGDH_WRITE_MASK),
        ("PGD", CSR_PGD, u64::MAX, PGD_WRITE_MASK, 0),
        ("PWCL", CSR_PWCL, u64::MAX, PWCL_WRITE_MASK, PWCL_WRITE_MASK),
        ("PWCH", CSR_PWCH, u64::MAX, PWCH_WRITE_MASK, PWCH_WRITE_MASK),
        (
            "STLBPS",
            CSR_STLBPS,
            u64::MAX,
            STLBPS_WRITE_MASK,
            STLBPS_WRITE_MASK,
        ),
        ("RVACFG", CSR_RVACFG, u64::MAX, RVACFG_WRITE_MASK, 0),
        ("CPUID", CSR_CPUID, u64::MAX, CPUID_WRITE_MASK, 0),
        (
            "PRCFG1",
            CSR_PRCFG1,
            u64::MAX,
            PRCFG1_WRITE_MASK,
            0x0000_72F8,
        ),
        (
            "PRCFG2",
            CSR_PRCFG2,
            u64::MAX,
            PRCFG2_WRITE_MASK,
            0x4020_5000,
        ),
        (
            "PRCFG3",
            CSR_PRCFG3,
            u64::MAX,
            PRCFG3_WRITE_MASK,
            0x0080_73F2,
        ),
        ("TID", CSR_TID, u64::MAX, TID_WRITE_MASK, TID_WRITE_MASK),
        ("TCFG", CSR_TCFG, u64::MAX, TCFG_WRITE_MASK, TCFG_WRITE_MASK),
        ("TVAL", CSR_TVAL, u64::MAX, TVAL_WRITE_MASK, 0),
        ("CNTC", CSR_CNTC, u64::MAX, CNTC_WRITE_MASK, u64::MAX),
        ("TICLR", CSR_TICLR, u64::MAX, TICLR_WRITE_MASK, 0),
        ("LLBCTL", CSR_LLBCTL, 0x4, LLBCTL_WRITE_MASK, 0),
        (
            "TLBRENTRY",
            CSR_TLBRENTRY,
            u64::MAX,
            TLBRENTRY_WRITE_MASK,
            TLBRENTRY_WRITE_MASK,
        ),
        (
            "TLBRBADV",
            CSR_TLBRBADV,
            u64::MAX,
            TLBRBADV_WRITE_MASK,
            u64::MAX,
        ),
        (
            "TLBRERA",
            CSR_TLBRERA,
            u64::MAX,
            TLBRERA_WRITE_MASK,
            u64::MAX,
        ),
        (
            "TLBRSAVE",
            CSR_TLBRSAVE,
            u64::MAX,
            TLBRSAVE_WRITE_MASK,
            u64::MAX,
        ),
        (
            "TLBRELO0",
            CSR_TLBRELO0,
            u64::MAX,
            TLBRELO_WRITE_MASK,
            TLBRELO_WRITE_MASK,
        ),
        (
            "TLBRELO1",
            CSR_TLBRELO1,
            u64::MAX,
            TLBRELO_WRITE_MASK,
            TLBRELO_WRITE_MASK,
        ),
        (
            "TLBREHI",
            CSR_TLBREHI,
            u64::MAX,
            TLBREHI_EXPECTED_WRITE_MASK,
            TLBREHI_EXPECTED_WRITE_MASK,
        ),
        (
            "TLBRPRMD",
            CSR_TLBRPRMD,
            u64::MAX,
            TLBRPRMD_WRITE_MASK,
            TLBRPRMD_WRITE_MASK,
        ),
        ("DMW0", CSR_DMW0, u64::MAX, DMW_WRITE_MASK, DMW_WRITE_MASK),
        ("DMW1", CSR_DMW1, u64::MAX, DMW_WRITE_MASK, DMW_WRITE_MASK),
        ("DMW2", CSR_DMW2, u64::MAX, DMW_WRITE_MASK, DMW_WRITE_MASK),
        ("DMW3", CSR_DMW3, u64::MAX, DMW_WRITE_MASK, DMW_WRITE_MASK),
    ];

    for &(name, csr, write, mask, expected_read) in write_cases {
        let mut cpu = LoongArchCpu::new();
        assert_eq!(csr_write_mask(csr), mask, "{name}");
        cpu.csr_write(csr, write);
        assert_eq!(cpu.csr_read(csr), expected_read, "{name}");
    }

    let mut cpu = LoongArchCpu::new();
    for i in 0..NUM_SAVE as u32 {
        let csr = CSR_SAVE0 + i;
        let value = 0xCAFE_0000_0000_0000 | u64::from(i);
        assert_eq!(csr_write_mask(csr), SAVE_WRITE_MASK);
        cpu.csr_write(csr, value);
        assert_eq!(cpu.csr_read(csr), value);
    }
    assert_eq!(csr_write_mask(CSR_SAVE_LAST + 1), 0);
    cpu.csr_write(CSR_SAVE_LAST + 1, 0xFFFF);
    assert_eq!(cpu.csr_read(CSR_SAVE_LAST + 1), 0);
}

#[test]
fn task15_csr_register_file_special_selection_and_side_effects() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_PGDL, 0x1234_5000);
    cpu.csr_write(CSR_PGDH, 0xFEDC_B000);
    cpu.set_badv_raw(0x0000_7FFF_FFFF_F000);
    assert_eq!(cpu.csr_read(CSR_PGD), 0x1234_5000);
    cpu.set_badv_raw(0x8000_0000_0000_0000);
    assert_eq!(cpu.csr_read(CSR_PGD), 0xFEDC_B000);
    cpu.csr_write(CSR_PGD, 0xFFFF_F000);
    assert_eq!(cpu.csr_read(CSR_PGD), 0xFEDC_B000);

    cpu.csr_write(CSR_TLBRERA, 1);
    cpu.csr_write(CSR_TLBRBADV, 0x0000_0000_1234_0000);
    cpu.set_badv_raw(0x8000_0000_0000_0000);
    assert_eq!(cpu.csr_read(CSR_PGD), 0x1234_5000);
    cpu.csr_write(CSR_TLBRBADV, 0x8000_0000_0000_0000);
    cpu.set_badv_raw(0);
    assert_eq!(cpu.csr_read(CSR_PGD), 0xFEDC_B000);

    let asid = cpu.csr_read(CSR_ASID);
    assert_eq!(asid & 0x3FF, 0);
    assert_eq!((asid >> 16) & 0xFF, 10);
    cpu.csr_write(CSR_ASID, 0xFFFF);
    let asid = cpu.csr_read(CSR_ASID);
    assert_eq!(asid & 0x3FF, 0x3FF);
    assert_eq!((asid >> 16) & 0xFF, 10);

    cpu.csr_write(CSR_TCFG, 0x1237);
    assert_eq!(cpu.csr_read(CSR_TCFG), 0x1237);
    assert_eq!(cpu.csr_read(CSR_TVAL), 0x1234);

    cpu.set_estat_hw((1 << 11) | 0x3);
    cpu.csr_write(CSR_TICLR, 1);
    assert_eq!(cpu.csr_read(CSR_ESTAT) & (1 << 11), 0);
    assert_eq!(cpu.csr_read(CSR_ESTAT) & 0x3, 0x3);
}

#[test]
fn task15_badi_is_readonly_for_direct_and_translated_csr_writes() {
    let mut cpu = LoongArchCpu::new();

    assert_eq!(csr_write_mask(CSR_BADI), 0);
    cpu.csr_write(CSR_BADI, 0xABCD);
    assert_eq!(cpu.csr_read(CSR_BADI), 0);
    assert_eq!(cpu.csr_xchg(CSR_BADI, 0xFFFF, u64::MAX), 0);
    assert_eq!(cpu.csr_read(CSR_BADI), 0);

    cpu.write_gpr(5, 0x1111);
    assert_eq!(run_priv_la(&mut cpu, &[csr_insn(CSR_BADI, 1, 5)]), 0);
    assert_eq!(cpu.read_gpr(5), 0);
    assert_eq!(cpu.csr_read(CSR_BADI), 0);

    cpu.write_gpr(6, 0x2222);
    cpu.write_gpr(7, u64::MAX);
    assert_eq!(run_priv_la(&mut cpu, &[csr_insn(CSR_BADI, 7, 6)]), 0);
    assert_eq!(cpu.read_gpr(6), 0);
    assert_eq!(cpu.csr_read(CSR_BADI), 0);
}

#[test]
fn task15_tlbrehi_preserves_ps_and_vppn_but_masks_reserved_bits() {
    let mut cpu = LoongArchCpu::new();
    let vppn = 0x0123_4567_89AB_C000;
    let ps = 0x2D;
    let reserved = 0x1FC0;
    let write = vppn | reserved | ps;
    let expected = (vppn | ps) & TLBREHI_EXPECTED_WRITE_MASK;

    assert_eq!(csr_write_mask(CSR_TLBREHI), TLBREHI_EXPECTED_WRITE_MASK);
    cpu.csr_write(CSR_TLBREHI, write);
    assert_eq!(cpu.csr_read(CSR_TLBREHI), expected);
    assert_eq!(cpu.csr_read(CSR_TLBREHI) & 0x3F, ps);
    assert_eq!(cpu.csr_read(CSR_TLBREHI) & 0x1FC0, 0);
}

#[test]
fn translated_plv0_csrrd_reads_csr_value() {
    let mut cpu = LoongArchCpu::new();

    assert_eq!(run_priv_la(&mut cpu, &[csr_insn(CSR_PRCFG1, 0, 4)]), 0);
    assert_eq!(cpu.read_gpr(4), 0x0000_72F8);
    assert_eq!(cpu.pc(), 4);
}

#[test]
fn translated_plv0_csrwr_returns_old_value_and_masks_write() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_ECFG, 0x1200);
    cpu.write_gpr(5, u64::MAX);

    assert_eq!(run_priv_la(&mut cpu, &[csr_insn(CSR_ECFG, 1, 5)]), 0);
    assert_eq!(cpu.read_gpr(5), 0x1200);
    assert_eq!(cpu.csr_read(CSR_ECFG), ECFG_WRITE_MASK);
    assert_eq!(cpu.pc(), 4);
}

#[test]
fn translated_plv0_csrxchg_returns_old_value_and_masks_exchange() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_ECFG, 0xAAAA);
    cpu.write_gpr(6, 0x5555);
    cpu.write_gpr(7, 0x0FF0);

    assert_eq!(run_priv_la(&mut cpu, &[csr_insn(CSR_ECFG, 7, 6)]), 0);
    assert_eq!(cpu.read_gpr(6), 0xAAAA);
    assert_eq!(
        cpu.csr_read(CSR_ECFG),
        (0xAAAA & !0x0FF0) | (0x5555 & 0x0FF0)
    );
    assert_eq!(cpu.pc(), 4);
}

#[test]
fn exception_entry_saves_state() {
    let mut cpu = LoongArchCpu::new();
    // Set CRMD: PLV=3, IE=1, DA=1, PG=1
    cpu.csr_write(CSR_CRMD, 0x1F);
    cpu.set_pc(0x8000_0100);
    cpu.csr_write(CSR_EENTRY, 0x9000_0000);

    unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_raise_exception(
                cpu.env_ptr(),
                0x0B, // SYS
                0,
            );
    }

    assert_eq!(cpu.csr_read(CSR_ERA), 0x8000_0100);
    assert_eq!(cpu.csr_read(CSR_PRMD), 0x07);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PLV_MASK, 0);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_IE, 0);
    assert_eq!((cpu.csr_read(CSR_ESTAT) >> 16) & 0x3F, 0x0B);
}

#[test]
fn ertn_restores_state() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_CRMD, 0);
    cpu.csr_write(CSR_PRMD, 0x07);
    cpu.csr_write(CSR_ERA, 0x8000_0200);
    cpu.csr_write(CSR_TLBRERA, 0);

    let pc = unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_ertn(cpu.env_ptr())
    };

    assert_eq!(pc, 0x8000_0200);
    assert_eq!(cpu.csr_read(CSR_CRMD) & 0x7, 0x07);
}

#[test]
fn exception_tlbr_uses_tlbr_csrs() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_CRMD, 0x17); // PLV=3, IE=1, DA=0, PG=1
    cpu.set_pc(0x1234_5678);
    cpu.csr_write(CSR_TLBRENTRY, 0x9000_1000);

    unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_raise_exception(
                cpu.env_ptr(),
                0x3F, // TLBR
                0,
            );
    }

    let tlbrera = cpu.csr_read(CSR_TLBRERA);
    assert_eq!(tlbrera & 1, 1);
    assert_eq!(tlbrera & !0x3, 0x1234_5678);
    assert_eq!(cpu.csr_read(CSR_TLBRPRMD), 0x07);
    assert_ne!(cpu.csr_read(CSR_CRMD) & CRMD_DA, 0);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PG, 0);
}

#[test]
fn ertn_from_tlbr_clears_istlbr() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_CRMD, CRMD_DA);
    cpu.csr_write(CSR_TLBRERA, 0x1234_5678 | 1);
    cpu.csr_write(CSR_TLBRPRMD, 0x07);

    let pc = unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_ertn(cpu.env_ptr())
    };

    assert_eq!(pc, 0x1234_5678);
    assert_eq!(cpu.csr_read(CSR_TLBRERA) & 1, 0);
    assert_eq!(cpu.csr_read(CSR_CRMD) & 0x7, 0x07);
}

#[test]
fn timer_tick_fires_interrupt() {
    let mut cpu = LoongArchCpu::new();
    // enable=1, periodic=0, initval=0x100
    cpu.csr_write(CSR_TCFG, 0x0101);

    cpu.timer_tick(0x100);

    assert_ne!(cpu.csr_read(CSR_ESTAT) & (1 << 11), 0);
    assert_eq!(cpu.tval(), 0);
}

#[test]
fn timer_tick_periodic_reloads() {
    let mut cpu = LoongArchCpu::new();
    // enable=1, periodic=1, initval=0x100
    cpu.csr_write(CSR_TCFG, 0x0103);
    // Tick down to 0x50 first
    cpu.timer_tick(0xB0);
    // Now it's at 0x50, tick the rest
    cpu.timer_tick(0x50);

    assert_ne!(cpu.csr_read(CSR_ESTAT) & (1 << 11), 0);
    assert_eq!(cpu.tval(), 0x100); // reloaded
}

#[test]
fn timer_disabled_no_countdown() {
    let mut cpu = LoongArchCpu::new();
    // enable=0, initval=0x100 - TCFG bit0=0 means disabled
    cpu.csr_write(CSR_TCFG, 0x0100);

    cpu.timer_tick(0x50);

    assert_eq!(cpu.csr_read(CSR_ESTAT) & (1 << 11), 0);
}

#[test]
fn timer_tick_partial_countdown() {
    let mut cpu = LoongArchCpu::new();
    // enable=1, periodic=0, initval=0x100
    cpu.csr_write(CSR_TCFG, 0x0101);

    cpu.timer_tick(0x50);

    assert_eq!(cpu.csr_read(CSR_ESTAT) & (1 << 11), 0);
    assert_eq!(cpu.tval(), 0xB0);
}

#[test]
fn task18_tcfg_masks_initval_and_tval_is_readonly() {
    let mut cpu = LoongArchCpu::new();

    cpu.csr_write(CSR_TCFG, u64::MAX);

    assert_eq!(cpu.csr_read(CSR_TCFG), TCFG_WRITE_MASK);
    assert_eq!(cpu.tval(), TCFG_WRITE_MASK & !0x3);

    let tval = cpu.csr_read(CSR_TVAL);
    cpu.csr_write(CSR_TVAL, 0);
    assert_eq!(cpu.csr_read(CSR_TVAL), tval);
}

#[test]
fn task18_one_shot_expiry_sets_timer_interrupt_and_disables_timer() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_TCFG, 0x0101);

    cpu.timer_tick(0x100);

    assert_ne!(cpu.csr_read(CSR_ESTAT) & TIMER_INTERRUPT, 0);
    assert_eq!(cpu.tval(), 0);
    assert_eq!(cpu.csr_read(CSR_TCFG) & 1, 0);
}

#[test]
fn task18_periodic_expiry_sets_interrupt_and_keeps_enabled() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_TCFG, 0x0103);

    cpu.timer_tick(0x100);

    assert_ne!(cpu.csr_read(CSR_ESTAT) & TIMER_INTERRUPT, 0);
    assert_eq!(cpu.tval(), 0x100);
    assert_eq!(cpu.csr_read(CSR_TCFG) & 0x3, 0x3);
}

#[test]
fn task18_disabled_timer_preserves_tval_without_interrupt() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_TCFG, 0x0101);
    cpu.csr_write(CSR_TCFG, 0x0100);

    cpu.timer_tick(0x50);

    assert_eq!(cpu.tval(), 0x100);
    assert_eq!(cpu.csr_read(CSR_ESTAT) & TIMER_INTERRUPT, 0);
}

#[test]
fn task18_partial_countdown_decrements_without_interrupt() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_TCFG, 0x0101);

    cpu.timer_tick(0x50);

    assert_eq!(cpu.tval(), 0xB0);
    assert_eq!(cpu.csr_read(CSR_ESTAT) & TIMER_INTERRUPT, 0);
}

#[test]
fn task18_ticlr_clears_only_timer_pending_bit() {
    let mut cpu = LoongArchCpu::new();
    let other_pending = (1_u64 << 12) | 0x3;
    cpu.set_estat_hw(TIMER_INTERRUPT | other_pending);

    cpu.csr_write(CSR_TICLR, 1);

    assert_eq!(cpu.csr_read(CSR_ESTAT) & TIMER_INTERRUPT, 0);
    assert_eq!(cpu.csr_read(CSR_ESTAT) & other_pending, other_pending);
}

#[test]
fn task18_timer_pending_transition_wakes_halted_and_breaks_chain() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_TCFG, 0x0101);
    cpu.set_halted_flag(true);

    cpu.timer_tick(0x100);

    assert!(!cpu.is_halted());
    assert_eq!(cpu.neg_align_val(), -1);
}

#[test]
fn task18_pending_interrupt_requires_ie_and_timer_lie() {
    let mut cpu = LoongArchCpu::new();
    cpu.set_estat_hw(TIMER_INTERRUPT);

    cpu.csr_write(CSR_CRMD, CRMD_DA);
    cpu.csr_write(CSR_ECFG, TIMER_INTERRUPT);
    assert!(!cpu.pending_interrupt());

    cpu.csr_write(CSR_CRMD, CRMD_DA | CRMD_IE);
    cpu.csr_write(CSR_ECFG, 0);
    assert!(!cpu.pending_interrupt());

    cpu.csr_write(CSR_ECFG, TIMER_INTERRUPT);
    assert!(cpu.pending_interrupt());
}

#[test]
fn plv3_csr_write_raises_ipe() {
    let mut cpu = LoongArchCpu::new();
    // Set PLV=3
    cpu.csr_write(CSR_CRMD, 0x03);
    cpu.set_pc(0x1000);
    cpu.csr_write(CSR_EENTRY, 0x9000_0000);

    let vec = unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_check_plv(cpu.env_ptr())
    };

    assert_ne!(vec, 0);
    assert_eq!(vec, 0x9000_0000);
    assert_eq!((cpu.csr_read(CSR_ESTAT) >> 16) & 0x3F, 0x0E); // IPE
    assert_eq!(cpu.csr_read(CSR_ERA), 0x1000);
}

#[test]
fn plv0_passes_privilege_check() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_CRMD, 0x00); // PLV=0
    cpu.set_pc(0x2000);

    let vec = unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_check_plv(cpu.env_ptr())
    };

    assert_eq!(vec, 0);
}

#[test]
fn task19_plv1_and_plv2_pass_privilege_check() {
    for plv in 1_u64..=2 {
        let mut cpu = LoongArchCpu::new();
        cpu.csr_write(CSR_CRMD, CRMD_DA | plv);
        cpu.set_pc(0x2000 + plv * 4);

        let vec = unsafe {
            machina_guest_loongarch::loongarch::trans::helpers
                ::loongarch_helper_check_plv(cpu.env_ptr())
        };

        assert_eq!(vec, 0, "PLV{plv} should execute privileged operations");
        assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PLV_MASK, u64::from(plv));
    }
}

#[test]
fn ertn_from_tlbr_restores_paged_mode() {
    let mut cpu = LoongArchCpu::new();
    // Simulate being in TLB refill handler: DA=1, PG=0
    cpu.csr_write(CSR_CRMD, CRMD_DA);
    // TLBRERA with ISTLBR=1, PC=0xABC0
    cpu.csr_write(CSR_TLBRERA, 0xABC0 | 1);
    // Pre-refill state was: PLV=0, IE=1, DA=0, PG=1
    cpu.csr_write(CSR_TLBRPRMD, 0x04); // IE=1, PLV=0

    let pc = unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_ertn(cpu.env_ptr())
    };

    assert_eq!(pc, 0xABC0);
    let crmd = cpu.csr_read(CSR_CRMD);
    assert_eq!(crmd & CRMD_DA, 0); // DA cleared
    assert_ne!(crmd & CRMD_PG, 0); // PG set
    assert_eq!(crmd & 0x7, 0x04); // PLV=0, IE=1
}

#[test]
fn cpucfg_returns_la464_prid() {
    let mut cpu = LoongArchCpu::new();
    let result = unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_cpucfg(cpu.env_ptr(), 0)
    };
    assert_eq!(result, 0x0014_C010);
}

#[test]
fn cpucfg_index1_reports_la64() {
    let mut cpu = LoongArchCpu::new();
    let result = unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_cpucfg(cpu.env_ptr(), 1)
    };
    assert_eq!(result, 0x03F2_F2FE);
    assert_eq!(result & 0x3, 2); // ARCH=2 (LA64)
    assert_ne!(result & (1 << 2), 0); // PGMMU
    assert_ne!(result & (1 << 3), 0); // IOCSR
    assert_eq!((result >> 4) & 0xFF, 47); // PALEN field = 47
    assert_eq!((result >> 12) & 0xFF, 47); // VALEN field = 47
    assert_eq!(result & (1 << 26), 0); // MSG_INT = 0
}

#[test]
fn cpucfg_index2_reports_fp_no_lsx() {
    use machina_guest_loongarch::loongarch::ext::LoongArchCfg;

    let mut cpu = LoongArchCpu::new();
    let result = unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_cpucfg(cpu.env_ptr(), 2)
    };
    assert_eq!(result, LoongArchCfg::default().cpucfg2());
    assert_eq!(result, 0x0060_CC0F);
    assert_ne!(result & 1, 0); // FP_SP
    assert_ne!(result & 2, 0); // FP_DP
    assert_eq!(result & (1 << 6), 0); // LSX=0
    assert_eq!(result & (1 << 7), 0); // LASX=0
    assert_ne!(result & (1 << 10), 0); // LVZ=1
    assert_ne!(result & (0x7 << 11), 0); // LVZ_VER non-zero
    assert_eq!(result & (1 << 18), 0); // LBT_X86=0
    assert_eq!(result & (1 << 19), 0); // LBT_ARM=0
    assert_eq!(result & (1 << 20), 0); // LBT_MIPS=0
}

#[test]
fn cpucfg_index2_derives_lsx_lasx_from_cpu_config() {
    use machina_guest_loongarch::loongarch::ext::LoongArchCfg;

    let mut cpu = LoongArchCpu::with_cfg(LoongArchCfg {
        has_lsx: true,
        has_lasx: true,
        ..LoongArchCfg::default()
    });
    let result = unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_cpucfg(cpu.env_ptr(), 2)
    };
    assert_eq!(result, 0x0060_CCCF);
    assert_ne!(result & (1 << 6), 0);
    assert_ne!(result & (1 << 7), 0);
}

#[test]
fn lvz_guest_control_csrs_report_gid_bits_and_mask_writes() {
    let mut cpu = LoongArchCpu::new();

    assert_eq!((cpu.csr_read(CSR_GSTAT) >> 4) & 0x3F, 8);

    cpu.csr_write(CSR_GSTAT, u64::MAX);
    assert_eq!(cpu.csr_read(CSR_GSTAT) & 0x00FF_0002, 0x00FF_0002);
    assert_eq!(cpu.csr_read(CSR_GSTAT) & 1, 0);
    assert_eq!((cpu.csr_read(CSR_GSTAT) >> 4) & 0x3F, 8);

    cpu.csr_write(CSR_GTLBC, u64::MAX);
    assert_eq!(cpu.csr_read(CSR_GTLBC), 0x00FF_303F);
}

#[test]
fn lvz_gintc_write_injects_guest_interrupt_pending_bits() {
    let mut cpu = LoongArchCpu::new();

    cpu.gcsr_write(CSR_ESTAT, 0x3);
    cpu.csr_write(CSR_GINTC, 0x00FF_FF55);

    assert_eq!(cpu.csr_read(CSR_GINTC), 0x00FF_FF00);
    assert_eq!(cpu.gcsr_read(CSR_ESTAT) & 0x3FF, 0x157);
}

#[test]
fn lvz_gintc_hwip_routes_hwi_to_guest_estat() {
    let mut cpu = LoongArchCpu::new();

    cpu.csr_write(CSR_GINTC, 0x0000_FF00);
    cpu.set_hwi_interrupt_pending(3, true);

    assert_eq!(cpu.csr_read(CSR_ESTAT) & (1 << 5), 0);
    assert_eq!(cpu.gcsr_read(CSR_ESTAT) & (1 << 5), 1 << 5);

    cpu.set_hwi_interrupt_pending(3, false);
    assert_eq!(cpu.gcsr_read(CSR_ESTAT) & (1 << 5), 0);
}

#[test]
fn lvz_ertn_with_previous_guest_mode_enters_guest_mode() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_ERA, 0x8020_0000);
    cpu.csr_xchg(CSR_GSTAT, 0x2, 0x2);

    let pc = unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_ertn(cpu.env_ptr())
    };

    assert_eq!(pc, 0x8020_0000);
    assert_ne!(cpu.csr_read(CSR_GSTAT) & 1, 0);
    assert_eq!(cpu.csr_read(CSR_GSTAT) & 0x2, 0);
}

#[test]
fn lvz_guest_ertn_restores_guest_prmd_and_era() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_ERA, 0x9000_0000);
    cpu.gcsr_write(CSR_CRMD, CRMD_DA);
    cpu.gcsr_write(CSR_PRMD, CRMD_PLV_MASK | CRMD_IE);
    cpu.gcsr_write(CSR_ERA, 0x8000_2000);
    cpu.gcsr_write(CSR_TLBRERA, 0);
    enter_guest_mode_for_test(&mut cpu);

    let pc = unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_ertn(cpu.env_ptr())
    };

    assert_eq!(pc, 0x8000_2000);
    assert_ne!(cpu.csr_read(CSR_GSTAT) & GSTAT_VM, 0);
    assert_eq!(
        cpu.gcsr_read(CSR_CRMD) & (CRMD_PLV_MASK | CRMD_IE),
        CRMD_PLV_MASK | CRMD_IE
    );
    assert_ne!(cpu.gcsr_read(CSR_CRMD) & CRMD_DA, 0);
    assert_eq!(cpu.csr_read(CSR_ERA), 0x9000_0000);
}

#[test]
fn lvz_guest_tlbr_ertn_restores_guest_tlbrprmd() {
    let mut cpu = LoongArchCpu::new();
    cpu.gcsr_write(CSR_CRMD, CRMD_DA);
    cpu.gcsr_write(CSR_TLBRPRMD, CRMD_PLV_MASK | CRMD_IE);
    cpu.gcsr_write(CSR_TLBRERA, 0x8000_3000 | 1);
    enter_guest_mode_for_test(&mut cpu);

    let pc = unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_ertn(cpu.env_ptr())
    };

    assert_eq!(pc, 0x8000_3000);
    assert_eq!(cpu.gcsr_read(CSR_TLBRERA) & 1, 0);
    assert_eq!(cpu.gcsr_read(CSR_CRMD) & CRMD_DA, 0);
    assert_ne!(cpu.gcsr_read(CSR_CRMD) & CRMD_PG, 0);
    assert_eq!(
        cpu.gcsr_read(CSR_CRMD) & (CRMD_PLV_MASK | CRMD_IE),
        CRMD_PLV_MASK | CRMD_IE
    );
    assert_eq!(cpu.csr_read(CSR_TLBRERA), 0);
}

#[test]
fn translated_gcsrwr_and_gcsrrd_access_guest_csr_state() {
    let mut cpu = LoongArchCpu::new();
    cpu.write_gpr(4, 0x9000_0000);

    assert_eq!(run_priv_la(&mut cpu, &[gcsr_insn(CSR_EENTRY, 1, 4)]), 0);
    assert_eq!(
        run_priv_la_at(&mut cpu, &[gcsr_insn(CSR_EENTRY, 0, 5)], 4),
        0
    );
    assert_eq!(cpu.read_gpr(4), 0);
    assert_eq!(cpu.read_gpr(5), 0x9000_0000);
    assert_eq!(cpu.csr_read(CSR_EENTRY), 0);
}

#[test]
fn translated_gcsrxchg_masks_guest_csr_state() {
    // GCSRXCHG (rj >= 2) is the masked read-modify-write form of the
    // guest-CSR accessors: rd supplies the new bits and receives the
    // old value, while rj selects which bits actually change.
    let mut cpu = LoongArchCpu::new();
    cpu.gcsr_write(CSR_EENTRY, 0x9000_0000);
    cpu.write_gpr(6, 0x1000_0000); // rj: mask selects bit 28 only
    cpu.write_gpr(7, 0x2000_0000); // rd: candidate new value bits

    assert_eq!(run_priv_la(&mut cpu, &[gcsr_insn(CSR_EENTRY, 6, 7)]), 0);

    // rd receives the old CSR value.
    assert_eq!(cpu.read_gpr(7), 0x9000_0000);
    // Only the masked bit 28 changes (cleared, since rd's bit 28 is 0);
    // bit 31 is preserved and rd's unmasked bit 29 is ignored.
    assert_eq!(cpu.gcsr_read(CSR_EENTRY), 0x8000_0000);
}

#[test]
fn translated_gcsrxchg_with_zero_mask_only_reads() {
    // A zero mask must leave the guest CSR untouched while still
    // returning the old value into rd.
    let mut cpu = LoongArchCpu::new();
    cpu.gcsr_write(CSR_EENTRY, 0x9000_0000);
    cpu.write_gpr(6, 0); // rj: empty mask
    cpu.write_gpr(7, 0x2000_0000);

    assert_eq!(run_priv_la(&mut cpu, &[gcsr_insn(CSR_EENTRY, 6, 7)]), 0);

    assert_eq!(cpu.read_gpr(7), 0x9000_0000);
    assert_eq!(cpu.gcsr_read(CSR_EENTRY), 0x9000_0000);
}

#[test]
fn translated_hvcl_raises_hvc_exception() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_EENTRY, 0x9000_0000);

    assert_eq!(
        run_priv_la(&mut cpu, &[code15_insn(HVCL_OP, 0x100)]),
        EXCP_LOONGARCH_DONE as usize
    );
    assert_eq!(cpu.pc(), 0x9000_0000);
    assert_eq!((cpu.csr_read(CSR_ESTAT) >> 16) & 0x3F, 23);
    assert_eq!(cpu.csr_read(CSR_ERA), 0);
}

#[test]
fn translated_guest_sensitive_ops_raise_gspr() {
    let cases = [
        ("cpucfg", r2_insn(CPUCFG_OP, 5, 4)),
        ("idle", code15_insn(IDLE_OP, 0)),
        ("cacop", cop_r_si12(CACOP_OP, 0, 0, 0)),
        ("iocsrrd.w", r2_insn(IOCSRRD_W_OP, 5, 4)),
        ("iocsrwr.w", r2_insn(IOCSRWR_W_OP, 5, 4)),
    ];

    for (name, insn) in cases {
        let mut cpu = LoongArchCpu::new();
        cpu.csr_write(CSR_EENTRY, 0x9000_0000);
        cpu.write_gpr(4, 0x55);
        cpu.write_gpr(5, 2);
        enter_guest_mode_for_test(&mut cpu);

        assert_eq!(
            run_priv_la(&mut cpu, &[insn]),
            EXCP_LOONGARCH_DONE as usize,
            "{name} did not exit to the hypervisor"
        );
        assert_eq!(cpu.pc(), 0x9000_0000, "{name} used wrong vector");
        assert_eq!(cpu.csr_read(CSR_ERA), 0, "{name} saved wrong ERA");
        assert_eq!(
            (cpu.csr_read(CSR_ESTAT) >> 16) & 0x3F,
            u64::from(ECODE_GSPR),
            "{name} did not raise GSPR"
        );
        assert_eq!(cpu.csr_read(CSR_GSTAT) & GSTAT_VM, 0, "{name} kept VM");
        assert_ne!(cpu.csr_read(CSR_GSTAT) & GSTAT_PVM, 0, "{name} lost PVM");
        assert_eq!(cpu.read_gpr(4), 0x55, "{name} changed GPR state");
        assert!(!cpu.is_halted(), "{name} halted instead of trapping");
    }
}

#[test]
fn translated_guest_csr_access_uses_gcsr_state() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_EENTRY, 0x9000_0000);
    cpu.gcsr_write(CSR_EENTRY, 0x8000_0000);
    enter_guest_mode_for_test(&mut cpu);

    assert_eq!(run_priv_la(&mut cpu, &[csr_insn(CSR_EENTRY, 0, 4)]), 0);
    assert_eq!(cpu.read_gpr(4), 0x8000_0000);
    assert_eq!(cpu.csr_read(CSR_EENTRY), 0x9000_0000);
    assert_ne!(cpu.csr_read(CSR_GSTAT) & GSTAT_VM, 0);

    cpu.write_gpr(5, 0x7000_0000);
    assert_eq!(
        run_priv_la_at(&mut cpu, &[csr_insn(CSR_EENTRY, 1, 5)], 4),
        0
    );
    assert_eq!(cpu.read_gpr(5), 0x8000_0000);
    assert_eq!(cpu.gcsr_read(CSR_EENTRY), 0x7000_0000);
    assert_eq!(cpu.csr_read(CSR_EENTRY), 0x9000_0000);
}

#[test]
fn translated_guest_plv3_csr_access_raises_guest_ipe() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_EENTRY, 0x9000_0000);
    cpu.gcsr_write(CSR_EENTRY, 0x8000_0000);
    cpu.gcsr_write(CSR_CRMD, CRMD_DA | CRMD_PLV_MASK);
    cpu.write_gpr(4, 0x55);
    enter_guest_mode_for_test(&mut cpu);

    assert_eq!(
        run_priv_la(&mut cpu, &[csr_insn(CSR_EENTRY, 0, 4)]),
        EXCP_LOONGARCH_DONE as usize
    );
    assert_eq!(cpu.pc(), 0x8000_0000);
    assert_ne!(cpu.csr_read(CSR_GSTAT) & GSTAT_VM, 0);
    assert_eq!(cpu.csr_read(CSR_GSTAT) & GSTAT_PVM, 0);
    assert_eq!(cpu.gcsr_read(CSR_ERA), 0);
    assert_eq!(
        (cpu.gcsr_read(CSR_ESTAT) >> 16) & 0x3F,
        u64::from(ECODE_IPE)
    );
    assert_eq!(cpu.csr_read(CSR_ERA), 0);
    assert_eq!(cpu.read_gpr(4), 0x55);
}

#[test]
fn translated_guest_invalid_csr_raises_gspr() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_EENTRY, 0x9000_0000);
    cpu.write_gpr(4, 0x55);
    enter_guest_mode_for_test(&mut cpu);

    assert_eq!(
        run_priv_la(&mut cpu, &[csr_insn(0x400, 0, 4)]),
        EXCP_LOONGARCH_DONE as usize
    );
    assert_eq!(cpu.pc(), 0x9000_0000);
    assert_eq!(cpu.csr_read(CSR_ERA), 0);
    assert_eq!(
        (cpu.csr_read(CSR_ESTAT) >> 16) & 0x3F,
        u64::from(ECODE_GSPR)
    );
    assert_eq!(cpu.csr_read(CSR_GSTAT) & GSTAT_VM, 0);
    assert_ne!(cpu.csr_read(CSR_GSTAT) & GSTAT_PVM, 0);
    assert_eq!(cpu.read_gpr(4), 0x55);
}

#[test]
fn translated_guest_lvz_control_csr_access_raises_gspr() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_EENTRY, 0x9000_0000);
    cpu.write_gpr(4, 0x55);
    enter_guest_mode_for_test(&mut cpu);

    assert_eq!(
        run_priv_la(&mut cpu, &[csr_insn(CSR_GSTAT, 0, 4)]),
        EXCP_LOONGARCH_DONE as usize
    );
    assert_eq!(cpu.pc(), 0x9000_0000);
    assert_eq!(cpu.csr_read(CSR_ERA), 0);
    assert_eq!(
        (cpu.csr_read(CSR_ESTAT) >> 16) & 0x3F,
        u64::from(ECODE_GSPR)
    );
    assert_eq!(cpu.csr_read(CSR_GSTAT) & GSTAT_VM, 0);
    assert_ne!(cpu.csr_read(CSR_GSTAT) & GSTAT_PVM, 0);
    assert_eq!(cpu.read_gpr(4), 0x55);
}

#[test]
fn translated_guest_syscall_uses_guest_exception_csrs() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_EENTRY, 0x9000_0000);
    cpu.gcsr_write(CSR_EENTRY, 0x8000_0000);
    enter_guest_mode_for_test(&mut cpu);

    assert_eq!(
        run_priv_la(&mut cpu, &[code15_insn(SYSCALL_OP, 0)]),
        EXCP_LOONGARCH_DONE as usize
    );
    assert_eq!(cpu.pc(), 0x8000_0000);
    assert_ne!(cpu.csr_read(CSR_GSTAT) & GSTAT_VM, 0);
    assert_eq!(cpu.csr_read(CSR_GSTAT) & GSTAT_PVM, 0);
    assert_eq!(cpu.gcsr_read(CSR_ERA), 0);
    assert_eq!(
        (cpu.gcsr_read(CSR_ESTAT) >> 16) & 0x3F,
        u64::from(ECODE_SYS)
    );
    assert_eq!(cpu.csr_read(CSR_ERA), 0);
}

#[test]
fn lvz_guest_interrupt_uses_guest_pending_and_vector_state() {
    let mut cpu = LoongArchCpu::new();
    cpu.gcsr_write(CSR_CRMD, 0x8 | CRMD_IE);
    cpu.gcsr_write(CSR_ECFG, 1 << 5);
    cpu.gcsr_write(CSR_EENTRY, 0x8000_0000);
    enter_guest_mode_for_test(&mut cpu);

    cpu.csr_write(CSR_GINTC, 0x0000_FF00);
    cpu.set_hwi_interrupt_pending(3, true);

    assert_eq!(cpu.pending_interrupt_line(), Some(5));
    let vec = unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_raise_exception(cpu.env_ptr(), u64::from(ECODE_INT), 0)
    };
    assert_eq!(cpu.external_interrupt_vector(5, vec), 0x8000_0000);
    assert_ne!(cpu.csr_read(CSR_GSTAT) & GSTAT_VM, 0);
    assert_eq!(
        (cpu.gcsr_read(CSR_ESTAT) >> 16) & 0x3F,
        u64::from(ECODE_INT)
    );
    assert_eq!(cpu.gcsr_read(CSR_ERA), 0);
}

#[test]
fn lvz_guest_da_translation_uses_guest_crmd_not_host_crmd() {
    let mut cpu = LoongArchCpu::new();
    let ps = 12;
    let idx = mmu::mtlb_flat_index(5).unwrap();

    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_STLBPS, ps);
    write_test_tlb_entry(
        &mut cpu,
        idx,
        0x1234,
        ps as u8,
        0,
        tlbelo_test(0, true, true, 0, 0, true, false),
        tlbelo_test(1, true, true, 0, 0, true, false),
    );
    cpu.gcsr_write(CSR_CRMD, CRMD_DA);
    enter_guest_mode_for_test(&mut cpu);

    assert_tlb_hit(
        cpu.translate_address(0x1234, mmu::AccessType::Load),
        0x1234,
        0,
    );
}

#[test]
fn lvz_guest_paging_ignores_host_direct_map_state() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_CRMD, CRMD_DA);
    cpu.gcsr_write(CSR_CRMD, CRMD_PG);
    enter_guest_mode_for_test(&mut cpu);

    assert_eq!(
        cpu.translate_address(0x1234, mmu::AccessType::Load),
        mmu::TlbLookupResult::Miss
    );
}

#[test]
fn lvz_guest_paging_uses_guest_tlb_not_host_tlb() {
    let mut cpu = LoongArchCpu::new();
    let va = 0x4000_0000;
    let ps = 12;
    let idx = mmu::mtlb_flat_index(6).unwrap();
    let stage2_idx = mmu::mtlb_flat_index(12).unwrap();

    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_STLBPS, ps);
    write_test_tlb_entry(
        &mut cpu,
        idx,
        va,
        ps as u8,
        0x11,
        tlbelo_test(0x111, true, true, 0, 1, false, false),
        tlbelo_test(0x112, true, true, 0, 1, false, false),
    );
    write_test_tlb_entry(
        &mut cpu,
        stage2_idx,
        0x220000,
        ps as u8,
        0,
        tlbelo_test(0x551, true, true, 0, 3, true, false),
        tlbelo_test(0x552, true, true, 0, 3, true, false),
    );

    cpu.gcsr_write(CSR_CRMD, CRMD_PG);
    cpu.gcsr_write(CSR_STLBPS, ps);
    enter_guest_mode_for_test(&mut cpu);
    write_guest_test_tlb_entry(
        &mut cpu,
        idx,
        va,
        ps as u8,
        0x22,
        tlbelo_test(0x220, true, true, 0, 2, false, false),
        tlbelo_test(0x221, true, true, 0, 2, false, false),
    );

    assert_tlb_hit(
        cpu.translate_address(va, mmu::AccessType::Load),
        0x551000,
        3,
    );
    assert_tlb_hit(
        cpu.translate_address(va + 0x1000, mmu::AccessType::Load),
        0x552000,
        3,
    );
}

#[test]
fn translated_guest_tlbsrch_uses_guest_tlb_state() {
    let mut cpu = LoongArchCpu::new();
    let va = 0x5000_0000;
    let ps = 12;
    let idx = mmu::mtlb_flat_index(7).unwrap();

    cpu.csr_write(CSR_EENTRY, 0x9000_0000);
    cpu.gcsr_write(CSR_CRMD, CRMD_PG);
    cpu.gcsr_write(CSR_STLBPS, ps);
    enter_guest_mode_for_test(&mut cpu);
    write_guest_test_tlb_entry(
        &mut cpu,
        idx,
        va,
        ps as u8,
        0x33,
        tlbelo_test(0x331, true, true, 0, 2, false, false),
        tlbelo_test(0x332, true, true, 0, 2, false, false),
    );
    cpu.gcsr_write(CSR_TLBEHI, tlb_pair_base(va, ps as u8) & !0x1FFF);
    cpu.gcsr_write(CSR_TLBIDX, 1 << 31);

    assert_eq!(run_priv_la(&mut cpu, &[TLBSRCH_INSN]), 0);
    assert_ne!(cpu.csr_read(CSR_GSTAT) & GSTAT_VM, 0);
    assert_eq!(cpu.csr_read(CSR_TLBIDX) & 0xFFF, 0);
    assert_eq!(cpu.gcsr_read(CSR_TLBIDX) & (1 << 31), 0);
    assert_eq!(cpu.gcsr_read(CSR_TLBIDX) & 0xFFF, idx as u64);
}

#[test]
fn lvz_host_tlb_entries_match_gtlbc_target_gid() {
    let mut cpu = LoongArchCpu::new();
    let gpa = 0x6000_0000;
    let ps = 12;
    let idx = mmu::mtlb_flat_index(8).unwrap();

    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_STLBPS, ps);
    cpu.csr_write(CSR_GTLBC, (1 << 12) | (3 << 16));
    write_test_tlb_entry(
        &mut cpu,
        idx,
        gpa,
        ps as u8,
        0,
        tlbelo_test(0x701, true, true, 0, 2, true, false),
        tlbelo_test(0x702, true, true, 0, 2, true, false),
    );

    assert_tlb_hit(
        cpu.translate_address(gpa, mmu::AccessType::Load),
        0x701000,
        2,
    );

    cpu.csr_write(CSR_GTLBC, (1 << 12) | (4 << 16));
    assert_eq!(
        cpu.translate_address(gpa, mmu::AccessType::Load),
        mmu::TlbLookupResult::Miss
    );

    cpu.csr_write(CSR_GTLBC, 0);
    assert_eq!(
        cpu.translate_address(gpa, mmu::AccessType::Load),
        mmu::TlbLookupResult::Miss
    );
}

#[test]
fn lvz_guest_translation_applies_second_stage_host_tlb_gid() {
    let mut cpu = LoongArchCpu::new();
    let gid = 7;
    let gva = 0x4000_0000;
    let gpa = 0x1000_0000;
    let ps = 12;
    let host_idx = mmu::mtlb_flat_index(9).unwrap();
    let guest_idx = mmu::mtlb_flat_index(10).unwrap();

    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_STLBPS, ps);
    cpu.csr_write(CSR_GSTAT, gid << 16);
    cpu.csr_write(CSR_GTLBC, (1 << 12) | (gid << 16));
    write_test_tlb_entry(
        &mut cpu,
        host_idx,
        gpa,
        ps as u8,
        0,
        tlbelo_test(0x801, true, true, 0, 1, true, false),
        tlbelo_test(0x802, true, true, 0, 1, true, false),
    );

    cpu.gcsr_write(CSR_CRMD, CRMD_PG);
    cpu.gcsr_write(CSR_STLBPS, ps);
    enter_guest_mode_for_test(&mut cpu);
    write_guest_test_tlb_entry(
        &mut cpu,
        guest_idx,
        gva,
        ps as u8,
        0x44,
        tlbelo_test(gpa >> 12, true, true, 0, 2, false, false),
        tlbelo_test((gpa >> 12) + 1, true, true, 0, 2, false, false),
    );

    assert_tlb_hit(
        cpu.translate_address(gva, mmu::AccessType::Load),
        0x801000,
        1,
    );
}

#[test]
fn lvz_guest_second_stage_miss_exits_to_host_gcm() {
    let mut cpu = LoongArchCpu::new();
    let gid = 5;
    let gva = 0x4100_0000;
    let gpa = 0x1100_0000;
    let fault_pc = 0x1234_5600;
    let eentry = 0x9000_2000;
    let ps = 12;
    let guest_idx = mmu::mtlb_flat_index(11).unwrap();

    cpu.csr_write(CSR_CRMD, 2 | CRMD_IE | CRMD_PG);
    cpu.csr_write(CSR_PRMD, 2 | CRMD_IE);
    cpu.csr_write(CSR_EENTRY, eentry);
    cpu.csr_write(CSR_TLBREHI, 0xDEAD_0000_0000_002A);
    let old_tlbrehi = cpu.csr_read(CSR_TLBREHI);
    cpu.csr_write(CSR_GSTAT, gid << 16);
    cpu.gcsr_write(CSR_CRMD, CRMD_PG);
    cpu.gcsr_write(CSR_STLBPS, ps);
    enter_guest_mode_for_test(&mut cpu);
    write_guest_test_tlb_entry(
        &mut cpu,
        guest_idx,
        gva,
        ps as u8,
        0x55,
        tlbelo_test(gpa >> 12, true, true, 0, 2, false, false),
        tlbelo_test((gpa >> 12) + 1, true, true, 0, 2, false, false),
    );

    assert_eq!(
        cpu.translate_address_or_exception(
            gva,
            mmu::AccessType::Load,
            fault_pc,
        ),
        Err(eentry)
    );
    assert_eq!(cpu.csr_read(CSR_GSTAT) & GSTAT_VM, 0);
    assert_ne!(cpu.csr_read(CSR_GSTAT) & GSTAT_PVM, 0);
    assert_eq!(cpu.csr_read(CSR_ERA), fault_pc);
    assert_eq!(cpu.csr_read(CSR_PRMD), 2 | CRMD_IE);
    assert_eq!(cpu.csr_read(CSR_BADV), gpa);
    let estat = cpu.csr_read(CSR_ESTAT);
    assert_eq!((estat >> 16) & 0x3F, u64::from(ECODE_GCM));
    assert_eq!((estat >> 22) & 0x1FF, u64::from(ESUBCODE_GCHC));
    assert_eq!(cpu.csr_read(CSR_TLBRERA) & 1, 0);
    assert_eq!(cpu.csr_read(CSR_TLBRBADV), 0);
    assert_eq!(cpu.csr_read(CSR_TLBREHI), old_tlbrehi);
    assert_eq!(cpu.gcsr_read(CSR_TLBRBADV), 0);
}

#[test]
fn task97_helper_load_stage2_miss_exits_to_host_gcm() {
    let mut cpu = LoongArchCpu::new();
    let gid = 6;
    let gva = 0x4200_0000;
    let gpa = 0x1200_0000;
    let fault_pc = 0x1234_5800;
    let eentry = 0x9000_3000;
    let ps = 12;
    let guest_idx = mmu::mtlb_flat_index(12).unwrap();

    cpu.csr_write(CSR_CRMD, 2 | CRMD_IE | CRMD_PG);
    cpu.csr_write(CSR_PRMD, 2 | CRMD_IE);
    cpu.csr_write(CSR_EENTRY, eentry);
    cpu.csr_write(CSR_TLBREHI, 0xDEAD_0000_0000_002A);
    let old_tlbrehi = cpu.csr_read(CSR_TLBREHI);
    cpu.csr_write(CSR_GSTAT, gid << 16);
    cpu.gcsr_write(CSR_CRMD, CRMD_PG);
    cpu.gcsr_write(CSR_STLBPS, ps);
    enter_guest_mode_for_test(&mut cpu);
    write_guest_test_tlb_entry(
        &mut cpu,
        guest_idx,
        gva,
        ps as u8,
        0x56,
        tlbelo_test(gpa >> 12, true, true, 0, 2, false, false),
        tlbelo_test((gpa >> 12) + 1, true, true, 0, 2, false, false),
    );
    cpu.set_memory_fault_pending(fault_pc);

    let value = unsafe { loongarch_mem_read(cpu.env_ptr(), gva, 8) };

    assert_eq!(value, 0);
    assert_eq!(cpu.pc(), eentry);
    assert_eq!(cpu.csr_read(CSR_GSTAT) & GSTAT_VM, 0);
    assert_ne!(cpu.csr_read(CSR_GSTAT) & GSTAT_PVM, 0);
    assert_eq!(cpu.csr_read(CSR_ERA), fault_pc);
    assert_eq!(cpu.csr_read(CSR_BADV), gpa);
    let estat = cpu.csr_read(CSR_ESTAT);
    assert_eq!((estat >> 16) & 0x3F, u64::from(ECODE_GCM));
    assert_eq!((estat >> 22) & 0x1FF, u64::from(ESUBCODE_GCHC));
    assert_eq!(cpu.csr_read(CSR_TLBRERA) & 1, 0);
    assert_eq!(cpu.csr_read(CSR_TLBRBADV), 0);
    assert_eq!(cpu.csr_read(CSR_TLBREHI), old_tlbrehi);
    assert_eq!(cpu.gcsr_read(CSR_TLBRBADV), 0);
}

#[test]
fn lvz_guest_lddir_ldpte_use_guest_walk_csrs_and_stage2() {
    let mut ram = vec![0_u8; 0x8000];
    let mut cpu = LoongArchCpu::new();
    let gid = 9;
    let va = 0x0000_0000_0001_5000;
    let dir_gpa = 0x2000;
    let pte_gpa = 0x3000;
    let dir_hpa = 0x5000;
    let pte_hpa = 0x6000;
    let ps = 12;
    let stage2_idx = mmu::mtlb_flat_index(13).unwrap();

    attach_page_walk_ram(&mut cpu, &ram);
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_STLBPS, ps);
    cpu.csr_write(CSR_GSTAT, gid << 16);
    cpu.csr_write(CSR_GTLBC, (1 << 12) | (gid << 16));
    write_test_tlb_entry(
        &mut cpu,
        stage2_idx,
        dir_gpa,
        ps as u8,
        0,
        tlbelo_test(dir_hpa >> 12, true, true, 0, 0, true, false),
        tlbelo_test(pte_hpa >> 12, true, true, 0, 0, true, false),
    );

    let dir_index = (va >> 15) & 0x7;
    let ptindex = (va >> 12) & 0x7;
    let pair = ptindex & !1;
    let elo0 = tlbelo_test(0xA10, true, true, 0, 1, false, false);
    let elo1 = tlbelo_test(0xA11, true, false, 0, 2, false, false);
    write_ram_u64(&mut ram, dir_hpa | (dir_index << 3), pte_gpa);
    write_ram_u64(&mut ram, pte_hpa | (pair << 3), elo0);
    write_ram_u64(&mut ram, pte_hpa | ((pair + 1) << 3), elo1);
    attach_page_walk_ram(&mut cpu, &ram);

    cpu.csr_write(CSR_PWCL, pwcl(14, 3, 20, 3, 0, 0));
    cpu.csr_write(CSR_TLBRBADV, 0);
    cpu.csr_write(CSR_TLBREHI, 0xDEAD_0000_0000_0000);
    cpu.gcsr_write(CSR_PWCL, pwcl(12, 3, 15, 3, 0, 0));
    cpu.gcsr_write(CSR_TLBRBADV, va);
    cpu.gcsr_write(CSR_TLBREHI, va & !0x1FFF);
    enter_guest_mode_for_test(&mut cpu);

    let walked = cpu.lddir(dir_gpa, 1);
    assert_eq!(walked, pte_gpa);
    cpu.ldpte(walked, 0);
    cpu.ldpte(walked, 1);

    assert_eq!(cpu.gcsr_read(CSR_TLBRELO0), elo0);
    assert_eq!(cpu.gcsr_read(CSR_TLBRELO1), elo1);
    assert_eq!(cpu.gcsr_read(CSR_TLBREHI) & 0x3F, ps);
    assert_eq!(cpu.csr_read(CSR_TLBRELO0), 0);
    assert_eq!(cpu.csr_read(CSR_TLBRELO1), 0);
}

#[test]
fn lvz_guest_timer_uses_guest_tcfg_tval_and_ticlr() {
    let mut cpu = LoongArchCpu::new();
    enter_guest_mode_for_test(&mut cpu);

    cpu.gcsr_write(CSR_TCFG, 0x0101);
    assert_eq!(cpu.gcsr_read(CSR_TVAL), 0x0100);

    cpu.timer_tick(0x0100);

    assert_ne!(cpu.gcsr_read(CSR_ESTAT) & TIMER_INTERRUPT, 0);
    assert_eq!(cpu.csr_read(CSR_ESTAT) & TIMER_INTERRUPT, 0);
    assert_eq!(cpu.gcsr_read(CSR_TCFG) & 1, 0);

    cpu.gcsr_write(CSR_TICLR, 1);
    assert_eq!(cpu.gcsr_read(CSR_ESTAT) & TIMER_INTERRUPT, 0);
}

#[test]
fn lvz_guest_asid_change_flushes_fast_translation_cache() {
    let mut cpu = LoongArchCpu::new();
    let gva = 0x4200_0000;
    let gpa = 0x220000;
    let hpa = 0x400000;
    let ps = 12;
    let host_idx = mmu::mtlb_flat_index(14).unwrap();
    let guest_idx = mmu::mtlb_flat_index(15).unwrap();

    cpu.set_guest_base(0x10_0000_0000);
    cpu.set_ram_base(0);
    cpu.set_ram_end(0x800000);
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_STLBPS, ps);
    write_test_tlb_entry(
        &mut cpu,
        host_idx,
        gpa,
        ps as u8,
        0,
        tlbelo_test(hpa >> 12, true, true, 0, 0, true, false),
        tlbelo_test((hpa >> 12) + 1, true, true, 0, 0, true, false),
    );
    cpu.gcsr_write(CSR_CRMD, CRMD_PG);
    cpu.gcsr_write(CSR_STLBPS, ps);
    enter_guest_mode_for_test(&mut cpu);
    write_guest_test_tlb_entry(
        &mut cpu,
        guest_idx,
        gva,
        ps as u8,
        0x12,
        tlbelo_test(gpa >> 12, true, true, 0, 0, false, false),
        tlbelo_test((gpa >> 12) + 1, true, true, 0, 0, false, false),
    );

    assert_tlb_hit(
        cpu.translate_address_and_cache(gva, mmu::AccessType::Load),
        hpa,
        0,
    );
    assert_eq!(
        cpu.fast_tlb_lookup_addend(gva, mmu::AccessType::Load),
        Some(expected_tlb_addend(&cpu, gva, hpa))
    );

    cpu.gcsr_write(CSR_ASID, 0x13);

    assert_eq!(cpu.fast_tlb_lookup_addend(gva, mmu::AccessType::Load), None);
    assert_eq!(
        cpu.translate_address(gva, mmu::AccessType::Load),
        mmu::TlbLookupResult::Miss
    );
}

#[test]
fn cpucfg_index4_and_5_report_la464_cache_and_timer() {
    let mut cpu = LoongArchCpu::new();
    let cache = unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_cpucfg(cpu.env_ptr(), 4)
    };
    let timer = unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_cpucfg(cpu.env_ptr(), 5)
    };

    assert_eq!(cache, 0x05F5_E100);
    assert_eq!(timer, 0x0001_0001);
}

#[test]
fn task48_cpucfg_reports_linux_cache_topology() {
    let mut cpu = LoongArchCpu::new();
    let mut cpucfg = |index| unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_cpucfg(cpu.env_ptr(), index)
    };

    assert_eq!(cpucfg(0x10), 0x0000_2c3d);
    assert_eq!(cpucfg(0x11), 0x0608_0003);
    assert_eq!(cpucfg(0x12), 0x0608_0003);
    assert_eq!(cpucfg(0x13), 0x0608_000f);
    assert_eq!(cpucfg(0x14), 0x060e_000f);
}

#[test]
fn cpucfg_unknown_index_returns_zero() {
    let mut cpu = LoongArchCpu::new();
    let result = unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_cpucfg(cpu.env_ptr(), 0x100)
    };
    assert_eq!(result, 0);
}

#[test]
fn prcfg2_reports_only_minimal_la464_page_sizes() {
    let cpu = LoongArchCpu::new();
    let val = cpu.csr_read(CSR_PRCFG2);
    assert_eq!(val, 0x4020_5000);
    assert_ne!(val & (1 << 12), 0); // 4K supported
    assert_ne!(val & (1 << 14), 0); // 16K supported
    assert_ne!(val & (1 << 21), 0); // 2M supported
    assert_ne!(val & (1 << 30), 0); // 1G supported
    assert_eq!(val & !((1 << 12) | (1 << 14) | (1 << 21) | (1 << 30)), 0);
}

#[test]
fn prcfg3_is_tlb_organization() {
    let cpu = LoongArchCpu::new();
    let val = cpu.csr_read(CSR_PRCFG3);
    assert_eq!(val, 0x0080_73F2);
    assert_eq!(val & 0xF, 2); // TLBType = MTLB+STLB
}

#[test]
fn prcfg1_has_nonzero_reset_value() {
    let cpu = LoongArchCpu::new();
    let val = cpu.csr_read(CSR_PRCFG1);
    assert_eq!(val, 0x0000_72F8);
    assert_eq!(val & 0xF, 8); // SAVE_NUM=8
    assert_eq!((val >> 4) & 0xFF, 0x2F); // TIMER_BITS=47
    assert_eq!((val >> 12) & 0x7, 7); // VSMAX=7
}

#[test]
fn exception_syscall_sets_pc_to_eentry() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_CRMD, 0x08); // DA=1
    cpu.set_pc(0x4000);
    cpu.csr_write(CSR_EENTRY, 0x9000_0000);

    let vec = unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_raise_exception(
                cpu.env_ptr(), 0x0B, 0,
            )
    };

    assert_eq!(vec, 0x9000_0000);
    assert_eq!(cpu.csr_read(CSR_ERA), 0x4000);
}

#[test]
fn task16_generic_exception_entry_uses_vs_vector_and_captures_badv() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_CRMD, CRMD_PLV_MASK | CRMD_IE | CRMD_DA | CRMD_PG);
    cpu.csr_write(CSR_ECFG, 1 << 16);
    cpu.set_estat_hw(0x120);
    cpu.set_pc(0x8000_0100);
    cpu.csr_write(CSR_EENTRY, 0x9000_0000);

    let vec = unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_raise_exception_with_badv(
                cpu.env_ptr(),
                u64::from(ECODE_ADE),
                u64::from(ESUBCODE_ADEM),
                0xFFFF_FFFF_8000_1234,
            )
    };

    assert_eq!(vec, exception_vector(0x9000_0000, ECODE_ADE, 1));
    assert_eq!(cpu.csr_read(CSR_ERA), 0x8000_0100);
    assert_eq!(cpu.csr_read(CSR_BADV), 0xFFFF_FFFF_8000_1234);
    assert_eq!(cpu.csr_read(CSR_PRMD), 0x07);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PLV_MASK, 0);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_IE, 0);
    assert_ne!(cpu.csr_read(CSR_CRMD) & CRMD_DA, 0);
    assert_ne!(cpu.csr_read(CSR_CRMD) & CRMD_PG, 0);
    assert_eq!(cpu.csr_read(CSR_ESTAT) & ESTAT_IS_MASK, 0x120);
    assert_eq!((cpu.csr_read(CSR_ESTAT) >> 16) & 0x3F, u64::from(ECODE_ADE));
    assert_eq!(
        (cpu.csr_read(CSR_ESTAT) >> 22) & 0x1FF,
        u64::from(ESUBCODE_ADEM)
    );
}

#[test]
fn task16_tlbr_exception_entry_uses_tlbrentry_and_captures_tlbrbadv() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_CRMD, CRMD_PLV_MASK | CRMD_IE | CRMD_PG);
    cpu.set_pc(0x1234_567A);
    cpu.csr_write(CSR_TLBRENTRY, 0x9000_1000);
    cpu.csr_write(CSR_EENTRY, 0x9000_0000);
    cpu.csr_write(CSR_ESTAT, 0x3);

    let vec = unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_raise_exception_with_badv(
                cpu.env_ptr(),
                u64::from(ECODE_TLBR),
                0,
                0xFFFF_FFFF_8123_4000,
            )
    };

    assert_eq!(vec, 0x9000_1000);
    assert_eq!(cpu.csr_read(CSR_TLBRERA) & 1, 1);
    assert_eq!(cpu.csr_read(CSR_TLBRERA) & !0x3, 0x1234_5678);
    assert_eq!(cpu.csr_read(CSR_TLBRBADV), 0xFFFF_FFFF_8123_4000);
    assert_eq!(cpu.csr_read(CSR_TLBRPRMD), 0x07);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PLV_MASK, 0);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_IE, 0);
    assert_ne!(cpu.csr_read(CSR_CRMD) & CRMD_DA, 0);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PG, 0);
    assert_eq!(cpu.csr_read(CSR_ESTAT) & ESTAT_IS_MASK, 0x3);
}

#[test]
fn task16_translated_normal_ertn_restores_prmd_and_returns_era() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_CRMD, CRMD_DA);
    cpu.csr_write(CSR_PRMD, CRMD_PLV_MASK | CRMD_IE);
    cpu.csr_write(CSR_ERA, 0x8000_4000);
    cpu.csr_write(CSR_TLBRERA, 0);

    let _ = run_priv_la(&mut cpu, &[ERTN_INSN]);

    assert_eq!(cpu.pc(), 0x8000_4000);
    assert_eq!(cpu.csr_read(CSR_CRMD) & (CRMD_PLV_MASK | CRMD_IE), 0x07);
    assert_ne!(cpu.csr_read(CSR_CRMD) & CRMD_DA, 0);
}

#[test]
fn task16_translated_tlbr_ertn_restores_tlbrprmd_and_clears_istlbr() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_CRMD, CRMD_DA);
    cpu.csr_write(CSR_TLBRERA, 0x8000_8000 | 1);
    cpu.csr_write(CSR_TLBRPRMD, CRMD_IE);

    let _ = run_priv_la(&mut cpu, &[ERTN_INSN]);

    assert_eq!(cpu.pc(), 0x8000_8000);
    assert_eq!(cpu.csr_read(CSR_TLBRERA) & 1, 0);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_DA, 0);
    assert_ne!(cpu.csr_read(CSR_CRMD) & CRMD_PG, 0);
    assert_eq!(cpu.csr_read(CSR_CRMD) & (CRMD_PLV_MASK | CRMD_IE), CRMD_IE);
}

#[test]
fn task16_translated_ertn_at_plv3_raises_ipe_without_restoring() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_CRMD, CRMD_PLV_MASK | CRMD_IE | CRMD_DA);
    cpu.csr_write(CSR_ECFG, 1 << 16);
    cpu.csr_write(CSR_EENTRY, 0x9000_0000);
    cpu.csr_write(CSR_PRMD, CRMD_IE);
    cpu.csr_write(CSR_ERA, 0x8000_4000);
    cpu.csr_write(CSR_TLBRERA, 0x8000_8000 | 1);

    let _ = run_priv_la(&mut cpu, &[ERTN_INSN]);

    assert_eq!(cpu.pc(), exception_vector(0x9000_0000, ECODE_IPE, 1));
    assert_eq!(cpu.csr_read(CSR_ERA), 0);
    assert_eq!((cpu.csr_read(CSR_ESTAT) >> 16) & 0x3F, u64::from(ECODE_IPE));
    assert_eq!(cpu.csr_read(CSR_TLBRERA), 0x8000_8000 | 1);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PLV_MASK, 0);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_IE, 0);
}

#[test]
fn task17_translated_syscall_uses_sys_ecode_and_zero_esubcode() {
    let mut cpu = LoongArchCpu::new();
    let pc = 0x40;
    cpu.csr_write(CSR_CRMD, CRMD_PLV_MASK | CRMD_IE | CRMD_DA | CRMD_PG);
    cpu.csr_write(CSR_ECFG, 2 << 16);
    cpu.set_estat_hw(0x123);
    cpu.csr_write(CSR_EENTRY, 0x9000_0000);

    let _ = run_priv_la_at(&mut cpu, &[code15_insn(SYSCALL_OP, 0x4321)], pc);

    assert_eq!(cpu.pc(), exception_vector(0x9000_0000, ECODE_SYS, 2));
    assert_eq!(cpu.csr_read(CSR_ERA), pc);
    assert_eq!(cpu.csr_read(CSR_PRMD), 0x07);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PLV_MASK, 0);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_IE, 0);
    assert_eq!(cpu.csr_read(CSR_ESTAT) & ESTAT_IS_MASK, 0x123);
    assert_eq!((cpu.csr_read(CSR_ESTAT) >> 16) & 0x3F, u64::from(ECODE_SYS));
    assert_eq!((cpu.csr_read(CSR_ESTAT) >> 22) & 0x1FF, 0);
}

#[test]
fn task17_translated_break_uses_brk_ecode_and_zero_esubcode() {
    let mut cpu = LoongArchCpu::new();
    let pc = 0x44;
    cpu.csr_write(CSR_CRMD, CRMD_IE | CRMD_DA);
    cpu.csr_write(CSR_ECFG, 1 << 16);
    cpu.set_estat_hw(0x45);
    cpu.csr_write(CSR_EENTRY, 0x9000_0000);

    let _ = run_priv_la_at(&mut cpu, &[code15_insn(BREAK_OP, 0x7FFE)], pc);

    assert_eq!(cpu.pc(), exception_vector(0x9000_0000, ECODE_BRK, 1));
    assert_eq!(cpu.csr_read(CSR_ERA), pc);
    assert_eq!(cpu.csr_read(CSR_PRMD), CRMD_IE);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PLV_MASK, 0);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_IE, 0);
    assert_eq!(cpu.csr_read(CSR_ESTAT) & ESTAT_IS_MASK, 0x45);
    assert_eq!((cpu.csr_read(CSR_ESTAT) >> 16) & 0x3F, u64::from(ECODE_BRK));
    assert_eq!((cpu.csr_read(CSR_ESTAT) >> 22) & 0x1FF, 0);
}

#[test]
fn task17_translated_idle_at_plv0_halts_and_advances_pc() {
    let mut cpu = LoongArchCpu::new();
    let pc = 0x48;
    cpu.csr_write(CSR_CRMD, CRMD_DA);

    let exit = run_priv_la_at(&mut cpu, &[code15_insn(IDLE_OP, 0x1234)], pc);

    assert_eq!(exit, machina_accel::ir::tb::EXCP_LOONGARCH_WFI as usize);
    assert!(cpu.is_halted());
    assert_eq!(cpu.pc(), pc + 4);
}

#[test]
fn task17_translated_idle_at_plv3_raises_ipe_without_halting() {
    let mut cpu = LoongArchCpu::new();
    let pc = 0x4C;
    cpu.csr_write(CSR_CRMD, CRMD_PLV_MASK | CRMD_IE | CRMD_DA);
    cpu.csr_write(CSR_ECFG, 1 << 16);
    cpu.csr_write(CSR_EENTRY, 0x9000_0000);

    let _ = run_priv_la_at(&mut cpu, &[code15_insn(IDLE_OP, 0x5678)], pc);

    assert!(!cpu.is_halted());
    assert_eq!(cpu.pc(), exception_vector(0x9000_0000, ECODE_IPE, 1));
    assert_eq!(cpu.csr_read(CSR_ERA), pc);
    assert_eq!((cpu.csr_read(CSR_ESTAT) >> 16) & 0x3F, u64::from(ECODE_IPE));
    assert_eq!((cpu.csr_read(CSR_ESTAT) >> 22) & 0x1FF, 0);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PLV_MASK, 0);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_IE, 0);
}

#[test]
fn translated_csrrd_under_plv3_sets_noreturn() {
    use machina_accel::ir::Context;
    use machina_guest_loongarch::loongarch::ext::LoongArchCfg;
    use machina_guest_loongarch::loongarch::trans::{
        LoongArchDisasContext, LoongArchTranslator,
    };
    use machina_guest_loongarch::{DisasJumpType, TranslatorOps};

    // CSRRD r1, CRMD(0x0): 00000100 00000000000000 00000 00001
    let insn: u32 = 0x0400_0001;
    let code: [u32; 1] = [insn];
    let guest_base = code.as_ptr().cast::<u8>();
    let mut ctx =
        LoongArchDisasContext::new(0, guest_base, LoongArchCfg::default());
    let mut ir = Context::new();

    LoongArchTranslator::init_disas_context(&mut ctx, &mut ir);
    LoongArchTranslator::insn_start(&mut ctx, &mut ir);
    LoongArchTranslator::translate_insn(&mut ctx, &mut ir);

    assert_eq!(ctx.base.is_jmp, DisasJumpType::Next);
    assert!(ir.num_ops() > 5);
}

#[test]
fn task19_plv0_plv1_plv2_execute_representative_privileged_ops() {
    for plv in 0_u64..=2 {
        let mut cpu = LoongArchCpu::new();
        cpu.csr_write(CSR_CRMD, CRMD_DA | plv);
        cpu.csr_write(CSR_EENTRY, 0x9000_0000);
        cpu.csr_write(CSR_ECFG, 0x1200);
        cpu.write_gpr(5, u64::MAX);

        assert_eq!(run_priv_la(&mut cpu, &[csr_insn(CSR_PRCFG1, 0, 4)]), 0);
        assert_eq!(cpu.read_gpr(4), 0x0000_72F8, "PLV{plv} CSRRD");
        assert_eq!(cpu.pc(), 4, "PLV{plv} CSRRD PC");

        assert_eq!(run_priv_la(&mut cpu, &[csr_insn(CSR_ECFG, 1, 5)]), 0);
        assert_eq!(cpu.read_gpr(5), 0x1200, "PLV{plv} CSRWR old value");
        assert_eq!(cpu.csr_read(CSR_ECFG), ECFG_WRITE_MASK, "PLV{plv} CSRWR");

        cpu.iocsr_write(0x1004, 0x1, 4);
        cpu.iocsr_write(0x1008, 0x1, 4);
        cpu.write_gpr(6, 0x1000);
        assert_eq!(run_priv_la(&mut cpu, &[r2_insn(IOCSRRD_W_OP, 6, 7)]), 0);
        assert_eq!(cpu.read_gpr(7), 1, "PLV{plv} IOCSRRD.W");
    }
}

#[test]
fn task19_plv3_privileged_ops_raise_ipe_without_side_effects() {
    let mut cpu = LoongArchCpu::new();
    let pc = 0x40;
    cpu.csr_write(CSR_CRMD, CRMD_PLV_MASK | CRMD_IE | CRMD_DA);
    cpu.csr_write(CSR_ECFG, 0x1200 | (1 << 16));
    cpu.csr_write(CSR_EENTRY, 0x9000_0000);
    cpu.write_gpr(5, u64::MAX);

    let _ = run_priv_la_at(&mut cpu, &[csr_insn(CSR_ECFG, 1, 5)], pc);

    assert_eq!(cpu.pc(), exception_vector(0x9000_0000, ECODE_IPE, 1));
    assert_eq!(cpu.csr_read(CSR_ERA), pc);
    assert_eq!(cpu.csr_read(CSR_ECFG), 0x1200 | (1 << 16));
    assert_eq!(cpu.read_gpr(5), u64::MAX);
    assert!(!cpu.is_halted());
    assert_eq!((cpu.csr_read(CSR_ESTAT) >> 16) & 0x3F, u64::from(ECODE_IPE));
    assert_eq!(cpu.csr_read(CSR_PRMD), 0x07);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PLV_MASK, 0);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_IE, 0);
}

#[test]
fn task19_exception_entry_saves_all_plv_values_to_prmd() {
    for plv in 0_u64..=3 {
        let mut cpu = LoongArchCpu::new();
        let ie = if plv & 1 == 0 { 0 } else { CRMD_IE };
        let expected_prmd = plv | ie;
        let pc = 0x80 + plv * 4;
        cpu.csr_write(CSR_CRMD, CRMD_DA | expected_prmd);
        cpu.csr_write(CSR_EENTRY, 0x9000_0000);
        cpu.set_pc(pc);

        let _ =
            run_priv_la_at(&mut cpu, &[code15_insn(SYSCALL_OP, 0x1000)], pc);

        assert_eq!(cpu.csr_read(CSR_ERA), pc, "PLV{plv} ERA");
        assert_eq!(cpu.csr_read(CSR_PRMD), expected_prmd, "PLV{plv} PRMD");
        assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PLV_MASK, 0);
        assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_IE, 0);
        assert_eq!(
            (cpu.csr_read(CSR_ESTAT) >> 16) & 0x3F,
            u64::from(ECODE_SYS)
        );
    }
}

#[test]
fn task19_tlbr_entry_saves_plv1_plv2_plv3_to_tlbrprmd() {
    for plv in 1_u64..=3 {
        let mut cpu = LoongArchCpu::new();
        let ie = if plv == 2 { 0 } else { CRMD_IE };
        let expected_prmd = plv | ie;
        cpu.csr_write(CSR_CRMD, CRMD_PG | expected_prmd);
        cpu.csr_write(CSR_TLBRENTRY, 0x9000_1000);
        cpu.set_estat_hw(0x123);
        cpu.set_pc(0x1234_5600 + plv * 4);

        let vec = unsafe {
            machina_guest_loongarch::loongarch::trans::helpers
                ::loongarch_helper_raise_exception_with_badv(
                    cpu.env_ptr(),
                    u64::from(ECODE_TLBR),
                    0,
                    0xFFFF_FFFF_8123_4000,
                )
        };

        assert_eq!(vec, 0x9000_1000);
        assert_eq!(cpu.csr_read(CSR_TLBRPRMD), expected_prmd, "PLV{plv}");
        assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PLV_MASK, 0);
        assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_IE, 0);
        assert_ne!(cpu.csr_read(CSR_CRMD) & CRMD_DA, 0);
        assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PG, 0);
        assert_eq!(cpu.csr_read(CSR_ESTAT) & ESTAT_IS_MASK, 0x123);
    }
}

#[test]
fn task19_ertn_restores_all_plv_values_from_prmd_and_tlbrprmd() {
    for plv in 0_u64..=3 {
        let expected_prmd = plv | if plv & 1 == 0 { 0 } else { CRMD_IE };

        let mut normal = LoongArchCpu::new();
        normal.csr_write(CSR_CRMD, CRMD_DA);
        normal.csr_write(CSR_PRMD, expected_prmd);
        normal.csr_write(CSR_ERA, 0x8000_4000 + plv * 0x10);
        normal.csr_write(CSR_TLBRERA, 0);

        let _ = run_priv_la(&mut normal, &[ERTN_INSN]);

        assert_eq!(
            normal.csr_read(CSR_CRMD) & (CRMD_PLV_MASK | CRMD_IE),
            expected_prmd,
            "normal PLV{plv}"
        );
        assert_eq!(normal.pc(), 0x8000_4000 + plv * 0x10);

        let mut tlbr = LoongArchCpu::new();
        tlbr.csr_write(CSR_CRMD, CRMD_DA);
        tlbr.csr_write(CSR_TLBRPRMD, expected_prmd);
        tlbr.csr_write(CSR_TLBRERA, (0x8000_8000 + plv * 0x10) | 1);

        let _ = run_priv_la(&mut tlbr, &[ERTN_INSN]);

        assert_eq!(
            tlbr.csr_read(CSR_CRMD) & (CRMD_PLV_MASK | CRMD_IE),
            expected_prmd,
            "TLBR PLV{plv}"
        );
        assert_eq!(tlbr.csr_read(CSR_CRMD) & CRMD_DA, 0);
        assert_ne!(tlbr.csr_read(CSR_CRMD) & CRMD_PG, 0);
        assert_eq!(tlbr.csr_read(CSR_TLBRERA) & 1, 0);
    }
}

#[test]
fn task19_csrwr_to_crmd_plv_exits_tb_before_following_privileged_op() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_CRMD, CRMD_DA);
    cpu.csr_write(CSR_EENTRY, 0x9000_0000);
    cpu.csr_write(CSR_ECFG, 1 << 16);
    cpu.write_gpr(5, CRMD_DA | CRMD_PLV_MASK);
    let code = [csr_insn(CSR_CRMD, 1, 5), csr_insn(CSR_PRCFG1, 0, 4)];

    assert_eq!(run_priv_la(&mut cpu, &code), 0);
    assert_eq!(cpu.pc(), 4);
    assert_eq!(cpu.read_gpr(4), 0);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PLV_MASK, CRMD_PLV_MASK);

    let _ = run_priv_la_at(&mut cpu, &[code[1]], 4);

    assert_eq!(cpu.csr_read(CSR_ERA), 4);
    assert_eq!(cpu.pc(), exception_vector(0x9000_0000, ECODE_IPE, 1));
    assert_eq!(cpu.read_gpr(4), 0);
}

#[test]
fn task19_csrxchg_to_crmd_plv_exits_tb_and_allows_plv2_next_tb() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_CRMD, CRMD_DA);
    cpu.write_gpr(5, 2);
    cpu.write_gpr(6, CRMD_PLV_MASK);
    let code = [csr_insn(CSR_CRMD, 6, 5), csr_insn(CSR_PRCFG1, 0, 4)];

    assert_eq!(run_priv_la(&mut cpu, &code), 0);
    assert_eq!(cpu.pc(), 4);
    assert_eq!(cpu.read_gpr(4), 0);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PLV_MASK, 2);

    assert_eq!(run_priv_la_at(&mut cpu, &[code[1]], 4), 0);
    assert_eq!(cpu.read_gpr(4), 0x0000_72F8);
    assert_eq!(cpu.pc(), 8);
}

#[test]
fn task20_plv3_ipe_handler_can_skip_and_ertn_without_side_effect() {
    let mut cpu = LoongArchCpu::new();
    let pc = 0x100;
    cpu.csr_write(CSR_CRMD, CRMD_PLV_MASK | CRMD_IE | CRMD_DA);
    cpu.csr_write(CSR_ECFG, 0x1200 | (1 << 16));
    cpu.csr_write(CSR_EENTRY, 0x9000_0000);
    cpu.write_gpr(5, ECFG_WRITE_MASK);

    let _ = run_priv_la_at(&mut cpu, &[csr_insn(CSR_ECFG, 1, 5)], pc);

    assert_eq!(cpu.pc(), exception_vector(0x9000_0000, ECODE_IPE, 1));
    assert_eq!(cpu.csr_read(CSR_ERA), pc);
    assert_eq!(cpu.csr_read(CSR_ECFG), 0x1200 | (1 << 16));

    cpu.csr_write(CSR_ERA, pc + 4);
    let _ = run_priv_la(&mut cpu, &[ERTN_INSN]);

    assert_eq!(cpu.pc(), pc + 4);
    assert_eq!(cpu.csr_read(CSR_CRMD) & (CRMD_PLV_MASK | CRMD_IE), 0x7);
    assert_eq!(cpu.csr_read(CSR_ECFG), 0x1200 | (1 << 16));
    assert_eq!(cpu.read_gpr(5), ECFG_WRITE_MASK);
}

#[test]
fn task20_timer_interrupt_full_system_entry_and_ertn_restore() {
    let mut cpu = LoongArchCpu::new();
    cpu.set_pc(0x4000);
    cpu.csr_write(CSR_CRMD, 2 | CRMD_IE | CRMD_DA);
    cpu.csr_write(CSR_ECFG, TIMER_INTERRUPT | (1 << 16));
    cpu.csr_write(CSR_EENTRY, 0x9000_0000);
    cpu.csr_write(CSR_TCFG, 0x0101);
    cpu.timer_tick(0x100);
    let mut sys = full_system_cpu(cpu);

    assert!(sys.pending_interrupt());
    sys.handle_interrupt();

    assert_eq!(sys.get_pc(), interrupt_vector(0x9000_0000, 11, 1));
    assert_eq!(sys.cpu.csr_read(CSR_ERA), 0x4000);
    assert_eq!(
        sys.cpu.csr_read(CSR_ESTAT) & TIMER_INTERRUPT,
        TIMER_INTERRUPT
    );
    assert_eq!(sys.cpu.csr_read(CSR_PRMD), 2 | CRMD_IE);
    assert_eq!(sys.cpu.csr_read(CSR_CRMD) & CRMD_PLV_MASK, 0);
    assert_eq!(sys.cpu.csr_read(CSR_CRMD) & CRMD_IE, 0);

    let _ = run_priv_la(&mut sys.cpu, &[ERTN_INSN]);

    assert_eq!(sys.cpu.pc(), 0x4000);
    assert_eq!(
        sys.cpu.csr_read(CSR_CRMD) & (CRMD_PLV_MASK | CRMD_IE),
        2 | CRMD_IE
    );
}

#[test]
fn task20_ticlr_after_interrupt_preserves_other_pending_and_ertn_restores() {
    let mut cpu = LoongArchCpu::new();
    cpu.set_pc(0x4040);
    cpu.csr_write(CSR_CRMD, 1 | CRMD_IE | CRMD_DA);
    cpu.csr_write(CSR_ECFG, TIMER_INTERRUPT);
    cpu.csr_write(CSR_EENTRY, 0x9000_0000);
    cpu.set_estat_hw(TIMER_INTERRUPT | 0x3 | (1 << 12));
    let mut sys = full_system_cpu(cpu);

    sys.handle_interrupt();
    assert_eq!(
        sys.cpu.csr_read(CSR_ESTAT) & TIMER_INTERRUPT,
        TIMER_INTERRUPT
    );

    sys.cpu.write_gpr(5, 1);
    let _ = run_priv_la(&mut sys.cpu, &[csr_insn(CSR_TICLR, 1, 5)]);

    assert_eq!(sys.cpu.csr_read(CSR_ESTAT) & TIMER_INTERRUPT, 0);
    assert_eq!(
        sys.cpu.csr_read(CSR_ESTAT) & (0x3 | (1 << 12)),
        0x3 | (1 << 12)
    );
    let _ = run_priv_la(&mut sys.cpu, &[ERTN_INSN]);

    assert_eq!(sys.cpu.pc(), 0x4040);
    assert_eq!(
        sys.cpu.csr_read(CSR_CRMD) & (CRMD_PLV_MASK | CRMD_IE),
        1 | CRMD_IE
    );
}

#[test]
fn task20_csr_masks_survive_exception_entry_and_return() {
    let mut cpu = LoongArchCpu::new();
    let pc = 0x108;
    cpu.csr_write(CSR_CRMD, CRMD_PLV_MASK | CRMD_IE | CRMD_DA);
    cpu.csr_write(CSR_ECFG, 1 << 16);
    cpu.csr_write(CSR_EENTRY, 0x9000_0000);
    cpu.csr_write(CSR_TCFG, 0x0101);
    let old_tval = cpu.csr_read(CSR_TVAL);

    let _ = run_priv_la_at(&mut cpu, &[code15_insn(SYSCALL_OP, 0x55)], pc);

    cpu.csr_write(CSR_ERA, pc + 4);
    cpu.csr_write(CSR_BADI, 0xDEAD_BEEF);
    cpu.csr_write(CSR_TVAL, 0);
    cpu.write_gpr(5, u64::MAX);
    cpu.write_gpr(6, 0x1001);
    let _ = run_priv_la(&mut cpu, &[csr_insn(CSR_ECFG, 6, 5)]);
    let _ = run_priv_la(&mut cpu, &[ERTN_INSN]);

    assert_eq!(cpu.pc(), pc + 4);
    assert_eq!(cpu.csr_read(CSR_BADI), 0);
    assert_eq!(cpu.csr_read(CSR_TVAL), old_tval);
    assert_eq!(cpu.csr_read(CSR_ECFG), (1 << 16) | 0x1001);
    assert_eq!(cpu.csr_read(CSR_CRMD) & (CRMD_PLV_MASK | CRMD_IE), 0x7);
}

#[test]
fn task20_tlbr_entry_preserves_pending_bits_and_ertn_restores_pg_mode() {
    let mut cpu = LoongArchCpu::new();
    let pending = TIMER_INTERRUPT | (1 << 12) | 0x2;
    cpu.set_pc(0x1234_567A);
    cpu.csr_write(CSR_CRMD, CRMD_PLV_MASK | CRMD_IE | CRMD_PG);
    cpu.csr_write(CSR_TLBRENTRY, 0x9000_1000);
    cpu.set_estat_hw(pending);

    let vec = unsafe {
        machina_guest_loongarch::loongarch::trans::helpers
            ::loongarch_helper_raise_exception_with_badv(
                cpu.env_ptr(),
                u64::from(ECODE_TLBR),
                0,
                0xFFFF_FFFF_8123_4000,
            )
    };

    assert_eq!(vec, 0x9000_1000);
    assert_eq!(cpu.csr_read(CSR_TLBRERA) & 1, 1);
    assert_eq!(cpu.csr_read(CSR_TLBRERA) & !0x3, 0x1234_5678);
    assert_eq!(cpu.csr_read(CSR_TLBRBADV), 0xFFFF_FFFF_8123_4000);
    assert_eq!(cpu.csr_read(CSR_TLBRPRMD), CRMD_PLV_MASK | CRMD_IE);
    assert_eq!(cpu.csr_read(CSR_ESTAT) & ESTAT_IS_MASK, pending);
    assert_ne!(cpu.csr_read(CSR_CRMD) & CRMD_DA, 0);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PG, 0);

    let _ = run_priv_la(&mut cpu, &[ERTN_INSN]);

    assert_eq!(cpu.csr_read(CSR_TLBRERA) & 1, 0);
    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_DA, 0);
    assert_ne!(cpu.csr_read(CSR_CRMD) & CRMD_PG, 0);
    assert_eq!(cpu.csr_read(CSR_ESTAT) & ESTAT_IS_MASK, pending);
    assert_eq!(cpu.csr_read(CSR_CRMD) & (CRMD_PLV_MASK | CRMD_IE), 0x7);
}

#[test]
fn task20_translated_csr_flag_changes_end_tb_and_update_full_system_flags() {
    let mut sys = full_system_cpu(LoongArchCpu::new());
    sys.cpu.csr_write(CSR_CRMD, CRMD_DA);
    sys.cpu.write_gpr(5, 2 | CRMD_PG);
    let crmd_code = [csr_insn(CSR_CRMD, 1, 5), csr_insn(CSR_PRCFG1, 0, 4)];

    assert_eq!(run_priv_la(&mut sys.cpu, &crmd_code), 0);
    assert_eq!(sys.cpu.pc(), 4);
    assert_eq!(sys.cpu.read_gpr(4), 0);
    assert_eq!(sys.get_flags(), 2 | LOONGARCH_TB_FLAG_PG);

    sys.cpu.write_gpr(6, EUEN_FPE);
    let euen_code = [csr_insn(CSR_EUEN, 1, 6), csr_insn(CSR_PRCFG1, 0, 4)];
    assert_eq!(run_priv_la(&mut sys.cpu, &euen_code), 0);
    assert_eq!(sys.cpu.pc(), 4);
    assert_eq!(
        sys.get_flags(),
        2 | LOONGARCH_TB_FLAG_PG | LOONGARCH_TB_FLAG_FPE
    );

    sys.cpu.write_gpr(7, CRMD_PLV_MASK | CRMD_DA);
    sys.cpu.write_gpr(8, CRMD_PLV_MASK | CRMD_DA | CRMD_PG);
    let xchg_code = [csr_insn(CSR_CRMD, 8, 7), csr_insn(CSR_PRCFG1, 0, 4)];
    assert_eq!(run_priv_la(&mut sys.cpu, &xchg_code), 0);
    assert_eq!(sys.cpu.pc(), 4);
    assert_eq!(
        sys.get_flags(),
        LOONGARCH_TB_FLAG_PLV_MASK
            | LOONGARCH_TB_FLAG_DA
            | LOONGARCH_TB_FLAG_FPE
    );
}

#[test]
fn task21_dmw_match_all_plvs_and_ignores_la64_pseg_bits() {
    let va = 0x9000_1234_5678_9ABC;
    let expected_pa = va & TARGET_VIRT_MASK;

    for plv in 0..=3_u64 {
        let mut cpu = LoongArchCpu::new();
        cpu.csr_write(CSR_CRMD, plv | CRMD_PG);
        cpu.csr_write(CSR_DMW0, dmw64(0x9, 0x7, 1 << plv));

        assert_eq!(cpu.csr_read(CSR_DMW0) & (1 << plv), 1 << plv);
        assert_eq!(mmu::dmw_match(&cpu, va), Some(expected_pa));
    }
}

#[test]
fn task21_dmw_mismatch_requires_plv_and_vseg_match() {
    let va = 0x9000_0000_1234_5678;
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_CRMD, 2 | CRMD_PG);
    cpu.csr_write(CSR_DMW0, dmw64(0x9, 0, 1 << 1));
    assert_eq!(mmu::dmw_match(&cpu, va), None);

    cpu.csr_write(CSR_CRMD, 1 | CRMD_PG);
    cpu.csr_write(CSR_DMW0, dmw64(0x8, 0, 1 << 1));
    assert_eq!(mmu::dmw_match(&cpu, va), None);
}

#[test]
fn task21_all_four_dmw_windows_can_match() {
    let cases = [
        (CSR_DMW0, 0x8_u64, 0x8000_0000_0000_1000_u64),
        (CSR_DMW1, 0x9_u64, 0x9000_0000_0000_2000_u64),
        (CSR_DMW2, 0xA_u64, 0xA000_0000_0000_3000_u64),
        (CSR_DMW3, 0xF_u64, 0xFFFF_0000_0000_4000_u64),
    ];

    for (csr, vseg, va) in cases {
        let mut cpu = LoongArchCpu::new();
        cpu.csr_write(CSR_CRMD, CRMD_PG);
        cpu.csr_write(csr, dmw64(vseg, 0, 1));

        assert_eq!(mmu::dmw_match(&cpu, va), Some(va & TARGET_VIRT_MASK));
    }
}

#[test]
fn task21_full_system_translate_pc_uses_da_then_dmw() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_CRMD, CRMD_DA);
    let sys = full_system_cpu(cpu);
    let canonical_high = 0xFFFF_8000_0000_1234;
    assert_eq!(
        sys.translate_pc(canonical_high),
        canonical_high & TARGET_VIRT_MASK
    );

    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_CRMD, 3 | CRMD_PG);
    cpu.csr_write(CSR_DMW0, dmw64(0xF, 0x6, 1 << 3));
    let sys = full_system_cpu(cpu);
    let dmw_va = 0xFFFF_0000_0000_4568;
    assert_eq!(sys.translate_pc(dmw_va), dmw_va & TARGET_VIRT_MASK);
}

#[test]
fn task21_dmw_csr_writes_request_tb_flush_on_effective_change() {
    let dmw_csrs = [CSR_DMW0, CSR_DMW1, CSR_DMW2, CSR_DMW3];

    for (idx, csr) in dmw_csrs.into_iter().enumerate() {
        let mut cpu = LoongArchCpu::new();
        let val = dmw64(0x8 + idx as u64, idx as u64, 1 << (idx % 4));

        cpu.csr_write(csr, val);
        assert!(cpu.take_tb_flush(), "DMW{idx} write did not request flush");
        assert!(!cpu.take_tb_flush(), "DMW{idx} flush flag was not consumed");

        cpu.csr_write(csr, val | !DMW_WRITE_MASK);
        assert!(
            !cpu.take_tb_flush(),
            "DMW{idx} no-op masked write requested another flush"
        );
    }
}

#[test]
fn task21_dmw_csrxchg_and_translated_writes_request_tb_flush() {
    let mut cpu = LoongArchCpu::new();
    let initial = dmw64(0x9, 0, 1);
    let changed = dmw64(0xA, 0, 1 << 2);

    cpu.csr_write(CSR_DMW1, initial);
    assert!(cpu.take_tb_flush());

    assert_eq!(cpu.csr_xchg(CSR_DMW1, changed, DMW_WRITE_MASK), initial);
    assert_eq!(cpu.csr_read(CSR_DMW1), changed);
    assert!(cpu.take_tb_flush());
    assert!(!cpu.take_tb_flush());

    assert_eq!(cpu.csr_xchg(CSR_DMW1, changed, DMW_WRITE_MASK), changed);
    assert!(!cpu.take_tb_flush());

    cpu.write_gpr(5, dmw64(0xB, 0, 1 << 3));
    assert_eq!(run_priv_la(&mut cpu, &[csr_insn(CSR_DMW2, 1, 5)]), 0);
    assert_eq!(cpu.read_gpr(5), 0);
    assert!(cpu.take_tb_flush());

    cpu.write_gpr(6, dmw64(0xC, 0, 1));
    cpu.write_gpr(7, DMW_WRITE_MASK);
    assert_eq!(run_priv_la(&mut cpu, &[csr_insn(CSR_DMW3, 7, 6)]), 0);
    assert_eq!(cpu.read_gpr(6), 0);
    assert!(cpu.take_tb_flush());
}

#[test]
fn task21_exec_loop_flushes_stale_dmw_tb_after_translated_csrwr() {
    let code = [csr_insn(CSR_DMW0, 1, 0), code15_insn(IDLE_OP, 0)];
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_DMW0, dmw64(0, 0, 1));
    cpu.csr_write(CSR_TLBRENTRY, 4);
    assert!(cpu.take_tb_flush());

    let mut sys = full_system_cpu_with_code(cpu, &code);
    let mut env = ExecEnv::new(X86_64CodeGen::new());

    sys.cpu.set_pc(4);
    let warm = unsafe { cpu_exec_loop_env(&mut env, &mut sys) };
    assert_eq!(warm, ExitReason::Halted);

    sys.cpu.set_pc(0);
    let after_dmw_disable = unsafe { cpu_exec_loop_env(&mut env, &mut sys) };

    assert_eq!(sys.cpu.csr_read(CSR_DMW0), 0);
    assert_eq!(after_dmw_disable, ExitReason::Halted);
    assert_eq!(sys.cpu.pc(), 8);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRERA) & 1, 1);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRERA) & !0x3, 4);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRBADV), 4);
    assert_ne!(sys.cpu.csr_read(CSR_CRMD) & CRMD_DA, 0);
    assert_eq!(sys.cpu.csr_read(CSR_CRMD) & CRMD_PG, 0);
}

#[test]
fn task22_tlb_reset_state_has_empty_mtlb_and_stlb() {
    let cpu = LoongArchCpu::new();

    assert_eq!(mmu::MTLB_SIZE, 64);
    assert_eq!(mmu::STLB_SETS, 256);
    assert_eq!(mmu::STLB_WAYS, 8);
    assert_eq!(mmu::STLB_SIZE, 2048);
    assert_eq!(mmu::TLB_TOTAL_SIZE, 2112);

    assert!(cpu
        .mmu()
        .mtlb
        .iter()
        .all(|entry| { *entry == mmu::TlbEntry::default() }));
    assert!(cpu
        .mmu()
        .stlb
        .iter()
        .flatten()
        .all(|entry| { *entry == mmu::TlbEntry::default() }));
}

#[test]
fn task22_tlb_index_helpers_match_qemu_stlb_mtlb_layout() {
    let va = 0xFFFF_0000_1234_5678;
    let stlb_ps = 14;
    let set = (((va & TARGET_VIRT_MASK) >> (stlb_ps + 1)) & 0xFF) as usize;
    let way = 6;
    let stlb_idx = way * mmu::STLB_SETS + set;
    let mtlb_idx = mmu::STLB_SIZE + 17;

    assert_eq!(mmu::stlb_set_index(va, stlb_ps), Some(set));
    assert_eq!(mmu::stlb_set_index(va, 0), None);
    assert_eq!(mmu::stlb_flat_index(set, way), Some(stlb_idx));
    assert_eq!(
        mmu::decode_tlb_index(stlb_idx),
        Some(mmu::TlbSlot::Stlb { set, way })
    );
    assert_eq!(mmu::mtlb_flat_index(17), Some(mtlb_idx));
    assert_eq!(
        mmu::decode_tlb_index(mtlb_idx),
        Some(mmu::TlbSlot::Mtlb { index: 17 })
    );
    assert_eq!(mmu::stlb_flat_index(mmu::STLB_SETS, 0), None);
    assert_eq!(mmu::stlb_flat_index(0, mmu::STLB_WAYS), None);
    assert_eq!(mmu::mtlb_flat_index(mmu::MTLB_SIZE), None);
    assert_eq!(mmu::decode_tlb_index(mmu::TLB_TOTAL_SIZE), None);
}

#[test]
fn task22_mtlb_indexed_write_read_preserves_page_pair_fields() {
    let mut cpu = LoongArchCpu::new();
    let idx = mmu::mtlb_flat_index(5).unwrap();
    let tlbehi = 0x0000_1234_5678_0000;
    let elo0 = tlbelo_test(0x12345, true, true, 2, 1, true, false);
    let elo1 = tlbelo_test(0x22345, true, false, 3, 2, true, true);

    cpu.csr_write(CSR_TLBEHI, tlbehi);
    cpu.csr_write(CSR_TLBELO0, elo0);
    cpu.csr_write(CSR_TLBELO1, elo1);
    cpu.csr_write(CSR_ASID, 0x3FF);
    cpu.csr_write(CSR_TLBIDX, (21 << 24) | idx as u64);
    cpu.tlb_write(idx);

    let entry = cpu.mmu().mtlb[5];
    assert_eq!(entry.vppn, tlbehi >> 13);
    assert_eq!(entry.page_size, 21);
    assert_eq!(entry.asid, 0x3FF);
    assert!(entry.g);
    assert!(entry.valid);
    assert_eq!(entry.ppn0, 0x12345);
    assert_eq!(entry.ppn1, 0x22345);
    assert_eq!(entry.plv0, 2);
    assert_eq!(entry.plv1, 3);
    assert_eq!(entry.mat0, 1);
    assert_eq!(entry.mat1, 2);
    assert!(entry.d0);
    assert!(!entry.d1);
    assert!(entry.v0);
    assert!(entry.v1);
    assert!(!entry.nr0);
    assert!(entry.nr1);

    cpu.csr_write(CSR_TLBEHI, 0);
    cpu.csr_write(CSR_TLBELO0, 0);
    cpu.csr_write(CSR_TLBELO1, 0);
    cpu.csr_write(CSR_ASID, 0);
    cpu.csr_write(CSR_TLBIDX, (1 << 31) | idx as u64);
    cpu.tlb_read(idx);

    assert_eq!(cpu.csr_read(CSR_TLBEHI), tlbehi);
    assert_eq!(cpu.csr_read(CSR_TLBELO0), elo0);
    assert_eq!(cpu.csr_read(CSR_TLBELO1), elo1);
    assert_eq!(cpu.csr_read(CSR_ASID) & 0x3FF, 0x3FF);
    assert_eq!(cpu.csr_read(CSR_TLBIDX) & 0xFFF, idx as u64);
    assert_eq!((cpu.csr_read(CSR_TLBIDX) >> 24) & 0x3F, 21);
    assert_eq!(cpu.csr_read(CSR_TLBIDX) & (1 << 31), 0);
}

#[test]
fn task22_stlb_set_way_write_preserves_metadata() {
    let mut cpu = LoongArchCpu::new();
    let stlb_ps = 14;
    let va = 0x0000_0000_4567_8000;
    let set = mmu::stlb_set_index(va, stlb_ps).unwrap();
    let way = 3;
    let idx = mmu::stlb_flat_index(set, way).unwrap();
    let tlbehi = va & !0x1FFF;
    let elo0 = tlbelo_test(0x34567, true, true, 1, 0, false, false);
    let elo1 = tlbelo_test(0x44567, false, false, 0, 3, false, true);

    cpu.csr_write(CSR_STLBPS, stlb_ps as u64);
    cpu.csr_write(CSR_TLBEHI, tlbehi);
    cpu.csr_write(CSR_TLBELO0, elo0);
    cpu.csr_write(CSR_TLBELO1, elo1);
    cpu.csr_write(CSR_ASID, 0x155);
    cpu.csr_write(CSR_TLBIDX, (stlb_ps as u64) << 24 | idx as u64);
    cpu.tlb_write(idx);

    for (candidate_way, entry) in cpu.mmu().stlb[set].iter().enumerate() {
        if candidate_way == way {
            assert!(entry.valid);
            assert_eq!(entry.vppn, tlbehi >> 13);
            assert_eq!(entry.page_size, stlb_ps);
            assert_eq!(entry.asid, 0x155);
            assert!(!entry.g);
            assert_eq!(entry.ppn0, 0x34567);
            assert_eq!(entry.ppn1, 0x44567);
            assert_eq!(entry.plv0, 1);
            assert_eq!(entry.plv1, 0);
            assert_eq!(entry.mat0, 0);
            assert_eq!(entry.mat1, 3);
            assert!(entry.d0);
            assert!(!entry.d1);
            assert!(entry.v0);
            assert!(!entry.v1);
            assert!(!entry.nr0);
            assert!(entry.nr1);
        } else {
            assert_eq!(*entry, mmu::TlbEntry::default());
        }
    }
}

#[test]
fn task22_invalid_tlb_indices_do_not_mutate_entries() {
    let mut cpu = LoongArchCpu::new();
    let idx = mmu::mtlb_flat_index(0).unwrap();
    cpu.csr_write(CSR_TLBEHI, 0x0000_2222_0000_0000);
    cpu.csr_write(
        CSR_TLBELO0,
        tlbelo_test(0x11111, true, true, 0, 1, true, false),
    );
    cpu.csr_write(
        CSR_TLBELO1,
        tlbelo_test(0x22222, true, true, 0, 1, true, false),
    );
    cpu.csr_write(CSR_ASID, 7);
    cpu.csr_write(CSR_TLBIDX, (12 << 24) | idx as u64);
    cpu.tlb_write(idx);
    let before = cpu.mmu().mtlb[0];

    cpu.csr_write(CSR_TLBEHI, 0xFFFF_3333_0000_0000);
    cpu.csr_write(
        CSR_TLBELO0,
        tlbelo_test(0xAAAAA, true, true, 0, 1, true, false),
    );
    cpu.tlb_write(mmu::TLB_TOTAL_SIZE);

    assert_eq!(cpu.mmu().mtlb[0], before);
    cpu.tlb_read(mmu::TLB_TOTAL_SIZE);
    assert_ne!(cpu.csr_read(CSR_TLBIDX) & (1 << 31), 0);
}

#[test]
fn task22_dmw_direct_mapping_still_has_priority_over_tlb_structures() {
    let mut cpu = LoongArchCpu::new();
    let va = 0x9000_0000_1234_8000;
    let stlb_ps = 14;
    let set = mmu::stlb_set_index(va, stlb_ps).unwrap();
    let idx = mmu::stlb_flat_index(set, 0).unwrap();

    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_DMW0, dmw64(0x9, 0, 1));
    assert!(cpu.take_tb_flush());
    cpu.csr_write(CSR_STLBPS, stlb_ps as u64);
    cpu.csr_write(CSR_TLBEHI, va & !0x1FFF);
    cpu.csr_write(
        CSR_TLBELO0,
        tlbelo_test(0x77777, true, true, 0, 1, true, false),
    );
    cpu.csr_write(
        CSR_TLBELO1,
        tlbelo_test(0x88888, true, true, 0, 1, true, false),
    );
    cpu.csr_write(CSR_ASID, 1);
    cpu.csr_write(CSR_TLBIDX, (stlb_ps as u64) << 24 | idx as u64);
    cpu.tlb_write(idx);

    assert_eq!(
        mmu::direct_map_address(&cpu, va),
        Some(va & TARGET_VIRT_MASK)
    );
}

#[test]
fn task23_direct_map_bypasses_architectural_tlb_lookup() {
    let mut cpu = LoongArchCpu::new();
    let va = 0xFFFF_8000_0000_1234;
    let idx = mmu::mtlb_flat_index(1).unwrap();
    write_test_tlb_entry(
        &mut cpu,
        idx,
        va,
        14,
        0,
        tlbelo_test(0x11111, true, true, 0, 1, true, false),
        tlbelo_test(0x22222, true, true, 0, 1, true, false),
    );
    cpu.csr_write(CSR_CRMD, CRMD_DA);

    assert_tlb_hit(
        cpu.translate_address(va, mmu::AccessType::Fetch),
        va & TARGET_VIRT_MASK,
        0,
    );

    let mut cpu = LoongArchCpu::new();
    let dmw_va = 0x9000_0000_0000_4568;
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_DMW0, dmw64(0x9, 0, 1));
    assert!(cpu.take_tb_flush());
    write_test_tlb_entry(
        &mut cpu,
        idx,
        dmw_va,
        14,
        0,
        tlbelo_test(0x33333, true, true, 0, 1, true, false),
        tlbelo_test(0x44444, true, true, 0, 1, true, false),
    );

    assert_tlb_hit(
        cpu.translate_address(dmw_va, mmu::AccessType::Load),
        dmw_va & TARGET_VIRT_MASK,
        0,
    );
}

#[test]
fn task23_mtlb_lookup_hits_even_and_odd_page_pairs() {
    let mut cpu = LoongArchCpu::new();
    let ps = 14;
    let even_va = 0x0000_0000_1234_8120;
    let odd_va = tlb_pair_base(even_va, ps) + (1 << ps) + 0x330;
    let idx = mmu::mtlb_flat_index(4).unwrap();
    let ppn0 = 0x41000;
    let ppn1 = 0x52000;
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_STLBPS, ps as u64);
    cpu.csr_write(CSR_ASID, 0x17);
    write_test_tlb_entry(
        &mut cpu,
        idx,
        even_va,
        ps,
        0x17,
        tlbelo_test(ppn0, true, true, 0, 1, false, false),
        tlbelo_test(ppn1, true, true, 0, 2, false, false),
    );

    assert_tlb_hit(
        cpu.translate_address(even_va, mmu::AccessType::Load),
        (ppn0 << 12) | (even_va & ((1 << ps) - 1)),
        1,
    );
    assert_tlb_hit(
        cpu.translate_address(odd_va, mmu::AccessType::Load),
        (ppn1 << 12) | (odd_va & ((1 << ps) - 1)),
        2,
    );
}

#[test]
fn task23_stlb_lookup_hits_selected_set_way() {
    let mut cpu = LoongArchCpu::new();
    let ps = 14;
    let va = 0x0000_0000_4567_9120;
    let set = mmu::stlb_set_index(va, ps).unwrap();
    let idx = mmu::stlb_flat_index(set, 5).unwrap();
    let ppn = 0x65000;
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_STLBPS, ps as u64);
    cpu.csr_write(CSR_ASID, 0x33);
    write_test_tlb_entry(
        &mut cpu,
        idx,
        va,
        ps,
        0x33,
        tlbelo_test(ppn, true, true, 0, 3, false, false),
        tlbelo_test(0x66000, true, true, 0, 0, false, false),
    );

    assert_tlb_hit(
        cpu.translate_address(va, mmu::AccessType::Fetch),
        (ppn << 12) | (va & ((1 << ps) - 1)),
        3,
    );
}

#[test]
fn task23_tlb_lookup_respects_asid_global_and_fault_classifications() {
    let ps = 14;
    let va = 0x0000_0000_7777_0120;
    let idx = mmu::mtlb_flat_index(7).unwrap();

    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_ASID, 0x44);
    write_test_tlb_entry(
        &mut cpu,
        idx,
        va,
        ps,
        0x45,
        tlbelo_test(0x70000, true, true, 0, 0, false, false),
        tlbelo_test(0x71000, true, true, 0, 0, false, false),
    );
    cpu.csr_write(CSR_ASID, 0x44);
    assert_eq!(
        cpu.translate_address(va, mmu::AccessType::Load),
        mmu::TlbLookupResult::Miss
    );

    write_test_tlb_entry(
        &mut cpu,
        idx,
        va,
        ps,
        0x45,
        tlbelo_test(0x72000, true, true, 0, 1, true, false),
        tlbelo_test(0x73000, true, true, 0, 1, true, false),
    );
    cpu.csr_write(CSR_ASID, 0x44);
    assert_tlb_hit(
        cpu.translate_address(va, mmu::AccessType::Load),
        (0x72000 << 12) | (va & ((1 << ps) - 1)),
        1,
    );

    write_test_tlb_entry(
        &mut cpu,
        idx,
        va,
        ps,
        0x44,
        tlbelo_test(0x74000, false, true, 0, 0, true, false),
        tlbelo_test(0x75000, true, true, 0, 0, true, false),
    );
    assert_eq!(
        cpu.translate_address(va, mmu::AccessType::Load),
        mmu::TlbLookupResult::Invalid
    );

    write_test_tlb_entry(
        &mut cpu,
        idx,
        va,
        ps,
        0x44,
        tlbelo_test(0x76000, true, false, 0, 0, true, false),
        tlbelo_test(0x77000, true, true, 0, 0, true, false),
    );
    assert_eq!(
        cpu.translate_address(va, mmu::AccessType::Store),
        mmu::TlbLookupResult::Dirty
    );

    write_test_tlb_entry(
        &mut cpu,
        idx,
        va,
        ps,
        0x44,
        tlbelo_test(0x78000, true, true, 0, 0, true, false),
        tlbelo_test(0x79000, true, true, 0, 0, true, false),
    );
    cpu.csr_write(CSR_CRMD, CRMD_PG | 3);
    assert_eq!(
        cpu.translate_address(va, mmu::AccessType::Load),
        mmu::TlbLookupResult::PrivViolation
    );

    cpu.csr_write(CSR_CRMD, CRMD_PG);
    write_test_tlb_entry(
        &mut cpu,
        idx,
        va,
        ps,
        0x44,
        tlbelo_test_with_perm(0x7A000, true, true, 0, 0, true, true, false),
        tlbelo_test(0x7B000, true, true, 0, 0, true, false),
    );
    assert_eq!(
        cpu.translate_address(va, mmu::AccessType::Load),
        mmu::TlbLookupResult::ReadProtect
    );
    assert_tlb_hit(
        cpu.translate_address(va, mmu::AccessType::Fetch),
        (0x7A000 << 12) | (va & ((1 << ps) - 1)),
        0,
    );

    write_test_tlb_entry(
        &mut cpu,
        idx,
        va,
        ps,
        0x44,
        tlbelo_test_with_perm(0x7C000, true, true, 0, 0, true, false, true),
        tlbelo_test(0x7D000, true, true, 0, 0, true, false),
    );
    assert_eq!(
        cpu.translate_address(va, mmu::AccessType::Fetch),
        mmu::TlbLookupResult::ExecProtect
    );
    assert_tlb_hit(
        cpu.translate_address(va, mmu::AccessType::Load),
        (0x7C000 << 12) | (va & ((1 << ps) - 1)),
        0,
    );
}

#[test]
fn task23_tlb_read_write_preserves_nr_and_nx_permission_bits() {
    let mut cpu = LoongArchCpu::new();
    let ps = 14;
    let va = 0x0000_0000_7778_0120;
    let idx = mmu::mtlb_flat_index(10).unwrap();
    let elo0 =
        tlbelo_test_with_perm(0x7E000, true, true, 0, 1, true, true, false);
    let elo1 =
        tlbelo_test_with_perm(0x7F000, true, true, 0, 2, true, false, true);

    write_test_tlb_entry(&mut cpu, idx, va, ps, 0x44, elo0, elo1);
    cpu.csr_write(CSR_TLBEHI, 0);
    cpu.csr_write(CSR_TLBELO0, 0);
    cpu.csr_write(CSR_TLBELO1, 0);
    cpu.tlb_read(idx);

    assert_eq!(cpu.csr_read(CSR_TLBELO0), elo0);
    assert_eq!(cpu.csr_read(CSR_TLBELO1), elo1);
}

#[test]
fn task23_mtlb_has_priority_over_stlb_when_both_match() {
    let mut cpu = LoongArchCpu::new();
    let ps = 14;
    let va = 0x0000_0000_2468_0120;
    let set = mmu::stlb_set_index(va, ps).unwrap();
    let stlb_idx = mmu::stlb_flat_index(set, 1).unwrap();
    let mtlb_idx = mmu::mtlb_flat_index(2).unwrap();
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_STLBPS, ps as u64);
    cpu.csr_write(CSR_ASID, 0x12);

    write_test_tlb_entry(
        &mut cpu,
        stlb_idx,
        va,
        ps,
        0x12,
        tlbelo_test(0x81000, true, true, 0, 1, true, false),
        tlbelo_test(0x82000, true, true, 0, 1, true, false),
    );
    write_test_tlb_entry(
        &mut cpu,
        mtlb_idx,
        va,
        ps,
        0x12,
        tlbelo_test(0x91000, true, true, 0, 2, true, false),
        tlbelo_test(0x92000, true, true, 0, 2, true, false),
    );

    assert_tlb_hit(
        cpu.translate_address(va, mmu::AccessType::Load),
        (0x91000 << 12) | (va & ((1 << ps) - 1)),
        2,
    );
}

#[test]
fn task23_full_system_translate_pc_uses_tlb_fetch_hit_and_faults_to_max() {
    let mut cpu = LoongArchCpu::new();
    let ps = 14;
    let va = 0x0000_0000_1357_0120;
    let ppn = 0xA1000;
    let idx = mmu::mtlb_flat_index(3).unwrap();
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_STLBPS, ps as u64);
    cpu.csr_write(CSR_ASID, 0x22);
    write_test_tlb_entry(
        &mut cpu,
        idx,
        va,
        ps,
        0x22,
        tlbelo_test(ppn, true, true, 0, 0, false, false),
        tlbelo_test(0xA2000, true, true, 0, 0, false, false),
    );

    let sys = full_system_cpu(cpu);
    assert_eq!(sys.translate_pc(va), (ppn << 12) | (va & ((1 << ps) - 1)));
    assert_eq!(sys.translate_pc(va + (1 << 22)), u64::MAX);
}

#[test]
fn task23_fast_tlb_layout_matches_x86_emitter_contract() {
    assert_eq!(mmu::FAST_TLB_SIZE, 256);
    assert_eq!(mmu::fast_tlb_offsets::ADDR_READ, 0);
    assert_eq!(mmu::fast_tlb_offsets::ADDR_WRITE, 8);
    assert_eq!(mmu::fast_tlb_offsets::ADDR_CODE, 16);
    assert_eq!(mmu::fast_tlb_offsets::ADDEND, 24);
    assert_eq!(mmu::fast_tlb_offsets::DIRTY, 32);
    assert!(mmu::fast_tlb_offsets::ENTRY_SIZE >= 40);
}

#[test]
fn task96_fast_tlb_mmio_fill_keeps_tags_invalid_for_slow_path() {
    let mut tlb = mmu::LoongArchMmu::new();
    let va = 0x8000_0000_1008_0005;
    let pa = 0x1008_0005;
    let idx = mmu::fast_tlb_index(va);

    tlb.fill_fast_tlb(va, mmu::AccessType::Load, pa, mmu::FAST_TLB_MMIO_ADDEND);

    assert_eq!(tlb.fast_tlb[idx].addr_read, mmu::FAST_TLB_INVALID_TAG);
    assert_eq!(tlb.fast_tlb[idx].addend, mmu::FAST_TLB_MMIO_ADDEND);
}

#[test]
fn task23_fast_tlb_cache_fills_and_invalidates_on_asid_change() {
    let mut cpu = LoongArchCpu::new();
    let ps = 14;
    let va = 0x0000_0000_5555_0120;
    let ppn = 0x40020;
    let idx = mmu::mtlb_flat_index(8).unwrap();
    cpu.set_guest_base(0x1_0000_0000);
    cpu.set_ram_base(0x4000_0000);
    cpu.set_ram_end(0x6000_0000);
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_STLBPS, ps as u64);
    cpu.csr_write(CSR_ASID, 0x55);
    write_test_tlb_entry(
        &mut cpu,
        idx,
        va,
        ps,
        0x55,
        tlbelo_test(ppn, true, true, 0, 0, false, false),
        tlbelo_test(0x40030, true, true, 0, 0, false, false),
    );
    let pa = (ppn << 12) | (va & ((1 << ps) - 1));

    assert_eq!(cpu.fast_tlb_lookup_addend(va, mmu::AccessType::Load), None);
    assert_tlb_hit(
        cpu.translate_address_and_cache(va, mmu::AccessType::Load),
        pa,
        0,
    );
    assert_eq!(
        cpu.fast_tlb_lookup_addend(va, mmu::AccessType::Load),
        Some(expected_tlb_addend(&cpu, va, pa))
    );

    cpu.csr_write(CSR_ASID, 0x56);
    assert_eq!(cpu.fast_tlb_lookup_addend(va, mmu::AccessType::Load), None);
}

#[test]
fn task23_fast_tlb_cache_invalidates_on_plv_change() {
    let mut cpu = LoongArchCpu::new();
    let ps = 14;
    let va = 0x0000_0000_5556_0120;
    let ppn = 0x40040;
    let idx = mmu::mtlb_flat_index(11).unwrap();
    cpu.set_guest_base(0x4_0000_0000);
    cpu.set_ram_base(0x4000_0000);
    cpu.set_ram_end(0x6000_0000);
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_STLBPS, ps as u64);
    cpu.csr_write(CSR_ASID, 0x55);
    write_test_tlb_entry(
        &mut cpu,
        idx,
        va,
        ps,
        0x55,
        tlbelo_test(ppn, true, true, 0, 0, false, false),
        tlbelo_test(0x40050, true, true, 0, 0, false, false),
    );
    let pa = (ppn << 12) | (va & ((1 << ps) - 1));

    assert_tlb_hit(
        cpu.translate_address_and_cache(va, mmu::AccessType::Load),
        pa,
        0,
    );
    assert_eq!(
        cpu.fast_tlb_lookup_addend(va, mmu::AccessType::Load),
        Some(expected_tlb_addend(&cpu, va, pa))
    );

    cpu.csr_write(CSR_CRMD, CRMD_PG | 3);
    assert_eq!(cpu.fast_tlb_lookup_addend(va, mmu::AccessType::Load), None);
    assert_eq!(
        cpu.translate_address(va, mmu::AccessType::Load),
        mmu::TlbLookupResult::PrivViolation
    );
}

#[test]
fn task23_fast_tlb_cache_invalidates_on_ertn_plv_restore() {
    let mut cpu = LoongArchCpu::new();
    let ps = 14;
    let va = 0x0000_0000_5557_0120;
    let ppn = 0x40060;
    let idx = mmu::mtlb_flat_index(12).unwrap();
    cpu.set_guest_base(0x5_0000_0000);
    cpu.set_ram_base(0x4000_0000);
    cpu.set_ram_end(0x6000_0000);
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_STLBPS, ps as u64);
    cpu.csr_write(CSR_ASID, 0x55);
    write_test_tlb_entry(
        &mut cpu,
        idx,
        va,
        ps,
        0x55,
        tlbelo_test(ppn, true, true, 0, 0, false, false),
        tlbelo_test(0x40070, true, true, 0, 0, false, false),
    );
    let pa = (ppn << 12) | (va & ((1 << ps) - 1));

    assert_tlb_hit(
        cpu.translate_address_and_cache(va, mmu::AccessType::Load),
        pa,
        0,
    );
    assert_eq!(
        cpu.fast_tlb_lookup_addend(va, mmu::AccessType::Load),
        Some(expected_tlb_addend(&cpu, va, pa))
    );

    cpu.csr_write(CSR_PRMD, 3);
    cpu.csr_write(CSR_ERA, 0x2000);
    cpu.csr_write(CSR_TLBRERA, 0);
    let _ = run_priv_la(&mut cpu, &[ERTN_INSN]);

    assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PLV_MASK, 3);
    assert_eq!(cpu.fast_tlb_lookup_addend(va, mmu::AccessType::Load), None);
    assert_eq!(
        cpu.translate_address(va, mmu::AccessType::Load),
        mmu::TlbLookupResult::PrivViolation
    );
}

#[test]
fn task23_fast_tlb_cache_invalidates_on_dmw_disable() {
    let mut cpu = LoongArchCpu::new();
    let va = 0x9000_0000_0000_2120;
    let pa = va & TARGET_VIRT_MASK;
    cpu.set_guest_base(0x2_0000_0000);
    cpu.set_ram_base(0);
    cpu.set_ram_end(1_u64 << 48);
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_DMW0, dmw64(0x9, 0, 1));
    assert!(cpu.take_tb_flush());

    assert_tlb_hit(
        cpu.translate_address_and_cache(va, mmu::AccessType::Fetch),
        pa,
        0,
    );
    assert_eq!(
        cpu.fast_tlb_lookup_addend(va, mmu::AccessType::Fetch),
        Some(expected_tlb_addend(&cpu, va, pa))
    );

    cpu.csr_write(CSR_DMW0, 0);
    assert!(cpu.take_tb_flush());
    assert_eq!(cpu.fast_tlb_lookup_addend(va, mmu::AccessType::Fetch), None);
}

#[test]
fn task23_fast_tlb_cache_invalidates_on_tlb_replacement() {
    let mut cpu = LoongArchCpu::new();
    let ps = 14;
    let va = 0x0000_0000_6666_0120;
    let idx = mmu::mtlb_flat_index(9).unwrap();
    cpu.set_guest_base(0x3_0000_0000);
    cpu.set_ram_base(0x5000_0000);
    cpu.set_ram_end(0x8000_0000);
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_STLBPS, ps as u64);
    cpu.csr_write(CSR_ASID, 0x66);
    write_test_tlb_entry(
        &mut cpu,
        idx,
        va,
        ps,
        0x66,
        tlbelo_test(0x50010, true, true, 0, 0, false, false),
        tlbelo_test(0x50020, true, true, 0, 0, false, false),
    );
    let pa1 = (0x50010 << 12) | (va & ((1 << ps) - 1));
    assert_tlb_hit(
        cpu.translate_address_and_cache(va, mmu::AccessType::Load),
        pa1,
        0,
    );
    assert_eq!(
        cpu.fast_tlb_lookup_addend(va, mmu::AccessType::Load),
        Some(expected_tlb_addend(&cpu, va, pa1))
    );

    write_test_tlb_entry(
        &mut cpu,
        idx,
        va,
        ps,
        0x66,
        tlbelo_test(0x60010, true, true, 0, 0, false, false),
        tlbelo_test(0x60020, true, true, 0, 0, false, false),
    );
    assert_eq!(cpu.fast_tlb_lookup_addend(va, mmu::AccessType::Load), None);

    let pa2 = (0x60010 << 12) | (va & ((1 << ps) - 1));
    assert_tlb_hit(
        cpu.translate_address_and_cache(va, mmu::AccessType::Load),
        pa2,
        0,
    );
    assert_eq!(
        cpu.fast_tlb_lookup_addend(va, mmu::AccessType::Load),
        Some(expected_tlb_addend(&cpu, va, pa2))
    );
}

#[test]
fn task24_tlb_misses_enter_tlbr_for_fetch_load_and_store() {
    let cases = [
        ("fetch", mmu::AccessType::Fetch, 0x0000_0000_7770_1000),
        ("load", mmu::AccessType::Load, 0x0000_0000_7770_2000),
        ("store", mmu::AccessType::Store, 0x0000_0000_7770_3000),
    ];

    for (name, access, va) in cases {
        let mut cpu = LoongArchCpu::new();
        let fault_pc = 0x1234_5600;
        cpu.csr_write(CSR_CRMD, 2 | CRMD_IE | CRMD_PG);
        cpu.csr_write(CSR_TLBRENTRY, 0x9000_1000);
        cpu.csr_write(CSR_TLBREHI, 0xDEAD_0000_0000_002A);
        cpu.set_estat_hw(0x2A5);

        assert_eq!(
            cpu.translate_address_or_exception(va, access, fault_pc),
            Err(0x9000_1000),
            "{name}"
        );
        assert_eq!(cpu.csr_read(CSR_TLBRERA) & 1, 1, "{name}");
        assert_eq!(cpu.csr_read(CSR_TLBRERA) & !0x3, fault_pc, "{name}");
        assert_eq!(cpu.csr_read(CSR_TLBRBADV), va, "{name}");
        assert_eq!(cpu.csr_read(CSR_TLBRPRMD), 2 | CRMD_IE, "{name}");
        assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PLV_MASK, 0, "{name}");
        assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_IE, 0, "{name}");
        assert_ne!(cpu.csr_read(CSR_CRMD) & CRMD_DA, 0, "{name}");
        assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PG, 0, "{name}");
        assert_eq!(cpu.csr_read(CSR_ESTAT) & ESTAT_IS_MASK, 0x2A5, "{name}");
        assert_eq!(cpu.csr_read(CSR_TLBREHI), (va & !0x1FFF) | 0x2A, "{name}");
    }
}

#[test]
fn task24_page_state_faults_use_generic_exception_and_badv() {
    let ps = 14;
    let va = 0x0000_0000_8888_0120;
    let fault_pc = 0x2000_0040;
    let eentry = 0x9000_0000;
    let vs = 2;
    let idx = mmu::mtlb_flat_index(13).unwrap();
    let cases = [
        (
            "invalid-load",
            mmu::AccessType::Load,
            tlbelo_test(0x81000, false, true, 3, 0, true, false),
            ECODE_PIL,
        ),
        (
            "invalid-store",
            mmu::AccessType::Store,
            tlbelo_test(0x82000, false, true, 3, 0, true, false),
            ECODE_PIS,
        ),
        (
            "invalid-fetch",
            mmu::AccessType::Fetch,
            tlbelo_test(0x83000, false, true, 3, 0, true, false),
            ECODE_PIF,
        ),
        (
            "dirty-store",
            mmu::AccessType::Store,
            tlbelo_test(0x84000, true, false, 3, 0, true, false),
            ECODE_PME,
        ),
        (
            "privilege",
            mmu::AccessType::Load,
            tlbelo_test(0x85000, true, true, 0, 0, true, false),
            ECODE_PPI,
        ),
        (
            "read-inhibit",
            mmu::AccessType::Load,
            tlbelo_test_with_perm(0x86000, true, true, 3, 0, true, true, false),
            ECODE_PNR,
        ),
        (
            "execute-inhibit",
            mmu::AccessType::Fetch,
            tlbelo_test_with_perm(0x87000, true, true, 3, 0, true, false, true),
            ECODE_PNX,
        ),
    ];

    for (name, access, elo0, ecode) in cases {
        let mut cpu = LoongArchCpu::new();
        cpu.csr_write(CSR_CRMD, CRMD_PLV_MASK | CRMD_IE | CRMD_PG);
        cpu.csr_write(CSR_EENTRY, eentry);
        cpu.csr_write(CSR_ECFG, vs << 16);
        cpu.set_estat_hw(0x181);
        write_test_tlb_entry(
            &mut cpu,
            idx,
            va,
            ps,
            0,
            elo0,
            tlbelo_test(0x88000, true, true, 3, 0, true, false),
        );
        cpu.csr_write(CSR_TLBEHI, 0xFFFF_0000_0000_0000);

        assert_eq!(
            cpu.translate_address_or_exception(va, access, fault_pc),
            Err(exception_vector(eentry, ecode, vs)),
            "{name}"
        );
        assert_eq!(cpu.csr_read(CSR_ERA), fault_pc, "{name}");
        assert_eq!(cpu.csr_read(CSR_BADV), va, "{name}");
        assert_eq!(cpu.csr_read(CSR_PRMD), CRMD_PLV_MASK | CRMD_IE, "{name}");
        assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_PLV_MASK, 0, "{name}");
        assert_eq!(cpu.csr_read(CSR_CRMD) & CRMD_IE, 0, "{name}");
        assert_ne!(cpu.csr_read(CSR_CRMD) & CRMD_PG, 0, "{name}");
        assert_eq!(cpu.csr_read(CSR_TLBRERA) & 1, 0, "{name}");
        assert_eq!(cpu.csr_read(CSR_ESTAT) & ESTAT_IS_MASK, 0x181, "{name}");
        assert_eq!(
            (cpu.csr_read(CSR_ESTAT) >> 16) & 0x3F,
            u64::from(ecode),
            "{name}"
        );
        assert_eq!((cpu.csr_read(CSR_ESTAT) >> 22) & 0x1FF, 0, "{name}");
        assert_eq!(cpu.csr_read(CSR_TLBEHI), va & !0x1FFF, "{name}");
    }
}

#[test]
fn task24_direct_map_hits_do_not_enter_tlb_exception_flow() {
    let mut cpu = LoongArchCpu::new();
    let va = 0xFFFF_8000_0000_2340;
    cpu.csr_write(CSR_CRMD, CRMD_DA);
    cpu.csr_write(CSR_ERA, 0x1111);
    cpu.csr_write(CSR_BADV, 0x2222);
    cpu.csr_write(CSR_TLBRERA, 0x3330);

    assert_eq!(
        cpu.translate_address_or_exception(va, mmu::AccessType::Fetch, 0x4000),
        Ok(va & TARGET_VIRT_MASK)
    );
    assert_eq!(cpu.csr_read(CSR_ERA), 0x1111);
    assert_eq!(cpu.csr_read(CSR_BADV), 0x2222);
    assert_eq!(cpu.csr_read(CSR_TLBRERA), 0x3330);

    let mut cpu = LoongArchCpu::new();
    let dmw_va = 0x9000_0000_0000_3450;
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_DMW0, dmw64(0x9, 0, 1));
    assert!(cpu.take_tb_flush());

    assert_eq!(
        cpu.translate_address_or_exception(
            dmw_va,
            mmu::AccessType::Load,
            0x4000
        ),
        Ok(dmw_va & TARGET_VIRT_MASK)
    );
    assert_eq!(cpu.csr_read(CSR_TLBRERA), 0);
    assert_eq!(cpu.csr_read(CSR_BADV), 0);
}

#[test]
fn task24_exec_loop_fetch_miss_enters_tlbrentry_and_continues() {
    let code = [code15_insn(IDLE_OP, 0)];
    let fault_pc = 0x0000_0000_4000_0000;
    let mut cpu = LoongArchCpu::new();
    cpu.set_pc(fault_pc);
    cpu.csr_write(CSR_CRMD, CRMD_PLV_MASK | CRMD_IE | CRMD_PG);
    cpu.csr_write(CSR_TLBRENTRY, 0);
    let mut sys = full_system_cpu_with_code(cpu, &code);
    let mut env = ExecEnv::new(X86_64CodeGen::new());

    let exit = unsafe { cpu_exec_loop_env(&mut env, &mut sys) };

    assert_eq!(exit, ExitReason::Halted);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRERA) & 1, 1);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRERA) & !0x3, fault_pc);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRBADV), fault_pc);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRPRMD), CRMD_PLV_MASK | CRMD_IE);
    assert_eq!(sys.cpu.csr_read(CSR_CRMD) & CRMD_PLV_MASK, 0);
    assert_ne!(sys.cpu.csr_read(CSR_CRMD) & CRMD_DA, 0);
    assert_eq!(sys.cpu.csr_read(CSR_CRMD) & CRMD_PG, 0);
    assert_eq!(sys.cpu.pc(), 4);
}

#[test]
fn task24_exec_loop_translated_load_store_misses_enter_tlbrentry() {
    let code_va = 0x9000_0000_0000_0000;
    let handler_off = 0x40usize;
    let data_pa = 0x80usize;
    let data_va = data_pa as u64;
    let load_sentinel = 0x1122_3344_5566_7788;
    let load_reexecute_value = 0x8877_6655_4433_2211;
    let store_sentinel = 0xA55A_A55A_C33C_C33C;
    let store_reexecute_value = 0xCAFE_BABE_DEAD_BEEF;
    let cases = [
        ("load", r2_si12(OP_LD_D, 0, 5, 6)),
        ("store", r2_si12(OP_ST_D, 0, 5, 7)),
    ];

    for (name, insn) in cases {
        let code_len = data_pa.max(handler_off) / 4 + 2;
        let mut code = vec![0x0340_0000; code_len];
        code[0] = insn;
        code[handler_off / 4] = code15_insn(IDLE_OP, 0);
        let initial_data = if name == "load" {
            load_reexecute_value
        } else {
            store_sentinel
        };
        code[data_pa / 4] = initial_data as u32;
        code[data_pa / 4 + 1] = (initial_data >> 32) as u32;
        let mut cpu = LoongArchCpu::new();
        cpu.set_pc(code_va);
        cpu.write_gpr(5, data_va);
        cpu.write_gpr(6, load_sentinel);
        cpu.write_gpr(7, store_reexecute_value);
        cpu.csr_write(CSR_CRMD, CRMD_PLV_MASK | CRMD_IE | CRMD_PG);
        cpu.csr_write(CSR_DMW0, dmw64(0x9, 0, 1 | (1 << 3)));
        assert!(cpu.take_tb_flush());
        let tlbrentry = code_va + handler_off as u64;
        cpu.csr_write(CSR_TLBRENTRY, tlbrentry);
        assert_eq!(cpu.csr_read(CSR_TLBRENTRY), tlbrentry);
        cpu.csr_write(CSR_TLBREHI, 0xFEED_0000_0000_003F);
        let mut sys = full_system_cpu_with_code(cpu, &code);
        let mut env = loongarch_soft_mmu_env();

        let exit = unsafe { cpu_exec_loop_env(&mut env, &mut sys) };

        assert_eq!(exit, ExitReason::Halted, "{name}");
        assert_eq!(sys.cpu.csr_read(CSR_TLBRERA) & 1, 1, "{name}");
        assert_eq!(sys.cpu.csr_read(CSR_TLBRERA) & !0x3, code_va, "{name}");
        assert_eq!(sys.cpu.csr_read(CSR_TLBRBADV), data_va, "{name}");
        assert_eq!(
            sys.cpu.csr_read(CSR_TLBRPRMD),
            CRMD_PLV_MASK | CRMD_IE,
            "{name}"
        );
        assert_ne!(sys.cpu.csr_read(CSR_CRMD) & CRMD_DA, 0, "{name}");
        assert_eq!(sys.cpu.csr_read(CSR_CRMD) & CRMD_PG, 0, "{name}");
        assert_eq!(
            sys.cpu.csr_read(CSR_TLBREHI),
            (data_va & !0x1FFF) | 0x3F,
            "{name}"
        );
        assert_eq!(sys.cpu.pc(), tlbrentry + 4, "{name}");
        if name == "load" {
            assert_eq!(sys.cpu.read_gpr(6), load_sentinel, "{name}");
        } else {
            let data_word = data_pa / 4;
            let data = u64::from(code[data_word])
                | (u64::from(code[data_word + 1]) << 32);
            assert_eq!(data, store_sentinel, "{name}");
        }
    }
}

#[test]
fn task24_exec_loop_translated_load_read_inhibit_sets_generic_fault() {
    let code_va = 0x9000_0000_0000_0000;
    let handler_off = 0x40usize;
    let data_va = 0x0000_0000_5555_0120;
    let eentry = code_va + handler_off as u64;
    let mut code = vec![0x0340_0000; handler_off / 4 + 1];
    code[0] = r2_si12(OP_LD_D, 0, 5, 6);
    code[handler_off / 4] = code15_insn(IDLE_OP, 0);
    let mut cpu = LoongArchCpu::new();
    cpu.set_pc(code_va);
    cpu.write_gpr(5, data_va);
    cpu.csr_write(CSR_CRMD, CRMD_PLV_MASK | CRMD_IE | CRMD_PG);
    cpu.csr_write(CSR_DMW0, dmw64(0x9, 0, 1 | (1 << 3)));
    assert!(cpu.take_tb_flush());
    cpu.csr_write(CSR_EENTRY, eentry);
    let idx = mmu::mtlb_flat_index(17).unwrap();
    write_test_tlb_entry(
        &mut cpu,
        idx,
        data_va,
        14,
        0,
        tlbelo_test_with_perm(0x100, true, true, 3, 0, true, true, false),
        tlbelo_test(0x200, true, true, 3, 0, true, false),
    );
    cpu.csr_write(CSR_TLBEHI, 0x1234_0000_0000_0000);
    let mut sys = full_system_cpu_with_code(cpu, &code);
    let mut env = loongarch_soft_mmu_env();

    let exit = unsafe { cpu_exec_loop_env(&mut env, &mut sys) };

    assert_eq!(exit, ExitReason::Halted);
    assert_eq!(sys.cpu.csr_read(CSR_ERA), code_va);
    assert_eq!(sys.cpu.csr_read(CSR_BADV), data_va);
    assert_eq!(sys.cpu.csr_read(CSR_PRMD), CRMD_PLV_MASK | CRMD_IE);
    assert_eq!(
        (sys.cpu.csr_read(CSR_ESTAT) >> 16) & 0x3F,
        u64::from(ECODE_PNR)
    );
    assert_eq!((sys.cpu.csr_read(CSR_ESTAT) >> 22) & 0x1FF, 0);
    assert_eq!(sys.cpu.csr_read(CSR_TLBEHI), data_va & !0x1FFF);
    assert_ne!(sys.cpu.csr_read(CSR_CRMD) & CRMD_PG, 0);
    assert_eq!(sys.cpu.csr_read(CSR_CRMD) & CRMD_PLV_MASK, 0);
    assert_eq!(sys.cpu.pc(), eentry + 4);
}

#[test]
fn task25_tlbsrch_translated_sets_index_and_ne_for_mtlb_stlb_and_tlbr_source() {
    let ps = 14;
    let mtlb_va = 0x0000_0000_2468_0120;
    let mtlb_idx = mmu::mtlb_flat_index(6).unwrap();
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_STLBPS, ps as u64);
    cpu.csr_write(CSR_ASID, 0x21);
    write_test_tlb_entry(
        &mut cpu,
        mtlb_idx,
        mtlb_va,
        ps,
        0x21,
        tlbelo_test(0x21000, true, true, 0, 0, false, false),
        tlbelo_test(0x22000, true, true, 0, 0, false, false),
    );
    cpu.csr_write(CSR_TLBEHI, mtlb_va & !0x1FFF);
    assert_eq!(run_priv_la(&mut cpu, &[TLBSRCH_INSN]), 0);
    assert_eq!(cpu.csr_read(CSR_TLBIDX) & 0xFFF, mtlb_idx as u64);
    assert_eq!(cpu.csr_read(CSR_TLBIDX) & (1 << 31), 0);

    cpu.csr_write(CSR_TLBEHI, 0x0000_0000_1357_0000);
    assert_eq!(run_priv_la(&mut cpu, &[TLBSRCH_INSN]), 0);
    assert_ne!(cpu.csr_read(CSR_TLBIDX) & (1 << 31), 0);

    let global_va = 0x0000_0000_3579_0120;
    write_test_tlb_entry(
        &mut cpu,
        mtlb_idx,
        global_va,
        ps,
        0x99,
        tlbelo_test(0x23000, true, true, 0, 0, true, false),
        tlbelo_test(0x24000, true, true, 0, 0, true, false),
    );
    cpu.csr_write(CSR_ASID, 0x21);
    cpu.csr_write(CSR_TLBEHI, global_va & !0x1FFF);
    assert_eq!(run_priv_la(&mut cpu, &[TLBSRCH_INSN]), 0);
    assert_eq!(cpu.csr_read(CSR_TLBIDX) & 0xFFF, mtlb_idx as u64);
    assert_eq!(cpu.csr_read(CSR_TLBIDX) & (1 << 31), 0);

    let stlb_va = 0x0000_0000_468A_0120;
    let stlb_set = mmu::stlb_set_index(stlb_va, ps).unwrap();
    let stlb_idx = mmu::stlb_flat_index(stlb_set, 2).unwrap();
    write_test_tlb_entry(
        &mut cpu,
        stlb_idx,
        stlb_va,
        ps,
        0x21,
        tlbelo_test(0x25000, true, true, 0, 0, false, false),
        tlbelo_test(0x26000, true, true, 0, 0, false, false),
    );
    cpu.csr_write(CSR_TLBEHI, stlb_va & !0x1FFF);
    assert_eq!(run_priv_la(&mut cpu, &[TLBSRCH_INSN]), 0);
    assert_eq!(cpu.csr_read(CSR_TLBIDX) & 0xFFF, stlb_idx as u64);
    assert_eq!(cpu.csr_read(CSR_TLBIDX) & (1 << 31), 0);

    let refill_va = 0x0000_0000_579B_0120;
    write_test_tlb_entry(
        &mut cpu,
        mtlb_idx,
        refill_va,
        ps,
        0x21,
        tlbelo_test(0x27000, true, true, 0, 0, false, false),
        tlbelo_test(0x28000, true, true, 0, 0, false, false),
    );
    cpu.csr_write(CSR_TLBEHI, 0x0000_0000_6666_0000);
    cpu.csr_write(CSR_TLBREHI, refill_va & !0x1FFF);
    cpu.csr_write(CSR_TLBRERA, 1);
    assert_eq!(run_priv_la(&mut cpu, &[TLBSRCH_INSN]), 0);
    assert_eq!(cpu.csr_read(CSR_TLBIDX) & 0xFFF, mtlb_idx as u64);
    assert_eq!(cpu.csr_read(CSR_TLBIDX) & (1 << 31), 0);
}

#[test]
fn task25_tlbrd_translated_restores_valid_and_clears_invalid_state() {
    let mut cpu = LoongArchCpu::new();
    let ps = 21;
    let va = 0x0000_0000_6780_0120;
    let idx = mmu::mtlb_flat_index(15).unwrap();
    let elo0 =
        tlbelo_test_with_perm(0x31000, true, true, 1, 2, true, true, false);
    let elo1 =
        tlbelo_test_with_perm(0x32000, true, false, 2, 1, true, false, true);
    write_test_tlb_entry(&mut cpu, idx, va, ps, 0x155, elo0, elo1);

    cpu.csr_write(CSR_TLBEHI, 0);
    cpu.csr_write(CSR_TLBELO0, 0);
    cpu.csr_write(CSR_TLBELO1, 0);
    cpu.csr_write(CSR_ASID, 0);
    cpu.csr_write(CSR_TLBIDX, (1 << 31) | idx as u64);
    assert_eq!(run_priv_la(&mut cpu, &[TLBRD_INSN]), 0);
    assert_eq!(cpu.csr_read(CSR_TLBIDX) & (1 << 31), 0);
    assert_eq!((cpu.csr_read(CSR_TLBIDX) >> 24) & 0x3F, ps as u64);
    assert_eq!(cpu.csr_read(CSR_TLBIDX) & 0xFFF, idx as u64);
    assert_eq!(cpu.csr_read(CSR_TLBEHI), tlb_pair_base(va, ps) & !0x1FFF);
    assert_eq!(cpu.csr_read(CSR_TLBELO0), elo0);
    assert_eq!(cpu.csr_read(CSR_TLBELO1), elo1);
    assert_eq!(cpu.csr_read(CSR_ASID) & 0x3FF, 0x155);

    cpu.csr_write(CSR_TLBIDX, mmu::mtlb_flat_index(16).unwrap() as u64);
    cpu.csr_write(CSR_TLBEHI, 0xFFFF_0000_0000_0000);
    cpu.csr_write(CSR_TLBELO0, 0xFFFF);
    cpu.csr_write(CSR_TLBELO1, 0xEEEE);
    cpu.csr_write(CSR_ASID, 0x3FF);
    assert_eq!(run_priv_la(&mut cpu, &[TLBRD_INSN]), 0);
    assert_ne!(cpu.csr_read(CSR_TLBIDX) & (1 << 31), 0);
    assert_eq!((cpu.csr_read(CSR_TLBIDX) >> 24) & 0x3F, 0);
    assert_eq!(cpu.csr_read(CSR_TLBEHI), 0);
    assert_eq!(cpu.csr_read(CSR_TLBELO0), 0);
    assert_eq!(cpu.csr_read(CSR_TLBELO1), 0);
    assert_eq!(cpu.csr_read(CSR_ASID) & 0x3FF, 0);
}

#[test]
fn task25_tlbwr_tlbfill_translated_preserve_fields_and_flush_fast_tlb() {
    let mut cpu = LoongArchCpu::new();
    let ps = 14;
    let va = 0x0000_0000_789A_0120;
    let idx = mmu::mtlb_flat_index(20).unwrap();
    cpu.set_guest_base(0x6_0000_0000);
    cpu.set_ram_base(0x4000_0000);
    cpu.set_ram_end(0x8000_0000);
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_ASID, 0x34);
    write_test_tlb_entry(
        &mut cpu,
        idx,
        va,
        ps,
        0x34,
        tlbelo_test(0x40010, true, true, 0, 0, false, false),
        tlbelo_test(0x40020, true, true, 0, 0, false, false),
    );
    let old_pa = (0x40010 << 12) | (va & ((1 << ps) - 1));
    assert_tlb_hit(
        cpu.translate_address_and_cache(va, mmu::AccessType::Load),
        old_pa,
        0,
    );
    assert!(cpu
        .fast_tlb_lookup_addend(va, mmu::AccessType::Load)
        .is_some());

    let elo0 =
        tlbelo_test_with_perm(0x50010, true, true, 1, 2, true, true, false);
    let elo1 =
        tlbelo_test_with_perm(0x50020, true, false, 2, 3, true, false, true);
    cpu.csr_write(CSR_TLBEHI, tlb_pair_base(va, ps) & !0x1FFF);
    cpu.csr_write(CSR_TLBELO0, elo0);
    cpu.csr_write(CSR_TLBELO1, elo1);
    cpu.csr_write(CSR_ASID, 0x34);
    cpu.csr_write(CSR_TLBIDX, (u64::from(ps) << 24) | idx as u64);
    assert_eq!(run_priv_la(&mut cpu, &[TLBWR_INSN]), 0);
    assert_eq!(cpu.fast_tlb_lookup_addend(va, mmu::AccessType::Load), None);
    assert!(cpu.take_tb_flush());
    assert!(!cpu.take_tb_flush());
    let entry = cpu.mmu().mtlb[20];
    assert!(entry.valid);
    assert_eq!(entry.vppn, (tlb_pair_base(va, ps) & !0x1FFF) >> 13);
    assert_eq!(entry.page_size, ps);
    assert_eq!(entry.asid, 0x34);
    assert!(entry.g);
    assert!(entry.nr0);
    assert!(entry.nx1);
    assert_eq!(entry.ppn0, 0x50010);
    assert_eq!(entry.ppn1, 0x50020);

    cpu.csr_write(CSR_TLBIDX, (1 << 31) | idx as u64);
    assert_eq!(run_priv_la(&mut cpu, &[TLBWR_INSN]), 0);
    assert!(cpu.take_tb_flush());
    assert!(!cpu.mmu().mtlb[20].valid);

    let refill_wr_va = 0x0000_0000_7ABC_0120;
    let refill_wr_ps = 16;
    let refill_wr_idx = mmu::mtlb_flat_index(21).unwrap();
    let refill_wr_elo0 =
        tlbelo_test_with_perm(0x53010, true, true, 1, 1, true, true, false);
    let refill_wr_elo1 =
        tlbelo_test_with_perm(0x53020, true, false, 2, 2, true, false, true);
    cpu.csr_write(CSR_TLBRERA, 1);
    cpu.csr_write(
        CSR_TLBREHI,
        (tlb_pair_base(refill_wr_va, refill_wr_ps) & !0x1FFF)
            | u64::from(refill_wr_ps),
    );
    cpu.csr_write(CSR_TLBRELO0, refill_wr_elo0);
    cpu.csr_write(CSR_TLBRELO1, refill_wr_elo1);
    cpu.csr_write(CSR_TLBEHI, 0x0000_0000_BAAD_0000);
    cpu.csr_write(
        CSR_TLBELO0,
        tlbelo_test_with_perm(0x5BAD0, true, true, 0, 0, false, false, true),
    );
    cpu.csr_write(
        CSR_TLBELO1,
        tlbelo_test_with_perm(0x5BAD1, true, true, 0, 0, false, true, false),
    );
    cpu.csr_write(CSR_ASID, 0x99);
    cpu.csr_write(CSR_TLBIDX, (12_u64 << 24) | refill_wr_idx as u64);
    assert_eq!(run_priv_la(&mut cpu, &[TLBWR_INSN]), 0);
    let refill_wr_entry = cpu.mmu().mtlb[21];
    assert!(refill_wr_entry.valid);
    assert_eq!(
        refill_wr_entry.vppn,
        (tlb_pair_base(refill_wr_va, refill_wr_ps) & !0x1FFF) >> 13
    );
    assert_eq!(refill_wr_entry.page_size, refill_wr_ps);
    assert_eq!(refill_wr_entry.asid, 0x99);
    assert!(refill_wr_entry.g);
    assert!(refill_wr_entry.nr0);
    assert!(refill_wr_entry.nx1);
    assert_eq!(refill_wr_entry.ppn0, 0x53010);
    assert_eq!(refill_wr_entry.ppn1, 0x53020);
    assert!(cpu.take_tb_flush());

    let stlb_va = 0x0000_0000_89AB_0120;
    let stlb_set = mmu::stlb_set_index(stlb_va, ps).unwrap();
    let refill_fill_elo0 =
        tlbelo_test_with_perm(0x61000, true, true, 1, 1, true, true, false);
    let refill_fill_elo1 =
        tlbelo_test_with_perm(0x62000, true, true, 2, 2, true, false, true);
    cpu.csr_write(CSR_STLBPS, ps as u64);
    cpu.csr_write(CSR_TLBRERA, 1);
    cpu.csr_write(CSR_TLBREHI, (stlb_va & !0x1FFF) | u64::from(ps));
    cpu.csr_write(CSR_TLBRELO0, refill_fill_elo0);
    cpu.csr_write(CSR_TLBRELO1, refill_fill_elo1);
    cpu.csr_write(CSR_TLBEHI, 0x0000_0000_DEAD_0000);
    cpu.csr_write(
        CSR_TLBELO0,
        tlbelo_test_with_perm(0x6BAD0, true, true, 0, 0, false, false, true),
    );
    cpu.csr_write(
        CSR_TLBELO1,
        tlbelo_test_with_perm(0x6BAD1, true, true, 0, 0, false, true, false),
    );
    cpu.csr_write(CSR_ASID, 0x56);
    cpu.csr_write(CSR_TLBIDX, u64::from(ps) << 24);
    assert_eq!(run_priv_la(&mut cpu, &[TLBFILL_INSN]), 0);
    assert!(cpu.take_tb_flush());
    let stlb_entry = cpu.mmu().stlb[stlb_set][0];
    assert!(stlb_entry.valid);
    assert_eq!(stlb_entry.vppn, (stlb_va & !0x1FFF) >> 13);
    assert_eq!(stlb_entry.page_size, ps);
    assert_eq!(stlb_entry.asid, 0x56);
    assert!(stlb_entry.g);
    assert!(stlb_entry.nr0);
    assert!(stlb_entry.nx1);
    assert_eq!(stlb_entry.ppn0, 0x61000);
    assert_eq!(stlb_entry.ppn1, 0x62000);

    let mtlb_va = 0x0000_0000_9ABC_0120;
    let mtlb_ps = 21;
    cpu.csr_write(CSR_TLBRERA, 0);
    cpu.csr_write(CSR_TLBEHI, tlb_pair_base(mtlb_va, mtlb_ps) & !0x1FFF);
    cpu.csr_write(
        CSR_TLBELO0,
        tlbelo_test(0x71000, true, true, 0, 1, false, false),
    );
    cpu.csr_write(
        CSR_TLBELO1,
        tlbelo_test(0x72000, true, true, 0, 2, false, false),
    );
    cpu.csr_write(CSR_ASID, 0x78);
    cpu.csr_write(CSR_TLBIDX, (u64::from(mtlb_ps) << 24) | 5);
    assert_eq!(run_priv_la(&mut cpu, &[TLBFILL_INSN]), 0);
    let mtlb_entry = cpu.mmu().mtlb[5];
    assert!(mtlb_entry.valid);
    assert_eq!(
        mtlb_entry.vppn,
        (tlb_pair_base(mtlb_va, mtlb_ps) & !0x1FFF) >> 13
    );
    assert_eq!(mtlb_entry.page_size, mtlb_ps);
    assert_eq!(mtlb_entry.asid, 0x78);
    assert!(cpu.take_tb_flush());
}

#[test]
fn task86_tlbclr_tlbflush_cacop_translate_without_illegal_instruction() {
    let cases = [
        ("tlbclr", TLBCLR_INSN, true),
        ("tlbflush", TLBFLUSH_INSN, true),
        ("cacop", cop_r_si12(CACOP_OP, -16, 5, 3), false),
    ];

    for (name, insn, flushes_tb) in cases {
        let mut cpu = LoongArchCpu::new();
        cpu.csr_write(CSR_EENTRY, 0x100);

        let exit = run_priv_la(&mut cpu, &[insn]);

        assert_eq!(exit, 0, "{name} did not exit through the normal TB path");
        assert_eq!(cpu.pc(), 4, "{name} did not advance past the insn");
        assert_ne!(
            (cpu.csr_read(CSR_ESTAT) >> 16) & 0x3F,
            u64::from(ECODE_INE),
            "{name} trapped as an illegal instruction"
        );
        assert_eq!(
            cpu.take_tb_flush(),
            flushes_tb,
            "{name} TB flush request mismatch"
        );
    }
}

#[test]
fn task86_tlbclr_clears_non_global_matching_asid_line_and_requests_flush() {
    let ps = 14;
    let set = 9;
    let matching_idx = mmu::stlb_flat_index(set, 0).unwrap();
    let global_idx = mmu::stlb_flat_index(set, 1).unwrap();
    let other_asid_idx = mmu::stlb_flat_index(set, 2).unwrap();
    let other_set_idx = mmu::stlb_flat_index(set + 1, 0).unwrap();

    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_STLBPS, ps as u64);
    write_test_tlb_entry(
        &mut cpu,
        matching_idx,
        0x0000_0000_A100_0120,
        ps,
        0x22,
        tlbelo_test(0xA1000, true, true, 0, 0, false, false),
        tlbelo_test(0xA1001, true, true, 0, 0, false, false),
    );
    write_test_tlb_entry(
        &mut cpu,
        global_idx,
        0x0000_0000_A200_0120,
        ps,
        0x22,
        tlbelo_test(0xA2000, true, true, 0, 0, true, false),
        tlbelo_test(0xA2001, true, true, 0, 0, true, false),
    );
    write_test_tlb_entry(
        &mut cpu,
        other_asid_idx,
        0x0000_0000_A300_0120,
        ps,
        0x33,
        tlbelo_test(0xA3000, true, true, 0, 0, false, false),
        tlbelo_test(0xA3001, true, true, 0, 0, false, false),
    );
    write_test_tlb_entry(
        &mut cpu,
        other_set_idx,
        0x0000_0000_A400_0120,
        ps,
        0x22,
        tlbelo_test(0xA4000, true, true, 0, 0, false, false),
        tlbelo_test(0xA4001, true, true, 0, 0, false, false),
    );
    let _ = cpu.take_tb_flush();

    cpu.csr_write(CSR_ASID, 0x22);
    cpu.csr_write(CSR_TLBIDX, (u64::from(ps) << 24) | matching_idx as u64);

    assert_eq!(run_priv_la(&mut cpu, &[TLBCLR_INSN]), 0);

    assert!(!cpu.mmu().stlb[set][0].valid);
    assert!(cpu.mmu().stlb[set][1].valid);
    assert!(cpu.mmu().stlb[set][2].valid);
    assert!(cpu.mmu().stlb[set + 1][0].valid);
    assert!(cpu.take_tb_flush());
}

#[test]
fn task86_tlbflush_clears_selected_stlb_line_and_mtlb_entries() {
    let ps = 14;
    let set = 17;
    let stlb_idx = mmu::stlb_flat_index(set, 3).unwrap();
    let other_set_idx = mmu::stlb_flat_index(set + 1, 0).unwrap();

    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_STLBPS, ps as u64);
    write_test_tlb_entry(
        &mut cpu,
        stlb_idx,
        0x0000_0000_B100_0120,
        ps,
        0x44,
        tlbelo_test(0xB1000, true, true, 0, 0, true, false),
        tlbelo_test(0xB1001, true, true, 0, 0, true, false),
    );
    write_test_tlb_entry(
        &mut cpu,
        other_set_idx,
        0x0000_0000_B200_0120,
        ps,
        0x44,
        tlbelo_test(0xB2000, true, true, 0, 0, false, false),
        tlbelo_test(0xB2001, true, true, 0, 0, false, false),
    );
    let _ = cpu.take_tb_flush();

    cpu.csr_write(CSR_TLBIDX, (u64::from(ps) << 24) | stlb_idx as u64);
    assert_eq!(run_priv_la(&mut cpu, &[TLBFLUSH_INSN]), 0);

    for way in 0..mmu::STLB_WAYS {
        assert!(!cpu.mmu().stlb[set][way].valid);
    }
    assert!(cpu.mmu().stlb[set + 1][0].valid);
    assert!(cpu.take_tb_flush());

    let mtlb_a = mmu::mtlb_flat_index(4).unwrap();
    let mtlb_b = mmu::mtlb_flat_index(5).unwrap();
    write_test_tlb_entry(
        &mut cpu,
        mtlb_a,
        0x0000_0000_B300_0120,
        21,
        0x55,
        tlbelo_test(0xB3000, true, true, 0, 0, true, false),
        tlbelo_test(0xB3001, true, true, 0, 0, true, false),
    );
    write_test_tlb_entry(
        &mut cpu,
        mtlb_b,
        0x0000_0000_B400_0120,
        21,
        0x66,
        tlbelo_test(0xB4000, true, true, 0, 0, false, false),
        tlbelo_test(0xB4001, true, true, 0, 0, false, false),
    );
    let _ = cpu.take_tb_flush();

    cpu.csr_write(CSR_TLBIDX, mmu::STLB_SIZE as u64);
    assert_eq!(run_priv_la(&mut cpu, &[TLBFLUSH_INSN]), 0);

    assert!(!cpu.mmu().mtlb[4].valid);
    assert!(!cpu.mmu().mtlb[5].valid);
    assert!(cpu.take_tb_flush());
}

#[test]
fn task86_tlbclr_tlbflush_cacop_check_plv() {
    let cases = [
        ("tlbclr", TLBCLR_INSN),
        ("tlbflush", TLBFLUSH_INSN),
        ("cacop", cop_r_si12(CACOP_OP, 0, 0, 0)),
    ];

    for (name, insn) in cases {
        let mut cpu = LoongArchCpu::new();
        cpu.csr_write(CSR_CRMD, CRMD_DA | CRMD_PLV_MASK);
        cpu.csr_write(CSR_EENTRY, 0x100);

        let _ = run_priv_la(&mut cpu, &[insn]);

        assert_eq!(cpu.pc(), 0x100, "{name} did not enter exception vector");
        assert_eq!(cpu.csr_read(CSR_ERA), 0, "{name} saved wrong ERA");
        assert_eq!(
            (cpu.csr_read(CSR_ESTAT) >> 16) & 0x3F,
            u64::from(ECODE_IPE),
            "{name} did not raise privilege exception"
        );
    }
}

#[test]
fn task25_invtlb_opcodes_invalidate_expected_entries() {
    let ps = 21;
    let va = 0x0000_0000_A000_1234;
    let same_large_page_va = va + 0x1000;
    let idx_global = mmu::mtlb_flat_index(1).unwrap();
    let idx_a = mmu::mtlb_flat_index(2).unwrap();
    let idx_b = mmu::mtlb_flat_index(3).unwrap();

    for op in 0..=6 {
        let mut cpu = LoongArchCpu::new();
        cpu.csr_write(CSR_STLBPS, 14);
        write_test_tlb_entry(
            &mut cpu,
            idx_global,
            va,
            ps,
            0x11,
            tlbelo_test(0x81000, true, true, 0, 0, true, false),
            tlbelo_test(0x82000, true, true, 0, 0, true, false),
        );
        write_test_tlb_entry(
            &mut cpu,
            idx_a,
            va,
            ps,
            0x22,
            tlbelo_test(0x83000, true, true, 0, 0, false, false),
            tlbelo_test(0x84000, true, true, 0, 0, false, false),
        );
        write_test_tlb_entry(
            &mut cpu,
            idx_b,
            va + (1 << (ps + 1)),
            ps,
            0x22,
            tlbelo_test(0x85000, true, true, 0, 0, false, false),
            tlbelo_test(0x86000, true, true, 0, 0, false, false),
        );

        cpu.invtlb(op, 0x22, same_large_page_va);

        match op {
            0 | 1 => {
                assert!(!cpu.mmu().mtlb[1].valid);
                assert!(!cpu.mmu().mtlb[2].valid);
                assert!(!cpu.mmu().mtlb[3].valid);
            }
            2 => {
                assert!(!cpu.mmu().mtlb[1].valid);
                assert!(cpu.mmu().mtlb[2].valid);
                assert!(cpu.mmu().mtlb[3].valid);
            }
            3 => {
                assert!(cpu.mmu().mtlb[1].valid);
                assert!(!cpu.mmu().mtlb[2].valid);
                assert!(!cpu.mmu().mtlb[3].valid);
            }
            4 => {
                assert!(cpu.mmu().mtlb[1].valid);
                assert!(!cpu.mmu().mtlb[2].valid);
                assert!(!cpu.mmu().mtlb[3].valid);
            }
            5 => {
                assert!(cpu.mmu().mtlb[1].valid);
                assert!(!cpu.mmu().mtlb[2].valid);
                assert!(cpu.mmu().mtlb[3].valid);
            }
            6 => {
                assert!(!cpu.mmu().mtlb[1].valid);
                assert!(!cpu.mmu().mtlb[2].valid);
                assert!(cpu.mmu().mtlb[3].valid);
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn task25_translated_invtlb_invalid_opcode_raises_ine() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_EENTRY, 0x100);
    cpu.write_gpr(1, 0x22);
    cpu.write_gpr(2, 0x0000_0000_A000_1234);

    let _ = run_priv_la(&mut cpu, &[r3_insn(INVTLB_OP, 2, 1, 7)]);

    assert_eq!(cpu.pc(), 0x100);
    assert_eq!(cpu.csr_read(CSR_ERA), 0);
    assert_eq!((cpu.csr_read(CSR_ESTAT) >> 16) & 0x3F, u64::from(ECODE_INE));
}

#[test]
fn task25_translated_lvz_invtlb_opcode_18_is_accepted() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_EENTRY, 0x100);

    let _ = run_priv_la(&mut cpu, &[r3_insn(INVTLB_OP, 0, 0, 0x12)]);

    assert_eq!(cpu.pc(), 4);
    assert_eq!((cpu.csr_read(CSR_ESTAT) >> 16) & 0x3F, 0);
    assert!(cpu.take_tb_flush());
}

#[test]
fn task25_translated_tlb_changers_stop_before_following_instruction() {
    let ps = 14;
    let va = 0x0000_0000_B000_0120;
    let idx = mmu::mtlb_flat_index(6).unwrap();
    let cases = [
        ("tlbwr", TLBWR_INSN),
        ("tlbfill", TLBFILL_INSN),
        ("invtlb", r3_insn(INVTLB_OP, 0, 0, 0)),
    ];

    for (name, insn) in cases {
        let mut cpu = LoongArchCpu::new();
        cpu.csr_write(CSR_CRMD, CRMD_PG);
        cpu.csr_write(CSR_STLBPS, ps as u64);
        cpu.csr_write(CSR_TLBEHI, tlb_pair_base(va, ps) & !0x1FFF);
        cpu.csr_write(
            CSR_TLBELO0,
            tlbelo_test(0x90000, true, true, 0, 0, false, false),
        );
        cpu.csr_write(
            CSR_TLBELO1,
            tlbelo_test(0x90010, true, true, 0, 0, false, false),
        );
        cpu.csr_write(CSR_ASID, 0x22);
        cpu.csr_write(CSR_TLBIDX, (u64::from(ps) << 24) | idx as u64);

        let exit = run_priv_la(&mut cpu, &[insn, code15_insn(IDLE_OP, 0)]);

        assert_eq!(exit, 0, "{name} did not stop at a chainable TB exit");
        assert_eq!(cpu.pc(), 4, "{name} did not advance only one insn");
        assert!(!cpu.is_halted(), "{name} executed the following IDLE");
        assert!(cpu.take_tb_flush(), "{name} did not request TB flush");
    }
}

#[test]
fn task25_exec_loop_invtlb_invalidates_fast_tlb_before_next_lookup() {
    let code_va = 0x9000_0000_0000_0000;
    let handler_off = 0x40usize;
    let data_pa = 0x80usize;
    let data_va = 0x0000_0000_4444_0080;
    let data_value = 0xD00D_F00D_CAFE_BEEF;
    let second_load_sentinel = 0x1234_5678_9ABC_DEF0;
    let asid = 0x22_u16;
    let ps = 14;
    let mut code = vec![0x0340_0000; data_pa / 4 + 2];
    code[0] = r2_si12(OP_LD_D, 0, 5, 6);
    code[1] = r3_insn(INVTLB_OP, 5, 8, 5);
    code[2] = r2_si12(OP_LD_D, 0, 5, 7);
    code[3] = code15_insn(IDLE_OP, 0);
    code[handler_off / 4] = code15_insn(IDLE_OP, 0);
    code[data_pa / 4] = data_value as u32;
    code[data_pa / 4 + 1] = (data_value >> 32) as u32;

    let mut cpu = LoongArchCpu::new();
    cpu.set_pc(code_va);
    cpu.write_gpr(5, data_va);
    cpu.write_gpr(7, second_load_sentinel);
    cpu.write_gpr(8, u64::from(asid));
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_DMW0, dmw64(0x9, 0, 1));
    assert!(cpu.take_tb_flush());
    cpu.csr_write(CSR_ASID, u64::from(asid));
    cpu.csr_write(CSR_STLBPS, ps as u64);
    cpu.csr_write(CSR_TLBRENTRY, code_va + handler_off as u64);
    write_test_tlb_entry(
        &mut cpu,
        mmu::mtlb_flat_index(4).unwrap(),
        data_va,
        ps,
        asid,
        tlbelo_test(0, true, true, 0, 0, false, false),
        tlbelo_test(0x20, true, true, 0, 0, false, false),
    );

    let mut sys = full_system_cpu_with_code(cpu, &code);
    let mut env = loongarch_soft_mmu_env();

    let exit = unsafe { cpu_exec_loop_env(&mut env, &mut sys) };

    assert_eq!(exit, ExitReason::Halted);
    assert_eq!(sys.cpu.read_gpr(6), data_value);
    assert_eq!(sys.cpu.read_gpr(7), second_load_sentinel);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRERA) & 1, 1);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRERA) & !0x3, code_va + 8);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRBADV), data_va);
    assert_eq!(sys.cpu.pc(), code_va + handler_off as u64 + 4);
}

#[test]
fn task25_exec_loop_tlb_maintenance_flushes_stale_fetch_tb() {
    let control_va = 0x9000_0000_0000_0000;
    let handler_off = 0x40usize;
    let target_pa = 0x100usize;
    let target_va = 0x0000_0000_5555_0100;
    let asid = 0x22_u16;
    let ps = 14;
    let mut code = vec![0x0340_0000; target_pa / 4 + 1];
    code[0] = r3_insn(INVTLB_OP, 9, 8, 5);
    code[1] = r2_si16(OP_JIRL, 0, 10, 0);
    code[handler_off / 4] = code15_insn(IDLE_OP, 0);
    code[target_pa / 4] = code15_insn(IDLE_OP, 0);

    let mut cpu = LoongArchCpu::new();
    cpu.set_pc(target_va);
    cpu.write_gpr(8, u64::from(asid));
    cpu.write_gpr(9, target_va);
    cpu.write_gpr(10, target_va);
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_DMW0, dmw64(0x9, 0, 1));
    cpu.csr_write(CSR_ASID, u64::from(asid));
    cpu.csr_write(CSR_STLBPS, ps as u64);
    cpu.csr_write(CSR_TLBRENTRY, control_va + handler_off as u64);
    write_test_tlb_entry(
        &mut cpu,
        mmu::mtlb_flat_index(6).unwrap(),
        target_va,
        ps,
        asid,
        tlbelo_test(0, true, true, 0, 0, false, false),
        tlbelo_test(0x10, true, true, 0, 0, false, false),
    );
    let _ = cpu.take_tb_flush();

    let mut sys = full_system_cpu_with_code(cpu, &code);
    let mut env = loongarch_soft_mmu_env();

    let warm = unsafe { cpu_exec_loop_env(&mut env, &mut sys) };
    assert_eq!(warm, ExitReason::Halted);
    assert_eq!(sys.cpu.pc(), target_va + 4);

    sys.cpu.set_pc(control_va);
    let after_invtlb = unsafe { cpu_exec_loop_env(&mut env, &mut sys) };

    assert_eq!(after_invtlb, ExitReason::Halted);
    assert!(!sys.cpu.mmu().mtlb[6].valid);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRERA) & 1, 1);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRERA) & !0x3, target_va);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRBADV), target_va);
    assert_eq!(sys.cpu.pc(), control_va + handler_off as u64 + 4);
}

#[test]
fn task82_ibar_flushes_stale_translations_after_self_modifying_store() {
    let target = 0x40usize;
    let old_target_insn = r2_si12(OP_ADDI_D, 1, 0, 4);
    let new_target_insn = r2_si12(OP_ADDI_D, 2, 0, 4);

    let mut code = vec![code15_insn(IDLE_OP, 0); target / 4 + 2];
    code[0] = r2_si12(OP_ST_W, 0, 12, 11);
    code[1] = code15_insn(IBAR_OP, 0);
    code[2] = r2_si16(OP_JIRL, 0, 10, 0);
    code[target / 4] = old_target_insn;
    code[target / 4 + 1] = code15_insn(IDLE_OP, 0);

    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_CRMD, CRMD_DA);
    cpu.set_pc(target as u64);

    let mut sys = full_system_cpu_with_code(cpu, &code);
    let mut env = loongarch_soft_mmu_env();

    let warm = unsafe { cpu_exec_loop_env(&mut env, &mut sys) };
    assert_eq!(warm, ExitReason::Halted);
    assert_eq!(sys.cpu.read_gpr(4), 1);

    sys.cpu.set_pc(0);
    sys.cpu.set_halted_flag(false);
    sys.cpu.write_gpr(4, 0);
    sys.cpu.write_gpr(10, target as u64);
    sys.cpu.write_gpr(11, u64::from(new_target_insn));
    sys.cpu.write_gpr(12, target as u64);

    let after_ibar = unsafe { cpu_exec_loop_env(&mut env, &mut sys) };

    assert_eq!(after_ibar, ExitReason::Halted);
    assert_eq!(code[target / 4], new_target_insn);
    assert_eq!(sys.cpu.read_gpr(4), 2);
}

#[test]
fn task26_lddir_ldpte_walks_two_three_four_level_tables() {
    let configs: &[(&str, &[u8], u64)] = &[
        ("two-level", &[1], 0x0000_0000_0002_5120),
        ("three-level", &[2, 1], 0x0000_0000_000E_5120),
        ("four-level", &[4, 3, 2, 1], 0x0000_0000_052E_5120),
    ];

    for (name, levels, va) in configs {
        let mut ram = vec![0_u8; 0x9000];
        let mut cpu = LoongArchCpu::new();
        attach_page_walk_ram(&mut cpu, &ram);
        cpu.csr_write(CSR_PWCL, pwcl(12, 3, 15, 3, 18, 3));
        cpu.csr_write(CSR_PWCH, pwch(21, 3, 24, 3));
        cpu.csr_write(CSR_TLBRBADV, *va);
        cpu.csr_write(CSR_TLBREHI, *va & !0x1FFF);

        let bases = [0x1000_u64, 0x2000, 0x3000, 0x4000, 0x5000];
        let mut base = bases[0];
        for (pos, level) in levels.iter().enumerate() {
            let next = bases[pos + 1];
            let (dir_base, dir_width) = match level {
                1 => (15, 3),
                2 => (18, 3),
                3 => (21, 3),
                4 => (24, 3),
                _ => unreachable!(),
            };
            let index = (*va >> dir_base) & ((1_u64 << dir_width) - 1);
            write_ram_u64(&mut ram, base | (index << 3), next);
            base = next;
        }

        let pte_base = base;
        let ptindex = (*va >> 12) & 0x7;
        let pair = ptindex & !1;
        let elo0 =
            tlbelo_test_with_perm(0x12340, true, true, 1, 2, true, true, false);
        let elo1 = tlbelo_test_with_perm(
            0x12350, true, false, 2, 3, true, false, true,
        );
        write_ram_u64(&mut ram, pte_base | (pair << 3), elo0);
        write_ram_u64(&mut ram, pte_base | ((pair + 1) << 3), elo1);
        attach_page_walk_ram(&mut cpu, &ram);

        let mut walked = bases[0];
        for level in *levels {
            walked = cpu.lddir(walked, u64::from(*level));
        }
        assert_eq!(walked, pte_base, "{name}");

        cpu.ldpte(walked, 0);
        cpu.ldpte(walked, 1);

        assert_eq!(cpu.csr_read(CSR_TLBRELO0), elo0, "{name}");
        assert_eq!(cpu.csr_read(CSR_TLBRELO1), elo1, "{name}");
        assert_eq!(cpu.csr_read(CSR_TLBREHI) & 0x3F, 12, "{name}");
    }
}

#[test]
fn task26_ldpte_uses_pwcl_ptbase_for_16k_pages() {
    let va = 0x0000_0000_000A_8120;
    let mut ram = vec![0_u8; 0x4000];
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_PWCL, pwcl(14, 3, 17, 3, 0, 0));
    cpu.csr_write(CSR_TLBRBADV, va);
    cpu.csr_write(CSR_TLBREHI, va & !0x1FFF);
    let pte_base = 0x1000_u64;
    let ptindex = (va >> 14) & 0x7;
    let pair = ptindex & !1;
    let elo0 = tlbelo_test(0x22000, true, true, 0, 1, false, false);
    let elo1 = tlbelo_test(0x22010, true, true, 0, 2, false, false);
    write_ram_u64(&mut ram, pte_base | (pair << 3), elo0);
    write_ram_u64(&mut ram, pte_base | ((pair + 1) << 3), elo1);
    attach_page_walk_ram(&mut cpu, &ram);

    cpu.ldpte(pte_base, 0);
    cpu.ldpte(pte_base, 1);

    assert_eq!(cpu.csr_read(CSR_TLBRELO0), elo0);
    assert_eq!(cpu.csr_read(CSR_TLBRELO1), elo1);
    assert_eq!(cpu.csr_read(CSR_TLBREHI) & 0x3F, 14);
}

#[test]
fn task26_ldpte_huge_entries_produce_large_page_sizes() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_PWCL, pwcl(12, 3, 19, 3, 28, 3));

    let huge_2m =
        tlbelo_test(0x44000, true, true, 0, 1, false, false) | (1 << 6);
    let marked_2m = cpu.lddir(huge_2m, 1);
    assert_eq!((marked_2m >> 13) & 0x3, 1);
    cpu.ldpte(marked_2m, 0);
    let even_2m = cpu.csr_read(CSR_TLBRELO0);
    cpu.ldpte(marked_2m, 1);
    assert_eq!(cpu.csr_read(CSR_TLBREHI) & 0x3F, 21);
    assert_eq!(cpu.csr_read(CSR_TLBRELO0), even_2m);
    assert_eq!(cpu.csr_read(CSR_TLBRELO1), even_2m + (1 << 21));

    let huge_1g =
        tlbelo_test(0x88000, true, true, 0, 2, false, false) | (1 << 6);
    let marked_1g = cpu.lddir(huge_1g, 2);
    assert_eq!((marked_1g >> 13) & 0x3, 2);
    cpu.ldpte(marked_1g, 0);
    let even_1g = cpu.csr_read(CSR_TLBRELO0);
    cpu.ldpte(marked_1g, 1);
    assert_eq!(cpu.csr_read(CSR_TLBREHI) & 0x3F, 30);
    assert_eq!(cpu.csr_read(CSR_TLBRELO0), even_1g);
    assert_eq!(cpu.csr_read(CSR_TLBRELO1), even_1g + (1 << 30));
}

#[test]
fn task26_invalid_walk_entry_refills_invalid_tlb_and_raises_pil() {
    let va = 0x0000_0000_0002_8120;
    let fault_pc = 0x9000_0000_0000_0200;
    let ram = vec![0_u8; 0x4000];
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_PWCL, pwcl(12, 3, 15, 3, 0, 0));
    cpu.csr_write(CSR_STLBPS, 12);
    cpu.csr_write(CSR_TLBRBADV, va);
    cpu.csr_write(CSR_TLBREHI, va & !0x1FFF);
    attach_page_walk_ram(&mut cpu, &ram);

    let walked = cpu.lddir(0x1000, 1);
    assert_eq!(walked, 0);
    cpu.ldpte(walked, 0);
    cpu.ldpte(walked, 1);
    cpu.csr_write(CSR_TLBRERA, 1);
    cpu.tlb_fill();
    cpu.csr_write(CSR_TLBRERA, 0);

    let vector = cpu
        .translate_address_or_exception(va, mmu::AccessType::Load, fault_pc)
        .unwrap_err();

    assert_eq!(vector, cpu.csr_read(CSR_EENTRY));
    assert_eq!(cpu.csr_read(CSR_ERA), fault_pc);
    assert_eq!(cpu.csr_read(CSR_BADV), va);
    assert_eq!(cpu.csr_read(CSR_TLBEHI), va & !0x1FFF);
    assert_eq!((cpu.csr_read(CSR_ESTAT) >> 16) & 0x3F, u64::from(ECODE_PIL));
}

#[test]
fn task26_exec_loop_refill_handler_walks_page_table_and_retries_load() {
    let code_va = 0x9000_0000_0000_0000;
    let handler_off = 0x40usize;
    let root_base = 0x1000usize;
    let pte_base = 0x2000usize;
    let data_pa = 0x3000usize;
    let data_va = 0x0000_0000_0002_0120;
    let data_value = 0x8877_6655_4433_2211;
    let mut code = vec![0x0340_0000; 0x4000 / 4];
    code[0] = r2_si12(OP_LD_D, 0, 5, 6);
    code[1] = code15_insn(IDLE_OP, 0);
    code[handler_off / 4] = r2_ui8(LDDIR_OP, 1, 10, 10);
    code[handler_off / 4 + 1] = r2_ui8(LDPTE_OP, 0, 10, 0);
    code[handler_off / 4 + 2] = r2_ui8(LDPTE_OP, 1, 10, 0);
    code[handler_off / 4 + 3] = TLBFILL_INSN;
    code[handler_off / 4 + 4] = ERTN_INSN;

    let dir_index = (data_va >> 15) & 0x7;
    let ptindex = (data_va >> 12) & 0x7;
    let pair = ptindex & !1;
    write_code_u64(
        &mut code,
        root_base | ((dir_index as usize) << 3),
        pte_base as u64,
    );
    write_code_u64(
        &mut code,
        pte_base | ((pair as usize) << 3),
        tlbelo_test((data_pa >> 12) as u64, true, true, 0, 0, false, false),
    );
    write_code_u64(
        &mut code,
        pte_base | (((pair + 1) as usize) << 3),
        tlbelo_test(
            ((data_pa >> 12) + 1) as u64,
            true,
            true,
            0,
            0,
            false,
            false,
        ),
    );
    write_code_u64(&mut code, data_pa + (data_va as usize & 0xFFF), data_value);

    let mut cpu = LoongArchCpu::new();
    cpu.set_pc(code_va);
    cpu.write_gpr(5, data_va);
    cpu.write_gpr(10, root_base as u64);
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_PWCL, pwcl(12, 3, 15, 3, 0, 0));
    cpu.csr_write(CSR_STLBPS, 12);
    cpu.csr_write(CSR_TLBRENTRY, code_va + handler_off as u64);
    cpu.csr_write(CSR_DMW0, dmw64(0x9, 0, 1));
    assert!(cpu.take_tb_flush());
    let mut sys = full_system_cpu_with_code(cpu, &code);
    let mut env = loongarch_soft_mmu_env();

    let exit = unsafe { cpu_exec_loop_env(&mut env, &mut sys) };

    assert_eq!(exit, ExitReason::Halted);
    assert_eq!(sys.cpu.read_gpr(6), data_value);
    assert_eq!(sys.cpu.pc(), code_va + 8);
    assert_eq!(sys.cpu.csr_read(CSR_TLBREHI) & 0x3F, 12);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRERA) & 1, 0);
    assert_ne!(
        sys.cpu.translate_address(data_va, mmu::AccessType::Load),
        mmu::TlbLookupResult::Miss
    );
}

#[test]
fn task27_ldpte_sanitizes_hw_pte_bits_and_enforces_rplv() {
    let va = 0x0000_0000_0002_0120;
    let pte_base = 0x1000_u64;
    let mut ram = vec![0_u8; 0x3000];
    let raw_pte = tlbelo_test_with_rplv(
        0x12345, true, true, 1, 2, false, false, false, true,
    ) | 0xE00;
    let expected_pte = raw_pte & HW_PTE_MASK_LA64;
    write_ram_u64(&mut ram, pte_base, raw_pte);
    write_ram_u64(&mut ram, pte_base + 8, raw_pte + (1 << 12));

    let mut cpu = LoongArchCpu::new();
    attach_page_walk_ram(&mut cpu, &ram);
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_PWCL, pwcl(12, 1, 0, 0, 0, 0));
    cpu.csr_write(CSR_STLBPS, 12);
    cpu.csr_write(CSR_ASID, 0x55);
    cpu.csr_write(CSR_TLBRBADV, va);
    cpu.csr_write(CSR_TLBREHI, va & !0x1FFF);

    cpu.ldpte(pte_base, 0);
    cpu.ldpte(pte_base, 1);

    assert_eq!(cpu.csr_read(CSR_TLBRELO0), expected_pte);
    assert_eq!(
        cpu.csr_read(CSR_TLBRELO1),
        (raw_pte + (1 << 12)) & HW_PTE_MASK_LA64
    );
    assert_eq!(cpu.csr_read(CSR_TLBRELO0) & 0xE00, 0);
    assert_ne!(cpu.csr_read(CSR_TLBRELO0) & (1 << 63), 0);

    cpu.csr_write(CSR_TLBRERA, 1);
    cpu.tlb_fill();
    cpu.csr_write(CSR_TLBRERA, 0);

    cpu.csr_write(CSR_CRMD, CRMD_PG);
    assert_eq!(
        cpu.translate_address(va, mmu::AccessType::Load),
        mmu::TlbLookupResult::PrivViolation
    );

    cpu.csr_write(CSR_CRMD, CRMD_PG | 1);
    assert_tlb_hit(
        cpu.translate_address(va, mmu::AccessType::Load),
        (0x12345 << 12) | (va & 0xFFF),
        2,
    );

    cpu.csr_write(CSR_CRMD, CRMD_PG | 2);
    assert_eq!(
        cpu.translate_address(va, mmu::AccessType::Load),
        mmu::TlbLookupResult::PrivViolation
    );
}

#[test]
fn task27_integrates_direct_map_tlb_page_sizes_asid_and_faults() {
    let mut cpu = LoongArchCpu::new();
    cpu.set_guest_base(0x1000_0000);
    cpu.set_ram_base(0);
    cpu.set_ram_end(0x9000_0000);

    let da_va = 0x0000_0000_0012_3456;
    cpu.csr_write(CSR_CRMD, CRMD_DA);
    assert_tlb_hit(
        cpu.translate_address(da_va, mmu::AccessType::Fetch),
        da_va,
        0,
    );

    let dmw_va = 0x9000_0000_0000_0120;
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_DMW0, dmw64(0x9, 0, 1));
    assert_tlb_hit(
        cpu.translate_address(dmw_va, mmu::AccessType::Load),
        0x120,
        0,
    );

    let asid = 0x44_u16;
    cpu.csr_write(CSR_DMW0, dmw64(0x9, 0, 1 << 3));
    cpu.csr_write(CSR_ASID, u64::from(asid));
    cpu.csr_write(CSR_STLBPS, 12);
    write_test_tlb_entry(
        &mut cpu,
        mmu::stlb_flat_index(mmu::stlb_set_index(dmw_va, 12).unwrap(), 0)
            .unwrap(),
        dmw_va,
        12,
        asid,
        tlbelo_test(0x321, true, true, 0, 1, false, false),
        tlbelo_test(0x322, true, true, 0, 1, false, false),
    );
    assert_tlb_hit(
        cpu.translate_address(dmw_va, mmu::AccessType::Load),
        (0x321 << 12) | (dmw_va & 0xFFF),
        1,
    );

    for (ps, va, ppn, mat) in [
        (14_u8, 0x0000_0000_0040_1234, 0x400_u64, 2_u8),
        (21, 0x0000_0000_0061_2345, 0x600, 1),
        (30, 0x0000_0000_4061_2345, 0x40000, 3),
    ] {
        let idx = mmu::mtlb_flat_index(usize::from(ps % 16)).unwrap();
        let even_ppn = ppn & !((1_u64 << (u32::from(ps) - 12)) - 1);
        write_test_tlb_entry(
            &mut cpu,
            idx,
            va,
            ps,
            asid,
            tlbelo_test(even_ppn, true, true, 0, mat, false, false),
            tlbelo_test(
                even_ppn + (1 << (u32::from(ps) - 12)),
                true,
                true,
                0,
                mat,
                false,
                false,
            ),
        );
        let selected_ppn = if (va >> ps) & 1 != 0 {
            even_ppn + (1 << (u32::from(ps) - 12))
        } else {
            even_ppn
        };
        assert_tlb_hit(
            cpu.translate_address(va, mmu::AccessType::Load),
            (selected_ppn << 12) | (va & ((1_u64 << ps) - 1)),
            mat,
        );
    }

    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_ASID, u64::from(asid));
    cpu.csr_write(CSR_STLBPS, 12);
    let asid_va = 0x0000_0000_0080_0120;
    write_test_tlb_entry(
        &mut cpu,
        mmu::mtlb_flat_index(20).unwrap(),
        asid_va,
        14,
        0x77,
        tlbelo_test(0x700, true, true, 0, 0, false, false),
        tlbelo_test(0x710, true, true, 0, 0, false, false),
    );
    cpu.csr_write(CSR_ASID, u64::from(asid));
    assert_eq!(
        cpu.translate_address(asid_va, mmu::AccessType::Load),
        mmu::TlbLookupResult::Miss
    );
    write_test_tlb_entry(
        &mut cpu,
        mmu::mtlb_flat_index(21).unwrap(),
        asid_va,
        14,
        0x77,
        tlbelo_test(0x720, true, true, 0, 0, true, false),
        tlbelo_test(0x730, true, true, 0, 0, true, false),
    );
    cpu.csr_write(CSR_ASID, u64::from(asid));
    assert_tlb_hit(
        cpu.translate_address(asid_va, mmu::AccessType::Load),
        (0x720 << 12) | (asid_va & 0x3FFF),
        0,
    );

    let fault_pc = 0x9000_0000_0000_0200;
    for (elo, access, ecode) in [
        (
            tlbelo_test(0x800, false, true, 0, 0, true, false),
            mmu::AccessType::Load,
            ECODE_PIL,
        ),
        (
            tlbelo_test(0x810, true, false, 0, 0, true, false),
            mmu::AccessType::Store,
            ECODE_PME,
        ),
        (
            tlbelo_test_with_perm(0x820, true, true, 0, 0, true, true, false),
            mmu::AccessType::Load,
            ECODE_PNR,
        ),
        (
            tlbelo_test_with_perm(0x830, true, true, 0, 0, true, false, true),
            mmu::AccessType::Fetch,
            ECODE_PNX,
        ),
        (
            tlbelo_test(0x840, true, true, 0, 0, true, false),
            mmu::AccessType::Load,
            ECODE_PPI,
        ),
    ] {
        let va = 0x0000_0000_00A0_0120 + (u64::from(ecode) << 15);
        write_test_tlb_entry(
            &mut cpu,
            mmu::mtlb_flat_index(ecode as usize).unwrap(),
            va,
            14,
            asid,
            elo,
            tlbelo_test(0x900, true, true, 0, 0, true, false),
        );
        if ecode == ECODE_PPI {
            cpu.csr_write(CSR_CRMD, CRMD_PG | 3);
        } else {
            cpu.csr_write(CSR_CRMD, CRMD_PG);
        }
        let vec = cpu
            .translate_address_or_exception(va, access, fault_pc)
            .unwrap_err();
        assert_eq!(vec, cpu.csr_read(CSR_EENTRY));
        assert_eq!(cpu.csr_read(CSR_BADV), va);
        assert_eq!((cpu.csr_read(CSR_ESTAT) >> 16) & 0x3F, u64::from(ecode));
    }
}

#[test]
fn task27_asid_and_stlbps_changes_flush_fast_cache_and_request_tb_flush() {
    let va = 0x0000_0000_0040_0120;
    let mut cpu = LoongArchCpu::new();
    cpu.set_guest_base(0x1000_0000);
    cpu.set_ram_base(0);
    cpu.set_ram_end(0x1000_0000);
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_ASID, 0x22);
    cpu.csr_write(CSR_STLBPS, 12);
    assert!(cpu.take_tb_flush());
    write_test_tlb_entry(
        &mut cpu,
        mmu::mtlb_flat_index(22).unwrap(),
        va,
        14,
        0x22,
        tlbelo_test(0x500, true, true, 0, 0, false, false),
        tlbelo_test(0x510, true, true, 0, 0, false, false),
    );
    assert!(cpu.take_tb_flush());

    assert_tlb_hit(
        cpu.translate_address_and_cache(va, mmu::AccessType::Load),
        (0x500 << 12) | (va & 0x3FFF),
        0,
    );
    assert!(cpu
        .fast_tlb_lookup_addend(va, mmu::AccessType::Load)
        .is_some());
    cpu.csr_write(CSR_ASID, 0x23);
    assert_eq!(cpu.fast_tlb_lookup_addend(va, mmu::AccessType::Load), None);
    assert!(cpu.take_tb_flush());

    cpu.csr_write(CSR_ASID, 0x22);
    assert!(cpu.take_tb_flush());
    assert_tlb_hit(
        cpu.translate_address_and_cache(va, mmu::AccessType::Load),
        (0x500 << 12) | (va & 0x3FFF),
        0,
    );
    cpu.csr_write(CSR_STLBPS, 14);
    assert_eq!(cpu.fast_tlb_lookup_addend(va, mmu::AccessType::Load), None);
    assert!(cpu.take_tb_flush());
}

#[test]
fn task27_tlbrd_asid_change_flushes_fast_cache_and_requests_tb_flush() {
    let va = 0x0000_0000_0060_0120;
    let ps = 14;
    let asid_a = 0x22_u16;
    let asid_b = 0x33_u16;
    let idx_a = mmu::mtlb_flat_index(23).unwrap();
    let idx_b = mmu::mtlb_flat_index(24).unwrap();
    let mut cpu = LoongArchCpu::new();
    cpu.set_guest_base(0x1000_0000);
    cpu.set_ram_base(0);
    cpu.set_ram_end(0x1000_0000);
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_STLBPS, ps as u64);
    write_test_tlb_entry(
        &mut cpu,
        idx_a,
        va,
        ps,
        asid_a,
        tlbelo_test(0x500, true, true, 0, 0, false, false),
        tlbelo_test(0x510, true, true, 0, 0, false, false),
    );
    write_test_tlb_entry(
        &mut cpu,
        idx_b,
        va,
        ps,
        asid_b,
        tlbelo_test(0x600, true, true, 0, 0, false, false),
        tlbelo_test(0x610, true, true, 0, 0, false, false),
    );
    cpu.csr_write(CSR_ASID, u64::from(asid_a));
    let _ = cpu.take_tb_flush();

    assert_tlb_hit(
        cpu.translate_address_and_cache(va, mmu::AccessType::Load),
        (0x500 << 12) | (va & 0x3FFF),
        0,
    );
    assert!(cpu
        .fast_tlb_lookup_addend(va, mmu::AccessType::Load)
        .is_some());

    cpu.csr_write(CSR_TLBIDX, idx_b as u64);
    assert_eq!(run_priv_la(&mut cpu, &[TLBRD_INSN]), 0);
    assert_eq!(cpu.csr_read(CSR_ASID) & 0x3FF, u64::from(asid_b));
    assert_eq!(cpu.fast_tlb_lookup_addend(va, mmu::AccessType::Load), None);
    assert!(cpu.take_tb_flush());
    assert_tlb_hit(
        cpu.translate_address(va, mmu::AccessType::Load),
        (0x600 << 12) | (va & 0x3FFF),
        0,
    );
}

#[test]
fn task27_translated_tlbrd_asid_change_stops_before_stale_same_tb_fetch() {
    let code_va = 0x0000_0000_0040_0000;
    let page_a_pa = 0x1000_usize;
    let page_b_pa = 0x3000_usize;
    let asid_a = 0x22_u16;
    let asid_b = 0x33_u16;
    let idx_a = mmu::mtlb_flat_index(31).unwrap();
    let idx_b = mmu::mtlb_flat_index(32).unwrap();
    let mut code = vec![0x0340_0000_u32; 0x5000 / 4];
    code[page_a_pa / 4] = TLBRD_INSN;
    code[page_a_pa / 4 + 1] = r2_si12(OP_ADDI_D, 77, 0, 11);
    code[page_a_pa / 4 + 2] = code15_insn(IDLE_OP, 0);
    code[page_b_pa / 4 + 1] = r2_si12(OP_ADDI_D, 42, 0, 11);
    code[page_b_pa / 4 + 2] = code15_insn(IDLE_OP, 0);

    let mut cpu = LoongArchCpu::new();
    cpu.set_pc(code_va);
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_STLBPS, 12);
    write_test_tlb_entry(
        &mut cpu,
        idx_a,
        code_va,
        12,
        asid_a,
        tlbelo_test((page_a_pa >> 12) as u64, true, true, 0, 0, false, false),
        tlbelo_test(0, false, false, 0, 0, false, false),
    );
    write_test_tlb_entry(
        &mut cpu,
        idx_b,
        code_va,
        12,
        asid_b,
        tlbelo_test((page_b_pa >> 12) as u64, true, true, 0, 0, false, false),
        tlbelo_test(0, false, false, 0, 0, false, false),
    );
    cpu.csr_write(CSR_ASID, u64::from(asid_a));
    cpu.csr_write(CSR_TLBIDX, idx_b as u64);
    let _ = cpu.take_tb_flush();

    let mut sys = full_system_cpu_with_code(cpu, &code);
    let mut env = loongarch_soft_mmu_env();
    let exit = unsafe { cpu_exec_loop_env(&mut env, &mut sys) };
    assert_eq!(exit, ExitReason::Halted);
    assert_eq!(sys.cpu.csr_read(CSR_ASID) & 0x3FF, u64::from(asid_b));
    assert_eq!(sys.cpu.read_gpr(11), 42);
    assert_eq!(sys.cpu.pc(), code_va + 12);
}

#[test]
fn task27_translated_tlbrd_asid_change_breaks_prepatched_chain() {
    let code_va = 0x0000_0000_0040_0000;
    let page_a_pa = 0x1000_usize;
    let page_b_pa = 0x3000_usize;
    let asid_a = 0x22_u16;
    let asid_b = 0x33_u16;
    let idx_a = mmu::mtlb_flat_index(33).unwrap();
    let idx_b = mmu::mtlb_flat_index(34).unwrap();
    let mut code = vec![0x0340_0000_u32; 0x5000 / 4];
    code[page_a_pa / 4] = TLBRD_INSN;
    code[page_a_pa / 4 + 1] = r2_si12(OP_ADDI_D, 77, 0, 11);
    code[page_a_pa / 4 + 2] = code15_insn(IDLE_OP, 0);
    code[page_b_pa / 4 + 1] = r2_si12(OP_ADDI_D, 42, 0, 11);
    code[page_b_pa / 4 + 2] = code15_insn(IDLE_OP, 0);

    let mut cpu = LoongArchCpu::new();
    cpu.set_pc(code_va);
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_STLBPS, 12);
    write_test_tlb_entry(
        &mut cpu,
        idx_a,
        code_va,
        12,
        asid_a,
        tlbelo_test((page_a_pa >> 12) as u64, true, true, 0, 0, false, false),
        tlbelo_test(0, false, false, 0, 0, false, false),
    );
    write_test_tlb_entry(
        &mut cpu,
        idx_b,
        code_va,
        12,
        asid_b,
        tlbelo_test((page_b_pa >> 12) as u64, true, true, 0, 0, false, false),
        tlbelo_test(0, false, false, 0, 0, false, false),
    );
    cpu.csr_write(CSR_ASID, u64::from(asid_a));
    cpu.csr_write(CSR_TLBIDX, idx_a as u64);
    let _ = cpu.take_tb_flush();

    let mut sys = full_system_cpu_with_code(cpu, &code);
    let mut env = loongarch_soft_mmu_env();
    let exit = unsafe { cpu_exec_loop_env(&mut env, &mut sys) };
    assert_eq!(exit, ExitReason::Halted);
    assert_eq!(sys.cpu.csr_read(CSR_ASID) & 0x3FF, u64::from(asid_a));
    assert_eq!(sys.cpu.read_gpr(11), 77);
    assert_eq!(sys.cpu.pc(), code_va + 12);

    sys.cpu.set_halted_flag(false);
    sys.cpu.reset_exit_request();
    sys.cpu.set_pc(code_va);
    sys.cpu.write_gpr(11, 0);
    sys.cpu.csr_write(CSR_TLBIDX, idx_b as u64);

    let exit = unsafe { cpu_exec_loop_env(&mut env, &mut sys) };
    assert_eq!(exit, ExitReason::Halted);
    assert_eq!(sys.cpu.csr_read(CSR_ASID) & 0x3FF, u64::from(asid_b));
    assert_eq!(sys.cpu.read_gpr(11), 42);
    assert_eq!(sys.cpu.pc(), code_va + 12);
}

#[test]
fn task27_translated_refill_sanitizes_pte_and_enforces_rplv() {
    let code_va = 0x9000_0000_0000_0000;
    let handler_off = 0x40usize;
    let root_base = 0x1000usize;
    let pte_base = 0x2000usize;
    let data_pa = 0x3000usize;
    let data_va = 0x0000_0000_0002_0120;
    let data_value = 0x1234_5678_9ABC_DEF0;
    let raw_pte = tlbelo_test_with_rplv(
        (data_pa >> 12) as u64,
        true,
        true,
        1,
        2,
        false,
        false,
        false,
        true,
    ) | 0xE00;
    let expected_pte = raw_pte & HW_PTE_MASK_LA64;
    let mut code = vec![0x0340_0000; 0x4000 / 4];
    code[0] = r2_si12(OP_LD_D, 0, 5, 6);
    code[1] = code15_insn(IDLE_OP, 0);
    code[handler_off / 4] = r2_ui8(LDDIR_OP, 1, 10, 10);
    code[handler_off / 4 + 1] = r2_ui8(LDPTE_OP, 0, 10, 0);
    code[handler_off / 4 + 2] = r2_ui8(LDPTE_OP, 1, 10, 0);
    code[handler_off / 4 + 3] = TLBFILL_INSN;
    code[handler_off / 4 + 4] = ERTN_INSN;

    let dir_index = (data_va >> 15) & 0x7;
    let ptindex = (data_va >> 12) & 0x7;
    let pair = ptindex & !1;
    write_code_u64(
        &mut code,
        root_base | ((dir_index as usize) << 3),
        pte_base as u64,
    );
    write_code_u64(&mut code, pte_base | ((pair as usize) << 3), raw_pte);
    write_code_u64(
        &mut code,
        pte_base | (((pair + 1) as usize) << 3),
        raw_pte + (1 << 12),
    );
    write_code_u64(&mut code, data_pa + (data_va as usize & 0xFFF), data_value);

    let mut cpu = LoongArchCpu::new();
    cpu.set_pc(code_va);
    cpu.write_gpr(5, data_va);
    cpu.write_gpr(10, root_base as u64);
    cpu.csr_write(CSR_CRMD, CRMD_PG | 1);
    cpu.csr_write(CSR_PWCL, pwcl(12, 3, 15, 3, 0, 0));
    cpu.csr_write(CSR_STLBPS, 12);
    cpu.csr_write(CSR_TLBRENTRY, code_va + handler_off as u64);
    cpu.csr_write(CSR_DMW0, dmw64(0x9, 0, 1 | (1 << 1)));
    assert!(cpu.take_tb_flush());
    let mut sys = full_system_cpu_with_code(cpu, &code);
    let mut env = loongarch_soft_mmu_env();

    let exit = unsafe { cpu_exec_loop_env(&mut env, &mut sys) };

    assert_eq!(exit, ExitReason::Halted);
    assert_eq!(sys.cpu.read_gpr(6), data_value);
    assert_eq!(sys.cpu.pc(), code_va + 8);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRELO0), expected_pte);
    assert_ne!(sys.cpu.csr_read(CSR_TLBRELO0) & (1 << 63), 0);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRELO0) & 0xE00, 0);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRERA) & 1, 0);

    sys.cpu.csr_write(CSR_CRMD, CRMD_PG | 2);
    assert_eq!(
        sys.cpu.translate_address(data_va, mmu::AccessType::Load),
        mmu::TlbLookupResult::PrivViolation
    );
}

#[test]
fn task27_cross_page_fetch_translates_next_page_at_tb_boundary() {
    let fetch_va = 0x0000_0000_0000_1FFC;
    let next_fetch_va = 0x0000_0000_0000_2000;
    let page0_pa = 0x1000usize;
    let wrong_contiguous_pa = 0x2000usize;
    let page1_pa = 0x3000usize;
    let mut code = vec![0x0340_0000; 0x5000 / 4];
    code[(page0_pa + 0xFFC) / 4] = 0x0340_0000;
    code[wrong_contiguous_pa / 4] = r2_si12(OP_ADDI_D, 99, 0, 11);
    code[wrong_contiguous_pa / 4 + 1] = code15_insn(IDLE_OP, 0);
    code[page1_pa / 4] = code15_insn(IDLE_OP, 0);

    let mut cpu = LoongArchCpu::new();
    cpu.set_pc(fetch_va);
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_STLBPS, 12);
    write_test_tlb_entry(
        &mut cpu,
        mmu::mtlb_flat_index(25).unwrap(),
        fetch_va,
        12,
        0,
        tlbelo_test(0, false, false, 0, 0, false, false),
        tlbelo_test((page0_pa >> 12) as u64, true, true, 0, 0, false, false),
    );
    write_test_tlb_entry(
        &mut cpu,
        mmu::mtlb_flat_index(26).unwrap(),
        next_fetch_va,
        12,
        0,
        tlbelo_test((page1_pa >> 12) as u64, true, true, 0, 0, false, false),
        tlbelo_test(0, false, false, 0, 0, false, false),
    );
    let mut sys = full_system_cpu_with_code(cpu, &code);
    let mut env = loongarch_soft_mmu_env();

    let exit = unsafe { cpu_exec_loop_env(&mut env, &mut sys) };

    assert_eq!(exit, ExitReason::Halted);
    assert_eq!(sys.cpu.read_gpr(11), 0);
    assert_eq!(sys.cpu.pc(), next_fetch_va + 4);
}

#[test]
fn task27_cross_page_fetch_faults_on_unmapped_next_page() {
    let code_va = 0x9000_0000_0000_0000;
    let handler_off = 0x40usize;
    let fetch_va = 0x0000_0000_0000_1FFC;
    let next_fetch_va = 0x0000_0000_0000_2000;
    let page0_pa = 0x1000usize;
    let wrong_contiguous_pa = 0x2000usize;
    let mut code = vec![0x0340_0000; 0x4000 / 4];
    code[handler_off / 4] = code15_insn(IDLE_OP, 0);
    code[(page0_pa + 0xFFC) / 4] = 0x0340_0000;
    code[wrong_contiguous_pa / 4] = r2_si12(OP_ADDI_D, 77, 0, 11);
    code[wrong_contiguous_pa / 4 + 1] = code15_insn(IDLE_OP, 0);

    let mut cpu = LoongArchCpu::new();
    cpu.set_pc(fetch_va);
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_STLBPS, 12);
    cpu.csr_write(CSR_TLBRENTRY, code_va + handler_off as u64);
    cpu.csr_write(CSR_DMW0, dmw64(0x9, 0, 1));
    write_test_tlb_entry(
        &mut cpu,
        mmu::mtlb_flat_index(27).unwrap(),
        fetch_va,
        12,
        0,
        tlbelo_test(0, false, false, 0, 0, false, false),
        tlbelo_test((page0_pa >> 12) as u64, true, true, 0, 0, false, false),
    );
    let mut sys = full_system_cpu_with_code(cpu, &code);
    let mut env = loongarch_soft_mmu_env();

    let exit = unsafe { cpu_exec_loop_env(&mut env, &mut sys) };

    assert_eq!(exit, ExitReason::Halted);
    assert_eq!(sys.cpu.read_gpr(11), 0);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRERA) & 1, 1);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRERA) & !0x3, next_fetch_va);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRBADV), next_fetch_va);
    assert_eq!(sys.cpu.pc(), code_va + handler_off as u64 + 4);
}

#[test]
fn task27_cross_page_load_uses_second_page_translation_on_slow_and_fast_paths()
{
    let code_va = 0x9000_0000_0000_0000;
    let data_va = 0x0000_0000_0000_1FFC;
    let page0_pa = 0x1000usize;
    let wrong_contiguous_pa = 0x2000usize;
    let page1_pa = 0x3000usize;
    let low = 0x5566_7788_u32;
    let high = 0x1122_3344_u32;
    let wrong_high = 0xAABB_CCDD_u32;
    let expected = u64::from(low) | (u64::from(high) << 32);
    let mut code = vec![0x0340_0000; 0x5000 / 4];
    code[0] = r2_si12(OP_LD_D, 0, 5, 6);
    code[1] = r2_si12(OP_LD_D, 0, 5, 7);
    code[2] = code15_insn(IDLE_OP, 0);
    code[(page0_pa + 0xFFC) / 4] = low;
    code[wrong_contiguous_pa / 4] = wrong_high;
    code[page1_pa / 4] = high;

    let mut cpu = LoongArchCpu::new();
    cpu.set_pc(code_va);
    cpu.write_gpr(5, data_va);
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_STLBPS, 12);
    cpu.csr_write(CSR_DMW0, dmw64(0x9, 0, 1));
    write_test_tlb_entry(
        &mut cpu,
        mmu::mtlb_flat_index(28).unwrap(),
        data_va,
        12,
        0,
        tlbelo_test(0, false, false, 0, 0, false, false),
        tlbelo_test((page0_pa >> 12) as u64, true, true, 0, 0, false, false),
    );
    write_test_tlb_entry(
        &mut cpu,
        mmu::mtlb_flat_index(29).unwrap(),
        data_va + 4,
        12,
        0,
        tlbelo_test((page1_pa >> 12) as u64, true, true, 0, 0, false, false),
        tlbelo_test(0, false, false, 0, 0, false, false),
    );
    let mut sys = full_system_cpu_with_code(cpu, &code);
    let mut env = loongarch_soft_mmu_env();

    let exit = unsafe { cpu_exec_loop_env(&mut env, &mut sys) };

    assert_eq!(exit, ExitReason::Halted);
    assert_eq!(sys.cpu.read_gpr(6), expected);
    assert_eq!(sys.cpu.read_gpr(7), expected);
}

#[test]
fn task27_cross_page_store_fault_has_no_partial_side_effect() {
    let code_va = 0x9000_0000_0000_0000;
    let handler_off = 0x40usize;
    let data_va = 0x0000_0000_0000_1FFC;
    let page0_pa = 0x1000usize;
    let wrong_contiguous_pa = 0x2000usize;
    let low_sentinel = 0x1357_9BDF_u32;
    let wrong_sentinel = 0x2468_ACE0_u32;
    let store_value = 0xCAFE_BABE_DEAD_BEEF;
    let mut code = vec![0x0340_0000; 0x4000 / 4];
    code[0] = r2_si12(OP_ST_D, 0, 5, 6);
    code[1] = code15_insn(IDLE_OP, 0);
    code[handler_off / 4] = code15_insn(IDLE_OP, 0);
    code[(page0_pa + 0xFFC) / 4] = low_sentinel;
    code[wrong_contiguous_pa / 4] = wrong_sentinel;

    let mut cpu = LoongArchCpu::new();
    cpu.set_pc(code_va);
    cpu.write_gpr(5, data_va);
    cpu.write_gpr(6, store_value);
    cpu.csr_write(CSR_CRMD, CRMD_PG);
    cpu.csr_write(CSR_STLBPS, 12);
    cpu.csr_write(CSR_TLBRENTRY, code_va + handler_off as u64);
    cpu.csr_write(CSR_DMW0, dmw64(0x9, 0, 1));
    write_test_tlb_entry(
        &mut cpu,
        mmu::mtlb_flat_index(30).unwrap(),
        data_va,
        12,
        0,
        tlbelo_test(0, false, false, 0, 0, false, false),
        tlbelo_test((page0_pa >> 12) as u64, true, true, 0, 0, false, false),
    );
    let mut sys = full_system_cpu_with_code(cpu, &code);
    let mut env = loongarch_soft_mmu_env();

    let exit = unsafe { cpu_exec_loop_env(&mut env, &mut sys) };

    assert_eq!(exit, ExitReason::Halted);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRERA) & 1, 1);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRERA) & !0x3, code_va);
    assert_eq!(sys.cpu.csr_read(CSR_TLBRBADV), data_va + 4);
    assert_eq!(sys.cpu.pc(), code_va + handler_off as u64 + 4);
    assert_eq!(sys.cpu.read_gpr(6), store_value);
    assert_eq!(code[(page0_pa + 0xFFC) / 4], low_sentinel);
    assert_eq!(code[wrong_contiguous_pa / 4], wrong_sentinel);
}

#[test]
fn tlb_write_and_search_via_cpu() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_TLBEHI, 0x8000_0000_0000_0000);
    cpu.csr_write(CSR_TLBELO0, 0x0000_0001_0000_0001); // V=1, PPN=0x10000
    cpu.csr_write(CSR_TLBELO1, 0x0000_0002_0000_0001); // V=1, PPN=0x20000
    cpu.csr_write(CSR_ASID, 0x05);
    let idx = mmu::mtlb_flat_index(0).unwrap();
    // PS=14 (16KB pages), first MTLB index in the QEMU TLBIDX space.
    cpu.csr_write(CSR_TLBIDX, (14 << 24) | idx as u64);

    cpu.tlb_write(idx);

    // Search: set TLBEHI to same VPPN
    cpu.csr_write(CSR_TLBEHI, 0x8000_0000_0000_0000);
    cpu.csr_write(CSR_ASID, 0x05);
    let found = cpu.tlb_search();
    assert_eq!(found, Some(idx));
}

#[test]
fn tlb_write_read_roundtrip() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_TLBEHI, 0x0000_1234_0000_0000);
    cpu.csr_write(CSR_TLBELO0, 0x0000_00AB_C000_0005); // V=1,D=0,PLV=1
    cpu.csr_write(CSR_TLBELO1, 0x0000_00DE_F000_0003); // V=1,D=1
    cpu.csr_write(CSR_ASID, 0x0A);
    cpu.csr_write(CSR_TLBIDX, 12 << 24 | 3); // PS=12, idx=3

    cpu.tlb_write(3);

    // Read it back
    cpu.csr_write(CSR_TLBIDX, 3);
    cpu.tlb_read(3);

    assert_eq!(cpu.csr_read(CSR_TLBEHI), 0x0000_1234_0000_0000);
    assert_eq!(cpu.csr_read(CSR_ASID) & 0x3FF, 0x0A);
}

#[test]
fn tlb_search_miss_sets_ne_bit() {
    let mut cpu = LoongArchCpu::new();
    cpu.csr_write(CSR_TLBEHI, 0xFFFF_0000_0000_0000);
    cpu.csr_write(CSR_ASID, 1);

    let found = cpu.tlb_search();
    assert_eq!(found, None);
}

#[test]
fn invtlb_op0_clears_all() {
    let mut cpu = LoongArchCpu::new();
    let idx = mmu::mtlb_flat_index(0).unwrap();
    // Write an entry
    cpu.csr_write(CSR_TLBEHI, 0x1000_0000_0000_0000);
    cpu.csr_write(CSR_TLBELO0, 1);
    cpu.csr_write(CSR_TLBELO1, 1);
    cpu.csr_write(CSR_ASID, 1);
    cpu.csr_write(CSR_TLBIDX, (12 << 24) | idx as u64);
    cpu.tlb_write(idx);

    assert!(cpu.mmu().mtlb[0].valid);

    cpu.invtlb(0, 0, 0);

    assert!(!cpu.mmu().mtlb[0].valid);
}

#[test]
fn dmw_match_basic() {
    let mut cpu = LoongArchCpu::new();
    // PLV=0 (bit 0 of DMW enables PLV0 match)
    cpu.csr_write(CSR_CRMD, 0);
    // DMW0: VSEG=0x9, PLV0=1(bit0), PSEG=0x0
    cpu.csr_write(CSR_DMW0, (0x9u64 << 60) | 1);

    let va = 0x9000_0000_1234_5678;
    let result = mmu::dmw_match(&cpu, va);
    assert_eq!(result, Some(0x0000_0000_1234_5678));
}

#[test]
fn dmw_no_match_wrong_plv() {
    let mut cpu = LoongArchCpu::new();
    // PLV=3
    cpu.csr_write(CSR_CRMD, 3);
    // DMW0: only PLV0 enabled (bit 0)
    cpu.csr_write(CSR_DMW0, (0x9u64 << 60) | 1);

    let va = 0x9000_0000_1234_5678;
    let result = mmu::dmw_match(&cpu, va);
    assert_eq!(result, None);
}

#[test]
fn iocsr_width_byte_read() {
    let mut cpu = LoongArchCpu::new();
    cpu.iocsr_write(0x1020, 0xDEAD_BEEF_1234_5678, 8);
    assert_eq!(cpu.iocsr_read(0x1020, 1), 0x78);
    assert_eq!(cpu.iocsr_read(0x1020, 2), 0x5678);
    assert_eq!(cpu.iocsr_read(0x1020, 4), 0x1234_5678);
    assert_eq!(cpu.iocsr_read(0x1020, 8), 0xDEAD_BEEF_1234_5678);
}

#[test]
fn iocsr_ipi_set_asserts_estat_is12() {
    let mut cpu = LoongArchCpu::new();
    cpu.iocsr_write(0x1004, 0x1, 4); // enable bit 0
    cpu.iocsr_write(0x1008, 0x1, 4); // set bit 0
    assert_ne!(cpu.csr_read(CSR_ESTAT) & (1 << 12), 0);
}

#[test]
fn iocsr_ipi_clear_deasserts_estat_is12() {
    let mut cpu = LoongArchCpu::new();
    cpu.iocsr_write(0x1004, 0x1, 4);
    cpu.iocsr_write(0x1008, 0x1, 4);
    assert_ne!(cpu.csr_read(CSR_ESTAT) & (1 << 12), 0);
    cpu.iocsr_write(0x100C, 0x1, 4); // clear
    assert_eq!(cpu.csr_read(CSR_ESTAT) & (1 << 12), 0);
}

#[test]
fn iocsr_ipi_masked_no_interrupt() {
    let mut cpu = LoongArchCpu::new();
    cpu.iocsr_write(0x1004, 0x0, 4); // enable=0
    cpu.iocsr_write(0x1008, 0x1, 4); // set bit 0
    assert_eq!(cpu.csr_read(CSR_ESTAT) & (1 << 12), 0);
}

#[test]
fn iocsr_partial_write_preserves_upper_bits() {
    let mut cpu = LoongArchCpu::new();
    cpu.iocsr_write(0x1020, 0xDEAD_BEEF_1234_5678, 8);
    cpu.iocsr_write(0x1020, 0xAA, 1); // byte write
    assert_eq!(cpu.iocsr_read(0x1020, 8), 0xDEAD_BEEF_1234_56AA);
}

#[test]
fn iocsr_word_write_preserves_upper_word() {
    let mut cpu = LoongArchCpu::new();
    cpu.iocsr_write(0x1020, 0xFFFF_FFFF_0000_0000, 8);
    cpu.iocsr_write(0x1020, 0x1234_5678, 4);
    assert_eq!(cpu.iocsr_read(0x1020, 8), 0xFFFF_FFFF_1234_5678);
}

#[test]
fn iocsr_ipi_send_triggers_self_ipi() {
    let mut cpu = LoongArchCpu::new();
    cpu.iocsr_write(0x1004, 0x3, 4); // enable bits 0,1
                                     // Send vector=0 to CPU 0: val = (0 << 16) | 0
    cpu.iocsr_write(0x1040, 0x0, 4);
    assert_eq!(cpu.iocsr_read(0x1000, 4), 1); // bit 0 set
    assert_ne!(cpu.csr_read(CSR_ESTAT) & (1 << 12), 0);
}

#[test]
fn iocsr_ipi_send_vector_decode() {
    let mut cpu = LoongArchCpu::new();
    cpu.iocsr_write(0x1004, 0xFF, 4); // enable bits 0-7
                                      // Send vector=3 to CPU 0: val = 3
    cpu.iocsr_write(0x1040, 3, 4);
    assert_eq!(cpu.iocsr_read(0x1000, 4) & (1 << 3), 1 << 3);
}

#[test]
fn iocsr_mailbox_high_lane() {
    let mut cpu = LoongArchCpu::new();
    cpu.iocsr_write(0x1020, 0xAAAA_BBBB, 4); // low word
    cpu.iocsr_write(0x1024, 0xCCCC_DDDD, 4); // high word
    assert_eq!(cpu.iocsr_read(0x1020, 4), 0xAAAA_BBBB);
    assert_eq!(cpu.iocsr_read(0x1024, 4), 0xCCCC_DDDD);
    assert_eq!(cpu.iocsr_read(0x1020, 8), 0xCCCC_DDDD_AAAA_BBBB);
}

#[test]
fn iocsr_ipi_send_nonzero_target_no_effect() {
    let mut cpu = LoongArchCpu::new();
    cpu.iocsr_write(0x1004, 0xFF, 4);
    // Send vector=0 to target CPU 1: val = (1 << 16) | 0
    cpu.iocsr_write(0x1040, 1 << 16, 4);
    assert_eq!(cpu.iocsr_read(0x1000, 4), 0); // no self-delivery
    assert_eq!(cpu.csr_read(CSR_ESTAT) & (1 << 12), 0);
}

#[test]
fn iocsr_mail_send_writes_all_bytes() {
    let mut cpu = LoongArchCpu::new();
    // mask=0x0 means write ALL bytes (no preserve)
    let data: u64 = 0xDEAD_BEEF;
    let byte_mask: u64 = 0x0;
    let val = (data << 32) | (byte_mask << 27);
    cpu.iocsr_write(0x1048, val, 8);
    assert_eq!(cpu.iocsr_read(0x1020, 4), 0xDEAD_BEEF);
}

#[test]
fn iocsr_mail_send_partial_mask_preserves() {
    let mut cpu = LoongArchCpu::new();
    // Seed mailbox with known value
    cpu.iocsr_write(0x1020, 0xAAAA_BBBB, 4);
    // mask=0x5 preserves bytes 0 and 2, writes bytes 1 and 3
    // data=0x1122_3344
    let data: u64 = 0x1122_3344;
    let byte_mask: u64 = 0x5; // preserve byte 0 and byte 2
    let val = (data << 32) | (byte_mask << 27);
    cpu.iocsr_write(0x1048, val, 8);
    // byte0 preserved (0xBB), byte1 written (0x33),
    // byte2 preserved (0xAA), byte3 written (0x11)
    assert_eq!(cpu.iocsr_read(0x1020, 4), 0x11AA_33BB);
}

#[test]
fn iocsr_byte_offset_read() {
    let mut cpu = LoongArchCpu::new();
    cpu.iocsr_write(0x1020, 0xDEAD_BEEF_1234_5678, 8);
    assert_eq!(cpu.iocsr_read(0x1021, 1), 0x56);
    assert_eq!(cpu.iocsr_read(0x1022, 2), 0x1234);
    assert_eq!(cpu.iocsr_read(0x1024, 4), 0xDEAD_BEEF);
}

#[test]
fn iocsr_byte_offset_write() {
    let mut cpu = LoongArchCpu::new();
    cpu.iocsr_write(0x1020, 0x0000_0000_0000_0000, 8);
    cpu.iocsr_write(0x1021, 0xAB, 1);
    assert_eq!(cpu.iocsr_read(0x1020, 8), 0x0000_0000_0000_AB00);
}

#[test]
fn iocsr_enable_byte_offset() {
    let mut cpu = LoongArchCpu::new();
    // Enable is at offset 4 within the 0x1000 block
    cpu.iocsr_write(0x1004, 0xFF, 4);
    assert_eq!(cpu.iocsr_read(0x1004, 4), 0xFF);
    // Read byte 5 (first byte of enable)
    assert_eq!(cpu.iocsr_read(0x1005, 1), 0);
}

#[test]
fn iocsr_any_send_writes_mailbox() {
    let mut cpu = LoongArchCpu::new();
    // ANY_SEND to address 0x1020 (lane 0 low word), mask=0, data=0x4242
    let data: u64 = 0x4242_4242;
    let byte_mask: u64 = 0x0;
    let addr: u64 = 0x1020;
    let val = (data << 32) | (byte_mask << 27) | addr;
    cpu.iocsr_write(0x1158, val, 8);
    assert_eq!(cpu.iocsr_read(0x1020, 4), 0x4242_4242);
}

#[test]
fn iocsr_any_send_to_high_lane() {
    let mut cpu = LoongArchCpu::new();
    // ANY_SEND to address 0x1024 (lane 0 high word)
    let data: u64 = 0xBEEF_CAFE;
    let byte_mask: u64 = 0x0;
    let addr: u64 = 0x1024;
    let val = (data << 32) | (byte_mask << 27) | addr;
    cpu.iocsr_write(0x1158, val, 8);
    assert_eq!(cpu.iocsr_read(0x1024, 4), 0xBEEF_CAFE);
}

#[test]
fn iocsr_any_send_to_set_register() {
    let mut cpu = LoongArchCpu::new();
    cpu.iocsr_write(0x1004, 0xFF, 4); // enable
                                      // ANY_SEND to CORE_SET (0x1008), data=0x01, mask=0
    let data: u64 = 0x0000_0001;
    let addr: u64 = 0x1008;
    let val = (data << 32) | addr;
    cpu.iocsr_write(0x1158, val, 8);
    assert_eq!(cpu.iocsr_read(0x1000, 4) & 1, 1);
    assert_ne!(cpu.csr_read(CSR_ESTAT) & (1 << 12), 0);
}

#[test]
fn iocsr_any_send_to_ipi_send_register() {
    let mut cpu = LoongArchCpu::new();
    cpu.iocsr_write(0x1004, 0xFF, 4); // enable
                                      // ANY_SEND to 0x1040 (IPI_SEND), data encodes target=0, vector=2
    let data: u64 = 2; // target=0(bits[25:16]=0), vector=2(bits[4:0]=2)
    let addr: u64 = 0x1040;
    let val = (data << 32) | addr;
    cpu.iocsr_write(0x1158, val, 8);
    assert_eq!(cpu.iocsr_read(0x1000, 4) & (1 << 2), 1 << 2);
    assert_ne!(cpu.csr_read(CSR_ESTAT) & (1 << 12), 0);
}

#[test]
fn iocsr_any_send_to_ipi_send_nonzero_target() {
    let mut cpu = LoongArchCpu::new();
    cpu.iocsr_write(0x1004, 0xFF, 4);
    // ANY_SEND to 0x1040, merged data has target=1, vector=2
    let data: u64 = (1 << 16) | 2; // target=1, vector=2
    let addr: u64 = 0x1040;
    let val = (data << 32) | addr;
    cpu.iocsr_write(0x1158, val, 8);
    // Should NOT self-deliver because target != 0
    assert_eq!(cpu.iocsr_read(0x1000, 4), 0);
    assert_eq!(cpu.csr_read(CSR_ESTAT) & (1 << 12), 0);
}

#[test]
fn ipi_pending_transition_wakes_halted() {
    let mut cpu = LoongArchCpu::new();
    cpu.set_halted_flag(true);
    cpu.iocsr_write(0x1004, 0x1, 4); // enable bit 0
    cpu.iocsr_write(0x1008, 0x1, 4); // set bit 0 → pending
    assert!(!cpu.is_halted());
}

#[test]
fn ipi_pending_transition_requests_chain_break() {
    let mut cpu = LoongArchCpu::new();
    cpu.iocsr_write(0x1004, 0x1, 4);
    cpu.iocsr_write(0x1008, 0x1, 4);
    // neg_align should be -1 after pending transition
    assert_eq!(cpu.neg_align_val(), -1);
}

#[test]
fn iocsr_status_write_is_noop() {
    let mut cpu = LoongArchCpu::new();
    cpu.iocsr_write(0x1004, 0x1, 4); // enable
    cpu.iocsr_write(0x1008, 0x1, 4); // set → status=1
    assert_eq!(cpu.iocsr_read(0x1000, 4), 1);
    cpu.iocsr_write(0x1000, 0xFF, 4); // write to status: no-op
    assert_eq!(cpu.iocsr_read(0x1000, 4), 1);
}

#[test]
fn iocsr_set_byte_offset() {
    let mut cpu = LoongArchCpu::new();
    // Byte write to set register at offset 1 (0x1009)
    cpu.iocsr_write(0x1009, 0x01, 1); // sets bit 8 in status
    assert_eq!(cpu.iocsr_read(0x1000, 4) & 0x100, 0x100);
}

#[test]
fn iocsr_clear_byte_offset() {
    let mut cpu = LoongArchCpu::new();
    cpu.iocsr_write(0x1008, 0xFFFF, 4); // set bits 0-15
    assert_eq!(cpu.iocsr_read(0x1000, 4), 0xFFFF);
    // Clear byte at offset 1 (0x100D) = clear bits 8-15
    cpu.iocsr_write(0x100D, 0xFF, 1);
    assert_eq!(cpu.iocsr_read(0x1000, 4), 0x00FF);
}
