//! Completion and signature help.

use std::collections::HashSet;

use super::Analysis;
use crate::lsp::json::Json;
use crate::sema::{builtin_const, builtin_method, AdtKind, DefKind, FnId, Item, Prim, Ty, TyKind};
use crate::syntax::{Keyword, NodeKind};

// LSP CompletionItemKind numbers
pub const KIND_METHOD: u32 = 2;
pub const KIND_FUNCTION: u32 = 3;
pub const KIND_FIELD: u32 = 5;
pub const KIND_VARIABLE: u32 = 6;
pub const KIND_CLASS: u32 = 7;
pub const KIND_INTERFACE: u32 = 8;
pub const KIND_MODULE: u32 = 9;
pub const KIND_ENUM: u32 = 13;
pub const KIND_KEYWORD: u32 = 14;
pub const KIND_CONSTANT: u32 = 21;
pub const KIND_STRUCT: u32 = 22;
pub const KIND_ENUM_MEMBER: u32 = 20;

#[derive(Debug, Clone, PartialEq)]
pub struct Item2 {
    pub label: String,
    pub kind: u32,
    pub detail: String,
}

impl Item2 {
    fn new(label: impl Into<String>, kind: u32, detail: impl Into<String>) -> Item2 {
        Item2 { label: label.into(), kind, detail: detail.into() }
    }

    pub fn to_json(&self, sort_group: char) -> Json {
        let mut fields = vec![
            ("label", Json::from(self.label.clone())),
            ("kind", Json::from(self.kind)),
            ("sortText", Json::from(format!("{sort_group}{}", self.label))),
        ];
        if !self.detail.is_empty() {
            fields.push(("detail", self.detail.clone().into()));
        }
        Json::object(fields)
    }
}

/// What the cursor is completing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionContext {
    /// After `expr.prefix`: byte offset of the dot.
    Members { dot: usize },
    /// After `Qualifier::prefix`.
    Path { qualifier: String },
    /// A plain name.
    Scope,
    /// Inside a string or comment: nothing to offer.
    Nothing,
}

/// One unclosed `(`, `[` or `{` found by [`scan`].
#[derive(Debug, Clone, Copy)]
pub struct Open {
    pub ch: u8,
    pub pos: usize,
    /// Commas seen at this nesting level since the opener.
    pub commas: usize,
}

