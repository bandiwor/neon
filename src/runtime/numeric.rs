//! Typed arithmetic on untagged 64-bit values.
//!
//! A value of any numeric type is a `u64` in *canonical form*:
//!
//! * signed integers are sign-extended to 64 bits (`-5i8` is `0xFFFF_FFFF_FFFF_FFFB`),
//! * unsigned integers are zero-extended (`250u8` is `0xFA`),
//! * `f64` is its IEEE bits and `f32` its IEEE bits zero-extended.
//!
//! Integer arithmetic is checked: a result outside the type's range is a
//! [`Trap::Overflow`]. Shift amounts are `i64`s and must lie in `0..bits`.

use super::trap::Trap;
use crate::sema::Prim;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NumTy {
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
}

impl NumTy {
    pub const ALL: [NumTy; 10] = [
        NumTy::I8,
        NumTy::I16,
        NumTy::I32,
        NumTy::I64,
        NumTy::U8,
        NumTy::U16,
        NumTy::U32,
        NumTy::U64,
        NumTy::F32,
        NumTy::F64,
    ];
    pub const INTS: [NumTy; 8] =
        [NumTy::I8, NumTy::I16, NumTy::I32, NumTy::I64, NumTy::U8, NumTy::U16, NumTy::U32, NumTy::U64];

    pub fn from_prim(p: Prim) -> Option<NumTy> {
        Some(match p {
            Prim::I8 => NumTy::I8,
            Prim::I16 => NumTy::I16,
            Prim::I32 => NumTy::I32,
            Prim::I64 => NumTy::I64,
            Prim::U8 => NumTy::U8,
            Prim::U16 => NumTy::U16,
            Prim::U32 => NumTy::U32,
            Prim::U64 => NumTy::U64,
            Prim::F32 => NumTy::F32,
            Prim::F64 => NumTy::F64,
            Prim::Bool | Prim::Char | Prim::Str => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            NumTy::I8 => "i8",
            NumTy::I16 => "i16",
            NumTy::I32 => "i32",
            NumTy::I64 => "i64",
            NumTy::U8 => "u8",
            NumTy::U16 => "u16",
            NumTy::U32 => "u32",
            NumTy::U64 => "u64",
            NumTy::F32 => "f32",
            NumTy::F64 => "f64",
        }
    }

    pub fn is_float(self) -> bool {
        matches!(self, NumTy::F32 | NumTy::F64)
    }

    pub fn is_int(self) -> bool {
        !self.is_float()
    }

    pub fn is_signed(self) -> bool {
        matches!(self, NumTy::I8 | NumTy::I16 | NumTy::I32 | NumTy::I64)
    }

    pub fn bits(self) -> u32 {
        match self {
            NumTy::I8 | NumTy::U8 => 8,
            NumTy::I16 | NumTy::U16 => 16,
            NumTy::I32 | NumTy::U32 | NumTy::F32 => 32,
            NumTy::I64 | NumTy::U64 | NumTy::F64 => 64,
        }
    }

    /// Smallest value of an integer type.
    pub fn min_value(self) -> i128 {
        debug_assert!(self.is_int());
        if self.is_signed() {
            -(1i128 << (self.bits() - 1))
        } else {
            0
        }
    }

    /// Largest value of an integer type.
    pub fn max_value(self) -> i128 {
        debug_assert!(self.is_int());
        if self.is_signed() {
            (1i128 << (self.bits() - 1)) - 1
        } else {
            (1i128 << self.bits()) - 1
        }
    }
}

// ---- encoding ---------------------------------------------------------------------------

/// Mathematical value of an integer in canonical form.
pub fn decode_int(ty: NumTy, bits: u64) -> i128 {
    debug_assert!(ty.is_int());
    if ty.is_signed() {
        bits as i64 as i128
    } else {
        bits as i128
    }
}

/// Canonical bits of `v`, or `None` if it does not fit `ty`.
pub fn encode_int(ty: NumTy, v: i128) -> Option<u64> {
    debug_assert!(ty.is_int());
    if v < ty.min_value() || v > ty.max_value() {
        None
    } else {
        Some(v as i64 as u64)
    }
}

/// Truncates `v` to `ty`'s width (two's complement) and canonicalizes.
pub fn wrap_int(ty: NumTy, v: i128) -> u64 {
    canon_bits(ty, v as u64)
}

/// Re-canonicalizes raw low bits for integer type `ty`.
pub fn canon_bits(ty: NumTy, raw: u64) -> u64 {
    let bits = ty.bits();
    if bits == 64 {
        return raw;
    }
    let masked = raw & ((1u64 << bits) - 1);
    if ty.is_signed() && masked & (1u64 << (bits - 1)) != 0 {
        masked | !((1u64 << bits) - 1)
    } else {
        masked
    }
}

fn f32_of(bits: u64) -> f32 {
    f32::from_bits(bits as u32)
}

fn f32_bits(v: f32) -> u64 {
    v.to_bits() as u64
}

// ---- arithmetic ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    Xor,
    /// The right operand is an `i64` shift amount in `0..bits`.
    Shl,
    Shr,
}

