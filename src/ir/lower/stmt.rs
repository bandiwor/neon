//! Statements and module-level code.

use super::func::FnLower;
use crate::ir::types::*;
use crate::syntax::{NodeId, NodeKind};

impl FnLower<'_, '_> {
    pub(super) fn stmt(&mut self, s: NodeId) {
        match *self.ast.kind(s) {
            NodeKind::Let { pat, init, .. } => {
                let Some(init) = init.get() else { return };
                let v = self.expr(init);
                self.bind_irrefutable(pat, v);
            }
            NodeKind::ExprStmt { expr, .. } => {
                self.expr(expr);
            }
            // declarations are lowered through their own instances
            _ => {}
        }
    }

    /// The top-level code of a module: `let`/`const` initialize globals.
    pub(super) fn lower_module_init(&mut self, root: NodeId) {
        let ast = self.ast.clone();
        let NodeKind::Module { items } = *ast.kind(root) else { return };
        for &item in ast.list(items) {
            let item = match *ast.kind(item) {
                NodeKind::Export { item: inner } => inner,
                _ => item,
            };
            match *ast.kind(item) {
                NodeKind::Let { pat, init, .. } => {
                    let Some(init) = init.get() else { continue };
                    let v = self.expr(init);
                    if matches!(ast.kind(pat), NodeKind::PatBinding { .. }) {
                        self.store_global(item, v);
                    }
                }
                NodeKind::ConstDecl { init, .. } => {
                    let v = self.expr(init);
                    self.store_global(item, v);
                }
                NodeKind::ExprStmt { expr, .. } => {
                    self.expr(expr);
                }
                _ => {}
            }
        }
    }

    fn store_global(&mut self, decl: NodeId, v: VReg) {
        let Some(&g) = self.cx.prog.modules[self.module.index()].info.globals.get(&decl) else { return };
        let slot = self.cx.global_slot(g);
        if self.cx.out.globals[slot.index()].ty == self.f.reg_ty(v) {
            self.emit(Inst::SetGlobal { slot, src: v });
        }
    }
}
