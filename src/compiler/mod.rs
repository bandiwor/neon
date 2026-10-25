//! IR → bytecode: block layout, register allocation, instruction selection,
//! peephole optimization, stack maps and assembly.
//!
//! Calling convention ("sliding window"): a call places its arguments in
//! consecutive registers starting at `base`, above every register that is live
//! across the call. The callee's frame *is* the caller's registers from
//! `base` upward, and the result comes back in `base`.

mod func;
mod regalloc;
mod stackmap;

use std::fmt;

use crate::bytecode::peephole::peephole;
use crate::bytecode::{assemble, BytecodeProgram, Chunk, Insn};
use crate::gc::BitSet;
use crate::ir::{IrFunc, IrProgram};

pub use func::cmp_kind;
pub use regalloc::{allocate, block_layout, Allocation, VSet};

/// Knobs for [`compile_program`].
#[derive(Debug, Clone, Copy)]
pub struct CompileOptions {
    /// Run the bytecode peephole optimizer.
    pub peephole: bool,
}

impl Default for CompileOptions {
    fn default() -> Self {
        CompileOptions { peephole: true }
    }
}

/// A function that cannot be expressed in the bytecode's limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    pub func: String,
    pub message: String,
}

impl CompileError {
    pub fn new(func: &str, message: &str) -> CompileError {
        CompileError { func: func.to_string(), message: message.to_string() }
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot compile `{}`: {}", self.func, self.message)
    }
}

impl std::error::Error for CompileError {}

/// Compiles every function of `ir`. Chunk `i` is the translation of function `i`.
pub fn compile_program(ir: &IrProgram, opts: &CompileOptions) -> Result<BytecodeProgram, CompileError> {
    if ir.descs.len() >= 0x7FFF {
        return Err(CompileError::new("<program>", "too many type descriptors"));
    }
    let mut consts = func::ConstPool::default();
    let mut chunks = Vec::with_capacity(ir.funcs.len());
    for f in &ir.funcs {
        chunks.push(compile_function(f, &mut consts, opts)?);
    }
    Ok(BytecodeProgram {
        chunks,
        constants: consts.values,
        strings: ir.strings.clone(),
        descs: ir.descs.clone(),
        globals: ir.globals.clone(),
        entry: ir.entry.0,
    })
}

fn compile_function(f: &IrFunc, consts: &mut func::ConstPool, opts: &CompileOptions) -> Result<Chunk, CompileError> {
    let err = |m: &str| CompileError::new(&f.name, m);
    if f.params > 250 {
        return Err(err("more than 250 parameters"));
    }
    if f.upvals.len() > 255 {
        return Err(err("more than 255 captured variables"));
    }
    let alloc = allocate(f).map_err(|_| err("needs more than 256 registers"))?;

    let mut em = func::FuncEmitter::new(f, &alloc, consts);
    em.emit_function()?;
    let mut items = em.items;

    // registers captured by closures must keep their stores and stay rooted
    let mut pinned = BitSet::new();
    for (v, &p) in f.pinned.iter().enumerate() {
        if p && alloc.slots[v] != regalloc::NO_SLOT {
            pinned.insert(alloc.slots[v] as u8);
        }
    }
    if opts.peephole {
        peephole(&mut items, &pinned);
    }

    let param_refs: Vec<bool> = (0..f.params as usize).map(|i| f.regs[i].is_ref()).collect();
    let maps = stackmap::compute(&items, &pinned, &param_refs);
    let insns: Vec<Insn> = items.iter().map(|i| i.insn.clone()).collect();
    let asm = assemble(&insns).map_err(|e| err(&format!("assembly failed: {e}")))?;
    let stack_maps =
        maps.into_iter().map(|(i, roots)| crate::gc::StackMap { pc: asm.pcs[i], live_roots: roots }).collect();

    let frame_size = frame_size(&items, f.params);
    Ok(Chunk {
        name: f.name.clone(),
        code: asm.code,
        params: f.params as u8,
        frame_size,
        upvals: f.upvals.iter().map(|t| t.is_ref()).collect(),
        param_refs,
        stack_maps,
    })
}

/// One more than the highest register any instruction touches.
fn frame_size(items: &[crate::bytecode::Item], params: u32) -> u16 {
    let mut top = params.max(1) as u16;
    for it in items {
        it.insn.for_each_use(|r| top = top.max(r as u16 + 1));
        if let Some(d) = it.insn.def() {
            top = top.max(d as u16 + 1);
        }
        if let Insn::MakeClosure { captures, .. } = &it.insn {
            for c in captures {
                if let crate::bytecode::Capture::Local(r) = c {
                    top = top.max(*r as u16 + 1);
                }
            }
        }
        match &it.insn {
            Insn::SetUpval { src, .. } | Insn::SetGlobal { src, .. } => top = top.max(*src as u16 + 1),
            Insn::Close { reg } => top = top.max(*reg as u16 + 1),
            _ => {}
        }
    }
    top
}
