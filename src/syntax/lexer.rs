//! Streaming tokenizer.
//!
//! The lexer never fails: malformed input is reported to the [`DiagBag`] and
//! turned into the closest sensible token (or [`TokenKind::Error`]), so the
//! parser always sees a complete token stream ending in `Eof`.
//!
//! Diagnostic codes: E0001 unknown character, E0002 unterminated string,
//! E0003 unterminated block comment, E0004 bad escape, E0005 bad number,
//! E0006 bad character literal.

use super::interner::Interner;
use super::literal::{FloatLit, FloatSuffix, IntLit, IntSuffix, LitPool};
use super::token::{Keyword, Token, TokenKind};
use crate::diag::{DiagBag, Diagnostic, FileId, Span};

pub struct Lexer<'a> {
    file: FileId,
    src: &'a str,
    pos: usize,
    interner: &'a mut Interner,
    pool: &'a mut LitPool,
    diags: &'a mut DiagBag,
    scratch: String,
    finished: bool,
}

impl std::fmt::Debug for Lexer<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lexer").field("file", &self.file).field("pos", &self.pos).finish_non_exhaustive()
    }
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

fn is_ident_continue(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// Characters that begin some token; anything else is reported as unknown.
fn begins_token(c: char) -> bool {
    c.is_whitespace()
        || is_ident_start(c)
        || c.is_ascii_digit()
        || matches!(c, '"' | '\'' | '(' | ')' | '{' | '}' | '[' | ']' | ',' | ';' | ':' | '.')
        || matches!(c, '+' | '-' | '*' | '/' | '%' | '&' | '|' | '^' | '~' | '<' | '>' | '!' | '=')
}

impl<'a> Lexer<'a> {
    /// `src` must be the text registered under `file` in the `SourceMap`
    /// (and therefore at most `u32::MAX` bytes long).
    pub fn new(
        file: FileId,
        src: &'a str,
        interner: &'a mut Interner,
        pool: &'a mut LitPool,
        diags: &'a mut DiagBag,
    ) -> Lexer<'a> {
        debug_assert!(src.len() <= u32::MAX as usize);
        let mut pos = 0;
        if src.starts_with('\u{feff}') {
            pos = '\u{feff}'.len_utf8();
        }
        // A `#!` line lets scripts be executable.
        if src[pos..].starts_with("#!") {
            pos += src[pos..].find('\n').unwrap_or(src.len() - pos);
        }
        Lexer { file, src, pos, interner, pool, diags, scratch: String::new(), finished: false }
    }

    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek_nth(&self, n: usize) -> Option<char> {
        self.src[self.pos..].chars().nth(n)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.pos += expected.len_utf8();
            true
        } else {
            false
        }
    }

    fn span(&self, lo: usize) -> Span {
        Span::new(self.file, lo as u32, self.pos as u32)
    }

    fn error(&mut self, lo: usize, code: &'static str, message: &str, label: &str) {
        let span = self.span(lo);
        self.diags.report(Diagnostic::error(message).with_code(code).with_label(span, label));
    }

    /// Returns the next token; yields `Eof` forever once the input is exhausted.
    pub fn next_token(&mut self) -> Token {
        self.skip_trivia();
        let lo = self.pos;
        let Some(c) = self.bump() else {
            return Token { kind: TokenKind::Eof, span: self.span(lo) };
        };
        use TokenKind::*;
        let kind = match c {
            '(' => LParen,
            ')' => RParen,
            '{' => LBrace,
            '}' => RBrace,
            '[' => LBracket,
            ']' => RBracket,
            ',' => Comma,
            ';' => Semi,
            '~' => Tilde,
            '?' => Question,
            ':' => self.pick(':', ColonColon, Colon),
            '.' => {
                if self.eat('.') {
                    self.pick('=', DotDotEq, DotDot)
                } else {
                    Dot
                }
            }
            '+' => self.pick('=', PlusEq, Plus),
            '*' => self.pick('=', StarEq, Star),
            '/' => self.pick('=', SlashEq, Slash),
            '%' => self.pick('=', PercentEq, Percent),
            '^' => self.pick('=', CaretEq, Caret),
            '!' => self.pick('=', BangEq, Bang),
            '-' => {
                if self.eat('>') {
                    Arrow
                } else {
                    self.pick('=', MinusEq, Minus)
                }
            }
            '=' => {
                if self.eat('>') {
                    FatArrow
                } else {
                    self.pick('=', EqEq, Eq)
                }
            }
            '&' => {
                if self.eat('&') {
                    AmpAmp
                } else {
                    self.pick('=', AmpEq, Amp)
                }
            }
            '|' => {
                if self.eat('|') {
                    PipePipe
                } else {
                    self.pick('=', PipeEq, Pipe)
                }
            }
            '<' => {
                if self.eat('<') {
                    self.pick('=', ShlEq, Shl)
                } else {
                    self.pick('=', Le, Lt)
                }
            }
            '>' => {
                if self.eat('>') {
                    self.pick('=', ShrEq, Shr)
                } else {
                    self.pick('=', Ge, Gt)
                }
            }
            '"' => self.string(lo),
            '\'' => self.char_literal(lo),
            c if c.is_ascii_digit() => self.number(lo, c),
            c if is_ident_start(c) => self.ident(lo),
            c => self.unknown(lo, c),
        };
        Token { kind, span: self.span(lo) }
    }

    /// `then` if the next char is `expected` (consuming it), else `otherwise`.
    fn pick(&mut self, expected: char, then: TokenKind, otherwise: TokenKind) -> TokenKind {
        if self.eat(expected) {
            then
        } else {
            otherwise
        }
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.pos += c.len_utf8();
                }
                Some('/') if self.peek_nth(1) == Some('/') => {
                    self.pos += self.src[self.pos..].find('\n').unwrap_or(self.src.len() - self.pos);
                }
                Some('/') if self.peek_nth(1) == Some('*') => self.block_comment(),
                _ => return,
            }
        }
    }

    /// Skips a (nestable) block comment starting at `/*`.
    fn block_comment(&mut self) {
        let lo = self.pos;
        self.pos += 2;
        let mut depth = 1u32;
        while depth > 0 {
            match self.bump() {
                None => {
                    self.error(lo, "E0003", "unterminated block comment", "comment starts here");
                    return;
                }
                Some('/') if self.peek() == Some('*') => {
                    self.pos += 1;
                    depth += 1;
                }
                Some('*') if self.peek() == Some('/') => {
                    self.pos += 1;
                    depth -= 1;
                }
                Some(_) => {}
            }
        }
    }

    fn ident(&mut self, lo: usize) -> TokenKind {
        while matches!(self.peek(), Some(c) if is_ident_continue(c)) {
            self.bump();
        }
        let text = &self.src[lo..self.pos];
        if text == "_" {
            return TokenKind::Underscore;
        }
        let sym = self.interner.intern(text);
        match Keyword::from_symbol(sym) {
            Some(kw) => TokenKind::Kw(kw),
            None => TokenKind::Ident(sym),
        }
    }

    /// Reports a run of unrecognised characters as one error.
    fn unknown(&mut self, lo: usize, first: char) -> TokenKind {
        while matches!(self.peek(), Some(c) if !begins_token(c)) {
            self.bump();
        }
        let shown: String = self.src[lo..self.pos].chars().take(8).collect();
        let msg = if self.pos - lo == first.len_utf8() {
            format!("unknown character `{}`", first.escape_debug())
        } else {
            format!("unknown characters `{}`", shown.escape_debug())
        };
        self.error(lo, "E0001", &msg, "not valid in Neon source");
        TokenKind::Error
    }

    // ---- numbers ---------------------------------------------------------

    fn eat_digits(&mut self, radix: u32) {
        while matches!(self.peek(), Some(c) if c == '_' || c.is_digit(radix)) {
            self.bump();
        }
    }

    /// Consumes an identifier-shaped literal suffix and returns its text.
    fn take_suffix(&mut self) -> Option<&'a str> {
        if !matches!(self.peek(), Some(c) if is_ident_start(c)) {
            return None;
        }
        let start = self.pos;
        while matches!(self.peek(), Some(c) if is_ident_continue(c)) {
            self.bump();
        }
        Some(&self.src[start..self.pos])
    }

    fn bad_number(&mut self, lo: usize, message: &str, label: &str) {
        self.error(lo, "E0005", message, label);
    }

    fn int_token(&mut self, value: u64, suffix: Option<IntSuffix>) -> TokenKind {
        TokenKind::Int(self.pool.add_int(IntLit { value, suffix }))
    }

    fn float_token(&mut self, value: f64, suffix: Option<FloatSuffix>) -> TokenKind {
        TokenKind::Float(self.pool.add_float(FloatLit { value, suffix }))
    }

    fn number(&mut self, lo: usize, first: char) -> TokenKind {
        if first == '0' {
            let radix = match self.peek() {
                Some('x') => 16,
                Some('b') => 2,
                Some('o') => 8,
                _ => 0,
            };
            if radix != 0 {
                self.bump();
                return self.radix_int(lo, radix);
            }
        }

        self.eat_digits(10);
        let mut is_float = false;
        // `1.5` is a float; `1..5` and `1.max(2)` are not.
        if self.peek() == Some('.') && matches!(self.peek_nth(1), Some(d) if d.is_ascii_digit()) {
            self.bump();
            self.eat_digits(10);
            is_float = true;
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            let has_exponent = match self.peek_nth(1) {
                Some(d) if d.is_ascii_digit() => true,
                Some('+' | '-') => matches!(self.peek_nth(2), Some(d) if d.is_ascii_digit()),
                _ => false,
            };
            if has_exponent {
                self.bump();
                if matches!(self.peek(), Some('+' | '-')) {
                    self.bump();
                }
                self.eat_digits(10);
                is_float = true;
            }
        }
        let body: String = self.src[lo..self.pos].chars().filter(|&c| c != '_').collect();

        let mut int_suffix = None;
        let mut float_suffix = None;
        if let Some(text) = self.take_suffix() {
            if let Some(s) = IntSuffix::parse(text) {
                int_suffix = Some(s);
            } else if let Some(s) = FloatSuffix::parse(text) {
                float_suffix = Some(s);
            } else {
                self.bad_number(lo, &format!("invalid suffix `{text}` for number literal"), "unknown suffix");
            }
        }

        if is_float || float_suffix.is_some() {
            if int_suffix.is_some() {
                self.bad_number(lo, "float literal with an integer suffix", "use `f32` or `f64`");
            }
            let value = match body.parse::<f64>() {
                Ok(v) if v.is_finite() => v,
                _ => {
                    self.bad_number(lo, "float literal out of range", "does not fit in `f64`");
                    0.0
                }
            };
            self.float_token(value, float_suffix)
        } else {
            let value = body.parse::<u64>().unwrap_or_else(|_| {
                self.bad_number(lo, "integer literal is too large", "exceeds `u64::MAX`");
                0
            });
            self.int_token(value, int_suffix)
        }
    }

    /// After a `0x` / `0b` / `0o` prefix.
    fn radix_int(&mut self, lo: usize, radix: u32) -> TokenKind {
        let name = match radix {
            2 => "binary",
            8 => "octal",
            _ => "hexadecimal",
        };
        let digits_start = self.pos;
        self.eat_digits(radix);
        let digits: String = self.src[digits_start..self.pos].chars().filter(|&c| c != '_').collect();

        if let Some(bad) = self.peek().filter(char::is_ascii_digit) {
            while matches!(self.peek(), Some(c) if c.is_alphanumeric() || c == '_') {
                self.bump();
            }
            self.bad_number(
                lo,
                &format!("invalid digit `{bad}` in {name} literal"),
                "digit out of range for this base",
            );
            return self.int_token(0, None);
        }
        let mut suffix = None;
        if let Some(text) = self.take_suffix() {
            match IntSuffix::parse(text) {
                Some(s) => suffix = Some(s),
                None => self.bad_number(lo, &format!("invalid suffix `{text}` for {name} literal"), "unknown suffix"),
            }
        }
        if digits.is_empty() {
            self.bad_number(lo, &format!("missing digits after {name} prefix"), "");
            return self.int_token(0, suffix);
        }
        let value = u64::from_str_radix(&digits, radix).unwrap_or_else(|_| {
            self.bad_number(lo, "integer literal is too large", "exceeds `u64::MAX`");
            0
        });
        self.int_token(value, suffix)
    }

    // ---- strings and characters ------------------------------------------

    /// Decodes the escape after a consumed `\` (whose offset is `esc_lo`).
    /// Reports and returns `None` for a malformed escape.
    fn escape(&mut self, esc_lo: usize) -> Option<char> {
        let Some(c) = self.bump() else {
            self.error(esc_lo, "E0004", "unterminated escape sequence", "");
            return None;
        };
        let decoded = match c {
            'n' => Some('\n'),
            't' => Some('\t'),
            'r' => Some('\r'),
            '0' => Some('\0'),
            '\\' => Some('\\'),
            '"' => Some('"'),
            '\'' => Some('\''),
            'x' => self.hex_escape(),
            'u' => self.unicode_escape(),
            _ => None,
        };
        if decoded.is_none() {
            let what = if matches!(c, 'x' | 'u') {
                format!("invalid `\\{c}` escape")
            } else {
                format!("unknown escape `\\{}`", c.escape_debug())
            };
            self.error(esc_lo, "E0004", &what, "");
        }
        decoded
    }

    /// `\xNN`, limited to ASCII.
    fn hex_escape(&mut self) -> Option<char> {
        let mut value = 0u32;
        for _ in 0..2 {
            let d = self.peek()?.to_digit(16)?;
            self.bump();
            value = value * 16 + d;
        }
        (value <= 0x7f).then(|| char::from(value as u8))
    }

    /// `\u{1F600}`
    fn unicode_escape(&mut self) -> Option<char> {
        if !self.eat('{') {
            return None;
        }
        let mut value = 0u32;
        let mut count = 0;
        while let Some(d) = self.peek().and_then(|c| c.to_digit(16)) {
            self.bump();
            count += 1;
            value = value.saturating_mul(16).saturating_add(d);
        }
        if count == 0 || !self.eat('}') {
            return None;
        }
        char::from_u32(value)
    }

    fn string(&mut self, lo: usize) -> TokenKind {
        let mut buf = std::mem::take(&mut self.scratch);
        buf.clear();
        loop {
            let at = self.pos;
            match self.bump() {
                None => {
                    self.error(lo, "E0002", "unterminated string literal", "string starts here");
                    break;
                }
                Some('"') => break,
                Some('\\') => match self.peek() {
                    // A backslash before a newline joins the lines and drops the indentation.
                    Some('\n' | '\r') => {
                        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
                            self.bump();
                        }
                    }
                    _ => {
                        if let Some(c) = self.escape(at) {
                            buf.push(c);
                        }
                    }
                },
                Some(c) => buf.push(c),
            }
        }
        let id = self.pool.add_str(&buf);
        self.scratch = buf;
        TokenKind::Str(id)
    }

    fn char_literal(&mut self, lo: usize) -> TokenKind {
        let value = match self.peek() {
            None | Some('\n') => {
                self.error(lo, "E0006", "unterminated character literal", "");
                return TokenKind::Error;
            }
            Some('\'') => {
                self.bump();
                self.error(lo, "E0006", "empty character literal", "expected a character between the quotes");
                return TokenKind::Error;
            }
            Some('\\') => {
                let at = self.pos;
                self.bump();
                self.escape(at)
            }
            Some(c) => {
                self.bump();
                Some(c)
            }
        };
        if self.eat('\'') {
            return value.map_or(TokenKind::Error, TokenKind::Char);
        }
        // Recover: skip to a closing quote on this line, if there is one.
        let rest = &self.src[self.pos..];
        let line_end = rest.find('\n').unwrap_or(rest.len());
        match rest[..line_end].find('\'') {
            Some(i) => {
                self.pos += i + 1;
                self.error(lo, "E0006", "character literal may only contain one character", "use a string literal");
            }
            None => self.error(lo, "E0006", "unterminated character literal", "missing closing `'`"),
        }
        TokenKind::Error
    }
}