/// A forward scan of `text`: the delimiters still open at its end, and whether the end lies
/// inside a string, char literal or comment.
pub fn scan(text: &str) -> (Vec<Open>, bool) {
    let b = text.as_bytes();
    let mut stack: Vec<Open> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                match text[i..].find('\n') {
                    Some(n) => i += n,
                    None => return (stack, true),
                }
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let mut depth = 1;
                let mut j = i + 2;
                while j < b.len() && depth > 0 {
                    if b[j] == b'/' && b.get(j + 1) == Some(&b'*') {
                        depth += 1;
                        j += 2;
                    } else if b[j] == b'*' && b.get(j + 1) == Some(&b'/') {
                        depth -= 1;
                        j += 2;
                    } else {
                        j += 1;
                    }
                }
                if depth > 0 {
                    return (stack, true);
                }
                i = j;
                continue;
            }
            b'"' => {
                let mut j = i + 1;
                loop {
                    match b.get(j) {
                        None => return (stack, true),
                        Some(b'\\') => j += 2,
                        Some(b'"') => break,
                        Some(_) => j += 1,
                    }
                }
                i = j + 1;
                continue;
            }
            b'\'' => {
                // a character literal: 'x' or '\n'
                let mut j = i + 1;
                if b.get(j) == Some(&b'\\') {
                    j += 2;
                } else if let Some(c) = text[j..].chars().next() {
                    j += c.len_utf8();
                }
                if b.get(j) == Some(&b'\'') {
                    i = j + 1;
                    continue;
                }
                return (stack, i + 1 >= b.len()); // a stray quote at the very end: still typing
            }
            c @ (b'(' | b'[' | b'{') => stack.push(Open { ch: c, pos: i, commas: 0 }),
            b')' | b']' | b'}' => {
                stack.pop();
            }
            b',' => {
                if let Some(top) = stack.last_mut() {
                    top.commas += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    (stack, false)
}

/// The delimiters that close everything still open at `cut`, when the document as a whole is
/// unbalanced (an unfinished edit). A balanced document needs no help.
pub fn closing_delimiters(text: &str, cut: usize) -> String {
    if scan(text).0.is_empty() {
        return String::new();
    }
    let (stack, _) = scan(&text[..cut.min(text.len())]);
    stack
        .iter()
        .rev()
        .map(|o| match o.ch {
            b'(' => ')',
            b'[' => ']',
            _ => '}',
        })
        .collect()
}

fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

/// Start of the identifier ending at `end`.
pub fn ident_start(text: &str, end: usize) -> usize {
    let b = text.as_bytes();
    let mut s = end;
    while s > 0 && is_ident_byte(b[s - 1]) {
        s -= 1;
    }
    // stay on a character boundary
    while !text.is_char_boundary(s) {
        s += 1;
    }
    s
}

pub fn completion_context(text: &str, offset: usize) -> CompletionContext {
    let offset = offset.min(text.len());
    let (_, inside) = scan(&text[..offset]);
    if inside {
        return CompletionContext::Nothing;
    }
    let start = ident_start(text, offset);
    let before = &text[..start];
    if let Some(prev) = before.strip_suffix('.') {
        // `1.` and `1.5` are numbers, not member access
        let tail = &prev[ident_start(prev, prev.len())..];
        if !(tail.chars().next().is_some_and(|c| c.is_ascii_digit())) {
            return CompletionContext::Members { dot: start - 1 };
        }
        return CompletionContext::Nothing;
    }
    if let Some(prev) = before.strip_suffix("::") {
        let q = &prev[ident_start(prev, prev.len())..];
        if !q.is_empty() {
            return CompletionContext::Path { qualifier: q.to_string() };
        }
    }
    CompletionContext::Scope
}

/// The call whose argument list contains the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallContext {
    /// Offset of the first character of the callee's name.
    pub name_start: usize,
    pub is_method: bool,
    pub active: usize,
}

pub fn call_context(text: &str, offset: usize) -> Option<CallContext> {
    let offset = offset.min(text.len());
    let (stack, inside) = scan(&text[..offset]);
    if inside {
        return None;
    }
    let open = stack.last().filter(|o| o.ch == b'(')?;
    let name_end = open.pos;
    let name_start = ident_start(text, name_end);
    if name_start == name_end {
        return None;
    }
    let is_method = text[..name_start].ends_with('.');
    Some(CallContext { name_start, is_method, active: open.commas })
}

/// Whether the head of two types is the same (generic arguments are ignored).
fn same_head(a: &TyKind, b: &TyKind) -> bool {
    match (a, b) {
        (TyKind::Adt(x, _), TyKind::Adt(y, _)) => x == y,
        (TyKind::Array(_), TyKind::Array(_))
        | (TyKind::Set(_), TyKind::Set(_))
        | (TyKind::Map(..), TyKind::Map(..)) => true,
        (TyKind::Fn(..), TyKind::Fn(..)) => true,
        (x, y) => x == y,
    }
}

