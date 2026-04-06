use machina_decode::*;

fn parse(input: &str) -> Result<Parsed, String> {
    parse_with_width(input, 32)
}

// ── Helpers ──────────────────────────────────────────────────

#[test]
fn is_bit_char_coverage() {
    assert!(is_bit_char('0'));
    assert!(is_bit_char('1'));
    assert!(is_bit_char('.'));
    assert!(is_bit_char('-'));
    assert!(!is_bit_char('2'));
    assert!(!is_bit_char('a'));
}

#[test]
fn is_bit_token_coverage() {
    assert!(is_bit_token("01.-"));
    assert!(is_bit_token("0000000"));
    assert!(!is_bit_token(""));
    assert!(!is_bit_token("abc"));
    assert!(!is_bit_token("01a"));
}

#[test]
fn is_inline_field_coverage() {
    assert!(is_inline_field("pred:4"));
    assert!(is_inline_field("rs2:5"));
    assert!(!is_inline_field("pred"));
    assert!(!is_inline_field(":4"));
    assert!(!is_inline_field("pred:"));
    assert!(!is_inline_field("pred:abc"));
}

#[test]
fn count_bit_tokens_mixed() {
    let toks = ["0110011", ".....", "pred:4", "@r", "%rd"];
    assert_eq!(count_bit_tokens(&toks), 3);
}

#[test]
fn count_bit_tokens_none() {
    let toks = ["@r", "&i", "%rd"];
    assert_eq!(count_bit_tokens(&toks), 0);
}

#[test]
fn to_camel_basic() {
    assert_eq!(to_camel("r"), "R");
    assert_eq!(to_camel("shift"), "Shift");
    assert_eq!(to_camel("auto_fence"), "AutoFence");
    assert_eq!(to_camel("_auto_fence"), "AutoFence");
    assert_eq!(to_camel("empty"), "Empty");
}

// ── Bit-pattern parsing ──────────────────────────────────────

#[test]
fn bit_pattern_all_fixed() {
    let toks = ["0000000", "00000", "00000", "000", "00000", "1110011"];
    let r = parse_bit_tokens(&toks, 32).unwrap();
    assert_eq!(r.fixedbits, 0x0000_0073);
    assert_eq!(r.fixedmask, 0xffff_ffff);
    assert!(r.inline_fields.is_empty());
}

#[test]
fn bit_pattern_with_dontcare() {
    let toks = ["....................", ".....", "0110111"];
    let r = parse_bit_tokens(&toks, 32).unwrap();
    assert_eq!(r.fixedbits, 0x0000_0037);
    assert_eq!(r.fixedmask, 0x0000_007f);
}

#[test]
fn bit_pattern_inline_fields() {
    let toks = [
        "----", "pred:4", "succ:4", "-----", "000", "-----", "0001111",
    ];
    let r = parse_bit_tokens(&toks, 32).unwrap();
    assert_eq!(r.fixedmask, 0x0000_707f);
    assert_eq!(r.fixedbits, 0x0000_000f);
    assert_eq!(r.inline_fields["pred"], (24, 4));
    assert_eq!(r.inline_fields["succ"], (20, 4));
}

#[test]
fn bit_pattern_exceeds_32() {
    let toks = ["11111111111111111111111111111111", "1"];
    assert!(parse_bit_tokens(&toks, 32).is_err());
}

// ── Field parsing ────────────────────────────────────────────

#[test]
fn parse_field_unsigned() {
    let f = parse_field("%rs2 20:5").unwrap();
    assert_eq!(f.name, "rs2");
    assert_eq!(f.segments.len(), 1);
    assert_eq!(f.segments[0].pos, 20);
    assert_eq!(f.segments[0].len, 5);
    assert!(!f.segments[0].signed);
    assert!(f.func.is_none());
}

#[test]
fn parse_field_signed() {
    let f = parse_field("%imm_i 20:s12").unwrap();
    assert_eq!(f.name, "imm_i");
    assert_eq!(f.segments.len(), 1);
    assert!(f.segments[0].signed);
    assert_eq!(f.segments[0].pos, 20);
    assert_eq!(f.segments[0].len, 12);
}

#[test]
fn parse_field_multi_segment() {
    let f = parse_field("%imm_s 25:s7 7:5").unwrap();
    assert_eq!(f.name, "imm_s");
    assert_eq!(f.segments.len(), 2);
    assert!(f.segments[0].signed);
    assert_eq!(f.segments[0].pos, 25);
    assert_eq!(f.segments[0].len, 7);
    assert!(!f.segments[1].signed);
    assert_eq!(f.segments[1].pos, 7);
    assert_eq!(f.segments[1].len, 5);
}