impl Iterator for Lexer<'_> {
    type Item = Token;

    /// Yields every token except the final `Eof`.
    fn next(&mut self) -> Option<Token> {
        if self.finished {
            return None;
        }
        let tok = self.next_token();
        if tok.kind == TokenKind::Eof {
            self.finished = true;
            return None;
        }
        Some(tok)
    }
}

/// Lexes the whole of `src`. The result always ends with an `Eof` token.
pub fn tokenize(
    file: FileId,
    src: &str,
    interner: &mut Interner,
    pool: &mut LitPool,
    diags: &mut DiagBag,
) -> Vec<Token> {
    let mut lexer = Lexer::new(file, src, interner, pool, diags);
    let mut tokens = Vec::new();
    loop {
        let tok = lexer.next_token();
        tokens.push(tok);
        if tok.kind == TokenKind::Eof {
            return tokens;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::literal::{FloatLit, IntLit};
    use TokenKind::*;

    struct Out {
        tokens: Vec<Token>,
        interner: Interner,
        pool: LitPool,
        diags: DiagBag,
    }

    fn lex(src: &str) -> Out {
        let mut interner = Interner::new();
        let mut pool = LitPool::new();
        let mut diags = DiagBag::new();
        let tokens = tokenize(FileId(0), src, &mut interner, &mut pool, &mut diags);
        Out { tokens, interner, pool, diags }
    }

    impl Out {
        /// Token kinds without the trailing `Eof`.
        fn kinds(&self) -> Vec<TokenKind> {
            let (last, rest) = self.tokens.split_last().unwrap();
            assert_eq!(last.kind, Eof);
            rest.iter().map(|t| t.kind).collect()
        }

        fn ints(&self) -> Vec<IntLit> {
            self.tokens
                .iter()
                .filter_map(|t| if let Int(id) = t.kind { Some(self.pool.int(id)) } else { None })
                .collect()
        }

        fn floats(&self) -> Vec<FloatLit> {
            self.tokens
                .iter()
                .filter_map(|t| if let Float(id) = t.kind { Some(self.pool.float(id)) } else { None })
                .collect()
        }

        fn strs(&self) -> Vec<&str> {
            self.tokens
                .iter()
                .filter_map(|t| if let Str(id) = t.kind { Some(self.pool.str(id)) } else { None })
                .collect()
        }

        fn error_codes(&self) -> Vec<&'static str> {
            self.diags.iter().map(|d| d.code.unwrap()).collect()
        }

        fn error_messages(&self) -> Vec<&str> {
            self.diags.iter().map(|d| d.message.as_str()).collect()
        }
    }

    fn int(value: u64) -> IntLit {
        IntLit { value, suffix: None }
    }

    #[test]
    fn empty_and_whitespace_input() {
        for src in ["", "   \n\t\r\n  "] {
            let out = lex(src);
            assert_eq!(out.tokens.len(), 1);
            assert_eq!(out.tokens[0].kind, Eof);
            assert_eq!(out.tokens[0].span.lo as usize, src.len());
            assert!(out.diags.is_empty());
        }
    }

    #[test]
    fn keywords_identifiers_and_primitives() {
        let out = lex("let var x_1 = foo; fn Self self _ _y i32 string 名前 elif mut");
        let k = out.kinds();
        assert_eq!(k[0], Kw(Keyword::Let));
        assert_eq!(k[1], Kw(Keyword::Var));
        let Ident(x) = k[2] else { panic!() };
        assert_eq!(out.interner.resolve(x), "x_1");
        assert_eq!(k[3], Eq);
        assert!(matches!(k[4], Ident(_)));
        assert_eq!(k[5], Semi);
        assert_eq!(k[6], Kw(Keyword::Fn));
        assert_eq!(k[7], Kw(Keyword::SelfType));
        assert_eq!(k[8], Kw(Keyword::SelfValue));
        assert_eq!(k[9], Underscore);
        let Ident(y) = k[10] else { panic!() };
        assert_eq!(out.interner.resolve(y), "_y");
        assert_eq!(k[11], Kw(Keyword::I32));
        assert_eq!(k[12], Kw(Keyword::String));
        let Ident(n) = k[13] else { panic!() };
        assert_eq!(out.interner.resolve(n), "名前");
        assert_eq!(k[14], Kw(Keyword::Elif));
        // `mut` is no longer reserved
        assert!(matches!(k[15], Ident(_)));
        assert!(out.diags.is_empty());
    }

    #[test]
    fn contextual_words_are_plain_identifiers() {
        let out = lex("from set");
        let k = out.kinds();
        assert_eq!(k[0], Ident(crate::syntax::sym::FROM));
        assert_eq!(k[1], Ident(crate::syntax::sym::SET));
    }

    #[test]
    fn same_identifier_same_symbol() {
        let out = lex("a b a");
        let k = out.kinds();
        assert_eq!(k[0], k[2]);
        assert_ne!(k[0], k[1]);
    }

    #[test]
    fn all_primitive_keywords_lex() {
        let out = lex("i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 bool char string");
        let kws: Vec<_> = out.kinds().into_iter().map(|k| matches!(k, Kw(kw) if kw.is_primitive_type())).collect();
        assert_eq!(kws, vec![true; 13]);
    }

    #[test]
    fn operators_use_longest_match() {
        let out = lex(
            ":: : . .. ..= -> => + - * / % & | ^ ~ ? << >> ! && || == != < <= > >= = += -= *= /= %= &= |= ^= <<= >>=",
        );
        assert_eq!(
            out.kinds(),
            vec![
                ColonColon, Colon, Dot, DotDot, DotDotEq, Arrow, FatArrow, Plus, Minus, Star, Slash, Percent, Amp,
                Pipe, Caret, Tilde, Question, Shl, Shr, Bang, AmpAmp, PipePipe, EqEq, BangEq, Lt, Le, Gt, Ge, Eq,
                PlusEq, MinusEq, StarEq, SlashEq, PercentEq, AmpEq, PipeEq, CaretEq, ShlEq, ShrEq
            ]
        );
        assert!(out.diags.is_empty());
    }

    #[test]
    fn delimiters() {
        let out = lex("( ) { } [ ] , ;");
        assert_eq!(out.kinds(), vec![LParen, RParen, LBrace, RBrace, LBracket, RBracket, Comma, Semi]);
    }

    #[test]
    fn closure_pipes() {
        assert_eq!(lex("|a, b| => a").kinds()[0], Pipe);
        assert_eq!(lex("|| => 1").kinds(), vec![PipePipe, FatArrow, Int(crate::syntax::IntId(0))]);
    }

    #[test]
    fn adjacent_generic_closers_lex_as_shift() {
        // The parser splits `>>` when it closes two generic argument lists.
        assert_eq!(lex("A<B<C>>").kinds()[5], Shr);
        assert_eq!(lex("a >>= 1").kinds()[1], ShrEq);
    }

    #[test]
    fn decimal_integers() {
        let out = lex("0 7 1_000 18446744073709551615");
        assert_eq!(out.ints(), vec![int(0), int(7), int(1000), int(u64::MAX)]);
        assert!(out.diags.is_empty());
    }

    #[test]
    fn radix_integers() {
        let out = lex("0xFF 0xdead_beef 0b1010 0o777 0x0");
        assert_eq!(out.ints(), vec![int(255), int(0xdeadbeef), int(10), int(511), int(0)]);
        assert!(out.diags.is_empty());
    }

    #[test]
    fn integer_suffixes() {
        let out = lex("1i8 2i16 3i32 4i64 5u8 6u16 7u32 8u64 9_u8 0xFFu8 0b1u16");
        let got: Vec<_> = out.ints().iter().map(|l| (l.value, l.suffix.unwrap().as_str())).collect();
        assert_eq!(
            got,
            vec![
                (1, "i8"),
                (2, "i16"),
                (3, "i32"),
                (4, "i64"),
                (5, "u8"),
                (6, "u16"),
                (7, "u32"),
                (8, "u64"),
                (9, "u8"),
                (255, "u8"),
                (1, "u16")
            ]
        );
        assert!(out.diags.is_empty());
    }

    #[test]
    fn float_literals() {
        let out = lex("1.5 0.25 1e3 2.5e-2 1E+2 3.0f32 4f64 1_0.5_0");
        let vals: Vec<_> = out.floats().iter().map(|f| f.value).collect();
        assert_eq!(vals, vec![1.5, 0.25, 1000.0, 0.025, 100.0, 3.0, 4.0, 10.5]);
        let suffixes: Vec<_> = out.floats().iter().map(|f| f.suffix).collect();
        assert_eq!(suffixes[5], Some(FloatSuffix::F32));
        assert_eq!(suffixes[6], Some(FloatSuffix::F64));
        assert_eq!(suffixes[0], None);
        assert!(out.diags.is_empty());
    }

    #[test]
    fn float_boundary_values() {
        let out = lex("3.4028235e38f32 1.7976931348623157e308 5e-324 0.0");
        let vals: Vec<_> = out.floats().iter().map(|f| f.value).collect();
        assert_eq!(vals, vec![3.4028235e38, f64::MAX, 5e-324, 0.0]);
        assert!(out.diags.is_empty());
    }

    #[test]
    fn dot_after_integer_is_not_a_float() {
        let out = lex("1..5 1..=5 1.max 2.0.max");
        let k = out.kinds();
        assert!(matches!(k[0], Int(_)) && k[1] == DotDot && matches!(k[2], Int(_)));
        assert!(matches!(k[3], Int(_)) && k[4] == DotDotEq && matches!(k[5], Int(_)));
        assert!(matches!(k[6], Int(_)) && k[7] == Dot && matches!(k[8], Ident(_)));
        assert!(matches!(k[9], Float(_)) && k[10] == Dot && matches!(k[11], Ident(_)));
        assert!(out.diags.is_empty());
    }

    #[test]
    fn exponent_marker_needs_digits() {
        // `1e` is the integer 1 with the (invalid) suffix `e`.
        let out = lex("1e");
        assert_eq!(out.error_codes(), vec!["E0005"]);
        assert!(out.error_messages()[0].contains("invalid suffix `e`"));
    }

    #[test]
    fn number_errors() {
        let cases: &[(&str, &str)] = &[
            ("18446744073709551616", "too large"),
            ("0xFFFFFFFFFFFFFFFFF", "too large"),
            ("0x", "missing digits after hexadecimal prefix"),
            ("0b", "missing digits after binary prefix"),
            ("0b102", "invalid digit `2` in binary literal"),
            ("0o78", "invalid digit `8` in octal literal"),
            ("12abc", "invalid suffix `abc`"),
            ("1.5u8", "float literal with an integer suffix"),
            ("1e999", "float literal out of range"),
            ("0b1f32", "invalid suffix `f32` for binary literal"),
        ];
        for (src, expect) in cases {
            let out = lex(src);
            assert_eq!(out.diags.error_count(), 1, "{src}: {:?}", out.error_messages());
            assert!(out.error_messages()[0].contains(expect), "{src}: {:?}", out.error_messages());
            assert_eq!(out.error_codes(), vec!["E0005"], "{src}");
            // still exactly one literal token, so the parser can continue
            assert_eq!(out.kinds().len(), 1, "{src}");
        }
    }

    #[test]
    fn simple_strings() {
        let out = lex(r#""hello" "" "wörld 😀""#);
        assert_eq!(out.strs(), vec!["hello", "", "wörld 😀"]);
        assert!(out.diags.is_empty());
    }

    #[test]
    fn string_escapes() {
        let out = lex(r#""a\nb\tc\r\0\\\"\'\x41\u{48}\u{1F600}""#);
        assert_eq!(out.strs(), vec!["a\nb\tc\r\0\\\"'AH😀"]);
        assert!(out.diags.is_empty());
    }

    #[test]
    fn multiline_strings_and_continuations() {
        let out = lex("\"one\ntwo\" \"joined \\\n     together\"");
        assert_eq!(out.strs(), vec!["one\ntwo", "joined together"]);
        assert!(out.diags.is_empty());
    }

    #[test]
    fn string_errors_recover() {
        let out = lex(r#""bad \q escape" 1"#);
        assert_eq!(out.error_codes(), vec!["E0004"]);
        assert_eq!(out.strs(), vec!["bad  escape"]);
        assert!(matches!(out.kinds()[1], Int(_)), "lexing continues after the bad escape");

        let out = lex(r#""\x80""#);
        assert_eq!(out.error_codes(), vec!["E0004"]);
        let out = lex(r#""\u{110000}""#);
        assert_eq!(out.error_codes(), vec!["E0004"]);
        let out = lex(r#""\u{}""#);
        assert_eq!(out.error_codes(), vec!["E0004"]);
        let out = lex(r#""\u41""#);
        assert_eq!(out.error_codes(), vec!["E0004"]);
    }

    #[test]
    fn unterminated_string_runs_to_eof() {
        let out = lex("let s = \"abc\nlet t = 1;");
        assert_eq!(out.error_codes(), vec!["E0002"]);
        let last_str = out.tokens.iter().find(|t| matches!(t.kind, Str(_))).unwrap();
        assert_eq!(last_str.span.lo, 8);
        assert_eq!(last_str.span.hi as usize, "let s = \"abc\nlet t = 1;".len());
        assert_eq!(out.tokens.last().unwrap().kind, Eof);
    }

    #[test]
    fn char_literals() {
        let out = lex(r"'a' 'é' '😀' '\n' '\'' '\\' '\x41' '\u{1F600}' '\0'");
        let chars: Vec<char> = out.kinds().into_iter().map(|k| if let Char(c) = k { c } else { '?' }).collect();
        assert_eq!(chars, vec!['a', 'é', '😀', '\n', '\'', '\\', 'A', '😀', '\0']);
        assert!(out.diags.is_empty());
    }

    #[test]
    fn char_literal_errors() {
        for (src, expect) in [
            ("''", "empty character literal"),
            ("'ab'", "may only contain one character"),
            ("'a", "unterminated character literal"),
            ("'", "unterminated character literal"),
            ("'\\q'", "unknown escape"),
        ] {
            let out = lex(src);
            assert!(!out.diags.is_empty(), "{src}");
            assert!(out.error_messages()[0].contains(expect), "{src}: {:?}", out.error_messages());
            assert!(out.error_codes().iter().all(|c| *c == "E0006" || *c == "E0004"), "{src}");
            assert_eq!(out.tokens.last().unwrap().kind, Eof);
        }
    }

    #[test]
    fn char_error_recovery_resumes_after_quote() {
        let out = lex("'ab' + 1");
        assert_eq!(out.kinds()[1], Plus);
        assert!(matches!(out.kinds()[2], Int(_)));
    }

    #[test]
    fn comments_are_skipped() {
        let out = lex("a // line comment\nb /* block */ c /* nested /* inner */ still */ d // eof");
        assert_eq!(out.kinds().len(), 4);
        assert!(out.diags.is_empty());
    }

    #[test]
    fn division_is_not_a_comment() {
        assert_eq!(lex("a / b").kinds()[1], Slash);
        assert_eq!(lex("a /= b").kinds()[1], SlashEq);
    }

    #[test]
    fn unterminated_block_comment() {
        let out = lex("a /* never closed\nb");
        assert_eq!(out.error_codes(), vec!["E0003"]);
        assert_eq!(out.kinds().len(), 1);
    }

    #[test]
    fn unknown_characters_are_grouped() {
        let out = lex("a @@ b");
        assert_eq!(out.error_codes(), vec!["E0001"]);
        assert!(out.error_messages()[0].contains("unknown characters `@@`"));
        let k = out.kinds();
        assert_eq!((k.len(), k[1]), (3, Error));

        let out = lex("a @ b # c $");
        assert_eq!(out.diags.error_count(), 3);
        assert!(out.error_messages()[0].contains("unknown character `@`"));
    }

    #[test]
    fn shebang_and_bom() {
        let out = lex("#!/usr/bin/env neon\nlet");
        assert_eq!(out.kinds(), vec![Kw(Keyword::Let)]);
        assert!(out.diags.is_empty());
        let out = lex("\u{feff}let");
        assert_eq!(out.kinds(), vec![Kw(Keyword::Let)]);
        assert_eq!(out.tokens[0].span.lo, 3);
        // `#` elsewhere is just an unknown character
        assert_eq!(lex("a # b").diags.error_count(), 1);
    }

    #[test]
    fn spans_cover_exact_text() {
        let src = "let x = 12.5 + foo(\"hi\");";
        let out = lex(src);
        let texts: Vec<&str> = out.tokens.iter().map(|t| &src[t.span.range()]).collect();
        assert_eq!(texts, vec!["let", "x", "=", "12.5", "+", "foo", "(", "\"hi\"", ")", ";", ""]);
    }

    #[test]
    fn iterator_excludes_eof() {
        let mut interner = Interner::new();
        let mut pool = LitPool::new();
        let mut diags = DiagBag::new();
        let lexer = Lexer::new(FileId(0), "a b", &mut interner, &mut pool, &mut diags);
        assert_eq!(lexer.count(), 2);
    }

    #[test]
    fn next_token_repeats_eof() {
        let mut interner = Interner::new();
        let mut pool = LitPool::new();
        let mut diags = DiagBag::new();
        let mut lexer = Lexer::new(FileId(0), "", &mut interner, &mut pool, &mut diags);
        assert_eq!(lexer.next_token().kind, Eof);
        assert_eq!(lexer.next_token().kind, Eof);
    }

    #[test]
    fn representative_program() {
        let src = "enum Option<T> { Some(T), None }\nfn id<T>(x: T) -> T { return x; }\nlet add = |a, b| => { a + b };";
        let out = lex(src);
        assert!(out.diags.is_empty());
        assert_eq!(out.kinds().len(), 45); // 13 + 17 + 15
    }

    /// Deterministic fuzzing: whatever the input, the lexer must not panic,
    /// must make progress, and must produce well-formed, ordered spans.
    #[test]
    fn fuzz_spans_are_well_formed() {
        const PIECES: &[&str] = &[
            "\"", "'", "\\", "/*", "*/", "//", "0x", "0b", "0o", "1e", "1e+", ".", "..", "..=", "é", "😀", "\n",
            "\r\n", " ", "_", "a", "Z9", "<", ">", "=", "|", "&", "-", "u8", "f32", "1", "42", "\\u{", "}", "{", "#!",
            "?", "\0", "\u{feff}", "\u{2028}", "'\\", "\"\\",
        ];
        let mut state = 0x9E3779B97F4A7C15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..3000 {
            let n = (next() % 40) as usize;
            let src: String = (0..n).map(|_| PIECES[(next() % PIECES.len() as u64) as usize]).collect();
            let out = lex(&src);
            let mut prev_end = 0u32;
            for (i, t) in out.tokens.iter().enumerate() {
                assert!(t.span.lo <= t.span.hi && t.span.hi as usize <= src.len(), "{src:?}");
                assert!(
                    src.is_char_boundary(t.span.lo as usize) && src.is_char_boundary(t.span.hi as usize),
                    "{src:?}"
                );
                assert!(t.span.lo >= prev_end, "overlap in {src:?}");
                prev_end = t.span.hi;
                if t.kind == Eof {
                    assert_eq!(i, out.tokens.len() - 1, "{src:?}");
                    assert_eq!(t.span.hi as usize, src.len());
                } else {
                    assert!(t.span.hi > t.span.lo, "empty non-Eof token in {src:?}");
                }
            }
            assert_eq!(out.tokens.last().unwrap().kind, Eof, "{src:?}");
        }
    }
}
