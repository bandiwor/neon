//! Error-resilient recursive-descent parser producing a flat [`Ast`].
//!
//! Recovery is panic-mode: when a statement or declaration fails to parse, one
//! diagnostic is recorded, tokens are skipped up to the next statement
//! delimiter (a `;`, the `}` closing the failed construct, or the keyword that
//! starts the next declaration), and an [`NodeKind::Error`] node stands in for
//! the skipped text. Parsing then continues, so one run reports many errors
//! and tooling always receives a (partial) tree.
//!
//! Diagnostic codes: E0101 unexpected token, E0102 expected expression,
//! E0103 expected type, E0104 expected pattern, E0105 unclosed delimiter,
//! E0106 invalid assignment target, E0107 chained comparison,
//! E0108 expected declaration.

mod expr;
mod item;
mod pat;
mod stmt;
mod ty;

use super::ast::{Ast, NodeId, NodeKind, NodeList, OptionNodeId};
use super::interner::{Interner, Symbol};
use super::lexer::tokenize;
use super::literal::LitPool;
use super::token::{Keyword, Token, TokenKind};
use crate::diag::{DiagBag, Diagnostic, FileId, Span};

/// Marker for "an error was already reported; unwind to the nearest
/// recovery point".
#[derive(Debug, Clone, Copy)]
pub(super) struct Recover;

pub(super) type PResult<T> = Result<T, Recover>;

pub(super) struct Parser<'a> {
    pub(super) file: FileId,
    pub(super) src: &'a str,
    pub(super) tokens: Vec<Token>,
    pub(super) pos: usize,
    pub(super) ast: Ast,
    pub(super) diags: &'a mut DiagBag,
    /// Struct literals (`Name { .. }`) are not allowed in `if`/`while`/`for`/
    /// `match` headers, where `{` starts the body.
    pub(super) no_struct: bool,
    last_error_lo: Option<u32>,
}

/// Lexes and parses `src`. Lexical and syntax errors go to `diags`; the
/// returned tree is always complete enough to walk (see [`NodeKind::Error`]).
pub fn parse_source(file: FileId, src: &str, interner: &mut Interner, diags: &mut DiagBag) -> Ast {
    let mut lits = LitPool::new();
    let tokens = tokenize(file, src, interner, &mut lits, diags);
    Parser { file, src, tokens, pos: 0, ast: Ast::new(file, lits), diags, no_struct: false, last_error_lo: None }
        .parse_module()
}

impl<'a> Parser<'a> {
    // ---- token cursor ------------------------------------------------------

    pub(super) fn tok(&self) -> Token {
        self.tokens[self.pos]
    }

    pub(super) fn peek(&self) -> TokenKind {
        self.tokens[self.pos].kind
    }

    pub(super) fn peek_at(&self, n: usize) -> TokenKind {
        self.tokens[(self.pos + n).min(self.tokens.len() - 1)].kind
    }

    /// Consumes and returns the current token; `Eof` is never consumed.
    pub(super) fn bump(&mut self) -> Token {
        let t = self.tokens[self.pos];
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        t
    }

    pub(super) fn at(&self, kind: TokenKind) -> bool {
        self.peek() == kind
    }

    pub(super) fn at_kw(&self, kw: Keyword) -> bool {
        self.peek() == TokenKind::Kw(kw)
    }

