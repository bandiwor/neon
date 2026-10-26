//! Human-readable bytecode listings.

use std::collections::BTreeSet;
use std::fmt::Write;

use super::asm::{decode_one, DecodeError};
use super::chunk::{BytecodeProgram, Chunk};
use super::explain::{explain_insn, Explain};
use super::insn::*;
use super::opcode::Opcode;
use crate::ir::TypeDesc;

fn reg(r: Reg) -> String {
    format!("r{r}")
}

fn window(base: Reg, n: u32) -> String {
    match n {
        0 => "[]".to_string(),
        1 => format!("[r{base}]"),
        n => format!("[r{base}..r{}]", base as u32 + n - 1),
    }
}

/// Formats one instruction; jump targets show as `L<pc>`.
pub fn format_insn(insn: &Insn) -> String {
    use Insn::*;
    let lbl = |l: &super::insn::Label| format!("L{}", l.0);
    match insn {
        Label(l) => format!("L{}:", l.0),
        Move { dst, src } => format!("move      {}, {}", reg(*dst), reg(*src)),
        LoadI { dst, imm } => format!("loadi     {}, {imm}", reg(*dst)),
        LoadK { dst, k } => format!("loadk     {}, k{k}", reg(*dst)),
        LoadStr { dst, s } => format!("loadstr   {}, s{s}", reg(*dst)),
        Arith { op, ty, dst, a, b } => {
            format!("{:<9} {}, {}, {}", format!("{}.{}", op.name(), ty.name()), reg(*dst), reg(*a), reg(*b))
        }
        Bitwise { op, dst, a, b } => format!("{:<9} {}, {}, {}", op.name(), reg(*dst), reg(*a), reg(*b)),
        AddImm { ty, dst, src, imm } => {
            format!("{:<9} {}, {}, {imm}", format!("addi.{}", ty.name()), reg(*dst), reg(*src))
        }
        SubImm { ty, dst, src, imm } => {
            format!("{:<9} {}, {}, {imm}", format!("subi.{}", ty.name()), reg(*dst), reg(*src))
        }
        Neg { ty, dst, src } => format!("{:<9} {}, {}", format!("neg.{}", ty.name()), reg(*dst), reg(*src)),
        BitNot { ty, dst, src } => format!("{:<9} {}, {}", format!("bnot.{}", ty.name()), reg(*dst), reg(*src)),
        BoolNot { dst, src } => format!("not       {}, {}", reg(*dst), reg(*src)),
        Cmp { kind, dst, a, b } => {
            format!("{:<9} {}, {}, {}", Opcode::cmp(*kind).mnemonic(), reg(*dst), reg(*a), reg(*b))
        }
        Cast { from, to, dst, src } => {
            format!("cast      {}, {} ({}->{})", reg(*dst), reg(*src), from.name(), to.name())
        }
        Jmp { to } => format!("jmp       {}", lbl(to)),
        JmpIf { cond, to } => format!("jmpif     {}, {}", reg(*cond), lbl(to)),
        JmpIfNot { cond, to } => format!("jmpifnot  {}, {}", reg(*cond), lbl(to)),
        CmpJmp { kind, a, b, to } => {
            format!("{:<9} {}, {}, {}", Opcode::fused(*kind).mnemonic(), reg(*a), reg(*b), lbl(to))
        }
        MatchTagJmp { obj, tag, to } => format!("matchtag  {}, #{tag}, {}", reg(*obj), lbl(to)),
        Call { base, func, nargs } => format!("call      f{func}, {}", window(*base, *nargs as u32)),
        CallClosure { base, nargs } => {
            format!("callc     {}, args {}", reg(*base), window(base.wrapping_add(1), *nargs as u32))
        }
        Ret { src } => format!("ret       {}", reg(*src)),
        Unreachable => "unreachable".to_string(),
        NewStruct { dst, base, n, desc } => format!("newstruct {}, {}, d{desc}", reg(*dst), window(*base, *n as u32)),
        GetField { dst, obj, idx } => format!("getfield  {}, {}.{idx}", reg(*dst), reg(*obj)),
        SetField { obj, idx, src } => format!("setfield  {}.{idx}, {}", reg(*obj), reg(*src)),
        NewEnum { dst, base, n, desc, tag } => {
            format!("newenum   {}, #{tag}, {}, d{desc}", reg(*dst), window(*base, *n as u32))
        }
        EnumTag { dst, obj } => format!("enumtag   {}, {}", reg(*dst), reg(*obj)),
        EnumField { dst, obj, idx } => format!("enumfield {}, {}.{idx}", reg(*dst), reg(*obj)),
        NewArray { dst, base, n, desc } => format!("newarray  {}, {}, d{desc}", reg(*dst), window(*base, *n as u32)),
        ArrayGet { dst, arr, idx } => format!("aget      {}, {}[{}]", reg(*dst), reg(*arr), reg(*idx)),
        ArraySet { arr, idx, src } => format!("aset      {}[{}], {}", reg(*arr), reg(*idx), reg(*src)),
        NewMap { dst, base, pairs, desc } => {
            format!("newmap    {}, {}, d{desc}", reg(*dst), window(*base, *pairs as u32 * 2))
        }
        MapGet { dst, map, key } => format!("mget      {}, {}[{}]", reg(*dst), reg(*map), reg(*key)),
        MapSet { map, key, val } => format!("mset      {}[{}], {}", reg(*map), reg(*key), reg(*val)),
        NewSet { dst, base, n, desc } => format!("newset    {}, {}, d{desc}", reg(*dst), window(*base, *n as u32)),
        MakeClosure { dst, func, captures } => {
            let caps: Vec<String> = captures
                .iter()
                .map(|c| match c {
                    Capture::Local(r) => reg(*r),
                    Capture::Upvalue(i) => format!("up{i}"),
                })
                .collect();
            format!("closure   {}, f{func}, [{}]", reg(*dst), caps.join(", "))
        }
        GetUpval { dst, idx } => format!("getupval  {}, up{idx}", reg(*dst)),
        SetUpval { idx, src } => format!("setupval  up{idx}, {}", reg(*src)),
        Close { reg: r } => format!("close     {}", reg(*r)),
        GetGlobal { dst, g } => format!("getglobal {}, g{g}", reg(*dst)),
        SetGlobal { src, g } => format!("setglobal g{g}, {}", reg(*src)),
        Intrinsic { dst, base, nargs, b, desc } => {
            let d = desc.map(|d| format!(", d{d}")).unwrap_or_default();
            format!("intrinsic {}, {:?}, {}{d}", reg(*dst), b, window(*base, *nargs as u32))
        }
    }
}