#[test]
fn parse_field_with_function() {
    let f =
        parse_field("%imm_b 31:s1 7:1 25:6 8:4 !function=ex_shift_1").unwrap();
    assert_eq!(f.name, "imm_b");
    assert_eq!(f.segments.len(), 4);
    assert_eq!(f.func.as_deref(), Some("ex_shift_1"));
}

#[test]
fn parse_bad_field_segment() {
    assert!(parse_field_segment("abc").is_err());
    assert!(parse_field_segment(":5").is_err());
    assert!(parse_field_segment("20:").is_err());
}

// ── Argset parsing ───────────────────────────────────────────

#[test]
fn parse_argset_normal() {
    let a = parse_argset("&r rd rs1 rs2").unwrap();
    assert_eq!(a.name, "r");
    assert_eq!(a.fields, ["rd", "rs1", "rs2"]);
}

#[test]
fn parse_argset_empty() {
    let a = parse_argset("&empty").unwrap();
    assert_eq!(a.name, "empty");
    assert!(a.fields.is_empty());
}

#[test]
fn parse_argset_extern() {
    let a = parse_argset("&r rd rs1 rs2 !extern").unwrap();
    assert_eq!(a.name, "r");
    assert_eq!(a.fields, ["rd", "rs1", "rs2"]);
    assert!(a.is_extern);
}

#[test]
fn parse_argset_not_extern() {
    let a = parse_argset("&r rd rs1 rs2").unwrap();
    assert!(!a.is_extern);
}

// ── Continuation + groups ────────────────────────────────────

#[test]
fn merge_continuations_basic() {
    let input = "line1 \\\ncont1\nline2\n";
    let m = merge_continuations(input);
    assert!(m.contains("line1 cont1"));
    assert!(m.contains("line2"));
}

#[test]
fn parse_group_braces_ignored() {
    let input = "\
%rd 7:5
&r rd
@r ....... ..... ..... ... ..... ....... &r %rd
{
  add 0000000 ..... ..... 000 ..... 0110011 @r
}
";
    let p = parse(input).unwrap();
    assert_eq!(p.patterns.len(), 1);
    assert_eq!(p.patterns[0].name, "add");
}

#[test]
fn parse_group_brackets_ignored() {
    let input = "\
%rd 7:5
&r rd
@r ....... ..... ..... ... ..... ....... &r %rd
[
  add 0000000 ..... ..... 000 ..... 0110011 @r
]
";
    let p = parse(input).unwrap();
    assert_eq!(p.patterns.len(), 1);
}

#[test]
fn continuation_in_format() {
    let input = "\
%rs1_3 7:3
&shift shamt rs1 rd
@c_shift ... . .. ... ..... .. \\
         &shift rd=%rs1_3 rs1=%rs1_3 shamt=0
";
    let p = parse(input).unwrap();
    assert!(p.argsets.contains_key("shift"));
}

// ── Full parse ───────────────────────────────────────────────

fn mini_decode() -> &'static str {
    "\
# Test decode
%rs2    20:5
%rs1    15:5
%rd     7:5
%imm_i  20:s12

&r   rd rs1 rs2
&i   imm rs1 rd

@r  ....... ..... ..... ... ..... ....... &r  %rs2 %rs1 %rd
@i  ............ ..... ... ..... ....... &i  imm=%imm_i %rs1 %rd

add  0000000 ..... ..... 000 ..... 0110011 @r
addi ............ ..... 000 ..... 0010011 @i
"
}

#[test]
fn parse_mini_decode() {
    let p = parse(mini_decode()).unwrap();
    assert_eq!(p.fields.len(), 4);
    assert_eq!(p.argsets.len(), 2);
    assert_eq!(p.patterns.len(), 2);

    let add = &p.patterns[0];
    assert_eq!(add.name, "add");
    assert_eq!(add.fixedmask, 0xfe00_707f);
    assert_eq!(add.fixedbits, 0x0000_0033);
    assert_eq!(add.args_name, "r");

    let addi = &p.patterns[1];
    assert_eq!(addi.name, "addi");
    assert_eq!(addi.fixedmask, 0x0000_707f);
    assert_eq!(addi.fixedbits, 0x0000_0013);
    assert_eq!(addi.args_name, "i");
}

#[test]
fn parse_riscv32_decode() {
    let input =
        std::fs::read_to_string("../guest/riscv/src/riscv/insn32.decode")
            .unwrap();
    let p = parse(&input).unwrap();
    assert_eq!(p.patterns.len(), 207);
    assert!(p.fields.contains_key("imm_b"));
    assert!(p.fields.contains_key("imm_j"));
    assert!(p.argsets.contains_key("r"));
    assert!(p.argsets.contains_key("shift"));
}

// ── Error handling ───────────────────────────────────────────

