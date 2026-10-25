//! Encoding of [`Insn`]s into 32-bit words and decoding back.

use crate::fx::FxHashMap as HashMap;

use super::insn::*;
use super::opcode::{Format, Opcode, EXT_ESCAPE};
use crate::builtin::Builtin;
use crate::runtime::numeric::{ArithOp, NumTy};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsmError {
    UndefinedLabel(Label),
    DuplicateLabel(Label),
}

impl std::fmt::Display for AsmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AsmError::UndefinedLabel(l) => write!(f, "jump to undefined label L{}", l.0),
            AsmError::DuplicateLabel(l) => write!(f, "label L{} defined twice", l.0),
        }
    }
}

impl std::error::Error for AsmError {}

/// The result of assembling a function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assembled {
    pub code: Vec<u32>,
    /// Word offset of each input instruction (a label's offset is that of the next instruction).
    pub pcs: Vec<u32>,
}

fn r3(op: Opcode, a: u8, b: u8, c: u8) -> u32 {
    ((op as u32) << 24) | ((a as u32) << 16) | ((b as u32) << 8) | c as u32
}

fn r2i(op: Opcode, a: u8, imm: u16) -> u32 {
    ((op as u32) << 24) | ((a as u32) << 16) | imm as u32
}

fn ext(op: Opcode, a: u8, b: u8) -> u32 {
    ((EXT_ESCAPE as u32) << 24) | ((op as u32) << 16) | ((a as u32) << 8) | b as u32
}

/// The descriptor field value meaning "no descriptor" in an `intrinsic`.
pub const NO_DESC: u16 = 0xFFFF;

fn builtin_index(b: Builtin) -> u8 {
    Builtin::ALL.iter().position(|&x| x == b).expect("builtin is in ALL") as u8
}

/// Per-instruction layout decision.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wide {
    No,
    Yes,
}

fn size_of(insn: &Insn, wide: Wide) -> u32 {
    use Insn::*;
    match insn {
        Label(_) => 0,
        Jmp { .. } | JmpIf { .. } | JmpIfNot { .. } | LoadK { .. } | LoadStr { .. } | Call { .. } => match wide {
            Wide::No => 1,
            Wide::Yes => 2,
        },
        CmpJmp { .. }
        | MatchTagJmp { .. }
        | NewStruct { .. }
        | NewEnum { .. }
        | NewArray { .. }
        | NewMap { .. }
        | NewSet { .. }
        | Intrinsic { .. } => 2,
        MakeClosure { captures, .. } => 2 + captures.len() as u32,
        _ => 1,
    }
}

