//! Python-style format specifications: `[[fill]align][sign][#][0][width][,|_][.precision][type]`.
//!
//! Used by the format strings `f"{x:>8.2f}"` and by `x.format(spec)`. Every engine formats through
//! [`format_with`], and the compiler checks literal specifiers with [`validate`], so what is
//! accepted at compile time is exactly what works at run time.
//!
//! Supported types: integers `d b o x X c` (and `e f g %` by converting to a float), floats
//! `e E f F g G %` (no type: the shortest round-trip form), strings `s`, booleans and characters as
//! strings (`s`, `c`) or as integers (`d b o x X`).

use super::format::{format_f32, format_f64};
use super::numeric::{decode_int, NumTy};

/// What is being formatted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Int(NumTy),
    Float(NumTy),
    Str,
    Bool,
    Char,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    fill: char,
    align: Option<char>,
    sign: char,
    alt: bool,
    zero: bool,
    width: usize,
    grouping: Option<char>,
    precision: Option<usize>,
    ty: Option<char>,
}

/// What a value of the type described by `desc` formats as, if it can take a specifier at all.
pub fn kind_of(desc: &crate::ir::TypeDesc) -> Option<Kind> {
    use crate::ir::TypeDesc;
    Some(match desc {
        TypeDesc::Num(n) if n.is_float() => Kind::Float(*n),
        TypeDesc::Num(n) => Kind::Int(*n),
        TypeDesc::Str => Kind::Str,
        TypeDesc::Bool => Kind::Bool,
        TypeDesc::Char => Kind::Char,
        _ => return None,
    })
}

/// Parses a specifier; the message says what is wrong with it.
pub fn parse(spec: &str) -> Result<Spec, String> {
    let c: Vec<char> = spec.chars().collect();
    let mut s = Spec {
        fill: ' ',
        align: None,
        sign: '-',
        alt: false,
        zero: false,
        width: 0,
        grouping: None,
        precision: None,
        ty: None,
    };
    let is_align = |ch: char| matches!(ch, '<' | '>' | '^' | '=');
    let mut i = 0;
    if c.len() >= 2 && is_align(c[1]) {
        s.fill = c[0];
        s.align = Some(c[1]);
        i = 2;
    } else if !c.is_empty() && is_align(c[0]) {
        s.align = Some(c[0]);
        i = 1;
    }
    if i < c.len() && matches!(c[i], '+' | '-' | ' ') {
        s.sign = c[i];
        i += 1;
    }
    if i < c.len() && c[i] == '#' {
        s.alt = true;
        i += 1;
    }
    if i < c.len() && c[i] == '0' {
        s.zero = true;
        i += 1;
    }
    let digits = |i: &mut usize| -> Option<usize> {
        let start = *i;
        while *i < c.len() && c[*i].is_ascii_digit() {
            *i += 1;
        }
        (*i > start).then(|| c[start..*i].iter().collect::<String>().parse().unwrap_or(usize::MAX))
    };
    if let Some(w) = digits(&mut i) {
        if w > 1 << 20 {
            return Err("format width is too large".into());
        }
        s.width = w;
    }
    if i < c.len() && matches!(c[i], ',' | '_') {
        s.grouping = Some(c[i]);
        i += 1;
    }
    if i < c.len() && c[i] == '.' {
        i += 1;
        match digits(&mut i) {
            Some(p) if p <= 1 << 20 => s.precision = Some(p),
            Some(_) => return Err("format precision is too large".into()),
            None => return Err("format specifier is missing the precision after `.`".into()),
        }
    }
    match c.len() - i {
        0 => {}
        1 => s.ty = Some(c[i]),
        _ => return Err(format!("invalid format specifier `{spec}`")),
    }
    Ok(s)
}

/// Whether `spec` can format values of `kind`.
pub fn validate(kind: Kind, spec: &str) -> Result<(), String> {
    format_with(kind, 0, Some(""), spec).map(|_| ())
}

/// Formats `bits` (the canonical register value; ignored for `Kind::Str`, which uses `text`).
pub fn format_with(kind: Kind, bits: u64, text: Option<&str>, spec: &str) -> Result<String, String> {
    let s = parse(spec)?;
    match kind {
        Kind::Int(ty) => format_int(decode_int(ty, bits), &s),
        Kind::Float(ty) => format_float(ty, bits, &s),
        Kind::Str => format_str(text.unwrap_or(""), &s),
        Kind::Bool => match s.ty {
            Some('d' | 'b' | 'o' | 'x' | 'X' | 'n') => format_int((bits != 0) as i128, &s),
            _ => format_str(if bits != 0 { "true" } else { "false" }, &s),
        },
        Kind::Char => match s.ty {
            Some('d' | 'b' | 'o' | 'x' | 'X' | 'n') => format_int(bits as u32 as i128, &s),
            _ => format_str(&char::from_u32(bits as u32).unwrap_or('\u{fffd}').to_string(), &s),
        },
    }
}

