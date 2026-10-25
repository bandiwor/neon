//! Human-readable descriptions of definitions, for hover, completion and signature help.

use super::Analysis;
use crate::sema::{
    builtin_method, AdtKind, Callee, DefId, DefKind, FnId, FnOwner, Generics, Item, ModuleId, TraitRef, TyKind,
};
use crate::syntax::NodeKind;

/// A function's signature split into the whole label and one label per parameter.
#[derive(Debug, Clone)]
pub struct Signature {
    pub label: String,
    pub params: Vec<String>,
}

impl Analysis {
    fn bounds_text(&self, bounds: &[TraitRef], generics: &Generics) -> String {
        bounds
            .iter()
            .map(|b| {
                let name = self.interner.resolve(self.prog.trait_def(b.trait_id).name);
                // trait arguments after `Self`
                let args: Vec<String> =
                    b.args.iter().map(|&a| self.prog.show_ty(&self.interner, a, generics)).collect();
                if args.is_empty() {
                    name.to_string()
                } else {
                    format!("{name}<{}>", args.join(", "))
                }
            })
            .collect::<Vec<_>>()
            .join(" + ")
    }

    /// `<T: Ord, U>` for the given generic parameters (empty text if none).
    fn generics_text(&self, own: &[crate::sema::GenericParam], all: &Generics) -> String {
        if own.is_empty() {
            return String::new();
        }
        let parts: Vec<String> = own
            .iter()
            .map(|p| {
                let name = self.interner.resolve(p.name);
                if p.bounds.is_empty() {
                    name.to_string()
                } else {
                    format!("{name}: {}", self.bounds_text(&p.bounds, all))
                }
            })
            .collect();
        format!("<{}>", parts.join(", "))
    }

    pub fn signature(&self, f: FnId) -> Signature {
        let fd = self.prog.func(f);
        let ast = &self.prog.module(fd.module).ast;
        let owner_generics = match fd.owner {
            FnOwner::Impl(i) => self.prog.impl_def(i).generics.len(),
            FnOwner::Trait(t) => self.prog.trait_def(t).generics.len(),
            _ => 0,
        };
        let own = &fd.generics.params[owner_generics.min(fd.generics.params.len())..];
        let param_nodes: Vec<_> = match ast.kind(fd.node) {
            NodeKind::FnDecl { params, .. } => ast.list(*params).to_vec(),
            _ => Vec::new(),
        };
        let mut params = Vec::new();
        for (i, &ty) in fd.params.iter().enumerate() {
            let (name, mutable) = match param_nodes.get(i).map(|&n| ast.kind(n)) {
                Some(NodeKind::Param { name, mutable, .. }) => (self.interner.resolve(*name).to_string(), *mutable),
                _ => (format!("arg{i}"), false),
            };
            let shown = self.prog.show_ty(&self.interner, ty, &fd.generics);
            let prefix = if mutable { "var " } else { "" };
            if fd.has_self && i == 0 {
                params.push(format!("{prefix}self"));
            } else {
                params.push(format!("{prefix}{name}: {shown}"));
            }
        }
        let ret = if matches!(self.prog.types.kind(fd.ret), TyKind::Unit) {
            String::new()
        } else {
            format!(" -> {}", self.prog.show_ty(&self.interner, fd.ret, &fd.generics))
        };
        let name = self.interner.resolve(fd.name);
        let g = self.generics_text(own, &fd.generics);
        Signature { label: format!("fn {name}{g}({}){ret}", params.join(", ")), params }
    }

    fn owner_note(&self, f: FnId) -> Option<String> {
        let fd = self.prog.func(f);
        match fd.owner {
            FnOwner::Impl(i) => {
                let imp = self.prog.impl_def(i);
                let ty = self.prog.show_ty(&self.interner, imp.self_ty, &imp.generics);
                Some(match &imp.trait_ref {
                    Some(t) => {
                        format!("// in `impl {} for {ty}`", self.interner.resolve(self.prog.trait_def(t.trait_id).name))
                    }
                    None => format!("// in `impl {ty}`"),
                })
            }
            FnOwner::Trait(t) => Some(format!("// in `trait {}`", self.interner.resolve(self.prog.trait_def(t).name))),
            _ => None,
        }
    }

    fn generics_for_node(&self, owner: crate::syntax::NodeId, module: ModuleId) -> Generics {
        self.prog
            .fns
            .iter()
            .find(|f| f.module == module && f.node == owner)
            .map(|f| f.generics.clone())
            .unwrap_or_default()
    }

    /// The text of a `const`'s initializer, shortened.
    fn const_init_text(&self, module: ModuleId, g: crate::sema::GlobalId) -> Option<String> {
        let m = self.prog.module(module);
        let node = m.info.globals.iter().find(|(_, &gid)| gid == g).map(|(&n, _)| n)?;
        let NodeKind::ConstDecl { init, .. } = *m.ast.kind(node) else { return None };
        let span = m.ast.span(init);
        let text = self.map.get(span.file)?.slice(span.lo, span.hi)?;
        let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
        Some(if one_line.chars().count() > 60 {
            format!("{}…", one_line.chars().take(60).collect::<String>())
        } else {
            one_line
        })
    }

