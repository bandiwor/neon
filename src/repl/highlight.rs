//! Syntax highlighting for the interactive prompt.
//!
//! Highlighting runs on every keystroke over text that is usually half typed, so it cannot depend on
//! the parser: a forgiving scanner cuts the text into tokens (an unterminated string or comment simply
//! runs to the end), and a light pass over the tokens decides what an identifier is from its
//! neighbourhood: a declaration, a parameter, a local, a global of the session, a function, a type.

use crate::fx::FxHashSet as HashSet;
use crate::syntax::Keyword;

/// What a stretch of text is, as far as colors are concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Plain,
    Keyword,
    Type,
    Variant,
    Global,
    Param,
    Local,
    Function,
    Number,
    Str,
    Char,
    Operator,
    Punct,
    Bracket,
    Comment,
}

impl Class {
    const ALL: [Class; 15] = [
        Class::Plain,
        Class::Keyword,
        Class::Type,
        Class::Variant,
        Class::Global,
        Class::Param,
        Class::Local,
        Class::Function,
        Class::Number,
        Class::Str,
        Class::Char,
        Class::Operator,
        Class::Punct,
        Class::Bracket,
        Class::Comment,
    ];

    /// The color of the class, as RGB.
    fn rgb(self) -> (u8, u8, u8) {
        match self {
            Class::Plain | Class::Global => (0xff, 0xff, 0xff),
            Class::Keyword => (0xc6, 0x8a, 0xff),
            Class::Type => (0x4e, 0xc9, 0xb0),
            Class::Variant => (0xff, 0xb8, 0x6c),
            Class::Param => (0xff, 0x9e, 0x64),
            Class::Local => (0xe6, 0xdb, 0x74),
            Class::Function => (0x6f, 0xb3, 0xff),
            Class::Number => (0x70, 0x6a, 0xff),
            Class::Str | Class::Char => (0x8c, 0xcf, 0x6a),
            Class::Operator => (0xff, 0x4d, 0x5e),
            Class::Punct | Class::Bracket => (0xd8, 0xd8, 0xd8),
            Class::Comment => (0x6b, 0x72, 0x89),
        }
    }
}

/// Escape sequences for every class, in the richest color mode the terminal has.
#[derive(Debug, Clone)]
pub struct Theme {
    codes: Vec<String>,
    enabled: bool,
}

pub const RESET: &str = "\x1b[0m";
/// Gray text for hints: the preview and the completion menu.
pub const DIM: &str = "\x1b[38;5;244m";

impl Theme {
    /// A theme that adds no escape sequences at all.
    pub fn plain() -> Theme {
        Theme { codes: vec![String::new(); Class::ALL.len()], enabled: false }
    }

    /// Colors for a terminal: 24-bit when it says so, the 256-color cube otherwise; none when
    /// `NO_COLOR` is set.
    pub fn detect() -> Theme {
        if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
            return Theme::plain();
        }
        let truecolor = std::env::var("COLORTERM").is_ok_and(|v| v == "truecolor" || v == "24bit");
        Theme::new(truecolor)
    }

    pub fn new(truecolor: bool) -> Theme {
        let codes = Class::ALL
            .iter()
            .map(|c| {
                let (r, g, b) = c.rgb();
                if truecolor {
                    format!("\x1b[38;2;{r};{g};{b}m")
                } else {
                    format!("\x1b[38;5;{}m", xterm256(r, g, b))
                }
            })
            .collect();
        Theme { codes, enabled: true }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn code(&self, c: Class) -> &str {
        &self.codes[Class::ALL.iter().position(|&x| x == c).expect("every class is listed")]
    }

    /// The gray used for hints (empty when colors are off).
    pub fn dim(&self) -> &'static str {
        if self.enabled {
            DIM
        } else {
            ""
        }
    }

    pub fn reset(&self) -> &'static str {
        if self.enabled {
            RESET
        } else {
            ""
        }
    }
}

