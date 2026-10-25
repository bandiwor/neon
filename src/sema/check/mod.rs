//! Type checking of function bodies and module top-level code.
//!
//! One [`Checker`] is created per function body (and per module init). It
//! resolves names through lexical scopes, infers types by unification, picks
//! overloads/impls, tracks closure captures and records its results in the
//! module's [`ModuleInfo`] once all inference variables are resolved.
//!
//! Diagnostic codes (E021x-E024x): see the individual checks.

mod call;
mod expr;
mod ops;
mod pat;
mod path;
mod stmt;

use pat::PatCtx;

use std::collections::HashSet;
use std::sync::Arc;

use super::builtins::Require;
use super::exhaust;
use super::ids::*;
use super::infer::{InferCtx, VarKind};
use super::program::*;
use super::traits::{solve_bound, Solved};
use super::typeres::TypeResolver;
use super::types::{Prim, Ty, TyKind};
use crate::diag::{DiagBag, Diagnostic, Span};
use crate::syntax::{Ast, BinOp, Interner, NodeId, NodeKind, Symbol, UnOp};

/// A `ty: trait` requirement collected while checking, solved at the end.
#[derive(Debug, Clone)]
pub(super) struct Obligation {
    pub ty: Ty,
    pub tr: TraitRef,
    pub span: Span,
}

#[derive(Debug)]
pub(super) struct LoopCtx {
    /// Type of `break value`s; `None` until the first one (and for `while`/`for`).
    pub break_ty: Option<Ty>,
    pub allows_value: bool,
    pub has_break: bool,
}

#[derive(Debug)]
pub(super) struct Frame {
    /// The `FnDecl`, closure or module node whose frame holds locals.
    pub owner: NodeId,
    pub ret: Ty,
    pub loops: Vec<LoopCtx>,
    pub captures: Vec<LocalId>,
}

pub(super) struct Checker<'a> {
    pub prog: &'a mut Program,
    pub interner: &'a Interner,
    pub diags: &'a mut DiagBag,
    pub module: ModuleId,
    pub ast: Arc<Ast>,
    pub infer: InferCtx,
    /// Generic parameters in scope: the bounds environment.
    pub generics: Generics,
    pub self_ty: Option<Ty>,
    pub scopes: Vec<Vec<(Symbol, LocalId)>>,
    pub frames: Vec<Frame>,
    pub top_level: bool,

    // bookkeeping to resolve at the end of the body
    pub touched: Vec<NodeId>,
    pub touched_res: Vec<NodeId>,
    pub touched_callees: Vec<NodeId>,
    pub new_locals: Vec<LocalId>,
    pub new_globals: Vec<GlobalId>,
    pub obligations: Vec<Obligation>,
    pub int_lits: Vec<(NodeId, Ty)>,
    pub float_lits: Vec<(NodeId, Ty)>,
    pub negated: HashSet<NodeId>,
    pub casts: Vec<(NodeId, Ty, Ty)>,
    pub arith: Vec<(NodeId, BinOp, Ty)>,
    pub unary_checks: Vec<(NodeId, UnOp, Ty)>,
    pub requires: Vec<(Require, Ty, Span, &'static str)>,
    pub matches: Vec<(NodeId, Ty)>,
    pub closure_nodes: Vec<NodeId>,
    pub reported_unresolved: bool,
}

impl<'a> Checker<'a> {
    pub fn new(
        prog: &'a mut Program,
        interner: &'a Interner,
        diags: &'a mut DiagBag,
        module: ModuleId,
        generics: Generics,
        self_ty: Option<Ty>,
    ) -> Self {
        let ast = prog.modules[module.index()].ast.clone();
        Checker {
            prog,
            interner,
            diags,
            module,
            ast,
            infer: InferCtx::new(),
            generics,
            self_ty,
            scopes: Vec::new(),
            frames: Vec::new(),
            top_level: false,
            touched: Vec::new(),
            touched_res: Vec::new(),
            touched_callees: Vec::new(),
            new_locals: Vec::new(),
            new_globals: Vec::new(),
            obligations: Vec::new(),
            int_lits: Vec::new(),
            float_lits: Vec::new(),
            negated: HashSet::new(),
            casts: Vec::new(),
            arith: Vec::new(),
            unary_checks: Vec::new(),
            requires: Vec::new(),
            matches: Vec::new(),
            closure_nodes: Vec::new(),
            reported_unresolved: false,
        }
    }

