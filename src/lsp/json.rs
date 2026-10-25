//! A small JSON implementation (std only): enough for JSON-RPC.
//!
//! Objects keep their key order, numbers are `f64` (integers up to 2^53 print
//! without a fraction), and the parser rejects trailing input and nesting
//! deeper than [`MAX_DEPTH`] so hostile input cannot overflow the stack.

use std::fmt;

pub const MAX_DEPTH: usize = 128;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    pub offset: usize,
    pub message: String,
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid JSON at byte {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for JsonError {}

impl Json {
    pub fn parse(text: &str) -> Result<Json, JsonError> {
        let mut p = Parser { s: text.as_bytes(), text, pos: 0 };
        p.skip_ws();
        let v = p.value(0)?;
        p.skip_ws();
        if p.pos != p.s.len() {
            return Err(p.err("trailing characters after the value"));
        }
        Ok(v)
    }

    /// An object from `(key, value)` pairs.
    pub fn object<K: Into<String>>(pairs: Vec<(K, Json)>) -> Json {
        Json::Obj(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Num(n) if n.fract() == 0.0 && n.abs() < 9.007_199_254_740_992e15 => Some(*n as i64),
            _ => None,
        }
    }

    pub fn as_u32(&self) -> Option<u32> {
        self.as_i64().and_then(|n| u32::try_from(n).ok())
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }

    /// Follows `a.b.c` style keys through nested objects.
    pub fn path(&self, keys: &[&str]) -> Option<&Json> {
        keys.iter().try_fold(self, |v, k| v.get(k))
    }
}

impl From<&str> for Json {
    fn from(s: &str) -> Json {
        Json::Str(s.to_string())
    }
}

impl From<String> for Json {
    fn from(s: String) -> Json {
        Json::Str(s)
    }
}

impl From<bool> for Json {
    fn from(b: bool) -> Json {
        Json::Bool(b)
    }
}

impl From<u32> for Json {
    fn from(n: u32) -> Json {
        Json::Num(n as f64)
    }
}

impl From<i64> for Json {
    fn from(n: i64) -> Json {
        Json::Num(n as f64)
    }
}

impl From<usize> for Json {
    fn from(n: usize) -> Json {
        Json::Num(n as f64)
    }
}

impl<T: Into<Json>> From<Vec<T>> for Json {
    fn from(v: Vec<T>) -> Json {
        Json::Arr(v.into_iter().map(Into::into).collect())
    }
}

impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(v: Option<T>) -> Json {
        v.map_or(Json::Null, Into::into)
    }
}

// ---- writing -----------------------------------------------------------------------------

impl fmt::Display for Json {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Json::Null => f.write_str("null"),
            Json::Bool(b) => write!(f, "{b}"),
            Json::Num(n) => {
                if !n.is_finite() {
                    f.write_str("null")
                } else if n.fract() == 0.0 && n.abs() < 9.007_199_254_740_992e15 {
                    write!(f, "{}", *n as i64)
                } else {
                    write!(f, "{n}")
                }
            }
            Json::Str(s) => write_string(f, s),
            Json::Arr(a) => {
                f.write_str("[")?;
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{v}")?;
                }
                f.write_str("]")
            }
            Json::Obj(o) => {
                f.write_str("{")?;
                for (i, (k, v)) in o.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write_string(f, k)?;
                    write!(f, ":{v}")?;
                }
                f.write_str("}")
            }
        }
    }
}

fn write_string(f: &mut fmt::Formatter<'_>, s: &str) -> fmt::Result {
    f.write_str("\"")?;
    for c in s.chars() {
        match c {
            '"' => f.write_str("\\\"")?,
            '\\' => f.write_str("\\\\")?,
            '\n' => f.write_str("\\n")?,
            '\r' => f.write_str("\\r")?,
            '\t' => f.write_str("\\t")?,
            c if (c as u32) < 0x20 => write!(f, "\\u{:04x}", c as u32)?,
            c => write!(f, "{c}")?,
        }
    }
    f.write_str("\"")
}

// ---- parsing ------------------------------------------------------------------------------

struct Parser<'a> {
    s: &'a [u8],
    text: &'a str,
    pos: usize,
}

