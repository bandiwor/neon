//! Enums that live in one word (see [`PackedEnum`]): construction, variant tests and payload reads are
//! plain integer operations, so they optimize like any arithmetic and never allocate.

use super::func::FnLower;
use crate::ir::*;
use crate::runtime::numeric::{ArithOp, CmpOp};
use crate::runtime::NumTy;

impl FnLower<'_, '_> {
    fn u64_const(&mut self, v: u64) -> VReg {
        self.const_bits(IrTy::Num(NumTy::U64), v)
    }

    fn u64_arith(&mut self, op: ArithOp, lhs: VReg, rhs: VReg) -> VReg {
        let dst = self.new_reg(IrTy::Num(NumTy::U64));
        self.emit(Inst::Arith { dst, op, ty: NumTy::U64, lhs, rhs });
        dst
    }

    /// The register type of a field stored in a packed word.
    fn packed_field_ty(f: &PackedField) -> IrTy {
        match f.kind {
            PackedKind::Bool => IrTy::Bool,
            PackedKind::Char => IrTy::Char,
            PackedKind::Unit => IrTy::Unit,
            PackedKind::Int { signed } => IrTy::Num(NumTy::int(f.bits, signed)),
        }
    }

    /// The word of variant `tag` with these field registers.
    pub(super) fn pack_enum(&mut self, pk: &PackedEnum, tag: u32, fields: &[VReg]) -> VReg {
        let mut acc = self.u64_const(tag as u64);
        for (f, &reg) in pk.variants[tag as usize].iter().zip(fields) {
            if f.bits == 0 {
                continue;
            }
            let from = match self.f.reg_ty(reg) {
                IrTy::Num(n) => n,
                IrTy::Bool => NumTy::U8,
                IrTy::Char => NumTy::U32,
                other => panic!("packed field in a {other:?} register"),
            };
            // widen to a word (signed numbers are sign-extended, so they are cut to size below)
            let wide = if from == NumTy::U64 {
                reg
            } else {
                let d = self.new_reg(IrTy::Num(NumTy::U64));
                self.emit(Inst::Cast { dst: d, from, to: NumTy::U64, src: reg });
                d
            };
            let masked = if matches!(f.kind, PackedKind::Int { signed: true }) {
                let m = self.u64_const((1u64 << f.bits) - 1);
                self.u64_arith(ArithOp::And, wide, m)
            } else {
                wide
            };
            let shifted = if f.shift == 0 {
                masked
            } else {
                let sh = self.const_bits(IrTy::Num(NumTy::I64), f.shift as u64);
                self.u64_arith(ArithOp::Shl, masked, sh)
            };
            acc = self.u64_arith(ArithOp::Or, acc, shifted);
        }
        acc
    }

    /// Jumps to `then` if the packed word `obj` is of variant `tag`, else to `els`.
    pub(super) fn packed_branch_tag(&mut self, obj: VReg, tag: u32, then: BlockId, els: BlockId) {
        let t = self.packed_tag(obj);
        let k = self.u64_const(tag as u64);
        let c = self.new_reg(IrTy::Bool);
        self.emit(Inst::Cmp { dst: c, op: CmpOp::Eq, ty: CmpTy::Num(NumTy::U64), lhs: t, rhs: k });
        self.terminate(Term::Branch { cond: c, then, els });
    }

    pub(super) fn packed_tag(&mut self, obj: VReg) -> VReg {
        let m = self.u64_const(0xFF);
        self.u64_arith(ArithOp::And, obj, m)
    }