    // ---- diagnostics ------------------------------------------------------------------

    pub fn err(&mut self, span: Span, code: &'static str, message: impl Into<String>, label: impl Into<String>) {
        self.diags.report(Diagnostic::error(message).with_code(code).with_label(span, label));
    }

    pub fn name(&self, s: Symbol) -> &str {
        self.interner.resolve(s)
    }

    /// Renders a type for a message; unresolved literal variables read `{integer}`/`{float}`.
    pub fn show(&mut self, ty: Ty) -> String {
        let ty = self.infer.resolve(&mut self.prog.types, ty);
        let prog = &*self.prog;
        let infer = &self.infer;
        let interner = self.interner;
        let generics = &self.generics;
        prog.types.display(
            ty,
            &|a| interner.resolve(prog.adts[a.index()].name).to_string(),
            &|i| match generics.params.get(i as usize) {
                Some(p) => interner.resolve(p.name).to_string(),
                None => format!("T{i}"),
            },
            &|v| match infer.var_kind(v) {
                VarKind::Int => "{integer}".to_string(),
                VarKind::Float => "{float}".to_string(),
                VarKind::General => "_".to_string(),
            },
        )
    }

    pub fn mismatch(&mut self, span: Span, expected: Ty, found: Ty) {
        let (e, f) = (self.show(expected), self.show(found));
        self.err(span, "E0211", format!("mismatched types: expected `{e}`, found `{f}`"), format!("expected `{e}`"));
    }

    /// Unifies `found` with `expected`, reporting a mismatch at `span`.
    pub fn unify_at(&mut self, span: Span, expected: Ty, found: Ty) -> bool {
        match self.infer.unify(&mut self.prog.types, expected, found) {
            Ok(()) => true,
            Err(_) => {
                self.mismatch(span, expected, found);
                false
            }
        }
    }

    pub fn shallow(&self, ty: Ty) -> Ty {
        self.infer.shallow(&self.prog.types, ty)
    }

    pub fn kind(&self, ty: Ty) -> TyKind {
        self.prog.types.kind(self.shallow(ty)).clone()
    }

    pub fn is_never(&self, ty: Ty) -> bool {
        matches!(self.kind(ty), TyKind::Never)
    }

    pub fn fresh(&mut self, span: Span) -> Ty {
        self.infer.new_var(&mut self.prog.types, VarKind::General, span)
    }

    pub fn fresh_kind(&mut self, kind: VarKind, span: Span) -> Ty {
        self.infer.new_var(&mut self.prog.types, kind, span)
    }

    pub fn unit(&self) -> Ty {
        self.prog.types.unit()
    }

    pub fn error_ty(&self) -> Ty {
        self.prog.types.error()
    }

    pub fn prim(&self, p: Prim) -> Ty {
        self.prog.types.prim(p)
    }

    pub fn span(&self, node: NodeId) -> Span {
        self.ast.span(node)
    }

    // ---- results ------------------------------------------------------------------------

    pub fn set_ty(&mut self, node: NodeId, ty: Ty) {
        self.prog.modules[self.module.index()].info.node_types[node.0 as usize] = ty;
        self.touched.push(node);
    }

    pub fn ty_of(&self, node: NodeId) -> Ty {
        self.prog.modules[self.module.index()].info.node_types[node.0 as usize]
    }

    pub fn set_res(&mut self, node: NodeId, res: Res) {
        self.prog.modules[self.module.index()].info.res.insert(node, res);
        self.touched_res.push(node);
    }

    pub fn set_callee(&mut self, node: NodeId, callee: Callee) {
        self.prog.modules[self.module.index()].info.callees.insert(node, callee);
        self.touched_callees.push(node);
    }

