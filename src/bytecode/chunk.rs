//! Compiled functions and whole compiled programs.

use crate::gc::StackMap;
use crate::ir::{GlobalInfo, TypeDesc};

/// One compiled function.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub name: String,
    pub code: Vec<u32>,
    /// Number of parameters; they arrive in registers `0..params`.
    pub params: u8,
    /// Registers the function uses (at most 256).
    pub frame_size: u16,
    /// For each parameter: is it a heap reference? (The collector roots these
    /// when a call has just transferred control and no stack map applies yet.)
    pub param_refs: Vec<bool>,
    /// For each upvalue of a closure: does it hold a heap reference?
    pub upvals: Vec<bool>,
    /// Sorted by `pc`; one entry per safe-point instruction.
    pub stack_maps: Vec<StackMap>,
}

impl Chunk {
    /// The stack map of the instruction at word offset `pc`.
    pub fn stack_map_at(&self, pc: u32) -> Option<&StackMap> {
        self.stack_maps.binary_search_by_key(&pc, |m| m.pc).ok().map(|i| &self.stack_maps[i])
    }
}

/// A whole program ready to run.
#[derive(Debug, Clone, Default)]
pub struct BytecodeProgram {
    pub chunks: Vec<Chunk>,
    /// 64-bit constants referenced by `loadk`.
    pub constants: Vec<u64>,
    /// String literals referenced by `loadstr`.
    pub strings: Vec<String>,
    pub descs: Vec<TypeDesc>,
    pub globals: Vec<GlobalInfo>,
    /// Index of the chunk that starts the program.
    pub entry: u32,
}