impl ArithOp {
    pub fn is_shift(self) -> bool {
        matches!(self, ArithOp::Shl | ArithOp::Shr)
    }

    pub fn is_bitwise(self) -> bool {
        matches!(self, ArithOp::And | ArithOp::Or | ArithOp::Xor | ArithOp::Shl | ArithOp::Shr)
    }

    pub fn name(self) -> &'static str {
        match self {
            ArithOp::Add => "add",
            ArithOp::Sub => "sub",
            ArithOp::Mul => "mul",
            ArithOp::Div => "div",
            ArithOp::Rem => "rem",
            ArithOp::And => "and",
            ArithOp::Or => "or",
            ArithOp::Xor => "xor",
            ArithOp::Shl => "shl",
            ArithOp::Shr => "shr",
        }
    }
}

/// `a op b` for values of type `ty`.
pub fn arith(op: ArithOp, ty: NumTy, a: u64, b: u64) -> Result<u64, Trap> {
    match ty {
        NumTy::F64 => {
            let (x, y) = (f64::from_bits(a), f64::from_bits(b));
            let r = match op {
                ArithOp::Add => x + y,
                ArithOp::Sub => x - y,
                ArithOp::Mul => x * y,
                ArithOp::Div => x / y,
                ArithOp::Rem => x % y,
                _ => return Err(Trap::Panic(format!("operator `{}` is not defined for f64", op.name()))),
            };
            Ok(r.to_bits())
        }
        NumTy::F32 => {
            let (x, y) = (f32_of(a), f32_of(b));
            let r = match op {
                ArithOp::Add => x + y,
                ArithOp::Sub => x - y,
                ArithOp::Mul => x * y,
                ArithOp::Div => x / y,
                ArithOp::Rem => x % y,
                _ => return Err(Trap::Panic(format!("operator `{}` is not defined for f32", op.name()))),
            };
            Ok(f32_bits(r))
        }
        _ => int_arith(op, ty, a, b),
    }
}

/// Add / sub / mul / div / rem on the four widest integer types without leaving the
/// native word size. The general path below handles everything else.
#[inline]
fn fast_int_arith(op: ArithOp, ty: NumTy, a: u64, b: u64) -> Option<Result<u64, Trap>> {
    macro_rules! checked {
        ($t:ty, $wide:ty) => {{
            let (x, y) = (a as $t, b as $t);
            let r = match op {
                ArithOp::Add => x.checked_add(y),
                ArithOp::Sub => x.checked_sub(y),
                ArithOp::Mul => x.checked_mul(y),
                ArithOp::Div | ArithOp::Rem if y == 0 => return Some(Err(Trap::DivideByZero)),
                ArithOp::Div => x.checked_div(y),
                // `MIN % -1` is 0 rather than an overflow
                ArithOp::Rem => Some(x.wrapping_rem(y)),
                _ => return None,
            };
            // canonical form: signed sign-extended, unsigned zero-extended
            Some(r.map(|v| v as $wide as u64).ok_or(Trap::Overflow))
        }};
    }
    match ty {
        NumTy::I64 => checked!(i64, i64),
        NumTy::I32 => checked!(i32, i64),
        NumTy::U64 => checked!(u64, u64),
        NumTy::U32 => checked!(u32, u64),
        _ => None,
    }
}