    pub fn info(&mut self) -> &mut ModuleInfo {
        &mut self.prog.modules[self.module.index()].info
    }

    pub fn use_def(&mut self, span: Span, def: DefId) {
        self.prog.symbols.add_use(span, def);
    }

    pub fn resolve_type(&mut self, node: NodeId) -> Ty {
        let ast = self.ast.clone();
        let generics = self.generics.clone();
        let mut tr = TypeResolver {
            prog: &mut *self.prog,
            interner: self.interner,
            diags: &mut *self.diags,
            module: self.module,
            ast: &ast,
            generics: &generics,
            self_ty: self.self_ty,
        };
        tr.resolve(node)
    }

    // ---- scopes and locals ---------------------------------------------------------------

    pub fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    pub fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    pub fn frame(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("checking inside a frame")
    }

    pub fn declare_local(&mut self, name: Symbol, span: Span, ty: Ty, mutable: bool) -> LocalId {
        let owner = self.frames.last().expect("frame").owner;
        let id = LocalId(self.prog.locals.len() as u32);
        let def = self.prog.symbols.add_def(name, DefKind::Local(id), span, self.module);
        self.prog.locals.push(LocalDef { name, module: self.module, def, span, ty, mutable, owner, captured: false });
        self.new_locals.push(id);
        self.scopes.last_mut().expect("scope").push((name, id));
        id
    }

    /// Finds a visible local and records captures if it lives in an enclosing frame.
    pub fn lookup_local(&mut self, name: Symbol) -> Option<LocalId> {
        let found = self.scopes.iter().rev().find_map(|s| s.iter().rev().find(|(n, _)| *n == name).map(|&(_, l)| l))?;
        let owner = self.prog.locals[found.index()].owner;
        if let Some(pos) = self.frames.iter().position(|f| f.owner == owner) {
            if pos + 1 < self.frames.len() {
                self.prog.locals[found.index()].captured = true;
                for f in &mut self.frames[pos + 1..] {
                    if !f.captures.contains(&found) {
                        f.captures.push(found);
                    }
                }
            }
        }
        Some(found)
    }

    // ---- trait obligations ----------------------------------------------------------------

    /// Requires `ty: tr`; reports immediately when it can never hold, and
    /// otherwise remembers it to re-check once more is known.
    pub fn require_bound(&mut self, ty: Ty, tr: TraitRef, span: Span) {
        let generics = self.generics.clone();
        match solve_bound(self.prog, &mut self.infer, &generics, ty, &tr, 0) {
            Solved::Yes(_) => {}
            Solved::No => self.report_unsatisfied(ty, &tr, span),
            Solved::Maybe => self.obligations.push(Obligation { ty, tr, span }),
        }
    }

    fn report_unsatisfied(&mut self, ty: Ty, tr: &TraitRef, span: Span) {
        let t = self.show(ty);
        let name = self.name(self.prog.traits[tr.trait_id.index()].name).to_string();
        self.err(
            span,
            "E0237",
            format!("the trait bound `{t}: {name}` is not satisfied"),
            format!("`{t}` does not implement `{name}`"),
        );
    }

    fn solve_pending(&mut self, final_pass: bool) {
        loop {
            let pending = std::mem::take(&mut self.obligations);
            let before = pending.len();
            let generics = self.generics.clone();
            for ob in pending {
                match solve_bound(self.prog, &mut self.infer, &generics, ob.ty, &ob.tr, 0) {
                    Solved::Yes(_) => {}
                    Solved::No => self.report_unsatisfied(ob.ty, &ob.tr, ob.span),
                    Solved::Maybe => self.obligations.push(ob),
                }
            }
            if self.obligations.len() == before || self.obligations.is_empty() {
                break;
            }
        }
        if final_pass {
            for ob in std::mem::take(&mut self.obligations) {
                let t = self.show(ob.ty);
                self.err(
                    ob.span,
                    "E0212",
                    "type annotations needed",
                    format!("cannot determine whether `{t}` satisfies a trait bound"),
                );
            }
        }
    }

    // ---- finishing -------------------------------------------------------------------------

