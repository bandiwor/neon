//! Typing of builtin functions, methods and constants.

use super::infer::InferCtx;
use super::program::{ConstVal, LangAdts, Program};
use super::types::{Prim, Ty, TyKind, TypeTable};
use crate::builtin::Builtin;

/// A deferred requirement on a type, verified once inference has finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Require {
    /// Usable as a map key / set element: integer, `bool`, `char` or `string`.
    Hashable,
    /// Comparable with `==` by the VM: any primitive.
    Primitive,
    /// Orderable by the VM: numbers, `char`, `string`.
    Orderable,
    /// Exactly `string`.
    Str,
}

#[derive(Debug, Clone)]
pub struct BuiltinSig {
    pub builtin: Builtin,
    /// Parameters after the receiver (for methods).
    pub params: Vec<Ty>,
    pub ret: Ty,
    /// Mutates the receiver, so a `let` receiver must be declared with `var`.
    pub mutating: bool,
    pub requires: Vec<(Require, Ty)>,
}

fn sig(builtin: Builtin, params: Vec<Ty>, ret: Ty) -> Option<BuiltinSig> {
    Some(BuiltinSig { builtin, params, ret, mutating: false, requires: Vec::new() })
}

fn msig(builtin: Builtin, params: Vec<Ty>, ret: Ty) -> Option<BuiltinSig> {
    Some(BuiltinSig { builtin, params, ret, mutating: true, requires: Vec::new() })
}

impl BuiltinSig {
    fn requiring(mut self, r: Require, t: Ty) -> Option<BuiltinSig> {
        self.requires.push((r, t));
        Some(self)
    }
}

/// `Option<x>` / `Result<ok, err>` from the prelude's enums.
fn option_of(t: &mut TypeTable, la: LangAdts, x: Ty) -> Option<Ty> {
    Some(t.adt(la.option?, vec![x]))
}

fn result_of(t: &mut TypeTable, la: LangAdts, ok: Ty, err: Ty) -> Option<Ty> {
    Some(t.adt(la.result?, vec![ok, err]))
}

