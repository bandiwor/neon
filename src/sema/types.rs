//! Interned semantic types.

use std::collections::HashMap;

use super::ids::AdtId;
use crate::syntax::Keyword;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ty(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Prim {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    Bool,
    Char,
    Str,
}

impl Prim {
    pub const ALL: [Prim; 13] = [
        Prim::I8,
        Prim::I16,
        Prim::I32,
        Prim::I64,
        Prim::U8,
        Prim::U16,
        Prim::U32,
        Prim::U64,
        Prim::F32,
        Prim::F64,
        Prim::Bool,
        Prim::Char,
        Prim::Str,
    ];

    pub fn from_keyword(k: Keyword) -> Option<Prim> {
        Some(match k {
            Keyword::I8 => Prim::I8,
            Keyword::I16 => Prim::I16,
            Keyword::I32 => Prim::I32,
            Keyword::I64 => Prim::I64,
            Keyword::U8 => Prim::U8,
            Keyword::U16 => Prim::U16,
            Keyword::U32 => Prim::U32,
            Keyword::U64 => Prim::U64,
            Keyword::F32 => Prim::F32,
            Keyword::F64 => Prim::F64,
            Keyword::Bool => Prim::Bool,
            Keyword::Char => Prim::Char,
            Keyword::String => Prim::Str,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Prim::I8 => "i8",
            Prim::I16 => "i16",
            Prim::I32 => "i32",
            Prim::I64 => "i64",
            Prim::U8 => "u8",
            Prim::U16 => "u16",
            Prim::U32 => "u32",
            Prim::U64 => "u64",
            Prim::F32 => "f32",
            Prim::F64 => "f64",
            Prim::Bool => "bool",
            Prim::Char => "char",
            Prim::Str => "string",
        }
    }

    pub fn is_int(self) -> bool {
        matches!(self, Prim::I8 | Prim::I16 | Prim::I32 | Prim::I64 | Prim::U8 | Prim::U16 | Prim::U32 | Prim::U64)
    }

    pub fn is_signed(self) -> bool {
        matches!(self, Prim::I8 | Prim::I16 | Prim::I32 | Prim::I64)
    }

    pub fn is_float(self) -> bool {
        matches!(self, Prim::F32 | Prim::F64)
    }

    pub fn is_numeric(self) -> bool {
        self.is_int() || self.is_float()
    }

    /// Width in bits of an integer or float type.
    pub fn bits(self) -> u32 {
        match self {
            Prim::I8 | Prim::U8 => 8,
            Prim::I16 | Prim::U16 => 16,
            Prim::I32 | Prim::U32 | Prim::F32 | Prim::Char => 32,
            Prim::I64 | Prim::U64 | Prim::F64 => 64,
            Prim::Bool => 1,
            Prim::Str => 64,
        }
    }

    /// Largest value of an integer type, as a u64 magnitude.
    pub fn int_max(self) -> u64 {
        debug_assert!(self.is_int());
        match (self.is_signed(), self.bits()) {
            (true, b) => (1u64 << (b - 1)) - 1,
            (false, 64) => u64::MAX,
            (false, b) => (1u64 << b) - 1,
        }
    }

    /// Magnitude of the smallest value of an integer type (0 for unsigned).
    pub fn int_min_magnitude(self) -> u64 {
        debug_assert!(self.is_int());
        if self.is_signed() {
            1u64 << (self.bits() - 1)
        } else {
            0
        }
    }
}

/// An inference variable (see `infer`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InferVar(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TyKind {
    Prim(Prim),
    Unit,
    /// The type of `return`, `break`, `panic(..)`: coerces to anything.
    Never,
    /// Poison after a reported error; unifies with everything.
    Error,
    Array(Ty),
    Set(Ty),
    Map(Ty, Ty),
    Fn(Vec<Ty>, Ty),
    Adt(AdtId, Vec<Ty>),
    /// Generic parameter number `n` of the enclosing item.
    Param(u32),
    Infer(InferVar),
}

#[derive(Debug)]
pub struct TypeTable {
    kinds: Vec<TyKind>,
    map: HashMap<TyKind, Ty>,
    prims: [Ty; 13],
    unit: Ty,
    never: Ty,
    error: Ty,
}

impl Default for TypeTable {
    fn default() -> Self {
        TypeTable::new()
    }
}

impl TypeTable {
    pub fn new() -> TypeTable {
        let mut t = TypeTable {
            kinds: Vec::new(),
            map: HashMap::new(),
            prims: [Ty(0); 13],
            unit: Ty(0),
            never: Ty(0),
            error: Ty(0),
        };
        for (i, p) in Prim::ALL.iter().enumerate() {
            t.prims[i] = t.intern(TyKind::Prim(*p));
        }
        t.unit = t.intern(TyKind::Unit);
        t.never = t.intern(TyKind::Never);
        t.error = t.intern(TyKind::Error);
        t
    }

    pub fn intern(&mut self, kind: TyKind) -> Ty {
        if let Some(&ty) = self.map.get(&kind) {
            return ty;
        }
        let ty = Ty(self.kinds.len() as u32);
        self.kinds.push(kind.clone());
        self.map.insert(kind, ty);
        ty
    }

    pub fn kind(&self, ty: Ty) -> &TyKind {
        &self.kinds[ty.0 as usize]
    }

    pub fn prim(&self, p: Prim) -> Ty {
        self.prims[Prim::ALL.iter().position(|&q| q == p).unwrap()]
    }

    pub fn unit(&self) -> Ty {
        self.unit
    }

    pub fn never(&self) -> Ty {
        self.never
    }

    pub fn error(&self) -> Ty {
        self.error
    }

    pub fn bool(&self) -> Ty {
        self.prim(Prim::Bool)
    }

    pub fn array(&mut self, elem: Ty) -> Ty {
        self.intern(TyKind::Array(elem))
    }

    pub fn set(&mut self, elem: Ty) -> Ty {
        self.intern(TyKind::Set(elem))
    }

    pub fn map(&mut self, key: Ty, value: Ty) -> Ty {
        self.intern(TyKind::Map(key, value))
    }

    pub fn func(&mut self, params: Vec<Ty>, ret: Ty) -> Ty {
        self.intern(TyKind::Fn(params, ret))
    }

    pub fn adt(&mut self, id: AdtId, args: Vec<Ty>) -> Ty {
        self.intern(TyKind::Adt(id, args))
    }

    pub fn param(&mut self, index: u32) -> Ty {
        self.intern(TyKind::Param(index))
    }

    pub fn infer(&mut self, v: InferVar) -> Ty {
        self.intern(TyKind::Infer(v))
    }

    pub fn len(&self) -> usize {
        self.kinds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    pub fn as_prim(&self, ty: Ty) -> Option<Prim> {
        match self.kind(ty) {
            TyKind::Prim(p) => Some(*p),
            _ => None,
        }
    }

    /// Whether values of this type live on the garbage-collected heap.
    pub fn is_heap(&self, ty: Ty) -> bool {
        matches!(
            self.kind(ty),
            TyKind::Prim(Prim::Str)
                | TyKind::Array(_)
                | TyKind::Set(_)
                | TyKind::Map(..)
                | TyKind::Fn(..)
                | TyKind::Adt(..)
        )
    }

    /// Replaces `Param(i)` by `args[i]` throughout `ty`.
    pub fn subst(&mut self, ty: Ty, args: &[Ty]) -> Ty {
        if args.is_empty() {
            return ty;
        }
        match self.kind(ty).clone() {
            TyKind::Param(i) => args.get(i as usize).copied().unwrap_or(ty),
            TyKind::Array(e) => {
                let e = self.subst(e, args);
                self.array(e)
            }
            TyKind::Set(e) => {
                let e = self.subst(e, args);
                self.set(e)
            }
            TyKind::Map(k, v) => {
                let k = self.subst(k, args);
                let v = self.subst(v, args);
                self.map(k, v)
            }
            TyKind::Fn(ps, r) => {
                let ps = ps.iter().map(|&p| self.subst(p, args)).collect();
                let r = self.subst(r, args);
                self.func(ps, r)
            }
            TyKind::Adt(id, a) => {
                let a = a.iter().map(|&p| self.subst(p, args)).collect();
                self.adt(id, a)
            }
            _ => ty,
        }
    }

    /// Whether the type mentions a generic parameter.
    pub fn has_param(&self, ty: Ty) -> bool {
        match self.kind(ty) {
            TyKind::Param(_) => true,
            TyKind::Array(e) | TyKind::Set(e) => self.has_param(*e),
            TyKind::Map(k, v) => self.has_param(*k) || self.has_param(*v),
            TyKind::Fn(ps, r) => ps.iter().any(|&p| self.has_param(p)) || self.has_param(*r),
            TyKind::Adt(_, a) => a.iter().any(|&p| self.has_param(p)),
            _ => false,
        }
    }

    /// Whether the type mentions an unresolved inference variable.
    pub fn has_infer(&self, ty: Ty) -> bool {
        match self.kind(ty) {
            TyKind::Infer(_) => true,
            TyKind::Array(e) | TyKind::Set(e) => self.has_infer(*e),
            TyKind::Map(k, v) => self.has_infer(*k) || self.has_infer(*v),
            TyKind::Fn(ps, r) => ps.iter().any(|&p| self.has_infer(p)) || self.has_infer(*r),
            TyKind::Adt(_, a) => a.iter().any(|&p| self.has_infer(p)),
            _ => false,
        }
    }

    /// Renders a type; `adt_name` and `param_name` supply user-facing names.
    pub fn display(
        &self,
        ty: Ty,
        adt_name: &dyn Fn(AdtId) -> String,
        param_name: &dyn Fn(u32) -> String,
        infer_name: &dyn Fn(InferVar) -> String,
    ) -> String {
        let go = |t: Ty| self.display(t, adt_name, param_name, infer_name);
        match self.kind(ty) {
            TyKind::Prim(p) => p.name().to_string(),
            TyKind::Unit => "()".to_string(),
            TyKind::Never => "!".to_string(),
            TyKind::Error => "{error}".to_string(),
            TyKind::Array(e) => format!("[{}]", go(*e)),
            TyKind::Set(e) => format!("set<{}>", go(*e)),
            TyKind::Map(k, v) => format!("{{{}: {}}}", go(*k), go(*v)),
            TyKind::Fn(ps, r) => {
                let ps: Vec<_> = ps.iter().map(|&p| go(p)).collect();
                if *self.kind(*r) == TyKind::Unit {
                    format!("fn({})", ps.join(", "))
                } else {
                    format!("fn({}) -> {}", ps.join(", "), go(*r))
                }
            }
            TyKind::Adt(id, args) => {
                if args.is_empty() {
                    adt_name(*id)
                } else {
                    let a: Vec<_> = args.iter().map(|&p| go(p)).collect();
                    format!("{}<{}>", adt_name(*id), a.join(", "))
                }
            }
            TyKind::Param(i) => param_name(*i),
            TyKind::Infer(v) => infer_name(*v),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(t: &TypeTable, ty: Ty) -> String {
        t.display(ty, &|a| format!("Adt{}", a.0), &|i| format!("T{i}"), &|v| format!("?{}", v.0))
    }

    #[test]
    fn interning_is_hash_consing() {
        let mut t = TypeTable::new();
        let i32t = t.prim(Prim::I32);
        let a = t.array(i32t);
        let b = t.array(i32t);
        assert_eq!(a, b);
        assert_ne!(a, t.set(i32t));
        assert_eq!(t.unit(), t.intern(TyKind::Unit));
        assert_eq!(t.bool(), t.prim(Prim::Bool));
    }

    #[test]
    fn display_forms() {
        let mut t = TypeTable::new();
        let i = t.prim(Prim::I32);
        let s = t.prim(Prim::Str);
        let arr = t.array(i);
        let map = t.map(s, arr);
        let f = t.func(vec![i, i], i);
        let g = t.func(vec![], t.unit());
        let p = t.param(0);
        let adt = t.adt(AdtId(3), vec![p, i]);
        assert_eq!(show(&t, arr), "[i32]");
        assert_eq!(show(&t, map), "{string: [i32]}");
        assert_eq!(show(&t, f), "fn(i32, i32) -> i32");
        assert_eq!(show(&t, g), "fn()");
        assert_eq!(show(&t, adt), "Adt3<T0, i32>");
        assert_eq!(show(&t, t.never()), "!");
    }

    #[test]
    fn substitution() {
        let mut t = TypeTable::new();
        let p0 = t.param(0);
        let p1 = t.param(1);
        let i = t.prim(Prim::I64);
        let b = t.bool();
        let arr = t.array_of(p1);
        let ty = t.map(p0, arr);
        assert!(t.has_param(ty));
        let out = t.subst(ty, &[i, b]);
        assert_eq!(show(&t, out), "{i64: [bool]}");
        assert!(!t.has_param(out));
        assert_eq!(t.subst(ty, &[]), ty);
    }

    impl TypeTable {
        fn array_of(&mut self, e: Ty) -> Ty {
            self.array(e)
        }
    }

    #[test]
    fn prim_properties() {
        assert!(Prim::I8.is_signed() && !Prim::U8.is_signed());
        assert_eq!(Prim::I8.int_max(), 127);
        assert_eq!(Prim::I8.int_min_magnitude(), 128);
        assert_eq!(Prim::U8.int_max(), 255);
        assert_eq!(Prim::U64.int_max(), u64::MAX);
        assert_eq!(Prim::I64.int_max(), i64::MAX as u64);
        assert_eq!(Prim::I64.int_min_magnitude(), 1 << 63);
        assert_eq!(Prim::U32.int_max(), u32::MAX as u64);
        assert!(Prim::F32.is_float() && Prim::F32.is_numeric() && !Prim::Bool.is_numeric());
        assert_eq!(Prim::from_keyword(Keyword::String), Some(Prim::Str));
        assert_eq!(Prim::from_keyword(Keyword::Let), None);
    }

    #[test]
    fn heap_classification() {
        let mut t = TypeTable::new();
        let i = t.prim(Prim::I32);
        let s = t.prim(Prim::Str);
        assert!(!t.is_heap(i) && t.is_heap(s) && !t.is_heap(t.unit()));
        let a = t.array(i);
        assert!(t.is_heap(a));
        let f = t.func(vec![], i);
        assert!(t.is_heap(f));
    }
}
