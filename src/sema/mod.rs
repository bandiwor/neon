//! Semantic analysis: module loading, name resolution, type checking,
//! trait solving and exhaustiveness checking.
//!
//! [`analyze`] loads a program through a [`Vfs`](crate::vfs::Vfs), then for
//! each module in dependency order declares its items, resolves signatures,
//! checks its top-level code and finally every function body. The result is
//! a [`Program`]: definitions, per-node types and resolutions, and a
//! definition/reference table that maps source spans to symbols.

mod builtins;
mod check;
mod collect;
mod exhaust;
pub use exhaust::missing_patterns;
mod ids;
mod infer;
mod loader;
mod program;
mod traits;
mod typeres;
mod types;

pub use builtins::{builtin_const, builtin_fn, builtin_method, BuiltinFn, BuiltinSig, Require};
pub use ids::*;
pub use infer::{InferCtx, VarKind};
pub use loader::{load_program, load_program_from_source, LoadedModule, ModuleGraph, EXTENSION, PRELUDE_NAME};
pub use program::*;
pub use traits::{resolve_trait_method, solve_bound, ImplSource, Resolved, Solved};
pub use types::{InferVar, Prim, Ty, TyKind, TypeTable};

use crate::diag::{DiagBag, SourceMap};
use crate::syntax::Interner;
use crate::vfs::Vfs;

/// The embedded prelude source.
pub const PRELUDE_SRC: &str = include_str!("prelude.neon");

/// Analyzes the program rooted at `entry` (a path in `vfs`).
pub fn analyze(
    entry: &str,
    vfs: &dyn Vfs,
    map: &mut SourceMap,
    interner: &mut Interner,
    diags: &mut DiagBag,
) -> Program {
    let graph = load_program(entry, vfs, map, interner, diags, PRELUDE_SRC);
    analyze_graph(graph, interner, diags)
}

/// Analyzes a program whose entry module is given as text.
pub fn analyze_source(
    name: &str,
    text: &str,
    vfs: &dyn Vfs,
    map: &mut SourceMap,
    interner: &mut Interner,
    diags: &mut DiagBag,
) -> Program {
    let graph = load_program_from_source(name, text, vfs, map, interner, diags, PRELUDE_SRC);
    analyze_graph(graph, interner, diags)
}

/// Runs declaration, signature and body checking over an already loaded graph.
pub fn analyze_graph(graph: ModuleGraph, interner: &mut Interner, diags: &mut DiagBag) -> Program {
    let mut prog = Program::default();
    let mut c_files: Vec<String> = Vec::new();
    for lm in &graph.modules {
        let module = collect::declare_module(&mut prog, interner, diags, lm);
        debug_assert_eq!(module, lm.id);
        for c in &lm.c_files {
            if !c_files.contains(c) {
                c_files.push(c.clone());
            }
        }
        collect::resolve_signatures(&mut prog, interner, diags, module);
        check::check_module_init(&mut prog, interner, diags, module);
        let fns: Vec<FnId> = (0..prog.fns.len())
            .map(|i| FnId(i as u32))
            .filter(|&f| {
                let d = &prog.fns[f.index()];
                d.module == module && d.owner != FnOwner::ModuleInit && d.body.is_some()
            })
            .collect();
        for f in fns {
            check::check_fn_body(&mut prog, interner, diags, f);
        }
    }
    // the C files as `#include` paths: relative to the entry module's directory where they are below it
    let entry_dir = graph.entry().map(|m| m.path.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default());
    prog.c_sources = c_files
        .into_iter()
        .map(|c| match entry_dir.as_deref() {
            Some(d) if !d.is_empty() => c.strip_prefix(&format!("{d}/")).map(str::to_string).unwrap_or(c),
            _ => c,
        })
        .collect();
    prog
}

#[cfg(test)]
mod tests;
