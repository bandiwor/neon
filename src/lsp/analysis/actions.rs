//! Code actions (`textDocument/codeAction`): quick fixes for diagnostics and a few refactorings.
//!
//! * missing members of a trait `impl` (also when the trait's generic arguments are left out:
//!   `impl Add for Data {}`), all at once or one method at a time;
//! * the missing arms of a `match` (every uncovered case, or a wildcard), removal of an
//!   unreachable arm;
//! * the missing fields of a struct literal;
//! * `let x` assigned later: declare it with `var`;
//! * misspelled names, fields and enum variants: change to the closest one;
//! * a missing `;`;
//! * a string with `{name}` in it that is not a format string: make it an `f"..."`.
//!
//! The actions are computed from the typed program (and the syntax tree), not from rendered text, so
//! they also work while the code around them has other errors.

use super::complete::{KIND_FIELD, KIND_KEYWORD};
use super::Analysis;
use crate::diag::Span;
use crate::sema::{Item, Res, Ty, TyKind};
use crate::syntax::{NodeId, NodeKind};

/// Replace `lo..hi` of the document with `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub lo: u32,
    pub hi: u32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub title: String,
    /// `quickfix` or `refactor.rewrite`.
    pub kind: &'static str,
    pub edits: Vec<TextEdit>,
    /// The diagnostic code this fixes, if it fixes one.
    pub fixes: Option<&'static str>,
    pub preferred: bool,
}

const INDENT: &str = "    ";
const PLACEHOLDER: &str = "panic(\"not implemented\")";

// ---- small text helpers --------------------------------------------------------------------------

fn line_start(text: &str, off: usize) -> usize {
    text[..off.min(text.len())].rfind('\n').map_or(0, |i| i + 1)
}

