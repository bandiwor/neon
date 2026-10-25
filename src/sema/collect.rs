//! Declaration collection.
//!
//! * [`declare_module`]: registers every top-level item of a module under its
//!   name (types and signatures still empty) and processes its imports.
//! * [`resolve_signatures`]: fills in generics, field types, function
//!   signatures and impl headers, and validates impls against their traits.
//!
//! Diagnostic codes: E0201 duplicate name, E0206 impl method does not match
//! the trait, E0207 missing trait method, E0208 method not in trait,
//! E0209 misplaced `self`, E0210 trait/impl arity, E0304 missing export,
//! E0305 `export` of a non-item.

use super::ids::*;
use super::loader::LoadedModule;
use super::program::*;
use super::typeres::TypeResolver;
use super::types::Ty;
use crate::diag::{DiagBag, Diagnostic, Span};
use crate::syntax::{Ast, Interner, Keyword, NodeId, NodeKind, Symbol};

fn report_dup(diags: &mut DiagBag, interner: &Interner, name: Symbol, span: Span, prev: Span) {
    diags.report(
        Diagnostic::error(format!("the name `{}` is defined multiple times", interner.resolve(name)))
            .with_code("E0201")
            .with_label(span, "redefined here")
            .with_secondary(prev, "previous definition"),
    );
}

/// Inserts `item` into `module`'s scope, reporting a clash with an earlier
/// definition of the same name.
pub(super) fn insert_item(
    prog: &mut Program,
    interner: &Interner,
    diags: &mut DiagBag,
    module: ModuleId,
    name: Symbol,
    item: Item,
    span: Span,
    exported: bool,
) {
    if let Some(&prev) = prog.modules[module.index()].scope.items.get(&name) {
        let prev_def = prog.item_def(prev);
        let prev_span = prog.symbols.def(prev_def).span;
        report_dup(diags, interner, name, span, prev_span);
        return;
    }
    if let Some(&d) = prog.modules[module.index()]
        .scope
        .aliases
        .get(&name)
        .and_then(|_| prog.modules[module.index()].scope.alias_defs.get(&name))
    {
        let prev_span = prog.symbols.def(d).span;
        report_dup(diags, interner, name, span, prev_span);
        return;
    }
    let scope = &mut prog.modules[module.index()].scope;
    scope.items.insert(name, item);
    if exported {
        scope.exports.insert(name, item);
    }
}

fn new_fn(
    prog: &mut Program,
    ast: &Ast,
    module: ModuleId,
    node: NodeId,
    name: Symbol,
    owner: FnOwner,
    exported: bool,
) -> FnId {
    let id = FnId(prog.fns.len() as u32);
    let name_span = ast.name_span(node);
    let def = prog.symbols.add_def(name, DefKind::Fn(id), name_span, module);
    let body = match *ast.kind(node) {
        NodeKind::FnDecl { body, .. } => body.get(),
        _ => None,
    };
    prog.fns.push(FnDef {
        name,
        module,
        def,
        node,
        name_span,
        owner,
        generics: Generics::default(),
        params: Vec::new(),
        ret: prog.types.unit(),
        has_self: false,
        body,
        exported,
        param_locals: Vec::new(),
    });
    id
}

/// The generic parameters named by a decl's `GenericParam` nodes (bounds are
/// filled in later), registering a definition for each.
fn declare_generics(
    prog: &mut Program,
    ast: &Ast,
    module: ModuleId,
    list: crate::syntax::NodeList,
) -> Vec<GenericParam> {
    ast.list(list)
        .iter()
        .map(|&g| {
            let NodeKind::GenericParam { name, .. } = *ast.kind(g) else { unreachable!() };
            let span = ast.name_span(g);
            let def = prog.symbols.add_def(name, DefKind::GenericParam, span, module);
            GenericParam { name, def: Some(def), bounds: Vec::new(), span }
        })
        .collect()
}

