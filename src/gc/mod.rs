//! Garbage collection: the object heap, the mark-and-sweep collector and the
//! stack-map machinery that tells it which registers hold references.

pub mod heap;
pub mod roots;
pub mod stackmap;

pub use heap::{CellId, CellState, GcConfig, Handle, Heap, HeapStats, KeyKind, Obj, Roots};
pub use roots::{scan_globals, scan_stack, FrameInfo};
pub use stackmap::{BitSet, StackMap};

#[cfg(test)]
mod tests;
