//! Type inference variables and unification.
//!
//! Variables come in three kinds: `General` (any type), `Int` (an integer
//! literal's type: only integer primitives) and `Float`. Unsuffixed literals
//! get `Int`/`Float` variables that default to `i64`/`f64` when nothing else
//! constrains them. Every binding is logged on a trail so trial unifications
//! (impl matching) can be rolled back.

use super::types::{InferVar, Prim, Ty, TyKind, TypeTable};
use crate::diag::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarKind {
    General,
    Int,
    Float,
}

#[derive(Debug)]
struct VarState {
    kind: VarKind,
    binding: Option<Ty>,
    origin: Span,
}

#[derive(Debug)]
enum Trail {
    Bound(InferVar),
    Kind(InferVar, VarKind),
}

#[derive(Debug, Default)]
pub struct InferCtx {
    vars: Vec<VarState>,
    trail: Vec<Trail>,
}

/// A failed unification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mismatch;

impl InferCtx {
    pub fn new() -> InferCtx {
        InferCtx::default()
    }

    pub fn new_var(&mut self, types: &mut TypeTable, kind: VarKind, origin: Span) -> Ty {
        let v = InferVar(self.vars.len() as u32);
        self.vars.push(VarState { kind, binding: None, origin });
        types.infer(v)
    }

    pub fn var_kind(&self, v: InferVar) -> VarKind {
        self.vars[v.0 as usize].kind
    }

    pub fn var_origin(&self, v: InferVar) -> Span {
        self.vars[v.0 as usize].origin
    }

    /// Follows variable bindings at the top of `ty` only.
    pub fn shallow(&self, types: &TypeTable, mut ty: Ty) -> Ty {
        while let TyKind::Infer(v) = types.kind(ty) {
            match self.vars[v.0 as usize].binding {
                Some(next) => ty = next,
                None => break,
            }
        }
        ty
    }

    /// Replaces every bound variable inside `ty` by its binding.
    pub fn resolve(&self, types: &mut TypeTable, ty: Ty) -> Ty {
        let ty = self.shallow(types, ty);
        match types.kind(ty).clone() {
            TyKind::Array(e) => {
                let e = self.resolve(types, e);
                types.array(e)
            }
            TyKind::Set(e) => {
                let e = self.resolve(types, e);
                types.set(e)
            }
            TyKind::Map(k, v) => {
                let k = self.resolve(types, k);
                let v = self.resolve(types, v);
                types.map(k, v)
            }
            TyKind::Fn(ps, r) => {
                let ps = ps.iter().map(|&p| self.resolve(types, p)).collect();
                let r = self.resolve(types, r);
                types.func(ps, r)
            }
            TyKind::Adt(id, args) => {
                let args = args.iter().map(|&p| self.resolve(types, p)).collect();
                types.adt(id, args)
            }
            _ => ty,
        }
    }

    pub fn snapshot(&self) -> usize {
        self.trail.len()
    }

    pub fn rollback(&mut self, snapshot: usize) {
        while self.trail.len() > snapshot {
            match self.trail.pop().unwrap() {
                Trail::Bound(v) => self.vars[v.0 as usize].binding = None,
                Trail::Kind(v, k) => self.vars[v.0 as usize].kind = k,
            }
        }
    }

    fn occurs(&self, types: &TypeTable, v: InferVar, ty: Ty) -> bool {
        let ty = self.shallow(types, ty);
        match types.kind(ty) {
            TyKind::Infer(w) => *w == v,
            TyKind::Array(e) | TyKind::Set(e) => self.occurs(types, v, *e),
            TyKind::Map(k, x) => self.occurs(types, v, *k) || self.occurs(types, v, *x),
            TyKind::Fn(ps, r) => ps.iter().any(|&p| self.occurs(types, v, p)) || self.occurs(types, v, *r),
            TyKind::Adt(_, args) => args.iter().any(|&p| self.occurs(types, v, p)),
            _ => false,
        }
    }

    fn bind(&mut self, v: InferVar, ty: Ty) {
        self.vars[v.0 as usize].binding = Some(ty);
        self.trail.push(Trail::Bound(v));
    }

    fn set_kind(&mut self, v: InferVar, kind: VarKind) {
        let old = self.vars[v.0 as usize].kind;
        if old != kind {
            self.vars[v.0 as usize].kind = kind;
            self.trail.push(Trail::Kind(v, old));
        }
    }