/// Pads `sign + prefix + body` to the width; `default_align` applies when none was given.
fn pad(sign: &str, prefix: &str, body: &str, s: &Spec, default_align: char) -> String {
    let (mut fill, mut align) = (s.fill, s.align);
    if s.zero && align.is_none() {
        fill = '0';
        align = Some('=');
    }
    let align = align.unwrap_or(default_align);
    let len = sign.chars().count() + prefix.chars().count() + body.chars().count();
    let gap = s.width.saturating_sub(len);
    let fills = |n: usize| std::iter::repeat_n(fill, n).collect::<String>();
    match align {
        '<' => format!("{sign}{prefix}{body}{}", fills(gap)),
        '^' => format!("{}{sign}{prefix}{body}{}", fills(gap / 2), fills(gap - gap / 2)),
        '=' => format!("{sign}{prefix}{}{body}", fills(gap)),
        _ => format!("{}{sign}{prefix}{body}", fills(gap)),
    }
}

fn sign_of(negative: bool, s: &Spec) -> &'static str {
    if negative {
        "-"
    } else {
        match s.sign {
            '+' => "+",
            ' ' => " ",
            _ => "",
        }
    }
}

/// Inserts `sep` between groups of `size` digits, counting from the right.
fn group(digits: &str, sep: char, size: usize) -> String {
    let n = digits.len();
    let mut out = String::with_capacity(n + n / size);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (n - i).is_multiple_of(size) {
            out.push(sep);
        }
        out.push(ch);
    }
    out
}

fn unknown(code: char, what: &str) -> String {
    format!("unknown format code `{code}` for {what}")
}

fn format_int(v: i128, s: &Spec) -> Result<String, String> {
    let code = s.ty.unwrap_or('d');
    match code {
        'e' | 'E' | 'f' | 'F' | 'g' | 'G' | '%' => return format_f64_spec(v as f64, None, s),
        'd' | 'n' | 'b' | 'o' | 'x' | 'X' | 'c' => {}
        other => return Err(unknown(other, "an integer")),
    }
    if s.precision.is_some() {
        return Err("a precision is not allowed for integers".into());
    }
    if code == 'c' {
        if s.sign != '-' || s.alt || s.grouping.is_some() {
            return Err("sign, `#` and grouping are not allowed with the type `c`".into());
        }
        let ch = u32::try_from(v).ok().and_then(char::from_u32).ok_or("value is not a valid character")?;
        return Ok(pad("", "", &ch.to_string(), s, '<'));
    }
    let magnitude = v.unsigned_abs();
    let (digits, prefix, size) = match code {
        'b' => (format!("{magnitude:b}"), "0b", 4),
        'o' => (format!("{magnitude:o}"), "0o", 4),
        'x' => (format!("{magnitude:x}"), "0x", 4),
        'X' => (format!("{magnitude:X}"), "0X", 4),
        _ => (magnitude.to_string(), "", 3),
    };
    let digits = match s.grouping {
        Some(',') if size != 3 => return Err(format!("`,` cannot be used with the type `{code}` (use `_`)")),
        Some(sep) => group(&digits, sep, size),
        None => digits,
    };
    let prefix = if s.alt { prefix } else { "" };
    Ok(pad(sign_of(v < 0, s), prefix, &digits, s, '>'))
}

fn format_float(ty: NumTy, bits: u64, s: &Spec) -> Result<String, String> {
    let v = if ty == NumTy::F32 { f32::from_bits(bits as u32) as f64 } else { f64::from_bits(bits) };
    // no type and no precision: the same text `print` gives
    if s.ty.is_none() && s.precision.is_none() {
        let shortest = if ty == NumTy::F32 { format_f32(v as f32) } else { format_f64(v) };
        let (neg, body) = match shortest.strip_prefix('-') {
            Some(rest) => (true, rest.to_string()),
            None => (false, shortest),
        };
        let body = match s.grouping {
            Some(sep) => with_grouped_integer_part(&body, sep),
            None => body,
        };
        return Ok(pad(sign_of(neg, s), "", &body, s, '>'));
    }
    format_f64_spec(v, Some(ty), s)
}

fn with_grouped_integer_part(body: &str, sep: char) -> String {
    let int_end = body.find(|c: char| !c.is_ascii_digit()).unwrap_or(body.len());
    format!("{}{}", group(&body[..int_end], sep, 3), &body[int_end..])
}