    pub(super) fn eat(&mut self, kind: TokenKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    pub(super) fn eat_kw(&mut self, kw: Keyword) -> bool {
        self.eat(TokenKind::Kw(kw))
    }

    pub(super) fn cur_lo(&self) -> u32 {
        self.tok().span.lo
    }

    /// End offset of the last consumed token.
    pub(super) fn prev_hi(&self) -> u32 {
        if self.pos == 0 {
            0
        } else {
            self.tokens[self.pos - 1].span.hi
        }
    }

    /// Span from `lo` to the end of the last consumed token.
    pub(super) fn span_from(&self, lo: u32) -> Span {
        Span::new(self.file, lo, self.prev_hi().max(lo))
    }

    pub(super) fn node(&mut self, kind: NodeKind, lo: u32) -> NodeId {
        let span = self.span_from(lo);
        self.ast.push(kind, span)
    }

    /// Like [`node`](Self::node), also recording the span of the declared name.
    pub(super) fn node_n(&mut self, name_span: Span, kind: NodeKind, lo: u32) -> NodeId {
        let id = self.node(kind, lo);
        self.ast.set_name_span(id, name_span);
        id
    }

    pub(super) fn list(&mut self, ids: &[NodeId]) -> NodeList {
        self.ast.push_list(ids)
    }

    pub(super) fn with_no_struct<R>(&mut self, value: bool, f: impl FnOnce(&mut Self) -> R) -> R {
        let saved = std::mem::replace(&mut self.no_struct, value);
        let result = f(self);
        self.no_struct = saved;
        result
    }

    // ---- errors ------------------------------------------------------------

    /// Reports an error unless one was already reported at the same token
    /// (which would only be a cascade of the first).
    pub(super) fn error_at(&mut self, span: Span, code: &'static str, message: String, label: String) -> Recover {
        if self.last_error_lo != Some(span.lo) {
            self.last_error_lo = Some(span.lo);
            self.diags.report(Diagnostic::error(message).with_code(code).with_label(span, label));
        }
        Recover
    }

    /// How the current token is named in messages.
    fn describe_found(&self) -> String {
        let t = self.tok();
        match t.kind {
            TokenKind::Ident(_) | TokenKind::Int(_) | TokenKind::Float(_) | TokenKind::Str(_) | TokenKind::Char(_) => {
                let text = &self.src[t.span.range()];
                let shown: String = text.chars().take(24).collect();
                let dots = if shown.len() < text.len() { "..." } else { "" };
                format!("`{shown}{dots}`")
            }
            TokenKind::Kw(k) => format!("keyword `{}`", k.as_str()),
            other => other.describe().to_string(),
        }
    }

    /// "expected `what`, found X" at the current token. Lexer errors were
    /// already reported, so an `Error` token only triggers recovery.
    pub(super) fn unexpected(&mut self, code: &'static str, what: &str) -> Recover {
        if self.peek() == TokenKind::Error {
            return Recover;
        }
        let span = self.tok().span;
        let message = format!("expected {what}, found {}", self.describe_found());
        self.error_at(span, code, message, format!("expected {what}"))
    }

    pub(super) fn expect(&mut self, kind: TokenKind) -> PResult<Token> {
        if self.at(kind) {
            Ok(self.bump())
        } else {
            Err(self.unexpected("E0101", kind.describe()))
        }
    }

    pub(super) fn expect_kw(&mut self, kw: Keyword) -> PResult<Token> {
        if self.at_kw(kw) {
            Ok(self.bump())
        } else {
            Err(self.unexpected("E0101", &format!("`{}`", kw.as_str())))
        }
    }

    pub(super) fn expect_ident(&mut self) -> PResult<(Symbol, Span)> {
        if let TokenKind::Ident(sym) = self.peek() {
            let t = self.bump();
            Ok((sym, t.span))
        } else {
            Err(self.unexpected("E0101", "identifier"))
        }
    }

    /// A missing `;` is reported just after the previous token.
    pub(super) fn expect_semi(&mut self) -> PResult<()> {
        if self.eat(TokenKind::Semi) {
            return Ok(());
        }
        Err(self.missing_semi())
    }

    pub(super) fn missing_semi(&mut self) -> Recover {
        if self.peek() == TokenKind::Error {
            return Recover;
        }
        let at = if self.pos == 0 { self.tok().span } else { self.tokens[self.pos - 1].span.shrink_to_hi() };
        let message = format!("expected `;`, found {}", self.describe_found());
        self.error_at(at, "E0101", message, "expected `;`".to_string())
    }

    /// Consumes a `>` closing a generic list, splitting `>>`, `>=` and `>>=`
    /// when two closers (or a closer and an `=`) were lexed as one token.
    pub(super) fn expect_gt(&mut self) -> PResult<()> {
        let t = self.tok();
        let rest = match t.kind {
            TokenKind::Gt => {
                self.bump();
                return Ok(());
            }
            TokenKind::Shr => TokenKind::Gt,
            TokenKind::Ge => TokenKind::Eq,
            TokenKind::ShrEq => TokenKind::Ge,
            _ => return Err(self.unexpected("E0101", "`>`")),
        };
        self.tokens[self.pos] = Token { kind: rest, span: Span::new(self.file, t.span.lo + 1, t.span.hi) };
        // The consumed `>` is the first byte of the original token.
        Ok(())
    }

    /// Reports an unclosed delimiter at end of input.
    pub(super) fn unclosed(&mut self, open: Token, close: &str) {
        let eof = self.tok().span;
        if self.last_error_lo == Some(eof.lo) {
            return;
        }
        self.last_error_lo = Some(eof.lo);
        let opener = &self.src[open.span.range()];
        self.diags.report(
            Diagnostic::error(format!("unclosed delimiter `{opener}`"))
                .with_code("E0105")
                .with_label(open.span, "unclosed delimiter")
                .with_secondary(eof, format!("expected {close} before the end of the file")),
        );
    }

    // ---- recovery ----------------------------------------------------------

    /// Skips to the end of the construct that failed: just past the next `;`,
    /// just past a `}` that closes a block opened while skipping, or up to
    /// (not including) an unmatched `}` or a keyword that starts a new
    /// statement or declaration.
    pub(super) fn synchronize(&mut self) {
        let mut depth = 0u32;
        loop {
            match self.peek() {
                TokenKind::Eof => return,
                TokenKind::Semi if depth == 0 => {
                    self.bump();
                    return;
                }
                TokenKind::LBrace => {
                    depth += 1;
                    self.bump();
                }
                TokenKind::RBrace => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                    self.bump();
                    if depth == 0 {
                        return;
                    }
                }
                TokenKind::Kw(
                    Keyword::Let
                    | Keyword::Fn
                    | Keyword::Struct
                    | Keyword::Enum
                    | Keyword::Trait
                    | Keyword::Impl
                    | Keyword::Import
                    | Keyword::Export
                    | Keyword::Const,
                ) if depth == 0 => return,
                _ => {
                    self.bump();
                }
            }
        }
    }