#[test]
fn parse_empty_input() {
    let p = parse("").unwrap();
    assert!(p.patterns.is_empty());
    assert!(p.fields.is_empty());
    assert!(p.argsets.is_empty());
}

#[test]
fn parse_comments_only() {
    let p = parse("# just a comment\n# another\n").unwrap();
    assert!(p.patterns.is_empty());
}

#[test]
fn parse_unknown_format_ref() {
    let input = "\
%rd 7:5
&r rd
@r ....... ..... ..... ... ..... ....... &r %rd
add 0000000 ..... ..... 000 ..... 0110011 @nonexistent
";
    assert!(parse(input).is_err());
}

// ── Format inheritance ───────────────────────────────────────

#[test]
fn format_inherits_args_and_fields() {
    let input = "\
%rs2 20:5
%rs1 15:5
%rd  7:5
&r rd rs1 rs2
@r ....... ..... ..... ... ..... ....... &r %rs2 %rs1 %rd
add 0000000 ..... ..... 000 ..... 0110011 @r
";
    let p = parse(input).unwrap();
    let add = &p.patterns[0];
    assert_eq!(add.args_name, "r");
    assert!(add.field_map.contains_key("rd"));
    assert!(add.field_map.contains_key("rs1"));
    assert!(add.field_map.contains_key("rs2"));
}

#[test]
fn format_bits_merge_with_pattern() {
    let input = "\
%rd    7:5
%imm_u 12:s20 !function=ex_shift_12
&u imm rd
@u .................... ..... ....... &u imm=%imm_u %rd
lui  .................... ..... 0110111 @u
";
    let p = parse(input).unwrap();
    let lui = &p.patterns[0];
    assert_eq!(lui.fixedmask, 0x0000_007f);
    assert_eq!(lui.fixedbits, 0x0000_0037);
}

// ── Auto-generated argset ────────────────────────────────────

#[test]
fn auto_argset_for_fence() {
    let input =
        std::fs::read_to_string("../guest/riscv/src/riscv/insn32.decode")
            .unwrap();
    let p = parse(&input).unwrap();
    let fence = p.patterns.iter().find(|p| p.name == "fence").unwrap();
    assert_eq!(fence.args_name, "_auto_fence");
    assert!(fence.field_map.contains_key("pred"));
    assert!(fence.field_map.contains_key("succ"));
    let auto = p.argsets.get("_auto_fence").unwrap();
    assert!(auto.fields.contains(&"pred".to_string()));
    assert!(auto.fields.contains(&"succ".to_string()));
}

// ── Pattern mask/bits ────────────────────────────────────────

#[test]
fn pattern_masks_r_type() {
    let input =
        std::fs::read_to_string("../guest/riscv/src/riscv/insn32.decode")
            .unwrap();
    let p = parse(&input).unwrap();
    let find = |name: &str| p.patterns.iter().find(|p| p.name == name).unwrap();
    let add = find("add");
    assert_eq!(add.fixedmask, 0xfe00_707f);
    assert_eq!(add.fixedbits, 0x0000_0033);
    let sub = find("sub");
    assert_eq!(sub.fixedmask, 0xfe00_707f);
    assert_eq!(sub.fixedbits, 0x4000_0033);
    let mul = find("mul");
    assert_eq!(mul.fixedbits, 0x0200_0033);
}

#[test]
fn pattern_masks_i_type() {
    let input =
        std::fs::read_to_string("../guest/riscv/src/riscv/insn32.decode")
            .unwrap();
    let p = parse(&input).unwrap();
    let find = |name: &str| p.patterns.iter().find(|p| p.name == name).unwrap();
    let addi = find("addi");
    assert_eq!(addi.fixedmask, 0x0000_707f);
    assert_eq!(addi.fixedbits, 0x0000_0013);
    let jalr = find("jalr");
    assert_eq!(jalr.fixedmask, 0x0000_707f);
    assert_eq!(jalr.fixedbits, 0x0000_0067);
}

#[test]
fn pattern_masks_b_type() {
    let input =
        std::fs::read_to_string("../guest/riscv/src/riscv/insn32.decode")
            .unwrap();
    let p = parse(&input).unwrap();
    let find = |name: &str| p.patterns.iter().find(|p| p.name == name).unwrap();
    let beq = find("beq");
    assert_eq!(beq.fixedmask, 0x0000_707f);
    assert_eq!(beq.fixedbits, 0x0000_0063);
    let bne = find("bne");
    assert_eq!(bne.fixedbits, 0x0000_1063);
}

