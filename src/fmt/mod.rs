//! The source formatter behind `neon fmt` and the language server's formatting request.
//!
//! The formatter is conservative: it never moves, adds or removes a token, and it keeps the
//! author's line breaks. What it normalizes is indentation (driven by brackets, with one extra
//! level for continuation lines), the spacing between tokens, runs of blank lines, trailing
//! whitespace and the final newline. Operators are classified with the parser's tree, so `a - b`
//! and `-b`, or `a < b` and `Vec<T>`, are told apart. Comments are kept verbatim.
//!
//! Before returning, the output is lexed again and compared with the input token by token (and
//! comment by comment); a mismatch is reported as an error instead of being written.

use std::fmt;

use crate::diag::{DiagBag, FileId};
use crate::syntax::{parse_source, tokenize, BinOp, Interner, Keyword, LitPool, NodeKind, Token, TokenKind, UnOp};

/// Spaces per indentation level.
const INDENT: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatError(pub String);

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FormatError {}

/// Formats a whole module. Source with syntax errors is rejected (it has no reliable structure).
pub fn format_source(src: &str) -> Result<String, FormatError> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src).replace("\r\n", "\n");
    let out = format_normalized(&src)?;
    verify(&src, &out)?;
    Ok(out)
}

fn lex(src: &str) -> Result<Vec<Token>, FormatError> {
    let mut interner = Interner::new();
    let mut pool = LitPool::new();
    let mut diags = DiagBag::new();
    let mut tokens = tokenize(FileId(0), src, &mut interner, &mut pool, &mut diags);
    if let Some(d) = diags.iter().next().filter(|_| diags.has_errors()) {
        return Err(FormatError(format!("cannot format: {}", d.message)));
    }
    tokens.pop(); // Eof
    Ok(tokens)
}

fn text<'a>(src: &'a str, t: &Token) -> &'a str {
    &src[t.span.lo as usize..t.span.hi as usize]
}

/// A token, a comment, or the end of a line.
enum Piece {
    Tok(usize),
    Line(String),
    Block(String),
}

struct Line {
    pieces: Vec<Piece>,
    blank_before: bool,
}

/// Splits `gap` (whitespace and comments between two tokens) and appends to `lines`.
struct Builder {
    lines: Vec<Line>,
    cur: Vec<Piece>,
    blank_before: bool,
    newlines: usize,
}

impl Builder {
    fn push(&mut self, piece: Piece) {
        if self.cur.is_empty() {
            self.blank_before = !self.lines.is_empty() && self.newlines >= 2;
        }
        self.newlines = 0;
        self.cur.push(piece);
    }

    fn newline(&mut self) {
        if !self.cur.is_empty() {
            let pieces = std::mem::take(&mut self.cur);
            self.lines.push(Line { pieces, blank_before: self.blank_before });
        }
        self.newlines += 1;
    }

    fn gap(&mut self, gap: &str) {
        let b = gap.as_bytes();
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'\n' => {
                    self.newline();
                    i += 1;
                }
                b'/' if b.get(i + 1) == Some(&b'/') => {
                    let end = gap[i..].find('\n').map_or(gap.len(), |n| i + n);
                    self.push(Piece::Line(gap[i..end].trim_end().to_string()));
                    i = end;
                }
                b'/' if b.get(i + 1) == Some(&b'*') => {
                    let mut depth = 0usize;
                    let mut j = i;
                    while j < b.len() {
                        if b[j] == b'/' && b.get(j + 1) == Some(&b'*') {
                            depth += 1;
                            j += 2;
                        } else if b[j] == b'*' && b.get(j + 1) == Some(&b'/') {
                            depth -= 1;
                            j += 2;
                            if depth == 0 {
                                break;
                            }
                        } else {
                            j += 1;
                        }
                    }
                    self.push(Piece::Block(gap[i..j].to_string()));
                    i = j;
                }
                _ => i += 1, // spaces, tabs, stray carriage returns
            }
        }
    }
}

fn is_closer(k: TokenKind) -> bool {
    matches!(k, TokenKind::RParen | TokenKind::RBrace | TokenKind::RBracket)
}

fn is_opener(k: TokenKind) -> bool {
    matches!(k, TokenKind::LParen | TokenKind::LBrace | TokenKind::LBracket)
}

fn bin_token(op: BinOp) -> TokenKind {
    use TokenKind as T;
    match op {
        BinOp::Add => T::Plus,
        BinOp::Sub => T::Minus,
        BinOp::Mul => T::Star,
        BinOp::Div => T::Slash,
        BinOp::Rem => T::Percent,
        BinOp::BitAnd => T::Amp,
        BinOp::BitOr => T::Pipe,
        BinOp::BitXor => T::Caret,
        BinOp::Shl => T::Shl,
        BinOp::Shr => T::Shr,
        BinOp::And => T::AmpAmp,
        BinOp::Or => T::PipePipe,
        BinOp::Eq => T::EqEq,
        BinOp::Ne => T::BangEq,
        BinOp::Lt => T::Lt,
        BinOp::Le => T::Le,
        BinOp::Gt => T::Gt,
        BinOp::Ge => T::Ge,
    }
}

