//! Patterns and `match`.

use super::func::{FnLower, Slot};
use crate::ir::types::*;
use crate::runtime::CmpOp;
use crate::sema::TyKind;
use crate::syntax::{NodeId, NodeKind};

impl FnLower<'_, '_> {
    /// Binds a pattern's variables to `val` without testing anything
    /// (sema guarantees `let`/`for`/parameter patterns are irrefutable).
    pub(super) fn bind_irrefutable(&mut self, pat: NodeId, val: VReg) {
        if let NodeKind::PatBinding { .. } = *self.ast.kind(pat) {
            self.bind(pat, val);
        }
    }

    fn bind(&mut self, pat: NodeId, val: VReg) {
        let Some(&l) = self.cx.prog.modules[self.module.index()].info.binding_locals.get(&pat) else {
            self.cx.error("internal error: pattern binding without a variable");
            return;
        };
        let reg = match self.locals.get(&l) {
            Some(Slot::Reg(r)) => *r,
            _ => self.declare_local(l),
        };
        if self.f.reg_ty(reg) == self.f.reg_ty(val) {
            self.emit(Inst::Move { dst: reg, src: val });
        }
    }

    /// Tests `val` against `pat`: falls through (in the current block) on a
    /// match, with the variables bound, and branches to `fail` otherwise.
    pub(super) fn lower_pat(&mut self, pat: NodeId, val: VReg, fail: BlockId) {
        let ast = self.ast.clone();
        match *ast.kind(pat) {
            NodeKind::PatWild => {}
            NodeKind::PatBinding { .. } => self.bind(pat, val),
            NodeKind::PatLit { expr } => {
                if matches!(ast.kind(expr), NodeKind::UnitLit) {
                    return;
                }
                let ty = self.node_ty(pat);
                let lit = self.expr(expr);
                let eq = self.builtin_cmp(CmpOp::Eq, ty, val, lit);
                self.branch_or_fail(eq, fail);
            }
            NodeKind::PatRange { lo, hi, inclusive } => {
                let ty = self.node_ty(pat);
                let lo_v = self.expr(lo);
                let ge = self.builtin_cmp(CmpOp::Ge, ty, val, lo_v);
                self.branch_or_fail(ge, fail);
                let hi_v = self.expr(hi);
                let le = self.builtin_cmp(if inclusive { CmpOp::Le } else { CmpOp::Lt }, ty, val, hi_v);
                self.branch_or_fail(le, fail);
            }
            NodeKind::PatVariant { fields, .. } => {
                let (adt, idx) = self.cx.prog.modules[self.module.index()].info.pat_variants[&pat];
                if self.cx.prog.adts[adt.index()].variants().len() > 1 {
                    let ok = self.new_block();
                    self.terminate(Term::BranchTag { obj: val, tag: idx, then: ok, els: fail });
                    self.switch_to(ok);
                }
                for (i, &sub) in ast.list(fields).iter().enumerate() {
                    if matches!(ast.kind(sub), NodeKind::PatWild) {
                        continue;
                    }
                    let ty = self.ir_ty_of_node(sub);
                    let fv = self.new_reg(ty);
                    self.emit(Inst::EnumField { dst: fv, obj: val, idx: i as u32 });
                    self.lower_pat(sub, fv, fail);
                }
            }
            NodeKind::PatOr { alts } => {
                let alts = ast.list(alts).to_vec();
                let ok = self.new_block();
                for (i, &alt) in alts.iter().enumerate() {
                    if i + 1 == alts.len() {
                        self.lower_pat(alt, val, fail);
                        self.jump(ok);
                    } else {
                        let next = self.new_block();
                        self.lower_pat(alt, val, next);
                        self.jump(ok);
                        self.switch_to(next);
                    }
                }
                self.switch_to(ok);
            }
            _ => self.cx.error("internal error: unexpected pattern node"),
        }
    }

    fn branch_or_fail(&mut self, cond: VReg, fail: BlockId) {
        let ok = self.new_block();
        self.terminate(Term::Branch { cond, then: ok, els: fail });
        self.switch_to(ok);
    }

    pub(super) fn match_expr(&mut self, id: NodeId, scrutinee: NodeId, arms: &[NodeId]) -> VReg {
        let ty = self.node_ty(id);
        let ir = self.cx.ir_ty(ty);
        let never = matches!(self.cx.prog.types.kind(ty), TyKind::Never);
        let res = if never || ir == IrTy::Unit { None } else { Some(self.new_reg(ir)) };
        let scrut = self.expr(scrutinee);
        let end = self.new_block();
        for &arm in arms {
            let NodeKind::MatchArm { pat, guard, body } = *self.ast.kind(arm) else { continue };
            let next = self.new_block();
            self.push_scope();
            self.lower_pat(pat, scrut, next);
            if let Some(g) = guard.get() {
                let gv = self.expr_fast(g);
                let go = self.new_block();
                self.terminate(Term::Branch { cond: gv, then: go, els: next });
                self.switch_to(go);
            }
            let v = self.expr(body);
            if let Some(r) = res {
                if !self.sealed && self.f.reg_ty(v) == ir {
                    self.emit(Inst::Move { dst: r, src: v });
                }
            }
            self.pop_scope();
            self.jump(end);
            self.switch_to(next);
        }
        // no arm matched: unreachable for exhaustive matches
        self.terminate(Term::Unreachable);
        self.switch_to(end);
        match res {
            Some(r) => r,
            None => self.unit(),
        }
    }
}