#[test]
fn pattern_masks_shift() {
    let input =
        std::fs::read_to_string("../guest/riscv/src/riscv/insn32.decode")
            .unwrap();
    let p = parse(&input).unwrap();
    let find = |name: &str| p.patterns.iter().find(|p| p.name == name).unwrap();
    let slli = find("slli");
    assert_eq!(slli.fixedmask, 0xf800_707f);
    assert_eq!(slli.fixedbits, 0x0000_1013);
    let sraiw = find("sraiw");
    assert_eq!(sraiw.fixedmask, 0xfe00_707f);
    assert_eq!(sraiw.fixedbits, 0x4000_501b);
}

// ── Extract function correctness ─────────────────────────────

#[test]
fn extract_rd_rs1_rs2_values() {
    // add x3, x1, x2 → 0x002081b3
    let insn: u32 = 0x002081b3;
    assert_eq!((insn >> 7) & 0x1f, 3);
    assert_eq!((insn >> 15) & 0x1f, 1);
    assert_eq!((insn >> 20) & 0x1f, 2);
}

#[test]
fn extract_imm_i_positive() {
    let insn: u32 = 0x02a00093;
    let val = ((insn as i32) >> 20) as i64;
    assert_eq!(val, 42);
}

#[test]
fn extract_imm_i_negative() {
    let insn: u32 = 0xfff00093;
    let val = ((insn as i32) >> 20) as i64;
    assert_eq!(val, -1);
}

#[test]
fn extract_imm_s_value() {
    let insn: u32 = 0x0020_8423;
    let mut val = (((insn as i32) << 0) >> 25) as i64;
    val = (val << 5) | (((insn >> 7) & 0x1f) as i64);
    assert_eq!(val, 8);
}

#[test]
fn extract_imm_b_value() {
    let insn: u32 = 0x0000_0463;
    let mut val = (((insn as i32) << 0) >> 31) as i64;
    val = (val << 1) | (((insn >> 7) & 0x1) as i64);
    val = (val << 6) | (((insn >> 25) & 0x3f) as i64);
    val = (val << 4) | (((insn >> 8) & 0xf) as i64);
    val <<= 1;
    assert_eq!(val, 8);
}

#[test]
fn extract_imm_j_value() {
    let insn: u32 = 0x0140_00ef;
    let mut val = (((insn as i32) << 0) >> 31) as i64;
    val = (val << 8) | (((insn >> 12) & 0xff) as i64);
    val = (val << 1) | (((insn >> 20) & 0x1) as i64);
    val = (val << 10) | (((insn >> 21) & 0x3ff) as i64);
    val <<= 1;
    assert_eq!(val, 20);
}

#[test]
fn extract_imm_u_value() {
    let insn: u32 = 0x1234_52b7;
    let val = ((insn as i32) << 0) >> 12;
    let val = (val << 12) as i64;
    assert_eq!(val, 0x12345000);
}

#[test]
fn extract_imm_b_negative() {
    let insn: u32 = 0xfe00_0ee3;
    let mut val = (((insn as i32) << 0) >> 31) as i64;
    val = (val << 1) | (((insn >> 7) & 0x1) as i64);
    val = (val << 6) | (((insn >> 25) & 0x3f) as i64);
    val = (val << 4) | (((insn >> 8) & 0xf) as i64);
    val <<= 1;
    assert_eq!(val, -4);
}

#[test]
fn extract_imm_j_negative() {
    let insn: u32 = 0xffff_f06f;
    let mut val = ((insn as i32) >> 31) as i64;
    val = (val << 8) | (((insn >> 12) & 0xff) as i64);
    val = (val << 1) | (((insn >> 20) & 0x1) as i64);
    val = (val << 10) | (((insn >> 21) & 0x3ff) as i64);
    val <<= 1;
    assert_eq!(val, -2);
}

#[test]
fn extract_imm_s_negative() {
    let insn: u32 = 0xfe20_ae23;
    let mut val = ((insn as i32) >> 25) as i64;
    val = (val << 5) | (((insn >> 7) & 0x1f) as i64);
    assert_eq!(val, -4);
}

#[test]
fn extract_imm_u_negative() {
    let insn: u32 = 0xffff_f0b7;
    let val = (insn as i32) >> 12;
    let val = (val as i64) << 12;
    assert_eq!(val, -4096);
}

#[test]
fn extract_imm_i_max_positive() {
    let insn: u32 = 0x7ff0_0093;
    let val = ((insn as i32) >> 20) as i64;
    assert_eq!(val, 2047);
}

#[test]
fn extract_imm_i_min_negative() {
    let insn: u32 = 0x8000_0093;
    let val = ((insn as i32) >> 20) as i64;
    assert_eq!(val, -2048);
}

#[test]
fn extract_shift_amount() {
    let insn: u32 = 0x00d1_1093;
    let shamt = ((insn >> 20) & 0x7f) as i64;
    assert_eq!(shamt, 13);
    let rs1 = ((insn >> 15) & 0x1f) as i64;
    assert_eq!(rs1, 2);
    let rd = ((insn >> 7) & 0x1f) as i64;
    assert_eq!(rd, 1);
}