/// Pass A (+ imports): registers the module and its top-level names.
pub fn declare_module(prog: &mut Program, interner: &mut Interner, diags: &mut DiagBag, lm: &LoadedModule) -> ModuleId {
    let module = ModuleId(prog.modules.len() as u32);
    let ast = lm.ast.clone();

    let init_name = interner.intern("<init>");
    let init = FnId(prog.fns.len() as u32);
    let init_def = prog.symbols.add_def(init_name, DefKind::Fn(init), Span::DUMMY, module);
    prog.fns.push(FnDef {
        name: init_name,
        module,
        def: init_def,
        node: ast.root(),
        name_span: Span::DUMMY,
        owner: FnOwner::ModuleInit,
        generics: Generics::default(),
        params: Vec::new(),
        ret: prog.types.unit(),
        has_self: false,
        body: Some(ast.root()),
        exported: false,
        param_locals: Vec::new(),
    });
    let info = ModuleInfo { node_types: vec![prog.types.error(); ast.len()], ..ModuleInfo::default() };
    prog.modules.push(Module {
        id: module,
        path: lm.path.clone(),
        file: lm.file,
        ast: ast.clone(),
        scope: ModuleScope::default(),
        info,
        init,
        imports: lm.imports.clone(),
    });

    let NodeKind::Module { items } = *ast.kind(ast.root()) else { unreachable!() };
    for &top in ast.list(items) {
        let (node, exported) = match *ast.kind(top) {
            NodeKind::Export { item } => (item, true),
            _ => (top, false),
        };
        declare_item(prog, interner, diags, &ast, module, node, exported);
    }
    declare_imports(prog, interner, diags, &ast, module, lm);
    module
}