/// Signature of the builtin method `name` on a receiver of type `recv`
/// (already resolved at the top level). `None` if there is no such method.
pub fn builtin_method(prog: &mut Program, recv: Ty, name: &str) -> Option<BuiltinSig> {
    let la = prog.lang_adts;
    let t = &mut prog.types;
    let (i64t, boolt, strt, chart, unit) =
        (t.prim(Prim::I64), t.bool(), t.prim(Prim::Str), t.prim(Prim::Char), t.unit());
    match t.kind(recv).clone() {
        TyKind::Prim(Prim::Str) => {
            let chars = t.array(chart);
            let strs = t.array(strt);
            match name {
                "len" => sig(Builtin::StrLen, vec![], i64t),
                "is_empty" => sig(Builtin::StrIsEmpty, vec![], boolt),
                "chars" => sig(Builtin::StrChars, vec![], chars),
                "substring" => sig(Builtin::StrSubstring, vec![i64t, i64t], strt),
                "contains" => sig(Builtin::StrContains, vec![strt], boolt),
                "starts_with" => sig(Builtin::StrStartsWith, vec![strt], boolt),
                "ends_with" => sig(Builtin::StrEndsWith, vec![strt], boolt),
                "to_upper" => sig(Builtin::StrToUpper, vec![], strt),
                "to_lower" => sig(Builtin::StrToLower, vec![], strt),
                "trim" => sig(Builtin::StrTrim, vec![], strt),
                "split" => sig(Builtin::StrSplit, vec![strt], strs),
                "char_at" => sig(Builtin::StrCharAt, vec![i64t], chart),
                "find" => sig(Builtin::StrFind, vec![strt], i64t),
                "parse_i64" => sig(Builtin::StrParseI64, vec![], i64t),
                "parse_f64" => sig(Builtin::StrParseF64, vec![], t.prim(Prim::F64)),
                "to_string" => sig(Builtin::ToString, vec![], strt),
                "try_parse_i64" => sig(Builtin::StrTryParseI64, vec![], result_of(t, la, i64t, strt)?),
                "try_parse_f64" => {
                    let f64t = t.prim(Prim::F64);
                    sig(Builtin::StrTryParseF64, vec![], result_of(t, la, f64t, strt)?)
                }
                "get_char" => sig(Builtin::StrGetChar, vec![i64t], option_of(t, la, chart)?),
                "try_substring" => sig(Builtin::StrTrySubstring, vec![i64t, i64t], result_of(t, la, strt, strt)?),
                "index_of" => sig(Builtin::StrIndexOf, vec![strt], option_of(t, la, i64t)?),
                _ => None,
            }
        }
        TyKind::Array(e) => {
            let arr = recv;
            match name {
                "len" => sig(Builtin::ArrLen, vec![], i64t),
                "is_empty" => sig(Builtin::ArrIsEmpty, vec![], boolt),
                "push" => msig(Builtin::ArrPush, vec![e], unit),
                "pop" => msig(Builtin::ArrPop, vec![], e),
                "get" => sig(Builtin::ArrGet, vec![i64t], option_of(t, la, e)?),
                "try_pop" => msig(Builtin::ArrTryPop, vec![], option_of(t, la, e)?),
                "try_remove" => msig(Builtin::ArrTryRemove, vec![i64t], option_of(t, la, e)?),
                "clear" => msig(Builtin::ArrClear, vec![], unit),
                "contains" => sig(Builtin::ArrContains, vec![e], boolt)?.requiring(Require::Primitive, e),
                "insert" => msig(Builtin::ArrInsert, vec![i64t, e], unit),
                "remove" => msig(Builtin::ArrRemove, vec![i64t], e),
                "reverse" => msig(Builtin::ArrReverse, vec![], unit),
                "clone" => sig(Builtin::ArrClone, vec![], arr),
                "join" => sig(Builtin::ArrJoin, vec![strt], strt)?.requiring(Require::Str, e),
                "sort" => {
                    let mut s = msig(Builtin::ArrSort, vec![], unit)?;
                    s.requires.push((Require::Orderable, e));
                    Some(s)
                }
                _ => None,
            }
        }
        TyKind::Map(k, v) => {
            let keys = t.array(k);
            let values = t.array(v);
            let mut s = match name {
                "len" => sig(Builtin::MapLen, vec![], i64t),
                "is_empty" => sig(Builtin::MapIsEmpty, vec![], boolt),
                "insert" => msig(Builtin::MapInsert, vec![k, v], unit),
                "get_or" => sig(Builtin::MapGetOr, vec![k, v], v),
                "get" => sig(Builtin::MapGet, vec![k], option_of(t, la, v)?),
                "contains_key" => sig(Builtin::MapContainsKey, vec![k], boolt),
                "remove" => msig(Builtin::MapRemove, vec![k], boolt),
                "keys" => sig(Builtin::MapKeys, vec![], keys),
                "values" => sig(Builtin::MapValues, vec![], values),
                "clear" => msig(Builtin::MapClear, vec![], unit),
                _ => None,
            }?;
            s.requires.push((Require::Hashable, k));
            Some(s)
        }
        TyKind::Set(e) => {
            let arr = t.array(e);
            let mut s = match name {
                "len" => sig(Builtin::SetLen, vec![], i64t),
                "is_empty" => sig(Builtin::SetIsEmpty, vec![], boolt),
                "insert" => msig(Builtin::SetInsert, vec![e], boolt),
                "contains" => sig(Builtin::SetContains, vec![e], boolt),
                "remove" => msig(Builtin::SetRemove, vec![e], boolt),
                "to_array" => sig(Builtin::SetToArray, vec![], arr),
                "clear" => msig(Builtin::SetClear, vec![], unit),
                _ => None,
            }?;
            s.requires.push((Require::Hashable, e));
            Some(s)
        }
        TyKind::Prim(p) => {
            if name == "to_string" {
                return sig(Builtin::ToString, vec![], strt);
            }
            if p == Prim::Char {
                return match name {
                    "is_digit" => sig(Builtin::CharIsDigit, vec![], boolt),
                    "is_alphabetic" => sig(Builtin::CharIsAlphabetic, vec![], boolt),
                    "is_whitespace" => sig(Builtin::CharIsWhitespace, vec![], boolt),
                    "to_upper" => sig(Builtin::CharToUpper, vec![], chart),
                    "to_lower" => sig(Builtin::CharToLower, vec![], chart),
                    _ => None,
                };
            }
            if !p.is_numeric() {
                return None;
            }
            let this = recv;
            match name {
                "abs" if p.is_signed() || p.is_float() => sig(Builtin::Abs, vec![], this),
                "min" => sig(Builtin::Min, vec![this], this),
                "max" => sig(Builtin::Max, vec![this], this),
                "pow" if p.is_int() => sig(Builtin::Pow, vec![t.prim(Prim::U32)], this),
                "pow" => sig(Builtin::Pow, vec![this], this),
                "checked_add" if p.is_int() => sig(Builtin::CheckedAdd, vec![this], option_of(t, la, this)?),
                "checked_sub" if p.is_int() => sig(Builtin::CheckedSub, vec![this], option_of(t, la, this)?),
                "checked_mul" if p.is_int() => sig(Builtin::CheckedMul, vec![this], option_of(t, la, this)?),
                "checked_div" if p.is_int() => sig(Builtin::CheckedDiv, vec![this], option_of(t, la, this)?),
                "checked_rem" if p.is_int() => sig(Builtin::CheckedRem, vec![this], option_of(t, la, this)?),
                "checked_shl" if p.is_int() => sig(Builtin::CheckedShl, vec![this], option_of(t, la, this)?),
                "checked_shr" if p.is_int() => sig(Builtin::CheckedShr, vec![this], option_of(t, la, this)?),
                "checked_neg" if p.is_signed() => sig(Builtin::CheckedNeg, vec![], option_of(t, la, this)?),
                "checked_pow" if p.is_int() => {
                    let u32t = t.prim(Prim::U32);
                    sig(Builtin::CheckedPow, vec![u32t], option_of(t, la, this)?)
                }
                "sqrt" if p.is_float() => sig(Builtin::Sqrt, vec![], this),
                "sin" if p.is_float() => sig(Builtin::Sin, vec![], this),
                "cos" if p.is_float() => sig(Builtin::Cos, vec![], this),
                "tan" if p.is_float() => sig(Builtin::Tan, vec![], this),
                "floor" if p.is_float() => sig(Builtin::Floor, vec![], this),
                "ceil" if p.is_float() => sig(Builtin::Ceil, vec![], this),
                "round" if p.is_float() => sig(Builtin::Round, vec![], this),
                "ln" if p.is_float() => sig(Builtin::Ln, vec![], this),
                "exp" if p.is_float() => sig(Builtin::Exp, vec![], this),
                "is_nan" if p.is_float() => sig(Builtin::IsNan, vec![], boolt),
                _ => None,
            }
        }
        _ => None,
    }
}

