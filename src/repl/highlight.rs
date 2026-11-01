//! Syntax highlighting for the interactive prompt.
//!
//! The colors come from the compiler itself: the lexer cuts the input into tokens (keywords, literals,
//! operators, punctuation) and the very analysis the language server runs, over the session's cells
//! followed by the input, says what every identifier refers to: a parameter, a local, a global, a
//! function, a type, an enum variant. Half-typed input is fine, the parser recovers from errors.

use crate::diag::FileId;
use crate::lsp::analysis::complete::{closing_delimiters, scan};
use crate::lsp::analysis::tokens::{
    M_LOCAL, T_COMMENT, T_ENUM, T_ENUM_MEMBER, T_FUNCTION, T_INTERFACE, T_KEYWORD, T_METHOD, T_NAMESPACE, T_PARAMETER,
    T_STRUCT, T_TYPE, T_TYPE_PARAM, T_VARIABLE,
};
use crate::lsp::analysis::Analysis;
use crate::syntax::{tokenize, Interner, Keyword, LitPool, TokenKind};
use crate::vfs::Vfs;

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

// ---- classification --------------------------------------------------------------------------------

fn token_class(kind: TokenKind) -> Option<Class> {
    use TokenKind::*;
    Some(match kind {
        Kw(k) if k.is_primitive_type() => Class::Type,
        Kw(Keyword::SelfValue | Keyword::SelfType) => Class::Keyword,
        Kw(_) => Class::Keyword,
        Int(_) | Float(_) => Class::Number,
        Str(_) | Char(_) | FStrBegin | FStrLit(_) | FStrSpec(_) | FStrEnd => Class::Str,
        FStrOpen | FStrClose | LParen | RParen | LBrace | RBrace | LBracket | RBracket => Class::Bracket,
        Comma | Semi | Colon | ColonColon | Dot => Class::Punct,
        Ident(_) | Underscore => return None,
        _ => Class::Operator,
    })
}