impl Analysis {
    /// Methods and fields available on a value of type `ty`.
    pub fn complete_members_of(&mut self, ty: Ty) -> Vec<Item2> {
        let mut out: Vec<Item2> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let kind = self.prog.types.kind(ty).clone();

        if let TyKind::Adt(a, args) = &kind {
            if let AdtKind::Struct { fields } = &self.prog.adt(*a).kind {
                let fields: Vec<(String, Ty)> =
                    fields.iter().map(|f| (self.interner.resolve(f.name).to_string(), f.ty)).collect();
                for (name, fty) in fields {
                    let fty = self.prog.types.subst(fty, args);
                    let detail = self.show_ty_plain(fty).unwrap_or_default();
                    if seen.insert(name.clone()) {
                        out.push(Item2::new(name, KIND_FIELD, detail));
                    }
                }
            }
        }

        // user-written methods: inherent impls, then trait impls (with their default methods)
        let mut methods: Vec<FnId> = Vec::new();
        let inherent = self.prog.impl_index.inherent.clone();
        let by_trait: Vec<_> = self.prog.impl_index.by_trait.iter().map(|(t, v)| (*t, v.clone())).collect();
        for i in inherent {
            if same_head(self.prog.types.kind(self.prog.impl_def(i).self_ty), &kind) {
                methods.extend(self.prog.impl_def(i).methods.iter().copied());
            }
        }
        for (t, impls) in by_trait {
            for i in impls {
                if same_head(self.prog.types.kind(self.prog.impl_def(i).self_ty), &kind) {
                    methods.extend(self.prog.impl_def(i).methods.iter().copied());
                    methods.extend(self.prog.trait_def(t).methods.iter().copied());
                }
            }
        }
        for f in methods {
            let fd = self.prog.func(f);
            if !fd.has_self {
                continue;
            }
            let name = self.interner.resolve(fd.name).to_string();
            if seen.insert(name.clone()) {
                let sig = self.signature(f);
                out.push(Item2::new(name, KIND_METHOD, sig.label));
            }
        }

        // built-in methods of strings, collections, numbers and chars
        let mut names: Vec<&'static str> = crate::builtin::Builtin::ALL.iter().map(|b| b.name()).collect();
        names.sort_unstable();
        names.dedup();
        for name in names {
            if seen.contains(name) {
                continue;
            }
            if let Some(sig) = builtin_method(&mut self.prog, ty, name) {
                let params: Vec<String> = sig.params.iter().filter_map(|&p| self.show_ty_plain(p)).collect();
                let ret = match self.show_ty_plain(sig.ret) {
                    Some(r) if r != "()" => format!(" -> {r}"),
                    _ => String::new(),
                };
                seen.insert(name.to_string());
                out.push(Item2::new(name, KIND_METHOD, format!("{name}({}){ret}", params.join(", "))));
            }
        }
        out
    }

    /// Members of the expression ending right before `dot` (a byte offset).
    pub fn complete_members(&mut self, dot: u32) -> Vec<Item2> {
        if dot == 0 {
            return Vec::new();
        }
        let ast = self.prog.modules[self.module.index()].ast.clone();
        let Some(node) = ast.node_at(dot - 1) else { return Vec::new() };
        let Some(&ty) = self.prog.modules[self.module.index()].info.node_types.get(node.0 as usize) else {
            return Vec::new();
        };
        if matches!(self.prog.types.kind(ty), TyKind::Error | TyKind::Infer(_)) {
            return Vec::new();
        }
        self.complete_members_of(ty)
    }

    fn item_entry(&self, name: &str, item: Item) -> Item2 {
        match item {
            Item::Fn(f) => Item2::new(name, KIND_FUNCTION, self.signature(f).label),
            Item::Adt(a) => {
                let adt = self.prog.adt(a);
                if adt.is_enum() {
                    Item2::new(name, KIND_ENUM, "enum")
                } else {
                    Item2::new(name, KIND_STRUCT, "struct")
                }
            }
            Item::Trait(_) => Item2::new(name, KIND_INTERFACE, "trait"),
            Item::Global(g) => {
                let gd = self.prog.global(g);
                let ty = self.show_ty_plain(gd.ty).unwrap_or_default();
                if gd.is_const {
                    Item2::new(name, KIND_CONSTANT, format!("const {name}: {ty}"))
                } else {
                    Item2::new(name, KIND_VARIABLE, format!("{} {name}: {ty}", if gd.mutable { "var" } else { "let" }))
                }
            }
        }
    }

