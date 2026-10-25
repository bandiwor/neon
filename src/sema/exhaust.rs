//! Match exhaustiveness and reachability.
//!
//! Patterns are normalized to constructor trees and checked with the classic
//! "usefulness" algorithm (Maranget). Enum variants and booleans form finite
//! constructor sets; integers, characters, strings and floats are infinite
//! domains, so only a wildcard can complete them.
//!
//! Diagnostic codes: E0240 non-exhaustive patterns, W0241 unreachable pattern
//! (a warning).

use super::ids::*;
use super::program::*;
use super::types::{Ty, TyKind};
use crate::diag::{DiagBag, Diagnostic, Span};
use crate::syntax::{Ast, Interner, NodeId, NodeKind, UnOp};

#[derive(Debug, Clone, PartialEq)]
enum Ctor {
    Variant(u32),
    Bool(bool),
    Int(i128),
    Str(String),
    Float(u64),
    Range(i128, i128),
}

impl Ctor {
    /// Whether a row headed by `self` matches every value that `other`
    /// (a constructor being specialized) matches.
    fn covers(&self, other: &Ctor) -> bool {
        match (self, other) {
            (Ctor::Range(lo, hi), Ctor::Int(n)) => lo <= n && n <= hi,
            (Ctor::Range(lo, hi), Ctor::Range(a, b)) => lo <= a && b <= hi,
            (Ctor::Int(n), Ctor::Range(a, b)) => a == b && a == n,
            (a, b) => a == b,
        }
    }
}

#[derive(Debug, Clone)]
enum P {
    Wild,
    Ctor(Ctor, Vec<P>),
    Or(Vec<P>),
}

/// A pattern missing from a match, for the error message.
#[derive(Debug, Clone)]
enum W {
    Wild,
    Ctor(String, Vec<W>),
}

impl W {
    fn show(&self) -> String {
        match self {
            W::Wild => "_".to_string(),
            W::Ctor(name, subs) if subs.is_empty() => name.clone(),
            W::Ctor(name, subs) => format!("{name}({})", subs.iter().map(W::show).collect::<Vec<_>>().join(", ")),
        }
    }
}

struct Env<'a> {
    prog: &'a mut Program,
    interner: &'a Interner,
    ast: &'a Ast,
    module: ModuleId,
}

/// A finite constructor set: (constructor, field types, display name).
type Universe = Vec<(Ctor, Vec<Ty>, String)>;

