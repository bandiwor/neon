//! Types: primitives, paths with generic arguments, `[T]`, `{K: V}`, `set<T>`,
//! `fn(A) -> R`, `()` and `Self`.

use super::{PResult, Parser};
use crate::syntax::ast::{NodeId, NodeKind, NodeList};
use crate::syntax::interner::sym;
use crate::syntax::token::{Keyword, TokenKind};

impl Parser<'_> {
    pub(super) fn parse_type(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        match self.peek() {
            TokenKind::Kw(k) if k.is_primitive_type() => {
                if self.peek_at(1) == TokenKind::ColonColon {
                    return self.parse_path(true);
                }
                self.bump();
                Ok(self.node(NodeKind::TyPrim(k), lo))
            }
            TokenKind::Kw(Keyword::SelfType) => {
                if self.peek_at(1) == TokenKind::ColonColon {
                    return self.parse_path(true);
                }
                self.bump();
                Ok(self.node(NodeKind::TySelf, lo))
            }
            TokenKind::Ident(s) if s == sym::SET && self.peek_at(1) == TokenKind::Lt => {
                self.bump();
                self.bump();
                let elem = self.parse_type()?;
                self.expect_gt()?;
                Ok(self.node(NodeKind::TySet { elem }, lo))
            }
            TokenKind::Ident(_) => self.parse_path(true),
            TokenKind::LBracket => {
                self.bump();
                let elem = self.parse_type()?;
                self.expect(TokenKind::RBracket)?;
                Ok(self.node(NodeKind::TyArray { elem }, lo))
            }
            TokenKind::LBrace => {
                self.bump();
                let key = self.parse_type()?;
                self.expect(TokenKind::Colon)?;
                let value = self.parse_type()?;
                self.expect(TokenKind::RBrace)?;
                Ok(self.node(NodeKind::TyMap { key, value }, lo))
            }
            TokenKind::LParen => {
                self.bump();
                self.expect(TokenKind::RParen)?;
                Ok(self.node(NodeKind::TyUnit, lo))
            }
            TokenKind::Kw(Keyword::Fn) => {
                self.bump();
                self.expect(TokenKind::LParen)?;
                let mut params = Vec::new();
                loop {
                    if self.eat(TokenKind::RParen) {
                        break;
                    }
                    params.push(self.parse_type()?);
                    if self.eat(TokenKind::Comma) {
                        continue;
                    }
                    self.expect(TokenKind::RParen)?;
                    break;
                }
                let ret = if self.eat(TokenKind::Arrow) { Some(self.parse_type()?) } else { None };
                let params = self.list(&params);
                Ok(self.node(NodeKind::TyFn { params, ret: self.opt(ret) }, lo))
            }
            _ => Err(self.unexpected("E0103", "type")),
        }
    }

    /// `<T, U>` after a path segment (the `<` is the current token).
    pub(super) fn parse_type_args(&mut self) -> PResult<NodeList> {
        self.expect(TokenKind::Lt)?;
        let mut args = Vec::new();
        loop {
            if self.at(TokenKind::Gt) || matches!(self.peek(), TokenKind::Shr | TokenKind::Ge | TokenKind::ShrEq) {
                self.expect_gt()?;
                break;
            }
            args.push(self.parse_type()?);
            if self.eat(TokenKind::Comma) {
                continue;
            }
            self.expect_gt()?;
            break;
        }
        Ok(self.list(&args))
    }
}