    /// Makes `a` and `b` equal. `Error` and `Never` unify with anything and
    /// bind nothing.
    pub fn unify(&mut self, types: &mut TypeTable, a: Ty, b: Ty) -> Result<(), Mismatch> {
        let a = self.shallow(types, a);
        let b = self.shallow(types, b);
        if a == b {
            return Ok(());
        }
        let (ka, kb) = (types.kind(a).clone(), types.kind(b).clone());
        match (&ka, &kb) {
            (TyKind::Error | TyKind::Never, _) | (_, TyKind::Error | TyKind::Never) => Ok(()),
            (TyKind::Infer(x), TyKind::Infer(y)) => {
                let merged = match (self.var_kind(*x), self.var_kind(*y)) {
                    (VarKind::General, k) | (k, VarKind::General) => k,
                    (k1, k2) if k1 == k2 => k1,
                    _ => return Err(Mismatch),
                };
                // the younger variable points at the older one: a chain of unifications with fresh
                // variables stays one link long instead of growing with every statement
                let (child, root, root_ty) = if x.0 > y.0 { (*x, *y, b) } else { (*y, *x, a) };
                self.set_kind(root, merged);
                self.bind(child, root_ty);
                Ok(())
            }
            (TyKind::Infer(v), _) => self.bind_var(types, *v, b, &kb),
            (_, TyKind::Infer(v)) => self.bind_var(types, *v, a, &ka),
            (TyKind::Array(x), TyKind::Array(y)) | (TyKind::Set(x), TyKind::Set(y)) => self.unify(types, *x, *y),
            (TyKind::Map(k1, v1), TyKind::Map(k2, v2)) => {
                self.unify(types, *k1, *k2)?;
                self.unify(types, *v1, *v2)
            }
            (TyKind::Fn(p1, r1), TyKind::Fn(p2, r2)) => {
                if p1.len() != p2.len() {
                    return Err(Mismatch);
                }
                for (x, y) in p1.iter().zip(p2) {
                    self.unify(types, *x, *y)?;
                }
                self.unify(types, *r1, *r2)
            }
            (TyKind::Adt(i1, a1), TyKind::Adt(i2, a2)) => {
                if i1 != i2 || a1.len() != a2.len() {
                    return Err(Mismatch);
                }
                for (x, y) in a1.iter().zip(a2) {
                    self.unify(types, *x, *y)?;
                }
                Ok(())
            }
            _ => Err(Mismatch),
        }
    }

    fn bind_var(&mut self, types: &TypeTable, v: InferVar, ty: Ty, kind: &TyKind) -> Result<(), Mismatch> {
        match self.var_kind(v) {
            VarKind::Int if !matches!(kind, TyKind::Prim(p) if p.is_int()) => return Err(Mismatch),
            VarKind::Float if !matches!(kind, TyKind::Prim(p) if p.is_float()) => return Err(Mismatch),
            _ => {}
        }
        if self.occurs(types, v, ty) {
            return Err(Mismatch);
        }
        self.bind(v, ty);
        Ok(())
    }

    /// Binds unresolved literal variables to `i64` / `f64`.
    pub fn apply_literal_defaults(&mut self, types: &mut TypeTable) {
        for i in 0..self.vars.len() {
            if self.vars[i].binding.is_some() {
                continue;
            }
            let default = match self.vars[i].kind {
                VarKind::Int => types.prim(Prim::I64),
                VarKind::Float => types.prim(Prim::F64),
                VarKind::General => continue,
            };
            self.vars[i].binding = Some(default);
        }
    }

