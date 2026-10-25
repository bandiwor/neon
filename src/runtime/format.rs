//! Text forms of primitive values, as printed by `println` / `to_string`.

use super::numeric::NumTy;

/// Floats print like Rust's `{:?}`: `1.0`, `0.1`, `1e16`, `NaN`, `inf`.
pub fn format_f64(v: f64) -> String {
    format!("{v:?}")
}

pub fn format_f32(v: f32) -> String {
    format!("{v:?}")
}

/// Formats the canonical bits of a number of type `ty`.
pub fn format_num(ty: NumTy, bits: u64) -> String {
    match ty {
        NumTy::F32 => format_f32(f32::from_bits(bits as u32)),
        NumTy::F64 => format_f64(f64::from_bits(bits)),
        t if t.is_signed() => (bits as i64).to_string(),
        _ => bits.to_string(),
    }
}

pub fn format_bool(bits: u64) -> &'static str {
    if bits != 0 {
        "true"
    } else {
        "false"
    }
}

pub fn format_char(bits: u64) -> String {
    char::from_u32(bits as u32).unwrap_or('\u{fffd}').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats() {
        assert_eq!(format_f64(1.0), "1.0");
        assert_eq!(format_f64(0.1), "0.1");
        assert_eq!(format_f64(-2.5), "-2.5");
        assert_eq!(format_f64(f64::NAN), "NaN");
        assert_eq!(format_f64(f64::INFINITY), "inf");
        assert_eq!(format_f32(0.1), "0.1");
        assert_eq!(format_f32(16777216.0), "16777216.0");
    }

    #[test]
    fn numbers_by_type() {
        assert_eq!(format_num(NumTy::I8, (-5i64) as u64), "-5");
        assert_eq!(format_num(NumTy::U8, 250), "250");
        assert_eq!(format_num(NumTy::U64, u64::MAX), "18446744073709551615");
        assert_eq!(format_num(NumTy::I64, i64::MIN as u64), "-9223372036854775808");
        assert_eq!(format_num(NumTy::F32, 1.5f32.to_bits() as u64), "1.5");
        assert_eq!(format_num(NumTy::F64, 2.0f64.to_bits()), "2.0");
    }

    #[test]
    fn bool_and_char() {
        assert_eq!(format_bool(1), "true");
        assert_eq!(format_bool(0), "false");
        assert_eq!(format_char('é' as u64), "é");
        assert_eq!(format_char(0x110000), "\u{fffd}");
    }
}
