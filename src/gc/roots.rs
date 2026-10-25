//! Root enumeration: turns the VM's frames and the compiler's stack maps into
//! the set of references the collector must keep alive.

use super::heap::{Handle, Roots};
use crate::bytecode::BytecodeProgram;

/// One activation record, as the VM sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameInfo {
    /// Index of the executing chunk.
    pub chunk: u32,
    /// Word offset of the instruction the frame is stopped at: the instruction
    /// about to run for the innermost frame, the `call` for all others.
    pub pc: u32,
    /// Absolute stack index of the frame's register 0.
    pub base: usize,
    /// The closure being run (0 for plain functions); it must stay alive so its
    /// upvalue cells do.
    pub closure: Handle,
}

/// Adds every live reference held in the registers of `frames`.
///
/// # Panics
/// If a frame is not stopped at a safe-point, which means the compiler and the
/// VM disagree about where the collector may run.
pub fn scan_stack(prog: &BytecodeProgram, frames: &[FrameInfo], stack: &[u64], roots: &mut Roots) {
    for f in frames {
        roots.handle(f.closure);
        let chunk = &prog.chunks[f.chunk as usize];
        let map = chunk
            .stack_map_at(f.pc)
            .unwrap_or_else(|| panic!("no stack map for `{}` at pc {} (not a safe-point)", chunk.name, f.pc));
        for r in map.live_roots.iter() {
            roots.handle(stack[f.base + r as usize]);
        }
    }
}

/// Adds the references stored in global variables.
pub fn scan_globals(prog: &BytecodeProgram, globals: &[u64], roots: &mut Roots) {
    for (g, info) in prog.globals.iter().zip(globals) {
        if g.ty.is_ref() {
            roots.handle(*info);
        }
    }
}