/// `1.5e-7` in Rust's form -> mantissa and exponent.
fn split_exp(sci: &str) -> (&str, i32) {
    let (m, e) = sci.split_once('e').unwrap_or((sci, "0"));
    (m, e.parse().unwrap_or(0))
}

fn exp_text(mantissa: &str, exp: i32, upper: bool) -> String {
    let e = if upper { 'E' } else { 'e' };
    format!("{mantissa}{e}{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs())
}

fn strip_zeros(s: &str) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}

fn format_f64_spec(v: f64, _ty: Option<NumTy>, s: &Spec) -> Result<String, String> {
    let code = s.ty.unwrap_or('g');
    if !matches!(code, 'e' | 'E' | 'f' | 'F' | 'g' | 'G' | '%') {
        return Err(unknown(code, "a float"));
    }
    let upper = code.is_ascii_uppercase();
    let neg = v.is_sign_negative() && !v.is_nan();
    let a = v.abs();
    if !a.is_finite() {
        let text = if a.is_nan() { "nan" } else { "inf" };
        let text = if upper { text.to_uppercase() } else { text.to_string() };
        let text = if code == '%' { format!("{text}%") } else { text };
        return Ok(pad(sign_of(neg, s), "", &text, s, '>'));
    }
    let p = s.precision;
    let mut body = match code {
        'f' | 'F' => format!("{:.*}", p.unwrap_or(6), a),
        '%' => format!("{:.*}%", p.unwrap_or(6), a * 100.0),
        'e' | 'E' => {
            let sci = format!("{:.*e}", p.unwrap_or(6), a);
            let (m, e) = split_exp(&sci);
            exp_text(m, e, upper)
        }
        _ => {
            let digits = match p {
                None => 6,
                Some(0) => 1,
                Some(n) => n,
            };
            let sci = format!("{:.*e}", digits - 1, a);
            let (m, e) = split_exp(&sci);
            if e >= -4 && (e as i64) < digits as i64 {
                let decimals = (digits as i64 - 1 - e as i64).max(0) as usize;
                let fixed = format!("{a:.decimals$}");
                if s.alt {
                    fixed
                } else {
                    strip_zeros(&fixed)
                }
            } else {
                exp_text(&if s.alt { m.to_string() } else { strip_zeros(m) }, e, upper)
            }
        }
    };
    if let Some(sep) = s.grouping {
        body = with_grouped_integer_part(&body, sep);
    }
    Ok(pad(sign_of(neg, s), "", &body, s, '>'))
}