/// The xterm-256 color closest to an RGB value (the 6x6x6 cube or the gray ramp).
fn xterm256(r: u8, g: u8, b: u8) -> u8 {
    let level = |v: u8| -> u8 {
        match v {
            0..=47 => 0,
            48..=114 => 1,
            v => ((v as u16 - 35) / 40) as u8,
        }
    };
    let step = |l: u8| -> i32 { [0, 95, 135, 175, 215, 255][l as usize] };
    let (lr, lg, lb) = (level(r), level(g), level(b));
    let cube = 16 + 36 * lr + 6 * lg + lb;
    let d = |a: i32, b: i32| (a - b) * (a - b);
    let cube_dist = d(step(lr), r as i32) + d(step(lg), g as i32) + d(step(lb), b as i32);
    // the 24 grays: 8, 18, ... 238
    let avg = (r as i32 + g as i32 + b as i32) / 3;
    let gray_i = ((avg - 8).max(0) / 10).min(23);
    let gv = 8 + 10 * gray_i;
    let gray_dist = d(gv, r as i32) + d(gv, g as i32) + d(gv, b as i32);
    if gray_dist < cube_dist {
        232 + gray_i as u8
    } else {
        cube
    }
}

// ---- scanning ------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tok {
    Ident,
    Number,
    Str,
    Char,
    Comment,
    /// An operator or punctuation, classified later.
    Sym,
    Space,
}

