//! Numeric builtins shared by every engine.

use super::numeric::{arith, cast, compare, decode_int, encode_int, neg, ArithOp, CmpOp, NumTy};
use super::trap::Trap;
use crate::builtin::Builtin;

/// `abs`
pub fn abs(ty: NumTy, a: u64) -> Result<u64, Trap> {
    match ty {
        NumTy::F64 => Ok(f64::from_bits(a).abs().to_bits()),
        NumTy::F32 => Ok(f32::from_bits(a as u32).abs().to_bits() as u64),
        t if t.is_signed() => {
            if decode_int(t, a) < 0 {
                neg(t, a)
            } else {
                Ok(a)
            }
        }
        _ => Ok(a),
    }
}

/// `min` / `max`; for floats a NaN operand yields the other operand.
pub fn min_max(ty: NumTy, a: u64, b: u64, want_max: bool) -> u64 {
    if ty.is_float() {
        let (x, y) = (to_f64(ty, a), to_f64(ty, b));
        if x.is_nan() {
            return b;
        }
        if y.is_nan() {
            return a;
        }
    }
    let a_less = compare(CmpOp::Lt, ty, a, b);
    match (a_less, want_max) {
        (true, false) | (false, true) => a,
        _ => b,
    }
}

fn to_f64(ty: NumTy, bits: u64) -> f64 {
    if ty == NumTy::F32 {
        f32::from_bits(bits as u32) as f64
    } else {
        f64::from_bits(bits)
    }
}

/// `x.pow(n)`: integer exponent (a `u32`) for integers, same-type exponent for floats.
pub fn pow(ty: NumTy, base: u64, exp: u64) -> Result<u64, Trap> {
    match ty {
        NumTy::F64 => Ok(f64::from_bits(base).powf(f64::from_bits(exp)).to_bits()),
        NumTy::F32 => Ok(f32::from_bits(base as u32).powf(f32::from_bits(exp as u32)).to_bits() as u64),
        _ => {
            let mut result = encode_int(ty, 1).expect("1 fits every integer type");
            let mut remaining = exp;
            // square-and-multiply with overflow checks on every step that matters
            let mut square = base;
            while remaining > 0 {
                if remaining & 1 == 1 {
                    result = arith(ArithOp::Mul, ty, result, square)?;
                }
                remaining >>= 1;
                if remaining > 0 {
                    square = arith(ArithOp::Mul, ty, square, square)?;
                }
            }
            Ok(result)
        }
    }
}

/// Transcendental and rounding functions on floats (`Sqrt`, `Sin`, ...).
pub fn float_fn(b: Builtin, ty: NumTy, bits: u64) -> Option<u64> {
    macro_rules! apply {
        ($f:ident) => {
            if ty == NumTy::F32 {
                f32::from_bits(bits as u32).$f().to_bits() as u64
            } else {
                f64::from_bits(bits).$f().to_bits()
            }
        };
    }
    Some(match b {
        Builtin::Sqrt => apply!(sqrt),
        Builtin::Sin => apply!(sin),
        Builtin::Cos => apply!(cos),
        Builtin::Tan => apply!(tan),
        Builtin::Floor => apply!(floor),
        Builtin::Ceil => apply!(ceil),
        Builtin::Round => apply!(round),
        Builtin::Ln => apply!(ln),
        Builtin::Exp => apply!(exp),
        _ => return None,
    })
}

pub fn is_nan(ty: NumTy, bits: u64) -> bool {
    to_f64(ty, bits).is_nan()
}

/// Parses a decimal `i64`, as `parse_i64` does.
pub fn parse_i64(s: &str) -> Result<u64, Trap> {
    s.trim().parse::<i64>().map(|v| v as u64).map_err(|_| Trap::ParseError(s.to_string()))
}

/// Parses a decimal `u64`, as `parse_u64` does (a leading `+` is accepted, a sign `-` is not).
pub fn parse_u64(s: &str) -> Result<u64, Trap> {
    s.trim().parse::<u64>().map_err(|_| Trap::ParseError(s.to_string()))
}

pub fn parse_f64(s: &str) -> Result<u64, Trap> {
    s.trim().parse::<f64>().map(f64::to_bits).map_err(|_| Trap::ParseError(s.to_string()))
}

/// Converts an index value of integer type `ty` to a `usize`, or reports
/// it out of bounds for a container of length `len`.
pub fn checked_index(ty: NumTy, idx: u64, len: usize) -> Result<usize, Trap> {
    let v = decode_int(ty, idx);
    if v < 0 || v >= len as i128 {
        Err(Trap::IndexOutOfBounds { index: v, len })
    } else {
        Ok(v as usize)
    }
}

/// `checked_add`, `checked_pow`, ...: the result, or `None` where the plain
/// operation would trap (overflow, division by zero, a bad shift amount).
pub fn checked_op(b: Builtin, ty: NumTy, a: u64, x: u64) -> Option<u64> {
    match b {
        Builtin::CheckedAdd => arith(ArithOp::Add, ty, a, x).ok(),
        Builtin::CheckedSub => arith(ArithOp::Sub, ty, a, x).ok(),
        Builtin::CheckedMul => arith(ArithOp::Mul, ty, a, x).ok(),
        Builtin::CheckedDiv => arith(ArithOp::Div, ty, a, x).ok(),
        Builtin::CheckedRem => arith(ArithOp::Rem, ty, a, x).ok(),
        Builtin::CheckedShl => arith(ArithOp::Shl, ty, a, x).ok(),
        Builtin::CheckedShr => arith(ArithOp::Shr, ty, a, x).ok(),
        Builtin::CheckedPow => pow(ty, a, x).ok(),
        Builtin::CheckedNeg => neg(ty, a).ok(),
        other => panic!("{other:?} is not a checked operation"),
    }
}