fn int_arith(op: ArithOp, ty: NumTy, a: u64, b: u64) -> Result<u64, Trap> {
    if let Some(r) = fast_int_arith(op, ty, a, b) {
        return r;
    }
    int_arith_generic(op, ty, a, b)
}

/// The reference implementation: every integer type through `i128`.
fn int_arith_generic(op: ArithOp, ty: NumTy, a: u64, b: u64) -> Result<u64, Trap> {
    match op {
        ArithOp::And => return Ok(a & b),
        ArithOp::Or => return Ok(a | b),
        ArithOp::Xor => return Ok(a ^ b),
        ArithOp::Shl | ArithOp::Shr => {
            let n = b as i64;
            if n < 0 || n >= ty.bits() as i64 {
                return Err(Trap::ShiftOverflow);
            }
            return Ok(if op == ArithOp::Shl {
                canon_bits(ty, a << n)
            } else if ty.is_signed() {
                ((a as i64) >> n) as u64
            } else {
                a >> n
            });
        }
        _ => {}
    }
    let (x, y) = (decode_int(ty, a), decode_int(ty, b));
    let r = match op {
        ArithOp::Add => x.checked_add(y),
        ArithOp::Sub => x.checked_sub(y),
        ArithOp::Mul => x.checked_mul(y),
        ArithOp::Div => {
            if y == 0 {
                return Err(Trap::DivideByZero);
            }
            x.checked_div(y)
        }
        ArithOp::Rem => {
            if y == 0 {
                return Err(Trap::DivideByZero);
            }
            // `MIN % -1` is mathematically 0, so unlike `MIN / -1` it does not overflow
            x.checked_rem(y)
        }
        _ => unreachable!("bitwise handled above"),
    };
    r.and_then(|v| encode_int(ty, v)).ok_or(Trap::Overflow)
}

/// Arithmetic negation.
pub fn neg(ty: NumTy, a: u64) -> Result<u64, Trap> {
    match ty {
        NumTy::F64 => Ok((-f64::from_bits(a)).to_bits()),
        NumTy::F32 => Ok(f32_bits(-f32_of(a))),
        _ => encode_int(ty, -decode_int(ty, a)).ok_or(Trap::Overflow),
    }
}

/// Bitwise complement of an integer.
pub fn bit_not(ty: NumTy, a: u64) -> u64 {
    debug_assert!(ty.is_int());
    canon_bits(ty, !a)
}

// ---- comparison ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    pub fn name(self) -> &'static str {
        match self {
            CmpOp::Eq => "eq",
            CmpOp::Ne => "ne",
            CmpOp::Lt => "lt",
            CmpOp::Le => "le",
            CmpOp::Gt => "gt",
            CmpOp::Ge => "ge",
        }
    }

    /// The operator with its operands swapped (`a < b` is `b > a`).
    pub fn swapped(self) -> CmpOp {
        match self {
            CmpOp::Lt => CmpOp::Gt,
            CmpOp::Le => CmpOp::Ge,
            CmpOp::Gt => CmpOp::Lt,
            CmpOp::Ge => CmpOp::Le,
            other => other,
        }
    }

    /// The logical negation (`!(a < b)` is `a >= b`); not valid for floats with NaN.
    pub fn negated(self) -> CmpOp {
        match self {
            CmpOp::Eq => CmpOp::Ne,
            CmpOp::Ne => CmpOp::Eq,
            CmpOp::Lt => CmpOp::Ge,
            CmpOp::Le => CmpOp::Gt,
            CmpOp::Gt => CmpOp::Le,
            CmpOp::Ge => CmpOp::Lt,
        }
    }

    fn holds(self, ord: Option<std::cmp::Ordering>) -> bool {
        use std::cmp::Ordering::*;
        match (self, ord) {
            (CmpOp::Ne, None) => true,
            (_, None) => false,
            (CmpOp::Eq, Some(o)) => o == Equal,
            (CmpOp::Ne, Some(o)) => o != Equal,
            (CmpOp::Lt, Some(o)) => o == Less,
            (CmpOp::Le, Some(o)) => o != Greater,
            (CmpOp::Gt, Some(o)) => o == Greater,
            (CmpOp::Ge, Some(o)) => o != Less,
        }
    }
}

