//! Enum construction and inspection for both representations. A packed enum (see [`PackedEnum`]) is
//! built and taken apart by the very same instructions as a heap enum, just in a register of type
//! `Packed`; the final `legalize` pass turns those into arithmetic after the optimizer is done with them.

use super::func::FnLower;
use crate::ir::*;

impl FnLower<'_, '_> {
    /// The register type of a value of the enum `desc`.
    fn enum_reg_ty(&mut self, desc: DescId) -> IrTy {
        if self.cx.packed_desc(desc).is_some() {
            IrTy::Packed
        } else {
            IrTy::Ref
        }
    }

    /// A new value of the enum `desc`: variant `tag` with these fields.
    pub(super) fn enum_new(&mut self, desc: DescId, tag: u32, fields: Vec<VReg>) -> VReg {
        let ty = self.enum_reg_ty(desc);
        let dst = self.new_reg(ty);
        self.emit(Inst::NewEnum { dst, desc, tag, fields });
        dst
    }

    /// Like [`Self::enum_new`], into an existing register (for results assigned on several paths).
    pub(super) fn enum_new_into(&mut self, dst: VReg, desc: DescId, tag: u32, fields: Vec<VReg>) {
        self.emit(Inst::NewEnum { dst, desc, tag, fields });
    }

    /// Branches on the variant of an enum value of either representation.
    pub(super) fn enum_branch_tag(&mut self, obj: VReg, tag: u32, then: BlockId, els: BlockId) {
        self.terminate(Term::BranchTag { obj, tag, then, els });
    }

    /// Field `idx` of variant `tag` of the enum value `obj` of type `desc`; `ty` is the register type the
    /// caller expects.
    pub(super) fn enum_get(&mut self, desc: DescId, obj: VReg, tag: u32, idx: u32, ty: IrTy) -> VReg {
        let dst = self.new_reg(ty);
        self.emit(Inst::EnumField { dst, obj, idx, desc, tag });
        dst
    }

    /// The value of the packed enum `desc` that a builtin returned as a heap object: the same variant
    /// and fields, as a word.
    pub(super) fn pack_from_heap(&mut self, desc: DescId, obj: VReg) -> VReg {
        let variants = match self.cx.out.descs[desc.index()].clone() {
            TypeDesc::Enum { variants, .. } => variants,
            other => panic!("expected an enum descriptor, got {other:?}"),
        };
        let res = self.new_reg(IrTy::Packed);
        let join = self.new_block();
        let n = variants.len();
        for (tag, (_, fields)) in variants.iter().enumerate() {
            let tag = tag as u32;
            let next = if tag as usize + 1 == n {
                None
            } else {
                let ok = self.new_block();
                let next = self.new_block();
                self.terminate(Term::BranchTag { obj, tag, then: ok, els: next });
                self.switch_to(ok);
                Some(next)
            };
            let mut regs = Vec::new();
            for (i, &fd) in fields.iter().enumerate() {
                let ty = self.cx.out.desc_ir_ty(fd);
                regs.push(self.enum_get(desc, obj, tag, i as u32, ty));
            }
            self.emit(Inst::NewEnum { dst: res, desc, tag, fields: regs });
            self.jump(join);
            if let Some(next) = next {
                self.switch_to(next);
            }
        }
        self.switch_to(join);
        res
    }
}
