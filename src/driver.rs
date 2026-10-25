//! The compilation pipeline, from source text to the representation each
//! phase produces.

use crate::bytecode::BytecodeProgram;
use crate::compiler::{self, CompileOptions};
use crate::diag::Span;
use crate::diag::{DiagBag, Emitter, SourceMap};
use crate::ir::{self, IrProgram, OptConfig};
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
    /// Set by [`compile`] / [`compile_source`] when everything succeeded.
    pub bytecode: Option<BytecodeProgram>,
}

/// Settings for the full pipeline.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub opt: OptConfig,
    pub compile: CompileOptions,
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
    Compiled { map, interner, diags, ir, bytecode: None }
}

/// Compiles a program whose entry module is given as text.
pub fn compile_ir_source(name: &str, text: &str, vfs: &dyn Vfs) -> Compiled {
    let mut map = SourceMap::new();
    let mut interner = Interner::new();
    let mut diags = DiagBag::new();
    let mut prog = sema::analyze_source(name, text, vfs, &mut map, &mut interner, &mut diags);
    let ir = lower(&mut prog, &interner, &mut diags);
    Compiled { map, interner, diags, ir, bytecode: None }
}

fn lower(prog: &mut sema::Program, interner: &Interner, diags: &mut DiagBag) -> Option<IrProgram> {
    if diags.has_errors() {
        return None;
    }
    let ir = ir::lower_program(prog, interner, diags)?;
    debug_assert!(ir::verify_program(&ir).is_ok(), "lowering produced invalid IR: {:?}", ir::verify_program(&ir));
    Some(ir)
}

/// Source text to bytecode: analysis, lowering, IR optimization, code generation.
pub fn compile(entry: &str, vfs: &dyn Vfs, opts: &Options) -> Compiled {
    let c = compile_ir(entry, vfs);
    finish(c, opts)
}

/// Like [`compile`], for a program whose entry module is given as text.
pub fn compile_source(name: &str, text: &str, vfs: &dyn Vfs, opts: &Options) -> Compiled {
    let c = compile_ir_source(name, text, vfs);
    finish(c, opts)
}

fn finish(mut c: Compiled, opts: &Options) -> Compiled {
    let Some(ir) = c.ir.as_mut() else { return c };
    ir::optimize(ir, &opts.opt);
    debug_assert!(ir::verify_program(ir).is_ok(), "optimizer produced invalid IR: {:?}", ir::verify_program(ir));
    match compiler::compile_program(ir, &opts.compile) {
        Ok(bc) => {
            debug_assert!(
                crate::bytecode::verify_program(&bc).is_ok(),
                "compiler produced invalid bytecode: {:?}",
                crate::bytecode::verify_program(&bc)
            );
            c.bytecode = Some(bc);
        }
        Err(e) => c.diags.error(Span::DUMMY, e.to_string()),
    }
    c
}