impl Env<'_> {
    fn literal_ctor(&self, expr: NodeId) -> Option<Ctor> {
        let ast = self.ast;
        match *ast.kind(expr) {
            NodeKind::IntLit(id) => Some(Ctor::Int(ast.lits.int(id).value as i128)),
            NodeKind::CharLit(c) => Some(Ctor::Int(c as i128)),
            NodeKind::BoolLit(b) => Some(Ctor::Bool(b)),
            NodeKind::StrLit(id) => Some(Ctor::Str(ast.lits.str(id).to_string())),
            NodeKind::FloatLit(id) => Some(Ctor::Float(ast.lits.float(id).value.to_bits())),
            NodeKind::Unary { op: UnOp::Neg, operand } => match self.literal_ctor(operand)? {
                Ctor::Int(n) => Some(Ctor::Int(-n)),
                Ctor::Float(bits) => Some(Ctor::Float((-f64::from_bits(bits)).to_bits())),
                _ => None,
            },
            _ => None,
        }
    }

    fn convert(&self, pat: NodeId) -> P {
        let ast = self.ast;
        match *ast.kind(pat) {
            NodeKind::PatWild | NodeKind::PatBinding { .. } => P::Wild,
            NodeKind::PatLit { expr } => match self.literal_ctor(expr) {
                Some(c) => P::Ctor(c, Vec::new()),
                None => P::Wild, // unit
            },
            NodeKind::PatRange { lo, hi, inclusive } => match (self.literal_ctor(lo), self.literal_ctor(hi)) {
                (Some(Ctor::Int(a)), Some(Ctor::Int(b))) => {
                    let b = if inclusive { b } else { b - 1 };
                    P::Ctor(Ctor::Range(a, b), Vec::new())
                }
                _ => P::Wild,
            },
            NodeKind::PatVariant { fields, .. } => {
                let Some(&(_, idx)) = self.prog.modules[self.module.index()].info.pat_variants.get(&pat) else {
                    return P::Wild;
                };
                let subs = ast.list(fields).iter().map(|&f| self.convert(f)).collect();
                P::Ctor(Ctor::Variant(idx), subs)
            }
            NodeKind::PatOr { alts } => P::Or(ast.list(alts).iter().map(|&a| self.convert(a)).collect()),
            _ => P::Wild,
        }
    }

    /// The finite constructor set of `ty`, or `None` for infinite domains.
    fn universe(&mut self, ty: Ty) -> Option<Universe> {
        match self.prog.types.kind(ty).clone() {
            TyKind::Prim(super::types::Prim::Bool) => Some(vec![
                (Ctor::Bool(false), vec![], "false".to_string()),
                (Ctor::Bool(true), vec![], "true".to_string()),
            ]),
            TyKind::Adt(adt, args) if self.prog.adts[adt.index()].is_enum() => {
                let enum_name = self.interner.resolve(self.prog.adts[adt.index()].name).to_string();
                let variants: Vec<(String, Vec<Ty>)> = self.prog.adts[adt.index()]
                    .variants()
                    .iter()
                    .map(|v| (self.interner.resolve(v.name).to_string(), v.fields.clone()))
                    .collect();
                Some(
                    variants
                        .into_iter()
                        .enumerate()
                        .map(|(i, (name, fields))| {
                            let ftys = fields.iter().map(|&f| self.prog.types.subst(f, &args)).collect();
                            (Ctor::Variant(i as u32), ftys, format!("{enum_name}::{name}"))
                        })
                        .collect(),
                )
            }
            _ => None,
        }
    }

    fn arity_and_tys(&mut self, uni: &Option<Universe>, c: &Ctor) -> Vec<Ty> {
        match uni {
            Some(u) => u.iter().find(|(k, _, _)| k == c).map(|(_, t, _)| t.clone()).unwrap_or_default(),
            None => Vec::new(),
        }
    }

    // ---- matrix operations --------------------------------------------------------------

    fn expand_or(rows: Vec<Vec<P>>) -> Vec<Vec<P>> {
        let mut out = Vec::new();
        for mut row in rows {
            if row.is_empty() {
                out.push(row);
                continue;
            }
            match row.remove(0) {
                P::Or(alts) => {
                    let tail = row;
                    let expanded: Vec<Vec<P>> = alts
                        .into_iter()
                        .map(|a| {
                            let mut r = vec![a];
                            r.extend(tail.iter().cloned());
                            r
                        })
                        .collect();
                    out.extend(Self::expand_or(expanded));
                }
                head => {
                    row.insert(0, head);
                    out.push(row);
                }
            }
        }
        out
    }

    fn specialize(rows: &[Vec<P>], c: &Ctor, arity: usize) -> Vec<Vec<P>> {
        let mut out = Vec::new();
        for row in rows {
            match &row[0] {
                P::Wild => {
                    let mut r = vec![P::Wild; arity];
                    r.extend(row[1..].iter().cloned());
                    out.push(r);
                }
                P::Ctor(h, subs) if h.covers(c) => {
                    let mut r: Vec<P> = if subs.len() == arity { subs.clone() } else { vec![P::Wild; arity] };
                    r.extend(row[1..].iter().cloned());
                    out.push(r);
                }
                _ => {}
            }
        }
        out
    }

    fn default_matrix(rows: &[Vec<P>]) -> Vec<Vec<P>> {
        rows.iter().filter(|r| matches!(r[0], P::Wild)).map(|r| r[1..].to_vec()).collect()
    }

    /// Whether `v` matches some value no row of `rows` matches.
    fn useful(&mut self, rows: Vec<Vec<P>>, v: Vec<P>, tys: &[Ty]) -> bool {
        if v.is_empty() {
            return rows.is_empty();
        }
        let rows = Self::expand_or(rows);
        let mut v = v;
        let head = v.remove(0);
        let rest = v;
        match head {
            P::Or(alts) => alts.into_iter().any(|a| {
                let mut nv = vec![a];
                nv.extend(rest.iter().cloned());
                self.useful(rows.clone(), nv, tys)
            }),
            P::Ctor(c, subs) => {
                let uni = self.universe(tys[0]);
                let ftys = self.arity_and_tys(&uni, &c);
                let arity = subs.len().max(ftys.len());
                let spec = Self::specialize(&rows, &c, arity);
                let mut nv = subs;
                nv.resize(arity, P::Wild);
                nv.extend(rest);
                let mut ntys = ftys;
                ntys.resize(arity, tys[0]);
                ntys.extend_from_slice(&tys[1..]);
                self.useful(spec, nv, &ntys)
            }
            P::Wild => {
                let uni = self.universe(tys[0]);
                let used: Vec<Ctor> = rows
                    .iter()
                    .filter_map(|r| if let P::Ctor(c, _) = &r[0] { Some(c.clone()) } else { None })
                    .collect();
                match &uni {
                    Some(all) if all.iter().all(|(c, _, _)| used.iter().any(|u| u.covers(c))) => {
                        all.clone().into_iter().any(|(c, ftys, _)| {
                            let spec = Self::specialize(&rows, &c, ftys.len());
                            let mut nv = vec![P::Wild; ftys.len()];
                            nv.extend(rest.iter().cloned());
                            let mut ntys = ftys;
                            ntys.extend_from_slice(&tys[1..]);
                            self.useful(spec, nv, &ntys)
                        })
                    }
                    _ => self.useful(Self::default_matrix(&rows), rest, &tys[1..]),
                }
            }
        }
    }

    /// A pattern vector no row matches, if any.
    fn missing(&mut self, rows: Vec<Vec<P>>, tys: &[Ty]) -> Option<Vec<W>> {
        if tys.is_empty() {
            return if rows.is_empty() { Some(Vec::new()) } else { None };
        }
        let rows = Self::expand_or(rows);
        let uni = self.universe(tys[0]);
        let used: Vec<Ctor> =
            rows.iter().filter_map(|r| if let P::Ctor(c, _) = &r[0] { Some(c.clone()) } else { None }).collect();
        match uni {
            Some(all) => {
                let complete = all.iter().all(|(c, _, _)| used.iter().any(|u| u.covers(c)));
                if complete {
                    for (c, ftys, name) in all {
                        let spec = Self::specialize(&rows, &c, ftys.len());
                        let mut ntys = ftys.clone();
                        ntys.extend_from_slice(&tys[1..]);
                        if let Some(mut w) = self.missing(spec, &ntys) {
                            let rest = w.split_off(ftys.len());
                            let mut out = vec![W::Ctor(name, w)];
                            out.extend(rest);
                            return Some(out);
                        }
                    }
                    None
                } else {
                    let mut w = self.missing(Self::default_matrix(&rows), &tys[1..])?;
                    let (c, ftys, name) = all
                        .into_iter()
                        .find(|(c, _, _)| !used.iter().any(|u| u.covers(c)))
                        .expect("incomplete signature has a missing constructor");
                    let _ = c;
                    let mut out = vec![W::Ctor(name, vec![W::Wild; ftys.len()])];
                    out.append(&mut w);
                    Some(out)
                }
            }
            None => {
                let mut w = self.missing(Self::default_matrix(&rows), &tys[1..])?;
                let mut out = vec![W::Wild];
                out.append(&mut w);
                Some(out)
            }
        }
    }
}