    /// Completions after `qualifier::`.
    pub fn complete_path(&mut self, qualifier: &str) -> Vec<Item2> {
        let mut out = Vec::new();
        // primitive types: `i32::MAX`
        if let Some(prim) = Prim::ALL.iter().copied().find(|p| p.name() == qualifier) {
            for name in ["MAX", "MIN", "EPSILON", "INFINITY", "NEG_INFINITY", "NAN"] {
                if builtin_const(prim, name).is_some() {
                    out.push(Item2::new(name, KIND_CONSTANT, format!("{qualifier}::{name}")));
                }
            }
            return out;
        }
        let Some(sym) = self.interner.get(qualifier) else { return out };
        // `import "path" as alias;`
        if let Some(&m) = self.prog.module(self.module).scope.aliases.get(&sym) {
            let exports: Vec<(String, Item)> = self
                .prog
                .module(m)
                .scope
                .exports
                .iter()
                .map(|(s, &i)| (self.interner.resolve(*s).to_string(), i))
                .collect();
            for (name, item) in exports {
                out.push(self.item_entry(&name, item));
            }
            return out;
        }
        match self.prog.lookup_item(self.module, sym) {
            Some(Item::Adt(a)) => {
                let adt = self.prog.adt(a);
                if let AdtKind::Enum { variants } = &adt.kind {
                    for v in variants {
                        let name = self.interner.resolve(v.name).to_string();
                        let detail = if v.fields.is_empty() {
                            name.clone()
                        } else {
                            let fs: Vec<String> =
                                v.fields.iter().map(|&t| self.prog.show_ty(&self.interner, t, &adt.generics)).collect();
                            format!("{name}({})", fs.join(", "))
                        };
                        out.push(Item2::new(name, KIND_ENUM_MEMBER, detail));
                    }
                }
                // associated functions of inherent impls
                let kind = TyKind::Adt(a, Vec::new());
                let inherent = self.prog.impl_index.inherent.clone();
                for i in inherent {
                    if same_head(self.prog.types.kind(self.prog.impl_def(i).self_ty), &kind) {
                        for &f in &self.prog.impl_def(i).methods.clone() {
                            let name = self.interner.resolve(self.prog.func(f).name).to_string();
                            out.push(Item2::new(name, KIND_FUNCTION, self.signature(f).label));
                        }
                    }
                }
            }
            Some(Item::Trait(t)) => {
                for &f in &self.prog.trait_def(t).methods.clone() {
                    let name = self.interner.resolve(self.prog.func(f).name).to_string();
                    out.push(Item2::new(name, KIND_METHOD, self.signature(f).label));
                }
            }
            _ => {}
        }
        out
    }