/// Operators that are always binary or always separated by spaces.
fn is_spaced_kind(k: TokenKind) -> bool {
    use TokenKind as T;
    matches!(
        k,
        T::Arrow
            | T::FatArrow
            | T::Eq
            | T::EqEq
            | T::BangEq
            | T::Le
            | T::Ge
            | T::PlusEq
            | T::MinusEq
            | T::StarEq
            | T::SlashEq
            | T::PercentEq
            | T::AmpEq
            | T::PipeEq
            | T::CaretEq
            | T::ShlEq
            | T::ShrEq
    )
}

fn is_word(k: TokenKind) -> bool {
    use TokenKind as T;
    matches!(k, T::Ident(_) | T::Kw(_) | T::Int(_) | T::Float(_) | T::Str(_) | T::Char(_) | T::Underscore)
}

/// Keywords that behave like values (`self`, `true`, `i32`), unlike `if`, `return` or `as`.
fn is_value_kw(k: Keyword) -> bool {
    k.is_primitive_type() || matches!(k, Keyword::SelfValue | Keyword::SelfType | Keyword::True | Keyword::False)
}

struct Ctx<'a> {
    toks: &'a [Token],
    binary: Vec<bool>,
    unary: Vec<bool>,
}

impl Ctx<'_> {
    /// Whether a space separates the adjacent tokens `a` and `b` (indices) on one line.
    fn space(&self, a: usize, b: usize) -> bool {
        use TokenKind as T;
        let (ka, kb) = (self.toks[a].kind, self.toks[b].kind);
        let kept = self.toks[a].span.hi != self.toks[b].span.lo;
        // never a space
        if matches!(kb, T::Comma | T::Semi | T::Colon | T::Question | T::Dot | T::ColonColon | T::RParen | T::RBracket)
            || matches!(ka, T::Dot | T::ColonColon | T::LParen | T::LBracket)
            || self.unary[a]
        {
            return false;
        }
        // always a space
        if matches!(ka, T::Colon) || is_spaced_kind(ka) || is_spaced_kind(kb) || self.binary[a] || self.binary[b] {
            return true;
        }
        match (ka, kb) {
            (T::Comma, T::RBrace) => kept,
            (T::Comma, _) => true,
            (T::Semi, T::RBrace) => kept,
            (T::Semi, _) => true,
            (T::LBrace, _) | (_, T::RBrace) => kept,
            (T::RBrace, T::Kw(Keyword::Else | Keyword::Elif)) => true,
            (_, T::LBrace) => !matches!(ka, T::LParen | T::LBracket),
            (T::Kw(Keyword::Fn), T::LParen) | (T::Kw(Keyword::Impl), T::Lt) => false,
            (_, T::LParen | T::LBracket) => match ka {
                T::Ident(_) | T::RParen | T::RBracket | T::Question | T::Gt => false,
                T::Kw(k) if is_value_kw(k) => false,
                T::Kw(_) => true,
                _ => kept,
            },
            (T::Kw(k), _) if !is_value_kw(k) => true,
            _ if is_word(ka) && is_word(kb) => true,
            _ => kept,
        }
    }
}

