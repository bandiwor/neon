//! Blocks and statements.

use super::{PResult, Parser, Recover};
use crate::syntax::ast::{NodeId, NodeKind, OptionNodeId};
use crate::syntax::token::{Keyword, TokenKind};

/// Outcome of parsing one statement.
pub(super) enum Stmt {
    /// A complete statement or declaration node.
    Node(NodeId),
    /// An expression with no `;` directly before `}` / end of file: the
    /// value of the enclosing block.
    Tail(NodeId),
    /// A bare `;`.
    Empty,
}

impl Parser<'_> {
    /// Whether the current token starts an expression that needs no `;` when
    /// used as a statement (and that ends where its closing brace does).
    fn at_block_like(&self) -> bool {
        matches!(
            self.peek(),
            TokenKind::LBrace
                | TokenKind::Kw(Keyword::If | Keyword::Match | Keyword::While | Keyword::Loop | Keyword::For)
        )
    }

    fn is_block_like_node(&self, id: NodeId) -> bool {
        matches!(
            self.ast.kind(id),
            NodeKind::Block { .. }
                | NodeKind::If { .. }
                | NodeKind::Match { .. }
                | NodeKind::While { .. }
                | NodeKind::Loop { .. }
                | NodeKind::For { .. }
        )
    }

    pub(super) fn is_block_like_expr(&self, id: NodeId) -> bool {
        self.is_block_like_node(id)
    }

    pub(super) fn parse_stmt(&mut self) -> PResult<Stmt> {
        let lo = self.cur_lo();
        match self.peek() {
            TokenKind::Semi => {
                self.bump();
                Ok(Stmt::Empty)
            }
            TokenKind::Kw(Keyword::Let) => self.parse_let().map(Stmt::Node),
            TokenKind::Kw(
                Keyword::Fn
                | Keyword::Struct
                | Keyword::Enum
                | Keyword::Trait
                | Keyword::Impl
                | Keyword::Const
                | Keyword::Import
                | Keyword::Export,
            ) => self.parse_item().map(Stmt::Node),
            _ => {
                let block_like = self.at_block_like();
                let expr = if block_like && !self.brace_starts_map(false) {
                    // `if`/`match`/loops/blocks end at their closing brace;
                    // an operator after them starts a new statement.
                    self.parse_block_like()?
                } else {
                    self.parse_expr()?
                };
                if self.eat(TokenKind::Semi) {
                    return Ok(Stmt::Node(self.node(NodeKind::ExprStmt { expr, semi: true }, lo)));
                }
                if matches!(self.peek(), TokenKind::RBrace | TokenKind::Eof) {
                    return Ok(Stmt::Tail(expr));
                }
                if !self.is_block_like_node(expr) {
                    // The expression is complete: report the missing `;`
                    // and carry on with the next statement.
                    let _ = self.missing_semi();
                }
                Ok(Stmt::Node(self.node(NodeKind::ExprStmt { expr, semi: false }, lo)))
            }
        }
    }

    /// `if`, `match`, `while`, `loop`, `for` or a block.
    pub(super) fn parse_block_like(&mut self) -> PResult<NodeId> {
        match self.peek() {
            TokenKind::LBrace => self.parse_block(),
            TokenKind::Kw(Keyword::If) => self.parse_if(),
            TokenKind::Kw(Keyword::Match) => self.parse_match(),
            TokenKind::Kw(Keyword::While) => self.parse_while(),
            TokenKind::Kw(Keyword::Loop) => self.parse_loop(),
            TokenKind::Kw(Keyword::For) => self.parse_for(),
            _ => Err(self.unexpected("E0102", "expression")),
        }
    }

    fn parse_let(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        self.bump();
        let pat = self.parse_pattern()?;
        let ty = if self.eat(TokenKind::Colon) { Some(self.parse_type()?) } else { None };
        let init = if self.eat(TokenKind::Eq) { Some(self.parse_expr()?) } else { None };
        self.expect_semi()?;
        Ok(self.node(NodeKind::Let { pat, ty: self.opt(ty), init: self.opt(init) }, lo))
    }

    /// `{ stmt* tail? }`. Statements that fail to parse become `Error` nodes
    /// and parsing resumes at the next statement.
    pub(super) fn parse_block(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        let open = self.expect(TokenKind::LBrace)?;
        let saved = std::mem::replace(&mut self.no_struct, false);
        let mut stmts = Vec::new();
        let mut tail = OptionNodeId::NONE;
        loop {
            match self.peek() {
                TokenKind::RBrace => {
                    self.bump();
                    break;
                }
                TokenKind::Eof => {
                    self.unclosed(open, "`}`");
                    break;
                }
                _ => {}
            }
            let start = self.pos;
            match self.parse_stmt() {
                Ok(Stmt::Node(id)) => stmts.push(id),
                Ok(Stmt::Tail(expr)) => tail = OptionNodeId::some(expr),
                Ok(Stmt::Empty) => {}
                Err(Recover) => stmts.push(self.recover_node(start, false)),
            }
        }
        self.no_struct = saved;
        let stmts = self.list(&stmts);
        Ok(self.node(NodeKind::Block { stmts, tail }, lo))
    }
}