fn declare_item(
    prog: &mut Program,
    interner: &Interner,
    diags: &mut DiagBag,
    ast: &Ast,
    module: ModuleId,
    node: NodeId,
    exported: bool,
) {
    let name_span = ast.name_span(node);
    match *ast.kind(node) {
        NodeKind::FnDecl { name, generics, .. } => {
            let id = new_fn(prog, ast, module, node, name, FnOwner::Free, exported);
            prog.fns[id.index()].generics.params = declare_generics(prog, ast, module, generics);
            let _ = generics;
            insert_item(prog, interner, diags, module, name, Item::Fn(id), name_span, exported);
        }
        NodeKind::StructDecl { name, generics, fields } => {
            let id = AdtId(prog.adts.len() as u32);
            let def = prog.symbols.add_def(name, DefKind::Adt(id), name_span, module);
            let gen = declare_generics(prog, ast, module, generics);
            let field_defs = ast
                .list(fields)
                .iter()
                .enumerate()
                .map(|(i, &f)| {
                    let NodeKind::FieldDecl { name, .. } = *ast.kind(f) else { unreachable!() };
                    let span = ast.name_span(f);
                    let fdef = prog.symbols.add_def(name, DefKind::Field(id, i as u32), span, module);
                    FieldDef { name, ty: prog.types.error(), span, def: fdef }
                })
                .collect();
            prog.adts.push(AdtDef {
                name,
                module,
                def,
                node,
                name_span,
                generics: Generics { params: gen },
                kind: AdtKind::Struct { fields: field_defs },
                exported,
            });
            insert_item(prog, interner, diags, module, name, Item::Adt(id), name_span, exported);
        }
        NodeKind::EnumDecl { name, generics, variants } => {
            let id = AdtId(prog.adts.len() as u32);
            let def = prog.symbols.add_def(name, DefKind::Adt(id), name_span, module);
            let gen = declare_generics(prog, ast, module, generics);
            let mut seen: Vec<Symbol> = Vec::new();
            let variant_defs = ast
                .list(variants)
                .iter()
                .enumerate()
                .map(|(i, &v)| {
                    let NodeKind::Variant { name: vname, .. } = *ast.kind(v) else { unreachable!() };
                    let span = ast.name_span(v);
                    if seen.contains(&vname) {
                        diags.report(
                            Diagnostic::error(format!(
                                "variant `{}` is defined multiple times",
                                interner.resolve(vname)
                            ))
                            .with_code("E0201")
                            .with_label(span, "duplicate variant"),
                        );
                    }
                    seen.push(vname);
                    let vdef = prog.symbols.add_def(vname, DefKind::Variant(id, i as u32), span, module);
                    VariantDef { name: vname, fields: Vec::new(), span, def: vdef }
                })
                .collect();
            prog.adts.push(AdtDef {
                name,
                module,
                def,
                node,
                name_span,
                generics: Generics { params: gen },
                kind: AdtKind::Enum { variants: variant_defs },
                exported,
            });
            insert_item(prog, interner, diags, module, name, Item::Adt(id), name_span, exported);
        }
        NodeKind::TraitDecl { name, generics, items } => {
            let id = TraitId(prog.traits.len() as u32);
            let def = prog.symbols.add_def(name, DefKind::Trait(id), name_span, module);
            let self_name = Symbol(Keyword::SelfType as u32);
            let mut gen = vec![GenericParam { name: self_name, def: None, bounds: Vec::new(), span: name_span }];
            gen.extend(declare_generics(prog, ast, module, generics));
            let mut methods = Vec::new();
            for &m in ast.list(items) {
                if let NodeKind::FnDecl { name: mname, generics: mgen, .. } = *ast.kind(m) {
                    let fid = new_fn(prog, ast, module, m, mname, FnOwner::Trait(id), exported);
                    let mut params = gen.clone();
                    params.extend(declare_generics(prog, ast, module, mgen));
                    prog.fns[fid.index()].generics.params = params;
                    methods.push(fid);
                }
            }
            prog.traits.push(TraitDef {
                name,
                module,
                def,
                node,
                name_span,
                generics: Generics { params: gen },
                methods,
                exported,
            });
            if module.0 == 0 {
                if let Some((kind, _)) = LangTrait::ALL.iter().find(|(_, n)| *n == interner.resolve(name)) {
                    prog.lang.by_kind.insert(*kind, id);
                    prog.lang.kinds.insert(id, *kind);
                }
            }
            insert_item(prog, interner, diags, module, name, Item::Trait(id), name_span, exported);
        }
        NodeKind::ImplDecl { generics, items, .. } => {
            let id = ImplId(prog.impls.len() as u32);
            let gen = declare_generics(prog, ast, module, generics);
            let mut methods = Vec::new();
            for &m in ast.list(items) {
                if let NodeKind::FnDecl { name: mname, generics: mgen, .. } = *ast.kind(m) {
                    let fid = new_fn(prog, ast, module, m, mname, FnOwner::Impl(id), false);
                    let mut params = gen.clone();
                    params.extend(declare_generics(prog, ast, module, mgen));
                    prog.fns[fid.index()].generics.params = params;
                    methods.push(fid);
                }
            }
            prog.impls.push(ImplDef {
                module,
                node,
                span: ast.span(node),
                generics: Generics { params: gen },
                trait_ref: None,
                self_ty: prog.types.error(),
                methods,
            });
        }
        NodeKind::ConstDecl { .. } | NodeKind::Let { .. } | NodeKind::ExprStmt { .. } | NodeKind::Error => {}
        NodeKind::ImportNamed { .. } | NodeKind::ImportAlias { .. } => {}
        _ => {
            if exported {
                diags.report(
                    Diagnostic::error("only functions, types, traits and constants can be exported")
                        .with_code("E0305")
                        .with_label(ast.span(node), "cannot export this"),
                );
            }
        }
    }
}