    /// Resolves everything inferred in this body and runs the deferred checks.
    pub fn finish(mut self) {
        self.solve_pending(false);
        self.infer.apply_literal_defaults(&mut self.prog.types);
        self.solve_pending(true);

        // unresolved variables in anything the program will use
        let touched = std::mem::take(&mut self.touched);
        for &n in &touched {
            let nt = self.ty_of(n);
            let t = self.infer.resolve(&mut self.prog.types, nt);
            if self.prog.types.has_infer(t) && !self.reported_unresolved {
                self.reported_unresolved = true;
                let span = self.span(n);
                self.err(span, "E0212", "type annotations needed", "cannot infer the type of this expression");
            }
        }

        // zonk node types
        let mut seen = HashSet::new();
        for n in touched {
            if seen.insert(n) {
                let nt = self.ty_of(n);
                let t = self.infer.resolve(&mut self.prog.types, nt);
                let t = self.erase_unresolved(t);
                self.prog.modules[self.module.index()].info.node_types[n.0 as usize] = t;
            }
        }
        for l in std::mem::take(&mut self.new_locals) {
            let t = self.infer.resolve(&mut self.prog.types, self.prog.locals[l.index()].ty);
            self.prog.locals[l.index()].ty = self.erase_unresolved(t);
        }
        for g in std::mem::take(&mut self.new_globals) {
            let t = self.infer.resolve(&mut self.prog.types, self.prog.globals[g.index()].ty);
            self.prog.globals[g.index()].ty = self.erase_unresolved(t);
        }
        for n in std::mem::take(&mut self.touched_res) {
            let Some(res) = self.prog.modules[self.module.index()].info.res.get(&n).cloned() else { continue };
            let res = match res {
                Res::Callee(c) => Res::Callee(self.zonk_callee(c)),
                Res::Variant { adt, idx, args } => Res::Variant { adt, idx, args: self.zonk_all(&args) },
                other => other,
            };
            self.prog.modules[self.module.index()].info.res.insert(n, res);
        }
        for n in std::mem::take(&mut self.touched_callees) {
            let Some(c) = self.prog.modules[self.module.index()].info.callees.get(&n).cloned() else { continue };
            let c = self.zonk_callee(c);
            self.prog.modules[self.module.index()].info.callees.insert(n, c);
        }
        for n in std::mem::take(&mut self.closure_nodes) {
            if let Some(mut ci) = self.prog.modules[self.module.index()].info.closures.remove(&n) {
                ci.ret = self.zonk(ci.ret);
                self.prog.modules[self.module.index()].info.closures.insert(n, ci);
            }
        }

        self.run_deferred_checks();
    }

    fn zonk(&mut self, t: Ty) -> Ty {
        let r = self.infer.resolve(&mut self.prog.types, t);
        self.erase_unresolved(r)
    }

    fn zonk_all(&mut self, ts: &[Ty]) -> Vec<Ty> {
        ts.iter().map(|&t| self.zonk(t)).collect()
    }

    fn zonk_callee(&mut self, c: Callee) -> Callee {
        match c {
            Callee::Fn(f, a) => Callee::Fn(f, self.zonk_all(&a)),
            Callee::Trait(f, a) => Callee::Trait(f, self.zonk_all(&a)),
            Callee::Builtin(b, a) => Callee::Builtin(b, self.zonk_all(&a)),
        }
    }

    /// After a reported "annotations needed" error, leftover variables become `Error`.
    fn erase_unresolved(&mut self, ty: Ty) -> Ty {
        if !self.prog.types.has_infer(ty) {
            return ty;
        }
        match self.prog.types.kind(ty).clone() {
            TyKind::Infer(_) => self.prog.types.error(),
            TyKind::Array(e) => {
                let e = self.erase_unresolved(e);
                self.prog.types.array(e)
            }
            TyKind::Set(e) => {
                let e = self.erase_unresolved(e);
                self.prog.types.set(e)
            }
            TyKind::Map(k, v) => {
                let k = self.erase_unresolved(k);
                let v = self.erase_unresolved(v);
                self.prog.types.map(k, v)
            }
            TyKind::Fn(ps, r) => {
                let ps = ps.iter().map(|&p| self.erase_unresolved(p)).collect();
                let r = self.erase_unresolved(r);
                self.prog.types.func(ps, r)
            }
            TyKind::Adt(id, args) => {
                let args = args.iter().map(|&p| self.erase_unresolved(p)).collect();
                self.prog.types.adt(id, args)
            }
            _ => ty,
        }
    }