    /// Names visible at `offset`: locals, module items, the prelude, built-ins and keywords.
    pub fn complete_scope(&mut self, offset: u32) -> Vec<(char, Item2)> {
        let mut out: Vec<(char, Item2)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let ast = self.prog.module(self.module).ast.clone();

        // local variables declared before the cursor in an enclosing function or closure
        let mut locals: Vec<(u32, String, Item2)> = Vec::new();
        for l in &self.prog.locals {
            if l.module != self.module || l.span.file != self.file || l.span.lo >= offset {
                continue;
            }
            let owner = ast.span(l.owner);
            if !(owner.lo <= offset && offset <= owner.hi) {
                continue;
            }
            let name = self.interner.resolve(l.name).to_string();
            let ty = self.show_ty_in(l.ty, &self.generics_at(offset)).map(|t| format!(": {t}")).unwrap_or_default();
            let kw = if l.mutable { "var" } else { "let" };
            locals.push((
                l.span.lo,
                name.clone(),
                Item2::new(name, KIND_VARIABLE, format!("{kw} {}{ty}", self.interner.resolve(l.name))),
            ));
        }
        locals.sort_by_key(|(lo, ..)| std::cmp::Reverse(*lo));
        for (_, name, item) in locals {
            if seen.insert(name) {
                out.push(('0', item));
            }
        }

        let own: Vec<(String, Item)> = self
            .prog
            .module(self.module)
            .scope
            .items
            .iter()
            .map(|(s, &i)| (self.interner.resolve(*s).to_string(), i))
            .collect();
        for (name, item) in own {
            if seen.insert(name.clone()) {
                let e = self.item_entry(&name, item);
                out.push(('1', e));
            }
        }
        let aliases: Vec<String> =
            self.prog.module(self.module).scope.aliases.keys().map(|s| self.interner.resolve(*s).to_string()).collect();
        for name in aliases {
            if seen.insert(name.clone()) {
                out.push(('1', Item2::new(name, KIND_MODULE, "module")));
            }
        }
        if self.module.0 != 0 {
            let prelude: Vec<(String, Item)> = self.prog.modules[0]
                .scope
                .exports
                .iter()
                .map(|(s, &i)| (self.interner.resolve(*s).to_string(), i))
                .collect();
            for (name, item) in prelude {
                if seen.insert(name.clone()) {
                    let e = self.item_entry(&name, item);
                    out.push(('2', e));
                }
            }
        }
        for (name, detail) in [
            ("print", "print(value)"),
            ("println", "println(value)"),
            ("input", "input() -> string"),
            ("read_line", "read_line() -> Option<string>"),
            ("panic", "panic(message: string) -> !"),
            ("assert", "assert(condition: bool, message: string)"),
        ] {
            if seen.insert(name.to_string()) {
                out.push(('2', Item2::new(name, KIND_FUNCTION, detail)));
            }
        }
        for k in Keyword::ALL {
            let name = k.as_str();
            let kind = if k.is_primitive_type() { KIND_CLASS } else { KIND_KEYWORD };
            if seen.insert(name.to_string()) {
                out.push((
                    '3',
                    Item2::new(name, kind, if k.is_primitive_type() { "primitive type" } else { "keyword" }),
                ));
            }
        }
        out
    }

    /// Signature help for the call whose callee name starts at `ctx.name_start`.
    pub fn signature_help(&mut self, ctx: &CallContext) -> Option<Json> {
        let def_id = self.def_at(ctx.name_start as u32)?;
        let d = self.prog.symbols.def(def_id).clone();
        let (label, mut params) = match d.kind {
            DefKind::Fn(f) => {
                let s = self.signature(f);
                (s.label, s.params)
            }
            DefKind::Variant(a, i) => {
                let adt = self.prog.adt(a);
                let v = &adt.variants()[i as usize];
                let ps: Vec<String> =
                    v.fields.iter().map(|&t| self.prog.show_ty(&self.interner, t, &adt.generics)).collect();
                (
                    format!(
                        "{}::{}({})",
                        self.interner.resolve(adt.name),
                        self.interner.resolve(v.name),
                        ps.join(", ")
                    ),
                    ps,
                )
            }
            _ => return None,
        };
        // a method call supplies the receiver itself
        if ctx.is_method && params.first().is_some_and(|p| p.ends_with("self")) {
            params.remove(0);
        }
        let active = ctx.active.min(params.len().saturating_sub(1));
        let parameters: Vec<Json> =
            params.iter().map(|p| Json::object(vec![("label", Json::from(p.clone()))])).collect();
        Some(Json::object(vec![
            (
                "signatures",
                Json::Arr(vec![Json::object(vec![
                    ("label", Json::from(label)),
                    ("parameters", Json::Arr(parameters)),
                ])]),
            ),
            ("activeSignature", Json::from(0u32)),
            ("activeParameter", Json::from(active)),
        ]))
    }
}

#[allow(dead_code)]
fn _unused(_: NodeKind) {}
