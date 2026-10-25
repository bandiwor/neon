//! Resolution of identifiers and paths in expression position.

use super::Checker;
use crate::diag::Span;
use crate::sema::builtins::{builtin_const, builtin_fn};
use crate::sema::ids::*;
use crate::sema::program::*;
use crate::sema::traits::{instantiate_inherent, instantiate_trait, lookup_assoc_fn, MethodLookup};
use crate::sema::types::{Prim, Ty};
use crate::syntax::{Keyword, NodeId, NodeKind, NodeList, Symbol};

/// What a value path denotes.
#[derive(Debug, Clone)]
pub(in crate::sema) enum ValueRes {
    Local(LocalId),
    Global(GlobalId),
    Func { callee: Callee, params: Vec<Ty>, ret: Ty, min_args: usize },
    Variant { adt: AdtId, idx: u32, args: Vec<Ty>, fields: Vec<Ty> },
    Const(ConstVal, Ty),
    Err,
}

#[derive(Clone, Copy)]
struct Seg {
    name: Symbol,
    args: NodeList,
    span: Span,
}

impl Checker<'_> {
    fn segments(&self, node: NodeId) -> Vec<Seg> {
        match *self.ast.kind(node) {
            NodeKind::Ident(name) => vec![Seg { name, args: NodeList::EMPTY, span: self.span(node) }],
            NodeKind::Path { segments } => self
                .ast
                .list(segments)
                .iter()
                .map(|&s| {
                    let NodeKind::PathSegment { name, args } = *self.ast.kind(s) else { unreachable!() };
                    Seg { name, args, span: self.span(s) }
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    fn name_only(&self, seg: Seg) -> Span {
        Span::new(seg.span.file, seg.span.lo, seg.span.lo + self.name(seg.name).len() as u32)
    }

    fn type_args(&mut self, list: NodeList) -> Vec<Ty> {
        let ast = self.ast.clone();
        ast.list(list).iter().map(|&a| self.resolve_type(a)).collect()
    }

    /// Instantiates the generics of `fid` (after `prefix` owner arguments),
    /// returning all arguments plus the substituted signature, and requiring
    /// the callee's own bounds.
    pub fn instantiate_fn(
        &mut self,
        fid: FnId,
        prefix: Vec<Ty>,
        explicit: &[Ty],
        span: Span,
    ) -> (Vec<Ty>, Vec<Ty>, Ty) {
        let (total, params, ret, bounds) = {
            let f = &self.prog.fns[fid.index()];
            let bounds: Vec<Vec<TraitRef>> = f.generics.params.iter().map(|p| p.bounds.clone()).collect();
            (f.generics.len(), f.params.clone(), f.ret, bounds)
        };
        let own = total - prefix.len();
        let mut all = prefix;
        if !explicit.is_empty() && explicit.len() != own {
            self.err(
                span,
                "E0203",
                format!("expected {own} generic argument(s), found {}", explicit.len()),
                "wrong number of generic arguments",
            );
        }
        for i in 0..own {
            all.push(explicit.get(i).copied().unwrap_or_else(|| self.fresh(span)));
        }
        let params: Vec<Ty> = params.iter().map(|&p| self.prog.types.subst(p, &all)).collect();
        let ret = self.prog.types.subst(ret, &all);
        let start = total - own;
        for (i, bs) in bounds.iter().enumerate().skip(start) {
            for b in bs {
                let args = b.args.iter().map(|&a| self.prog.types.subst(a, &all)).collect();
                self.require_bound(all[i], TraitRef { trait_id: b.trait_id, args }, span);
            }
        }
        (all, params, ret)
    }

    fn fn_item(&mut self, fid: FnId, seg: Seg) -> ValueRes {
        let explicit = self.type_args(seg.args);
        let def = self.prog.fns[fid.index()].def;
        self.use_def(self.name_only(seg), def);
        let (all, params, ret) = self.instantiate_fn(fid, Vec::new(), &explicit, seg.span);
        let n = params.len();
        ValueRes::Func { callee: Callee::Fn(fid, all), params, ret, min_args: n }
    }

    fn variant_res(&mut self, adt: AdtId, idx: u32, explicit: &[Ty], span: Span) -> ValueRes {
        let n = self.prog.adts[adt.index()].generics.len();
        if !explicit.is_empty() && explicit.len() != n {
            self.err(
                span,
                "E0203",
                format!("expected {n} generic argument(s), found {}", explicit.len()),
                "wrong number of generic arguments",
            );
        }
        let args: Vec<Ty> = (0..n).map(|i| explicit.get(i).copied().unwrap_or_else(|| self.fresh(span))).collect();
        let fields = self.prog.adts[adt.index()].variants()[idx as usize].fields.clone();
        let fields = fields.iter().map(|&f| self.prog.types.subst(f, &args)).collect();
        let def = self.prog.adts[adt.index()].variants()[idx as usize].def;
        self.use_def(span, def);
        ValueRes::Variant { adt, idx, args, fields }
    }

    fn find_variant(&self, adt: AdtId, name: Symbol) -> Option<u32> {
        self.prog.adts[adt.index()].variants().iter().position(|v| v.name == name).map(|i| i as u32)
    }

    /// `Type::name` where `ty` is any type: a variant, builtin constant or
    /// associated function.
    fn assoc_of_type(&mut self, ty: Ty, ty_seg: Seg, seg: Seg) -> ValueRes {
        let name_str = self.name(seg.name).to_string();
        if let crate::sema::types::TyKind::Adt(adt, args) = self.kind(ty) {
            if let Some(idx) = self.find_variant(adt, seg.name) {
                let explicit = if ty_seg.args.is_empty() { Vec::new() } else { args };
                let sp = self.name_only(seg);
                return self.variant_res(adt, idx, &explicit, sp);
            }
        }
        if let crate::sema::types::TyKind::Prim(p) = self.kind(ty) {
            if let Some((v, vp)) = builtin_const(p, &name_str) {
                let t = self.prim(vp);
                return ValueRes::Const(v, t);
            }
        }
        let explicit = self.type_args(seg.args);
        let generics = self.generics.clone();
        let lookup = lookup_assoc_fn(self.prog, &mut self.infer, &generics, ty, seg.name);
        match lookup {
            MethodLookup::Inherent { impl_id, method } => {
                let impl_args = instantiate_inherent(self.prog, &mut self.infer, &generics, impl_id, ty);
                let def = self.prog.fns[method.index()].def;
                self.use_def(self.name_only(seg), def);
                let (all, params, ret) = self.instantiate_fn(method, impl_args, &explicit, seg.span);
                let n = params.len();
                ValueRes::Func { callee: Callee::Fn(method, all), params, ret, min_args: n }
            }
            MethodLookup::Trait { trait_id, method } => {
                let targs = instantiate_trait(self.prog, &mut self.infer, &generics, trait_id, ty);
                let mut prefix = vec![ty];
                prefix.extend(targs);
                let def = self.prog.fns[method.index()].def;
                self.use_def(self.name_only(seg), def);
                let (all, params, ret) = self.instantiate_fn(method, prefix, &explicit, seg.span);
                let n = params.len();
                ValueRes::Func { callee: Callee::Trait(method, all), params, ret, min_args: n }
            }
            MethodLookup::Ambiguous(_) => {
                self.err(seg.span, "E0213", format!("multiple applicable items named `{name_str}`"), "ambiguous");
                ValueRes::Err
            }
            MethodLookup::NotFound => {
                let shown = self.show(ty);
                self.err(
                    seg.span,
                    "E0214",
                    format!("no associated item named `{name_str}` found for type `{shown}`"),
                    "not found",
                );
                ValueRes::Err
            }
        }
    }

    /// `Trait::method`, with `Self` left to inference.
    fn trait_method(&mut self, trait_id: TraitId, seg: Seg) -> ValueRes {
        let name_str = self.name(seg.name).to_string();
        let method = self.prog.traits[trait_id.index()]
            .methods
            .iter()
            .copied()
            .find(|&m| self.prog.fns[m.index()].name == seg.name);
        let Some(method) = method else {
            self.err(seg.span, "E0214", format!("trait has no method named `{name_str}`"), "not found");
            return ValueRes::Err;
        };
        let explicit = self.type_args(seg.args);
        let n = self.prog.traits[trait_id.index()].generics.len();
        let prefix: Vec<Ty> = (0..n).map(|_| self.fresh(seg.span)).collect();
        let tr = TraitRef { trait_id, args: prefix[1..].to_vec() };
        self.require_bound(prefix[0], tr, seg.span);
        let (all, params, ret) = self.instantiate_fn(method, prefix, &explicit, seg.span);
        let k = params.len();
        ValueRes::Func { callee: Callee::Trait(method, all), params, ret, min_args: k }
    }

    fn module_item(&mut self, target: ModuleId, alias_seg: Seg, seg: Seg) -> Option<Item> {
        if let Some(&d) = self.prog.modules[self.module.index()].scope.alias_defs.get(&alias_seg.name) {
            self.use_def(self.name_only(alias_seg), d);
        }
        let item = self.prog.modules[target.index()].scope.exports.get(&seg.name).copied();
        if item.is_none() {
            let n = self.name(seg.name).to_string();
            let m = self.name(alias_seg.name).to_string();
            self.err(seg.span, "E0304", format!("module `{m}` has no exported item `{n}`"), "not exported");
        }
        item
    }

    fn item_value(&mut self, item: Item, seg: Seg) -> ValueRes {
        match item {
            Item::Fn(f) => self.fn_item(f, seg),
            Item::Global(g) => {
                let def = self.prog.globals[g.index()].def;
                self.use_def(self.name_only(seg), def);
                ValueRes::Global(g)
            }
            Item::Adt(_) | Item::Trait(_) => {
                let n = self.name(seg.name).to_string();
                self.err(seg.span, "E0210", format!("expected a value, found type or trait `{n}`"), "not a value");
                ValueRes::Err
            }
        }
    }

    /// Resolves an `Ident` or `Path` node in value position.
    pub(in crate::sema) fn resolve_value(&mut self, node: NodeId) -> ValueRes {
        let segs = self.segments(node);
        match segs.len() {
            1 => self.resolve_single(segs[0]),
            2 => self.resolve_two(segs[0], segs[1]),
            3 => {
                // alias::Enum::Variant
                let Some(&target) = self.prog.modules[self.module.index()].scope.aliases.get(&segs[0].name) else {
                    let n = self.name(segs[0].name).to_string();
                    self.err(segs[0].span, "E0210", format!("cannot find module `{n}`"), "not an imported module");
                    return ValueRes::Err;
                };
                match self.module_item(target, segs[0], segs[1]) {
                    Some(Item::Adt(adt)) => self.adt_assoc(adt, segs[1], segs[2]),
                    Some(_) => {
                        self.err(segs[1].span, "E0210", "expected an enum", "not an enum");
                        ValueRes::Err
                    }
                    None => ValueRes::Err,
                }
            }
            _ => {
                let span = self.span(node);
                self.err(span, "E0210", "path is too long", "too many segments");
                ValueRes::Err
            }
        }
    }

    fn resolve_single(&mut self, seg: Seg) -> ValueRes {
        if seg.args.is_empty() {
            if let Some(l) = self.lookup_local(seg.name) {
                let def = self.prog.locals[l.index()].def;
                self.use_def(seg.span, def);
                return ValueRes::Local(l);
            }
        }
        if let Some(item) = self.prog.lookup_item(self.module, seg.name) {
            return self.item_value(item, seg);
        }
        let name = self.name(seg.name).to_string();
        let generics_ok = seg.args.is_empty();
        if generics_ok {
            if let Some(b) = builtin_fn(self.prog, &mut self.infer, &name) {
                let args =
                    b.params.first().copied().filter(|_| b.min_args > 0 || name == "println").into_iter().collect();
                return ValueRes::Func {
                    callee: Callee::Builtin(b.builtin, args),
                    params: b.params,
                    ret: b.ret,
                    min_args: b.min_args,
                };
            }
        }
        self.err(seg.span, "E0210", format!("cannot find value `{name}` in this scope"), "not found in this scope");
        ValueRes::Err
    }

    fn adt_assoc(&mut self, adt: AdtId, ty_seg: Seg, seg: Seg) -> ValueRes {
        let explicit = self.type_args(ty_seg.args);
        let n = self.prog.adts[adt.index()].generics.len();
        let args: Vec<Ty> =
            (0..n).map(|i| explicit.get(i).copied().unwrap_or_else(|| self.fresh(ty_seg.span))).collect();
        let ty = self.prog.types.adt(adt, args);
        let def = self.prog.adts[adt.index()].def;
        self.use_def(self.name_only(ty_seg), def);
        self.assoc_of_type(ty, ty_seg, seg)
    }

    fn resolve_two(&mut self, first: Seg, second: Seg) -> ValueRes {
        // module alias
        if let Some(&target) = self.prog.modules[self.module.index()].scope.aliases.get(&first.name) {
            return match self.module_item(target, first, second) {
                Some(item) => self.item_value(item, second),
                None => ValueRes::Err,
            };
        }
        // Self::...
        if first.name == Symbol(Keyword::SelfType as u32) {
            return match self.self_ty {
                Some(t) => self.assoc_of_type(t, first, second),
                None => {
                    self.err(
                        first.span,
                        "E0205",
                        "`Self` is only available in impls and traits",
                        "not in an impl or trait",
                    );
                    ValueRes::Err
                }
            };
        }
        // primitive types: i32::MAX, f64::sqrt(..)
        if let Some(prim) = Keyword::from_symbol(first.name).and_then(Prim::from_keyword) {
            let t = self.prim(prim);
            return self.assoc_of_type(t, first, second);
        }
        // generic parameter: T::default()
        if let Some(i) = self.generics.params.iter().rposition(|p| p.name == first.name) {
            let t = self.prog.types.param(i as u32);
            if let Some(d) = self.generics.params[i].def {
                self.use_def(self.name_only(first), d);
            }
            return self.assoc_of_type(t, first, second);
        }
        match self.prog.lookup_item(self.module, first.name) {
            Some(Item::Adt(adt)) => self.adt_assoc(adt, first, second),
            Some(Item::Trait(t)) => {
                let def = self.prog.traits[t.index()].def;
                self.use_def(self.name_only(first), def);
                self.trait_method(t, second)
            }
            Some(_) => {
                let n = self.name(first.name).to_string();
                self.err(first.span, "E0210", format!("`{n}` is not a type, trait or module"), "cannot use `::` here");
                ValueRes::Err
            }
            None => {
                let n = self.name(first.name).to_string();
                self.err(first.span, "E0210", format!("cannot find `{n}` in this scope"), "not found");
                ValueRes::Err
            }
        }
    }

    /// The type of a value path, recording what it resolved to.
    pub fn value_path(&mut self, id: NodeId, _expected: Option<Ty>) -> Ty {
        let vr = self.resolve_value(id);
        self.value_of(id, vr)
    }

    pub(in crate::sema) fn value_of(&mut self, id: NodeId, vr: ValueRes) -> Ty {
        let span = self.span(id);
        match vr {
            ValueRes::Local(l) => {
                self.set_res(id, Res::Local(l));
                self.prog.locals[l.index()].ty
            }
            ValueRes::Global(g) => {
                self.set_res(id, Res::Global(g));
                self.prog.globals[g.index()].ty
            }
            ValueRes::Func { callee, params, ret, min_args } => {
                if min_args != params.len() || matches!(callee, Callee::Builtin(..)) {
                    self.err(span, "E0224", "builtin functions cannot be used as values", "call it directly");
                    return self.error_ty();
                }
                self.set_res(id, Res::Callee(callee));
                self.prog.types.func(params, ret)
            }
            ValueRes::Variant { adt, idx, args, fields } => {
                if !fields.is_empty() {
                    self.err(span, "E0224", "an enum variant with fields must be called like a function", "add `(..)`");
                    return self.error_ty();
                }
                self.set_res(id, Res::Variant { adt, idx, args: args.clone() });
                self.prog.types.adt(adt, args)
            }
            ValueRes::Const(v, ty) => {
                self.set_res(id, Res::Const(v));
                ty
            }
            ValueRes::Err => self.error_ty(),
        }
    }

    // ---- struct and variant paths (literals, patterns) ------------------------------------------

    /// Resolves the path of a struct literal to its struct and fresh generic arguments.
    pub fn resolve_struct_path(&mut self, node: NodeId, expected: Option<Ty>) -> Option<(AdtId, Vec<Ty>)> {
        let segs = self.segments(node);
        let last = *segs.last()?;
        let item = match segs.len() {
            1 if last.name == Symbol(Keyword::SelfType as u32) => {
                return match self.self_ty.map(|t| self.kind(t)) {
                    Some(crate::sema::types::TyKind::Adt(a, args)) if !self.prog.adts[a.index()].is_enum() => {
                        Some((a, args))
                    }
                    _ => {
                        self.err(last.span, "E0205", "`Self` is not a struct here", "not a struct");
                        None
                    }
                };
            }
            1 => self.prog.lookup_item(self.module, last.name),
            2 => match self.prog.modules[self.module.index()].scope.aliases.get(&segs[0].name).copied() {
                Some(t) => self.module_item(t, segs[0], last),
                None => None,
            },
            _ => None,
        };
        match item {
            Some(Item::Adt(adt)) if !self.prog.adts[adt.index()].is_enum() => {
                let def = self.prog.adts[adt.index()].def;
                self.use_def(self.name_only(last), def);
                let explicit = self.type_args(last.args);
                let n = self.prog.adts[adt.index()].generics.len();
                let args: Vec<Ty> =
                    (0..n).map(|i| explicit.get(i).copied().unwrap_or_else(|| self.fresh(last.span))).collect();
                if let Some(e) = expected {
                    if let crate::sema::types::TyKind::Adt(a, eargs) = self.kind(e) {
                        if a == adt {
                            for (&x, &y) in args.iter().zip(&eargs) {
                                let _ = self.infer.unify(&mut self.prog.types, x, y);
                            }
                        }
                    }
                }
                Some((adt, args))
            }
            _ => {
                let n = self.name(last.name).to_string();
                self.err(last.span, "E0210", format!("cannot find struct `{n}` in this scope"), "not a struct");
                None
            }
        }
    }

    /// Resolves a pattern path (`Enum::Variant`) to its variant.
    pub fn resolve_variant_path(&mut self, node: NodeId) -> Option<(AdtId, u32, Vec<Ty>)> {
        let segs = self.segments(node);
        let (adt_seg, var_seg, adt) = match segs.len() {
            2 => {
                let (a, v) = (segs[0], segs[1]);
                if a.name == Symbol(Keyword::SelfType as u32) {
                    match self.self_ty.map(|t| self.kind(t)) {
                        Some(crate::sema::types::TyKind::Adt(adt, _)) => (a, v, adt),
                        _ => {
                            self.err(a.span, "E0205", "`Self` is not an enum here", "not an enum");
                            return None;
                        }
                    }
                } else {
                    match self.prog.lookup_item(self.module, a.name) {
                        Some(Item::Adt(adt)) => (a, v, adt),
                        _ => {
                            let n = self.name(a.name).to_string();
                            self.err(a.span, "E0222", format!("cannot find enum `{n}` in this scope"), "not an enum");
                            return None;
                        }
                    }
                }
            }
            3 => {
                let Some(&target) = self.prog.modules[self.module.index()].scope.aliases.get(&segs[0].name) else {
                    self.err(segs[0].span, "E0222", "cannot find module", "not an imported module");
                    return None;
                };
                match self.module_item(target, segs[0], segs[1]) {
                    Some(Item::Adt(adt)) => (segs[1], segs[2], adt),
                    _ => return None,
                }
            }
            _ => {
                let n = self.name(segs[0].name).to_string();
                self.err(
                    segs[0].span,
                    "E0222",
                    format!("cannot find variant `{n}`; write the enum name too (e.g. `Option::Some`)"),
                    "variants are qualified by their enum",
                );
                return None;
            }
        };
        let Some(idx) = self.find_variant(adt, var_seg.name) else {
            let (e, v) = (self.name(adt_seg.name).to_string(), self.name(var_seg.name).to_string());
            self.err(var_seg.span, "E0222", format!("enum `{e}` has no variant named `{v}`"), "no such variant");
            return None;
        };
        let def = self.prog.adts[adt.index()].variants()[idx as usize].def;
        self.use_def(var_seg.span, def);
        let explicit = self.type_args(adt_seg.args);
        let n = self.prog.adts[adt.index()].generics.len();
        let args: Vec<Ty> =
            (0..n).map(|i| explicit.get(i).copied().unwrap_or_else(|| self.fresh(adt_seg.span))).collect();
        Some((adt, idx, args))
    }
}
