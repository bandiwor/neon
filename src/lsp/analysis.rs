//! One analyzed document: the program model plus the queries editors ask.

use super::json::Json;
use super::position::LineIndex;
use super::uri::path_to_uri;
use crate::diag::{DiagBag, FileId, Severity, SourceMap, Span};
use crate::sema::{self, DefId, DefKind, FnOwner, Generics, ModuleId, Program, Ty, TyKind};
use crate::syntax::{Interner, Keyword, NodeId, NodeKind};
use crate::vfs::Vfs;

pub mod complete;
mod describe;
mod symbols;
mod tokens;

pub use tokens::{TOKEN_MODIFIERS, TOKEN_TYPES};

/// The result of analyzing one document (as the root of its own import graph).
#[derive(Debug)]
pub struct Analysis {
    pub map: SourceMap,
    pub interner: Interner,
    pub prog: Program,
    pub diags: DiagBag,
    /// The analyzed document's module and file.
    pub module: ModuleId,
    pub file: FileId,
}

/// A source location as editors want it.
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub uri: String,
    pub range: Json,
}

impl Analysis {
    /// Analyzes `text`, registered under `path` so relative imports resolve from its directory.
    pub fn run(path: &str, text: &str, vfs: &dyn Vfs) -> Analysis {
        let mut map = SourceMap::new();
        let mut interner = Interner::new();
        let mut diags = DiagBag::new();
        let prog = sema::analyze_source(path, text, vfs, &mut map, &mut interner, &mut diags);
        let module = prog.modules.last().map(|m| m.id).unwrap_or(ModuleId(0));
        let file = prog.modules.last().map(|m| m.file).unwrap_or(FileId::DUMMY);
        Analysis { map, interner, prog, diags, module, file }
    }

    pub fn text(&self) -> &str {
        self.map.get(self.file).map(|f| f.text()).unwrap_or("")
    }

    /// Byte offset of an LSP position in the analyzed document.
    pub fn offset_at(&self, line: u32, character: u32) -> u32 {
        LineIndex::new(self.text()).offset(line, character) as u32
    }

    // ---- positions -------------------------------------------------------------------------------

    pub fn position(&self, file: FileId, offset: u32) -> Json {
        let Some(f) = self.map.get(file) else { return pos_json(0, 0) };
        let lc = f.line_col(offset);
        pos_json(lc.line, f.utf16_col(lc))
    }

    pub fn range(&self, span: Span) -> Json {
        Json::object(vec![("start", self.position(span.file, span.lo)), ("end", self.position(span.file, span.hi))])
    }

    /// The location of a span, unless it lies in a source that is not a real file (the prelude).
    pub fn location(&self, span: Span) -> Option<Location> {
        if span.is_dummy() {
            return None;
        }
        let name = self.map.get(span.file)?.name();
        if name.starts_with('<') {
            return None;
        }
        Some(Location { uri: path_to_uri(name), range: self.range(span) })
    }

    pub fn location_json(l: &Location) -> Json {
        Json::object(vec![("uri", Json::from(l.uri.clone())), ("range", l.range.clone())])
    }

    // ---- diagnostics ---------------------------------------------------------------------------------

    /// LSP diagnostics located in the analyzed document.
    pub fn diagnostics(&self) -> Vec<Json> {
        let mut out = Vec::new();
        for d in self.diags.iter() {
            let primary = d.labels.iter().find(|l| l.primary).or_else(|| d.labels.first());
            let span = primary.map(|l| l.span).filter(|s| !s.is_dummy());
            let (range, in_doc) = match span {
                Some(s) => (self.range(s), s.file == self.file),
                // problems with no location (a missing import, ...) go to the top of the document
                None => (Json::object(vec![("start", pos_json(0, 0)), ("end", pos_json(0, 0))]), true),
            };
            if !in_doc {
                continue;
            }
            let mut message = d.message.clone();
            for n in &d.notes {
                message.push_str(&format!("\nnote: {n}"));
            }
            for h in &d.helps {
                message.push_str(&format!("\nhelp: {h}"));
            }
            let mut fields = vec![
                ("range", range),
                ("severity", Json::from(if d.severity == Severity::Error { 1u32 } else { 2 })),
                ("source", "neon".into()),
                ("message", message.into()),
            ];
            if let Some(code) = d.code {
                fields.push(("code", code.into()));
            }
            let related: Vec<Json> = d
                .labels
                .iter()
                .filter(|l| !l.primary && !l.message.is_empty())
                .filter_map(|l| {
                    let loc = self.location(l.span)?;
                    Some(Json::object(vec![
                        ("location", Self::location_json(&loc)),
                        ("message", Json::from(l.message.clone())),
                    ]))
                })
                .collect();
            if !related.is_empty() {
                fields.push(("relatedInformation", Json::Arr(related)));
            }
            out.push(Json::object(fields));
        }
        out
    }

