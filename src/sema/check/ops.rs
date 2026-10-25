//! Unary, binary and assignment operators.
//!
//! Operators on primitives are built in. On any other type they dispatch to
//! the prelude traits (`Add`, `Eq`, `Ord`, ...): the checker records a
//! `Callee::Trait` and the trait solver picks the impl at monomorphization.

use super::Checker;
use crate::sema::builtins::Require;
use crate::sema::ids::*;
use crate::sema::infer::VarKind;
use crate::sema::program::*;
use crate::sema::types::{Prim, Ty, TyKind};
use crate::syntax::{BinOp, NodeId, NodeKind, UnOp};

fn lang_for(op: BinOp) -> Option<(LangTrait, &'static str)> {
    Some(match op {
        BinOp::Add => (LangTrait::Add, "add"),
        BinOp::Sub => (LangTrait::Sub, "sub"),
        BinOp::Mul => (LangTrait::Mul, "mul"),
        BinOp::Div => (LangTrait::Div, "div"),
        BinOp::Rem => (LangTrait::Rem, "rem"),
        BinOp::BitAnd => (LangTrait::BitAnd, "bitand"),
        BinOp::BitOr => (LangTrait::BitOr, "bitor"),
        BinOp::BitXor => (LangTrait::BitXor, "bitxor"),
        BinOp::Shl => (LangTrait::Shl, "shl"),
        BinOp::Shr => (LangTrait::Shr, "shr"),
        BinOp::Eq => (LangTrait::Eq, "eq"),
        BinOp::Ne => (LangTrait::Eq, "ne"),
        BinOp::Lt => (LangTrait::Ord, "lt"),
        BinOp::Le => (LangTrait::Ord, "le"),
        BinOp::Gt => (LangTrait::Ord, "gt"),
        BinOp::Ge => (LangTrait::Ord, "ge"),
        BinOp::And | BinOp::Or => return None,
    })
}

