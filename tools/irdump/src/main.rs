//! tcg-irdump — static ELF → IR dump tool.
//!
//! Reads a guest ELF binary, translates it TB-by-TB into TCG IR,
//! and prints the IR in a human-readable format.

mod elf;

use std::env;
use std::fs;
use std::io::{self, BufWriter, Write};
use std::process;

use machina_core::context::Context;
use machina_core::dump::dump_ops_with;
use machina_core::serialize;
use machina_core::TempIdx;
use machina_frontend::riscv::cpu::NUM_GPRS;
use machina_frontend::riscv::ext::RiscvCfg;
use machina_frontend::riscv::{RiscvDisasContext, RiscvTranslator};
use machina_frontend::{translator_loop, DisasJumpType, TranslatorOps};

const EM_RISCV: u16 = 243;

#[derive(Clone, Copy, PartialEq)]
enum Arch {
    Riscv64,
}

impl Arch {
    fn from_name(s: &str) -> Option<Arch> {
        match s {
            "riscv64" => Some(Arch::Riscv64),
            _ => None,
        }
    }

    fn from_e_machine(em: u16) -> Option<Arch> {
        match em {
            EM_RISCV => Some(Arch::Riscv64),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Arch::Riscv64 => "riscv64",
        }
    }
}

struct Args {
    elf_path: String,
    arch: Option<String>,
    output: Option<String>,
    emit_bin: Option<String>,
    start: Option<u64>,
    count: Option<usize>,
    max_insns: u32,
}

const USAGE: &str = "\
usage: tcg-irdump <elf> [options]

Options:
  --arch <name>      Guest architecture (default: auto)
  -o <file>          Output to file
  --emit-bin <file>  Emit binary .tcgir file
  --start <hex>      Start address
  --count <n>        Max TBs to translate
  --max-insns <n>    Max insns per TB (default: 512)
  -h, --help         Show this help

Supported architectures: riscv64";

fn parse_args() -> Args {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 || args[1] == "--help" || args[1] == "-h" {
        eprintln!("{USAGE}");
        process::exit(if args.len() < 2 { 1 } else { 0 });
    }

    let mut a = Args {
        elf_path: args[1].clone(),
        arch: None,
        output: None,
        emit_bin: None,
        start: None,
        count: None,
        max_insns: 512,
    };

    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--arch" => {
                i += 1;
                a.arch = Some(args[i].clone());
            }
            "-o" => {
                i += 1;
                a.output = Some(args[i].clone());
            }
            "--emit-bin" => {
                i += 1;
                a.emit_bin = Some(args[i].clone());
            }
            "--start" => {
                i += 1;
                let s = args[i].trim_start_matches("0x");
                a.start = Some(
                    u64::from_str_radix(s, 16).expect("invalid hex address"),
                );
            }
            "--count" => {
                i += 1;
                a.count = Some(args[i].parse().expect("invalid count"));
            }
            "--max-insns" => {
                i += 1;
                a.max_insns = args[i].parse().expect("invalid max-insns");
            }
            other => {
                eprintln!("unknown option: {other}");
                process::exit(1);
            }
        }
        i += 1;
    }
    a
}

/// Build a flat guest memory image from ELF segments.
/// Returns (base_addr, image_buffer).
fn build_image(info: &elf::ElfInfo) -> (u64, Vec<u8>) {
    let exec_segs: Vec<&elf::Segment> =
        info.segments.iter().filter(|s| s.executable).collect();
    if exec_segs.is_empty() {
        eprintln!("no executable segments found");
        process::exit(1);
    }

    let lo = exec_segs.iter().map(|s| s.vaddr).min().unwrap();
    let hi = exec_segs
        .iter()
        .map(|s| s.vaddr + s.data.len() as u64)
        .max()
        .unwrap();

    let size = (hi - lo) as usize;
    let mut image = vec![0u8; size];
    for seg in &exec_segs {
        let off = (seg.vaddr - lo) as usize;
        let len = seg.data.len();
        image[off..off + len].copy_from_slice(&seg.data);
    }
    (lo, image)
}

fn insn_annotation_riscv64(
    pc: u64,
    guest_base: *const u8,
    w: &mut dyn Write,
) -> io::Result<()> {
    unsafe {
        let ptr = guest_base.add(pc as usize);
        let half = (ptr as *const u16).read_unaligned();
        let len = if half & 0x3 != 0x3 { 2 } else { 4 };
        let data = std::slice::from_raw_parts(ptr, len);
        let (asm, _) = machina_disas::riscv::print_insn_riscv64(pc, data);
        if len == 2 {
            write!(w, "  {half:04x}      {asm}")
        } else {
            let insn = (ptr as *const u32).read_unaligned();
            write!(w, "  {insn:08x}  {asm}")
        }
    }
}

/// Translate one TB starting at `pc` and dump its IR.
fn translate_tb(
    arch: Arch,
    ir: &mut Context,
    pc: u64,
    guest_base: *const u8,
    max_insns: u32,
    w: &mut impl Write,
) -> (u64, DisasJumpType) {
    match arch {
        Arch::Riscv64 => translate_tb_riscv64(ir, pc, guest_base, max_insns, w),
    }
}