/// Lists one function: word offsets, instructions, jump-target labels and stack maps.
pub fn disassemble_chunk(chunk: &Chunk) -> Result<String, DecodeError> {
    disassemble_chunk_with(chunk, None)
}

/// Like [`disassemble_chunk`]; with a context every instruction also gets a `# pseudo-code` note
/// at the end of its line (aligned within the function).
pub fn disassemble_chunk_with(chunk: &Chunk, explain: Option<&Explain<'_>>) -> Result<String, DecodeError> {
    let mut decoded = Vec::new();
    let mut pc = 0u32;
    while (pc as usize) < chunk.code.len() {
        let d = decode_one(&chunk.code, pc)?;
        pc += d.len;
        decoded.push(d);
    }
    let targets: BTreeSet<u32> = decoded.iter().filter_map(|d| d.insn.target().map(|l| l.0)).collect();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "== {} (params={}, frame={}, code={} words) ==",
        chunk.name,
        chunk.params,
        chunk.frame_size,
        chunk.code.len()
    );
    // (labels, instruction text, note) per instruction; notes are aligned afterwards
    let mut lines: Vec<(bool, String, Option<String>)> = Vec::new();
    for d in &decoded {
        if targets.contains(&d.pc) {
            lines.push((true, format!("      L{}:", d.pc), None));
        }
        let mut left = format!("{:>5}  {}", d.pc, format_insn(&d.insn));
        if let Some(m) = chunk.stack_map_at(d.pc) {
            let roots: Vec<String> = m.live_roots.iter().map(|r| format!("r{r}")).collect();
            left.push_str(&format!("    ; roots {{{}}}", roots.join(", ")));
        }
        let note = explain.and_then(|cx| explain_insn(&d.insn, cx));
        lines.push((false, left, note));
    }
    let width = lines
        .iter()
        .filter(|(label, _, note)| !label && note.is_some())
        .map(|(_, l, _)| l.chars().count())
        .max()
        .unwrap_or(0);
    for (_, left, note) in &lines {
        match note {
            Some(note) => {
                let pad = width.saturating_sub(left.chars().count());
                let _ = writeln!(out, "{left}{}   # {note}", " ".repeat(pad));
            }
            None => {
                let _ = writeln!(out, "{left}");
            }
        }
    }
    if targets.contains(&(chunk.code.len() as u32)) {
        let _ = writeln!(out, "      L{}:", chunk.code.len());
    }
    Ok(out)
}