#[derive(Debug, Clone, Copy)]
struct Span {
    lo: usize,
    hi: usize,
    tok: Tok,
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

fn is_ident_char(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// Cuts `src` into tokens, in order, covering every character. Inside an `f"..."` the `{...}` parts are
/// scanned as code. Stops at `until_brace` (a `}` or a format `:`) when scanning an interpolation.
fn scan(src: &[char], i: &mut usize, out: &mut Vec<Span>, in_interp: bool) {
    let mut depth = 0usize;
    while *i < src.len() {
        let c = src[*i];
        let lo = *i;
        let next = src.get(*i + 1).copied();
        if in_interp && depth == 0 && (c == '}' || (c == ':' && next != Some(':'))) {
            return;
        }
        if c.is_whitespace() {
            while *i < src.len() && src[*i].is_whitespace() {
                *i += 1;
            }
            out.push(Span { lo, hi: *i, tok: Tok::Space });
        } else if c == '/' && next == Some('/') {
            while *i < src.len() && src[*i] != '\n' {
                *i += 1;
            }
            out.push(Span { lo, hi: *i, tok: Tok::Comment });
        } else if c == '/' && next == Some('*') {
            let mut nest = 0;
            while *i < src.len() {
                if src[*i] == '/' && src.get(*i + 1) == Some(&'*') {
                    nest += 1;
                    *i += 2;
                } else if src[*i] == '*' && src.get(*i + 1) == Some(&'/') {
                    nest -= 1;
                    *i += 2;
                    if nest == 0 {
                        break;
                    }
                } else {
                    *i += 1;
                }
            }
            out.push(Span { lo, hi: *i, tok: Tok::Comment });
        } else if c == 'f' && next == Some('"') {
            *i += 2;
            scan_fstring(src, i, out, lo);
        } else if c == '"' {
            *i += 1;
            while *i < src.len() {
                match src[*i] {
                    '\\' => *i += 2,
                    '"' => {
                        *i += 1;
                        break;
                    }
                    _ => *i += 1,
                }
            }
            *i = (*i).min(src.len());
            out.push(Span { lo, hi: *i, tok: Tok::Str });
        } else if c == '\'' {
            // 'x' or '\n'; a lone quote is just a symbol
            let mut j = *i + 1;
            if src.get(j) == Some(&'\\') {
                j += 2;
            } else {
                j += 1;
            }
            if src.get(j) == Some(&'\'') {
                *i = j + 1;
                out.push(Span { lo, hi: *i, tok: Tok::Char });
            } else {
                *i += 1;
                out.push(Span { lo, hi: *i, tok: Tok::Sym });
            }
        } else if c.is_ascii_digit() {
            while *i < src.len() {
                let d = src[*i];
                if d.is_alphanumeric() || d == '_' {
                    // 1e-5 / 1E+5
                    if (d == 'e' || d == 'E') && matches!(src.get(*i + 1), Some('+') | Some('-')) {
                        *i += 1;
                    }
                    *i += 1;
                } else if d == '.' && src.get(*i + 1).is_some_and(|n| n.is_ascii_digit()) {
                    *i += 1;
                } else {
                    break;
                }
            }
            out.push(Span { lo, hi: *i, tok: Tok::Number });
        } else if is_ident_start(c) {
            while *i < src.len() && is_ident_char(src[*i]) {
                *i += 1;
            }
            out.push(Span { lo, hi: *i, tok: Tok::Ident });
        } else {
            // symbols: group the multi-character operators
            let two: String = src[*i..(*i + 3).min(src.len())].iter().collect();
            let len = if two.starts_with("..=") || two.starts_with("<<=") || two.starts_with(">>=") {
                3
            } else if [
                "->", "=>", "==", "!=", "<=", ">=", "&&", "||", "<<", ">>", "+=", "-=", "*=", "/=", "%=", "&=", "|=",
                "^=", "..", "::",
            ]
            .iter()
            .any(|op| two.starts_with(op))
            {
                2
            } else {
                1
            };
            match c {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth = depth.saturating_sub(1),
                _ => {}
            }
            *i += len;
            out.push(Span { lo, hi: *i, tok: Tok::Sym });
        }
    }
}

/// After `f"`: the literal pieces are string text, the `{...}` parts are code.
fn scan_fstring(src: &[char], i: &mut usize, out: &mut Vec<Span>, start: usize) {
    let mut lit_lo = start;
    while *i < src.len() {
        match src[*i] {
            '\\' => *i += 2,
            '"' => {
                *i += 1;
                out.push(Span { lo: lit_lo, hi: *i, tok: Tok::Str });
                return;
            }
            '{' if src.get(*i + 1) == Some(&'{') => *i += 2,
            '}' if src.get(*i + 1) == Some(&'}') => *i += 2,
            '{' => {
                out.push(Span { lo: lit_lo, hi: *i, tok: Tok::Str });
                let open = *i;
                out.push(Span { lo: open, hi: open + 1, tok: Tok::Sym });
                *i += 1;
                scan(src, i, out, true);
                // an optional format spec after a `:` is text
                if src.get(*i) == Some(&':') {
                    let lo = *i;
                    while *i < src.len() && src[*i] != '}' && src[*i] != '"' {
                        *i += 1;
                    }
                    out.push(Span { lo, hi: *i, tok: Tok::Str });
                }
                if src.get(*i) == Some(&'}') {
                    out.push(Span { lo: *i, hi: *i + 1, tok: Tok::Sym });
                    *i += 1;
                }
                lit_lo = *i;
            }
            _ => *i += 1,
        }
    }
    *i = (*i).min(src.len());
    out.push(Span { lo: lit_lo, hi: *i, tok: Tok::Str });
}

// ---- names ---------------------------------------------------------------------------------------

/// Free functions that exist without being declared.
const BUILTIN_FUNCTIONS: &[&str] = &["print", "println", "input", "read_line", "panic", "assert"];

/// The top-level names the session has declared so far.
#[derive(Debug, Default, Clone)]
pub struct Names {
    pub vars: HashSet<String>,
    pub functions: HashSet<String>,
    pub types: HashSet<String>,
}

impl Names {
    pub fn from_cells(cells: &[String]) -> Names {
        let mut n = Names::default();
        for c in cells {
            n.absorb(c);
        }
        n
    }

    /// Adds what `cell` declares at its top level.
    pub fn absorb(&mut self, cell: &str) {
        let src: Vec<char> = cell.chars().collect();
        let mut toks = Vec::new();
        scan(&src, &mut 0, &mut toks, false);
        let toks: Vec<Span> = toks.into_iter().filter(|t| t.tok != Tok::Space && t.tok != Tok::Comment).collect();
        let text = |t: &Span| -> String { src[t.lo..t.hi].iter().collect() };
        let mut depth = 0i32;
        for (k, t) in toks.iter().enumerate() {
            let s = text(t);
            match s.as_str() {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth -= 1,
                _ => {}
            }
            if depth != 0 || t.tok != Tok::Ident {
                continue;
            }
            let Some(name) = toks.get(k + 1).filter(|n| n.tok == Tok::Ident).map(&text) else { continue };
            match s.as_str() {
                "fn" => {
                    self.functions.insert(name);
                }
                "let" | "var" | "const" => {
                    self.vars.insert(name);
                }
                "struct" | "enum" | "trait" => {
                    self.types.insert(name);
                }
                _ => {}
            }
        }
    }
}

// ---- classification --------------------------------------------------------------------------------

/// The class of every character of `text`.
pub fn highlight(text: &str, names: &Names) -> Vec<Class> {
    let src: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    scan(&src, &mut 0, &mut spans, false);
    let mut classes = vec![Class::Plain; src.len()];
    let word = |s: &Span| -> String { src[s.lo..s.hi].iter().collect() };

    // significant tokens only, with their index in `spans`
    let sig: Vec<usize> = (0..spans.len()).filter(|&k| !matches!(spans[k].tok, Tok::Space | Tok::Comment)).collect();
    let words: Vec<String> = sig.iter().map(|&k| word(&spans[k])).collect();
    let n = sig.len();

    // per token class, filled below; identifiers start as `None`
    let mut class: Vec<Option<Class>> = vec![None; n];
    for (j, &k) in sig.iter().enumerate() {
        class[j] = match spans[k].tok {
            Tok::Number => Some(Class::Number),
            Tok::Str => Some(Class::Str),
            Tok::Char => Some(Class::Char),
            Tok::Sym => Some(sym_class(&words[j])),
            _ => None,
        };
    }

    // names bound by parameters, `let`, `for`, closures and match arms: (name, class, from, until) in
    // token indices; a use is looked up by position
    let mut bindings: Vec<(String, Class, usize, usize)> = Vec::new();
    let mut declared: Vec<Option<Class>> = vec![None; n];
    let ident = |m: usize| spans[sig[m]].tok == Tok::Ident;
    // the token index of the `}` matching the `{` at `open` (or the end)
    let block_end = |open: usize| -> usize {
        let mut d = 0;
        for (m, w) in words.iter().enumerate().skip(open) {
            match w.as_str() {
                "{" => d += 1,
                "}" => {
                    d -= 1;
                    if d == 0 {
                        return m;
                    }
                }
                _ => {}
            }
        }
        n
    };
    let mut depth_brace = 0i32;
    for j in 0..n {
        let w = words[j].as_str();
        if !ident(j) {
            match w {
                "{" => depth_brace += 1,
                "}" => depth_brace -= 1,
                "|" if expression_may_start(&words, j) => {
                    // closure parameters: `|a, b|`, `|(a, b)|`, `|x: i64|`
                    let mut m = j + 1;
                    while m < n && words[m] != "|" {
                        if ident(m) && keyword(&words[m]).is_none() && !words[m.wrapping_sub(1)].eq(":") {
                            declared[m] = Some(Class::Param);
                            bindings.push((words[m].clone(), Class::Param, m, n));
                        }
                        m += 1;
                    }
                }
                "=>" => {
                    // match-arm bindings: lowercase names of the pattern before `=>`
                    let mut m = j;
                    while m > 0 {
                        m -= 1;
                        let wm = words[m].as_str();
                        if matches!(wm, "," | "{" | ";" | "|") || (wm == "}" && depth_of(&words, m, j) == 0) {
                            break;
                        }
                        if ident(m)
                            && keyword(wm).is_none()
                            && !starts_upper(wm)
                            && declared[m].is_none()
                            && !words.get(m + 1).is_some_and(|x| x == "(" || x == "::")
                            && !words.get(m.wrapping_sub(1)).is_some_and(|x| x == "::" || x == ".")
                        {
                            declared[m] = Some(Class::Local);
                            bindings.push((wm.to_string(), Class::Local, m, n));
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        match w {
            "fn" => {
                // fn name<..>(params) -> ty { body }
                let mut m = j + 1;
                if m < n && ident(m) {
                    declared[m] = Some(Class::Function);
                    m += 1;
                }
                if m < n && words[m] == "<" {
                    let mut g = 0;
                    while m < n {
                        match words[m].as_str() {
                            "<" => g += 1,
                            ">" => {
                                g -= 1;
                                if g == 0 {
                                    m += 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                        m += 1;
                    }
                }
                if m < n && words[m] == "(" {
                    // the body starts at the first `{` after the parameter list
                    let mut p = 0;
                    let mut close = m;
                    while close < n {
                        match words[close].as_str() {
                            "(" | "[" => p += 1,
                            ")" | "]" => {
                                p -= 1;
                                if p == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        close += 1;
                    }
                    let mut body = close;
                    while body < n && words[body] != "{" && words[body] != ";" {
                        body += 1;
                    }
                    let until = if body < n && words[body] == "{" { block_end(body) } else { body };
                    let mut p = 0;
                    for q in m..close.min(n) {
                        match words[q].as_str() {
                            "(" | "[" => p += 1,
                            ")" | "]" => p -= 1,
                            _ => {}
                        }
                        if p == 1
                            && ident(q)
                            && words.get(q + 1).is_some_and(|x| x == ":")
                            && keyword(&words[q]).is_none()
                        {
                            declared[q] = Some(Class::Param);
                            bindings.push((words[q].clone(), Class::Param, q, until));
                        } else if p == 1 && words[q] == "self" {
                            declared[q] = Some(Class::Keyword);
                        }
                    }
                }
            }
            "let" | "var" | "const" | "for" => {
                // binders up to `=`, `:` or `in` at the same nesting
                let top_level = depth_brace == 0 && w != "for";
                let stop: &[&str] = if w == "for" { &["in"] } else { &["=", ":", ";"] };
                let mut m = j + 1;
                let mut p = 0;
                while m < n && !(p == 0 && stop.contains(&words[m].as_str())) {
                    match words[m].as_str() {
                        "(" | "[" => p += 1,
                        ")" | "]" => p -= 1,
                        "{" | "}" => break,
                        _ => {}
                    }
                    if ident(m) && keyword(&words[m]).is_none() && !starts_upper(&words[m]) {
                        if top_level {
                            declared[m] = Some(Class::Global);
                        } else {
                            declared[m] = Some(Class::Local);
                            bindings.push((words[m].clone(), Class::Local, m, n));
                        }
                    }
                    m += 1;
                }
            }
            "struct" | "enum" | "trait" | "impl" if j + 1 < n && ident(j + 1) => declared[j + 1] = Some(Class::Type),
            _ => {}
        }
    }

    let bound = |w: &str, j: usize| -> Option<Class> {
        bindings.iter().rev().find(|(name, _, from, until)| name == w && *from <= j && j < *until).map(|b| b.1)
    };
    for j in 0..n {
        if class[j].is_some() || !ident(j) {
            continue;
        }
        let w = words[j].as_str();
        let prev = if j > 0 { words[j - 1].as_str() } else { "" };
        let next = words.get(j + 1).map(|s| s.as_str()).unwrap_or("");
        let c = if let Some(d) = declared[j] {
            d
        } else if let Some(k) = keyword(w) {
            if k.is_primitive_type() {
                Class::Type
            } else {
                Class::Keyword
            }
        } else if prev == "." {
            if next == "(" {
                Class::Function
            } else {
                Class::Plain
            }
        } else if next == "::" {
            Class::Type
        } else if prev == "::" {
            if starts_upper(w) {
                Class::Variant
            } else if next == "(" {
                Class::Function
            } else {
                Class::Plain
            }
        } else if starts_upper(w) {
            if w.chars().count() > 1 && w.chars().all(|c| c.is_uppercase() || c.is_ascii_digit() || c == '_') {
                Class::Global // CONSTANT
            } else {
                Class::Type
            }
        } else if let Some(b) = bound(w, j) {
            b
        } else if next == "(" || names.functions.contains(w) || BUILTIN_FUNCTIONS.contains(&w) {
            Class::Function
        } else if names.vars.contains(w) {
            Class::Global
        } else if names.types.contains(w) {
            Class::Type
        } else {
            Class::Plain
        };
        class[j] = Some(c);
    }

    // comments and everything else
    for (k, sp) in spans.iter().enumerate() {
        let c = match sig.binary_search(&k) {
            Ok(j) => class[j].unwrap_or(Class::Plain),
            Err(_) if sp.tok == Tok::Comment => Class::Comment,
            Err(_) => Class::Plain,
        };
        for slot in &mut classes[sp.lo..sp.hi] {
            *slot = c;
        }
    }
    classes
}

fn keyword(w: &str) -> Option<Keyword> {
    Keyword::ALL.iter().copied().find(|k| k.as_str() == w)
}

fn starts_upper(w: &str) -> bool {
    w.chars().next().is_some_and(|c| c.is_uppercase())
}

fn sym_class(w: &str) -> Class {
    match w {
        "(" | ")" | "[" | "]" | "{" | "}" => Class::Bracket,
        "," | "." | ";" | ":" | "::" => Class::Punct,
        _ => Class::Operator,
    }
}

/// Whether a `|` at token `j` can open a closure (it follows something after which an expression starts).
fn expression_may_start(words: &[String], j: usize) -> bool {
    if j == 0 {
        return true;
    }
    matches!(
        words[j - 1].as_str(),
        "(" | ","
            | "="
            | "=>"
            | "{"
            | ";"
            | "return"
            | "+"
            | "-"
            | "*"
            | "/"
            | "&&"
            | "||"
            | "["
            | ":"
            | "?"
            | "else"
            | "in"
    )
}

/// Net bracket depth of the tokens `from..to` (to find a `}` that ends a previous arm).
fn depth_of(words: &[String], from: usize, to: usize) -> i32 {
    let mut d = 0;
    for w in &words[from..to] {
        match w.as_str() {
            "(" | "[" | "{" => d += 1,
            ")" | "]" | "}" => d -= 1,
            _ => {}
        }
    }
    d
}

// ---- rendering -------------------------------------------------------------------------------------

/// `text` with escape sequences: one string per line of the text, each starting in the default style.
pub fn render_lines(text: &str, classes: &[Class], theme: &Theme) -> Vec<String> {
    let mut lines = vec![String::new()];
    let mut current: Option<Class> = None;
    for (ch, &class) in text.chars().zip(classes) {
        if ch == '\n' {
            if current.is_some() {
                lines.last_mut().expect("one line").push_str(theme.reset());
                current = None;
            }
            lines.push(String::new());
            continue;
        }
        if theme.enabled() && current != Some(class) {
            lines.last_mut().expect("one line").push_str(theme.code(class));
            current = Some(class);
        }
        lines.last_mut().expect("one line").push(ch);
    }
    if current.is_some() {
        lines.last_mut().expect("one line").push_str(theme.reset());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes(text: &str) -> Vec<(String, Class)> {
        classes_with(text, &Names::default())
    }

    /// The text grouped into runs of one class (whitespace dropped), for readable assertions.
    fn classes_with(text: &str, names: &Names) -> Vec<(String, Class)> {
        let cl = highlight(text, names);
        let mut out: Vec<(String, Class)> = Vec::new();
        for (ch, c) in text.chars().zip(cl) {
            if ch.is_whitespace() {
                out.push((String::new(), Class::Plain)); // run separator
                continue;
            }
            match out.last_mut() {
                Some((s, last)) if *last == c && !s.is_empty() => s.push(ch),
                _ => out.push((ch.to_string(), c)),
            }
        }
        out.into_iter().filter(|(s, _)| !s.is_empty()).collect()
    }

    fn of(text: &str, word: &str) -> Class {
        classes(text).into_iter().find(|(s, _)| s == word).unwrap_or_else(|| panic!("no run {word:?} in {text:?}")).1
    }

    #[test]
    fn the_screenshot_example_gets_its_colors() {
        let src = "fn input_data() -> Data {\n    let parts = read_line().expect(\"x\").split(\" \");\n    if parts.len() != 4 { panic(\"bad\"); }\n    let v = parts.skip(1).map(|s| => s.parse_u64() as u8);\n    match parts[0] {\n        \"rgb\" => Data::RGB(v[0], v[1]),\n        other => panic(f\"Invalid type {other}\"),\n    }\n}";
        assert_eq!(of(src, "fn"), Class::Keyword);
        assert_eq!(of(src, "input_data"), Class::Function);
        assert_eq!(of(src, "->"), Class::Operator);
        assert_eq!(of(src, "=>"), Class::Operator);
        assert_eq!(of(src, "Data"), Class::Type);
        assert_eq!(of(src, "parts"), Class::Local);
        assert_eq!(of(src, "read_line"), Class::Function);
        assert_eq!(of(src, "split"), Class::Function);
        assert_eq!(of(src, "!="), Class::Operator);
        assert_eq!(of(src, "4"), Class::Number);
        assert_eq!(of(src, "u8"), Class::Type);
        assert_eq!(of(src, "RGB"), Class::Variant);
        assert_eq!(of(src, "other"), Class::Local);
        assert_eq!(of(src, "\"rgb\""), Class::Str);
    }

    #[test]
    fn parameters_locals_and_globals_have_their_own_colors() {
        let mut names = Names::default();
        names.absorb("let total = 5;\nfn helper() -> i64 { 1 }");
        let got = classes_with("fn add(a: i64, b: i64) -> i64 { let c = a + b; c + total + helper() }", &names);
        let class_of = |w: &str, nth: usize| got.iter().filter(|(s, _)| s == w).nth(nth).map(|(_, c)| *c);
        assert_eq!(class_of("add", 0), Some(Class::Function));
        assert_eq!(class_of("a", 0), Some(Class::Param), "declaration");
        assert_eq!(class_of("a", 1), Some(Class::Param), "use");
        assert_eq!(class_of("c", 0), Some(Class::Local));
        assert_eq!(class_of("c", 1), Some(Class::Local));
        assert_eq!(class_of("total", 0), Some(Class::Global));
        assert_eq!(class_of("helper", 0), Some(Class::Function));
    }

    #[test]
    fn top_level_names_are_globals_and_closure_parameters_are_parameters() {
        assert_eq!(classes("let count = 3;").iter().find(|(s, _)| s == "count").unwrap().1, Class::Global);
        let got = classes("xs.map(|v| => v * 2)");
        assert_eq!(
            got.iter().filter(|(s, _)| s == "v").map(|(_, c)| *c).collect::<Vec<_>>(),
            [Class::Param, Class::Param]
        );
        assert_eq!(of("xs.map(|v| => v)", "map"), Class::Function);
    }

    #[test]
    fn brackets_commas_and_dots_are_not_operators() {
        let got = classes("a.b(1, [2])");
        assert!(got.iter().any(|(s, c)| s == "(" && *c == Class::Bracket));
        assert!(got.iter().any(|(s, c)| s == "," && *c == Class::Punct));
        assert!(got.iter().any(|(s, c)| s == "." && *c == Class::Punct));
        assert_eq!(of("1 + 2 == 3", "+"), Class::Operator);
        assert_eq!(of("1 + 2 == 3", "=="), Class::Operator);
        assert_eq!(of("0..=5", "..="), Class::Operator);
    }

    #[test]
    fn literals_and_comments() {
        assert_eq!(of("let x = 3.14;", "3.14"), Class::Number);
        assert_eq!(of("let x = 0xFFu8;", "0xFFu8"), Class::Number);
        assert_eq!(of("1..5", "1"), Class::Number);
        assert_eq!(of("'a'", "'a'"), Class::Char);
        assert_eq!(of("'\\n'", "'\\n'"), Class::Char);
        assert_eq!(of("x // note", "//"), Class::Comment);
        assert_eq!(of("/* a */ x", "/*"), Class::Comment);
        assert_eq!(of("true && false", "true"), Class::Keyword);
    }

    #[test]
    fn unfinished_input_does_not_confuse_the_scanner() {
        // an open string, comment and f-string run to the end
        assert_eq!(classes("\"abc")[0].1, Class::Str);
        assert_eq!(classes("/* abc")[0].1, Class::Comment);
        let cl = highlight("f\"x {a", &Names::default());
        assert_eq!(cl.len(), 6);
        assert_eq!(cl[0], Class::Str);
        // every character is classified, whatever the input
        for text in ["", "'", "f\"{", "f\"{a:", "1e", "a::", "|", "=>", "\"\\"] {
            assert_eq!(highlight(text, &Names::default()).len(), text.chars().count(), "{text:?}");
        }
    }

    #[test]
    fn fstring_interpolations_are_code() {
        let src = "f\"sum {a + 1:>4} done\"";
        let cl = highlight(src, &Names::default());
        let at = |s: &str| src.chars().position(|c| c.to_string() == s).unwrap();
        assert_eq!(cl[0], Class::Str);
        assert_eq!(cl[at("+")], Class::Operator);
        assert_eq!(cl[at("1")], Class::Number);
        assert_eq!(cl[at("{")], Class::Bracket);
        assert_eq!(cl[src.len() - 3], Class::Str);
    }

    #[test]
    fn declarations_register_session_names() {
        let mut names = Names::default();
        names.absorb("struct Point { x: i64 }\nlet origin = 1;\nfn dist() { let inner = 2; }\nenum Shape { A }");
        assert!(names.types.contains("Point") && names.types.contains("Shape"));
        assert!(names.vars.contains("origin") && !names.vars.contains("inner"));
        assert!(names.functions.contains("dist"));
    }

    #[test]
    fn rendering_resets_at_line_breaks_and_can_be_plain() {
        let text = "let a = \"x\ny\";";
        let cl = highlight(text, &Names::default());
        let lines = render_lines(text, &cl, &Theme::new(true));
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with(RESET) && lines[1].starts_with("\x1b[38;2;"), "{lines:?}");
        let plain = render_lines(text, &cl, &Theme::plain());
        assert_eq!(plain, ["let a = \"x", "y\";"]);
    }

    #[test]
    fn the_256_color_fallback_is_close() {
        assert_eq!(xterm256(0xff, 0xff, 0xff), 231);
        assert_eq!(xterm256(0, 0, 0), 16);
        assert!((232..=255).contains(&xterm256(0x80, 0x80, 0x80)));
        assert_eq!(xterm256(0xff, 0x00, 0x00), 196);
    }
}