fn translate_tb_riscv64(
    ir: &mut Context,
    pc: u64,
    guest_base: *const u8,
    max_insns: u32,
    w: &mut impl Write,
) -> (u64, DisasJumpType) {
    let cfg = RiscvCfg::default();
    if ir.nb_globals() == 0 {
        // First TB — register globals via translator_loop.
        let mut d = RiscvDisasContext::new(pc, guest_base, cfg);
        d.base.max_insns = max_insns;
        translator_loop::<RiscvTranslator>(&mut d, ir);
        let gb = guest_base;
        dump_ops_with(ir, w, |pc, w| insn_annotation_riscv64(pc, gb, w))
            .expect("write failed");
        (d.base.pc_next, d.base.is_jmp)
    } else {
        // Subsequent TBs — globals already registered.
        ir.reset();
        let mut d = RiscvDisasContext::new(pc, guest_base, cfg);
        d.base.max_insns = max_insns;
        d.env = TempIdx(0);
        for i in 0..NUM_GPRS {
            d.gpr[i] = TempIdx(1 + i as u32);
        }
        d.pc = TempIdx(1 + NUM_GPRS as u32);
        d.load_res = TempIdx(1 + NUM_GPRS as u32 + 1);
        d.load_val = TempIdx(1 + NUM_GPRS as u32 + 2);
        RiscvTranslator::tb_start(&mut d, ir);
        loop {
            RiscvTranslator::insn_start(&mut d, ir);
            RiscvTranslator::translate_insn(&mut d, ir);
            if d.base.is_jmp != DisasJumpType::Next {
                break;
            }
            if d.base.num_insns >= d.base.max_insns {
                d.base.is_jmp = DisasJumpType::TooMany;
                break;
            }
        }
        RiscvTranslator::tb_stop(&mut d, ir);
        let gb = guest_base;
        dump_ops_with(ir, w, |pc, w| insn_annotation_riscv64(pc, gb, w))
            .expect("write failed");
        (d.base.pc_next, d.base.is_jmp)
    }
}

fn main() {
    let args = parse_args();

    let data = fs::read(&args.elf_path).unwrap_or_else(|e| {
        let p = &args.elf_path;
        eprintln!("failed to read {p}: {e}");
        process::exit(1);
    });

    let info = elf::parse(&data).unwrap_or_else(|e| {
        eprintln!("ELF parse error: {e}");
        process::exit(1);
    });

    // Resolve architecture: --arch flag takes priority, otherwise
    // auto-detect from ELF e_machine.
    let arch = if let Some(ref name) = args.arch {
        Arch::from_name(name).unwrap_or_else(|| {
            eprintln!("unsupported architecture: {name}");
            process::exit(1);
        })
    } else {
        Arch::from_e_machine(info.e_machine).unwrap_or_else(|| {
            let em = info.e_machine;
            eprintln!(
                "unknown ELF e_machine {em}, \
                 use --arch to specify"
            );
            process::exit(1);
        })
    };

    eprintln!("arch: {}", arch.name());

    let (base_addr, image) = build_image(&info);
    let image_end = base_addr + image.len() as u64;
    // guest_base: host pointer such that guest_base + vaddr
    // points to the right byte in `image`.
    let guest_base = image.as_ptr().wrapping_sub(base_addr as usize);

    let start_pc = args.start.unwrap_or(info.entry);
    let max_count = args.count.unwrap_or(usize::MAX);

    let mut out: Box<dyn Write> = match &args.output {
        Some(path) => {
            let f = fs::File::create(path).unwrap_or_else(|e| {
                eprintln!("cannot create {path}: {e}");
                process::exit(1);
            });
            Box::new(BufWriter::new(f))
        }
        None => Box::new(BufWriter::new(io::stdout().lock())),
    };

    let mut ir = Context::new();
    let mut pc = start_pc;
    let mut tb_count = 0usize;

    // Binary output: collect contexts, write at end.
    let mut bin_contexts: Vec<Context> = Vec::new();
    let emit_bin = args.emit_bin.is_some();

    while pc >= base_addr && pc < image_end && tb_count < max_count {
        writeln!(out, "TB #{tb_count} @ 0x{pc:x}").expect("write failed");
        let (next_pc, _) = translate_tb(
            arch,
            &mut ir,
            pc,
            guest_base,
            args.max_insns,
            &mut out,
        );
        writeln!(out).expect("write failed");

        if emit_bin {
            // Snapshot current context for serialization.
            // Re-create from raw parts to capture this TB.
            let ctx_snap = Context::from_raw_parts(
                ir.temps().to_vec(),
                ir.ops().to_vec(),
                ir.labels().to_vec(),
                ir.nb_globals(),
            );
            bin_contexts.push(ctx_snap);
        }

        tb_count += 1;
        pc = next_pc;
    }

    if let Some(ref path) = args.emit_bin {
        let f = fs::File::create(path).unwrap_or_else(|e| {
            eprintln!("cannot create {path}: {e}");
            process::exit(1);
        });
        let mut bw = BufWriter::new(f);
        for ctx in &bin_contexts {
            serialize::serialize(ctx, &mut bw).expect("serialize failed");
        }
        bw.flush().expect("flush failed");
        eprintln!("wrote {} TB(s) to {path}", bin_contexts.len());
    }
}
