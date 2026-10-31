//! The last IR pass: packed enums (see [`PackedEnum`]) become plain 64-bit words.
//!
//! Until now a packed enum was built by `NewEnum`, taken apart by `EnumTag` / `EnumField` / `BranchTag`
//! and typed `Packed`, so the optimizer could treat it as the object it semantically is: scalarize the
//! ones that stay private, decide tag tests on known variants, forward fields. What is left (the words that
//! really are passed, returned, stored or merged at joins) is turned into shifts, masks and compares here,
//! and the type becomes `Num(U64)`, so the interpreter's reference semantics, the bytecode compiler and
//! the C translation never see the difference.

use std::collections::HashMap;

use super::types::*;
use crate::runtime::numeric::{ArithOp, CmpOp};
use crate::runtime::NumTy;

/// Replaces every packed-enum operation of `prog` by arithmetic and the type `Packed` by `Num(U64)`.
/// Returns whether the program had any.
pub fn legalize(prog: &mut IrProgram) -> bool {
    let mut layouts: HashMap<DescId, Option<PackedEnum>> = HashMap::new();
    for i in 0..prog.descs.len() {
        let d = DescId(i as u32);
        layouts.insert(d, packed_enum(&prog.descs, d));
    }
    let mut any = false;
    for f in &mut prog.funcs {
        any |= legalize_func(f, &layouts);
    }
    for g in &mut prog.globals {
        any |= g.ty == IrTy::Packed;
        g.ty = word(g.ty);
    }
    any
}

fn word(t: IrTy) -> IrTy {
    if t == IrTy::Packed {
        IrTy::Num(NumTy::U64)
    } else {
        t
    }
}

const U64: IrTy = IrTy::Num(NumTy::U64);

struct Emit<'a> {
    f: &'a mut IrFunc,
    out: Vec<Inst>,
}

impl Emit<'_> {
    fn reg(&mut self, ty: IrTy) -> VReg {
        self.f.new_reg(ty)
    }

    fn konst(&mut self, ty: IrTy, v: u64) -> VReg {
        let dst = self.reg(ty);
        self.out.push(Inst::Const { dst, val: Const::Bits(v) });
        dst
    }

    fn arith(&mut self, dst: Option<VReg>, op: ArithOp, lhs: VReg, rhs: VReg) -> VReg {
        let dst = dst.unwrap_or_else(|| self.f.new_reg(U64));
        self.out.push(Inst::Arith { dst, op, ty: NumTy::U64, lhs, rhs });
        dst
    }

    fn shift(&mut self, op: ArithOp, src: VReg, by: u32) -> VReg {
        let k = self.konst(IrTy::Num(NumTy::I64), by as u64);
        self.arith(None, op, src, k)
    }

    /// `dst = (tag, fields...)` as a word.
    fn pack(&mut self, dst: VReg, pk: &PackedEnum, tag: u32, fields: &[VReg]) {
        let mut parts: Vec<VReg> = Vec::new();
        for (f, &reg) in pk.variants[tag as usize].iter().zip(fields) {
            if f.bits == 0 {
                continue;
            }
            let from = match self.f.reg_ty(reg) {
                IrTy::Num(n) => n,
                IrTy::Bool => NumTy::U8,
                IrTy::Char => NumTy::U32,
                other => panic!("a packed field in a {other:?} register"),
            };
            // signed numbers are sign-extended in their register: cut them to size
            let mut v = reg;
            if from != NumTy::U64 {
                v = self.reg(U64);
                self.out.push(Inst::Cast { dst: v, from, to: NumTy::U64, src: reg });
            }
            if matches!(f.kind, PackedKind::Int { signed: true }) {
                let m = self.konst(U64, (1u64 << f.bits) - 1);
                v = self.arith(None, ArithOp::And, v, m);
            }
            if f.shift > 0 {
                v = self.shift(ArithOp::Shl, v, f.shift);
            }
            parts.push(v);
        }
        if parts.is_empty() {
            self.out.push(Inst::Const { dst, val: Const::Bits(tag as u64) });
            return;
        }
        // the tag is 0 for variant 0: no need to or it in
        let mut acc = parts[0];
        let rest = &parts[1..];
        if tag != 0 {
            let t = self.konst(U64, tag as u64);
            let last_is_acc = rest.is_empty();
            acc = self.arith(last_is_acc.then_some(dst), ArithOp::Or, acc, t);
            if last_is_acc {
                return;
            }
        }
        for (i, &p) in rest.iter().enumerate() {
            let last = i + 1 == rest.len();
            acc = self.arith(last.then_some(dst), ArithOp::Or, acc, p);
        }
        if rest.is_empty() && tag == 0 {
            self.out.push(Inst::Move { dst, src: acc });
        }
    }

    fn field(&mut self, dst: VReg, pk: &PackedEnum, obj: VReg, tag: u32, idx: u32) {
        let f = pk.variants[tag as usize][idx as usize];
        if f.bits == 0 {
            self.out.push(Inst::Const { dst, val: Const::Unit });
            return;
        }
        let at = if f.shift == 0 { obj } else { self.shift(ArithOp::Shr, obj, f.shift) };
        let to = match f.kind {
            PackedKind::Bool => NumTy::U8,
            PackedKind::Char => NumTy::U32,
            PackedKind::Int { signed } => NumTy::int(f.bits, signed),
            PackedKind::Unit => unreachable!("handled above"),
        };
        self.out.push(Inst::Cast { dst, from: NumTy::U64, to, src: at });
    }

    fn tag_of(&mut self, dst: Option<VReg>, obj: VReg) -> VReg {
        let m = self.konst(U64, 0xFF);
        self.arith(dst, ArithOp::And, obj, m)
    }
}