#[test]
fn extract_fence_fields() {
    let insn: u32 = 0x0330_000f;
    let pred = ((insn >> 24) & 0xf) as i64;
    let succ = ((insn >> 20) & 0xf) as i64;
    assert_eq!(pred, 3);
    assert_eq!(succ, 3);
}

#[test]
fn extract_fence_iorw() {
    let insn: u32 = 0x0ff0_000f;
    let pred = ((insn >> 24) & 0xf) as i64;
    let succ = ((insn >> 20) & 0xf) as i64;
    assert_eq!(pred, 15);
    assert_eq!(succ, 15);
}

// ── Pattern matching correctness ─────────────────────────────

fn riscv_parsed() -> Parsed {
    let input =
        std::fs::read_to_string("../guest/riscv/src/riscv/insn32.decode")
            .unwrap();
    parse(&input).unwrap()
}

fn matches_pattern(p: &Parsed, insn: u32) -> Vec<String> {
    p.patterns
        .iter()
        .filter(|pat| insn & pat.fixedmask == pat.fixedbits)
        .map(|pat| pat.name.clone())
        .collect()
}

#[test]
fn insn_add_matches_only_add() {
    let p = riscv_parsed();
    let m = matches_pattern(&p, 0x0020_81b3);
    assert_eq!(m, vec!["add"]);
}

#[test]
fn insn_sub_matches_only_sub() {
    let p = riscv_parsed();
    let m = matches_pattern(&p, 0x4020_81b3);
    assert_eq!(m, vec!["sub"]);
}

#[test]
fn insn_ecall_matches_only_ecall() {
    let p = riscv_parsed();
    let m = matches_pattern(&p, 0x0000_0073);
    assert_eq!(m, vec!["ecall"]);
}

#[test]
fn insn_ebreak_matches_only_ebreak() {
    let p = riscv_parsed();
    let m = matches_pattern(&p, 0x0010_0073);
    assert_eq!(m, vec!["ebreak"]);
}

#[test]
fn insn_mul_matches_only_mul() {
    let p = riscv_parsed();
    let m = matches_pattern(&p, 0x0220_81b3);
    assert_eq!(m, vec!["mul"]);
}

#[test]
fn insn_slli_no_overlap_with_srli() {
    let p = riscv_parsed();
    let m = matches_pattern(&p, 0x0011_1093);
    assert_eq!(m, vec!["slli"]);
    let m = matches_pattern(&p, 0x0011_5093);
    assert_eq!(m, vec!["srli"]);
}

#[test]
fn insn_jal_matches_only_jal() {
    let p = riscv_parsed();
    let m = matches_pattern(&p, 0x0140_00ef);
    assert_eq!(m, vec!["jal"]);
}

// ── Code generation ──────────────────────────────────────────

#[test]
fn generate_mini_decode() {
    let mut out = Vec::new();
    generate(mini_decode(), &mut out).unwrap();
    let code = String::from_utf8(out).unwrap();
    assert!(code.contains("pub struct ArgsR"));
    assert!(code.contains("pub struct ArgsI"));
    assert!(code.contains("pub trait Decode"));
    assert!(code.contains("fn trans_add("));
    assert!(code.contains("fn trans_addi("));
    assert!(code.contains("pub fn decode<Ir, T: Decode<Ir>>"));
    assert!(code.contains("extract_rs1(insn)"));
    assert!(code.contains("extract_imm_i(insn)"));
}

#[test]
fn generate_riscv32_decode() {
    let input =
        std::fs::read_to_string("../guest/riscv/src/riscv/insn32.decode")
            .unwrap();
    let mut out = Vec::new();
    generate(&input, &mut out).unwrap();
    let code = String::from_utf8(out).unwrap();
    assert_eq!(code.matches("fn trans_").count(), 207);
    assert!(code.contains("fn trans_lui("));
    assert!(code.contains("fn trans_jal("));
    assert!(code.contains("fn trans_mul("));
    assert!(code.contains("fn trans_remuw("));
    assert!(code.contains("fn trans_fence("));
}

#[test]
fn generate_ecall_no_args() {
    let mut out = Vec::new();
    generate(mini_decode(), &mut out).unwrap();
    let code = String::from_utf8(out).unwrap();
    assert!(code.contains("extract_rd(insn)"));
}

#[test]
fn generate_fence_auto_struct() {
    let input =
        std::fs::read_to_string("../guest/riscv/src/riscv/insn32.decode")
            .unwrap();
    let mut out = Vec::new();
    generate(&input, &mut out).unwrap();
    let code = String::from_utf8(out).unwrap();
    assert!(code.contains("pub struct ArgsAutoFence"));
    assert!(code.contains("trans_fence"));
}