fn declare_imports(
    prog: &mut Program,
    interner: &Interner,
    diags: &mut DiagBag,
    ast: &Ast,
    module: ModuleId,
    lm: &LoadedModule,
) {
    for &(node, target) in &lm.imports {
        match *ast.kind(node) {
            NodeKind::ImportNamed { names, path } => {
                let path_text = ast.lits.str(path).to_string();
                for &n in ast.list(names) {
                    let NodeKind::ImportName { name, alias } = *ast.kind(n) else { continue };
                    let span = ast.name_span(n);
                    let Some(&item) = prog.modules[target.index()].scope.exports.get(&name) else {
                        diags.report(
                            Diagnostic::error(format!(
                                "module `{}` has no exported item `{}`",
                                path_text,
                                interner.resolve(name)
                            ))
                            .with_code("E0304")
                            .with_label(span, "not exported"),
                        );
                        continue;
                    };
                    let def = prog.item_def(item);
                    prog.symbols.add_use(span, def);
                    insert_item(prog, interner, diags, module, alias.unwrap_or(name), item, span, false);
                }
            }
            NodeKind::ImportAlias { alias, .. } => {
                let span = ast.name_span(node);
                if prog.modules[module.index()].scope.aliases.contains_key(&alias)
                    || prog.modules[module.index()].scope.items.contains_key(&alias)
                {
                    let prev = prog.modules[module.index()]
                        .scope
                        .alias_defs
                        .get(&alias)
                        .map(|&d| prog.symbols.def(d).span)
                        .or_else(|| {
                            prog.modules[module.index()]
                                .scope
                                .items
                                .get(&alias)
                                .map(|&i| prog.symbols.def(prog.item_def(i)).span)
                        })
                        .unwrap_or(span);
                    report_dup(diags, interner, alias, span, prev);
                    continue;
                }
                let def = prog.symbols.add_def(alias, DefKind::Module(target), span, module);
                let scope = &mut prog.modules[module.index()].scope;
                scope.aliases.insert(alias, target);
                scope.alias_defs.insert(alias, def);
            }
            _ => {}
        }
    }
}

// ---- pass B: signatures ------------------------------------------------------------------

/// Resolves the bounds of `generics_nodes` (the decl's `GenericParam` nodes)
/// into `params[offset..]`.
fn resolve_bounds(
    prog: &mut Program,
    interner: &Interner,
    diags: &mut DiagBag,
    ast: &Ast,
    module: ModuleId,
    all: &Generics,
    self_ty: Option<Ty>,
    nodes: crate::syntax::NodeList,
    offset: usize,
) -> Vec<(usize, Vec<TraitRef>)> {
    let mut out = Vec::new();
    for (i, &g) in ast.list(nodes).iter().enumerate() {
        let NodeKind::GenericParam { bounds, .. } = *ast.kind(g) else { continue };
        let mut refs = Vec::new();
        for &b in ast.list(bounds) {
            let mut tr = TypeResolver { prog, interner, diags, module, ast, generics: all, self_ty };
            if let Some(r) = tr.resolve_trait_ref(b) {
                refs.push(r);
            }
        }
        out.push((offset + i, refs));
    }
    out
}

fn assign_bounds(generics: &mut Generics, bounds: Vec<(usize, Vec<TraitRef>)>) {
    for (i, b) in bounds {
        generics.params[i].bounds.extend(b);
    }
}

/// Pass B for one module.
pub fn resolve_signatures(prog: &mut Program, interner: &Interner, diags: &mut DiagBag, module: ModuleId) {
    let ast = prog.modules[module.index()].ast.clone();
    let NodeKind::Module { items } = *ast.kind(ast.root()) else { unreachable!() };
    let mut adts = Vec::new();
    let mut traits = Vec::new();
    let mut impls = Vec::new();
    let mut fns = Vec::new();
    for (i, f) in prog.fns.iter().enumerate() {
        if f.module == module && f.owner == FnOwner::Free {
            fns.push(FnId(i as u32));
        }
    }
    for (i, a) in prog.adts.iter().enumerate() {
        if a.module == module {
            adts.push(AdtId(i as u32));
        }
    }
    for (i, t) in prog.traits.iter().enumerate() {
        if t.module == module {
            traits.push(TraitId(i as u32));
        }
    }
    for (i, im) in prog.impls.iter().enumerate() {
        if im.module == module {
            impls.push(ImplId(i as u32));
        }
    }
    let _ = items;

    for id in adts {
        resolve_adt(prog, interner, diags, &ast, module, id);
    }
    for id in traits {
        resolve_trait(prog, interner, diags, &ast, module, id);
    }
    for id in fns {
        let node = prog.fns[id.index()].node;
        resolve_fn_sig(prog, interner, diags, &ast, module, id, node, None);
    }
    for id in impls {
        resolve_impl(prog, interner, diags, &ast, module, id);
    }
}

