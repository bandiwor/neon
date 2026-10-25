//! Expression checking.

use super::{Checker, Frame, LoopCtx};
use crate::diag::Span;
use crate::sema::infer::VarKind;
use crate::sema::program::*;
use crate::sema::types::{Prim, Ty, TyKind};
use crate::syntax::{FloatSuffix, IntSuffix, NodeId, NodeKind};

fn prim_of_int_suffix(s: IntSuffix) -> Prim {
    match s {
        IntSuffix::I8 => Prim::I8,
        IntSuffix::I16 => Prim::I16,
        IntSuffix::I32 => Prim::I32,
        IntSuffix::I64 => Prim::I64,
        IntSuffix::U8 => Prim::U8,
        IntSuffix::U16 => Prim::U16,
        IntSuffix::U32 => Prim::U32,
        IntSuffix::U64 => Prim::U64,
    }
}

impl Checker<'_> {
    /// Checks `id` and requires its type to equal `ty`.
    pub fn expr_has_type(&mut self, id: NodeId, ty: Ty) -> Ty {
        let t = self.expr(id, Some(ty));
        let span = self.span(id);
        self.unify_at(span, ty, t);
        t
    }

    /// Type of expression `id`, using `expected` only as a hint for literals,
    /// closures and empty collections.
    pub fn expr(&mut self, id: NodeId, expected: Option<Ty>) -> Ty {
        let ty = self.expr_inner(id, expected);
        self.set_ty(id, ty);
        ty
    }

    fn expr_inner(&mut self, id: NodeId, expected: Option<Ty>) -> Ty {
        let ast = self.ast.clone();
        let span = ast.span(id);
        match *ast.kind(id) {
            NodeKind::IntLit(lit) => {
                let l = ast.lits.int(lit);
                let ty = match l.suffix {
                    Some(s) => self.prim(prim_of_int_suffix(s)),
                    None => match expected.map(|e| self.shallow(e)) {
                        Some(e) if matches!(self.prog.types.kind(e), TyKind::Prim(p) if p.is_int()) => e,
                        _ => self.fresh_kind(VarKind::Int, span),
                    },
                };
                self.int_lits.push((id, ty));
                ty
            }
            NodeKind::FloatLit(lit) => {
                let l = ast.lits.float(lit);
                let ty = match l.suffix {
                    Some(FloatSuffix::F32) => self.prim(Prim::F32),
                    Some(FloatSuffix::F64) => self.prim(Prim::F64),
                    None => match expected.map(|e| self.shallow(e)) {
                        Some(e) if matches!(self.prog.types.kind(e), TyKind::Prim(p) if p.is_float()) => e,
                        _ => self.fresh_kind(VarKind::Float, span),
                    },
                };
                self.float_lits.push((id, ty));
                ty
            }
            NodeKind::StrLit(_) => self.prim(Prim::Str),
            NodeKind::CharLit(_) => self.prim(Prim::Char),
            NodeKind::BoolLit(_) => self.prim(Prim::Bool),
            NodeKind::UnitLit => self.unit(),
            NodeKind::Ident(_) | NodeKind::Path { .. } => self.value_path(id, expected),
            NodeKind::Unary { op, operand } => self.unary(id, op, operand, expected),
            NodeKind::Binary { op, lhs, rhs } => self.binary(id, op, lhs, rhs, expected),
            NodeKind::Assign { op, target, value } => self.assign(id, op, target, value),
            NodeKind::Cast { expr, ty } => {
                let from = self.expr(expr, None);
                let to = self.resolve_type(ty);
                self.casts.push((id, from, to));
                to
            }
            NodeKind::Call { callee, args } => self.call(id, callee, args, expected),
            NodeKind::MethodCall { receiver, name, generics, args } => {
                self.method_call(id, receiver, name, generics, args)
            }
            NodeKind::Field { base, name } => self.field(id, base, name),
            NodeKind::Index { base, index } => self.index(id, base, index),
            NodeKind::Try { expr } => self.try_expr(expr, span),
            NodeKind::Range { .. } => {
                self.err(span, "E0218", "ranges can only be used as the iterable of a `for` loop", "not allowed here");
                self.error_ty()
            }
            NodeKind::ArrayLit { elems } => {
                let elem = match expected.map(|e| self.kind(e)) {
                    Some(TyKind::Array(e)) => e,
                    _ => self.fresh(span),
                };
                for &e in ast.list(elems) {
                    let t = self.expr(e, Some(elem));
                    let s = self.span(e);
                    self.unify_at(s, elem, t);
                }
                self.prog.types.array(elem)
            }
            NodeKind::SetLit { elems } => {
                let elem = match expected.map(|e| self.kind(e)) {
                    Some(TyKind::Set(e)) => e,
                    _ => self.fresh(span),
                };
                for &e in ast.list(elems) {
                    let t = self.expr(e, Some(elem));
                    let s = self.span(e);
                    self.unify_at(s, elem, t);
                }
                self.requires.push((crate::sema::builtins::Require::Hashable, elem, span, "a set"));
                self.prog.types.set(elem)
            }
            NodeKind::MapLit { entries } => {
                let (k, v) = match expected.map(|e| self.kind(e)) {
                    Some(TyKind::Map(k, v)) => (k, v),
                    _ => (self.fresh(span), self.fresh(span)),
                };
                for &e in ast.list(entries) {
                    let NodeKind::MapEntry { key, value } = *ast.kind(e) else { continue };
                    let kt = self.expr(key, Some(k));
                    let ks = self.span(key);
                    self.unify_at(ks, k, kt);
                    let vt = self.expr(value, Some(v));
                    let vs = self.span(value);
                    self.unify_at(vs, v, vt);
                    self.set_ty(e, self.unit());
                }
                self.requires.push((crate::sema::builtins::Require::Hashable, k, span, "a map key"));
                self.prog.types.map(k, v)
            }
            NodeKind::StructLit { path, fields } => self.struct_lit(id, path, fields, expected),
            NodeKind::Closure { params, ret, body } => self.closure(id, params, ret.get(), body, expected),
            NodeKind::Block { .. } => self.block_value(id, expected),
            NodeKind::If { cond, then_branch, else_branch } => {
                self.expr_has_type(cond, self.prim(Prim::Bool));
                match else_branch.get() {
                    None => {
                        self.block_value(then_branch, None);
                        self.unit()
                    }
                    Some(e) => {
                        let mut acc: Option<Ty> = None;
                        let t1 = self.expr(then_branch, expected);
                        self.join(&mut acc, then_branch, t1);
                        let hint = acc.or(expected);
                        let t2 = self.expr(e, hint);
                        self.join(&mut acc, e, t2);
                        acc.unwrap_or_else(|| self.prog.types.never())
                    }
                }
            }
            NodeKind::While { cond, body } => {
                self.expr_has_type(cond, self.prim(Prim::Bool));
                self.loop_body(body, false);
                self.unit()
            }
            NodeKind::Loop { body } => {
                let ctx = self.loop_body(body, true);
                match ctx.break_ty {
                    Some(t) => t,
                    None if ctx.has_break => self.unit(),
                    None => self.prog.types.never(),
                }
            }
            NodeKind::For { pat, iter, body } => self.for_loop(id, pat, iter, body),
            NodeKind::Match { scrutinee, arms } => {
                let st = self.expr(scrutinee, None);
                let mut acc: Option<Ty> = None;
                for &arm in ast.list(arms) {
                    let NodeKind::MatchArm { pat, guard, body } = *ast.kind(arm) else { continue };
                    self.push_scope();
                    let mut ctx = super::PatCtx::default();
                    self.pattern(pat, st, &mut ctx);
                    if let Some(g) = guard.get() {
                        self.expr_has_type(g, self.prim(Prim::Bool));
                    }
                    let hint = acc.or(expected);
                    let bt = self.expr(body, hint);
                    self.join(&mut acc, body, bt);
                    self.pop_scope();
                    self.set_ty(arm, self.unit());
                }
                self.matches.push((id, st));
                acc.unwrap_or_else(|| self.prog.types.never())
            }
            NodeKind::Return { value } => {
                let ret = self.frames.last().map(|f| f.ret).unwrap_or_else(|| self.unit());
                if self.top_level && self.frames.len() == 1 {
                    self.err(span, "E0221", "`return` outside of a function", "not inside a function");
                } else {
                    match value.get() {
                        Some(v) => {
                            self.expr_has_type(v, ret);
                        }
                        None => {
                            let unit = self.unit();
                            if self.infer.unify(&mut self.prog.types, ret, unit).is_err() {
                                let r = self.show(ret);
                                self.err(
                                    span,
                                    "E0221",
                                    format!("`return` without a value in a function returning `{r}`"),
                                    "expected a value",
                                );
                            }
                        }
                    }
                }
                self.prog.types.never()
            }
            NodeKind::Break { value } => {
                let has_loop = self.frames.last().map(|f| !f.loops.is_empty()).unwrap_or(false);
                if !has_loop {
                    self.err(span, "E0232", "`break` outside of a loop", "not inside a loop");
                    if let Some(v) = value.get() {
                        self.expr(v, None);
                    }
                    return self.prog.types.never();
                }
                let allows = self.frame().loops.last().unwrap().allows_value;
                self.frame().loops.last_mut().unwrap().has_break = true;
                if let Some(v) = value.get() {
                    if !allows {
                        self.err(
                            span,
                            "E0232",
                            "`break` with a value is only allowed in `loop`",
                            "cannot break with a value here",
                        );
                        self.expr(v, None);
                    } else {
                        let current = self.frame().loops.last().unwrap().break_ty;
                        let hint = current;
                        let t = self.expr(v, hint);
                        match current {
                            Some(c) => {
                                let s = self.span(v);
                                self.unify_at(s, c, t);
                            }
                            None => self.frame().loops.last_mut().unwrap().break_ty = Some(t),
                        }
                    }
                }
                self.prog.types.never()
            }
            NodeKind::Continue => {
                let has_loop = self.frames.last().map(|f| !f.loops.is_empty()).unwrap_or(false);
                if !has_loop {
                    self.err(span, "E0232", "`continue` outside of a loop", "not inside a loop");
                }
                self.prog.types.never()
            }
            NodeKind::Error => self.error_ty(),
            _ => {
                self.err(span, "E0102", "expected an expression", "not an expression");
                self.error_ty()
            }
        }
    }

    /// Adds a branch of type `t` to the running join of `acc`.
    pub fn join(&mut self, acc: &mut Option<Ty>, node: NodeId, t: Ty) {
        if self.is_never(t) {
            return;
        }
        match *acc {
            None => *acc = Some(t),
            Some(a) => {
                let span = self.span(node);
                self.unify_at(span, a, t);
            }
        }
    }

    fn loop_body(&mut self, body: NodeId, allows_value: bool) -> LoopCtx {
        self.frame().loops.push(LoopCtx { break_ty: None, allows_value, has_break: false });
        self.block_value(body, None);
        self.frame().loops.pop().unwrap()
    }

    fn for_loop(&mut self, id: NodeId, pat: NodeId, iter: NodeId, body: NodeId) -> Ty {
        let ast = self.ast.clone();
        let (elem, kind) = if let NodeKind::Range { start, end, .. } = *ast.kind(iter) {
            let span = ast.span(iter);
            let elem = self.fresh_kind(VarKind::Int, span);
            for part in [start, end] {
                match part.get() {
                    Some(p) => {
                        self.expr_has_type(p, elem);
                    }
                    None => self.err(span, "E0218", "a `for` range needs both a start and an end", "unbounded range"),
                }
            }
            self.set_ty(iter, elem);
            (elem, IterKind::Range)
        } else {
            let t = self.expr(iter, None);
            match self.kind(t) {
                TyKind::Array(e) => (e, IterKind::Array),
                TyKind::Set(e) => (e, IterKind::Set),
                TyKind::Map(k, _) => (k, IterKind::MapKeys),
                TyKind::Error => (self.error_ty(), IterKind::Array),
                _ => {
                    let shown = self.show(t);
                    let span = ast.span(iter);
                    self.err(
                        span,
                        "E0231",
                        format!("cannot iterate over a value of type `{shown}`"),
                        "not iterable (try `.chars()`, `.keys()` or a range)",
                    );
                    (self.error_ty(), IterKind::Array)
                }
            }
        };
        self.info().iter_kinds.insert(id, kind);
        self.push_scope();
        let mut ctx = super::PatCtx::default();
        self.pattern(pat, elem, &mut ctx);
        if ctx.refutable {
            let span = ast.span(pat);
            self.err(span, "E0230", "refutable pattern in `for` loop", "use a simple binding");
        }
        self.loop_body(body, false);
        self.pop_scope();
        self.unit()
    }

    fn closure(
        &mut self,
        id: NodeId,
        params: crate::syntax::NodeList,
        ret: Option<NodeId>,
        body: NodeId,
        expected: Option<Ty>,
    ) -> Ty {
        let ast = self.ast.clone();
        let span = ast.span(id);
        let (exp_params, exp_ret) = match expected.map(|e| self.kind(e)) {
            Some(TyKind::Fn(ps, r)) if ps.len() == ast.list(params).len() => (Some(ps), Some(r)),
            _ => (None, None),
        };
        let ret_ty = match ret {
            Some(r) => self.resolve_type(r),
            None => exp_ret.unwrap_or_else(|| self.fresh(span)),
        };
        self.frames.push(Frame { owner: id, ret: ret_ty, loops: Vec::new(), captures: Vec::new() });
        self.push_scope();
        let mut ptys = Vec::new();
        let mut locals = Vec::new();
        for (i, &p) in ast.list(params).iter().enumerate() {
            let NodeKind::Param { name, mutable, ty } = *ast.kind(p) else { continue };
            let t = match ty.get() {
                Some(t) => {
                    let ann = self.resolve_type(t);
                    if let Some(ep) = exp_params.as_ref().map(|v| v[i]) {
                        let s = ast.span(p);
                        self.unify_at(s, ep, ann);
                    }
                    ann
                }
                None => exp_params.as_ref().map(|v| v[i]).unwrap_or_else(|| self.fresh(ast.span(p))),
            };
            let l = self.declare_local(name, ast.name_span(p), t, mutable);
            self.info().binding_locals.insert(p, l);
            self.set_ty(p, t);
            ptys.push(t);
            locals.push(l);
        }
        let body_ty = self.expr(body, Some(ret_ty));
        if !self.is_never(body_ty) {
            let s = ast.span(body);
            self.unify_at(s, ret_ty, body_ty);
        }
        self.pop_scope();
        let frame = self.frames.pop().unwrap();
        self.info().closures.insert(id, ClosureInfo { params: locals, captures: frame.captures, ret: ret_ty });
        self.closure_nodes.push(id);
        self.prog.types.func(ptys, ret_ty)
    }
}