/// The leading white space of the line containing `off`.
fn indent_of(text: &str, off: usize) -> String {
    let s = line_start(text, off);
    text[s..].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

/// The position right after the last non-white-space character before `off`.
fn after_last_token(text: &str, off: usize) -> usize {
    text[..off.min(text.len())].trim_end().len()
}

/// Where a comma that ends the element at `..end` is: `end` itself when the element's span already
/// includes it, else after the next token if that is a comma.
fn comma_end(text: &str, end: usize, limit: usize) -> Option<usize> {
    if text[..end].ends_with(',') {
        return Some(end);
    }
    let rest = &text[end..limit.max(end)];
    rest.trim_start().starts_with(',').then(|| end + rest.find(',').expect("comma") + 1)
}

fn overlaps(lo: u32, hi: u32, a: u32, b: u32) -> bool {
    lo <= b && a <= hi
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Replaces whole identifiers: `Self` and the trait's own type parameters.
fn replace_words(text: &str, map: &[(String, String)]) -> String {
    let mut out = String::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if !word.is_empty() {
            match map.iter().find(|(from, _)| from == word) {
                Some((_, to)) => out.push_str(to),
                None => out.push_str(word),
            }
            word.clear();
        }
    };
    for c in text.chars() {
        if is_ident_char(c) {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// Edit distance where swapping two neighbours counts once (`coutn` -> `count`).
fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

/// Up to three candidates that look like `name`, closest first.
fn similar(name: &str, candidates: impl IntoIterator<Item = String>) -> Vec<String> {
    let limit = (name.chars().count() / 3).clamp(1, 3);
    let mut found: Vec<(usize, String)> = candidates
        .into_iter()
        .filter(|c| c != name)
        .map(|c| (distance(&name.to_lowercase(), &c.to_lowercase()), c))
        .filter(|(d, _)| *d <= limit)
        .collect();
    found.sort();
    found.dedup();
    found.into_iter().take(3).map(|(_, c)| c).collect()
}

/// The text between the first pair of backticks after `after` in a diagnostic message.
fn quoted(message: &str, after: &str) -> Option<String> {
    let rest = &message[message.find(after)? + after.len()..];
    let start = rest.find('`')? + 1;
    let end = rest[start..].find('`')? + start;
    Some(rest[start..end].to_string())
}

impl Analysis {
    fn src_text(&self, span: Span) -> String {
        self.text().get(span.lo as usize..span.hi as usize).unwrap_or("").to_string()
    }

    /// Code actions for the byte range `lo..hi` of the analyzed document.
    pub fn code_actions(&mut self, lo: u32, hi: u32) -> Vec<Action> {
        let text = self.text().to_string();
        let mut out: Vec<Action> = Vec::new();
        let diags: Vec<(Option<&'static str>, Span, String)> = self
            .diags
            .iter()
            .filter_map(|d| {
                let l = d.labels.iter().find(|l| l.primary).or_else(|| d.labels.first())?;
                (!l.span.is_dummy() && l.span.file == self.file).then(|| (d.code, l.span, d.message.clone()))
            })
            .collect();
        for (code, span, message) in diags {
            if !overlaps(span.lo, span.hi, lo, hi) {
                continue;
            }
            match code {
                Some("E0101") => self.fix_semicolon(&text, span, &message, &mut out),
                Some("E0210") => self.fix_unknown_name(span, &message, &mut out),
                Some("E0214") => self.fix_unknown_member(&text, span, &message, &mut out),
                Some("E0217") => self.fix_unknown_field(&text, span, &message, &mut out),
                Some("E0220") => self.fix_immutable(&text, span, &mut out),
                Some("E0226") => self.fix_missing_fields(&text, span, &message, &mut out),
                Some("E0240") => self.fix_non_exhaustive(&text, span, &mut out),
                Some("W0241") => self.fix_unreachable_arm(&text, span, &mut out),
                _ => {}
            }
        }
        self.implement_members(&text, lo, hi, &mut out);
        self.make_format_string(&text, lo, hi, &mut out);
        let mut seen: Vec<(String, Vec<TextEdit>)> = Vec::new();
        out.retain(|a| {
            let key = (a.title.clone(), a.edits.clone());
            !seen.contains(&key) && {
                seen.push(key);
                true
            }
        });
        out
    }

    // ---- punctuation --------------------------------------------------------------------------------

    fn fix_semicolon(&self, text: &str, span: Span, message: &str, out: &mut Vec<Action>) {
        if !message.starts_with("expected `;`") {
            return;
        }
        let at = after_last_token(text, span.lo as usize) as u32;
        out.push(Action {
            title: "Insert `;`".into(),
            kind: "quickfix",
            edits: vec![TextEdit { lo: at, hi: at, text: ";".into() }],
            fixes: Some("E0101"),
            preferred: true,
        });
    }

    // ---- misspelled names ----------------------------------------------------------------------------

    fn rename_actions(
        &self,
        span: Span,
        found: &str,
        candidates: Vec<String>,
        code: &'static str,
        out: &mut Vec<Action>,
    ) {
        for (i, c) in similar(found, candidates).into_iter().enumerate() {
            out.push(Action {
                title: format!("Change to `{c}`"),
                kind: "quickfix",
                edits: vec![TextEdit { lo: span.lo, hi: span.hi, text: c }],
                fixes: Some(code),
                preferred: i == 0,
            });
        }
    }

    /// `cannot find value `x`` / ``cannot find `Colour` ``: the closest name in scope.
    fn fix_unknown_name(&mut self, span: Span, message: &str, out: &mut Vec<Action>) {
        let Some(name) = quoted(message, "cannot find") else { return };
        let candidates: Vec<String> = self
            .complete_scope(span.lo)
            .into_iter()
            .filter(|(_, i)| i.kind != KIND_KEYWORD)
            .map(|(_, i)| i.label)
            .collect();
        // the squiggle may cover more than the name (`Colour::Red`): only the name is replaced
        let name_span = Span::new(span.file, span.lo, span.lo + name.len() as u32);
        self.rename_actions(name_span, &name, candidates, "E0210", out);
    }

    /// `Color::Gren`: the variants and associated functions of `Color`.
    fn fix_unknown_member(&mut self, text: &str, span: Span, message: &str, out: &mut Vec<Action>) {
        let Some(name) = quoted(message, "named") else { return };
        let before = text[..span.lo as usize].trim_end();
        let Some(qualifier) = before.strip_suffix("::") else { return };
        let start = qualifier.rfind(|c: char| !(is_ident_char(c) || c == ':')).map_or(0, |i| i + 1);
        let candidates: Vec<String> = self.complete_path(&qualifier[start..]).into_iter().map(|i| i.label).collect();
        self.rename_actions(span, &name, candidates, "E0214", out);
    }

    /// `p.z`: the closest field of `p`'s type.
    fn fix_unknown_field(&mut self, text: &str, span: Span, message: &str, out: &mut Vec<Action>) {
        let Some(name) = quoted(message, "no field") else { return };
        let end = span.hi as usize;
        let Some(start) = end.checked_sub(name.len()) else { return };
        if text.get(start..end) != Some(name.as_str()) || start == 0 || !text[..start].ends_with('.') {
            return;
        }
        let candidates: Vec<String> = self
            .complete_members(start as u32 - 1)
            .into_iter()
            .filter(|i| i.kind == KIND_FIELD)
            .map(|i| i.label)
            .collect();
        let name_span = Span::new(span.file, start as u32, end as u32);
        self.rename_actions(name_span, &name, candidates, "E0217", out);
    }

    // ---- let -> var ------------------------------------------------------------------------------------

    /// `x = 2` where `x` was declared with `let` (or is a parameter).
    fn fix_immutable(&self, text: &str, span: Span, out: &mut Vec<Action>) {
        let ast = self.prog.module(self.module).ast.clone();
        let Some(node) = ast.node_at(span.lo) else { return };
        let Some(res) = self.prog.module(self.module).info.res.get(&node) else { return };
        let (decl, name) = match res {
            Res::Local(l) => {
                let d = &self.prog.locals[l.index()];
                (d.span, self.interner.resolve(d.name).to_string())
            }
            Res::Global(g) => {
                let d = &self.prog.globals[g.index()];
                (d.name_span, self.interner.resolve(d.name).to_string())
            }
            _ => return,
        };
        if decl.file != self.file || decl.is_dummy() {
            return;
        }
        let before = &text[..decl.lo as usize];
        let head = before.trim_end();
        let edit = if let Some(rest) = head.strip_suffix("let") {
            let word_start = rest.len() as u32;
            (!rest.ends_with(is_ident_char)).then_some(TextEdit {
                lo: word_start,
                hi: word_start + 3,
                text: "var".into(),
            })
        } else if head.strip_suffix('(').is_some() || head.ends_with(',') {
            // a parameter: `fn f(x: i64)` -> `fn f(var x: i64)`
            let is_param = ast.node_at(decl.lo).is_some_and(|n| matches!(ast.kind(n), NodeKind::Param { .. }));
            is_param.then_some(TextEdit { lo: decl.lo, hi: decl.lo, text: "var ".into() })
        } else {
            None
        };
        if let Some(edit) = edit {
            out.push(Action {
                title: format!("Declare `{name}` with `var`"),
                kind: "quickfix",
                edits: vec![edit],
                fixes: Some("E0220"),
                preferred: true,
            });
        }
    }

    // ---- struct literals ------------------------------------------------------------------------------

    /// A value to put in a new field: zero, empty, `Option::None`, or a placeholder.
    fn default_expr(&self, ty: Ty) -> String {
        match self.prog.types.kind(ty) {
            TyKind::Prim(p) if p.is_int() => "0".into(),
            TyKind::Prim(p) if p.is_float() => "0.0".into(),
            TyKind::Prim(crate::sema::Prim::Bool) => "false".into(),
            TyKind::Prim(crate::sema::Prim::Str) => "\"\"".into(),
            TyKind::Prim(crate::sema::Prim::Char) => "' '".into(),
            TyKind::Array(_) => "[]".into(),
            TyKind::Map(..) => "{}".into(),
            TyKind::Adt(a, _) if self.interner.resolve(self.prog.adt(*a).name) == "Option" => "Option::None".into(),
            _ => PLACEHOLDER.into(),
        }
    }

    fn fix_missing_fields(&mut self, text: &str, span: Span, message: &str, out: &mut Vec<Action>) {
        let ast = self.prog.module(self.module).ast.clone();
        let Some((id, _)) = ast.iter().find(|(_, n)| n.span == span && matches!(n.kind, NodeKind::StructLit { .. }))
        else {
            return;
        };
        let NodeKind::StructLit { fields, .. } = *ast.kind(id) else { return };
        let Some(adt_name) = quoted(message, "initializer of") else { return };
        let Some(sym) = self.interner.get(&adt_name) else { return };
        let Some(Item::Adt(adt)) = self.prog.lookup_item(self.module, sym) else { return };
        let given: Vec<String> = ast
            .list(fields)
            .iter()
            .filter_map(|&f| match *ast.kind(f) {
                NodeKind::FieldInit { name, .. } => Some(self.interner.resolve(name).to_string()),
                _ => None,
            })
            .collect();
        let missing: Vec<(String, Ty)> = self
            .prog
            .adt(adt)
            .fields()
            .iter()
            .map(|f| (self.interner.resolve(f.name).to_string(), f.ty))
            .filter(|(n, _)| !given.contains(n))
            .collect();
        if missing.is_empty() {
            return;
        }
        let items: Vec<String> = missing.iter().map(|(n, t)| format!("{n}: {}", self.default_expr(*t))).collect();
        let close = span.hi as usize - 1;
        if text.as_bytes().get(close) != Some(&b'}') {
            return;
        }
        let open = match text[span.lo as usize..close].find('{') {
            Some(i) => span.lo as usize + i,
            None => return,
        };
        let multiline = text[open..close].contains('\n');
        let edit = if given.is_empty() {
            if multiline {
                let base = indent_of(text, span.lo as usize);
                let body: String = items.iter().map(|i| format!("{base}{INDENT}{i},\n")).collect();
                TextEdit { lo: open as u32 + 1, hi: close as u32, text: format!("\n{body}{base}") }
            } else {
                TextEdit { lo: open as u32 + 1, hi: close as u32, text: format!(" {} ", items.join(", ")) }
            }
        } else {
            let last = ast.span(*ast.list(fields).last().expect("some field"));
            let mut at = last.hi as usize;
            let comma = comma_end(text, at, close);
            let trailing_comma = comma.is_some();
            if let Some(c) = comma {
                at = c;
            }
            if multiline {
                let ind = indent_of(text, last.lo as usize);
                let lead = if trailing_comma { "" } else { "," };
                let body: String = items.iter().map(|i| format!("\n{ind}{i},")).collect();
                TextEdit { lo: at as u32, hi: at as u32, text: format!("{lead}{body}") }
            } else if trailing_comma {
                TextEdit { lo: at as u32, hi: at as u32, text: format!(" {},", items.join(", ")) }
            } else {
                TextEdit { lo: at as u32, hi: at as u32, text: format!(", {}", items.join(", ")) }
            }
        };
        let names: Vec<String> = missing.iter().map(|(n, _)| format!("`{n}`")).collect();
        out.push(Action {
            title: format!("Add missing field{} {}", if missing.len() == 1 { "" } else { "s" }, names.join(", ")),
            kind: "quickfix",
            edits: vec![edit],
            fixes: Some("E0226"),
            preferred: true,
        });
    }

    // ---- match ------------------------------------------------------------------------------------------

    fn match_node_at(&self, span: Span) -> Option<NodeId> {
        let ast = &self.prog.module(self.module).ast;
        ast.iter().find(|(_, n)| n.span == span && matches!(n.kind, NodeKind::Match { .. })).map(|(id, _)| id)
    }

    fn fix_non_exhaustive(&mut self, text: &str, span: Span, out: &mut Vec<Action>) {
        let Some(id) = self.match_node_at(span) else { return };
        let ast = self.prog.module(self.module).ast.clone();
        let NodeKind::Match { scrutinee, arms } = *ast.kind(id) else { return };
        let Some(&ty) = self.prog.module(self.module).info.node_types.get(scrutinee.0 as usize) else { return };
        let missing = crate::sema::missing_patterns(&mut self.prog, &self.interner, self.module, id, ty, 32);
        if missing.is_empty() {
            return;
        }
        let close = span.hi as usize - 1;
        if text.as_bytes().get(close) != Some(&b'}') {
            return;
        }
        let arm_list = ast.list(arms);
        let arm_indent = match arm_list.last() {
            Some(&a) => indent_of(text, ast.span(a).lo as usize),
            None => format!("{}{INDENT}", indent_of(text, span.lo as usize)),
        };
        let build = |patterns: &[String]| -> Vec<TextEdit> {
            let body: String = patterns.iter().map(|p| format!("\n{arm_indent}{p} => {PLACEHOLDER},")).collect();
            let mut edits = Vec::new();
            match arm_list.last() {
                Some(&last) => {
                    let end = ast.span(last).hi as usize;
                    // the last arm may lack its comma (`x => 1` before `}`)
                    if let Some(at) = comma_end(text, end, close) {
                        edits.push(TextEdit { lo: at as u32, hi: at as u32, text: body });
                    } else {
                        edits.push(TextEdit { lo: end as u32, hi: end as u32, text: format!(",{body}") });
                    }
                }
                None => {
                    let base = indent_of(text, span.lo as usize);
                    let open = text[span.lo as usize..close].rfind('{').map_or(close, |i| span.lo as usize + i + 1);
                    edits.push(TextEdit { lo: open as u32, hi: close as u32, text: format!("{body}\n{base}") });
                }
            }
            edits
        };
        let shown: Vec<String> = missing.iter().take(3).map(|m| format!("`{m}`")).collect();
        let more = if missing.len() > 3 { format!(" and {} more", missing.len() - 3) } else { String::new() };
        out.push(Action {
            title: format!(
                "Add missing match arm{}: {}{more}",
                if missing.len() == 1 { "" } else { "s" },
                shown.join(", ")
            ),
            kind: "quickfix",
            edits: build(&missing),
            fixes: Some("E0240"),
            preferred: true,
        });
        if missing.iter().any(|m| m != "_") {
            out.push(Action {
                title: "Add a wildcard arm `_`".into(),
                kind: "quickfix",
                edits: build(&["_".to_string()]),
                fixes: Some("E0240"),
                preferred: false,
            });
        }
    }

    fn fix_unreachable_arm(&self, text: &str, span: Span, out: &mut Vec<Action>) {
        let ast = &self.prog.module(self.module).ast;
        let Some((_, arm)) = ast.iter().find(|(_, n)| match n.kind {
            NodeKind::MatchArm { pat, .. } => ast.span(pat) == span,
            _ => false,
        }) else {
            return;
        };
        let (mut lo, mut hi) = (arm.span.lo as usize, arm.span.hi as usize);
        if let Some(c) = comma_end(text, hi, text.len()) {
            hi = c;
        }
        // an arm on its own lines goes away with its line break
        let ls = line_start(text, lo);
        let line_end = text[hi..].find('\n').map_or(text.len(), |i| hi + i);
        if text[ls..lo].trim().is_empty() && text[hi..line_end].trim().is_empty() {
            lo = ls;
            hi = (line_end + 1).min(text.len());
        }
        out.push(Action {
            title: "Remove the unreachable arm".into(),
            kind: "quickfix",
            edits: vec![TextEdit { lo: lo as u32, hi: hi as u32, text: String::new() }],
            fixes: Some("W0241"),
            preferred: true,
        });
    }

    // ---- impl blocks ------------------------------------------------------------------------------------

    /// Stubs for the trait methods an `impl` leaves out; works when the cursor is on the `impl` (its
    /// header, an empty body) and also fills in the generic arguments of the trait if they were left out.
    fn implement_members(&mut self, text: &str, lo: u32, hi: u32, out: &mut Vec<Action>) {
        let ast = self.prog.module(self.module).ast.clone();
        for (_, node) in ast.iter() {
            let NodeKind::ImplDecl { trait_ref, self_ty, items, .. } = node.kind else { continue };
            let span = node.span;
            if span.file != self.file || !overlaps(span.lo, span.hi, lo, hi) {
                continue;
            }
            // not while the cursor is inside one of the methods
            if ast.list(items).iter().any(|&m| {
                let s = ast.span(m);
                s.lo < lo && hi < s.hi
            }) {
                continue;
            }
            let Some(trait_ref) = trait_ref.get() else { continue };
            let NodeKind::TyPath { segments } = *ast.kind(trait_ref) else { continue };
            let segs = ast.list(segments);
            let [seg] = segs else { continue };
            let NodeKind::PathSegment { name, args } = *ast.kind(*seg) else { continue };
            let Some(Item::Trait(tid)) = self.prog.lookup_item(self.module, name) else { continue };
            let tdef = self.prog.trait_def(tid);
            let trait_name = self.interner.resolve(tdef.name).to_string();
            let params: Vec<String> =
                tdef.generics.params.iter().skip(1).map(|g| self.interner.resolve(g.name).to_string()).collect();
            let self_text = self.src_text(ast.span(self_ty));
            let given: Vec<String> = ast.list(args).iter().map(|&a| self.src_text(ast.span(a))).collect();
            if !given.is_empty() && given.len() != params.len() {
                continue;
            }
            let mut edits_head = Vec::new();
            let arg_texts: Vec<String> = if given.is_empty() && !params.is_empty() {
                // `impl Add for Data`: the usual argument for the operator traits is the type itself
                let at = ast.span(trait_ref).hi;
                edits_head.push(TextEdit {
                    lo: at,
                    hi: at,
                    text: format!("<{}>", vec![self_text.clone(); params.len()].join(", ")),
                });
                vec![self_text.clone(); params.len()]
            } else {
                given
            };
            let mut subst: Vec<(String, String)> = vec![("Self".to_string(), self_text.clone())];
            subst.extend(params.iter().cloned().zip(arg_texts.iter().cloned()));

            let have: Vec<String> = ast
                .list(items)
                .iter()
                .filter_map(|&m| match *ast.kind(m) {
                    NodeKind::FnDecl { name, .. } => Some(self.interner.resolve(name).to_string()),
                    _ => None,
                })
                .collect();
            let mut stubs: Vec<(String, String)> = Vec::new();
            for &m in &self.prog.trait_def(tid).methods {
                let f = self.prog.func(m);
                let fname = self.interner.resolve(f.name).to_string();
                if f.body.is_some() || have.contains(&fname) {
                    continue;
                }
                let tast = self.prog.module(f.module).ast.clone();
                let sig_span = tast.span(f.node);
                let Some(file) = self.map.get(sig_span.file) else { continue };
                let sig = file
                    .text()
                    .get(sig_span.lo as usize..sig_span.hi as usize)
                    .unwrap_or("")
                    .trim()
                    .trim_end_matches(';')
                    .trim_end();
                stubs.push((fname, replace_words(sig, &subst)));
            }
            if stubs.is_empty() && edits_head.is_empty() {
                continue;
            }

            let base = indent_of(text, span.lo as usize);
            let close = span.hi as usize - 1;
            if text.as_bytes().get(close) != Some(&b'}') {
                continue;
            }
            let open = text[ast.span(self_ty).hi as usize..close].find('{').map(|i| ast.span(self_ty).hi as usize + i);
            let Some(open) = open else { continue };
            let render = |which: &[&(String, String)]| -> String {
                which
                    .iter()
                    .map(|(_, sig)| {
                        format!("{base}{INDENT}{sig} {{\n{base}{INDENT}{INDENT}{PLACEHOLDER}\n{base}{INDENT}}}")
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n")
            };
            let place = |which: &[&(String, String)]| -> Vec<TextEdit> {
                let body = render(which);
                let inner = &text[open + 1..close];
                let mut edits = edits_head.clone();
                if which.is_empty() {
                    return edits;
                }
                if inner.trim().is_empty() {
                    edits.push(TextEdit { lo: open as u32 + 1, hi: close as u32, text: format!("\n{body}\n{base}") });
                } else {
                    let at = after_last_token(text, close) as u32;
                    edits.push(TextEdit { lo: at, hi: at, text: format!("\n\n{body}") });
                }
                edits
            };
            let all: Vec<&(String, String)> = stubs.iter().collect();
            let names: Vec<String> = stubs.iter().map(|(n, _)| format!("`{n}`")).collect();
            let title = match (stubs.len(), edits_head.is_empty()) {
                (0, _) => {
                    format!("Add the generic argument{} of `{trait_name}`", if params.len() == 1 { "" } else { "s" })
                }
                (1, true) => format!("Implement missing method {}", names[0]),
                (1, false) => format!("Implement `{trait_name}<{}>`: add {}", arg_texts.join(", "), names[0]),
                (n, true) => format!("Implement {n} missing methods of `{trait_name}` ({})", names.join(", ")),
                (n, false) => format!(
                    "Implement `{trait_name}<{}>`: add {n} methods ({})",
                    arg_texts.join(", "),
                    names.join(", ")
                ),
            };
            out.push(Action {
                title,
                kind: "quickfix",
                edits: place(&all),
                fixes: if stubs.is_empty() { Some("E0203") } else { Some("E0207") },
                preferred: true,
            });
            if stubs.len() > 1 {
                for s in &stubs {
                    out.push(Action {
                        title: format!("Implement missing method `{}`", s.0),
                        kind: "quickfix",
                        edits: place(&[s]),
                        fixes: Some("E0207"),
                        preferred: false,
                    });
                }
            }
        }
    }

    // ---- f-strings -------------------------------------------------------------------------------------

    /// `"{x}"` is just text; offer to turn it into `f"{x}"`.
    fn make_format_string(&self, text: &str, lo: u32, hi: u32, out: &mut Vec<Action>) {
        let ast = &self.prog.module(self.module).ast;
        for (_, node) in ast.iter() {
            if !matches!(node.kind, NodeKind::StrLit(_)) || node.span.file != self.file {
                continue;
            }
            let s = node.span;
            if !overlaps(s.lo, s.hi, lo, hi) {
                continue;
            }
            let lit = &text[s.lo as usize..s.hi as usize];
            if !lit.starts_with('"') || (s.lo > 0 && text[..s.lo as usize].ends_with('f')) {
                continue;
            }
            if has_placeholder(lit) {
                out.push(Action {
                    title: "Make this a format string (`f\"...\"`)".into(),
                    kind: "refactor.rewrite",
                    edits: vec![TextEdit { lo: s.lo, hi: s.lo, text: "f".into() }],
                    fixes: None,
                    preferred: false,
                });
            }
        }
    }
}

/// Whether a string literal contains `{name...}` (not the escaped `{{`).
fn has_placeholder(lit: &str) -> bool {
    let b = lit.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'{' if b.get(i + 1) == Some(&b'{') => i += 2,
            b'{' => {
                let rest = &lit[i + 1..];
                let close = rest.find('}');
                let first = rest.chars().next();
                if close.is_some() && first.is_some_and(|c| c.is_alphabetic() || c == '_' || c == '(') {
                    return true;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::MemVfs;

    /// Applies an action's edits to `text`.
    fn apply(text: &str, action: &Action) -> String {
        let mut edits = action.edits.clone();
        edits.sort_by_key(|e| std::cmp::Reverse(e.lo));
        let mut out = text.to_string();
        for e in edits {
            out.replace_range(e.lo as usize..e.hi as usize, &e.text);
        }
        out
    }

    /// The actions offered with the cursor at `marker` (the first occurrence in `text`).
    fn actions_at(text: &str, marker: &str) -> Vec<Action> {
        let mut a = Analysis::run("t.neon", text, &MemVfs::new());
        let at = text.find(marker).unwrap_or_else(|| panic!("`{marker}` not in the text")) as u32;
        a.code_actions(at, at)
    }

    fn action<'a>(acts: &'a [Action], title_part: &str) -> &'a Action {
        acts.iter().find(|a| a.title.contains(title_part)).unwrap_or_else(|| {
            panic!("no action with `{title_part}` in {:?}", acts.iter().map(|a| &a.title).collect::<Vec<_>>())
        })
    }

    #[test]
    fn an_impl_without_generic_arguments_gets_them_and_its_methods() {
        let src = "struct Data { x: i64 }\nimpl Add for Data {\n    \n}\n";
        let acts = actions_at(src, "Add");
        let a = action(&acts, "Implement `Add<Data>`");
        assert_eq!(a.fixes, Some("E0207"));
        let fixed = apply(src, a);
        assert_eq!(
            fixed,
            "struct Data { x: i64 }\nimpl Add<Data> for Data {\n    fn add(self, rhs: Data) -> Data {\n        panic(\"not implemented\")\n    }\n}\n"
        );
        // and what it produces is a valid program
        let again = Analysis::run("t.neon", &fixed, &MemVfs::new());
        assert!(
            !again.diags.has_errors(),
            "{}",
            again.diags.iter().map(|d| d.message.clone()).collect::<Vec<_>>().join("; ")
        );
    }

    #[test]
    fn missing_trait_methods_are_added_all_at_once_or_one_by_one() {
        let src = "trait Shape { fn area(self) -> f64; fn name(self) -> string; fn describe(self) -> string { \"shape\" } }\n\
                   struct Sq { s: f64 }\nimpl Shape for Sq {}\n";
        let acts = actions_at(src, "impl Shape");
        let all = action(&acts, "Implement 2 missing methods");
        let fixed = apply(src, all);
        assert!(fixed.contains("fn area(self) -> f64 {\n        panic(\"not implemented\")\n    }"), "{fixed}");
        assert!(fixed.contains("fn name(self) -> string {"), "{fixed}");
        assert_eq!(fixed.matches("fn describe").count(), 1, "default methods are not required:\n{fixed}");
        assert!(!Analysis::run("t.neon", &fixed, &MemVfs::new()).diags.has_errors());
        let one = action(&acts, "missing method `name`");
        let fixed = apply(src, one);
        assert!(fixed.matches("fn name(self)").count() == 2 && fixed.matches("fn area").count() == 1, "{fixed}");
        // a partly implemented impl keeps its methods and gets the rest after them
        let src = "trait T { fn a(self) -> i64; fn b(self) -> i64; }\nstruct S;\nimpl T for S {\n    fn a(self) -> i64 { 1 }\n}\n";
        let acts = actions_at(src, "impl T");
        let fixed = apply(src, action(&acts, "missing method `b`"));
        assert_eq!(
            fixed,
            "trait T { fn a(self) -> i64; fn b(self) -> i64; }\nstruct S;\nimpl T for S {\n    fn a(self) -> i64 { 1 }\n\n    fn b(self) -> i64 {\n        panic(\"not implemented\")\n    }\n}\n"
        );
        // a complete impl offers nothing
        let done = "trait T { fn a(self) -> i64; }\nstruct S;\nimpl T for S { fn a(self) -> i64 { 1 } }\n";
        assert!(actions_at(done, "impl T").is_empty());
    }

    #[test]
    fn match_arms_for_every_uncovered_case() {
        let src = "enum Color { Red, Green, Blue(i64) }\nfn f(c: Color) -> i64 {\n    match c {\n        Color::Red => 1,\n    }\n}\n";
        let acts = actions_at(src, "match c");
        let a = action(&acts, "Add missing match arms");
        assert!(a.title.contains("Color::Green") && a.title.contains("Color::Blue(_)"), "{}", a.title);
        let fixed = apply(src, a);
        assert_eq!(
            fixed,
            "enum Color { Red, Green, Blue(i64) }\nfn f(c: Color) -> i64 {\n    match c {\n        Color::Red => 1,\n        Color::Green => panic(\"not implemented\"),\n        Color::Blue(_) => panic(\"not implemented\"),\n    }\n}\n"
        );
        assert!(!Analysis::run("t.neon", &fixed, &MemVfs::new()).diags.has_errors());
        let wild = apply(src, action(&acts, "wildcard"));
        assert!(wild.contains("        _ => panic(\"not implemented\"),\n    }"), "{wild}");
        // an arm without its comma gets one; an empty match gets its first arms
        let src = "enum E { A, B }\nfn f(e: E) -> i64 { match e { E::A => 1 } }\n";
        let fixed = apply(src, action(&actions_at(src, "match e"), "Add missing match arm"));
        assert!(fixed.contains("E::A => 1, E::B =>") || fixed.contains("E::A => 1,\n"), "{fixed}");
        let src = "enum E { A, B }\nfn f(e: E) -> i64 {\n    match e {\n    }\n}\n";
        let fixed = apply(src, action(&actions_at(src, "match e"), "Add missing match arms"));
        assert!(fixed.contains("        E::A => panic") && fixed.contains("        E::B => panic"), "{fixed}");
        // integers can only be completed by a wildcard
        let src = "fn f(n: i64) -> i64 {\n    match n {\n        0 => 1,\n    }\n}\n";
        let acts = actions_at(src, "match n");
        assert!(acts.iter().any(|a| a.title.contains("`_`")), "{acts:?}");
        assert_eq!(acts.iter().filter(|a| a.fixes == Some("E0240")).count(), 1);
    }

    #[test]
    fn an_unreachable_arm_can_be_removed() {
        let src = "enum C { R, G }\nfn g(c: C) -> i64 {\n    match c {\n        C::R => 1,\n        C::R => 2,\n        _ => 3,\n    }\n}\n";
        let acts = actions_at(src, "C::R => 2");
        let fixed = apply(src, action(&acts, "Remove the unreachable arm"));
        assert_eq!(
            fixed,
            "enum C { R, G }\nfn g(c: C) -> i64 {\n    match c {\n        C::R => 1,\n        _ => 3,\n    }\n}\n"
        );
    }

    #[test]
    fn struct_literals_get_their_missing_fields() {
        let src = "struct P { x: i64, name: string, ok: bool, v: [i64], r: f64 }\nlet a = P { };\nlet b = P { x: 1, ok: true };\n";
        let fixed = apply(src, action(&actions_at(src, "P { }"), "Add missing fields"));
        assert!(fixed.contains("let a = P { x: 0, name: \"\", ok: false, v: [], r: 0.0 };"), "{fixed}");
        let fixed = apply(src, action(&actions_at(src, "P { x: 1"), "Add missing fields"));
        assert!(fixed.contains("let b = P { x: 1, ok: true, name: \"\", v: [], r: 0.0 };"), "{fixed}");
        let src = "struct P { x: i64, y: i64 }\nlet a = P {\n    x: 1,\n};\n";
        let fixed = apply(src, action(&actions_at(src, "P {\n"), "Add missing field `y`"));
        assert_eq!(fixed, "struct P { x: i64, y: i64 }\nlet a = P {\n    x: 1,\n    y: 0,\n};\n");
        assert!(!Analysis::run("t.neon", &fixed, &MemVfs::new()).diags.has_errors());
    }

    #[test]
    fn let_becomes_var_when_the_variable_is_assigned() {
        let src = "let n = 1;\nn = 2;\nprintln(n);\n";
        let acts = actions_at(src, "n = 2");
        assert_eq!(apply(src, action(&acts, "Declare `n` with `var`")), "var n = 1;\nn = 2;\nprintln(n);\n");
        let src = "fn f(a: i64) -> i64 {\n    a = 2;\n    a\n}\n";
        let fixed = apply(src, action(&actions_at(src, "a = 2"), "with `var`"));
        assert_eq!(fixed, "fn f(var a: i64) -> i64 {\n    a = 2;\n    a\n}\n");
        assert!(!Analysis::run("t.neon", &fixed, &MemVfs::new()).diags.has_errors());
    }

    #[test]
    fn misspelled_names_fields_and_variants_are_corrected() {
        let src = "let count = 1;\nprintln(coutn);\n";
        assert_eq!(
            apply(src, action(&actions_at(src, "coutn"), "Change to `count`")),
            "let count = 1;\nprintln(count);\n"
        );
        let src = "enum Color { Red, Green }\nlet w = Color::Gren;\n";
        assert_eq!(
            apply(src, action(&actions_at(src, "Gren"), "Change to `Green`")),
            "enum Color { Red, Green }\nlet w = Color::Green;\n"
        );
        let src = "struct P { width: i64 }\nlet p = P { width: 1 };\nlet y = p.widht;\n";
        let fixed = apply(src, action(&actions_at(src, "widht"), "Change to `width`"));
        assert!(fixed.ends_with("let y = p.width;\n"), "{fixed}");
        let src = "struct Point { x: i64 }\nlet p = Pint { x: 1 };\n";
        assert!(apply(src, action(&actions_at(src, "Pint"), "Change to `Point`")).contains("Point { x: 1 }"));
        assert!(actions_at("println(qqqqqq);\n", "qqqqqq").is_empty());
    }

    #[test]
    fn a_missing_semicolon_is_inserted_after_the_previous_token() {
        let src = "let x = 1\nlet y = 2;\nprintln(x + y);\n";
        let acts = actions_at(src, "\nlet y");
        assert_eq!(apply(src, action(&acts, "Insert `;`")), "let x = 1;\nlet y = 2;\nprintln(x + y);\n");
    }

    #[test]
    fn a_string_with_placeholders_can_become_a_format_string() {
        let src = "let i = 3;\nprintln(\"{i}! = {i + 1}\");\n";
        let acts = actions_at(src, "{i}!");
        let a = action(&acts, "format string");
        assert_eq!(a.kind, "refactor.rewrite");
        assert_eq!(apply(src, a), "let i = 3;\nprintln(f\"{i}! = {i + 1}\");\n");
        // already an f-string, or no placeholder: nothing
        assert!(actions_at("let i = 3;\nprintln(f\"{i}\");\n", "{i}").iter().all(|a| a.kind != "refactor.rewrite"));
        assert!(actions_at("println(\"hello\");\n", "hello").is_empty());
    }

    #[test]
    fn identifiers_are_replaced_whole() {
        let map = vec![("Self".to_string(), "Data".to_string()), ("Rhs".to_string(), "i64".to_string())];
        assert_eq!(replace_words("fn add(self, rhs: Rhs) -> Self", &map), "fn add(self, rhs: i64) -> Data");
        assert_eq!(replace_words("fn f(x: SelfShaped, r: Rhs2)", &map), "fn f(x: SelfShaped, r: Rhs2)");
    }

    #[test]
    fn similar_names() {
        let c = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(similar("Gren", c(&["Red", "Green", "Blue"])), ["Green"]);
        assert_eq!(similar("colour", c(&["color", "count", "Color"])), ["Color", "color"]);
        assert!(similar("zzz", c(&["a", "b"])).is_empty());
        assert_eq!(distance("kitten", "sitting"), 3);
    }

    #[test]
    fn message_names_are_extracted() {
        assert_eq!(quoted("cannot find value `n_` in this scope", "cannot find").as_deref(), Some("n_"));
        assert_eq!(quoted("no associated item named `Gren` found for type `Color`", "named").as_deref(), Some("Gren"));
        assert_eq!(quoted("nothing here", "cannot find"), None);
    }

    #[test]
    fn placeholders_in_strings() {
        assert!(has_placeholder("\"{i}! = {f(i)}\""));
        assert!(has_placeholder("\"a {x:>8} b\""));
        assert!(!has_placeholder("\"{{literal}}\""));
        assert!(!has_placeholder("\"{}\""));
        assert!(!has_placeholder("\"plain\""));
        assert!(!has_placeholder("\"\\{x}\""));
    }

    #[test]
    fn text_positions() {
        let t = "a\n    bc\nd";
        assert_eq!(line_start(t, 7), 2);
        assert_eq!(indent_of(t, 7), "    ");
        assert_eq!(after_last_token("let x = 1  \n  ", 14), 9);
    }
}
