//! Assignment targets.

use super::expr::arith_op;
use super::func::{FnLower, Slot};
use crate::ir::types::*;
use crate::runtime::NumTy;
use crate::sema::{Callee, Res, TyKind};
use crate::syntax::{BinOp, NodeId, NodeKind};

/// A storage location, with its sub-expressions already evaluated.
pub(super) enum Place {
    Reg(VReg),
    Up(u32, IrTy),
    Global(GlobalSlot, IrTy),
    Field { obj: VReg, idx: u32, ty: IrTy },
    Array { arr: VReg, idx: VReg, idx_ty: NumTy, ty: IrTy },
    MapKey { map: VReg, key: VReg, ty: IrTy },
}

impl FnLower<'_, '_> {
    pub(super) fn place(&mut self, target: NodeId) -> Place {
        let ast = self.ast.clone();
        match *ast.kind(target) {
            NodeKind::Ident(_) => match self.res(target) {
                Some(Res::Local(l)) => match self.slot_of(l) {
                    Slot::Reg(r) => Place::Reg(r),
                    Slot::Up(i) => Place::Up(i, self.f.upvals[i as usize]),
                },
                Some(Res::Global(g)) => {
                    let slot = self.cx.global_slot(g);
                    let ty = self.cx.out.globals[slot.index()].ty;
                    Place::Global(slot, ty)
                }
                _ => {
                    self.cx.error("internal error: assignment to a non-variable");
                    Place::Reg(self.unit())
                }
            },
            NodeKind::Field { base, .. } => {
                let obj = self.expr_fast(base);
                let idx = self.cx.prog.modules[self.module.index()].info.field_indices[&target];
                let ty = self.ir_ty_of_node(target);
                Place::Field { obj, idx, ty }
            }
            NodeKind::Index { base, index } => {
                let regs = self.operands(&[base, index]);
                let bt = self.node_ty(base);
                let ty = self.ir_ty_of_node(target);
                match self.cx.prog.types.kind(bt) {
                    TyKind::Map(..) => Place::MapKey { map: regs[0], key: regs[1], ty },
                    _ => {
                        let idx_ty = self.index_num_ty(index);
                        Place::Array { arr: regs[0], idx: regs[1], idx_ty, ty }
                    }
                }
            }
            _ => {
                self.cx.error("internal error: invalid assignment target");
                Place::Reg(self.unit())
            }
        }
    }

    pub(super) fn load(&mut self, p: &Place) -> VReg {
        match *p {
            Place::Reg(r) => {
                let dst = self.new_reg(self.f.reg_ty(r));
                self.emit(Inst::Move { dst, src: r });
                dst
            }
            Place::Up(idx, ty) => {
                let dst = self.new_reg(ty);
                self.emit(Inst::GetUpval { dst, idx });
                dst
            }
            Place::Global(slot, ty) => {
                let dst = self.new_reg(ty);
                self.emit(Inst::GetGlobal { dst, slot });
                dst
            }
            Place::Field { obj, idx, ty } => {
                let dst = self.new_reg(ty);
                self.emit(Inst::GetField { dst, obj, idx });
                dst
            }
            Place::Array { arr, idx, idx_ty, ty } => {
                let dst = self.new_reg(ty);
                self.emit(Inst::ArrayGet { dst, arr, idx, idx_ty });
                dst
            }
            Place::MapKey { map, key, ty } => {
                let dst = self.new_reg(ty);
                self.emit(Inst::MapGet { dst, map, key });
                dst
            }
        }
    }

    pub(super) fn store(&mut self, p: &Place, v: VReg) {
        match *p {
            Place::Reg(r) => self.emit(Inst::Move { dst: r, src: v }),
            Place::Up(idx, _) => self.emit(Inst::SetUpval { idx, src: v }),
            Place::Global(slot, _) => self.emit(Inst::SetGlobal { slot, src: v }),
            Place::Field { obj, idx, .. } => self.emit(Inst::SetField { obj, idx, src: v }),
            Place::Array { arr, idx, idx_ty, .. } => self.emit(Inst::ArraySet { arr, idx, idx_ty, src: v }),
            Place::MapKey { map, key, .. } => self.emit(Inst::MapSet { map, key, val: v }),
        }
    }

    /// `target = value` and `target op= value`.
    pub(super) fn assign(&mut self, id: NodeId, op: Option<BinOp>, target: NodeId, value: NodeId) {
        match op {
            None => {
                let v = self.expr(value);
                let p = self.place(target);
                self.store(&p, v);
            }
            Some(bop) => {
                let p = self.place(target);
                // in-place update of a plain register: `x += 1` is one instruction
                if let (Place::Reg(r), None) = (&p, self.cx.prog.modules[self.module.index()].info.callees.get(&id)) {
                    let r = *r;
                    let rhs = self.expr(value);
                    let lt = self.node_ty(target);
                    let rt = self.node_ty(value);
                    let res = self.builtin_arith(arith_op(bop), lt, rt, r, rhs);
                    self.emit(Inst::Move { dst: r, src: res });
                    return;
                }
                let cur = self.load(&p);
                let rhs = self.expr(value);
                let callee = self.cx.prog.modules[self.module.index()].info.callees.get(&id).cloned();
                let result = match callee {
                    Some(c @ Callee::Trait(..)) => self.emit_callee(id, &c, vec![cur, rhs], None),
                    _ => {
                        let lt = self.node_ty(target);
                        let rt = self.node_ty(value);
                        self.builtin_arith(arith_op(bop), lt, rt, cur, rhs)
                    }
                };
                self.store(&p, result);
            }
        }
    }
}
