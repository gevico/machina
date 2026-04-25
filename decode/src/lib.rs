use std::collections::BTreeMap;
use std::io::Write;

// ── Data structures ─────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct FieldSegment {
    pub pos: u32,
    pub len: u32,
    pub signed: bool,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub segments: Vec<FieldSegment>,
    pub func: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ArgSet {
    pub name: String,
    pub fields: Vec<String>,
    pub is_extern: bool,
}

#[derive(Clone, Debug)]
pub enum FieldMapping {
    FieldRef(String),
    Inline { pos: u32, len: u32, signed: bool },
    Const(i32),
}

#[derive(Clone, Debug)]
pub struct Format {
    #[allow(dead_code)]
    pub name: String,
    pub fixedbits: u32,
    pub fixedmask: u32,
    pub args_name: String,
    pub field_map: BTreeMap<String, FieldMapping>,
}

#[derive(Clone, Debug)]
pub struct Pattern {
    pub name: String,
    pub fixedbits: u32,
    pub fixedmask: u32,
    pub args_name: String,
    pub field_map: BTreeMap<String, FieldMapping>,
}

#[derive(Debug)]
pub struct Parsed {
    pub fields: BTreeMap<String, Field>,
    pub argsets: BTreeMap<String, ArgSet>,
    pub patterns: Vec<Pattern>,
}

// ── Bit-pattern parsing ─────────────────────────────────────────

pub fn is_bit_char(c: char) -> bool {
    matches!(c, '0' | '1' | '.' | '-')
}

pub fn is_bit_token(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_bit_char)
}

pub fn is_inline_field(s: &str) -> bool {
    if let Some(idx) = s.find(':') {
        let name = &s[..idx];
        let rest = &s[idx + 1..];
        !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !rest.is_empty()
            && rest.chars().all(|c| c.is_ascii_digit())
    } else {
        false
    }
}

pub struct BitPatternResult {
    pub fixedbits: u32,
    pub fixedmask: u32,
    pub inline_fields: BTreeMap<String, (u32, u32)>,
}

pub fn parse_bit_tokens(
    tokens: &[&str],
    width: u32,
) -> Result<BitPatternResult, String> {
    let mut fixedbits = 0u32;
    let mut fixedmask = 0u32;
    let mut inline_fields = BTreeMap::new();
    let mut bit_pos = width as i32 - 1;

    for tok in tokens {
        if is_bit_token(tok) {
            for c in tok.chars() {
                if bit_pos < 0 {
                    return Err(format!(
                        "bit pattern exceeds {width} bits at token '{tok}'"
                    ));
                }
                let pos = bit_pos as u32;
                match c {
                    '0' => {
                        fixedmask |= 1 << pos;
                    }
                    '1' => {
                        fixedbits |= 1 << pos;
                        fixedmask |= 1 << pos;
                    }
                    '.' | '-' => {}
                    _ => unreachable!(),
                }
                bit_pos -= 1;
            }
        } else if is_inline_field(tok) {
            let idx = tok.find(':').unwrap();
            let name = &tok[..idx];
            let len: u32 = tok[idx + 1..]
                .parse()
                .map_err(|e| format!("inline field '{tok}' has invalid length: {e}"))?;
            if bit_pos - (len as i32) + 1 < 0 {
                return Err(format!(
                    "bit pattern exceeds {width} bits at token '{tok}'"
                ));
            }
            let pos = bit_pos - len as i32 + 1;
            inline_fields.insert(name.to_string(), (pos as u32, len));
            bit_pos -= len as i32;
        } else {
            break;
        }
    }

    Ok(BitPatternResult {
        fixedbits,
        fixedmask,
        inline_fields,
    })
}

pub fn count_bit_tokens(tokens: &[&str]) -> usize {
    tokens
        .iter()
        .take_while(|t| is_bit_token(t) || is_inline_field(t))
        .count()
}

// ── Field segment parsing ──────────────────────────────────────