    fn run_deferred_checks(&mut self) {
        // integer literal ranges
        for (node, ty) in std::mem::take(&mut self.int_lits) {
            let NodeKind::IntLit(id) = *self.ast.kind(node) else { continue };
            let value = self.ast.lits.int(id).value;
            let zt = self.zonk(ty);
            let Some(p) = self.prog.types.as_prim(zt) else { continue };
            if !p.is_int() {
                continue;
            }
            let negated = self.negated.contains(&node);
            let span = self.span(node);
            if negated && !p.is_signed() {
                self.err(
                    span,
                    "E0235",
                    format!("cannot negate an unsigned `{}` literal", p.name()),
                    "unsigned values cannot be negative",
                );
            } else {
                let limit = if negated { p.int_min_magnitude() } else { p.int_max() };
                if value > limit {
                    let shown = if negated { format!("-{value}") } else { value.to_string() };
                    self.err(
                        span,
                        "E0235",
                        format!("literal `{shown}` is out of range for `{}`", p.name()),
                        "out of range",
                    );
                }
            }
        }
        for (node, ty) in std::mem::take(&mut self.float_lits) {
            let NodeKind::FloatLit(id) = *self.ast.kind(node) else { continue };
            let v = self.ast.lits.float(id).value;
            let zt = self.zonk(ty);
            if self.prog.types.as_prim(zt) == Some(Prim::F32) && !(v as f32).is_finite() {
                let span = self.span(node);
                self.err(span, "E0235", "literal is out of range for `f32`", "out of range");
            }
        }
        for (node, from, to) in std::mem::take(&mut self.casts) {
            let (from, to) = (self.zonk(from), self.zonk(to));
            if !self.cast_allowed(from, to) {
                let (f, t) = (self.show(from), self.show(to));
                let span = self.span(node);
                self.err(span, "E0215", format!("cannot cast `{f}` as `{t}`"), "invalid cast");
            }
        }
        for (node, op, ty) in std::mem::take(&mut self.arith) {
            let ty = self.zonk(ty);
            let ok = match self.prog.types.kind(ty) {
                TyKind::Prim(p) => match op {
                    BinOp::Add => p.is_numeric() || *p == Prim::Str,
                    BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => p.is_numeric(),
                    BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => p.is_int() || *p == Prim::Bool,
                    BinOp::Shl | BinOp::Shr => p.is_int(),
                    _ => true,
                },
                TyKind::Error => true,
                _ => false,
            };
            if !ok {
                let t = self.show(ty);
                let span = self.span(node);
                self.err(
                    span,
                    "E0236",
                    format!("cannot apply operator `{}` to type `{t}`", op.as_str()),
                    "unsupported operand type",
                );
            }
        }
        for (node, op, ty) in std::mem::take(&mut self.unary_checks) {
            let ty = self.zonk(ty);
            let literal =
                matches!(self.ast.kind(node), NodeKind::Unary { operand, .. } if self.negated.contains(operand));
            let ok = match self.prog.types.kind(ty) {
                TyKind::Prim(p) => match op {
                    UnOp::Neg => p.is_signed() || p.is_float() || literal,
                    UnOp::Not => *p == Prim::Bool || p.is_int(),
                    UnOp::BitNot => p.is_int(),
                },
                TyKind::Error => true,
                _ => false,
            };
            if !ok {
                let t = self.show(ty);
                let span = self.span(node);
                self.err(
                    span,
                    "E0236",
                    format!("cannot apply unary operator `{}` to type `{t}`", op.as_str()),
                    "unsupported operand type",
                );
            }
        }
        for (req, ty, span, what) in std::mem::take(&mut self.requires) {
            let ty = self.zonk(ty);
            let ok = match (self.prog.types.kind(ty), req) {
                (TyKind::Error, _) => true,
                (TyKind::Prim(p), Require::Hashable) => p.is_int() || matches!(p, Prim::Bool | Prim::Char | Prim::Str),
                (TyKind::Prim(_), Require::Primitive) => true,
                (TyKind::Prim(p), Require::Orderable) => p.is_numeric() || matches!(p, Prim::Char | Prim::Str),
                (TyKind::Prim(Prim::Str), Require::Str) => true,
                _ => false,
            };
            if !ok {
                let t = self.show(ty);
                self.err(span, "E0219", format!("type `{t}` cannot be used with {what}"), "unsupported type");
            }
        }
        for (node, scrut_ty) in std::mem::take(&mut self.matches) {
            let ty = self.zonk(scrut_ty);
            exhaust::check_match(self.prog, self.interner, self.diags, self.module, node, ty);
        }
    }