    /// Field `idx` of variant `tag` of the packed word `obj`, in a register of type `ty`.
    pub(super) fn packed_field(&mut self, pk: &PackedEnum, obj: VReg, tag: u32, idx: u32) -> VReg {
        let f = pk.variants[tag as usize][idx as usize];
        let ty = Self::packed_field_ty(&f);
        if f.bits == 0 {
            return self.unit();
        }
        let shifted = if f.shift == 0 {
            obj
        } else {
            let sh = self.const_bits(IrTy::Num(NumTy::I64), f.shift as u64);
            self.u64_arith(ArithOp::Shr, obj, sh)
        };
        let to = match ty {
            IrTy::Num(n) => n,
            IrTy::Bool => NumTy::U8,
            IrTy::Char => NumTy::U32,
            _ => unreachable!(),
        };
        let dst = self.new_reg(ty);
        self.emit(Inst::Cast { dst, from: NumTy::U64, to, src: shifted });
        dst
    }

    /// A packed word built from a heap `Option` / `Result` object that a builtin returned.
    pub(super) fn pack_from_heap(&mut self, pk: &PackedEnum, desc: DescId, obj: VReg) -> VReg {
        let res = self.new_reg(IrTy::Num(NumTy::U64));
        let join = self.new_block();
        let n = pk.variants.len();
        for tag in 0..n as u32 {
            let last = tag as usize + 1 == n;
            let next = if last { None } else { Some(self.new_block()) };
            if let Some(next) = next {
                let ok = self.new_block();
                self.terminate(Term::BranchTag { obj, tag, then: ok, els: next });
                self.switch_to(ok);
            }
            let mut fields = Vec::new();
            for (i, f) in pk.variants[tag as usize].iter().enumerate() {
                let ty = Self::packed_field_ty(f);
                let r = self.new_reg(ty);
                self.emit(Inst::EnumField { dst: r, obj, idx: i as u32 });
                fields.push(r);
            }
            let w = self.pack_enum(pk, tag, &fields);
            self.emit(Inst::Move { dst: res, src: w });
            self.jump(join);
            if let Some(next) = next {
                self.switch_to(next);
            }
        }
        let _ = desc;
        self.switch_to(join);
        res
    }
}

/// Representation-independent enum operations: the same call sites build and inspect heap enums
/// and packed words.
impl FnLower<'_, '_> {
    /// A new value of the enum `desc`: variant `tag` with these fields.
    pub(super) fn enum_new(&mut self, desc: DescId, tag: u32, fields: Vec<VReg>) -> VReg {
        if let Some(pk) = self.cx.packed_desc(desc) {
            return self.pack_enum(&pk, tag, &fields);
        }
        let dst = self.new_reg(IrTy::Ref);
        self.emit(Inst::NewEnum { dst, desc, tag, fields });
        dst
    }

    /// Like [`Self::enum_new`], into an existing register (for results assigned on several paths).
    pub(super) fn enum_new_into(&mut self, dst: VReg, desc: DescId, tag: u32, fields: Vec<VReg>) {
        if let Some(pk) = self.cx.packed_desc(desc) {
            let w = self.pack_enum(&pk, tag, &fields);
            self.emit(Inst::Move { dst, src: w });
        } else {
            self.emit(Inst::NewEnum { dst, desc, tag, fields });
        }
    }

    /// Branches on the variant of an enum value of either representation.
    pub(super) fn enum_branch_tag(&mut self, obj: VReg, tag: u32, then: BlockId, els: BlockId) {
        if self.f.reg_ty(obj) == IrTy::Ref {
            self.terminate(Term::BranchTag { obj, tag, then, els });
        } else {
            self.packed_branch_tag(obj, tag, then, els);
        }
    }

    /// Field `idx` of variant `tag` of the enum value `obj` of type `desc`; `ty` is the register type the
    /// caller expects.
    pub(super) fn enum_get(&mut self, desc: DescId, obj: VReg, tag: u32, idx: u32, ty: IrTy) -> VReg {
        if let Some(pk) = self.cx.packed_desc(desc) {
            return self.packed_field(&pk, obj, tag, idx);
        }
        let dst = self.new_reg(ty);
        self.emit(Inst::EnumField { dst, obj, idx });
        dst
    }
}