fn format_normalized(src: &str) -> Result<String, FormatError> {
    // syntax errors make the structure unreliable
    let mut interner = Interner::new();
    let mut diags = DiagBag::new();
    let ast = parse_source(FileId(0), src, &mut interner, &mut diags);
    if let Some(d) = diags.iter().next().filter(|_| diags.has_errors()) {
        return Err(FormatError(format!("cannot format: {}", d.message)));
    }
    let toks = lex(src)?;

    // classify operators with the tree
    let first_at = |off: u32| toks.partition_point(|t| t.span.lo < off);
    let mut binary = vec![false; toks.len()];
    let mut unary = vec![false; toks.len()];
    for (_, node) in ast.iter() {
        match node.kind {
            NodeKind::Binary { op, lhs, .. } => {
                let mut i = first_at(ast.span(lhs).hi);
                while i < toks.len() && toks[i].kind == TokenKind::RParen {
                    i += 1;
                }
                if i < toks.len() && toks[i].kind == bin_token(op) {
                    binary[i] = true;
                }
            }
            NodeKind::Unary { op, .. } => {
                let i = first_at(node.span.lo);
                let want = match op {
                    UnOp::Neg => TokenKind::Minus,
                    UnOp::Not => TokenKind::Bang,
                    UnOp::BitNot => TokenKind::Tilde,
                };
                if i < toks.len() && toks[i].kind == want {
                    unary[i] = true;
                }
            }
            _ => {}
        }
    }
    let cx = Ctx { toks: &toks, binary, unary };

    // lines of pieces
    let mut b = Builder { lines: Vec::new(), cur: Vec::new(), blank_before: false, newlines: 0 };
    let mut prev_hi = 0usize;
    for (i, t) in toks.iter().enumerate() {
        b.gap(&src[prev_hi..t.span.lo as usize]);
        b.push(Piece::Tok(i));
        prev_hi = t.span.hi as usize;
    }
    b.gap(&src[prev_hi..]);
    b.newline();
    let lines = b.lines;

    // indentation
    struct Open {
        line_indent: usize,
    }
    let mut stack: Vec<Open> = Vec::new();
    let mut out = String::new();
    let mut prev_code_last: Option<usize> = None;
    let mut prev_emitted_opener = false;
    for (n, line) in lines.iter().enumerate() {
        let toks_in_line: Vec<usize> =
            line.pieces.iter().filter_map(|p| if let Piece::Tok(i) = p { Some(*i) } else { None }).collect();
        let first = toks_in_line.first().copied();
        let leading_closers = toks_in_line.iter().take_while(|&&i| is_closer(toks[i].kind)).count();
        // a line that only has comments sits at the inner level
        let cont = match first {
            None => false,
            Some(f) => {
                leading_closers == 0
                    && (toks[f].kind == TokenKind::Dot
                        || cx.binary[f]
                        || prev_code_last.is_some_and(|p| {
                            let k = toks[p].kind;
                            let assigns = is_spaced_kind(k) && !matches!(k, TokenKind::EqEq | TokenKind::BangEq);
                            cx.binary[p] || assigns
                        }))
            }
        };
        let indent = if leading_closers > 0 {
            stack.last().map_or(0, |e| e.line_indent)
        } else {
            stack.last().map_or(0, |e| e.line_indent + 1) + usize::from(cont)
        };

        // text of the line
        let mut text = String::new();
        let mut prev_tok: Option<usize> = None;
        let mut first_piece = true;
        for p in &line.pieces {
            match p {
                Piece::Tok(i) => {
                    if let Some(a) = prev_tok {
                        if cx.space(a, *i) {
                            text.push(' ');
                        }
                    } else if !first_piece {
                        text.push(' '); // after a block comment
                    }
                    text.push_str(self::text(src, &toks[*i]));
                    prev_tok = Some(*i);
                }
                Piece::Block(c) | Piece::Line(c) => {
                    if !first_piece {
                        text.push(' ');
                    }
                    text.push_str(c);
                    prev_tok = None;
                }
            }
            first_piece = false;
        }

        // blank lines: at most one; none after an opener, before a closer, or at the start
        let starts_with_closer = leading_closers > 0;
        if line.blank_before && n > 0 && !prev_emitted_opener && !starts_with_closer {
            out.push('\n');
        }
        for _ in 0..indent * INDENT {
            out.push(' ');
        }
        out.push_str(&text);
        out.push('\n');

        // update bracket state with this line's tokens
        for &i in &toks_in_line {
            let k = toks[i].kind;
            if is_opener(k) {
                stack.push(Open { line_indent: indent });
            } else if is_closer(k) {
                stack.pop();
            }
        }
        if let Some(&last) = toks_in_line.last() {
            prev_code_last = Some(last);
        }
        prev_emitted_opener = toks_in_line.last().is_some_and(|&i| is_opener(toks[i].kind));
    }
    Ok(out)
}

/// Comments of `src`, in order, with trailing whitespace of line comments removed.
fn comments(src: &str, toks: &[Token]) -> Vec<String> {
    let mut b = Builder { lines: Vec::new(), cur: Vec::new(), blank_before: false, newlines: 0 };
    let mut prev_hi = 0usize;
    for t in toks {
        b.gap(&src[prev_hi..t.span.lo as usize]);
        prev_hi = t.span.hi as usize;
    }
    b.gap(&src[prev_hi..]);
    b.newline();
    b.lines
        .into_iter()
        .flat_map(|l| l.pieces)
        .filter_map(|p| match p {
            Piece::Line(c) | Piece::Block(c) => Some(c),
            Piece::Tok(_) => None,
        })
        .collect()
}

/// The formatted text must contain exactly the same tokens and comments.
fn verify(src: &str, out: &str) -> Result<(), FormatError> {
    let (a, b) = (lex(src)?, lex(out)?);
    let same_tokens =
        a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| x.kind == y.kind && text(src, x) == text(out, y));
    if !same_tokens || comments(src, &a) != comments(out, &b) {
        return Err(FormatError("internal error: the formatted text differs from the input".to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
