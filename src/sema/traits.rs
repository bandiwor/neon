//! Static trait solving and method lookup.
//!
//! Neon has no dynamic dispatch: every trait method call is resolved to one
//! impl at compile time. During checking the solver decides whether bounds
//! hold; once code is monomorphized (all types concrete) [`resolve_trait_method`]
//! picks the exact impl method to call.

use super::ids::*;
use super::infer::{InferCtx, VarKind};
use super::program::*;
use super::types::{Prim, Ty, TyKind};
use crate::diag::Span;
use crate::syntax::Symbol;

const MAX_DEPTH: u32 = 8;

/// Where a satisfied bound comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImplSource {
    /// A user `impl`, with its generic arguments.
    Impl { id: ImplId, args: Vec<Ty> },
    /// Built into the language (operators on primitives).
    Builtin,
    /// Assumed from a `T: Trait` bound of the enclosing generic item.
    Bound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Solved {
    Yes(ImplSource),
    No,
    /// Not decidable yet (unresolved inference variables).
    Maybe,
}

/// Fresh general inference variables for each of `n` generic parameters.
pub fn fresh_args(prog: &mut Program, infer: &mut InferCtx, n: usize, origin: Span) -> Vec<Ty> {
    (0..n).map(|_| infer.new_var(&mut prog.types, VarKind::General, origin)).collect()
}

/// Whether primitive `p` implements the language trait `kind`, and which
/// type the trait's single argument must equal (if it has one).
fn builtin_prim_impl(kind: LangTrait, p: Prim) -> bool {
    use LangTrait::*;
    match kind {
        Add => p.is_numeric() || p == Prim::Str,
        Sub | Mul | Div | Rem => p.is_numeric(),
        Neg => p.is_signed() || p.is_float(),
        Not => p == Prim::Bool || p.is_int(),
        BitAnd | BitOr | BitXor => p == Prim::Bool || p.is_int(),
        Shl | Shr => p.is_int(),
        Eq | Display => true,
        Ord => p.is_numeric() || p == Prim::Char || p == Prim::Str,
    }
}

/// Decides `ty: tr` for a type that is not a bare inference variable or
/// generic parameter. `env` supplies the bounds of generic parameters.
pub fn solve_bound(
    prog: &mut Program,
    infer: &mut InferCtx,
    env: &Generics,
    ty: Ty,
    tr: &TraitRef,
    depth: u32,
) -> Solved {
    let ty = infer.shallow(&prog.types, ty);
    match prog.types.kind(ty).clone() {
        TyKind::Error | TyKind::Never => return Solved::Yes(ImplSource::Builtin),
        TyKind::Infer(_) => return Solved::Maybe,
        TyKind::Param(i) => {
            let bounds: Vec<TraitRef> = env.params.get(i as usize).map(|p| p.bounds.clone()).unwrap_or_default();
            for b in bounds {
                if b.trait_id != tr.trait_id || b.args.len() != tr.args.len() {
                    continue;
                }
                let snap = infer.snapshot();
                let ok = b.args.iter().zip(&tr.args).all(|(&x, &y)| infer.unify(&mut prog.types, x, y).is_ok());
                if ok {
                    return Solved::Yes(ImplSource::Bound);
                }
                infer.rollback(snap);
            }
            return Solved::No;
        }
        _ => {}
    }

    // Builtin operator impls for primitives.
    if let (TyKind::Prim(p), Some(kind)) = (prog.types.kind(ty).clone(), prog.lang.kind_of(tr.trait_id)) {
        if builtin_prim_impl(kind, p) {
            // `Add<Rhs>` etc. take the same type on the right (shifts: any integer).
            let snap = infer.snapshot();
            let ok = tr.args.iter().all(|&a| {
                if matches!(kind, LangTrait::Shl | LangTrait::Shr) {
                    let a = infer.shallow(&prog.types, a);
                    match prog.types.kind(a) {
                        TyKind::Prim(q) => q.is_int(),
                        TyKind::Infer(_) => infer.unify(&mut prog.types, a, ty).is_ok(),
                        _ => false,
                    }
                } else {
                    infer.unify(&mut prog.types, a, ty).is_ok()
                }
            });
            if ok {
                return Solved::Yes(ImplSource::Builtin);
            }
            infer.rollback(snap);
        }
    }

    if depth > MAX_DEPTH {
        return Solved::No;
    }
    let candidates: Vec<ImplId> = prog.impl_index.by_trait.get(&tr.trait_id).cloned().unwrap_or_default();
    let mut matching = Vec::new();
    for id in candidates {
        let snap = infer.snapshot();
        let hit = match_impl(prog, infer, env, id, ty, Some(&tr.args), depth).is_some();
        infer.rollback(snap);
        if hit {
            matching.push(id);
        }
    }
    match matching.len() {
        0 => Solved::No,
        1 => {
            let args =
                match_impl(prog, infer, env, matching[0], ty, Some(&tr.args), depth).expect("matched a moment ago");
            Solved::Yes(ImplSource::Impl { id: matching[0], args })
        }
        _ => Solved::Maybe,
    }
}