    /// Recovery for a failed match arm: skip past the next top-level `,` or
    /// stop before the `}` that ends the match.
    pub(super) fn synchronize_arm(&mut self) {
        let mut depth = 0u32;
        loop {
            match self.peek() {
                TokenKind::Eof => return,
                TokenKind::Comma if depth == 0 => {
                    self.bump();
                    return;
                }
                TokenKind::LBrace => {
                    depth += 1;
                    self.bump();
                }
                TokenKind::RBrace => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                    self.bump();
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    /// Finishes a failed construct that began at token index `start`:
    /// synchronizes, guarantees progress, and returns an `Error` node
    /// covering the skipped tokens.
    pub(super) fn recover_node(&mut self, start: usize, arm: bool) -> NodeId {
        let lo = self.tokens[start].span.lo;
        if arm {
            self.synchronize_arm();
        } else {
            self.synchronize();
        }
        if self.pos == start && self.peek() != TokenKind::Eof {
            self.bump();
        }
        self.node(NodeKind::Error, lo)
    }

    // ---- module ------------------------------------------------------------

    fn parse_module(mut self) -> Ast {
        let mut items = Vec::new();
        while !self.at(TokenKind::Eof) {
            let start = self.pos;
            match self.parse_stmt() {
                Ok(stmt::Stmt::Node(id)) => items.push(id),
                Ok(stmt::Stmt::Tail(expr)) => {
                    // A final expression without `;` (what a REPL echoes).
                    let span = self.ast.span(expr);
                    items.push(self.ast.push(NodeKind::ExprStmt { expr, semi: false }, span));
                }
                Ok(stmt::Stmt::Empty) => {}
                Err(Recover) => items.push(self.recover_node(start, false)),
            }
        }
        let list = self.list(&items);
        let span = Span::new(self.file, 0, self.src.len() as u32);
        let root = self.ast.push(NodeKind::Module { items: list }, span);
        self.ast.set_root(root);
        self.ast
    }

    pub(super) fn opt(&self, id: Option<NodeId>) -> OptionNodeId {
        id.into()
    }
}

#[cfg(test)]
mod tests;