/// A builtin free function such as `println`.
#[derive(Debug, Clone)]
pub struct BuiltinFn {
    pub builtin: Builtin,
    pub params: Vec<Ty>,
    /// Trailing parameters beyond this count may be omitted.
    pub min_args: usize,
    pub ret: Ty,
}

pub fn builtin_fn(prog: &mut Program, infer: &mut InferCtx, name: &str) -> Option<BuiltinFn> {
    use super::infer::VarKind;
    let la = prog.lang_adts;
    let t = &mut prog.types;
    let (strt, boolt, unit) = (t.prim(Prim::Str), t.bool(), t.unit());
    Some(match name {
        "print" | "println" => {
            let any = infer.new_var(t, VarKind::General, crate::diag::Span::DUMMY);
            let b = if name == "print" { Builtin::Print } else { Builtin::Println };
            BuiltinFn { builtin: b, params: vec![any], min_args: if name == "print" { 1 } else { 0 }, ret: unit }
        }
        "input" => BuiltinFn { builtin: Builtin::Input, params: vec![], min_args: 0, ret: strt },
        "read_line" => {
            let ret = option_of(t, la, strt)?;
            BuiltinFn { builtin: Builtin::ReadLine, params: vec![], min_args: 0, ret }
        }
        "panic" => BuiltinFn { builtin: Builtin::Panic, params: vec![strt], min_args: 1, ret: t.never() },
        "assert" => BuiltinFn { builtin: Builtin::Assert, params: vec![boolt, strt], min_args: 1, ret: unit },
        _ => return None,
    })
}