/// Unifies impl `id` with `self_ty` (and optional trait arguments), checking
/// the impl's own bounds. On success the unifications stay in place and the
/// impl's generic arguments are returned; on failure everything is undone.
pub fn match_impl(
    prog: &mut Program,
    infer: &mut InferCtx,
    env: &Generics,
    id: ImplId,
    self_ty: Ty,
    trait_args: Option<&[Ty]>,
    depth: u32,
) -> Option<Vec<Ty>> {
    let snap = infer.snapshot();
    let (n, impl_self, impl_trait, bounds) = {
        let imp = &prog.impls[id.index()];
        let bounds: Vec<(usize, Vec<TraitRef>)> =
            imp.generics.params.iter().enumerate().map(|(i, p)| (i, p.bounds.clone())).collect();
        (imp.generics.len(), imp.self_ty, imp.trait_ref.clone(), bounds)
    };
    let args = fresh_args(prog, infer, n, Span::DUMMY);
    let fail = |infer: &mut InferCtx| {
        infer.rollback(snap);
        None
    };
    let want_self = prog.types.subst(impl_self, &args);
    if infer.unify(&mut prog.types, want_self, self_ty).is_err() {
        return fail(infer);
    }
    if let (Some(given), Some(tr)) = (trait_args, &impl_trait) {
        if given.len() != tr.args.len() {
            return fail(infer);
        }
        for (&g, &a) in given.iter().zip(&tr.args) {
            let a = prog.types.subst(a, &args);
            if infer.unify(&mut prog.types, g, a).is_err() {
                return fail(infer);
            }
        }
    }
    for (i, bs) in bounds {
        for b in bs {
            let bound =
                TraitRef { trait_id: b.trait_id, args: b.args.iter().map(|&t| prog.types.subst(t, &args)).collect() };
            if solve_bound(prog, infer, env, args[i], &bound, depth + 1) == Solved::No {
                return fail(infer);
            }
        }
    }
    Some(args)
}

/// Result of looking up `recv.name(..)` among user-defined methods.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MethodLookup {
    Inherent {
        impl_id: ImplId,
        method: FnId,
    },
    /// A method of a trait the receiver implements (or is bounded by).
    Trait {
        trait_id: TraitId,
        method: FnId,
    },
    Ambiguous(Vec<TraitId>),
    NotFound,
}

fn method_named(prog: &Program, methods: &[FnId], name: Symbol) -> Option<FnId> {
    methods.iter().copied().find(|&m| prog.fns[m.index()].name == name)
}

/// Finds a user-defined method `name` callable on `recv` (inherent impls
/// first, then traits). Nothing stays bound afterwards.
pub fn lookup_method(prog: &mut Program, infer: &mut InferCtx, env: &Generics, recv: Ty, name: Symbol) -> MethodLookup {
    let recv = infer.shallow(&prog.types, recv);

    if !matches!(prog.types.kind(recv), TyKind::Param(_)) {
        for id in prog.impl_index.inherent.clone() {
            let Some(method) = method_named(prog, &prog.impls[id.index()].methods, name) else { continue };
            let snap = infer.snapshot();
            let hit = match_impl(prog, infer, env, id, recv, None, 0).is_some();
            infer.rollback(snap);
            if hit {
                return MethodLookup::Inherent { impl_id: id, method };
            }
        }
    }

    let mut found: Vec<(TraitId, FnId)> = Vec::new();
    for t in 0..prog.traits.len() {
        let trait_id = TraitId(t as u32);
        let Some(method) = method_named(prog, &prog.traits[t].methods, name) else { continue };
        if !prog.fns[method.index()].has_self {
            continue;
        }
        let n = prog.traits[t].generics.len() - 1;
        let args = fresh_args(prog, infer, n, Span::DUMMY);
        let snap = infer.snapshot();
        let solved = solve_bound(prog, infer, env, recv, &TraitRef { trait_id, args }, 0);
        infer.rollback(snap);
        if solved != Solved::No {
            found.push((trait_id, method));
        }
    }
    match found.len() {
        0 => MethodLookup::NotFound,
        1 => MethodLookup::Trait { trait_id: found[0].0, method: found[0].1 },
        _ => MethodLookup::Ambiguous(found.into_iter().map(|(t, _)| t).collect()),
    }
}

