//! The compilation pipeline, from source text to the representation each
//! phase produces.

use crate::diag::{DiagBag, Emitter, SourceMap};
use crate::ir::{self, IrProgram};
use crate::sema;
use crate::syntax::Interner;
use crate::vfs::Vfs;

/// Everything a compilation produces.
#[derive(Debug)]
pub struct Compiled {
    pub map: SourceMap,
    pub interner: Interner,
    pub diags: DiagBag,
    /// `None` if any error was reported.
    pub ir: Option<IrProgram>,
}

impl Compiled {
    /// All diagnostics rendered as ASCII text.
    pub fn render_diagnostics(&self) -> String {
        Emitter::new(&self.map).render_all(&self.diags)
    }
}

/// Compiles the program rooted at `entry` (a path in `vfs`) to IR.
pub fn compile_ir(entry: &str, vfs: &dyn Vfs) -> Compiled {
    let mut map = SourceMap::new();
    let mut interner = Interner::new();
    let mut diags = DiagBag::new();
    let mut prog = sema::analyze(entry, vfs, &mut map, &mut interner, &mut diags);
    let ir = lower(&mut prog, &interner, &mut diags);
    Compiled { map, interner, diags, ir }
}

/// Compiles a program whose entry module is given as text.
pub fn compile_ir_source(name: &str, text: &str, vfs: &dyn Vfs) -> Compiled {
    let mut map = SourceMap::new();
    let mut interner = Interner::new();
    let mut diags = DiagBag::new();
    let mut prog = sema::analyze_source(name, text, vfs, &mut map, &mut interner, &mut diags);
    let ir = lower(&mut prog, &interner, &mut diags);
    Compiled { map, interner, diags, ir }
}

fn lower(prog: &mut sema::Program, interner: &Interner, diags: &mut DiagBag) -> Option<IrProgram> {
    if diags.has_errors() {
        return None;
    }
    let ir = ir::lower_program(prog, interner, diags)?;
    debug_assert!(ir::verify_program(&ir).is_ok(), "lowering produced invalid IR: {:?}", ir::verify_program(&ir));
    Some(ir)
}