fn format_str(text: &str, s: &Spec) -> Result<String, String> {
    match s.ty {
        None | Some('s' | 'c') => {}
        Some(other) => return Err(unknown(other, "a string")),
    }
    if s.sign != '-' {
        return Err("a sign is not allowed in a string format specifier".into());
    }
    if s.alt {
        return Err("`#` is not allowed in a string format specifier".into());
    }
    if s.grouping.is_some() {
        return Err("grouping is not allowed in a string format specifier".into());
    }
    if s.align == Some('=') {
        return Err("`=` alignment is not allowed in a string format specifier".into());
    }
    let shown: String = match s.precision {
        Some(p) => text.chars().take(p).collect(),
        None => text.to_string(),
    };
    Ok(pad("", "", &shown, s, '<'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(v: i64, spec: &str) -> String {
        format_with(Kind::Int(NumTy::I64), v as u64, None, spec).unwrap()
    }

    fn float(v: f64, spec: &str) -> String {
        format_with(Kind::Float(NumTy::F64), v.to_bits(), None, spec).unwrap()
    }

    fn string(v: &str, spec: &str) -> String {
        format_with(Kind::Str, 0, Some(v), spec).unwrap()
    }

    #[test]
    fn integers_follow_python() {
        assert_eq!(int(42, ""), "42");
        assert_eq!(int(42, "5"), "   42");
        assert_eq!(int(42, "<5"), "42   ");
        assert_eq!(int(42, "^6"), "  42  ");
        assert_eq!(int(42, "*>6"), "****42");
        assert_eq!(int(42, "05"), "00042");
        assert_eq!(int(-42, "05"), "-0042");
        assert_eq!(int(42, "+"), "+42");
        assert_eq!(int(42, " d"), " 42");
        assert_eq!(int(255, "x"), "ff");
        assert_eq!(int(255, "X"), "FF");
        assert_eq!(int(255, "#x"), "0xff");
        assert_eq!(int(255, "#010x"), "0x000000ff");
        assert_eq!(int(5, "b"), "101");
        assert_eq!(int(5, "#b"), "0b101");
        assert_eq!(int(8, "o"), "10");
        assert_eq!(int(-255, "x"), "-ff");
        assert_eq!(int(1234567, ","), "1,234,567");
        assert_eq!(int(1234567, "_"), "1_234_567");
        assert_eq!(int(-1234567, ">12,"), "  -1,234,567");
        assert_eq!(int(0xdeadbeef, "_x"), "dead_beef");
        assert_eq!(int(65, "c"), "A");
        assert_eq!(int(3, "f"), "3.000000");
        assert_eq!(int(3, ".2f"), "3.00");
        assert_eq!(int(i64::MIN, ""), "-9223372036854775808");
    }

    #[test]
    fn unsigned_values_use_their_full_range() {
        assert_eq!(format_with(Kind::Int(NumTy::U64), u64::MAX, None, "x").unwrap(), "ffffffffffffffff");
        assert_eq!(format_with(Kind::Int(NumTy::U8), 200, None, "08b").unwrap(), "11001000");
    }

    #[test]
    fn floats_follow_python() {
        assert_eq!(float(1.23456, ".2f"), "1.23");
        assert_eq!(float(1.23456, "8.3f"), "   1.235");
        assert_eq!(float(1.23456, "<8.1f"), "1.2     ");
        assert_eq!(float(2.5, ".0f"), "2");
        assert_eq!(float(-0.5, "+.1f"), "-0.5");
        assert_eq!(float(0.5, "+.1f"), "+0.5");
        assert_eq!(float(1234567.891, ",.2f"), "1,234,567.89");
        assert_eq!(float(0.25, ".1%"), "25.0%");
        assert_eq!(float(12345.678, ".3e"), "1.235e+04");
        assert_eq!(float(0.00012, "e"), "1.200000e-04");
        assert_eq!(float(1.5, "E"), "1.500000E+00");
        assert_eq!(float(100000.0, "g"), "100000");
        assert_eq!(float(1000000.0, "g"), "1e+06");
        assert_eq!(float(0.0001, "g"), "0.0001");
        assert_eq!(float(0.00001, "g"), "1e-05");
        assert_eq!(float(1.23456, ".3g"), "1.23");
        assert_eq!(float(1.23456, ".3"), "1.23");
        assert_eq!(float(f64::INFINITY, "f"), "inf");
        assert_eq!(float(f64::NAN, "5.1f"), "  nan");
        assert_eq!(float(-0.0, ".1f"), "-0.0");
        // no type, no precision: like print
        assert_eq!(float(2.0, ""), "2.0");
        assert_eq!(float(2.0, "6"), "   2.0");
        assert_eq!(float(-2.5, "08"), "-00002.5");
        assert_eq!(float(0.1, ""), "0.1");
        assert_eq!(format_with(Kind::Float(NumTy::F32), 0.1f32.to_bits() as u64, None, "").unwrap(), "0.1");
    }

    #[test]
    fn strings_booleans_and_characters() {
        assert_eq!(string("ab", ""), "ab");
        assert_eq!(string("ab", "5"), "ab   ");
        assert_eq!(string("ab", ">5"), "   ab");
        assert_eq!(string("ab", "^6"), "  ab  ");
        assert_eq!(string("ab", "-^7"), "--ab---");
        assert_eq!(string("abcdef", ".3"), "abc");
        assert_eq!(string("abcdef", "6.2"), "ab    ");
        assert_eq!(string("héllo", ">7"), "  héllo");
        assert_eq!(format_with(Kind::Bool, 1, None, ">6").unwrap(), "  true");
        assert_eq!(format_with(Kind::Bool, 1, None, "d").unwrap(), "1");
        assert_eq!(format_with(Kind::Char, 'x' as u64, None, "^3").unwrap(), " x ");
        assert_eq!(format_with(Kind::Char, 'A' as u64, None, "d").unwrap(), "65");
    }

    #[test]
    fn bad_specifiers_are_reported() {
        let bad = |kind, spec: &str| validate(kind, spec).unwrap_err();
        assert!(bad(Kind::Int(NumTy::I32), "s").contains("unknown format code `s`"));
        assert!(bad(Kind::Int(NumTy::I32), ".2").contains("precision"));
        assert!(bad(Kind::Int(NumTy::I32), ",x").contains("`,`"));
        assert!(bad(Kind::Float(NumTy::F64), "d").contains("for a float"));
        assert!(bad(Kind::Str, "d").contains("for a string"));
        assert!(bad(Kind::Str, "+").contains("sign"));
        assert!(bad(Kind::Str, "=5").contains("alignment"));
        assert!(bad(Kind::Int(NumTy::I32), "5.").contains("precision"));
        assert!(bad(Kind::Int(NumTy::I32), "xyz").contains("invalid format specifier"));
        assert!(validate(Kind::Int(NumTy::I32), "08.0f").is_ok());
        assert!(validate(Kind::Str, "").is_ok());
    }
}
