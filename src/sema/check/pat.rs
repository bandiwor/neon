//! Pattern checking: declares bindings and types every pattern node against
//! the scrutinee type.

use std::collections::HashMap;

use super::Checker;
use crate::sema::ids::*;
use crate::sema::types::{Prim, Ty, TyKind};
use crate::syntax::{NodeId, NodeKind, Symbol};

#[derive(Debug, Default)]
pub(in crate::sema) struct PatCtx {
    /// While checking the 2nd+ alternative of an or-pattern: the variables
    /// the first alternative bound, to be reused.
    pub or_names: Option<HashMap<Symbol, LocalId>>,
    /// Variables bound by the alternative being checked.
    pub bound: Vec<(Symbol, LocalId)>,
    /// The pattern can fail to match.
    pub refutable: bool,
}

impl Checker<'_> {
    pub fn pattern(&mut self, pat: NodeId, expected: Ty, ctx: &mut PatCtx) {
        let ast = self.ast.clone();
        let span = ast.span(pat);
        self.set_ty(pat, expected);
        match *ast.kind(pat) {
            NodeKind::PatWild => {}
            NodeKind::PatBinding { name, mutable } => {
                let reused = ctx.or_names.as_ref().and_then(|m| m.get(&name).copied());
                let local = match reused {
                    Some(l) => {
                        let lty = self.prog.locals[l.index()].ty;
                        self.unify_at(span, lty, expected);
                        l
                    }
                    None => self.declare_local(name, ast.name_span(pat), expected, mutable),
                };
                self.info().binding_locals.insert(pat, local);
                ctx.bound.push((name, local));
            }
            NodeKind::PatLit { expr } => {
                let t = self.expr(expr, Some(expected));
                self.unify_at(span, expected, t);
                let ok =
                    matches!(self.kind(expected), TyKind::Prim(_) | TyKind::Unit | TyKind::Infer(_) | TyKind::Error);
                if !ok {
                    let shown = self.show(expected);
                    self.err(
                        span,
                        "E0211",
                        format!("literal patterns cannot match values of type `{shown}`"),
                        "unsupported pattern",
                    );
                }
                if !matches!(ast.kind(expr), NodeKind::UnitLit) {
                    ctx.refutable = true;
                }
            }
            NodeKind::PatRange { lo, hi, .. } => {
                for bound in [lo, hi] {
                    if let NodeKind::Unary { operand, .. } = *ast.kind(bound) {
                        self.negated.insert(operand);
                    }
                    let t = self.expr(bound, Some(expected));
                    let s = self.span(bound);
                    self.unify_at(s, expected, t);
                }
                let ok = match self.kind(expected) {
                    TyKind::Prim(p) => p.is_int() || p == Prim::Char,
                    TyKind::Infer(_) | TyKind::Error => true,
                    _ => false,
                };
                if !ok {
                    let shown = self.show(expected);
                    self.err(
                        span,
                        "E0211",
                        format!("range patterns match integers and characters, not `{shown}`"),
                        "unsupported pattern",
                    );
                }
                ctx.refutable = true;
            }
            NodeKind::PatVariant { path, fields, parens } => {
                self.variant_pattern(pat, path, fields, parens, expected, ctx)
            }
            NodeKind::PatOr { alts } => {
                let alts = ast.list(alts).to_vec();
                let mut first_names: Option<HashMap<Symbol, LocalId>> = None;
                let mut first_set: Vec<Symbol> = Vec::new();
                for (i, &alt) in alts.iter().enumerate() {
                    let mut sub = PatCtx {
                        or_names: first_names.clone().or_else(|| ctx.or_names.clone()),
                        bound: Vec::new(),
                        refutable: false,
                    };
                    self.pattern(alt, expected, &mut sub);
                    ctx.refutable |= sub.refutable;
                    if i == 0 {
                        first_set = sub.bound.iter().map(|(n, _)| *n).collect();
                        first_names = Some(sub.bound.iter().copied().collect());
                        ctx.bound.extend(sub.bound);
                    } else {
                        let mut names: Vec<Symbol> = sub.bound.iter().map(|(n, _)| *n).collect();
                        names.sort_by_key(|s| s.0);
                        let mut want = first_set.clone();
                        want.sort_by_key(|s| s.0);
                        if names != want {
                            let alt_span = self.span(alt);
                            self.err(
                                alt_span,
                                "E0234",
                                "all alternatives of an or-pattern must bind the same variables",
                                "binds a different set of variables",
                            );
                        }
                    }
                }
                ctx.refutable = true;
            }
            _ => {
                self.err(span, "E0104", "expected a pattern", "not a pattern");
            }
        }
    }

    fn variant_pattern(
        &mut self,
        pat: NodeId,
        path: NodeId,
        fields: crate::syntax::NodeList,
        parens: bool,
        expected: Ty,
        ctx: &mut PatCtx,
    ) {
        let ast = self.ast.clone();
        let span = ast.span(pat);
        let subs = ast.list(fields).to_vec();
        let Some((adt, idx, args)) = self.resolve_variant_path(path) else {
            for &s in &subs {
                let mut sub = PatCtx::default();
                let e = self.error_ty();
                self.pattern(s, e, &mut sub);
            }
            ctx.refutable = true;
            return;
        };
        let ty = self.prog.types.adt(adt, args.clone());
        self.set_ty(path, ty);
        self.unify_at(span, expected, ty);
        self.info().pat_variants.insert(pat, (adt, idx));
        let ftys = self.prog.adts[adt.index()].variants()[idx as usize].fields.clone();
        if ftys.len() != subs.len() || (!parens && !ftys.is_empty()) {
            let name = self.name(self.prog.adts[adt.index()].variants()[idx as usize].name).to_string();
            let want = ftys.len();
            self.err(
                span,
                "E0233",
                format!("variant `{name}` has {want} field(s) but the pattern has {}", subs.len()),
                "wrong number of fields",
            );
        }
        for (i, &s) in subs.iter().enumerate() {
            let fty = match ftys.get(i) {
                Some(&f) => self.prog.types.subst(f, &args),
                None => self.error_ty(),
            };
            self.pattern(s, fty, ctx);
        }
        if self.prog.adts[adt.index()].variants().len() > 1 {
            ctx.refutable = true;
        }
    }
}