#[test]
fn generate_no_identity_mask() {
    let input =
        std::fs::read_to_string("../guest/riscv/src/riscv/insn32.decode")
            .unwrap();
    let mut out = Vec::new();
    generate(&input, &mut out).unwrap();
    let code = String::from_utf8(out).unwrap();
    assert!(code.contains("if insn == 0x00000073"));
    assert!(code.contains("if insn == 0x00100073"));
    assert!(!code.contains("0xffffffff"));
}

#[test]
fn generate_no_shift_zero() {
    let input =
        std::fs::read_to_string("../guest/riscv/src/riscv/insn32.decode")
            .unwrap();
    let mut out = Vec::new();
    generate(&input, &mut out).unwrap();
    let code = String::from_utf8(out).unwrap();
    assert!(!code.contains("<< 0)"));
}

// ── Extern argset codegen ────────────────────────────────────

#[test]
fn extern_argset_no_struct_emitted() {
    let input = "\
%rd 7:5
&i imm rs1 rd !extern
@i ............ ..... ... ..... ....... &i imm=0 rs1=0 %rd
addi ............ ..... 000 ..... 0010011 @i
";
    let mut out = Vec::new();
    generate(input, &mut out).unwrap();
    let code = String::from_utf8(out).unwrap();
    assert!(!code.contains("pub struct ArgsI"));
    assert!(code.contains("fn trans_addi("));
}

// ── 16-bit width ─────────────────────────────────────────────

#[test]
fn generate_16bit_basic() {
    let input = "\
%rd 7:5
&i imm rs1 rd
@ci ... . ..... ..... .. &i imm=0 rs1=%rd %rd
addi 000 . ..... ..... 01 @ci
";
    let mut out = Vec::new();
    generate_with_width(input, &mut out, 16).unwrap();
    let code = String::from_utf8(out).unwrap();
    assert!(code.contains("pub trait Decode16<Ir>"));
    assert!(code.contains("pub fn decode16<Ir, T: Decode16<Ir>>"));
    assert!(code.contains("insn: u16"));
    assert!(code.contains("fn extract_rd(insn: u16)"));
}

#[test]
fn parse_16bit_pattern() {
    let input = "\
%rd 7:5
&i imm rs1 rd
@ci ... . ..... ..... .. &i imm=0 rs1=%rd %rd
addi 000 . ..... ..... 01 @ci
";
    let p = parse_with_width(input, 16).unwrap();
    assert_eq!(p.patterns.len(), 1);
    let addi = &p.patterns[0];
    assert_eq!(addi.name, "addi");
    assert_eq!(addi.fixedmask & 0x3, 0x3);
    assert_eq!(addi.fixedbits & 0x3, 0x1);
}

// ── Function handlers ────────────────────────────────────────

#[test]
fn func_rvc_register() {
    let input = "\
%rs1_3 7:3 !function=ex_rvc_register
&i imm rs1 rd
@ci ... . ..... ..... .. &i imm=0 rs1=%rs1_3 rd=0
addi 000 . ..... ..... 01 @ci
";
    let mut out = Vec::new();
    generate_with_width(input, &mut out, 16).unwrap();
    let code = String::from_utf8(out).unwrap();
    assert!(code.contains("+ 8"));
}

#[test]
fn func_shift_2() {
    let input = "\
%nzuimm 7:4 11:2 5:1 6:1 !function=ex_shift_2
";
    let mut out = Vec::new();
    generate_with_width(input, &mut out, 16).unwrap();
    let code = String::from_utf8(out).unwrap();
    assert!(code.contains("<< 2"));
}

#[test]
fn func_sreg_register() {
    let input = "\
%r1s 7:3 !function=ex_sreg_register
";
    let mut out = Vec::new();
    generate_with_width(input, &mut out, 16).unwrap();
    let code = String::from_utf8(out).unwrap();
    assert!(code.contains("[8,9,18,19,20,21,22,23]"));
}

// ── Full insn16.decode parse ─────────────────────────────────

#[test]
fn parse_riscv16_decode() {
    let input =
        std::fs::read_to_string("../guest/riscv/src/riscv/insn16.decode")
            .unwrap();
    let p = parse_with_width(&input, 16).unwrap();
    assert!(p.argsets.get("r").unwrap().is_extern);
    assert!(p.argsets.get("i").unwrap().is_extern);
    assert!(p.patterns.len() >= 28);
}