    // ---- symbols ---------------------------------------------------------------------------------------

    /// The definition denoted by the name at `offset` (a use or a declaration).
    pub fn def_at(&self, offset: u32) -> Option<DefId> {
        self.prog.symbols.def_at(self.file, offset)
    }

    /// The source span of the name at `offset` (the use or the declaration under the cursor).
    fn name_span_at(&self, offset: u32) -> Option<Span> {
        let table = &self.prog.symbols;
        table
            .uses
            .iter()
            .find(|u| u.span.file == self.file && u.span.contains(offset))
            .map(|u| u.span)
            .or_else(|| table.defs.iter().find(|d| d.span.file == self.file && d.span.contains(offset)).map(|d| d.span))
    }

    pub fn definition(&self, offset: u32) -> Vec<Location> {
        let Some(def) = self.def_at(offset) else { return Vec::new() };
        self.location(self.prog.symbols.def(def).span).into_iter().collect()
    }

    /// Every place `def` is mentioned in the analyzed program.
    fn reference_spans(&self, def: DefId, include_declaration: bool) -> Vec<Span> {
        let mut spans: Vec<Span> = Vec::new();
        if include_declaration {
            spans.push(self.prog.symbols.def(def).span);
        }
        spans.extend(self.prog.symbols.uses_of(def).map(|u| u.span));
        spans.retain(|s| !s.is_dummy());
        spans.sort_by_key(|s| (s.file.0, s.lo, s.hi));
        spans.dedup();
        spans
    }

    pub fn references(&self, offset: u32, include_declaration: bool) -> Vec<Location> {
        let Some(def) = self.def_at(offset) else { return Vec::new() };
        self.reference_spans(def, include_declaration).into_iter().filter_map(|s| self.location(s)).collect()
    }

    /// Highlights of the symbol under the cursor within the document.
    pub fn highlights(&self, offset: u32) -> Vec<Span> {
        let Some(def) = self.def_at(offset) else { return Vec::new() };
        self.reference_spans(def, true).into_iter().filter(|s| s.file == self.file).collect()
    }

    /// The name's range and current text, if it can be renamed.
    pub fn prepare_rename(&self, offset: u32) -> Result<(Span, String), String> {
        let def = self.def_at(offset).ok_or("there is no symbol here")?;
        let d = self.prog.symbols.def(def);
        self.check_renamable(def)?;
        let span = self.name_span_at(offset).unwrap_or(d.span);
        Ok((span, self.interner.resolve(d.name).to_string()))
    }

    fn check_renamable(&self, def: DefId) -> Result<(), String> {
        let d = self.prog.symbols.def(def);
        if d.span.is_dummy() || self.map.get(d.span.file).is_some_and(|f| f.name().starts_with('<')) {
            return Err("built-in names cannot be renamed".into());
        }
        match d.kind {
            DefKind::Module(_) | DefKind::ImportName => {
                Err("this name comes from an `import`; rename it at its definition".into())
            }
            _ => Ok(()),
        }
    }