pub fn parse_field_segment(s: &str) -> Result<FieldSegment, String> {
    let (pos_str, rest) = s
        .split_once(':')
        .ok_or_else(|| format!("invalid segment '{s}', expected format: pos:len / pos:slen"))?;
    let signed = rest.starts_with('s');
    let len_str = if signed { &rest[1..] } else { rest };

    let pos = pos_str
        .parse()
        .map_err(|_| format!("segment '{s}': invalid position '{pos_str}'"))?;
    let len = len_str
        .parse()
        .map_err(|_| format!("segment '{s}': invalid length '{len_str}'"))?;

    Ok(FieldSegment { pos, len, signed })
}

pub fn parse_field(line: &str) -> Result<Field, String> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let name = tokens[0][1..].to_string();
    let mut segments = Vec::new();
    let mut func = None;

    for &tok in &tokens[1..] {
        if let Some(f) = tok.strip_prefix("!function=") {
            func = Some(f.to_string());
        } else {
            segments.push(parse_field_segment(tok)?);
        }
    }

    Ok(Field {
        name,
        segments,
        func,
    })
}

pub fn parse_argset(line: &str) -> Result<ArgSet, String> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let name = tokens[0][1..].to_string();
    let is_extern = tokens.last() == Some(&"!extern");
    let end = if is_extern { tokens.len() - 1 } else { tokens.len() };
    let fields = tokens[1..end].iter().map(|s| s.to_string()).collect();

    Ok(ArgSet {
        name,
        fields,
        is_extern,
    })
}

// ── Attribute parsing ─────────────────────────────────────────

fn parse_attrs(
    tokens: &[&str],
    fields: &BTreeMap<String, Field>,
) -> Result<(String, BTreeMap<String, FieldMapping>), String> {
    let mut args_name = String::new();
    let mut field_map = BTreeMap::new();

    for &tok in tokens {
        if let Some(a) = tok.strip_prefix('&') {
            args_name = a.to_string();
        } else if let Some(f) = tok.strip_prefix('%') {
            field_map.insert(f.to_string(), FieldMapping::FieldRef(f.to_string()));
        } else if let Some(idx) = tok.find('=') {
            let key = &tok[..idx];
            let val = &tok[idx + 1..];
            if let Some(f) = val.strip_prefix('%') {
                field_map.insert(key.to_string(), FieldMapping::FieldRef(f.to_string()));
            } else if let Ok(n) = val.parse() {
                field_map.insert(key.to_string(), FieldMapping::Const(n));
            } else {
                return Err(format!(
                    "invalid attribute '{tok}': only field ref or integer constant allowed"
                ));
            }
        }
    }

    Ok((args_name, field_map))
}

// ── Format / Pattern parsing ───────────────────────────────────

fn parse_format(
    line: &str,
    fields: &BTreeMap<String, Field>,
    width: u32,
) -> Result<(String, Format), String> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let name = tokens[0][1..].to_string();
    let bit_count = count_bit_tokens(&tokens[1..]);
    let bp = parse_bit_tokens(&tokens[1..1 + bit_count], width)?;
    let (args_name, mut field_map) = parse_attrs(&tokens[1 + bit_count..], fields)?;

    for (fname, &(pos, len)) in &bp.inline_fields {
        field_map.entry(fname.clone()).or_insert_with(|| FieldMapping::Inline {
            pos,
            len,
            signed: false,
        });
    }

    Ok((
        name.clone(),
        Format {
            name,
            fixedbits: bp.fixedbits,
            fixedmask: bp.fixedmask,
            args_name,
            field_map,
        },
    ))
}

