//! Loads the module graph through a [`Vfs`] and orders it topologically.
//!
//! Module 0 is always the embedded prelude. Ids are assigned in post-order of
//! a depth-first walk from the entry module, so every module's imports have
//! smaller ids than the module itself (the graph is a DAG; cycles are
//! reported as errors).
//!
//! Diagnostic codes: E0301 module not found, E0302 circular import,
//! E0303 import not at module level.

use std::collections::HashMap;
use std::sync::Arc;

use super::ids::ModuleId;
use crate::diag::{DiagBag, Diagnostic, FileId, SourceMap, Span};
use crate::syntax::{parse_source, Ast, Interner, NodeId, NodeKind};
use crate::vfs::{path, Vfs};

/// Name under which the prelude is registered in the `SourceMap`.
pub const PRELUDE_NAME: &str = "<prelude>";

/// File extension appended to import paths that have none.
pub const EXTENSION: &str = "neon";

#[derive(Debug)]
pub struct LoadedModule {
    pub id: ModuleId,
    /// Canonical path, or the display name for in-memory sources.
    pub path: String,
    pub file: FileId,
    pub ast: Arc<Ast>,
    /// The `Import*` node of each successfully resolved import, with its target.
    pub imports: Vec<(NodeId, ModuleId)>,
}

#[derive(Debug, Default)]
pub struct ModuleGraph {
    /// Dependencies first; `modules[0]` is the prelude and the last module is the entry.
    pub modules: Vec<LoadedModule>,
}

impl ModuleGraph {
    pub fn module(&self, id: ModuleId) -> &LoadedModule {
        &self.modules[id.index()]
    }

    pub fn entry(&self) -> Option<&LoadedModule> {
        self.modules.last()
    }
}

#[derive(Clone, Copy)]
enum State {
    Visiting,
    Done(ModuleId),
}

struct Loader<'a> {
    vfs: &'a dyn Vfs,
    map: &'a mut SourceMap,
    interner: &'a mut Interner,
    diags: &'a mut DiagBag,
    states: HashMap<String, State>,
    stack: Vec<String>,
    graph: ModuleGraph,
}

/// Loads `entry` (a path in `vfs`) and everything it imports.
pub fn load_program(
    entry: &str,
    vfs: &dyn Vfs,
    map: &mut SourceMap,
    interner: &mut Interner,
    diags: &mut DiagBag,
    prelude_src: &str,
) -> ModuleGraph {
    let mut l = Loader::new(vfs, map, interner, diags, prelude_src);
    let canon = vfs.canonicalize(entry).unwrap_or_else(|_| path::normalize(entry));
    match l.map.load(vfs, &canon) {
        Ok(file) => {
            l.visit(canon, file);
        }
        Err(e) => {
            l.diags.report(
                Diagnostic::error(format!("cannot read `{entry}`")).with_code("E0301").with_note(e.to_string()),
            );
        }
    }
    l.graph
}

/// Like [`load_program`] but the entry module's text is supplied directly
/// (REPL input, tests, editors with unsaved buffers). Its relative imports
/// resolve against `name`.
pub fn load_program_from_source(
    name: &str,
    text: &str,
    vfs: &dyn Vfs,
    map: &mut SourceMap,
    interner: &mut Interner,
    diags: &mut DiagBag,
    prelude_src: &str,
) -> ModuleGraph {
    let mut l = Loader::new(vfs, map, interner, diags, prelude_src);
    match l.map.add_file(name, text) {
        Ok(file) => {
            l.visit(name.to_string(), file);
        }
        Err(e) => {
            l.diags.report(Diagnostic::error(e.to_string()).with_code("E0301"));
        }
    }
    l.graph
}