    fn cast_allowed(&self, from: Ty, to: Ty) -> bool {
        let (tf, tt) = (self.prog.types.kind(from), self.prog.types.kind(to));
        match (tf, tt) {
            (TyKind::Error, _) | (_, TyKind::Error) => true,
            (TyKind::Prim(a), TyKind::Prim(b)) => {
                if a == b {
                    return true;
                }
                match (a, b) {
                    (a, b) if a.is_numeric() && b.is_numeric() => true,
                    (Prim::Bool, b) if b.is_int() => true,
                    (Prim::Char, b) if b.is_int() => true,
                    (Prim::U8, Prim::Char) => true,
                    _ => false,
                }
            }
            _ => false,
        }
    }
}

// ---- entry points ------------------------------------------------------------------------------

/// Checks the body of function `fid`.
pub fn check_fn_body(prog: &mut Program, interner: &Interner, diags: &mut DiagBag, fid: FnId) {
    let (module, node, generics, owner, ret, body) = {
        let f = &prog.fns[fid.index()];
        (f.module, f.node, f.generics.clone(), f.owner, f.ret, f.body)
    };
    let Some(body) = body else { return };
    let self_ty = match owner {
        FnOwner::Impl(i) => Some(prog.impls[i.index()].self_ty),
        FnOwner::Trait(_) => Some(prog.types.param(0)),
        _ => None,
    };
    let ptys = prog.fns[fid.index()].params.clone();
    let mut c = Checker::new(prog, interner, diags, module, generics, self_ty);
    c.frames.push(Frame { owner: node, ret, loops: Vec::new(), captures: Vec::new() });
    c.push_scope();

    let ast = c.ast.clone();
    let NodeKind::FnDecl { params, .. } = *ast.kind(node) else { return };
    let mut locals = Vec::new();
    for (&p, &ty) in ast.list(params).iter().zip(&ptys) {
        let NodeKind::Param { name, mutable, .. } = *ast.kind(p) else { continue };
        let l = c.declare_local(name, ast.name_span(p), ty, mutable);
        c.info().binding_locals.insert(p, l);
        locals.push(l);
    }
    c.prog.fns[fid.index()].param_locals = locals;

    let t = c.block_value(body, Some(ret));
    c.coerce_body(body, ret, t);
    c.pop_scope();
    c.finish();
}

/// Checks a module's top-level statements, declaring its globals.
pub fn check_module_init(prog: &mut Program, interner: &Interner, diags: &mut DiagBag, module: ModuleId) {
    let init = prog.modules[module.index()].init;
    let root = prog.fns[init.index()].node;
    let unit = prog.types.unit();
    let mut c = Checker::new(prog, interner, diags, module, Generics::default(), None);
    c.top_level = true;
    c.frames.push(Frame { owner: root, ret: unit, loops: Vec::new(), captures: Vec::new() });
    c.push_scope();
    let ast = c.ast.clone();
    let NodeKind::Module { items } = *ast.kind(root) else { return };
    for &item in ast.list(items) {
        c.top_level_item(item);
    }
    c.pop_scope();
    c.finish();
}