/// Encodes `insns`, resolving labels. Jumps start short and are widened
/// until every offset fits.
pub fn assemble(insns: &[Insn]) -> Result<Assembled, AsmError> {
    let mut wide: Vec<Wide> = insns
        .iter()
        .map(|i| match i {
            Insn::LoadK { k, .. } if *k > u16::MAX as u32 => Wide::Yes,
            Insn::LoadStr { s, .. } if *s > u16::MAX as u32 => Wide::Yes,
            Insn::Call { func, .. } if *func > u16::MAX as u32 => Wide::Yes,
            _ => Wide::No,
        })
        .collect();

    let mut label_ids: HashMap<Label, usize> = HashMap::default();
    for (i, insn) in insns.iter().enumerate() {
        if let Insn::Label(l) = insn {
            if label_ids.insert(*l, i).is_some() {
                return Err(AsmError::DuplicateLabel(*l));
            }
        }
    }

    let mut pcs;
    loop {
        pcs = Vec::with_capacity(insns.len() + 1);
        let mut pc = 0u32;
        for (i, insn) in insns.iter().enumerate() {
            pcs.push(pc);
            pc += size_of(insn, wide[i]);
        }
        pcs.push(pc);

        let mut changed = false;
        for (i, insn) in insns.iter().enumerate() {
            if wide[i] == Wide::Yes {
                continue;
            }
            if let Insn::Jmp { to } | Insn::JmpIf { to, .. } | Insn::JmpIfNot { to, .. } = insn {
                let target = *label_ids.get(to).ok_or(AsmError::UndefinedLabel(*to))?;
                let off = pcs[target] as i64 - (pcs[i] as i64 + 1);
                if off < i16::MIN as i64 || off > i16::MAX as i64 {
                    wide[i] = Wide::Yes;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    let mut code = Vec::with_capacity(*pcs.last().unwrap() as usize);
    for (i, insn) in insns.iter().enumerate() {
        let next = pcs[i] as i64 + size_of(insn, wide[i]) as i64;
        let rel = |to: &Label| -> Result<i64, AsmError> {
            let target = *label_ids.get(to).ok_or(AsmError::UndefinedLabel(*to))?;
            Ok(pcs[target] as i64 - next)
        };
        encode(insn, wide[i] == Wide::Yes, &rel, &mut code)?;
    }
    debug_assert_eq!(code.len() as u32, *pcs.last().unwrap());
    pcs.pop();
    Ok(Assembled { code, pcs })
}

fn encode(
    insn: &Insn,
    wide: bool,
    rel: &dyn Fn(&Label) -> Result<i64, AsmError>,
    out: &mut Vec<u32>,
) -> Result<(), AsmError> {
    use Insn::*;
    match insn {
        Label(_) => {}
        Move { dst, src } => out.push(r3(Opcode::Move, *dst, *src, 0)),
        LoadI { dst, imm } => out.push(r2i(Opcode::LoadI, *dst, *imm as u16)),
        LoadK { dst, k } => {
            if wide {
                out.push(ext(Opcode::LoadKW, *dst, 0));
                out.push(*k);
            } else {
                out.push(r2i(Opcode::LoadK, *dst, *k as u16));
            }
        }
        LoadStr { dst, s } => {
            if wide {
                out.push(ext(Opcode::LoadStrW, *dst, 0));
                out.push(*s);
            } else {
                out.push(r2i(Opcode::LoadStr, *dst, *s as u16));
            }
        }
        Arith { op, ty, dst, a, b } => {
            let opc = Opcode::arith(*op, *ty).expect("no opcode for this arithmetic combination");
            out.push(r3(opc, *dst, *a, *b));
        }
        Bitwise { op, dst, a, b } => out.push(r3(Opcode::bitwise(*op).expect("bitwise op"), *dst, *a, *b)),
        AddImm { ty, dst, src, imm } => out.push(r3(Opcode::addi(*ty).expect("integer type"), *dst, *src, *imm as u8)),
        SubImm { ty, dst, src, imm } => out.push(r3(Opcode::subi(*ty).expect("integer type"), *dst, *src, *imm)),
        Neg { ty, dst, src } => out.push(r3(Opcode::neg(*ty), *dst, *src, 0)),
        BitNot { ty, dst, src } => out.push(r3(Opcode::bnot(*ty).expect("integer type"), *dst, *src, 0)),
        BoolNot { dst, src } => out.push(r3(Opcode::BoolNot, *dst, *src, 0)),
        Cmp { kind, dst, a, b } => out.push(r3(Opcode::cmp(*kind), *dst, *a, *b)),
        Cast { from, to, dst, src } => {
            let kind = NumTy::ALL.iter().position(|t| t == from).unwrap() * 10
                + NumTy::ALL.iter().position(|t| t == to).unwrap();
            out.push(r3(Opcode::Cast, *dst, *src, kind as u8));
        }
        Jmp { to } => jump(Opcode::Jmp, Opcode::JmpW, 0, rel(to)?, wide, out),
        JmpIf { cond, to } => jump(Opcode::JmpIf, Opcode::JmpIfW, *cond, rel(to)?, wide, out),
        JmpIfNot { cond, to } => jump(Opcode::JmpIfNot, Opcode::JmpIfNotW, *cond, rel(to)?, wide, out),
        CmpJmp { kind, a, b, to } => {
            out.push(ext(Opcode::fused(*kind), *a, *b));
            out.push(rel(to)? as i32 as u32);
        }
        MatchTagJmp { obj, tag, to } => {
            out.push(ext(Opcode::MatchTagJmp, *obj, *tag));
            out.push(rel(to)? as i32 as u32);
        }
        Call { base, func, .. } => {
            if wide {
                out.push(ext(Opcode::CallW, *base, 0));
                out.push(*func);
            } else {
                out.push(r2i(Opcode::Call, *base, *func as u16));
            }
        }
        CallClosure { base, nargs } => out.push(r3(Opcode::CallClosure, *base, *nargs, 0)),
        Ret { src } => out.push(r2i(Opcode::Ret, *src, 0)),
        Unreachable => out.push(r2i(Opcode::Unreachable, 0, 0)),
        NewStruct { dst, base, n, desc } => {
            out.push(ext(Opcode::NewStruct, *dst, *base));
            out.push(*desc as u32 | (*n as u32) << 16);
        }
        GetField { dst, obj, idx } => out.push(r3(Opcode::GetField, *dst, *obj, *idx)),
        SetField { obj, idx, src } => out.push(r3(Opcode::SetField, *obj, *idx, *src)),
        NewEnum { dst, base, n, desc, tag } => {
            out.push(ext(Opcode::NewEnum, *dst, *base));
            out.push(*desc as u32 | (*n as u32) << 16 | (*tag as u32) << 24);
        }
        EnumTag { dst, obj } => out.push(r3(Opcode::EnumTag, *dst, *obj, 0)),
        EnumField { dst, obj, idx } => out.push(r3(Opcode::EnumField, *dst, *obj, *idx)),
        NewArray { dst, base, n, desc } => {
            out.push(ext(Opcode::NewArray, *dst, *base));
            out.push(*desc as u32 | (*n as u32) << 16);
        }
        ArrayGet { dst, arr, idx } => out.push(r3(Opcode::ArrayGet, *dst, *arr, *idx)),
        ArraySet { arr, idx, src } => out.push(r3(Opcode::ArraySet, *arr, *idx, *src)),
        NewMap { dst, base, pairs, desc } => {
            out.push(ext(Opcode::NewMap, *dst, *base));
            out.push(*desc as u32 | (*pairs as u32) << 16);
        }
        MapGet { dst, map, key } => out.push(r3(Opcode::MapGet, *dst, *map, *key)),
        MapSet { map, key, val } => out.push(r3(Opcode::MapSet, *map, *key, *val)),
        NewSet { dst, base, n, desc } => {
            out.push(ext(Opcode::NewSet, *dst, *base));
            out.push(*desc as u32 | (*n as u32) << 16);
        }
        MakeClosure { dst, func, captures } => {
            out.push(ext(Opcode::MakeClosure, *dst, captures.len() as u8));
            out.push(*func);
            for c in captures {
                out.push(match c {
                    Capture::Local(r) => *r as u32,
                    Capture::Upvalue(i) => 0x100 | *i as u32,
                });
            }
        }
        GetUpval { dst, idx } => out.push(r3(Opcode::GetUpval, *dst, *idx, 0)),
        SetUpval { idx, src } => out.push(r3(Opcode::SetUpval, *idx, *src, 0)),
        Close { reg } => out.push(r2i(Opcode::Close, *reg, 0)),
        GetGlobal { dst, g } => out.push(r2i(Opcode::GetGlobal, *dst, *g)),
        SetGlobal { src, g } => out.push(r2i(Opcode::SetGlobal, *src, *g)),
        Intrinsic { dst, base, nargs, b, desc } => {
            out.push(ext(Opcode::Intrinsic, *dst, *base));
            out.push(desc.unwrap_or(NO_DESC) as u32 | (*nargs as u32) << 16 | (builtin_index(*b) as u32) << 24);
        }
    }
    Ok(())
}

fn jump(short: Opcode, long: Opcode, a: u8, off: i64, wide: bool, out: &mut Vec<u32>) {
    if wide {
        out.push(ext(long, a, 0));
        out.push(off as i32 as u32);
    } else {
        out.push(r2i(short, a, off as i16 as u16));
    }
}

// ---- decoding --------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    BadOpcode { pc: u32, byte: u8 },
    Truncated { pc: u32 },
    BadOperand { pc: u32 },
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::BadOpcode { pc, byte } => write!(f, "invalid opcode {byte:#x} at {pc}"),
            DecodeError::Truncated { pc } => write!(f, "truncated instruction at {pc}"),
            DecodeError::BadOperand { pc } => write!(f, "invalid operand at {pc}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// A decoded instruction with its position and length in words.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub pc: u32,
    pub len: u32,
    /// Jump targets are `Label(absolute pc)`.
    pub insn: Insn,
}

/// The header fields of the instruction starting at `code[pc]`, without fully decoding it.
pub fn decode_one(code: &[u32], pc: u32) -> Result<Decoded, DecodeError> {
    let w = *code.get(pc as usize).ok_or(DecodeError::Truncated { pc })?;
    let top = (w >> 24) as u8;
    let (op, a, b, c, imm, is_ext) = if top == EXT_ESCAPE {
        let op = Opcode::from_u8((w >> 16) as u8).ok_or(DecodeError::BadOpcode { pc, byte: (w >> 16) as u8 })?;
        if op.format() != Format::Ext {
            return Err(DecodeError::BadOpcode { pc, byte: op as u8 });
        }
        (op, (w >> 8) as u8, w as u8, 0u8, 0u16, true)
    } else {
        let op = Opcode::from_u8(top).ok_or(DecodeError::BadOpcode { pc, byte: top })?;
        if op.format() == Format::Ext {
            return Err(DecodeError::BadOpcode { pc, byte: top });
        }
        (op, (w >> 16) as u8, (w >> 8) as u8, w as u8, w as u16, false)
    };
    let extra = if is_ext { Some(*code.get(pc as usize + 1).ok_or(DecodeError::Truncated { pc })?) } else { None };
    let len = if is_ext { 2 } else { 1 };
    let mut length = len;
    let rel_target = |len: u32, off: i32| Label((pc as i64 + len as i64 + off as i64) as u32);
    let insn = match op.format() {
        Format::R3 | Format::R2I => {
            decode_narrow(op, a, b, c, imm, |off| rel_target(1, off)).ok_or(DecodeError::BadOperand { pc })?
        }
        Format::Ext => {
            let e = extra.expect("ext has an extension word");
            let off = e as i32;
            use Opcode::*;
            match op {
                LoadKW => Insn::LoadK { dst: a, k: e },
                LoadStrW => Insn::LoadStr { dst: a, s: e },
                JmpW => Insn::Jmp { to: rel_target(2, off) },
                JmpIfW => Insn::JmpIf { cond: a, to: rel_target(2, off) },
                JmpIfNotW => Insn::JmpIfNot { cond: a, to: rel_target(2, off) },
                CallW => Insn::Call { base: a, func: e, nargs: 0 },
                MatchTagJmp => Insn::MatchTagJmp { obj: a, tag: b, to: rel_target(2, off) },
                NewStruct => Insn::NewStruct { dst: a, base: b, n: (e >> 16) as u8, desc: e as u16 },
                NewEnum => Insn::NewEnum { dst: a, base: b, n: (e >> 16) as u8, desc: e as u16, tag: (e >> 24) as u8 },
                NewArray => Insn::NewArray { dst: a, base: b, n: (e >> 16) as u8, desc: e as u16 },
                NewMap => Insn::NewMap { dst: a, base: b, pairs: (e >> 16) as u8, desc: e as u16 },
                NewSet => Insn::NewSet { dst: a, base: b, n: (e >> 16) as u8, desc: e as u16 },
                MakeClosure => {
                    let n = b as u32;
                    length = 2 + n;
                    let mut captures = Vec::new();
                    for i in 0..n {
                        let cw = *code.get((pc + 2 + i) as usize).ok_or(DecodeError::Truncated { pc })?;
                        captures.push(if cw & 0x100 != 0 {
                            Capture::Upvalue(cw as u8)
                        } else {
                            Capture::Local(cw as u8)
                        });
                    }
                    Insn::MakeClosure { dst: a, func: e, captures }
                }
                Intrinsic => {
                    let bi = (e >> 24) as usize;
                    let builtin = *Builtin::ALL.get(bi).ok_or(DecodeError::BadOperand { pc })?;
                    let desc = e as u16;
                    Insn::Intrinsic {
                        dst: a,
                        base: b,
                        nargs: (e >> 16) as u8,
                        b: builtin,
                        desc: if desc == NO_DESC { None } else { Some(desc) },
                    }
                }
                other => match other.fused_kind() {
                    Some(kind) => Insn::CmpJmp { kind, a, b, to: rel_target(2, off) },
                    None => return Err(DecodeError::BadOpcode { pc, byte: other as u8 }),
                },
            }
        }
    };
    Ok(Decoded { pc, len: length, insn })
}

fn decode_narrow(op: Opcode, a: u8, b: u8, c: u8, imm: u16, rel: impl Fn(i32) -> Label) -> Option<Insn> {
    use Opcode::*;
    Some(match op {
        Move => Insn::Move { dst: a, src: b },
        LoadI => Insn::LoadI { dst: a, imm: imm as i16 },
        LoadK => Insn::LoadK { dst: a, k: imm as u32 },
        LoadStr => Insn::LoadStr { dst: a, s: imm as u32 },
        And => Insn::Bitwise { op: ArithOp::And, dst: a, a: b, b: c },
        Or => Insn::Bitwise { op: ArithOp::Or, dst: a, a: b, b: c },
        Xor => Insn::Bitwise { op: ArithOp::Xor, dst: a, a: b, b: c },
        Cast => {
            let k = c as usize;
            if k >= 100 {
                return None;
            }
            Insn::Cast { from: NumTy::ALL[k / 10], to: NumTy::ALL[k % 10], dst: a, src: b }
        }
        BoolNot => Insn::BoolNot { dst: a, src: b },
        Jmp => Insn::Jmp { to: rel(imm as i16 as i32) },
        JmpIf => Insn::JmpIf { cond: a, to: rel(imm as i16 as i32) },
        JmpIfNot => Insn::JmpIfNot { cond: a, to: rel(imm as i16 as i32) },
        Ret => Insn::Ret { src: a },
        Unreachable => Insn::Unreachable,
        Call => Insn::Call { base: a, func: imm as u32, nargs: 0 },
        CallClosure => Insn::CallClosure { base: a, nargs: b },
        GetField => Insn::GetField { dst: a, obj: b, idx: c },
        SetField => Insn::SetField { obj: a, idx: b, src: c },
        EnumTag => Insn::EnumTag { dst: a, obj: b },
        EnumField => Insn::EnumField { dst: a, obj: b, idx: c },
        ArrayGet => Insn::ArrayGet { dst: a, arr: b, idx: c },
        ArraySet => Insn::ArraySet { arr: a, idx: b, src: c },
        MapGet => Insn::MapGet { dst: a, map: b, key: c },
        MapSet => Insn::MapSet { map: a, key: b, val: c },
        GetUpval => Insn::GetUpval { dst: a, idx: b },
        SetUpval => Insn::SetUpval { idx: a, src: b },
        Close => Insn::Close { reg: a },
        GetGlobal => Insn::GetGlobal { dst: a, g: imm },
        SetGlobal => Insn::SetGlobal { src: a, g: imm },
        other => {
            if let Some((aop, ty)) = other.arith_parts() {
                Insn::Arith { op: aop, ty, dst: a, a: b, b: c }
            } else if let Some(ty) = other.neg_ty() {
                Insn::Neg { ty, dst: a, src: b }
            } else if let Some(ty) = other.bnot_ty() {
                Insn::BitNot { ty, dst: a, src: b }
            } else if let Some(ty) = other.addi_ty() {
                Insn::AddImm { ty, dst: a, src: b, imm: c as i8 }
            } else if let Some(ty) = other.subi_ty() {
                Insn::SubImm { ty, dst: a, src: b, imm: c }
            } else {
                Insn::Cmp { kind: other.cmp_kind()?, dst: a, a: b, b: c }
            }
        }
    })
}

/// Decodes a whole function body. Jump targets become `Label(absolute pc)`
/// and matching [`Insn::Label`] pseudo-instructions are inserted, so the
/// result can be re-assembled.
pub fn decode(code: &[u32]) -> Result<Vec<Insn>, DecodeError> {
    let mut decoded = Vec::new();
    let mut pc = 0u32;
    while (pc as usize) < code.len() {
        let d = decode_one(code, pc)?;
        pc += d.len;
        decoded.push(d);
    }
    let mut targets: Vec<u32> = decoded.iter().filter_map(|d| d.insn.target().map(|l| l.0)).collect();
    targets.sort_unstable();
    targets.dedup();
    let mut out = Vec::new();
    let mut ti = 0;
    for d in decoded {
        while ti < targets.len() && targets[ti] <= d.pc {
            if targets[ti] == d.pc {
                out.push(Insn::Label(Label(d.pc)));
            }
            ti += 1;
        }
        out.push(d.insn);
    }
    // a label at the very end of the function
    for &t in &targets[ti..] {
        if t as usize == code.len() {
            out.push(Insn::Label(Label(t)));
        }
    }
    Ok(out)
}