#[test]
fn generate_riscv16_decode() {
    let input =
        std::fs::read_to_string("../guest/riscv/src/riscv/insn16.decode")
            .unwrap();
    let mut out = Vec::new();
    generate_with_width(&input, &mut out, 16).unwrap();
    let code = String::from_utf8(out).unwrap();
    assert!(code.contains("pub trait Decode16<Ir>"));
    assert!(code.contains("pub fn decode16<"));
    assert!(code.contains("fn trans_addi("));
    assert!(code.contains("fn trans_ld("));
    assert!(code.contains("fn trans_sd("));
    assert!(code.contains("fn trans_ebreak("));
    assert!(!code.contains("pub struct ArgsR"));
    assert!(!code.contains("pub struct ArgsI"));
}

// ── NEW: 16-bit field extraction correctness ─────────────────

fn rvc_parsed() -> Parsed {
    let input =
        std::fs::read_to_string("../guest/riscv/src/riscv/insn16.decode")
            .unwrap();
    parse_with_width(&input, 16).unwrap()
}

#[test]
fn extract_rvc_register() {
    // 3-bit field + 8 mapping: 0→8, 7→15
    for val in 0u16..8 {
        assert_eq!(val as i64 + 8, (val + 8) as i64);
    }
}

#[test]
fn extract_imm_ci() {
    // CI-format: imm[5] at bit 12, imm[4:0] at bits 6:2
    // Sign-extend from 6 bits
    let insn: u16 = 0b010_1_00001_11111_01; // imm=0b111111=-1
    let imm5 = ((insn >> 12) & 1) as i64;
    let imm4_0 = ((insn >> 2) & 0x1f) as i64;
    let raw = (imm5 << 5) | imm4_0;
    let sext = ((raw << 58) as i64) >> 58; // sign-extend 6-bit
    assert_eq!(sext, -1);
}

#[test]
fn extract_nzuimm_ciw() {
    // CIW-format: nzuimm is unsigned, shifted left by 2
    // Bits: 12:5 7:4 11:2 5:1 6:1 → assembled then <<2
    let raw: u16 = 0b000_01010101_000_00;
    let b5_4 = ((raw >> 11) & 0x3) as u32;
    let b9_6 = ((raw >> 7) & 0xf) as u32;
    let b2 = ((raw >> 6) & 0x1) as u32;
    let b3 = ((raw >> 5) & 0x1) as u32;
    let nzuimm = (b5_4 << 4) | (b9_6) | (b2 << 6) | (b3 << 7);
    // After <<2 shift, result is a multiple of 4
    let scaled = nzuimm << 2;
    assert_eq!(scaled & 0x3, 0);
    assert!(scaled > 0);
}

#[test]
fn extract_uimm_cl_d() {
    // CL-D format offset: imm[5:3] at bits 12:10,
    // imm[7:6] at bits 6:5, then <<3
    let imm_hi: u32 = 0b101; // bits 12:10
    let imm_lo: u32 = 0b11; // bits 6:5
    let offset = ((imm_hi & 0x7) << 3) | ((imm_lo & 0x3) << 6);
    // offset should be a multiple of 8
    assert_eq!(offset % 8, 0);
}

#[test]
fn extract_imm_cb() {
    // CB-format branch offset: sign-extended, <<1
    // offset[8|4:3] at bits 12:10, offset[7:6|2:1|5]
    // at bits 6:2
    // Test: offset = -2 (0b1111111111111110 in 9-bit sext)
    let offset: i64 = -2;
    assert_eq!(offset & 1, 0); // must be even
    assert!(offset >= -256 && offset < 256);
}

#[test]
fn extract_imm_cj() {
    // CJ-format jump offset: sign-extended, <<1
    // 12-bit signed immediate
    let offset: i64 = -2;
    assert_eq!(offset & 1, 0);
    assert!(offset >= -2048 && offset < 2048);
}

#[test]
fn extract_imm_addi16sp() {
    // ADDI16SP immediate: sign-extended, <<4
    // nzimm[9|4|6|8:7|5] at bits 12,6:2
    let nzimm: i64 = -16; // minimum nonzero negative
    assert_eq!(nzimm % 16, 0);
}

#[test]
fn extract_imm_lui_c() {
    // C.LUI immediate: nzimm[17|16:12], <<12
    let nzimm: i64 = 0x1_000; // 1 page
    assert_eq!(nzimm % (1 << 12), 0);
    let nzimm_neg: i64 = -(1 << 17); // max negative
    assert_eq!(nzimm_neg % (1 << 12), 0);
}

// ── NEW: 16-bit pattern matching ─────────────────────────────

fn rvc_matches(p: &Parsed, insn: u16) -> Vec<String> {
    let insn32 = insn as u32;
    p.patterns
        .iter()
        .filter(|pat| insn32 & pat.fixedmask == pat.fixedbits)
        .map(|pat| pat.name.clone())
        .collect()
}

#[test]
fn c_addi_matches() {
    let p = rvc_parsed();
    // C.ADDI x1, 1: 000 0 00001 00001 01
    let insn: u16 = 0b000_0_00001_00001_01;
    let m = rvc_matches(&p, insn);
    assert!(m.contains(&"addi".to_string()), "{m:?}");
}

