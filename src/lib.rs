//! Neon: a statically typed, register-bytecode language implemented in pure Rust.
//!
//! The engine (everything in this library) never touches the host OS directly:
//! files go through [`vfs::Vfs`] and VM I/O goes through pluggable reader/writer
//! interfaces, so the whole crate builds for `wasm32-unknown-unknown`.

#![warn(missing_debug_implementations, rust_2018_idioms)]
#![allow(clippy::too_many_arguments)]

pub mod builtin;
pub mod bytecode;
pub mod cli;
pub mod compiler;
pub mod diag;
pub mod driver;
pub mod gc;
pub mod ir;
pub mod repl;
pub mod runtime;
pub mod sema;
pub mod syntax;
pub mod vfs;
pub mod vm;