impl Parser<'_> {
    fn err(&self, message: &str) -> JsonError {
        JsonError { offset: self.pos, message: message.to_string() }
    }

    fn skip_ws(&mut self) {
        while matches!(self.s.get(self.pos), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn eat(&mut self, lit: &str) -> bool {
        if self.text[self.pos..].starts_with(lit) {
            self.pos += lit.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, JsonError> {
        if depth > MAX_DEPTH {
            return Err(self.err("nesting is too deep"));
        }
        match self.s.get(self.pos) {
            None => Err(self.err("unexpected end of input")),
            Some(b'n') if self.eat("null") => Ok(Json::Null),
            Some(b't') if self.eat("true") => Ok(Json::Bool(true)),
            Some(b'f') if self.eat("false") => Ok(Json::Bool(false)),
            Some(b'"') => self.string().map(Json::Str),
            Some(b'[') => {
                self.pos += 1;
                let mut items = Vec::new();
                self.skip_ws();
                if self.s.get(self.pos) == Some(&b']') {
                    self.pos += 1;
                    return Ok(Json::Arr(items));
                }
                loop {
                    self.skip_ws();
                    items.push(self.value(depth + 1)?);
                    self.skip_ws();
                    match self.s.get(self.pos) {
                        Some(b',') => self.pos += 1,
                        Some(b']') => {
                            self.pos += 1;
                            return Ok(Json::Arr(items));
                        }
                        _ => return Err(self.err("expected `,` or `]`")),
                    }
                }
            }
            Some(b'{') => {
                self.pos += 1;
                let mut pairs = Vec::new();
                self.skip_ws();
                if self.s.get(self.pos) == Some(&b'}') {
                    self.pos += 1;
                    return Ok(Json::Obj(pairs));
                }
                loop {
                    self.skip_ws();
                    if self.s.get(self.pos) != Some(&b'"') {
                        return Err(self.err("expected a string key"));
                    }
                    let key = self.string()?;
                    self.skip_ws();
                    if self.s.get(self.pos) != Some(&b':') {
                        return Err(self.err("expected `:`"));
                    }
                    self.pos += 1;
                    self.skip_ws();
                    let v = self.value(depth + 1)?;
                    pairs.push((key, v));
                    self.skip_ws();
                    match self.s.get(self.pos) {
                        Some(b',') => self.pos += 1,
                        Some(b'}') => {
                            self.pos += 1;
                            return Ok(Json::Obj(pairs));
                        }
                        _ => return Err(self.err("expected `,` or `}`")),
                    }
                }
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.err("unexpected character")),
        }
    }

    fn number(&mut self) -> Result<Json, JsonError> {
        let start = self.pos;
        if self.s.get(self.pos) == Some(&b'-') {
            self.pos += 1;
        }
        let digits = |p: &mut Parser<'_>| {
            let from = p.pos;
            while matches!(p.s.get(p.pos), Some(b'0'..=b'9')) {
                p.pos += 1;
            }
            p.pos > from
        };
        if !digits(self) {
            return Err(self.err("expected digits"));
        }
        if self.s.get(self.pos) == Some(&b'.') {
            self.pos += 1;
            if !digits(self) {
                return Err(self.err("expected digits after the decimal point"));
            }
        }
        if matches!(self.s.get(self.pos), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.s.get(self.pos), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !digits(self) {
                return Err(self.err("expected exponent digits"));
            }
        }
        self.text[start..self.pos].parse::<f64>().map(Json::Num).map_err(|_| self.err("invalid number"))
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        let h = self.text.get(self.pos..self.pos + 4).ok_or_else(|| self.err("truncated \\u escape"))?;
        let v = u32::from_str_radix(h, 16).map_err(|_| self.err("invalid \\u escape"))?;
        self.pos += 4;
        Ok(v)
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.pos += 1; // opening quote
        let mut out = String::new();
        loop {
            let Some(&b) = self.s.get(self.pos) else { return Err(self.err("unterminated string")) };
            match b {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    let Some(&e) = self.s.get(self.pos) else { return Err(self.err("unterminated escape")) };
                    self.pos += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let c = if (0xD800..0xDC00).contains(&hi) {
                                // a surrogate pair
                                if !self.eat("\\u") {
                                    return Err(self.err("unpaired surrogate"));
                                }
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return Err(self.err("invalid low surrogate"));
                                }
                                char::from_u32(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00))
                            } else {
                                char::from_u32(hi)
                            };
                            out.push(c.ok_or_else(|| self.err("invalid code point"))?);
                        }
                        _ => return Err(self.err("invalid escape")),
                    }
                }
                0..=0x1f => return Err(self.err("control character in string")),
                _ => {
                    // copy one UTF-8 character
                    let ch = self.text[self.pos..].chars().next().expect("in bounds");
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Json {
        Json::parse(s).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    #[test]
    fn scalars_and_containers() {
        assert_eq!(p("null"), Json::Null);
        assert_eq!(p(" true "), Json::Bool(true));
        assert_eq!(p("-12.5e1"), Json::Num(-125.0));
        assert_eq!(p("0"), Json::Num(0.0));
        assert_eq!(p("[1, 2,3]"), Json::Arr(vec![Json::Num(1.0), Json::Num(2.0), Json::Num(3.0)]));
        assert_eq!(p("{}"), Json::Obj(vec![]));
        assert_eq!(p("[]"), Json::Arr(vec![]));
        let o = p(r#"{"a": {"b": [true, null]}, "c": "x"}"#);
        assert_eq!(o.path(&["a", "b"]).and_then(Json::as_array).map(<[Json]>::len), Some(2));
        assert_eq!(o.get("c").and_then(Json::as_str), Some("x"));
        assert!(o.get("zzz").is_none());
    }

    #[test]
    fn strings_with_escapes_and_unicode() {
        assert_eq!(p(r#""a\"b\\c\/d\n\t\u0041""#), Json::Str("a\"b\\c/d\n\tA".into()));
        assert_eq!(p(r#""\ud83d\ude00""#), Json::Str("😀".into()));
        assert_eq!(p("\"héllo 名前\""), Json::Str("héllo 名前".into()));
    }

    #[test]
    fn output_is_compact_and_round_trips() {
        let v = Json::object(vec![
            ("id", Json::from(7u32)),
            ("name", "a\"b\n\u{1}é".into()),
            ("list", vec![1i64, 2, 3].into()),
            ("none", Json::Null),
            ("pi", Json::Num(3.5)),
            ("flag", true.into()),
        ]);
        let text = v.to_string();
        assert_eq!(text, r#"{"id":7,"name":"a\"b\n\u0001é","list":[1,2,3],"none":null,"pi":3.5,"flag":true}"#);
        assert_eq!(Json::parse(&text).unwrap(), v);
        assert_eq!(Json::Num(f64::NAN).to_string(), "null");
        assert_eq!(Json::Num(-0.0).to_string(), "0");
    }

    #[test]
    fn accessors() {
        let v = p(r#"{"n": 42, "f": 1.5, "big": 99999999999, "neg": -1}"#);
        assert_eq!(v.get("n").and_then(Json::as_i64), Some(42));
        assert_eq!(v.get("n").and_then(Json::as_u32), Some(42));
        assert_eq!(v.get("f").and_then(Json::as_i64), None);
        assert_eq!(v.get("f").and_then(Json::as_f64), Some(1.5));
        assert_eq!(v.get("big").and_then(Json::as_u32), None);
        assert_eq!(v.get("neg").and_then(Json::as_u32), None);
        assert_eq!(Json::from(Option::<u32>::None), Json::Null);
        assert_eq!(Json::from(Some(3u32)), Json::Num(3.0));
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in [
            "",
            "{",
            "[1,",
            "[1 2]",
            "{\"a\" 1}",
            "{1: 2}",
            "\"abc",
            "\"\\x\"",
            "tru",
            "01x",
            "-",
            "1.",
            "1e",
            "[1]]",
            "nul",
            "\"\\ud800\"",
            "\"\\ud800\\u0041\"",
            "\"a\u{1}b\"",
            "{\"a\":1,}",
            "[1,]",
        ] {
            assert!(Json::parse(bad).is_err(), "{bad:?} should not parse");
        }
        let e = Json::parse("[1, @]").unwrap_err();
        assert_eq!(e.offset, 4);
    }

    #[test]
    fn nesting_is_limited() {
        let deep = "[".repeat(MAX_DEPTH + 5) + &"]".repeat(MAX_DEPTH + 5);
        assert!(Json::parse(&deep).unwrap_err().message.contains("too deep"));
        let ok = "[".repeat(MAX_DEPTH) + &"]".repeat(MAX_DEPTH);
        assert!(Json::parse(&ok).is_ok());
    }
}
