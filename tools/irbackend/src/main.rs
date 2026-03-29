//! tcg-irbackend — IR → x86-64 backend code generation tool.
//!
//! Reads a .tcgir binary IR file, runs the backend pipeline
//! (optimize → liveness → regalloc → codegen), and outputs
//! the generated x86-64 machine code.

use std::env;
use std::fs;
use std::io::{self, BufWriter, Write};
use std::process;

use machina_backend::code_buffer::CodeBuffer;
use machina_backend::translate::translate;
use machina_backend::{HostCodeGen, X86_64CodeGen};
use machina_core::serialize;

struct Args {
    ir_path: String,
    output: Option<String>,
    raw: bool,
    disas: bool,
}

const USAGE: &str = "\
usage: tcg-irbackend <ir-file> [options]

Options:
  -o <file>   Output to file (default: stdout)
  --raw       Output raw machine code bytes
  --disas     Disassemble via objdump
  -h, --help  Show this help";

fn parse_args() -> Args {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 || args[1] == "--help" || args[1] == "-h" {
        eprintln!("{USAGE}");
        process::exit(if args.len() < 2 { 1 } else { 0 });
    }

    let mut a = Args {
        ir_path: args[1].clone(),
        output: None,
        raw: false,
        disas: false,
    };

    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                i += 1;
                a.output = Some(args[i].clone());
            }
            "--raw" => a.raw = true,
            "--disas" => a.disas = true,
            other => {
                eprintln!("unknown option: {other}");
                process::exit(1);
            }
        }
        i += 1;
    }
    a
}

fn hex_dump(data: &[u8], w: &mut impl Write) -> io::Result<()> {
    for (i, chunk) in data.chunks(16).enumerate() {
        write!(w, "{:04x}: ", i * 16)?;
        for (j, byte) in chunk.iter().enumerate() {
            if j > 0 && j % 4 == 0 {
                write!(w, " ")?;
            }
            write!(w, " {byte:02x}")?;
        }
        writeln!(w)?;
    }
    Ok(())
}

fn disassemble(code: &[u8]) {
    let tmp = "/tmp/tcg-irbackend-tmp.bin";
    fs::write(tmp, code).expect("write tmp failed");
    let status = process::Command::new("objdump")
        .args(["-b", "binary", "-m", "i386:x86-64", "-D", tmp])
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            eprintln!("objdump exited with {s}");
        }
        Err(e) => {
            eprintln!("failed to run objdump: {e}");
        }
    }
    let _ = fs::remove_file(tmp);
}

fn main() {
    let args = parse_args();

    let data = fs::read(&args.ir_path).unwrap_or_else(|e| {
        let p = &args.ir_path;
        eprintln!("failed to read {p}: {e}");
        process::exit(1);
    });

    let mut cursor = io::Cursor::new(&data);
    let contexts = serialize::deserialize(&mut cursor).unwrap_or_else(|e| {
        eprintln!("deserialize error: {e}");
        process::exit(1);
    });

    eprintln!("loaded {} TB(s)", contexts.len());

    let mut backend = X86_64CodeGen::new();
    let mut buf = CodeBuffer::new(64 * 1024).expect("mmap failed");

    // Emit prologue + epilogue first (ExitTb needs
    // tb_ret_offset).
    backend.emit_prologue(&mut buf);
    backend.emit_epilogue(&mut buf);
    let prologue_size = buf.offset();

    for (i, mut ctx) in contexts.into_iter().enumerate() {
        backend.init_context(&mut ctx);
        backend.clear_goto_tb_offsets();
        let tb_start = translate(&mut ctx, &backend, &mut buf);
        let tb_end = buf.offset();
        let tb_size = tb_end - tb_start;
        eprintln!("TB #{i}: {tb_size} bytes @ offset 0x{tb_start:x}");
    }

    let code = &buf.as_slice()[prologue_size..];
    let total = buf.offset();
    eprintln!(
        "total: {total} bytes ({prologue_size} prologue + \
         {} TB code)",
        total - prologue_size
    );

    if args.disas {
        disassemble(buf.as_slice());
    } else if args.raw {
        let mut out: Box<dyn Write> = match &args.output {
            Some(path) => {
                let f = fs::File::create(path).unwrap_or_else(|e| {
                    eprintln!("cannot create {path}: {e}");
                    process::exit(1);
                });
                Box::new(BufWriter::new(f))
            }
            None => Box::new(io::stdout().lock()),
        };
        out.write_all(code).expect("write failed");
    } else {
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
        hex_dump(code, &mut out).expect("write failed");
    }
}
