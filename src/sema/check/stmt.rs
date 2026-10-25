//! Statements, blocks and module-level declarations.

use super::{Checker, PatCtx};
use crate::diag::Span;
use crate::sema::collect::insert_item;
use crate::sema::ids::*;
use crate::sema::program::*;
use crate::sema::types::Ty;
use crate::syntax::{NodeId, NodeKind, Symbol};

impl Checker<'_> {
    /// Unifies a function body's type with its declared return type.
    pub fn coerce_body(&mut self, body: NodeId, ret: Ty, t: Ty) {
        if self.is_never(t) {
            return;
        }
        let ast = self.ast.clone();
        let span = match *ast.kind(body) {
            NodeKind::Block { tail, .. } => match tail.get() {
                Some(t) => ast.span(t),
                None => {
                    let s = ast.span(body);
                    Span::new(s.file, s.hi.saturating_sub(1), s.hi)
                }
            },
            _ => ast.span(body),
        };
        self.unify_at(span, ret, t);
    }

    /// Checks a `Block` node and returns the type of its value.
    pub fn block_value(&mut self, block: NodeId, expected: Option<Ty>) -> Ty {
        let ast = self.ast.clone();
        let NodeKind::Block { stmts, tail } = *ast.kind(block) else {
            // `if`/`while` bodies are always blocks; be lenient with error nodes
            let t = self.expr(block, expected);
            return t;
        };
        self.push_scope();
        let mut diverges = false;
        for &s in ast.list(stmts) {
            if self.stmt(s) {
                diverges = true;
            }
        }
        let ty = match tail.get() {
            Some(t) => self.expr(t, expected),
            None if diverges => self.prog.types.never(),
            None => self.unit(),
        };
        self.pop_scope();
        self.set_ty(block, ty);
        ty
    }

    /// Checks one statement; returns whether it never completes normally.
    pub fn stmt(&mut self, s: NodeId) -> bool {
        let ast = self.ast.clone();
        match *ast.kind(s) {
            NodeKind::Let { pat, ty, init } => {
                self.local_let(s, pat, ty.get(), init.get());
                false
            }
            NodeKind::ExprStmt { expr, .. } => {
                let t = self.expr(expr, None);
                self.is_never(t)
            }
            NodeKind::Error => false,
            NodeKind::ImportNamed { .. } | NodeKind::ImportAlias { .. } => {
                self.err(
                    ast.span(s),
                    "E0303",
                    "imports must appear at the top level of a module",
                    "not at module level",
                );
                false
            }
            NodeKind::FnDecl { .. }
            | NodeKind::StructDecl { .. }
            | NodeKind::EnumDecl { .. }
            | NodeKind::TraitDecl { .. }
            | NodeKind::ImplDecl { .. }
            | NodeKind::ConstDecl { .. }
            | NodeKind::Export { .. } => {
                self.err(
                    ast.span(s),
                    "E0223",
                    "declarations are only allowed at the top level of a module",
                    "move this out of the block (use a closure for a local function)",
                );
                false
            }
            _ => {
                self.expr(s, None);
                false
            }
        }
    }

    fn local_let(&mut self, node: NodeId, pat: NodeId, ty: Option<NodeId>, init: Option<NodeId>) {
        let ann = ty.map(|t| self.resolve_type(t));
        let decl = self.let_type(node, ann, init);
        let mut ctx = PatCtx::default();
        self.pattern(pat, decl, &mut ctx);
        if ctx.refutable {
            let span = self.span(pat);
            self.err(span, "E0230", "refutable pattern in `let` binding", "pattern can fail to match; use `match`");
        }
    }

    /// Types the initializer of a `let`/`const`, returning the declared type.
    fn let_type(&mut self, node: NodeId, ann: Option<Ty>, init: Option<NodeId>) -> Ty {
        match init {
            Some(i) => {
                let t = self.expr(i, ann);
                if let Some(a) = ann {
                    let span = self.span(i);
                    self.unify_at(span, a, t);
                }
                ann.unwrap_or(t)
            }
            None => {
                let span = self.span(node);
                self.err(span, "E0229", "variables must be initialized where they are declared", "add `= value`");
                ann.unwrap_or_else(|| self.error_ty())
            }
        }
    }

    // ---- module level ---------------------------------------------------------------------

    pub fn top_level_item(&mut self, item: NodeId) {
        let ast = self.ast.clone();
        match *ast.kind(item) {
            NodeKind::Export { item: inner } => {
                if let NodeKind::ConstDecl { .. } = ast.kind(inner) {
                    self.const_decl(inner, true);
                }
            }
            NodeKind::ConstDecl { .. } => self.const_decl(item, false),
            NodeKind::Let { pat, ty, init } => self.global_let(item, pat, ty.get(), init.get()),
            NodeKind::ExprStmt { .. } => {
                self.stmt(item);
            }
            _ => {}
        }
    }

    fn const_decl(&mut self, node: NodeId, exported: bool) {
        let ast = self.ast.clone();
        let NodeKind::ConstDecl { name, ty, init } = *ast.kind(node) else { return };
        let ann = ty.get().map(|t| self.resolve_type(t));
        let t = self.let_type(node, ann, Some(init));
        self.declare_global(node, name, ast.name_span(node), t, false, true, exported);
    }

    fn global_let(&mut self, node: NodeId, pat: NodeId, ty: Option<NodeId>, init: Option<NodeId>) {
        let ast = self.ast.clone();
        let ann = ty.map(|t| self.resolve_type(t));
        let t = self.let_type(node, ann, init);
        match *ast.kind(pat) {
            NodeKind::PatBinding { name, mutable } => {
                self.set_ty(pat, t);
                self.declare_global(node, name, ast.name_span(pat), t, mutable, false, false);
            }
            NodeKind::PatWild => self.set_ty(pat, t),
            _ => {
                self.err(ast.span(pat), "E0230", "top-level `let` must bind a single name", "use a simple name or `_`");
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn declare_global(
        &mut self,
        node: NodeId,
        name: Symbol,
        span: Span,
        ty: Ty,
        mutable: bool,
        is_const: bool,
        exported: bool,
    ) {
        let id = GlobalId(self.prog.globals.len() as u32);
        let def = self.prog.symbols.add_def(name, DefKind::Global(id), span, self.module);
        self.prog.globals.push(GlobalDef {
            name,
            module: self.module,
            def,
            name_span: span,
            ty,
            mutable,
            is_const,
            exported,
        });
        self.new_globals.push(id);
        self.info().globals.insert(node, id);
        insert_item(self.prog, self.interner, self.diags, self.module, name, Item::Global(id), span, exported);
    }
}