    /// The edits renaming the symbol at `offset`, grouped as `(span, new text)`.
    pub fn rename(&self, offset: u32, new_name: &str) -> Result<Vec<Span>, String> {
        if !is_identifier(new_name) {
            return Err(format!("`{new_name}` is not a valid identifier"));
        }
        if Keyword::ALL.iter().any(|k| k.as_str() == new_name) {
            return Err(format!("`{new_name}` is a keyword"));
        }
        let def = self.def_at(offset).ok_or("there is no symbol here")?;
        self.check_renamable(def)?;
        let spans = self.reference_spans(def, true);
        if spans.iter().any(|s| self.map.get(s.file).is_some_and(|f| f.name().starts_with('<'))) {
            return Err("built-in names cannot be renamed".into());
        }
        Ok(spans)
    }

    // ---- hover -----------------------------------------------------------------------------------------

    /// Markdown for the hover at `offset`, and the range it applies to.
    pub fn hover(&mut self, offset: u32) -> Option<(String, Span)> {
        if let Some(def) = self.def_at(offset) {
            let span = self.name_span_at(offset)?;
            let text = self.describe_def(def)?;
            return Some((format!("```neon\n{text}\n```"), span));
        }
        let module = &self.prog.modules[self.module.index()];
        let ast = module.ast.clone();
        let node = ast.node_at(offset)?;
        if !is_expression(ast.kind(node)) {
            return None;
        }
        // a builtin method call: show the method rather than the whole call's type
        if let NodeKind::MethodCall { .. } = ast.kind(node) {
            if let Some(text) = self.describe_builtin_call(node) {
                return Some((format!("```neon\n{text}\n```"), ast.span(node)));
            }
        }
        let ty = *self.prog.modules[self.module.index()].info.node_types.get(node.0 as usize)?;
        let shown = self.show_ty_plain(ty)?;
        Some((format!("```neon\n{shown}\n```"), ast.span(node)))
    }

    /// Renders `ty`, or `None` when it is still unknown.
    pub fn show_ty_plain(&self, ty: Ty) -> Option<String> {
        self.show_ty_in(ty, &Generics::default())
    }

    pub fn show_ty_in(&self, ty: Ty, generics: &Generics) -> Option<String> {
        if matches!(self.prog.types.kind(ty), TyKind::Error | TyKind::Infer(_)) {
            return None;
        }
        Some(self.prog.show_ty(&self.interner, ty, generics))
    }

    /// The generics of the function whose source contains `offset`, for naming `T`s.
    pub(crate) fn generics_at(&self, offset: u32) -> Generics {
        let ast = &self.prog.modules[self.module.index()].ast;
        let mut best: Option<(u32, &Generics)> = None;
        for f in &self.prog.fns {
            if f.module != self.module || matches!(f.owner, FnOwner::ModuleInit) {
                continue;
            }
            let span = ast.span(f.node);
            if span.contains(offset) && best.is_none_or(|(l, _)| span.len() < l) {
                best = Some((span.len(), &f.generics));
            }
        }
        best.map(|(_, g)| g.clone()).unwrap_or_default()
    }
}

pub fn pos_json(line: u32, character: u32) -> Json {
    Json::object(vec![("line", Json::from(line)), ("character", Json::from(character))])
}

pub fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' => {}
        _ => return false,
    }
    s != "_" && chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// Nodes whose type is worth showing on hover.
fn is_expression(kind: &NodeKind) -> bool {
    use NodeKind::*;
    matches!(
        kind,
        IntLit(_)
            | FloatLit(_)
            | StrLit(_)
            | CharLit(_)
            | BoolLit(_)
            | Ident(_)
            | Path { .. }
            | Try { .. }
            | Unary { .. }
            | Binary { .. }
            | Cast { .. }
            | Call { .. }
            | MethodCall { .. }
            | Field { .. }
            | Index { .. }
            | ArrayLit { .. }
            | MapLit { .. }
            | SetLit { .. }
            | StructLit { .. }
            | Closure { .. }
            | If { .. }
            | Match { .. }
            | Block { .. }
    )
}

#[allow(dead_code)]
fn _assert_node_id_is_copy(n: NodeId) -> NodeId {
    n
}
