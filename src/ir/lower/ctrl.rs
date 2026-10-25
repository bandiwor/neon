//! Conditionals and loops.

use super::func::{FnLower, LoopCtx};
use crate::builtin::Builtin;
use crate::ir::types::*;
use crate::runtime::{ArithOp, CmpOp, NumTy};
use crate::sema::{IterKind, TyKind};
use crate::syntax::{NodeId, NodeKind};

impl FnLower<'_, '_> {
    /// The register that receives an expression's value, if it has one.
    fn result_reg(&mut self, id: NodeId) -> Option<VReg> {
        let ty = self.node_ty(id);
        let ir = self.cx.ir_ty(ty);
        if matches!(self.cx.prog.types.kind(ty), TyKind::Never) || ir == IrTy::Unit {
            None
        } else {
            Some(self.new_reg(ir))
        }
    }

    fn move_result(&mut self, res: Option<VReg>, v: VReg) {
        if let Some(r) = res {
            if !self.sealed && self.f.reg_ty(v) == self.f.reg_ty(r) {
                self.emit(Inst::Move { dst: r, src: v });
            }
        }
    }

    pub(super) fn if_expr(&mut self, id: NodeId, cond: NodeId, then_b: NodeId, else_b: Option<NodeId>) -> VReg {
        let res = self.result_reg(id);
        let c = self.expr_fast(cond);
        let then_blk = self.new_block();
        let end = self.new_block();
        let else_blk = if else_b.is_some() { self.new_block() } else { end };
        self.terminate(Term::Branch { cond: c, then: then_blk, els: else_blk });

        self.switch_to(then_blk);
        let v = self.expr(then_b);
        self.move_result(res, v);
        self.jump(end);

        if let Some(e) = else_b {
            self.switch_to(else_blk);
            let v = self.expr(e);
            self.move_result(res, v);
            self.jump(end);
        }
        self.switch_to(end);
        match res {
            Some(r) => r,
            None => self.unit(),
        }
    }

    pub(super) fn while_loop(&mut self, cond: NodeId, body: NodeId) -> VReg {
        let head = self.new_block();
        let body_b = self.new_block();
        let end = self.new_block();
        self.jump(head);
        self.switch_to(head);
        let c = self.expr_fast(cond);
        self.terminate(Term::Branch { cond: c, then: body_b, els: end });
        self.switch_to(body_b);
        self.loops.push(LoopCtx { break_to: end, continue_to: head, scope_depth: self.scopes.len(), result: None });
        self.expr(body);
        self.loops.pop();
        self.jump(head);
        self.switch_to(end);
        self.unit()
    }

    pub(super) fn loop_expr(&mut self, id: NodeId, body: NodeId) -> VReg {
        let res = self.result_reg(id);
        let body_b = self.new_block();
        let end = self.new_block();
        self.jump(body_b);
        self.switch_to(body_b);
        self.loops.push(LoopCtx { break_to: end, continue_to: body_b, scope_depth: self.scopes.len(), result: res });
        self.expr(body);
        self.loops.pop();
        self.jump(body_b);
        self.switch_to(end);
        match res {
            Some(r) => r,
            None => self.unit(),
        }
    }

    pub(super) fn for_loop(&mut self, id: NodeId, pat: NodeId, iter: NodeId, body: NodeId) -> VReg {
        let kind = self.cx.prog.modules[self.module.index()].info.iter_kinds[&id];
        match kind {
            IterKind::Range => self.for_range(pat, iter, body),
            IterKind::Array => {
                let arr = self.expr(iter);
                let ty = self.node_ty(iter);
                self.for_array(pat, arr, ty, body)
            }
            IterKind::Set | IterKind::MapKeys => {
                let coll = self.expr(iter);
                let ty = self.node_ty(iter);
                let (b, elem) = match self.cx.prog.types.kind(ty).clone() {
                    TyKind::Set(e) => (Builtin::SetToArray, e),
                    TyKind::Map(k, _) => (Builtin::MapKeys, k),
                    _ => unreachable!("sema classified the iterable"),
                };
                let desc = self.cx.desc(ty);
                let arr = self.new_reg(IrTy::Ref);
                self.emit(Inst::Intrinsic { dst: arr, b, desc: Some(desc), args: vec![coll] });
                let arr_ty = self.cx.prog.types.array(elem);
                self.for_array(pat, arr, arr_ty, body)
            }
        }
    }