/// The class of every character of `text`, typed after the session's `cells`.
pub fn highlight(cells: &[String], text: &str, vfs: &dyn Vfs) -> Vec<Class> {
    let mut by_byte = vec![Class::Plain; text.len()];
    // tokens: everything but identifiers
    let mut interner = Interner::new();
    let mut pool = LitPool::new();
    let mut diags = crate::diag::DiagBag::new();
    let tokens = tokenize(FileId(0), text, &mut interner, &mut pool, &mut diags);
    // what an unfinished input needs for the parser to see a whole expression: an operand after a
    // trailing operator, a name after a dot
    let tail = match tokens.iter().rev().map(|t| t.kind).find(|k| *k != TokenKind::Eof) {
        Some(TokenKind::Dot) => " _p",
        Some(k) if token_class(k) == Some(Class::Operator) && k != TokenKind::Question => " 0",
        Some(TokenKind::Comma | TokenKind::Colon | TokenKind::ColonColon) => " _p",
        _ => "",
    };
    for t in tokens {
        if let Some(c) = token_class(t.kind) {
            let (lo, hi) = (t.span.lo as usize, (t.span.hi as usize).min(text.len()));
            if lo < hi {
                by_byte[lo..hi].fill(c);
            }
        }
    }
    // identifiers and comments: what the analysis knows
    let mut doc = String::new();
    for c in cells {
        doc.push_str(c);
        doc.push('\n');
    }
    let base = doc.len();
    doc.push_str(text);
    let end = doc.len();
    // an unfinished input: complete it, so the parser has a whole definition to work with
    doc.push_str(tail);
    // a statement cut off inside a block (or at the top level) still lacks its `;`
    let (open, inside) = scan(&doc);
    let last = doc.trim_end().chars().last();
    if !inside && open.last().is_none_or(|o| o.ch == b'{') && !matches!(last, Some(';' | '}' | '{') | None) {
        doc.push(';');
    }
    doc.push_str(&closing_delimiters(&doc, doc.len()));
    let analysis = Analysis::run("<repl>", &doc, vfs);
    for p in analysis.pieces() {
        if p.lo < base || p.hi > end {
            continue;
        }
        let class = match p.ty {
            T_COMMENT => Class::Comment,
            T_KEYWORD => continue,
            T_FUNCTION | T_METHOD => Class::Function,
            T_TYPE | T_STRUCT | T_ENUM | T_INTERFACE | T_TYPE_PARAM | T_NAMESPACE => Class::Type,
            T_ENUM_MEMBER => Class::Variant,
            T_PARAMETER => Class::Param,
            T_VARIABLE if p.modifiers & M_LOCAL != 0 => Class::Local,
            T_VARIABLE => Class::Global,
            _ => continue, // literals are already colored, fields stay plain
        };
        by_byte[p.lo - base..p.hi - base].fill(class);
    }
    text.char_indices().map(|(i, _)| by_byte[i]).collect()
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
    use crate::vfs::MemVfs;

    fn classes_after(cells: &[&str], text: &str) -> Vec<(String, Class)> {
        let cells: Vec<String> = cells.iter().map(|s| s.to_string()).collect();
        let cl = highlight(&cells, text, &MemVfs::new());
        assert_eq!(cl.len(), text.chars().count());
        // runs of one class, split at blanks, for readable assertions
        let mut out: Vec<(String, Class)> = Vec::new();
        let mut open = false;
        for (ch, c) in text.chars().zip(cl) {
            if ch.is_whitespace() {
                open = false;
                continue;
            }
            match out.last_mut() {
                Some((s, last)) if open && *last == c => s.push(ch),
                _ => out.push((ch.to_string(), c)),
            }
            open = true;
        }
        out
    }

    fn classes(text: &str) -> Vec<(String, Class)> {
        classes_after(&[], text)
    }

    fn of(text: &str, word: &str) -> Class {
        classes(text).into_iter().find(|(s, _)| s == word).unwrap_or_else(|| panic!("no run {word:?} in {text:?}")).1
    }

    #[test]
    fn the_screenshot_example_gets_its_colors() {
        let cells = ["enum Data { RGB(u8, u8, u8), Pos(u8, u8, u8) }"];
        let src = "fn input_data() -> Data {\n    let parts = read_line().expect(\"x\").split(\" \");\n    if parts.len() != 4 { panic(\"bad\"); }\n    let v = parts.skip(1).map(|s| => s.parse_u64() as u8);\n    match parts[0] {\n        \"rgb\" => Data::RGB(v[0], v[1], v[2]),\n        other => panic(f\"Invalid type {other}\"),\n    }\n}";
        let got = classes_after(&cells, src);
        let class_of = |w: &str, nth: usize| got.iter().filter(|(s, _)| s == w).nth(nth).map(|(_, c)| *c);
        assert_eq!(class_of("fn", 0), Some(Class::Keyword));
        assert_eq!(class_of("input_data", 0), Some(Class::Function));
        assert_eq!(class_of("->", 0), Some(Class::Operator));
        assert_eq!(class_of("=>", 0), Some(Class::Operator));
        assert_eq!(class_of("Data", 0), Some(Class::Type));
        assert_eq!(class_of("parts", 0), Some(Class::Local));
        assert_eq!(class_of("read_line", 0), Some(Class::Function));
        assert_eq!(class_of("split", 0), Some(Class::Function));
        assert_eq!(class_of("!=", 0), Some(Class::Operator));
        assert_eq!(class_of("4", 0), Some(Class::Number));
        assert_eq!(class_of("u8", 0), Some(Class::Type));
        assert_eq!(class_of("RGB", 0), Some(Class::Variant));
        assert_eq!(class_of("s", 0), Some(Class::Param), "closure parameter");
        assert_eq!(class_of("s", 1), Some(Class::Param), "and its use");
        assert_eq!(class_of("v", 0), Some(Class::Local));
        assert_eq!(class_of("v", 1), Some(Class::Local));
        assert_eq!(class_of("other", 0), Some(Class::Local), "match binding");
        assert_eq!(class_of("\"rgb\"", 0), Some(Class::Str));
        assert_eq!(class_of("{", 0), Some(Class::Bracket));
        assert_eq!(class_of(",", 0), Some(Class::Punct));
        assert_eq!(class_of(".", 0), Some(Class::Punct));
    }

    #[test]
    fn parameters_locals_and_globals_have_their_own_colors() {
        let cells = ["let total = 5;", "fn helper() -> i64 { 1 }"];
        let got = classes_after(&cells, "fn add(a: i64, b: i64) -> i64 { let c = a + b; c + total + helper() }");
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
    fn what_a_name_is_depends_on_what_it_refers_to_not_on_its_look() {
        // the same word is a parameter in one function and a global elsewhere; a function called without
        // parentheses-looking syntax is still a function; an unknown name is plain
        let cells = ["let count = 3;", "fn twice(n: i64) -> i64 { n * 2 }"];
        let got = classes_after(&cells, "count + twice(count) + nothing");
        assert_eq!(got[0], ("count".into(), Class::Global));
        assert_eq!(got[2], ("twice".into(), Class::Function));
        assert_eq!(got.last().unwrap(), &("nothing".into(), Class::Plain));
        let got = classes_after(&["let n = 1;"], "fn f(n: i64) -> i64 { n }");
        let ns: Vec<Class> = got.iter().filter(|(s, _)| s == "n").map(|(_, c)| *c).collect();
        assert_eq!(ns, [Class::Param, Class::Param], "the parameter shadows the global");
    }

    #[test]
    fn methods_variants_and_types() {
        let cells = ["let xs = [3, 1, 2];", "enum Shape { Dot, Circle(u8) }", "struct P { x: i64 }"];
        assert_eq!(classes_after(&cells, "xs.len()").iter().find(|(s, _)| s == "len").unwrap().1, Class::Function);
        let got = classes_after(&cells, "Shape::Circle(3)");
        assert_eq!(got[0], ("Shape".into(), Class::Type));
        assert_eq!(got[2], ("Circle".into(), Class::Variant));
        assert_eq!(classes_after(&cells, "P { x: 1 }")[0].1, Class::Type);
        assert_eq!(
            classes_after(&cells, "let p: Option<i32> = Option::None;").iter().find(|(s, _)| s == "Option").unwrap().1,
            Class::Type
        );
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
        assert_eq!(of("'a'", "'a'"), Class::Str);
        assert_eq!(of("x // note", "//"), Class::Comment);
        assert_eq!(of("/* a */ x", "/*"), Class::Comment);
        assert_eq!(of("true && false", "true"), Class::Keyword);
    }

    #[test]
    fn unfinished_input_does_not_confuse_it() {
        assert_eq!(classes("\"abc")[0].1, Class::Str);
        assert_eq!(classes("/* abc")[0].1, Class::Comment);
        let cl = highlight(&[], "f\"x {a", &MemVfs::new());
        assert_eq!(cl.len(), 6);
        assert_eq!(cl[0], Class::Str);
        // one class per character, whatever the input
        for text in ["", "'", "f\"{", "f\"{a:", "1e", "a::", "|", "=>", "\"\\", "fn (", "let = ;", "é + ü", "match {"]
        {
            assert_eq!(highlight(&[], text, &MemVfs::new()).len(), text.chars().count(), "{text:?}");
        }
        // a half-typed definition still colors what is known
        let got = classes("fn f(a: i64) -> i64 { let b = a +");
        assert_eq!(got.iter().find(|(s, _)| s == "b").unwrap().1, Class::Local);
        assert_eq!(got.iter().find(|(s, _)| s == "i64").unwrap().1, Class::Type);
    }

    #[test]
    fn fstring_interpolations_are_code() {
        let src = "let n = 1; f\"sum {n + 1:>4} done\"";
        let cl = highlight(&[], src, &MemVfs::new());
        let at = |s: &str| src.chars().position(|c| c.to_string() == s).unwrap();
        let q = src.chars().position(|c| c == 'f').unwrap();
        assert_eq!(cl[q], Class::Str);
        let plus = src.chars().skip(q).position(|c| c == '+').unwrap() + q;
        assert_eq!(cl[plus], Class::Operator);
        assert_eq!(cl[plus + 2], Class::Number);
        // the interpolated name is the local variable it refers to... here a global of the input
        let n_in = src.chars().skip(q).position(|c| c == 'n').unwrap() + q;
        assert_eq!(cl[n_in], Class::Global);
        assert_eq!(cl[src.chars().skip(q).position(|c| c == '{').unwrap() + q], Class::Bracket);
        assert_eq!(cl[src.len() - 3], Class::Str);
        let _ = at;
    }

    #[test]
    fn rendering_resets_at_line_breaks_and_can_be_plain() {
        let text = "let a = \"x\ny\";";
        let cl = highlight(&[], text, &MemVfs::new());
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