fn resolve_adt(prog: &mut Program, interner: &Interner, diags: &mut DiagBag, ast: &Ast, module: ModuleId, id: AdtId) {
    let node = prog.adts[id.index()].node;
    let mut generics = prog.adts[id.index()].generics.clone();
    let (gen_nodes, own_self) = match *ast.kind(node) {
        NodeKind::StructDecl { generics, .. } | NodeKind::EnumDecl { generics, .. } => (generics, None),
        _ => unreachable!(),
    };
    let bounds = resolve_bounds(prog, interner, diags, ast, module, &generics, own_self, gen_nodes, 0);
    assign_bounds(&mut generics, bounds);

    let self_args: Vec<Ty> = (0..generics.len()).map(|i| prog.types.param(i as u32)).collect();
    let self_ty = prog.types.adt(id, self_args);

    match *ast.kind(node) {
        NodeKind::StructDecl { fields, .. } => {
            let mut tys = Vec::new();
            for &f in ast.list(fields) {
                let NodeKind::FieldDecl { ty, .. } = *ast.kind(f) else { unreachable!() };
                let mut tr =
                    TypeResolver { prog, interner, diags, module, ast, generics: &generics, self_ty: Some(self_ty) };
                tys.push(tr.resolve(ty));
            }
            if let AdtKind::Struct { fields } = &mut prog.adts[id.index()].kind {
                for (f, t) in fields.iter_mut().zip(tys) {
                    f.ty = t;
                }
            }
        }
        NodeKind::EnumDecl { variants, .. } => {
            let mut all: Vec<Vec<Ty>> = Vec::new();
            for &v in ast.list(variants) {
                let NodeKind::Variant { fields, .. } = *ast.kind(v) else { unreachable!() };
                let mut tys = Vec::new();
                for &t in ast.list(fields) {
                    let mut tr = TypeResolver {
                        prog,
                        interner,
                        diags,
                        module,
                        ast,
                        generics: &generics,
                        self_ty: Some(self_ty),
                    };
                    tys.push(tr.resolve(t));
                }
                all.push(tys);
            }
            if let AdtKind::Enum { variants } = &mut prog.adts[id.index()].kind {
                for (v, t) in variants.iter_mut().zip(all) {
                    v.fields = t;
                }
            }
        }
        _ => unreachable!(),
    }
    prog.adts[id.index()].generics = generics;
}

fn resolve_trait(
    prog: &mut Program,
    interner: &Interner,
    diags: &mut DiagBag,
    ast: &Ast,
    module: ModuleId,
    id: TraitId,
) {
    let node = prog.traits[id.index()].node;
    let NodeKind::TraitDecl { generics: gen_nodes, .. } = *ast.kind(node) else { unreachable!() };
    let mut generics = prog.traits[id.index()].generics.clone();
    let self_param = prog.types.param(0);
    let bounds = resolve_bounds(prog, interner, diags, ast, module, &generics, Some(self_param), gen_nodes, 1);
    assign_bounds(&mut generics, bounds);
    prog.traits[id.index()].generics = generics.clone();

    // `Self: ThisTrait<params..>` holds inside the trait's own methods.
    let own_args: Vec<Ty> = (1..generics.len()).map(|i| prog.types.param(i as u32)).collect();
    let methods = prog.traits[id.index()].methods.clone();
    for m in methods {
        for (i, p) in generics.params.iter().enumerate() {
            prog.fns[m.index()].generics.params[i].bounds = p.bounds.clone();
        }
        let node = prog.fns[m.index()].node;
        resolve_fn_sig(prog, interner, diags, ast, module, m, node, Some(self_param));
        prog.fns[m.index()].generics.params[0].bounds.push(TraitRef { trait_id: id, args: own_args.clone() });
    }
}