/// Compares two values of type `ty`. Floats follow IEEE (NaN is unordered).
pub fn compare(op: CmpOp, ty: NumTy, a: u64, b: u64) -> bool {
    let ord = match ty {
        NumTy::F64 => f64::from_bits(a).partial_cmp(&f64::from_bits(b)),
        NumTy::F32 => f32_of(a).partial_cmp(&f32_of(b)),
        t if t.is_signed() => Some((a as i64).cmp(&(b as i64))),
        _ => Some(a.cmp(&b)),
    };
    op.holds(ord)
}

// ---- casts -------------------------------------------------------------------------------------

/// `as` conversion between numeric types: integers wrap, floats convert to
/// integers by saturating (NaN becomes 0), integers to floats round.
pub fn cast(from: NumTy, to: NumTy, a: u64) -> u64 {
    if from == to {
        return a;
    }
    match (from.is_float(), to.is_float()) {
        (false, false) => wrap_int(to, decode_int(from, a)),
        (false, true) => {
            let v = decode_int(from, a);
            if to == NumTy::F64 {
                (v as f64).to_bits()
            } else {
                f32_bits(v as f32)
            }
        }
        (true, false) => {
            let v = if from == NumTy::F64 { f64::from_bits(a) } else { f32_of(a) as f64 };
            float_to_int(to, v)
        }
        (true, true) => {
            if to == NumTy::F64 {
                (f32_of(a) as f64).to_bits()
            } else {
                f32_bits(f64::from_bits(a) as f32)
            }
        }
    }
}

fn float_to_int(to: NumTy, v: f64) -> u64 {
    if to == NumTy::U64 {
        return v as u64;
    }
    let wide = v as i64; // saturating, NaN -> 0
    let clamped = (wide as i128).clamp(to.min_value(), to.max_value());
    clamped as i64 as u64
}

