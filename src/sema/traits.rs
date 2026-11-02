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

/// How deep the solver follows the bounds of impls into one another (`[[[T]]]: Show` asks `[[T]]: Show`, ...).
const MAX_DEPTH: u32 = 64;

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
        Iterator => false,
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
    // what is fully known stays known: ask once
    let ground = prog.types.is_ground(ty) && tr.args.iter().all(|&a| prog.types.is_ground(a));
    let key = ground.then(|| (tr.trait_id, ty, tr.args.clone()));
    if let Some(k) = &key {
        if let Some(hit) = prog.solve_cache.get(k) {
            return hit.clone();
        }
    }
    let candidates = prog.impl_index.trait_candidates(tr.trait_id, Head::of(&prog.types, ty));
    let answer = if let [only] = candidates[..] {
        // the usual case: one impl could apply. Matching it once is enough; matching it a second time "for
        // real" would double the work at every level of a nested type (`[[[T]]]` costs 2^depth)
        match match_impl(prog, infer, env, only, ty, Some(&tr.args), depth) {
            Some(args) => Solved::Yes(ImplSource::Impl { id: only, args }),
            None => Solved::No,
        }
    } else {
        let mut matching = Vec::new();
        for id in candidates {
            let snap = infer.snapshot();
            let hit = match_impl(prog, infer, env, id, ty, Some(&tr.args), depth).is_some();
            infer.rollback(snap);
            if hit {
                matching.push(id);
            }
        }
        match most_specific(prog, infer, &matching, true) {
            Some(id) => {
                let args = match_impl(prog, infer, env, id, ty, Some(&tr.args), depth).expect("matched a moment ago");
                Solved::Yes(ImplSource::Impl { id, args })
            }
            None if matching.is_empty() => Solved::No,
            None => Solved::Maybe,
        }
    };
    if let Some(k) = key {
        // the arguments of the impl are variables of this inference context: only what they are bound
        // to means anything to the next asker
        let storable = match &answer {
            Solved::Yes(ImplSource::Impl { id, args }) => {
                let args: Vec<Ty> = args.iter().map(|&t| infer.resolve(&mut prog.types, t)).collect();
                args.iter().all(|&t| prog.types.is_ground(t)).then_some(Solved::Yes(ImplSource::Impl { id: *id, args }))
            }
            Solved::Maybe => None,
            other => Some(other.clone()),
        };
        if let Some(stored) = storable {
            prog.solve_cache.insert(k, stored);
        }
    }
    answer
}

/// Whether impl `a` applies to every type that impl `b` applies to ... the other way round: `a` is at
/// least as specific as `b` when `b`'s header can be made equal to `a`'s own (so `impl Tr for [i64]` is at
/// least as specific as `impl<T> Tr for [T]`, and not the other way). `a`'s parameters are rigid, `b`'s
/// are free.
fn at_least_as_specific(prog: &mut Program, infer: &mut InferCtx, a: ImplId, b: ImplId, with_trait_args: bool) -> bool {
    let snap = infer.snapshot();
    let (b_n, b_self, b_trait) = {
        let imp = &prog.impls[b.index()];
        (imp.generics.len(), imp.self_ty, imp.trait_ref.clone())
    };
    let (a_self, a_trait) = {
        let imp = &prog.impls[a.index()];
        (imp.self_ty, imp.trait_ref.clone())
    };
    let fresh = fresh_args(prog, infer, b_n, Span::DUMMY);
    let want = prog.types.subst(b_self, &fresh);
    let mut ok = infer.unify(&mut prog.types, want, a_self).is_ok();
    if ok && with_trait_args {
        if let (Some(ta), Some(tb)) = (&a_trait, &b_trait) {
            ok = ta.args.len() == tb.args.len()
                && ta.args.iter().zip(&tb.args).all(|(&x, &y)| {
                    let y = prog.types.subst(y, &fresh);
                    infer.unify(&mut prog.types, y, x).is_ok()
                });
        }
    }
    infer.rollback(snap);
    ok
}

/// How two impls relate when both could apply to the same type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlap {
    /// No type is covered by both.
    Disjoint,
    /// One covers a strict subset of the other: the narrower one wins (specialization).
    Nested,
    /// They cover exactly the same types.
    Identical,
    /// Some types are covered by both and neither impl is the narrower: which one applies is unclear.
    Ambiguous,
}