/// Resolves a function declaration's signature. `self_ty` is the type of
/// `Self` (an impl's self type, or the trait's `Self` parameter).
fn resolve_fn_sig(
    prog: &mut Program,
    interner: &Interner,
    diags: &mut DiagBag,
    ast: &Ast,
    module: ModuleId,
    id: FnId,
    node: NodeId,
    self_ty: Option<Ty>,
) {
    let NodeKind::FnDecl { generics: gen_nodes, params, ret, .. } = *ast.kind(node) else { return };
    let mut generics = prog.fns[id.index()].generics.clone();
    let own_start = generics.len() - ast.list(gen_nodes).len();
    let bounds = resolve_bounds(prog, interner, diags, ast, module, &generics, self_ty, gen_nodes, own_start);
    assign_bounds(&mut generics, bounds);

    let mut ptys = Vec::new();
    let mut has_self = false;
    for (i, &p) in ast.list(params).iter().enumerate() {
        let NodeKind::Param { name, ty, .. } = *ast.kind(p) else { continue };
        let is_self = name == Symbol(Keyword::SelfValue as u32);
        if is_self && i != 0 {
            diags.report(
                Diagnostic::error("`self` must be the first parameter")
                    .with_code("E0209")
                    .with_label(ast.name_span(p), "not the first parameter"),
            );
        }
        let t = match ty.get() {
            Some(t) => {
                let mut tr = TypeResolver { prog, interner, diags, module, ast, generics: &generics, self_ty };
                tr.resolve(t)
            }
            None if is_self => match self_ty {
                Some(s) => s,
                None => {
                    diags.report(
                        Diagnostic::error("`self` parameter is only allowed in methods")
                            .with_code("E0209")
                            .with_label(ast.name_span(p), "not in an impl or trait"),
                    );
                    prog.types.error()
                }
            },
            None => prog.types.error(),
        };
        if is_self && i == 0 {
            has_self = true;
        }
        ptys.push(t);
    }
    let ret_ty = match ret.get() {
        Some(r) => {
            let mut tr = TypeResolver { prog, interner, diags, module, ast, generics: &generics, self_ty };
            tr.resolve(r)
        }
        None => prog.types.unit(),
    };
    let f = &mut prog.fns[id.index()];
    f.generics = generics;
    f.params = ptys;
    f.ret = ret_ty;
    f.has_self = has_self;
}

fn resolve_impl(prog: &mut Program, interner: &Interner, diags: &mut DiagBag, ast: &Ast, module: ModuleId, id: ImplId) {
    let node = prog.impls[id.index()].node;
    let NodeKind::ImplDecl { generics: gen_nodes, trait_ref, self_ty, .. } = *ast.kind(node) else { unreachable!() };
    let mut generics = prog.impls[id.index()].generics.clone();

    // self type first (bounds may mention `Self`)
    let self_t = {
        let mut tr = TypeResolver { prog, interner, diags, module, ast, generics: &generics, self_ty: None };
        tr.resolve(self_ty)
    };
    let bounds = resolve_bounds(prog, interner, diags, ast, module, &generics, Some(self_t), gen_nodes, 0);
    assign_bounds(&mut generics, bounds);

    let trait_r = match trait_ref.get() {
        Some(t) => {
            let mut tr =
                TypeResolver { prog, interner, diags, module, ast, generics: &generics, self_ty: Some(self_t) };
            tr.resolve_trait_ref(t)
        }
        None => None,
    };

    {
        let imp = &mut prog.impls[id.index()];
        imp.generics = generics.clone();
        imp.self_ty = self_t;
        imp.trait_ref = trait_r.clone();
    }
    let methods = prog.impls[id.index()].methods.clone();
    for &m in &methods {
        let node = prog.fns[m.index()].node;
        // the impl's bounds are already part of the method's inherited generics
        let inherited = generics.len();
        let mut fg = prog.fns[m.index()].generics.clone();
        for (i, p) in generics.params.iter().enumerate() {
            fg.params[i].bounds = p.bounds.clone();
        }
        prog.fns[m.index()].generics = fg;
        let _ = inherited;
        resolve_fn_sig(prog, interner, diags, ast, module, m, node, Some(self_t));
    }

    match &trait_r {
        None => prog.impl_index.inherent.push(id),
        Some(tr) => {
            prog.impl_index.by_trait.entry(tr.trait_id).or_default().push(id);
            validate_impl(prog, interner, diags, ast, id, tr);
        }
    }
}