/// Canonical bits for a literal integer `v` of type `ty` (wrapping).
pub fn const_int(ty: NumTy, v: i128) -> u64 {
    wrap_int(ty, v)
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_fast_paths_agree_with_the_i128_reference() {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for ty in [NumTy::I64, NumTy::I32, NumTy::U64, NumTy::U32] {
            let (lo, hi) = (ty.min_value(), ty.max_value());
            let mut vals: Vec<i128> = vec![lo, lo + 1, -1, 0, 1, 2, 3, 10, hi / 2, hi - 1, hi];
            vals.retain(|v| (lo..=hi).contains(v));
            for _ in 0..40 {
                let r = next() as i128;
                vals.push(lo + r.rem_euclid(hi - lo + 1));
            }
            for op in [ArithOp::Add, ArithOp::Sub, ArithOp::Mul, ArithOp::Div, ArithOp::Rem] {
                for &x in &vals {
                    for &y in &vals {
                        let (a, b) = (encode_int(ty, x).unwrap(), encode_int(ty, y).unwrap());
                        let fast = fast_int_arith(op, ty, a, b).expect("covered type and op");
                        assert_eq!(fast, int_arith_generic(op, ty, a, b), "{ty:?} {op:?} {x} {y}");
                    }
                }
            }
        }
    }

    use super::*;

    fn enc(ty: NumTy, v: i128) -> u64 {
        encode_int(ty, v).unwrap_or_else(|| panic!("{v} does not fit {}", ty.name()))
    }

    #[test]
    fn canonical_forms() {
        assert_eq!(enc(NumTy::I8, -5), 0xFFFF_FFFF_FFFF_FFFB);
        assert_eq!(enc(NumTy::U8, 250), 0xFA);
        assert_eq!(enc(NumTy::I64, i64::MIN as i128), 1u64 << 63);
        assert_eq!(enc(NumTy::U64, u64::MAX as i128), u64::MAX);
        assert_eq!(decode_int(NumTy::I8, enc(NumTy::I8, -128)), -128);
        assert_eq!(decode_int(NumTy::U64, u64::MAX), u64::MAX as i128);
        assert_eq!(encode_int(NumTy::I8, 128), None);
        assert_eq!(encode_int(NumTy::U8, -1), None);
        assert_eq!(wrap_int(NumTy::I8, 200), enc(NumTy::I8, -56));
        assert_eq!(wrap_int(NumTy::U16, -1), 0xFFFF);
        assert_eq!(wrap_int(NumTy::I32, 1 << 31), enc(NumTy::I32, i32::MIN as i128));
    }

    #[test]
    fn type_ranges() {
        for ty in NumTy::INTS {
            assert!(encode_int(ty, ty.min_value()).is_some());
            assert!(encode_int(ty, ty.max_value()).is_some());
            assert!(encode_int(ty, ty.min_value() - 1).is_none());
            assert!(encode_int(ty, ty.max_value() + 1).is_none());
        }
        assert_eq!(NumTy::I16.min_value(), -32768);
        assert_eq!(NumTy::U32.max_value(), u32::MAX as i128);
    }

    /// Every integer type: MAX + 1 and MIN - 1 overflow; the boundary values themselves work.
    #[test]
    fn add_sub_overflow_at_every_boundary() {
        for ty in NumTy::INTS {
            let (min, max) = (ty.min_value(), ty.max_value());
            let one = enc(ty, 1);
            assert_eq!(arith(ArithOp::Add, ty, enc(ty, max), one), Err(Trap::Overflow), "{} max+1", ty.name());
            assert_eq!(arith(ArithOp::Sub, ty, enc(ty, min), one), Err(Trap::Overflow), "{} min-1", ty.name());
            assert_eq!(arith(ArithOp::Add, ty, enc(ty, max - 1), one), Ok(enc(ty, max)));
            assert_eq!(arith(ArithOp::Sub, ty, enc(ty, min + 1), one), Ok(enc(ty, min)));
            assert_eq!(arith(ArithOp::Add, ty, enc(ty, min), enc(ty, max)).map(|v| decode_int(ty, v)), Ok(min + max));
        }
    }

    #[test]
    fn multiplication_overflow() {
        assert_eq!(arith(ArithOp::Mul, NumTy::I8, enc(NumTy::I8, 16), enc(NumTy::I8, 8)), Err(Trap::Overflow));
        assert_eq!(arith(ArithOp::Mul, NumTy::I8, enc(NumTy::I8, -16), enc(NumTy::I8, 8)), Ok(enc(NumTy::I8, -128)));
        assert_eq!(arith(ArithOp::Mul, NumTy::U8, enc(NumTy::U8, 16), enc(NumTy::U8, 16)), Err(Trap::Overflow));
        assert_eq!(arith(ArithOp::Mul, NumTy::U64, u64::MAX, 2), Err(Trap::Overflow));
        assert_eq!(arith(ArithOp::Mul, NumTy::U64, u64::MAX, u64::MAX), Err(Trap::Overflow));
        assert_eq!(arith(ArithOp::Mul, NumTy::I64, i64::MIN as u64, (-1i64) as u64), Err(Trap::Overflow));
        assert_eq!(
            arith(ArithOp::Mul, NumTy::I32, enc(NumTy::I32, 46341), enc(NumTy::I32, 46341)),
            Err(Trap::Overflow)
        );
        assert_eq!(
            arith(ArithOp::Mul, NumTy::I32, enc(NumTy::I32, 46340), enc(NumTy::I32, 46340))
                .map(|v| decode_int(NumTy::I32, v)),
            Ok(2147395600)
        );
    }

    #[test]
    fn division_and_remainder() {
        let i = |v: i128| enc(NumTy::I32, v);
        assert_eq!(arith(ArithOp::Div, NumTy::I32, i(7), i(2)), Ok(i(3)));
        assert_eq!(arith(ArithOp::Div, NumTy::I32, i(-7), i(2)), Ok(i(-3))); // truncates toward zero
        assert_eq!(arith(ArithOp::Rem, NumTy::I32, i(-7), i(2)), Ok(i(-1)));
        assert_eq!(arith(ArithOp::Rem, NumTy::I32, i(7), i(-2)), Ok(i(1)));
        assert_eq!(arith(ArithOp::Div, NumTy::I32, i(1), i(0)), Err(Trap::DivideByZero));
        assert_eq!(arith(ArithOp::Rem, NumTy::I32, i(1), i(0)), Err(Trap::DivideByZero));
        assert_eq!(arith(ArithOp::Div, NumTy::I32, i(i32::MIN as i128), i(-1)), Err(Trap::Overflow));
        assert_eq!(arith(ArithOp::Rem, NumTy::I32, i(i32::MIN as i128), i(-1)), Ok(i(0)));
        assert_eq!(arith(ArithOp::Rem, NumTy::I64, i64::MIN as u64, -1i64 as u64), Ok(0));
        assert_eq!(arith(ArithOp::Div, NumTy::U64, u64::MAX, 2), Ok(u64::MAX / 2));
        assert_eq!(arith(ArithOp::Rem, NumTy::U8, 200, 7), Ok(200 % 7));
        assert_eq!(arith(ArithOp::Div, NumTy::U8, 5, 0), Err(Trap::DivideByZero));
    }

    #[test]
    fn bitwise_ops_preserve_canonical_form() {
        let a = enc(NumTy::I8, -16); // 0xF0
        let b = enc(NumTy::I8, 0x0F);
        assert_eq!(arith(ArithOp::And, NumTy::I8, a, b), Ok(0));
        assert_eq!(arith(ArithOp::Or, NumTy::I8, a, b), Ok(enc(NumTy::I8, -1)));
        assert_eq!(arith(ArithOp::Xor, NumTy::I8, a, b), Ok(enc(NumTy::I8, -1)));
        assert_eq!(bit_not(NumTy::I8, 0), enc(NumTy::I8, -1));
        assert_eq!(bit_not(NumTy::U8, 0), 0xFF);
        assert_eq!(bit_not(NumTy::U16, 0xFFFF), 0);
        assert_eq!(bit_not(NumTy::I32, enc(NumTy::I32, 7)), enc(NumTy::I32, -8));
    }

    #[test]
    fn shifts() {
        assert_eq!(arith(ArithOp::Shl, NumTy::U8, 1, 7), Ok(128));
        assert_eq!(arith(ArithOp::Shl, NumTy::U8, 0xFF, 4), Ok(0xF0)); // bits shifted out are dropped
        assert_eq!(arith(ArithOp::Shl, NumTy::I8, 1, 7), Ok(enc(NumTy::I8, -128)));
        assert_eq!(arith(ArithOp::Shl, NumTy::I64, 1, 63), Ok(1 << 63));
        assert_eq!(arith(ArithOp::Shr, NumTy::I8, enc(NumTy::I8, -128), 7), Ok(enc(NumTy::I8, -1))); // arithmetic
        assert_eq!(arith(ArithOp::Shr, NumTy::U8, 0x80, 7), Ok(1)); // logical
        assert_eq!(arith(ArithOp::Shr, NumTy::U64, u64::MAX, 63), Ok(1));
        for ty in NumTy::INTS {
            assert_eq!(arith(ArithOp::Shl, ty, 1, ty.bits() as u64), Err(Trap::ShiftOverflow), "{}", ty.name());
            assert_eq!(arith(ArithOp::Shr, ty, 1, (ty.bits() - 1) as u64), Ok(0));
            assert_eq!(arith(ArithOp::Shl, ty, 1, (-1i64) as u64), Err(Trap::ShiftOverflow));
        }
    }

    #[test]
    fn negation() {
        assert_eq!(neg(NumTy::I8, enc(NumTy::I8, 5)), Ok(enc(NumTy::I8, -5)));
        assert_eq!(neg(NumTy::I8, enc(NumTy::I8, -128)), Err(Trap::Overflow));
        assert_eq!(neg(NumTy::I64, i64::MIN as u64), Err(Trap::Overflow));
        assert_eq!(neg(NumTy::U8, 0), Ok(0));
        assert_eq!(neg(NumTy::U8, 1), Err(Trap::Overflow));
        assert_eq!(neg(NumTy::F64, 2.5f64.to_bits()), Ok((-2.5f64).to_bits()));
        assert_eq!(neg(NumTy::F32, 2.5f32.to_bits() as u64), Ok((-2.5f32).to_bits() as u64));
    }

    #[test]
    fn float_arithmetic_is_ieee() {
        let f = |v: f64| v.to_bits();
        let g = |bits: u64| f64::from_bits(bits);
        assert_eq!(g(arith(ArithOp::Add, NumTy::F64, f(0.1), f(0.2)).unwrap()), 0.1 + 0.2);
        assert_eq!(g(arith(ArithOp::Div, NumTy::F64, f(1.0), f(0.0)).unwrap()), f64::INFINITY);
        assert!(g(arith(ArithOp::Div, NumTy::F64, f(0.0), f(0.0)).unwrap()).is_nan());
        assert_eq!(g(arith(ArithOp::Rem, NumTy::F64, f(5.5), f(2.0)).unwrap()), 1.5);
        assert_eq!(g(arith(ArithOp::Sub, NumTy::F64, f(f64::MAX), f(-f64::MAX)).unwrap()), f64::INFINITY);
        assert!(arith(ArithOp::Shl, NumTy::F64, f(1.0), 1).is_err());
        // f32 arithmetic happens in f32
        let h = |v: f32| v.to_bits() as u64;
        let s = arith(ArithOp::Add, NumTy::F32, h(0.1), h(0.2)).unwrap();
        assert_eq!(f32::from_bits(s as u32), 0.1f32 + 0.2f32);
        let big = arith(ArithOp::Mul, NumTy::F32, h(f32::MAX), h(2.0)).unwrap();
        assert_eq!(f32::from_bits(big as u32), f32::INFINITY);
    }

    #[test]
    fn comparisons() {
        let i8v = |v: i128| enc(NumTy::I8, v);
        assert!(compare(CmpOp::Lt, NumTy::I8, i8v(-1), i8v(1)));
        assert!(!compare(CmpOp::Lt, NumTy::U8, 0xFF, 1)); // unsigned compare
        assert!(compare(CmpOp::Gt, NumTy::U64, u64::MAX, 1));
        assert!(compare(CmpOp::Lt, NumTy::I64, i64::MIN as u64, 0));
        assert!(compare(CmpOp::Le, NumTy::I32, enc(NumTy::I32, 3), enc(NumTy::I32, 3)));
        assert!(compare(CmpOp::Ne, NumTy::I32, 1, 2));
        let nan = f64::NAN.to_bits();
        let one = 1.0f64.to_bits();
        assert!(!compare(CmpOp::Eq, NumTy::F64, nan, nan));
        assert!(compare(CmpOp::Ne, NumTy::F64, nan, nan));
        assert!(!compare(CmpOp::Lt, NumTy::F64, nan, one) && !compare(CmpOp::Ge, NumTy::F64, nan, one));
        assert!(compare(CmpOp::Eq, NumTy::F64, 0.0f64.to_bits(), (-0.0f64).to_bits()));
        assert!(compare(CmpOp::Lt, NumTy::F32, (-1.5f32).to_bits() as u64, 0.5f32.to_bits() as u64));
    }

    #[test]
    fn cmp_op_algebra() {
        for op in [CmpOp::Eq, CmpOp::Ne, CmpOp::Lt, CmpOp::Le, CmpOp::Gt, CmpOp::Ge] {
            for (a, b) in [(1u64, 2u64), (2, 2), (3, 2)] {
                assert_eq!(compare(op, NumTy::U64, a, b), compare(op.swapped(), NumTy::U64, b, a));
                assert_eq!(compare(op, NumTy::U64, a, b), !compare(op.negated(), NumTy::U64, a, b));
            }
        }
    }

    #[test]
    fn int_to_int_casts_wrap() {
        let c = cast;
        assert_eq!(c(NumTy::I32, NumTy::U8, enc(NumTy::I32, 300)), 44);
        assert_eq!(c(NumTy::I32, NumTy::I8, enc(NumTy::I32, 200)), enc(NumTy::I8, -56));
        assert_eq!(c(NumTy::I8, NumTy::U8, enc(NumTy::I8, -1)), 255);
        assert_eq!(c(NumTy::I8, NumTy::I64, enc(NumTy::I8, -1)), (-1i64) as u64);
        assert_eq!(c(NumTy::I8, NumTy::U64, enc(NumTy::I8, -1)), u64::MAX);
        assert_eq!(c(NumTy::U64, NumTy::I64, u64::MAX), (-1i64) as u64);
        assert_eq!(c(NumTy::U8, NumTy::I8, 200), enc(NumTy::I8, -56));
        assert_eq!(c(NumTy::U32, NumTy::I32, u32::MAX as u64), enc(NumTy::I32, -1));
        assert_eq!(c(NumTy::I64, NumTy::I64, 77), 77);
    }

    #[test]
    fn int_float_casts() {
        let f64b = |v: f64| v.to_bits();
        assert_eq!(cast(NumTy::I32, NumTy::F64, enc(NumTy::I32, -3)), f64b(-3.0));
        assert_eq!(cast(NumTy::U64, NumTy::F64, u64::MAX), f64b(u64::MAX as f64));
        assert_eq!(cast(NumTy::I64, NumTy::F32, enc(NumTy::I64, 16_777_217)), (16_777_217i64 as f32).to_bits() as u64);
        // float -> int saturates, NaN -> 0, truncates toward zero
        assert_eq!(cast(NumTy::F64, NumTy::I32, f64b(3.99)), enc(NumTy::I32, 3));
        assert_eq!(cast(NumTy::F64, NumTy::I32, f64b(-3.99)), enc(NumTy::I32, -3));
        assert_eq!(cast(NumTy::F64, NumTy::I8, f64b(1e10)), enc(NumTy::I8, 127));
        assert_eq!(cast(NumTy::F64, NumTy::I8, f64b(-1e10)), enc(NumTy::I8, -128));
        assert_eq!(cast(NumTy::F64, NumTy::U8, f64b(-5.0)), 0);
        assert_eq!(cast(NumTy::F64, NumTy::U8, f64b(300.0)), 255);
        assert_eq!(cast(NumTy::F64, NumTy::I32, f64::NAN.to_bits()), 0);
        assert_eq!(cast(NumTy::F64, NumTy::U64, f64b(1e30)), u64::MAX);
        assert_eq!(cast(NumTy::F64, NumTy::I64, f64b(-1e30)), i64::MIN as u64);
        assert_eq!(cast(NumTy::F64, NumTy::U32, f64b(4294967296.0)), u32::MAX as u64);
        assert_eq!(cast(NumTy::F32, NumTy::I16, (-40000.5f32).to_bits() as u64), enc(NumTy::I16, -32768));
    }

    #[test]
    fn float_float_casts() {
        assert_eq!(cast(NumTy::F32, NumTy::F64, 0.1f32.to_bits() as u64), (0.1f32 as f64).to_bits());
        assert_eq!(cast(NumTy::F64, NumTy::F32, 0.1f64.to_bits()), (0.1f64 as f32).to_bits() as u64);
        assert_eq!(f32::from_bits(cast(NumTy::F64, NumTy::F32, 1e300f64.to_bits()) as u32), f32::INFINITY);
    }

    /// Exhaustively cross-check 8-bit arithmetic against a reference built on i32.
    #[test]
    fn exhaustive_8_bit_arithmetic() {
        for ty in [NumTy::I8, NumTy::U8] {
            let (min, max) = (ty.min_value(), ty.max_value());
            for x in min..=max {
                for y in min..=max {
                    for (op, f) in [
                        (ArithOp::Add, (|a, b| a + b) as fn(i128, i128) -> i128),
                        (ArithOp::Sub, |a, b| a - b),
                        (ArithOp::Mul, |a, b| a * b),
                    ] {
                        let expect = f(x, y);
                        let got = arith(op, ty, enc(ty, x), enc(ty, y));
                        if expect < min || expect > max {
                            assert_eq!(got, Err(Trap::Overflow), "{} {x} {} {y}", ty.name(), op.name());
                        } else {
                            assert_eq!(
                                got.map(|v| decode_int(ty, v)),
                                Ok(expect),
                                "{} {x} {} {y}",
                                ty.name(),
                                op.name()
                            );
                        }
                    }
                    if y != 0 {
                        let q = x / y;
                        let got = arith(ArithOp::Div, ty, enc(ty, x), enc(ty, y));
                        if q > max {
                            assert_eq!(got, Err(Trap::Overflow));
                        } else {
                            assert_eq!(got.map(|v| decode_int(ty, v)), Ok(q));
                        }
                    }
                }
            }
        }
    }
}