/// Checks one `match` expression whose scrutinee has type `scrut_ty`.
pub fn check_match(
    prog: &mut Program,
    interner: &Interner,
    diags: &mut DiagBag,
    module: ModuleId,
    node: NodeId,
    scrut_ty: Ty,
) {
    let ast = prog.modules[module.index()].ast.clone();
    let NodeKind::Match { arms, .. } = *ast.kind(node) else { return };
    if matches!(prog.types.kind(scrut_ty), TyKind::Error) {
        return;
    }
    let mut env = Env { prog, interner, ast: &ast, module };
    let mut rows: Vec<Vec<P>> = Vec::new();
    for &arm in ast.list(arms) {
        let NodeKind::MatchArm { pat, guard, .. } = *ast.kind(arm) else { continue };
        let p = env.convert(pat);
        let tys = [scrut_ty];
        if !env.useful(rows.clone(), vec![p.clone()], &tys) {
            diags.report(
                Diagnostic::warning("unreachable pattern")
                    .with_code("W0241")
                    .with_label(ast.span(pat), "this arm can never match"),
            );
        }
        if guard.is_none() {
            rows.push(vec![p]);
        }
    }
    if let Some(w) = env.missing(rows, &[scrut_ty]) {
        let shown = w.first().map(W::show).unwrap_or_else(|| "_".to_string());
        let span: Span = ast.span(node);
        diags.report(
            Diagnostic::error(format!("non-exhaustive patterns: `{shown}` not covered"))
                .with_code("E0240")
                .with_label(span, format!("pattern `{shown}` is not handled"))
                .with_help("add an arm for it, or a wildcard `_` arm"),
        );
    }
}