    /// Origins of general variables that never got bound.
    pub fn unresolved_origins(&self) -> Vec<Span> {
        self.vars.iter().filter(|v| v.binding.is_none()).map(|v| v.origin).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sema::ids::AdtId;

    fn setup() -> (TypeTable, InferCtx) {
        (TypeTable::new(), InferCtx::new())
    }

    fn var(t: &mut TypeTable, i: &mut InferCtx, k: VarKind) -> Ty {
        i.new_var(t, k, Span::DUMMY)
    }

    #[test]
    fn binds_and_resolves() {
        let (mut t, mut i) = setup();
        let v = var(&mut t, &mut i, VarKind::General);
        let i32t = t.prim(Prim::I32);
        let arr = t.array(v);
        i.unify(&mut t, v, i32t).unwrap();
        assert_eq!(i.shallow(&t, v), i32t);
        let resolved = i.resolve(&mut t, arr);
        assert_eq!(resolved, t.array(i32t));
    }

    #[test]
    fn mismatches_are_errors() {
        let (mut t, mut i) = setup();
        let (a, b) = (t.prim(Prim::I32), t.prim(Prim::I64));
        assert_eq!(i.unify(&mut t, a, b), Err(Mismatch));
        let arr = t.array(a);
        assert_eq!(i.unify(&mut t, arr, a), Err(Mismatch));
        let set = t.set(a);
        assert_eq!(i.unify(&mut t, arr, set), Err(Mismatch));
    }

    #[test]
    fn var_kinds_restrict_bindings() {
        let (mut t, mut i) = setup();
        let n = var(&mut t, &mut i, VarKind::Int);
        let f = var(&mut t, &mut i, VarKind::Float);
        let (u8t, f32t, strt) = (t.prim(Prim::U8), t.prim(Prim::F32), t.prim(Prim::Str));
        assert_eq!(i.unify(&mut t, n, f32t), Err(Mismatch));
        assert_eq!(i.unify(&mut t, n, strt), Err(Mismatch));
        assert_eq!(i.unify(&mut t, f, u8t), Err(Mismatch));
        assert!(i.unify(&mut t, n, u8t).is_ok());
        assert!(i.unify(&mut t, f, f32t).is_ok());
    }

    #[test]
    fn var_var_merges_kinds() {
        let (mut t, mut i) = setup();
        let g = var(&mut t, &mut i, VarKind::General);
        let n = var(&mut t, &mut i, VarKind::Int);
        let f = var(&mut t, &mut i, VarKind::Float);
        i.unify(&mut t, g, n).unwrap();
        // g is now an Int variable too: it rejects a float binding
        let f64t = t.prim(Prim::F64);
        assert_eq!(i.unify(&mut t, g, f64t), Err(Mismatch));
        assert_eq!(i.unify(&mut t, n, f), Err(Mismatch));
    }

    #[test]
    fn occurs_check() {
        let (mut t, mut i) = setup();
        let v = var(&mut t, &mut i, VarKind::General);
        let arr = t.array(v);
        assert_eq!(i.unify(&mut t, v, arr), Err(Mismatch));
    }

    #[test]
    fn never_and_error_unify_with_anything_without_binding() {
        let (mut t, mut i) = setup();
        let v = var(&mut t, &mut i, VarKind::General);
        let never = t.never();
        let err = t.error();
        let i32t = t.prim(Prim::I32);
        assert!(i.unify(&mut t, never, i32t).is_ok());
        assert!(i.unify(&mut t, v, never).is_ok());
        assert!(i.unify(&mut t, err, i32t).is_ok());
        assert_eq!(i.shallow(&t, v), v, "Never must not bind a variable");
    }

    #[test]
    fn structural_unification() {
        let (mut t, mut i) = setup();
        let (a, b) = (var(&mut t, &mut i, VarKind::General), var(&mut t, &mut i, VarKind::General));
        let (i8t, strt) = (t.prim(Prim::I8), t.prim(Prim::Str));
        let m1 = t.map(a, strt);
        let m2 = t.map(i8t, b);
        i.unify(&mut t, m1, m2).unwrap();
        assert_eq!(i.shallow(&t, a), i8t);
        assert_eq!(i.shallow(&t, b), strt);

        let f1 = t.func(vec![i8t], strt);
        let f2 = t.func(vec![i8t, i8t], strt);
        assert_eq!(i.unify(&mut t, f1, f2), Err(Mismatch));

        let adt1 = t.adt(AdtId(0), vec![i8t]);
        let adt2 = t.adt(AdtId(1), vec![i8t]);
        assert_eq!(i.unify(&mut t, adt1, adt2), Err(Mismatch));
    }

    #[test]
    fn rollback_undoes_bindings_and_kinds() {
        let (mut t, mut i) = setup();
        let g = var(&mut t, &mut i, VarKind::General);
        let n = var(&mut t, &mut i, VarKind::Int);
        let snap = i.snapshot();
        i.unify(&mut t, g, n).unwrap();
        let u8t = t.prim(Prim::U8);
        i.unify(&mut t, g, u8t).unwrap();
        i.rollback(snap);
        assert_eq!(i.shallow(&t, g), g);
        assert_eq!(i.shallow(&t, n), n);
        assert_eq!(i.var_kind(InferVar(0)), VarKind::General);
        // and the variables are usable again
        let f64t = t.prim(Prim::F64);
        assert!(i.unify(&mut t, g, f64t).is_ok());
    }

    #[test]
    fn literal_defaults() {
        let (mut t, mut i) = setup();
        let n = var(&mut t, &mut i, VarKind::Int);
        let f = var(&mut t, &mut i, VarKind::Float);
        let g = var(&mut t, &mut i, VarKind::General);
        let bound = var(&mut t, &mut i, VarKind::Int);
        let u8t = t.prim(Prim::U8);
        i.unify(&mut t, bound, u8t).unwrap();
        i.apply_literal_defaults(&mut t);
        assert_eq!(i.shallow(&t, n), t.prim(Prim::I64));
        assert_eq!(i.shallow(&t, f), t.prim(Prim::F64));
        assert_eq!(i.shallow(&t, bound), u8t);
        assert_eq!(i.shallow(&t, g), g);
        assert_eq!(i.unresolved_origins().len(), 1);
    }
}