    fn for_array(&mut self, pat: NodeId, arr: VReg, arr_ty: crate::sema::Ty, body: NodeId) -> VReg {
        let desc = self.cx.desc(arr_ty);
        let i64t = IrTy::Num(NumTy::I64);
        let idx = self.new_reg(i64t);
        let zero = self.const_bits(i64t, 0);
        self.emit(Inst::Move { dst: idx, src: zero });
        let head = self.new_block();
        let body_b = self.new_block();
        let latch = self.new_block();
        let end = self.new_block();
        self.jump(head);

        self.switch_to(head);
        let len = self.new_reg(i64t);
        self.emit(Inst::Intrinsic { dst: len, b: Builtin::ArrLen, desc: Some(desc), args: vec![arr] });
        let more = self.new_reg(IrTy::Bool);
        self.emit(Inst::Cmp { dst: more, op: CmpOp::Lt, ty: CmpTy::Num(NumTy::I64), lhs: idx, rhs: len });
        self.terminate(Term::Branch { cond: more, then: body_b, els: end });

        self.switch_to(body_b);
        self.push_scope();
        let elem_ty = self.ir_ty_of_node(pat);
        let elem = self.new_reg(elem_ty);
        self.emit(Inst::ArrayGet { dst: elem, arr, idx, idx_ty: NumTy::I64 });
        self.bind_irrefutable(pat, elem);
        self.loops.push(LoopCtx {
            break_to: end,
            continue_to: latch,
            scope_depth: self.scopes.len() - 1,
            result: None,
        });
        self.expr(body);
        self.loops.pop();
        self.pop_scope();
        self.jump(latch);

        self.switch_to(latch);
        let one = self.const_bits(i64t, 1);
        self.emit(Inst::Arith { dst: idx, op: ArithOp::Add, ty: NumTy::I64, lhs: idx, rhs: one });
        self.jump(head);

        self.switch_to(end);
        self.unit()
    }

    fn for_range(&mut self, pat: NodeId, iter: NodeId, body: NodeId) -> VReg {
        let NodeKind::Range { start, end, inclusive } = *self.ast.kind(iter) else {
            unreachable!("sema checked the range")
        };
        let ir = self.ir_ty_of_node(pat);
        let IrTy::Num(num) = ir else {
            self.cx.error("internal error: non-integer range");
            return self.unit();
        };
        let s = self.expr(start.get().expect("sema requires range bounds"));
        let e = self.expr(end.get().expect("sema requires range bounds"));
        let i = self.new_reg(ir);
        self.emit(Inst::Move { dst: i, src: s });

        let body_b = self.new_block();
        let latch = self.new_block();
        let exit = self.new_block();
        let in_range = self.new_reg(IrTy::Bool);
        let op = if inclusive { CmpOp::Le } else { CmpOp::Lt };
        // initial check; the exclusive form re-checks at `latch`, the inclusive one at its end
        let head = if inclusive { None } else { Some(self.new_block()) };
        if let Some(h) = head {
            self.jump(h);
            self.switch_to(h);
        }
        self.emit(Inst::Cmp { dst: in_range, op, ty: CmpTy::Num(num), lhs: i, rhs: e });
        self.terminate(Term::Branch { cond: in_range, then: body_b, els: exit });

        self.switch_to(body_b);
        self.push_scope();
        self.bind_irrefutable(pat, i);
        self.loops.push(LoopCtx {
            break_to: exit,
            continue_to: latch,
            scope_depth: self.scopes.len() - 1,
            result: None,
        });
        self.expr(body);
        self.loops.pop();
        self.pop_scope();
        self.jump(latch);

        self.switch_to(latch);
        if inclusive {
            // stop before incrementing past the last value (which could overflow)
            let done = self.new_reg(IrTy::Bool);
            self.emit(Inst::Cmp { dst: done, op: CmpOp::Eq, ty: CmpTy::Num(num), lhs: i, rhs: e });
            let inc = self.new_block();
            self.terminate(Term::Branch { cond: done, then: exit, els: inc });
            self.switch_to(inc);
        }
        let one = self.const_bits(ir, 1);
        self.emit(Inst::Arith { dst: i, op: ArithOp::Add, ty: num, lhs: i, rhs: one });
        match head {
            Some(h) => self.jump(h),
            None => self.jump(body_b),
        }
        self.switch_to(exit);
        self.unit()
    }
}
