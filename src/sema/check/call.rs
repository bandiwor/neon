//! Calls, method calls, struct literals, field access and indexing.

use std::collections::HashSet;

use super::path::ValueRes;
use super::Checker;
use crate::diag::Span;
use crate::sema::builtins::builtin_method;
use crate::sema::ids::*;
use crate::sema::infer::VarKind;
use crate::sema::program::*;
use crate::sema::traits::{instantiate_inherent, instantiate_trait, lookup_method, MethodLookup};
use crate::sema::types::{Prim, Ty, TyKind};
use crate::syntax::{NodeId, NodeKind, NodeList, Symbol};

impl Checker<'_> {
    /// Checks call arguments against `params`. Closures are checked last so
    /// the other arguments can fix the types they depend on.
    fn check_args(&mut self, call_span: Span, args: &[NodeId], params: &[Ty], min_args: usize, what: &str) {
        if args.len() < min_args || args.len() > params.len() {
            let expected = if min_args == params.len() {
                params.len().to_string()
            } else {
                format!("{min_args} to {}", params.len())
            };
            self.err(
                call_span,
                "E0216",
                format!("{what} takes {expected} argument(s) but {} were supplied", args.len()),
                format!("expected {expected} argument(s)"),
            );
        }
        let ast = self.ast.clone();
        for pass in 0..2 {
            for (i, &a) in args.iter().enumerate() {
                let is_closure = matches!(ast.kind(a), NodeKind::Closure { .. });
                if is_closure != (pass == 1) {
                    continue;
                }
                match params.get(i) {
                    Some(&p) => {
                        self.expr_has_type(a, p);
                    }
                    None => {
                        self.expr(a, None);
                    }
                }
            }
        }
    }

    pub fn call(&mut self, id: NodeId, callee: NodeId, args: NodeList, _expected: Option<Ty>) -> Ty {
        let ast = self.ast.clone();
        let arg_nodes = ast.list(args).to_vec();
        let span = ast.span(id);
        if matches!(ast.kind(callee), NodeKind::Ident(_) | NodeKind::Path { .. }) {
            let vr = self.resolve_value(callee);
            match vr {
                ValueRes::Func { callee: c, params, ret, min_args } => {
                    let fn_ty = self.prog.types.func(params.clone(), ret);
                    self.set_res(callee, Res::Callee(c));
                    self.set_ty(callee, fn_ty);
                    self.check_args(span, &arg_nodes, &params, min_args, "this function");
                    // optional trailing parameters that were omitted stay unconstrained
                    for &p in params.iter().skip(arg_nodes.len()) {
                        let unit = self.unit();
                        let _ = self.infer.unify(&mut self.prog.types, p, unit);
                    }
                    return ret;
                }
                ValueRes::Variant { adt, idx, args: targs, fields } => {
                    let ty = self.prog.types.adt(adt, targs.clone());
                    self.set_res(callee, Res::Variant { adt, idx, args: targs });
                    let fn_ty = self.prog.types.func(fields.clone(), ty);
                    self.set_ty(callee, fn_ty);
                    let n = fields.len();
                    self.check_args(span, &arg_nodes, &fields, n, "this enum variant");
                    return ty;
                }
                ValueRes::Err => {
                    self.set_ty(callee, self.error_ty());
                    for &a in &arg_nodes {
                        self.expr(a, None);
                    }
                    return self.error_ty();
                }
                other => {
                    let t = self.value_of(callee, other);
                    self.set_ty(callee, t);
                    return self.call_value(span, t, &arg_nodes);
                }
            }
        }
        let t = self.expr(callee, None);
        self.call_value(span, t, &arg_nodes)
    }

    /// Calls a value of function type.
    fn call_value(&mut self, span: Span, callee_ty: Ty, args: &[NodeId]) -> Ty {
        match self.kind(callee_ty) {
            TyKind::Fn(params, ret) => {
                let n = params.len();
                self.check_args(span, args, &params, n, "this function");
                ret
            }
            TyKind::Infer(_) => {
                let params: Vec<Ty> = args.iter().map(|_| self.fresh(span)).collect();
                let ret = self.fresh(span);
                let fn_ty = self.prog.types.func(params.clone(), ret);
                let _ = self.infer.unify(&mut self.prog.types, callee_ty, fn_ty);
                self.check_args(span, args, &params, params.len(), "this function");
                ret
            }
            TyKind::Error => {
                for &a in args {
                    self.expr(a, None);
                }
                self.error_ty()
            }
            _ => {
                let shown = self.show(callee_ty);
                self.err(span, "E0225", format!("expected a function, found `{shown}`"), "not callable");
                for &a in args {
                    self.expr(a, None);
                }
                self.error_ty()
            }
        }
    }

    /// Whether the `self` parameter of `fid` was declared `mut self`.
    fn self_is_mut(&self, fid: FnId) -> bool {
        let f = &self.prog.fns[fid.index()];
        let ast = &self.prog.modules[f.module.index()].ast;
        let NodeKind::FnDecl { params, .. } = *ast.kind(f.node) else { return false };
        matches!(ast.list(params).first().map(|&p| *ast.kind(p)), Some(NodeKind::Param { mutable: true, .. }))
    }

    pub fn method_call(
        &mut self,
        id: NodeId,
        receiver: NodeId,
        name: Symbol,
        generics: NodeList,
        args: NodeList,
    ) -> Ty {
        let ast = self.ast.clone();
        let span = ast.span(id);
        let arg_nodes = ast.list(args).to_vec();
        let recv_ty = self.expr(receiver, None);
        let name_str = self.name(name).to_string();

        // literal receivers default early: `1.max(2)`
        let recv_ty = match self.kind(recv_ty) {
            TyKind::Infer(v) => match self.infer.var_kind(v) {
                VarKind::Int => {
                    let t = self.prim(Prim::I64);
                    let _ = self.infer.unify(&mut self.prog.types, recv_ty, t);
                    t
                }
                VarKind::Float => {
                    let t = self.prim(Prim::F64);
                    let _ = self.infer.unify(&mut self.prog.types, recv_ty, t);
                    t
                }
                VarKind::General => {
                    self.err(
                        span,
                        "E0212",
                        "type annotations needed: the receiver's type is not known here",
                        "cannot call a method on an unknown type",
                    );
                    for &a in &arg_nodes {
                        self.expr(a, None);
                    }
                    return self.error_ty();
                }
            },
            TyKind::Error => {
                for &a in &arg_nodes {
                    self.expr(a, None);
                }
                return self.error_ty();
            }
            _ => recv_ty,
        };

        let explicit: Vec<Ty> = ast.list(generics).to_vec().iter().map(|&g| self.resolve_type(g)).collect();
        let generics_env = self.generics.clone();
        let lookup = lookup_method(self.prog, &mut self.infer, &generics_env, recv_ty, name);
        match lookup {
            MethodLookup::Inherent { impl_id, method } => {
                let impl_args = instantiate_inherent(self.prog, &mut self.infer, &generics_env, impl_id, recv_ty);
                let (all, params, ret) = self.instantiate_fn(method, impl_args, &explicit, span);
                self.finish_user_method(id, receiver, method, Callee::Fn(method, all), &params, ret, &arg_nodes, span);
                ret
            }
            MethodLookup::Trait { trait_id, method } => {
                let targs = instantiate_trait(self.prog, &mut self.infer, &generics_env, trait_id, recv_ty);
                let mut prefix = vec![recv_ty];
                prefix.extend(targs);
                let (all, params, ret) = self.instantiate_fn(method, prefix, &explicit, span);
                self.finish_user_method(
                    id,
                    receiver,
                    method,
                    Callee::Trait(method, all),
                    &params,
                    ret,
                    &arg_nodes,
                    span,
                );
                ret
            }
            MethodLookup::Ambiguous(_) => {
                self.err(
                    span,
                    "E0213",
                    format!("multiple traits provide a method named `{name_str}` for this type"),
                    "ambiguous method call",
                );
                for &a in &arg_nodes {
                    self.expr(a, None);
                }
                self.error_ty()
            }
            MethodLookup::NotFound => {
                let resolved = self.shallow(recv_ty);
                if let Some(sig) = builtin_method(self.prog, resolved, &name_str) {
                    if sig.mutating {
                        self.check_mutable_place(receiver);
                    }
                    for (req, ty) in &sig.requires {
                        self.requires.push((*req, *ty, span, "this method"));
                    }
                    self.set_callee(id, Callee::Builtin(sig.builtin, vec![resolved]));
                    let n = sig.params.len();
                    self.check_args(span, &arg_nodes, &sig.params, n, &format!("method `{name_str}`"));
                    return sig.ret;
                }
                let shown = self.show(recv_ty);
                self.err(
                    span,
                    "E0214",
                    format!("no method named `{name_str}` found for type `{shown}`"),
                    "method not found",
                );
                for &a in &arg_nodes {
                    self.expr(a, None);
                }
                self.error_ty()
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_user_method(
        &mut self,
        id: NodeId,
        receiver: NodeId,
        method: FnId,
        callee: Callee,
        params: &[Ty],
        ret: Ty,
        args: &[NodeId],
        span: Span,
    ) {
        let _ = ret;
        let (def, name_span) = (self.prog.fns[method.index()].def, self.ast.name_span(id));
        self.use_def(name_span, def);
        self.set_callee(id, callee);
        if !self.prog.fns[method.index()].has_self {
            let n = self.name(self.prog.fns[method.index()].name).to_string();
            self.err(
                span,
                "E0214",
                format!("`{n}` is an associated function, not a method; call it as `Type::{n}(..)`"),
                "no `self` parameter",
            );
        } else {
            let recv_ty = self.ty_of(receiver);
            let rspan = self.span(receiver);
            self.unify_at(rspan, params[0], recv_ty);
            if self.self_is_mut(method) {
                self.check_mutable_place(receiver);
            }
        }
        let rest = if self.prog.fns[method.index()].has_self { &params[1..] } else { params };
        let n = rest.len();
        let name = self.name(self.prog.fns[method.index()].name).to_string();
        self.check_args(span, args, rest, n, &format!("method `{name}`"));
    }

    pub fn struct_lit(&mut self, id: NodeId, path: NodeId, fields: NodeList, expected: Option<Ty>) -> Ty {
        let ast = self.ast.clone();
        let span = ast.span(id);
        let Some((adt, targs)) = self.resolve_struct_path(path, expected) else {
            for &f in ast.list(fields) {
                if let NodeKind::FieldInit { value, .. } = *ast.kind(f) {
                    self.expr(value, None);
                }
            }
            return self.error_ty();
        };
        let ty = self.prog.types.adt(adt, targs.clone());
        self.set_ty(path, ty);
        let adt_name = self.name(self.prog.adts[adt.index()].name).to_string();
        let mut seen: HashSet<Symbol> = HashSet::new();
        for &f in ast.list(fields) {
            let NodeKind::FieldInit { name, value } = *ast.kind(f) else { continue };
            let fspan = ast.span(f);
            let found = self.prog.adts[adt.index()].fields().iter().position(|fd| fd.name == name);
            match found {
                Some(i) => {
                    if !seen.insert(name) {
                        let n = self.name(name).to_string();
                        self.err(fspan, "E0228", format!("field `{n}` is specified more than once"), "duplicate field");
                    }
                    let def = self.prog.adts[adt.index()].fields()[i].def;
                    self.use_def(fspan, def);
                    let fty = self.prog.adts[adt.index()].fields()[i].ty;
                    let fty = self.prog.types.subst(fty, &targs);
                    self.expr_has_type(value, fty);
                    self.info().field_indices.insert(f, i as u32);
                }
                None => {
                    let n = self.name(name).to_string();
                    self.err(fspan, "E0227", format!("struct `{adt_name}` has no field named `{n}`"), "unknown field");
                    self.expr(value, None);
                }
            }
            self.set_ty(f, self.unit());
        }
        let missing: Vec<String> = self.prog.adts[adt.index()]
            .fields()
            .iter()
            .filter(|fd| !seen.contains(&fd.name))
            .map(|fd| self.interner.resolve(fd.name).to_string())
            .collect();
        if !missing.is_empty() {
            self.err(
                span,
                "E0226",
                format!(
                    "missing field(s) {} in initializer of `{adt_name}`",
                    missing.iter().map(|m| format!("`{m}`")).collect::<Vec<_>>().join(", ")
                ),
                "missing fields",
            );
        }
        ty
    }

    pub fn field(&mut self, id: NodeId, base: NodeId, name: Symbol) -> Ty {
        let span = self.span(id);
        let bt = self.expr(base, None);
        let n = self.name(name).to_string();
        match self.kind(bt) {
            TyKind::Adt(adt, args) if !self.prog.adts[adt.index()].is_enum() => {
                match self.prog.adts[adt.index()].fields().iter().position(|f| f.name == name) {
                    Some(i) => {
                        let def = self.prog.adts[adt.index()].fields()[i].def;
                        let name_span = self.ast.name_span(id);
                        self.use_def(name_span, def);
                        self.info().field_indices.insert(id, i as u32);
                        let fty = self.prog.adts[adt.index()].fields()[i].ty;
                        self.prog.types.subst(fty, &args)
                    }
                    None => {
                        let shown = self.show(bt);
                        self.err(span, "E0217", format!("no field `{n}` on type `{shown}`"), "unknown field");
                        self.error_ty()
                    }
                }
            }
            TyKind::Error => self.error_ty(),
            _ => {
                let shown = self.show(bt);
                self.err(span, "E0217", format!("no field `{n}` on type `{shown}`"), "this type has no fields");
                self.error_ty()
            }
        }
    }

    pub fn index(&mut self, id: NodeId, base: NodeId, index: NodeId) -> Ty {
        let span = self.span(id);
        let bt = self.expr(base, None);
        match self.kind(bt) {
            TyKind::Array(e) => {
                let it = self.expr(index, None);
                let ok = match self.kind(it) {
                    TyKind::Prim(p) => p.is_int(),
                    TyKind::Infer(v) => self.infer.var_kind(v) == VarKind::Int,
                    TyKind::Error => true,
                    _ => false,
                };
                if !ok {
                    let shown = self.show(it);
                    let s = self.span(index);
                    self.err(s, "E0211", format!("array index must be an integer, found `{shown}`"), "not an integer");
                }
                e
            }
            TyKind::Map(k, v) => {
                self.expr_has_type(index, k);
                self.requires.push((crate::sema::builtins::Require::Hashable, k, span, "a map key"));
                v
            }
            TyKind::Error => {
                self.expr(index, None);
                self.error_ty()
            }
            _ => {
                let shown = self.show(bt);
                self.err(
                    span,
                    "E0217",
                    format!("cannot index into a value of type `{shown}`"),
                    "not an array or map (strings: use `.char_at(i)`)",
                );
                self.expr(index, None);
                self.error_ty()
            }
        }
    }
}
