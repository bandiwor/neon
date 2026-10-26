//! One-line pseudo-code for bytecode instructions (`neon disasm --annotate`).
//!
//! ```text
//!    16  addi.i64  r2, r2, 1             # r2 = r2 + 1
//!    17  le.s.jf   r1, r2, L14           # if !(r1 <= r2) goto L14
//! ```
//!
//! The text is for reading only; the real semantics are those of the instruction (arithmetic is
//! overflow-checked, indexing bounds-checked, ...).

use super::chunk::BytecodeProgram;
use super::insn::*;
use super::opcode::CmpKind;
use crate::builtin::Builtin;
use crate::ir::TypeDesc;
use crate::runtime::numeric::ArithOp;

/// What the program's pools say about the operands of an instruction.
#[derive(Debug)]
pub struct Explain<'a> {
    pub strings: &'a [String],
    pub constants: &'a [u64],
    pub descs: &'a [TypeDesc],
    pub globals: Vec<&'a str>,
    pub functions: Vec<&'a str>,
}

impl<'a> Explain<'a> {
    pub fn new(p: &'a BytecodeProgram) -> Explain<'a> {
        Explain {
            strings: &p.strings,
            constants: &p.constants,
            descs: &p.descs,
            globals: p.globals.iter().map(|g| g.name.as_str()).collect(),
            functions: p.chunks.iter().map(|c| c.name.as_str()).collect(),
        }
    }
}

fn r(x: Reg) -> String {
    format!("r{x}")
}

fn regs(base: Reg, n: u32) -> Vec<String> {
    (0..n).map(|i| format!("r{}", base as u32 + i)).collect()
}

/// `r1, r2, r3`, or `r1..r9` when long.
fn list(base: Reg, n: u32) -> String {
    if n > 6 {
        format!("r{base}..r{}", base as u32 + n - 1)
    } else {
        regs(base, n).join(", ")
    }
}

fn symbol(op: ArithOp) -> &'static str {
    match op {
        ArithOp::Add => "+",
        ArithOp::Sub => "-",
        ArithOp::Mul => "*",
        ArithOp::Div => "/",
        ArithOp::Rem => "%",
        ArithOp::And => "&",
        ArithOp::Or => "|",
        ArithOp::Xor => "^",
        ArithOp::Shl => "<<",
        ArithOp::Shr => ">>",
    }
}

/// The operator of a comparison and a note about its operand type where that matters.
fn compare(kind: CmpKind) -> (&'static str, &'static str) {
    use CmpKind::*;
    match kind {
        Eq => ("==", ""),
        Ne => ("!=", ""),
        LtS => ("<", ""),
        LeS => ("<=", ""),
        LtU => ("<", " (unsigned)"),
        LeU => ("<=", " (unsigned)"),
        EqF32 => ("==", " (f32)"),
        NeF32 => ("!=", " (f32)"),
        LtF32 => ("<", " (f32)"),
        LeF32 => ("<=", " (f32)"),
        EqF64 => ("==", " (f64)"),
        NeF64 => ("!=", " (f64)"),
        LtF64 => ("<", " (f64)"),
        LeF64 => ("<=", " (f64)"),
        EqStr => ("==", " (string)"),
        NeStr => ("!=", " (string)"),
        LtStr => ("<", " (string)"),
        LeStr => ("<=", " (string)"),
    }
}

fn quoted(s: &str) -> String {
    const MAX: usize = 36;
    let mut shown: String = s.chars().take(MAX).collect();
    let cut = s.chars().count() > MAX;
    if cut {
        shown.push('…');
    }
    let escaped = format!("{shown:?}");
    escaped
}

fn builtin_call(b: Builtin, args: &[String]) -> String {
    let dbg = format!("{b:?}");
    let owner = ["Arr", "Str", "Map", "Set", "Char"].iter().find(|p| dbg.starts_with(**p)).map(|p| match *p {
        "Arr" => "array",
        "Str" => "string",
        "Map" => "map",
        "Set" => "set",
        _ => "char",
    });
    match owner {
        Some(o) => format!("{o}.{}({})", b.name(), args.join(", ")),
        None => format!("{}({})", b.name(), args.join(", ")),
    }
}