/// Like [`lookup_method`] for associated functions without a receiver
/// (`Type::name(..)`), which may have no `self` parameter.
pub fn lookup_assoc_fn(prog: &mut Program, infer: &mut InferCtx, env: &Generics, ty: Ty, name: Symbol) -> MethodLookup {
    let ty = infer.shallow(&prog.types, ty);
    if !matches!(prog.types.kind(ty), TyKind::Param(_)) {
        for id in prog.impl_index.inherent.clone() {
            let Some(method) = method_named(prog, &prog.impls[id.index()].methods, name) else { continue };
            let snap = infer.snapshot();
            let hit = match_impl(prog, infer, env, id, ty, None, 0).is_some();
            infer.rollback(snap);
            if hit {
                return MethodLookup::Inherent { impl_id: id, method };
            }
        }
    }
    let mut found = Vec::new();
    for t in 0..prog.traits.len() {
        let trait_id = TraitId(t as u32);
        let Some(method) = method_named(prog, &prog.traits[t].methods, name) else { continue };
        let n = prog.traits[t].generics.len() - 1;
        let args = fresh_args(prog, infer, n, Span::DUMMY);
        let snap = infer.snapshot();
        let solved = solve_bound(prog, infer, env, ty, &TraitRef { trait_id, args }, 0);
        infer.rollback(snap);
        if solved != Solved::No {
            found.push((trait_id, method));
        }
    }
    match found.len() {
        0 => MethodLookup::NotFound,
        1 => MethodLookup::Trait { trait_id: found[0].0, method: found[0].1 },
        _ => MethodLookup::Ambiguous(found.into_iter().map(|(t, _)| t).collect()),
    }
}

/// Binds the impl found by [`lookup_method`] to `recv`, returning its generic
/// arguments (inference variables now unified with the receiver).
pub fn instantiate_inherent(
    prog: &mut Program,
    infer: &mut InferCtx,
    env: &Generics,
    impl_id: ImplId,
    recv: Ty,
) -> Vec<Ty> {
    match_impl(prog, infer, env, impl_id, recv, None, 0).expect("impl matched during lookup")
}

/// Solves `recv: Trait<_>` for real, returning the trait arguments.
pub fn instantiate_trait(
    prog: &mut Program,
    infer: &mut InferCtx,
    env: &Generics,
    trait_id: TraitId,
    recv: Ty,
) -> Vec<Ty> {
    let n = prog.traits[trait_id.index()].generics.len() - 1;
    let args = fresh_args(prog, infer, n, Span::DUMMY);
    solve_bound(prog, infer, env, recv, &TraitRef { trait_id, args: args.clone() }, 0);
    args
}

// ---- monomorphization-time resolution ----------------------------------------------

/// The concrete function a trait-method call resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// A user function (impl method or trait default) with concrete generic arguments.
    Fn(FnId, Vec<Ty>),
    /// An operator or formatting of a primitive type, implemented by the compiler.
    BuiltinOp(LangTrait),
}

/// Resolves a call of trait method `method` with fully concrete `args`
/// (`[Self, trait args.., method args..]`) to the impl that provides it.
pub fn resolve_trait_method(prog: &mut Program, method: FnId, args: &[Ty]) -> Option<Resolved> {
    let FnOwner::Trait(trait_id) = prog.fns[method.index()].owner else { return None };
    let name = prog.fns[method.index()].name;
    let trait_n = prog.traits[trait_id.index()].generics.len();
    let mut infer = InferCtx::new();
    let env = Generics::default();
    let tr = TraitRef { trait_id, args: args[1..trait_n].to_vec() };
    match solve_bound(prog, &mut infer, &env, args[0], &tr, 0) {
        Solved::Yes(ImplSource::Builtin) => prog.lang.kind_of(trait_id).map(Resolved::BuiltinOp),
        Solved::Yes(ImplSource::Impl { id, args: impl_args }) => {
            let impl_args: Vec<Ty> = impl_args.iter().map(|&t| infer.resolve(&mut prog.types, t)).collect();
            match method_named(prog, &prog.impls[id.index()].methods, name) {
                Some(m) => {
                    let mut full = impl_args;
                    full.extend_from_slice(&args[trait_n..]);
                    Some(Resolved::Fn(m, full))
                }
                // not overridden: the trait's default body, with Self = args[0]
                None => Some(Resolved::Fn(method, args.to_vec())),
            }
        }
        _ => None,
    }
}