impl<'a> Loader<'a> {
    fn new(
        vfs: &'a dyn Vfs,
        map: &'a mut SourceMap,
        interner: &'a mut Interner,
        diags: &'a mut DiagBag,
        prelude_src: &str,
    ) -> Loader<'a> {
        let mut l = Loader {
            vfs,
            map,
            interner,
            diags,
            states: HashMap::new(),
            stack: Vec::new(),
            graph: ModuleGraph::default(),
        };
        let file = l.map.add_file(PRELUDE_NAME, prelude_src).expect("prelude fits in memory");
        let ast = Arc::new(parse_source(file, prelude_src, l.interner, l.diags));
        l.graph.modules.push(LoadedModule {
            id: ModuleId(0),
            path: PRELUDE_NAME.to_string(),
            file,
            ast,
            imports: Vec::new(),
        });
        l.states.insert(PRELUDE_NAME.to_string(), State::Done(ModuleId(0)));
        l
    }

    /// Parses `file`, loads its imports, then registers it. Returns its id.
    fn visit(&mut self, canon: String, file: FileId) -> ModuleId {
        self.states.insert(canon.clone(), State::Visiting);
        self.stack.push(canon.clone());
        let text = self.map.file(file).text().to_string();
        let ast = Arc::new(parse_source(file, &text, self.interner, self.diags));

        let mut imports = Vec::new();
        let NodeKind::Module { items } = *ast.kind(ast.root()) else { unreachable!("root is a module") };
        for &item in ast.list(items) {
            let spec = match *ast.kind(item) {
                NodeKind::ImportNamed { path, .. } | NodeKind::ImportAlias { path, .. } => {
                    ast.lits.str(path).to_string()
                }
                _ => continue,
            };
            let span = ast.span(item);
            if let Some(target) = self.resolve_import(&canon, &spec, span) {
                imports.push((item, target));
            }
        }

        self.stack.pop();
        let id = ModuleId(self.graph.modules.len() as u32);
        self.graph.modules.push(LoadedModule { id, path: canon.clone(), file, ast, imports });
        self.states.insert(canon, State::Done(id));
        id
    }

    fn resolve_import(&mut self, importer: &str, spec: &str, span: Span) -> Option<ModuleId> {
        let joined = self.vfs.resolve(importer, spec);
        let with_ext = path::with_default_extension(&joined, EXTENSION);
        if !self.vfs.is_file(&with_ext) {
            self.diags.report(
                Diagnostic::error(format!("cannot find module `{spec}`"))
                    .with_code("E0301")
                    .with_label(span, format!("no file `{with_ext}`")),
            );
            return None;
        }
        let canon = self.vfs.canonicalize(&with_ext).unwrap_or(with_ext);
        match self.states.get(&canon).copied() {
            Some(State::Done(id)) => Some(id),
            Some(State::Visiting) => {
                let start = self.stack.iter().position(|p| *p == canon).unwrap_or(0);
                let mut chain: Vec<&str> = self.stack[start..].iter().map(String::as_str).collect();
                chain.push(&canon);
                self.diags.report(
                    Diagnostic::error(format!("circular import of `{spec}`"))
                        .with_code("E0302")
                        .with_label(span, "this import closes a cycle")
                        .with_note(format!("import chain: {}", chain.join(" -> "))),
                );
                None
            }
            None => match self.map.load(self.vfs, &canon) {
                Ok(file) => Some(self.visit(canon, file)),
                Err(e) => {
                    self.diags.report(
                        Diagnostic::error(format!("cannot read module `{spec}`"))
                            .with_code("E0301")
                            .with_label(span, e.to_string()),
                    );
                    None
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::MemVfs;

    fn load(vfs: &MemVfs, entry: &str) -> (ModuleGraph, DiagBag, SourceMap) {
        let mut map = SourceMap::new();
        let mut interner = Interner::new();
        let mut diags = DiagBag::new();
        let g = load_program(entry, vfs, &mut map, &mut interner, &mut diags, "");
        (g, diags, map)
    }

    fn names(g: &ModuleGraph) -> Vec<&str> {
        g.modules.iter().map(|m| m.path.as_str()).collect()
    }

    #[test]
    fn single_module_has_prelude_first() {
        let vfs = MemVfs::new().with_file("main.neon", "let x = 1;");
        let (g, diags, _) = load(&vfs, "main.neon");
        assert!(diags.is_empty());
        assert_eq!(names(&g), ["<prelude>", "main.neon"]);
        assert_eq!(g.entry().unwrap().id, ModuleId(1));
    }

    #[test]
    fn dependencies_come_first() {
        let vfs = MemVfs::new()
            .with_file("main.neon", "import { a } from \"a\";\nimport { b } from \"lib/b\";")
            .with_file("a.neon", "import { c } from \"lib/c\";\nexport fn a() {}")
            .with_file("lib/b.neon", "import { c } from \"c\";\nexport fn b() {}")
            .with_file("lib/c.neon", "export fn c() {}");
        let (g, diags, _) = load(&vfs, "main.neon");
        assert!(diags.is_empty(), "{:?}", diags.as_slice());
        // `lib/c` is shared by `a` and `lib/b` (a diamond) and loaded once.
        assert_eq!(names(&g), ["<prelude>", "lib/c.neon", "a.neon", "lib/b.neon", "main.neon"]);
        for m in &g.modules {
            for &(_, dep) in &m.imports {
                assert!(dep < m.id, "{} imports a later module", m.path);
            }
        }
        let main = g.entry().unwrap();
        assert_eq!(main.imports.len(), 2);
    }

    #[test]
    fn import_kinds_are_both_followed() {
        let vfs =
            MemVfs::new().with_file("main.neon", "import \"util\" as u;").with_file("util.neon", "export fn f() {}");
        let (g, diags, _) = load(&vfs, "main.neon");
        assert!(diags.is_empty());
        assert_eq!(g.modules.len(), 3);
    }

    #[test]
    fn missing_module_is_reported_at_the_import() {
        let vfs = MemVfs::new().with_file("main.neon", "import { x } from \"nope\";\nlet y = 1;");
        let (g, diags, map) = load(&vfs, "main.neon");
        assert_eq!(diags.error_count(), 1);
        assert_eq!(diags.as_slice()[0].code, Some("E0301"));
        assert!(diags.as_slice()[0].message.contains("cannot find module `nope`"));
        let r = map.resolve(diags.as_slice()[0].primary_span().unwrap()).unwrap();
        assert_eq!(r.start.line, 0);
        // the rest of the program still loads
        assert_eq!(g.modules.len(), 2);
        assert!(g.entry().unwrap().imports.is_empty());
    }

    #[test]
    fn cycles_are_reported_with_the_chain() {
        let vfs = MemVfs::new()
            .with_file("a.neon", "import { b } from \"b\";")
            .with_file("b.neon", "import { c } from \"c\";")
            .with_file("c.neon", "import { a } from \"a\";");
        let (g, diags, _) = load(&vfs, "a.neon");
        assert_eq!(diags.error_count(), 1);
        let d = &diags.as_slice()[0];
        assert_eq!(d.code, Some("E0302"));
        assert!(d.notes[0].contains("a.neon -> b.neon -> c.neon -> a.neon"), "{:?}", d.notes);
        assert_eq!(g.modules.len(), 4); // prelude + a + b + c, each once
    }

    #[test]
    fn self_import_is_a_cycle() {
        let vfs = MemVfs::new().with_file("a.neon", "import { x } from \"a\";");
        let (_, diags, _) = load(&vfs, "a.neon");
        assert_eq!(diags.as_slice()[0].code, Some("E0302"));
    }

    #[test]
    fn missing_entry_is_an_error() {
        let vfs = MemVfs::new();
        let (g, diags, _) = load(&vfs, "main.neon");
        assert_eq!(diags.error_count(), 1);
        assert_eq!(g.modules.len(), 1); // just the prelude
        assert!(g.entry().unwrap().path == PRELUDE_NAME);
    }

    #[test]
    fn extension_is_optional_and_paths_are_relative_to_the_importer() {
        let vfs = MemVfs::new()
            .with_file("app/main.neon", "import { x } from \"../lib/x.neon\";")
            .with_file("lib/x.neon", "export const x = 1;");
        let (g, diags, _) = load(&vfs, "app/main.neon");
        assert!(diags.is_empty(), "{:?}", diags.as_slice());
        assert_eq!(names(&g), ["<prelude>", "lib/x.neon", "app/main.neon"]);
    }

    #[test]
    fn source_text_entry() {
        let vfs = MemVfs::new().with_file("lib.neon", "export fn f() {}");
        let mut map = SourceMap::new();
        let mut interner = Interner::new();
        let mut diags = DiagBag::new();
        let g = load_program_from_source(
            "<repl>",
            "import { f } from \"lib\";",
            &vfs,
            &mut map,
            &mut interner,
            &mut diags,
            "",
        );
        assert!(diags.is_empty(), "{:?}", diags.as_slice());
        assert_eq!(names(&g), ["<prelude>", "lib.neon", "<repl>"]);
    }

    #[test]
    fn syntax_errors_in_dependencies_are_collected() {
        let vfs =
            MemVfs::new().with_file("main.neon", "import { f } from \"dep\";").with_file("dep.neon", "export fn f( {");
        let (g, diags, _) = load(&vfs, "main.neon");
        assert!(diags.has_errors());
        assert_eq!(g.modules.len(), 3);
    }
}