impl Checker<'_> {
    /// The prelude trait for an operator and its method.
    fn lang_method(&self, kind: LangTrait, method: &str) -> Option<(TraitId, FnId)> {
        let t = self.prog.lang.get(kind)?;
        let m = self.prog.traits[t.index()]
            .methods
            .iter()
            .copied()
            .find(|&m| self.interner.resolve(self.prog.fns[m.index()].name) == method)?;
        Some((t, m))
    }

    /// Whether `ty` is handled by the compiler's own operators (primitives,
    /// literal variables, unknowns) rather than by trait dispatch.
    fn is_builtin_operand(&self, ty: Ty) -> bool {
        matches!(self.kind(ty), TyKind::Prim(_) | TyKind::Infer(_) | TyKind::Error | TyKind::Never)
    }

    fn numeric_hint(&self, expected: Option<Ty>) -> Option<Ty> {
        let e = expected?;
        match self.kind(e) {
            TyKind::Prim(p) if p.is_numeric() => Some(e),
            TyKind::Infer(_) => Some(e),
            _ => None,
        }
    }

    pub fn unary(&mut self, id: NodeId, op: UnOp, operand: NodeId, expected: Option<Ty>) -> Ty {
        let span = self.span(id);
        if op == UnOp::Neg && matches!(self.ast.kind(operand), NodeKind::IntLit(_)) {
            self.negated.insert(operand);
        }
        let hint = if op == UnOp::Not { None } else { self.numeric_hint(expected) };
        let t = self.expr(operand, hint);
        if self.is_builtin_operand(t) {
            self.unary_checks.push((id, op, t));
            return t;
        }
        let (kind, name) = match op {
            UnOp::Neg => (LangTrait::Neg, "neg"),
            UnOp::Not => (LangTrait::Not, "not"),
            UnOp::BitNot => {
                let shown = self.show(t);
                self.err(
                    span,
                    "E0236",
                    format!("cannot apply unary operator `~` to type `{shown}`"),
                    "only integers support `~`",
                );
                return self.error_ty();
            }
        };
        match self.lang_method(kind, name) {
            Some((trait_id, method)) => {
                self.require_bound(t, TraitRef { trait_id, args: vec![] }, span);
                self.set_callee(id, Callee::Trait(method, vec![t]));
                t
            }
            None => {
                self.err(span, "E0236", "operator traits are not available", "missing prelude");
                self.error_ty()
            }
        }
    }

    pub fn binary(&mut self, id: NodeId, op: BinOp, lhs: NodeId, rhs: NodeId, expected: Option<Ty>) -> Ty {
        let boolt = self.prim(Prim::Bool);
        if op.is_logical() {
            self.expr_has_type(lhs, boolt);
            self.expr_has_type(rhs, boolt);
            return boolt;
        }
        if op.is_comparison() {
            self.comparison(id, op, lhs, rhs);
            return boolt;
        }
        let hint = self.numeric_hint(expected);
        let lt = self.expr(lhs, hint);
        self.arith_with(id, op, lt, rhs, false)
    }

    fn comparison(&mut self, id: NodeId, op: BinOp, lhs: NodeId, rhs: NodeId) {
        let span = self.span(id);
        let lt = self.expr(lhs, None);
        let rt = self.expr(rhs, Some(lt));
        let rspan = self.span(rhs);
        self.unify_at(rspan, lt, rt);
        if self.is_builtin_operand(lt) {
            let req = if matches!(op, BinOp::Eq | BinOp::Ne) { Require::Primitive } else { Require::Orderable };
            self.requires.push((req, lt, span, "comparison"));
            return;
        }
        let (kind, name) = lang_for(op).expect("comparison operators have traits");
        match self.lang_method(kind, name) {
            Some((trait_id, method)) => {
                self.require_bound(lt, TraitRef { trait_id, args: vec![] }, span);
                self.set_callee(id, Callee::Trait(method, vec![lt]));
            }
            None => self.err(span, "E0236", "operator traits are not available", "missing prelude"),
        }
    }

    /// `lhs op rhs` where `lhs` already has type `lt`. Returns the result type.
    /// `node` is the `Binary` or compound `Assign` node.
    fn arith_with(&mut self, node: NodeId, op: BinOp, lt: Ty, rhs: NodeId, _compound: bool) -> Ty {
        let span = self.span(node);
        let is_shift = matches!(op, BinOp::Shl | BinOp::Shr);
        if self.is_builtin_operand(lt) {
            if is_shift {
                let rt = self.expr(rhs, None);
                let ok = match self.kind(rt) {
                    TyKind::Prim(p) => p.is_int(),
                    TyKind::Infer(_) => {
                        let want = self.fresh_kind(VarKind::Int, span);
                        self.infer.unify(&mut self.prog.types, rt, want).is_ok()
                    }
                    TyKind::Error => true,
                    _ => false,
                };
                if !ok {
                    let shown = self.show(rt);
                    let s = self.span(rhs);
                    self.err(s, "E0236", format!("shift amount must be an integer, found `{shown}`"), "not an integer");
                }
            } else {
                let rt = self.expr(rhs, Some(lt));
                let s = self.span(rhs);
                self.unify_at(s, lt, rt);
            }
            self.arith.push((node, op, lt));
            return lt;
        }
        let (kind, name) = lang_for(op).expect("arithmetic operators have traits");
        let rt = self.expr(rhs, None);
        match self.lang_method(kind, name) {
            Some((trait_id, method)) => {
                self.require_bound(lt, TraitRef { trait_id, args: vec![rt] }, span);
                self.set_callee(node, Callee::Trait(method, vec![lt, rt]));
                lt
            }
            None => {
                self.err(span, "E0236", "operator traits are not available", "missing prelude");
                self.error_ty()
            }
        }
    }

    pub fn assign(&mut self, id: NodeId, op: Option<BinOp>, target: NodeId, value: NodeId) -> Ty {
        let tt = self.expr(target, None);
        self.check_mutable_place(target);
        match op {
            None => {
                self.expr_has_type(value, tt);
            }
            Some(bop) => {
                let result = self.arith_with(id, bop, tt, value, true);
                let span = self.span(id);
                self.unify_at(span, tt, result);
            }
        }
        self.unit()
    }

    /// Reports an error unless `node` is a place the program may mutate:
    /// the root variable of a field/index chain must be declared with `var`.
    pub fn check_mutable_place(&mut self, node: NodeId) {
        let ast = self.ast.clone();
        match *ast.kind(node) {
            NodeKind::Ident(name) => {
                let span = ast.span(node);
                let res = self.prog.modules[self.module.index()].info.res.get(&node).cloned();
                let shown = self.name(name).to_string();
                match res {
                    Some(Res::Local(l)) => {
                        if !self.prog.locals[l.index()].mutable {
                            self.err(
                                span,
                                "E0220",
                                format!("cannot assign to immutable variable `{shown}`"),
                                format!("declare it with `var {shown}`"),
                            );
                        }
                    }
                    Some(Res::Global(g)) => {
                        if !self.prog.globals[g.index()].mutable {
                            self.err(
                                span,
                                "E0220",
                                format!("cannot assign to immutable `{shown}`"),
                                "declared without `var`",
                            );
                        }
                    }
                    Some(_) => self.err(span, "E0220", format!("cannot assign to `{shown}`"), "not a variable"),
                    None => {}
                }
            }
            NodeKind::Field { base, .. } | NodeKind::Index { base, .. } => self.check_mutable_place(base),
            _ => {}
        }
    }
}