fn legalize_func(f: &mut IrFunc, layouts: &HashMap<DescId, Option<PackedEnum>>) -> bool {
    let any = f.regs.contains(&IrTy::Packed) || f.ret == IrTy::Packed || f.upvals.contains(&IrTy::Packed);
    let layout = |d: &DescId| layouts[d].as_ref().expect("a packed register holds a packed enum");
    for bi in 0..f.blocks.len() {
        let insts = std::mem::take(&mut f.blocks[bi].insts);
        let mut e = Emit { f, out: Vec::with_capacity(insts.len()) };
        for inst in insts {
            match inst {
                Inst::NewEnum { dst, desc, tag, fields } if e.f.reg_ty(dst) == IrTy::Packed => {
                    e.pack(dst, layout(&desc), tag, &fields);
                }
                Inst::EnumTag { dst, obj } if e.f.reg_ty(obj) == IrTy::Packed => {
                    e.tag_of(Some(dst), obj);
                }
                Inst::EnumField { dst, obj, idx, desc, tag } if e.f.reg_ty(obj) == IrTy::Packed => {
                    e.field(dst, layout(&desc), obj, tag, idx);
                }
                other => e.out.push(other),
            }
        }
        let mut out = e.out;
        if let Term::BranchTag { obj, tag, then, els } = f.blocks[bi].term {
            if f.reg_ty(obj) == IrTy::Packed {
                let mut e = Emit { f, out: Vec::new() };
                let t = e.tag_of(None, obj);
                let k = e.konst(U64, tag as u64);
                let c = e.reg(IrTy::Bool);
                e.out.push(Inst::Cmp { dst: c, op: CmpOp::Eq, ty: CmpTy::Num(NumTy::U64), lhs: t, rhs: k });
                out.append(&mut e.out);
                f.blocks[bi].term = Term::Branch { cond: c, then, els };
            }
        }
        f.blocks[bi].insts = out;
    }
    for t in f.regs.iter_mut().chain(f.upvals.iter_mut()) {
        *t = word(*t);
    }
    f.ret = word(f.ret);
    any
}