/// Lists a whole program: pools, descriptors and every function.
pub fn disassemble(p: &BytecodeProgram) -> Result<String, DecodeError> {
    disassemble_with(p, false)
}

/// Like [`disassemble`]; `annotate` adds a pseudo-code comment to every instruction.
pub fn disassemble_with(p: &BytecodeProgram, annotate: bool) -> Result<String, DecodeError> {
    let explain = annotate.then(|| Explain::new(p));
    let mut out = String::new();
    if !p.constants.is_empty() {
        let ks: Vec<String> = p.constants.iter().enumerate().map(|(i, k)| format!("k{i}={k:#x}")).collect();
        let _ = writeln!(out, "constants: {}", ks.join(" "));
    }
    for (i, s) in p.strings.iter().enumerate() {
        let _ = writeln!(out, "s{i} = {s:?}");
    }
    for (i, d) in p.descs.iter().enumerate() {
        let _ = writeln!(out, "d{i} = {}", format_desc(d));
    }
    for (i, g) in p.globals.iter().enumerate() {
        let _ = writeln!(out, "g{i} = {} : {:?}", g.name, g.ty);
    }
    for (i, c) in p.chunks.iter().enumerate() {
        let entry = if i as u32 == p.entry { " (entry)" } else { "" };
        let _ = write!(out, "f{i}{entry}: ");
        out.push_str(&disassemble_chunk_with(c, explain.as_ref())?);
    }
    Ok(out)
}

fn format_desc(d: &TypeDesc) -> String {
    match d {
        TypeDesc::Unit => "unit".into(),
        TypeDesc::Num(n) => n.name().into(),
        TypeDesc::Bool => "bool".into(),
        TypeDesc::Char => "char".into(),
        TypeDesc::Str => "string".into(),
        TypeDesc::Array(e) => format!("[d{}]", e.0),
        TypeDesc::Set(e) => format!("set<d{}>", e.0),
        TypeDesc::Map(k, v) => format!("{{d{}: d{}}}", k.0, v.0),
        TypeDesc::Struct { name, fields } => format!(
            "struct {name} {{{}}}",
            fields.iter().map(|(n, d)| format!("{n}: d{}", d.0)).collect::<Vec<_>>().join(", ")
        ),
        TypeDesc::Enum { name, variants } => format!(
            "enum {name} {{{}}}",
            variants
                .iter()
                .map(|(n, ds)| format!(
                    "{n}({})",
                    ds.iter().map(|d| format!("d{}", d.0)).collect::<Vec<_>>().join(", ")
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeDesc::Fn => "fn".into(),
    }
}
