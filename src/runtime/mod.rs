//! Semantics shared by every execution engine (constant folder, IR
//! interpreter, VM): typed arithmetic, casts, traps and value formatting.
//!
//! Keeping these in one place is what guarantees that folding `127i8 + 1` at
//! compile time and executing it at run time agree.

pub mod fmtspec;
pub mod format;
pub mod mathfn;
pub mod numeric;
pub mod trap;
pub mod value;

pub use numeric::{ArithOp, CmpOp, NumTy};
pub use trap::Trap;
pub use value::HeapView;