fn desc_name(cx: &Explain<'_>, d: u16) -> String {
    match cx.descs.get(d as usize) {
        Some(TypeDesc::Struct { name, .. }) | Some(TypeDesc::Enum { name, .. }) => name.clone(),
        Some(_) | None => format!("d{d}"),
    }
}

/// Pseudo-code for `insn`, or `None` for labels.
pub fn explain_insn(insn: &Insn, cx: &Explain<'_>) -> Option<String> {
    use Insn::*;
    let text = match insn {
        Label(_) => return None,
        Move { dst, src } => format!("{} = {}", r(*dst), r(*src)),
        LoadI { dst, imm } => format!("{} = {imm}", r(*dst)),
        LoadK { dst, k } => match cx.constants.get(*k as usize) {
            Some(v) => format!("{} = {} (0x{v:x})", r(*dst), *v as i64),
            None => format!("{} = k{k}", r(*dst)),
        },
        LoadStr { dst, s } => match cx.strings.get(*s as usize) {
            Some(text) => format!("{} = {}", r(*dst), quoted(text)),
            None => format!("{} = s{s}", r(*dst)),
        },
        Arith { op, dst, a, b, .. } | Bitwise { op, dst, a, b } => {
            format!("{} = {} {} {}", r(*dst), r(*a), symbol(*op), r(*b))
        }
        AddImm { dst, src, imm, .. } => format!("{} = {} + {imm}", r(*dst), r(*src)),
        SubImm { dst, src, imm, .. } => format!("{} = {} - {imm}", r(*dst), r(*src)),
        Neg { dst, src, .. } => format!("{} = -{}", r(*dst), r(*src)),
        BitNot { dst, src, .. } => format!("{} = ~{}", r(*dst), r(*src)),
        BoolNot { dst, src } => format!("{} = !{}", r(*dst), r(*src)),
        Cmp { kind, dst, a, b } => {
            let (op, note) = compare(*kind);
            format!("{} = {} {op} {}{note}", r(*dst), r(*a), r(*b))
        }
        Cast { from, to, dst, src } => format!("{} = {} as {}  (from {})", r(*dst), r(*src), to.name(), from.name()),
        Jmp { to } => format!("goto L{}", to.0),
        JmpIf { cond, to } => format!("if {} goto L{}", r(*cond), to.0),
        JmpIfNot { cond, to } => format!("if !{} goto L{}", r(*cond), to.0),
        CmpJmp { kind, a, b, to } => {
            let (op, note) = compare(kind.cmp());
            format!("if !({} {op} {}) goto L{}{note}", r(*a), r(*b), to.0)
        }
        MatchTagJmp { obj, tag, to } => format!("if {}.variant != #{tag} goto L{}", r(*obj), to.0),
        Call { base, func, nargs } => {
            let name = cx.functions.get(*func as usize).copied().unwrap_or("?");
            format!("{} = {name}({})", r(*base), regs(*base, *nargs as u32).join(", "))
        }
        CallClosure { base, nargs } => {
            let args = regs(base.wrapping_add(1), *nargs as u32).join(", ");
            format!("{} = {}({args})", r(*base), r(*base))
        }
        Ret { src } => format!("return {}", r(*src)),
        Unreachable => "unreachable".to_string(),
        NewStruct { dst, base, n, desc } => {
            let fields = match cx.descs.get(*desc as usize) {
                Some(TypeDesc::Struct { fields, .. }) if fields.len() == *n as usize => fields
                    .iter()
                    .zip(regs(*base, *n as u32))
                    .map(|((name, _), reg)| format!("{name}: {reg}"))
                    .collect::<Vec<_>>()
                    .join(", "),
                _ => list(*base, *n as u32),
            };
            if fields.is_empty() {
                format!("{} = {} {{}}", r(*dst), desc_name(cx, *desc))
            } else {
                format!("{} = {} {{ {fields} }}", r(*dst), desc_name(cx, *desc))
            }
        }
        GetField { dst, obj, idx } => format!("{} = {}.field{idx}", r(*dst), r(*obj)),
        SetField { obj, idx, src } => format!("{}.field{idx} = {}", r(*obj), r(*src)),
        NewEnum { dst, base, n, desc, tag } => {
            let variant = match cx.descs.get(*desc as usize) {
                Some(TypeDesc::Enum { name, variants }) => {
                    variants.get(*tag as usize).map(|(v, _)| format!("{name}::{v}")).unwrap_or_else(|| name.clone())
                }
                _ => format!("{}#{tag}", desc_name(cx, *desc)),
            };
            if *n == 0 {
                format!("{} = {variant}", r(*dst))
            } else {
                format!("{} = {variant}({})", r(*dst), list(*base, *n as u32))
            }
        }
        EnumTag { dst, obj } => format!("{} = {}.variant", r(*dst), r(*obj)),
        EnumField { dst, obj, idx } => format!("{} = {}.payload{idx}", r(*dst), r(*obj)),
        NewArray { dst, base, n, .. } => format!("{} = [{}]", r(*dst), list(*base, *n as u32)),
        ArrayGet { dst, arr, idx } => format!("{} = {}[{}]", r(*dst), r(*arr), r(*idx)),
        ArraySet { arr, idx, src } => format!("{}[{}] = {}", r(*arr), r(*idx), r(*src)),
        NewMap { dst, base, pairs, .. } => {
            let entries: Vec<String> = (0..*pairs as u32)
                .map(|i| format!("r{}: r{}", *base as u32 + 2 * i, *base as u32 + 2 * i + 1))
                .collect();
            if *pairs > 4 {
                format!("{} = {{ {} pairs from r{base} }}", r(*dst), pairs)
            } else {
                format!("{} = {{ {} }}", r(*dst), entries.join(", "))
            }
        }
        MapGet { dst, map, key } => format!("{} = {}[{}]", r(*dst), r(*map), r(*key)),
        MapSet { map, key, val } => format!("{}[{}] = {}", r(*map), r(*key), r(*val)),
        NewSet { dst, base, n, .. } => format!("{} = set {{ {} }}", r(*dst), list(*base, *n as u32)),
        MakeClosure { dst, func, captures } => {
            let name = cx.functions.get(*func as usize).copied().unwrap_or("?");
            let caps: Vec<String> = captures
                .iter()
                .map(|c| match c {
                    Capture::Local(x) => r(*x),
                    Capture::Upvalue(i) => format!("up{i}"),
                })
                .collect();
            format!("{} = closure {name} capturing [{}]", r(*dst), caps.join(", "))
        }
        GetUpval { dst, idx } => format!("{} = up{idx}", r(*dst)),
        SetUpval { idx, src } => format!("up{idx} = {}", r(*src)),
        Close { reg } => format!("close {}  (its captured variable moves to the heap)", r(*reg)),
        GetGlobal { dst, g } => format!("{} = {}", r(*dst), cx.globals.get(*g as usize).copied().unwrap_or("global")),
        SetGlobal { src, g } => format!("{} = {}", cx.globals.get(*g as usize).copied().unwrap_or("global"), r(*src)),
        Intrinsic { dst, base, nargs, b, .. } => {
            let call = builtin_call(*b, &regs(*base, *nargs as u32));
            format!("{} = {call}", r(*dst))
        }
    };
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::opcode::FusedKind;
    use crate::runtime::numeric::NumTy;

    fn cx<'a>(strings: &'a [String], descs: &'a [TypeDesc]) -> Explain<'a> {
        Explain { strings, constants: &[0x1234], descs, globals: vec!["counter"], functions: vec!["<start>", "add"] }
    }

    fn say(insn: Insn) -> String {
        let strings = vec!["hi \"there\"".to_string(), "x".repeat(80)];
        let descs = vec![
            TypeDesc::Struct {
                name: "Human".into(),
                fields: vec![("name".into(), crate::ir::DescId(1)), ("age".into(), crate::ir::DescId(2))],
            },
            TypeDesc::Str,
            TypeDesc::Num(NumTy::U64),
            TypeDesc::Enum { name: "Shape".into(), variants: vec![("Circle".into(), vec![]), ("Rect".into(), vec![])] },
        ];
        explain_insn(&insn, &cx(&strings, &descs)).expect("an instruction, not a label")
    }

    #[test]
    fn arithmetic_and_moves() {
        assert_eq!(say(Insn::AddImm { ty: NumTy::I64, dst: 1, src: 1, imm: 4 }), "r1 = r1 + 4");
        assert_eq!(say(Insn::SubImm { ty: NumTy::I64, dst: 2, src: 3, imm: 7 }), "r2 = r3 - 7");
        assert_eq!(say(Insn::Arith { op: ArithOp::Mul, ty: NumTy::I32, dst: 0, a: 1, b: 2 }), "r0 = r1 * r2");
        assert_eq!(say(Insn::Bitwise { op: ArithOp::Xor, dst: 0, a: 1, b: 2 }), "r0 = r1 ^ r2");
        assert_eq!(say(Insn::Move { dst: 5, src: 6 }), "r5 = r6");
        assert_eq!(say(Insn::LoadI { dst: 0, imm: -3 }), "r0 = -3");
        assert_eq!(say(Insn::LoadK { dst: 0, k: 0 }), "r0 = 4660 (0x1234)");
        assert_eq!(say(Insn::Neg { ty: NumTy::I64, dst: 1, src: 2 }), "r1 = -r2");
        assert_eq!(say(Insn::Cast { from: NumTy::I64, to: NumTy::U8, dst: 1, src: 2 }), "r1 = r2 as u8  (from i64)");
    }

    #[test]
    fn strings_are_quoted_and_cut() {
        assert_eq!(say(Insn::LoadStr { dst: 3, s: 0 }), "r3 = \"hi \\\"there\\\"\"");
        let long = say(Insn::LoadStr { dst: 3, s: 1 });
        assert!(long.ends_with("…\""), "{long}");
        assert!(long.len() < 60, "{long}");
    }

    #[test]
    fn control_flow() {
        assert_eq!(say(Insn::Jmp { to: Label(14) }), "goto L14");
        assert_eq!(say(Insn::JmpIfNot { cond: 3, to: Label(9) }), "if !r3 goto L9");
        // fused compares jump when the comparison is false
        assert_eq!(say(Insn::CmpJmp { kind: FusedKind::LeS, a: 1, b: 2, to: Label(14) }), "if !(r1 <= r2) goto L14");
        assert_eq!(
            say(Insn::CmpJmp { kind: FusedKind::LtF64, a: 1, b: 2, to: Label(4) }),
            "if !(r1 < r2) goto L4 (f64)"
        );
        assert_eq!(say(Insn::Ret { src: 0 }), "return r0");
        assert_eq!(say(Insn::Call { base: 5, func: 1, nargs: 2 }), "r5 = add(r5, r6)");
    }

    #[test]
    fn objects_use_their_type_names() {
        assert_eq!(say(Insn::NewStruct { dst: 1, base: 1, n: 2, desc: 0 }), "r1 = Human { name: r1, age: r2 }");
        assert_eq!(say(Insn::NewEnum { dst: 1, base: 2, n: 0, desc: 3, tag: 0 }), "r1 = Shape::Circle");
        assert_eq!(say(Insn::NewArray { dst: 1, base: 1, n: 3, desc: 0 }), "r1 = [r1, r2, r3]");
        assert_eq!(say(Insn::NewArray { dst: 1, base: 1, n: 12, desc: 0 }), "r1 = [r1..r12]");
        assert_eq!(say(Insn::ArrayGet { dst: 8, arr: 1, idx: 2 }), "r8 = r1[r2]");
        assert_eq!(say(Insn::GetGlobal { dst: 1, g: 0 }), "r1 = counter");
    }

    #[test]
    fn builtins_read_like_calls() {
        let call = |b| say(Insn::Intrinsic { dst: 4, base: 5, nargs: 1, b, desc: None });
        assert_eq!(call(Builtin::Println), "r4 = println(r5)");
        assert_eq!(call(Builtin::ArrLen), "r4 = array.len(r5)");
        assert_eq!(call(Builtin::StrToUpper), "r4 = string.to_upper(r5)");
    }

    #[test]
    fn labels_have_no_note() {
        let (strings, descs) = (vec![], vec![]);
        assert!(explain_insn(&Insn::Label(Label(1)), &cx(&strings, &descs)).is_none());
    }
}