fn parse_pattern(
    line: &str,
    formats: &BTreeMap<String, Format>,
    fields: &BTreeMap<String, Field>,
    auto_argsets: &mut BTreeMap<String, ArgSet>,
    width: u32,
    lineno: usize,
) -> Result<Pattern, String> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let name = tokens[0].to_string();
    let bit_count = count_bit_tokens(&tokens[1..]);
    let bp = parse_bit_tokens(&tokens[1..1 + bit_count], width)?;
    let rest = &tokens[1 + bit_count..];

    let (args_name, mut field_map, fmt_bits, fmt_mask) =
        if let Some(fmt_name) = rest.iter().find(|t| t.starts_with('@')) {
            let fmt_name = &fmt_name[1..];
            let fmt = formats
                .get(fmt_name)
                .ok_or_else(|| format!("line {}: unknown format reference @{fmt_name}", lineno + 1))?;
            (
                fmt.args_name.clone(),
                fmt.field_map.clone(),
                fmt.fixedbits,
                fmt.fixedmask,
            )
        } else {
            let (a, f) = parse_attrs(rest, fields)?;
            (a, f, 0, 0)
        };

    let fixedbits = bp.fixedbits | fmt_bits;
    let fixedmask = bp.fixedmask | fmt_mask;

    for (fname, &(pos, len)) in &bp.inline_fields {
        field_map.entry(fname.clone()).or_insert_with(|| FieldMapping::Inline {
            pos,
            len,
            signed: false,
        });
    }

    let args_name = if args_name.is_empty() && !field_map.is_empty() {
        let auto_name = format!("_auto_{}", name);
        auto_argsets
            .entry(auto_name.clone())
            .or_insert_with(|| ArgSet {
                name: auto_name.clone(),
                fields: field_map.keys().cloned().collect(),
                is_extern: false,
            });
        auto_name
    } else {
        args_name
    };

    Ok(Pattern {
        name,
        fixedbits,
        fixedmask,
        args_name,
        field_map,
    })
}

// ── Line merging ───────────────────────────────────────────────

pub fn merge_continuations(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut cont = false;

    for line in input.lines() {
        let line = line.split_once('#').map_or(line, |(l, _)| l);
        if cont {
            out.push(' ');
            out.push_str(line.trim());
        } else {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(line);
        }
        cont = out.ends_with('\\');
        if cont {
            out.pop();
            while out.ends_with(' ') {
                out.pop();
            }
        }
    }

    out
}

// ── Top-level parse ────────────────────────────────────────────

pub fn parse_with_width(input: &str, width: u32) -> Result<Parsed, String> {
    let merged = merge_continuations(input);
    let mut fields = BTreeMap::new();
    let mut argsets = BTreeMap::new();
    let mut formats = BTreeMap::new();
    let mut patterns = Vec::new();
    let mut auto_argsets = BTreeMap::new();

    for (lineno, line) in merged.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('%') {
            let f = parse_field(line)?;
            fields.insert(f.name.clone(), f);
        } else if line.starts_with('&') {
            let a = parse_argset(line)?;
            argsets.insert(a.name.clone(), a);
        } else if line.starts_with('@') {
            let (name, fmt) = parse_format(line, &fields, width)?;
            formats.insert(name, fmt);
        } else if line.starts_with('{') || line.starts_with('}') || line.starts_with('[') || line.starts_with(']') {
            continue;
        } else {
            let p = parse_pattern(
                line,
                &formats,
                &fields,
                &mut auto_argsets,
                width,
                lineno,
            )?;
            patterns.push(p);
        }
    }

    argsets.extend(auto_argsets);

    Ok(Parsed {
        fields,
        argsets,
        patterns,
    })
}

// ── Code generation ────────────────────────────────────────────

pub fn format_hex(val: u32, width: u32) -> String {
    if width <= 16 {
        format!("{val:#06x}")
    } else {
        format!("{val:#010x}")
    }
}

pub fn to_camel(s: &str) -> String {
    let mut result = String::new();
    let mut upper = true;
    for c in s.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            result.push(c.to_ascii_uppercase());
            upper = false;
        } else {
            result.push(c);
        }
    }
    result
}

