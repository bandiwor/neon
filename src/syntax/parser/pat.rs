//! Patterns: wildcards, bindings, literals, ranges, enum variants, `|` alternatives.

use super::{PResult, Parser};
use crate::syntax::ast::{NodeId, NodeKind, UnOp};
use crate::syntax::token::{Keyword, TokenKind};

impl Parser<'_> {
    /// A pattern including top-level `A | B` alternatives.
    pub(super) fn parse_pattern(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        let first = self.parse_pattern_atom()?;
        if !self.at(TokenKind::Pipe) {
            return Ok(first);
        }
        let mut alts = vec![first];
        while self.eat(TokenKind::Pipe) {
            alts.push(self.parse_pattern_atom()?);
        }
        let alts = self.list(&alts);
        Ok(self.node(NodeKind::PatOr { alts }, lo))
    }

    fn parse_pattern_atom(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        match self.peek() {
            TokenKind::Underscore => {
                self.bump();
                Ok(self.node(NodeKind::PatWild, lo))
            }
            TokenKind::Kw(Keyword::Mut) => {
                self.bump();
                let (name, _) = self.expect_ident()?;
                Ok(self.node(NodeKind::PatBinding { name, mutable: true }, lo))
            }
            TokenKind::Int(_)
            | TokenKind::Float(_)
            | TokenKind::Str(_)
            | TokenKind::Char(_)
            | TokenKind::Minus
            | TokenKind::Kw(Keyword::True | Keyword::False) => self.parse_literal_pattern(),
            TokenKind::LParen if self.peek_at(1) == TokenKind::RParen => {
                self.bump();
                self.bump();
                let unit = self.node(NodeKind::UnitLit, lo);
                Ok(self.node(NodeKind::PatLit { expr: unit }, lo))
            }
            TokenKind::Ident(_) | TokenKind::Kw(Keyword::SelfType) => self.parse_path_pattern(),
            _ => Err(self.unexpected("E0104", "pattern")),
        }
    }

    /// `1`, `-2`, `"s"`, `'c'`, `true`, and ranges `1..5` / `'a'..='z'`.
    fn parse_literal_pattern(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        let first = self.parse_literal_operand()?;
        if matches!(self.peek(), TokenKind::DotDot | TokenKind::DotDotEq) {
            let inclusive = self.bump().kind == TokenKind::DotDotEq;
            let hi = self.parse_literal_operand()?;
            return Ok(self.node(NodeKind::PatRange { lo: first, hi, inclusive }, lo));
        }
        Ok(self.node(NodeKind::PatLit { expr: first }, lo))
    }

    /// A literal, optionally negated.
    fn parse_literal_operand(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        let negate = self.eat(TokenKind::Minus);
        let kind = match self.peek() {
            TokenKind::Int(id) => NodeKind::IntLit(id),
            TokenKind::Float(id) => NodeKind::FloatLit(id),
            TokenKind::Str(id) if !negate => NodeKind::StrLit(id),
            TokenKind::Char(c) if !negate => NodeKind::CharLit(c),
            TokenKind::Kw(Keyword::True) if !negate => NodeKind::BoolLit(true),
            TokenKind::Kw(Keyword::False) if !negate => NodeKind::BoolLit(false),
            _ => return Err(self.unexpected("E0104", "literal pattern")),
        };
        let lit_lo = self.cur_lo();
        self.bump();
        let lit = self.node(kind, lit_lo);
        if negate {
            Ok(self.node(NodeKind::Unary { op: UnOp::Neg, operand: lit }, lo))
        } else {
            Ok(lit)
        }
    }

    /// `x`, `Enum::Variant`, `Enum::Variant(p, ..)`, `Some(p)`.
    fn parse_path_pattern(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        let path = self.parse_path(false)?;
        if self.eat(TokenKind::LParen) {
            let mut fields = Vec::new();
            loop {
                if self.eat(TokenKind::RParen) {
                    break;
                }
                fields.push(self.parse_pattern()?);
                if self.eat(TokenKind::Comma) {
                    continue;
                }
                self.expect(TokenKind::RParen)?;
                break;
            }
            let fields = self.list(&fields);
            return Ok(self.node(NodeKind::PatVariant { path, fields, parens: true }, lo));
        }
        match *self.ast.kind(path) {
            NodeKind::Ident(name) => Ok(self.node(NodeKind::PatBinding { name, mutable: false }, lo)),
            _ => {
                let fields = self.list(&[]);
                Ok(self.node(NodeKind::PatVariant { path, fields, parens: false }, lo))
            }
        }
    }
}
