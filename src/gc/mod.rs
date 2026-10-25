//! Garbage collection support.
//!
//! This module currently provides the stack-map types the compiler emits;
//! the heap and collector are added in a later phase.

pub mod stackmap;

pub use stackmap::{BitSet, StackMap};