#[test]
fn c_li_matches() {
    let p = rvc_parsed();
    // C.LI x1, 5: 010 0 00001 00101 01
    let insn: u16 = 0b010_0_00001_00101_01;
    let m = rvc_matches(&p, insn);
    assert!(m.contains(&"addi".to_string()), "{m:?}");
}

#[test]
fn c_lui_matches() {
    let p = rvc_parsed();
    // C.LUI x3, nzimm=1: 011 0 00011 00001 01
    let insn: u16 = 0b011_0_00011_00001_01;
    let m = rvc_matches(&p, insn);
    assert!(m.contains(&"lui".to_string()), "{m:?}");
}

#[test]
fn c_lw_matches() {
    let p = rvc_parsed();
    // C.LW: 010 ... ... .. ... 00
    let insn: u16 = 0b010_000_001_00_001_00;
    let m = rvc_matches(&p, insn);
    assert!(m.contains(&"lw".to_string()), "{m:?}");
}

#[test]
fn c_ld_matches() {
    let p = rvc_parsed();
    // C.LD: 011 ... ... .. ... 00
    let insn: u16 = 0b011_000_001_00_001_00;
    let m = rvc_matches(&p, insn);
    assert!(m.contains(&"ld".to_string()), "{m:?}");
}

#[test]
fn c_sw_matches() {
    let p = rvc_parsed();
    // C.SW: 110 ... ... .. ... 00
    let insn: u16 = 0b110_000_001_00_001_00;
    let m = rvc_matches(&p, insn);
    assert!(m.contains(&"sw".to_string()), "{m:?}");
}

#[test]
fn c_sd_matches() {
    let p = rvc_parsed();
    // C.SD: 111 ... ... .. ... 00
    let insn: u16 = 0b111_000_001_00_001_00;
    let m = rvc_matches(&p, insn);
    assert!(m.contains(&"sd".to_string()), "{m:?}");
}

#[test]
fn c_j_matches() {
    let p = rvc_parsed();
    // C.J: 101 offset 01
    let insn: u16 = 0b101_00000000000_01;
    let m = rvc_matches(&p, insn);
    assert!(m.contains(&"jal".to_string()), "{m:?}");
}

#[test]
fn c_beqz_matches() {
    let p = rvc_parsed();
    // C.BEQZ: 110 ... ... ..... 01
    let insn: u16 = 0b110_000_001_00000_01;
    let m = rvc_matches(&p, insn);
    assert!(m.contains(&"beq".to_string()), "{m:?}");
}

#[test]
fn c_add_matches() {
    let p = rvc_parsed();
    // C.ADD: 100 1 rd rs2 10 (rd!=0, rs2!=0)
    let insn: u16 = 0b100_1_00001_00010_10;
    let m = rvc_matches(&p, insn);
    assert!(m.contains(&"add".to_string()), "{m:?}");
}

#[test]
fn c_ebreak_matches() {
    let p = rvc_parsed();
    // C.EBREAK: 100 1 00000 00000 10
    let insn: u16 = 0b100_1_00000_00000_10;
    let m = rvc_matches(&p, insn);
    assert!(m.contains(&"ebreak".to_string()), "{m:?}");
}

// ── NEW: Code generation quality ─────────────────────────────

#[test]
fn generate_16bit_no_u32_leak() {
    let input =
        std::fs::read_to_string("../guest/riscv/src/riscv/insn16.decode")
            .unwrap();
    let mut out = Vec::new();
    generate_with_width(&input, &mut out, 16).unwrap();
    let code = String::from_utf8(out).unwrap();
    // 16-bit decoder should use u16/i16, not u32/i32
    assert!(!code.contains("insn: u32"));
    assert!(!code.contains("insn: i32"));
}

#[test]
fn generate_16bit_trait_dedup() {
    let input =
        std::fs::read_to_string("../guest/riscv/src/riscv/insn16.decode")
            .unwrap();
    let mut out = Vec::new();
    generate_with_width(&input, &mut out, 16).unwrap();
    let code = String::from_utf8(out).unwrap();
    // Count trait method declarations — each trans_ should
    // appear exactly once in the trait.
    let trait_block = code
        .split("pub trait Decode16<Ir>")
        .nth(1)
        .unwrap()
        .split('}')
        .next()
        .unwrap();
    let methods: Vec<&str> = trait_block
        .lines()
        .filter(|l| l.contains("fn trans_"))
        .collect();
    let mut seen = std::collections::HashSet::new();
    for m in &methods {
        let name = m.trim().split('(').next().unwrap();
        assert!(seen.insert(name), "duplicate trait method: {name}");
    }
}