fn emit_arg_structs(
    w: &mut dyn Write,
    argsets: &BTreeMap<String, ArgSet>,
) -> std::io::Result<()> {
    for a in argsets.values() {
        if a.is_extern {
            continue;
        }
        let sname = format!("Args{}", to_camel(&a.name));
        writeln!(w, "#[derive(Debug, Clone, Copy, Default)]")?;
        writeln!(w, "pub struct {sname} {{")?;
        for f in &a.fields {
            writeln!(w, "    pub {f}: i64,")?;
        }
        writeln!(w, "}}\n")?;
    }
    Ok(())
}

fn emit_extract_field(
    w: &mut dyn Write,
    field: &Field,
    width: u32,
) -> std::io::Result<()> {
    let insn_ty = if width <= 16 { "u16" } else { "u32" };
    let signed_ty = if width <= 16 { "i16" } else { "i32" };
    writeln!(w, "fn extract_{}(insn: {insn_ty}) -> i64 {{", field.name)?;

    let segs = &field.segments;
    if segs.len() == 1 {
        let s = &segs[0];
        if s.signed {
            let shift = width - s.pos - s.len;
            let rshift = width - s.len;
            writeln!(w, "    let val = ((insn as {signed_ty}) << {shift}) >> {rshift};")?;
        } else {
            let mask = (1 << s.len) - 1;
            writeln!(w, "    let val = (insn >> {}) & {mask:#x};", s.pos)?;
        }
    } else {
        let s0 = &segs[0];
        if s0.signed {
            let shift = width - s0.pos - s0.len;
            let rshift = width - s0.len;
            writeln!(w, "    let mut val = (((insn as {signed_ty}) << {shift}) >> {rshift}) as i64;")?;
        } else {
            let mask = (1 << s0.len) - 1;
            writeln!(w, "    let mut val = ((insn >> {}) & {mask:#x}) as i64;", s0.pos)?;
        }
        for s in &segs[1..] {
            let mask = (1 << s.len) - 1;
            writeln!(w, "    val = (val << {}) | ((insn >> {}) & {mask:#x}) as i64;", s.len, s.pos)?;
        }
    }

    let cast = if segs.len() == 1 { "val as i64" } else { "val" };
    if let Some(func) = &field.func {
        match func.as_str() {
            "ex_shift_1" => writeln!(w, "    {cast} << 1")?,
            "ex_shift_2" => writeln!(w, "    {cast} << 2")?,
            "ex_shift_3" => writeln!(w, "    {cast} << 3")?,
            "ex_shift_4" => writeln!(w, "    {cast} << 4")?,
            "ex_shift_12" => writeln!(w, "    {cast} << 12")?,
            "ex_rvc_register" => writeln!(w, "    {cast} + 8")?,
            "ex_sreg_register" => writeln!(w, "    [8,9,18,19,20,21,22,23][{cast} as usize & 7]")?,
            "ex_rvc_shiftli" | "ex_rvc_shiftri" => writeln!(w, "    {cast}")?,
            _ => writeln!(w, "    {cast}")?,
        }
    } else {
        writeln!(w, "    {cast}")?;
    }

    writeln!(w, "}}\n")
}

fn emit_field_expr(
    w: &mut dyn Write,
    mapping: &FieldMapping,
    width: u32,
) -> std::io::Result<()> {
    let insn_ty = if width <= 16 { "u16" } else { "u32" };
    let signed_ty = if width <= 16 { "i16" } else { "i32" };
    match mapping {
        FieldMapping::FieldRef(name) => write!(w, "extract_{name}(insn)"),
        FieldMapping::Inline { pos, len, signed } => {
            if *signed {
                let shift = width - pos - len;
                let rshift = width - len;
                write!(w, "(((insn as {signed_ty}) << {shift}) >> {rshift}) as i64")
            } else {
                let mask = (1 << len) - 1;
                write!(w, "((insn >> {pos}) & {mask:#x}) as i64")
            }
        }
        FieldMapping::Const(n) => write!(w, "{n}i64"),
    }
}

