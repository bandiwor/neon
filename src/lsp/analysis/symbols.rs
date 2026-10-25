//! `textDocument/documentSymbol`: the outline of a module.

use super::Analysis;
use crate::lsp::json::Json;
use crate::syntax::{NodeId, NodeKind};

// LSP SymbolKind numbers
const CLASS: u32 = 5;
const METHOD: u32 = 6;
const ENUM: u32 = 10;
const INTERFACE: u32 = 11;
const FUNCTION: u32 = 12;
const VARIABLE: u32 = 13;
const CONSTANT: u32 = 14;
const FIELD: u32 = 8;
const ENUM_MEMBER: u32 = 22;
const STRUCT: u32 = 23;
const NAMESPACE: u32 = 3;

impl Analysis {
    pub fn document_symbols(&self) -> Json {
        let ast = self.prog.module(self.module).ast.clone();
        let NodeKind::Module { items } = *ast.kind(ast.root()) else { return Json::Arr(vec![]) };
        let symbols: Vec<Json> = ast.list(items).iter().filter_map(|&n| self.symbol_of(n, false)).collect();
        Json::Arr(symbols)
    }

    fn symbol(&self, node: NodeId, name: String, detail: &str, kind: u32, children: Vec<Json>) -> Json {
        let ast = &self.prog.module(self.module).ast;
        let mut fields = vec![
            ("name", Json::from(name)),
            ("kind", Json::from(kind)),
            ("range", self.range(ast.span(node))),
            ("selectionRange", self.range(ast.name_span(node))),
        ];
        if !detail.is_empty() {
            fields.push(("detail", detail.into()));
        }
        if !children.is_empty() {
            fields.push(("children", Json::Arr(children)));
        }
        Json::object(fields)
    }

    fn symbol_of(&self, node: NodeId, in_impl: bool) -> Option<Json> {
        let ast = self.prog.module(self.module).ast.clone();
        let name_of = |s| self.interner.resolve(s).to_string();
        match *ast.kind(node) {
            NodeKind::Export { item } => self.symbol_of(item, in_impl),
            NodeKind::FnDecl { name, .. } => {
                let detail = self
                    .prog
                    .fns
                    .iter()
                    .position(|f| f.module == self.module && f.node == node)
                    .map(|i| self.signature(crate::sema::FnId(i as u32)).label)
                    .unwrap_or_default();
                Some(self.symbol(node, name_of(name), &detail, if in_impl { METHOD } else { FUNCTION }, vec![]))
            }
            NodeKind::StructDecl { name, fields, .. } => {
                let children = ast
                    .list(fields)
                    .iter()
                    .filter_map(|&f| match *ast.kind(f) {
                        NodeKind::FieldDecl { name, .. } => Some(self.symbol(f, name_of(name), "", FIELD, vec![])),
                        _ => None,
                    })
                    .collect();
                Some(self.symbol(node, name_of(name), "struct", STRUCT, children))
            }
            NodeKind::EnumDecl { name, variants, .. } => {
                let children = ast
                    .list(variants)
                    .iter()
                    .filter_map(|&v| match *ast.kind(v) {
                        NodeKind::Variant { name, .. } => Some(self.symbol(v, name_of(name), "", ENUM_MEMBER, vec![])),
                        _ => None,
                    })
                    .collect();
                Some(self.symbol(node, name_of(name), "enum", ENUM, children))
            }
            NodeKind::TraitDecl { name, items, .. } => {
                let children = ast.list(items).iter().filter_map(|&m| self.symbol_of(m, true)).collect();
                Some(self.symbol(node, name_of(name), "trait", INTERFACE, children))
            }
            NodeKind::ImplDecl { items, self_ty, trait_ref, .. } => {
                let text = |n: NodeId| {
                    let s = ast.span(n);
                    self.map.get(s.file).and_then(|f| f.slice(s.lo, s.hi)).unwrap_or("?").to_string()
                };
                let name = match trait_ref.get() {
                    Some(t) => format!("impl {} for {}", text(t), text(self_ty)),
                    None => format!("impl {}", text(self_ty)),
                };
                let children = ast.list(items).iter().filter_map(|&m| self.symbol_of(m, true)).collect();
                let ast_span = ast.span(node);
                // the selection is the type the impl is for
                let mut sym = self.symbol(node, name, "", CLASS, children);
                if let Json::Obj(fields) = &mut sym {
                    for (k, v) in fields.iter_mut() {
                        if k == "selectionRange" {
                            *v = self.range(ast.span(self_ty));
                        }
                    }
                }
                let _ = ast_span;
                Some(sym)
            }
            NodeKind::ConstDecl { name, .. } => Some(self.symbol(node, name_of(name), "const", CONSTANT, vec![])),
            NodeKind::Let { pat, .. } => match *ast.kind(pat) {
                NodeKind::PatBinding { name, mutable } => {
                    let mut sym =
                        self.symbol(node, name_of(name), if mutable { "var" } else { "let" }, VARIABLE, vec![]);
                    if let Json::Obj(fields) = &mut sym {
                        for (k, v) in fields.iter_mut() {
                            if k == "selectionRange" {
                                *v = self.range(ast.name_span(pat));
                            }
                        }
                    }
                    Some(sym)
                }
                _ => None,
            },
            NodeKind::ImportAlias { alias, .. } => Some(self.symbol(node, name_of(alias), "import", NAMESPACE, vec![])),
            _ => None,
        }
    }
}