impl Checker<'_> {
    /// `operand?`: the value inside an `Option` / `Result`, or an early return
    /// of the failure. The enclosing function must return the same family
    /// (`Option<_>` for an `Option`, `Result<_, E>` with the same `E` for a
    /// `Result`).
    fn try_expr(&mut self, operand: NodeId, span: Span) -> Ty {
        let t = self.expr(operand, None);
        let t = self.shallow(t);
        let la = self.prog.lang_adts;
        let (is_option, payload, err) = match self.prog.types.kind(t).clone() {
            TyKind::Adt(adt, args) if Some(adt) == la.option => (true, args[0], None),
            TyKind::Adt(adt, args) if Some(adt) == la.result => (false, args[0], Some(args[1])),
            TyKind::Error => return self.error_ty(),
            _ => {
                let shown = self.show(t);
                self.err(
                    span,
                    "E0260",
                    format!("the `?` operator needs an `Option` or a `Result`, found `{shown}`"),
                    "cannot use `?` here",
                );
                return self.error_ty();
            }
        };
        if self.top_level && self.frames.len() == 1 {
            self.err(span, "E0262", "`?` outside of a function", "there is no function to return from");
            return payload;
        }
        let ret = self.frames.last().map(|f| f.ret).unwrap_or_else(|| self.unit());
        let rest = self.fresh(span);
        let wanted = if is_option {
            self.prog.types.adt(la.option.expect("Option"), vec![rest])
        } else {
            self.prog.types.adt(la.result.expect("Result"), vec![rest, err.expect("error type")])
        };
        if self.infer.unify(&mut self.prog.types, ret, wanted).is_err() {
            let (family, shown_ret) = (if is_option { "Option" } else { "Result" }, self.show(ret));
            let help = if is_option {
                "the function must return an `Option<_>`; convert with `.ok_or(error)` to use `?` in a `Result` function"
            } else {
                "the function must return a `Result<_, E>` with the same error type"
            };
            self.err_help(
                span,
                "E0261",
                format!("`?` on a `{family}` cannot be used in a function that returns `{shown_ret}`"),
                "the early return does not fit the function's return type",
                help,
            );
        }
        payload
    }
}