/// `i32::MAX`, `f64::EPSILON`, ... with their type.
pub fn builtin_const(prim: Prim, name: &str) -> Option<(ConstVal, Prim)> {
    if prim.is_int() {
        let v = match name {
            "MAX" => prim.int_max() as i128,
            "MIN" => -(prim.int_min_magnitude() as i128),
            _ => return None,
        };
        return Some((ConstVal::Int(v), prim));
    }
    let v = match (prim, name) {
        (Prim::F64, "MAX") => f64::MAX,
        (Prim::F64, "MIN") => f64::MIN,
        (Prim::F64, "EPSILON") => f64::EPSILON,
        (Prim::F32, "MAX") => f32::MAX as f64,
        (Prim::F32, "MIN") => f32::MIN as f64,
        (Prim::F32, "EPSILON") => f32::EPSILON as f64,
        (p, "INFINITY") if p.is_float() => f64::INFINITY,
        (p, "NEG_INFINITY") if p.is_float() => f64::NEG_INFINITY,
        (p, "NAN") if p.is_float() => f64::NAN,
        (Prim::F64, "PI") => std::f64::consts::PI,
        (Prim::F32, "PI") => std::f32::consts::PI as f64,
        (Prim::F64, "E") => std::f64::consts::E,
        (Prim::F32, "E") => std::f32::consts::E as f64,
        _ => return None,
    };
    Some((ConstVal::Float(v), prim))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_consts() {
        assert_eq!(builtin_const(Prim::I8, "MAX"), Some((ConstVal::Int(127), Prim::I8)));
        assert_eq!(builtin_const(Prim::I8, "MIN"), Some((ConstVal::Int(-128), Prim::I8)));
        assert_eq!(builtin_const(Prim::U64, "MAX"), Some((ConstVal::Int(u64::MAX as i128), Prim::U64)));
        assert_eq!(builtin_const(Prim::U8, "MIN"), Some((ConstVal::Int(0), Prim::U8)));
        assert_eq!(builtin_const(Prim::I64, "MIN"), Some((ConstVal::Int(i64::MIN as i128), Prim::I64)));
        assert_eq!(builtin_const(Prim::I32, "NAN"), None);
    }

    #[test]
    fn float_consts() {
        assert_eq!(builtin_const(Prim::F32, "MAX").unwrap().0, ConstVal::Float(f32::MAX as f64));
        assert_eq!(builtin_const(Prim::F64, "EPSILON").unwrap().0, ConstVal::Float(f64::EPSILON));
        assert!(matches!(builtin_const(Prim::F64, "NAN"), Some((ConstVal::Float(v), _)) if v.is_nan()));
        assert_eq!(builtin_const(Prim::Bool, "MAX"), None);
    }

    #[test]
    fn method_signatures() {
        let mut prog = Program::default();
        let i32t = prog.types.prim(Prim::I32);
        let arr = prog.types.array(i32t);
        let push = builtin_method(&mut prog, arr, "push").unwrap();
        assert_eq!(push.builtin, Builtin::ArrPush);
        assert!(push.mutating);
        assert_eq!(push.params, vec![i32t]);
        let len = builtin_method(&mut prog, arr, "len").unwrap();
        assert!(!len.mutating);
        assert!(builtin_method(&mut prog, arr, "nope").is_none());
        let abs = builtin_method(&mut prog, i32t, "abs").unwrap();
        assert_eq!(abs.ret, i32t);
        let u8t = prog.types.prim(Prim::U8);
        assert!(builtin_method(&mut prog, u8t, "abs").is_none());
        assert!(builtin_method(&mut prog, i32t, "sqrt").is_none());
    }
}
