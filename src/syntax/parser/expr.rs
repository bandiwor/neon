//! Expressions.
//!
//! Precedence, loosest to tightest: assignment (right-assoc), ranges, `||`,
//! `&&`, comparisons (non-chaining), `|`, `^`, `&`, shifts, `+ -`, `* / %`,
//! `as`, unary `- ! ~`, postfix (call, index, field, method call).

use super::{PResult, Parser, Recover};
use crate::diag::Diagnostic;
use crate::syntax::ast::{BinOp, NodeId, NodeKind, OptionNodeId, UnOp};
use crate::syntax::interner::{sym, Symbol};
use crate::syntax::literal::StrId;
use crate::syntax::token::{Keyword, TokenKind};

const CAST_PREC: u8 = 10;

impl Parser<'_> {
    pub(super) fn parse_expr(&mut self) -> PResult<NodeId> {
        self.parse_assign()
    }

    /// An expression with struct literals allowed again (inside delimiters).
    pub(super) fn parse_expr_free(&mut self) -> PResult<NodeId> {
        self.with_no_struct(false, |p| p.parse_expr())
    }

    fn parse_assign(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        let target = self.parse_range()?;
        let op = match self.peek() {
            TokenKind::Eq => None,
            TokenKind::PlusEq => Some(BinOp::Add),
            TokenKind::MinusEq => Some(BinOp::Sub),
            TokenKind::StarEq => Some(BinOp::Mul),
            TokenKind::SlashEq => Some(BinOp::Div),
            TokenKind::PercentEq => Some(BinOp::Rem),
            TokenKind::AmpEq => Some(BinOp::BitAnd),
            TokenKind::PipeEq => Some(BinOp::BitOr),
            TokenKind::CaretEq => Some(BinOp::BitXor),
            TokenKind::ShlEq => Some(BinOp::Shl),
            TokenKind::ShrEq => Some(BinOp::Shr),
            _ => return Ok(target),
        };
        if !matches!(self.ast.kind(target), NodeKind::Ident(_) | NodeKind::Field { .. } | NodeKind::Index { .. }) {
            let span = self.ast.span(target);
            let _ = self.error_at(
                span,
                "E0106",
                "invalid left-hand side of assignment".to_string(),
                "cannot assign to this expression".to_string(),
            );
        }
        self.bump();
        let value = self.parse_assign()?;
        Ok(self.node(NodeKind::Assign { op, target, value }, lo))
    }

    /// Whether the current token can begin an expression (used to decide
    /// whether `..`, `return` and `break` are followed by a value).
    pub(super) fn can_start_expr(&self) -> bool {
        match self.peek() {
            TokenKind::Ident(_)
            | TokenKind::Int(_)
            | TokenKind::Float(_)
            | TokenKind::Str(_)
            | TokenKind::Char(_)
            | TokenKind::LParen
            | TokenKind::LBracket
            | TokenKind::Minus
            | TokenKind::Bang
            | TokenKind::Tilde
            | TokenKind::Pipe
            | TokenKind::PipePipe => true,
            TokenKind::LBrace => !self.no_struct,
            TokenKind::Kw(k) => {
                k.is_primitive_type()
                    || matches!(
                        k,
                        Keyword::True
                            | Keyword::False
                            | Keyword::SelfValue
                            | Keyword::SelfType
                            | Keyword::If
                            | Keyword::Match
                            | Keyword::Loop
                            | Keyword::While
                            | Keyword::For
                            | Keyword::Return
                            | Keyword::Break
                            | Keyword::Continue
                    )
            }
            _ => false,
        }
    }

    fn parse_range(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        if matches!(self.peek(), TokenKind::DotDot | TokenKind::DotDotEq) {
            let inclusive = self.bump().kind == TokenKind::DotDotEq;
            let end = if self.can_start_expr() { Some(self.parse_binary(1)?) } else { None };
            return Ok(self.node(NodeKind::Range { start: OptionNodeId::NONE, end: self.opt(end), inclusive }, lo));
        }
        let start = self.parse_binary(1)?;
        if matches!(self.peek(), TokenKind::DotDot | TokenKind::DotDotEq) {
            let inclusive = self.bump().kind == TokenKind::DotDotEq;
            let end = if self.can_start_expr() { Some(self.parse_binary(1)?) } else { None };
            return Ok(
                self.node(NodeKind::Range { start: OptionNodeId::some(start), end: self.opt(end), inclusive }, lo)
            );
        }
        Ok(start)
    }

    fn binop_at(&self) -> Option<(BinOp, u8)> {
        Some(match self.peek() {
            TokenKind::PipePipe => (BinOp::Or, 1),
            TokenKind::AmpAmp => (BinOp::And, 2),
            TokenKind::EqEq => (BinOp::Eq, 3),
            TokenKind::BangEq => (BinOp::Ne, 3),
            TokenKind::Lt => (BinOp::Lt, 3),
            TokenKind::Le => (BinOp::Le, 3),
            TokenKind::Gt => (BinOp::Gt, 3),
            TokenKind::Ge => (BinOp::Ge, 3),
            TokenKind::Pipe => (BinOp::BitOr, 4),
            TokenKind::Caret => (BinOp::BitXor, 5),
            TokenKind::Amp => (BinOp::BitAnd, 6),
            TokenKind::Shl => (BinOp::Shl, 7),
            TokenKind::Shr => (BinOp::Shr, 7),
            TokenKind::Plus => (BinOp::Add, 8),
            TokenKind::Minus => (BinOp::Sub, 8),
            TokenKind::Star => (BinOp::Mul, 9),
            TokenKind::Slash => (BinOp::Div, 9),
            TokenKind::Percent => (BinOp::Rem, 9),
            _ => return None,
        })
    }

    /// Precedence climbing over binary operators and `as`; all are
    /// left-associative.
    fn parse_binary(&mut self, min_prec: u8) -> PResult<NodeId> {
        let lo = self.cur_lo();
        let mut lhs = self.parse_unary()?;
        loop {
            if self.at_kw(Keyword::As) {
                if CAST_PREC < min_prec {
                    break;
                }
                self.bump();
                let ty = self.parse_type()?;
                lhs = self.node(NodeKind::Cast { expr: lhs, ty }, lo);
                continue;
            }
            let Some((op, prec)) = self.binop_at() else { break };
            if prec < min_prec {
                break;
            }
            self.bump();
            let rhs = self.parse_binary(prec + 1)?;
            lhs = self.node(NodeKind::Binary { op, lhs, rhs }, lo);
            if op.is_comparison() && matches!(self.binop_at(), Some((next, _)) if next.is_comparison()) {
                let span = self.tok().span;
                let _ = self.error_at(
                    span,
                    "E0107",
                    "comparison operators cannot be chained".to_string(),
                    "use `&&` to combine comparisons".to_string(),
                );
            }
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        let op = match self.peek() {
            TokenKind::Minus => UnOp::Neg,
            TokenKind::Bang => UnOp::Not,
            TokenKind::Tilde => UnOp::BitNot,
            _ => return self.parse_postfix(),
        };
        self.bump();
        let operand = self.parse_unary()?;
        Ok(self.node(NodeKind::Unary { op, operand }, lo))
    }

    fn parse_postfix(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        let mut e = self.parse_primary()?;
        loop {
            match self.peek() {
                TokenKind::LParen => {
                    self.bump();
                    let args = self.parse_expr_list(TokenKind::RParen)?;
                    e = self.node(NodeKind::Call { callee: e, args }, lo);
                }
                TokenKind::LBracket => {
                    self.bump();
                    let index = self.parse_expr_free()?;
                    self.expect(TokenKind::RBracket)?;
                    e = self.node(NodeKind::Index { base: e, index }, lo);
                }
                TokenKind::Dot => {
                    self.bump();
                    let (name, _) = self.expect_ident()?;
                    let mut generics = crate::syntax::ast::NodeList::EMPTY;
                    if self.at(TokenKind::ColonColon) && self.peek_at(1) == TokenKind::Lt {
                        self.bump();
                        generics = self.parse_type_args()?;
                    }
                    if self.eat(TokenKind::LParen) {
                        let args = self.parse_expr_list(TokenKind::RParen)?;
                        e = self.node(NodeKind::MethodCall { receiver: e, name, generics, args }, lo);
                    } else {
                        e = self.node(NodeKind::Field { base: e, name }, lo);
                    }
                }
                _ => break,
            }
        }
        Ok(e)
    }

    /// Comma-separated expressions up to and including `close`.
    pub(super) fn parse_expr_list(&mut self, close: TokenKind) -> PResult<crate::syntax::ast::NodeList> {
        let mut items = Vec::new();
        loop {
            if self.eat(close) {
                break;
            }
            items.push(self.parse_expr_free()?);
            if self.eat(TokenKind::Comma) {
                continue;
            }
            self.expect(close)?;
            break;
        }
        Ok(self.list(&items))
    }

    fn lit_str(&mut self, id: StrId, lo: u32) -> NodeId {
        self.node(NodeKind::StrLit(id), lo)
    }

    fn parse_primary(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        match self.peek() {
            TokenKind::Int(id) => {
                self.bump();
                Ok(self.node(NodeKind::IntLit(id), lo))
            }
            TokenKind::Float(id) => {
                self.bump();
                Ok(self.node(NodeKind::FloatLit(id), lo))
            }
            TokenKind::Str(id) => {
                self.bump();
                Ok(self.lit_str(id, lo))
            }
            TokenKind::Char(c) => {
                self.bump();
                Ok(self.node(NodeKind::CharLit(c), lo))
            }
            TokenKind::Kw(Keyword::True) => {
                self.bump();
                Ok(self.node(NodeKind::BoolLit(true), lo))
            }
            TokenKind::Kw(Keyword::False) => {
                self.bump();
                Ok(self.node(NodeKind::BoolLit(false), lo))
            }
            TokenKind::Kw(Keyword::SelfValue) if self.peek_at(1) != TokenKind::ColonColon => {
                self.bump();
                Ok(self.node(NodeKind::Ident(Symbol(Keyword::SelfValue as u32)), lo))
            }
            TokenKind::Ident(s) if s == sym::SET && self.peek_at(1) == TokenKind::LBrace && !self.no_struct => {
                self.parse_set_lit()
            }
            TokenKind::Ident(_) | TokenKind::Kw(Keyword::SelfValue | Keyword::SelfType) => self.parse_path_expr(),
            TokenKind::Kw(k) if k.is_primitive_type() && self.peek_at(1) == TokenKind::ColonColon => {
                self.parse_path_expr()
            }
            TokenKind::LParen => {
                self.bump();
                if self.eat(TokenKind::RParen) {
                    return Ok(self.node(NodeKind::UnitLit, lo));
                }
                let inner = self.parse_expr_free()?;
                self.expect(TokenKind::RParen)?;
                Ok(inner)
            }
            TokenKind::LBracket => {
                self.bump();
                let elems = self.parse_expr_list(TokenKind::RBracket)?;
                Ok(self.node(NodeKind::ArrayLit { elems }, lo))
            }
            TokenKind::LBrace => self.parse_brace_expr(true),
            TokenKind::Pipe | TokenKind::PipePipe => self.parse_closure(),
            TokenKind::Kw(Keyword::If) => self.parse_if(),
            TokenKind::Kw(Keyword::Match) => self.parse_match(),
            TokenKind::Kw(Keyword::While) => self.parse_while(),
            TokenKind::Kw(Keyword::Loop) => self.parse_loop(),
            TokenKind::Kw(Keyword::For) => self.parse_for(),
            TokenKind::Kw(Keyword::Return) => {
                self.bump();
                let value = if self.can_start_expr() { Some(self.parse_expr()?) } else { None };
                Ok(self.node(NodeKind::Return { value: self.opt(value) }, lo))
            }
            TokenKind::Kw(Keyword::Break) => {
                self.bump();
                let value = if self.can_start_expr() { Some(self.parse_expr()?) } else { None };
                Ok(self.node(NodeKind::Break { value: self.opt(value) }, lo))
            }
            TokenKind::Kw(Keyword::Continue) => {
                self.bump();
                Ok(self.node(NodeKind::Continue, lo))
            }
            _ => Err(self.unexpected("E0102", "expression")),
        }
    }

    /// A path expression, possibly followed by a struct literal body.
    fn parse_path_expr(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        let path = self.parse_path(false)?;
        if self.at(TokenKind::LBrace) && !self.no_struct && self.looks_like_struct_lit() {
            return self.parse_struct_lit(path, lo);
        }
        Ok(path)
    }

    /// Parses `a`, `a::b`, `Self::new`, `Vec::<i32>::new`. A bare identifier
    /// becomes an `Ident`; anything else a `Path` of `PathSegment`s. With
    /// `ty` set, the result is always a list of segments for a type path and
    /// generic arguments may follow a segment directly (`Option<T>`).
    pub(super) fn parse_path(&mut self, ty: bool) -> PResult<NodeId> {
        use crate::syntax::ast::NodeList;
        struct Seg {
            name: Symbol,
            lo: u32,
            hi: u32,
            args: NodeList,
        }
        let lo = self.cur_lo();
        let first = self.tok();
        let name = match first.kind {
            TokenKind::Ident(s) => s,
            TokenKind::Kw(k @ (Keyword::SelfValue | Keyword::SelfType)) => Symbol(k as u32),
            TokenKind::Kw(k) if k.is_primitive_type() => Symbol(k as u32),
            _ => return Err(self.unexpected("E0101", "identifier")),
        };
        self.bump();
        let mut segs = vec![Seg { name, lo: first.span.lo, hi: first.span.hi, args: NodeList::EMPTY }];
        if ty && self.at(TokenKind::Lt) {
            let args = self.parse_type_args()?;
            let last = segs.last_mut().unwrap();
            last.args = args;
            last.hi = self.prev_hi();
        }
        while self.at(TokenKind::ColonColon) {
            self.bump();
            if self.at(TokenKind::Lt) {
                // Turbofish: the arguments belong to the preceding segment.
                let args = self.parse_type_args()?;
                let last = segs.last_mut().unwrap();
                last.args = args;
                last.hi = self.prev_hi();
                continue;
            }
            let (name, span) = self.expect_ident()?;
            segs.push(Seg { name, lo: span.lo, hi: span.hi, args: NodeList::EMPTY });
            if ty && self.at(TokenKind::Lt) {
                let args = self.parse_type_args()?;
                let last = segs.last_mut().unwrap();
                last.args = args;
                last.hi = self.prev_hi();
            }
        }
        if !ty && segs.len() == 1 && segs[0].args.is_empty() {
            return Ok(self.node(NodeKind::Ident(segs[0].name), lo));
        }
        let mut ids = Vec::with_capacity(segs.len());
        for seg in &segs {
            let span = crate::diag::Span::new(self.file, seg.lo, seg.hi);
            ids.push(self.ast.push(NodeKind::PathSegment { name: seg.name, args: seg.args }, span));
        }
        let segments = self.list(&ids);
        Ok(self.node(if ty { NodeKind::TyPath { segments } } else { NodeKind::Path { segments } }, lo))
    }

    /// After a path, at `{`: is this `Name { field: .. }` rather than a block?
    fn looks_like_struct_lit(&self) -> bool {
        match self.peek_at(1) {
            TokenKind::RBrace => true,
            TokenKind::Ident(_) => {
                matches!(self.peek_at(2), TokenKind::Colon | TokenKind::Comma | TokenKind::RBrace)
            }
            _ => false,
        }
    }

    fn parse_struct_lit(&mut self, path: NodeId, lo: u32) -> PResult<NodeId> {
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
            let field_lo = self.cur_lo();
            let (name, name_span) = self.expect_ident()?;
            let value = if self.eat(TokenKind::Colon) {
                self.parse_expr_free()?
            } else {
                self.ast.push(NodeKind::Ident(name), name_span)
            };
            fields.push(self.node(NodeKind::FieldInit { name, value }, field_lo));
            if self.eat(TokenKind::Comma) {
                continue;
            }
            self.expect(TokenKind::RBrace)?;
            break;
        }
        let fields = self.list(&fields);
        Ok(self.node(NodeKind::StructLit { path, fields }, lo))
    }

    fn parse_set_lit(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        self.bump();
        self.expect(TokenKind::LBrace)?;
        let elems = self.parse_expr_list(TokenKind::RBrace)?;
        Ok(self.node(NodeKind::SetLit { elems }, lo))
    }

    /// At `{`: does it open a map literal (`{ key: value, .. }`) rather than a
    /// block? A block cannot start with `key :` because Neon has no labels, so
    /// one key-shaped atom followed by `:` decides it. A key atom is a literal,
    /// a (possibly `::`-qualified) name, or a parenthesized expression.
    /// `empty_is_map` says what `{}` means in this position.
    pub(super) fn brace_starts_map(&self, empty_is_map: bool) -> bool {
        if self.peek_at(1) == TokenKind::RBrace {
            return empty_is_map;
        }
        let mut i = 1;
        if self.peek_at(i) == TokenKind::Minus {
            i += 1;
        }
        match self.peek_at(i) {
            TokenKind::Ident(_) | TokenKind::Kw(Keyword::SelfType) => {
                i += 1;
                while self.peek_at(i) == TokenKind::ColonColon && matches!(self.peek_at(i + 1), TokenKind::Ident(_)) {
                    i += 2;
                }
            }
            TokenKind::Int(_)
            | TokenKind::Float(_)
            | TokenKind::Str(_)
            | TokenKind::Char(_)
            | TokenKind::Kw(Keyword::True | Keyword::False) => i += 1,
            TokenKind::LParen => {
                let mut depth = 0u32;
                loop {
                    match self.peek_at(i) {
                        TokenKind::LParen => depth += 1,
                        TokenKind::RParen => {
                            depth -= 1;
                            if depth == 0 {
                                i += 1;
                                break;
                            }
                        }
                        TokenKind::Eof => return false,
                        _ => {}
                    }
                    i += 1;
                }
            }
            _ => return false,
        }
        self.peek_at(i) == TokenKind::Colon
    }

    /// `{` in expression position: a map literal or a block.
    pub(super) fn parse_brace_expr(&mut self, empty_is_map: bool) -> PResult<NodeId> {
        if self.brace_starts_map(empty_is_map) {
            self.parse_map_lit()
        } else {
            self.parse_block()
        }
    }

    fn parse_map_lit(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        let open = self.expect(TokenKind::LBrace)?;
        let mut entries = Vec::new();
        loop {
            if self.eat(TokenKind::RBrace) {
                break;
            }
            if self.at(TokenKind::Eof) {
                self.unclosed(open, "`}`");
                break;
            }
            let entry_lo = self.cur_lo();
            let key = self.parse_expr_free()?;
            self.expect(TokenKind::Colon)?;
            let value = self.parse_expr_free()?;
            entries.push(self.node(NodeKind::MapEntry { key, value }, entry_lo));
            if self.eat(TokenKind::Comma) {
                continue;
            }
            self.expect(TokenKind::RBrace)?;
            break;
        }
        let entries = self.list(&entries);
        Ok(self.node(NodeKind::MapLit { entries }, lo))
    }

    /// `|a, b: i32| -> R => body`; the body is a block, a map, or an expression.
    fn parse_closure(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        let mut params = Vec::new();
        if !self.eat(TokenKind::PipePipe) {
            self.expect(TokenKind::Pipe)?;
            loop {
                if self.eat(TokenKind::Pipe) {
                    break;
                }
                let p_lo = self.cur_lo();
                let mutable = self.eat_kw(Keyword::Var);
                let (name, name_span) = self.expect_ident()?;
                let ty = if self.eat(TokenKind::Colon) { Some(self.parse_type()?) } else { None };
                params.push(self.node_n(name_span, NodeKind::Param { name, mutable, ty: self.opt(ty) }, p_lo));
                if self.eat(TokenKind::Comma) {
                    continue;
                }
                self.expect(TokenKind::Pipe)?;
                break;
            }
        }
        let ret = if self.eat(TokenKind::Arrow) { Some(self.parse_type()?) } else { None };
        self.expect(TokenKind::FatArrow)?;
        let body = self.parse_arm_body()?;
        let params = self.list(&params);
        Ok(self.node(NodeKind::Closure { params, ret: self.opt(ret), body }, lo))
    }

    /// The body after `=>`: an empty `{}` is a block here, not a map.
    pub(super) fn parse_arm_body(&mut self) -> PResult<NodeId> {
        if self.at(TokenKind::LBrace) {
            self.parse_brace_expr(false)
        } else {
            self.parse_expr()
        }
    }

    pub(super) fn parse_if(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        self.bump();
        let cond = self.with_no_struct(true, |p| p.parse_expr())?;
        let then_branch = self.parse_block()?;
        let else_branch = if self.at_kw(Keyword::Elif) {
            Some(self.parse_if()?)
        } else if self.eat_kw(Keyword::Else) {
            if self.at_kw(Keyword::If) {
                // `else if` is spelled `elif`; accept it after reporting so the rest still parses
                let span = self.tok().span;
                self.diags.report(
                    Diagnostic::error("`else if` is spelled `elif`")
                        .with_code("E0110")
                        .with_label(span, "write `elif` instead of `else if`".to_string()),
                );
                Some(self.parse_if()?)
            } else {
                Some(self.parse_block()?)
            }
        } else {
            None
        };
        Ok(self.node(NodeKind::If { cond, then_branch, else_branch: self.opt(else_branch) }, lo))
    }

    pub(super) fn parse_while(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        self.bump();
        let cond = self.with_no_struct(true, |p| p.parse_expr())?;
        let body = self.parse_block()?;
        Ok(self.node(NodeKind::While { cond, body }, lo))
    }

    pub(super) fn parse_loop(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        self.bump();
        let body = self.parse_block()?;
        Ok(self.node(NodeKind::Loop { body }, lo))
    }

    pub(super) fn parse_for(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        self.bump();
        let pat = self.parse_pattern()?;
        self.expect_kw(Keyword::In)?;
        let iter = self.with_no_struct(true, |p| p.parse_expr())?;
        let body = self.parse_block()?;
        Ok(self.node(NodeKind::For { pat, iter, body }, lo))
    }

    pub(super) fn parse_match(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        self.bump();
        let scrutinee = self.with_no_struct(true, |p| p.parse_expr())?;
        let open = self.expect(TokenKind::LBrace)?;
        let saved = std::mem::replace(&mut self.no_struct, false);
        let mut arms = Vec::new();
        loop {
            if self.eat(TokenKind::RBrace) {
                break;
            }
            if self.at(TokenKind::Eof) {
                self.unclosed(open, "`}`");
                break;
            }
            let start = self.pos;
            match self.parse_arm() {
                Ok(arm) => arms.push(arm),
                Err(Recover) => arms.push(self.recover_node(start, true)),
            }
        }
        self.no_struct = saved;
        let arms = self.list(&arms);
        Ok(self.node(NodeKind::Match { scrutinee, arms }, lo))
    }

    fn parse_arm(&mut self) -> PResult<NodeId> {
        let lo = self.cur_lo();
        let pat = self.parse_pattern()?;
        let guard = if self.eat_kw(Keyword::If) { Some(self.parse_expr()?) } else { None };
        self.expect(TokenKind::FatArrow)?;
        let body = self.parse_arm_body()?;
        // Arms are separated by `,`, which is optional after a block-like body.
        if !self.eat(TokenKind::Comma) && !self.at(TokenKind::RBrace) && !self.is_block_like_expr(body) {
            let span = self.tokens[self.pos - 1].span.shrink_to_hi();
            let _ = self.error_at(
                span,
                "E0101",
                format!("expected `,` after match arm, found {}", self.describe_found()),
                "expected `,`".to_string(),
            );
        }
        Ok(self.node(NodeKind::MatchArm { pat, guard: self.opt(guard), body }, lo))
    }
}