/// Do impls `a` and `b` (of the same trait, or both inherent) apply to a common type? The bounds of the
/// impls are not looked at (as in Rust): an impl for `T: Show` and one for `T: Debug` are the same shape.
pub fn impls_overlap(prog: &mut Program, a: ImplId, b: ImplId) -> Overlap {
    let mut infer = InferCtx::new();
    let (a_n, a_self, a_trait) = {
        let i = &prog.impls[a.index()];
        (i.generics.len(), i.self_ty, i.trait_ref.clone())
    };
    let (b_n, b_self, b_trait) = {
        let i = &prog.impls[b.index()];
        (i.generics.len(), i.self_ty, i.trait_ref.clone())
    };
    let fa = fresh_args(prog, &mut infer, a_n, Span::DUMMY);
    let fb = fresh_args(prog, &mut infer, b_n, Span::DUMMY);
    let (sa, sb) = (prog.types.subst(a_self, &fa), prog.types.subst(b_self, &fb));
    let mut common = infer.unify(&mut prog.types, sa, sb).is_ok();
    if let (true, Some(ta), Some(tb)) = (common, &a_trait, &b_trait) {
        common = ta.args.len() == tb.args.len()
            && ta.args.iter().zip(&tb.args).all(|(&x, &y)| {
                let (x, y) = (prog.types.subst(x, &fa), prog.types.subst(y, &fb));
                infer.unify(&mut prog.types, x, y).is_ok()
            });
    }
    if !common {
        return Overlap::Disjoint;
    }
    let mut infer = InferCtx::new();
    let a_in_b = at_least_as_specific(prog, &mut infer, a, b, true);
    let b_in_a = at_least_as_specific(prog, &mut infer, b, a, true);
    match (a_in_b, b_in_a) {
        (true, true) => Overlap::Identical,
        (true, false) | (false, true) => Overlap::Nested,
        (false, false) => Overlap::Ambiguous,
    }
}

/// Of several impls that all apply, the one that is strictly more specific than every other. `None` if
/// there is no such impl (several unrelated ones, or none at all).
fn most_specific(prog: &mut Program, infer: &mut InferCtx, ids: &[ImplId], with_trait_args: bool) -> Option<ImplId> {
    match ids {
        [] => None,
        [one] => Some(*one),
        _ => ids.iter().copied().find(|&a| {
            ids.iter().all(|&b| {
                a == b
                    || (at_least_as_specific(prog, infer, a, b, with_trait_args)
                        && !at_least_as_specific(prog, infer, b, a, with_trait_args))
            })
        }),
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
        if let Some(found) = inherent_method(prog, infer, env, recv, name) {
            return found;
        }
    }

    trait_method(prog, infer, env, recv, name, true)
}

/// The inherent impl that provides `name` for `recv`: of those that apply, the most specific one (so the
/// order of the impls in the source does not matter).
fn inherent_method(
    prog: &mut Program,
    infer: &mut InferCtx,
    env: &Generics,
    recv: Ty,
    name: Symbol,
) -> Option<MethodLookup> {
    let candidates = prog.impl_index.inherent_candidates(name, Head::of(&prog.types, recv)).to_vec();
    let mut hits = Vec::new();
    for id in candidates {
        let snap = infer.snapshot();
        let hit = match_impl(prog, infer, env, id, recv, None, 0).is_some();
        infer.rollback(snap);
        if hit {
            hits.push(id);
        }
    }
    // with several that apply and none the most specific, the first one defined is taken
    let id = most_specific(prog, infer, &hits, false).or_else(|| hits.first().copied())?;
    let method = method_named(prog, &prog.impls[id.index()].methods, name)?;
    Some(MethodLookup::Inherent { impl_id: id, method })
}

/// The trait method `name` for `ty`: a unique trait that has it and that `ty` implements.
fn trait_method(
    prog: &mut Program,
    infer: &mut InferCtx,
    env: &Generics,
    ty: Ty,
    name: Symbol,
    need_self: bool,
) -> MethodLookup {
    let mut found: Vec<(TraitId, FnId)> = Vec::new();
    let traits = prog.impl_index.traits_with_method(name).to_vec();
    for trait_id in traits {
        let t = trait_id.index();
        let Some(method) = method_named(prog, &prog.traits[t].methods, name) else { continue };
        if need_self && !prog.fns[method.index()].has_self {
            continue;
        }
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

/// Like [`lookup_method`] for associated functions without a receiver
/// (`Type::name(..)`), which may have no `self` parameter.
pub fn lookup_assoc_fn(prog: &mut Program, infer: &mut InferCtx, env: &Generics, ty: Ty, name: Symbol) -> MethodLookup {
    let ty = infer.shallow(&prog.types, ty);
    if !matches!(prog.types.kind(ty), TyKind::Param(_)) {
        if let Some(found) = inherent_method(prog, infer, env, ty, name) {
            return found;
        }
    }
    trait_method(prog, infer, env, ty, name, false)
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
    let key = (method, args.to_vec());
    if let Some(hit) = prog.resolve_cache.get(&key) {
        return hit.clone();
    }
    let answer = resolve_uncached(prog, method, args);
    prog.resolve_cache.insert(key, answer.clone());
    answer
}

fn resolve_uncached(prog: &mut Program, method: FnId, args: &[Ty]) -> Option<Resolved> {
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
