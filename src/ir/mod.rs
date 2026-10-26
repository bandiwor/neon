//! Linear intermediate representation: three-address code in basic blocks.
//!
//! * [`types`] — the IR itself
//! * [`lower`] — typed AST to IR, with monomorphization
//! * [`effects`] — what code can do (const / pure / impure), for the optimizer
//! * [`print`], [`verify`] — textual form and well-formedness checks

pub mod effects;
pub mod escape;
pub mod interp;
pub mod lower;
pub mod opt;
pub mod print;
pub mod types;
pub mod verify;

pub use effects::{analyze as analyze_effects, FuncEffects};
pub use lower::lower_program;
pub use opt::{optimize, OptConfig, OptStats};
pub use types::*;
pub use verify::{verify_func, verify_program};

#[cfg(test)]
mod tests;