/// An `i64` quantity (a length, position) as the integer type `ty` expects.
pub fn to_i64_bits(v: i64) -> u64 {
    cast(NumTy::I64, NumTy::I64, v as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn i(ty: NumTy, v: i128) -> u64 {
        encode_int(ty, v).unwrap()
    }

    #[test]
    fn abs_and_overflow() {
        assert_eq!(abs(NumTy::I8, i(NumTy::I8, -5)), Ok(i(NumTy::I8, 5)));
        assert_eq!(abs(NumTy::I8, i(NumTy::I8, -128)), Err(Trap::Overflow));
        assert_eq!(abs(NumTy::U8, 7), Ok(7));
        assert_eq!(abs(NumTy::F64, (-2.5f64).to_bits()), Ok(2.5f64.to_bits()));
        assert_eq!(abs(NumTy::F32, (-2.5f32).to_bits() as u64), Ok(2.5f32.to_bits() as u64));
    }

    #[test]
    fn min_max_ints_and_floats() {
        assert_eq!(min_max(NumTy::I32, i(NumTy::I32, -1), i(NumTy::I32, 1), false), i(NumTy::I32, -1));
        assert_eq!(min_max(NumTy::I32, i(NumTy::I32, -1), i(NumTy::I32, 1), true), i(NumTy::I32, 1));
        assert_eq!(min_max(NumTy::U8, 200, 100, false), 100);
        let nan = f64::NAN.to_bits();
        let two = 2.0f64.to_bits();
        assert_eq!(min_max(NumTy::F64, nan, two, false), two);
        assert_eq!(min_max(NumTy::F64, two, nan, true), two);
        assert_eq!(min_max(NumTy::F64, 1.0f64.to_bits(), two, true), two);
    }

    #[test]
    fn integer_pow() {
        assert_eq!(pow(NumTy::I32, i(NumTy::I32, 3), 4), Ok(i(NumTy::I32, 81)));
        assert_eq!(pow(NumTy::I32, i(NumTy::I32, 7), 0), Ok(i(NumTy::I32, 1)));
        assert_eq!(pow(NumTy::I32, i(NumTy::I32, -2), 31), Ok(i(NumTy::I32, i32::MIN as i128)));
        assert_eq!(pow(NumTy::I32, i(NumTy::I32, 2), 31), Err(Trap::Overflow));
        assert_eq!(pow(NumTy::U8, 2, 7), Ok(128));
        assert_eq!(pow(NumTy::U8, 2, 8), Err(Trap::Overflow));
        assert_eq!(pow(NumTy::U64, 2, 63), Ok(1 << 63));
        assert_eq!(pow(NumTy::U64, 10, 20), Err(Trap::Overflow));
        assert_eq!(pow(NumTy::I64, 1, u32::MAX as u64), Ok(1));
        assert_eq!(pow(NumTy::F64, 2.0f64.to_bits(), 10.0f64.to_bits()), Ok(1024.0f64.to_bits()));
    }

    #[test]
    fn float_functions() {
        assert_eq!(float_fn(Builtin::Sqrt, NumTy::F64, 9.0f64.to_bits()), Some(3.0f64.to_bits()));
        assert_eq!(float_fn(Builtin::Floor, NumTy::F32, (-1.5f32).to_bits() as u64), Some((-2.0f32).to_bits() as u64));
        assert_eq!(float_fn(Builtin::Round, NumTy::F64, 2.5f64.to_bits()), Some(3.0f64.to_bits()));
        assert_eq!(float_fn(Builtin::Abs, NumTy::F64, 0), None);
        assert!(is_nan(NumTy::F64, f64::NAN.to_bits()));
        assert!(!is_nan(NumTy::F32, 1.0f32.to_bits() as u64));
    }

    #[test]
    fn parsing_and_indexing() {
        assert_eq!(parse_i64(" -42 "), Ok((-42i64) as u64));
        assert!(matches!(parse_i64("4x"), Err(Trap::ParseError(_))));
        assert_eq!(parse_u64(" 18446744073709551615 "), Ok(u64::MAX));
        assert!(matches!(parse_u64("-1"), Err(Trap::ParseError(_))));
        assert!(matches!(parse_u64("18446744073709551616"), Err(Trap::ParseError(_))));
        assert_eq!(parse_f64("2.5"), Ok(2.5f64.to_bits()));
        assert_eq!(checked_index(NumTy::I32, i(NumTy::I32, 2), 3), Ok(2));
        assert_eq!(checked_index(NumTy::I32, i(NumTy::I32, 3), 3), Err(Trap::IndexOutOfBounds { index: 3, len: 3 }));
        assert_eq!(checked_index(NumTy::I8, i(NumTy::I8, -1), 3), Err(Trap::IndexOutOfBounds { index: -1, len: 3 }));
        assert_eq!(
            checked_index(NumTy::U64, u64::MAX, 3),
            Err(Trap::IndexOutOfBounds { index: u64::MAX as i128, len: 3 })
        );
    }
}
