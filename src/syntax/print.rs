//! Compact S-expression rendering of an [`Ast`], for tests and debugging.
//!
//! Expressions print as `(op args..)` (`(+ a b)`, `(call f x)`), leaves print
//! as source-like text (`x`, `5u8`, `"s"`), absent optional parts are simply
//! omitted, and statements are `(stmt X)` (terminated by `;`) or `(stmt- X)`.

use std::fmt::Write;

use super::ast::{Ast, NodeId, NodeKind, NodeList};
use super::interner::{Interner, Symbol};

/// Renders the subtree rooted at `id`.
pub fn sexpr(ast: &Ast, interner: &Interner, id: NodeId) -> String {
    let mut p = Printer { ast, interner, out: String::new() };
    p.node(id);
    p.out
}

/// Renders a whole module.
pub fn sexpr_module(ast: &Ast, interner: &Interner) -> String {
    sexpr(ast, interner, ast.root())
}

struct Printer<'a> {
    ast: &'a Ast,
    interner: &'a Interner,
    out: String,
}

impl Printer<'_> {
    fn sym(&mut self, s: Symbol) {
        self.out.push_str(self.interner.resolve(s));
    }

    fn text(&mut self, s: &str) {
        self.out.push_str(s);
    }

    fn list(&mut self, list: NodeList) {
        for &id in self.ast.list(list) {
            self.out.push(' ');
            self.node(id);
        }
    }

    fn opt(&mut self, id: super::ast::OptionNodeId) {
        if let Some(id) = id.get() {
            self.out.push(' ');
            self.node(id);
        }
    }

    fn open(&mut self, head: &str) {
        self.out.push('(');
        self.out.push_str(head);
    }

    fn close(&mut self) {
        self.out.push(')');
    }

    /// `(head child child ..)`
    fn form(&mut self, head: &str, children: &[NodeId]) {
        self.open(head);
        for &c in children {
            self.out.push(' ');
            self.node(c);
        }
        self.close();
    }

    fn comma_list(&mut self, list: NodeList, sep: &str) {
        for (i, &id) in self.ast.list(list).iter().enumerate() {
            if i > 0 {
                self.out.push_str(sep);
            }
            self.node(id);
        }
    }

    fn generics(&mut self, list: NodeList) {
        if !list.is_empty() {
            self.out.push_str(" <");
            self.comma_list(list, ", ");
            self.out.push('>');
        }
    }

    fn segments(&mut self, list: NodeList) {
        self.comma_list(list, "::");
    }

    fn node(&mut self, id: NodeId) {
        use NodeKind::*;
        let kind = *self.ast.kind(id);
        match kind {
            Module { items } => {
                self.open("module");
                self.list(items);
                self.close();
            }
            ImportNamed { names, path } => {
                self.open("import");
                let _ = write!(self.out, " {:?}", self.ast.lits.str(path));
                self.list(names);
                self.close();
            }
            ImportName { name, alias } => {
                self.sym(name);
                if let Some(a) = alias {
                    self.out.push(':');
                    self.sym(a);
                }
            }
            ImportAlias { path, alias } => {
                self.open("import");
                let _ = write!(self.out, " {:?} as ", self.ast.lits.str(path));
                self.sym(alias);
                self.close();
            }
            Export { item } => self.form("export", &[item]),
            FnDecl { name, generics, params, ret, body } => {
                self.open("fn ");
                self.sym(name);
                self.generics(generics);
                self.text(" (");
                self.comma_list(params, " ");
                self.text(")");
                if let Some(r) = ret.get() {
                    self.text(" -> ");
                    self.node(r);
                }
                match body.get() {
                    Some(b) => {
                        self.out.push(' ');
                        self.node(b);
                    }
                    None => self.text(" ;"),
                }
                self.close();
            }
            GenericParam { name, bounds } => {
                self.sym(name);
                if !bounds.is_empty() {
                    self.text(": ");
                    self.comma_list(bounds, " + ");
                }
            }
            Param { name, mutable, ty } => {
                if mutable {
                    self.text("var ");
                }
                self.sym(name);
                if let Some(t) = ty.get() {
                    self.text(":");
                    self.node(t);
                }
            }
            StructDecl { name, generics, fields } => {
                self.open("struct ");
                self.sym(name);
                self.generics(generics);
                self.text(" (");
                self.comma_list(fields, " ");
                self.text(")");
                self.close();
            }
            FieldDecl { name, ty } => {
                self.sym(name);
                self.text(":");
                self.node(ty);
            }
            EnumDecl { name, generics, variants } => {
                self.open("enum ");
                self.sym(name);
                self.generics(generics);
                self.text(" (");
                self.comma_list(variants, " ");
                self.text(")");
                self.close();
            }
            Variant { name, fields } => {
                self.sym(name);
                if !fields.is_empty() {
                    self.text("(");
                    self.comma_list(fields, ", ");
                    self.text(")");
                }
            }
            TraitDecl { name, generics, items } => {
                self.open("trait ");
                self.sym(name);
                self.generics(generics);
                self.list(items);
                self.close();
            }
            ImplDecl { generics, trait_ref, self_ty, items } => {
                self.open("impl");
                self.generics(generics);
                if let Some(t) = trait_ref.get() {
                    self.out.push(' ');
                    self.node(t);
                    self.text(" for");
                }
                self.out.push(' ');
                self.node(self_ty);
                self.list(items);
                self.close();
            }
            ConstDecl { name, ty, init } => {
                self.open("const ");
                self.sym(name);
                if let Some(t) = ty.get() {
                    self.text(":");
                    self.node(t);
                }
                self.out.push(' ');
                self.node(init);
                self.close();
            }
            Let { pat, ty, init } => {
                self.open("let ");
                self.node(pat);
                if let Some(t) = ty.get() {
                    self.text(":");
                    self.node(t);
                }
                self.opt(init);
                self.close();
            }
            ExprStmt { expr, semi } => self.form(if semi { "stmt" } else { "stmt-" }, &[expr]),
            IntLit(i) => {
                let lit = self.ast.lits.int(i);
                let _ = write!(self.out, "{}", lit.value);
                if let Some(s) = lit.suffix {
                    self.text(s.as_str());
                }
            }
            FloatLit(f) => {
                let lit = self.ast.lits.float(f);
                let _ = write!(self.out, "{:?}", lit.value);
                if let Some(s) = lit.suffix {
                    self.text(s.as_str());
                }
            }
            StrLit(s) => {
                let _ = write!(self.out, "{:?}", self.ast.lits.str(s));
            }
            FString { parts } => {
                self.open("fstring ");
                self.comma_list(parts, " ");
                self.close();
            }
            FStrArg { expr, spec } => {
                self.open("arg ");
                self.node(expr);
                if !spec.is_empty() {
                    self.text(" : ");
                    self.comma_list(spec, " ");
                }
                self.close();
            }
            CharLit(c) => {
                let _ = write!(self.out, "{c:?}");
            }
            BoolLit(b) => self.text(if b { "true" } else { "false" }),
            UnitLit => self.text("()"),
            Ident(s) => self.sym(s),
            Path { segments } => self.segments(segments),
            PathSegment { name, args } => {
                self.sym(name);
                if !args.is_empty() {
                    self.text("<");
                    self.comma_list(args, ", ");
                    self.text(">");
                }
            }
            Unary { op, operand } => self.form(op.as_str(), &[operand]),
            Binary { op, lhs, rhs } => self.form(op.as_str(), &[lhs, rhs]),
            Assign { op, target, value } => {
                let head = match op {
                    Some(op) => format!("{}=", op.as_str()),
                    None => "=".to_string(),
                };
                self.form(&head, &[target, value]);
            }
            Cast { expr, ty } => self.form("as", &[expr, ty]),
            Call { callee, args } => {
                self.open("call ");
                self.node(callee);
                self.list(args);
                self.close();
            }
            MethodCall { receiver, name, generics, args } => {
                self.open("mcall ");
                self.node(receiver);
                self.out.push(' ');
                self.sym(name);
                if !generics.is_empty() {
                    self.text("<");
                    self.comma_list(generics, ", ");
                    self.text(">");
                }
                self.list(args);
                self.close();
            }
            Field { base, name } => {
                self.open("field ");
                self.node(base);
                self.out.push(' ');
                self.sym(name);
                self.close();
            }
            Index { base, index } => self.form("index", &[base, index]),
            Try { expr } => self.form("try", &[expr]),
            Range { start, end, inclusive } => {
                self.open(if inclusive { "..=" } else { ".." });
                for part in [start, end] {
                    match part.get() {
                        Some(p) => {
                            self.out.push(' ');
                            self.node(p);
                        }
                        None => self.text(" _"),
                    }
                }
                self.close();
            }
            ArrayLit { elems } => {
                self.open("array");
                self.list(elems);
                self.close();
            }
            MapLit { entries } => {
                self.open("map");
                self.list(entries);
                self.close();
            }
            MapEntry { key, value } => {
                self.out.push('(');
                self.node(key);
                self.out.push(' ');
                self.node(value);
                self.close();
            }
            SetLit { elems } => {
                self.open("set");
                self.list(elems);
                self.close();
            }
            StructLit { path, fields } => {
                self.open("struct-lit ");
                self.node(path);
                self.list(fields);
                self.close();
            }
            FieldInit { name, value } => {
                self.out.push('(');
                self.sym(name);
                self.out.push(' ');
                self.node(value);
                self.close();
            }
            Closure { params, ret, body } => {
                self.open("closure (");
                self.comma_list(params, " ");
                self.text(")");
                if let Some(r) = ret.get() {
                    self.text(" -> ");
                    self.node(r);
                }
                self.out.push(' ');
                self.node(body);
                self.close();
            }
            Block { stmts, tail } => {
                self.open("block");
                self.list(stmts);
                if let Some(t) = tail.get() {
                    self.text(" (tail ");
                    self.node(t);
                    self.text(")");
                }
                self.close();
            }
            If { cond, then_branch, else_branch } => {
                self.open("if ");
                self.node(cond);
                self.out.push(' ');
                self.node(then_branch);
                self.opt(else_branch);
                self.close();
            }
            While { cond, body } => self.form("while", &[cond, body]),
            Loop { body } => self.form("loop", &[body]),
            For { pat, iter, body } => self.form("for", &[pat, iter, body]),
            Match { scrutinee, arms } => {
                self.open("match ");
                self.node(scrutinee);
                self.list(arms);
                self.close();
            }
            MatchArm { pat, guard, body } => {
                self.open("arm ");
                self.node(pat);
                if let Some(g) = guard.get() {
                    self.text(" if ");
                    self.node(g);
                }
                self.out.push(' ');
                self.node(body);
                self.close();
            }
            Return { value } => {
                self.open("return");
                self.opt(value);
                self.close();
            }
            Break { value } => {
                self.open("break");
                self.opt(value);
                self.close();
            }
            Continue => self.text("(continue)"),
            PatWild => self.text("_"),
            PatBinding { name, mutable } => {
                if mutable {
                    self.text("var ");
                }
                self.sym(name);
            }
            PatLit { expr } => self.node(expr),
            PatRange { lo, hi, inclusive } => self.form(if inclusive { "..=" } else { ".." }, &[lo, hi]),
            PatVariant { path, fields, parens } => {
                if parens {
                    self.open("variant ");
                    self.node(path);
                    self.list(fields);
                    self.close();
                } else {
                    self.node(path);
                }
            }
            PatOr { alts } => {
                self.open("or");
                self.list(alts);
                self.close();
            }
            TyPrim(k) => self.text(k.as_str()),
            TyPath { segments } => self.segments(segments),
            TySelf => self.text("Self"),
            TyArray { elem } => {
                self.text("[");
                self.node(elem);
                self.text("]");
            }
            TyMap { key, value } => {
                self.text("{");
                self.node(key);
                self.text(": ");
                self.node(value);
                self.text("}");
            }
            TySet { elem } => {
                self.text("set<");
                self.node(elem);
                self.text(">");
            }
            TyFn { params, ret } => {
                self.text("fn(");
                self.comma_list(params, ", ");
                self.text(")");
                if let Some(r) = ret.get() {
                    self.text(" -> ");
                    self.node(r);
                }
            }
            TyUnit => self.text("()"),
            Error => self.text("<error>"),
        }
    }
}