    /// The hover text (a code snippet) for a definition.
    pub fn describe_def(&mut self, def: DefId) -> Option<String> {
        let d = self.prog.symbols.def(def).clone();
        let name = self.interner.resolve(d.name).to_string();
        Some(match d.kind {
            DefKind::Fn(f) => {
                let sig = self.signature(f).label;
                match self.owner_note(f) {
                    Some(note) => format!("{note}\n{sig}"),
                    None => sig,
                }
            }
            DefKind::Adt(a) => {
                let adt = self.prog.adt(a);
                let g = self.generics_text(&adt.generics.params, &adt.generics);
                match &adt.kind {
                    AdtKind::Struct { fields } => {
                        let mut lines: Vec<String> = fields
                            .iter()
                            .take(12)
                            .map(|f| {
                                format!(
                                    "    {}: {},",
                                    self.interner.resolve(f.name),
                                    self.prog.show_ty(&self.interner, f.ty, &adt.generics)
                                )
                            })
                            .collect();
                        if fields.len() > 12 {
                            lines.push("    ...".into());
                        }
                        if lines.is_empty() {
                            format!("struct {name}{g}")
                        } else {
                            format!("struct {name}{g} {{\n{}\n}}", lines.join("\n"))
                        }
                    }
                    AdtKind::Enum { variants } => {
                        let mut lines: Vec<String> =
                            variants.iter().take(12).map(|v| self.variant_text(a, v)).collect();
                        if variants.len() > 12 {
                            lines.push("    ...".into());
                        }
                        format!("enum {name}{g} {{\n{}\n}}", lines.join("\n"))
                    }
                }
            }
            DefKind::Variant(a, i) => {
                let adt = self.prog.adt(a);
                let v = &adt.variants()[i as usize];
                format!("{}::{}", self.interner.resolve(adt.name), self.variant_text(a, v).trim().trim_end_matches(','))
            }
            DefKind::Field(a, i) => {
                let adt = self.prog.adt(a);
                let f = &adt.fields()[i as usize];
                format!(
                    "// field of `{}`\n{}: {}",
                    self.interner.resolve(adt.name),
                    self.interner.resolve(f.name),
                    self.prog.show_ty(&self.interner, f.ty, &adt.generics)
                )
            }
            DefKind::Trait(t) => {
                let tr = self.prog.trait_def(t);
                // `Self` is the first parameter; show only the declared ones
                let declared = &tr.generics.params[1.min(tr.generics.params.len())..];
                let g = self.generics_text(declared, &tr.generics);
                let mut lines: Vec<String> =
                    tr.methods.iter().take(12).map(|&m| format!("    {};", self.signature(m).label)).collect();
                if tr.methods.len() > 12 {
                    lines.push("    ...".into());
                }
                format!("trait {name}{g} {{\n{}\n}}", lines.join("\n"))
            }
            DefKind::Global(g) => {
                let gd = self.prog.global(g);
                let kw = if gd.is_const {
                    "const"
                } else if gd.mutable {
                    "var"
                } else {
                    "let"
                };
                let ty = self.show_ty_plain(gd.ty).map(|t| format!(": {t}")).unwrap_or_default();
                let init = if gd.is_const {
                    self.const_init_text(gd.module, g).map(|t| format!(" = {t}")).unwrap_or_default()
                } else {
                    String::new()
                };
                format!("{kw} {name}{ty}{init}")
            }
            DefKind::Local(l) => {
                let ld = self.prog.local(l);
                let generics = self.generics_for_node(ld.owner, ld.module);
                let ty = self.show_ty_in(ld.ty, &generics).map(|t| format!(": {t}")).unwrap_or_default();
                format!("{} {name}{ty}", if ld.mutable { "var" } else { "let" })
            }
            DefKind::GenericParam => format!("// type parameter\n{name}"),
            DefKind::Module(m) => format!("module \"{}\"", self.prog.module(m).path),
            DefKind::ImportName => {
                // `import { name }`: describe what it names in the entry module's scope
                match self.prog.lookup_item(self.module, d.name) {
                    Some(Item::Fn(f)) => self.signature(f).label,
                    Some(Item::Adt(a)) => {
                        let def = self.prog.adt(a).def;
                        return self.describe_def(def);
                    }
                    Some(Item::Trait(t)) => {
                        let def = self.prog.trait_def(t).def;
                        return self.describe_def(def);
                    }
                    Some(Item::Global(g)) => {
                        let def = self.prog.global(g).def;
                        return self.describe_def(def);
                    }
                    None => format!("// imported\n{name}"),
                }
            }
        })
    }

    fn variant_text(&self, a: crate::sema::AdtId, v: &crate::sema::VariantDef) -> String {
        let adt = self.prog.adt(a);
        let name = self.interner.resolve(v.name);
        if v.fields.is_empty() {
            format!("    {name},")
        } else {
            let fs: Vec<String> =
                v.fields.iter().map(|&t| self.prog.show_ty(&self.interner, t, &adt.generics)).collect();
            format!("    {name}({}),", fs.join(", "))
        }
    }

    /// `[i64].push(i64)`-style text for a call of a built-in method.
    pub fn describe_builtin_call(&mut self, node: crate::syntax::NodeId) -> Option<String> {
        let callee = self.prog.modules[self.module.index()].info.callees.get(&node)?.clone();
        let Callee::Builtin(b, tys) = callee else { return None };
        let recv = *tys.first()?;
        let sig = builtin_method(&mut self.prog, recv, b.name())?;
        let recv_text = self.show_ty_plain(recv)?;
        let params: Vec<String> = sig.params.iter().filter_map(|&p| self.show_ty_plain(p)).collect();
        let ret = match self.show_ty_plain(sig.ret) {
            Some(r) if r != "()" => format!(" -> {r}"),
            _ => String::new(),
        };
        Some(format!("{recv_text}.{}({}){ret}", b.name(), params.join(", ")))
    }
}
