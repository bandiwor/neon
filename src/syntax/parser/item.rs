//! Declarations: imports, functions, structs, enums, traits, impls, consts.

use super::{PResult, Parser, Recover};
use crate::syntax::ast::{NodeId, NodeKind, NodeList};
use crate::syntax::interner::{sym, Symbol};
use crate::syntax::token::{Keyword, TokenKind};

impl Parser<'_> {
    pub(super) fn parse_item(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        match self.peek() {
            TokenKind::Kw(Keyword::Export) => {
                self.bump();
                let item = match self.peek() {
                    TokenKind::Kw(Keyword::Fn) => self.parse_fn()?,
                    TokenKind::Kw(Keyword::Struct) => self.parse_struct()?,
                    TokenKind::Kw(Keyword::Enum) => self.parse_enum()?,
                    TokenKind::Kw(Keyword::Trait) => self.parse_trait()?,
                    TokenKind::Kw(Keyword::Const) => self.parse_const()?,
                    _ => return Err(self.unexpected("E0108", "a declaration after `export`")),
                };
                Ok(self.node(NodeKind::Export { item }, lo))
            }
            TokenKind::Kw(Keyword::Import) => self.parse_import(),
            TokenKind::Kw(Keyword::Fn) => self.parse_fn(),
            TokenKind::Kw(Keyword::Struct) => self.parse_struct(),
            TokenKind::Kw(Keyword::Enum) => self.parse_enum(),
            TokenKind::Kw(Keyword::Trait) => self.parse_trait(),
            TokenKind::Kw(Keyword::Impl) => self.parse_impl(),
            TokenKind::Kw(Keyword::Const) => self.parse_const(),
            _ => Err(self.unexpected("E0108", "a declaration")),
        }
    }

    /// `import { a, b as c } from "path";` or `import "path" as alias;`
    fn parse_import(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        self.bump();
        if let TokenKind::Str(path) = self.peek() {
            self.bump();
            self.expect_kw(Keyword::As)?;
            let (alias, alias_span) = self.expect_ident()?;
            self.expect_semi()?;
            return Ok(self.node_n(alias_span, NodeKind::ImportAlias { path, alias }, lo));
        }
        let open = self.expect(TokenKind::LBrace)?;
        let mut names = Vec::new();
        loop {
            if self.eat(TokenKind::RBrace) {
                break;
            }
            if self.at(TokenKind::Eof) {
                self.unclosed(open, "`}`");
                return Err(Recover);
            }
            let n_lo = self.cur_lo();
            let (name, name_span) = self.expect_ident()?;
            let alias = if self.eat_kw(Keyword::As) { Some(self.expect_ident()?.0) } else { None };
            names.push(self.node_n(name_span, NodeKind::ImportName { name, alias }, n_lo));
            if self.eat(TokenKind::Comma) {
                continue;
            }
            self.expect(TokenKind::RBrace)?;
            break;
        }
        match self.peek() {
            TokenKind::Ident(s) if s == sym::FROM => {
                self.bump();
            }
            _ => return Err(self.unexpected("E0101", "`from`")),
        }
        let TokenKind::Str(path) = self.peek() else {
            return Err(self.unexpected("E0101", "a module path string"));
        };
        self.bump();
        self.expect_semi()?;
        let names = self.list(&names);
        Ok(self.node(NodeKind::ImportNamed { names, path }, lo))
    }

    /// `<T: A + B, U>`; empty when the next token is not `<`.
    fn parse_generic_params(&mut self) -> PResult<NodeList> {
        if !self.eat(TokenKind::Lt) {
            return Ok(NodeList::EMPTY);
        }
        let mut params = Vec::new();
        loop {
            if self.at(TokenKind::Gt) {
                self.expect_gt()?;
                break;
            }
            let lo = self.cur_lo();
            let (name, name_span) = self.expect_ident()?;
            let mut bounds = Vec::new();
            if self.eat(TokenKind::Colon) {
                loop {
                    bounds.push(self.parse_path(true)?);
                    if !self.eat(TokenKind::Plus) {
                        break;
                    }
                }
            }
            let bounds = self.list(&bounds);
            params.push(self.node_n(name_span, NodeKind::GenericParam { name, bounds }, lo));
            if self.eat(TokenKind::Comma) {
                continue;
            }
            self.expect_gt()?;
            break;
        }
        Ok(self.list(&params))
    }

    /// `fn name<G>(params) -> Ret { body }`; the body is replaced by `;` in a
    /// trait signature.
    pub(super) fn parse_fn(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        self.expect_kw(Keyword::Fn)?;
        let (name, name_span) = self.expect_ident()?;
        let generics = self.parse_generic_params()?;
        self.expect(TokenKind::LParen)?;
        let mut params = Vec::new();
        loop {
            if self.eat(TokenKind::RParen) {
                break;
            }
            params.push(self.parse_param()?);
            if self.eat(TokenKind::Comma) {
                continue;
            }
            self.expect(TokenKind::RParen)?;
            break;
        }
        let ret = if self.eat(TokenKind::Arrow) { Some(self.parse_type()?) } else { None };
        let body = if self.at(TokenKind::LBrace) {
            Some(self.parse_block()?)
        } else {
            self.expect_semi()?;
            None
        };
        let params = self.list(&params);
        Ok(self.node_n(
            name_span,
            NodeKind::FnDecl { name, generics, params, ret: self.opt(ret), body: self.opt(body) },
            lo,
        ))
    }

    /// `[var] name: Type`, or a `self` receiver.
    fn parse_param(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        let mutable = self.eat_kw(Keyword::Var);
        let self_span = self.tok().span;
        if self.eat_kw(Keyword::SelfValue) {
            let name = Symbol(Keyword::SelfValue as u32);
            let ty = if self.eat(TokenKind::Colon) { Some(self.parse_type()?) } else { None };
            return Ok(self.node_n(self_span, NodeKind::Param { name, mutable, ty: self.opt(ty) }, lo));
        }
        let (name, name_span) = self.expect_ident()?;
        self.expect(TokenKind::Colon)?;
        let ty = self.parse_type()?;
        Ok(self.node_n(name_span, NodeKind::Param { name, mutable, ty: ty.into() }, lo))
    }

    /// `struct Name<G> { a: T, b: U }` or `struct Name;`
    fn parse_struct(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        self.bump();
        let (name, name_span) = self.expect_ident()?;
        let generics = self.parse_generic_params()?;
        if self.eat(TokenKind::Semi) {
            return Ok(self.node_n(name_span, NodeKind::StructDecl { name, generics, fields: NodeList::EMPTY }, lo));
        }
        let open = self.expect(TokenKind::LBrace)?;
        let mut fields = Vec::new();
        loop {
            if self.eat(TokenKind::RBrace) {
                break;
            }
            if self.at(TokenKind::Eof) {
                self.unclosed(open, "`}`");
                break;
            }
            let f_lo = self.cur_lo();
            let (fname, fname_span) = self.expect_ident()?;
            self.expect(TokenKind::Colon)?;
            let ty = self.parse_type()?;
            fields.push(self.node_n(fname_span, NodeKind::FieldDecl { name: fname, ty }, f_lo));
            if self.eat(TokenKind::Comma) {
                continue;
            }
            self.expect(TokenKind::RBrace)?;
            break;
        }
        let fields = self.list(&fields);
        Ok(self.node_n(name_span, NodeKind::StructDecl { name, generics, fields }, lo))
    }

    /// `enum Name<G> { A, B(T, U) }`
    fn parse_enum(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        self.bump();
        let (name, name_span) = self.expect_ident()?;
        let generics = self.parse_generic_params()?;
        let open = self.expect(TokenKind::LBrace)?;
        let mut variants = Vec::new();
        loop {
            if self.eat(TokenKind::RBrace) {
                break;
            }
            if self.at(TokenKind::Eof) {
                self.unclosed(open, "`}`");
                break;
            }
            let v_lo = self.cur_lo();
            let (vname, vname_span) = self.expect_ident()?;
            let mut payload = Vec::new();
            if self.eat(TokenKind::LParen) {
                loop {
                    if self.eat(TokenKind::RParen) {
                        break;
                    }
                    payload.push(self.parse_type()?);
                    if self.eat(TokenKind::Comma) {
                        continue;
                    }
                    self.expect(TokenKind::RParen)?;
                    break;
                }
            }
            let fields = self.list(&payload);
            variants.push(self.node_n(vname_span, NodeKind::Variant { name: vname, fields }, v_lo));
            if self.eat(TokenKind::Comma) {
                continue;
            }
            self.expect(TokenKind::RBrace)?;
            break;
        }
        let variants = self.list(&variants);
        Ok(self.node_n(name_span, NodeKind::EnumDecl { name, generics, variants }, lo))
    }

    /// Methods inside `trait`/`impl` braces; a bad member becomes an `Error`
    /// node and parsing resumes at the next `fn`.
    fn parse_methods(&mut self) -> PResult<NodeList> {
        let open = self.expect(TokenKind::LBrace)?;
        let mut items = Vec::new();
        loop {
            if self.eat(TokenKind::RBrace) {
                break;
            }
            if self.at(TokenKind::Eof) {
                self.unclosed(open, "`}`");
                break;
            }
            let start = self.pos;
            let result =
                if self.at_kw(Keyword::Fn) { self.parse_fn() } else { Err(self.unexpected("E0108", "`fn` or `}`")) };
            match result {
                Ok(id) => items.push(id),
                Err(Recover) => items.push(self.recover_node(start, false)),
            }
        }
        Ok(self.list(&items))
    }

    fn parse_trait(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        self.bump();
        let (name, name_span) = self.expect_ident()?;
        let generics = self.parse_generic_params()?;
        let items = self.parse_methods()?;
        Ok(self.node_n(name_span, NodeKind::TraitDecl { name, generics, items }, lo))
    }

    /// `impl<G> Type { .. }` or `impl<G> Trait<A> for Type { .. }`
    fn parse_impl(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        self.bump();
        let generics = self.parse_generic_params()?;
        let first = self.parse_type()?;
        let (trait_ref, self_ty) =
            if self.eat_kw(Keyword::For) { (Some(first), self.parse_type()?) } else { (None, first) };
        let items = self.parse_methods()?;
        Ok(self.node(NodeKind::ImplDecl { generics, trait_ref: self.opt(trait_ref), self_ty, items }, lo))
    }

    /// `const NAME: Type = expr;`
    fn parse_const(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        self.bump();
        let (name, name_span) = self.expect_ident()?;
        let ty = if self.eat(TokenKind::Colon) { Some(self.parse_type()?) } else { None };
        self.expect(TokenKind::Eq)?;
        let init = self.parse_expr()?;
        self.expect_semi()?;
        Ok(self.node_n(name_span, NodeKind::ConstDecl { name, ty: self.opt(ty), init }, lo))
    }
}