fn emit_decode_trait(
    w: &mut dyn Write,
    patterns: &[Pattern],
    argsets: &BTreeMap<String, ArgSet>,
    width: u32,
) -> std::io::Result<()> {
    let trait_name = if width <= 16 { "Decode16" } else { "Decode" };
    writeln!(w, "pub trait {trait_name}<Ir> {{")?;
    let mut seen = std::collections::HashSet::new();
    for p in patterns {
        if !seen.insert(&p.name) {
            continue;
        }
        let sname = if p.args_name.is_empty() {
            "()".to_string()
        } else {
            format!("Args{}", to_camel(&p.args_name))
        };
        writeln!(w, "    fn trans_{}(&mut self, ir: &mut Ir, args: {sname});", p.name)?;
    }
    writeln!(w, "}}\n")
}

fn emit_decode_body(
    w: &mut dyn Write,
    patterns: &[Pattern],
    width: u32,
) -> std::io::Result<()> {
    let insn_ty = if width <= 16 { "u16" } else { "u32" };
    let full_mask = if width <= 16 { 0xffff } else { 0xffff_ffff };

    for p in patterns {
        if p.fixedmask == full_mask {
            let bits = format_hex(p.fixedbits, width);
            writeln!(w, "    if insn == {bits} {{")?;
        } else {
            let mask = format_hex(p.fixedmask, width);
            let bits = format_hex(p.fixedbits, width);
            writeln!(w, "    if (insn & {mask}) == {bits} {{")?;
        }

        let a = match argsets.get(&p.args_name) {
            Some(a) => a,
            None => {
                writeln!(w, "        self.trans_{}(ir, ());", p.name)?;
                writeln!(w, "        return;")?;
                writeln!(w, "    }}")?;
                continue;
            }
        };

        if a.is_extern {
            writeln!(w, "        self.trans_{}(ir, ());", p.name)?;
        } else {
            let sname = format!("Args{}", to_camel(&p.name));
            writeln!(w, "        let args = {sname} {{")?;
            for f in &a.fields {
                if let Some(mapping) = p.field_map.get(f) {
                    write!(w, "            {f}: ")?;
                    emit_field_expr(w, mapping, width)?;
                    writeln!(w, ",")?;
                } else {
                    writeln!(w, "            {f}: 0,")?;
                }
            }
            writeln!(w, "        }};")?;
            writeln!(w, "        self.trans_{}(ir, args);", p.name)?;
        }
        writeln!(w, "        return;")?;
        writeln!(w, "    }}")?;
    }

    Ok(())
}

fn emit_decode_fn(
    w: &mut dyn Write,
    patterns: &[Pattern],
    argsets: &BTreeMap<String, ArgSet>,
    width: u32,
) -> std::io::Result<()> {
    let insn_ty = if width <= 16 { "u16" } else { "u32" };
    let trait_name = if width <= 16 { "Decode16" } else { "Decode" };
    let fn_name = if width <= 16 { "decode16" } else { "decode" };

    writeln!(w, "pub fn {fn_name}<Ir, T: {trait_name}<Ir>>(insn: {insn_ty}, ir: &mut Ir, t: &mut T) {{")?;
    emit_decode_body(w, patterns, width)?;
    writeln!(w, "}}")
}

pub fn generate_with_width(
    input: &str,
    w: &mut dyn Write,
    width: u32,
) -> Result<(), String> {
    let parsed = parse_with_width(input, width)?;
    writeln!(w, "// Auto-generated by machina-decode").map_err(|e| e.to_string())?;
    writeln!(w, "// Do not edit!\n").map_err(|e| e.to_string())?;

    emit_arg_structs(w, &parsed.argsets).map_err(|e| e.to_string())?;
    for f in parsed.fields.values() {
        emit_extract_field(w, f, width).map_err(|e| e.to_string())?;
    }
    emit_decode_trait(w, &parsed.patterns, &parsed.argsets, width).map_err(|e| e.to_string())?;
    emit_decode_fn(w, &parsed.patterns, &parsed.argsets, width).map_err(|e| e.to_string())?;

    Ok(())
}

pub fn generate(input: &str, w: &mut dyn Write) -> Result<(), String> {
    generate_with_width(input, w, 32)
}
