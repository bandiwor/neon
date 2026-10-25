//! Resolution of syntactic types (`NodeKind::Ty*`) to semantic [`Ty`]s.
//!
//! Diagnostic codes: E0202 expected a type, E0203 wrong number of generic
//! arguments, E0205 `Self` outside an impl or trait, E0209 `self` outside the
//! first parameter, E0204 expected a trait.

use super::ids::*;
use super::program::*;
use super::types::{Prim, Ty};
use crate::diag::{DiagBag, Diagnostic, Span};
use crate::syntax::{Ast, Interner, NodeId, NodeKind};

pub struct TypeResolver<'a> {
    pub prog: &'a mut Program,
    pub interner: &'a Interner,
    pub diags: &'a mut DiagBag,
    pub module: ModuleId,
    pub ast: &'a Ast,
    /// The generic parameters in scope (flattened, outermost first).
    pub generics: &'a Generics,
    pub self_ty: Option<Ty>,
}

impl TypeResolver<'_> {
    fn error(&mut self, span: Span, code: &'static str, message: String, label: &str) -> Ty {
        self.diags.report(Diagnostic::error(message).with_code(code).with_label(span, label));
        self.prog.types.error()
    }

    fn name(&self, s: crate::syntax::Symbol) -> &str {
        self.interner.resolve(s)
    }

    pub fn resolve(&mut self, node: NodeId) -> Ty {
        let span = self.ast.span(node);
        match *self.ast.kind(node) {
            NodeKind::TyPrim(k) => match Prim::from_keyword(k) {
                Some(p) => self.prog.types.prim(p),
                None => self.error(span, "E0202", format!("`{}` is not a type", k.as_str()), "expected a type"),
            },
            NodeKind::TyUnit => self.prog.types.unit(),
            NodeKind::TySelf => match self.self_ty {
                Some(t) => t,
                None => self.error(
                    span,
                    "E0205",
                    "`Self` is only available in impls and traits".to_string(),
                    "not in an impl or trait",
                ),
            },
            NodeKind::TyArray { elem } => {
                let e = self.resolve(elem);
                self.prog.types.array(e)
            }
            NodeKind::TySet { elem } => {
                let e = self.resolve(elem);
                self.prog.types.set(e)
            }
            NodeKind::TyMap { key, value } => {
                let k = self.resolve(key);
                let v = self.resolve(value);
                self.prog.types.map(k, v)
            }
            NodeKind::TyFn { params, ret } => {
                let ps: Vec<Ty> = self.ast.list(params).to_vec().into_iter().map(|p| self.resolve(p)).collect();
                let r = match ret.get() {
                    Some(r) => self.resolve(r),
                    None => self.prog.types.unit(),
                };
                self.prog.types.func(ps, r)
            }
            NodeKind::TyPath { segments } => self.resolve_path(node, segments),
            NodeKind::Error => self.prog.types.error(),
            _ => self.error(span, "E0202", "expected a type".to_string(), "not a type"),
        }
    }

    fn segment(&self, seg: NodeId) -> (crate::syntax::Symbol, crate::syntax::NodeList, Span) {
        match *self.ast.kind(seg) {
            NodeKind::PathSegment { name, args } => (name, args, self.ast.span(seg)),
            _ => unreachable!("type paths consist of path segments"),
        }
    }

    fn resolve_args(&mut self, args: crate::syntax::NodeList) -> Vec<Ty> {
        self.ast.list(args).to_vec().into_iter().map(|a| self.resolve(a)).collect()
    }

    fn resolve_path(&mut self, node: NodeId, segments: crate::syntax::NodeList) -> Ty {
        let segs = self.ast.list(segments).to_vec();
        let span = self.ast.span(node);
        let (name, args, seg_span) = self.segment(*segs.last().unwrap());

        let item = match segs.len() {
            1 => {
                if args.is_empty() {
                    if let Some(i) = self.generics.params.iter().rposition(|p| p.name == name) {
                        if let Some(d) = self.generics.params[i].def {
                            self.prog.symbols.add_use(name_only(seg_span, self.name(name).len()), d);
                        }
                        return self.prog.types.param(i as u32);
                    }
                }
                self.prog.lookup_item(self.module, name)
            }
            2 => {
                let (first, _, first_span) = self.segment(segs[0]);
                match self.prog.modules[self.module.index()].scope.aliases.get(&first).copied() {
                    Some(target) => {
                        if let Some(&d) = self.prog.modules[self.module.index()].scope.alias_defs.get(&first) {
                            self.prog.symbols.add_use(name_only(first_span, self.name(first).len()), d);
                        }
                        self.prog.modules[target.index()].scope.exports.get(&name).copied()
                    }
                    None => {
                        let n = self.name(first).to_string();
                        return self.error(
                            first_span,
                            "E0202",
                            format!("cannot find module `{n}`"),
                            "not an imported module",
                        );
                    }
                }
            }
            _ => {
                return self.error(
                    span,
                    "E0202",
                    "type paths have at most two segments".to_string(),
                    "too many segments",
                )
            }
        };

        let shown = self.name(name).to_string();
        match item {
            Some(Item::Adt(id)) => {
                let def = self.prog.adts[id.index()].def;
                self.prog.symbols.add_use(name_only(seg_span, shown.len()), def);
                let expected = self.prog.adts[id.index()].generics.len();
                let found = self.resolve_args(args);
                if found.len() != expected {
                    let msg = if expected == 0 {
                        format!("type `{shown}` takes no generic arguments")
                    } else {
                        format!("type `{shown}` takes {expected} generic argument(s) but {} were supplied", found.len())
                    };
                    return self.error(seg_span, "E0203", msg, "wrong number of generic arguments");
                }
                self.prog.types.adt(id, found)
            }
            Some(Item::Trait(_)) => {
                self.error(seg_span, "E0202", format!("expected a type, found trait `{shown}`"), "traits are not types")
            }
            Some(_) => self.error(seg_span, "E0202", format!("expected a type, found `{shown}`"), "not a type"),
            None => self.error(seg_span, "E0202", format!("cannot find type `{shown}` in this scope"), "not found"),
        }
    }

    /// Resolves a trait reference such as `Add<Vec2>` (a `TyPath` node).
    pub fn resolve_trait_ref(&mut self, node: NodeId) -> Option<TraitRef> {
        let span = self.ast.span(node);
        let NodeKind::TyPath { segments } = *self.ast.kind(node) else {
            self.error(span, "E0204", "expected a trait".to_string(), "not a trait");
            return None;
        };
        let segs = self.ast.list(segments).to_vec();
        let (name, args, seg_span) = self.segment(*segs.last().unwrap());
        let item = match segs.len() {
            1 => self.prog.lookup_item(self.module, name),
            2 => {
                let (first, _, _) = self.segment(segs[0]);
                let target = self.prog.modules[self.module.index()].scope.aliases.get(&first).copied();
                target.and_then(|t| self.prog.modules[t.index()].scope.exports.get(&name).copied())
            }
            _ => None,
        };
        let shown = self.name(name).to_string();
        match item {
            Some(Item::Trait(id)) => {
                let def = self.prog.traits[id.index()].def;
                self.prog.symbols.add_use(seg_span, def);
                let expected = self.prog.traits[id.index()].generics.len() - 1;
                let found = self.resolve_args(args);
                if found.len() != expected {
                    self.error(
                        seg_span,
                        "E0203",
                        format!(
                            "trait `{shown}` takes {expected} generic argument(s) but {} were supplied",
                            found.len()
                        ),
                        "wrong number of generic arguments",
                    );
                    return None;
                }
                Some(TraitRef { trait_id: id, args: found })
            }
            Some(_) => {
                self.error(seg_span, "E0204", format!("expected a trait, found `{shown}`"), "not a trait");
                None
            }
            None => {
                self.error(seg_span, "E0204", format!("cannot find trait `{shown}` in this scope"), "not found");
                None
            }
        }
    }
}

/// The first `len` bytes of a path segment's span: its name without generic arguments.
pub(super) fn name_only(seg: Span, len: usize) -> Span {
    Span::new(seg.file, seg.lo, seg.lo + len as u32)
}