/// Checks an impl's methods against the trait it implements.
fn validate_impl(prog: &mut Program, interner: &Interner, diags: &mut DiagBag, ast: &Ast, id: ImplId, tr: &TraitRef) {
    let trait_name = interner.resolve(prog.traits[tr.trait_id.index()].name).to_string();
    let impl_span = prog.impls[id.index()].span;
    let impl_n = prog.impls[id.index()].generics.len();
    let self_ty = prog.impls[id.index()].self_ty;
    let trait_n = prog.traits[tr.trait_id.index()].generics.len();
    let trait_methods = prog.traits[tr.trait_id.index()].methods.clone();
    let impl_methods = prog.impls[id.index()].methods.clone();

    for &im in &impl_methods {
        let name = prog.fns[im.index()].name;
        let span = prog.fns[im.index()].name_span;
        let Some(&tm) = trait_methods.iter().find(|&&t| prog.fns[t.index()].name == name) else {
            diags.report(
                Diagnostic::error(format!(
                    "method `{}` is not a member of trait `{trait_name}`",
                    interner.resolve(name)
                ))
                .with_code("E0208")
                .with_label(span, "not in the trait"),
            );
            continue;
        };
        let own_impl = prog.fns[im.index()].generics.len() - impl_n;
        let own_trait = prog.fns[tm.index()].generics.len() - trait_n;
        if own_impl != own_trait {
            diags.report(
                Diagnostic::error(format!(
                    "method `{}` has {own_impl} generic parameter(s) but the trait declares {own_trait}",
                    interner.resolve(name)
                ))
                .with_code("E0206")
                .with_label(span, "generic parameter count differs"),
            );
            continue;
        }
        // trait signature with Self := the impl's type and trait params := the impl's trait args
        let mut subst: Vec<Ty> = vec![self_ty];
        subst.extend(tr.args.iter().copied());
        for j in 0..own_trait {
            subst.push(prog.types.param((impl_n + j) as u32));
        }
        let t_params = prog.fns[tm.index()].params.clone();
        let t_ret = prog.fns[tm.index()].ret;
        let exp_params: Vec<Ty> = t_params.iter().map(|&p| prog.types.subst(p, &subst)).collect();
        let exp_ret = prog.types.subst(t_ret, &subst);
        let got_params = prog.fns[im.index()].params.clone();
        let got_ret = prog.fns[im.index()].ret;
        if exp_params != got_params || exp_ret != got_ret {
            let g = prog.fns[im.index()].generics.clone();
            let show = |prog: &Program, t: Ty| prog.show_ty(interner, t, &g);
            let sig = |prog: &Program, ps: &[Ty], r: Ty| {
                format!("fn({}) -> {}", ps.iter().map(|&p| show(prog, p)).collect::<Vec<_>>().join(", "), show(prog, r))
            };
            diags.report(
                Diagnostic::error(format!(
                    "method `{}` has an incompatible type for trait `{trait_name}`",
                    interner.resolve(name)
                ))
                .with_code("E0206")
                .with_label(span, format!("found {}", sig(prog, &got_params, got_ret)))
                .with_note(format!("expected {}", sig(prog, &exp_params, exp_ret))),
            );
        }
    }
    for &tm in &trait_methods {
        let f = &prog.fns[tm.index()];
        if f.body.is_none() && !impl_methods.iter().any(|&im| prog.fns[im.index()].name == f.name) {
            diags.report(
                Diagnostic::error(format!(
                    "missing trait method `{}` in impl of `{trait_name}`",
                    interner.resolve(f.name)
                ))
                .with_code("E0207")
                .with_label(impl_span, "method not implemented"),
            );
        }
    }
    let _ = ast;
}
